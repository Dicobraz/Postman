//! Плагин EWS (`postman_backend_ews`) против подменного сервера EWS: HTTP без TLS (сборка `postman-ews` с `testkit`),
//! ответы — корпус `crates/ews/tests/fixtures` (по примерам Microsoft, обезличен). Проверяет синхронизацию почты тем
//! же договором команд и событий без боевого Exchange: вход, роли папок, запасное дерево `FindFolder` (сервер падает на
//! `SyncFolderHierarchy`, как боевой 2013), первые письма папки, «прочитано», отправка из MIME, справочник, области.
//! Боевой Exchange — `stand_exchange.rs` (`#[ignore]`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // тест: «не падать» — для кода приложения (S2-03)

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use postman_cache::Cache;
use postman_core::backend::{MailPlugin, Spawn};
use postman_core::model::{Auth, Feature};
use postman_core::sync::{Account, Command, Event, Status, UnboundedSender};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ews/tests/fixtures/");

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap()
}

/// `SyncFolderHierarchy` боевого Exchange 2013: внутренняя ошибка сервера.
const HIERARCHY_FAILS: &str = r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:SyncFolderHierarchyResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"><m:ResponseMessages><m:SyncFolderHierarchyResponseMessage ResponseClass="Error"><m:MessageText>internal</m:MessageText><m:ResponseCode>ErrorInternalServerError</m:ResponseCode></m:SyncFolderHierarchyResponseMessage></m:ResponseMessages></m:SyncFolderHierarchyResponse></s:Body></s:Envelope>"#;

/// `FindItem` пустой папки. Готовым ответом, а не правкой текста корпуса: на CI файлы корпуса — с CRLF
/// (`core.autocrlf`), и замена многострочного куска молча не срабатывала.
const EMPTY_PAGE: &str = r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><m:FindItemResponse xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages" xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types"><m:ResponseMessages><m:FindItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:RootFolder IndexedPagingOffset="0" TotalItemsInView="0" IncludesLastItemInRange="true"><t:Items/></m:RootFolder></m:FindItemResponseMessage></m:ResponseMessages></m:FindItemResponse></s:Body></s:Envelope>"#;

/// Ответ подменного сервера по телу запроса.
fn answer(body: &str) -> String {
    let op = |tag: &str| body.contains(&format!("<m:{tag}"));
    if op("GetFolder") && body.contains(r#"Id="msgfolderroot""#) {
        fixture("get_folder_msgroot.xml")
    } else if op("GetFolder") && body.contains(r#"Id="inbox""#) {
        fixture("get_folder_mail_roles.xml")
    } else if op("GetFolder") {
        fixture("get_folder_calendar.xml")
    } else if op("SyncFolderHierarchy") {
        HIERARCHY_FAILS.to_owned()
    } else if op("FindFolder") {
        fixture("find_folder_mail.xml")
    } else if op("SyncFolderItems") {
        fixture("sync_mail_items.xml")
    } else if op("FindItem") && body.contains("CalendarView") {
        fixture("find_item_calendar_view.xml")
    } else if op("FindItem") && body.contains("AAMkADinbox") {
        fixture("find_item_mail_page.xml")
    } else if op("FindItem") {
        // Другие папки — пустые: в корпусе одни и те же письма, и строки одной папки иначе перезапишут другую.
        EMPTY_PAGE.to_owned()
    } else if op("GetItem") && body.contains("IncludeMimeContent") {
        fixture("get_item_message_mime.xml")
    } else if op("GetItem") && body.contains("calendar:") {
        fixture("get_item_meeting.xml")
    } else if op("GetItem") {
        fixture("get_item_headers.xml")
    } else if op("UpdateItem") {
        fixture("update_item_read.xml")
    } else if op("CreateItem") {
        fixture("create_item_send.xml")
    } else if op("ResolveNames") {
        fixture("resolve_names.xml")
    } else if op("GetUserAvailabilityRequest") {
        fixture("get_user_availability.xml")
    } else {
        fixture("fault_schema.xml")
    }
}

async fn serve(mut s: TcpStream, log: Arc<Mutex<Vec<String>>>) {
    loop {
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if s.read(&mut b).await.unwrap_or(0) == 0 {
                return;
            }
            head.push(b[0]);
        }
        let head = String::from_utf8_lossy(&head).to_string();
        let len: usize =
            head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).await.unwrap();
        let body = String::from_utf8(body).unwrap();
        let reply = answer(&body);
        log.lock().unwrap().push(body);
        let out = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\n\r\n{reply}", reply.len());
        s.write_all(out.as_bytes()).await.unwrap();
    }
}

/// Сервер в своём потоке (синхронизация плагина — в своём).
fn server() -> (String, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let l = log.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            while let Ok((s, _)) = listener.accept().await {
                tokio::spawn(serve(s, l.clone()));
            }
        });
    });
    (format!("http://{}/EWS/Exchange.asmx", rx.recv().unwrap()), log)
}

struct Live {
    commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    cache: Cache,
}

impl Live {
    fn wait(&self, what: &str, pick: impl Fn(&Event) -> bool) -> Event {
        let start = Instant::now();
        loop {
            let left = Duration::from_secs(20).checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
            match self.events.recv_timeout(left) {
                Ok(Event::Error(e)) => panic!("синхронизация: {e}"),
                Ok(ev) if pick(&ev) => return ev,
                Ok(_) => {}
                Err(e) => panic!("не дождались: {what} ({e})"),
            }
        }
    }
}

fn start(url: &str) -> Live {
    let (tx, events) = std::sync::mpsc::channel();
    let cache = Cache::open_in_memory().unwrap();
    // Логин не адрес почты — без Autodiscover (только подменный сервер).
    let account = Account { url: url.to_owned(), auth: Auth::Basic { user: r"EXAMPLE\user1".into(), password: "secret".into() } };
    let commands = postman_backend_ews::EwsPlugin.spawn(Spawn {
        account,
        profile: None,
        cache: cache.clone(),
        index: None,
        events: tx,
        repaint: Box::new(|| {}),
        areas: Vec::new(),
    });
    Live { commands, events, cache }
}

#[test]
fn exchange_mail_through_the_plugin() {
    let (url, log) = server();
    let live = start(&url);
    let Event::Features(f) = live.wait("Features", |e| matches!(e, Event::Features(_))) else { unreachable!() };
    assert!(f.has(Feature::Calendar) && f.has(Feature::FreeBusy) && f.has(Feature::Directory));
    assert!(!f.has(Feature::Tasks) && !f.has(Feature::Contacts) && !f.has(Feature::Rules));
    live.wait("Synced", |e| matches!(e, Event::Status(Status::Synced)));

    // Дерево — запасным путём (FindFolder), роли — по стандартным папкам; календарь и контакты — не почта.
    let boxes = live.cache.mailboxes().unwrap();
    let names: Vec<(&str, Option<&str>)> = boxes.iter().map(|b| (b.name.as_str(), b.role.as_deref())).collect();
    assert!(names.contains(&("Inbox", Some("inbox"))) && names.contains(&("Sent Items", Some("sent"))), "{names:?}");
    assert!(!names.iter().any(|(n, _)| *n == "Calendar"));
    let project = boxes.iter().find(|b| b.name == "Проект").unwrap();
    let inbox = boxes.iter().find(|b| b.role.as_deref() == Some("inbox")).unwrap();
    assert_eq!(project.parent_id.as_deref(), Some(inbox.id.as_str()));
    assert!(log.lock().unwrap().iter().any(|b| b.contains("<m:SyncFolderHierarchy")), "сначала — SyncFolderHierarchy");

    // Первые письма «Входящих»: заголовки списка.
    let rows = live.cache.page(&inbox.id, 0, 50).unwrap();
    let mut subjects: Vec<&str> = rows.iter().map(|r| r.subject.as_str()).collect();
    subjects.sort_unstable();
    assert_eq!(subjects, ["Re: Планы", "Отчёт за неделю"]);
    let report = rows.iter().find(|r| r.subject == "Отчёт за неделю").unwrap();
    assert!(!report.seen && report.flagged && report.has_attachment);

    // «Прочитано» — UpdateItem.
    let n = log.lock().unwrap().len();
    live.commands.send(Command::Keyword { id: report.id.clone(), keyword: "$seen", on: true }).unwrap();
    let start = Instant::now();
    while !log.lock().unwrap()[n..].iter().any(|b| b.contains("<m:UpdateItem")) {
        assert!(start.elapsed() < Duration::from_secs(10), "UpdateItem не ушёл");
        std::thread::sleep(Duration::from_millis(50));
    }
    let upd = log.lock().unwrap().iter().rev().find(|b| b.contains("<m:UpdateItem")).cloned().unwrap();
    assert!(upd.contains(r#"FieldURI="message:IsRead""#) && upd.contains("<t:IsRead>true</t:IsRead>") && upd.contains("AAMkADmsg1"));

    // Справочник — ResolveNames.
    live.commands.send(Command::FindPeople { text: "Петр".into() }).unwrap();
    let Event::PeopleFound { people, .. } = live.wait("PeopleFound", |e| matches!(e, Event::PeopleFound { .. })) else { unreachable!() };
    assert_eq!(people.len(), 3);

    // Отправка из MIME — CreateItem SendAndSaveCopy.
    let draft = postman_compose::Draft {
        from: postman_compose::Address::new("", "user1@example.test"),
        to: vec![postman_compose::Address::new("Борис", "boris@example.test")],
        subject: "Проверка".into(),
        markdown: "Тело".into(),
        ..Default::default()
    };
    live.commands.send(Command::Send { draft: Box::new(draft), replace: None, ticket: 7, release: postman_core::model::Release::Now }).unwrap();
    live.wait("Sent", |e| matches!(e, Event::Sent { ticket: 7, .. }));
    let create = log.lock().unwrap().iter().rev().find(|b| b.contains("<m:CreateItem")).cloned().unwrap();
    assert!(create.contains(r#"MessageDisposition="SendAndSaveCopy""#) && create.contains("<t:MimeContent>"));

    // Неделя календаря — областями EWS.
    live.commands.send(Command::Events { from: 1_792_368_000, to: 1_792_972_800 }).unwrap();
    let Event::EventsLoaded { events, .. } = live.wait("EventsLoaded", |e| matches!(e, Event::EventsLoaded { .. })) else { unreachable!() };
    assert_eq!(events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Планёрка & отчёт"]);
}

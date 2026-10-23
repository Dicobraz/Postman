//! «Сообщить о фишинге» (S5-08) против живого Stalwart тем же путём, что и приложение: план правок —
//! `postman_core::bulk::plan(Act::Phishing)`, на сервер — `sync::Command::Patch` потоком синхронизации. Письмо уходит в
//! папку с ролью `junk` и получает `$phishing` и `$junk` (быстрый фильтр «Угрозы» ищет `hasKeyword $phishing`);
//! «Отменить» (обратный план) возвращает всё как было. Пересылка вложением (`Command::ReportPhishing`) — письмо с
//! частью `message/rfc822` приходит адресату.
//!
//! Письма создаются прямо во «Входящих» user1 (`Email/set create`, без SMTP) и в конце удаляются.
//!
//! Запуск: `POSTMAN_STAND_URL`, `POSTMAN_STAND_USER1`, `POSTMAN_STAND_PASSWORD1` (и `POSTMAN_INSECURE_TLS=1`
//! напрямую), `cargo test -p postman-backend-jmap --test stand_phishing -- --ignored --nocapture --test-threads=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::time::{Duration, Instant};

use postman_cache::{EmailRow, MailboxRow, Rights};
use postman_compose::Address;
use postman_core::bulk::{self, Act, JUNK};
use postman_core::filters::PHISHING;
use postman_core::sync::{self, Account, Command, Event};
use postman_jmap::{Auth, CORE, Client, Filter, Invocation, MAIL};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(30);

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

async fn user1() -> Client {
    let auth = Auth::Basic { user: env("POSTMAN_STAND_USER1"), password: env("POSTMAN_STAND_PASSWORD1") };
    Client::connect(&env("POSTMAN_STAND_URL"), auth).await.unwrap_or_else(|e| panic!("вход user1: {e}"))
}

fn stamp() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}{:03}", t.as_secs(), t.subsec_millis())
}

/// Папки как в кэше приложения (своего аккаунта ключ = id сервера).
async fn mailboxes(c: &Client) -> Vec<MailboxRow> {
    let (boxes, _) = c.mailboxes().await.unwrap();
    boxes
        .into_iter()
        .map(|m| MailboxRow {
            id: m.id,
            parent_id: m.parent_id,
            name: m.name,
            role: m.role,
            sort_order: m.sort_order,
            total: m.total_emails,
            unread: m.unread_emails,
            rights: Rights::ALL,
        })
        .collect()
}

fn role(boxes: &[MailboxRow], r: &str) -> String {
    boxes.iter().find(|m| m.role.as_deref() == Some(r)).map(|m| m.id.clone()).unwrap_or_else(|| panic!("нет папки с ролью {r}"))
}

/// Письмо во «Входящих» без доставки.
async fn create(c: &Client, inbox: &str, subject: &str) -> String {
    let acc = c.mail_account().unwrap().to_owned();
    let email = json!({
        "mailboxIds": { inbox: true },
        "keywords": { "$notjunk": true },
        "from": [{ "name": "Bank security", "email": "security@bank-notify.example" }],
        "to": [{ "email": env("POSTMAN_STAND_USER1") }],
        "subject": subject,
        "bodyValues": { "t": { "value": "Подтвердите платёж по ссылке (S5-08)." } },
        "textBody": [{ "partId": "t", "type": "text/plain" }],
    });
    let calls = [Invocation("Email/set".into(), json!({ "accountId": acc, "create": { "n": email } }), "s".into())];
    let r = c.call(&[CORE, MAIL], &calls).await.unwrap();
    r[0].1["created"]["n"]["id"].as_str().unwrap_or_else(|| panic!("письмо не создано: {}", r[0].1)).to_owned()
}

/// Строка письма как в кэше: папки и ключевые слова с сервера.
async fn row_of(c: &Client, id: &str) -> EmailRow {
    let (emails, _) = c.email_get(&[id.to_owned()]).await.unwrap();
    let e = &emails[0];
    let mut row = EmailRow {
        id: id.to_owned(),
        thread_id: String::new(),
        mailbox_ids: e.mailbox_ids.iter().filter(|(_, on)| **on).map(|(k, _)| k.clone()).collect(),
        received_at: 0,
        subject: e.subject.clone().unwrap_or_default(),
        from_name: String::new(),
        from_addr: String::new(),
        preview: String::new(),
        seen: e.has_keyword("$seen"),
        flagged: e.has_keyword("$flagged"),
        has_attachment: false,
        size: 0,
        keywords: Vec::new(),
    };
    for (k, on) in &e.keywords {
        if *on {
            row.set_keyword(k, true);
        }
    }
    row
}

/// Поток синхронизации user1 на кэше в памяти: команды и события; ждёт первой загрузки.
fn sync_user1() -> (sync::UnboundedSender<Command>, std::sync::mpsc::Receiver<Event>) {
    let account = Account {
        url: env("POSTMAN_STAND_URL"),
        auth: postman_core::model::Auth::Basic { user: env("POSTMAN_STAND_USER1"), password: env("POSTMAN_STAND_PASSWORD1") },
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let commands = postman_backend_jmap::spawn(account, postman_cache::Cache::open_in_memory().unwrap(), None, tx, || {});
    wait_for(&rx, "первая синхронизация", |e| matches!(e, Event::Status(sync::Status::Synced)).then_some(()));
    (commands, rx)
}

fn wait_for<T>(rx: &std::sync::mpsc::Receiver<Event>, what: &str, pick: impl Fn(&Event) -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        let left = WAIT.checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
        match rx.recv_timeout(left) {
            Ok(ev) => {
                if let Some(v) = pick(&ev) {
                    return v;
                }
                if let Event::Error(e) = ev {
                    panic!("синхронизация ({what}): {e}");
                }
            }
            Err(e) => panic!("не дождались: {what} ({e})"),
        }
    }
}

/// Повторять, пока письмо на сервере не станет таким, как ждём.
async fn eventually(c: &Client, id: &str, what: &str, ok: impl Fn(&EmailRow) -> bool) -> EmailRow {
    let start = Instant::now();
    loop {
        let row = row_of(c, id).await;
        if ok(&row) {
            return row;
        }
        assert!(start.elapsed() < WAIT, "не дождались ({what}): {row:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// «Сообщить о фишинге»: в «Нежелательные» с `$phishing` и `$junk` без `$notjunk`; «Угрозы» (`hasKeyword`) находит;
/// «Отменить» — обратно во «Входящие» с прежними словами.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn phishing_moves_to_junk_with_keyword_and_undo_restores() {
    let c = user1().await;
    let boxes = mailboxes(&c).await;
    let (inbox, junk) = (role(&boxes, "inbox"), role(&boxes, "junk"));
    let id = create(&c, &inbox, &format!("Postman phishing {}", stamp())).await;
    let row = row_of(&c, &id).await;

    let plan = bulk::plan(std::slice::from_ref(&row), &Act::Phishing, &boxes, |_, _| true);
    assert_eq!(plan.patches.len(), 1, "{plan:?}");
    let (commands, _rx) = sync_user1();
    assert!(commands.send(Command::Patch { patches: plan.patches.clone(), archive_name: "Archive".into() }).is_ok());
    let after = eventually(&c, &id, "в «Нежелательных» с $phishing", |r| r.mailbox_ids == [junk.clone()] && r.has_keyword(PHISHING)).await;
    println!("after report: boxes {:?}, keywords {:?}", after.mailbox_ids, after.keywords);
    assert!(after.has_keyword(JUNK) && !after.has_keyword("$notjunk"), "{after:?}");
    // Быстрый фильтр «Угрозы» на сервере («Все папки»).
    let threats = c.filtered(&Filter::HasKeyword(PHISHING.into()), 100).await.unwrap();
    assert!(threats.ids.contains(&id), "hasKeyword $phishing не нашёл письмо");

    // «Отменить».
    assert!(commands.send(Command::Patch { patches: plan.undo.clone(), archive_name: "Archive".into() }).is_ok());
    let back = eventually(&c, &id, "обратно во «Входящие»", |r| r.mailbox_ids == [inbox.clone()] && !r.has_keyword(PHISHING)).await;
    assert!(back.has_keyword("$notjunk") && !back.has_keyword(JUNK), "{back:?}");
    c.destroy_email(&id).await.unwrap();
}

/// Пересылка вложением: одно письмо на адрес из настроек, внутри — часть `message/rfc822` с исходным письмом.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn phishing_report_is_forwarded_as_an_attachment() {
    let c = user1().await;
    let boxes = mailboxes(&c).await;
    let inbox = role(&boxes, "inbox");
    let run = stamp();
    let id = create(&c, &inbox, &format!("Postman phishing fwd {run}")).await;
    let (commands, rx) = sync_user1();
    let report = format!("Postman phishing report {run}");
    let to = Address::new("", env("POSTMAN_STAND_USER1"));
    let cmd = Command::ReportPhishing { ids: vec![id.clone()], to, subject: report.clone(), text: "Фишинг.".into(), ticket: 7 };
    assert!(commands.send(cmd).is_ok());
    let error = wait_for(&rx, "Reported", |e| match e {
        Event::Reported { ticket: 7, error } => Some(error.clone()),
        _ => None,
    });
    assert_eq!(error, None);
    // Письмо приходит адресату (себе) с вложением message/rfc822.
    let start = Instant::now();
    let got = loop {
        let page = c.search(&report, Some(&inbox), 5).await.unwrap();
        if let Some(e) = page.emails.into_iter().find(|e| e.subject.as_deref() == Some(report.as_str())) {
            break e.id;
        }
        assert!(start.elapsed() < WAIT, "сообщение о фишинге не пришло");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let body = c.email_body(&got).await.unwrap();
    let kinds: Vec<&str> = body.attachments.iter().map(|a| a.mime.as_str()).collect();
    println!("report attachments: {kinds:?}");
    assert!(kinds.contains(&"message/rfc822"), "нет вложения message/rfc822: {kinds:?}");
    // Убрать: исходное, пришедшее и копию в «Отправленных».
    for e in c.search(&format!("Postman phishing {}", ""), None, 50).await.unwrap().emails {
        let s = e.subject.as_deref().unwrap_or("");
        if s.contains(&run) {
            c.destroy_email(&e.id).await.unwrap();
        }
    }
    let _ = c.destroy_email(&id).await;
}

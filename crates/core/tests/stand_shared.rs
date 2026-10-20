//! Общие и делегированные ящики (S4-06) против живого Stalwart: user3 видит ящик группы `shared-sales@itt.test`,
//! синхронизация кладёт его «Входящие» в кэш с префиксом аккаунта, письмо, доставленное на адрес группы, читается,
//! флаг ставится на сервере; отправка от имени ящика; папка, открытая только для чтения (`shared-ro@itt.test`).
//! Подготовка стенда — `docs/STALWART.md`, «Общие ящики». Без стенда тесты пропускаются (`#[ignore]`).
//!
//! Запуск: `POSTMAN_STAND_URL`, `POSTMAN_STAND_USER3`, `POSTMAN_STAND_PASSWORD3` (и `POSTMAN_INSECURE_TLS=1` напрямую),
//! `cargo test -p postman-core --test stand_shared -- --ignored`. Писем — по одному на прогон и адрес (ограничение
//! входящих на получателя, `docs/STALWART.md`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::time::{Duration, Instant};

use postman_cache::{Cache, scoped};
use postman_compose::{Address, Draft};
use postman_core::sync::{self, Account, Command, Event, Status};
use postman_jmap::{Auth, Client, Envelope, SharedAccount};

/// Сколько ждать доставку и синхронизацию.
const DELIVERY: Duration = Duration::from_secs(30);
/// Ящик группы (`x:Account/set` `@type: Group`, user3 — участник).
const GROUP: &str = "shared-sales@itt.test";
/// Ящик пользователя, «Входящие» которого открыты user3 только для чтения (`Mailbox/set` `shareWith`).
const READ_ONLY: &str = "shared-ro@itt.test";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

fn auth() -> Auth {
    Auth::Basic { user: env("POSTMAN_STAND_USER3"), password: env("POSTMAN_STAND_PASSWORD3") }
}

async fn viewer() -> Client {
    Client::connect(&env("POSTMAN_STAND_URL"), auth()).await.unwrap_or_else(|e| panic!("вход user3: {e}"))
}

fn stamp() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}-{}", t.as_secs(), t.subsec_nanos() % 1000)
}

fn shared(c: &Client, name: &str) -> SharedAccount {
    let all = c.session().shared_accounts();
    all.iter()
        .find(|a| a.name == name)
        .cloned()
        .unwrap_or_else(|| panic!("в сессии user3 нет общего ящика {name}: {all:?} (подготовка — docs/STALWART.md)"))
}

async fn role(c: &Client, r: &str) -> String {
    let (boxes, _) = c.mailboxes().await.unwrap();
    boxes.into_iter().find(|m| m.role.as_deref() == Some(r)).map(|m| m.id).unwrap_or_else(|| panic!("нет папки {r}"))
}

/// Письмо от user3 (из своего аккаунта) на адрес `to`.
async fn send_from_own(c: &Client, to: &str, subject: &str) {
    let me = env("POSTMAN_STAND_USER3");
    let identity = c.identities().await.unwrap().into_iter().find(|i| i.email == me).unwrap();
    let draft = Draft {
        from: Address::new("User 3", &me),
        to: vec![Address::new("", to)],
        subject: subject.into(),
        markdown: "Письмо в **общий ящик**.".into(),
        ..Default::default()
    };
    let now = jiff::Timestamp::now().as_second();
    let raw = postman_compose::build(&draft, now, &postman_compose::new_message_id(&draft.from, now)).unwrap();
    let envelope = Envelope { mail_from: me, rcpt_to: vec![to.into()] };
    c.send(raw, &identity.id, &envelope, &role(c, "drafts").await, &role(c, "sent").await).await.unwrap();
}

/// Письмо с темой в папке аккаунта клиента `c` (сервер), id сервера.
async fn find(c: &Client, mailbox: &str, subject: &str) -> Option<String> {
    let page = c.search(subject, Some(mailbox), 5).await.ok()?;
    page.emails.into_iter().find(|e| e.subject.as_deref() == Some(subject)).map(|e| e.id)
}

type Events = std::sync::mpsc::Receiver<Event>;

/// Синхронизация user3 на кэше в памяти; кэш — общий с тестом (тот же `Database`).
fn sync_user3() -> (sync::UnboundedSender<Command>, Events, Cache) {
    let cache = Cache::open_in_memory().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let commands = sync::spawn(Account { url: env("POSTMAN_STAND_URL"), auth: auth() }, cache.clone(), None, tx, || {});
    (commands, rx, cache)
}

fn wait_for<T>(rx: &Events, what: &str, pick: impl Fn(&Event) -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        let left = DELIVERY.checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
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

/// Ждать, пока в кэше (папка — ключ `mailbox`) появится письмо с темой; события синхронизации читаются, чтобы
/// заметить её ошибку.
fn wait_in_cache(rx: &Events, cache: &Cache, mailbox: &str, subject: &str) -> String {
    let start = Instant::now();
    loop {
        if let Some(r) = cache.page(mailbox, 0, 50).unwrap().into_iter().find(|r| r.subject == subject) {
            return r.id;
        }
        assert!(start.elapsed() < DELIVERY, "письмо «{subject}» не пришло в кэш {mailbox} за {DELIVERY:?}");
        if let Ok(Event::Error(e)) = rx.recv_timeout(Duration::from_millis(300)) {
            panic!("синхронизация: {e}");
        }
    }
}

/// user3 видит ящик группы; письмо, доставленное на адрес группы, приходит в кэш (push по аккаунту группы),
/// читается (`Body`) и помечается флагом (`Keyword`) — на сервере, в аккаунте группы.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn shared_inbox_is_synced_read_and_flagged() {
    let c = viewer().await;
    let group = shared(&c, GROUP);
    assert!(!group.is_personal && !group.is_read_only && group.can_submit, "{group:?}");
    let theirs = c.in_account(&group.id);
    let inbox = role(&theirs, "inbox").await;

    let (commands, rx, cache) = sync_user3();
    wait_for(&rx, "первая синхронизация", |e| {
        matches!(e, Event::Status(Status::Synced)).then_some(())
    });
    // Ящик группы — в кэше: аккаунт, папки с префиксом.
    assert!(cache.accounts().unwrap().iter().any(|a| a.id == group.id && a.name == GROUP));
    let key = scoped(Some(&group.id), &inbox);
    assert!(cache.mailboxes().unwrap().iter().any(|m| m.id == key && m.role.as_deref() == Some("inbox")));

    // Сверка после подключения push — дать ей пройти: письмо дальше приходит только по push аккаунта группы.
    tokio::time::sleep(Duration::from_secs(3)).await;
    while rx.try_recv().is_ok() {}
    let subject = format!("Postman shared {}", stamp());
    let sent = Instant::now();
    send_from_own(&c, GROUP, &subject).await;
    let row = wait_in_cache(&rx, &cache, &key, &subject);
    // Опрос — раз в 2 минуты: за это время письмо могло прийти только по push.
    println!("delivered to {GROUP} → cache in {:?} (push)", sent.elapsed());
    let (scope, id) = postman_cache::unscoped(&row);
    assert_eq!(scope, Some(group.id.as_str()));

    assert!(commands.send(Command::Body(row.clone())).is_ok());
    wait_for(&rx, "тело письма", |e| matches!(e, Event::BodyReady(k) if *k == row).then_some(()));
    let body = cache.body(&row).unwrap().unwrap();
    assert!(body.text.is_some() || body.html.is_some(), "у письма нет тела");

    assert!(commands.send(Command::Keyword { id: row.clone(), keyword: "$flagged", on: true }).is_ok());
    let id = id.to_owned();
    let start = Instant::now();
    loop {
        let (emails, _) = theirs.email_get(std::slice::from_ref(&id)).await.unwrap();
        if emails.first().is_some_and(|e| e.has_keyword("$flagged")) {
            break;
        }
        assert!(start.elapsed() < DELIVERY, "флаг не дошёл до сервера");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    theirs.destroy_email(&id).await.unwrap();
    if let Some(copy) = find(&c, &role(&c, "sent").await, &subject).await {
        c.destroy_email(&copy).await.unwrap();
    }
}

/// Отправка от имени ящика группы: Identity группы есть в её аккаунте, окно письма получает её (`Senders`);
/// письмо уходит из аккаунта группы (копия — в её «Отправленных»), у получателя «От» — адрес группы.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn mail_is_sent_as_the_shared_mailbox() {
    let c = viewer().await;
    let group = shared(&c, GROUP);
    let theirs = c.in_account(&group.id);
    let me = env("POSTMAN_STAND_USER3");

    let (commands, rx, _cache) = sync_user3();
    let senders = wait_for(&rx, "отправители", |e| match e {
        Event::Senders(s) => Some(s.clone()),
        _ => None,
    });
    let shared_sender = senders
        .iter()
        .find(|s| s.address.email.eq_ignore_ascii_case(GROUP))
        .unwrap_or_else(|| panic!("нет отправителя {GROUP}: {senders:?}"));
    assert_eq!(shared_sender.account.as_deref(), Some(group.id.as_str()), "адрес группы — из её аккаунта");
    assert_eq!(senders.first().map(|s| s.address.email.as_str()), Some(me.as_str()), "свой адрес — первым");
    // Ящик только для чтения (не мой) — не отправитель.
    assert!(!senders.iter().any(|s| s.address.email.eq_ignore_ascii_case(READ_ONLY)));
    wait_for(&rx, "первая синхронизация", |e| {
        matches!(e, Event::Status(Status::Synced)).then_some(())
    });

    let subject = format!("Postman send-as {}", stamp());
    let draft = Draft {
        from: shared_sender.address.clone(),
        to: vec![Address::new("User 3", &me)],
        subject: subject.clone(),
        markdown: "От имени общего ящика.".into(),
        ..Default::default()
    };
    assert!(commands.send(Command::Send { draft: Box::new(draft), replace: None, ticket: 7 }).is_ok());
    wait_for(&rx, "отправлено", |e| match e {
        Event::Sent { ticket: 7 } => Some(()),
        Event::ComposeFailed { ticket: 7, error } => panic!("отправка от имени {GROUP}: {error}"),
        _ => None,
    });

    // У получателя (user3) — «От» группы.
    let own_inbox = role(&c, "inbox").await;
    let start = Instant::now();
    let got = loop {
        if let Some(id) = find(&c, &own_inbox, &subject).await {
            break id;
        }
        assert!(start.elapsed() < DELIVERY, "письмо от имени группы не пришло");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let (emails, _) = c.email_get(std::slice::from_ref(&got)).await.unwrap();
    let from = emails.first().and_then(|e| e.from.as_ref()).and_then(|f| f.first()).and_then(|a| a.email.clone());
    assert_eq!(from.as_deref().map(str::to_lowercase).as_deref(), Some(GROUP));
    // Копия — в «Отправленных» группы, не в своих.
    let copy = find(&theirs, &role(&theirs, "sent").await, &subject).await.expect("копии нет в «Отправленных» группы");
    assert!(find(&c, &role(&c, "sent").await, &subject).await.is_none(), "копия попала в свои «Отправленные»");

    c.destroy_email(&got).await.unwrap();
    theirs.destroy_email(&copy).await.unwrap();
}

/// Папка, открытая только для чтения: аккаунт в сессии есть (`isReadOnly` у Stalwart — `false`), права —
/// в `myRights` папки; Identity чужого ящика не отдаются (`forbidden`).
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn read_only_share_has_rights_but_no_identity() {
    let c = viewer().await;
    let ro = shared(&c, READ_ONLY);
    let theirs = c.in_account(&ro.id);
    let (boxes, _) = theirs.mailboxes().await.unwrap();
    let inbox = boxes.iter().find(|m| m.role.as_deref() == Some("inbox")).expect("«Входящие» не открыты user3");
    let r = inbox.my_rights;
    assert!(r.may_read_items && !r.may_add_items && !r.may_remove_items && !r.may_submit, "{r:?}");
    println!("read-only share: isReadOnly={} rights={r:?} folders={}", ro.is_read_only, boxes.len());
    match theirs.identities().await {
        Err(postman_jmap::Error::Method { kind, .. }) => assert_eq!(kind, "forbidden"),
        other => panic!("Identity чужого ящика: {other:?}"),
    }
}

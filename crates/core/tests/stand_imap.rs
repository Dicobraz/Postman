//! Учётная запись IMAP + SMTP через синхронизацию Postman (`postman_core::imap`, S7-04б) против IMAP и submission
//! стенда Stalwart: вход, папки, письмо себе, тело (разбор MIME в воркере), вложение, прочитано, перенос, удаление,
//! копия в «Отправленных», черновик. Без стенда тесты пропускаются (`#[ignore]`); за собой убирают.
//!
//! Запуск (пароли и адрес — только в окружении; самоподписанный сертификат стенда — `POSTMAN_INSECURE_TLS=1`;
//! воркер — `cargo build -p postman-worker` заранее):
//!   POSTMAN_STAND_IMAP=<адрес>:993 POSTMAN_STAND_SMTP=<адрес>:465 POSTMAN_STAND_USER1=user1@itt.test
//!   POSTMAN_STAND_PASSWORD1=… cargo test -p postman-core --test stand_imap -- --ignored --test-threads=1

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use postman_cache::{Cache, EmailRow};
use postman_compose::{Address, Attachment, Draft};
use postman_core::imap::{self, ImapAccount};
use postman_core::model::Release;
use postman_core::sync::{Command, Event, Status, UnboundedSender};

const WAIT: Duration = Duration::from_secs(40);

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

/// Воркер-песочница — рядом с исполняемым файлом теста (`target/debug/deps`), как у приложения.
fn worker_next_to_tests() {
    let exe = std::env::current_exe().unwrap();
    let deps = exe.parent().unwrap();
    let name = "postman-worker.exe";
    let built = deps.parent().unwrap().join(name);
    if !deps.join(name).exists() {
        std::fs::copy(&built, deps.join(name)).unwrap_or_else(|e| panic!("{}: {e} — сначала cargo build -p postman-worker", built.display()));
    }
}

struct Live {
    cache: Cache,
    commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    me: String,
}

impl Live {
    fn start() -> Live {
        worker_next_to_tests();
        let me = env("POSTMAN_STAND_USER1");
        let url = format!("imaps://{}?smtp={}", env("POSTMAN_STAND_IMAP"), env("POSTMAN_STAND_SMTP"));
        let account = ImapAccount::from_url(&url, &me, &env("POSTMAN_STAND_PASSWORD1")).unwrap();
        let cache = Cache::open_in_memory().unwrap();
        let (tx, events) = std::sync::mpsc::channel();
        let commands = imap::spawn(account, None, cache.clone(), None, tx, || {});
        let live = Live { cache, commands, events, me };
        live.wait("Synced", |e| matches!(e, Event::Status(Status::Synced)));
        live
    }

    fn wait(&self, what: &str, pick: impl Fn(&Event) -> bool) -> Event {
        let start = Instant::now();
        loop {
            let left = WAIT.checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
            match self.events.recv_timeout(left) {
                Ok(Event::Error(e)) => panic!("синхронизация: {e}"),
                Ok(Event::ComposeFailed { error, .. }) => panic!("письмо: {error}"),
                Ok(ev) if pick(&ev) => return ev,
                Ok(_) => {}
                Err(e) => panic!("не дождались: {what} ({e})"),
            }
        }
    }

    /// Письмо с темой в папке роли `role` — когда оно появится в кэше.
    fn letter(&self, role: &str, subject: &str) -> EmailRow {
        let start = Instant::now();
        loop {
            let folder = self.cache.mailboxes().unwrap().into_iter().find(|m| m.role.as_deref() == Some(role)).expect(role);
            if let Some(r) = self.cache.page(&folder.id, 0, 1000).unwrap().into_iter().find(|r| r.subject == subject) {
                return r;
            }
            assert!(start.elapsed() < WAIT, "письмо «{subject}» не появилось в «{role}»");
            // Опрос папок — раз в 2 мин; «Входящие» — по IDLE. Поиск по серверу ускоряет «Отправленные».
            let _ = self.commands.send(Command::Search { query: subject.into(), mailbox: Some(folder.id.clone()), ticket: 0 });
            std::thread::sleep(Duration::from_millis(700));
        }
    }

    fn folder(&self, role: &str) -> String {
        self.cache.mailboxes().unwrap().into_iter().find(|m| m.role.as_deref() == Some(role)).expect(role).id
    }
}

fn stamp() -> String {
    format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
}

/// Вход, папки с ролями, письмо себе (SMTP) с вложением → во «Входящих» (IDLE) и копия в «Отправленных»; тело и
/// вложение — через воркер; прочитано на сервере; перенос в «Удалённые» и удаление.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_account_round_trip() {
    let live = Live::start();
    let roles: Vec<String> = live.cache.mailboxes().unwrap().into_iter().filter_map(|m| m.role).collect();
    for r in ["inbox", "sent", "drafts", "trash"] {
        assert!(roles.iter().any(|x| x == r), "нет папки {r}: {roles:?}");
    }
    let subject = format!("postman imap sync {}", stamp());
    let draft = Draft {
        from: Address::new("", &live.me),
        to: vec![Address::new("", &live.me)],
        subject: subject.clone(),
        markdown: "Проверка **IMAP** в Postman.".into(),
        attachments: vec![Attachment { name: "note.txt".into(), mime: "text/plain".into(), data: b"hello".to_vec(), inline_cid: None, blob_id: None }],
        ..Default::default()
    };
    live.commands.send(Command::Send { draft: Box::new(draft), replace: None, ticket: 7, release: Release::Now }).unwrap();
    live.wait("Sent", |e| matches!(e, Event::Sent { ticket: 7, .. }));
    let inbox = live.letter("inbox", &subject);
    let sent = live.letter("sent", &subject);
    assert!(sent.seen && inbox.has_attachment);

    live.commands.send(Command::Body(inbox.id.clone())).unwrap();
    live.wait("BodyReady", |e| matches!(e, Event::BodyReady(k) if *k == inbox.id));
    let body = live.cache.body(&inbox.id).unwrap().unwrap();
    assert!(body.text.as_deref().unwrap_or_default().contains("IMAP"), "{:?}", body.text);
    assert_eq!(body.to.first().map(|a| a.email.as_str()), Some(live.me.as_str()));
    let a = body.attachments.iter().find(|a| a.name == "note.txt").expect("вложение");
    live.commands.send(Command::Blob { blob_id: a.blob_id.clone(), name: a.name.clone(), mime: a.mime.clone() }).unwrap();
    let Event::BlobReady { data, .. } = live.wait("BlobReady", |e| matches!(e, Event::BlobReady { .. })) else { unreachable!() };
    assert_eq!(data.as_slice(), b"hello");

    live.commands.send(Command::Keyword { id: inbox.id.clone(), keyword: "$seen", on: true }).unwrap();
    let trash = live.folder("trash");
    live.commands.send(Command::Move { id: inbox.id.clone(), from: live.folder("inbox"), to: trash.clone() }).unwrap();
    let moved = live.letter("trash", &subject);
    assert!(moved.seen, "прочитано — на сервере, и после переноса");
    live.commands.send(Command::Destroy(moved.id.clone())).unwrap();
    live.commands.send(Command::Destroy(sent.id.clone())).unwrap();
    let start = Instant::now();
    while !live.cache.rows_by_ids(&[moved.id.clone(), sent.id.clone()]).unwrap().is_empty() {
        assert!(start.elapsed() < WAIT, "письма не удалились");
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Черновик: сохранить, заменить новой версией (прежняя удаляется), удалить.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_draft_replace() {
    let live = Live::start();
    let subject = format!("postman imap draft {}", stamp());
    let draft = |text: &str| Draft {
        from: Address::new("", &live.me),
        to: vec![Address::new("", &live.me)],
        subject: subject.clone(),
        markdown: text.into(),
        ..Default::default()
    };
    live.commands.send(Command::SaveDraft { draft: Box::new(draft("первая")), replace: None, ticket: 1 }).unwrap();
    let Event::DraftSaved { id: first, .. } = live.wait("DraftSaved 1", |e| matches!(e, Event::DraftSaved { ticket: 1, .. })) else { unreachable!() };
    live.commands.send(Command::SaveDraft { draft: Box::new(draft("вторая")), replace: Some(first.clone()), ticket: 2 }).unwrap();
    let Event::DraftSaved { id: second, .. } = live.wait("DraftSaved 2", |e| matches!(e, Event::DraftSaved { ticket: 2, .. })) else { unreachable!() };
    assert_ne!(first, second);
    let drafts = live.folder("drafts");
    let same: Vec<EmailRow> = live.cache.page(&drafts, 0, 1000).unwrap().into_iter().filter(|r| r.subject == subject).collect();
    assert_eq!(same.len(), 1, "прежняя версия черновика осталась");
    live.commands.send(Command::Destroy(second)).unwrap();
}

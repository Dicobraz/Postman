//! Клиент IMAP и SMTP против IMAP и submission стенда Stalwart (`docs/STALWART.md`): вход, папки с ролями,
//! отправка себе, заголовок, ключевые слова, перенос и удаление, IDLE. Без стенда тесты пропускаются (`#[ignore]`).
//!
//! Запуск (пароли — только в окружении; самоподписанный сертификат стенда — `POSTMAN_INSECURE_TLS=1`):
//!   POSTMAN_STAND_IMAP=localhost:993 POSTMAN_STAND_SMTP=localhost:465 POSTMAN_STAND_USER1=user1@itt.test
//!   POSTMAN_STAND_PASSWORD1=… cargo test -p postman-imap --test stand -- --ignored --test-threads=1
//! Порт 993 и 465 — TLS сразу, другой — STARTTLS.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::time::{Duration, Instant};

use postman_imap::{Config, Role, Security, Session, flags, smtp, uid_set};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

fn config(var: &str) -> Config {
    let at = env(var);
    let (host, port) = at.rsplit_once(':').expect("host:port");
    let port: u16 = port.parse().expect("port");
    let security = if port == 993 || port == 465 { Security::Tls } else { Security::StartTls };
    Config { host: host.into(), port, security, user: env("POSTMAN_STAND_USER1"), password: env("POSTMAN_STAND_PASSWORD1") }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn letter(to: &str, subject: &str) -> Vec<u8> {
    format!("From: <{to}>\r\nTo: <{to}>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@postman.test>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nПроверка IMAP.\r\n").into_bytes()
}

async fn wait_for(s: &mut Session, subject: &str) -> u32 {
    let start = Instant::now();
    loop {
        s.select("INBOX").await.unwrap();
        if let Some(&uid) = s.search_text(subject).await.unwrap().last() {
            return uid;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "письмо «{subject}» не пришло за 30 с");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn stamp() -> String {
    format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
}

/// Вход, папки с ролями, письмо себе по SMTP, заголовок, ключевые слова, перенос в «Удалённые» и удаление.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_mail_round_trip() {
    rt().block_on(async {
        let imap = config("POSTMAN_STAND_IMAP");
        let me = imap.user.clone();
        let mut s = Session::connect(&imap).await.unwrap();
        assert!(s.has("IMAP4rev1") || s.has("IMAP4rev2"), "{:?}", s.capabilities());
        let folders = s.folders().await.unwrap();
        let role = |r: Role| folders.iter().find(|f| f.role == Some(r)).map(|f| f.raw.clone());
        assert_eq!(role(Role::Inbox).as_deref(), Some("INBOX"));
        let trash = role(Role::Trash).expect("папка «Удалённые»");
        let selected = s.select("INBOX").await.unwrap();
        assert!(selected.state.uid_validity > 0);

        let subject = format!("postman-imap-{}", stamp());
        smtp::submit(&config("POSTMAN_STAND_SMTP"), &me, std::slice::from_ref(&me), &letter(&me, &subject)).await.unwrap();
        let uid = wait_for(&mut s, &subject).await;
        let h = s.headers(&uid.to_string()).await.unwrap().pop().expect("заголовок");
        assert_eq!((h.subject.as_str(), h.from_addr.as_str()), (subject.as_str(), me.as_str()));
        assert!(h.received_at > 0 && h.size > 0);

        // Ключевые слова Postman — если сервер их хранит.
        let keywords = s.select("INBOX").await.unwrap().keywords;
        let due = flags::flag("pm.due").unwrap();
        s.store(&uid.to_string(), &[due.clone(), "\\Seen".into()], true).await.unwrap();
        let h = s.headers(&uid.to_string()).await.unwrap().pop().unwrap();
        assert!(h.flags.iter().any(|f| f == "\\Seen"), "{:?}", h.flags);
        assert_eq!(h.flags.iter().any(|f| f.eq_ignore_ascii_case(&due)), keywords, "{:?}", h.flags);

        s.move_to(&uid_set(&[uid]), &trash).await.unwrap();
        assert!(!s.uids().await.unwrap().contains(&uid));
        s.select(&trash).await.unwrap();
        let in_trash = s.search_text(&subject).await.unwrap();
        assert!(!in_trash.is_empty());
        s.delete(&uid_set(&in_trash)).await.unwrap();
        assert!(s.search_text(&subject).await.unwrap().is_empty());
        s.logout().await.unwrap();
    });
}

/// CONDSTORE: изменённые флаги — после `HIGHESTMODSEQ`, без сверки всей папки.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_condstore() {
    rt().block_on(async {
        let imap = config("POSTMAN_STAND_IMAP");
        let me = imap.user.clone();
        let mut s = Session::connect(&imap).await.unwrap();
        assert!(s.has("CONDSTORE") || s.has("QRESYNC"), "{:?}", s.capabilities());
        let subject = format!("postman-condstore-{}", stamp());
        smtp::submit(&config("POSTMAN_STAND_SMTP"), &me, std::slice::from_ref(&me), &letter(&me, &subject)).await.unwrap();
        let uid = wait_for(&mut s, &subject).await;
        let before = s.select("INBOX").await.unwrap().state.modseq.expect("HIGHESTMODSEQ");
        s.store(&uid.to_string(), &["\\Flagged".into()], true).await.unwrap();
        let changed = s.changed_since(before).await.unwrap();
        assert!(changed.iter().any(|c| c.uid == uid && c.flags.iter().any(|f| f == "\\Flagged")), "{changed:?}");
        s.delete(&uid.to_string()).await.unwrap();
        s.logout().await.unwrap();
    });
}

/// IDLE: новое письмо будит сессию раньше срока.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_idle() {
    rt().block_on(async {
        let imap = config("POSTMAN_STAND_IMAP");
        let me = imap.user.clone();
        let mut s = Session::connect(&imap).await.unwrap();
        assert!(s.has("IDLE"), "{:?}", s.capabilities());
        s.select("INBOX").await.unwrap();
        let subject = format!("postman-idle-{}", stamp());
        let send = {
            let (smtp_cfg, me, raw) = (config("POSTMAN_STAND_SMTP"), me.clone(), letter(&me, &subject));
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                smtp::submit(&smtp_cfg, &me, std::slice::from_ref(&me), &raw).await.unwrap();
            })
        };
        let start = Instant::now();
        let (mut s, changed) = s.idle(Duration::from_secs(60)).await.unwrap();
        send.await.unwrap();
        assert!(changed && start.elapsed() < Duration::from_secs(30), "IDLE не проснулся");
        let uid = wait_for(&mut s, &subject).await;
        s.delete(&uid.to_string()).await.unwrap();
        s.logout().await.unwrap();
    });
}

/// Неверный пароль — ошибка входа, не сети.
#[test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
fn imap_wrong_password() {
    rt().block_on(async {
        let imap = Config { password: "not-the-password".into(), ..config("POSTMAN_STAND_IMAP") };
        assert!(matches!(Session::connect(&imap).await, Err(postman_imap::Error::Auth(_))));
    });
}

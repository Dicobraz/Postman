//! Настройки ящика на живом Stalwart (S4-05, S4-07): автоответ и квота — тем же кодом, что и приложение
//! (`postman_core::settings`). Меняются настройки только user2; user1 пишет ему письма. Без стенда тесты
//! пропускаются (`#[ignore]`); за собой убирают (автоответ выключен, письма удалены).
//!
//! Запуск — как `stand.rs` (`docs/STALWART.md`):
//!   cargo test -p postman-core --test stand_settings -- --ignored

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::cell::RefCell;
use std::future::Future;
use std::time::{Duration, Instant};

use postman_compose::{Address, Draft};
use postman_core::settings::{self, Reply, Request};
use postman_jmap::{Auth, Client, Envelope, VacationResponse};

/// Сколько ждать доставки письма и автоответа.
const DELIVERY: Duration = Duration::from_secs(60);

struct User {
    email: String,
    client: Client,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

async fn user(n: u8) -> User {
    let email = env(&format!("POSTMAN_STAND_USER{n}"));
    let password = env(&format!("POSTMAN_STAND_PASSWORD{n}"));
    let client = Client::connect(&env("POSTMAN_STAND_URL"), Auth::Basic { user: email.clone(), password })
        .await
        .unwrap_or_else(|e| panic!("вход {email}: {e}"));
    User { email, client }
}

fn stamp() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}-{}", t.as_secs(), t.subsec_nanos() % 1000)
}

/// Повторять `f`, пока не вернёт `Some`, не дольше `DELIVERY`; `None` — не дождались.
async fn eventually<T, F, Fut>(mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = Instant::now();
    loop {
        if let Some(v) = f().await {
            return Some(v);
        }
        if start.elapsed() > DELIVERY {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

/// Запрос настроек как в приложении; ответы — списком.
async fn settings(client: &Client, request: Request) -> Vec<Reply> {
    let replies = RefCell::new(Vec::new());
    settings::handle(client, request, &|r| replies.borrow_mut().push(r)).await;
    replies.into_inner()
}

fn role(boxes: &[postman_jmap::Mailbox], r: &str) -> String {
    boxes.iter().find(|m| m.role.as_deref() == Some(r)).map(|m| m.id.clone()).unwrap()
}

/// Письмо `from` → `to` тем же путём, что и приложение (blob → «Черновики» → EmailSubmission).
async fn send(from: &User, to: &str, subject: &str, text: &str) -> String {
    let (boxes, _) = from.client.mailboxes().await.unwrap();
    let identity = from.client.identities().await.unwrap().into_iter().find(|i| i.email == from.email).unwrap();
    let draft = Draft {
        from: Address::new("User 1", &from.email),
        to: vec![Address::new("", to)],
        subject: subject.into(),
        markdown: text.into(),
        ..Default::default()
    };
    let now = jiff::Timestamp::now().as_second();
    let raw = postman_compose::build(&draft, now, &postman_compose::new_message_id(&draft.from, now)).unwrap();
    let envelope = Envelope { mail_from: from.email.clone(), rcpt_to: vec![to.into()] };
    from.client.send(raw, &identity.id, &envelope, &role(&boxes, "drafts"), &role(&boxes, "sent")).await.unwrap()
}

/// Письма с текстом `text` во всех папках пользователя: (id, тема, ящики).
async fn find(u: &User, text: &str) -> Vec<postman_jmap::EmailSummary> {
    u.client.search(text, None, 20).await.map(|p| p.emails).unwrap_or_default()
}

async fn destroy_all(u: &User, text: &str) {
    for e in find(u, text).await {
        let _ = u.client.destroy_email(&e.id).await;
    }
}

/// S4-05: автоответ user2 включается, user1 получает ответ на своё письмо, автоответ выключается.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn vacation_reply_round_trip() {
    let (a, b) = (user(1).await, user(2).await);
    let stamp = stamp();
    let before = b.client.vacation().await.unwrap();
    let reply_subject = format!("Postman vacation {stamp}");

    let result = async {
        let on = VacationResponse {
            is_enabled: true,
            from_date: None,
            to_date: None,
            subject: Some(reply_subject.clone()),
            text_body: Some("Нет на месте (тест стенда).".into()),
            html_body: None,
        };
        let saved = match settings(&b.client, Request::SetVacation(on.clone())).await.pop() {
            Some(Reply::VacationSaved(v)) => v,
            Some(Reply::Failed { error, .. }) => return Err(format!("автоответ не включён: {error}")),
            _ => return Err("нет ответа VacationSaved".into()),
        };
        if saved != (VacationResponse { html_body: saved.html_body.clone(), ..on }) {
            return Err(format!("сервер сохранил не то: {saved:?}"));
        }
        send(&a, &b.email, &format!("Postman probe {stamp}"), "Проверка автоответа.").await;
        let got = eventually(|| async {
            find(&a, &reply_subject).await.into_iter().find(|e| {
                e.subject.as_deref() == Some(reply_subject.as_str())
                    && e.from
                        .iter()
                        .flatten()
                        .any(|f| f.email.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&b.email)))
            })
        })
        .await;
        got.map(|_| ()).ok_or_else(|| format!("автоответ не пришёл за {DELIVERY:?}"))
    }
    .await;

    // Уборка — и при сбое: автоответ как был (выключен), письма прогона удалены.
    let off = VacationResponse { is_enabled: false, ..before };
    let replies = settings(&b.client, Request::SetVacation(off)).await;
    let off_ok = matches!(replies.last(), Some(Reply::VacationSaved(v)) if !v.is_enabled);
    destroy_all(&a, &stamp).await;
    destroy_all(&b, &stamp).await;
    result.unwrap();
    assert!(off_ok, "автоответ не выключился");
    assert!(!b.client.vacation().await.unwrap().is_enabled);
}

/// S4-05: квота читается (у пользователя без ограничений — `None`), запрос без ошибки.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn quota_is_read() {
    let b = user(2).await;
    let replies = settings(&b.client, Request::Quota).await;
    match replies.as_slice() {
        [Reply::Quota(usage)] => {
            if let Some(u) = usage {
                assert!(u.limit > 0 && u.used <= u.limit.saturating_mul(2), "{u:?}");
            }
        }
        [Reply::Failed { error, .. }] => panic!("Quota/get: {error}"),
        _ => panic!("нет ответа Quota"),
    }
}

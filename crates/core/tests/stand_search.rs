//! Поиск в командной строке (S5-10) против живого Stalwart: условия запроса (`postman_core::query`) — `text`,
//! `from`, `to`, `subject`, `after`/`before`, `notKeyword $seen` — становятся `Email/query`, и сервер находит
//! ровно то, что ждёт «Везде». Что сервер делает не так — печатается и идёт строкой в `docs/STALWART-GAPS.md`.
//!
//! Два письма создаются прямо во «Входящих» user1 (`Email/set create`, без SMTP) и в конце удаляются. Без стенда
//! тест пропускается (`#[ignore]`).
//!
//! Запуск: `POSTMAN_STAND_URL`, `POSTMAN_STAND_USER1`, `POSTMAN_STAND_PASSWORD1` (и `POSTMAN_INSECURE_TLS=1`
//! напрямую), `cargo test -p postman-core --test stand_search -- --ignored --nocapture`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use postman_core::query::Query;
use postman_jmap::{Auth, CORE, Client, Filter, Invocation, MAIL};
use serde_json::json;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} не задан: стенд — docs/STALWART.md"))
}

async fn user1() -> Client {
    let auth = Auth::Basic { user: env("POSTMAN_STAND_USER1"), password: env("POSTMAN_STAND_PASSWORD1") };
    Client::connect(&env("POSTMAN_STAND_URL"), auth).await.unwrap_or_else(|e| panic!("вход user1: {e}"))
}

/// Метка прогона буквами (числа полнотекстовый индекс может резать): `qzk` + base-26 времени.
fn stamp() -> String {
    let mut t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let mut s = String::from("qzk");
    while t > 0 {
        s.push(char::from(b'a' + (t % 26) as u8));
        t /= 26;
    }
    s
}

async fn create(c: &Client, inbox: &str, from: &str, subject: &str, body: &str, received: &str, seen: bool) -> String {
    let acc = c.mail_account().unwrap().to_owned();
    let keywords = if seen { json!({ "$seen": true }) } else { json!({}) };
    let email = json!({
        "mailboxIds": { inbox: true },
        "keywords": keywords,
        "receivedAt": received,
        "from": [{ "name": "Stand Search", "email": from }],
        "to": [{ "email": env("POSTMAN_STAND_USER1") }],
        "subject": subject,
        "bodyValues": { "t": { "value": body } },
        "textBody": [{ "partId": "t", "type": "text/plain" }],
    });
    let calls = [Invocation("Email/set".into(), json!({ "accountId": acc, "create": { "n": email } }), "s".into())];
    let r = c.call(&[CORE, MAIL], &calls).await.unwrap();
    r[0].1["created"]["n"]["id"].as_str().unwrap_or_else(|| panic!("письмо не создано: {}", r[0].1)).to_owned()
}

/// Условия запроса — `Email/query` во «Входящих»: какие из писем `a`, `b` нашлись.
async fn hits(c: &Client, inbox: &str, input: &str, a: &str, b: &str) -> (bool, bool) {
    let q = Query::parse(input);
    let now = jiff::Zoned::now();
    let env = q.env(&now, |_| None, &[env("POSTMAN_STAND_USER1")]);
    let filter = q.server_filter(&env).expect("условие");
    let ids = c.query_ids(&Filter::And(vec![Filter::InMailbox(inbox.to_owned()), filter]), 100).await.unwrap();
    let found = (ids.iter().any(|x| x == a), ids.iter().any(|x| x == b));
    println!("{input:<55} → a={} b={}", found.0, found.1);
    found
}

#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn query_conditions_find_on_server() {
    let c = user1().await;
    let (boxes, _) = c.mailboxes().await.unwrap();
    let inbox = boxes.into_iter().find(|m| m.role.as_deref() == Some("inbox")).map(|m| m.id).expect("входящие");
    let run = stamp();
    let a = create(&c, &inbox, "alpha@itt.test", &format!("Postman search {run} договор ИБ"), "Итоги квартала по договору.", "2026-01-10T09:00:00Z", false).await;
    let b = create(&c, &inbox, "beta@itt.test", &format!("Postman search {run} отчёт"), "Отчёт готов.", "2026-03-10T09:00:00Z", true).await;
    // Полнотекстовый индекс Stalwart строится в фоне: подождать.
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let mut wrong = Vec::new();
    for (input, want) in [
        (format!("{run} договор"), (true, false)),
        (format!("{run} from:alpha@itt.test"), (true, false)),
        (format!("{run} from:\"Stand Search\""), (true, true)),
        (format!("{run} subject:отчёт"), (false, true)),
        (format!("{run} to:me"), (true, true)),
        (format!("{run} is:unread"), (true, false)),
        (format!("{run} after:2026-02-01"), (false, true)),
        (format!("{run} before:2026-02-01"), (true, false)),
        // Словоформа: «договоры» — «договор» в теме, «договору» в тексте.
        (format!("{run} договоры"), (true, false)),
        // Слово только в тексте письма.
        (format!("{run} квартала"), (true, false)),
    ] {
        let got = hits(&c, &inbox, &input, &a, &b).await;
        if got != want {
            wrong.push(format!("{input}: {got:?}, ждали {want:?}"));
        }
    }
    // Текст письма напрямую (`body`) и та же форма слова, что в теме, — отличить «нет словоформ» от «нет текста».
    for (what, f) in [
        ("body:квартала", Filter::Body("квартала".into())),
        ("body:Итоги", Filter::Body("Итоги".into())),
        ("text:договору", Filter::Text("договору".into())),
        ("subject:договора", Filter::Subject("договора".into())),
    ] {
        let ids = c.query_ids(&Filter::And(vec![Filter::InMailbox(inbox.clone()), Filter::Text(run.clone()), f]), 100).await.unwrap();
        println!("{what:<55} → a={}", ids.contains(&a));
    }
    c.destroy_email(&a).await.unwrap();
    c.destroy_email(&b).await.unwrap();
    println!("не так: {wrong:#?}");
    // Всё, кроме словоформ: их у Stalwart нет (`docs/STALWART-GAPS.md`), их находит свой индекс.
    assert!(wrong.iter().all(|w| w.contains("договоры")), "{wrong:#?}");
}

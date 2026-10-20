//! Клиент Postman против живого Stalwart (стенд, `docs/STALWART.md`): сессия, почта, push, приглашения,
//! занятость, контакты — тем же кодом, что и приложение. Без стенда тесты пропускаются (`#[ignore]`).
//!
//! Запуск (пароли — только в окружении, на диск не пишем):
//!   POSTMAN_STAND_URL=http://localhost:18080 POSTMAN_STAND_USER1=user1@itt.test POSTMAN_STAND_PASSWORD1=…
//!   POSTMAN_STAND_USER2=user2@itt.test POSTMAN_STAND_PASSWORD2=… cargo test -p postman-core --test stand -- --ignored

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::future::Future;
use std::time::{Duration, Instant};

use postman_calendar::{Event, NewEvent, Partstat, participant_id};
use postman_compose::{Address, Draft};
use postman_jmap::{Auth, Client, Envelope};
use serde_json::Value;

/// Сколько ждать, пока сервер доставит письмо или приглашение.
const DELIVERY: Duration = Duration::from_secs(30);

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

/// Уникальная метка прогона: письма и события разных прогонов не путаются.
fn stamp() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}-{}", t.as_secs(), t.subsec_nanos() % 1000)
}

/// Повторять `f`, пока не вернёт `Some`, не дольше `DELIVERY`.
async fn eventually<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = Instant::now();
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(start.elapsed() < DELIVERY, "не дождались за {DELIVERY:?}: {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Начало события через `days` дней, 14:00 по Москве (как в эталонной коллекции стенда).
fn start_in(days: i64) -> jiff::civil::DateTime {
    let today = jiff::Zoned::now().with_time_zone(jiff::tz::TimeZone::get("Europe/Moscow").unwrap()).date();
    today.checked_add(jiff::Span::new().days(days)).unwrap().at(14, 0, 0, 0)
}

async fn first_calendar(c: &Client) -> String {
    let mut cals = c.calendars().await.unwrap();
    cals.sort_by_key(|c| (!c.is_visible, c.sort_order));
    cals.first().expect("у пользователя нет календаря").id.clone()
}

#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn session_follows_connection_origin_and_has_capabilities() {
    let u = user(1).await;
    let s = u.client.session();
    let base = env("POSTMAN_STAND_URL");
    // Сервер объявляет свой внутренний адрес; клиент ходит туда же, куда подключился.
    for url in [&s.api_url, &s.download_url, &s.upload_url, &s.event_source_url] {
        assert!(url.starts_with(&base), "{url} не на {base}");
    }
    for cap in [postman_jmap::MAIL, postman_jmap::SUBMISSION, postman_jmap::CALENDARS, postman_jmap::CONTACTS] {
        assert!(u.client.has(cap), "нет {cap}");
    }
    assert!(u.client.has(postman_jmap::PRINCIPALS) && u.client.has(postman_jmap::PRINCIPALS_AVAILABILITY));
    assert!(u.client.has(postman_jmap::WEBSOCKET));
}

#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn mail_is_sent_received_searched_and_flagged() {
    let (a, b) = (user(1).await, user(2).await);
    let subject = format!("Postman stand {}", stamp());

    // Отправка как в приложении: письмо целиком → blob → «Черновики» → EmailSubmission → «Отправленные».
    let (boxes, _) = a.client.mailboxes().await.unwrap();
    let role = |r: &str| boxes.iter().find(|m| m.role.as_deref() == Some(r)).map(|m| m.id.clone()).unwrap();
    let identity = a.client.identities().await.unwrap().into_iter().find(|i| i.email == a.email).unwrap();
    let draft = Draft {
        from: Address::new("User 1", &a.email),
        to: vec![Address::new("User 2", &b.email)],
        subject: subject.clone(),
        markdown: "Привет со **стенда**.".into(),
        ..Default::default()
    };
    let now = jiff::Timestamp::now().as_second();
    let raw = postman_compose::build(&draft, now, &postman_compose::new_message_id(&draft.from, now)).unwrap();
    let envelope = Envelope { mail_from: a.email.clone(), rcpt_to: vec![b.email.clone()] };
    a.client.send(raw, &identity.id, &envelope, &role("drafts"), &role("sent")).await.unwrap();

    // Получатель находит его поиском во «Входящих».
    let (boxes, _) = b.client.mailboxes().await.unwrap();
    let inbox = boxes.iter().find(|m| m.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let id = eventually("письмо у получателя", || async {
        let page = b.client.search(&subject, Some(&inbox), 5).await.ok()?;
        page.emails.into_iter().find(|e| e.subject.as_deref() == Some(subject.as_str())).map(|e| e.id)
    })
    .await;
    let body = b.client.email_body(&id).await.unwrap();
    assert!(body.text.is_some() || body.html.is_some(), "у письма нет тела");

    // Флаг «прочитано» и дельта изменений.
    let (_, state) = b.client.email_get(std::slice::from_ref(&id)).await.unwrap();
    b.client.set_keyword(&id, "$seen", true).await.unwrap();
    let changes = b.client.changes("Email", &state, 50).await.unwrap();
    assert!(changes.updated.contains(&id), "Email/changes не вернул изменённое письмо");
    b.client.destroy_email(&id).await.unwrap();
}

#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn push_delivers_state_change() {
    let (a, b) = (user(1).await, user(2).await);
    let mut events = b.client.event_source(5).await.unwrap();
    // Изменение в ящике получателя: письмо от user1.
    let (boxes, _) = a.client.mailboxes().await.unwrap();
    let role = |r: &str| boxes.iter().find(|m| m.role.as_deref() == Some(r)).map(|m| m.id.clone()).unwrap();
    let identity = a.client.identities().await.unwrap().into_iter().find(|i| i.email == a.email).unwrap();
    let draft = Draft {
        from: Address::new("", &a.email),
        to: vec![Address::new("", &b.email)],
        subject: format!("Postman push {}", stamp()),
        markdown: "push".into(),
        ..Default::default()
    };
    let now = jiff::Timestamp::now().as_second();
    let raw = postman_compose::build(&draft, now, &postman_compose::new_message_id(&draft.from, now)).unwrap();
    let envelope = Envelope { mail_from: a.email.clone(), rcpt_to: vec![b.email.clone()] };
    a.client.send(raw, &identity.id, &envelope, &role("drafts"), &role("sent")).await.unwrap();
    let got = tokio::time::timeout(DELIVERY, async {
        while let Some(ev) = events.next().await.unwrap() {
            if ev.event == "state" {
                return ev;
            }
        }
        panic!("сервер закрыл EventSource");
    })
    .await
    .expect("нет события state");
    assert!(got.data.contains("changed"), "{}", got.data);
}

/// Сценарий эталонной коллекции: user1 приглашает user2 → у user2 событие (своё id, ищем по uid) →
/// user2 принимает → у user1 статус accepted → user1 удаляет → у user2 отмена.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn invitation_round_trip() {
    let (a, b) = (user(1).await, user(2).await);
    let uid = format!("postman-stand-{}@itt.test", stamp());
    let cal = first_calendar(&a.client).await;
    let attendees = [("User 2", b.email.as_str(), false)];
    let ev = NewEvent {
        uid: &uid,
        calendar_id: &cal,
        title: "Postman: проверка приглашения",
        description: "",
        location: "Переговорная 1",
        start: start_in(8),
        time_zone: "Europe/Moscow",
        duration_secs: 3600,
        organizer: ("User 1", &a.email),
        attendees: &attendees,
    };
    let my_id = a.client.create_event(ev.to_jmap(), true).await.unwrap();

    let theirs = eventually("приглашение у участника", || async { b.client.event_by_uid(&uid).await.ok().flatten() }).await;
    // id — в своём аккаунте участника (может совпасть с id организатора по значению, но это другое событие).
    let their_id = theirs["id"].as_str().unwrap().to_string();
    let parsed = Event::from_jmap(&theirs, &jiff::tz::TimeZone::UTC).unwrap();
    assert_eq!(parsed.organizer().map(|o| o.email.as_str()), Some(a.email.as_str()));

    // Ответ — тем же путём, что «Принять» в приложении (`postman_core::invite::via_server`).
    let pid = participant_id(&theirs, &b.email).expect("себя среди участников");
    b.client.reply(&their_id, &pid, postman_calendar::partstat_json(Partstat::Accepted)).await.unwrap();

    eventually("ответ у организатора", || async {
        let mine = a.client.event_by_uid(&uid).await.ok().flatten()?;
        let e = Event::from_jmap(&mine, &jiff::tz::TimeZone::UTC).ok()?;
        e.participants.iter().any(|p| p.email.eq_ignore_ascii_case(&b.email) && p.status == Partstat::Accepted).then_some(())
    })
    .await;

    a.client.destroy_event(&my_id, true).await.unwrap();
    eventually("отмена у участника", || async {
        match b.client.event_by_uid(&uid).await.ok()? {
            None => Some(()),
            Some(e) => (e["status"].as_str() == Some("cancelled")).then_some(()),
        }
    })
    .await;
    // Убрать отменённое событие у участника (без рассылки).
    if let Some(e) = b.client.event_by_uid(&uid).await.unwrap() {
        b.client.destroy_event(e["id"].as_str().unwrap(), false).await.unwrap();
    }
}

/// «Помощник планирования»: занятость коллеги видна только после того, как он открыл календарь.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn free_busy_after_sharing() {
    let (a, c) = (user(1).await, user(3).await);
    let me = a.client.principal_by_email(&a.email).await.unwrap().expect("principal user1");
    let viewer = c.client.principal_by_email(&c.email).await.unwrap().expect("principal user3");
    let cal = first_calendar(&a.client).await;
    let uid = format!("postman-busy-{}@itt.test", stamp());
    let start = start_in(9);
    let ev = NewEvent {
        uid: &uid,
        calendar_id: &cal,
        title: "Postman: занятость",
        description: "",
        location: "",
        start,
        time_zone: "Europe/Moscow",
        duration_secs: 3600,
        organizer: ("User 1", &a.email),
        attendees: &[],
    };
    let id = a.client.create_event(ev.to_jmap(), false).await.unwrap();

    let day = |d: jiff::civil::DateTime| {
        d.to_zoned(jiff::tz::TimeZone::get("Europe/Moscow").unwrap()).unwrap().timestamp().strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
    };
    let (from, to) = (day(start.date().at(0, 0, 0, 0)), day(start.date().at(23, 0, 0, 0)));
    let busy_for = |client: Client| {
        let (pid, from, to) = (me.id.clone(), from.clone(), to.clone());
        async move { client.availability(&pid, &from, &to).await.unwrap() }
    };

    a.client.share_calendar(&cal, &viewer.id, false).await.unwrap();
    let busy = eventually("занятость после шаринга", || {
        let f = busy_for(c.client.clone());
        async move { Some(f.await).filter(|b| !b.is_empty()) }
    })
    .await;
    assert!(busy.iter().any(|p| p.utc_start.starts_with(&day(start)[..13])), "нет занятости 14:00 МСК: {busy:?}");
    a.client.destroy_event(&id, false).await.unwrap();
}

#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn contacts_are_read() {
    let a = user(1).await;
    let stamp = stamp();
    let acc = a.client.account_for(postman_jmap::CONTACTS).unwrap().to_string();
    let using = [postman_jmap::CORE, postman_jmap::CONTACTS];
    let call = |name: &str, args: Value| postman_jmap::Invocation(name.into(), args, "c".into());
    let books = a.client.call(&using, &[call("AddressBook/get", serde_json::json!({ "accountId": acc, "ids": null }))]).await.unwrap();
    let book = books[0].1["list"][0]["id"].as_str().expect("адресная книга").to_string();
    let card = serde_json::json!({
        "@type": "Card", "version": "1.0", "addressBookIds": { book: true },
        "name": { "components": [{ "kind": "given", "value": "Иван" }, { "kind": "surname", "value": format!("Стендов{stamp}") }] },
        "emails": { "e1": { "address": format!("ivan.{stamp}@example.com") } }
    });
    let created = a
        .client
        .call(&using, &[call("ContactCard/set", serde_json::json!({ "accountId": acc, "create": { "n": card } }))])
        .await
        .unwrap();
    let id = created[0].1["created"]["n"]["id"].as_str().expect("карточка создана").to_string();

    let cards = a.client.contact_cards().await.unwrap();
    let c = cards.iter().find(|c| c.id == id).expect("карточка в списке");
    // Stalwart сам заполняет `name.full` по компонентам в их порядке (`isOrdered`): имя, фамилия.
    assert_eq!(c.name, format!("Иван Стендов{stamp}"));
    assert_eq!(c.emails, [format!("ivan.{stamp}@example.com")]);
    a.client
        .call(&using, &[call("ContactCard/set", serde_json::json!({ "accountId": acc, "destroy": [id] }))])
        .await
        .unwrap();
}

/// Календарь на сервере (S3-05) через поток синхронизации, как в приложении: user3 открывает user1 свою
/// занятость и занят в 14:00 → user1 командой `CreateMeeting` приглашает user3 на 16:00 → у user3 событие по uid →
/// `Events` отдаёт встречу в неделе user1 → `Availability` показывает занятость user3. Затем всё удаляется.
#[tokio::test]
#[ignore = "нужен стенд Stalwart: docs/STALWART.md"]
async fn calendar_through_sync() {
    use postman_core::schedule::{self, Meeting};
    use postman_core::sync::{self, Account, Command, Event as SyncEvent};

    let (a, c) = (user(1).await, user(3).await);
    let tz = "Europe/Moscow";
    let busy_start = start_in(10);
    let meeting_start = busy_start.date().at(16, 0, 0, 0);
    let stamp = stamp();

    // user3: открыть занятость user1 и занять 14:00–15:00.
    schedule::share_free_busy(&c.client, &a.email).await.unwrap();
    let busy = Meeting {
        uid: format!("postman-s305-busy-{stamp}@itt.test"),
        title: "Postman: занят".into(),
        location: String::new(),
        description: String::new(),
        start: busy_start,
        time_zone: tz.into(),
        duration_secs: 3600,
        attendees: Vec::new(),
    };
    let identities3 = c.client.identities().await.unwrap();
    let busy_id = schedule::create(&c.client, &identities3, &busy).await.unwrap();

    // user1 — поток синхронизации приложения.
    let account = Account {
        url: env("POSTMAN_STAND_URL"),
        auth: Auth::Basic { user: a.email.clone(), password: env("POSTMAN_STAND_PASSWORD1") },
    };
    let cache = postman_cache::Cache::open_in_memory().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let commands = sync::spawn(account, cache, None, tx, || {});
    let wait = |what: &str, pick: &dyn Fn(&SyncEvent) -> bool| -> SyncEvent {
        let start = Instant::now();
        loop {
            let left = DELIVERY.checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
            match rx.recv_timeout(left) {
                Ok(SyncEvent::Error(e)) => panic!("синхронизация: {e}"),
                Ok(ev) if pick(&ev) => return ev,
                Ok(_) => {}
                Err(e) => panic!("не дождались: {what} ({e})"),
            }
        }
    };
    let uid = format!("postman-s305-{stamp}@itt.test");
    let meeting = Meeting {
        uid: uid.clone(),
        title: "Postman: встреча S3-05".into(),
        location: "Переговорная 1".into(),
        description: String::new(),
        start: meeting_start,
        time_zone: tz.into(),
        duration_secs: 1800,
        attendees: vec![("User 3".into(), c.email.clone())],
    };
    assert!(commands.send(Command::CreateMeeting { meeting: Box::new(meeting), ticket: 1 }).is_ok());
    match wait("MeetingCreated", &|e| matches!(e, SyncEvent::MeetingCreated { .. } | SyncEvent::MeetingFailed { .. })) {
        SyncEvent::MeetingCreated { ticket, uid: got } => assert_eq!((ticket, got.as_str()), (1, uid.as_str())),
        SyncEvent::MeetingFailed { error, .. } => panic!("встреча не создана: {error}"),
        _ => unreachable!(),
    }
    eventually("приглашение у user3", || async { c.client.event_by_uid(&uid).await.ok().flatten() }).await;

    let zoned = |d: jiff::civil::DateTime| d.to_zoned(jiff::tz::TimeZone::get(tz).unwrap()).unwrap().timestamp().as_second();
    let (from, to) = (zoned(busy_start.date().at(0, 0, 0, 0)), zoned(busy_start.date().at(23, 59, 0, 0)));
    assert!(commands.send(Command::Events { from, to }).is_ok());
    let SyncEvent::EventsLoaded { events, .. } = wait("EventsLoaded", &|e| matches!(e, SyncEvent::EventsLoaded { .. })) else {
        unreachable!()
    };
    let mine = events.iter().find(|e| e.uid == uid).expect("встреча в неделе user1");
    assert_eq!((mine.span.start, mine.span.len()), (zoned(meeting_start), 1800));
    assert!(mine.participants.iter().any(|p| p.email.eq_ignore_ascii_case(&c.email)));

    assert!(commands.send(Command::Availability { ticket: 2, emails: vec![c.email.clone()], from, to }).is_ok());
    let SyncEvent::Availability { me, people, error, .. } = wait("Availability", &|e| matches!(e, SyncEvent::Availability { .. }))
    else {
        unreachable!()
    };
    assert_eq!(error, None);
    assert!(me.iter().any(|b| b.span.start == zoned(meeting_start)), "моя занятость без встречи: {me:?}");
    let theirs = people[0].1.as_ref().expect("занятость user3 после шаринга");
    assert!(theirs.iter().any(|b| b.span.start == zoned(busy_start)), "нет занятости user3 в 14:00: {theirs:?}");
    drop(commands);

    // Уборка: встреча с отменой, копия у user3, занятость user3.
    let my = a.client.event_by_uid(&uid).await.unwrap().expect("встреча у user1");
    a.client.destroy_event(my["id"].as_str().unwrap(), true).await.unwrap();
    eventually("отмена у user3", || async {
        match c.client.event_by_uid(&uid).await.ok()? {
            None => Some(()),
            Some(e) => (e["status"].as_str() == Some("cancelled")).then_some(()),
        }
    })
    .await;
    if let Some(e) = c.client.event_by_uid(&uid).await.unwrap() {
        c.client.destroy_event(e["id"].as_str().unwrap(), false).await.unwrap();
    }
    c.client.destroy_event(&busy_id, false).await.unwrap();
}

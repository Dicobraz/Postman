//! Учётная запись Exchange через плагин Postman (`postman_backend_ews`, S7-06г) против боевого Exchange компании — только
//! тестовые учётки (S7 «Проверка на боевом Exchange»). Стенда Exchange нет: без окружения тесты пропускаются
//! (`#[ignore]`), CI их не запускает.
//!
//! Правила: только тестовые учётки; письма и встречи — только им (`test_recipient`); созданное — с меткой
//! `[postman-test <прогон>]` в теме, убирается в начале и в конце (`cleanup`, только ящики из окружения); справочник и
//! занятость — только чтение; запросы — последовательно (клиент EWS — не чаще 5 в секунду), ожидание — раз в 5 с, не
//! дольше минуты. Вывод — только счётчики и коды.
//!
//! Окружение — как у `postman-ews` (`crates/ews/tests/stand.rs`): POSTMAN_EWS_URL (или Autodiscover по адресу),
//! POSTMAN_EWS_USER, POSTMAN_EWS_PASSWORD, POSTMAN_EWS_USER2/PASSWORD2 (необязательно).
//! Запуск (воркер — `cargo build -p postman-worker` заранее):
//!   cargo test -p postman-backend-ews --test stand_exchange -- --ignored --test-threads=1

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use postman_cache::Cache;
use postman_core::backend::{MailPlugin, Spawn};
use postman_core::model::{Auth, Feature, Features};
use postman_core::schedule::Meeting;
use postman_core::sync::{Account, Command, Event, Status, UnboundedSender};
use postman_ews::{Client, DeleteMode, WellKnown};

const WAIT: Duration = Duration::from_secs(60);
/// Метка всего, что создают тесты: по ней — уборка.
const TAG: &str = "[postman-test";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn need(name: &str) -> String {
    env(name).unwrap_or_else(|| panic!("{name} не задан: S7 «Проверка на боевом Exchange»"))
}

fn tag() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    format!("{TAG} {ms}]")
}

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

struct Account2 {
    user: String,
    password: String,
    email: String,
}

fn account(n: &str) -> Option<Account2> {
    let user = env(&format!("POSTMAN_EWS_USER{n}"))?;
    let password = need(&format!("POSTMAN_EWS_PASSWORD{n}"));
    let email = env(&format!("POSTMAN_EWS_EMAIL{n}")).unwrap_or_else(|| user.clone());
    // Страховка правил S7: только тестовые учётки.
    assert!(email.split('@').next().unwrap_or("").to_lowercase().contains("test"), "учётка не похожа на тестовую — стоп");
    Some(Account2 { user, password, email })
}

fn first() -> Account2 {
    account("").unwrap_or_else(|| panic!("POSTMAN_EWS_USER не задан"))
}

/// Только адреса тестовых учёток; любой другой — паника до отправки.
fn test_recipient(email: &str) -> String {
    let ok = [account(""), account("2")].into_iter().flatten().any(|a| a.email.eq_ignore_ascii_case(email));
    assert!(ok, "адресат не из тестовых учёток — отправка запрещена");
    email.to_owned()
}

async fn ews_url(a: &Account2) -> String {
    match env("POSTMAN_EWS_URL") {
        Some(u) => u,
        None => postman_ews::discover(&a.email, &postman_backend_ews::auth(&a.user, &a.password), None).await.unwrap().urls.remove(0),
    }
}

/// Созданное тестами (метка в теме) — насовсем, без рассылки, только в ящике тестовой учётки.
async fn cleanup(a: &Account2) {
    let mut c = Client::new(&ews_url(a).await, postman_backend_ews::auth(&a.user, &a.password)).unwrap();
    for folder in [WellKnown::Calendar, WellKnown::Inbox, WellKnown::Drafts, WellKnown::SentItems, WellKnown::DeletedItems] {
        if let Ok(ids) = c.find_by_subject(folder, TAG, 20).await
            && !ids.is_empty()
            && let Err(e) = c.delete(&ids, DeleteMode::HardDelete, postman_ews::Notify::None).await
        {
            eprintln!("уборка {folder:?}: {}", e.code().unwrap_or("ошибка"));
        }
    }
}

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
    commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    cache: Cache,
    features: Features,
}

impl Live {
    /// Учётка Exchange, как её заводит «Добавить аккаунт»: адрес — URL EWS, вход — логин и пароль.
    fn start(a: &Account2) -> Live {
        worker_next_to_tests();
        let url = rt().block_on(ews_url(a));
        let (tx, events) = std::sync::mpsc::channel();
        let cache = Cache::open_in_memory().unwrap();
        let account = Account { url, auth: Auth::Basic { user: a.user.clone(), password: a.password.clone() } };
        let commands = postman_backend_ews::EwsPlugin.spawn(Spawn {
            account,
            profile: None,
            cache: cache.clone(),
            index: None,
            events: tx,
            repaint: Box::new(|| {}),
            areas: Vec::new(),
        });
        let mut live = Live { commands, events, cache, features: Features::NONE };
        let Event::Features(f) = live.wait("Features", |e| matches!(e, Event::Features(_))) else { unreachable!() };
        live.features = f;
        live.wait("Synced", |e| matches!(e, Event::Status(Status::Synced)));
        live
    }

    fn send(&self, c: Command) {
        self.commands.send(c).unwrap();
    }

    fn wait(&self, what: &str, pick: impl Fn(&Event) -> bool) -> Event {
        let start = Instant::now();
        loop {
            let left = WAIT.checked_sub(start.elapsed()).unwrap_or_else(|| panic!("не дождались: {what}"));
            match self.events.recv_timeout(left) {
                Ok(Event::Error(e)) => panic!("синхронизация: {:?}", e.kind),
                Ok(Event::CalendarFailed(_)) => panic!("календарь Exchange не ответил"),
                Ok(ev) if pick(&ev) => return ev,
                Ok(_) => {}
                Err(e) => panic!("не дождались: {what} ({e})"),
            }
        }
    }

    fn inbox(&self) -> String {
        self.cache.mailboxes().unwrap().into_iter().find(|b| b.role.as_deref() == Some("inbox")).expect("«Входящие»").id
    }

    /// Письмо с меткой во «Входящих»: сверка по `Reconnect` раз в 5 с, не дольше минуты.
    fn arrived(&self, tag: &str) -> postman_cache::EmailRow {
        let start = Instant::now();
        loop {
            let found = self.cache.page(&self.inbox(), 0, 200).unwrap().into_iter().find(|r| r.subject.contains(tag));
            if let Some(r) = found {
                return r;
            }
            assert!(start.elapsed() < WAIT, "письмо не пришло за минуту");
            self.send(Command::Reconnect);
            std::thread::sleep(Duration::from_secs(5));
            while self.events.try_recv().is_ok() {}
        }
    }
}

/// Вход учётки Exchange целиком: почта, календарь, занятость и справочник; задач, контактов, правил — нет.
#[test]
#[ignore = "боевой Exchange: S7 «Проверка на боевом Exchange»"]
fn sign_in() {
    let live = Live::start(&first());
    for f in [Feature::Calendar, Feature::FreeBusy, Feature::Directory] {
        assert!(live.features.has(f), "{f:?}");
    }
    for f in [Feature::Tasks, Feature::Contacts, Feature::Rules, Feature::Autoreply] {
        assert!(!live.features.has(f), "{f:?} у Exchange не показывается");
    }
    let boxes = live.cache.mailboxes().unwrap();
    for role in ["inbox", "sent", "drafts", "trash"] {
        assert!(boxes.iter().any(|b| b.role.as_deref() == Some(role)), "папка {role}");
    }
    println!("папок: {}", boxes.len());
}

/// Почта через синхронизацию Postman: письмо себе, во «Входящих», тело, «прочитано», удаление.
#[test]
#[ignore = "боевой Exchange: S7 «Проверка на боевом Exchange»"]
fn exchange_mail_round_trip() {
    let a = first();
    rt().block_on(cleanup(&a));
    let live = Live::start(&a);
    let tag = tag();
    let me = test_recipient(&a.email);
    let draft = postman_compose::Draft {
        from: postman_compose::Address::new("", &me),
        to: vec![postman_compose::Address::new("", &me)],
        subject: format!("{tag} plugin"),
        markdown: "Проверка почты Exchange через плагин.".into(),
        ..Default::default()
    };
    live.send(Command::Send { draft: Box::new(draft), replace: None, ticket: 1, release: postman_core::model::Release::Now });
    live.wait("Sent", |e| matches!(e, Event::Sent { ticket: 1, .. }));
    let row = live.arrived(&tag);
    assert!(!row.seen && row.size > 0);

    live.send(Command::Body(row.id.clone()));
    live.wait("BodyReady", |e| matches!(e, Event::BodyReady(k) if *k == row.id));
    let body = live.cache.body(&row.id).unwrap().expect("тело");
    assert!(body.text.unwrap_or_default().contains("Проверка почты Exchange"));

    live.send(Command::Keyword { id: row.id.clone(), keyword: "$seen", on: true });
    live.send(Command::Destroy(row.id.clone()));
    live.wait("удалено", |e| matches!(e, Event::MailboxesChanged));
    rt().block_on(cleanup(&a));
    println!("почта: отправлено, пришло, тело, прочитано, удалено");
}

/// Календарь, справочник и занятость через синхронизацию: своя встреча в неделе, своя учётка в GAL, занятость.
#[test]
#[ignore = "боевой Exchange: S7 «Проверка на боевом Exchange»"]
fn exchange_calendar_and_people() {
    let a = first();
    rt().block_on(cleanup(&a));
    let live = Live::start(&a);
    let title = format!("Встреча {}", tag());
    let start = jiff::Timestamp::from_second((now() / 3600 + 48) * 3600).unwrap();
    let local = start.to_zoned(jiff::tz::TimeZone::get("Europe/Moscow").unwrap()).datetime();
    let meeting = Meeting {
        uid: String::new(),
        title: title.clone(),
        location: String::new(),
        description: String::new(),
        start: local,
        time_zone: "Europe/Moscow".into(),
        duration_secs: 1800,
        attendees: vec![],
        optional: vec![],
        from: None,
    };
    live.send(Command::CreateMeeting { meeting: Box::new(meeting), ticket: 2 });
    live.wait("MeetingCreated", |e| matches!(e, Event::MeetingCreated { ticket: 2, .. }));
    live.send(Command::Events { from: start.as_second() - 86_400, to: start.as_second() + 86_400 });
    let Event::EventsLoaded { events, .. } = live.wait("EventsLoaded", |e| matches!(e, Event::EventsLoaded { .. })) else { unreachable!() };
    let mine = events.iter().find(|e| e.title == title).expect("встреча в неделе");
    assert_eq!(mine.span.start, start.as_second());

    live.send(Command::FindPeople { text: a.email.clone() });
    let Event::PeopleFound { people, .. } = live.wait("PeopleFound", |e| matches!(e, Event::PeopleFound { .. })) else { unreachable!() };
    assert!(people.iter().any(|p| p.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(&a.email))), "своя учётка в GAL");

    let other = account("2").map_or(a.email.clone(), |b| b.email);
    live.send(Command::Availability { ticket: 3, emails: vec![other], from: now(), to: now() + 7 * 86_400, sender: None });
    let Event::Availability { people, error, .. } = live.wait("Availability", |e| matches!(e, Event::Availability { ticket: 3, .. })) else { unreachable!() };
    assert_eq!(error, None);
    assert!(people[0].1.is_some(), "занятость тестовой учётки известна");
    rt().block_on(cleanup(&a));
    println!("календарь, справочник, занятость: ок");
}

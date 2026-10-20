//! Окно новой встречи (S3-05): отдельное окно ОС, как окно письма (ADR 0002); основное окно работает.
//!
//! Модель ([`MeetingModel`]) — без gpui: поля → `postman_core::schedule::Meeting`, ошибки времени и адресов,
//! итог отправки по тикету; её проверяют тесты. View ([`MeetingWindow`]) — поля, подсказки адресов (адресная
//! книга окна письма), «Помощник планирования»: занятость участников с сервера (`Command::Availability` →
//! `Principal/getAvailability`), «нет данных» отдельно от «свободен», общие окна (`find_slots`). Отправка —
//! `Command::CreateMeeting` (сервер рассылает приглашения); итог — тостом в основном окне, встреча — в неделе.
//! Демо: занятость — демо-коллеги, отправка — «нужен сервер».

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{Enter, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_calendar::{Busy, Span, WorkHours};
use postman_compose::parse_addresses;
use postman_core::schedule::Meeting;
use postman_core::sync::{Command, Event};
use postman_demo::calendar as demo;

use crate::calendar::{
    BORDER, ERROR, FIRST_HOUR, HOURS, MUTED, add_days, busy_grid, button, button_small, common_slots, day_name,
    day_start, monday_of, slot_button, slot_label,
};
use crate::commands::meeting;
use crate::compose::{Book, accept_suggestion, current_token};
use crate::{Mailer, sys};

const LABEL_W: f32 = 96.0;
const SUGGESTIONS: usize = 6;
const SUGGEST_FROM: usize = 2;
/// Занятость спрашиваем, когда участники перестали меняться.
const QUERY_DELAY: Duration = Duration::from_millis(400);
const DURATIONS: [i64; 4] = [1800, 3600, 5400, 7200];
const DAYS: usize = 5;

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// Тикет команды окна встречи: итог синхронизации находит своё окно по нему.
fn next_ticket() -> u64 {
    NEXT_TICKET.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Field {
    Title,
    Location,
    Date,
    Start,
    Attendees,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    None,
    Sending,
    /// Дата или время не разобраны: что ввёл пользователь.
    BadTime(String),
    BadAddresses(Vec<String>),
    NoServer,
    Failed(String),
}

/// Поля окна встречи и итог отправки — без gpui.
#[derive(Debug)]
pub(crate) struct MeetingModel {
    pub(crate) title: String,
    pub(crate) location: String,
    /// «дд.мм.гггг» (или «дд.мм», «гггг-мм-дд»).
    pub(crate) date: String,
    /// «чч:мм».
    pub(crate) start: String,
    pub(crate) duration: i64,
    pub(crate) attendees: String,
    pub(crate) status: Status,
    /// Тикет отправки, итог которой ждём.
    sending: Option<u64>,
}

/// Дата: «дд.мм.гггг», «дд.мм» (год — текущий), «гггг-мм-дд».
pub(crate) fn parse_date(s: &str, today: Date) -> Option<Date> {
    let s = s.trim();
    if let Ok(d) = s.parse::<Date>() {
        return Some(d);
    }
    let parts: Vec<&str> = s.split(['.', '/']).collect();
    let n = |i: usize| parts.get(i).and_then(|p| p.trim().parse::<i16>().ok());
    let (day, month) = (n(0)?, n(1)?);
    let year = match parts.get(2) {
        None => today.year(),
        Some(_) => match n(2)? {
            y if y < 100 => 2000 + y,
            y => y,
        },
    };
    if parts.len() > 3 {
        return None;
    }
    Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// Время: «чч:мм», «чч.мм» или «чч».
pub(crate) fn parse_time(s: &str) -> Option<Time> {
    let s = s.trim();
    let (h, m) = s.split_once([':', '.']).unwrap_or((s, "0"));
    Time::new(h.trim().parse().ok()?, m.trim().parse().ok()?, 0, 0).ok()
}

/// Начало новой встречи по умолчанию: ближайшие полчаса в рабочее время, иначе 10:00 следующего рабочего дня.
pub(crate) fn default_start(now: &jiff::Zoned) -> DateTime {
    let mins = i64::from(now.hour()) * 60 + i64::from(now.minute());
    let next = (mins / 30 + 1) * 30;
    let workday = |d: Date| d.weekday().to_monday_zero_offset() < 5;
    let mut date = now.date();
    let at = |h: i64| {
        Time::new(i8::try_from(h / 60).unwrap_or(10), i8::try_from(h % 60).unwrap_or(0), 0, 0)
            .unwrap_or(Time::midnight())
    };
    if workday(date) && next <= 17 * 60 {
        return date.to_datetime(at(next.max(9 * 60)));
    }
    loop {
        date = add_days(date, 1);
        if workday(date) {
            return date.to_datetime(at(10 * 60));
        }
    }
}

impl MeetingModel {
    pub(crate) fn new(start: DateTime, duration: i64) -> Self {
        Self {
            title: String::new(),
            location: String::new(),
            date: start.strftime("%d.%m.%Y").to_string(),
            start: start.strftime("%H:%M").to_string(),
            duration,
            attendees: String::new(),
            status: Status::None,
            sending: None,
        }
    }

    pub(crate) fn field(&self, f: Field) -> &str {
        match f {
            Field::Title => &self.title,
            Field::Location => &self.location,
            Field::Date => &self.date,
            Field::Start => &self.start,
            Field::Attendees => &self.attendees,
        }
    }

    pub(crate) fn set(&mut self, f: Field, value: String) {
        let slot = match f {
            Field::Title => &mut self.title,
            Field::Location => &mut self.location,
            Field::Date => &mut self.date,
            Field::Start => &mut self.start,
            Field::Attendees => &mut self.attendees,
        };
        *slot = value;
        if !self.busy() {
            self.status = Status::None;
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.sending.is_some()
    }

    /// Локальное начало из полей «Дата» и «Начало».
    pub(crate) fn start_at(&self, today: Date) -> Option<DateTime> {
        Some(parse_date(&self.date, today)?.to_datetime(parse_time(&self.start)?))
    }

    /// Время встречи в Unix-секундах (подсветка в помощнике).
    pub(crate) fn span(&self, tz: &TimeZone, today: Date) -> Option<Span> {
        let start = self.start_at(today)?.to_zoned(tz.clone()).ok()?.timestamp().as_second();
        Some(Span::new(start, start + self.duration))
    }

    /// Распознанные участники: (имя, адрес).
    pub(crate) fn people(&self) -> Vec<(String, String)> {
        parse_addresses(&self.attendees).0.into_iter().map(|a| (a.name, a.email)).collect()
    }

    pub(crate) fn bad_addresses(&self) -> Vec<String> {
        parse_addresses(&self.attendees).1
    }

    /// Встреча для синхронизации; ошибки — до отправки, строкой в окне.
    pub(crate) fn meeting(&self, uid: String, tz: &TimeZone, today: Date) -> Result<Meeting, Status> {
        let bad = self.bad_addresses();
        if !bad.is_empty() {
            return Err(Status::BadAddresses(bad));
        }
        let start = self
            .start_at(today)
            .ok_or_else(|| Status::BadTime(format!("{} {}", self.date.trim(), self.start.trim())))?;
        // Зона без имени IANA (редко на Windows) — время в UTC: сервер его поймёт.
        let (start, time_zone) = match tz.iana_name() {
            Some(name) => (start, name.to_string()),
            None => {
                let utc = start.to_zoned(tz.clone()).map_err(|e| Status::BadTime(e.to_string()))?;
                (utc.with_time_zone(TimeZone::UTC).datetime(), "Etc/UTC".to_string())
            }
        };
        Ok(Meeting {
            uid,
            title: self.title.trim().to_string(),
            location: self.location.trim().to_string(),
            description: String::new(),
            start,
            time_zone,
            duration_secs: self.duration,
            attendees: self.people(),
        })
    }

    /// Начать отправку: команда для `Backend::send` или `None` (ошибка — в `status`).
    pub(crate) fn begin(&mut self, ticket: u64, uid: String, tz: &TimeZone, today: Date) -> Option<Command> {
        match self.meeting(uid, tz, today) {
            Ok(m) => {
                self.sending = Some(ticket);
                self.status = Status::Sending;
                Some(Command::CreateMeeting { meeting: Box::new(m), ticket })
            }
            Err(s) => {
                self.status = s;
                None
            }
        }
    }

    /// Сервера нет (демо) или синхронизация остановилась: поля остаются.
    pub(crate) fn refused(&mut self) {
        self.sending = None;
        self.status = Status::NoServer;
    }

    /// Встреча создана: окно закрывается.
    pub(crate) fn on_created(&mut self, ticket: u64) -> bool {
        let mine = self.sending == Some(ticket);
        if mine {
            self.sending = None;
            self.status = Status::None;
        }
        mine
    }

    pub(crate) fn on_failed(&mut self, ticket: u64, error: String) -> bool {
        let mine = self.sending == Some(ticket);
        if mine {
            self.sending = None;
            self.status = Status::Failed(error);
        }
        mine
    }

    /// Выбрано предложенное окно: дата и начало — из него.
    pub(crate) fn set_slot(&mut self, s: Span, tz: &TimeZone) {
        if let Ok(t) = jiff::Timestamp::from_second(s.start) {
            let z = t.to_zoned(tz.clone());
            self.date = z.strftime("%d.%m.%Y").to_string();
            self.start = z.strftime("%H:%M").to_string();
        }
        self.duration = s.len().max(1800);
    }

    /// Другой день той же недели: время остаётся.
    pub(crate) fn set_day(&mut self, date: Date) {
        self.date = date.strftime("%d.%m.%Y").to_string();
    }
}

// --- окна встречи у экрана ---

/// Встреча создана (итог для тоста в основном окне).
pub(crate) struct Created {
    pub(crate) title: String,
    pub(crate) invited: bool,
}

#[derive(Default)]
pub(crate) struct Meetings {
    windows: Vec<WeakEntity<MeetingWindow>>,
}

impl Meetings {
    /// Событие синхронизации — окнам (каждое ищет свой тикет); созданные встречи — экрану для тоста.
    pub(crate) fn on_sync(&mut self, ev: &Event, cx: &mut App) -> Vec<Created> {
        let mut created = Vec::new();
        self.windows.retain(|w| {
            w.update(cx, |m, cx| {
                if let Some(c) = m.on_sync(ev, cx) {
                    created.push(c);
                }
            })
            .is_ok()
        });
        created
    }

    /// Последнее открытое окно (снимки `--screens`).
    pub(crate) fn last(&self) -> Option<Entity<MeetingWindow>> {
        self.windows.iter().rev().find_map(WeakEntity::upgrade)
    }
}

impl Mailer {
    /// Окно новой встречи; основное окно продолжает работать.
    pub(crate) fn open_meeting(
        &mut self,
        model: Option<MeetingModel>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<MeetingWindow>> {
        self.load_address_book(cx);
        let now = jiff::Zoned::now();
        let model = model.unwrap_or_else(|| MeetingModel::new(default_start(&now), 3600));
        let open = Open {
            book: self.compose.book.clone(),
            mailer: cx.entity().downgrade(),
            live: self.backend.is_live(),
            screens: self.screens.is_some(),
        };
        let bounds = Bounds::centered(None, size(px(1080.), px(700.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(tr!("meeting-title-new").into()), ..Default::default() }),
            window_min_size: Some(size(px(720.), px(480.))),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| MeetingWindow::new(model, open, window, cx)))
        {
            Ok((_, w)) => {
                self.meetings.windows.retain(|w| w.upgrade().is_some());
                self.meetings.windows.push(w.downgrade());
                Some(w)
            }
            Err(e) => {
                tracing::error!("meeting window: {e}");
                let text = tr!("meeting-window-failed", error = e.to_string());
                self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx));
                None
            }
        }
    }

    /// Встречи созданы: тост и неделя заново.
    pub(crate) fn meetings_created(&mut self, created: Vec<Created>, cx: &mut Context<Self>) {
        for c in &created {
            let title = if c.title.is_empty() { tr!("meeting-untitled") } else { c.title.clone() };
            let text =
                if c.invited { tr!("meeting-sent", title = title) } else { tr!("meeting-created", title = title) };
            self.toasts.update(cx, |t, cx| t.push(Toast::Success, text, cx));
        }
        if !created.is_empty() {
            self.cal.update(cx, |c, cx| c.refresh(cx));
        }
    }

    /// Сцена `meeting` для `--screens`: окно с участниками демо (один — без данных) и помощником. Живой ящик —
    /// участники из `POSTMAN_SCREENS_ATTENDEES`, время по умолчанию, занятость с сервера.
    pub(crate) fn open_meeting_screen(&mut self, cx: &mut Context<Self>) {
        if self.backend.is_live() {
            let mut model = MeetingModel::new(default_start(&jiff::Zoned::now()), 3600);
            model.attendees = std::env::var("POSTMAN_SCREENS_ATTENDEES").unwrap_or_default();
            self.open_meeting(Some(model), cx);
            return;
        }
        let tz = TimeZone::system();
        let monday = jiff::Timestamp::from_second(demo::week_start())
            .map(|t| monday_of(t.to_zoned(tz).date()))
            .unwrap_or_else(|_| monday_of(jiff::Zoned::now().date()));
        let mut model = MeetingModel::new(add_days(monday, 2).at(15, 0, 0, 0), 3600);
        model.title = "Обсуждение пилота".into(); // i18n-ok: образец встречи
        model.location = "Переговорная 3".into(); // i18n-ok: образец встречи
        let people: Vec<String> = demo::colleagues().iter().take(3).map(|(n, e, _)| format!("{n} <{e}>")).collect();
        model.attendees = format!("{}; guest@partner.example; ", people.join("; "));
        self.open_meeting(Some(model), cx);
    }
}

// --- окно встречи ---

struct Open {
    book: Book,
    mailer: WeakEntity<Mailer>,
    live: bool,
    screens: bool,
}

/// Занятость для помощника: неделя и адреса запроса, ответ.
#[derive(Default)]
struct Availability {
    /// Что спрошено: понедельник недели и адреса.
    asked: Option<(Date, Vec<String>)>,
    ticket: Option<u64>,
    me: Option<Vec<Busy>>,
    people: HashMap<String, Option<Vec<Busy>>>,
    error: Option<String>,
    /// Таймер запроса запущен.
    armed: bool,
}

#[derive(Default)]
struct Screen {
    on: bool,
    frames: u32,
    hwnd: Option<isize>,
}

pub(crate) struct MeetingWindow {
    pub(crate) model: MeetingModel,
    title: Entity<InputState>,
    location: Entity<InputState>,
    date: Entity<InputState>,
    start: Entity<InputState>,
    attendees: Entity<InputState>,
    focus: FocusHandle,
    mailer: WeakEntity<Mailer>,
    book: Book,
    suggest: Vec<String>,
    suggest_on: bool,
    selected: usize,
    tz: TimeZone,
    hours: WorkHours,
    live: bool,
    busy: Availability,
    /// Поля модели поменялись не от ввода (выбрано окно, день) — переписать поля при отрисовке.
    sync_inputs: bool,
    close: bool,
    window_title: String,
    screen: Screen,
    _subscriptions: Vec<Subscription>,
}

impl MeetingWindow {
    fn new(model: MeetingModel, open: Open, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut line = |f: Field, placeholder: &str, cx: &mut Context<Self>| {
            let (value, placeholder) = (model.field(f).to_string(), tr!(placeholder));
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let title = line(Field::Title, "meeting-subject-placeholder", cx);
        let location = line(Field::Location, "meeting-location-placeholder", cx);
        let date = line(Field::Date, "meeting-date-placeholder", cx);
        let start = line(Field::Start, "meeting-start-placeholder", cx);
        let attendees = line(Field::Attendees, "meeting-attendees-placeholder", cx);
        let mut subscriptions = Vec::new();
        for (f, state) in [
            (Field::Title, &title),
            (Field::Location, &location),
            (Field::Date, &date),
            (Field::Start, &start),
            (Field::Attendees, &attendees),
        ] {
            subscriptions.push(cx.subscribe_in(state, window, move |this, state, ev: &InputEvent, _, cx| {
                let value = state.read(cx).value().to_string();
                this.on_input(f, value, ev, cx);
            }));
        }
        title.update(cx, |s, cx| s.focus(window, cx));
        let tz = TimeZone::system();
        let w = Self {
            model,
            title,
            location,
            date,
            start,
            attendees,
            focus: cx.focus_handle(),
            mailer: open.mailer,
            book: open.book,
            suggest: Vec::new(),
            suggest_on: false,
            selected: 0,
            hours: WorkHours::standard(tz.clone()),
            tz,
            live: open.live,
            busy: Availability::default(),
            sync_inputs: false,
            close: false,
            window_title: String::new(),
            screen: Screen { on: open.screens, ..Default::default() },
            _subscriptions: subscriptions,
        };
        // Окно открывается посреди обновления экрана (`Mailer`): запрос к нему — следующим шагом.
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |w, cx| w.query(cx));
        })
        .detach();
        w
    }

    /// Дескриптор окна для снимка: окно нарисовано несколько раз и занятость получена.
    pub(crate) fn screen_hwnd(&self) -> Option<isize> {
        self.screen.hwnd.filter(|_| self.screen.frames >= 3 && self.busy.asked.is_some() && self.busy.ticket.is_none())
    }

    fn today(&self) -> Date {
        jiff::Zoned::now().with_time_zone(self.tz.clone()).date()
    }

    /// Неделя встречи: понедельник по полю «Дата» (не разобрано — эта неделя).
    fn monday(&self) -> Date {
        let today = self.today();
        monday_of(parse_date(&self.model.date, today).unwrap_or(today))
    }

    fn on_input(&mut self, f: Field, value: String, ev: &InputEvent, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Change => {
                self.model.set(f, value);
                if f == Field::Attendees {
                    self.refresh_suggestions();
                }
                if matches!(f, Field::Attendees | Field::Date) {
                    self.schedule_query(cx);
                }
            }
            InputEvent::Focus => {
                self.suggest_on = f == Field::Attendees;
                self.refresh_suggestions();
            }
            _ => return,
        }
        cx.notify();
    }

    fn refresh_suggestions(&mut self) {
        self.selected = 0;
        self.suggest.clear();
        if !self.suggest_on {
            return;
        }
        let token = current_token(&self.model.attendees);
        if token.chars().count() < SUGGEST_FROM || token.contains('<') {
            return;
        }
        let now = jiff::Timestamp::now().as_second();
        self.suggest = self.book.borrow().suggest(token, SUGGESTIONS, now).into_iter().map(|e| e.display()).collect();
    }

    fn suggesting(&self) -> bool {
        self.suggest_on && !self.suggest.is_empty()
    }

    fn accept(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.suggest.get(i) else { return };
        let value = accept_suggestion(&self.model.attendees, item);
        self.attendees.update(cx, |s, cx| {
            s.set_value(value.clone(), window, cx);
            s.set_selected_range(value.len()..value.len(), cx);
            s.focus(window, cx);
        });
        self.model.set(Field::Attendees, value);
        self.refresh_suggestions();
        self.schedule_query(cx);
        cx.notify();
    }

    fn move_suggestion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if !self.suggesting() {
            return false;
        }
        self.selected = self.selected.saturating_add_signed(delta).min(self.suggest.len() - 1);
        cx.notify();
        true
    }

    /// Запрос занятости — когда участники и дата перестали меняться: не на каждую букву.
    fn schedule_query(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.busy.armed, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUERY_DELAY).await;
            let _ = this.update(cx, |w, cx| {
                w.busy.armed = false;
                w.query(cx);
            });
        })
        .detach();
    }

    /// Занятость недели встречи для участников. Сервер — `Command::Availability`; демо — демо-коллеги.
    fn query(&mut self, cx: &mut Context<Self>) {
        let monday = self.monday();
        let emails: Vec<String> = self.model.people().into_iter().map(|(_, e)| e.to_lowercase()).collect();
        let want = (monday, emails.clone());
        if self.busy.asked.as_ref() == Some(&want) {
            return;
        }
        self.busy.asked = Some(want);
        self.busy.error = None;
        if !self.live {
            let colleagues = demo::colleagues();
            self.busy.me = Some(demo::my_busy());
            self.busy.people = emails
                .into_iter()
                .map(|e| {
                    let busy =
                        colleagues.iter().find(|(_, mail, _)| mail.eq_ignore_ascii_case(&e)).map(|(_, _, b)| b.clone());
                    (e, busy)
                })
                .collect();
            self.busy.ticket = None;
            cx.notify();
            return;
        }
        let ticket = next_ticket();
        let (from, to) = (day_start(monday, &self.tz), day_start(add_days(monday, 7), &self.tz));
        let sent = self
            .mailer
            .update(cx, |m, _| m.backend.send(Command::Availability { ticket, emails, from, to }))
            .unwrap_or(false);
        self.busy.ticket = sent.then_some(ticket);
        cx.notify();
    }

    /// Строки помощника: я и участники по порядку полей; ответа ещё нет — «нет данных».
    fn rows(&self) -> Vec<(String, Option<Vec<Busy>>)> {
        let mut rows = vec![(tr!("cal-me"), self.busy.me.clone())];
        for (name, email) in self.model.people() {
            let busy = self.busy.people.get(&email.to_lowercase()).cloned().flatten();
            rows.push((if name.is_empty() { email } else { name }, busy));
        }
        rows
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.model.busy() {
            return;
        }
        let ticket = next_ticket();
        let uid = format!("postman-{}-{ticket}", jiff::Timestamp::now().as_nanosecond());
        let today = self.today();
        let Some(command) = self.model.begin(ticket, uid, &self.tz, today) else {
            cx.notify();
            return;
        };
        let accepted = self
            .mailer
            .update(cx, |m, cx| {
                let ok = m.backend.send(command);
                if !ok {
                    m.needs_server(cx);
                }
                ok
            })
            .unwrap_or(false);
        if !accepted {
            self.model.refused();
        }
        cx.notify();
    }

    /// Итог по тикету: встреча создана — окно закрывается, экран показывает тост.
    fn on_sync(&mut self, ev: &Event, cx: &mut Context<Self>) -> Option<Created> {
        match ev {
            Event::Availability { ticket, me, people, error } if self.busy.ticket == Some(*ticket) => {
                self.busy.ticket = None;
                self.busy.me = Some(me.clone());
                self.busy.people = people.iter().map(|(e, b)| (e.to_lowercase(), b.clone())).collect();
                self.busy.error = error.clone();
                cx.notify();
                None
            }
            Event::MeetingCreated { ticket, .. } if self.model.on_created(*ticket) => {
                self.close = true;
                cx.notify();
                Some(Created { title: self.model.title.trim().to_string(), invited: !self.model.people().is_empty() })
            }
            Event::MeetingFailed { ticket, error } => {
                if self.model.on_failed(*ticket, error.clone()) {
                    cx.notify();
                }
                None
            }
            _ => None,
        }
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.suggesting() {
            self.suggest.clear();
            cx.notify();
        } else {
            window.remove_window();
        }
    }

    fn pick_slot(&mut self, s: Span, cx: &mut Context<Self>) {
        self.model.set_slot(s, &self.tz);
        self.sync_inputs = true;
        cx.notify();
    }

    fn pick_day(&mut self, d: usize, cx: &mut Context<Self>) {
        self.model.set_day(add_days(self.monday(), d as i64));
        self.sync_inputs = true;
        cx.notify();
    }

    fn status_line(&self) -> Option<(String, bool)> {
        Some(match &self.model.status {
            Status::None => return None,
            Status::Sending => (tr!("meeting-sending"), false),
            Status::BadTime(t) => (tr!("meeting-bad-time", text = t.clone()), true),
            Status::BadAddresses(b) => (tr!("meeting-bad-addresses", list = b.join(", ")), true),
            Status::NoServer => (tr!("meeting-no-server"), true),
            Status::Failed(e) => (tr!("meeting-failed", error = e.clone()), true),
        })
    }

    fn label(text: String) -> Div {
        div().w(px(LABEL_W)).flex_none().text_color(rgb(MUTED)).child(text)
    }

    fn attendees_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bad = self.model.bad_addresses();
        let field = div()
            .flex_1()
            .min_w_0()
            .rounded_md()
            .border_1()
            .border_color(if bad.is_empty() { rgba(0) } else { rgb(ERROR) })
            .child(Input::new(&self.attendees))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.move_suggestion(1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.move_suggestion(-1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, a: &Enter, window, cx| {
                if !a.secondary && this.suggesting() {
                    this.accept(this.selected, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.suggesting() {
                    this.accept(this.selected, window, cx);
                    cx.stop_propagation();
                }
            }));
        let label = Self::label(tr!("meeting-attendees")).when(!bad.is_empty(), |d| d.text_color(rgb(ERROR)));
        let mut col =
            div().flex().flex_col().gap_0p5().child(div().flex().items_center().gap_2().child(label).child(field));
        if self.suggesting() {
            let items = self.suggest.iter().enumerate().map(|(i, s)| {
                div()
                    .id(("suggest", i))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(i == self.selected, |d| d.bg(rgb(0xdbe9f7)))
                    .hover(|d| d.bg(rgb(0xececec)))
                    .child(s.clone())
                    .on_click(cx.listener(move |this, _, window, cx| this.accept(i, window, cx)))
            });
            col = col.child(
                div()
                    .ml(px(LABEL_W + 8.))
                    .p_1()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .children(items),
            );
        }
        col
    }

    fn assistant(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let today = self.today();
        let monday = self.monday();
        let date = parse_date(&self.model.date, today).unwrap_or(monday);
        let day = (date.weekday().to_monday_zero_offset() as usize).min(DAYS - 1);
        let start = day_start(add_days(monday, day as i64), &self.tz);
        let range = Span::new(start + FIRST_HOUR * 3600, start + (FIRST_HOUR + HOURS) * 3600);
        let rows = self.rows();
        let no_data = rows.iter().any(|(_, b)| b.is_none());
        // Окна — с этой минуты: прошедшие дни недели не предлагаем.
        let now = jiff::Timestamp::now().as_second();
        let week = Span::new(day_start(monday, &self.tz).max(now), day_start(add_days(monday, DAYS as i64), &self.tz));
        let slots = common_slots(&rows, week, self.model.duration, &self.hours);
        let picked = self.model.span(&self.tz, today);
        let day_tabs = div().flex().gap_1().children((0..DAYS).map(|d| {
            let date = add_days(monday, d as i64);
            button_small(("day", d), format!("{} {}", day_name(d), date.strftime("%d.%m")), d == day)
                .on_click(cx.listener(move |this, _, _, cx| this.pick_day(d, cx)))
        }));
        let suggestions = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(220.))
            .flex_none()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("cal-free-for-all")))
            .when(slots.is_empty(), |d| d.child(div().text_color(rgb(MUTED)).child(tr!("cal-no-slots"))))
            .children(slots.into_iter().enumerate().map(|(n, s)| {
                slot_button(n, slot_label(s, &self.tz), picked == Some(s))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_slot(s, cx)))
            }));
        let note = if self.busy.ticket.is_some() {
            Some((tr!("meeting-busy-loading"), false))
        } else if let Some(e) = &self.busy.error {
            Some((tr!("meeting-busy-failed", error = e.clone()), true))
        } else {
            no_data.then(|| (tr!("meeting-no-data-note"), false))
        };
        div()
            .id("assistant")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .bg(rgb(0xffffff))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("meeting-assistant")))
            .child(day_tabs)
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(div().flex_1().min_w_0().child(busy_grid(&rows, range, &self.hours, picked)))
                    .child(suggestions),
            )
            .when_some(note, |d, (text, error)| {
                d.child(div().text_xs().text_color(rgb(if error { ERROR } else { MUTED })).child(text))
            })
    }
}

impl Render for MeetingWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.close {
            window.remove_window();
        }
        if std::mem::take(&mut self.sync_inputs) {
            for (f, state) in [(Field::Date, self.date.clone()), (Field::Start, self.start.clone())] {
                let value = self.model.field(f).to_string();
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
            self.schedule_query(cx);
        }
        let title =
            if self.model.title.trim().is_empty() { tr!("meeting-title-new") } else { self.model.title.clone() };
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        if self.screen.on {
            self.screen.frames += 1;
            if self.screen.hwnd.is_none() {
                self.screen.hwnd = sys::window_id(window).ok();
            }
            if self.screen.frames < 8 {
                window.request_animation_frame();
            }
        }
        let busy = self.model.busy();
        let durations = div().flex().gap_1().children(DURATIONS.map(|secs| {
            button_small(("duration", secs as usize), tr!("cal-minutes", n = secs / 60), secs == self.model.duration)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.duration = secs;
                    cx.notify();
                }))
        }));
        let row = |label: String, input: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Self::label(label))
                .child(div().flex_1().min_w_0().child(Input::new(input)))
        };
        let fields = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(row(tr!("meeting-subject"), &self.title))
            .child(row(tr!("meeting-location"), &self.location))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Self::label(tr!("meeting-date")))
                    .child(div().w(px(130.)).child(Input::new(&self.date)))
                    .child(div().text_color(rgb(MUTED)).child(tr!("meeting-start")))
                    .child(div().w(px(80.)).child(Input::new(&self.start)))
                    .child(div().text_color(rgb(MUTED)).child(tr!("meeting-duration")))
                    .child(durations),
            )
            .child(self.attendees_row(cx));
        let status = self.status_line().map(|(text, error)| {
            div()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(if error { ERROR } else { MUTED }))
                .child(text)
        });
        div()
            .key_context("Meeting")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &meeting::Send, _, cx| this.send(cx)))
            .on_action(cx.listener(|this, _: &meeting::Close, window, cx| this.escape(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xfafafa))
            .text_color(rgb(0x222222))
            .text_size(px(13.))
            .child(
                div().flex().items_center().gap_2().px_3().py_2().border_b_1().border_color(rgb(BORDER)).child(
                    button("send", tr!("meeting-send"), true)
                        .when(busy, |d| d.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                ),
            )
            .child(fields)
            .child(self.assistant(cx))
            .children(status)
    }
}

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{Command, Date, MeetingModel, Status, TimeZone, default_start, parse_date, parse_time};
    use postman_calendar::Span;

    fn today() -> Date {
        Date::constant(2026, 10, 20)
    }

    fn filled() -> MeetingModel {
        let mut m = MeetingModel::new("2026-10-21T11:00:00".parse().unwrap(), 3600);
        m.title = " Планёрка ".into();
        m.location = "Переговорная 1".into();
        m.attendees = "User 3 <user3@itt.test>; ivan@corp.example; ".into();
        m
    }

    #[test]
    fn fields_become_a_new_event() {
        let tz = TimeZone::get("Europe/Moscow").unwrap();
        let mut m = filled();
        m.duration = 1800;
        let meeting = m.meeting("u1".into(), &tz, today()).unwrap();
        assert_eq!(meeting.title, "Планёрка");
        assert_eq!(meeting.time_zone, "Europe/Moscow");
        assert_eq!(meeting.start, "2026-10-21T11:00:00".parse().unwrap());
        assert_eq!(
            meeting.attendees,
            vec![
                ("User 3".to_string(), "user3@itt.test".to_string()),
                (String::new(), "ivan@corp.example".to_string())
            ]
        );
        // Дальше — `NewEvent::to_jmap`, как его ждёт Stalwart.
        let attendees = meeting.attendee_refs();
        let v = meeting.new_event("cal", ("User 1", "user1@itt.test"), &attendees).to_jmap();
        assert_eq!(v["duration"], "PT30M");
        assert_eq!(v["timeZone"], "Europe/Moscow");
        assert_eq!(v["participants"]["a0"]["calendarAddress"], "mailto:user3@itt.test");
        assert_eq!(v["participants"]["a1"]["calendarAddress"], "mailto:ivan@corp.example");
        assert_eq!(v["locations"]["l1"]["name"], "Переговорная 1");
        // 11:00 МСК — 08:00 UTC: так встреча стоит в помощнике.
        let start = "2026-10-21T08:00:00Z".parse::<jiff::Timestamp>().unwrap().as_second();
        assert_eq!(m.span(&tz, today()), Some(Span::new(start, start + 1800)));
    }

    #[test]
    fn errors_stay_in_the_window() {
        let tz = TimeZone::UTC;
        let mut m = filled();
        m.attendees.push_str("petr@corp");
        assert!(m.begin(1, "u".into(), &tz, today()).is_none());
        assert_eq!(m.status, Status::BadAddresses(vec!["petr@corp".into()]));
        let mut m = filled();
        m.start = "25:00".into();
        assert!(m.begin(1, "u".into(), &tz, today()).is_none());
        assert!(matches!(m.status, Status::BadTime(_)));
        assert!(!m.busy());
    }

    #[test]
    fn send_goes_through_a_ticket() {
        let tz = TimeZone::UTC;
        let mut m = filled();
        let Some(Command::CreateMeeting { meeting, ticket }) = m.begin(7, "u7".into(), &tz, today()) else {
            panic!("нет команды")
        };
        assert_eq!((ticket, meeting.uid.as_str()), (7, "u7"));
        assert!(m.busy() && m.status == Status::Sending);
        assert!(!m.on_created(8));
        assert!(m.on_failed(7, "offline".into()));
        assert_eq!(m.status, Status::Failed("offline".into()));
        assert!(m.begin(9, "u9".into(), &tz, today()).is_some());
        assert!(m.on_created(9) && !m.busy());
        // Демо: сервера нет — поля остаются, строка в окне.
        assert!(m.begin(10, "u10".into(), &tz, today()).is_some());
        m.refused();
        assert_eq!(m.status, Status::NoServer);
        assert!(!m.busy());
    }

    #[test]
    fn dates_times_and_slots() {
        assert_eq!(parse_date("21.10.2026", today()), Some(Date::constant(2026, 10, 21)));
        assert_eq!(parse_date("21.10", today()), Some(Date::constant(2026, 10, 21)));
        assert_eq!(parse_date("2026-10-21", today()), Some(Date::constant(2026, 10, 21)));
        assert_eq!(parse_date("31.02.2026", today()), None);
        assert_eq!(parse_date("завтра", today()), None);
        assert_eq!(parse_time("9:30"), Some(jiff::civil::Time::constant(9, 30, 0, 0)));
        assert_eq!(parse_time("14"), Some(jiff::civil::Time::constant(14, 0, 0, 0)));
        assert_eq!(parse_time("24:00"), None);
        let tz = TimeZone::UTC;
        let mut m = filled();
        let at = "2026-10-22T15:30:00Z".parse::<jiff::Timestamp>().unwrap().as_second();
        m.set_slot(Span::new(at, at + 5400), &tz);
        assert_eq!((m.date.as_str(), m.start.as_str(), m.duration), ("22.10.2026", "15:30", 5400));
    }

    #[test]
    fn default_start_is_in_work_hours() {
        let z = |s: &str| s.parse::<jiff::civil::DateTime>().unwrap().to_zoned(TimeZone::UTC).unwrap();
        // Вторник 10:10 → 10:30 того же дня; 7:00 → 9:00; 18:00 → среда 10:00; пятница вечер → понедельник.
        assert_eq!(default_start(&z("2026-10-20T10:10:00")), "2026-10-20T10:30:00".parse().unwrap());
        assert_eq!(default_start(&z("2026-10-20T07:00:00")), "2026-10-20T09:00:00".parse().unwrap());
        assert_eq!(default_start(&z("2026-10-20T18:00:00")), "2026-10-21T10:00:00".parse().unwrap());
        assert_eq!(default_start(&z("2026-10-23T17:45:00")), "2026-10-26T10:00:00".parse().unwrap());
    }
}

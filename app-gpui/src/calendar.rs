//! Календарь (S3-05, вид — S5-11 по макету «Календарь: рабочая неделя»): командная строка раздела
//! ([`CalendarBar`]: «Создать встречу», «Сегодня», листание, диапазон), слева — месяц, мои календари, категории и
//! «Открыть мою занятость», в середине — рабочая неделя (весь день, часы 8–19, линия «сейчас», блоки событий),
//! справа — подробности события с ответом на приглашение, пересечениями и участниками.
//!
//! Неделя — демо (`postman_demo::calendar`) или с сервера (`CalendarEvent/query`, повторения разворачивает сервер,
//! команды S3-05). Цвет блока — категория события (JSCalendar `keywords`/`categories` → таблица категорий,
//! `Categories::for_event`), иначе роли `app.event.*`; ждёт ответа — пунктир, «под вопросом» — штрихованная полоса,
//! отклонённое — не показывается.
//!
//! Своя кэшируемая view: перерисовывается только по своему `notify`. С сервером говорит событиями
//! [`CalendarAction`] (`Mailer` отдаёт их в `Backend`); события синхронизации приходят в [`CalendarView::on_sync`].

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use pets_ui::theme::Metric;
use postman_calendar::{Busy, BusyKind, Event, EventStatus, Invitation, Participant, Partstat, Span, WorkHours, layout_day};
use postman_compose::{Address, Draft, parse_addresses};
use postman_core::labels::Categories;
use postman_core::sync::{Command, Event as SyncEvent};
use postman_demo::calendar as demo;

use crate::chrome::icon;
use crate::roles::{Look, Pm};

pub(crate) const FIRST_HOUR: i64 = 8;
pub(crate) const HOURS: i64 = 11;
const HOUR_PX: f32 = 60.;
/// Шаг сетки помощника и выравнивание предложенных окон.
pub(crate) const STEP: i64 = 1800;
const GUTTER: f32 = 52.;
const SIDE_W: f32 = 236.;
const DETAILS_W: f32 = 344.;
const DAYS: usize = 5;

/// Значки Segoe Fluent Icons календаря, встречи и контактов.
pub(crate) mod g {
    pub(crate) const PREV: char = '\u{E76B}';
    pub(crate) const NEXT: char = '\u{E76C}';
    pub(crate) const DOWN: char = '\u{E70D}';
    pub(crate) const ADD: char = '\u{E710}';
    pub(crate) const CALENDAR: char = '\u{E787}';
    pub(crate) const TODAY: char = '\u{E8D1}';
    pub(crate) const CLOCK: char = '\u{E823}';
    pub(crate) const REPEAT: char = '\u{E8EE}';
    pub(crate) const PIN: char = '\u{E707}';
    pub(crate) const PEOPLE: char = '\u{E716}';
    pub(crate) const PERSON: char = '\u{E77B}';
    pub(crate) const MAIL: char = '\u{E715}';
    pub(crate) const CHECK: char = '\u{E73E}';
    pub(crate) const CANCEL: char = '\u{E711}';
    pub(crate) const OVERLAP: char = '\u{E7C4}';
    pub(crate) const COPY: char = '\u{E8C8}';
    pub(crate) const EDIT: char = '\u{E70F}';
    pub(crate) const DELETE: char = '\u{E74D}';
    pub(crate) const GLOBE: char = '\u{E774}';
    pub(crate) const SEND: char = '\u{E724}';
    pub(crate) const HIDDEN: char = '\u{ED1A}';
    pub(crate) const SEARCH: char = '\u{E721}';
    pub(crate) const INFO: char = '\u{E946}';
}

/// Короткое имя дня недели: 0 — понедельник.
pub(crate) fn day_name(d: usize) -> String {
    tr!("cal-day", d = d)
}

/// Понедельник недели, в которую попадает `date`.
pub(crate) fn monday_of(date: Date) -> Date {
    date.checked_sub(jiff::Span::new().days(i64::from(date.weekday().to_monday_zero_offset()))).unwrap_or(date)
}

pub(crate) fn add_days(date: Date, n: i64) -> Date {
    date.checked_add(jiff::Span::new().days(n)).unwrap_or(date)
}

/// Начало дня в зоне `tz` (Unix-секунды): смена летнего времени учтена, сутки не всегда 86 400 с.
pub(crate) fn day_start(date: Date, tz: &TimeZone) -> i64 {
    date.to_zoned(tz.clone()).map(|z| z.timestamp().as_second()).unwrap_or(0)
}

pub(crate) fn zoned(ts: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    jiff::Timestamp::from_second(ts).ok().map(|t| t.to_zoned(tz.clone()))
}

pub(crate) fn hm(ts: i64, tz: &TimeZone) -> String {
    zoned(ts, tz).map(|z| z.strftime("%H:%M").to_string()).unwrap_or_default()
}


/// «Вторник, 20 октября».
pub(crate) fn long_day(date: Date) -> String {
    tr!(
        "cal-long-day",
        wd = tr!("cal-weekday", d = date.weekday().to_monday_zero_offset()),
        day = date.day(),
        month = tr!("cal-month-of", m = date.month())
    )
}

/// «1 ч 30 мин».
pub(crate) fn duration_text(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    match (h, m) {
        (0, m) => tr!("cal-minutes", n = m),
        (h, 0) => tr!("cal-hours", n = h),
        (h, m) => format!("{} {}", tr!("cal-hours", n = h), tr!("cal-minutes", n = m)),
    }
}

/// «МСК (UTC+3)» — зона пользователя для подписей времени.
pub(crate) fn zone_label(tz: &TimeZone) -> String {
    let now = jiff::Zoned::now().with_time_zone(tz.clone());
    let offset = now.offset().seconds();
    let (sign, abs) = if offset < 0 { ('−', -offset) } else { ('+', offset) };
    let utc = if abs % 3600 == 0 { format!("UTC{sign}{}", abs / 3600) } else { format!("UTC{sign}{}:{:02}", abs / 3600, abs % 3600 / 60) };
    let short = now.strftime("%Z").to_string();
    if short.is_empty() || short.starts_with(['+', '-']) { utc } else { format!("{short} ({utc})") }
}

/// Диапазон недели в заголовке: «19–23 октября 2026», через месяц — «29 сентября – 3 октября 2026».
pub(crate) fn range_label(first: Date, last: Date) -> String {
    let month = |d: Date| tr!("cal-month-of", m = d.month());
    if first.year() != last.year() {
        format!("{} {} {} – {} {} {}", first.day(), month(first), first.year(), last.day(), month(last), last.year())
    } else if first.month() != last.month() {
        format!("{} {} – {} {} {}", first.day(), month(first), last.day(), month(last), last.year())
    } else {
        format!("{}–{} {} {}", first.day(), last.day(), month(last), last.year())
    }
}

/// События дня `d` недели (по началу): со временем и на весь день — индексы в `events`.
pub(crate) fn day_events(events: &[Event], monday: Date, d: usize, tz: &TimeZone) -> (Vec<usize>, Vec<usize>) {
    let (from, to) = (day_start(add_days(monday, d as i64), tz), day_start(add_days(monday, d as i64 + 1), tz));
    let today = |e: &Event| e.span.start >= from && e.span.start < to;
    let (mut timed, mut all_day) = (Vec::new(), Vec::new());
    for (i, e) in events.iter().enumerate().filter(|(_, e)| today(e)) {
        if e.all_day { all_day.push(i) } else { timed.push(i) }
    }
    (timed, all_day)
}


/// Мой ответ в событии: организатор — «принято»; меня нет — `None`.
pub(crate) fn my_part<'a>(e: &'a Event, me: Option<&str>) -> Option<&'a Participant> {
    let me = me?;
    e.participants.iter().find(|p| p.email.eq_ignore_ascii_case(me))
}

/// Как показать событие в неделе.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// Принято, моё.
    Normal,
    /// Ждёт моего ответа: пунктир, без полосы.
    Pending,
    /// Под вопросом: штрихованная полоса.
    Tentative,
    /// Отклонено мной: в сетке не показываем.
    Hidden,
}

/// Вид события по моему ответу и его статусу.
pub(crate) fn event_look(e: &Event, me: Option<&str>) -> Shown {
    let mine = my_part(e, me);
    match mine.map(|p| (p.owner, p.status)) {
        Some((false, Partstat::Declined)) => Shown::Hidden,
        Some((false, Partstat::NeedsAction)) => Shown::Pending,
        Some((false, Partstat::Tentative)) => Shown::Tentative,
        _ if e.status == EventStatus::Tentative => Shown::Tentative,
        _ => Shown::Normal,
    }
}

/// Что календарь просит у экрана: сервер, вкладка встречи, окно письма, тост.
pub(crate) enum CalendarAction {
    /// Неделя с сервера (`Command::Events`).
    Load {
        from: i64,
        to: i64,
    },
    /// Открыть коллеге мою занятость (`Command::ShareFreeBusy`).
    Share(String),
    /// Ответ на приглашение из подробностей события (`Command::RespondInvite`, путь S3-03).
    Respond {
        event: Box<Event>,
        status: Partstat,
    },
    /// Письмо участникам встречи.
    Write(Vec<Address>),
    Toast(Toast, String),
}

pub(crate) struct CalendarView {
    tz: TimeZone,
    monday: Date,
    /// Месяц слева (первое число).
    month: Date,
    events: Vec<Event>,
    live: bool,
    /// Ждём неделю с сервера: её `[from, to)`; пришла другая — уже не нужна.
    pending: Option<(i64, i64)>,
    /// Показанная неделя пришла с сервера (снимки `--screens --live` ждут её).
    pub(crate) loaded: bool,
    /// Подробности события сбоку: индекс в `events`.
    selected: Option<usize>,
    /// Мой адрес: мой ответ в событиях.
    me: Option<String>,
    /// Таблица категорий: цвет событий по их меткам.
    categories: Categories,
    /// Ответ ушёл, итога нет: (uid, ответ).
    answering: Option<(String, Partstat)>,
    /// «Изменить ответ»: кнопки ответа снова видны.
    changing: bool,
    /// Снимки: «сейчас» — здесь, а не по часам (линия «сейчас» видна в рабочее время).
    pub(crate) clock: Option<i64>,
    share_open: bool,
    share: Entity<InputState>,
    /// Итог «Открыть»: текст и ошибка ли это.
    share_status: Option<(String, bool)>,
    sharing: Option<String>,
    hours: WorkHours,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CalendarAction> for CalendarView {}

impl CalendarView {
    /// Демо-неделя; живой ящик переключает [`CalendarView::go_live`].
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Пояс и рабочее время — «Настройки → Календарь» (S5-20); меняются на лету.
        let cal = crate::prefs::get(cx).calendar;
        let tz = cal.zone();
        let share = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("cal-share-placeholder")));
        let on_share = cx.subscribe_in(&share, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit_share(window, cx);
            }
        });
        let monday = zoned(demo::week_start(), &tz).map(|z| z.date()).unwrap_or_else(|| jiff::Zoned::now().date());
        let monday = monday_of(monday);
        let on_prefs = crate::prefs::observe_with(cx, |this: &mut Self, cx| this.prefs_changed(cx));
        Self {
            hours: cal.work_hours(tz.clone()),
            tz,
            monday,
            month: monday.first_of_month(),
            events: demo::my_events(),
            live: false,
            pending: None,
            loaded: false,
            selected: None,
            me: Some(postman_demo::ME.to_string()),
            categories: crate::labels::default_table(),
            answering: None,
            changing: false,
            clock: None,
            share_open: false,
            share,
            share_status: None,
            sharing: None,
            scroll: ScrollHandle::new(),
            _subscriptions: vec![on_share, on_prefs],
        }
    }

    /// «Настройки → Календарь» изменились: рабочее время — затенение сетки; пояс — неделя заново.
    fn prefs_changed(&mut self, cx: &mut Context<Self>) {
        let cal = crate::prefs::get(cx).calendar;
        let tz = cal.zone();
        let hours = cal.work_hours(tz.clone());
        if hours == self.hours && tz == self.tz {
            return;
        }
        let zone_changed = tz != self.tz;
        self.tz = tz;
        self.hours = hours;
        if zone_changed {
            self.refresh(cx);
        }
        cx.notify();
    }

    /// Живой ящик: неделя — с сервера, демо-события убираются.
    pub(crate) fn go_live(&mut self, cx: &mut Context<Self>) {
        self.live = true;
        self.events.clear();
        self.loaded = false;
        self.selected = None;
        self.me = None;
        self.monday = monday_of(self.now_zoned().date());
        self.month = self.monday.first_of_month();
        cx.notify();
    }

    fn now(&self) -> i64 {
        self.clock.unwrap_or_else(|| jiff::Timestamp::now().as_second())
    }

    fn now_zoned(&self) -> jiff::Zoned {
        zoned(self.now(), &self.tz).unwrap_or_else(|| jiff::Zoned::now().with_time_zone(self.tz.clone()))
    }

    /// Таблица категорий аккаунта (цвета событий).
    pub(crate) fn set_categories(&mut self, table: Categories, cx: &mut Context<Self>) {
        if self.categories != table {
            self.categories = table;
            cx.notify();
        }
    }

    fn range(&self) -> (i64, i64) {
        (day_start(self.monday, &self.tz), day_start(add_days(self.monday, 7), &self.tz))
    }

    /// Перечитать неделю с сервера (раздел открыт, неделя сменилась, создана встреча).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        let (from, to) = self.range();
        self.pending = Some((from, to));
        cx.emit(CalendarAction::Load { from, to });
        cx.notify();
    }

    /// Поток синхронизации не принял команду: ждать нечего.
    pub(crate) fn not_sent(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        self.answering = None;
        if self.sharing.take().is_some() {
            self.share_status = None;
        }
        cx.notify();
    }

    pub(crate) fn shift(&mut self, weeks: i64, cx: &mut Context<Self>) {
        self.go_to(add_days(self.monday, weeks * 7), cx);
    }

    pub(crate) fn today(&mut self, cx: &mut Context<Self>) {
        self.go_to(self.now_zoned().date(), cx);
    }

    fn go_to(&mut self, date: Date, cx: &mut Context<Self>) {
        let monday = monday_of(date);
        self.month = date.first_of_month();
        if monday == self.monday {
            cx.notify();
            return;
        }
        self.monday = monday;
        self.selected = None;
        self.changing = false;
        if self.live {
            self.events.clear();
            self.loaded = false;
        }
        self.refresh(cx);
        cx.notify();
    }

    pub(crate) fn on_sync(&mut self, ev: &SyncEvent, cx: &mut Context<Self>) {
        match ev {
            SyncEvent::EventsLoaded { from, to, events } if self.pending == Some((*from, *to)) => {
                self.pending = None;
                // Та же неделя перечитана (push): подробности открытого события остаются, если оно есть.
                let open = self.selected.and_then(|i| self.events.get(i)).map(|e| (e.uid.clone(), e.span.start));
                self.events = events.clone();
                self.loaded = true;
                self.selected = open.and_then(|(uid, start)| {
                    self.events.iter().position(|e| e.uid == uid && e.span.start == start)
                });
            }
            SyncEvent::Me { address, .. } => self.me = Some(address.email.clone()),
            SyncEvent::InviteAnswered { uid, status, .. } => {
                if self.answering.as_ref().is_some_and(|(u, _)| u == uid) {
                    self.answering = None;
                }
                self.changing = false;
                let me = self.me.clone();
                for e in self.events.iter_mut().filter(|e| &e.uid == uid) {
                    if let Some(p) = e.participants.iter_mut().find(|p| me.as_deref().is_some_and(|m| p.email.eq_ignore_ascii_case(m))) {
                        p.status = *status;
                    }
                }
                self.refresh(cx);
            }
            SyncEvent::InviteFailed { uid, .. } if self.answering.as_ref().is_some_and(|(u, _)| u == uid) => {
                self.answering = None;
            }
            SyncEvent::FreeBusyShared { email } => {
                self.sharing = None;
                let text = tr!("cal-share-done", email = email.clone());
                self.share_status = Some((text.clone(), false));
                cx.emit(CalendarAction::Toast(Toast::Success, text));
            }
            SyncEvent::CalendarFailed(error) => {
                self.pending = None;
                let text = tr!("cal-failed", error = error.clone());
                if self.sharing.take().is_some() {
                    self.share_status = Some((text.clone(), true));
                }
                cx.emit(CalendarAction::Toast(Toast::Error, text));
            }
            _ => return,
        }
        cx.notify();
    }

    fn submit_share(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.share.read(cx).value().to_string();
        let (ok, bad) = parse_addresses(&text);
        match (ok.as_slice(), bad.first()) {
            ([one], None) => {
                let email = one.email.clone();
                self.share_status = Some((tr!("cal-share-sending", email = email.clone()), false));
                self.sharing = Some(email.clone());
                cx.emit(CalendarAction::Share(email));
            }
            _ => {
                let shown = bad.first().cloned().unwrap_or_else(|| text.trim().to_string());
                self.share_status = Some((tr!("cal-share-bad", email = shown), true));
            }
        }
        cx.notify();
    }

    /// Подробности события с самым большим числом участников (снимок `event`).
    pub(crate) fn select_busiest(&mut self, cx: &mut Context<Self>) {
        let me = self.me.clone();
        self.selected = (0..self.events.len())
            .filter(|&i| event_look(&self.events[i], me.as_deref()) != Shown::Hidden)
            .max_by_key(|&i| (self.events[i].participants.len(), std::cmp::Reverse(i)));
        cx.notify();
    }

    /// Подробности приглашения, которое ждёт моего ответа (снимок `calendar`), иначе — самого людного события.
    pub(crate) fn select_pending(&mut self, cx: &mut Context<Self>) {
        let me = self.me.clone();
        match (0..self.events.len()).find(|&i| event_look(&self.events[i], me.as_deref()) == Shown::Pending) {
            Some(i) => {
                self.selected = Some(i);
                cx.notify();
            }
            None => self.select_busiest(cx),
        }
    }

    pub(crate) fn close_details(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        cx.notify();
    }

    /// Своя занятость с сегодня на неделю вперёд (S6-08, «Время в календаре»): загруженные события, кроме
    /// свободных и отменённых.
    pub(crate) fn busy_today(&self) -> Vec<Busy> {
        let now = self.clock.unwrap_or_else(|| jiff::Timestamp::now().as_second());
        let Some(today) = zoned(now, &self.tz).map(|z| z.date()) else { return Vec::new() };
        let (from, to) = (day_start(today, &self.tz), day_start(add_days(today, 8), &self.tz));
        let day = Span::new(from, to);
        self.events
            .iter()
            .filter(|e| e.span.overlaps(&day) && !e.all_day)
            .filter(|e| e.free_busy == postman_calendar::FreeBusy::Busy && e.status != postman_calendar::EventStatus::Cancelled)
            .map(|e| Busy {
                span: e.span,
                kind: if e.status == postman_calendar::EventStatus::Tentative { BusyKind::Tentative } else { BusyKind::Busy },
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn selected(&self) -> bool {
        self.selected.is_some()
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        self.selected = if self.selected == Some(i) { None } else { Some(i) };
        self.changing = false;
        cx.notify();
    }

    fn respond(&mut self, i: usize, status: Partstat, cx: &mut Context<Self>) {
        let Some(e) = self.events.get(i) else { return };
        if self.answering.is_some() {
            return;
        }
        self.answering = Some((e.uid.clone(), status));
        cx.emit(CalendarAction::Respond { event: Box::new(e.clone()), status });
        cx.notify();
    }

    /// Ответ не ушёл (демо: нужен сервер).
    pub(crate) fn respond_refused(&mut self, cx: &mut Context<Self>) {
        self.answering = None;
        cx.notify();
    }

    /// Цвета события: (полоса, подложка, текст) — категория по меткам или роли `app.event.*`.
    fn colors(&self, e: &Event, l: &Look) -> (Hsla, Hsla, Hsla) {
        match self.categories.for_event(&e.keywords) {
            Some(c) => {
                let c = l.cat(c.shown_color());
                (c.dot, c.bg, c.text)
            }
            None => (l.pm(Pm::EventBorder), l.pm(Pm::EventBg), l.text()),
        }
    }

    /// Имя «календаря» события в подробностях: категория или «Мой календарь».
    fn source_name(&self, e: &Event) -> String {
        self.categories.for_event(&e.keywords).map(|c| c.name.clone()).unwrap_or_else(|| tr!("cal-my-calendar"))
    }

    // --- слева: месяц, календари, категории, занятость ---

    fn side(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        let month = self.month;
        let today = self.now_zoned().date();
        let first = month.first_of_month();
        let lead = i64::from(first.weekday().to_monday_zero_offset());
        let start = add_days(first, -lead);
        let week_end = add_days(self.monday, DAYS as i64 - 1);
        let header = div()
            .flex()
            .items_center()
            .h(px(32.))
            .pl(px(8.))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr!("cal-month-title", month = tr!("cal-month", m = month.month()), year = month.year())),
            )
            .child(icon_button("month-prev", g::PREV, &l).on_click(cx.listener(|this, _, _, cx| {
                this.month = add_days(this.month.first_of_month(), -1).first_of_month();
                cx.notify();
            })))
            .child(icon_button("month-next", g::NEXT, &l).on_click(cx.listener(|this, _, _, cx| {
                this.month = add_days(this.month.last_of_month(), 1);
                cx.notify();
            })));
        let names = div().flex().pl(px(4.)).children((0..7).map(|d| {
            div().flex_1().h(px(24.)).flex().items_center().justify_center().text_size(px(11.)).text_color(l.text2()).child(day_name(d))
        }));
        let weeks = (0..6).map(|w| {
            div().flex().pl(px(4.)).children((0..7).map(|d| {
                let date = add_days(start, w * 7 + d);
                let other = date.month() != month.month();
                let in_week = date >= self.monday && date <= week_end;
                let is_today = date == today;
                let n = w * 7 + d;
                div()
                    .id(("day", n as usize))
                    .flex_1()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(in_week, |c| {
                        c.bg(l.picked())
                            .when(date == self.monday, |c| c.rounded_l(px(13.)))
                            .when(date == week_end, |c| c.rounded_r(px(13.)))
                    })
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .text_size(px(12.))
                            .when(is_today, |c| c.bg(l.accent()).text_color(l.on_accent()).font_weight(FontWeight::SEMIBOLD))
                            .when(!is_today && other, |c| c.text_color(l.text3()))
                            .when(!is_today && !other && d >= 5, |c| c.text_color(l.text2()))
                            .when(!is_today, |c| c.hover(|c| c.bg(l.hover())))
                            .child(date.day().to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(date, cx)))
            }))
        });
        let title = |key: &str| div().text_size(px(12.)).text_color(l.text2()).px(px(8.)).pt(px(10.)).pb(px(4.)).child(tr!(key));
        let check = div()
            .size(px(16.))
            .flex_none()
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .bg(l.pm(Pm::EventBorder))
            .text_color(l.on_accent())
            .child(icon(g::CHECK, 10.));
        let mine = div().h(px(32.)).px(px(8.)).flex().items_center().gap(px(10.)).child(check).child(tr!("cal-my-calendar"));
        let cats = self.categories.visible().map(|c| {
            let color = l.cat(c.shown_color());
            div()
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().size(px(10.)).flex_none().rounded(px(3.)).bg(color.dot))
                .child(div().min_w_0().truncate().child(c.name.clone()))
        });
        let share = if self.share_open {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(8.))
                .pt(px(8.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(tr!("cal-share-label")))
                .child(Input::new(&self.share))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            crate::buttons::primary("share", &l)
                                .child(tr!("cal-share-button"))
                                .on_click(cx.listener(|this, _, window, cx| this.submit_share(window, cx))),
                        )
                        .child(crate::buttons::secondary("share-cancel", &l).child(tr!("cal-share-cancel")).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.share_open = false;
                                cx.notify();
                            },
                        ))),
                )
                .when_some(self.share_status.clone(), |d, (text, error)| {
                    d.child(div().text_size(px(12.)).text_color(if error { l.error() } else { l.text2() }).child(text))
                })
                .child(div().text_size(px(11.)).line_height(px(15.)).text_color(l.text2()).child(tr!("cal-share-note")))
                .into_any_element()
        } else {
            div()
                .id("share-open")
                .h(px(30.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(l.accent_text())
                .cursor_pointer()
                .hover(|d| d.bg(l.hover()))
                .rounded(px(4.))
                .child(icon(g::ADD, 12.))
                .child(tr!("cal-share-open"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.share_open = true;
                    this.share.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                }))
                .into_any_element()
        };
        div()
            .id("cal-side")
            .w(px(SIDE_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(4.))
            .pr(px(8.))
            .pl(px(4.))
            .text_size(px(13.))
            .child(header)
            .child(div().flex().flex_col().gap(px(2.)).child(names).children(weeks))
            .child(div().h(px(1.)).mx(px(8.)).mt(px(10.)).mb(px(2.)).bg(l.border()))
            .child(title("cal-group-mine"))
            .child(mine)
            .child(title("cal-group-categories"))
            .children(cats)
            .child(div().h(px(1.)).mx(px(8.)).mt(px(10.)).mb(px(2.)).bg(l.border()))
            .child(title("cal-group-share"))
            .child(share)
    }

    // --- неделя ---

    fn week(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        let height = HOURS as f32 * HOUR_PX;
        let tz = &self.tz;
        let me = self.me.as_deref();
        let now = self.now();
        let now_z = self.now_zoned();
        let today = now_z.date();
        let now_min = i64::from(now_z.hour()) * 60 + i64::from(now_z.minute());
        let now_y = ((now_min - FIRST_HOUR * 60) as f32 / 60. * HOUR_PX).clamp(-1., height + 1.);
        let now_visible = (0. ..=height).contains(&now_y);
        let header = div()
            .h(px(44.))
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(l.border())
            .child(
                div()
                    .w(px(GUTTER))
                    .flex_none()
                    .flex()
                    .items_end()
                    .justify_end()
                    .pr(px(6.))
                    .pb(px(6.))
                    .text_size(px(11.))
                    .text_color(l.text3())
                    .child(zone_short(tz)),
            )
            .children((0..DAYS).map(|d| {
                let date = add_days(self.monday, d as i64);
                let is_today = date == today;
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .border_l_1()
                    .border_color(l.border())
                    .child(
                        div()
                            .min_w(px(26.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(18.))
                            .when(is_today, |c| {
                                c.rounded(px(13.)).bg(l.accent()).text_color(l.on_accent()).font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!is_today && date < today, |c| c.text_color(l.text2()))
                            .child(date.day().to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .when(is_today, |c| c.text_color(l.accent_text()).font_weight(FontWeight::SEMIBOLD))
                            .when(!is_today, |c| c.text_color(l.text2()))
                            .child(day_name(d)),
                    )
            }));
        // Весь день: событие от дня начала до дня конца (в пределах недели).
        let (week_from, _) = self.range();
        let all_day: Vec<AnyElement> = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.all_day && event_look(e, me) != Shown::Hidden)
            .filter_map(|(i, e)| {
                let day = |t: i64| ((t - week_from) as f64 / 86_400.).floor() as i64;
                let (d0, d1) = (day(e.span.start).max(0), day(e.span.end - 1).min(DAYS as i64 - 1));
                (d0 < DAYS as i64 && d1 >= 0 && d0 <= d1).then(|| {
                    let (bar, bg, fg) = self.colors(e, &l);
                    div()
                        .id(("all-day", i))
                        .absolute()
                        .top(px(3.))
                        .h(px(22.))
                        .left(relative(d0 as f32 / DAYS as f32))
                        .w(relative((d1 - d0 + 1) as f32 / DAYS as f32))
                        .px(px(2.))
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .pl(px(3.))
                                .pr(px(8.))
                                .bg(bg)
                                .text_color(fg)
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .overflow_hidden()
                                .child(div().w(px(3.)).h_full().flex_none().bg(bar))
                                .child(div().truncate().child(e.title.clone()))
                                .when(self.selected == Some(i), |d| d.border_1().border_color(bar)),
                        )
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
                        .into_any_element()
                })
            })
            .collect();
        let all_day_row = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(l.border())
            .child(
                div()
                    .w(px(GUTTER))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .justify_center()
                    .pr(px(6.))
                    .text_size(px(10.))
                    .line_height(px(11.))
                    .text_color(l.text3())
                    .child(tr!("cal-all-day-1"))
                    .child(tr!("cal-all-day-2")),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .h_full()
                    .children((0..DAYS).map(|d| {
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(d as f32 / DAYS as f32))
                            .w(relative(1. / DAYS as f32))
                            .border_l_1()
                            .border_color(l.border())
                    }))
                    .children(all_day),
            );
        // Время слева: часы и «сейчас».
        let times = div()
            .w(px(GUTTER))
            .flex_none()
            .relative()
            .h(px(height))
            .children((0..HOURS).map(|h| {
                let near_now = now_visible && ((FIRST_HOUR + h) * 60 - now_min).abs() < 25;
                div()
                    .absolute()
                    .top(px(h as f32 * HOUR_PX + 3.))
                    .right(px(6.))
                    .text_size(px(11.))
                    .text_color(l.text2())
                    .when(near_now, |d| d.invisible())
                    .child(format!("{:02}:00", FIRST_HOUR + h))
            }))
            .when(now_visible && self.monday <= today && today <= add_days(self.monday, DAYS as i64 - 1), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(now_y - 9.))
                        .right(px(2.))
                        .px(px(4.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(l.accent())
                        .text_color(l.on_accent())
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(hm(now, tz)),
                )
            });
        // Рабочее время — «Настройки → Календарь» (S5-20): часы до минуты, нерабочий день затенён целиком.
        let y = |t: jiff::civil::Time| {
            ((i64::from(t.hour()) * 60 + i64::from(t.minute()) - FIRST_HOUR * 60) as f32 / 60. * HOUR_PX).clamp(0., height)
        };
        let (work_start, work_end) = (y(self.hours.start), y(self.hours.end));
        let mut columns = Vec::new();
        for d in 0..DAYS {
            let date = add_days(self.monday, d as i64);
            let is_today = date == today;
            let day0 = day_start(date, tz) + FIRST_HOUR * 3600;
            let (timed, _) = day_events(&self.events, self.monday, d, tz);
            let timed: Vec<usize> = timed.into_iter().filter(|&i| event_look(&self.events[i], me) != Shown::Hidden).collect();
            let placed = layout_day(&timed.iter().map(|&i| self.events[i].span).collect::<Vec<_>>());
            let (ws, we) = if self.hours.days.contains(&date.weekday()) { (work_start, work_end) } else { (height, height) };
            let shade = [(0., ws), (we, height)].map(|(a, b)| {
                div().absolute().left_0().right_0().top(px(a)).h(px(b - a)).bg(l.pm(Pm::CalOff))
            });
            let lines = (0..HOURS).flat_map(|h| {
                let y = h as f32 * HOUR_PX;
                [
                    div().absolute().top(px(y)).left_0().right_0().h(px(1.)).bg(l.border()),
                    div().absolute().top(px(y + HOUR_PX / 2.)).left_0().right_0().h(px(1.)).bg(l.border().opacity(0.45)),
                ]
            });
            let blocks: Vec<AnyElement> = placed
                .into_iter()
                .map(|p| {
                    let i = timed[p.index];
                    let e = &self.events[i];
                    let top = ((e.span.start - day0) as f32 / 3600. * HOUR_PX).clamp(0., height - 18.) + 1.;
                    let h = (e.span.len() as f32 / 3600. * HOUR_PX).clamp(18., height - top) - 3.;
                    div()
                        .absolute()
                        .top(px(top))
                        .h(px(h))
                        .left(relative(p.column as f32 / p.columns as f32))
                        .w(relative(p.span_columns as f32 / p.columns as f32))
                        .px(px(2.))
                        .child(self.block(i, e, h, &l, cx))
                        .into_any_element()
                })
                .collect();
            let now_line = now_visible.then(|| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(now_y))
                    .h(px(if is_today { 2. } else { 1. }))
                    .bg(if is_today { l.accent() } else { l.accent().opacity(0.3) })
            });
            let now_dot = (now_visible && is_today)
                .then(|| div().absolute().left(px(-5.)).top(px(now_y - 4.)).size(px(9.)).rounded_full().bg(l.accent()));
            columns.push(
                div()
                    .flex_1()
                    .min_w_0()
                    .relative()
                    .h(px(height))
                    .border_l_1()
                    .border_color(l.border())
                    .children(shade)
                    .children(lines)
                    .children(blocks)
                    .children(now_line)
                    .children(now_dot),
            );
        }
        crate::mailer::card(&l)
            .flex_1()
            .mr(px(8.))
            .child(header)
            .child(all_day_row)
            .child(
                div()
                    .id("week-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().flex().child(times).children(columns)),
            )
            .when(self.pending.is_some() && self.events.is_empty(), |d| {
                d.child(div().px(px(14.)).py(px(6.)).text_color(l.text2()).child(tr!("cal-loading")))
            })
    }

    /// Блок события в неделе.
    fn block(&self, i: usize, e: &Event, h: f32, l: &Look, cx: &mut Context<Self>) -> Stateful<Div> {
        let tz = &self.tz;
        let look = event_look(e, self.me.as_deref());
        let (bar, bg, fg) = self.colors(e, l);
        let selected = self.selected == Some(i);
        let place = if e.location.trim().is_empty() { String::new() } else { format!(" · {}", e.location.trim()) };
        let sub = match look {
            Shown::Pending => format!("{}–{} · {}", hm(e.span.start, tz), hm(e.span.end, tz), tr!("cal-waits-answer")),
            _ => format!("{}–{}{place}", hm(e.span.start, tz), hm(e.span.end, tz)),
        };
        let stripe: AnyElement = match look {
            // Без ответа (DESIGN «Календарь»): без заливки, контур и полоса цветом календаря на 70 % и 45 %.
            Shown::Pending => div().absolute().left_0().top_0().bottom_0().w(px(4.)).bg(bar.opacity(0.45)).into_any_element(),
            Shown::Tentative => div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(4.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap(px(3.))
                .children((0..((h / 6.) as usize + 1)).map(|_| div().h(px(3.)).flex_none().bg(bar)))
                .into_any_element(),
            _ => div().absolute().left_0().top_0().bottom_0().w(px(4.)).bg(bar).into_any_element(),
        };
        div()
            .id(("event", i))
            .relative()
            .size_full()
            .overflow_hidden()
            .pl(px(9.))
            .pr(px(6.))
            .pt(px(3.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .text_size(px(12.))
            .line_height(px(15.))
            .text_color(fg)
            .when(look == Shown::Pending, |d| d.bg(l.none()).border_1().border_dashed().border_color(bar.opacity(0.7)))
            .when(look != Shown::Pending, |d| d.bg(bg))
            .when(selected, |d| d.border_2().border_color(bar).shadow_md())
            .cursor_pointer()
            .tooltip({
                let tip = format!("{}, {}–{}", e.title, hm(e.span.start, tz), hm(e.span.end, tz));
                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .child(stripe)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().min_w_0().truncate().child(e.title.clone()))
                    .when(e.recurring, |d| d.child(icon(g::REPEAT, 10.).opacity(0.8))),
            )
            .when(h >= 36., |d| d.child(div().truncate().text_size(px(11.)).opacity(0.9).child(sub)))
            .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
    }

    // --- подробности ---

    fn details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        let body = match self.selected.and_then(|i| self.events.get(i).map(|e| (i, e))) {
            Some((i, e)) => self.event_details(i, e, &l, cx).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .px(px(24.))
                .text_color(l.text2())
                .child(icon(g::CALENDAR, 28.).text_color(l.text3()))
                .child(div().text_center().child(tr!("cal-details-empty")))
                .into_any_element(),
        };
        crate::mailer::card(&l).w(px(DETAILS_W)).flex_none().child(body)
    }

    fn event_details(&self, i: usize, e: &Event, l: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let tz = &self.tz;
        let me = self.me.as_deref();
        let mine = my_part(e, me);
        let owner = mine.is_some_and(|p| p.owner) || e.organizer().is_none();
        let status = mine.map(|p| p.status);
        let (bar, _, _) = self.colors(e, l);
        let answering = self.answering.as_ref().filter(|(u, _)| *u == e.uid).map(|(_, s)| *s);
        let (chip, outlined) = match (owner, status) {
            (true, _) => (tr!("cal-chip-organizer"), false),
            (false, Some(Partstat::NeedsAction)) | (false, None) => (tr!("cal-chip-waits"), true),
            (false, Some(Partstat::Tentative)) => (tr!("cal-chip-tentative"), false),
            (false, Some(Partstat::Declined)) => (tr!("cal-chip-declined"), false),
            (false, Some(_)) => (tr!("cal-chip-accepted"), false),
        };
        let top = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(l.text2())
            .child(div().size(px(10.)).flex_none().rounded(px(3.)).bg(bar))
            .child(div().flex_1().min_w_0().truncate().child(self.source_name(e)))
            .child(
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .when(outlined, |d| d.border_1().border_color(bar).text_color(l.text()))
                    .when(!outlined, |d| d.bg(l.fill()).text_color(l.text()))
                    .child(chip),
            )
            .child(icon_button("details-close", g::CANCEL, l).on_click(cx.listener(|this, _, _, cx| this.close_details(cx))));
        let title = div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).line_height(px(26.)).child(e.title.clone());
        // Ответ на приглашение: кнопки — пока ответа нет или по «Изменить ответ».
        let invited = !owner && mine.is_some();
        let asking = invited && (status == Some(Partstat::NeedsAction) || self.changing || answering.is_some());
        let invite_block = asking.then(|| {
            let from = e.organizer().map(|o| if o.name.is_empty() { o.email.clone() } else { o.name.clone() }).unwrap_or_default();
            let button = |id: &'static str, key: &str, s: Partstat, primary: bool, cx: &mut Context<Self>| {
                let sending = answering == Some(s);
                let b = if primary { crate::buttons::primary(id, l) } else { crate::buttons::secondary(id, l) };
                b.flex_1()
                    .justify_center()
                    .when(answering.is_some(), |d| d.opacity(if sending { 1. } else { 0.5 }))
                    .child(tr!(key))
                    .on_click(cx.listener(move |this, _, _, cx| this.respond(i, s, cx)))
            };
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .mx(px(-20.))
                .px(px(20.))
                .py(px(12.))
                .bg(l.base())
                .child(div().text_size(px(12.)).text_color(l.text2()).child(tr!("cal-invited-by", name = from)))
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(button("accept", "cal-accept", Partstat::Accepted, true, cx))
                        .child(button("tentative", "cal-tentative-answer", Partstat::Tentative, false, cx))
                        .child(button("decline", "cal-decline", Partstat::Declined, false, cx)),
                )
                .when(answering.is_some(), |d| {
                    d.child(div().text_size(px(12.)).text_color(l.text2()).child(tr!("reading-invite-sending")))
                })
        });
        let answered = (invited && !asking).then(|| {
            let text = match status {
                Some(Partstat::Tentative) => tr!("cal-answered-tentative"),
                Some(Partstat::Declined) => tr!("cal-answered-declined"),
                _ => tr!("cal-answered-accepted"),
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .bg(l.fill_passive())
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    div()
                        .id("change-answer")
                        .flex_none()
                        .text_color(l.accent_text())
                        .cursor_pointer()
                        .child(tr!("cal-change-answer"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.changing = true;
                            cx.notify();
                        })),
                )
        });
        let when = if e.all_day {
            zoned(e.span.start, tz).map(|z| format!("{} · {}", long_day(z.date()), tr!("cal-all-day"))).unwrap_or_default()
        } else {
            zoned(e.span.start, tz)
                .map(|z| format!("{} · {}–{}", long_day(z.date()), hm(e.span.start, tz), hm(e.span.end, tz)))
                .unwrap_or_default()
        };
        let row = |glyph: char, main: AnyElement| {
            div()
                .flex()
                .gap(px(10.))
                .items_start()
                .child(icon(glyph, 16.).mt(px(1.)).text_color(l.text2()))
                .child(div().flex_1().min_w_0().child(main))
        };
        let my_events: Vec<Event> = self.events.clone();
        let conflicts = postman_core::invite::conflicts(e.span, &e.uid, &my_events, me);
        let conflict_text = conflicts.first().map(|c| {
            let more = conflicts.len().saturating_sub(1);
            let base = tr!("cal-conflict", title = c.title.clone(), time = format!("{}–{}", hm(c.span.start, tz), hm(c.span.end, tz)));
            if more > 0 { format!("{base} {}", tr!("cal-conflict-more", n = more)) } else { base }
        });
        let info = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pb(px(12.))
            .border_b_1()
            .border_color(l.border())
            .child(row(
                g::CLOCK,
                div()
                    .flex()
                    .flex_col()
                    .child(when)
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(l.text2())
                            .child(format!("{} · {}", duration_text(e.span.len()), zone_label(tz))),
                    )
                    .into_any_element(),
            ))
            .when(e.recurring, |d| d.child(row(g::REPEAT, div().child(tr!("cal-details-recurring")).into_any_element())))
            .when(!e.location.trim().is_empty(), |d| d.child(row(g::PIN, div().child(e.location.clone()).into_any_element())))
            .when_some(conflict_text, |d, t| {
                d.child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .items_start()
                        .px(px(8.))
                        .py(px(6.))
                        .mx(px(-8.))
                        .rounded(px(6.))
                        .bg(l.pm(Pm::ConflictBg))
                        .text_color(l.pm(Pm::ConflictFg))
                        .child(icon(g::OVERLAP, 16.).mt(px(1.)))
                        .child(div().flex_1().min_w_0().child(t)),
                )
            });
        // Участники: организатор первым, затем по имени.
        let mut people: Vec<&Participant> = e.participants.iter().filter(|p| p.attendee || p.owner).collect();
        people.sort_by_key(|p| (!p.owner, p.name.to_lowercase(), p.email.to_lowercase()));
        let my_domain = me.and_then(|m| m.split_once('@')).map(|(_, d)| d.to_lowercase());
        let count = |s: Partstat| people.iter().filter(|p| !p.owner && p.status == s).count();
        let accepted = people.iter().filter(|p| p.owner || p.status == Partstat::Accepted).count();
        let mut summary = Vec::new();
        if accepted > 0 {
            summary.push(tr!("cal-sum-accepted", n = accepted));
        }
        if count(Partstat::Tentative) > 0 {
            summary.push(tr!("cal-sum-tentative", n = count(Partstat::Tentative)));
        }
        if count(Partstat::Declined) > 0 {
            summary.push(tr!("cal-sum-declined", n = count(Partstat::Declined)));
        }
        if count(Partstat::NeedsAction) > 0 {
            summary.push(tr!("cal-sum-none", n = count(Partstat::NeedsAction)));
        }
        let list = people.iter().map(|p| {
            let name = if p.name.trim().is_empty() { p.email.clone() } else { p.name.clone() };
            let you = me.is_some_and(|m| p.email.eq_ignore_ascii_case(m));
            let shown = if you { tr!("cal-you", name = name.clone()) } else { name.clone() };
            let external = my_domain.as_ref().is_some_and(|d| {
                p.email.split_once('@').is_some_and(|(_, pd)| !pd.eq_ignore_ascii_case(d))
            });
            let note = if p.owner {
                tr!("cal-organizer")
            } else if p.optional {
                tr!("cal-optional")
            } else {
                String::new()
            };
            let status = if p.owner { Partstat::Accepted } else { p.status };
            div()
                .h(px(34.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(avatar(&name, 28., l))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .when(status == Partstat::Declined, |d| d.line_through().text_color(l.text3()))
                                .child(shown),
                        )
                        .when(!note.is_empty(), |d| d.child(div().truncate().text_size(px(11.)).text_color(l.text2()).child(note))),
                )
                .when(external, |d| d.child(chip_outline(tr!("cal-external"), l)))
                .child(status_glyph(status, l))
        });
        let write: Vec<Address> = people
            .iter()
            .filter(|p| !me.is_some_and(|m| p.email.eq_ignore_ascii_case(m)) && !p.email.is_empty())
            .map(|p| Address::new(&p.name, &p.email))
            .collect();
        let has_people = people.len() > 1;
        div()
            .id("event-details")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(20.))
            .pt(px(18.))
            .pb(px(16.))
            .child(top)
            .child(title)
            .children(invite_block)
            .children(answered)
            .child(info)
            .when(has_people, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr!("cal-participants-n", n = people.len())),
                        )
                        .child(div().text_size(px(12.)).text_color(l.text2()).truncate().child(summary.join(", "))),
                )
                .child(div().flex().flex_col().children(list))
            })
            .when(!e.description.trim().is_empty(), |d| {
                d.child(
                    div()
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(l.border())
                        .line_height(px(19.))
                        .child(e.description.trim().to_string()),
                )
            })
            .when(!write.is_empty() && has_people, |d| {
                d.child(div().flex_1()).child(
                    crate::buttons::secondary("write-all", l)
                        .justify_center()
                        .child(icon(g::MAIL, 14.))
                        .child(tr!("cal-write-participants"))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(CalendarAction::Write(write.clone())))),
                )
            })
    }
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .min_w_0()
            .text_size(px(13.))
            .child(self.side(cx))
            .child(self.week(cx))
            .child(self.details(cx))
    }
}

/// «МСК» для шапки недели (короткое имя зоны или смещение).
fn zone_short(tz: &TimeZone) -> String {
    let now = jiff::Zoned::now().with_time_zone(tz.clone());
    let s = now.strftime("%Z").to_string();
    if s.is_empty() || s.starts_with(['+', '-']) { now.strftime("%:z").to_string() } else { s }
}

// --- командная строка раздела ---

/// Командная строка календаря (макет: на месте строки почты): «Создать встречу», «Сегодня», ‹ ›, диапазон недели.
pub(crate) struct CalendarBar {
    cal: Entity<CalendarView>,
    _subscription: Subscription,
}

impl CalendarBar {
    pub(crate) fn new(cal: Entity<CalendarView>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&cal, |_, _, cx| cx.notify());
        Self { cal, _subscription: sub }
    }
}

impl Render for CalendarBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        let c = self.cal.read(cx);
        let range = range_label(c.monday, add_days(c.monday, DAYS as i64 - 1));
        let loading = c.pending.is_some();
        let tip = |text: String| move |window: &mut Window, cx: &mut App| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx);
        let cal = self.cal.clone();
        let nav = |id: &'static str, glyph: char, weeks: i64, text: String| {
            let cal = cal.clone();
            crate::chrome::quiet_button(id, &l)
                .w(px(32.))
                .justify_center()
                .px_0()
                .child(icon(glyph, 14.))
                .tooltip(tip(text))
                .on_click(move |_, _, cx| cal.update(cx, |c, cx| c.shift(weeks, cx)))
        };
        let today = {
            let cal = self.cal.clone();
            crate::chrome::quiet_button("cal-today", &l)
                .child(icon(g::TODAY, 16.))
                .child(tr!("cal-today"))
                .tooltip(tip(tr!("cal-today-tip")))
                .on_click(move |_, _, cx| cal.update(cx, |c, cx| c.today(cx)))
        };
        let new = div()
            .id("cal-new")
            .h(px(32.))
            .px(px(14.))
            .mr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(l.px(Metric::RadiusControl))
            .bg(l.accent())
            .text_color(l.on_accent())
            .cursor_pointer()
            .hover(|d| d.bg(l.accent_hover()))
            .child(icon(g::ADD, 14.))
            .child(tr!("cal-new-meeting"))
            .tooltip(tip(crate::keytips::with_key(tr!("cal-new-meeting-tip"), crate::keytips::key("calendar.new_meeting", cx))))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::commands::calendar::NewMeeting), cx));
        div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(12.))
            .text_color(l.text())
            .child(new)
            .child(today)
            .child(nav("cal-prev", g::PREV, -1, tr!("cal-prev-week-tip")))
            .child(nav("cal-next", g::NEXT, 1, tr!("cal-next-week-tip")))
            .child(div().px(px(8.)).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(range))
            .when(loading, |d| d.child(div().text_color(l.text2()).child(tr!("cal-loading"))))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded(px(6.))
                    .bg(l.fill())
                    .child(
                        div()
                            .h(px(28.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .bg(l.panel())
                            .font_weight(FontWeight::SEMIBOLD)
                            .shadow_sm()
                            .child(tr!("cal-work-week")),
                    ),
            )
    }
}

impl crate::Mailer {
    /// Просьба календаря: команда синхронизации, вкладка встречи, письмо или тост. Нет сервера — «нужен сервер».
    pub(crate) fn on_calendar(&mut self, action: &CalendarAction, cx: &mut Context<Self>) {
        let command = match action {
            CalendarAction::Load { from, to } => Command::Events { from: *from, to: *to },
            CalendarAction::Share(email) => Command::ShareFreeBusy { email: email.clone() },
            CalendarAction::Respond { event, status } => Command::RespondInvite {
                invite: Box::new(Invitation {
                    method: "REQUEST".into(),
                    sequence: event.sequence,
                    raw: serde_json::json!({ "uid": event.uid }),
                    event: (**event).clone(),
                }),
                status: *status,
                in_reply_to: None,
            },
            CalendarAction::Write(to) => {
                self.open_compose(Draft { to: to.clone(), ..Default::default() }, cx);
                return;
            }
            CalendarAction::Toast(level, text) => {
                self.toasts.update(cx, |t, cx| t.push(*level, text.clone(), cx));
                return;
            }
        };
        let respond = matches!(action, CalendarAction::Respond { .. });
        if !self.backend.send(command) {
            self.needs_server(cx);
            self.cal.update(cx, |c, cx| if respond { c.respond_refused(cx) } else { c.not_sent(cx) });
        }
    }

    /// Листание недель (`Some(±1)`) и «Сегодня» (`None`) — в разделе «Календарь».
    pub(crate) fn calendar_week(&mut self, weeks: Option<i64>, cx: &mut Context<Self>) {
        if self.section != crate::Section::Calendar {
            return;
        }
        self.cal.update(cx, |c, cx| match weeks {
            Some(n) => c.shift(n, cx),
            None => c.today(cx),
        });
    }
}

// --- общие элементы раздела (календарь, встреча, контакты) ---

/// Кнопка-значок 28 px без рамки.
pub(crate) fn icon_button(id: impl Into<ElementId>, glyph: char, l: &Look) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(l.px(Metric::RadiusControl))
        .text_color(l.text2())
        .cursor_pointer()
        .hover(|d| d.bg(l.hover()))
        .child(icon(glyph, 12.))
}


/// Чип с рамкой («Внешний»).
pub(crate) fn chip_outline(text: String, l: &Look) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .rounded(px(4.))
        .border_1()
        .border_color(l.stroke())
        .text_size(px(11.))
        .text_color(l.text2())
        .child(text)
}

/// Буквы имени для кружка: «Иванова Мария» → «ИМ», адрес → первая буква.
pub(crate) fn initials(name: &str) -> String {
    let name = name.split('@').next().unwrap_or(name);
    let name = if name.contains(' ') { name } else { name.split('.').next_back().unwrap_or(name) };
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |w: &str| w.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => String::new(),
        [one] => first(one),
        [a, b, ..] => first(a) + &first(b),
    }
}

/// Кружок с буквами имени.
pub(crate) fn avatar(name: &str, size: f32, l: &Look) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(l.fill())
        .text_color(l.text2())
        .text_size(px((size * 0.4).round().max(10.)))
        .child(initials(name))
}

/// Значок ответа участника: принял, под вопросом, отклонил, нет ответа.
pub(crate) fn status_glyph(s: Partstat, l: &Look) -> Div {
    let base = div().size(px(20.)).flex_none().rounded_full().flex().items_center().justify_center();
    match s {
        Partstat::Accepted | Partstat::Delegated => {
            base.bg(l.success()).text_color(l.on_accent()).child(icon(g::CHECK, 10.))
        }
        Partstat::Tentative => base.bg(l.pm(Pm::FbBusyLine)).text_color(l.on_accent()).text_size(px(12.)).font_weight(FontWeight::BOLD).child("?"),
        Partstat::Declined => base.bg(l.text3()).text_color(l.on_accent()).child(icon(g::CANCEL, 9.)),
        Partstat::NeedsAction => base.border_2().border_dashed().border_color(l.text3()),
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{Date, Event, Shown, TimeZone, WorkHours, add_days, day_events, day_start, event_look, initials, monday_of, range_label};
    use postman_calendar::{Busy, BusyKind, Cell, Span, grid};

    fn ts(s: &str) -> i64 {
        s.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    #[test]
    fn server_events_land_on_their_days() {
        let tz = TimeZone::get("Europe/Moscow").unwrap();
        let v = |id: &str, start: &str, end: &str, all_day: bool| serde_json::json!({ "id": id, "title": id, "utcStart": start, "utcEnd": end, "showWithoutTime": all_day });
        let values = [
            v("mon", "2026-10-19T07:00:00Z", "2026-10-19T08:00:00Z", false),
            // 23:30 МСК понедельника — ещё понедельник, хотя в UTC уже 20:30 того же дня.
            v("late", "2026-10-19T20:30:00Z", "2026-10-19T21:00:00Z", false),
            v("wed-all", "2026-10-20T21:00:00Z", "2026-10-21T21:00:00Z", true),
            v("sun", "2026-10-25T09:00:00Z", "2026-10-25T10:00:00Z", false),
            v("prev", "2026-10-16T09:00:00Z", "2026-10-16T10:00:00Z", false),
        ];
        let events: Vec<Event> = postman_core::schedule::parse_events(&values, &tz);
        let monday = monday_of(Date::constant(2026, 10, 22));
        assert_eq!(monday, Date::constant(2026, 10, 19));
        let ids = |d| {
            let (timed, all) = day_events(&events, monday, d, &tz);
            (
                timed.iter().map(|&i| events[i].id.as_str()).collect::<Vec<_>>(),
                all.iter().map(|&i| events[i].id.as_str()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(ids(0), (vec!["mon", "late"], vec![]));
        assert_eq!(ids(1), (vec![], vec![]));
        assert_eq!(ids(2), (vec![], vec!["wed-all"]));
        // Выходные и прошлая неделя в рабочую неделю не попадают.
        let shown: usize = (0..5).map(|d| ids(d).0.len() + ids(d).1.len()).sum();
        assert_eq!(shown, 3);
        assert_eq!(day_start(add_days(monday, 7), &tz) - day_start(monday, &tz), 7 * 86_400);
    }

    #[test]
    fn no_data_is_not_free() {
        let tz = TimeZone::UTC;
        let hours = WorkHours::standard(tz.clone());
        let day = ts("2026-10-19T00:00:00Z");
        let range = Span::new(day + 8 * 3600, day + 20 * 3600);
        let busy = |h: i64| Busy { span: Span::new(day + h * 3600, day + (h + 1) * 3600), kind: BusyKind::Busy };
        let people = [
            ("me".to_string(), Some(vec![busy(9)])),
            ("shared".to_string(), Some(vec![busy(10)])),
            ("not shared".to_string(), None),
        ];
        let cells = grid(&people.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(), range, 1800, &hours);
        // 09:00 у меня занято, у коллеги без шаринга — «нет данных» во всех ячейках, не «свободно».
        assert_eq!(cells[0][2], Cell::Busy);
        assert_eq!(cells[1][2], Cell::Free);
        assert!(cells[2].iter().all(|c| *c == Cell::Unknown));
    }

    /// Вид блока по моему ответу: жду ответа — пунктир, «под вопросом» — штрих, отклонено — скрыто; организатор —
    /// обычный.
    #[test]
    fn event_look_follows_my_answer() {
        let ev = |mine: &str, owner: bool| {
            let v = serde_json::json!({ "utcStart": "2026-10-19T07:00:00Z", "utcEnd": "2026-10-19T08:00:00Z", "participants": {
                "o": { "calendarAddress": "mailto:boss@corp.example", "roles": { "owner": !owner } },
                "m": { "calendarAddress": "mailto:me@corp.example", "roles": { "attendee": true, "owner": owner }, "participationStatus": mine }
            }});
            Event::from_jmap(&v, &TimeZone::UTC).unwrap()
        };
        let me = Some("ME@corp.example");
        assert_eq!(event_look(&ev("needs-action", false), me), Shown::Pending);
        assert_eq!(event_look(&ev("tentative", false), me), Shown::Tentative);
        assert_eq!(event_look(&ev("declined", false), me), Shown::Hidden);
        assert_eq!(event_look(&ev("accepted", false), me), Shown::Normal);
        assert_eq!(event_look(&ev("needs-action", true), me), Shown::Normal, "своя встреча");
        assert_eq!(event_look(&ev("needs-action", false), None), Shown::Normal, "адрес не известен");
        let mut t = ev("accepted", false);
        t.status = postman_calendar::EventStatus::Tentative;
        assert_eq!(event_look(&t, me), Shown::Tentative);
    }

    #[test]
    fn labels_for_people_and_weeks() {
        assert_eq!(initials("Иванова Мария"), "ИМ");
        assert_eq!(initials("m.ivanova@corp.example"), "I");
        assert_eq!(initials(""), "");
        assert_eq!(initials("Переговорная 3"), "П3");
        let d = |m, day| Date::constant(2026, m, day);
        assert!(range_label(d(10, 19), d(10, 23)).starts_with("19–23 "));
        assert!(range_label(d(9, 28), d(10, 2)).starts_with("28 "));
    }
}

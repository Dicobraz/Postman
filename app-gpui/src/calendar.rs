//! Календарь (S3-05): рабочая неделя — демо (`postman_demo::calendar`) или с сервера (`CalendarEvent/query`,
//! повторения разворачивает сервер), листание недель, подробности события сбоку, открытие своей занятости коллеге
//! и (в демо) «Помощник планирования» под неделей. Окно новой встречи с помощником — `meeting.rs`.
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
use postman_calendar::{Busy, Cell, Event, EventStatus, Partstat, Span, WorkHours, find_slots, grid, layout_day};
use postman_compose::parse_addresses;
use postman_core::sync::{Command, Event as SyncEvent};
use postman_demo::calendar as demo;

pub(crate) const FIRST_HOUR: i64 = 8;
pub(crate) const HOURS: i64 = 12;
const HOUR_PX: f32 = 44.;
/// Шаг сетки помощника и выравнивание предложенных окон.
pub(crate) const STEP: i64 = 1800;
const CELL_PX: f32 = 26.;
const NAME_W: f32 = 170.;
const DETAILS_W: f32 = 300.;
const DAYS: usize = 5;
pub(crate) const BORDER: u32 = 0xdddddd;
pub(crate) const MUTED: u32 = 0x666666;
pub(crate) const ERROR: u32 = 0xc42b1c;
pub(crate) const ACCENT: u32 = 0x0f6cbd;
pub(crate) const PICKED: u32 = 0xcce4ff;
pub(crate) const BUTTON: u32 = 0xececec;

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

fn zoned(ts: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    jiff::Timestamp::from_second(ts).ok().map(|t| t.to_zoned(tz.clone()))
}

pub(crate) fn hm(ts: i64, tz: &TimeZone) -> String {
    zoned(ts, tz).map(|z| z.strftime("%H:%M").to_string()).unwrap_or_default()
}

/// «Пн 20.10».
pub(crate) fn day_label(ts: i64, tz: &TimeZone) -> String {
    zoned(ts, tz)
        .map(|z| format!("{} {}", day_name(z.weekday().to_monday_zero_offset() as usize), z.strftime("%d.%m")))
        .unwrap_or_default()
}

/// «Пн 20.10 10:00–11:00» — предложенное окно, время события.
pub(crate) fn slot_label(s: Span, tz: &TimeZone) -> String {
    format!("{} {}–{}", day_label(s.start, tz), hm(s.start, tz), hm(s.end, tz))
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

/// Общие окна по тем, чья занятость известна: «нет данных» (`None`) не значит «свободен» и в подбор не входит.
pub(crate) fn common_slots(
    people: &[(String, Option<Vec<Busy>>)],
    range: Span,
    duration: i64,
    hours: &WorkHours,
) -> Vec<Span> {
    let known: Vec<Vec<Busy>> = people.iter().filter_map(|(_, b)| b.clone()).collect();
    find_slots(&known, &[], range, duration, STEP, hours, 8)
}

/// Что календарь просит у экрана: сервер, окно встречи, тост.
pub(crate) enum CalendarAction {
    /// Неделя с сервера (`Command::Events`).
    Load {
        from: i64,
        to: i64,
    },
    /// Открыть коллеге мою занятость (`Command::ShareFreeBusy`).
    Share(String),
    NewMeeting,
    Toast(Toast, String),
}

pub(crate) struct CalendarView {
    tz: TimeZone,
    monday: Date,
    events: Vec<Event>,
    live: bool,
    /// Ждём неделю с сервера (её начало); пришла другая — уже не нужна.
    pending: Option<i64>,
    /// Показанная неделя пришла с сервера (снимки `--screens --live` ждут её).
    pub(crate) loaded: bool,
    /// Подробности события сбоку: индекс в `events`.
    selected: Option<usize>,
    /// Демо: помощник под неделей. Участники: (имя, занятость); первый — я.
    pub(crate) assistant: bool,
    people: Vec<(String, Option<Vec<Busy>>)>,
    day: usize,
    duration: i64,
    slot: Option<Span>,
    hours: WorkHours,
    share: Entity<InputState>,
    /// Итог «Открыть»: текст и ошибка ли это.
    share_status: Option<(String, bool)>,
    sharing: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CalendarAction> for CalendarView {}

impl CalendarView {
    /// Демо-неделя; живой ящик переключает [`CalendarView::go_live`].
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tz = TimeZone::system();
        let mut people = vec![(tr!("cal-me"), Some(demo::my_busy()))];
        people.extend(demo::colleagues().into_iter().map(|(n, _, b)| (n.to_string(), Some(b))));
        let share = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("cal-share-placeholder")));
        let on_share = cx.subscribe_in(&share, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit_share(window, cx);
            }
        });
        let monday = zoned(demo::week_start(), &tz).map(|z| z.date()).unwrap_or_else(|| jiff::Zoned::now().date());
        Self {
            hours: WorkHours::standard(tz.clone()),
            tz,
            monday: monday_of(monday),
            events: demo::my_events(),
            live: false,
            pending: None,
            loaded: false,
            selected: None,
            assistant: false,
            people,
            day: 0,
            duration: 3600,
            slot: None,
            share,
            share_status: None,
            sharing: None,
            _subscriptions: vec![on_share],
        }
    }

    /// Живой ящик: неделя — с сервера, демо-события и демо-помощник убираются.
    pub(crate) fn go_live(&mut self, cx: &mut Context<Self>) {
        self.live = true;
        self.events.clear();
        self.people.clear();
        self.assistant = false;
        self.loaded = false;
        self.selected = None;
        self.monday = monday_of(jiff::Zoned::now().with_time_zone(self.tz.clone()).date());
        cx.notify();
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
        self.pending = Some(from);
        cx.emit(CalendarAction::Load { from, to });
        cx.notify();
    }

    /// Поток синхронизации не принял команду: ждать нечего.
    pub(crate) fn not_sent(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        if self.sharing.take().is_some() {
            self.share_status = None;
        }
        cx.notify();
    }

    pub(crate) fn shift(&mut self, weeks: i64, cx: &mut Context<Self>) {
        self.monday = add_days(self.monday, weeks * 7);
        self.after_move(cx);
    }

    pub(crate) fn today(&mut self, cx: &mut Context<Self>) {
        self.monday = monday_of(jiff::Zoned::now().with_time_zone(self.tz.clone()).date());
        self.after_move(cx);
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.slot = None;
        if self.live {
            self.events.clear();
            self.loaded = false;
        }
        self.refresh(cx);
        cx.notify();
    }

    pub(crate) fn on_sync(&mut self, ev: &SyncEvent, cx: &mut Context<Self>) {
        match ev {
            SyncEvent::EventsLoaded { from, events, .. } if self.pending == Some(*from) => {
                self.pending = None;
                self.events = events.clone();
                self.loaded = true;
                self.selected = None;
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

    fn slots(&self) -> Vec<Span> {
        let (from, _) = self.range();
        common_slots(
            &self.people,
            Span::new(from, day_start(add_days(self.monday, DAYS as i64), &self.tz)),
            self.duration,
            &self.hours,
        )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let last = add_days(self.monday, DAYS as i64 - 1);
        let range = tr!(
            "cal-week-range",
            from = self.monday.strftime("%d.%m").to_string(),
            to = last.strftime("%d.%m.%Y").to_string()
        );
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("cal-work-week")))
            .child(
                button("prev-week", tr!("cal-prev-week"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.shift(-1, cx))),
            )
            .child(button("today", tr!("cal-today"), false).on_click(cx.listener(|this, _, _, cx| this.today(cx))))
            .child(
                button("next-week", tr!("cal-next-week"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.shift(1, cx))),
            )
            .child(range)
            .when(self.pending.is_some(), |d| d.child(div().text_color(rgb(MUTED)).child(tr!("cal-loading"))))
            .child(div().flex_1())
            .child(
                button("new-meeting", tr!("cal-new-meeting"), true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CalendarAction::NewMeeting))),
            )
            .when(!self.live, |d| {
                d.child(button("assistant", tr!("cal-assistant"), self.assistant).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.assistant = !this.assistant;
                        cx.notify();
                    },
                )))
            })
    }

    fn week_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let height = HOURS as f32 * HOUR_PX;
        let tz = &self.tz;
        let times = div().w(px(48.)).flex_none().relative().h(px(height)).children((0..HOURS).map(|h| {
            div()
                .absolute()
                .top(px(h as f32 * HOUR_PX))
                .right(px(6.))
                .text_xs()
                .text_color(rgb(0x888888))
                .child(format!("{:02}:00", FIRST_HOUR + h))
        }));
        let mut all_day_row = Vec::new();
        let mut columns = Vec::new();
        for d in 0..DAYS {
            let date = add_days(self.monday, d as i64);
            let day0 = day_start(date, tz) + FIRST_HOUR * 3600;
            let (timed, all_day) = day_events(&self.events, self.monday, d, tz);
            all_day_row.push(div().flex_1().min_w_0().flex().flex_col().gap_0p5().px_0p5().children(
                all_day.into_iter().map(|i| {
                    let e = &self.events[i];
                    event_chip(("all-day", i), e, self.selected == Some(i))
                        .h(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
                }),
            ));
            let placed = layout_day(&timed.iter().map(|&i| self.events[i].span).collect::<Vec<_>>());
            let lines = (0..HOURS)
                .map(|h| div().absolute().top(px(h as f32 * HOUR_PX)).left_0().right_0().h(px(1.)).bg(rgb(BUTTON)));
            let blocks: Vec<AnyElement> = placed
                .into_iter()
                .map(|p| {
                    let i = timed[p.index];
                    let e = &self.events[i];
                    let top = ((e.span.start - day0) as f32 / 3600. * HOUR_PX).clamp(0., height - 18.);
                    let h = (e.span.len() as f32 / 3600. * HOUR_PX).clamp(18., height - top);
                    div()
                        .absolute()
                        .top(px(top))
                        .h(px(h - 2.))
                        .left(relative(p.column as f32 / p.columns as f32))
                        .w(relative(p.span_columns as f32 / p.columns as f32))
                        .px_0p5()
                        .child(
                            event_chip(("event", i), e, self.selected == Some(i))
                                .size_full()
                                // Короткое событие — одна строка темы: время не влезает.
                                .when(h >= 34., |d| {
                                    d.child(div().truncate().text_color(rgb(0x555555)).child(format!(
                                        "{}–{} {}",
                                        hm(e.span.start, tz),
                                        hm(e.span.end, tz),
                                        e.location
                                    )))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx))),
                        )
                        .into_any_element()
                })
                .collect();
            columns.push(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .child(div().relative().h(px(height)).children(lines).children(blocks)),
            );
        }
        let today = jiff::Zoned::now().with_time_zone(tz.clone()).date();
        let header = div().flex().pl(px(48.)).border_b_1().border_color(rgb(BORDER)).children((0..DAYS).map(|d| {
            let date = add_days(self.monday, d as i64);
            div()
                .flex_1()
                .py_1()
                .text_center()
                .when(date == today, |d| d.text_color(rgb(ACCENT)).font_weight(FontWeight::SEMIBOLD))
                .child(format!("{} {}", day_name(d), date.strftime("%d.%m")))
        }));
        let has_all_day = self.events.iter().any(|e| e.all_day);
        div()
            .id("week")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .when(has_all_day, |d| {
                d.child(div().flex().pl(px(48.)).py_0p5().border_b_1().border_color(rgb(BORDER)).children(all_day_row))
            })
            .child(div().id("week-scroll").flex().flex_1().overflow_y_scroll().child(times).children(columns))
    }

    /// Подробности события с самым большим числом участников (снимок `event`).
    pub(crate) fn select_busiest(&mut self, cx: &mut Context<Self>) {
        self.selected =
            (0..self.events.len()).max_by_key(|&i| (self.events[i].participants.len(), std::cmp::Reverse(i)));
        cx.notify();
    }

    pub(crate) fn close_details(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        cx.notify();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn selected(&self) -> bool {
        self.selected.is_some()
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        self.selected = if self.selected == Some(i) { None } else { Some(i) };
        cx.notify();
    }

    fn details(&self, e: &Event, cx: &mut Context<Self>) -> impl IntoElement {
        let tz = &self.tz;
        let when = if e.all_day {
            format!("{}, {}", day_label(e.span.start, tz), tr!("cal-all-day"))
        } else {
            slot_label(e.span, tz)
        };
        let field = |label: &str, value: String| {
            div()
                .flex()
                .flex_col()
                .child(div().text_xs().text_color(rgb(MUTED)).child(tr!(label)))
                .child(div().child(value))
        };
        let organizer = e.organizer().map(|o| person(&o.name, &o.email));
        let people = e.participants.iter().filter(|p| !p.owner).map(|p| {
            let optional = if p.optional { format!(" · {}", tr!("cal-optional")) } else { String::new() };
            div()
                .flex()
                .justify_between()
                .gap_2()
                .child(div().min_w_0().truncate().child(person(&p.name, &p.email)))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(rgb(status_color(p.status)))
                        .child(format!("{}{optional}", status_text(p.status))),
                )
        });
        div()
            .id("event-details")
            .w(px(DETAILS_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xffffff))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(15.))
                            .child(e.title.clone()),
                    )
                    .child(button("details-close", tr!("cal-details-close"), false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.selected = None;
                            cx.notify();
                        },
                    ))),
            )
            .child(field("cal-details-when", when))
            .when(e.recurring, |d| d.child(div().text_xs().text_color(rgb(MUTED)).child(tr!("cal-details-recurring"))))
            .when(!e.location.is_empty(), |d| d.child(field("cal-details-where", e.location.clone())))
            .when_some(organizer, |d, o| d.child(field("cal-details-organizer", o)))
            .when(e.participants.iter().any(|p| !p.owner), |d| {
                d.child(div().text_xs().text_color(rgb(MUTED)).child(tr!("cal-details-participants"))).children(people)
            })
            .when(!e.description.trim().is_empty(), |d| {
                d.child(
                    div()
                        .pt_1()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .text_color(rgb(0x444444))
                        .child(e.description.clone()),
                )
            })
    }

    fn assistant(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let date = add_days(self.monday, self.day as i64);
        let start = day_start(date, &self.tz);
        let range = Span::new(start + FIRST_HOUR * 3600, start + (FIRST_HOUR + HOURS) * 3600);
        let day_tabs = div().flex().gap_1().children((0..DAYS).map(|d| {
            button_small(("day", d), day_name(d), d == self.day).on_click(cx.listener(move |this, _, _, cx| {
                this.day = d;
                cx.notify();
            }))
        }));
        let durations = div().flex().gap_1().children([1800i64, 3600, 5400].map(|secs| {
            button_small(("dur", secs as usize), tr!("cal-minutes", n = secs / 60), secs == self.duration).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.duration = secs;
                    this.slot = None;
                    cx.notify();
                }),
            )
        }));
        let suggestions = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(210.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("cal-free-for-all")))
            .children(self.slots().into_iter().enumerate().map(|(n, s)| {
                let day = zoned(s.start, &self.tz).map(|z| z.weekday().to_monday_zero_offset() as usize).unwrap_or(0);
                slot_button(n, slot_label(s, &self.tz), self.slot == Some(s)).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.slot = Some(s);
                        this.day = day.min(DAYS - 1);
                        cx.notify();
                    },
                ))
            }));
        div()
            .flex()
            .gap_4()
            .p_3()
            .border_t_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xffffff))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().flex().gap_4().child(day_tabs).child(durations))
                    .child(busy_grid(&self.people, range, &self.hours, self.slot)),
            )
            .child(suggestions)
    }

    fn share_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xf7f7f7))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(tr!("cal-share-label"))
                    .child(div().w(px(260.)).child(Input::new(&self.share)))
                    .child(
                        button("share", tr!("cal-share-button"), false)
                            .on_click(cx.listener(|this, _, window, cx| this.submit_share(window, cx))),
                    )
                    .when_some(self.share_status.clone(), |d, (text, error)| {
                        d.child(div().text_color(rgb(if error { ERROR } else { MUTED })).child(text))
                    }),
            )
            .child(div().text_xs().text_color(rgb(MUTED)).child(tr!("cal-share-note")))
    }
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let details = self.selected.and_then(|i| self.events.get(i)).map(|e| self.details(e, cx).into_any_element());
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(self.toolbar(cx))
            .child(div().flex().flex_1().min_h_0().child(self.week_grid(cx)).children(details))
            .when(self.assistant && !self.live, |d| d.child(self.assistant(cx)))
            .child(self.share_row(cx))
    }
}

impl crate::Mailer {
    /// Просьба календаря: команда синхронизации, окно встречи или тост. Нет сервера — «нужен сервер».
    pub(crate) fn on_calendar(&mut self, action: &CalendarAction, cx: &mut Context<Self>) {
        let command = match action {
            CalendarAction::Load { from, to } => Command::Events { from: *from, to: *to },
            CalendarAction::Share(email) => Command::ShareFreeBusy { email: email.clone() },
            CalendarAction::NewMeeting => {
                self.open_meeting(None, cx);
                return;
            }
            CalendarAction::Toast(level, text) => {
                self.toasts.update(cx, |t, cx| t.push(*level, text.clone(), cx));
                return;
            }
        };
        if !self.backend.send(command) {
            self.needs_server(cx);
            self.cal.update(cx, |c, cx| c.not_sent(cx));
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

/// «Имя <адрес>» или адрес.
fn person(name: &str, email: &str) -> String {
    match (name.trim(), email.trim()) {
        ("", e) => e.to_string(),
        (n, "") => n.to_string(),
        (n, e) => format!("{n} <{e}>"),
    }
}

fn status_text(s: Partstat) -> String {
    tr!(match s {
        Partstat::Accepted => "cal-status-accepted",
        Partstat::Declined => "cal-status-declined",
        Partstat::Tentative => "cal-status-tentative",
        Partstat::Delegated => "cal-status-delegated",
        Partstat::NeedsAction => "cal-status-needs-action",
    })
}

fn status_color(s: Partstat) -> u32 {
    match s {
        Partstat::Accepted => 0x1a7f37,
        Partstat::Declined => ERROR,
        Partstat::Tentative => 0x9a6700,
        _ => MUTED,
    }
}

/// Блок события: полоса слева, тема; «под вопросом» — светлее, выбранное — рамкой.
fn event_chip(id: impl Into<ElementId>, e: &Event, selected: bool) -> Stateful<Div> {
    let tentative = e.status == EventStatus::Tentative;
    div()
        .id(id)
        .overflow_hidden()
        .rounded_sm()
        .border_l_4()
        .border_color(rgb(0x2f6db5))
        .when(selected, |d| d.border_1().border_l_4().border_color(rgb(0x1b4f8a)))
        .bg(if tentative { rgb(0xeef4fb) } else { rgb(0xd6e6f9) })
        .px_1()
        .text_xs()
        .line_height(px(15.))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(0xc4dbf5)))
        .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(e.title.clone()))
}

pub(crate) fn button(id: &'static str, text: String, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .when(primary, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)).hover(|d| d.bg(rgb(0x115ea3))))
        .when(!primary, |d| d.bg(rgb(BUTTON)).hover(|d| d.bg(rgb(0xe0e0e0))))
        .child(text)
}

/// Переключатель (день, длительность): выбранный подсвечен.
pub(crate) fn button_small(id: impl Into<ElementId>, text: String, picked: bool) -> Stateful<Div> {
    div().id(id).px_2().rounded_md().cursor_pointer().bg(if picked { rgb(PICKED) } else { rgb(BUTTON) }).child(text)
}

pub(crate) fn slot_button(n: usize, text: String, picked: bool) -> Stateful<Div> {
    div()
        .id(("slot", n))
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .bg(if picked { rgb(0xd5f0dc) } else { rgb(0xf3f3f3) })
        .hover(|d| d.bg(rgb(0xe6f4ea)))
        .child(text)
}

/// Сетка помощника на `range` (день с `FIRST_HOUR`): шапка часов, строка на участника и легенда. Участник без
/// данных (`None`) — серые ячейки и подпись «нет данных», а не белые «свободно». `picked` — выбранное время.
pub(crate) fn busy_grid(
    people: &[(String, Option<Vec<Busy>>)],
    range: Span,
    hours: &WorkHours,
    picked: Option<Span>,
) -> Div {
    let busy: Vec<Option<Vec<Busy>>> = people.iter().map(|(_, b)| b.clone()).collect();
    let cells = grid(&busy, range, STEP, hours);
    let picked_cells = picked.filter(|s| s.overlaps(&range)).map(|s| {
        ((s.start - range.start).max(0) / STEP) as usize..((s.end - range.start + STEP - 1) / STEP).max(0) as usize
    });
    let time_row = div().flex().pl(px(NAME_W)).children((0..HOURS).map(|h| {
        div().w(px(CELL_PX * 2.)).text_xs().text_color(rgb(0x888888)).child(format!("{:02}", FIRST_HOUR + h))
    }));
    let rows = people.iter().zip(cells).map(|((name, busy), row)| {
        let label = div()
            .w(px(NAME_W))
            .flex()
            .gap_1()
            .items_center()
            .pr_1()
            .child(div().min_w_0().truncate().child(name.clone()));
        let label = match busy {
            Some(_) => label,
            None => label.child(div().flex_none().text_xs().text_color(rgb(MUTED)).child(tr!("cal-no-data"))),
        };
        div().flex().items_center().h(px(22.)).child(label).children(row.into_iter().enumerate().map(|(i, c)| {
            let on = picked_cells.as_ref().is_some_and(|r| r.contains(&i));
            div()
                .w(px(CELL_PX))
                .h_full()
                .border_r_1()
                .border_color(if i % 2 == 1 { rgb(0xcccccc) } else { rgb(0xeeeeee) })
                .bg(cell_color(c))
                .when(on, |d| d.border_t_2().border_b_2().border_color(rgb(0x1a8c3a)))
        }))
    });
    let legend = div().flex().flex_wrap().gap_3().text_xs().children(
        [
            (Cell::Busy, "cal-busy"),
            (Cell::Tentative, "cal-tentative"),
            (Cell::Unavailable, "cal-unavailable"),
            (Cell::OffHours, "cal-off-hours"),
            (Cell::Unknown, "cal-no-data"),
        ]
        .map(|(c, l)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().w(px(12.)).h(px(12.)).border_1().border_color(rgb(0xcccccc)).bg(cell_color(c)))
                .child(tr!(l))
        }),
    );
    div().flex().flex_col().gap_1().child(time_row).children(rows).child(legend)
}

fn cell_color(c: Cell) -> Rgba {
    match c {
        Cell::Free => rgb(0xffffff),
        Cell::OffHours => rgb(0xf0f0f0),
        Cell::Tentative => rgb(0x9cc3ee),
        Cell::Busy => rgb(0x2f6db5),
        Cell::Unavailable => rgb(0x7b3fa0),
        Cell::Unknown => rgb(0xc8c8c8),
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{
        Busy, Date, Event, Span, TimeZone, WorkHours, add_days, common_slots, day_events, day_start, monday_of,
    };
    use postman_calendar::{BusyKind, Cell, grid};

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
    fn no_data_is_not_free_and_is_left_out_of_slots() {
        let tz = TimeZone::UTC;
        let hours = WorkHours::standard(tz.clone());
        let day = ts("2026-10-19T00:00:00Z");
        let range = Span::new(day + 8 * 3600, day + 20 * 3600);
        let busy = |h: i64| Busy { span: Span::new(day + h * 3600, day + (h + 1) * 3600), kind: BusyKind::Busy };
        let people = vec![
            ("me".to_string(), Some(vec![busy(9)])),
            ("shared".to_string(), Some(vec![busy(10)])),
            ("not shared".to_string(), None),
        ];
        let cells = grid(&people.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(), range, 1800, &hours);
        // 09:00 у меня занято, у коллеги без шаринга — «нет данных» во всех ячейках, не «свободно».
        assert_eq!(cells[0][2], Cell::Busy);
        assert_eq!(cells[1][2], Cell::Free);
        assert!(cells[2].iter().all(|c| *c == Cell::Unknown));
        // Окна — по известным: первое свободное у обоих в 11:00; «нет данных» окна не закрывает и не открывает.
        let slots = common_slots(&people, Span::new(day, day + 86_400), 3600, &hours);
        assert_eq!(slots.first(), Some(&Span::new(day + 11 * 3600, day + 12 * 3600)));
        assert!(!slots.iter().any(|s| s.overlaps(&Span::new(day + 9 * 3600, day + 11 * 3600))));
    }
}

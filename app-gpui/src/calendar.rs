//! Календарь: рабочая неделя с раскладкой пересекающихся событий и «Помощник
//! планирования» — сетка занятости участников и предложенные общие окна.

use gpui_kit::prelude::*;
use gpui_kit::*;
use postman_calendar::{Busy, Cell, Event, EventStatus, Span, WorkHours, find_slots, grid, layout_day};
use pets_i18n::tr;
use postman_demo::calendar as demo;

use crate::Mailer;

const FIRST_HOUR: i64 = 8;
const HOURS: i64 = 12;
const HOUR_PX: f32 = 44.;
const STEP: i64 = 1800;
const CELL_PX: f32 = 26.;
/// Короткое имя дня рабочей недели: 0 — понедельник.
fn day_name(d: usize) -> String {
    tr!("cal-day", d = d)
}

pub struct CalendarView {
    week: i64,
    events: Vec<Event>,
    /// Участники помощника: (имя, занятость); первый — я.
    people: Vec<(String, Vec<Busy>)>,
    pub assistant: bool,
    day: usize,
    duration: i64,
    slot: Option<Span>,
    hours: WorkHours,
}

impl CalendarView {
    pub fn demo() -> Self {
        let mut people = vec![(tr!("cal-me"), demo::my_busy())];
        people.extend(demo::colleagues().into_iter().map(|(n, _, b)| (n.to_string(), b)));
        Self {
            week: demo::week_start(),
            events: demo::my_events(),
            people,
            assistant: false,
            day: 0,
            duration: 3600,
            slot: None,
            hours: WorkHours::standard(jiff::tz::TimeZone::system()),
        }
    }

    fn day_start(&self, d: usize) -> i64 {
        self.week + d as i64 * 86_400
    }

    fn slots(&self) -> Vec<Span> {
        let required: Vec<Vec<Busy>> = self.people.iter().map(|(_, b)| b.clone()).collect();
        let week = Span::new(self.week, self.week + 5 * 86_400);
        find_slots(&required, &[], week, self.duration, STEP, &self.hours, 8)
    }
}

fn hm(ts: i64) -> String {
    jiff::Timestamp::from_second(ts)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
}

pub fn render(m: &mut Mailer, cx: &mut Context<Mailer>) -> AnyElement {
    let v = &m.cal;
    let toolbar = div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .border_b_1()
        .border_color(rgb(0xdddddd))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("cal-work-week")))
        .child(div().flex_1())
        .child(
            div()
                .id("new-meeting")
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(if v.assistant { rgb(0xcce4ff) } else { rgb(0xececec) })
                .cursor_pointer()
                .child(tr!("cal-assistant"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cal.assistant = !this.cal.assistant;
                    cx.notify();
                })),
        );
    let mut col = div().flex().flex_col().flex_1().min_w_0().child(toolbar).child(week_grid(v));
    if v.assistant {
        col = col.child(assistant(v, cx));
    }
    col.into_any_element()
}

fn week_grid(v: &CalendarView) -> impl IntoElement {
    let height = HOURS as f32 * HOUR_PX;
    let times = div().w(px(48.)).relative().h(px(height)).children((0..HOURS).map(|h| {
        div()
            .absolute()
            .top(px(h as f32 * HOUR_PX))
            .right(px(6.))
            .text_xs()
            .text_color(rgb(0x888888))
            .child(format!("{:02}:00", FIRST_HOUR + h))
    }));
    let days = (0..5).map(|d| {
        let day0 = v.day_start(d) + FIRST_HOUR * 3600;
        let todays: Vec<&Event> = v.events.iter().filter(|e| e.span.start >= v.day_start(d) && e.span.start < v.day_start(d + 1)).collect();
        let placed = layout_day(&todays.iter().map(|e| e.span).collect::<Vec<_>>());
        let lines = (0..HOURS).map(|h| div().absolute().top(px(h as f32 * HOUR_PX)).left_0().right_0().h(px(1.)).bg(rgb(0xececec)));
        let blocks = placed.into_iter().map(move |p| {
            let e = todays[p.index];
            let top = (e.span.start - day0) as f32 / 3600. * HOUR_PX;
            let h = (e.span.len() as f32 / 3600. * HOUR_PX).max(18.);
            let tentative = e.status == EventStatus::Tentative;
            div()
                .absolute()
                .top(px(top))
                .h(px(h - 2.))
                .left(relative(p.column as f32 / p.columns as f32))
                .w(relative(p.span_columns as f32 / p.columns as f32))
                .px_0p5()
                .child(
                    div()
                        .size_full()
                        .overflow_hidden()
                        .rounded_sm()
                        .border_l_4()
                        .border_color(rgb(0x2f6db5))
                        .bg(if tentative { rgb(0xeef4fb) } else { rgb(0xd6e6f9) })
                        .px_1()
                        .text_xs()
                        .line_height(px(15.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(e.title.clone()))
                        .child(div().truncate().text_color(rgb(0x555555)).child(format!("{}–{} {}", hm(e.span.start), hm(e.span.end), e.location))),
                )
        });
        div().flex_1().min_w_0().border_l_1().border_color(rgb(0xdddddd)).child(
            div().relative().h(px(height)).children(lines).children(blocks),
        )
    });
    let header = div().flex().pl(px(48.)).border_b_1().border_color(rgb(0xdddddd)).children((0..5).map(|d| {
        let date = jiff::Timestamp::from_second(v.day_start(d))
            .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%d.%m").to_string())
            .unwrap_or_default();
        div().flex_1().py_1().text_center().child(format!("{} {date}", day_name(d)))
    }));
    div()
        .id("week")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(header)
        .child(div().id("week-scroll").flex().flex_1().overflow_y_scroll().child(times).children(days))
}

fn assistant(v: &CalendarView, cx: &mut Context<Mailer>) -> impl IntoElement {
    let range = Span::new(v.day_start(v.day) + FIRST_HOUR * 3600, v.day_start(v.day) + (FIRST_HOUR + HOURS) * 3600);
    let people: Vec<Option<Vec<Busy>>> = v.people.iter().map(|(_, b)| Some(b.clone())).collect();
    let cells = grid(&people, range, STEP, &v.hours);
    let slot_cells = v.slot.filter(|s| s.overlaps(&range)).map(|s| ((s.start - range.start) / STEP) as usize..((s.end - range.start) / STEP) as usize);

    let day_tabs = div().flex().gap_1().children((0..5).map(|d| {
        div()
            .id(("day", d))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .bg(if d == v.day { rgb(0xcce4ff) } else { rgb(0xececec) })
            .child(day_name(d))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.cal.day = d;
                cx.notify();
            }))
    }));
    let durations = div().flex().gap_1().children([1800i64, 3600, 5400].map(|secs| {
        div()
            .id(("dur", secs as usize))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .bg(if secs == v.duration { rgb(0xcce4ff) } else { rgb(0xececec) })
            .child(tr!("cal-minutes", n = secs / 60))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.cal.duration = secs;
                this.cal.slot = None;
                cx.notify();
            }))
    }));
    let time_row = div().flex().pl(px(150.)).children((0..HOURS).map(|h| {
        div().w(px(CELL_PX * 2.)).text_xs().text_color(rgb(0x888888)).child(format!("{:02}", FIRST_HOUR + h))
    }));
    let rows = v.people.iter().zip(cells).map(|((name, _), row)| {
        div()
            .flex()
            .items_center()
            .h(px(22.))
            .child(div().w(px(150.)).truncate().child(name.clone()))
            .children(row.into_iter().enumerate().map(|(i, c)| {
                let picked = slot_cells.as_ref().is_some_and(|r| r.contains(&i));
                div()
                    .w(px(CELL_PX))
                    .h_full()
                    .border_r_1()
                    .border_color(if i % 2 == 1 { rgb(0xcccccc) } else { rgb(0xeeeeee) })
                    .bg(cell_color(c))
                    .when(picked, |d| d.border_t_2().border_b_2().border_color(rgb(0x1a8c3a)))
            }))
    });
    let slots = v.slots();
    let suggestions = div().flex().flex_col().gap_1().w(px(210.)).child(div().font_weight(FontWeight::SEMIBOLD).child(tr!("cal-free-for-all"))).children(
        slots.into_iter().enumerate().map(|(n, s)| {
            let day = ((s.start - v.week) / 86_400) as usize;
            let picked = v.slot == Some(s);
            div()
                .id(("slot", n))
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .bg(if picked { rgb(0xd5f0dc) } else { rgb(0xf3f3f3) })
                .child(format!("{} {}–{}", day_name(day.min(4)), hm(s.start), hm(s.end)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cal.slot = Some(s);
                    this.cal.day = day.min(4);
                    cx.notify();
                }))
        }),
    );
    let legend = div().flex().gap_3().text_xs().children(
        [(Cell::Busy, "cal-busy"), (Cell::Tentative, "cal-tentative"), (Cell::Unavailable, "cal-unavailable"), (Cell::OffHours, "cal-off-hours")]
            .map(|(c, l)| div().flex().items_center().gap_1().child(div().w(px(12.)).h(px(12.)).bg(cell_color(c))).child(tr!(l))),
    );
    div()
        .flex()
        .gap_4()
        .p_3()
        .border_t_1()
        .border_color(rgb(0xdddddd))
        .bg(rgb(0xffffff))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().flex().gap_4().child(day_tabs).child(durations))
                .child(time_row)
                .children(rows)
                .child(legend),
        )
        .child(suggestions)
}

fn cell_color(c: Cell) -> Rgba {
    match c {
        Cell::Free => rgb(0xffffff),
        Cell::OffHours => rgb(0xf0f0f0),
        Cell::Tentative => rgb(0x9cc3ee),
        Cell::Busy => rgb(0x2f6db5),
        Cell::Unavailable => rgb(0x7b3fa0),
        Cell::Unknown => rgb(0xd0d0d0),
    }
}

//! Карточка приглашения на встречу над текстом письма (вложение `text/calendar`; вид — S5-11 по макету
//! «Приглашение в письме»). `.ics` разбирает воркер-песочница; здесь — готовое [`Invitation`], мой ответ и мой день.
//!
//! Состояние — `postman_core::invite::classify`: новое (кнопки «Принять / Под вопросом / Отклонить», клавиши
//! A / T / N), отвечено («Вы приняли · Изменить ответ»), изменено организатором (что было — зачёркнуто), отменено,
//! устарело (есть новее, встреча прошла). Справа — «Ваш день»: мои события вокруг встречи и пересечения
//! (`invite::conflicts`). Ответ — путь S3-03 (`Command::RespondInvite`); мой день — `Command::Events` на сутки встречи.

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use postman_calendar::{Event, Invitation, Partstat, Span, layout_day};
use postman_core::invite::{Change, InviteState, Known, Outdated, Seen, classify, conflicts};
use postman_core::sync::Event as SyncEvent;

use crate::calendar::{accent_button, add_days, day_start, duration_text, g, hm, outline_button, zoned};
use crate::chrome::icon;
use crate::reading::{ReadingEvent, ReadingPane, SyncRequest};
use crate::roles::{Look, Pm};

/// «Ваш день»: часов в окне и высота часа.
const DAY_HOURS: i64 = 5;
const DAY_HOUR_PX: f32 = 24.;
const DAY_W: f32 = 196.;

pub(crate) enum State {
    /// Ждём байты вложения или ответ воркера.
    Loading {
        blob_id: String,
    },
    Ready(Box<Invitation>),
    Failed(String),
}

pub(crate) struct Card {
    pub(crate) state: State,
    /// Мой ответ, принятый сервером (или запомненный в хранилище), и версия встречи, на которую он дан.
    pub(crate) answer: Option<Seen>,
    /// Ответ ушёл, итога ещё нет: кнопки неактивны.
    pub(crate) sending: Option<Partstat>,
    /// Мои события в сутки встречи (пересечения, «Ваш день», копия встречи в календаре); `None` — ещё нет.
    pub(crate) day: Option<Vec<Event>>,
    /// Ждём их с сервера: `[from, to)`.
    pub(crate) day_pending: Option<(i64, i64)>,
    /// «Изменить ответ»: кнопки ответа снова видны.
    pub(crate) changing: bool,
    /// Снимки: «сейчас» — здесь, а не по часам.
    pub(crate) clock: Option<i64>,
}

impl Card {
    pub(crate) fn loading(blob_id: String) -> Self {
        Self {
            state: State::Loading { blob_id },
            answer: None,
            sending: None,
            day: None,
            day_pending: None,
            changing: false,
            clock: None,
        }
    }

    pub(crate) fn invitation(&self) -> Option<&Invitation> {
        match &self.state {
            State::Ready(inv) => Some(inv),
            _ => None,
        }
    }

    pub(crate) fn uid(&self) -> Option<&str> {
        self.invitation().map(|i| i.event.uid.as_str())
    }

    pub(crate) fn waits_for(&self, blob: &str) -> bool {
        matches!(&self.state, State::Loading { blob_id } if blob_id == blob)
    }

    /// Ответ принят: запомнить с версией встречи; строка для хранилища.
    pub(crate) fn remember(&mut self, status: Partstat) -> String {
        let seen = match self.invitation() {
            Some(inv) => Seen::of(inv, status),
            None => Seen { status, version: None },
        };
        let value = seen.encode();
        self.answer = Some(seen);
        self.sending = None;
        self.changing = false;
        value
    }

    fn now(&self) -> i64 {
        self.clock.unwrap_or_else(|| jiff::Timestamp::now().as_second())
    }

    /// Копия встречи в моём календаре (тот же UID) — из событий дня.
    fn server_copy(&self) -> Option<&Event> {
        let uid = self.uid()?;
        self.day.as_ref()?.iter().find(|e| e.uid == uid)
    }

    /// Состояние для меня (`me` — мой адрес).
    pub(crate) fn state_for(&self, me: Option<&str>) -> Option<InviteState> {
        let inv = self.invitation()?;
        let known = Known { seen: self.answer.as_ref(), server: self.server_copy(), now: self.now() };
        Some(classify(inv, me, &known))
    }

    /// Можно ответить (кнопки видны): новое, изменённое, «Изменить ответ».
    pub(crate) fn can_answer(&self, me: Option<&str>) -> bool {
        match self.state_for(me) {
            Some(InviteState::New | InviteState::Updated(_)) => true,
            Some(InviteState::Answered(_)) => self.changing,
            _ => false,
        }
    }

    /// Снимки: состояние карточки — подменой входов классификации (ответ, версия, метод, «сейчас»).
    pub(crate) fn screen_state(&mut self, which: &str) {
        // Прошлая сцена могла сделать письмо отменой — вернуть запрос.
        if let State::Ready(inv) = &mut self.state
            && inv.method == "CANCEL"
        {
            inv.method = "REQUEST".into();
            inv.sequence -= 1;
        }
        let Some(inv) = self.invitation().cloned() else { return };
        self.clock = Some(inv.event.span.start - 86_400);
        match which {
            "answered" => self.answer = Some(Seen::of(&inv, Partstat::Accepted)),
            "updated" => {
                let mut seen = Seen::of(&inv, Partstat::Accepted);
                if let Some(v) = seen.version.as_mut() {
                    v.sequence -= 1;
                    v.span = Span::new(v.span.start - 3600, v.span.end - 3600);
                    v.location = tr!("invite-sample-old-place");
                }
                self.answer = Some(seen);
            }
            "cancelled" => {
                if let State::Ready(inv) = &mut self.state {
                    inv.method = "CANCEL".into();
                    inv.sequence += 1;
                }
                self.answer = None;
            }
            "outdated" => {
                self.answer = None;
                self.clock = Some(inv.event.span.end + 3600);
            }
            _ => self.answer = None,
        }
        self.changing = false;
    }
}

/// Ключ хранилища для запомненного ответа.
pub(crate) fn state_key(uid: &str) -> String {
    format!("invite:{uid}")
}

/// Ответ без версии (запись S3-03): одно слово статуса.
pub(crate) fn encode(s: Partstat) -> String {
    Seen { status: s, version: None }.encode()
}

/// Запомненный ответ: JSON с версией или прежнее слово статуса.
pub(crate) fn decode(s: &str) -> Option<Seen> {
    Seen::decode(s)
}

/// Участники, кроме организатора: (имя или адрес, ответ), по алфавиту — порядок в приглашении случаен.
pub(crate) fn attendees(inv: &Invitation) -> Vec<(String, Partstat)> {
    let org = inv.organizer_email();
    let mut list: Vec<(String, Partstat)> = inv
        .event
        .participants
        .iter()
        .filter(|p| p.attendee && Some(p.email.as_str()) != org)
        .map(|p| (if p.name.is_empty() { p.email.clone() } else { p.name.clone() }, p.status))
        .collect();
    list.sort_by_key(|(name, _)| name.to_lowercase());
    list
}


/// «Чт, 23 октября · 11:00–12:00».
fn when_line(span: Span, all_day: bool, tz: &TimeZone) -> String {
    let Some(a) = zoned(span.start, tz) else { return String::new() };
    let short = tr!("invite-day", wd = tr!("cal-day", d = a.weekday().to_monday_zero_offset()), day = a.day(), month = tr!("cal-month-of", m = a.month()));
    if all_day { format!("{short} · {}", tr!("cal-all-day")) } else { format!("{short} · {}–{}", hm(span.start, tz), hm(span.end, tz)) }
}

/// Листок календаря: день недели и число.
fn date_tile(span: Span, dim: bool, small: bool, tz: &TimeZone, l: &Look) -> Div {
    let z = zoned(span.start, tz);
    let wd = z.as_ref().map(|z| tr!("cal-day", d = z.weekday().to_monday_zero_offset()).to_uppercase()).unwrap_or_default();
    let day = z.as_ref().map(|z| z.day().to_string()).unwrap_or_default();
    let (w, h, head, num) = if small { (30., 34., 11., 13.) } else { (36., 40., 13., 16.) };
    div()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .flex()
        .flex_col()
        .rounded(px(4.))
        .overflow_hidden()
        .border_1()
        .border_color(l.border())
        .bg(l.panel())
        .child(
            div()
                .h(px(head))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.))
                .font_weight(FontWeight::SEMIBOLD)
                .bg(if dim { l.text3() } else { l.accent() })
                .text_color(l.on_accent())
                .child(wd),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(num))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if dim { l.text3() } else { l.text() })
                .child(day),
        )
}

/// Карточка; кнопки ответа зовут [`ReadingPane::answer_invite`].
pub(crate) fn render(card: &Card, me: Option<&str>, cx: &mut Context<ReadingPane>) -> AnyElement {
    let l = crate::roles::look(cx);
    let tz = TimeZone::system();
    let frame = || div().rounded(px(6.)).border_1().border_color(l.c(pets_ui::theme::Role::StrokeCard)).text_size(px(13.));
    let inv = match &card.state {
        State::Loading { .. } => {
            return frame().p_3().text_color(l.text2()).child(tr!("reading-invite-loading")).into_any_element();
        }
        State::Failed(e) => {
            return frame().p_3().child(tr!("reading-invite-failed-parse", error = e.as_str())).into_any_element();
        }
        State::Ready(inv) => inv,
    };
    let e = &inv.event;
    let state = card.state_for(me).unwrap_or(InviteState::Info);
    // Компактная строка: отмена, устаревшее, прочие методы.
    let compact = |reason: String, glyph: char, dim: bool, strike: bool| {
        frame()
            .h(px(44.))
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(6.))
            .pr(px(10.))
            .child(date_tile(e.span, dim, true, &tz, &l))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .line_height(px(17.))
                    .child(
                        div()
                            .truncate()
                            .when(dim, |d| d.text_color(l.text2()))
                            .when(strike, |d| d.line_through())
                            .child(format!("{} · {}", e.title, when_line(e.span, e.all_day, &tz))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(12.))
                            .text_color(l.text2())
                            .child(icon(glyph, 12.))
                            .child(div().truncate().child(reason)),
                    ),
            )
            .into_any_element()
    };
    let organizer = e.organizer().map(|o| if o.name.is_empty() { o.email.clone() } else { o.name.clone() }).unwrap_or_default();
    match &state {
        InviteState::Cancelled => {
            return compact(tr!("invite-cancelled", organizer = organizer), g::CANCEL, true, true);
        }
        InviteState::Outdated(why) => {
            let reason = match why {
                Outdated::Newer => tr!("invite-outdated-newer"),
                Outdated::Past => tr!("invite-outdated-past"),
            };
            return compact(reason, g::CLOCK, true, false);
        }
        InviteState::Info => {
            return compact(tr!("reading-invite-head", method = inv.method.as_str()), g::INFO, false, false);
        }
        _ => {}
    }
    let changes = match &state {
        InviteState::Updated(c) => c.clone(),
        _ => Vec::new(),
    };
    let changed = |c: fn(&Change) -> bool| changes.iter().any(c);
    let highlight = |d: Div, on: bool| if on { d.px(px(4.)).rounded(px(3.)).bg(l.picked()) } else { d };
    // Строка «когда» и «где»: изменённое подсвечено, прежнее — зачёркнуто рядом.
    let was_time = changes.iter().find_map(|c| match c {
        Change::Time(s) => Some(*s),
        _ => None,
    });
    let was_place = changes.iter().find_map(|c| match c {
        Change::Location(p) => Some(p.clone()),
        _ => None,
    });
    let was_title = changes.iter().find_map(|c| match c {
        Change::Title(t) => Some(t.clone()),
        _ => None,
    });
    let old = |text: String| div().text_size(px(12.)).text_color(l.text3()).line_through().child(text);
    let when_row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(6.))
        .min_w_0()
        .child(
            highlight(div().flex_none().font_weight(FontWeight::SEMIBOLD), changed(|c| matches!(c, Change::Time(_))))
                .child(when_line(e.span, e.all_day, &tz)),
        )
        .child(div().flex_none().text_color(l.text2()).child(format!("· {}", duration_text(e.span.len()))))
        .when_some(was_time, |d, s| d.child(old(when_line(s, e.all_day, &tz))));
    let place_row = (!e.location.trim().is_empty() || was_place.is_some()).then(|| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(5.))
            .text_size(px(12.))
            .text_color(l.text2())
            .child(icon(g::PIN, 12.))
            .child(highlight(div().flex_none(), was_place.is_some()).child(e.location.clone()))
            .when_some(was_place.clone(), |d, p| d.when(!p.trim().is_empty(), |d| d.child(old(p))))
    });
    let head = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(date_tile(e.span, false, false, &tz, &l))
        .child(div().flex_1().min_w_0().flex().flex_col().line_height(px(19.)).child(when_row).children(place_row));
    // Организатор и ответы участников точками.
    let people = attendees(inv);
    let accepted = people.iter().filter(|(_, s)| *s == Partstat::Accepted).count() + 1;
    let total = people.len() + 1;
    let dots = div().flex().gap(px(2.)).children(
        std::iter::once(Partstat::Accepted).chain(people.iter().map(|(_, s)| *s)).take(12).map(|s| {
            let d = div().size(px(6.)).rounded_full();
            match s {
                Partstat::Accepted => d.bg(l.success()),
                Partstat::Tentative => d.bg(l.text3()),
                Partstat::Declined => d.bg(l.error().opacity(0.6)),
                _ => d.border_1().border_color(l.text3()),
            }
        }),
    );
    let names = people.iter().map(|(n, s)| format!("{n} — {}", tr!("reading-partstat", s = postman_calendar::partstat_json(*s)))).collect::<Vec<_>>().join(", ");
    let org_row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(6.))
        .text_size(px(12.))
        .text_color(l.text2())
        .child(div().min_w_0().truncate().child(tr!("invite-organizer", name = organizer)))
        .child(div().text_color(l.text3()).child("·"))
        .child(dots)
        .child(
            div()
                .id("invite-people")
                .flex_none()
                .text_color(l.text())
                .child(tr!("invite-accepted-of", n = accepted, total = total))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(names.clone()).build(window, cx)),
        );
    let updated_banner = (!changes.is_empty()).then(|| {
        let what: Vec<String> = changes
            .iter()
            .map(|c| match c {
                Change::Time(_) => tr!("invite-changed-time"),
                Change::Location(_) => tr!("invite-changed-place"),
                Change::Title(_) => tr!("invite-changed-title"),
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(l.accent_text())
            .child(icon(g::EDIT, 12.))
            .child(tr!("invite-updated", what = what.join(", ")))
            .when_some(was_title, |d, t| d.child(old(t)))
    });
    // Ответ: кнопки или итог.
    let answered = match &state {
        InviteState::Answered(s) if !card.changing => Some(*s),
        _ => None,
    };
    let actions: AnyElement = match (&state, answered) {
        (InviteState::NotInvited, _) => {
            div().text_size(px(12.)).text_color(l.text2()).child(tr!("reading-invite-not-invited")).into_any_element()
        }
        (_, Some(s)) => {
            let (text, color) = match s {
                Partstat::Accepted => (tr!("invite-done-accepted"), l.success()),
                Partstat::Tentative => (tr!("invite-done-tentative"), l.text()),
                Partstat::Declined => (tr!("invite-done-declined"), l.text()),
                _ => (tr!("reading-invite-mine", status = tr!("reading-partstat", s = postman_calendar::partstat_json(s))), l.text()),
            };
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .when(s == Partstat::Accepted, |d| d.child(icon(g::CHECK, 12.)))
                        .child(text),
                )
                .child(
                    div()
                        .id("invite-change")
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_color(l.accent_text())
                        .cursor_pointer()
                        .child(tr!("cal-change-answer"))
                        .child(icon(g::DOWN, 9.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(card) = this.reading.as_mut().and_then(|r| r.invite.as_mut()) {
                                card.changing = true;
                            }
                            cx.notify();
                        })),
                )
                .into_any_element()
        }
        _ => {
            let sending = card.sending;
            let button = |id: &'static str, key: &str, hotkey: &str, s: Partstat, primary: bool, cx: &mut Context<ReadingPane>| {
                let b = if primary { accent_button(id, &l) } else { outline_button(id, &l) };
                b.h(px(28.))
                    .px(px(9.))
                    .text_size(px(12.))
                    .when(primary, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(sending.is_some(), |d| d.opacity(if sending == Some(s) { 1. } else { 0.5 }))
                    .when(primary, |d| d.child(icon(g::CHECK, 11.)))
                    .child(tr!(key))
                    .child(
                        div()
                            .px(px(4.))
                            .rounded(px(3.))
                            .text_size(px(10.))
                            .when(primary, |d| d.bg(l.on_accent().opacity(0.22)))
                            .when(!primary, |d| d.border_1().border_color(l.stroke()).text_color(l.text2()))
                            .child(hotkey.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if sending.is_none() {
                            this.answer_invite(s, cx);
                        }
                    }))
            };
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(button("invite-accept", "cal-accept", "A", Partstat::Accepted, true, cx))
                .child(button("invite-tentative", "cal-tentative-answer", "T", Partstat::Tentative, false, cx))
                .child(button("invite-decline", "cal-decline", "N", Partstat::Declined, false, cx))
                .when(sending.is_some(), |d| {
                    d.child(div().text_size(px(12.)).text_color(l.text2()).child(tr!("reading-invite-sending")))
                })
                .into_any_element()
        }
    };
    let route = div()
        .text_size(px(11.))
        .text_color(l.text3())
        .truncate()
        .child(tr!(if answered.is_some() { "invite-route-done" } else { "invite-route" }));
    let left = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .px(px(12.))
        .py(px(10.))
        .children(updated_banner)
        .child(head)
        .child(org_row)
        .child(actions)
        .child(route);
    frame()
        .flex()
        .child(left)
        .child(your_day(card, inv, me, &tz, &l))
        .into_any_element()
}

/// «Ваш день»: мои события вокруг встречи, сама встреча и пересечения.
fn your_day(card: &Card, inv: &Invitation, me: Option<&str>, tz: &TimeZone, l: &Look) -> Div {
    let e = &inv.event;
    let panel = div()
        .w(px(DAY_W))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(10.))
        .py(px(8.))
        .border_l_1()
        .border_color(l.border())
        .rounded_r(px(6.))
        .bg(l.pm(Pm::CalOff));
    let Some(z) = zoned(e.span.start, tz) else { return panel };
    let first_hour = (i64::from(z.hour()) - 2).clamp(0, 24 - DAY_HOURS);
    let day0 = day_start(z.date(), tz);
    let window = Span::new(day0 + first_hour * 3600, day0 + (first_hour + DAY_HOURS) * 3600);
    let title = div()
        .flex()
        .text_size(px(11.))
        .text_color(l.text2())
        .child(div().flex_1().child(tr!("invite-your-day", day = short_date(z.date()))))
        .child(format!("{}–{}", first_hour, first_hour + DAY_HOURS));
    let Some(day) = &card.day else {
        return panel.child(title).child(div().text_size(px(11.)).text_color(l.text3()).child(tr!("invite-day-loading")));
    };
    let hits = conflicts(e.span, &e.uid, day, me);
    // В окне: мои события (без отклонённых и самой встречи) и встреча.
    let mine: Vec<&Event> = day
        .iter()
        .filter(|x| x.uid != e.uid && !x.all_day && x.span.overlaps(&window))
        .filter(|x| crate::calendar::event_look(x, me) != crate::calendar::Shown::Hidden)
        .collect();
    let mut spans: Vec<Span> = mine.iter().map(|x| x.span).collect();
    spans.push(e.span);
    let placed = layout_day(&spans);
    let y = |t: i64| ((t - window.start) as f32 / 3600. * DAY_HOUR_PX).clamp(0., DAY_HOURS as f32 * DAY_HOUR_PX);
    let accepted = matches!(card.state_for(me), Some(InviteState::Answered(Partstat::Accepted)));
    let blocks = placed.into_iter().map(|p| {
        let (span, label, kind) = match mine.get(p.index) {
            Some(x) => (x.span, x.title.clone(), if hits.iter().any(|h| h.uid == x.uid && h.span == x.span) { 2 } else { 0 }),
            None => (e.span, e.title.clone(), 1),
        };
        let (top, bottom) = (y(span.start), y(span.end));
        div()
            .absolute()
            .top(px(top + 1.))
            .h(px((bottom - top - 1.).max(8.)))
            .left(relative(p.column as f32 / p.columns as f32))
            .w(relative(p.span_columns as f32 / p.columns as f32))
            .px(px(1.))
            .child(
                div()
                    .size_full()
                    .px(px(4.))
                    .rounded(px(3.))
                    .text_size(px(10.))
                    .line_height(px(13.))
                    .overflow_hidden()
                    .truncate()
                    .map(|d| match kind {
                        1 if accepted => d.bg(l.accent()).text_color(l.on_accent()).font_weight(FontWeight::SEMIBOLD),
                        1 => d.bg(l.panel()).border_1().border_dashed().border_color(l.accent()).text_color(l.accent_text()).font_weight(FontWeight::SEMIBOLD),
                        2 => d.bg(l.pm(Pm::ConflictBg)).border_l_2().border_color(l.pm(Pm::ConflictFg)).text_color(l.pm(Pm::ConflictFg)),
                        _ => d.bg(l.fill()).border_l_2().border_color(l.text3()).text_color(l.text2()),
                    })
                    .child(label),
            )
    });
    let lines = (0..=DAY_HOURS).map(|h| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(h as f32 * DAY_HOUR_PX - 7.))
            .h(px(14.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().w(px(18.)).text_right().text_size(px(10.)).text_color(l.text3()).child((first_hour + h).to_string()))
            .child(div().flex_1().h(px(1.)).bg(l.border()))
    });
    let timeline = div()
        .relative()
        .h(px(DAY_HOURS as f32 * DAY_HOUR_PX))
        .mt(px(4.))
        .children(lines)
        .child(div().absolute().left(px(24.)).right_0().top_0().bottom_0().children(blocks));
    let conflict = hits.first().map(|h| {
        let text = if hits.len() > 1 {
            tr!("invite-conflict-many", title = h.title.clone(), time = hm(h.span.start, tz), n = hits.len() - 1)
        } else {
            tr!("invite-conflict", title = h.title.clone(), time = hm(h.span.start, tz))
        };
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .mt(px(2.))
            .text_size(px(11.))
            .text_color(l.pm(Pm::ConflictFg))
            .child(icon(g::OVERLAP, 12.))
            .child(div().truncate().child(text))
    });
    let free = hits.is_empty().then(|| {
        div().mt(px(2.)).flex().items_center().gap(px(5.)).text_size(px(11.)).text_color(l.text2()).child(icon(g::CHECK, 11.)).child(tr!("invite-no-conflict"))
    });
    panel.child(title).child(timeline).children(conflict).children(free)
}

/// «чт 23 окт».
fn short_date(d: jiff::civil::Date) -> String {
    format!("{} {} {}", tr!("reading-weekday", d = d.weekday().to_monday_zero_offset()), d.day(), tr!("invite-month-short", m = d.month()))
}

// --- область чтения: мой день и клавиши ---

impl ReadingPane {
    /// Приглашение разобрано: мой день встречи — с сервера (`Command::Events` на сутки) или из демо-недели.
    pub(crate) fn invite_ready(&mut self, cx: &mut Context<Self>) {
        let live = self.live;
        let tz = TimeZone::system();
        let Some(card) = self.reading.as_mut().and_then(|r| r.invite.as_mut()) else { return };
        let Some(inv) = card.invitation() else { return };
        let Some(date) = zoned(inv.event.span.start, &tz).map(|z| z.date()) else { return };
        let (from, to) = (day_start(date, &tz), day_start(add_days(date, 1), &tz));
        if !live {
            card.day = Some(demo_day(date, &tz));
            return;
        }
        card.day_pending = Some((from, to));
        cx.emit(ReadingEvent::Sync(SyncRequest::Events { from, to }));
    }

    /// События дня пришли с сервера.
    pub(crate) fn invite_events(&mut self, from: i64, to: i64, events: &[Event], cx: &mut Context<Self>) {
        let Some(card) = self.reading.as_mut().and_then(|r| r.invite.as_mut()) else { return };
        if card.day_pending == Some((from, to)) {
            card.day_pending = None;
            card.day = Some(events.to_vec());
            cx.notify();
        }
    }

    /// События дня не придут (сервера нет, ошибка): карточка без «Вашего дня».
    pub(crate) fn invite_events_failed(&mut self) {
        if let Some(card) = self.reading.as_mut().and_then(|r| r.invite.as_mut()) {
            card.day_pending = None;
            card.day.get_or_insert_with(Vec::new);
        }
    }

    /// Клавиши A / T / N: ответ, если карточка его ждёт. `false` — ответить нечего.
    pub(crate) fn invite_hotkey(&mut self, status: Partstat, cx: &mut Context<Self>) -> bool {
        let me = self.me.clone();
        let can = self.reading.as_ref().and_then(|r| r.invite.as_ref()).is_some_and(|c| c.sending.is_none() && c.can_answer(me.as_deref()));
        if can {
            self.answer_invite(status, cx);
        }
        can
    }

    /// Итог синхронизации для карточки: мой день (`EventsLoaded`), сбой календаря.
    pub(crate) fn invite_sync(&mut self, ev: &SyncEvent, cx: &mut Context<Self>) {
        match ev {
            SyncEvent::EventsLoaded { from, to, events } => self.invite_events(*from, *to, events, cx),
            SyncEvent::CalendarFailed(_) => {
                self.invite_events_failed();
                cx.notify();
            }
            _ => {}
        }
    }
}

/// Демо: мой день встречи — события демо-недели в тот же день недели, перенесённые на дату встречи.
fn demo_day(date: jiff::civil::Date, tz: &TimeZone) -> Vec<Event> {
    let wd = date.weekday();
    postman_demo::calendar::my_events()
        .into_iter()
        .filter_map(|mut e| {
            let d = zoned(e.span.start, tz)?.date();
            if d.weekday() != wd {
                return None;
            }
            let shift = day_start(date, tz) - day_start(d, tz);
            e.span = Span::new(e.span.start + shift, e.span.end + shift);
            Some(e)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{Card, State, attendees, decode, encode, state_key};
    use jiff::tz::TimeZone;
    use postman_calendar::{Invitation, Partstat};
    use postman_core::invite::{InviteState, Seen};
    use serde_json::json;

    fn inv(method: &str) -> Invitation {
        let v = json!({ "method": method, "data": {
            "@type": "Event", "uid": "u-1@partner.example", "title": "Созвон",
            "start": "2026-10-21T11:00:00", "timeZone": "Europe/Moscow", "duration": "PT1H",
            "participants": {
                "o": { "name": "Иван", "calendarAddress": "mailto:ivan@partner.example", "roles": { "owner": true, "attendee": true } },
                "m": { "name": "Я", "calendarAddress": "mailto:me@corp.example", "roles": { "attendee": true }, "participationStatus": "needs-action" },
                "x": { "calendarAddress": "mailto:maria@partner.example", "roles": { "attendee": true }, "participationStatus": "accepted" }
            }
        }});
        Invitation::from_worker(&v, &TimeZone::UTC).unwrap()
    }

    fn ready(method: &str) -> Card {
        let mut c = Card::loading("ics".into());
        c.state = State::Ready(Box::new(inv(method)));
        c.clock = Some("2026-10-20T00:00:00Z".parse::<jiff::Timestamp>().unwrap().as_second());
        c
    }

    #[test]
    fn card_states_and_answer_buttons() {
        let me = Some("ME@corp.example");
        let mut c = ready("REQUEST");
        assert_eq!(c.state_for(me), Some(InviteState::New));
        assert!(c.can_answer(me));
        assert_eq!(c.state_for(Some("other@corp.example")), Some(InviteState::NotInvited));
        assert!(!c.can_answer(Some("other@corp.example")));
        // Ответ принят — запомнен с версией; кнопки — только по «Изменить ответ».
        let stored = c.remember(Partstat::Tentative);
        assert_eq!(decode(&stored).map(|s| s.status), Some(Partstat::Tentative));
        assert_eq!(c.state_for(me), Some(InviteState::Answered(Partstat::Tentative)));
        assert!(!c.can_answer(me));
        c.changing = true;
        assert!(c.can_answer(me));
        assert_eq!(ready("CANCEL").state_for(me), Some(InviteState::Cancelled));
        // Снимки: состояния подменой входов.
        for (which, want) in [("answered", "Answered"), ("updated", "Updated"), ("cancelled", "Cancelled"), ("outdated", "Outdated")] {
            let mut c = ready("REQUEST");
            c.screen_state(which);
            let got = format!("{:?}", c.state_for(me));
            assert!(got.contains(want), "{which}: {got}");
        }
    }

    #[test]
    fn stored_answer_round_trip() {
        for s in [Partstat::Accepted, Partstat::Tentative, Partstat::Declined] {
            assert_eq!(decode(&encode(s)), Some(Seen { status: s, version: None }));
        }
        // Запись S3-03 — одно слово.
        assert_eq!(decode("accepted").map(|s| s.status), Some(Partstat::Accepted));
        assert_eq!(state_key("u-1"), "invite:u-1");
    }

    #[test]
    fn attendees_skip_the_organizer() {
        let a = attendees(&inv("REQUEST"));
        assert_eq!(
            a,
            [("maria@partner.example".to_string(), Partstat::Accepted), ("Я".to_string(), Partstat::NeedsAction)]
        );
    }

    #[test]
    fn card_waits_for_its_blob() {
        let c = Card::loading("ics".into());
        assert!(c.waits_for("ics") && !c.waits_for("logo"));
        assert_eq!(c.uid(), None);
        let c = ready("REQUEST");
        assert_eq!(c.uid(), Some("u-1@partner.example"));
        assert!(!c.waits_for("ics"));
    }
}

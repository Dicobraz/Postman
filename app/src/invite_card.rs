//! Карточка приглашения на встречу над текстом письма (вложение `text/calendar`).

use eframe::egui;
use jiff::tz::TimeZone;
use postman_cache::AttachmentRow;
use postman_calendar::{Invitation, Partstat};

pub enum State {
    /// Ждём байты вложения или ответ воркера.
    Loading(String),
    Ready(Box<Invitation>),
    Failed(String),
}

/// Состояние ответа на приглашение (на время сеанса и из хранилища).
#[derive(Clone, Copy, PartialEq)]
pub enum Answer {
    Sending(Partstat),
    Done(Partstat),
}

pub fn is_invite(a: &AttachmentRow) -> bool {
    let mime = a.mime.to_ascii_lowercase();
    mime.starts_with("text/calendar") || mime == "application/ics" || a.name.to_ascii_lowercase().ends_with(".ics")
}

/// Ключ в хранилище для запомненного ответа.
pub fn state_key(uid: &str) -> String {
    format!("invite:{uid}")
}

pub fn encode(s: Partstat) -> &'static str {
    postman_calendar::partstat_json(s)
}

pub fn decode(s: &str) -> Option<Partstat> {
    [Partstat::Accepted, Partstat::Tentative, Partstat::Declined].into_iter().find(|p| encode(*p) == s)
}

/// Показать карточку; возвращает выбранный ответ.
pub fn show(ui: &mut egui::Ui, state: &State, me: &str, answer: Option<Answer>) -> Option<Partstat> {
    let mut chosen = None;
    let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.18);
    egui::Frame::new().fill(fill).inner_margin(8.0).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let inv = match state {
            State::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Приглашение на встречу…");
                });
                return;
            }
            State::Failed(e) => {
                ui.label(format!("📅 Приглашение не разобрано: {e}"));
                return;
            }
            State::Ready(inv) => inv,
        };
        let e = &inv.event;
        let cancelled = inv.method == "CANCEL";
        let head = match inv.method.as_str() {
            "CANCEL" => "📅 Встреча отменена",
            "REPLY" => "📅 Ответ на приглашение",
            "COUNTER" => "📅 Предложено другое время",
            _ => "📅 Приглашение на встречу",
        };
        ui.weak(head);
        let title = egui::RichText::new(&e.title).size(16.0).strong();
        ui.label(if cancelled { title.strikethrough() } else { title });
        egui::Grid::new("invite").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
            ui.weak("Когда:");
            ui.label(when(e.span.start, e.span.end, e.all_day));
            ui.end_row();
            if !e.location.is_empty() {
                ui.weak("Где:");
                ui.label(&e.location);
                ui.end_row();
            }
            if let Some(o) = e.organizer() {
                ui.weak("Организатор:");
                ui.label(if o.name.is_empty() { o.email.clone() } else { format!("{} <{}>", o.name, o.email) });
                ui.end_row();
            }
            let others = e.participants.iter().filter(|p| p.attendee && !p.email.eq_ignore_ascii_case(me)).count();
            if others > 0 {
                ui.weak("Участники:");
                ui.label(format!("ещё {others}"));
                ui.end_row();
            }
            if inv.method == "REPLY" {
                for p in e.participants.iter().filter(|p| p.attendee) {
                    ui.weak("Ответ:");
                    ui.label(format!("{} — {}", if p.name.is_empty() { &p.email } else { &p.name }, status_text(p.status)));
                    ui.end_row();
                }
            }
        });
        if inv.method != "REQUEST" {
            return;
        }
        let Some(current) = inv.my_status(me) else {
            ui.weak("Вас нет среди участников — ответить нельзя.");
            return;
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (mine, sending) = match answer {
                Some(Answer::Sending(s)) => (s, true),
                Some(Answer::Done(s)) => (s, false),
                None => (current, false),
            };
            for (label, s) in [("✔ Принять", Partstat::Accepted), ("? Под вопросом", Partstat::Tentative), ("✖ Отклонить", Partstat::Declined)] {
                let b = egui::Button::new(label).selected(mine == s);
                if ui.add_enabled(!sending, b).clicked() && mine != s {
                    chosen = Some(s);
                }
            }
            if sending {
                ui.spinner();
            } else if mine != Partstat::NeedsAction {
                ui.weak(format!("Ваш ответ: {}", status_text(mine)));
            }
        });
    });
    chosen
}

fn status_text(s: Partstat) -> &'static str {
    match s {
        Partstat::Accepted => "принято",
        Partstat::Tentative => "под вопросом",
        Partstat::Declined => "отклонено",
        Partstat::Delegated => "передано",
        Partstat::NeedsAction => "нет ответа",
    }
}

/// «ср 21.10.2026 11:00–12:00» в местном времени.
fn when(start: i64, end: i64, all_day: bool) -> String {
    const DAYS: [&str; 7] = ["пн", "вт", "ср", "чт", "пт", "сб", "вс"];
    let tz = TimeZone::system();
    let z = |t: i64| jiff::Timestamp::from_second(t).ok().map(|t| t.to_zoned(tz.clone()));
    let (Some(a), Some(b)) = (z(start), z(end)) else { return String::new() };
    let day = |z: &jiff::Zoned| format!("{} {}", DAYS[z.weekday().to_monday_zero_offset() as usize], z.strftime("%d.%m.%Y"));
    if all_day {
        return format!("{}, весь день", day(&a));
    }
    if a.date() == b.date() {
        format!("{} {}–{}", day(&a), a.strftime("%H:%M"), b.strftime("%H:%M"))
    } else {
        format!("{} {} – {} {}", day(&a), a.strftime("%H:%M"), day(&b), b.strftime("%H:%M"))
    }
}

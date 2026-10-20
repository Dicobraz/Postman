//! Карточка приглашения на встречу над текстом письма (вложение `text/calendar`).
//! `.ics` разбирает воркер-песочница; здесь — только готовое [`Invitation`] и мой ответ.

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use postman_calendar::{Invitation, Partstat};

use crate::reading::ReadingPane;

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
    /// Мой ответ, принятый сервером (или запомненный в хранилище).
    pub(crate) answer: Option<Partstat>,
    /// Ответ ушёл, итога ещё нет: кнопки неактивны.
    pub(crate) sending: Option<Partstat>,
}

impl Card {
    pub(crate) fn loading(blob_id: String) -> Self {
        Self { state: State::Loading { blob_id }, answer: None, sending: None }
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
}

/// Можно ли ответить на приглашение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reply {
    /// Не запрос (отмена, чужой ответ, встречное предложение) — кнопок нет.
    NotRequest,
    /// Меня нет среди участников.
    NotInvited,
    /// Мой статус в самом приглашении.
    Can(Partstat),
}

pub(crate) fn reply(inv: &Invitation, me: Option<&str>) -> Reply {
    if inv.method != "REQUEST" {
        return Reply::NotRequest;
    }
    match me.and_then(|me| inv.my_status(me)) {
        Some(s) => Reply::Can(s),
        None => Reply::NotInvited,
    }
}

/// Что показывают кнопки: выбранный ответ и идёт ли отправка.
pub(crate) fn shown(current: Partstat, answer: Option<Partstat>, sending: Option<Partstat>) -> (Partstat, bool) {
    match (sending, answer) {
        (Some(s), _) => (s, true),
        (None, Some(a)) => (a, false),
        (None, None) => (current, false),
    }
}

/// Нажатие кнопки ответа: новый ответ или ничего (идёт отправка, этот ответ уже выбран).
pub(crate) fn choose(mine: Partstat, sending: bool, pressed: Partstat) -> Option<Partstat> {
    (!sending && mine != pressed).then_some(pressed)
}

/// Ключ хранилища для запомненного ответа.
pub(crate) fn state_key(uid: &str) -> String {
    format!("invite:{uid}")
}

pub(crate) fn encode(s: Partstat) -> &'static str {
    postman_calendar::partstat_json(s)
}

pub(crate) fn decode(s: &str) -> Option<Partstat> {
    [Partstat::Accepted, Partstat::Tentative, Partstat::Declined].into_iter().find(|p| encode(*p) == s)
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

fn status_text(s: Partstat) -> String {
    tr!("reading-partstat", s = encode(s))
}

/// «ср 21.10.2026 11:00–12:00» в местном времени.
pub(crate) fn when(start: i64, end: i64, all_day: bool, tz: &TimeZone) -> String {
    let z = |t: i64| jiff::Timestamp::from_second(t).ok().map(|t| t.to_zoned(tz.clone()));
    let (Some(a), Some(b)) = (z(start), z(end)) else { return String::new() };
    let day = |z: &jiff::Zoned| {
        let wd = tr!("reading-weekday", d = z.weekday().to_monday_zero_offset());
        format!("{wd} {}", z.strftime("%d.%m.%Y"))
    };
    if all_day {
        return tr!("reading-invite-all-day", day = day(&a));
    }
    if a.date() == b.date() {
        format!("{} {}–{}", day(&a), a.strftime("%H:%M"), b.strftime("%H:%M"))
    } else {
        format!("{} {} – {} {}", day(&a), a.strftime("%H:%M"), day(&b), b.strftime("%H:%M"))
    }
}

fn row(label: String, value: impl IntoElement) -> Div {
    let l = crate::roles::now();
    div()
        .flex()
        .gap_2()
        .child(div().w(px(96.)).flex_none().text_color(l.text2()).child(label))
        .child(div().flex_1().min_w_0().child(value))
}

/// Карточка; кнопки ответа зовут [`ReadingPane::answer_invite`].
pub(crate) fn render(card: &Card, me: Option<&str>, cx: &mut Context<ReadingPane>) -> AnyElement {
    let l = crate::roles::look(cx);
    let frame =
        div().flex().flex_col().gap_1().p_3().rounded_md().bg(l.picked()).border_1().border_color(l.c(pets_ui::theme::Role::StrokeCard));
    let inv = match &card.state {
        State::Loading { .. } => {
            return frame.text_color(l.text2()).child(tr!("reading-invite-loading")).into_any_element();
        }
        State::Failed(e) => {
            return frame.child(tr!("reading-invite-failed-parse", error = e.as_str())).into_any_element();
        }
        State::Ready(inv) => inv,
    };
    let e = &inv.event;
    let cancelled = inv.method == "CANCEL";
    let title = div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(e.title.clone());
    let mut col = frame
        .child(div().text_color(l.accent_text()).child(tr!("reading-invite-head", method = inv.method.as_str())))
        .child(if cancelled { title.line_through() } else { title })
        .child(row(tr!("reading-invite-when"), when(e.span.start, e.span.end, e.all_day, &TimeZone::system())));
    if !e.location.is_empty() {
        col = col.child(row(tr!("reading-invite-where"), e.location.clone()));
    }
    if let Some(o) = e.organizer() {
        let org = if o.name.is_empty() { o.email.clone() } else { format!("{} <{}>", o.name, o.email) };
        col = col.child(row(tr!("reading-invite-organizer"), org));
    }
    let people = attendees(inv);
    if !people.is_empty() {
        let list =
            people.iter().map(|(name, s)| format!("{name} ({})", status_text(*s))).collect::<Vec<_>>().join(", ");
        col = col.child(row(tr!("reading-invite-attendees"), list));
    }
    let current = match reply(inv, me) {
        Reply::NotRequest => return col.into_any_element(),
        Reply::NotInvited => {
            return col
                .child(div().text_color(l.text2()).child(tr!("reading-invite-not-invited")))
                .into_any_element();
        }
        Reply::Can(s) => s,
    };
    let (mine, sending) = shown(current, card.answer, card.sending);
    let buttons = [
        ("reading-invite-accept", Partstat::Accepted),
        ("reading-invite-tentative", Partstat::Tentative),
        ("reading-invite-decline", Partstat::Declined),
    ]
    .map(|(key, s)| {
        let selected = mine == s;
        div()
            .id(key)
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if selected { l.accent() } else { l.stroke() })
            .bg(if selected { l.accent() } else { l.panel() })
            .text_color(if selected { l.on_accent() } else { l.text() })
            .when(sending, |d| d.opacity(0.6))
            .when(!sending, |d| d.cursor_pointer().hover(|d| d.border_color(l.accent())))
            .child(tr!(key))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(s) = choose(mine, sending, s) {
                    this.answer_invite(s, cx);
                }
            }))
    });
    let note = if sending {
        tr!("reading-invite-sending")
    } else if mine != Partstat::NeedsAction {
        tr!("reading-invite-mine", status = status_text(mine))
    } else {
        String::new()
    };
    col.child(
        div().flex().items_center().gap_2().pt_1().children(buttons).child(div().text_color(l.text2()).child(note)),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{Card, Reply, State, attendees, choose, decode, encode, reply, shown, state_key};
    use jiff::tz::TimeZone;
    use postman_calendar::{Invitation, Partstat};
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

    #[test]
    fn who_can_reply() {
        assert_eq!(reply(&inv("REQUEST"), Some("ME@corp.example")), Reply::Can(Partstat::NeedsAction));
        assert_eq!(reply(&inv("REQUEST"), Some("other@corp.example")), Reply::NotInvited);
        assert_eq!(reply(&inv("REQUEST"), None), Reply::NotInvited);
        assert_eq!(reply(&inv("CANCEL"), Some("me@corp.example")), Reply::NotRequest);
    }

    #[test]
    fn buttons_show_sending_then_answer_then_invitation() {
        use Partstat::*;
        assert_eq!(shown(NeedsAction, None, None), (NeedsAction, false));
        assert_eq!(shown(NeedsAction, Some(Accepted), None), (Accepted, false));
        assert_eq!(shown(NeedsAction, Some(Accepted), Some(Declined)), (Declined, true));
        // Во время отправки и повторное нажатие того же ответа — ничего не шлём.
        assert_eq!(choose(Accepted, true, Declined), None);
        assert_eq!(choose(Accepted, false, Accepted), None);
        assert_eq!(choose(NeedsAction, false, Tentative), Some(Tentative));
    }

    #[test]
    fn stored_answer_round_trip() {
        for s in [Partstat::Accepted, Partstat::Tentative, Partstat::Declined] {
            assert_eq!(decode(encode(s)), Some(s));
        }
        assert_eq!(decode("needs-action"), None);
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
        let c = Card { state: State::Ready(Box::new(inv("REQUEST"))), answer: None, sending: None };
        assert_eq!(c.uid(), Some("u-1@partner.example"));
        assert!(!c.waits_for("ics"));
    }
}

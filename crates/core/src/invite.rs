//! Ответ на приглашение, пришедшее письмом (iMIP).
//!
//! Если событие уже есть в календаре на сервере (сервер разобрал письмо сам) — меняем
//! свой статус там, ответ организатору шлёт сервер. Иначе отвечаем письмом METHOD:REPLY,
//! а принятое/«под вопросом» кладём в свой календарь без рассылки.

use postman_calendar::{Invitation, Partstat, imip_reply, partstat_json, participant_id, reply_subject};
use postman_compose::{Address, CalendarPart, Draft};
use postman_jmap::Client;
use serde_json::{Value, json};

/// Как ушёл ответ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Статус изменён в событии на сервере, организатору ответил сервер.
    Server,
    /// Ответ письмом; `imported` — событие добавлено в мой календарь.
    Mail { imported: bool },
}

/// Письмо-ответ организатору с частью `text/calendar; method=REPLY`.
pub fn reply_draft(inv: &Invitation, me: &Address, status: Partstat, in_reply_to: Option<String>, now: i64) -> Option<Draft> {
    let org = inv.event.organizer().filter(|o| !o.email.is_empty())?;
    let subject = reply_subject(&inv.event.title, status);
    Some(Draft {
        from: me.clone(),
        to: vec![Address::new(&org.name, &org.email)],
        markdown: subject.clone(),
        subject,
        references: in_reply_to.iter().cloned().collect(),
        in_reply_to,
        calendar: Some(CalendarPart {
            method: "REPLY".into(),
            ics: imip_reply(inv, (&me.name, &me.email), status, now),
        }),
        ..Default::default()
    })
}

/// Событие для импорта в календарь: мой статус, календарь назначения, без серверного id.
pub fn import_event(inv: &Invitation, me: &str, status: Partstat, calendar_id: &str) -> Value {
    let mut ev = inv.with_my_status(me, status);
    if let Some(o) = ev.as_object_mut() {
        o.remove("id");
        o.insert("calendarIds".into(), json!({ calendar_id: true }));
    }
    ev
}

/// Попробовать ответить через календарь сервера. `None` — события там нет (или меня в нём нет).
pub async fn via_server(client: &Client, inv: &Invitation, me: &str, status: Partstat) -> postman_jmap::Result<Option<()>> {
    if !client.has(postman_jmap::CALENDARS) {
        return Ok(None);
    }
    let Some(ev) = client.event_by_uid(&inv.event.uid).await? else { return Ok(None) };
    let (Some(id), Some(pid)) = (ev["id"].as_str(), participant_id(&ev, me)) else { return Ok(None) };
    client.reply(id, &pid, partstat_json(status)).await?;
    Ok(Some(()))
}

/// Положить событие в первый видимый календарь (отказ не рассылается — его не кладём).
pub async fn import(client: &Client, inv: &Invitation, me: &str, status: Partstat) -> postman_jmap::Result<bool> {
    if status == Partstat::Declined || !client.has(postman_jmap::CALENDARS) {
        return Ok(false);
    }
    let mut cals = client.calendars().await?;
    cals.sort_by_key(|c| (!c.is_visible, c.sort_order));
    let Some(cal) = cals.first() else { return Ok(false) };
    client.create_event(import_event(inv, me, status, &cal.id), false).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::TimeZone;

    fn inv() -> Invitation {
        let v = json!({ "method": "request", "data": {
            "@type": "Event", "uid": "u-1@partner.example", "title": "Созвон", "id": "srv",
            "start": "2026-10-20T10:00:00", "timeZone": "Europe/Moscow", "duration": "PT1H",
            "participants": {
                "o": { "name": "Иван", "calendarAddress": "mailto:ivan@partner.example", "roles": { "owner": true } },
                "m": { "calendarAddress": "mailto:me@corp.example", "roles": { "attendee": true }, "participationStatus": "needs-action" }
            }
        }});
        Invitation::from_worker(&v, &TimeZone::UTC).unwrap()
    }

    #[test]
    fn reply_goes_to_organizer() {
        let me = Address::new("Я", "me@corp.example");
        let d = reply_draft(&inv(), &me, Partstat::Tentative, Some("m1@partner.example".into()), 0).unwrap();
        assert_eq!(d.to, vec![Address::new("Иван", "ivan@partner.example")]);
        assert_eq!(d.subject, "Под вопросом: Созвон");
        assert_eq!(d.references, ["m1@partner.example"]);
        let cal = d.calendar.unwrap();
        assert_eq!(cal.method, "REPLY");
        assert!(cal.ics.contains("PARTSTAT=TENTATIVE"), "{}", cal.ics);
    }

    #[test]
    fn import_sets_calendar_and_status() {
        let ev = import_event(&inv(), "me@corp.example", Partstat::Accepted, "cal1");
        assert_eq!(ev["calendarIds"], json!({ "cal1": true }));
        assert_eq!(ev["participants"]["m"]["participationStatus"], "accepted");
        assert!(ev.get("id").is_none() && ev.get("method").is_none());
    }
}

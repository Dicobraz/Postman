//! Сборка iCalendar для CalDAV (S7-05б): встреча (VEVENT с участниками), блок времени задачи, задача на хранение
//! (VTODO), правка своего ответа участника и `SCHEDULE-AGENT` (RFC 6638 §7.1). Разбор чужих .ics — только в
//! воркере; здесь — своё событие или своя копия на своём сервере, построчно.
//!
//! `SCHEDULE-AGENT`: встреча — `SERVER` (приглашения, ответы, отмены рассылает сервер, неявное планирование);
//! задача и поручение — `CLIENT` (письма iTIP шлёт сам Postman, S6-09; сервер не должен слать свои).

use jiff::tz::TimeZone;

use crate::invite::{fold, text};
use crate::task::{cn, utc_ical};
use crate::{NewEvent, Partstat, Task};

/// Кто рассылает сообщения планирования участнику (RFC 6638 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAgent {
    Server,
    Client,
}

impl ScheduleAgent {
    fn ical(self) -> &'static str {
        match self {
            ScheduleAgent::Server => "SERVER",
            ScheduleAgent::Client => "CLIENT",
        }
    }
}

/// PARTSTAT iCalendar.
pub fn partstat_ical(s: Partstat) -> &'static str {
    match s {
        Partstat::NeedsAction => "NEEDS-ACTION",
        Partstat::Accepted => "ACCEPTED",
        Partstat::Declined => "DECLINED",
        Partstat::Tentative => "TENTATIVE",
        Partstat::Delegated => "DELEGATED",
    }
}

fn join(lines: &[String]) -> String {
    lines.iter().map(|x| fold(x)).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

impl NewEvent<'_> {
    /// Встреча для PUT в календарь CalDAV: время — UTC (без VTIMEZONE: зону сервер не обязан знать), организатор —
    /// `ORGANIZER` и участник-председатель, участники — `SCHEDULE-AGENT=SERVER`: приглашения рассылает сервер
    /// (`docs/STALWART.md` п. 47).
    pub fn to_ics(&self, now: i64) -> Result<String, String> {
        let tz = TimeZone::get(self.time_zone).unwrap_or(TimeZone::UTC);
        let start = self.start.to_zoned(tz).map_err(|e| e.to_string())?.timestamp().as_second();
        let mut l = vec![
            "BEGIN:VCALENDAR".to_string(),
            "PRODID:-//Postman//Postman//RU".into(),
            "VERSION:2.0".into(),
            "BEGIN:VEVENT".into(),
            format!("UID:{}", text(self.uid)),
            format!("DTSTAMP:{}", utc_ical(now)),
            format!("DTSTART:{}", utc_ical(start)),
            format!("DTEND:{}", utc_ical(start + self.duration_secs.max(0))),
            "SEQUENCE:0".into(),
            format!("SUMMARY:{}", text(self.title)),
        ];
        if !self.description.is_empty() {
            l.push(format!("DESCRIPTION:{}", text(self.description)));
        }
        if !self.location.is_empty() {
            l.push(format!("LOCATION:{}", text(self.location)));
        }
        if !self.attendees.is_empty() {
            let (name, email) = self.organizer;
            l.push(format!("ORGANIZER{}:mailto:{email}", cn(name)));
            l.push(format!("ATTENDEE{};ROLE=CHAIR;PARTSTAT=ACCEPTED:mailto:{email}", cn(name)));
            for (name, email, optional) in self.attendees {
                let role = if *optional { "OPT-PARTICIPANT" } else { "REQ-PARTICIPANT" };
                l.push(format!("ATTENDEE{};ROLE={role};PARTSTAT=NEEDS-ACTION;RSVP=TRUE;SCHEDULE-AGENT=SERVER:mailto:{email}", cn(name)));
            }
        }
        l.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
        Ok(join(&l))
    }
}

/// Задача на хранение в календаре CalDAV: VTODO без METHOD, участникам — `SCHEDULE-AGENT=CLIENT` (письма шага
/// поручения шлёт Postman, как JMAP без `sendSchedulingMessages`).
pub fn stored_task(t: &Task, now: i64, tz: &TimeZone) -> String {
    schedule_agent(&t.to_ics(None, now, tz), ScheduleAgent::Client)
}

/// Блок времени задачи (S6-08): «занят», без участников, со ссылкой на задачу (`RELATED-TO;RELTYPE=PARENT`) и
/// меткой `pm.task` — как `tasks::block_event` у JMAP.
pub fn block(uid: &str, title: &str, start: i64, end: i64, task_uid: &str, now: i64) -> String {
    join(&[
        "BEGIN:VCALENDAR".to_string(),
        "PRODID:-//Postman//Postman//RU".into(),
        "VERSION:2.0".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", text(uid)),
        format!("DTSTAMP:{}", utc_ical(now)),
        format!("DTSTART:{}", utc_ical(start)),
        format!("DTEND:{}", utc_ical(end.max(start + 60))),
        format!("SUMMARY:{}", text(title)),
        "TRANSP:OPAQUE".into(),
        "CATEGORIES:pm.task".into(),
        format!("RELATED-TO;RELTYPE=PARENT:{}", text(task_uid)),
        "END:VEVENT".into(),
        "END:VCALENDAR".into(),
    ])
}

/// Строки без переносов (RFC 5545 §3.1).
fn unfold(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = out.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !line.is_empty() {
            out.push(line.to_owned());
        }
    }
    out
}

/// Имя свойства строки (`ATTENDEE`, `ORGANIZER`), заглавными; часть до `:` вне кавычек — заголовок.
fn split(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}

fn set_param(head: &str, name: &str, value: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in head.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    let prefix = format!("{}=", name.to_ascii_uppercase());
    let mut found = false;
    for p in parts.iter_mut().skip(1) {
        if p.to_ascii_uppercase().starts_with(&prefix) {
            *p = format!("{prefix}{value}");
            found = true;
        }
    }
    if !found {
        parts.push(format!("{prefix}{value}"));
    }
    parts.join(";")
}

fn is(head: &str, name: &str) -> bool {
    head.split(';').next().is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// `SCHEDULE-AGENT` у каждого `ATTENDEE` (прежний — заменяется).
pub fn schedule_agent(ics: &str, agent: ScheduleAgent) -> String {
    let lines: Vec<String> = unfold(ics)
        .into_iter()
        .map(|l| match split(&l) {
            Some((head, value)) if is(head, "ATTENDEE") => format!("{}:{value}", set_param(head, "SCHEDULE-AGENT", agent.ical())),
            _ => l,
        })
        .collect();
    join(&lines)
}

/// Мой ответ в своей копии встречи (участник, RFC 6638 §3.2.2): `PARTSTAT` моей строки `ATTENDEE` во всех
/// компонентах, `RSVP` снят. `None` — меня среди участников нет.
pub fn with_partstat(ics: &str, me: &str, status: Partstat) -> Option<String> {
    let mine = format!("mailto:{}", me.to_ascii_lowercase());
    let mut found = false;
    let lines: Vec<String> = unfold(ics)
        .into_iter()
        .map(|l| match split(&l) {
            Some((head, value)) if is(head, "ATTENDEE") && value.trim().to_ascii_lowercase() == mine => {
                found = true;
                let head = set_param(head, "PARTSTAT", partstat_ical(status));
                let head: Vec<&str> = head.split(';').filter(|p| !p.to_ascii_uppercase().starts_with("RSVP=")).collect();
                format!("{}:{value}", head.join(";"))
            }
            _ => l,
        })
        .collect();
    found.then(|| join(&lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> i64 {
        s.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    #[test]
    fn meeting_is_a_vevent_scheduled_by_the_server() {
        let attendees = [("User 2", "user2@itt.test", false), ("Переговорная", "room@itt.test", true)];
        let e = NewEvent {
            uid: "u1@postman",
            calendar_id: "",
            title: "Планёрка; итоги",
            description: "",
            location: "Каб. 1",
            start: "2026-10-21T11:00:00".parse().unwrap(),
            time_zone: "Europe/Moscow",
            duration_secs: 1800,
            organizer: ("User 1", "user1@itt.test"),
            attendees: &attendees,
        };
        let ics = e.to_ics(at("2026-10-20T00:00:00Z")).unwrap();
        assert!(ics.contains("DTSTART:20261021T080000Z\r\nDTEND:20261021T083000Z\r\n"), "{ics}");
        assert!(ics.contains("SUMMARY:Планёрка\\; итоги\r\n") && ics.contains("LOCATION:Каб. 1\r\n"));
        assert!(ics.contains("ORGANIZER;CN=\"User 1\":mailto:user1@itt.test\r\n"));
        let lines = unfold(&ics);
        assert!(lines.iter().any(|l| l.starts_with("ATTENDEE;CN=\"User 2\";ROLE=REQ-PARTICIPANT;") && l.contains("SCHEDULE-AGENT=SERVER")));
        assert!(lines.iter().any(|l| l.contains("ROLE=OPT-PARTICIPANT") && l.ends_with("mailto:room@itt.test")));
        // Без участников — просто событие: ни организатора, ни рассылки.
        let alone = NewEvent { attendees: &[], ..e };
        assert!(!alone.to_ics(0).unwrap().contains("ORGANIZER"));
    }

    #[test]
    fn schedule_agent_is_set_on_every_attendee() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nORGANIZER:mailto:a@x\r\nATTENDEE;PARTSTAT=NEEDS-ACTION;CN=\"B; b\":mailto:b@x\r\nATTENDEE;SCHEDULE-AGENT=SERVER:mailto:c@x\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let out = schedule_agent(ics, ScheduleAgent::Client);
        let lines = unfold(&out);
        assert!(lines.contains(&"ATTENDEE;PARTSTAT=NEEDS-ACTION;CN=\"B; b\";SCHEDULE-AGENT=CLIENT:mailto:b@x".to_string()), "{lines:?}");
        assert!(lines.contains(&"ATTENDEE;SCHEDULE-AGENT=CLIENT:mailto:c@x".to_string()));
        assert!(lines.contains(&"ORGANIZER:mailto:a@x".to_string()));
    }

    #[test]
    fn my_answer_in_my_copy() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nORGANIZER:mailto:a@x\r\nATTENDEE;CN=\"Me\";PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:Me\r\n @X\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:other@x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = with_partstat(ics, "me@x", Partstat::Accepted).unwrap();
        let lines = unfold(&out);
        assert!(lines.iter().any(|l| l.starts_with("ATTENDEE;CN=\"Me\";PARTSTAT=ACCEPTED:") && !l.contains("RSVP")), "{lines:?}");
        assert!(lines.contains(&"ATTENDEE;PARTSTAT=ACCEPTED:mailto:other@x".to_string()));
        assert!(with_partstat(ics, "nobody@x", Partstat::Declined).is_none());
    }

    #[test]
    fn stored_task_and_block() {
        let mut t = Task::new("t1", "cal", "Отчёт");
        t.participants.push(crate::TaskParticipant {
            key: "p1".into(),
            name: "B".into(),
            email: "b@x".into(),
            role: crate::Role::Assignee,
            status: crate::TaskPartstat::NeedsAction,
        });
        let ics = stored_task(&t, 0, &TimeZone::UTC);
        assert!(!ics.contains("METHOD:") && ics.contains("BEGIN:VTODO"));
        assert!(unfold(&ics).iter().filter(|l| l.starts_with("ATTENDEE")).all(|l| l.contains("SCHEDULE-AGENT=CLIENT")));
        let b = block("b1", "Отчёт", 3600, 7200, "t1", 0);
        assert!(b.contains("DTSTART:19700101T010000Z\r\nDTEND:19700101T020000Z\r\n") && b.contains("RELATED-TO;RELTYPE=PARENT:t1\r\n"));
    }
}

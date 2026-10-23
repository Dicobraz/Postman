//! Планирование (RFC 6638). Занятость — POST VFREEBUSY в outbox (§5): ответ `schedule-response` по участнику со
//! строками `FREEBUSY` → `postman_calendar::Busy`. Приглашения, ответы и отмены рассылает сам сервер при PUT и
//! DELETE события с `ORGANIZER` и `ATTENDEE` (неявное планирование, §3): клиент лишь кладёт свою копию, у
//! участника — с `If-Schedule-Tag-Match` (`Precondition::ScheduleTag`); ответы и приглашения, положенные сервером,
//! лежат во входящих планирования (`Discovery::schedule_inbox`) и читаются как обычная коллекция.

use postman_calendar::{Busy, BusyKind, Span};

use crate::sync::{Resource, ical_utc};
use crate::xml::{CALDAV, DAV};
use crate::{Client, Error, Result};

/// Ответ outbox по одному участнику.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeBusyReply {
    /// Адрес участника без `mailto:`.
    pub email: String,
    /// `REQUEST-STATUS` (RFC 5545 §3.8.8.3): `2.0;Success`, `3.7;Invalid calendar user…`.
    pub status: String,
    /// Занятость окна; `None` — сервер её не дал (не наш пользователь, нет прав): «не известна», а не «свободен».
    pub busy: Option<Vec<Busy>>,
}

impl FreeBusyReply {
    /// Класс `2.x` — успех.
    pub fn ok(&self) -> bool {
        self.status.starts_with('2')
    }
}

impl Client {
    /// Занятость участников в окне от имени `organizer` (адрес учётной записи, `Discovery::email`). Stalwart
    /// v0.16.27 отдаёт её, только если календарь участника открыт запрашивающему с правами `read` и
    /// `read-free-busy` сразу (`docs/STALWART.md` п. 6).
    pub async fn free_busy(&self, outbox: &str, organizer: &str, attendees: &[&str], range: Span, uid: &str) -> Result<Vec<FreeBusyReply>> {
        let body = request_body(organizer, attendees, range, uid, jiff::Timestamp::now().as_second())?;
        let reply = self.send("POST", outbox, &[], Some(("text/calendar; charset=utf-8; method=REQUEST", body))).await?;
        if reply.status != 200 {
            return Err(crate::status_error(&reply));
        }
        parse_response(&reply.body)
    }

    /// Входящие планирования целиком (RFC 6638 §2.2): приглашения, ответы, отмены, положенные сервером; после
    /// обработки клиент их удаляет (`delete`). Stalwart v0.16.27 ресурсы ящика по `calendar-multiget` не отдаёт
    /// (404 на каждый) — недостающее читается GET по одному.
    pub async fn inbox(&self, inbox: &str) -> Result<Vec<Resource>> {
        let hrefs: Vec<String> = self.etags(inbox).await?.into_iter().map(|i| i.href).collect();
        let got = self.calendar_multiget(inbox, &hrefs).await?;
        let mut out = got.resources;
        for href in got.missing {
            match self.get(&href).await {
                Ok(r) => out.push(r),
                Err(Error::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
}

/// Строка свойства iCalendar без переводов строки и управляющих символов: адрес не должен вставить своё свойство.
fn clean(s: &str) -> Result<&str> {
    if s.is_empty() || s.chars().any(char::is_control) {
        return Err(Error::Protocol("bad calendar address".into()));
    }
    Ok(s)
}

/// Запрос VFREEBUSY (RFC 6638 §5, RFC 5546 §3.3.2).
fn request_body(organizer: &str, attendees: &[&str], range: Span, uid: &str, now: i64) -> Result<String> {
    let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Postman//DAV//RU\r\nMETHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\n");
    s.push_str(&format!("UID:{}\r\nDTSTAMP:{}\r\n", clean(uid)?, ical_utc(now)?));
    s.push_str(&format!("DTSTART:{}\r\nDTEND:{}\r\n", ical_utc(range.start)?, ical_utc(range.end)?));
    s.push_str(&format!("ORGANIZER:mailto:{}\r\n", clean(organizer)?));
    for a in attendees {
        s.push_str(&format!("ATTENDEE:mailto:{}\r\n", clean(a)?));
    }
    s.push_str("END:VFREEBUSY\r\nEND:VCALENDAR\r\n");
    Ok(s)
}

/// Разобрать `schedule-response`.
pub fn parse_response(body: &[u8]) -> Result<Vec<FreeBusyReply>> {
    let root = crate::xml::parse(body)?;
    if !root.is(CALDAV, "schedule-response") {
        return Err(Error::Protocol(format!("expected schedule-response, got {}", root.name)));
    }
    let mut out = Vec::new();
    for r in root.all(CALDAV, "response") {
        let Some(recipient) = r.child(CALDAV, "recipient").and_then(|p| p.child(DAV, "href")).map(|h| h.value().to_owned()) else {
            continue;
        };
        let email = crate::discover::strip_mailto(&recipient).unwrap_or(&recipient).to_owned();
        let status = r.child(CALDAV, "request-status").map(|s| s.value().to_owned()).unwrap_or_default();
        let busy = if status.starts_with('2') { r.child(CALDAV, "calendar-data").map(|d| busy_periods(&d.text)) } else { None };
        out.push(FreeBusyReply { email, status, busy });
    }
    Ok(out)
}

/// Строки iCalendar с развёрнутыми продолжениями (RFC 5545 §3.1).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        lines.push(line.to_owned());
    }
    lines
}

/// Периоды `FREEBUSY` ответа сервера: `FBTYPE` BUSY (по умолчанию), BUSY-TENTATIVE, BUSY-UNAVAILABLE; FREE и
/// неразборчивые периоды пропускаются. Период — `начало/конец` или `начало/длительность`, время — UTC.
pub fn busy_periods(ics: &str) -> Vec<Busy> {
    let mut out = Vec::new();
    for line in unfold(ics) {
        let Some((head, value)) = line.split_once(':') else { continue };
        let mut parts = head.split(';');
        if !parts.next().is_some_and(|n| n.eq_ignore_ascii_case("FREEBUSY")) {
            continue;
        }
        let fbtype = parts.find_map(|p| p.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case("FBTYPE")).map(|(_, v)| v.to_ascii_uppercase()));
        let kind = match fbtype.as_deref() {
            None | Some("BUSY") => BusyKind::Busy,
            Some("BUSY-TENTATIVE") => BusyKind::Tentative,
            Some("BUSY-UNAVAILABLE") => BusyKind::Unavailable,
            Some("FREE") => continue,
            // Неизвестный тип — как BUSY (RFC 5545 §3.2.9).
            Some(_) => BusyKind::Busy,
        };
        for period in value.split(',') {
            if let Some(span) = period_span(period.trim()) {
                out.push(Busy { span, kind });
            }
        }
    }
    out
}

fn period_span(p: &str) -> Option<Span> {
    let (a, b) = p.split_once('/')?;
    let start = utc_time(a)?;
    let end = if b.starts_with('P') { start + postman_calendar::parse_duration(b).ok()? } else { utc_time(b)? };
    (end > start).then(|| Span::new(start, end))
}

fn utc_time(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let dt = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%S", s).ok()?;
    Some(dt.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp().as_second())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> i64 {
        utc_time(s).unwrap()
    }

    #[test]
    fn stand_answer_has_busy_unknown_and_empty() {
        let replies = parse_response(include_bytes!("../tests/fixtures/free-busy.xml")).unwrap();
        assert_eq!(replies.len(), 3);
        let user3 = replies.iter().find(|r| r.email == "user3@itt.test").unwrap();
        assert_eq!((user3.ok(), user3.busy.as_deref()), (true, Some(&[][..])), "свободен — пустой список");
        let nobody = replies.iter().find(|r| r.email == "nobody@itt.test").unwrap();
        assert!(!nobody.ok() && nobody.busy.is_none() && nobody.status.starts_with("3.7"));
        let user2 = replies.iter().find(|r| r.email == "user2@itt.test").unwrap();
        // Строка FREEBUSY свёрнута (перенос посреди времени) — разворачивается.
        assert_eq!(
            user2.busy.as_deref().unwrap(),
            [
                Busy { span: Span::new(at("20261021T090000Z"), at("20261021T100000Z")), kind: BusyKind::Busy },
                Busy { span: Span::new(at("20261028T110000Z"), at("20261028T120000Z")), kind: BusyKind::Busy },
            ]
        );
    }

    #[test]
    fn fbtypes_durations_and_garbage() {
        let ics = "BEGIN:VFREEBUSY\r\nFREEBUSY;FBTYPE=BUSY-TENTATIVE:20261021T090000Z/PT30M\r\nFREEBUSY:20261021T120000Z/20261021T130000Z\r\nFREEBUSY;FBTYPE=FREE:20261021T140000Z/PT1H\r\nfreebusy;fbtype=busy-unavailable:20261022T000000Z/P1D\r\nFREEBUSY:garbage,20261021T150000/PT1H\r\nEND:VFREEBUSY\r\n";
        let b = busy_periods(ics);
        assert_eq!(b.len(), 3, "{b:?}");
        assert_eq!(b[0], Busy { span: Span::new(at("20261021T090000Z"), at("20261021T093000Z")), kind: BusyKind::Tentative });
        assert_eq!(b[1].kind, BusyKind::Busy);
        assert_eq!((b[2].kind, b[2].span.len()), (BusyKind::Unavailable, 86_400));
    }

    #[test]
    fn request_is_vfreebusy_with_attendees() {
        let r = request_body("user1@itt.test", &["user2@itt.test", "user3@itt.test"], Span::new(0, 3600), "fb-1", 60).unwrap();
        assert!(r.starts_with("BEGIN:VCALENDAR\r\n") && r.contains("METHOD:REQUEST\r\n"));
        assert!(r.contains("DTSTART:19700101T000000Z\r\nDTEND:19700101T010000Z\r\n"));
        assert!(r.contains("ORGANIZER:mailto:user1@itt.test\r\nATTENDEE:mailto:user2@itt.test\r\nATTENDEE:mailto:user3@itt.test\r\n"));
        assert!(request_body("a@b\r\nATTENDEE:mailto:x@y", &[], Span::new(0, 1), "u", 0).is_err());
        assert!(request_body("a@b", &[""], Span::new(0, 1), "u", 0).is_err());
    }

    #[test]
    fn other_root_is_an_error() {
        assert!(parse_response(b"<d:multistatus xmlns:d=\"DAV:\"/>").is_err());
    }
}

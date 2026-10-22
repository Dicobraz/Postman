//! Календарь на сервере (S3-05): неделя из `CalendarEvent/query` (повторения разворачивает сервер), занятость
//! коллег из `Principal/getAvailability`, встреча с приглашениями (`sendSchedulingMessages`), открытие своей
//! занятости коллеге (`Calendar/set shareWith`). Поведение сервера — `docs/STALWART.md`.
//!
//! Чистые функции (разбор, перевод в занятость, встреча → JSCalendar) проверяются тестами; сетевые — тестом
//! `stand`. Вызывает их поток синхронизации (`sync`).

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use postman_calendar::{Busy, BusyKind, Event, EventStatus, FreeBusy, NewEvent, Owner};
use postman_jmap::{BusyPeriod, Calendar, Client, Error, Identity, Result};
use serde_json::Value;

/// Новая встреча из окна встречи: всё своё (команда уходит в поток синхронизации). Календарь и организатора
/// подставляет синхронизация: календарь по умолчанию и адрес учётной записи.
#[derive(Debug, Clone, PartialEq)]
pub struct Meeting {
    pub uid: String,
    pub title: String,
    pub location: String,
    pub description: String,
    /// Локальное начало в зоне `time_zone`.
    pub start: DateTime,
    pub time_zone: String,
    pub duration_secs: i64,
    /// Обязательные участники: (имя, адрес).
    pub attendees: Vec<(String, String)>,
    /// Необязательные участники (и переговорные, ресурсы): (имя, адрес).
    pub optional: Vec<(String, String)>,
}

impl Meeting {
    /// Участники в форме `NewEvent`: обязательные, затем необязательные.
    pub fn attendee_refs(&self) -> Vec<(&str, &str, bool)> {
        let required = self.attendees.iter().map(|(n, e)| (n.as_str(), e.as_str(), false));
        required.chain(self.optional.iter().map(|(n, e)| (n.as_str(), e.as_str(), true))).collect()
    }

    /// Событие для `CalendarEvent/set`; `attendees` — из [`Meeting::attendee_refs`].
    pub fn new_event<'a>(
        &'a self,
        calendar_id: &'a str,
        organizer: (&'a str, &'a str),
        attendees: &'a [(&'a str, &'a str, bool)],
    ) -> NewEvent<'a> {
        NewEvent {
            uid: &self.uid,
            calendar_id,
            title: &self.title,
            description: &self.description,
            location: &self.location,
            start: self.start,
            time_zone: &self.time_zone,
            duration_secs: self.duration_secs,
            organizer,
            attendees,
        }
    }

    pub fn to_jmap(&self, calendar_id: &str, organizer: (&str, &str)) -> Value {
        let attendees = self.attendee_refs();
        self.new_event(calendar_id, organizer, &attendees).to_jmap()
    }
}

/// Unix-секунды → `YYYY-MM-DDTHH:MM:SSZ` (фильтры и запросы JMAP).
pub fn utc(ts: i64) -> String {
    jiff::Timestamp::from_second(ts).unwrap_or(jiff::Timestamp::UNIX_EPOCH).strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// События недели: разобранные, без отменённых, по времени начала. Неразборчивое событие пропускается —
/// неделя показывается без него.
pub fn parse_events(values: &[Value], tz: &TimeZone) -> Vec<Event> {
    let mut out: Vec<Event> = values
        .iter()
        .filter_map(|v| Event::from_jmap(v, tz).ok())
        .filter(|e| e.status != EventStatus::Cancelled)
        .collect();
    out.sort_by(|a, b| (a.span.start, a.span.end, &a.title).cmp(&(b.span.start, b.span.end, &b.title)));
    out
}

/// Моя занятость из моих событий: «свободно» (`freeBusyStatus: free`) и отменённые не считаются.
pub fn busy_from_events(events: &[Event]) -> Vec<Busy> {
    events
        .iter()
        .filter(|e| e.free_busy == FreeBusy::Busy && e.status != EventStatus::Cancelled)
        .map(|e| Busy {
            span: e.span,
            kind: if e.status == EventStatus::Tentative { BusyKind::Tentative } else { BusyKind::Busy },
        })
        .collect()
}

/// Ответ `Principal/getAvailability` → занятость. Пустой список — `None`, «нет данных»: Stalwart отдаёт пустой
/// список без ошибки, если календарь не открыт (`docs/STALWART.md` п. 5), — это не «свободен».
pub fn busy_from_periods(periods: &[BusyPeriod]) -> Option<Vec<Busy>> {
    let busy: Vec<Busy> = periods
        .iter()
        .filter_map(|p| {
            let span = postman_calendar::Span::new(ts(&p.utc_start)?, ts(&p.utc_end)?);
            let kind = match p.busy_status.as_deref() {
                Some("tentative") => BusyKind::Tentative,
                Some("unavailable") => BusyKind::Unavailable,
                _ => BusyKind::Busy,
            };
            Some(Busy { span, kind })
        })
        .collect();
    (!busy.is_empty()).then_some(busy)
}

fn ts(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

/// Календарь для новых встреч: первый видимый по порядку.
pub fn default_calendar(calendars: &[Calendar]) -> Option<&Calendar> {
    calendars.iter().min_by_key(|c| (!c.is_visible, c.sort_order))
}

/// События в `[from, to)`, повторения развёрнуты сервером.
pub async fn week(client: &Client, from: i64, to: i64) -> Result<Vec<Event>> {
    let values = client.events(None, &utc(from), &utc(to)).await?;
    Ok(parse_events(&values, &TimeZone::system()))
}

/// Неделя с календарями коллег (S5-22): мои события и события календарей, открытых мне (`mayReadItems`) — отдельные
/// аккаунты сессии с календарями (`docs/STALWART.md` п. 44); у их событий — владелец (адрес аккаунта и имя из
/// справочника). Календарь коллеги, который не ответил, пропускается: его неделя — без него. Коллеги — все открывшие,
/// и без событий в неделе (список слева в календаре).
pub async fn week_with_colleagues(client: &Client, from: i64, to: i64) -> Result<(Vec<Event>, Vec<Owner>)> {
    let mut events = week(client, from, to).await?;
    let mut colleagues = Vec::new();
    let tz = TimeZone::system();
    for (account, email) in client.session().shared_calendar_accounts() {
        let name = match client.principal_by_email(&email).await {
            Ok(Some(p)) => p.display_name().to_owned(),
            _ => email.clone(),
        };
        let owner = Owner { email, name };
        if let Ok(values) = client.events_of(&account, None, &utc(from), &utc(to)).await {
            // Встреча, где и я участник, уже есть в моём календаре — копию коллеги не показываем.
            let theirs: Vec<Event> = parse_events(&values, &tz)
                .into_iter()
                .filter(|e| !events.iter().any(|m| m.owner.is_none() && m.uid == e.uid && m.span.start == e.span.start))
                .map(|mut e| {
                    e.owner = Some(owner.clone());
                    e
                })
                .collect();
            events.extend(theirs);
        }
        colleagues.push(owner);
    }
    events.sort_by(|a, b| (a.span.start, a.span.end, &a.title).cmp(&(b.span.start, b.span.end, &b.title)));
    Ok((events, colleagues))
}

/// Найденных событий в календаре — не больше (S5-21: список под поиском).
pub const SEARCH_LIMIT: usize = 100;

/// Поиск в календаре (S5-21): события с текстом `text` в `[from, to)` — по теме, месту, описанию и участникам на
/// сервере (`filter.text`), по времени начала.
pub async fn search(client: &Client, text: &str, from: i64, to: i64) -> Result<Vec<Event>> {
    let values = client.search_events(text, &utc(from), &utc(to), SEARCH_LIMIT).await?;
    Ok(parse_events(&values, &TimeZone::system()))
}

/// Занятость в `[from, to)`: моя (из моих событий) и каждого адреса (`None` — нет данных: сотрудник не найден,
/// календарь не открыт мне или сервер не ответил).
pub async fn availability(
    client: &Client,
    emails: &[String],
    from: i64,
    to: i64,
) -> Result<(Vec<Busy>, Vec<(String, Option<Vec<Busy>>)>)> {
    let mine = busy_from_events(&week(client, from, to).await?);
    let mut people = Vec::with_capacity(emails.len());
    for email in emails {
        let busy = match principal(client, email).await {
            Ok(Some(id)) => {
                client.availability(&id, &utc(from), &utc(to)).await.ok().and_then(|p| busy_from_periods(&p))
            }
            _ => None,
        };
        people.push((email.clone(), busy));
    }
    Ok((mine, people))
}

/// Сотрудник по адресу: фильтр `email`, иначе поиск по тексту.
async fn principal(client: &Client, email: &str) -> Result<Option<String>> {
    if let Some(p) = client.principal_by_email(email).await? {
        return Ok(Some(p.id));
    }
    let found = client.principals(email, 5).await?;
    Ok(found.into_iter().find(|p| p.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(email))).map(|p| p.id))
}

/// Создать встречу и разослать приглашения (сервер); организатор — первая Identity. Возвращает id события.
pub async fn create(client: &Client, identities: &[Identity], meeting: &Meeting) -> Result<String> {
    let me = identities.first().ok_or_else(|| Error::Protocol("на сервере нет Identity для организатора".into()))?;
    let calendars = client.calendars().await?;
    let cal = default_calendar(&calendars).ok_or_else(|| Error::Protocol("у учётной записи нет календаря".into()))?;
    let send = !meeting.attendees.is_empty() || !meeting.optional.is_empty();
    client.create_event(meeting.to_jmap(&cal.id, (&me.name, &me.email)), send).await
}

/// Открыть коллеге (по адресу) только занятость всех своих календарей — без подробностей событий.
pub async fn share_free_busy(client: &Client, email: &str) -> Result<()> {
    let id = principal(client, email).await?.ok_or_else(|| Error::Protocol(format!("сотрудник {email} не найден")))?;
    let calendars = client.calendars().await?;
    if calendars.is_empty() {
        return Err(Error::Protocol("у учётной записи нет календаря".into()));
    }
    for c in &calendars {
        client.share_calendar(&c.id, &id, false).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(s: &str) -> i64 {
        s.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    #[test]
    fn jmap_events_become_the_week() {
        let values = vec![
            json!({ "id": "b", "title": "Позже", "utcStart": "2026-10-20T12:00:00Z", "utcEnd": "2026-10-20T13:00:00Z" }),
            // Экземпляр повторения: сервер отдал его со своим `recurrenceId` и UTC-временем.
            json!({ "id": "a", "title": "Планёрка", "recurrenceId": "2026-10-20T09:00:00", "start": "2026-10-20T09:00:00",
                    "timeZone": "Europe/Moscow", "duration": "PT30M", "status": "tentative" }),
            json!({ "id": "c", "title": "Отменена", "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z",
                    "status": "cancelled" }),
            json!({ "id": "d", "title": "Битое", "start": "не время" }),
        ];
        let week = parse_events(&values, &TimeZone::UTC);
        assert_eq!(week.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(week[0].span.start, at("2026-10-20T06:00:00Z"));
        assert!(week[0].recurring);
        let busy = busy_from_events(&week);
        assert_eq!(busy[0].kind, BusyKind::Tentative);
        assert_eq!(
            busy[1],
            Busy {
                span: postman_calendar::Span::new(at("2026-10-20T12:00:00Z"), at("2026-10-20T13:00:00Z")),
                kind: BusyKind::Busy
            }
        );
        // «Свободно» в событии — не занятость.
        let free = parse_events(
            &[
                json!({ "utcStart": "2026-10-20T12:00:00Z", "utcEnd": "2026-10-20T13:00:00Z", "freeBusyStatus": "free" }),
            ],
            &TimeZone::UTC,
        );
        assert!(busy_from_events(&free).is_empty());
    }

    #[test]
    fn empty_availability_is_no_data_not_free() {
        assert_eq!(busy_from_periods(&[]), None);
        let p = |s: &str, e: &str, k: Option<&str>| BusyPeriod {
            utc_start: s.into(),
            utc_end: e.into(),
            busy_status: k.map(str::to_owned),
        };
        let busy = busy_from_periods(&[
            p("2026-10-20T07:00:00Z", "2026-10-20T08:00:00Z", None),
            p("2026-10-20T09:00:00Z", "2026-10-20T10:00:00Z", Some("tentative")),
            p("2026-10-20T11:00:00Z", "2026-10-20T12:00:00Z", Some("unavailable")),
            p("плохо", "2026-10-20T12:00:00Z", None),
        ])
        .unwrap();
        assert_eq!(
            busy.iter().map(|b| b.kind).collect::<Vec<_>>(),
            [BusyKind::Busy, BusyKind::Tentative, BusyKind::Unavailable]
        );
        assert_eq!(busy[0].span.start, at("2026-10-20T07:00:00Z"));
    }

    #[test]
    fn meeting_becomes_an_invitation() {
        let m = Meeting {
            uid: "u1@postman".into(),
            title: "Планёрка".into(),
            location: "Переговорная 1".into(),
            description: String::new(),
            start: "2026-10-21T11:00:00".parse().unwrap(),
            time_zone: "Europe/Moscow".into(),
            duration_secs: 1800,
            attendees: vec![("User 3".into(), "user3@itt.test".into())],
            optional: vec![("Переговорная 1".into(), "room1@itt.test".into())],
        };
        let v = m.to_jmap("cal1", ("User 1", "user1@itt.test"));
        assert_eq!(v["calendarIds"]["cal1"], true);
        assert_eq!(v["start"], "2026-10-21T11:00:00");
        assert_eq!(v["duration"], "PT30M");
        assert_eq!(v["organizerCalendarAddress"], "mailto:user1@itt.test");
        assert_eq!(v["participants"]["a0"]["calendarAddress"], "mailto:user3@itt.test");
        assert_eq!(v["participants"]["a1"]["calendarAddress"], "mailto:room1@itt.test");
        assert_eq!(v["participants"]["a1"]["roles"]["optional"], true);
        assert_eq!(v["locations"]["l1"]["name"], "Переговорная 1");
        assert_eq!(utc(at("2026-10-21T08:00:00Z")), "2026-10-21T08:00:00Z");
    }

    #[test]
    fn default_calendar_is_first_visible() {
        let c = |id: &str, order: u32, visible: bool| Calendar {
            id: id.into(),
            name: id.into(),
            color: None,
            sort_order: order,
            is_visible: visible,
            is_subscribed: true,
        };
        let cals = [c("hidden", 0, false), c("b", 2, true), c("a", 1, true)];
        assert_eq!(default_calendar(&cals).map(|c| c.id.as_str()), Some("a"));
        assert!(default_calendar(&[]).is_none());
    }
}

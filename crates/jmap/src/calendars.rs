//! JMAP for Calendars (draft-ietf-jmap-calendars) и Principals (RFC 9670).
//! События возвращаются как JSON (JSCalendar) — разбор в `postman-calendar`.
//! Имена методов и аргументов — по черновику; поведение Stalwart проверено на стенде (`docs/STALWART.md`,
//! `crates/core/tests/stand.rs`).

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mail::{GetResponse, inv, set_error};
use crate::{CALENDARS, CORE, Client, Error, PRINCIPALS, PRINCIPALS_AVAILABILITY, Result, take};

const USING: &[&str] = &[CORE, CALENDARS, PRINCIPALS];

/// Свойства события, нужные для отображения; `utcStart`/`utcEnd` вычисляет сервер.
pub const EVENT_PROPERTIES: &[&str] = &[
    "id", "uid", "calendarIds", "title", "description", "start", "timeZone", "duration", "showWithoutTime",
    "status", "freeBusyStatus", "participants", "locations", "recurrenceRules", "recurrenceId", "utcStart",
    "utcEnd",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    #[serde(default)]
    pub sort_order: u32,
    #[serde(default = "yes")]
    pub is_visible: bool,
    #[serde(default)]
    pub is_subscribed: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub email: Option<String>,
    /// individual, group, resource, location…
    #[serde(rename = "type", default)]
    pub kind: String,
}

/// Период занятости из `Principal/getAvailability`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyPeriod {
    pub utc_start: String,
    pub utc_end: String,
    /// confirmed | tentative | unavailable
    #[serde(default)]
    pub busy_status: Option<String>,
}

#[derive(Deserialize)]
struct AvailabilityResponse {
    list: Vec<BusyPeriod>,
}

impl Client {
    pub fn calendar_account(&self) -> Result<&str> {
        self.account_for(CALENDARS)
    }

    pub async fn calendars(&self) -> Result<Vec<Calendar>> {
        let acc = self.calendar_account()?;
        let mut r = self.call(USING, &[inv("Calendar/get", json!({ "accountId": acc, "ids": null }), "c")]).await?;
        let g: GetResponse<Calendar> = take(&mut r, "c")?;
        Ok(g.list)
    }

    /// События в интервале (UTC, `YYYY-MM-DDTHH:MM:SSZ`), повторения разворачивает сервер.
    pub async fn events(&self, calendar_ids: Option<&[String]>, utc_after: &str, utc_before: &str) -> Result<Vec<Value>> {
        let acc = self.calendar_account()?;
        let mut filter = json!({ "after": utc_after, "before": utc_before });
        if let Some(ids) = calendar_ids {
            filter["inCalendars"] = json!(ids);
        }
        let calls = [
            inv(
                "CalendarEvent/query",
                json!({ "accountId": acc, "filter": filter, "expandRecurrences": true, "sort": [{ "property": "start" }] }),
                "q",
            ),
            inv(
                "CalendarEvent/get",
                json!({
                    "accountId": acc,
                    "#ids": { "resultOf": "q", "name": "CalendarEvent/query", "path": "/ids" },
                    "properties": EVENT_PROPERTIES,
                }),
                "g",
            ),
        ];
        let mut r = self.call(USING, &calls).await?;
        let g: GetResponse<Value> = take(&mut r, "g")?;
        Ok(g.list)
    }

    /// Создать событие; `send` — разослать приглашения (iTIP/iMIP силами сервера).
    pub async fn create_event(&self, event: Value, send: bool) -> Result<String> {
        let acc = self.calendar_account()?;
        let args = json!({ "accountId": acc, "create": { "new": event }, "sendSchedulingMessages": send });
        let mut r = self.call(USING, &[inv("CalendarEvent/set", args, "s")]).await?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notCreated", "new", "CalendarEvent/set")?;
        v["created"]["new"]["id"].as_str().map(str::to_string).ok_or_else(|| Error::Protocol("CalendarEvent/set не вернул id".into()))
    }

    /// Событие по UID (приглашение, пришедшее письмом, могло уже лечь в календарь сервером).
    /// У приглашённого своё id события — искать только так (фильтр `uid` черновика; Stalwart — да).
    pub async fn event_by_uid(&self, uid: &str) -> Result<Option<Value>> {
        let acc = self.calendar_account()?;
        let calls = [
            inv("CalendarEvent/query", json!({ "accountId": acc, "filter": { "uid": uid }, "limit": 1 }), "q"),
            inv(
                "CalendarEvent/get",
                json!({
                    "accountId": acc,
                    "#ids": { "resultOf": "q", "name": "CalendarEvent/query", "path": "/ids" },
                    "properties": EVENT_PROPERTIES,
                }),
                "g",
            ),
        ];
        let mut r = self.call(USING, &calls).await?;
        let g: GetResponse<Value> = take(&mut r, "g")?;
        Ok(g.list.into_iter().find(|e| e["uid"].as_str() == Some(uid)))
    }

    /// Изменить событие патчем (`"title": …`, `"participants/<id>/participationStatus": …`).
    pub async fn update_event(&self, id: &str, patch: Map<String, Value>, send: bool) -> Result<()> {
        let acc = self.calendar_account()?;
        let args = json!({ "accountId": acc, "update": { id: patch }, "sendSchedulingMessages": send });
        let mut r = self.call(USING, &[inv("CalendarEvent/set", args, "s")]).await?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notUpdated", id, "CalendarEvent/set")
    }

    /// Удалить событие; `send` — организатор рассылает отмену (CANCEL), участник — отказ.
    pub async fn destroy_event(&self, id: &str, send: bool) -> Result<()> {
        let acc = self.calendar_account()?;
        let args = json!({ "accountId": acc, "destroy": [id], "sendSchedulingMessages": send });
        let mut r = self.call(USING, &[inv("CalendarEvent/set", args, "s")]).await?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notDestroyed", id, "CalendarEvent/set")
    }

    /// Открыть календарь сотруднику (`principal_id`): только занятость или ещё и события.
    /// Без этого `Principal/getAvailability` у него — пустой список без ошибки (Stalwart).
    pub async fn share_calendar(&self, calendar_id: &str, principal_id: &str, items: bool) -> Result<()> {
        let acc = self.calendar_account()?;
        let mut patch = Map::new();
        patch.insert(format!("shareWith/{principal_id}"), json!({ "mayReadFreeBusy": true, "mayReadItems": items }));
        let args = json!({ "accountId": acc, "update": { calendar_id: patch } });
        let mut r = self.call(USING, &[inv("Calendar/set", args, "s")]).await?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notUpdated", calendar_id, "Calendar/set")
    }

    /// Ответ на приглашение: accepted | tentative | declined.
    pub async fn reply(&self, event_id: &str, participant_id: &str, status: &str) -> Result<()> {
        let mut patch = Map::new();
        patch.insert(format!("participants/{participant_id}/participationStatus"), json!(status));
        self.update_event(event_id, patch, true).await
    }

    /// Поиск сотрудников, переговорных и ресурсов (для приглашений и помощника).
    pub async fn principals(&self, text: &str, limit: u64) -> Result<Vec<Principal>> {
        let acc = self.account_for(PRINCIPALS)?;
        let calls = [
            inv("Principal/query", json!({ "accountId": acc, "filter": { "text": text }, "limit": limit }), "q"),
            inv(
                "Principal/get",
                json!({ "accountId": acc, "#ids": { "resultOf": "q", "name": "Principal/query", "path": "/ids" } }),
                "g",
            ),
        ];
        let mut r = self.call(USING, &calls).await?;
        let g: GetResponse<Principal> = take(&mut r, "g")?;
        Ok(g.list)
    }

    /// Сотрудник по адресу (фильтр `email`).
    pub async fn principal_by_email(&self, email: &str) -> Result<Option<Principal>> {
        let acc = self.account_for(PRINCIPALS)?;
        let calls = [
            inv("Principal/query", json!({ "accountId": acc, "filter": { "email": email }, "limit": 5 }), "q"),
            inv(
                "Principal/get",
                json!({ "accountId": acc, "#ids": { "resultOf": "q", "name": "Principal/query", "path": "/ids" } }),
                "g",
            ),
        ];
        let mut r = self.call(USING, &calls).await?;
        let g: GetResponse<Principal> = take(&mut r, "g")?;
        Ok(g.list.into_iter().find(|p| p.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(email))))
    }

    /// Занятость сотрудника в интервале — без подробностей событий. Пусто, если календарь не открыт
    /// (`share_calendar`): ошибки сервер не даёт.
    pub async fn availability(&self, principal_id: &str, utc_start: &str, utc_end: &str) -> Result<Vec<BusyPeriod>> {
        let acc = self.account_for(PRINCIPALS)?;
        let args = json!({ "accountId": acc, "id": principal_id, "utcStart": utc_start, "utcEnd": utc_end, "showDetails": false });
        let using = [CORE, CALENDARS, PRINCIPALS, PRINCIPALS_AVAILABILITY];
        let mut r = self.call(&using, &[inv("Principal/getAvailability", args, "a")]).await?;
        let a: AvailabilityResponse = take(&mut r, "a")?;
        Ok(a.list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_shape() {
        let v = json!({ "list": [{ "utcStart": "2026-10-20T07:00:00Z", "utcEnd": "2026-10-20T08:00:00Z", "busyStatus": "tentative" }] });
        let a: AvailabilityResponse = serde_json::from_value(v).unwrap();
        assert_eq!(a.list[0].busy_status.as_deref(), Some("tentative"));
        let c: Calendar = serde_json::from_value(json!({ "id": "c1", "name": "Рабочий" })).unwrap();
        assert!(c.is_visible);
    }
}

//! Занятость (`GetUserAvailability`, вид `FreeBusy`) → интервалы [`postman_calendar::Busy`] для «Помощника
//! планирования». Окно и ответ — в UTC (пояс запроса — UTC без перевода часов).

use postman_calendar::{Busy, BusyKind, Span};

use crate::calendar::parse_time;
use crate::soap;
use crate::xml::{El, esc};
use crate::{Client, Error, Result};

/// Занятость одного адреса. `error` — код сервера (`ErrorMailRecipientNotFound`, `ErrorNoFreeBusyAccess`…): занятость
/// не известна, не «свободен».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub email: String,
    pub busy: Vec<Busy>,
    pub error: Option<String>,
}

/// Пояс запроса: UTC (смещение 0, без перевода часов) — `StandardTime` и `DaylightTime` одинаковые.
const UTC: &str = concat!(
    "<t:TimeZone><t:Bias>0</t:Bias>",
    "<t:StandardTime><t:Bias>0</t:Bias><t:Time>00:00:00</t:Time><t:DayOrder>1</t:DayOrder><t:Month>1</t:Month><t:DayOfWeek>Sunday</t:DayOfWeek></t:StandardTime>",
    "<t:DaylightTime><t:Bias>0</t:Bias><t:Time>00:00:00</t:Time><t:DayOrder>1</t:DayOrder><t:Month>1</t:Month><t:DayOfWeek>Sunday</t:DayOfWeek></t:DaylightTime>",
    "</t:TimeZone>"
);

/// Время окна в поясе запроса (UTC), без суффикса пояса — так требует `TimeWindow`.
fn local(ts: i64) -> String {
    let s = crate::calendar::format_time(ts);
    s.trim_end_matches('Z').to_owned()
}

pub(crate) fn request(emails: &[&str], window: Span) -> String {
    let mailboxes: String = emails
        .iter()
        .map(|e| {
            format!(
                "<t:MailboxData><t:Email><t:Address>{}</t:Address></t:Email><t:AttendeeType>Required</t:AttendeeType><t:ExcludeConflicts>false</t:ExcludeConflicts></t:MailboxData>",
                esc(e)
            )
        })
        .collect();
    format!(
        "<m:GetUserAvailabilityRequest>{UTC}<m:MailboxDataArray>{mailboxes}</m:MailboxDataArray><t:FreeBusyViewOptions><t:TimeWindow><t:StartTime>{}</t:StartTime><t:EndTime>{}</t:EndTime></t:TimeWindow><t:MergedFreeBusyIntervalInMinutes>30</t:MergedFreeBusyIntervalInMinutes><t:RequestedView>FreeBusy</t:RequestedView></t:FreeBusyViewOptions></m:GetUserAvailabilityRequest>",
        local(window.start),
        local(window.end)
    )
}

/// Ответы — в порядке адресов запроса.
pub(crate) fn parse(resp: &El, emails: &[&str]) -> Result<Vec<Availability>> {
    let array = resp.child("FreeBusyResponseArray").ok_or_else(|| Error::Xml("no FreeBusyResponseArray".into()))?;
    let mut out = Vec::new();
    for (i, r) in array.all("FreeBusyResponse").enumerate() {
        let email = emails.get(i).map(|e| (*e).to_owned()).unwrap_or_default();
        let msg = r.child("ResponseMessage");
        if let Some(Err(e)) = msg.map(soap::check) {
            let code = match e {
                Error::Ews { code, .. } => code,
                e => return Err(e),
            };
            out.push(Availability { email, busy: Vec::new(), error: Some(code) });
            continue;
        }
        let busy = r
            .at(&["FreeBusyView", "CalendarEventArray"])
            .into_iter()
            .flat_map(|a| a.all("CalendarEvent"))
            .filter_map(|e| {
                let kind = match e.text_at(&["BusyType"])? {
                    "Busy" => BusyKind::Busy,
                    "Tentative" => BusyKind::Tentative,
                    "OOF" => BusyKind::Unavailable,
                    _ => return None, // Free, NoData
                };
                let span = Span::new(parse_time(e.text_at(&["StartTime"])?)?, parse_time(e.text_at(&["EndTime"])?)?);
                Some(Busy { span, kind })
            })
            .collect();
        out.push(Availability { email, busy, error: None });
    }
    Ok(out)
}

impl Client {
    /// Занятость адресов в окне (до 100 адресов и 42 дней — пределы сервера; вызывающий держит меньше).
    pub async fn availability(&mut self, emails: &[&str], window: Span) -> Result<Vec<Availability>> {
        if emails.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.call("GetUserAvailability", &request(emails, window)).await?;
        parse(&resp, emails)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse as xml;

    #[test]
    fn busy_intervals() {
        let resp = soap::response(xml(include_bytes!("../tests/fixtures/get_user_availability.xml")).unwrap()).unwrap();
        let emails = ["boris@example.test", "vera@example.test", "nobody@example.test"];
        let a = parse(&resp, &emails).unwrap();
        assert_eq!(a.len(), 3);
        let t = |s: &str| parse_time(s).unwrap();
        assert_eq!(
            a[0].busy,
            [
                Busy { span: Span::new(t("2026-10-20T07:00:00Z"), t("2026-10-20T08:00:00Z")), kind: BusyKind::Busy },
                Busy { span: Span::new(t("2026-10-20T11:00:00Z"), t("2026-10-20T11:30:00Z")), kind: BusyKind::Tentative },
            ]
        );
        assert_eq!(a[1].busy[0].kind, BusyKind::Unavailable);
        assert_eq!((a[2].email.as_str(), a[2].error.as_deref()), ("nobody@example.test", Some("ErrorMailRecipientNotFound")));
    }

    #[test]
    fn request_window_in_utc() {
        let t = |s: &str| parse_time(s).unwrap();
        let r = request(&["a&b@example.test"], Span::new(t("2026-10-20T00:00:00Z"), t("2026-10-27T00:00:00Z")));
        let x = xml(format!(r#"<x xmlns:m="m" xmlns:t="t">{r}</x>"#).as_bytes()).unwrap();
        let req = x.child("GetUserAvailabilityRequest").unwrap();
        assert_eq!(req.text_at(&["FreeBusyViewOptions", "TimeWindow", "StartTime"]), Some("2026-10-20T00:00:00"));
        assert_eq!(req.text_at(&["MailboxDataArray", "MailboxData", "Email", "Address"]), Some("a&b@example.test"));
        assert_eq!(req.text_at(&["TimeZone", "Bias"]), Some("0"));
    }
}

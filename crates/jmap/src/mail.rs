//! JMAP Mail (RFC 8621): ящики, письма, изменения.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{CORE, Client, Invocation, MAIL, Result, take};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub sort_order: u32,
    #[serde(default)]
    pub total_emails: u32,
    #[serde(default)]
    pub unread_emails: u32,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub email: Option<String>,
}

impl EmailAddress {
    pub fn display(&self) -> &str {
        self.name.as_deref().filter(|n| !n.is_empty()).or(self.email.as_deref()).unwrap_or("")
    }
}

pub const SUMMARY_PROPERTIES: &[&str] = &[
    "id", "threadId", "mailboxIds", "keywords", "receivedAt", "subject", "from", "preview",
    "hasAttachment", "size",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSummary {
    pub id: String,
    pub thread_id: String,
    pub mailbox_ids: HashMap<String, bool>,
    #[serde(default)]
    pub keywords: HashMap<String, bool>,
    pub received_at: String,
    pub subject: Option<String>,
    pub from: Option<Vec<EmailAddress>>,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub has_attachment: bool,
    #[serde(default)]
    pub size: u64,
}

impl EmailSummary {
    pub fn has_keyword(&self, kw: &str) -> bool {
        self.keywords.get(kw).copied().unwrap_or(false)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyPart {
    pub part_id: Option<String>,
    pub blob_id: Option<String>,
    #[serde(default)]
    pub size: u64,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub mime: String,
    pub disposition: Option<String>,
    pub cid: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBody {
    message_id: Option<Vec<String>>,
    references: Option<Vec<String>>,
    reply_to: Option<Vec<EmailAddress>>,
    sent_at: Option<String>,
    to: Option<Vec<EmailAddress>>,
    cc: Option<Vec<EmailAddress>>,
    #[serde(default)]
    body_values: HashMap<String, BodyValue>,
    #[serde(default)]
    text_body: Vec<BodyPart>,
    #[serde(default)]
    html_body: Vec<BodyPart>,
    #[serde(default)]
    attachments: Vec<BodyPart>,
}

#[derive(Debug, Clone, Deserialize)]
struct BodyValue {
    value: String,
}

/// Тело письма, декодированное сервером (MIME на клиенте не разбираем).
#[derive(Debug, Clone)]
pub struct EmailBody {
    /// Message-ID оригинала (без угловых скобок) — для In-Reply-To.
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub reply_to: Vec<EmailAddress>,
    /// Дата из заголовка Date (UTCDate/Date JMAP), если есть.
    pub sent_at: Option<String>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<BodyPart>,
}

#[derive(Debug, Clone)]
pub struct EmailPage {
    pub total: u64,
    pub position: u64,
    pub emails: Vec<EmailSummary>,
    pub query_state: String,
    /// Состояние `Email` — точка отсчёта для `Email/changes`.
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub old_state: String,
    pub new_state: String,
    pub has_more_changes: bool,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub destroyed: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct GetResponse<T> {
    pub(crate) state: String,
    pub(crate) list: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryResponse {
    query_state: String,
    #[serde(default)]
    total: Option<u64>,
    position: u64,
}

pub(crate) const USING: &[&str] = &[CORE, MAIL];

pub(crate) fn inv(name: &str, args: Value, id: &str) -> Invocation {
    Invocation(name.into(), args, id.into())
}

impl Client {
    pub fn mail_account(&self) -> Result<&str> {
        self.account_for(MAIL)
    }

    /// Все ящики и состояние `Mailbox`.
    pub async fn mailboxes(&self) -> Result<(Vec<Mailbox>, String)> {
        let acc = self.mail_account()?;
        let mut r = self.call(USING, &[inv("Mailbox/get", json!({ "accountId": acc, "ids": null }), "m")]).await?;
        let g: GetResponse<Mailbox> = take(&mut r, "m")?;
        Ok((g.list, g.state))
    }

    /// Страница писем ящика, новые сверху: Email/query + Email/get одним запросом.
    pub async fn email_page(&self, mailbox_id: &str, position: u64, limit: u64) -> Result<EmailPage> {
        let acc = self.mail_account()?;
        let calls = [
            inv(
                "Email/query",
                json!({
                    "accountId": acc,
                    "filter": { "inMailbox": mailbox_id },
                    "sort": [{ "property": "receivedAt", "isAscending": false }],
                    "position": position,
                    "limit": limit,
                    "calculateTotal": true,
                }),
                "q",
            ),
            inv(
                "Email/get",
                json!({
                    "accountId": acc,
                    "#ids": { "resultOf": "q", "name": "Email/query", "path": "/ids" },
                    "properties": SUMMARY_PROPERTIES,
                }),
                "g",
            ),
        ];
        let mut r = self.call(USING, &calls).await?;
        let q: QueryResponse = take(&mut r, "q")?;
        let g: GetResponse<EmailSummary> = take(&mut r, "g")?;
        Ok(EmailPage { total: q.total.unwrap_or(0), position: q.position, emails: g.list, query_state: q.query_state, state: g.state })
    }

    pub async fn email_get(&self, ids: &[String]) -> Result<(Vec<EmailSummary>, String)> {
        let acc = self.mail_account()?;
        let args = json!({ "accountId": acc, "ids": ids, "properties": SUMMARY_PROPERTIES });
        let mut r = self.call(USING, &[inv("Email/get", args, "g")]).await?;
        let g: GetResponse<EmailSummary> = take(&mut r, "g")?;
        Ok((g.list, g.state))
    }

    /// Дельта с указанного состояния (`Email/changes` или `Mailbox/changes`).
    pub async fn changes(&self, kind: &str, since: &str, max: u64) -> Result<Changes> {
        let acc = self.mail_account()?;
        let args = json!({ "accountId": acc, "sinceState": since, "maxChanges": max });
        let mut r = self.call(USING, &[inv(&format!("{kind}/changes"), args, "c")]).await?;
        take(&mut r, "c")
    }

    pub async fn email_body(&self, id: &str) -> Result<EmailBody> {
        let acc = self.mail_account()?;
        let args = json!({
            "accountId": acc,
            "ids": [id],
            "properties": ["messageId", "references", "replyTo", "sentAt", "to", "cc", "bodyValues", "textBody", "htmlBody", "attachments"],
            "fetchTextBodyValues": true,
            "fetchHTMLBodyValues": true,
            "maxBodyValueBytes": 4 * 1024 * 1024,
        });
        let mut r = self.call(USING, &[inv("Email/get", args, "b")]).await?;
        let g: GetResponse<RawBody> = take(&mut r, "b")?;
        let raw = g.list.into_iter().next().ok_or_else(|| crate::Error::Protocol(format!("письмо {id} не найдено")))?;
        let join = |parts: &[BodyPart], want: &str| -> Option<String> {
            let mut out = String::new();
            for p in parts.iter().filter(|p| p.mime == want) {
                if let Some(v) = p.part_id.as_ref().and_then(|pid| raw.body_values.get(pid)) {
                    out.push_str(&v.value);
                }
            }
            (!out.is_empty()).then_some(out)
        };
        Ok(EmailBody {
            message_id: raw.message_id.as_ref().and_then(|v| v.first().cloned()),
            references: raw.references.clone().unwrap_or_default(),
            reply_to: raw.reply_to.clone().unwrap_or_default(),
            sent_at: raw.sent_at.clone(),
            html: join(&raw.html_body, "text/html"),
            text: join(&raw.text_body, "text/plain"),
            to: raw.to.clone().unwrap_or_default(),
            cc: raw.cc.clone().unwrap_or_default(),
            attachments: raw.attachments.clone(),
        })
    }

    /// Поставить/снять ключевое слово (`$seen`, `$flagged`, …).
    pub async fn set_keyword(&self, id: &str, keyword: &str, on: bool) -> Result<()> {
        self.email_patch(id, json!({ format!("keywords/{keyword}"): if on { Value::Bool(true) } else { Value::Null } }))
            .await
    }

    pub async fn move_email(&self, id: &str, from: &str, to: &str) -> Result<()> {
        self.email_patch(id, json!({ format!("mailboxIds/{from}"): null, format!("mailboxIds/{to}"): true })).await
    }

    pub async fn destroy_email(&self, id: &str) -> Result<()> {
        let acc = self.mail_account()?;
        let mut r = self.call(USING, &[inv("Email/set", json!({ "accountId": acc, "destroy": [id] }), "s")]).await?;
        check_set(&mut r, id, "notDestroyed")
    }

    async fn email_patch(&self, id: &str, patch: Value) -> Result<()> {
        let acc = self.mail_account()?;
        let args = json!({ "accountId": acc, "update": { id: patch } });
        let mut r = self.call(USING, &[inv("Email/set", args, "s")]).await?;
        check_set(&mut r, id, "notUpdated")
    }
}

fn check_set(r: &mut [Invocation], id: &str, field: &str) -> Result<()> {
    let v: Value = take(r, "s")?;
    set_error(&v, field, id, "Email/set")
}

/// Ошибка из `notCreated`/`notUpdated`/`notDestroyed` по ключу.
pub(crate) fn set_error(v: &Value, field: &str, key: &str, method: &str) -> Result<()> {
    if let Some(err) = v.get(field).and_then(|m| m.get(key)) {
        return Err(crate::Error::Method {
            method: method.into(),
            kind: err["type"].as_str().unwrap_or("?").into(),
            description: err["description"].as_str().unwrap_or("").into(),
        });
    }
    Ok(())
}

/// JMAP UTCDate (`2024-05-01T12:00:00Z`) -> Unix-время в секундах.
pub fn utc_to_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // Дни от эпохи по алгоритму Howard Hinnant (days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(utc_to_unix("2026-10-17T12:30:00Z"), Some(1_792_240_200));
        assert_eq!(utc_to_unix("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(utc_to_unix("мусор"), None);
    }

    #[test]
    fn invocation_is_array() {
        let s = serde_json::to_string(&inv("Mailbox/get", json!({}), "m")).unwrap();
        assert_eq!(s, r#"["Mailbox/get",{},"m"]"#);
    }
}

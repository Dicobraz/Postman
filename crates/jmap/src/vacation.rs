//! Автоответ (RFC 8621 §8, `VacationResponse`, один объект `singleton`) и квоты (RFC 9425, `Quota/get`).
//!
//! Stalwart хранит автоответ Sieve-скриптом `vacation`: включение делает его активным и выключает другой
//! активный скрипт (активный всегда один), выключение снимает только его (`docs/STALWART.md`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mail::{GetResponse, inv, set_error};
use crate::{CORE, Client, QUOTA, Result, VACATION_RESPONSE, take};

/// Автоответ. Даты — UTCDate JMAP (`2026-10-20T00:00:00Z`); `None` — без границы.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VacationResponse {
    #[serde(default)]
    pub is_enabled: bool,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

/// Квота (RFC 9425 §4.1): `used` и `hard_limit` — в единицах `resource_type` (`octets` — байты, `count` — штуки).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub id: String,
    pub resource_type: String,
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub hard_limit: u64,
    #[serde(default)]
    pub warn_limit: Option<u64>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub name: String,
    /// Типы данных под квотой (`Mail`, `Calendar`…); пусто — все.
    #[serde(default)]
    pub types: Vec<String>,
}

const SINGLETON: &str = "singleton";

impl Client {
    /// Автоответ учётной записи; у сервера его нет — выключенный пустой.
    pub async fn vacation(&self) -> Result<VacationResponse> {
        let acc = self.account_for(VACATION_RESPONSE)?;
        let args = json!({ "accountId": acc, "ids": [SINGLETON] });
        let mut r = self.call(&[CORE, VACATION_RESPONSE], &[inv("VacationResponse/get", args, "v")]).await?;
        let g: GetResponse<VacationResponse> = take(&mut r, "v")?;
        Ok(g.list.into_iter().next().unwrap_or_default())
    }

    /// Записать автоответ целиком (все поля; HTML-текст не ведём — `null`).
    pub async fn set_vacation(&self, v: &VacationResponse) -> Result<()> {
        let acc = self.account_for(VACATION_RESPONSE)?;
        let patch = json!({
            "isEnabled": v.is_enabled,
            "fromDate": v.from_date,
            "toDate": v.to_date,
            "subject": v.subject,
            "textBody": v.text_body,
            "htmlBody": Value::Null,
        });
        let args = json!({ "accountId": acc, "update": { SINGLETON: patch } });
        let mut r = self.call(&[CORE, VACATION_RESPONSE], &[inv("VacationResponse/set", args, "s")]).await?;
        let v: Value = take(&mut r, "s")?;
        set_error(&v, "notUpdated", SINGLETON, "VacationResponse/set")
    }

    /// Все квоты учётной записи (у пользователя без ограничений — пустой список).
    pub async fn quotas(&self) -> Result<Vec<Quota>> {
        let acc = self.account_for(QUOTA)?;
        let args = json!({ "accountId": acc, "ids": null });
        let mut r = self.call(&[CORE, QUOTA], &[inv("Quota/get", args, "q")]).await?;
        let g: GetResponse<Quota> = take(&mut r, "q")?;
        Ok(g.list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalwart_shapes() {
        let v: VacationResponse = serde_json::from_value(json!({
            "id": "singleton", "isEnabled": true, "fromDate": "2026-10-20T00:00:00Z", "toDate": null,
            "subject": "Нет на месте", "textBody": "До 27.10.", "htmlBody": null
        }))
        .unwrap();
        assert!(v.is_enabled);
        assert_eq!(v.from_date.as_deref(), Some("2026-10-20T00:00:00Z"));
        assert_eq!(v.to_date, None);
        let q: Quota = serde_json::from_value(json!({
            "id": "q1", "resourceType": "octets", "used": 1024, "hardLimit": 2048, "scope": "account",
            "name": "user2", "types": ["Mail"]
        }))
        .unwrap();
        assert_eq!((q.used, q.hard_limit, q.types.as_slice()), (1024, 2048, &["Mail".to_string()][..]));
    }
}

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub capabilities: HashMap<String, Value>,
    pub accounts: HashMap<String, Account>,
    pub primary_accounts: HashMap<String, String>,
    pub username: String,
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub event_source_url: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub is_personal: bool,
    #[serde(default)]
    pub is_read_only: bool,
    #[serde(default)]
    pub account_capabilities: HashMap<String, Value>,
}

/// Почтовый аккаунт, кроме основного (S4-06): общий ящик группы или ящик, открытый другим (`mail:share`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedAccount {
    pub id: String,
    /// Имя аккаунта у сервера (Stalwart — адрес ящика).
    pub name: String,
    pub is_personal: bool,
    /// Весь аккаунт только для чтения. Права на отдельные папки — `Mailbox.myRights`.
    pub is_read_only: bool,
    /// Аккаунт объявляет `submission`: отправка от его имени возможна, если в нём есть Identity.
    pub can_submit: bool,
}

impl Session {
    /// Почтовые аккаунты, кроме основного (`primaryAccounts` почты), по имени. Аккаунт без capability почты
    /// (только календарь или файлы) — не ящик.
    pub fn shared_accounts(&self) -> Vec<SharedAccount> {
        let primary = self.primary_accounts.get(crate::MAIL);
        let mut out: Vec<SharedAccount> = self
            .accounts
            .iter()
            .filter(|(id, a)| Some(*id) != primary && a.account_capabilities.contains_key(crate::MAIL))
            .map(|(id, a)| SharedAccount {
                id: id.clone(),
                name: a.name.clone(),
                is_personal: a.is_personal,
                is_read_only: a.is_read_only,
                can_submit: a.account_capabilities.contains_key(crate::SUBMISSION),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// Аккаунты с календарями, кроме своего (`primaryAccounts` календаря): календари, открытые мне коллегами (S5-22).
    /// По имени аккаунта (Stalwart — адрес владельца).
    pub fn shared_calendar_accounts(&self) -> Vec<(String, String)> {
        let primary = self.primary_accounts.get(crate::CALENDARS);
        let mut out: Vec<(String, String)> = self
            .accounts
            .iter()
            .filter(|(id, a)| Some(*id) != primary && a.account_capabilities.contains_key(crate::CALENDARS))
            .map(|(id, a)| (id.clone(), a.name.clone()))
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// URL сессии — на тот же адрес, по которому получена сессия: относительные дополняем, у абсолютных
    /// заменяем начало (схема, хост, порт). Сервер за прокси или туннелем объявляет свой внутренний адрес
    /// (Stalwart: `http://192.0.2.10:8080/jmap/`), а пароль и токен не должны уходить на другой хост.
    /// Шаблоны (`{accountId}` и т.п.) не трогаем: склеиваем строкой.
    pub(crate) fn resolve_urls(&mut self, base: &url::Url) -> Result<(), url::ParseError> {
        let origin = base.origin().ascii_serialization();
        for u in [&mut self.api_url, &mut self.download_url, &mut self.upload_url, &mut self.event_source_url] {
            *u = rebase(u, &origin);
        }
        url::Url::parse(&self.api_url).map(|_| ())
    }
}

/// `http://другой:8080/jmap/x` → `{origin}/jmap/x`; `/jmap/x` → `{origin}/jmap/x`.
fn rebase(u: &str, origin: &str) -> String {
    let path = match u.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => u,
    };
    let sep = if path.starts_with('/') { "" } else { "/" };
    format!("{origin}{sep}{path}")
}

#[cfg(test)]
mod tests {
    use super::{Session, SharedAccount, rebase};

    /// Сессия user3 со стенда (S4-06): свой аккаунт, общий ящик группы и папка, открытая только для чтения;
    /// аккаунт без почты — не ящик.
    #[test]
    fn shared_mail_accounts_are_found() {
        let s: Session = serde_json::from_value(serde_json::json!({
            "capabilities": {},
            "accounts": {
                "e": { "name": "user3@itt.test", "isPersonal": true, "isReadOnly": false,
                       "accountCapabilities": { "urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:submission": {} } },
                "j": { "name": "shared-ro@itt.test", "isPersonal": false,
                       "accountCapabilities": { "urn:ietf:params:jmap:mail": {} } },
                "i": { "name": "shared-sales@itt.test", "isPersonal": false, "isReadOnly": true,
                       "accountCapabilities": { "urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:submission": {} } },
                "k": { "name": "files", "isPersonal": false,
                       "accountCapabilities": { "urn:ietf:params:jmap:filenode": {} } }
            },
            "primaryAccounts": { "urn:ietf:params:jmap:mail": "e" },
            "username": "user3@itt.test",
            "apiUrl": "/jmap/", "downloadUrl": "/d", "uploadUrl": "/u", "eventSourceUrl": "/e", "state": "s"
        }))
        .unwrap();
        let shared = s.shared_accounts();
        let acc = |id: &str, name: &str, read_only, submit| SharedAccount {
            id: id.into(),
            name: name.into(),
            is_personal: false,
            is_read_only: read_only,
            can_submit: submit,
        };
        assert_eq!(shared, [acc("j", "shared-ro@itt.test", false, false), acc("i", "shared-sales@itt.test", true, true)]);
    }

    #[test]
    fn urls_follow_the_connection_origin() {
        let o = "http://localhost:18080";
        assert_eq!(rebase("/jmap/", o), "http://localhost:18080/jmap/");
        assert_eq!(rebase("http://192.0.2.10:8080/jmap/", o), "http://localhost:18080/jmap/");
        assert_eq!(
            rebase("http://192.0.2.10:8080/jmap/download/{accountId}/{blobId}/{name}?accept={type}", o),
            "http://localhost:18080/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
        );
        assert_eq!(rebase("https://mail.example", o), "http://localhost:18080/");
    }
}

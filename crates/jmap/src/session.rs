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

impl Session {
    /// URL сессии — на тот же адрес, по которому получена сессия: относительные дополняем, у абсолютных
    /// заменяем начало (схема, хост, порт). Сервер за прокси или туннелем объявляет свой внутренний адрес
    /// (Stalwart: `http://10.70.2.40:8080/jmap/`), а пароль и токен не должны уходить на другой хост.
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
    use super::rebase;

    #[test]
    fn urls_follow_the_connection_origin() {
        let o = "http://localhost:18080";
        assert_eq!(rebase("/jmap/", o), "http://localhost:18080/jmap/");
        assert_eq!(rebase("http://10.70.2.40:8080/jmap/", o), "http://localhost:18080/jmap/");
        assert_eq!(
            rebase("http://10.70.2.40:8080/jmap/download/{accountId}/{blobId}/{name}?accept={type}", o),
            "http://localhost:18080/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
        );
        assert_eq!(rebase("https://mail.example", o), "http://localhost:18080/");
    }
}

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
    /// URL в сессии могут быть относительными — приводим к абсолютным.
    /// Шаблоны (`{accountId}` и т.п.) не трогаем: склеиваем строкой.
    pub(crate) fn resolve_urls(&mut self, base: &url::Url) -> Result<(), url::ParseError> {
        let origin = base.origin().ascii_serialization();
        for u in [&mut self.api_url, &mut self.download_url, &mut self.upload_url, &mut self.event_source_url] {
            if u.starts_with('/') {
                *u = format!("{origin}{u}");
            }
        }
        url::Url::parse(&self.api_url).map(|_| ())
    }
}

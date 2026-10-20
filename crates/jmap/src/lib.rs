//! Тонкий JMAP-клиент: сессия, произвольные вызовы методов и типизированные
//! обёртки для почты. Своя реализация вместо `jmap-client`, чтобы без
//! ограничений добавлять Calendars/Contacts/Principals.

mod calendars;
mod contacts;
mod mail;
mod push;
mod session;
mod submission;

pub use calendars::{BusyPeriod, Calendar, EVENT_PROPERTIES, Principal};
pub use contacts::ContactCard;
pub use mail::*;
pub use push::{EventSource, SseEvent};
pub use session::{Account, Session};
pub use submission::{Envelope, Identity};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CORE: &str = "urn:ietf:params:jmap:core";
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";
pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
pub const PRINCIPALS: &str = "urn:ietf:params:jmap:principals";
pub const PRINCIPALS_AVAILABILITY: &str = "urn:ietf:params:jmap:principals:availability";
pub const WEBSOCKET: &str = "urn:ietf:params:jmap:websocket";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("сервер не объявил {0}")]
    MissingCapability(&'static str),
    #[error("JMAP {method}: {kind} {description}")]
    Method { method: String, kind: String, description: String },
    #[error("неожиданный ответ: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub enum Auth {
    Basic { user: String, password: String },
    Bearer(String),
}

/// Вызов метода: `[имя, аргументы, id]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation(pub String, pub Value, pub String);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    using: &'a [&'a str],
    method_calls: &'a [Invocation],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    method_responses: Vec<Invocation>,
}

/// Дешёвый в клонировании: HTTP-клиент внутри разделяемый.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    auth: Auth,
    session: Session,
}

impl Client {
    /// Автообнаружение: `GET {base}/.well-known/jmap`.
    pub async fn connect(base: &str, auth: Auth) -> Result<Self> {
        let mut builder = reqwest::Client::builder().user_agent(concat!("Postman/", env!("CARGO_PKG_VERSION")));
        if insecure_tls() {
            tracing::warn!("POSTMAN_INSECURE_TLS: server certificate is NOT verified (test stand only)");
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder.build()?;
        let base = url::Url::parse(base)?;
        let well_known = base.join("/.well-known/jmap")?;
        let resp = authorize(http.get(well_known), &auth).send().await?.error_for_status()?;
        let final_url = resp.url().clone();
        let mut session: Session = resp.json().await?;
        session.resolve_urls(&final_url)?;
        if !session.capabilities.contains_key(MAIL) {
            return Err(Error::MissingCapability(MAIL));
        }
        Ok(Self { http, auth, session })
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn authorized(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        authorize(rb, &self.auth)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn has(&self, capability: &str) -> bool {
        self.session.capabilities.contains_key(capability)
    }

    pub fn account_for(&self, capability: &'static str) -> Result<&str> {
        self.session
            .primary_accounts
            .get(capability)
            .map(String::as_str)
            .ok_or(Error::MissingCapability(capability))
    }

    /// Один JMAP-запрос с несколькими вызовами. Ответы — в порядке вызовов;
    /// ответ `error` превращается в [`Error::Method`].
    pub async fn call(&self, using: &[&str], calls: &[Invocation]) -> Result<Vec<Invocation>> {
        let resp: Response = authorize(self.http.post(&self.session.api_url), &self.auth)
            .json(&Request { using, method_calls: calls })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for r in &resp.method_responses {
            if r.0 == "error" {
                let method = calls.iter().find(|c| c.2 == r.2).map(|c| c.0.clone()).unwrap_or_default();
                return Err(Error::Method {
                    method,
                    kind: r.1["type"].as_str().unwrap_or("?").to_string(),
                    description: r.1["description"].as_str().unwrap_or("").to_string(),
                });
            }
        }
        Ok(resp.method_responses)
    }

    /// Скачать blob (вложение, исходное письмо).
    pub async fn download(&self, account: &str, blob_id: &str, name: &str, mime: &str) -> Result<Vec<u8>> {
        let url = self
            .session
            .download_url
            .replace("{accountId}", &enc(account))
            .replace("{blobId}", &enc(blob_id))
            .replace("{name}", &enc(name))
            .replace("{type}", &enc(mime));
        let bytes = authorize(self.http.get(url), &self.auth).send().await?.error_for_status()?.bytes().await?;
        Ok(bytes.to_vec())
    }
}

/// `POSTMAN_INSECURE_TLS=1` — не проверять сертификат сервера. Только для стенда с самоподписанным
/// сертификатом без нужного имени (`docs/STALWART.md`); у рабочего сервера сертификат должен проверяться.
fn insecure_tls() -> bool {
    std::env::var("POSTMAN_INSECURE_TLS").is_ok_and(|v| v == "1")
}

fn authorize(rb: reqwest::RequestBuilder, auth: &Auth) -> reqwest::RequestBuilder {
    match auth {
        Auth::Basic { user, password } => rb.basic_auth(user, Some(password)),
        Auth::Bearer(token) => rb.bearer_auth(token),
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

/// Достать аргументы ответа по id вызова и десериализовать.
pub fn take<T: serde::de::DeserializeOwned>(responses: &mut [Invocation], call_id: &str) -> Result<T> {
    let r = responses
        .iter_mut()
        .find(|r| r.2 == call_id)
        .ok_or_else(|| Error::Protocol(format!("нет ответа на {call_id}")))?;
    Ok(serde_json::from_value(r.1.take())?)
}

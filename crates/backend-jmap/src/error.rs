//! Ошибки пути JMAP: род (`ErrorKind`) — по ошибке протокола, как было в `core::sync` до плагинов; на границе с
//! общим кодом — нейтральная `postman_core::model::Error` с тем же родом и текстом.

use postman_core::model::ErrorKind;
use postman_core::sync::{Stage, SyncError};

/// Ошибки кэша идут как `Error::Protocol` с этим началом: так их отличает [`kind_of`].
pub const CACHE_ERROR: &str = "cache: ";

/// Ошибка, у которой есть род: протокола JMAP или общего кода.
pub trait Kinded: std::fmt::Display {
    fn kind(&self) -> ErrorKind;
}

impl Kinded for postman_jmap::Error {
    fn kind(&self) -> ErrorKind {
        use postman_jmap::Error as E;
        match self {
            E::Http(h) => match h.status().map(|s| s.as_u16()) {
                Some(401) => ErrorKind::Auth,
                Some(_) => ErrorKind::Server,
                None if h.is_decode() => ErrorKind::Protocol,
                None => ErrorKind::Network,
            },
            E::Method { .. } => ErrorKind::Server,
            E::Protocol(s) if s.starts_with(CACHE_ERROR) => ErrorKind::Storage,
            E::Url(_) | E::Json(_) | E::MissingCapability(_) | E::Protocol(_) => ErrorKind::Protocol,
        }
    }
}

/// Ошибка команды: протокола или общего кода (задачи, `postman_core::tasks`).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Jmap(#[from] postman_jmap::Error),
    #[error(transparent)]
    Core(#[from] postman_core::model::Error),
}

impl Kinded for Error {
    fn kind(&self) -> ErrorKind {
        match self {
            Error::Jmap(e) => e.kind(),
            Error::Core(e) => e.kind(),
        }
    }
}

pub fn kind_of(e: &impl Kinded) -> ErrorKind {
    e.kind()
}

/// Ошибка синхронизации: род, шаг и текст ошибки как есть.
pub fn sync_error(stage: Stage, e: &impl Kinded) -> SyncError {
    SyncError { kind: e.kind(), stage, detail: e.to_string() }
}

/// Ошибка JMAP → нейтральная (задачи и прочий общий код): род и текст те же, отказ метода — с кодом.
pub fn core_error(e: postman_jmap::Error) -> postman_core::model::Error {
    match e {
        postman_jmap::Error::Method { method, kind, description } => {
            postman_core::model::Error::Method { method: format!("JMAP {method}"), kind, description }
        }
        e => postman_core::model::Error::other(e.kind(), e.to_string()),
    }
}

/// Ошибка кэша — как ошибка пути (род `Storage`).
pub fn db_err(e: postman_cache::Error) -> postman_jmap::Error {
    postman_jmap::Error::Protocol(format!("{CACHE_ERROR}{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use postman_jmap::Error as E;

    #[test]
    fn errors_have_kinds() {
        assert_eq!(kind_of(&E::Protocol(format!("{CACHE_ERROR}table"))), ErrorKind::Storage);
        assert_eq!(kind_of(&E::Protocol("x".into())), ErrorKind::Protocol);
        assert_eq!(kind_of(&E::MissingCapability(postman_jmap::MAIL)), ErrorKind::Protocol);
        let m = E::Method { method: "Email/get".into(), kind: "serverFail".into(), description: String::new() };
        let e = sync_error(Stage::Refresh, &m);
        assert_eq!(e.kind, ErrorKind::Server);
        assert!(e.to_string().starts_with("Refresh failed (Server)"), "{e}");
        // Нейтральная ошибка: тот же текст и код отказа.
        let c = core_error(E::Method { method: "CalendarEvent/changes".into(), kind: "cannotCalculateChanges".into(), description: String::new() });
        assert_eq!((c.code(), c.to_string().as_str()), (Some("cannotCalculateChanges"), "JMAP CalendarEvent/changes: cannotCalculateChanges "));
        let p = core_error(E::Protocol("y".into()));
        assert_eq!((p.kind(), p.to_string().as_str()), (ErrorKind::Protocol, "неожиданный ответ: y"));
        assert_eq!(kind_of(&Error::from(postman_core::model::Error::network("down"))), ErrorKind::Network);
    }
}

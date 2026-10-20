//! Учётная запись (S3-04): откуда она берётся при запуске, где лежит её кэш и что хранится на диске.
//!
//! На диске — только несекретная часть (`account.json`: адрес сервера и логин); пароль — в диспетчере учётных
//! данных Windows (`sys::credentials`), цель `Postman:<адрес>:<логин>`. Токен (`POSTMAN_TOKEN`) — только из
//! окружения, не сохраняется.

use std::path::{Path, PathBuf};

use postman_core::sync::Account;
use postman_jmap::Auth;
use serde::{Deserialize, Serialize};

use crate::sys;

const SETTINGS_FILE: &str = "account.json";

/// Несекретная часть учётной записи: её можно писать на диск.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub(crate) url: String,
    pub(crate) user: String,
}

impl Settings {
    pub(crate) fn of(account: &Account) -> Self {
        Self { url: account.url.clone(), user: user_of(account).to_owned() }
    }

    pub(crate) fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(SETTINGS_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn save(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(SETTINGS_FILE), text)
    }

    pub(crate) fn remove(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(SETTINGS_FILE));
    }

    /// Имя пароля в диспетчере учётных данных.
    pub(crate) fn credential_target(&self) -> String {
        format!("Postman:{}:{}", self.url, self.user)
    }
}

/// Логин учётной записи; вход по токену — «token».
pub(crate) fn user_of(account: &Account) -> &str {
    match &account.auth {
        Auth::Basic { user, .. } => user,
        Auth::Bearer(_) => "token",
    }
}

/// Кэш учётной записи: свой файл на пару «сервер + логин» (не `demo.redb`). Индекс поиска — рядом,
/// `<кэш>.search`.
pub(crate) fn cache_path(dir: &Path, account: &Account) -> PathBuf {
    let user = user_of(account);
    let readable: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c.to_ascii_lowercase() } else { '_' })
        .collect();
    dir.join(format!("account-{readable}-{:08x}.redb", fnv(&format!("{}\n{user}", account.url))))
}

/// FNV-1a, 32 бита: устойчивое между запусками имя файла (std-хэш от версии к версии меняется).
fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Что сохранено с прошлого входа.
pub(crate) enum Stored {
    Account(Account),
    /// Адрес и логин есть, пароля нет (удалён из диспетчера): строка входа с подставленными полями.
    Settings(Settings),
    None,
}

pub(crate) fn load_stored(dir: &Path) -> Stored {
    let Some(s) = Settings::load(dir) else { return Stored::None };
    match sys::credentials::read(&s.credential_target()) {
        Ok(Some(password)) => Stored::Account(Account { url: s.url, auth: Auth::Basic { user: s.user, password } }),
        Ok(None) => Stored::Settings(s),
        Err(e) => {
            tracing::warn!("credential manager: {e}");
            Stored::Settings(s)
        }
    }
}

/// Запомнить учётную запись после успешного входа: адрес и логин — в файл, пароль — в диспетчер.
pub(crate) fn remember(dir: &Path, account: &Account) -> Result<(), String> {
    let s = Settings::of(account);
    if let Auth::Basic { password, .. } = &account.auth {
        sys::credentials::write(&s.credential_target(), &s.user, password)?;
    }
    s.save(dir).map_err(|e| format!("{SETTINGS_FILE}: {e}"))
}

/// `--sign-out`: забыть сохранённую учётную запись (кэш остаётся).
pub(crate) fn forget(dir: &Path) {
    if let Some(s) = Settings::load(dir)
        && let Err(e) = sys::credentials::delete(&s.credential_target())
    {
        tracing::warn!("credential manager: {e}");
    }
    Settings::remove(dir);
}

/// С чем открыть окно.
pub(crate) enum Start {
    /// Демо-ящик: `--demo`, `--bench`, `--screens` без `--live`.
    Demo,
    Live(Account),
    /// Нет учётной записи: строка входа (поля — с прошлого входа, если есть).
    SignIn(Option<Settings>),
}

/// Флаги запуска, от которых зависит выбор.
#[derive(Clone, Copy, Default)]
pub(crate) struct Flags {
    pub(crate) demo: bool,
    pub(crate) bench: bool,
    pub(crate) screens: bool,
    pub(crate) live: bool,
}

/// Выбор при запуске: замер живого ящика — из окружения; демо по флагам; иначе окружение (`POSTMAN_URL`…), затем сохранённый вход, иначе — строка
/// входа. `--screens --live` — только из окружения (агенты снимают живой ящик стенда), без него — строка входа.
pub(crate) fn choose(flags: Flags, env: Option<Account>, stored: impl FnOnce() -> Stored) -> Start {
    // Замер живого ящика (`--bench --live`, S4-03) — только из окружения, как снимки.
    if flags.bench && flags.live {
        return env.map_or(Start::SignIn(None), Start::Live);
    }
    if flags.bench || flags.demo || (flags.screens && !flags.live) {
        return Start::Demo;
    }
    if let Some(a) = env {
        return Start::Live(a);
    }
    // Снимки без учётной записи в окружении — строка входа, без сохранённого входа (повторяемо).
    if flags.screens {
        return Start::SignIn(None);
    }
    match stored() {
        Stored::Account(a) => Start::Live(a),
        Stored::Settings(s) => Start::SignIn(Some(s)),
        Stored::None => Start::SignIn(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(url: &str, user: &str, password: &str) -> Account {
        Account { url: url.into(), auth: Auth::Basic { user: user.into(), password: password.into() } }
    }

    #[test]
    fn settings_round_trip_without_secrets() {
        let dir = tempfile::Builder::new().prefix("postman-").tempdir().unwrap();
        let account = basic("http://localhost:18080", "user1@itt.test", "not-a-real-secret");
        let s = Settings::of(&account);
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), Some(s.clone()));
        let text = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(!text.contains("not-a-real-secret"), "password on disk: {text}");
        assert_eq!(s.credential_target(), "Postman:http://localhost:18080:user1@itt.test");
        Settings::remove(dir.path());
        assert_eq!(Settings::load(dir.path()), None);
    }

    #[test]
    fn broken_settings_file_is_ignored() {
        let dir = tempfile::Builder::new().prefix("postman-").tempdir().unwrap();
        std::fs::write(dir.path().join(SETTINGS_FILE), "{ not json").unwrap();
        assert_eq!(Settings::load(dir.path()), None);
    }

    #[test]
    fn cache_is_per_account() {
        let dir = Path::new("data");
        let a = cache_path(dir, &basic("http://a", "user1@itt.test", "x"));
        assert_eq!(a, cache_path(dir, &basic("http://a", "user1@itt.test", "other password")));
        assert_ne!(a, cache_path(dir, &basic("http://b", "user1@itt.test", "x")));
        assert_ne!(a, cache_path(dir, &basic("http://a", "user2@itt.test", "x")));
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("account-user1@itt.test-".replace('@', "_").as_str()), "{name}");
        assert!(name.ends_with(".redb") && name != "demo.redb");
        let token = Account { url: "http://a".into(), auth: Auth::Bearer("t".into()) };
        assert!(cache_path(dir, &token).to_string_lossy().contains("account-token-"));
    }

    #[test]
    fn start_mode() {
        let env = || Some(basic("http://a", "u", "p"));
        let none = || Stored::None;
        let f = |demo, bench, screens, live| Flags { demo, bench, screens, live };
        assert!(matches!(choose(f(true, false, false, false), env(), none), Start::Demo));
        assert!(matches!(choose(f(false, true, false, false), env(), none), Start::Demo));
        assert!(matches!(choose(f(false, true, false, true), env(), || unreachable!()), Start::Live(_)));
        assert!(matches!(choose(f(false, true, false, true), None, || unreachable!()), Start::SignIn(None)));
        assert!(matches!(choose(f(false, false, true, false), env(), none), Start::Demo));
        assert!(matches!(choose(f(false, false, true, true), env(), none), Start::Live(_)));
        assert!(matches!(choose(f(false, false, true, true), None, || unreachable!()), Start::SignIn(None)));
        assert!(matches!(choose(f(false, false, false, false), env(), || unreachable!()), Start::Live(_)));
        assert!(matches!(choose(f(false, false, false, false), None, none), Start::SignIn(None)));
        let partial = || Stored::Settings(Settings { url: "http://a".into(), user: "u".into() });
        assert!(matches!(choose(f(false, false, false, false), None, partial), Start::SignIn(Some(_))));
        let saved = || Stored::Account(basic("http://a", "u", "p"));
        assert!(matches!(choose(f(false, false, false, false), None, saved), Start::Live(_)));
    }
}

//! Пути к серверам (ADR 0003): перевод типов протокола в нейтральную модель (`crate::model`) и общие для всех путей
//! настройки соединения. Сейчас путь один — JMAP ([`jmap`]); IMAP, CalDAV/CardDAV и EWS — S7-04…S7-08.

pub mod jmap;

/// Только TLS 1.3 («Настройки → Безопасность», S5-30); иначе — 1.2 и выше. Действует на новые подключения всех путей.
pub fn set_tls13_only(on: bool) {
    postman_jmap::set_tls13_only(on);
}

/// `POSTMAN_INSECURE_TLS=1` — сертификат сервера не проверяется (только стенд, `docs/STALWART.md`).
pub fn insecure_tls() -> bool {
    postman_jmap::insecure_tls()
}

//! Пути к серверам (ADR 0004, S7-02б): композиционный корень. Здесь — какие плагины протоколов знает Postman и в
//! каком порядке: почтовые пути — JMAP (путь по умолчанию: адрес без своей схемы), затем IMAP + SMTP; пути областей —
//! CalDAV и CardDAV. Новый протокол — плагин-крейт и строка здесь; `postman-core` и экраны не меняются.

use std::sync::OnceLock;

use postman_core::backend::Registry;

/// Реестр плагинов процесса.
pub(crate) fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Registry::default()
            .with_mail(postman_backend_jmap::JmapPlugin)
            .with_mail(postman_backend_imap::ImapPlugin)
            .with_area(postman_backend_dav::DavPlugin)
    })
}

#[cfg(test)]
mod tests {
    /// Адрес со своей схемой — путь IMAP; адрес без схемы и `https://` — ничей (проверит сеть, иначе JMAP).
    #[test]
    fn imap_scheme_is_claimed() {
        let r = super::registry();
        assert!(r.claims("imaps://mail.example.test") && r.claims("IMAP://x"));
        assert!(!r.claims("https://mail.example.test") && !r.claims("mail.example.test"));
    }
}

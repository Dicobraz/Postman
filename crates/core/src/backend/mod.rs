//! Пути к серверам (ADR 0003): перевод типов протокола в нейтральную модель (`crate::model`) и общие для всех путей
//! настройки соединения. Сейчас путь один — JMAP ([`jmap`]); IMAP, CalDAV/CardDAV и EWS — S7-04…S7-08.

pub mod jmap;

/// Только TLS 1.3 («Настройки → Безопасность», S5-30); иначе — 1.2 и выше. Действует на новые подключения всех путей.
pub fn set_tls13_only(on: bool) {
    postman_jmap::set_tls13_only(on);
}

/// Вид сервера по адресу (S7-04в): адрес без схемы IMAP — сначала JMAP (`/.well-known/jmap`), нет его — IMAP на том
/// же узле (порт 993 отвечает) → `imaps://узел`; ни того ни другого — адрес как есть (ошибку покажет вход). Адрес
/// `imaps://…`/`imap://…` и адрес с путём — как есть. SRV-записи (RFC 6186) — нет: без своего DNS-клиента.
pub async fn detect(url: &str) -> String {
    if crate::imap::is_imap_url(url) || postman_jmap::has_jmap(url).await {
        return url.to_owned();
    }
    let host = url.split_once("://").map_or(url, |(_, r)| r);
    let has_path = host.contains('/') || host.contains('?');
    let host = host.split(['/', '?']).next().unwrap_or_default();
    // Явный порт или путь — это адрес JMAP; угадывать не будем.
    if host.is_empty() || has_path || host.contains(':') {
        return url.to_owned();
    }
    let open = tokio::time::timeout(std::time::Duration::from_secs(5), tokio::net::TcpStream::connect((host, 993))).await;
    if matches!(open, Ok(Ok(_))) { format!("imaps://{host}") } else { url.to_owned() }
}

/// [`detect`] из потока без рантайма tokio (интерфейс — в фоне).
pub fn detect_blocking(url: &str) -> String {
    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt.block_on(detect(url)),
        Err(_) => url.to_owned(),
    }
}

/// `POSTMAN_INSECURE_TLS=1` — сертификат сервера не проверяется (только стенд, `docs/STALWART.md`).
pub fn insecure_tls() -> bool {
    postman_jmap::insecure_tls()
}

//! Пути к серверам (ADR 0003): перевод типов протокола в нейтральную модель (`crate::model`) и общие для всех путей
//! настройки соединения. JMAP — [`jmap`]; IMAP и SMTP — `crate::imap`, CalDAV и CardDAV — `crate::dav` (S7-04, S7-05);
//! EWS — S7-06…S7-08.

pub mod jmap;

/// Только TLS 1.3 («Настройки → Безопасность», S5-30); иначе — 1.2 и выше. Действует на новые подключения всех путей.
pub fn set_tls13_only(on: bool) {
    postman_jmap::set_tls13_only(on);
    postman_dav::set_tls13_only(on);
}

/// Вид сервера по адресу (S7-04в): адрес без схемы IMAP — сначала JMAP (`/.well-known/jmap`), нет его — IMAP на том
/// же узле (порт 993 отвечает) → `imaps://узел`; ни того ни другого — адрес как есть (ошибку покажет вход). Адрес с
/// путём — как есть. У адреса IMAP без `dav=` — ещё CalDAV и CardDAV (S7-05б): отвечает `/.well-known/caldav` того же
/// узла (RFC 6764) — `&dav=https://узел`. SRV-записи (RFC 6186) — нет: без своего DNS-клиента.
pub async fn detect(url: &str) -> String {
    if crate::imap::is_imap_url(url) {
        return with_dav(url).await;
    }
    if postman_jmap::has_jmap(url).await {
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
    if !matches!(open, Ok(Ok(_))) {
        return url.to_owned();
    }
    with_dav(&format!("imaps://{host}")).await
}

/// Адрес IMAP с CalDAV и CardDAV на узле IMAP, если там отвечает `.well-known/caldav`; `dav=` уже есть — как есть.
async fn with_dav(imap: &str) -> String {
    let Some(dav) = dav_guess(imap) else { return imap.to_owned() };
    if postman_dav::has_dav(&dav).await { format!("{imap}{}dav={dav}", if imap.contains('?') { '&' } else { '?' }) } else { imap.to_owned() }
}

/// `https://<узел IMAP>` для адреса IMAP без `dav=`.
fn dav_guess(imap: &str) -> Option<String> {
    let (_, rest) = imap.split_once("://")?;
    let (server, query) = rest.split_once('?').unwrap_or((rest, ""));
    if query.split('&').any(|kv| kv.starts_with("dav=")) {
        return None;
    }
    let host = server.trim_end_matches('/').rsplit_once(':').map_or(server.trim_end_matches('/'), |(h, _)| h);
    (!host.is_empty()).then(|| format!("https://{host}"))
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

#[cfg(test)]
mod tests {
    #[test]
    fn dav_is_guessed_on_the_imap_host() {
        assert_eq!(super::dav_guess("imaps://mail.example.test").as_deref(), Some("https://mail.example.test"));
        assert_eq!(super::dav_guess("imaps://mail.example.test:993?smtp=s.example.test").as_deref(), Some("https://mail.example.test"));
        assert_eq!(super::dav_guess("imaps://m.example.test?dav=https://d.example.test"), None);
    }
}

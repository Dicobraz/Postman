//! Отправка — SMTP submission (RFC 6409, 8314) через `mail-send`: неявный TLS (465) или STARTTLS (587), вход
//! паролем; сертификат проверяет Windows (`rustls-platform-verifier` внутри `mail-send`). Письмо — готовые байты
//! RFC 5322 от компоновщика Postman (S/MIME — тоже); конверт — отдельно (скрытые копии в письме не видны).

use mail_send::SmtpClientBuilder;
use mail_send::smtp::message::Message;

use crate::{Config, Error, Result, Security};

/// Отправить письмо: `from` — адрес конверта, `to` — все получатели, включая скрытые копии.
pub async fn submit(cfg: &Config, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
    if to.is_empty() {
        return Err(Error::Smtp("no recipients".into()));
    }
    // Провайдер криптографии процесса — тот же, что у reqwest и IMAP (aws-lc-rs); второй вызов ничего не меняет.
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let builder = SmtpClientBuilder::new(cfg.host.as_str(), cfg.port)
        .map_err(Error::Smtp)?
        .implicit_tls(cfg.security == Security::Tls)
        .credentials((cfg.user.as_str(), cfg.password.as_str()))
        .timeout(crate::TIMEOUT);
    let builder = if crate::insecure_tls() { builder.allow_invalid_certs() } else { builder };
    let message = Message::new(from, to.iter().map(String::as_str).collect::<Vec<_>>(), raw);
    let mut client = builder.connect().await.map_err(smtp_error)?;
    client.send(message).await.map_err(smtp_error)?;
    let _ = client.quit().await;
    Ok(())
}

fn smtp_error(e: mail_send::Error) -> Error {
    match e {
        mail_send::Error::AuthenticationFailed(r) => Error::Auth(r.to_string()),
        mail_send::Error::Timeout => Error::Timeout,
        mail_send::Error::Io(e) => Error::Io(e),
        e => Error::Smtp(e.to_string()),
    }
}

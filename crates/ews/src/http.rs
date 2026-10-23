//! HTTP/1.1 к серверу Exchange одним соединением TLS: NTLM (и Kerberos у IIS) привязан к TCP — всё рукопожатие
//! и следующие запросы идут по нему. Вход — по ответу 401: схема из `WWW-Authenticate`, токены — SSPI (`sys`),
//! с привязкой к каналу TLS. Соединение закрылось — новое и вход заново.

use std::sync::Arc;
use std::time::Instant;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;
use hyper::client::conn::http1::SendRequest;
use hyper::header;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::auth::{self, Scheme};
use crate::{Auth, Error, MIN_INTERVAL, Result, TIMEOUT};

/// Ответ больше — ошибка (пачка в 20 встреч с описаниями — сотни КБ).
const MAX_BODY: usize = 16 << 20;
/// Шагов рукопожатия: NTLM — 2, Kerberos — 1–2.
const MAX_ROUNDS: usize = 4;

pub(crate) struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub location: Option<String>,
    www_authenticate: Vec<String>,
}

struct Conn {
    host: String,
    port: u16,
    sender: SendRequest<Full<Bytes>>,
    /// `tls-server-end-point` сертификата сервера.
    binding: Option<Vec<u8>>,
}

pub(crate) struct Http {
    auth: Auth,
    conn: Option<Conn>,
    last: Option<Instant>,
    pub(crate) min_interval: std::time::Duration,
    /// Предел на запрос целиком (с соединением и входом).
    pub(crate) timeout: std::time::Duration,
}

/// `POSTMAN_INSECURE_TLS=1` — не проверять сертификат сервера (как у JMAP и IMAP). Только для стенда; у рабочего
/// сервера сертификат проверяется всегда.
fn insecure_tls() -> bool {
    std::env::var("POSTMAN_INSECURE_TLS").is_ok_and(|v| v == "1")
}

fn tls_connector() -> Result<tokio_rustls::TlsConnector> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
    let builder = tokio_rustls::rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?;
    let mut config = if insecure_tls() {
        tracing::warn!("POSTMAN_INSECURE_TLS: server certificate is NOT verified (test stand only)");
        builder.dangerous().with_custom_certificate_verifier(Arc::new(stand::AnyCert(provider))).with_no_client_auth()
    } else {
        builder.with_platform_verifier().map_err(|e| Error::Tls(e.to_string()))?.with_no_client_auth()
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// Проверка сертификата стенда (`POSTMAN_INSECURE_TLS=1`): любой сертификат, подписи рукопожатия — проверяются.
mod stand {
    use std::sync::Arc;

    use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use tokio_rustls::rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
    use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use tokio_rustls::rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AnyCert(pub(super) Arc<CryptoProvider>);

    impl ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
            verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
        }

        fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
            verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
}

fn hyper_err(e: hyper::Error) -> Error {
    if e.is_closed() || e.is_canceled() || e.is_incomplete_message() { Error::Closed } else { Error::Http(e.to_string()) }
}

async fn connect(host: &str, port: u16) -> Result<Conn> {
    let tcp = TcpStream::connect((host, port)).await?;
    tcp.set_nodelay(true)?;
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|e| Error::Tls(e.to_string()))?;
    let tls = tls_connector()?.connect(name, tcp).await.map_err(|e| Error::Tls(e.to_string()))?;
    let binding = tls.get_ref().1.peer_certificates().and_then(|c| c.first()).map(|c| auth::channel_binding(c.as_ref()));
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await.map_err(hyper_err)?;
    // Соединение живёт, пока жив `sender`; закрылось — следующий запрос откроет новое.
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "ews connection closed");
        }
    });
    Ok(Conn { host: host.to_owned(), port, sender, binding })
}

impl Conn {
    async fn send(&mut self, url: &url::Url, body: &Bytes, authorization: Option<&str>) -> Result<Reply> {
        self.sender.ready().await.map_err(hyper_err)?;
        let host = if self.port == 443 { self.host.clone() } else { format!("{}:{}", self.host, self.port) };
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        let mut req = hyper::Request::post(path)
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "text/xml; charset=utf-8")
            .header(header::USER_AGENT, concat!("Postman/", env!("CARGO_PKG_VERSION")));
        if let Some(a) = authorization {
            req = req.header(header::AUTHORIZATION, a);
        }
        let req = req.body(Full::new(body.clone())).map_err(|e| Error::Http(e.to_string()))?;
        let resp = self.sender.send_request(req).await.map_err(hyper_err)?;
        let status = resp.status().as_u16();
        let www_authenticate = resp
            .headers()
            .get_all(header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .collect();
        let location = resp.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let body = Limited::new(resp.into_body(), MAX_BODY).collect().await.map_err(|e| {
            if e.is::<http_body_util::LengthLimitError>() { Error::TooLarge } else { Error::Http(e.to_string()) }
        })?;
        Ok(Reply { status, body: body.to_bytes().to_vec(), location, www_authenticate })
    }
}

impl Http {
    pub(crate) fn new(auth: Auth) -> Http {
        Http { auth, conn: None, last: None, min_interval: MIN_INTERVAL, timeout: TIMEOUT }
    }

    /// POST тела XML; ответ — любой статус (разбирает вызывающий). Вход — по 401.
    pub(crate) async fn post(&mut self, url: &url::Url, body: Vec<u8>) -> Result<Reply> {
        if let Some(last) = self.last {
            let wait = self.min_interval.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        self.last = Some(Instant::now());
        let body = Bytes::from(body);
        tokio::time::timeout(self.timeout, self.post_once(url, &body)).await.map_err(|_| Error::Timeout)?
    }

    async fn conn(&mut self, url: &url::Url) -> Result<&mut Conn> {
        let host = url.host_str().ok_or_else(|| Error::Http("URL without host".into()))?;
        let port = url.port_or_known_default().unwrap_or(443);
        let stale = match &self.conn {
            Some(c) => c.host != host || c.port != port || c.sender.is_closed(),
            None => true,
        };
        if stale {
            self.conn = Some(connect(host, port).await?);
        }
        self.conn.as_mut().ok_or(Error::Closed)
    }

    async fn post_once(&mut self, url: &url::Url, body: &Bytes) -> Result<Reply> {
        // Сервер закрыл соединение, пока оно ждало (простой, перезапуск IIS), — одна попытка на новом.
        let first = match self.conn(url).await?.send(url, body, None).await {
            Err(Error::Closed) => {
                self.conn = None;
                self.conn(url).await?.send(url, body, None).await?
            }
            r => r?,
        };
        if first.status != 401 {
            return Ok(first);
        }
        let scheme = auth::choose(&first.www_authenticate).ok_or(Error::AuthScheme)?;
        // 401 с `Connection: close` — рукопожатие на новом соединении.
        let auth = self.auth.clone();
        let conn = self.conn(url).await?;
        tracing::debug!(scheme = scheme.name(), binding = conn.binding.is_some(), "ews sign-in");
        handshake(conn, &auth, scheme, url, body).await
    }
}

async fn handshake(conn: &mut Conn, auth: &Auth, scheme: Scheme, url: &url::Url, body: &Bytes) -> Result<Reply> {
    let mut ctx = crate::sys::Context::new(scheme, auth, &format!("HTTP/{}", conn.host), conn.binding.as_deref())?;
    let mut token = ctx.step(None)?;
    for _ in 0..MAX_ROUNDS {
        let reply = conn.send(url, body, Some(&auth::header(scheme, &token))).await.map_err(|e| match e {
            Error::Closed => Error::Auth("server closed the connection during the handshake".into()),
            e => e,
        })?;
        let server = auth::token(&reply.www_authenticate, scheme);
        if reply.status != 401 {
            // Последний токен Kerberos (взаимная проверка): сервер уже проверен TLS — сбой только в журнал.
            if let (Some(t), false) = (server, ctx.done())
                && let Err(e) = ctx.step(Some(&t))
            {
                tracing::warn!(error = %e, "ews: final server token rejected");
            }
            return Ok(reply);
        }
        match server {
            Some(t) if !ctx.done() => token = ctx.step(Some(&t))?,
            _ => return Err(Error::Auth("credentials rejected".into())),
        }
    }
    Err(Error::Auth("handshake did not finish".into()))
}

//! Письмо целиком (RFC 5322) → поля тела и вложения (S7-04б): у JMAP разбор MIME делает сервер, у IMAP — Postman, и
//! только здесь, в песочнице (`docs/HANDOFF.md` §3a). Вложения — по номеру в списке; байты — отдельным запросом.

use mail_parser::{Address, MimeHeaders};
use serde::{Deserialize, Serialize};

/// Больше не разбираем (письмо IMAP целиком; S/MIME — свой предел).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// Адрес: имя и адрес.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

/// Вложение: номер в списке вложений письма.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub index: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub cid: Option<String>,
    pub inline: bool,
}

/// Разобранное письмо: то, что у JMAP даёт сервер (`Email/get` с телом).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mime {
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub parts: Vec<Part>,
}

fn addrs(a: Option<&Address<'_>>) -> Vec<Addr> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                let email = x.address.as_deref()?.to_owned();
                Some(Addr { name: x.name.as_deref().unwrap_or_default().to_owned(), email })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn attachments<'a>(msg: &'a mail_parser::Message<'a>) -> Vec<&'a mail_parser::MessagePart<'a>> {
    msg.attachments().collect()
}

/// Поля тела и список вложений.
pub fn parse(raw: &[u8]) -> Result<Mime, String> {
    if raw.len() > MAX_MESSAGE {
        return Err("письмо слишком большое".into());
    }
    let msg = mail_parser::MessageParser::default().parse(raw).ok_or("не письмо RFC 5322")?;
    let references = match msg.references() {
        mail_parser::HeaderValue::Text(t) => vec![t.to_string()],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    };
    let parts = attachments(&msg)
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            let mime = a
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".into());
            Part {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                name: a.attachment_name().unwrap_or("attachment").to_owned(),
                mime: mime.to_ascii_lowercase(),
                size: a.contents().len() as u64,
                cid: a.content_id().map(|c| c.trim_matches(['<', '>']).to_owned()),
                inline: a.content_disposition().is_some_and(|d| d.is_inline()),
            }
        })
        .collect();
    Ok(Mime {
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        reply_to: addrs(msg.reply_to()),
        message_id: msg.message_id().map(str::to_owned),
        references,
        html: msg.body_html(0).map(|h| h.into_owned()),
        text: msg.body_text(0).map(|t| t.into_owned()),
        parts,
    })
}

/// Байты вложения `index` (номер из [`parse`]).
pub fn part(raw: &[u8], index: u32) -> Result<Vec<u8>, String> {
    if raw.len() > MAX_MESSAGE {
        return Err("письмо слишком большое".into());
    }
    let msg = mail_parser::MessageParser::default().parse(raw).ok_or("не письмо RFC 5322")?;
    let all = attachments(&msg);
    let a = all.get(index as usize).ok_or("нет такого вложения")?;
    Ok(a.contents().to_vec())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::{parse, part};

    const SAMPLE: &str = "From: Анна <anna@example.test>\r\nTo: Пётр <petr@example.test>, ivan@example.test\r\nCc: team@example.test\r\nSubject: =?utf-8?B?0J7RgtGH0ZHRgg==?=\r\nMessage-ID: <m1@example.test>\r\nReferences: <a@example.test> <b@example.test>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: multipart/alternative; boundary=\"b2\"\r\n\r\n--b2\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nПривет\r\n--b2\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Привет</p>\r\n--b2--\r\n--b1\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--b1--\r\n";

    #[test]
    fn message_becomes_body_and_parts() {
        let m = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(m.to.len(), 2);
        assert_eq!((m.to[0].name.as_str(), m.to[0].email.as_str()), ("Пётр", "petr@example.test"));
        assert_eq!(m.cc[0].email, "team@example.test");
        assert_eq!(m.message_id.as_deref(), Some("m1@example.test"));
        assert_eq!(m.references, ["a@example.test", "b@example.test"]);
        assert!(m.text.as_deref().unwrap().contains("Привет"));
        assert!(m.html.as_deref().unwrap().contains("<p>Привет</p>"));
        assert_eq!(m.parts.len(), 1);
        assert_eq!((m.parts[0].name.as_str(), m.parts[0].mime.as_str(), m.parts[0].size), ("report.pdf", "application/pdf", 5));
        assert_eq!(part(SAMPLE.as_bytes(), 0).unwrap(), b"Hello");
        assert!(part(SAMPLE.as_bytes(), 1).is_err());
        assert!(parse(b"").is_err() || parse(b"").unwrap().text.is_none());
    }
}

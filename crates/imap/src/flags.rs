//! Флаги IMAP ↔ ключевые слова Postman (как у JMAP, RFC 8621 §4.1.1): системные флаги — `$seen`, `$flagged`,
//! `$answered`, `$draft`; ключевые слова IMAP (`$Forwarded`, `pm.due`, `pm.cat.clients`) — как есть, в нижнем
//! регистре. `\Deleted` и `\Recent` — не ключевые слова.

use async_imap::types::Flag;

/// Флаг ответа сервера → текст IMAP (`\Seen`, `pm.due`).
pub fn flag_text(f: &Flag<'_>) -> String {
    match f {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(s) => s.to_string(),
    }
}

/// Флаг IMAP → ключевое слово (`None` — не слово: `\Deleted`, `\Recent`).
pub fn keyword(flag: &str) -> Option<String> {
    let lower = flag.to_ascii_lowercase();
    let k = match lower.as_str() {
        "\\seen" => "$seen",
        "\\flagged" => "$flagged",
        "\\answered" => "$answered",
        "\\draft" => "$draft",
        other if other.starts_with('\\') => return None,
        other => other,
    };
    Some(k.to_owned())
}

/// Символы, которых нет в атоме IMAP (RFC 3501 `atom-specials`, кроме пробела и управляющих).
const SPECIALS: &[u8] = b"(){%*\"\\]";

/// Ключевое слово → флаг IMAP; `None` — не годится в атом IMAP (пробел, скобки, кавычки…).
pub fn flag(keyword: &str) -> Option<String> {
    let f = match keyword.to_ascii_lowercase().as_str() {
        "$seen" => "\\Seen".to_owned(),
        "$flagged" => "\\Flagged".to_owned(),
        "$answered" => "\\Answered".to_owned(),
        "$draft" => "\\Draft".to_owned(),
        _ => keyword.to_owned(),
    };
    let body = f.strip_prefix('\\').unwrap_or(&f);
    let atom = !body.is_empty() && body.bytes().all(|b| b > 0x20 && b < 0x7f && !SPECIALS.contains(&b));
    atom.then_some(f)
}

#[cfg(test)]
mod tests {
    use super::{flag, keyword};

    #[test]
    fn flags_and_keywords_round_trip() {
        for (f, k) in [("\\Seen", "$seen"), ("\\Flagged", "$flagged"), ("\\Answered", "$answered"), ("$Forwarded", "$forwarded")] {
            assert_eq!(keyword(f).as_deref(), Some(k));
        }
        assert_eq!(keyword("\\Deleted"), None);
        assert_eq!(keyword("\\Recent"), None);
        assert_eq!(flag("$seen").as_deref(), Some("\\Seen"));
        assert_eq!(flag("pm.due.2026-10-23t1500").as_deref(), Some("pm.due.2026-10-23t1500"));
        assert_eq!(flag("pm.cat.clients").as_deref(), Some("pm.cat.clients"));
        assert_eq!(flag("two words"), None);
        assert_eq!(flag("a(b"), None);
        assert_eq!(flag(""), None);
    }
}

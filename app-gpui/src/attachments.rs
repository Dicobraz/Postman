//! Вложения в области чтения: что показать полосой под шапкой, значок, размер и действия
//! «Открыть» / «Сохранить как» (`postman_core::files`: безопасное имя, метка «из Интернета»).

use std::collections::HashSet;

use pets_i18n::tr;
use postman_cache::AttachmentRow;
use postman_core::files;

/// Действие с вложением; ждёт байты, если их ещё нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAct {
    Open,
    SaveAs,
}

/// Вид файла — для значка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Image,
    Pdf,
    Document,
    Sheet,
    Slides,
    Archive,
    Calendar,
    Mail,
    Text,
    /// Исполняемое и «активное» содержимое: открыть нельзя, только сохранить.
    Blocked,
    Other,
}

/// Строка полосы вложений.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Item {
    pub(crate) row: AttachmentRow,
    /// Имя без путей, управляющих и bidi-символов: подмена `счёт<U+202E>fdp.exe` видна как есть.
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) can_open: bool,
}

/// Вложения для полосы под шапкой: картинки, показанные в тексте (`cid:`), и приглашение, показанное
/// карточкой, — не дублируются (как в Outlook).
pub(crate) fn listed(all: &[AttachmentRow], shown_cids: &HashSet<String>, invite_card: bool) -> Vec<Item> {
    all.iter()
        .filter(|a| a.cid.as_ref().is_none_or(|c| !shown_cids.contains(c)))
        .filter(|a| !(invite_card && is_invite(a)))
        .map(|a| Item {
            row: a.clone(),
            name: files::safe_name(&a.name),
            kind: kind(&a.name, &a.mime),
            can_open: files::can_open(&a.name),
        })
        .collect()
}

/// Вложение — приглашение на встречу (`text/calendar`, `.ics`).
pub(crate) fn is_invite(a: &AttachmentRow) -> bool {
    let mime = a.mime.to_ascii_lowercase();
    mime.starts_with("text/calendar") || mime == "application/ics" || a.name.to_ascii_lowercase().ends_with(".ics")
}

pub(crate) fn kind(name: &str, mime: &str) -> Kind {
    if !files::can_open(name) && name.contains('.') {
        return Kind::Blocked;
    }
    let safe = files::safe_name(name);
    let ext = safe.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" => Kind::Image,
        "pdf" => Kind::Pdf,
        "doc" | "docx" | "odt" | "rtf" => Kind::Document,
        "xls" | "xlsx" | "ods" | "csv" => Kind::Sheet,
        "ppt" | "pptx" | "odp" => Kind::Slides,
        "zip" | "7z" | "rar" | "gz" | "tar" => Kind::Archive,
        "ics" => Kind::Calendar,
        "eml" | "msg" => Kind::Mail,
        "txt" | "log" | "md" => Kind::Text,
        _ if mime.starts_with("image/") => Kind::Image,
        _ if mime.starts_with("text/calendar") => Kind::Calendar,
        _ if mime.starts_with("text/") => Kind::Text,
        _ => Kind::Other,
    }
}

pub(crate) fn icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "🖼",
        Kind::Pdf => "📕",
        Kind::Document => "📝",
        Kind::Sheet => "📊",
        Kind::Slides => "📽",
        Kind::Archive => "🗜",
        Kind::Calendar => "📅",
        Kind::Mail => "✉",
        Kind::Text => "📄",
        Kind::Blocked => "⚠",
        Kind::Other => "📎",
    }
}

/// Размер для показа: (единица для `.ftl`, число). Байты и КБ — целые, МБ — с одним знаком.
pub(crate) fn size_parts(n: u64) -> (&'static str, f64) {
    const K: f64 = 1024.0;
    match n {
        n if n < 1024 => ("b", n as f64),
        n if n < 1024 * 1024 => ("kb", (n as f64 / K).round()),
        n => ("mb", (n as f64 / K / K * 10.0).round() / 10.0),
    }
}

pub(crate) fn size_text(n: u64) -> String {
    let (unit, n) = size_parts(n);
    tr!("reading-att-size", unit = unit, n = n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, mime: &str, cid: Option<&str>) -> AttachmentRow {
        AttachmentRow {
            blob_id: name.into(),
            name: name.into(),
            mime: mime.into(),
            size: 10,
            cid: cid.map(Into::into),
            inline: cid.is_some(),
        }
    }

    #[test]
    fn inline_images_and_invite_card_are_not_listed_twice() {
        let all = [
            att("logo.png", "image/png", Some("logo@corp")),
            att("other.png", "image/png", Some("unused@corp")),
            att("Итоги Q3.xlsx", "application/vnd.ms-excel", None),
            att("invite.ics", "text/calendar", None),
        ];
        let shown: HashSet<String> = ["logo@corp".to_string()].into();
        let names = |v: Vec<Item>| v.into_iter().map(|i| i.name).collect::<Vec<_>>();
        assert_eq!(names(listed(&all, &shown, true)), ["other.png", "Итоги Q3.xlsx"]);
        // Карточка не разобралась — приглашение остаётся файлом.
        assert_eq!(names(listed(&all, &shown, false)), ["other.png", "Итоги Q3.xlsx", "invite.ics"]);
        assert_eq!(listed(&all, &HashSet::new(), true).len(), 3);
    }

    #[test]
    fn dangerous_names_are_shown_safe_and_cannot_be_opened() {
        let all = [att("счёт\u{202e}fdp.exe", "application/octet-stream", None), att("invoice.pdf.exe", "", None)];
        let items = listed(&all, &HashSet::new(), false);
        assert_eq!(items[0].name, "счётfdp.exe");
        assert!(items.iter().all(|i| !i.can_open && i.kind == Kind::Blocked));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind("Итоги Q3.xlsx", ""), Kind::Sheet);
        assert_eq!(kind("scan.PDF", ""), Kind::Pdf);
        assert_eq!(kind("photo", "image/jpeg"), Kind::Image);
        assert_eq!(kind("invite.ics", "text/calendar"), Kind::Calendar);
        assert_eq!(kind("page.html", "text/html"), Kind::Blocked);
        assert_eq!(kind("noext", "application/octet-stream"), Kind::Other);
        assert!(is_invite(&att("x", "text/calendar; method=REQUEST", None)));
        assert!(is_invite(&att("Meeting.ICS", "application/octet-stream", None)));
        assert!(!is_invite(&att("a.txt", "text/plain", None)));
    }

    #[test]
    fn sizes() {
        assert_eq!(size_parts(512), ("b", 512.0));
        assert_eq!(size_parts(18_432), ("kb", 18.0));
        assert_eq!(size_parts(1_572_864), ("mb", 1.5));
    }
}

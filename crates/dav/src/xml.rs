//! XML WebDAV: разбор ответов (207 Multi-Status, `schedule-response`) в маленькое дерево и сборка тел запросов.
//!
//! Ответ сервера — без DTD и своих сущностей: `<!DOCTYPE` — ошибка, сущность кроме пяти встроенных XML и
//! символьных ссылок — ошибка. Пределы — глубина и число элементов; размер тела ограничивает транспорт
//! (`crate::MAX_BODY`).

use std::borrow::Cow;

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
/// `getctag` (Apple CalendarServer) — запасной признак изменений коллекции без `sync-collection`.
pub const CS: &str = "http://calendarserver.org/ns/";
/// `calendar-color` (Apple iCal).
pub const ICAL: &str = "http://apple.com/ns/ical/";

/// Глубже ответы WebDAV не бывают (multistatus → response → propstat → prop → свойство → …).
const MAX_DEPTH: usize = 64;
/// Элементов в одном ответе — с запасом на тысячи ресурсов коллекции.
const MAX_ELEMENTS: usize = 1_000_000;

/// Элемент XML: пространство имён (URI, не префикс), локальное имя, атрибуты без пространств имён, текст.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub ns: String,
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// Текст и CDATA подряд, как в документе (без обрезки).
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    pub fn child(&self, ns: &str, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is(ns, name))
    }

    pub fn all<'a>(&'a self, ns: &'a str, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.is(ns, name))
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Текст без пробелов по краям.
    pub fn value(&self) -> &str {
        self.text.trim()
    }

    /// Текст `DAV:href` внутри (`current-user-principal`, `calendar-home-set` …).
    pub fn href(&self) -> Option<&str> {
        self.child(DAV, "href").map(Element::value).filter(|h| !h.is_empty())
    }

    /// Все `DAV:href` внутри (`calendar-user-address-set`, несколько домашних наборов).
    pub fn hrefs(&self) -> Vec<String> {
        self.all(DAV, "href").map(|h| h.value().to_owned()).filter(|h| !h.is_empty()).collect()
    }
}

/// Ошибка разбора: текст для журнала, без данных ответа.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct XmlError(pub String);

fn err(s: impl Into<String>) -> XmlError {
    XmlError(s.into())
}

/// Разобрать документ целиком; корень — единственный элемент верхнего уровня.
pub fn parse(body: &[u8]) -> Result<Element, XmlError> {
    let mut reader = NsReader::from_reader(body);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut count = 0usize;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|e| err(format!("xml: {e}")))?;
        match event {
            Event::Start(_) | Event::Empty(_) if root.is_some() && stack.is_empty() => {
                return Err(err("xml: more than one root element"));
            }
            Event::Start(e) => {
                count += 1;
                if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                    return Err(err("xml: too many or too deep elements"));
                }
                stack.push(element(ns, &e)?);
            }
            Event::Empty(e) => {
                count += 1;
                if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                    return Err(err("xml: too many or too deep elements"));
                }
                let el = element(ns, &e)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::End(_) => {
                let el = stack.pop().ok_or_else(|| err("xml: unexpected end tag"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&t.decode().map_err(|e| err(format!("xml text: {e}")))?);
                }
            }
            Event::CData(t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&t.decode().map_err(|e| err(format!("xml cdata: {e}")))?);
                }
            }
            Event::GeneralRef(r) => {
                let ch: Cow<'static, str> = if let Some(c) = r.resolve_char_ref().map_err(|e| err(format!("xml ref: {e}")))? {
                    Cow::Owned(c.to_string())
                } else {
                    let name = r.decode().map_err(|e| err(format!("xml ref: {e}")))?;
                    let known = quick_xml::escape::resolve_predefined_entity(&name).ok_or_else(|| err("xml: unknown entity"))?;
                    Cow::Borrowed(known)
                };
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&ch);
                }
            }
            Event::DocType(_) => return Err(err("xml: DTD is not allowed")),
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(err("xml: unclosed element"));
    }
    root.ok_or_else(|| err("xml: empty document"))
}

fn element(ns: ResolveResult<'_>, e: &quick_xml::events::BytesStart<'_>) -> Result<Element, XmlError> {
    let ns = match ns {
        ResolveResult::Bound(n) => String::from_utf8_lossy(n.as_ref()).into_owned(),
        ResolveResult::Unbound => String::new(),
        ResolveResult::Unknown(p) => return Err(err(format!("xml: unknown prefix {}", String::from_utf8_lossy(&p)))),
    };
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| err(format!("xml attr: {e}")))?;
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        if a.key.as_ref().starts_with(b"xmlns") {
            continue;
        }
        let value = a.normalized_value(quick_xml::XmlVersion::default()).map_err(|e| err(format!("xml attr: {e}")))?.into_owned();
        attrs.push((key, value));
    }
    Ok(Element { ns, name, attrs, text: String::new(), children: Vec::new() })
}

/// Экранировать текст для тела запроса (href, токен, имя).
pub fn escape(s: &str) -> Cow<'_, str> {
    quick_xml::escape::escape(s)
}

/// Начало тела запроса: объявление и корень с пространствами имён `d:` (DAV), `c:` (CalDAV), `r:` (CardDAV),
/// `cs:` (CalendarServer), `a:` (Apple iCal).
pub(crate) fn open(root: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><{root} xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\" xmlns:r=\"{CARDDAV}\" xmlns:cs=\"{CS}\" xmlns:a=\"{ICAL}\">"
    )
}

/// `PROPFIND` с перечисленными свойствами (`d:displayname`, `c:calendar-home-set` …).
pub(crate) fn propfind(props: &[&str]) -> String {
    let mut s = open("d:propfind");
    s.push_str("<d:prop>");
    for p in props {
        s.push('<');
        s.push_str(p);
        s.push_str("/>");
    }
    s.push_str("</d:prop></d:propfind>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_resolve_by_uri_not_prefix() {
        let doc = br#"<?xml version="1.0"?><X:multistatus xmlns:X="DAV:"><X:response><calendar-color xmlns="http://apple.com/ns/ical/">#FF0000</calendar-color></X:response></X:multistatus>"#;
        let root = parse(doc).unwrap();
        assert!(root.is(DAV, "multistatus"));
        let color = root.child(DAV, "response").unwrap().child(ICAL, "calendar-color").unwrap();
        assert_eq!(color.value(), "#FF0000");
    }

    #[test]
    fn predefined_entities_and_char_refs_are_resolved() {
        let root = parse(br#"<a xmlns="DAV:">&quot;12&quot; &amp; &#x41;&#66;</a>"#).unwrap();
        assert_eq!(root.text, "\"12\" & AB");
    }

    #[test]
    fn dtd_and_custom_entities_are_rejected() {
        let bomb = br#"<?xml version="1.0"?><!DOCTYPE a [<!ENTITY x "xxxxxxxx">]><a>&x;</a>"#;
        assert!(parse(bomb).is_err());
        assert!(parse(br#"<a xmlns="DAV:">&x;</a>"#).is_err());
    }

    #[test]
    fn depth_limit_and_broken_documents() {
        let deep = format!("{}{}", "<a>".repeat(MAX_DEPTH + 1), "</a>".repeat(MAX_DEPTH + 1));
        assert!(parse(deep.as_bytes()).is_err());
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"<a>").is_err());
        assert!(parse(b"").is_err());
        assert!(parse(b"<a/><b/>").is_err());
        assert!(parse(b"<x:a/>").is_err());
    }

    #[test]
    fn cdata_keeps_calendar_data_as_is() {
        let root = parse(b"<a xmlns=\"DAV:\"><![CDATA[BEGIN:VCALENDAR\r\nX:<b>&amp;\r\n]]></a>").unwrap();
        assert_eq!(root.text, "BEGIN:VCALENDAR\r\nX:<b>&amp;\r\n");
    }

    #[test]
    fn request_body_is_well_formed() {
        let body = propfind(&["d:displayname", "c:calendar-home-set"]);
        let root = parse(body.as_bytes()).unwrap();
        let prop = root.child(DAV, "prop").unwrap();
        assert!(prop.child(CALDAV, "calendar-home-set").is_some());
        assert_eq!(escape("a<&\"b"), "a&lt;&amp;&quot;b");
    }
}

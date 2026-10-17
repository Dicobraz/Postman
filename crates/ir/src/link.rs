use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct LinkCheck {
    pub href: String,
    pub host: Option<String>,
    pub suspicious: bool,
}

/// Проверяет ссылку: разрешены только `http(s):` и `mailto:`.
/// `visible` — видимый текст ссылки; если он выглядит как адрес другого хоста,
/// ссылка помечается подозрительной.
pub fn check_link(href: &str, visible: &str) -> Option<LinkCheck> {
    let url = Url::parse(href.trim()).ok()?;
    match url.scheme() {
        "http" | "https" => {
            let host = url.host_str()?.to_string();
            let suspicious = visible_host(visible).is_some_and(|v| !same_host(&v, &host));
            Some(LinkCheck { href: url.into(), host: Some(host), suspicious })
        }
        "mailto" => Some(LinkCheck { href: url.into(), host: None, suspicious: false }),
        _ => None,
    }
}

/// Хост из видимого текста, если текст похож на URL или доменное имя.
fn visible_host(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) || !t.contains('.') {
        return None;
    }
    let candidate = if t.contains("://") { t.to_string() } else { format!("http://{t}") };
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    // «Итого 5.00» или «v1.2» — не домен.
    let tld = host.rsplit('.').next()?;
    if tld.len() < 2 || tld.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(host.to_string())
}

fn same_host(a: &str, b: &str) -> bool {
    a.trim_start_matches("www.") == b.trim_start_matches("www.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert!(check_link("javascript:alert(1)", "x").is_none());
        assert!(check_link("data:text/html,hi", "x").is_none());
        assert!(check_link("file:///c:/x", "x").is_none());
        assert!(check_link("mailto:a@b.c", "a@b.c").is_some());
    }

    #[test]
    fn mismatch() {
        let l = check_link("https://evil.example/login", "https://bank.ru").unwrap();
        assert!(l.suspicious);
        let l = check_link("https://www.bank.ru/x", "bank.ru").unwrap();
        assert!(!l.suspicious);
        let l = check_link("https://evil.example/", "Войти в банк").unwrap();
        assert!(!l.suspicious);
    }

    #[test]
    fn idn_shown_as_punycode() {
        let l = check_link("https://аpple.com/", "apple.com").unwrap();
        assert!(l.host.as_deref().unwrap().starts_with("xn--"));
        assert!(l.suspicious);
    }
}

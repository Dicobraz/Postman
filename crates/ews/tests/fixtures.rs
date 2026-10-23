//! Корпус ответов EWS обезличен (S7 «Проверка на боевом Exchange»): адреса — только `example.test` и его
//! поддомены; с окружением боевого Exchange — ни домена, ни адресов, ни имён входа из `POSTMAN_EWS_*`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // тест: «не падать» — для кода приложения (S2-03)

use std::path::PathBuf;

fn fixtures() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap()))
        .collect();
    out.sort();
    assert!(!out.is_empty());
    out
}

/// Домены всех `x@domain` в тексте.
fn domains(text: &str) -> Vec<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    text.match_indices('@')
        .map(|(i, _)| text[i + 1..].chars().take_while(|c| ok(*c)).collect::<String>().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}

#[test]
fn fixtures_have_no_stand_names() {
    let mut secrets: Vec<String> = Vec::new();
    for var in ["POSTMAN_EWS_URL", "POSTMAN_EWS_EMAIL", "POSTMAN_EWS_USER", "POSTMAN_EWS_EMAIL2", "POSTMAN_EWS_USER2"] {
        let Ok(v) = std::env::var(var) else { continue };
        let v = v.trim().to_ascii_lowercase();
        if v.is_empty() {
            continue;
        }
        // Адрес, вход и их домен (`user@corp.ru` → `corp.ru`, `CORP\user` → `corp`, URL → хост).
        secrets.extend(url_host(&v));
        if let Some((user, domain)) = v.rsplit_once('@') {
            secrets.push(user.to_owned());
            secrets.push(domain.to_owned());
        } else if let Some((domain, user)) = v.split_once('\\') {
            secrets.push(user.to_owned());
            secrets.push(domain.to_owned());
        }
        secrets.push(v);
    }
    secrets.retain(|s| s.len() >= 3 && !s.ends_with("example.test"));
    for (name, text) in fixtures() {
        for d in domains(&text) {
            assert!(d == "example.test" || d.ends_with(".example.test"), "{name}: адрес не из example.test");
        }
        let lower = text.to_ascii_lowercase();
        for s in &secrets {
            assert!(!lower.contains(s.as_str()), "{name}: в корпусе значение из окружения POSTMAN_EWS_*");
        }
    }
}

/// Хост адреса и его домен без первой метки (`https://mail.corp.ru/…` → `mail.corp.ru`, `corp.ru`).
fn url_host(v: &str) -> Vec<String> {
    let Some(rest) = v.strip_prefix("https://").or_else(|| v.strip_prefix("http://")) else { return Vec::new() };
    let host = rest.split(['/', ':']).next().unwrap_or("").to_owned();
    let mut out = vec![host.clone()];
    if let Some((_, parent)) = host.split_once('.')
        && parent.contains('.')
    {
        out.push(parent.to_owned());
    }
    out
}

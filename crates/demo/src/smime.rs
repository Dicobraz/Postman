//! Подписанные письма демо-ящика (S5-12 Ф1): готовые образцы S/MIME и корни демо-УЦ.
//!
//! Образцы собирает тест `crates/demo/tests/smime_samples.rs` (`POSTMAN_UPDATE_SAMPLES=1` — пересобрать; иначе он
//! проверяет, что образцы дают ожидаемые итоги). Ключи — тестовые (`postman_smime::testkit`), сертификаты действуют
//! с 2026 по 2046 год.

/// Образец: что видит список и что подписано.
pub struct Sample {
    /// Имя образца и ключ блоба письма целиком (`smime-<kind>`).
    pub kind: &'static str,
    pub from_name: &'static str,
    pub from_addr: &'static str,
    pub subject: &'static str,
    /// Текст, который показывает сервер (у `bad` — изменённый после подписи).
    pub html: &'static str,
    /// Что подписано (у `bad` — исходный текст).
    pub signed_html: &'static str,
}

const RSA_HTML: &str = "<p>Коллеги, добрый день!</p><p>Направляю подписанную версию договора поставки. Письмо подписано \
сертификатом корпоративного удостоверяющего центра.</p><p>С уважением,<br>Иван Петров</p>";
const GOST_HTML: &str = "<p>Добрый день!</p><p>Акт сверки за III квартал подписан электронной подписью по \
ГОСТ Р 34.10-2012.</p><p>Мария Соколова, бухгалтерия</p>";
const BAD_SIGNED: &str = "<p>Прошу оплатить счёт №2041 на сумму 120 000 ₽.</p><p>Бухгалтерия</p>";
const BAD_SHOWN: &str = "<p>Прошу оплатить счёт №2041 на сумму 920 000 ₽.</p><p>Бухгалтерия</p>";
const UNTRUSTED_HTML: &str = "<p>Здравствуйте!</p><p>Высылаем коммерческое предложение. Письмо подписано \
сертификатом нашей компании.</p><p>ООО «Вектор»</p>";

pub const SAMPLES: &[Sample] = &[
    Sample {
        kind: "rsa",
        from_name: "Петров Иван",
        from_addr: "i.petrov@corp.example",
        subject: "Договор поставки: подписанная версия",
        html: RSA_HTML,
        signed_html: RSA_HTML,
    },
    Sample {
        kind: "gost",
        from_name: "Соколова Мария",
        from_addr: "m.sokolova@corp.example",
        subject: "Акт сверки за III квартал",
        html: GOST_HTML,
        signed_html: GOST_HTML,
    },
    Sample {
        kind: "bad",
        from_name: "Бухгалтерия",
        from_addr: "buh@corp.example",
        subject: "Счёт на оплату №2041",
        html: BAD_SHOWN,
        signed_html: BAD_SIGNED,
    },
    Sample {
        kind: "untrusted",
        from_name: "ООО «Вектор»",
        from_addr: "info@vector.example",
        subject: "Коммерческое предложение",
        html: UNTRUSTED_HTML,
        signed_html: UNTRUSTED_HTML,
    },
];

/// Демо-письмо `n` — подписанный образец (одно на сотню для каждого вида, в начале ящика).
pub fn sample_for(n: usize) -> Option<&'static Sample> {
    match n % 100 {
        1 | 11 => SAMPLES.first(),
        3 | 13 => SAMPLES.get(1),
        5 | 15 => SAMPLES.get(2),
        7 | 17 => SAMPLES.get(3),
        _ => None,
    }
}

/// Начало текста для списка (как `preview` сервера): HTML без тегов.
pub fn preview(s: &Sample) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.html.chars() {
        match c {
            '<' => {
                tag = true;
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Письмо целиком (RFC 5322) образца.
pub fn raw(kind: &str) -> Option<&'static [u8]> {
    Some(match kind {
        "rsa" => include_bytes!("smime/rsa.eml"),
        "gost" => include_bytes!("smime/gost.eml"),
        "bad" => include_bytes!("smime/bad.eml"),
        "untrusted" => include_bytes!("smime/untrusted.eml"),
        _ => return None,
    })
}

/// Подпись образца (`smime.p7s` — вложение в ленте; её байты демо не нужны интерфейсу, только размер).
pub const P7S_SIZE: u64 = 2_300;

/// Корни демо-УЦ (DER) и метка «корпоративный»: корпоративный RSA и ГОСТ-УЦ контрагента; у `untrusted` корня нет.
pub fn anchors() -> Vec<(Vec<u8>, bool)> {
    vec![(include_bytes!("smime/ca-rsa.der").to_vec(), true), (include_bytes!("smime/ca-gost.der").to_vec(), false)]
}

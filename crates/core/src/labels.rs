//! Метки и категории на ключевых словах JMAP (S5-02, `docs/DESIGN.md` «Строка письма и метки»).
//!
//! Ключевое слово JMAP — ASCII без пробелов и `( ) { ] % * " \`, в нижнем регистре (RFC 8621 §4.1.1), а имена
//! категорий — любые («Клиенты»). Поэтому на сервере у письма не имя, а id категории: ключевое слово
//! `pm.cat.<id>` (id — `[a-z0-9-]`), имя и цвет — в таблице категорий аккаунта. Без `$`: так помечают слова
//! с общим смыслом (`$seen`, `$flagged`, `$label1`), а это — слово Postman.
//!
//! Совместимость: `$seen`, `$flagged`, `$answered`, `$forwarded`, `$draft` — состояния письма, меток в строке у них
//! нет (флаг — значком). Метки Thunderbird `$label1…$label5` («Важное», «Работа», «Личное», «Сделать», «Позже») —
//! встроенные категории: видны у письма нейтральной меткой; в таблицу их можно взять с цветом (ключевое слово
//! категории — как есть, `$label1`).
//!
//! Таблица категорий — по аккаунту ([`Book`]). Где она лежит — за [`Store`]: сейчас локальный файл ([`LocalFile`]),
//! дальше — файл аккаунта на сервере (JMAP FileNode, `docs/specs/S5.md`, «Решения для владельца»).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::palette;

pub const SEEN: &str = "$seen";
pub const FLAGGED: &str = "$flagged";
pub const ANSWERED: &str = "$answered";
pub const FORWARDED: &str = "$forwarded";
pub const DRAFT: &str = "$draft";
/// Префикс ключевого слова категории Postman.
pub const CATEGORY_PREFIX: &str = "pm.cat.";
/// Метки Thunderbird по номеру: `$label1` — 1.
pub const THUNDERBIRD: [&str; 5] = ["$label1", "$label2", "$label3", "$label4", "$label5"];
/// Больше меток в строке письма не помещается (DESIGN: имя отправителя ≥ 60 px).
pub const ROW_LABELS: usize = 2;

/// Что значит ключевое слово письма.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword<'a> {
    Seen,
    Flagged,
    Answered,
    Forwarded,
    Draft,
    /// Категория Postman: id из `pm.cat.<id>`.
    Category(&'a str),
    /// Метка Thunderbird `$labelN`, N = 1…5.
    Thunderbird(u8),
    /// Чужое ключевое слово (`$junk`, `$mdnsent`, слово другого клиента) — не показываем, не трогаем.
    Other(&'a str),
}

/// Разбор ключевого слова (регистр не важен: в JMAP он всегда нижний, но IMAP-клиенты пишут как придётся).
pub fn classify(keyword: &str) -> Keyword<'_> {
    let lower = |s: &str| keyword.eq_ignore_ascii_case(s);
    if lower(SEEN) {
        Keyword::Seen
    } else if lower(FLAGGED) {
        Keyword::Flagged
    } else if lower(ANSWERED) {
        Keyword::Answered
    } else if lower(FORWARDED) {
        Keyword::Forwarded
    } else if lower(DRAFT) {
        Keyword::Draft
    } else if let Some(n) = THUNDERBIRD.iter().position(|t| lower(t)) {
        Keyword::Thunderbird(n as u8 + 1)
    } else if let Some(id) = keyword.get(..CATEGORY_PREFIX.len()).filter(|p| p.eq_ignore_ascii_case(CATEGORY_PREFIX)) {
        let id = &keyword[id.len()..];
        if valid_id(id) { Keyword::Category(id) } else { Keyword::Other(keyword) }
    } else {
        Keyword::Other(keyword)
    }
}

/// id категории: 1–64 знака `[a-z0-9-]`.
pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Ключевое слово категории с id `id`.
pub fn category_keyword(id: &str) -> String {
    format!("{CATEGORY_PREFIX}{id}")
}

/// Допустимое ключевое слово JMAP: 1–255 знаков `%x21-7E` без `( ) { ] % * " \`, без заглавных (RFC 8621 §4.1.1).
pub fn valid_keyword(k: &str) -> bool {
    (1..=255).contains(&k.len())
        && k.bytes().all(|b| (0x21..=0x7e).contains(&b) && !b"(){]%*\"\\".contains(&b) && !b.is_ascii_uppercase())
}

/// Ключевое слово в виде, в котором его хранит JMAP (нижний регистр).
pub fn normalize(k: &str) -> String {
    k.to_ascii_lowercase()
}

/// Категория таблицы аккаунта.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// Ключевое слово на сервере: `pm.cat.<id>` или взятая в таблицу метка Thunderbird (`$label1`).
    pub keyword: String,
    pub name: String,
    /// Слот палитры ([`palette::HUES`]).
    pub color: u8,
    /// Клавиша 1–9, которая ставит и снимает категорию на выбранном письме.
    #[serde(default)]
    pub key: Option<u8>,
}

/// Ошибка в таблице категорий.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Problem {
    #[error("{0}: not a JMAP keyword")]
    BadKeyword(String),
    #[error("{0}: used twice")]
    DuplicateKeyword(String),
    #[error("key {0}: used twice")]
    DuplicateKey(u8),
    #[error("key {0}: not 1-9")]
    BadKey(u8),
    #[error("{name}: colour too close ({clash:?}, ΔE {delta:.1})")]
    Colour { name: String, clash: palette::Clash, delta: f64 },
}

/// Таблица категорий одного аккаунта; порядок — порядок меток в строке и фильтров.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Categories {
    pub list: Vec<Category>,
}

/// Категории по умолчанию (DESIGN.md «Цвет»): id, слот палитры, клавиша. Имена — у интерфейса (Fluent).
pub const DEFAULTS: &[(&str, u8, u8)] = &[
    ("clients", palette::BLUE, 1),
    ("team", palette::GREEN, 2),
    ("tenders", palette::VIOLET, 3),
    ("pentest", palette::TEAL, 4),
    ("services", palette::NEUTRAL, 5),
];

impl Categories {
    /// Таблица по умолчанию; `name(id)` — имя категории на языке интерфейса.
    pub fn defaults(name: impl Fn(&str) -> String) -> Self {
        let list = DEFAULTS
            .iter()
            .map(|&(id, color, key)| Category { keyword: category_keyword(id), name: name(id), color, key: Some(key) })
            .collect();
        Self { list }
    }

    /// Категория по ключевому слову и её место в таблице.
    pub fn by_keyword(&self, keyword: &str) -> Option<(usize, &Category)> {
        self.list.iter().enumerate().find(|(_, c)| c.keyword.eq_ignore_ascii_case(keyword))
    }

    /// Категория на клавише `key`.
    pub fn by_key(&self, key: u8) -> Option<&Category> {
        self.list.iter().find(|c| c.key == Some(key))
    }

    /// Ключевое слово для новой категории: `pm.cat.c<N>`, свободное в таблице.
    pub fn new_keyword(&self) -> String {
        (1..).map(|n| category_keyword(&format!("c{n}"))).find(|k| self.by_keyword(k).is_none()).unwrap_or_default()
    }

    /// Таблица годится: ключевые слова JMAP без повторов, клавиши 1–9 без повторов, цвета различимы (ΔE2000).
    pub fn check(&self) -> Result<(), Problem> {
        let mut used = Vec::new();
        for (i, c) in self.list.iter().enumerate() {
            if !valid_keyword(&c.keyword) {
                return Err(Problem::BadKeyword(c.keyword.clone()));
            }
            if self.list[..i].iter().any(|o| o.keyword.eq_ignore_ascii_case(&c.keyword)) {
                return Err(Problem::DuplicateKeyword(c.keyword.clone()));
            }
            if let Some(k) = c.key {
                if !(1..=9).contains(&k) {
                    return Err(Problem::BadKey(k));
                }
                if self.list[..i].iter().any(|o| o.key == Some(k)) {
                    return Err(Problem::DuplicateKey(k));
                }
            }
            if let Some((clash, delta)) = palette::clash(c.color, &used) {
                return Err(Problem::Colour { name: c.name.clone(), clash, delta });
            }
            used.push(c.color);
        }
        Ok(())
    }

    /// Метки письма с ключевыми словами `keywords` для строки списка: категории в порядке таблицы, затем метки
    /// Thunderbird, которых нет в таблице. Первая — категория строки (полоса слева).
    pub fn labels<'a>(&'a self, keywords: &[String]) -> Vec<Label<'a>> {
        let mut cats: Vec<(usize, &Category)> = keywords.iter().filter_map(|k| self.by_keyword(k)).collect();
        cats.sort_by_key(|(i, _)| *i);
        cats.dedup_by_key(|(i, _)| *i);
        let mut out: Vec<Label<'a>> = cats.into_iter().map(|(_, c)| Label::Category(c)).collect();
        let mut tb: Vec<u8> = keywords
            .iter()
            .filter(|k| self.by_keyword(k).is_none())
            .filter_map(|k| match classify(k) {
                Keyword::Thunderbird(n) => Some(n),
                _ => None,
            })
            .collect();
        tb.sort_unstable();
        tb.dedup();
        out.extend(tb.into_iter().map(Label::Thunderbird));
        out
    }
}

/// Метка в строке письма.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label<'a> {
    Category(&'a Category),
    /// Метка Thunderbird вне таблицы: имя — у интерфейса, цвет — нейтральный.
    Thunderbird(u8),
}

impl Label<'_> {
    /// Слот палитры метки.
    pub fn color(&self) -> u8 {
        match self {
            Label::Category(c) => c.color,
            Label::Thunderbird(_) => palette::NEUTRAL,
        }
    }
}

/// Переключить ключевое слово у писем: если у всех оно есть — снять, иначе — поставить (как категория в Outlook
/// на нескольких выбранных письмах). Ответ — (поставить, снять).
pub fn toggle<'a>(rows: impl IntoIterator<Item = &'a [String]>, keyword: &str) -> (Vec<String>, Vec<String>) {
    let all = rows.into_iter().all(|kws| kws.iter().any(|k| k.eq_ignore_ascii_case(keyword)));
    let k = vec![normalize(keyword)];
    if all { (Vec::new(), k) } else { (k, Vec::new()) }
}

/// Таблицы категорий по аккаунтам: ключ — id аккаунта на сервере, свой аккаунт — [`OWN`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    /// Формат файла: растёт при несовместимых изменениях.
    pub version: u32,
    pub accounts: BTreeMap<String, Categories>,
}

/// Ключ своего аккаунта в [`Book::accounts`].
pub const OWN: &str = "";
pub const BOOK_VERSION: u32 = 1;

impl Book {
    /// Только таблица своего аккаунта.
    pub fn own(table: Categories) -> Self {
        Self { version: BOOK_VERSION, accounts: BTreeMap::from([(OWN.to_owned(), table)]) }
    }

    /// Таблица аккаунта (`None` — свой); у общего ящика своей нет — своя.
    pub fn for_account(&self, account: Option<&str>) -> Option<&Categories> {
        account.and_then(|a| self.accounts.get(a)).or_else(|| self.accounts.get(OWN))
    }
}

/// Ошибка хранилища таблиц.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("labels file: {0}")]
    Io(#[from] std::io::Error),
    #[error("labels file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("labels file version {0} is newer than this Postman")]
    Version(u32),
}

/// Где лежат таблицы категорий. Сейчас — [`LocalFile`]; файл аккаунта на сервере (FileNode) — та же граница.
pub trait Store: Send + Sync {
    /// `None` — таблиц ещё нет (первый запуск): интерфейс берёт [`Categories::defaults`].
    fn load(&self) -> Result<Option<Book>, StoreError>;
    fn save(&self, book: &Book) -> Result<(), StoreError>;
}

/// Таблицы в файле JSON рядом с кэшем учётной записи (кэш пересоздаётся при смене формата — таблица в нём бы
/// пропала, поэтому отдельно).
pub struct LocalFile(pub PathBuf);

impl LocalFile {
    /// Файл таблиц для кэша `cache`: `<кэш>.labels.json`.
    pub fn beside(cache: &Path) -> Self {
        let mut name = cache.file_name().unwrap_or_default().to_os_string();
        name.push(".labels.json");
        Self(cache.with_file_name(name))
    }
}

impl Store for LocalFile {
    fn load(&self) -> Result<Option<Book>, StoreError> {
        let text = match std::fs::read_to_string(&self.0) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let book: Book = serde_json::from_str(&text)?;
        if book.version > BOOK_VERSION {
            return Err(StoreError::Version(book.version));
        }
        Ok(Some(book))
    }

    /// Запись через временный файл: сбой посреди записи не портит прежнюю таблицу.
    fn save(&self, book: &Book) -> Result<(), StoreError> {
        let text = serde_json::to_string_pretty(book)?;
        let mut tmp = self.0.clone().into_os_string();
        tmp.push(".tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kws(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn defaults() -> Categories {
        Categories::defaults(|id| format!("[{id}]"))
    }

    /// Ключевое слово категории — ASCII JMAP, туда и обратно; чужие и системные слова узнаются.
    #[test]
    fn keywords_round_trip() {
        for id in ["clients", "c12", "a-b"] {
            let k = category_keyword(id);
            assert!(valid_keyword(&k), "{k}");
            assert_eq!(classify(&k), Keyword::Category(id));
        }
        assert_eq!(classify("PM.Cat.clients"), Keyword::Category("clients"));
        assert_eq!(classify("pm.cat.Клиенты"), Keyword::Other("pm.cat.Клиенты"));
        assert_eq!(classify("pm.cat."), Keyword::Other("pm.cat."));
        assert_eq!(classify("$Flagged"), Keyword::Flagged);
        assert_eq!(classify("$seen"), Keyword::Seen);
        assert_eq!(classify("$answered"), Keyword::Answered);
        assert_eq!(classify("$forwarded"), Keyword::Forwarded);
        assert_eq!(classify("$draft"), Keyword::Draft);
        assert_eq!(classify("$label3"), Keyword::Thunderbird(3));
        assert_eq!(classify("$Label5"), Keyword::Thunderbird(5));
        assert_eq!(classify("$label6"), Keyword::Other("$label6"));
        assert_eq!(classify("$junk"), Keyword::Other("$junk"));
        assert!(!valid_keyword("Клиенты") && !valid_keyword("a b") && !valid_keyword("") && !valid_keyword("Pm.cat.x"));
        assert!(!valid_keyword("a(b") && !valid_keyword("a]") && !valid_keyword(&"x".repeat(256)));
        assert!(!valid_id("Clients") && !valid_id("") && !valid_id("a.b") && !valid_id(&"a".repeat(65)));
        assert_eq!(normalize("$Label1"), "$label1");
    }

    #[test]
    fn default_table_is_valid() {
        let t = defaults();
        assert_eq!(t.check(), Ok(()));
        assert_eq!(t.list.len(), 5);
        assert_eq!(t.by_key(1).map(|c| c.keyword.as_str()), Some("pm.cat.clients"));
        assert_eq!(t.by_key(5).map(|c| c.color), Some(palette::NEUTRAL));
        assert_eq!(t.by_key(6), None);
        assert_eq!(t.list[1].name, "[team]");
        assert_eq!(t.new_keyword(), "pm.cat.c1");
    }

    #[test]
    fn bad_tables_are_rejected() {
        let mut t = defaults();
        t.list[1].key = Some(1);
        assert_eq!(t.check(), Err(Problem::DuplicateKey(1)));
        let mut t = defaults();
        t.list[0].key = Some(10);
        assert_eq!(t.check(), Err(Problem::BadKey(10)));
        let mut t = defaults();
        t.list[2].keyword = "PM.CAT.clients".into();
        assert!(matches!(t.check(), Err(Problem::BadKeyword(_))));
        t.list[2].keyword = "pm.cat.clients".into();
        assert!(matches!(t.check(), Err(Problem::DuplicateKeyword(_))));
        // Шестая категория цветом рядом с синим.
        let mut t = defaults();
        let keyword = t.new_keyword();
        t.list.push(Category { keyword, name: "x".into(), color: 6, key: Some(6) });
        assert!(matches!(t.check(), Err(Problem::Colour { .. })));
    }

    /// Метки строки: категории в порядке таблицы (первая — полоса), затем метки Thunderbird вне таблицы.
    #[test]
    fn row_labels() {
        let t = defaults();
        let row = kws(&["$seen", "pm.cat.services", "$label2", "pm.cat.clients", "$junk", "pm.cat.unknown"]);
        let labels = t.labels(&row);
        let shown: Vec<String> = labels
            .iter()
            .map(|l| match l {
                Label::Category(c) => c.keyword.clone(),
                Label::Thunderbird(n) => format!("tb{n}"),
            })
            .collect();
        assert_eq!(shown, ["pm.cat.clients", "pm.cat.services", "tb2"]);
        assert_eq!(labels[0].color(), palette::BLUE);
        assert_eq!(labels[2].color(), palette::NEUTRAL);
        assert!(t.labels(&kws(&["$seen", "$flagged"])).is_empty());
        // Метка Thunderbird, взятая в таблицу, — категория с цветом.
        let mut t = t;
        t.list.insert(0, Category { keyword: "$label2".into(), name: "Work".into(), color: 6, key: None });
        assert_eq!(t.labels(&row)[0], Label::Category(&t.list[0]));
        assert_eq!(t.labels(&row).len(), 3);
    }

    #[test]
    fn toggle_sets_unless_all_have_it() {
        let (a, b) = (kws(&["pm.cat.team"]), kws(&[]));
        assert_eq!(toggle([a.as_slice(), b.as_slice()], "pm.cat.team"), (kws(&["pm.cat.team"]), vec![]));
        assert_eq!(toggle([a.as_slice()], "pm.cat.team"), (vec![], kws(&["pm.cat.team"])));
        assert_eq!(toggle([b.as_slice()], "$Label1"), (kws(&["$label1"]), vec![]));
    }

    #[test]
    fn book_falls_back_to_own_table() {
        let mut book = Book::own(defaults());
        assert_eq!(book.for_account(Some("i")).map(|t| t.list.len()), Some(5));
        book.accounts.insert("i".into(), Categories::default());
        assert_eq!(book.for_account(Some("i")).map(|t| t.list.len()), Some(0));
        assert_eq!(book.for_account(None).map(|t| t.list.len()), Some(5));
        assert_eq!(Book::default().for_account(None), None);
    }

    #[test]
    fn local_file_round_trip() {
        let dir = tempfile::Builder::new().prefix("postman-labels-").tempdir().unwrap();
        let store = LocalFile::beside(&dir.path().join("account-x.redb"));
        assert!(store.0.ends_with("account-x.redb.labels.json"));
        assert!(store.load().unwrap().is_none());
        let book = Book::own(defaults());
        store.save(&book).unwrap();
        assert_eq!(store.load().unwrap(), Some(book));
        std::fs::write(&store.0, r#"{"version": 99, "accounts": {}}"#).unwrap();
        assert!(matches!(store.load(), Err(StoreError::Version(99))));
        std::fs::write(&store.0, "not json").unwrap();
        assert!(matches!(store.load(), Err(StoreError::Json(_))));
    }
}

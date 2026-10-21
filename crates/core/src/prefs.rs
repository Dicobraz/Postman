//! Настройки пользователя (S5-09, `docs/DESIGN.md` «Настройки»): то, что меняется во вкладке «Настройки» и
//! хранится на этом компьютере. Таблица категорий — не здесь (на сервере, `categories`), правила и автоответ — тоже
//! (`settings`).
//!
//! - [`Prefs`] — общие для пользователя: тема, плотность, язык, быстрые фильтры, сроки и жизненный цикл, чтение,
//!   свои клавиши. Файл [`FILE`] в каталоге данных (`%LOCALAPPDATA%\Postman`).
//! - [`AccountPrefs`] — у учётной записи: подписи и шаблоны. Файл рядом с её кэшем ([`AccountPrefs::beside`]).
//!   Хранение на сервере (FileNode, как таблица категорий) — [проверить].
//!
//! Без UI и без gpui: интерфейс читает файл до первого окна (микросекунды), пишет — в фоне ([`Writer`]).
//! Жизненный цикл писем (S5-04) берёт сроки отсюда: [`Lifecycle::wait_reply`], [`Lifecycle::archive_newsletters`],
//! [`Lifecycle::expire_codes`]; порядок и видимость быстрых фильтров — [`QuickFilters::shown`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Файл общих настроек в каталоге данных.
pub const FILE: &str = "settings.json";
/// Формат файлов: растёт при несовместимых изменениях; файл новее — не читается (настройки по умолчанию).
pub const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("settings file: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("settings file version {0} is newer than this Postman")]
    Version(u32),
}

// --- общие ---

/// Общие настройки пользователя.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub version: u32,
    pub general: General,
    pub quick: QuickFilters,
    pub lifecycle: Lifecycle,
    pub reading: Reading,
    /// Свои клавиши: id команды → сочетания в записи gpui (`ctrl-shift-r`); пустой список — без клавиши. Нет id —
    /// клавиши по умолчанию из реестра команд.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            version: VERSION,
            general: General::default(),
            quick: QuickFilters::default(),
            lifecycle: Lifecycle::default(),
            reading: Reading::default(),
            keys: BTreeMap::new(),
        }
    }
}

/// Вид: тема, плотность, язык. `None` — по умолчанию («как в ОС», 24 px, язык ОС).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Id темы pets-ui: `system`, `fluent-light`, `fluent-dark`, `high-contrast` или своей.
    pub theme: Option<String>,
    /// Высота строки, px: 20, 24, 28 или 32.
    pub density: Option<u16>,
    /// `ru`, `en`; `None` — язык Windows.
    pub language: Option<String>,
    /// «Сообщить о фишинге» (S5-08): письмо ещё и вложением на этот адрес (служба безопасности); `None` — только в
    /// «Нежелательные» с меткой.
    pub phishing_report: Option<String>,
}

/// Плотности, которые можно выбрать (высота строки, px).
pub const DENSITIES: [u16; 4] = [20, 24, 28, 32];
/// Языки интерфейса.
pub const LANGUAGES: [&str; 2] = ["ru", "en"];

impl Prefs {
    /// Прочитать файл `path`. Нет файла — по умолчанию; файл новее этой версии — ошибка (вызывающий берёт
    /// умолчание и не перезаписывает файл). Значения вне допустимых — к ближайшим допустимым ([`Prefs::normalized`]).
    pub fn load(path: &Path) -> Result<Prefs, Error> {
        let Some(text) = read(path)? else { return Ok(Prefs::default()) };
        let version = version_of(&text)?;
        if version > VERSION {
            return Err(Error::Version(version));
        }
        let p: Prefs = serde_json::from_str(&text)?;
        Ok(p.normalized())
    }

    /// Записать через временный файл: сбой посреди записи не портит прежние настройки.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        write(path, &serde_json::to_string_pretty(self)?)
    }

    /// Привести к допустимым значениям: неизвестные плотность и язык — по умолчанию, быстрые фильтры — все пять по
    /// одному разу, сроки — в пределах.
    pub fn normalized(mut self) -> Prefs {
        self.version = VERSION;
        if self.general.density.is_some_and(|d| !DENSITIES.contains(&d)) {
            self.general.density = None;
        }
        if self.general.language.as_deref().is_some_and(|l| !LANGUAGES.contains(&l)) {
            self.general.language = None;
        }
        self.quick = std::mem::take(&mut self.quick).normalized();
        self.lifecycle = self.lifecycle.clamped();
        self.keys.retain(|id, _| !id.is_empty());
        // Адрес для фишинга: пробелы по краям — прочь, пустой и без «@» — нет адреса.
        self.general.phishing_report = phishing_address(self.general.phishing_report.as_deref());
        self
    }
}

/// Адрес «Сообщить о фишинге» (S5-08) из введённого текста: обрезанный, с «@» и без пробелов внутри; иначе `None`.
pub fn phishing_address(text: Option<&str>) -> Option<String> {
    let t = text?.trim();
    let ok = t.contains('@') && !t.starts_with('@') && !t.ends_with('@') && !t.chars().any(char::is_whitespace);
    ok.then(|| t.to_owned())
}

// --- быстрые фильтры ---

/// Быстрый фильтр (закладки списка, DESIGN «Быстрые фильтры»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quick {
    /// Угрозы: подозрительные и фишинг.
    Threats,
    /// Срок: впереди или прошёл.
    Deadline,
    /// Ждём ответа.
    Waiting,
    /// Мне лично.
    Mine,
    /// Истекают: коды и временные письма.
    Expiring,
}

impl Quick {
    /// Порядок по умолчанию — по срочности (DESIGN): Угрозы → Срок → Ждём → Мне → Истекают.
    pub const ALL: [Quick; 5] = [Quick::Threats, Quick::Deadline, Quick::Waiting, Quick::Mine, Quick::Expiring];

    /// Id для ключей текстов (`settings-quick-<id>`).
    pub fn id(self) -> &'static str {
        match self {
            Quick::Threats => "threats",
            Quick::Deadline => "deadline",
            Quick::Waiting => "waiting",
            Quick::Mine => "mine",
            Quick::Expiring => "expiring",
        }
    }
}

/// Быстрый фильтр в списке настроек: показан ли закладкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickItem {
    pub filter: Quick,
    pub shown: bool,
}

/// Порядок и видимость быстрых фильтров.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickFilters {
    pub items: Vec<QuickItem>,
    /// Закладки скрыты целиком («×» у закладок, S5-03); вернуть — Вид → Быстрые фильтры. Выбор `items` остаётся.
    pub hidden: bool,
}

impl Default for QuickFilters {
    fn default() -> Self {
        Self { items: Quick::ALL.iter().map(|&filter| QuickItem { filter, shown: true }).collect(), hidden: false }
    }
}

impl QuickFilters {
    /// Показанные закладками, по порядку (S5-03 строит по ним закладки списка).
    pub fn shown(&self) -> impl Iterator<Item = Quick> + '_ {
        self.items.iter().filter(|i| i.shown).map(|i| i.filter)
    }

    /// Закладки в списке: показанные, если полоса не скрыта целиком.
    pub fn tabs(&self) -> impl Iterator<Item = Quick> + '_ {
        self.shown().filter(|_| !self.hidden)
    }

    /// Каждый фильтр по одному разу; пропавшие (файл старой версии) — в конец, показанными.
    pub fn normalized(mut self) -> Self {
        let mut seen = Vec::new();
        self.items.retain(|i| {
            let fresh = !seen.contains(&i.filter);
            seen.push(i.filter);
            fresh
        });
        for q in Quick::ALL {
            if !seen.contains(&q) {
                self.items.push(QuickItem { filter: q, shown: true });
            }
        }
        self
    }

    /// Показать или скрыть закладку.
    pub fn toggle(&mut self, q: Quick) {
        if let Some(i) = self.items.iter_mut().find(|i| i.filter == q) {
            i.shown = !i.shown;
        }
    }

    /// Сдвинуть на `delta` позиций (−1 — выше); за край — не сдвигается. `true` — сдвинулся.
    pub fn shift(&mut self, q: Quick, delta: isize) -> bool {
        let Some(at) = self.items.iter().position(|i| i.filter == q) else { return false };
        let to = at as isize + delta;
        if to < 0 || to >= self.items.len() as isize {
            return false;
        }
        let item = self.items.remove(at);
        self.items.insert(to as usize, item);
        true
    }
}

// --- сроки и жизненный цикл (S5-04) ---

/// Сроки жизненного цикла (DESIGN «Быстрые фильтры, сроки и жизненный цикл»). Работают, пока Postman запущен
/// (решение владельца №2, `docs/specs/S5.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lifecycle {
    /// «Ждём ответа»: нет ответа на отправленное — вернуть во «Входящие» через `wait_reply_days`.
    pub wait_reply: bool,
    pub wait_reply_days: u16,
    /// Рассылки — в архив через `archive_news_days`.
    pub archive_news: bool,
    pub archive_news_days: u16,
    /// Коды подтверждения — в «Удалённые» через `expire_codes_hours` («Исчезнет»).
    pub expire_codes: bool,
    pub expire_codes_hours: u16,
    /// Просроченный срок — наверх списка.
    pub overdue_first: bool,
    /// Куда уходят письма, когда наступает «Исчезнет» (свой срок и коды подтверждения).
    pub expire_to: ExpireTo,
}

/// Куда уходит письмо, у которого наступило «Исчезнет».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpireTo {
    Archive,
    /// В «Удалённые» (по умолчанию: коды подтверждения — в корзину, макет настроек).
    #[default]
    Trash,
}

/// Пределы сроков: дни ожидания и архива, часы кодов.
pub const DAYS: std::ops::RangeInclusive<u16> = 1..=90;
pub const HOURS: std::ops::RangeInclusive<u16> = 1..=168;

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            wait_reply: true,
            wait_reply_days: 3,
            archive_news: false,
            archive_news_days: 7,
            expire_codes: true,
            expire_codes_hours: 24,
            overdue_first: true,
            expire_to: ExpireTo::Trash,
        }
    }
}

const DAY: u64 = 24 * 60 * 60;

impl Lifecycle {
    /// Сроки — в пределах [`DAYS`] и [`HOURS`].
    pub fn clamped(mut self) -> Self {
        let days = |d: u16| d.clamp(*DAYS.start(), *DAYS.end());
        self.wait_reply_days = days(self.wait_reply_days);
        self.archive_news_days = days(self.archive_news_days);
        self.expire_codes_hours = self.expire_codes_hours.clamp(*HOURS.start(), *HOURS.end());
        self
    }

    /// Через сколько вернуть письмо без ответа; `None` — правило выключено.
    pub fn wait_reply(&self) -> Option<Duration> {
        self.wait_reply.then(|| Duration::from_secs(u64::from(self.wait_reply_days) * DAY))
    }

    /// Через сколько рассылку — в архив; `None` — выключено.
    pub fn archive_newsletters(&self) -> Option<Duration> {
        self.archive_news.then(|| Duration::from_secs(u64::from(self.archive_news_days) * DAY))
    }

    /// Через сколько код подтверждения — в «Удалённые»; `None` — выключено.
    pub fn expire_codes(&self) -> Option<Duration> {
        self.expire_codes.then(|| Duration::from_secs(u64::from(self.expire_codes_hours) * 60 * 60))
    }
}

// --- чтение ---

/// Вид беседы (DESIGN «Беседы»).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conversation {
    /// Лента без цитат в панели чтения (по умолчанию).
    #[default]
    Feed,
    /// Стопка в панели чтения: старые письма свёрнуты.
    Stack,
    /// В списке, как Outlook.
    Outlook,
}

impl Conversation {
    pub const ALL: [Conversation; 3] = [Conversation::Feed, Conversation::Stack, Conversation::Outlook];
}

/// Где панель чтения.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    #[default]
    Right,
    Bottom,
    /// Скрыта: письмо открывается вместо списка, Esc — назад.
    Hidden,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Right, Pane::Bottom, Pane::Hidden];
}

/// Чтение. Внешние картинки не грузятся никогда — это политика Postman, не настройка (`docs/HANDOFF.md` §3a).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reading {
    pub conversation: Conversation,
    /// Вид беседы для папки (id папки → вид), поверх общего.
    pub folders: BTreeMap<String, Conversation>,
    pub pane: Pane,
    /// Предупреждать, если текст ссылки — один адрес, а ведёт она на другой.
    pub warn_link_mismatch: bool,
}

impl Default for Reading {
    fn default() -> Self {
        Self { conversation: Conversation::Feed, folders: BTreeMap::new(), pane: Pane::Right, warn_link_mismatch: true }
    }
}

impl Reading {
    /// Вид беседы в папке: свой у папки или общий.
    pub fn conversation_in(&self, mailbox: Option<&str>) -> Conversation {
        mailbox.and_then(|m| self.folders.get(m)).copied().unwrap_or(self.conversation)
    }

    /// Свой вид для папки; `None` — как везде.
    pub fn set_folder(&mut self, mailbox: &str, view: Option<Conversation>) {
        match view {
            Some(v) => {
                self.folders.insert(mailbox.to_owned(), v);
            }
            None => {
                self.folders.remove(mailbox);
            }
        }
    }
}

// --- у учётной записи: подписи и шаблоны ---

/// Подпись. У адреса (Identity) может быть несколько.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Постоянный id (`s<время в мс>`), на него ссылаются умолчания.
    pub id: String,
    pub name: String,
    /// Адрес отправителя, к которому подпись относится; `None` — к любому.
    #[serde(default)]
    pub identity: Option<String>,
    pub text: String,
}

/// Шаблон письма: вставляется в новое письмо.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

/// Какую подпись ставить адресу: в новое письмо и в ответ или пересылку (id подписи; `None` — без подписи).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub new: Option<String>,
    pub reply: Option<String>,
}

/// Настройки учётной записи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountPrefs {
    pub version: u32,
    pub signatures: Vec<Signature>,
    pub templates: Vec<Template>,
    /// Адрес отправителя → подписи по умолчанию.
    pub defaults: BTreeMap<String, Defaults>,
}

impl Default for AccountPrefs {
    fn default() -> Self {
        Self { version: VERSION, signatures: Vec::new(), templates: Vec::new(), defaults: BTreeMap::new() }
    }
}

impl AccountPrefs {
    /// Файл настроек для кэша учётной записи `cache`: `<кэш>.settings.json`.
    pub fn beside(cache: &Path) -> PathBuf {
        let mut name = cache.file_name().unwrap_or_default().to_os_string();
        name.push(".settings.json");
        cache.with_file_name(name)
    }

    pub fn load(path: &Path) -> Result<AccountPrefs, Error> {
        let Some(text) = read(path)? else { return Ok(AccountPrefs::default()) };
        let version = version_of(&text)?;
        if version > VERSION {
            return Err(Error::Version(version));
        }
        Ok(serde_json::from_str::<AccountPrefs>(&text)?.normalized())
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        write(path, &serde_json::to_string_pretty(self)?)
    }

    /// Умолчания ссылаются только на существующие подписи этого адреса (или любого).
    pub fn normalized(mut self) -> Self {
        self.version = VERSION;
        let sigs = self.signatures.clone();
        for (identity, d) in self.defaults.iter_mut() {
            let fits = |id: &Option<String>| {
                id.as_ref().is_none_or(|id| {
                    sigs.iter().any(|s| &s.id == id && s.identity.as_ref().is_none_or(|i| i == identity))
                })
            };
            if !fits(&d.new) {
                d.new = None;
            }
            if !fits(&d.reply) {
                d.reply = None;
            }
        }
        self.defaults.retain(|_, d| d.new.is_some() || d.reply.is_some());
        self
    }

    /// Подписи, которые можно поставить адресу `identity`: его и общие.
    pub fn signatures_for<'a>(&'a self, identity: &'a str) -> impl Iterator<Item = &'a Signature> + 'a {
        self.signatures.iter().filter(move |s| s.identity.as_deref().is_none_or(|i| i == identity))
    }

    /// Подпись письма от `identity`: в ответ (`reply`) или в новое (S5-06 вставляет её в композер).
    pub fn signature(&self, identity: &str, reply: bool) -> Option<&Signature> {
        let d = self.defaults.get(identity)?;
        let id = if reply { d.reply.as_ref() } else { d.new.as_ref() }?;
        self.signatures.iter().find(|s| &s.id == id)
    }

    /// Новый id для подписи или шаблона: `<буква><время в мс>`, свободный.
    pub fn new_id(&self, prefix: char, now_ms: i64) -> String {
        let taken = |id: &str| self.signatures.iter().any(|s| s.id == id) || self.templates.iter().any(|t| t.id == id);
        (now_ms.max(0)..).map(|n| format!("{prefix}{n}")).find(|id| !taken(id)).unwrap_or_default()
    }

    /// Удалить подпись и ссылки на неё в умолчаниях.
    pub fn remove_signature(&mut self, id: &str) {
        self.signatures.retain(|s| s.id != id);
        *self = std::mem::take(self).normalized();
    }
}

// --- файлы ---

fn read(path: &Path) -> Result<Option<String>, Error> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn version_of(text: &str) -> Result<u32, Error> {
    #[derive(Deserialize)]
    struct V {
        #[serde(default)]
        version: u32,
    }
    Ok(serde_json::from_str::<V>(text)?.version)
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Запись в фоне по порядку: каждая правка получает номер ([`Writer::next`]), запись с номером меньше уже
/// записанного пропускается — поздний поток не затрёт новые настройки старыми.
#[derive(Clone, Default)]
pub struct Writer {
    written: Arc<Mutex<u64>>,
    issued: Arc<Mutex<u64>>,
}

impl Writer {
    /// Номер следующей записи (звать в потоке интерфейса, по порядку правок).
    pub fn next(&self) -> u64 {
        let mut n = self.issued.lock().unwrap_or_else(PoisonError::into_inner);
        *n += 1;
        *n
    }

    /// Записать (в фоновом потоке): `save` выполняется, только если номер `seq` новее записанного. `Ok(false)` —
    /// пропущено.
    pub fn write(&self, seq: u64, save: impl FnOnce() -> Result<(), Error>) -> Result<bool, Error> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if seq <= *written {
            return Ok(false);
        }
        save()?;
        *written = seq;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("postman-").tempdir().unwrap()
    }

    /// Настройки туда и обратно: всё, что поменяли, читается тем же.
    #[test]
    fn prefs_round_trip() {
        let d = dir();
        let path = d.path().join(FILE);
        assert_eq!(Prefs::load(&path).unwrap(), Prefs::default(), "нет файла — по умолчанию");
        let mut p = Prefs {
            general: General {
                theme: Some("fluent-dark".into()),
                density: Some(20),
                language: Some("en".into()),
                phishing_report: Some("soc@corp.example".into()),
            },
            ..Prefs::default()
        };
        p.quick.toggle(Quick::Expiring);
        assert!(p.quick.shift(Quick::Mine, -3));
        p.lifecycle.archive_news = true;
        p.lifecycle.archive_news_days = 14;
        p.reading.pane = Pane::Bottom;
        p.reading.set_folder("news", Some(Conversation::Outlook));
        p.reading.warn_link_mismatch = false;
        p.keys.insert("mail.reply".into(), vec!["ctrl-shift-y".into()]);
        p.keys.insert("tab.close".into(), vec![]);
        p.save(&path).unwrap();
        let back = Prefs::load(&path).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.quick.shown().collect::<Vec<_>>(), [Quick::Mine, Quick::Threats, Quick::Deadline, Quick::Waiting]);
        assert_eq!(back.reading.conversation_in(Some("news")), Conversation::Outlook);
        assert_eq!(back.reading.conversation_in(Some("inbox")), Conversation::Feed);
        assert_eq!(back.lifecycle.archive_newsletters(), Some(Duration::from_secs(14 * DAY)));
        // Временного файла не остаётся.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    /// Адрес «Сообщить о фишинге» (S5-08): обрезается; не адрес — нет пересылки.
    #[test]
    fn phishing_address_is_checked() {
        assert_eq!(phishing_address(Some("  soc@corp.example ")), Some("soc@corp.example".into()));
        for bad in ["", "   ", "soc", "@corp", "soc@", "so c@corp.example"] {
            assert_eq!(phishing_address(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(phishing_address(None), None);
        let p = Prefs { general: General { phishing_report: Some("nope".into()), ..General::default() }, ..Prefs::default() };
        assert_eq!(p.normalized().general.phishing_report, None);
    }

    /// Чужие и испорченные значения — к допустимым; файл новее версии — ошибка, а не тихое затирание.
    #[test]
    fn bad_values_are_normalized_and_newer_file_is_refused() {
        let d = dir();
        let path = d.path().join(FILE);
        let text = r#"{"version":1,"general":{"density":23,"language":"de"},
            "quick":{"items":[{"filter":"mine","shown":false},{"filter":"mine","shown":true}]},
            "lifecycle":{"wait_reply_days":0,"expire_codes_hours":1000},"future_field":1}"#;
        std::fs::write(&path, text).unwrap();
        let p = Prefs::load(&path).unwrap();
        assert_eq!((p.general.density, p.general.language.as_deref()), (None, None));
        assert_eq!(p.quick.items.len(), 5);
        assert_eq!(p.quick.items[0], QuickItem { filter: Quick::Mine, shown: false });
        assert_eq!((p.lifecycle.wait_reply_days, p.lifecycle.expire_codes_hours), (1, 168));
        std::fs::write(&path, r#"{"version":99}"#).unwrap();
        assert!(matches!(Prefs::load(&path), Err(Error::Version(99))));
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Prefs::load(&path).is_err());
    }

    #[test]
    fn quick_filters_shift_within_bounds() {
        let mut q = QuickFilters::default();
        assert!(!q.shift(Quick::Threats, -1), "первый выше не сдвигается");
        assert!(!q.shift(Quick::Expiring, 1));
        assert!(q.shift(Quick::Threats, 1));
        assert_eq!(q.items[1].filter, Quick::Threats);
        q.toggle(Quick::Threats);
        assert!(!q.shown().any(|f| f == Quick::Threats));
    }

    #[test]
    fn lifecycle_api_for_s5_04() {
        let l = Lifecycle::default();
        assert_eq!(l.wait_reply(), Some(Duration::from_secs(3 * DAY)));
        assert_eq!(l.archive_newsletters(), None, "рассылки в архив — выключено по умолчанию");
        assert_eq!(l.expire_codes(), Some(Duration::from_secs(24 * 60 * 60)));
    }

    /// Подписи и шаблоны учётной записи — в файле рядом с её кэшем; умолчания без подписи отпадают.
    #[test]
    fn account_prefs_round_trip_and_defaults() {
        let d = dir();
        let path = AccountPrefs::beside(&d.path().join("account-u-1234.redb"));
        assert!(path.to_string_lossy().ends_with("account-u-1234.redb.settings.json"));
        let mut a = AccountPrefs::default();
        let main = a.new_id('s', 10);
        a.signatures.push(Signature { id: main.clone(), name: "Main".into(), identity: None, text: "--\nMe".into() });
        let short = a.new_id('s', 10);
        assert_ne!(main, short);
        a.signatures.push(Signature {
            id: short.clone(),
            name: "Short".into(),
            identity: Some("me@corp.example".into()),
            text: "Me".into(),
        });
        a.templates.push(Template { id: a.new_id('t', 10), name: "Offer".into(), subject: "S".into(), body: "B".into() });
        a.defaults.insert("me@corp.example".into(), Defaults { new: Some(main.clone()), reply: Some(short.clone()) });
        // Подпись другого адреса умолчанием не станет.
        a.defaults.insert("other@corp.example".into(), Defaults { new: Some(short.clone()), reply: None });
        a = a.normalized();
        assert!(!a.defaults.contains_key("other@corp.example"));
        a.save(&path).unwrap();
        let back = AccountPrefs::load(&path).unwrap();
        assert_eq!(back, a);
        assert_eq!(back.signature("me@corp.example", true).map(|s| s.name.as_str()), Some("Short"));
        assert_eq!(back.signatures_for("other@corp.example").count(), 1);
        let mut a = back;
        a.remove_signature(&short);
        assert_eq!(a.signature("me@corp.example", true), None);
        assert_eq!(a.signature("me@corp.example", false).map(|s| s.name.as_str()), Some("Main"));
    }

    /// Поздняя запись не затирает более новую.
    #[test]
    fn writer_skips_stale_writes() {
        let w = Writer::default();
        let (a, b) = (w.next(), w.next());
        let mut out = Vec::new();
        let mut push = |n: u64| {
            out.push(n);
            Ok(())
        };
        assert!(w.write(b, || push(b)).unwrap());
        assert!(!w.write(a, || push(a)).unwrap());
        assert_eq!(out, [b]);
    }
}

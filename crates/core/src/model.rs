//! Нейтральная модель (ADR 0003, S7-02): типы, которые видят `core` и интерфейс, — без привязки к протоколу
//! сервера. Пути к серверам переводят в них свои типы (`backend`); интерфейс протокольных крейтов не знает
//! (тест `layers.rs`). Поля — те же, что были у типов JMAP, поведение не меняется.

use serde::{Deserialize, Serialize};

/// Вход на сервер.
#[derive(Clone)]
pub enum Auth {
    Basic { user: String, password: String },
    Bearer(String),
}

/// Условие отбора писем на сервере — для фильтров панели и «Все папки».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    InMailbox(String),
    HasKeyword(String),
    NotKeyword(String),
    Text(String),
    /// Текст в From, To, теле (поиск подстроки в заголовке или тексте).
    From(String),
    To(String),
    Body(String),
    /// Текст в теме (S5-10).
    Subject(String),
    /// Есть вложение.
    HasAttachment(bool),
    /// Все условия.
    And(Vec<Filter>),
    /// Любое из условий: закладки быстрых фильтров между собой.
    Or(Vec<Filter>),
    /// Ни одно из условий: «внешние» — отправитель не из своих доменов.
    Not(Vec<Filter>),
    /// Получено раньше (UTCDate `2026-10-21T00:00:00Z`).
    Before(String),
    /// Получено не раньше (UTCDate; S5-10).
    After(String),
    /// Размер не меньше, байт («Очистить вложения…», S5-29).
    MinSize(u64),
}

/// Когда сервер выпускает письмо.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Сразу.
    Now,
    /// Через столько секунд после создания отправки: окно «Отменить» (S5-06).
    After(u32),
    /// В момент (Unix-секунды): «Отправить позже».
    At(i64),
}

/// Карточка контакта. У новой (ещё не на сервере) `id` пустой.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContactCard {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organization: String,
    /// Должность.
    pub title: String,
}

/// Запись справочника: человек, группа, ресурс.
#[derive(Debug, Clone)]
pub struct Principal {
    pub id: String,
    /// У Stalwart — логин (адрес), а не ФИО (`docs/STALWART.md` п. 9); показывать — [`Principal::display_name`].
    pub name: String,
    /// У Stalwart — ФИО учётной записи («Test user1»).
    pub description: Option<String>,
    pub email: Option<String>,
    /// individual, group, resource, location…
    pub kind: String,
}

impl Principal {
    /// Имя для людей: `description` (ФИО), а если его нет — `name`.
    pub fn display_name(&self) -> &str {
        match self.description.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => d,
            _ => &self.name,
        }
    }
}

/// Автоответ сервера. Даты — UTCDate (`2026-10-20T00:00:00Z`); `None` — без границы.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VacationResponse {
    #[serde(default)]
    pub is_enabled: bool,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

/// Область или функция, которой у сервера учётной записи может не быть (S7, «Нет у сервера — нет на экране»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Календарь, встречи, приглашения с ответом через сервер.
    Calendar,
    /// Задачи и поручения (S6).
    Tasks,
    /// Личные контакты.
    Contacts,
    /// Справочник организации и подсказки из него.
    Directory,
    /// Занятость коллег («Помощник планирования»).
    FreeBusy,
    /// Правила почты на сервере.
    Rules,
    /// Автоответ на сервере.
    Autoreply,
    /// Квота ящика.
    Quota,
}

impl Feature {
    pub const ALL: [Feature; 8] = [
        Feature::Calendar,
        Feature::Tasks,
        Feature::Contacts,
        Feature::Directory,
        Feature::FreeBusy,
        Feature::Rules,
        Feature::Autoreply,
        Feature::Quota,
    ];

    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// Что умеет сервер учётной записи (ADR 0003). Пока сервер не ответил — [`Features::ALL`]: интерфейс не прячет
/// разделы на время входа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features(u16);

impl Features {
    pub const ALL: Features = {
        let mut bits = 0;
        let mut i = 0;
        while i < Feature::ALL.len() {
            bits |= Feature::ALL[i].bit();
            i += 1;
        }
        Features(bits)
    };
    pub const NONE: Features = Features(0);

    pub fn has(self, f: Feature) -> bool {
        self.0 & f.bit() != 0
    }

    #[must_use]
    pub fn with(self, f: Feature, on: bool) -> Features {
        if on { Features(self.0 | f.bit()) } else { Features(self.0 & !f.bit()) }
    }
}

impl Default for Features {
    fn default() -> Self {
        Features::ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_set_and_clear() {
        assert!(Feature::ALL.iter().all(|&f| Features::ALL.has(f) && !Features::NONE.has(f)));
        let f = Features::ALL.with(Feature::Calendar, false).with(Feature::Rules, false);
        assert!(!f.has(Feature::Calendar) && !f.has(Feature::Rules) && f.has(Feature::Tasks));
        assert_eq!(f.with(Feature::Calendar, true).with(Feature::Rules, true), Features::ALL);
        assert_eq!(Features::default(), Features::ALL);
    }
}

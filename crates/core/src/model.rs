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

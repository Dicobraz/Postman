//! Настройки ящика на сервере: автоответ «Нет на месте» и заполненность ящика (S4-05), правила почты (S4-07).
//!
//! Синхронизация передаёт сюда [`Request`] (`sync::Command::Settings`) и отдаёт ответы событиями
//! `sync::Event::Settings` ([`Reply`]). Здесь — договор и чистое (состояние правил, заполненность); работа с
//! сервером — у плагина пути (`postman-backend-jmap`: Sieve, `VacationResponse`, квоты; ADR 0004); скрипт правил —
//! `rules.rs`, автоответ Postman — `autoreply.rs`.
//!
//! Stalwart держит активным один скрипт Sieve, а автоответ — это его скрипт `vacation` (`docs/STALWART.md`).
//! Поэтому при включённых правилах активен наш скрипт, а автоответ подключён к нему (`include`); `isEnabled`
//! сервера тогда `false`, и клиент считает автоответ включённым по нашему скрипту ([`effective`]).

use crate::autoreply::{Audience, AutoReply};
use crate::model::VacationResponse;
use crate::rules::Rule;

/// Запись автоответа Postman (S5-29).
#[derive(Debug, Clone)]
pub struct AutoReplySave {
    /// Включение и период; тема и текст — из `reply` ([`AutoReply::primary`]).
    pub vacation: VacationResponse,
    pub reply: AutoReply,
    pub audience: Audience,
    /// Название события «Вне офиса» на языке интерфейса.
    pub ooo_title: String,
    /// Пояс дней «Вне офиса» (IANA); пусто — пояс Windows.
    pub zone: String,
}

pub enum Request {
    /// Автоответ, правила и квота — при входе.
    Load,
    /// Записать автоответ → `VacationSaved` (что сервер сохранил) или `Failed`.
    SetVacation(VacationResponse),
    /// Автоответ Postman: `VacationResponse`, свой скрипт и «Вне офиса» → `VacationSaved`, `AutoReply` или `Failed`.
    SetAutoReply(Box<AutoReplySave>),
    /// Квота заново (после изменений в ящике).
    Quota,
    /// Правила с сервера → `Rules`.
    Rules,
    /// Записать правила целиком → `RulesSaved`, `RulesInvalid` (проверка сервера), `RulesConflict` (нужно
    /// «Заменить»: чужой скрипт) или `Failed`. `replace` — пользователь подтвердил замену.
    SaveRules { rules: Vec<Rule>, replace: bool },
}

pub enum Reply {
    /// Автоответ на сервере (с учётом правил: [`effective`]).
    Vacation(VacationResponse),
    /// Автоответ записан; внутри — прочитанный после записи.
    VacationSaved(VacationResponse),
    /// Настройки автоответа Postman из скрипта `vacation`; `None` — скрипт не наш (один текст сервера всем).
    AutoReply(Option<AutoReply>),
    /// Заполненность ящика; `None` — сервер её не ограничивает.
    Quota(Option<Usage>),
    Rules(RulesState),
    RulesSaved(RulesState),
    /// Сервер не принял скрипт: его сообщение (строка, столбец). На сервере ничего не изменилось.
    RulesInvalid(String),
    /// Есть чужой скрипт: `overwrite` — скрипт с нашим именем изменён вне Postman (его текст заменится), иначе —
    /// активен другой скрипт (он останется на сервере выключенным). Ничего не изменилось.
    RulesConflict {
        name: String,
        overwrite: bool,
    },
    /// Сервер этого не объявил (capability).
    Unsupported(Area),
    /// Запрос не удался (сообщение сервера или сети).
    Failed {
        area: Area,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Area {
    Vacation,
    Quota,
    Rules,
}

/// Скрипт сервера, написанный не в Postman: только показать.
#[derive(Debug, Clone, PartialEq)]
pub struct Foreign {
    pub name: String,
    pub text: String,
    pub active: bool,
}

/// Правила для экрана.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RulesState {
    pub rules: Vec<Rule>,
    /// Наш скрипт активен (правила работают).
    pub active: bool,
    pub foreign: Vec<Foreign>,
}

impl RulesState {
    /// Активный чужой скрипт: включение автоответа или правил его выключит.
    pub fn active_foreign(&self) -> Option<&str> {
        self.foreign.iter().find(|f| f.active).map(|f| f.name.as_str())
    }
}

/// Автоответ так, как его видит пользователь: включён и тогда, когда подключён к нашим правилам.
pub fn effective(v: VacationResponse, in_rules: bool) -> VacationResponse {
    VacationResponse { is_enabled: v.is_enabled || in_rules, ..v }
}

/// Занято и всего, байт.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub limit: u64,
}

impl Usage {
    /// Доля занятого, целые проценты (может быть больше 100).
    pub fn percent(&self) -> u64 {
        (self.used.saturating_mul(100)).checked_div(self.limit).unwrap_or(0)
    }
}

/// Предупреждать о заполненности с этой доли.
pub const WARN_PERCENT: u64 = 90;

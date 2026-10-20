//! Настройки ящика на сервере (S4-05): автоответ «Нет на месте» и заполненность ящика.
//!
//! Синхронизация передаёт сюда [`Request`] (`sync::Command::Settings`) и отдаёт ответы событиями
//! `sync::Event::Settings` ([`Reply`]). Чистое (выбор квоты) — с тестами здесь же.

use postman_jmap::{Client, QUOTA, Quota, VACATION_RESPONSE, VacationResponse};

pub enum Request {
    /// Автоответ и квота — при входе.
    Load,
    /// Записать автоответ → `VacationSaved` (что сервер сохранил) или `Failed`.
    SetVacation(VacationResponse),
    /// Квота заново (после изменений в ящике).
    Quota,
}

pub enum Reply {
    /// Автоответ на сервере.
    Vacation(VacationResponse),
    /// Автоответ записан; внутри — прочитанный после записи.
    VacationSaved(VacationResponse),
    /// Заполненность ящика; `None` — сервер её не ограничивает.
    Quota(Option<Usage>),
    /// Сервер этого не объявил (capability).
    Unsupported(Area),
    /// Запрос не удался (сообщение сервера или сети).
    Failed { area: Area, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Area {
    Vacation,
    Quota,
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

/// Квота почты в байтах: из квот с пределом, которые касаются писем, — самая заполненная.
pub fn mail_usage(quotas: &[Quota]) -> Option<Usage> {
    quotas
        .iter()
        .filter(|q| q.resource_type == "octets" && q.hard_limit > 0)
        .filter(|q| q.types.is_empty() || q.types.iter().any(|t| t == "Mail"))
        .map(|q| Usage { used: q.used, limit: q.hard_limit })
        .max_by(|a, b| (u128::from(a.used) * u128::from(b.limit)).cmp(&(u128::from(b.used) * u128::from(a.limit))))
}

pub async fn handle(client: &Client, request: Request, notify: &impl Fn(Reply)) {
    match request {
        Request::Load => {
            notify(if !client.has(VACATION_RESPONSE) {
                Reply::Unsupported(Area::Vacation)
            } else {
                match client.vacation().await {
                    Ok(v) => Reply::Vacation(v),
                    Err(e) => Reply::Failed { area: Area::Vacation, error: e.to_string() },
                }
            });
            quota(client, notify).await;
        }
        Request::SetVacation(v) => {
            let saved = match client.set_vacation(&v).await {
                Ok(()) => client.vacation().await,
                Err(e) => Err(e),
            };
            notify(match saved {
                Ok(v) => Reply::VacationSaved(v),
                Err(e) => Reply::Failed { area: Area::Vacation, error: e.to_string() },
            });
        }
        Request::Quota => quota(client, notify).await,
    }
}

async fn quota(client: &Client, notify: &impl Fn(Reply)) {
    if !client.has(QUOTA) {
        return notify(Reply::Unsupported(Area::Quota));
    }
    notify(match client.quotas().await {
        Ok(q) => Reply::Quota(mail_usage(&q)),
        Err(e) => Reply::Failed { area: Area::Quota, error: e.to_string() },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(resource: &str, used: u64, limit: u64, types: &[&str]) -> Quota {
        Quota {
            id: format!("{resource}{used}"),
            resource_type: resource.into(),
            used,
            hard_limit: limit,
            warn_limit: None,
            scope: "account".into(),
            name: String::new(),
            types: types.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn the_fullest_mail_quota_in_bytes() {
        assert_eq!(mail_usage(&[]), None);
        // Число писем и квота без предела — не то.
        assert_eq!(mail_usage(&[q("count", 10, 100, &["Mail"]), q("octets", 5, 0, &[])]), None);
        // Квота календаря — не про почту; без типов — про всё.
        assert_eq!(
            mail_usage(&[q("octets", 9, 10, &["Calendar"]), q("octets", 1, 10, &[])]),
            Some(Usage { used: 1, limit: 10 })
        );
        let u =
            mail_usage(&[q("octets", 300, 1000, &["Mail"]), q("octets", 950, 1000, &["Mail", "Calendar"])]).unwrap();
        assert_eq!(u, Usage { used: 950, limit: 1000 });
        assert_eq!(u.percent(), 95);
        assert_eq!(Usage { used: 3, limit: 0 }.percent(), 0);
        assert_eq!(Usage { used: u64::MAX, limit: 1 }.percent(), u64::MAX);
    }
}

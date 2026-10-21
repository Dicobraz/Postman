//! Ключевые слова исходящего письма (S5-06б, `docs/specs/S5.md` «Новое письмо вкладкой»): метка
//! «Конфиденциально» и «Ждать ответа» со сроком. Ставит их композер на черновик (`Draft::keywords`); после отправки
//! они остаются у письма в «Отправленных» — по ним работает жизненный цикл (S5-04): «Ждём ответа» до срока.
//!
//! Формат — ключевые слова JMAP (`crate::labels::valid_keyword`), без `$`: это слова Postman.
//! - `pm.conf` — метка «Конфиденциально». Шифрования нет: S/MIME — S5-12 (`docs/SMIME.md`).
//! - `pm.wait.<ГГГГ-ММ-ДД>` — ждать ответа до этой даты (местной). У слова JMAP нет значения, поэтому дата —
//!   в самом слове; у письма одно такое слово (новое заменяет прежнее).

use jiff::ToSpan;
use jiff::civil::Date;

/// Метка «Конфиденциально».
pub const CONFIDENTIAL: &str = "pm.conf";
/// Префикс «Ждать ответа до даты».
pub const WAIT_PREFIX: &str = "pm.wait.";
/// Сроки «Ждать ответа · N дней» на выбор и срок по умолчанию.
pub const WAIT_DAYS: [u8; 5] = [1, 2, 3, 5, 7];
pub const WAIT_DEFAULT: u8 = 3;

/// `pm.wait.2026-10-24`.
pub fn wait_keyword(due: Date) -> String {
    format!("{WAIT_PREFIX}{}", due.strftime("%Y-%m-%d"))
}

/// Срок из слова «Ждать ответа»; чужое слово или неверная дата — `None`.
pub fn parse_wait(keyword: &str) -> Option<Date> {
    let prefix = keyword.get(..WAIT_PREFIX.len()).filter(|p| p.eq_ignore_ascii_case(WAIT_PREFIX))?;
    let rest = &keyword[prefix.len()..];
    if rest.len() != 10 {
        return None;
    }
    rest.parse::<Date>().ok()
}

/// Срок через `days` дней от `today`.
pub fn wait_due(today: Date, days: u8) -> Option<Date> {
    today.checked_add(i64::from(days).days()).ok()
}

/// Слово управляется композером (метка, ожидание ответа): при сохранении черновика ставится заново.
pub fn is_compose_mark(keyword: &str) -> bool {
    keyword.eq_ignore_ascii_case(CONFIDENTIAL) || parse_wait(keyword).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::valid_keyword;

    #[test]
    fn wait_keyword_round_trips_and_is_a_valid_jmap_keyword() {
        let today: Date = "2026-10-21".parse().unwrap();
        let due = wait_due(today, 3).unwrap();
        let k = wait_keyword(due);
        assert_eq!(k, "pm.wait.2026-10-24");
        assert!(valid_keyword(&k) && valid_keyword(CONFIDENTIAL));
        assert_eq!(parse_wait(&k), Some(due));
        assert_eq!(parse_wait("PM.WAIT.2026-10-24"), Some(due), "регистр IMAP-клиентов");
        for bad in ["pm.wait.", "pm.wait.2026-13-01", "pm.wait.2026-10-24x", "pm.cat.team", "pm.wait"] {
            assert_eq!(parse_wait(bad), None, "{bad}");
        }
        // Конец месяца и года.
        assert_eq!(wait_due("2026-12-30".parse().unwrap(), 5).map(wait_keyword).as_deref(), Some("pm.wait.2027-01-04"));
        assert!(is_compose_mark("pm.conf") && is_compose_mark(&k) && !is_compose_mark("pm.cat.team"));
    }
}

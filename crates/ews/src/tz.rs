//! Пояса IANA ↔ Windows id (`StartTimeZone Id="Russian Standard Time"`): по поясу встречи Exchange разворачивает
//! повторы и переводы часов. Таблица — CLDR `windowsZones` целиком ([`zones`], вшита; в работе ничего не
//! запрашивается): все пояса базы IANA, включая прежние имена (`Asia/Calcutta` и `Asia/Kolkata`, `Europe/Kiev` и
//! `Europe/Kyiv`). Пояса нет в таблице — встреча уходит в UTC без пояса: одиночная — верно, серия через перевод
//! часов — со сдвигом.

mod zones;

use zones::{IANA_TO_WINDOWS, WINDOWS_TO_IANA};

fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| table[i].1)
}

/// Windows id пояса IANA.
pub fn windows_zone(iana: &str) -> Option<&'static str> {
    lookup(IANA_TO_WINDOWS, iana.trim())
}

/// Основной пояс IANA по Windows id (территория `001` CLDR).
pub fn iana_zone(windows: &str) -> Option<&'static str> {
    lookup(WINDOWS_TO_IANA, windows.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ways() {
        assert_eq!(windows_zone("Europe/Moscow"), Some("Russian Standard Time"));
        assert_eq!(windows_zone("Asia/Almaty"), Some("Central Asia Standard Time"));
        assert_eq!(windows_zone("Asia/Kolkata"), Some("India Standard Time"));
        assert_eq!(windows_zone("Asia/Calcutta"), Some("India Standard Time"));
        assert_eq!(windows_zone("Europe/Kyiv"), windows_zone("Europe/Kiev"));
        assert_eq!(windows_zone("UTC"), Some("UTC"));
        assert_eq!(iana_zone("Russian Standard Time"), Some("Europe/Moscow"));
        assert_eq!(iana_zone("North Asia Standard Time"), Some("Asia/Krasnoyarsk"));
        assert_eq!(iana_zone("UTC"), Some("Etc/UTC"));
        assert_eq!(windows_zone("Mars/Olympus"), None);
        assert_eq!(iana_zone("Mars Standard Time"), None);
    }

    /// Таблицы упорядочены (двоичный поиск), без повторов; каждый Windows id — в обе стороны.
    #[test]
    fn tables_are_sorted_and_closed() {
        for t in [IANA_TO_WINDOWS, WINDOWS_TO_IANA] {
            assert!(t.windows(2).all(|w| w[0].0 < w[1].0), "по ключу, без повторов");
        }
        assert!(IANA_TO_WINDOWS.len() > 400 && WINDOWS_TO_IANA.len() > 130, "таблица CLDR целиком");
        for (iana, win) in IANA_TO_WINDOWS {
            assert!(iana_zone(win).is_some(), "{iana}: у {win} нет основного пояса");
        }
        for (win, iana) in WINDOWS_TO_IANA {
            assert_eq!(windows_zone(iana), Some(*win), "{win} → {iana} → назад");
        }
    }

    /// Основные пояса знает и jiff (база IANA Windows или встроенная): по ним считается время встречи.
    #[test]
    fn jiff_knows_the_main_zones() {
        for (_, iana) in WINDOWS_TO_IANA {
            assert!(jiff::tz::TimeZone::get(iana).is_ok(), "{iana}");
        }
    }
}

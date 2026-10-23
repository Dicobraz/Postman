//! Пояса IANA ↔ Windows id (`StartTimeZone Id="Russian Standard Time"`): по поясу встречи Exchange разворачивает
//! повторы и переводы часов. Здесь — пояса России и соседей и самые частые зарубежные (по CLDR `windowsZones`);
//! полная таблица — с календарём Exchange в модели (S7-07). Пояса нет в таблице — встреча уходит в UTC без пояса:
//! одиночная — верно, серия через перевод часов — со сдвигом.

/// (IANA, Windows) — по CLDR `windowsZones.xml`, территория по умолчанию (`001`) и Россия.
const ZONES: &[(&str, &str)] = &[
    ("UTC", "UTC"),
    ("Etc/UTC", "UTC"),
    ("Europe/Kaliningrad", "Kaliningrad Standard Time"),
    ("Europe/Moscow", "Russian Standard Time"),
    ("Europe/Simferopol", "Russian Standard Time"),
    ("Europe/Kirov", "Russian Standard Time"),
    ("Europe/Volgograd", "Volgograd Standard Time"),
    ("Europe/Astrakhan", "Astrakhan Standard Time"),
    ("Europe/Ulyanovsk", "Astrakhan Standard Time"),
    ("Europe/Saratov", "Saratov Standard Time"),
    ("Europe/Samara", "Russia Time Zone 3"),
    ("Asia/Yekaterinburg", "Ekaterinburg Standard Time"),
    ("Asia/Omsk", "Omsk Standard Time"),
    ("Asia/Novosibirsk", "N. Central Asia Standard Time"),
    ("Asia/Barnaul", "Altai Standard Time"),
    ("Asia/Tomsk", "Tomsk Standard Time"),
    ("Asia/Krasnoyarsk", "North Asia Standard Time"),
    ("Asia/Novokuznetsk", "North Asia Standard Time"),
    ("Asia/Irkutsk", "North Asia East Standard Time"),
    ("Asia/Chita", "Transbaikal Standard Time"),
    ("Asia/Yakutsk", "Yakutsk Standard Time"),
    ("Asia/Vladivostok", "Vladivostok Standard Time"),
    ("Asia/Sakhalin", "Sakhalin Standard Time"),
    ("Asia/Magadan", "Magadan Standard Time"),
    ("Asia/Srednekolymsk", "Russia Time Zone 10"),
    ("Asia/Kamchatka", "Russia Time Zone 11"),
    ("Europe/Minsk", "Belarus Standard Time"),
    ("Asia/Tbilisi", "Georgian Standard Time"),
    ("Asia/Yerevan", "Caucasus Standard Time"),
    ("Asia/Baku", "Azerbaijan Standard Time"),
    ("Asia/Tashkent", "West Asia Standard Time"),
    ("Asia/Bishkek", "Central Asia Standard Time"),
    ("Europe/Istanbul", "Turkey Standard Time"),
    ("Europe/London", "GMT Standard Time"),
    ("Europe/Berlin", "W. Europe Standard Time"),
    ("Europe/Paris", "Romance Standard Time"),
    ("Europe/Warsaw", "Central European Standard Time"),
    ("Europe/Helsinki", "FLE Standard Time"),
    ("Asia/Dubai", "Arabian Standard Time"),
    ("Asia/Shanghai", "China Standard Time"),
    ("Asia/Tokyo", "Tokyo Standard Time"),
    ("America/New_York", "Eastern Standard Time"),
    ("America/Chicago", "Central Standard Time"),
    ("America/Los_Angeles", "Pacific Standard Time"),
];

/// Windows id пояса IANA.
pub fn windows_zone(iana: &str) -> Option<&'static str> {
    ZONES.iter().find(|(i, _)| *i == iana).map(|(_, w)| *w)
}

/// Пояс IANA по Windows id (первый в таблице — основной для этого id).
pub fn iana_zone(windows: &str) -> Option<&'static str> {
    ZONES.iter().find(|(_, w)| *w == windows).map(|(i, _)| *i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ways() {
        assert_eq!(windows_zone("Europe/Moscow"), Some("Russian Standard Time"));
        assert_eq!(iana_zone("Russian Standard Time"), Some("Europe/Moscow"));
        assert_eq!(iana_zone("North Asia Standard Time"), Some("Asia/Krasnoyarsk"));
        assert_eq!(windows_zone("Mars/Olympus"), None);
        // Каждый пояс таблицы знает и jiff (база IANA Windows или встроенная).
        for (iana, _) in ZONES {
            assert!(jiff::tz::TimeZone::get(iana).is_ok(), "{iana}");
        }
    }
}

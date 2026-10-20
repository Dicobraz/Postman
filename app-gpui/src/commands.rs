//! Команды Postman (S2-02): реестр `pets-commands` → действия gpui и привязки клавиш.
//! Действие команды `mail.reply_all` — `mail::ReplyAll`; обработчик вешает экран (`on_action`).
//! У каждой команды есть обработчик на экране (`docs/PARITY.md`); новая — сначала в таблицу, затем обработчик.

use std::rc::Rc;

use gpui_kit::*;
use pets_commands::Command;

pub mod mail {
    gpui_kit::actions!(
        mail,
        [Next, Prev, First, Last, PageDown, PageUp, MarkRead, MarkUnread, ToggleFlag, New, Reply, ReplyAll, Forward, Delete]
    );

    use gpui_kit::gpui;

    /// Категория на выбранном письме (S5-02): `key` — цифра клавиши, категория — та, у которой эта клавиша в
    /// таблице. Данные действия — из клавиши (`super::keyed_action`), не из JSON.
    #[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
    #[action(namespace = mail, no_json)]
    pub struct Categorize {
        pub key: u8,
    }
}

/// Беседа в области чтения (S5-05).
pub mod thread {
    gpui_kit::actions!(thread, [Toggle, Quotes, Next, Prev]);
}

pub mod view {
    gpui_kit::actions!(view, [Mail, Calendar, Contacts, Notifications]);
}

pub mod search {
    gpui_kit::actions!(search, [Focus, Clear]);
}

pub mod calendar {
    gpui_kit::actions!(calendar, [NewMeeting, PrevWeek, NextWeek, Today]);
}

/// Окно встречи (S3-05).
pub mod meeting {
    gpui_kit::actions!(meeting, [Send, Close]);
}

/// Настройки ящика на сервере (S4-05, S4-07): в Outlook — меню «Файл», клавиши нет.
pub mod settings {
    gpui_kit::actions!(settings, [Vacation, Rules]);
}

/// Окно письма (S3-01).
pub mod compose {
    gpui_kit::actions!(compose, [Send, Attach, Preview, Close]);
}

/// Раздел «Почта» вне полей ввода: клавиши без модификаторов в поиске — текст и курсор. Контекст раздела
/// (`MailView`, `CalendarView`, `ContactsView`, `SettingsView`) ставит корень экрана: в календаре, контактах и
/// настройках клавиши писем молчат.
const LIST: Option<&str> = Some("MailView && !Input");
/// Раздел «Почта», включая поле поиска.
const MAIL: Option<&str> = Some("MailView");
/// Раздел «Календарь».
const CALENDAR: Option<&str> = Some("CalendarView");
/// Весь экран почты (любой раздел), включая поле поиска.
const MAILER: Option<&str> = Some("Mailer");
/// Окно письма, в том числе поля ввода: `Composer > Input` глубже привязок поля (`ctrl-enter` в тексте —
/// отправка, а не перевод строки), а при равной глубине побеждает привязка, заданная позже (`bind` — после
/// `gpui_kit::init`).
const COMPOSER: Option<&str> = Some("Composer || (Composer > Input)");
/// Окно встречи, как окно письма.
const MEETING: Option<&str> = Some("Meeting || (Meeting > Input)");

const fn cmd(id: &'static str, keys: &'static [&'static str], context: Option<&'static str>) -> Command {
    Command { id, keys, context }
}

/// Все команды Postman. Хоткеи — как в classic Outlook (`docs/HANDOFF.md` §2a) плюс j/k.
pub const COMMANDS: &[Command] = &[
    cmd("mail.next", &["j", "down"], LIST),
    cmd("mail.prev", &["k", "up"], LIST),
    cmd("mail.first", &["home"], LIST),
    cmd("mail.last", &["end"], LIST),
    cmd("mail.page_down", &["pagedown"], LIST),
    cmd("mail.page_up", &["pageup"], LIST),
    cmd("mail.mark_read", &["ctrl-q"], MAIL),
    cmd("mail.mark_unread", &["ctrl-u"], MAIL),
    cmd("mail.toggle_flag", &["insert"], LIST),
    // Не Outlook (там Ctrl+F2…F12 по назначению): клавиши 1–9 — категории таблицы (DESIGN.md, «Настройки»).
    cmd("mail.categorize", &["1", "2", "3", "4", "5", "6", "7", "8", "9"], LIST),
    cmd("mail.new", &["ctrl-n"], MAILER),
    cmd("mail.reply", &["ctrl-r"], MAIL),
    cmd("mail.reply_all", &["ctrl-shift-r"], MAIL),
    cmd("mail.forward", &["ctrl-f"], MAIL),
    cmd("mail.delete", &["delete"], LIST),
    // Не Outlook (DESIGN, «Беседы»): лента беседы и цитаты — кнопками; по письмам беседы — Ctrl+→ / Ctrl+←
    // (в поле поиска эти клавиши — по словам).
    cmd("thread.toggle", &[], MAIL),
    cmd("thread.quotes", &[], MAIL),
    cmd("thread.next", &["ctrl-right"], LIST),
    cmd("thread.prev", &["ctrl-left"], LIST),
    cmd("view.mail", &["ctrl-1"], None),
    cmd("view.calendar", &["ctrl-2"], None),
    cmd("view.contacts", &["ctrl-3"], None),
    // Не Outlook: история тостов (значок 🔔 в строке состояния).
    cmd("view.notifications", &[], None),
    cmd("search.focus", &["ctrl-e"], None),
    cmd("search.clear", &["escape"], None),
    cmd("calendar.new_meeting", &["ctrl-shift-q"], None),
    // Outlook: предыдущая и следующая неделя — Alt+↑ и Alt+↓; «Сегодня» — кнопкой.
    cmd("calendar.prev_week", &["alt-up"], CALENDAR),
    cmd("calendar.next_week", &["alt-down"], CALENDAR),
    cmd("calendar.today", &[], CALENDAR),
    cmd("compose.send", &["ctrl-enter", "alt-s"], COMPOSER),
    // Не Outlook: вложить файл — кнопкой или перетаскиванием, без клавиши.
    cmd("compose.attach", &[], COMPOSER),
    cmd("compose.preview", &["ctrl-shift-p"], COMPOSER),
    cmd("compose.close", &["escape"], COMPOSER),
    cmd("meeting.send", &["ctrl-enter", "alt-s"], MEETING),
    cmd("meeting.close", &["escape"], MEETING),
    // Outlook: «Файл → Автоматические ответы», без клавиши.
    cmd("settings.vacation", &[], None),
    // Outlook: «Файл → Правила и оповещения», без клавиши.
    cmd("settings.rules", &[], None),
];

/// Привязать клавиши всех команд. Действие или сочетание, которое не собралось, — в журнал
/// (тесты ловят это раньше), остальные команды работают.
pub fn bind(cx: &mut App) {
    let mut bindings = Vec::new();
    for c in COMMANDS {
        let context = c.context.and_then(|s| KeyBindingContextPredicate::parse(s).ok()).map(Rc::new);
        for keys in c.keys {
            let action = match keyed_action(c, keys) {
                Some(a) => a,
                None => match cx.build_action(&c.action_name(), None) {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::error!("command {}: no action {} ({e})", c.id, c.action_name());
                        continue;
                    }
                },
            };
            match KeyBinding::load(keys, action, context.clone(), false, None, &DummyKeyboardMapper) {
                Ok(b) => bindings.push(b),
                Err(e) => tracing::error!("command {}: keys {keys}: {e:?}", c.id),
            }
        }
    }
    cx.bind_keys(bindings);
}

/// Действие с данными из клавиши: у `mail.categorize` клавиша — цифра категории. Остальные команды — `None`
/// (действие без данных по имени).
fn keyed_action(c: &Command, keys: &str) -> Option<Box<dyn Action>> {
    match c.id {
        "mail.categorize" => keys.parse::<u8>().ok().map(|key| Box::new(mail::Categorize { key }) as Box<dyn Action>),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::COMMANDS;
    use gpui_kit::generate_list_of_all_registered_actions;

    #[test]
    fn the_table_is_valid() {
        assert_eq!(pets_commands::validate(COMMANDS), vec![]);
    }

    #[test]
    fn every_command_has_a_gpui_action() {
        let registered: Vec<&str> = generate_list_of_all_registered_actions().map(|a| a.name).collect();
        for c in COMMANDS {
            assert!(registered.contains(&c.action_name().as_str()), "{}: нет действия {}", c.id, c.action_name());
        }
    }

    /// `i18n/{ru,en}/commands.ftl`: у каждой команды есть название, лишних ключей `cmd-…` нет.
    #[test]
    fn every_command_has_a_name_in_both_languages() {
        for (lang, ftl) in [("ru", super::parity::RU_FTL), ("en", include_str!("../i18n/en/commands.ftl"))] {
            let names = super::parity::ftl_names(ftl);
            for c in COMMANDS {
                assert!(names.iter().any(|(k, _)| *k == c.name_key()), "{lang}.ftl: нет {}", c.name_key());
            }
            for (k, _) in names.iter().filter(|(k, _)| k.starts_with("cmd-")) {
                assert!(COMMANDS.iter().any(|c| c.name_key() == *k), "{lang}.ftl: {k} — нет такой команды");
            }
        }
    }

    /// `docs/PARITY.md` — из реестра; доля сделанного не падает.
    /// Обновить: `POSTMAN_UPDATE_DOCS=1 cargo test -p postman-gpui parity`.
    #[test]
    fn parity_doc_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/PARITY.md");
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        // Обработчики `on_action` — в любом модуле экрана: читаем все исходники интерфейса.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = String::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "commands.rs") {
                sources += &std::fs::read_to_string(&path).unwrap();
            }
        }
        let (doc, done) = super::parity::document(&sources);
        if let Some(was) = super::parity::done_count(&old) {
            assert!(done >= was, "сделанных функций стало меньше: было {was}, стало {done}");
        }
        if std::env::var_os("POSTMAN_UPDATE_DOCS").is_some() {
            std::fs::write(&path, &doc).expect("запись docs/PARITY.md");
        } else {
            assert!(
                old.replace("\r\n", "\n") == doc,
                "docs/PARITY.md устарел: POSTMAN_UPDATE_DOCS=1 cargo test -p postman-gpui parity"
            );
        }
    }

    /// Клавиши писем действуют только в разделе «Почта»: в календаре и контактах Delete, j/k, Ctrl+R не трогают
    /// скрытый список писем. Создать письмо (Ctrl+N) — из любого раздела.
    #[test]
    fn mail_keys_are_scoped_to_the_mail_view() {
        for c in COMMANDS.iter().filter(|c| c.area() == "mail" && c.id != "mail.new" && !c.keys.is_empty()) {
            assert!(c.context.is_some_and(|ctx| ctx.contains("MailView")), "{}: клавиши вне раздела «Почта»", c.id);
        }
        for c in COMMANDS.iter().filter(|c| c.id.starts_with("calendar.") && c.id != "calendar.new_meeting") {
            assert!(c.context.is_some_and(|ctx| ctx.contains("CalendarView")), "{}: клавиши вне календаря", c.id);
        }
    }

    /// Клавиши 1–9 — категория с этой клавишей (S5-02): у каждой цифры своё действие с данными.
    #[test]
    fn digit_keys_carry_the_category_key() {
        let c = pets_commands::find(COMMANDS, "mail.categorize").unwrap();
        assert_eq!(c.keys.len(), 9);
        for (n, keys) in c.keys.iter().enumerate() {
            let a = super::keyed_action(c, keys).unwrap();
            let got = a.as_any().downcast_ref::<super::mail::Categorize>().copied();
            assert_eq!(got, Some(super::mail::Categorize { key: n as u8 + 1 }));
        }
        let next = pets_commands::find(COMMANDS, "mail.next").unwrap();
        assert!(super::keyed_action(next, "j").is_none());
    }

    /// Хоткеи classic Outlook из `docs/HANDOFF.md` §2a.
    #[test]
    fn outlook_hotkeys_are_bound() {
        for keys in ["ctrl-n", "ctrl-r", "ctrl-shift-r", "ctrl-f", "ctrl-e", "ctrl-1", "ctrl-2", "ctrl-3", "delete", "ctrl-q", "ctrl-u"] {
            assert!(!pets_commands::bound_to(COMMANDS, keys).is_empty(), "хоткей Outlook {keys} не привязан");
        }
    }
}

/// Чек-лист соответствия Outlook (S2-04): `docs/PARITY.md` из реестра команд.
#[cfg(test)]
pub(crate) mod parity {
    use super::COMMANDS;

    pub const RU_FTL: &str = include_str!("../i18n/ru/commands.ftl");

    /// Пары `ключ = значение` из `.ftl` (только простые сообщения в одну строку).
    pub fn ftl_names(ftl: &str) -> Vec<(&str, &str)> {
        ftl.lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with(' '))
            .filter_map(|l| l.split_once(" = "))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect()
    }

    /// Действия, у которых в коде экрана есть обработчик: `on_action(… _: &mail::Next …)` или с данными
    /// (`a: &mail::Categorize`). Прочие `x: &Тип` с именами действий не совпадают.
    fn handled(source: &str) -> Vec<String> {
        source
            .split(": &")
            .skip(1)
            .filter_map(|rest| rest.split([',', ')']).next())
            .map(|path| {
                let parts: Vec<&str> = path.trim().split("::").collect();
                parts[parts.len().saturating_sub(2)..].join("::")
            })
            .collect()
    }

    /// Число сделанных функций из прошлой версии документа (`Сделано: N из M`).
    pub fn done_count(doc: &str) -> Option<usize> {
        let rest = doc.split("Сделано: ").nth(1)?;
        rest.split_whitespace().next()?.parse().ok()
    }

    /// Текст документа и число сделанных функций.
    pub fn document(screen_source: &str) -> (String, usize) {
        let names = ftl_names(RU_FTL);
        let handled = handled(screen_source);
        let done = COMMANDS.iter().filter(|c| handled.contains(&c.action_name())).count();
        let mut s = String::new();
        s.push_str("# Соответствие classic Outlook\n\n");
        s.push_str("> Генерируется из реестра команд (`app-gpui/src/commands.rs`) тестом `parity_doc_is_current`.\n");
        s.push_str("> Руками не править. Обновить: `POSTMAN_UPDATE_DOCS=1 cargo test -p postman-gpui parity`.\n");
        s.push_str("> «есть» — у действия команды есть обработчик на экране; доля сделанного только растёт (тест).\n\n");
        s.push_str(&format!(
            "Сделано: {done} из {} ({} %).\n\n",
            COMMANDS.len(),
            done * 100 / COMMANDS.len().max(1)
        ));
        s.push_str("| Функция | Команда | Хоткей | Статус |\n|---|---|---|---|\n");
        for c in COMMANDS {
            let key = c.name_key();
            let name = names.iter().find(|(k, _)| *k == key).map_or(key.as_str(), |(_, v)| v);
            let keys: Vec<String> = c.keys.iter().map(|k| format!("`{k}`")).collect();
            let status = if handled.contains(&c.action_name()) { "есть" } else { "нет" };
            s.push_str(&format!("| {name} | `{}` | {} | {status} |\n", c.id, keys.join(", ")));
        }
        (s, done)
    }
}

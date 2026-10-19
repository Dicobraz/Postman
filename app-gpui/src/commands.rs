//! Команды Postman (S2-02): реестр `pets-commands` → действия gpui и привязки клавиш.
//! Действие команды `mail.reply_all` — `mail::ReplyAll`; обработчик вешает экран (`on_action`).
//! Команды без обработчика (ответ, пересылка, удаление, контакты) уже привязаны к клавишам Outlook —
//! экраны появятся в фазе 2.

use std::rc::Rc;

use gpui_kit::*;
use pets_commands::Command;

pub mod mail {
    gpui_kit::actions!(
        mail,
        [Next, Prev, First, Last, PageDown, PageUp, MarkRead, MarkUnread, ToggleFlag, New, Reply, ReplyAll, Forward, Delete]
    );
}

pub mod view {
    gpui_kit::actions!(view, [Mail, Calendar, Contacts]);
}

pub mod search {
    gpui_kit::actions!(search, [Focus, Clear]);
}

pub mod calendar {
    gpui_kit::actions!(calendar, [NewMeeting]);
}

/// Экран почты вне полей ввода: клавиши без модификаторов в поиске — текст и курсор.
const LIST: Option<&str> = Some("Mailer && !Input");
/// Весь экран почты, включая поле поиска.
const MAILER: Option<&str> = Some("Mailer");

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
    cmd("mail.mark_read", &["ctrl-q"], MAILER),
    cmd("mail.mark_unread", &["ctrl-u"], MAILER),
    cmd("mail.toggle_flag", &["insert"], LIST),
    cmd("mail.new", &["ctrl-n"], MAILER),
    cmd("mail.reply", &["ctrl-r"], MAILER),
    cmd("mail.reply_all", &["ctrl-shift-r"], MAILER),
    cmd("mail.forward", &["ctrl-f"], MAILER),
    cmd("mail.delete", &["delete"], LIST),
    cmd("view.mail", &["ctrl-1"], None),
    cmd("view.calendar", &["ctrl-2"], None),
    cmd("view.contacts", &["ctrl-3"], None),
    cmd("search.focus", &["ctrl-e"], None),
    cmd("search.clear", &["escape"], None),
    cmd("calendar.new_meeting", &["ctrl-shift-q"], None),
];

/// Привязать клавиши всех команд. Действие или сочетание, которое не собралось, — в журнал
/// (тесты ловят это раньше), остальные команды работают.
pub fn bind(cx: &mut App) {
    let mut bindings = Vec::new();
    for c in COMMANDS {
        let action = match cx.build_action(&c.action_name(), None) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("команда {}: нет действия {} ({e})", c.id, c.action_name());
                continue;
            }
        };
        let context = c.context.and_then(|s| KeyBindingContextPredicate::parse(s).ok()).map(Rc::new);
        for keys in c.keys {
            match KeyBinding::load(keys, action.boxed_clone(), context.clone(), false, None, &DummyKeyboardMapper) {
                Ok(b) => bindings.push(b),
                Err(e) => eprintln!("команда {}: сочетание {keys}: {e:?}", c.id),
            }
        }
    }
    cx.bind_keys(bindings);
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

    /// Хоткеи classic Outlook из `docs/HANDOFF.md` §2a.
    #[test]
    fn outlook_hotkeys_are_bound() {
        for keys in ["ctrl-n", "ctrl-r", "ctrl-shift-r", "ctrl-f", "ctrl-e", "ctrl-1", "ctrl-2", "ctrl-3", "delete", "ctrl-q", "ctrl-u"] {
            assert!(!pets_commands::bound_to(COMMANDS, keys).is_empty(), "хоткей Outlook {keys} не привязан");
        }
    }
}

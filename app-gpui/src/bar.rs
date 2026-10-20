//! Строка команд: кнопки действий и поле поиска. Кнопка — та же команда реестра, что и хоткей (S3-02).

use gpui_kit::*;
use gpui_kit::component::input::{Input, InputState};
use pets_i18n::tr;

use crate::commands::{mail, settings};

// --- строка команд ---

/// Действие кнопки (новое на каждое нажатие).
type MakeAction = fn() -> Box<dyn Action>;

/// Кнопки строки команд: текст и действие команды.
const BUTTONS: [(&str, MakeAction); 6] = [
    ("bar-new", || Box::new(mail::New)),
    ("bar-reply", || Box::new(mail::Reply)),
    ("bar-reply-all", || Box::new(mail::ReplyAll)),
    ("bar-forward", || Box::new(mail::Forward)),
    ("bar-delete", || Box::new(mail::Delete)),
    ("bar-flag", || Box::new(mail::ToggleFlag)),
];

/// Настройки ящика (в Outlook — меню «Файл»): справа, у поиска.
const SETTINGS: [(&str, MakeAction); 2] =
    [("bar-vacation", || Box::new(settings::Vacation)), ("bar-rules", || Box::new(settings::Rules))];

/// Высота строки команд: кэшированное view не измеряется по содержимому, размер задаётся явно.
pub(crate) const BAR_HEIGHT: f32 = 40.0;

pub(crate) struct CommandBar {
    pub(crate) search: Entity<InputState>,
}

impl Render for CommandBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(0xdddddd))
            .children(BUTTONS.map(button))
            .child(div().flex_1())
            .children(SETTINGS.map(button))
            .child(div().w(px(320.)).child(Input::new(&self.search)))
    }
}

fn button((key, action): (&'static str, MakeAction)) -> impl IntoElement {
    div()
        .id(key)
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(rgb(0xececec))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(0xe0e0e0)))
        .child(tr!(key))
        // Действие уходит в фокус окна (экран почты), как от клавиши.
        .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
}

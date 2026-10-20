//! Строка команд: кнопки действий и поле поиска.

use gpui_kit::*;
use gpui_kit::component::input::{Input, InputState};
use pets_i18n::tr;

// --- строка команд ---

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
            .children(["bar-new", "bar-reply", "bar-reply-all", "bar-forward", "bar-delete", "bar-flag"].map(|key| {
                div().px_2().py_0p5().rounded_md().bg(rgb(0xececec)).child(tr!(key))
            }))
            .child(div().flex_1())
            .child(div().w(px(320.)).child(Input::new(&self.search)))
    }
}

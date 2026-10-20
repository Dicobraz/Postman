//! Командная строка вместо ленты (S5-01, DESIGN «Главное окно»): одна акцентная кнопка «Новое письмо», действия
//! с письмом — тихими кнопками, справа поиск с охватом «Везде | Текущая папка» и «…» (автоответы, правила).
//! Кнопка — та же команда реестра, что и хоткей (S3-02).

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::theme::Metric;

use crate::chrome::{self, glyph};
use crate::commands::mail;
use crate::roles::{Look, look};

/// Действие кнопки (новое на каждое нажатие).
type MakeAction = fn() -> Box<dyn Action>;

/// Тихие кнопки: значок Segoe Fluent Icons, текст, действие команды.
const BUTTONS: [(char, &str, MakeAction); 5] = [
    (glyph::REPLY, "frame-reply", || Box::new(mail::Reply)),
    (glyph::REPLY_ALL, "frame-reply-all", || Box::new(mail::ReplyAll)),
    (glyph::FORWARD, "frame-forward", || Box::new(mail::Forward)),
    (glyph::DELETE, "frame-delete", || Box::new(mail::Delete)),
    (glyph::FLAG, "frame-flag", || Box::new(mail::ToggleFlag)),
];

/// Высота командной строки: кэшированное view не измеряется по содержимому, размер задаётся явно.
pub(crate) const BAR_HEIGHT: f32 = 48.0;
/// Ширина поля поиска (макет главного окна).
const SEARCH_W: f32 = 300.0;

/// Где искать: во всех папках или в открытой.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum Scope {
    All,
    #[default]
    Folder,
}

pub(crate) enum BarEvent {
    /// «…»: открыть или закрыть меню (рисует экран почты поверх всего).
    More,
    /// Охват поиска сменился: повторить поиск, если он идёт.
    Scope,
}

impl EventEmitter<BarEvent> for CommandBar {}

pub(crate) struct CommandBar {
    pub(crate) search: Entity<InputState>,
    pub(crate) scope: Scope,
    /// Меню «…» открыто: кнопка нажата.
    pub(crate) more_open: bool,
}

impl CommandBar {
    pub(crate) fn new(search: Entity<InputState>) -> Self {
        Self { search, scope: Scope::default(), more_open: false }
    }

    fn scope_switch(&self, l: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let seg = |id: &'static str, key: &'static str, scope: Scope| {
            let on = self.scope == scope;
            div()
                .id(id)
                .h_full()
                .px_2()
                .flex()
                .items_center()
                .rounded(px(l.m(Metric::RadiusControl) - 1.))
                .text_size(l.px(Metric::TypeCaption))
                .cursor_pointer()
                .when(on, |d| d.bg(l.panel()).text_color(l.text()).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |d| d.text_color(l.text2()).hover(|d| d.bg(l.hover())))
                .child(tr!(key))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.scope != scope {
                        this.scope = scope;
                        cx.emit(BarEvent::Scope);
                        cx.notify();
                    }
                }))
        };
        div()
            .flex_none()
            .h(px(28.))
            .p(px(2.))
            .flex()
            .gap(px(2.))
            .rounded(l.px(Metric::RadiusControl))
            .bg(l.fill())
            .child(seg("scope-all", "frame-scope-all", Scope::All))
            .child(seg("scope-folder", "frame-scope-folder", Scope::Folder))
    }
}

impl Render for CommandBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = look(cx);
        let new = div()
            .id("bar-new")
            .h(px(32.))
            .px(px(14.))
            .mr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(l.px(Metric::RadiusControl))
            .bg(l.accent())
            .text_color(l.on_accent())
            .cursor_pointer()
            .hover(|d| d.bg(l.accent_hover()))
            .child(chrome::icon(glyph::NEW_MAIL, 16.))
            .child(tr!("frame-new"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(mail::New), cx));
        let buttons = BUTTONS.map(|(g, key, action)| {
            chrome::quiet_button(key, &l)
                .child(chrome::icon(g, 16.))
                .child(tr!(key))
                // Действие уходит в фокус окна (экран почты), как от клавиши.
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
        });
        let kbd = div()
            .flex_none()
            .px(px(6.))
            .rounded(l.px(Metric::RadiusControl))
            .border_1()
            .border_color(l.stroke())
            .text_size(px(11.))
            .text_color(l.text3())
            .child("Ctrl+E");
        let search = Input::new(&self.search)
            .prefix(chrome::icon(glyph::SEARCH, 14.).text_color(l.text3()))
            .suffix(kbd);
        let more = chrome::quiet_button("bar-more", &l)
            .when(self.more_open, |d| d.bg(l.pressed()))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr!("frame-more")).build(window, cx))
            .child(chrome::icon(glyph::MORE, 16.))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(BarEvent::More)));
        div()
            .flex()
            .size_full()
            .items_center()
            .gap(px(2.))
            .px(px(12.))
            .text_color(l.text())
            .child(new)
            .children(buttons)
            .child(div().flex_1())
            .child(self.scope_switch(&l, cx))
            .child(div().ml(px(8.)).w(px(SEARCH_W)).flex_none().child(search))
            .child(div().ml(px(4.)).child(more))
    }
}

//! Каркас окна (S5-01, DESIGN «Главное окно»): свой заголовок — вкладки, место для перетаскивания и «─ □ ✕»
//! (зоны `WindowControlArea`: перетаскивание, двойной щелчок — развернуть, Snap Layouts на «□»), рейл модулей слева,
//! строка состояния и меню «…» командной строки.
//!
//! Заголовок рисует сам экран почты, не кэшируемая view: gpui не повторяет `WindowControlArea` из кэша
//! (`window_control_hitboxes` нет в `reuse_paint`), закэшированный заголовок потерял бы перетаскивание и кнопки.
//! Кнопки «─ □ ✕» и зона перетаскивания — без своих обработчиков мыши: нажатие обрабатывает Windows
//! (`WM_NCLBUTTONDOWN` по `HTCAPTION`/`HTMAXBUTTON`), иначе пропали бы Snap и развёртывание двойным щелчком.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::theme::Metric;

use crate::commands::{settings, view};
use crate::roles::{Look, Pm};
use crate::tabs::{Tab, TabKind};
use crate::{Mailer, Section};

/// Шрифт значков ОС (базис Pets): Segoe Fluent Icons, Windows 11.
pub(crate) const ICON_FONT: &str = "Segoe Fluent Icons";

/// Значки Segoe Fluent Icons.
pub(crate) mod glyph {
    pub(crate) const MAIL: char = '\u{E715}';
    pub(crate) const CALENDAR: char = '\u{E787}';
    pub(crate) const CONTACTS: char = '\u{E77B}';
    pub(crate) const SETTINGS: char = '\u{E713}';
    pub(crate) const NEW_MAIL: char = '\u{E70F}';
    pub(crate) const REPLY: char = '\u{E97A}';
    pub(crate) const REPLY_ALL: char = '\u{E8C2}';
    pub(crate) const FORWARD: char = '\u{E89C}';
    pub(crate) const DELETE: char = '\u{E74D}';
    pub(crate) const FLAG: char = '\u{E7C1}';
    pub(crate) const MORE: char = '\u{E712}';
    pub(crate) const SEARCH: char = '\u{E721}';
    pub(crate) const RULES: char = '\u{E71C}';
    pub(crate) const VACATION: char = '\u{E8BD}';
    pub(crate) const ATTACH: char = '\u{E723}';
    pub(crate) const CLOSE: char = '\u{E711}';
    pub(crate) const FOLDER: char = '\u{E8B7}';
    pub(crate) const SEND: char = '\u{E724}';
    pub(crate) const DRAFT: char = '\u{E70B}';
    pub(crate) const ARCHIVE: char = '\u{E7B8}';
    pub(crate) const JUNK: char = '\u{E8D8}';
    pub(crate) const MIN: char = '\u{E921}';
    pub(crate) const MAX: char = '\u{E922}';
    pub(crate) const RESTORE: char = '\u{E923}';
    pub(crate) const CAPTION_CLOSE: char = '\u{E8BB}';
}

/// Ширина кнопки «─ □ ✕» (Windows 11).
const W_CAPTION: f32 = 46.0;
/// Рейл модулей: ширина и кнопка (макет главного окна).
pub(crate) const W_RAIL: f32 = 52.0;
const RAIL_BUTTON: f32 = 40.0;
/// Вкладка: ширина и скругление верха.
const W_TAB_MIN: f32 = 96.0;
const W_TAB_MAX: f32 = 220.0;
/// Строка состояния.
pub(crate) const STATUS_HEIGHT: f32 = 28.0;
/// Меню «…» не открывается снова тем же щелчком, которым его закрыли (нажатие вне меню — по кнопке «…»).
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// Значок шрифтом ОС.
pub(crate) fn icon(g: char, size: f32) -> Div {
    div().flex_none().font_family(ICON_FONT).text_size(px(size)).line_height(px(size)).child(String::from(g))
}

/// Тихая кнопка командной строки: без заливки, наведение — слабая заливка.
pub(crate) fn quiet_button(id: &'static str, l: &Look) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(l.px(Metric::RadiusControl))
        .cursor_pointer()
        .hover(|d| d.bg(l.hover()))
        .active(|d| d.bg(l.pressed()))
}

/// Меню «…» командной строки.
#[derive(Default)]
pub(crate) struct MoreMenu {
    pub(crate) open: bool,
    closed_at: Option<Instant>,
}

impl MoreMenu {
    /// Щелчок по «…».
    pub(crate) fn toggle(&mut self) {
        let just_closed = self.closed_at.is_some_and(|t| t.elapsed() < REOPEN_GUARD);
        self.open = !self.open && !just_closed;
        self.closed_at = None;
    }

    pub(crate) fn close(&mut self) -> bool {
        let was = self.open;
        if was {
            self.open = false;
            self.closed_at = Some(Instant::now());
        }
        was
    }
}

impl Mailer {
    /// Название вкладки.
    fn tab_title(&self, t: &Tab, cx: &App) -> String {
        match t.kind {
            TabKind::Mail => tr!("frame-tab-mail"),
            TabKind::Settings => match &self.settings.panel {
                Some(crate::settings::Panel::Rules(_)) => tr!("frame-tab-rules"),
                _ => tr!("frame-tab-vacation"),
            },
            TabKind::Meeting => self.meetings.title(t.id, cx),
        }
    }

    /// Заголовок окна: [значок][вкладки][перетаскивание][─ □ ✕].
    pub(crate) fn titlebar(&self, l: &Look, window: &Window, cx: &mut Context<Self>) -> Div {
        let active = self.tabs.active().id;
        let tabs = self.tabs.iter().map(|t| {
            let on = t.id == active;
            let id = t.id;
            let close = t.kind.closable().then(|| {
                div()
                    .id(("tab-close", id as usize))
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(l.px(Metric::RadiusControl))
                    .hover(|d| d.bg(l.hover()))
                    .child(icon(glyph::CLOSE, 10.))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr!("frame-tab-close")).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_tab(id, window, cx);
                    }))
            });
            div()
                .id(("tab", id as usize))
                .h(l.px(Metric::HTab))
                .min_w(px(W_TAB_MIN))
                .max_w(px(W_TAB_MAX))
                .flex()
                .items_center()
                .gap(px(6.))
                .pl(px(14.))
                .pr(px(if close.is_some() { 6. } else { 14. }))
                .rounded_t(l.px(Metric::RadiusOverlay))
                .when(on, |d| d.bg(l.panel()).text_color(l.text()))
                .when(!on, |d| d.text_color(l.text2()).hover(|d| d.bg(l.hover())))
                .when(t.kind == TabKind::Meeting, |d| d.child(icon(crate::calendar::g::CALENDAR, 13.)))
                .child(div().flex_1().min_w_0().truncate().child(self.tab_title(t, cx)))
                .children(close)
                .on_click(cx.listener(move |this, _, _, cx| this.activate_tab(id, cx)))
        });
        let caption = |id: &'static str, g: char, area: WindowControlArea, close: bool| {
            div()
                .id(id)
                .w(px(W_CAPTION))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_color(l.text())
                .when(close, |d| d.hover(|d| d.bg(l.pm(Pm::CaptionClose)).text_color(l.pm(Pm::CaptionCloseOn))))
                .when(!close, |d| d.hover(|d| d.bg(l.hover())))
                .window_control_area(area)
                .child(icon(g, 10.))
        };
        let max = if window.is_maximized() { glyph::RESTORE } else { glyph::MAX };
        div()
            .h(l.px(Metric::HTitlebar))
            .w_full()
            .flex_none()
            .flex()
            .text_size(px(13.))
            .child(
                div()
                    .id("app-icon")
                    .w(px(44.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(l.accent_text())
                    .window_control_area(WindowControlArea::Drag)
                    .child(icon(glyph::MAIL, 16.)),
            )
            .child(div().h_full().flex().items_end().gap(px(2.)).min_w_0().children(tabs))
            // Пустое место — заголовок окна: перетаскивание, двойной щелчок — развернуть.
            .child(div().id("drag").flex_1().min_w(px(48.)).h_full().window_control_area(WindowControlArea::Drag))
            .child(caption("caption-min", glyph::MIN, WindowControlArea::Min, false))
            .child(caption("caption-max", max, WindowControlArea::Max, false))
            .child(caption("caption-close", glyph::CAPTION_CLOSE, WindowControlArea::Close, true))
    }

    /// Рейл модулей: Почта, Календарь, Контакты; внизу — Настройки. Подписи — подсказкой.
    pub(crate) fn rail(&self, l: &Look) -> Div {
        let settings_tab = self.tabs.is_active(TabKind::Settings);
        let button = |id: &'static str, g: char, key: &'static str, on: bool, action: fn() -> Box<dyn Action>| {
            div()
                .id(id)
                .size(px(RAIL_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(l.panel()).text_color(l.accent_text()))
                .when(!on, |d| d.text_color(l.text2()).hover(|d| d.bg(l.hover())))
                .child(icon(g, 20.))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tr!(key)).build(window, cx))
                .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
        };
        let module = |s: Section| self.tabs.is_active(TabKind::Mail) && self.section == s;
        div()
            .w(px(W_RAIL))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .pt(px(4.))
            .pb(px(8.))
            .child(button("rail-mail", glyph::MAIL, "frame-rail-mail", module(Section::Mail), || Box::new(view::Mail)))
            .child(button("rail-calendar", glyph::CALENDAR, "frame-rail-calendar", module(Section::Calendar), || {
                Box::new(view::Calendar)
            }))
            .child(button("rail-contacts", glyph::CONTACTS, "frame-rail-contacts", module(Section::Contacts), || {
                Box::new(view::Contacts)
            }))
            .child(div().flex_1())
            .child(button("rail-settings", glyph::SETTINGS, "frame-rail-settings", settings_tab, || {
                Box::new(settings::Vacation)
            }))
    }

    /// Строка состояния: элементы, синхронизация, квота, последнее нажатие, песочница, уведомления.
    pub(crate) fn status_bar(&self, l: &Look, cx: &mut Context<Self>) -> Div {
        let status = self.items_status(cx);
        let unread = self.toasts.read(cx).unread();
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(14.))
            .text_size(l.px(Metric::TypeCaption))
            .text_color(l.text2())
            .child(status)
            .child(self.live.status.clone())
            .child(self.settings.quota_text())
            .child(div().flex_1())
            .child(self.last_key.clone())
            .child(self.sandbox.map(crate::reading::sandbox_label).unwrap_or_default())
            .child(
                div()
                    .id("notifications")
                    .px_1()
                    .rounded(l.px(Metric::RadiusControl))
                    .cursor_pointer()
                    .hover(|d| d.bg(l.hover()))
                    .when(unread > 0, |d| d.text_color(l.accent_text()))
                    .child(tr!("status-notifications", n = unread))
                    .on_click(cx.listener(|this, _, _, cx| this.toasts.update(cx, |t, cx| t.toggle_history(cx)))),
            )
    }

    /// Меню «…» под командной строкой справа: автоответы и правила (настройки — вкладкой в S5-09).
    pub(crate) fn more_menu(&self, l: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.more.open {
            return None;
        }
        let item = |id: &'static str, g: char, key: &'static str, action: fn() -> Box<dyn Action>| {
            div()
                .id(id)
                .h(l.px(Metric::HMenuItem))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(l.px(Metric::RadiusControl))
                .cursor_pointer()
                .hover(|d| d.bg(l.hover()))
                .child(icon(g, 16.).text_color(l.text2()))
                .child(tr!(key))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.more.close();
                    this.bar.update(cx, |b, cx| {
                        b.more_open = false;
                        cx.notify();
                    });
                    window.dispatch_action(action(), cx);
                }))
        };
        let top = l.m(Metric::HTitlebar) + crate::bar::BAR_HEIGHT - 6.;
        let menu = div()
            .id("more-menu")
            .absolute()
            .top(px(top))
            .right(px(12.))
            .min_w(px(220.))
            .p(px(4.))
            .flex()
            .flex_col()
            .rounded(l.px(Metric::RadiusOverlay))
            .border_1()
            .border_color(l.c(pets_ui::theme::Role::StrokeCard))
            .bg(l.c(pets_ui::theme::Role::LayerOverlay))
            .text_color(l.text())
            .shadow_lg()
            .child(item("more-vacation", glyph::VACATION, "frame-vacation", || Box::new(settings::Vacation)))
            .child(item("more-rules", glyph::RULES, "frame-rules", || Box::new(settings::Rules)))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_more(cx);
            }));
        Some(menu.into_any_element())
    }

    pub(crate) fn toggle_more(&mut self, cx: &mut Context<Self>) {
        self.more.toggle();
        let open = self.more.open;
        self.bar.update(cx, |b, cx| {
            b.more_open = open;
            cx.notify();
        });
        cx.notify();
    }

    /// Закрыть меню «…»; `false` — не было открыто (Esc идёт дальше).
    pub(crate) fn close_more(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.more.close() {
            return false;
        }
        self.bar.update(cx, |b, cx| {
            b.more_open = false;
            cx.notify();
        });
        cx.notify();
        true
    }

    pub(crate) fn activate_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.tabs.activate(id) {
            cx.notify();
        }
    }

    /// Закрыть вкладку (× или Ctrl+W): вкладка настроек закрывает панель.
    pub(crate) fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id).copied() else { return };
        match tab.kind {
            TabKind::Mail => {}
            TabKind::Settings => {
                self.close_settings(window, cx);
            }
            TabKind::Meeting => self.close_meeting(id, window, cx),
        }
        cx.notify();
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab.
    pub(crate) fn step_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.tabs.step(delta);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::MoreMenu;

    /// Щелчок по «…» при открытом меню: нажатие вне меню закрыло его — тот же щелчок не открывает снова.
    #[test]
    fn more_menu_closes_on_its_own_button() {
        let mut m = MoreMenu::default();
        m.toggle();
        assert!(m.open);
        assert!(m.close());
        m.toggle();
        assert!(!m.open, "тот же щелчок");
        m.toggle();
        assert!(m.open);
    }
}

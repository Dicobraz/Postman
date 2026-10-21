//! Меню папки (S5-05б, DESIGN «Беседы»): правая кнопка по папке в дереве — «Вид беседы ▸ Лента / Стопка / Как
//! Outlook / По умолчанию». Слой поверх экрана, как контекстное меню письма (`menu.rs`): ↑ / ↓ — по пунктам, → и
//! Enter — подменю, ← — назад, Enter — выбрать, Esc и щелчок вне — закрыть. Выбор — свой вид папки в настройках
//! (`Reading::set_folder`); «По умолчанию» — вид из «Настройки → Чтение».

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::theme::{Metric, Role};
use postman_core::prefs::Conversation;

use crate::Mailer;
use crate::bulk::menu_line;
use crate::chrome::{glyph, icon};
use crate::roles::Look;

const WIDTH: f32 = 220.;
const SUB_WIDTH: f32 = 240.;
const ITEM_H: f32 = 28.;

/// Пункты подменю: свой вид папки или «По умолчанию» (`None`).
const VIEWS: [Option<Conversation>; 4] = [Some(Conversation::Feed), Some(Conversation::Stack), Some(Conversation::Outlook), None];

/// Открытое меню папки.
pub(crate) struct FolderMenu {
    /// Левый верхний угол (в окне).
    at: Point<Pixels>,
    folder: String,
    /// Подменю «Вид беседы» открыто; курсор — в нём (номер в [`VIEWS`]).
    sub: bool,
    cursor: usize,
    focus: FocusHandle,
    /// Фокус — меню (на следующем кадре: ему нужно окно).
    focus_pending: bool,
}

/// Название вида в меню.
fn view_name(v: Option<Conversation>) -> String {
    match v {
        Some(Conversation::Feed) => tr!("thread-view-feed"),
        Some(Conversation::Stack) => tr!("thread-view-stack"),
        Some(Conversation::Outlook) => tr!("thread-view-outlook"),
        None => tr!("thread-view-default"),
    }
}

impl Mailer {
    /// Правая кнопка по папке `folder` у точки `at`.
    pub(crate) fn open_folder_menu(&mut self, folder: String, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let own = crate::prefs::get(cx).reading.folders.get(&folder).copied();
        let cursor = VIEWS.iter().position(|v| *v == own).unwrap_or(0);
        self.folder_menu = Some(FolderMenu { at, folder, sub: false, cursor, focus: cx.focus_handle(), focus_pending: true });
        cx.notify();
    }

    /// Снимки (`--screens`): меню папки с открытым подменю, как по наведению.
    pub(crate) fn screen_folder_menu(&mut self, folder: String, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.open_folder_menu(folder, at, cx);
        if let Some(m) = &mut self.folder_menu {
            m.sub = true;
        }
    }

    /// Закрыть меню папки; `true` — было открыто (Esc идёт дальше, если нет).
    pub(crate) fn close_folder_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.folder_menu.take().is_none() {
            return false;
        }
        self.refocus = true;
        cx.notify();
        true
    }

    /// Пункт подменю выбран: свой вид папки (или как везде). Открытая папка перестраивается по настройкам.
    fn choose_view(&mut self, k: usize, cx: &mut Context<Self>) {
        let (Some(m), Some(&view)) = (&self.folder_menu, VIEWS.get(k)) else { return };
        let folder = m.folder.clone();
        self.close_folder_menu(cx);
        crate::prefs::update(cx, |p| p.reading.set_folder(&folder, view));
    }

    /// Клавиши в меню: ↑ / ↓, → и Enter — подменю, ← — назад, Enter и пробел в подменю — выбрать.
    fn folder_menu_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(m) = &mut self.folder_menu else { return };
        let n = VIEWS.len();
        match ev.keystroke.key.as_str() {
            "down" if m.sub => m.cursor = (m.cursor + 1) % n,
            "up" if m.sub => m.cursor = (m.cursor + n - 1) % n,
            "right" => m.sub = true,
            "left" => m.sub = false,
            "enter" | "space" if m.sub => {
                let k = m.cursor;
                return self.choose_view(k, cx);
            }
            "enter" | "space" | "down" | "up" => m.sub = true,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Меню поверх экрана (рядом с контекстным меню письма).
    pub(crate) fn folder_menu(&mut self, window: &mut Window, l: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.folder_menu.as_mut()?;
        if std::mem::take(&mut m.focus_pending) {
            window.focus(&m.focus, cx);
        }
        let m = self.folder_menu.as_ref()?;
        let prefs = crate::prefs::get(cx).reading;
        let own = prefs.folders.get(&m.folder).copied();
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let x = f32::from(m.at.x).min(vw - WIDTH - 8.).max(8.);
        let y = f32::from(m.at.y).min(vh - (VIEWS.len() as f32 * ITEM_H + 8.) - 8.).max(8.);
        let sub_right = x + WIDTH + 6. + SUB_WIDTH < vw - 8.;
        let sub_left = if sub_right { WIDTH + 6. } else { -(SUB_WIDTH + 6.) };
        let item = menu_line("folder-menu-view", l)
            .when(m.sub, |d| d.bg(l.hover()))
            .child(icon(glyph::CONVERSATION, 16.).text_color(l.text2()))
            .child(div().flex_1().min_w_0().truncate().child(tr!("folder-menu-view")))
            .child(icon(glyph::CHEVRON_RIGHT, 10.).text_color(l.text2()))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if let Some(m) = &mut this.folder_menu
                    && *hovered
                    && !m.sub
                {
                    m.sub = true;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(m) = &mut this.folder_menu {
                    m.sub = !m.sub;
                    cx.notify();
                }
            }));
        let sub = m.sub.then(|| {
            let mut col = div().flex().flex_col();
            for (k, v) in VIEWS.into_iter().enumerate() {
                let checked = own == v;
                // «По умолчанию» — с видом из настроек справа.
                let hint = v.is_none().then(|| view_name(Some(prefs.conversation)));
                col = col.child(
                    menu_line(("folder-menu-item", k), l)
                        .when(m.cursor == k, |d| d.bg(l.hover()))
                        .child(div().w(px(16.)).flex_none().when(checked, |d| d.child(icon(glyph::CHECK, 12.).text_color(l.accent_text()))))
                        .child(div().flex_1().min_w_0().truncate().child(view_name(v)))
                        .children(hint.map(|h| div().flex_none().text_size(px(11.)).text_color(l.text3()).child(h)))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if let Some(m) = &mut this.folder_menu
                                && *hovered
                                && m.cursor != k
                            {
                                m.cursor = k;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_view(k, cx))),
                );
            }
            div()
                .id("folder-menu-sub")
                .occlude()
                .absolute()
                .left(px(sub_left))
                .top(px(-1.))
                .w(px(SUB_WIDTH))
                .p(px(4.))
                .rounded(l.px(Metric::RadiusOverlay))
                .border_1()
                .border_color(l.c(Role::StrokeCard))
                .bg(l.c(Role::LayerOverlay))
                .shadow_lg()
                .child(col)
        });
        let sub_bounds = m.sub.then(|| {
            let h = VIEWS.len() as f32 * ITEM_H + 8.;
            Bounds::new(point(px(x + sub_left), px(y - 1.)), size(px(SUB_WIDTH), px(h)))
        });
        let focus = m.focus.clone();
        Some(
            div()
                .id("folder-menu")
                .key_context("ContextMenu")
                .track_focus(&focus)
                .occlude()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(WIDTH))
                .p(px(4.))
                .rounded(l.px(Metric::RadiusOverlay))
                .border_1()
                .border_color(l.c(Role::StrokeCard))
                .bg(l.c(Role::LayerOverlay))
                .shadow_lg()
                .text_size(px(13.))
                .text_color(l.text())
                .child(item)
                .children(sub)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.folder_menu_key(ev, cx)))
                .on_mouse_down_out(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    if sub_bounds.is_some_and(|b| b.contains(&ev.position)) {
                        return;
                    }
                    this.close_folder_menu(cx);
                }))
                .into_any_element(),
        )
    }
}

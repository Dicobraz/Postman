//! Цвета и размеры экранов (S5-01, `docs/DESIGN.md` «Цвет»): роли pets-ui ([`Role`], [`Metric`]) и роли Postman
//! (`app.*`, [`Pm`]) поверх них; итог темы для отрисовки — [`Look`] (`look(cx)`).
//!
//! Единственный модуль интерфейса с литералами цвета (тест `no_color_literals` в `main.rs`). Цвета категорий,
//! красный безопасности и янтарный сроков считает `postman_core::palette` (OKLCH, ΔE2000) — здесь они только
//! становятся ролями `app.*`, чтобы тема пользователя могла их переопределить, а высокая контрастность —
//! заменить системными.

use std::rc::Rc;
use std::sync::LazyLock;

use gpui_kit::{App, Global, Hsla, Pixels, Window};
use pets_ui::theme::{AppRole, Color, Density, Metric, Role, Theme, ThemeConfig, ThemeSpec, Themes};
use postman_core::palette::{self, Reserved};

/// Роль Postman: имя `app.…` и цвета светлой и тёмной темы ([`FIXED`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Pm {
    /// Основной текст. Тёмная тема — #D2D2D2 по DESIGN («основной текст #D2D2D2»), а не #FFFFFF ядра pets-ui
    /// (вопрос владельцу №2 в PR pets-ui открыт): решится в пользу ядра — тёмное значение станет ядерным здесь.
    Text,
    /// Вторичный текст: тёмная — #A6A6A6 по DESIGN.
    Text2,
    /// Подписи полей и кнопки панели письма, единые с «Отправить» (DESIGN «Новое письмо»).
    LabelAccent,
    /// Угроза (DESIGN: бледно-красная подложка с тёмно-красным текстом); красный — только безопасность.
    ThreatFg,
    ThreatBg,
    /// Полоса и точка угрозы.
    ThreatDot,
    /// Срок впереди — янтарный текст без подложки («До чт»).
    DeadlineFg,
    /// Срок прошёл — плотная янтарная заливка («Просрочено 1 д») и текст на ней.
    DeadlineFill,
    DeadlineOn,
    /// Флаг письма.
    Flag,
    /// «Закрыть» заголовка окна при наведении — красный Windows, и значок на нём.
    CaptionClose,
    CaptionCloseOn,
    /// Событие календаря: подложка, «под вопросом», полоса, выбранное.
    EventBg,
    EventBgTentative,
    EventBorder,
    EventBorderSelected,
    /// Занятость (помощник планирования): занят, под вопросом, нет на месте, нет данных, нерабочее время.
    FbBusy,
    FbTentative,
    FbOof,
    FbUnknown,
    FbOff,
}

impl Pm {
    pub(crate) const COUNT: usize = Pm::FbOff as usize + 1;
}

const fn c(v: u32) -> Color {
    Color::rgb(v)
}

/// Цвета ролей Postman: (роль, имя, светлая, тёмная, при высокой контрастности — роль pets-ui). Роли из палитры
/// (угроза, срок) — в [`roles`], по `palette::reserved`.
const FIXED: &[(Pm, &str, Color, Color, Role)] = &[
    (Pm::Text, "app.text", Color::argb(0xE400_0000), c(0xD2D2D2), Role::TextPrimary),
    (Pm::Text2, "app.text.secondary", Color::argb(0x9E00_0000), c(0xA6A6A6), Role::TextSecondary),
    (Pm::LabelAccent, "app.label.accent", c(0x0067C0), c(0x7DB8E8), Role::AccentText),
    (Pm::DeadlineFill, "app.deadline.fill", c(0xF8DEA8), c(0x6C5005), Role::CautionBg),
    (Pm::DeadlineOn, "app.deadline.on", c(0x5B4404), c(0xF7E2B8), Role::CautionFg),
    (Pm::Flag, "app.flag", c(0xC50F1F), c(0xD47C76), Role::TextPrimary),
    (Pm::CaptionClose, "app.caption.close", c(0xC42B1C), c(0xC42B1C), Role::AccentFill),
    (Pm::CaptionCloseOn, "app.caption.close.on", c(0xFFFFFF), c(0xFFFFFF), Role::AccentOn),
    (Pm::EventBg, "app.event.bg", c(0xD6E6F9), c(0x23364F), Role::AccentSubtle),
    (Pm::EventBgTentative, "app.event.bg.tentative", c(0xEEF4FB), c(0x1E2A38), Role::LayerPanel),
    (Pm::EventBorder, "app.event.border", c(0x2F6DB5), c(0x689BDB), Role::AccentFill),
    (Pm::EventBorderSelected, "app.event.border.selected", c(0x1B4F8A), c(0xAFD1FC), Role::TextPrimary),
    (Pm::FbBusy, "app.fb.busy", c(0x2F6DB5), c(0x4C82C4), Role::AccentFill),
    (Pm::FbTentative, "app.fb.tentative", c(0x9CC3EE), c(0x2F4C6E), Role::AccentSubtle),
    (Pm::FbOof, "app.fb.oof", c(0x7B3FA0), c(0x9466B0), Role::TextPrimary),
    (Pm::FbUnknown, "app.fb.unknown", c(0xC8C8C8), c(0x5A5A5A), Role::TextDisabled),
    (Pm::FbOff, "app.fb.off", c(0xF0F0F0), c(0x2C2C2C), Role::BgBase),
];

/// Части цвета категории: точка и полоса, текст и подложка метки.
const CAT_PARTS: [&str; 3] = ["dot", "text", "bg"];

/// Имена ролей категорий `app.cat.<слот палитры>.<часть>` — строки живут весь процесс (роль pets-ui — `&'static str`).
static CAT_NAMES: LazyLock<Vec<[String; 3]>> = LazyLock::new(|| {
    palette::HUES.iter().map(|h| CAT_PARTS.map(|part| format!("app.cat.{}.{part}", h.key))).collect()
});

/// Все роли Postman для `Themes::init`: постоянные, угроза и срок из палитры, категории по слотам палитры.
pub(crate) static ROLES: LazyLock<Vec<AppRole>> = LazyLock::new(roles);

fn roles() -> Vec<AppRole> {
    let mut out: Vec<AppRole> =
        FIXED.iter().map(|&(_, name, light, dark, hc)| AppRole { name, light, dark, hc }).collect();
    let reserved = |r: Reserved| (palette::reserved(r, palette::Theme::Light), palette::reserved(r, palette::Theme::Dark));
    let (tl, td) = reserved(Reserved::Threat);
    let (dl, dd) = reserved(Reserved::Deadline);
    out.extend([
        AppRole { name: "app.threat.fg", light: c(tl.text), dark: c(td.text), hc: Role::CriticalFg },
        AppRole { name: "app.threat.bg", light: c(tl.bg), dark: c(td.bg), hc: Role::LayerPanel },
        AppRole { name: "app.threat.dot", light: c(tl.dot), dark: c(td.dot), hc: Role::CriticalFg },
        AppRole { name: "app.deadline.fg", light: c(dl.text), dark: c(dd.text), hc: Role::CautionFg },
    ]);
    for (slot, names) in CAT_NAMES.iter().enumerate() {
        let slot = u8::try_from(slot).unwrap_or(palette::NEUTRAL);
        let (l, d) = (palette::swatch(slot, palette::Theme::Light), palette::swatch(slot, palette::Theme::Dark));
        let hc = [Role::TextPrimary, Role::TextPrimary, Role::LayerPanel];
        for (i, (light, dark)) in [(l.dot, d.dot), (l.text, d.text), (l.bg, d.bg)].into_iter().enumerate() {
            out.push(AppRole { name: names[i].as_str(), light: c(light), dark: c(dark), hc: hc[i] });
        }
    }
    out
}

/// Имя роли из палитры для [`Pm`] (постоянные — в [`FIXED`]).
fn pm_name(r: Pm) -> &'static str {
    match r {
        Pm::ThreatFg => "app.threat.fg",
        Pm::ThreatBg => "app.threat.bg",
        Pm::ThreatDot => "app.threat.dot",
        Pm::DeadlineFg => "app.deadline.fg",
        other => FIXED.iter().find(|f| f.0 == other).map_or("app.text", |f| f.1),
    }
}

const PMS: [Pm; Pm::COUNT] = [
    Pm::Text,
    Pm::Text2,
    Pm::LabelAccent,
    Pm::ThreatFg,
    Pm::ThreatBg,
    Pm::ThreatDot,
    Pm::DeadlineFg,
    Pm::DeadlineFill,
    Pm::DeadlineOn,
    Pm::Flag,
    Pm::CaptionClose,
    Pm::CaptionCloseOn,
    Pm::EventBg,
    Pm::EventBgTentative,
    Pm::EventBorder,
    Pm::EventBorderSelected,
    Pm::FbBusy,
    Pm::FbTentative,
    Pm::FbOof,
    Pm::FbUnknown,
    Pm::FbOff,
];

/// Цвета категории в теме.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Cat {
    pub(crate) dot: Hsla,
    pub(crate) text: Hsla,
    pub(crate) bg: Hsla,
}

/// Итоговая тема для отрисовки: роли pets-ui, роли Postman и категории — по индексу, без поиска по имени.
/// Пересчитывается при смене темы ([`init`]); в `render` — `let l = look(cx);` (дёшево: `Rc`).
#[derive(Clone, Debug)]
pub(crate) struct Look {
    theme: Theme,
    pm: [Hsla; Pm::COUNT],
    cats: Vec<Cat>,
}

impl Look {
    pub(crate) fn new(theme: &Theme) -> Look {
        let pm = PMS.map(|r| theme.app(pm_name(r)));
        let cats = CAT_NAMES
            .iter()
            .map(|n| Cat { dot: theme.app(&n[0]), text: theme.app(&n[1]), bg: theme.app(&n[2]) })
            .collect();
        Look { theme: theme.clone(), pm, cats }
    }

    pub(crate) fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Цвет роли pets-ui.
    pub(crate) fn c(&self, r: Role) -> Hsla {
        self.theme.c(r)
    }

    /// Цвет роли Postman.
    pub(crate) fn pm(&self, r: Pm) -> Hsla {
        self.pm[r as usize]
    }

    /// Цвета категории по слоту палитры; неизвестный слот — нейтральный.
    pub(crate) fn cat(&self, slot: u8) -> Cat {
        let i = usize::from(slot);
        self.cats.get(i).or(self.cats.first()).copied().unwrap_or(Cat {
            dot: self.c(Role::TextTertiary),
            text: self.text2(),
            bg: self.c(Role::FillCursor),
        })
    }

    pub(crate) fn px(&self, m: Metric) -> Pixels {
        self.theme.px(m)
    }

    pub(crate) fn m(&self, m: Metric) -> f32 {
        self.theme.m(m)
    }

    pub(crate) fn dark(&self) -> bool {
        self.theme.is_dark()
    }

    pub(crate) fn density(&self) -> Density {
        self.theme.density()
    }

    /// Компактная плотность: строка письма без аватара и превью (DESIGN «Главное окно»).
    pub(crate) fn compact(&self) -> bool {
        self.density() == Density::Compact
    }

    // --- частые роли ---

    pub(crate) fn text(&self) -> Hsla {
        self.pm(Pm::Text)
    }

    pub(crate) fn text2(&self) -> Hsla {
        self.pm(Pm::Text2)
    }

    pub(crate) fn text3(&self) -> Hsla {
        self.c(Role::TextTertiary)
    }

    /// Фон каркаса окна.
    pub(crate) fn base(&self) -> Hsla {
        self.c(Role::BgBase)
    }

    /// Карточка (список, область чтения, панели).
    pub(crate) fn panel(&self) -> Hsla {
        self.c(Role::LayerPanel)
    }

    /// Разделители и рамки карточек.
    pub(crate) fn border(&self) -> Hsla {
        self.c(Role::StrokeDivider)
    }

    /// Рамка контрола (кнопка, поле).
    pub(crate) fn stroke(&self) -> Hsla {
        self.c(Role::StrokeControl)
    }

    pub(crate) fn hover(&self) -> Hsla {
        self.c(Role::FillSubtleHover)
    }

    pub(crate) fn pressed(&self) -> Hsla {
        self.c(Role::FillSubtlePressed)
    }

    /// Выбранное в списке и выбранная кнопка-переключатель.
    pub(crate) fn picked(&self) -> Hsla {
        self.c(Role::AccentSubtle)
    }

    pub(crate) fn accent(&self) -> Hsla {
        self.c(Role::AccentFill)
    }

    pub(crate) fn accent_hover(&self) -> Hsla {
        self.c(Role::AccentFillHover)
    }

    pub(crate) fn accent_text(&self) -> Hsla {
        self.c(Role::AccentText)
    }

    /// Текст на акцентной заливке.
    pub(crate) fn on_accent(&self) -> Hsla {
        self.c(Role::AccentOn)
    }

    pub(crate) fn error(&self) -> Hsla {
        self.c(Role::CriticalFg)
    }

    pub(crate) fn error_bg(&self) -> Hsla {
        self.c(Role::CriticalBg)
    }

    pub(crate) fn warning_bg(&self) -> Hsla {
        self.c(Role::CautionBg)
    }

    pub(crate) fn warning(&self) -> Hsla {
        self.c(Role::CautionFg)
    }

    pub(crate) fn success(&self) -> Hsla {
        self.c(Role::SuccessFg)
    }

    pub(crate) fn success_bg(&self) -> Hsla {
        self.c(Role::SuccessBg)
    }

    /// Нейтральная заливка (код, полосы таблиц, подложки чипов).
    pub(crate) fn fill(&self) -> Hsla {
        self.c(Role::FillCursor)
    }

    pub(crate) fn fill_passive(&self) -> Hsla {
        self.c(Role::FillCursorPassive)
    }

    /// Прозрачный — рамка «нет рамки» той же толщины.
    pub(crate) fn none(&self) -> Hsla {
        gpui_kit::transparent_black()
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::new(&Theme::default())
    }
}

struct Current(Rc<Look>);

impl Global for Current {}

/// Текущая тема для отрисовки; до [`init`] (тесты) — Fluent светлая.
pub(crate) fn look(cx: &App) -> Rc<Look> {
    cx.try_global::<Current>().map_or_else(|| Rc::new(Look::default()), |c| c.0.clone())
}

thread_local! {
    /// Та же тема для мелких построителей без `cx` (кнопки, чипы): поток интерфейса один, тема ставится в
    /// [`refresh`] до перерисовки окон.
    static NOW: std::cell::RefCell<Option<Rc<Look>>> = const { std::cell::RefCell::new(None) };
}

/// Текущая тема без `cx` — для построителей элементов, которым её неоткуда передать; в `render` — [`look`].
pub(crate) fn now() -> Rc<Look> {
    NOW.with(|n| n.borrow().clone()).unwrap_or_else(|| Rc::new(Look::default()))
}

/// `--theme light|dark|system`: тема снимков и запуска; по умолчанию — «как в ОС».
pub(crate) fn choice(arg: Option<&str>) -> &'static str {
    match arg {
        Some("light") => pets_ui::theme::FLUENT_LIGHT,
        Some("dark") => pets_ui::theme::FLUENT_DARK,
        _ => pets_ui::theme::SYSTEM,
    }
}

/// `--density compact|normal|comfortable`.
pub(crate) fn density(arg: Option<&str>) -> Option<Density> {
    match arg {
        Some("compact") => Some(Density::Compact),
        Some("normal") => Some(Density::Normal),
        Some("comfortable") => Some(Density::Comfortable),
        Some("spacious") => Some(Density::Spacious),
        _ => None,
    }
}

/// Темы при запуске (до первого окна): «как в ОС» или `--theme`, плотность `--density`; смена темы ОС, акцента
/// или файла темы — на лету: [`Look`] пересчитывается, компоненты gpui-kit (поля ввода) — в тот же вид.
pub(crate) fn init(choice: &str, density: Option<Density>, cx: &mut App) {
    Themes::init(
        ThemeConfig {
            app: "Postman",
            choice: choice.to_owned(),
            overrides: density.map(|d| ThemeSpec { density: Some(d), ..ThemeSpec::default() }),
            app_roles: ROLES.as_slice(),
            ..ThemeConfig::default()
        },
        cx,
    );
    refresh(cx);
    cx.observe_global::<Themes>(refresh).detach();
}

/// Окно следит за видом ОС (светлая ↔ тёмная) сразу, не дожидаясь потока слежения pets-ui.
pub(crate) fn observe_window(window: &Window) {
    Themes::observe_window(window).detach();
}

fn refresh(cx: &mut App) {
    let next = Look::new(Theme::global(cx));
    let changed = cx.try_global::<Current>().is_none_or(|c| c.0.theme() != next.theme());
    if !changed {
        return;
    }
    sync_components(&next, cx);
    let next = Rc::new(next);
    NOW.with(|n| *n.borrow_mut() = Some(next.clone()));
    cx.set_global(Current(next));
}

/// Поля ввода и полосы прокрутки gpui-kit — в цвета темы: свой вид (светлая/тёмная) и роли pets-ui.
fn sync_components(l: &Look, cx: &mut App) {
    use gpui_kit::component::{Theme as Kit, ThemeMode};
    let mode = if l.dark() { ThemeMode::Dark } else { ThemeMode::Light };
    if Kit::global(cx).mode != mode {
        Kit::change(mode, None, cx);
    }
    Kit::update(cx, |k| {
        k.background = l.c(Role::FillControlInput);
        k.foreground = l.text();
        k.muted = l.fill();
        k.muted_foreground = l.text3();
        k.border = l.stroke();
        k.input = l.c(Role::StrokeControl);
        k.ring = l.accent();
        k.caret = l.text();
        k.selection = l.picked();
        k.accent = l.hover();
        k.accent_foreground = l.text();
        k.primary = l.accent();
        k.primary_foreground = l.on_accent();
        k.popover = l.c(Role::LayerOverlay);
        k.popover_foreground = l.text();
        k.link = l.accent_text();
    });
}

#[cfg(test)]
mod tests {
    use super::{Look, PMS, Pm, ROLES, pm_name};
    use pets_ui::theme::{FontSizes, OsLook, ThemeSpec, resolve};

    #[test]
    fn every_role_is_declared_once() {
        for (i, r) in PMS.iter().enumerate() {
            assert_eq!(*r as usize, i, "{r:?}: порядок PMS");
            assert!(ROLES.iter().any(|a| a.name == pm_name(*r)), "{r:?}: роль {} не объявлена", pm_name(*r));
        }
        let mut names: Vec<&str> = ROLES.iter().map(|a| a.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "имена ролей повторяются");
        assert!(names.iter().all(|n| n.starts_with("app.")));
    }

    /// Тёмная тема: текст — по DESIGN, категории — тёмные варианты палитры.
    #[test]
    fn dark_theme_uses_design_text_and_dark_palette() {
        let dark = OsLook { dark: true, ..OsLook::default() };
        let light = OsLook::default();
        let theme = |os: &OsLook| Look::new(&resolve(&ThemeSpec::default(), None, os, FontSizes::default(), &ROLES));
        let (l, d) = (theme(&light), theme(&dark));
        assert!(d.dark() && !l.dark());
        let hex = |c| pets_ui::theme::Color::from_hsla(c).hex();
        assert_eq!(hex(d.pm(Pm::Text)), "#D2D2D2");
        assert_eq!(hex(d.pm(Pm::Text2)), "#A6A6A6");
        let blue = postman_core::palette::BLUE;
        assert_eq!(hex(d.cat(blue).bg), format!("#{:06X}", postman_core::palette::swatch(blue, postman_core::palette::Theme::Dark).bg));
        assert_ne!(l.cat(blue).bg, d.cat(blue).bg);
        // Неизвестный слот — нейтральный.
        assert_eq!(l.cat(200), l.cat(postman_core::palette::NEUTRAL));
    }
}

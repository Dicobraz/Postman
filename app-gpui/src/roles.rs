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
use postman_core::prefs::FreeColor;

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
    /// Кнопки панели форматирования письма: светлая — как подписи, тёмная — #8CC4F0 (DESIGN «Новое письмо»).
    ToolbarAccent,
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
    /// Занятость (помощник планирования, макет «Новая встреча»): занят — заливка, текст на ней, линия (точка
    /// «занят», рамка «под вопросом», кружок «?» участника); под вопросом — заливка и текст; нет на месте — серый
    /// (S5-23): заливка, текст, точка; нет данных, нерабочее время. «Нет данных» и внешний — во встрече заливкой
    /// с прозрачностью.
    FbBusy,
    FbBusyFg,
    FbBusyLine,
    FbTentative,
    FbTentativeFg,
    FbOof,
    FbOofFg,
    FbOofLine,
    FbUnknown,
    FbOff,
    /// Занятость приглушённо (общие окна, S5-11); внешний участник. Свободное время — по выбору в настройках,
    /// [`Look::free`]; `FbFreeOn` — галочка на выбранном образце цвета.
    FbMute,
    FbMuteFg,
    FbFreeOn,
    FbExt,
    /// Пересечение с моим календарём (карточка приглашения, подробности события): текст и подложка.
    ConflictFg,
    ConflictBg,
    /// Лента дня приглашения (S5-24, макет «Приглашение в письме» Г1): пересечение — заливка и текст на ней; свои
    /// события — серые; приглашение — синее (принято — заливка, иначе — полупрозрачно с пунктиром).
    ConflictFill,
    ConflictOn,
    DayOwn,
    DayOwnFg,
    InviteFill,
    InviteOn,
    /// Тонированная второстепенная кнопка и метка «Ждёт ответа» (DESIGN «Цвет», синяя подложка) и её наведение.
    Tinted,
    TintedFg,
    TintedHover,
    /// Метка «Принято»: зелёная подложка и текст (DESIGN, токены категорий — зелёный).
    OkBg,
    OkFg,
    /// Нерабочие часы в сетке недели.
    CalOff,
    /// «Сообщить о фишинге» — красная маска (DESIGN «Действия и состояния», #CD605A): красный — безопасность.
    Phish,
    /// Совпадение поиска (S5-10): янтарная подложка — янтарный только у сроков и совпадений (DESIGN «Цвет»).
    SearchHit,
}

impl Pm {
    pub(crate) const COUNT: usize = Pm::SearchHit as usize + 1;
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
    (Pm::ToolbarAccent, "app.toolbar.accent", c(0x0067C0), c(0x8CC4F0), Role::AccentText),
    (Pm::DeadlineFill, "app.deadline.fill", c(0xF8DEA8), c(0x6C5005), Role::CautionBg),
    (Pm::DeadlineOn, "app.deadline.on", c(0x5B4404), c(0xF7E2B8), Role::CautionFg),
    (Pm::Flag, "app.flag", c(0xC50F1F), c(0xD47C76), Role::TextPrimary),
    (Pm::CaptionClose, "app.caption.close", c(0xC42B1C), c(0xC42B1C), Role::AccentFill),
    (Pm::CaptionCloseOn, "app.caption.close.on", c(0xFFFFFF), c(0xFFFFFF), Role::AccentOn),
    (Pm::EventBg, "app.event.bg", c(0xD6E6F9), c(0x23364F), Role::AccentSubtle),
    (Pm::EventBgTentative, "app.event.bg.tentative", c(0xEEF4FB), c(0x1E2A38), Role::LayerPanel),
    (Pm::EventBorder, "app.event.border", c(0x2F6DB5), c(0x689BDB), Role::AccentFill),
    (Pm::EventBorderSelected, "app.event.border.selected", c(0x1B4F8A), c(0xAFD1FC), Role::TextPrimary),
    (Pm::FbBusy, "app.fb.busy", c(0x4687D8), c(0x3A5F8F), Role::AccentFill),
    (Pm::FbBusyFg, "app.fb.busy.fg", c(0xFFFFFF), c(0xE6EEF8), Role::AccentOn),
    (Pm::FbBusyLine, "app.fb.busy.line", c(0x4687D8), c(0x689BDB), Role::AccentFill),
    (Pm::FbTentative, "app.fb.tentative", c(0xDFF0FF), c(0x1E3A5C), Role::AccentSubtle),
    (Pm::FbTentativeFg, "app.fb.tentative.fg", c(0x234E82), c(0xBFDDFF), Role::TextPrimary),
    (Pm::FbOof, "app.fb.oof", c(0x6E6E6E), c(0x4F4F4F), Role::TextPrimary),
    (Pm::FbOofFg, "app.fb.oof.fg", c(0xFFFFFF), c(0xE6E6E6), Role::LayerPanel),
    (Pm::FbOofLine, "app.fb.oof.line", c(0x6E6E6E), c(0x8A8A8A), Role::TextPrimary),
    (Pm::FbUnknown, "app.fb.unknown", c(0xB7B7B7), c(0x727272), Role::TextDisabled),
    (Pm::FbOff, "app.fb.off", c(0xF0F0F0), c(0x2C2C2C), Role::BgBase),
    (Pm::FbMute, "app.fb.mute", c(0xE3E3E3), c(0x3A3A3A), Role::TextDisabled),
    (Pm::FbMuteFg, "app.fb.mute.fg", c(0x8A8A8A), c(0x8A8A8A), Role::TextSecondary),
    (Pm::FbFreeOn, "app.fb.free.on", c(0xFFFFFF), c(0xFFFFFF), Role::AccentOn),
    (Pm::FbExt, "app.fb.ext", c(0x177CAA), c(0x5FA8D3), Role::TextSecondary),
    (Pm::ConflictFg, "app.conflict.fg", c(0x5C2E91), c(0xDFC1EE), Role::TextPrimary),
    (Pm::ConflictBg, "app.conflict.bg", c(0xF3EEFA), c(0x3F2D47), Role::LayerPanel),
    (Pm::ConflictFill, "app.conflict.fill", c(0xA569BF), c(0xB681C2), Role::TextPrimary),
    (Pm::ConflictOn, "app.conflict.on", c(0xFFFFFF), c(0x1A1A1A), Role::LayerPanel),
    (Pm::DayOwn, "app.day.own", c(0xE3E3E3), c(0x444444), Role::FillCursor),
    (Pm::DayOwnFg, "app.day.own.fg", c(0x424242), c(0xD2D2D2), Role::TextPrimary),
    (Pm::InviteFill, "app.invite.fill", c(0x0067C0), c(0x7DB8E8), Role::AccentFill),
    (Pm::InviteOn, "app.invite.on", c(0xFFFFFF), c(0x1A1A1A), Role::AccentOn),
    (Pm::Tinted, "app.tinted", c(0xDFF0FF), c(0x23364F), Role::AccentSubtle),
    (Pm::TintedFg, "app.tinted.fg", c(0x234E82), c(0xBFDDFF), Role::TextPrimary),
    (Pm::TintedHover, "app.tinted.hover", c(0xDCEBFA), c(0x2C4466), Role::FillSubtleHover),
    (Pm::OkBg, "app.ok.bg", c(0xE5F4DD), c(0x2A3B20), Role::LayerPanel),
    (Pm::OkFg, "app.ok.fg", c(0x34591B), c(0xBAD9AB), Role::SuccessFg),
    (Pm::CalOff, "app.cal.off", c(0xFAFAFA), c(0x272727), Role::LayerPanel),
    (Pm::Phish, "app.phish", c(0xCD605A), c(0xD47C76), Role::CriticalFg),
    (Pm::SearchHit, "app.search.hit", c(0xF9EDD5), c(0x413315), Role::AccentSubtle),
];

/// Цвет свободного времени (S5-20): только цвета бренда (решение владельца 2026-10-21) — розовый, синий,
/// серо-голубой. У каждого — рамка окна (и свечение, S5-23), подложка, подложка при наведении и текст; светлая и
/// тёмная тема. Тёмная — приглушённая (DESIGN «Цвет»): розовый — вариант B макета встречи, синий и серо-голубой —
/// в его духе. Роли `app.fb.free.<id>[.bg|.hover|.fg]`.
/// Выбор, имена ролей (рамка, подложка, наведение, текст), цвета светлой и тёмной темы.
type FreeRow = (FreeColor, [&'static str; 4], [u32; 4], [u32; 4]);

pub(crate) const FREE: [FreeRow; 3] = [
    (
        FreeColor::Brand,
        ["app.fb.free.brand", "app.fb.free.brand.bg", "app.fb.free.brand.hover", "app.fb.free.brand.fg"],
        [0xE4107F, 0xFCE4EF, 0xF8CDE1, 0x8C0B4E],
        [0xD2569A, 0x3A1A2A, 0x4A2135, 0xF0A8CA],
    ),
    (
        FreeColor::Blue,
        ["app.fb.free.blue", "app.fb.free.blue.bg", "app.fb.free.blue.hover", "app.fb.free.blue.fg"],
        [0x3A9BD5, 0xEDF6FC, 0xD9ECF8, 0x1F5F8A],
        [0x4F94C4, 0x1A2B3A, 0x213648, 0xA8D2F0],
    ),
    (
        FreeColor::Slate,
        ["app.fb.free.slate", "app.fb.free.slate.bg", "app.fb.free.slate.hover", "app.fb.free.slate.fg"],
        [0x7A8FA6, 0xF2F5F8, 0xE3E9EF, 0x4F6378],
        [0x8494A8, 0x262C34, 0x2F3741, 0xC2CCD8],
    ),
];

/// Цвета свободного времени в теме.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Free {
    /// Рамка окна, точка «свободен» у участника, образец в настройках.
    pub(crate) line: Hsla,
    pub(crate) bg: Hsla,
    /// Подложка окна под указателем.
    pub(crate) hover: Hsla,
    pub(crate) fg: Hsla,
}

/// Части цвета категории: точка и полоса, текст и подложка метки.
const CAT_PARTS: [&str; 3] = ["dot", "text", "bg"];

/// Имена ролей категорий `app.cat.<цвет Fluent>.<часть>` по порядку `palette::PALETTE` — строки живут весь процесс
/// (имя роли pets-ui — `&'static str`).
static CAT_NAMES: LazyLock<Vec<[String; 3]>> = LazyLock::new(|| {
    palette::PALETTE.iter().map(|c| CAT_PARTS.map(|part| format!("app.cat.{}.{part}", c.key))).collect()
});

/// Все роли Postman для `Themes::init`: постоянные, угроза и срок из палитры, цвета категорий (общие цвета Fluent 2).
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
    for (_, names, light, dark) in FREE {
        let hc = [Role::AccentFill, Role::LayerPanel, Role::FillSubtleHover, Role::TextPrimary];
        for i in 0..4 {
            out.push(AppRole { name: names[i], light: c(light[i]), dark: c(dark[i]), hc: hc[i] });
        }
    }
    for (color, names) in palette::PALETTE.iter().zip(CAT_NAMES.iter()) {
        let (l, d) = (color.swatch(palette::Theme::Light), color.swatch(palette::Theme::Dark));
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
    Pm::ToolbarAccent,
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
    Pm::FbBusyFg,
    Pm::FbBusyLine,
    Pm::FbTentative,
    Pm::FbTentativeFg,
    Pm::FbOof,
    Pm::FbOofFg,
    Pm::FbOofLine,
    Pm::FbUnknown,
    Pm::FbOff,
    Pm::FbMute,
    Pm::FbMuteFg,
    Pm::FbFreeOn,
    Pm::FbExt,
    Pm::ConflictFg,
    Pm::ConflictBg,
    Pm::ConflictFill,
    Pm::ConflictOn,
    Pm::DayOwn,
    Pm::DayOwnFg,
    Pm::InviteFill,
    Pm::InviteOn,
    Pm::Tinted,
    Pm::TintedFg,
    Pm::TintedHover,
    Pm::OkBg,
    Pm::OkFg,
    Pm::CalOff,
    Pm::Phish,
    Pm::SearchHit,
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
    free: [Free; 3],
}

impl Look {
    pub(crate) fn new(theme: &Theme) -> Look {
        let pm = PMS.map(|r| theme.app(pm_name(r)));
        let cats = CAT_NAMES
            .iter()
            .map(|n| Cat { dot: theme.app(&n[0]), text: theme.app(&n[1]), bg: theme.app(&n[2]) })
            .collect();
        let free = FREE.map(|(_, n, _, _)| Free { line: theme.app(n[0]), bg: theme.app(n[1]), hover: theme.app(n[2]), fg: theme.app(n[3]) });
        Look { theme: theme.clone(), pm, cats, free }
    }

    /// Цвета свободного времени по выбору «Настройки → Календарь».
    pub(crate) fn free(&self, color: FreeColor) -> Free {
        let i = FREE.iter().position(|f| f.0 == color).unwrap_or(0);
        self.free[i]
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

    /// Цвета категории по имени цвета Fluent; неизвестный цвет — нейтральный.
    pub(crate) fn cat(&self, key: &str) -> Cat {
        let index = |k: &str| palette::PALETTE.iter().position(|c| c.key == k);
        let i = index(key).or_else(|| index(palette::NEUTRAL));
        i.and_then(|i| self.cats.get(i)).copied().unwrap_or(Cat {
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

    /// Цвет свободного времени (S5-20), все три выбора в обеих темах: текст на подложке и на подложке наведения
    /// ≥ 4,5:1, рамка на панели (белая / #2B2B2B) ≥ 3:1 (не текст, WCAG 1.4.11), рамка далека от красного
    /// безопасности и янтарного сроков (ΔE2000 ≥ 25). Исключение — розовый бренда (#E4107F, тёмная — #D2569A): до
    /// красного ΔE ≈ 19 и 23,5 — решение владельца, DESIGN «Цвет» (строка «Исключение»: только свободное время, не
    /// рядом с красным безопасности); держим ≥ 18.
    #[test]
    fn free_time_colors_contrast_and_distance() {
        use postman_core::palette::{Reserved, Theme as T, delta_e, reserved};
        use postman_core::prefs::FreeColor;
        fn lum(v: u32) -> f64 {
            let ch = |s: u32| {
                let c = f64::from((v >> s) & 0xFF) / 255.0;
                if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
        }
        fn contrast(a: u32, b: u32) -> f64 {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        }
        assert_eq!(super::FREE.map(|f| f.0), FreeColor::ALL, "по выбору, по порядку");
        for (color, names, light, dark) in super::FREE {
            for (theme, [line, bg, hover, fg], panel) in [(T::Light, light, 0xFFFFFF), (T::Dark, dark, 0x2B2B2B)] {
                assert!(names[0].ends_with(color.id()), "{color:?}: {}", names[0]);
                for under in [bg, hover] {
                    let tc = contrast(fg, under);
                    assert!(tc >= 4.5, "{color:?} {theme:?}: текст {tc:.2}");
                }
                let lc = contrast(line, panel);
                assert!(lc >= 3.0, "{color:?} {theme:?}: рамка {lc:.2}");
                for r in Reserved::ALL {
                    let d = delta_e(line, reserved(r, theme).dot);
                    let min = if (color, r) == (FreeColor::Brand, Reserved::Threat) { 18.0 } else { 25.0 };
                    assert!(d >= min, "{color:?} {theme:?} ~ {r:?}: ΔE {d:.1}");
                }
            }
        }
        // Look отдаёт роли выбранного цвета; по умолчанию — розовый бренда.
        let look = Look::new(&resolve(&ThemeSpec::default(), None, &OsLook::default(), FontSizes::default(), &ROLES));
        let hex = |c| pets_ui::theme::Color::from_hsla(c).hex();
        assert_eq!(hex(look.free(FreeColor::default()).line), "#E4107F");
        assert_eq!(hex(look.free(FreeColor::Blue).line), "#3A9BD5");
        assert_eq!(hex(look.free(FreeColor::Slate).fg), "#4F6378");
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
        let dark_bg = postman_core::palette::swatch(blue, postman_core::palette::Theme::Dark).bg;
        assert_eq!(hex(d.cat(blue).bg), format!("#{dark_bg:06X}"));
        assert_ne!(l.cat(blue).bg, d.cat(blue).bg);
        // Неизвестный цвет — нейтральный.
        assert_eq!(l.cat("no-such-color"), l.cat(postman_core::palette::NEUTRAL));
    }
}

//! Палитра категорий и меток (S5-02, `docs/DESIGN.md` «Цвет», решение владельца №5 в `docs/specs/S5.md`):
//! «общие цвета» Fluent 2 (shared colors) — общеупотребимая палитра Microsoft 365, а не своя.
//!
//! У каждого цвета Fluent — ряд оттенков (`shade50…tint60`); роли берутся токенами Fluent UI v9
//! `colorPalette<Цвет>…` для светлой и тёмной темы:
//!
//! | Роль | Токен Fluent | Светлая | Тёмная |
//! |---|---|---|---|
//! | точка и полоса в строке письма | `Background3` (заливка) | `primary` | `primary` |
//! | подложка метки | `Background2` | `tint40` | `shade30` |
//! | текст метки | `Foreground2` | `shade30` | `tint40` |
//! | обводка (выбор цвета в настройках) | `BorderActive` | `primary` | `tint30` |
//!
//! Полоса — заливка, как у бейджа Fluent с заливкой (`Background3`): в тёмной теме это тот же `primary`, приглушены
//! крупные заливки — подложки меток (`shade30`), как в DESIGN.md.
//!
//! Красный — только безопасность, янтарный (`marigold`) — только сроки ([`Reserved`]). Категории берут только цвета,
//! далёкие от них (ΔE2000 ≥ [`MIN_DELTA_E`], [`usable`]): красные (`darkRed`, `cranberry`, `pumpkin`, `magenta`)
//! и янтарные (`peach`, `gold`, `brass`, `brown`) из выбора выпадают. В одной таблице категорий цвета тоже не близки
//! ([`clash`], [`check`]).
//!
//! Значения — `@fluentui/tokens` 9.x (`src/global/colors.ts`, ряды `sharedColors`; роли —
//! `src/alias/lightColorPalette.ts`, `darkColorPalette.ts`).
//!
//! Fluent UI: Copyright (c) Microsoft Corporation. Лицензия MIT
//! (<https://github.com/microsoft/fluentui/blob/master/LICENSE>); значения цветов взяты оттуда с этим уведомлением.

use std::sync::OnceLock;

/// Цвет sRGB `0xRRGGBB`.
pub type Rgb = u32;

/// Наименьшая разница цветов в одном списке (ΔE2000).
pub const MIN_DELTA_E: f64 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 2] = [Theme::Light, Theme::Dark];
}

/// Цвет в OKLCH: светлота 0…1, насыщенность (≈0…0,4), оттенок в градусах.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// Цвета роли в теме.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swatch {
    /// Точка у фильтра и полоса слева в строке письма (`Background3`).
    pub dot: Rgb,
    /// Текст метки (`Foreground2`).
    pub text: Rgb,
    /// Подложка метки (`Background2`).
    pub bg: Rgb,
    /// Обводка выбранного (`BorderActive`).
    pub border: Rgb,
}

/// Общий цвет Fluent 2: имя и оттенки ряда, из которых собраны роли.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// Имя Fluent (`blue`, `darkGreen`) — так цвет хранится в таблице категорий на сервере.
    pub key: &'static str,
    pub shade30: Rgb,
    pub primary: Rgb,
    pub tint30: Rgb,
    pub tint40: Rgb,
}

const fn fluent(key: &'static str, shade30: Rgb, primary: Rgb, tint30: Rgb, tint40: Rgb) -> Color {
    Color { key, shade30, primary, tint30, tint40 }
}

impl Color {
    pub fn swatch(&self, theme: Theme) -> Swatch {
        match theme {
            Theme::Light => Swatch { dot: self.primary, text: self.shade30, bg: self.tint40, border: self.primary },
            Theme::Dark => Swatch { dot: self.primary, text: self.tint40, bg: self.shade30, border: self.tint30 },
        }
    }
}

/// Общие цвета Fluent 2 в порядке Fluent (статусные `red`, `marigold`, `berry` и цвета персон).
pub const PALETTE: &[Color] = &[
    fluent("red", 0x751D1F, 0xD13438, 0xE37D80, 0xF1BBBC),
    fluent("darkRed", 0x420610, 0x750B1C, 0xAC4F5E, 0xD69CA5),
    fluent("cranberry", 0x6E0811, 0xC50F1F, 0xDC626D, 0xEEACB2),
    fluent("pumpkin", 0x712D09, 0xCA5010, 0xDF8E64, 0xEFC4AD),
    fluent("peach", 0x8F4E00, 0xFF8C00, 0xFFBA66, 0xFFDDB3),
    fluent("marigold", 0x835B00, 0xEAA300, 0xF2C661, 0xF9E2AE),
    fluent("gold", 0x6C5700, 0xC19C00, 0xDAC157, 0xECDFA5),
    fluent("brass", 0x553E06, 0x986F0B, 0xC1A256, 0xE0CEA2),
    fluent("brown", 0x50301A, 0x8E562E, 0xBB8F6F, 0xDDC3B0),
    fluent("forest", 0x294903, 0x498205, 0x85B44C, 0xBDD99B),
    fluent("seafoam", 0x00723B, 0x00CC6A, 0x5AE0A0, 0xA8F0CD),
    fluent("darkGreen", 0x063B06, 0x0B6A0B, 0x4DA64D, 0x9AD29A),
    fluent("lightTeal", 0x00666D, 0x00B7C3, 0x58D3DB, 0xA6E9ED),
    fluent("teal", 0x02494C, 0x038387, 0x4CB4B7, 0x9BD9DB),
    fluent("steel", 0x00333F, 0x005B70, 0x4496A9, 0x94C8D4),
    fluent("blue", 0x004377, 0x0078D4, 0x5CAAE5, 0xA9D3F2),
    fluent("royalBlue", 0x002C4E, 0x004E8C, 0x4A89BA, 0x9ABFDC),
    fluent("cornflower", 0x2C3C85, 0x4F6BED, 0x93A4F4, 0xC8D1FA),
    fluent("navy", 0x001665, 0x0027B4, 0x546FD2, 0xA3B2E8),
    fluent("lavender", 0x3F3682, 0x7160E8, 0xA79CF1, 0xD2CCF8),
    fluent("purple", 0x341A51, 0x5C2E91, 0x9470BD, 0xC6B1DE),
    fluent("grape", 0x4C0D55, 0x881798, 0xB55FC1, 0xD9A7E0),
    fluent("berry", 0x6D2064, 0xC239B3, 0xDA7ED0, 0xEDBBE7),
    fluent("lilac", 0x63276D, 0xB146C2, 0xCF87DA, 0xE6BFED),
    fluent("pink", 0x80215D, 0xE43BA6, 0xEF85C8, 0xF7C0E3),
    fluent("magenta", 0x6B0043, 0xBF0077, 0xD957A8, 0xECA5D1),
    fluent("plum", 0x43002B, 0x77004D, 0xAD4589, 0xD696C0),
    fluent("beige", 0x444241, 0x7A7574, 0xAFABAA, 0xD7D4D4),
    fluent("mink", 0x343231, 0x5D5A58, 0x9E9B99, 0xCECCCB),
    fluent("platinum", 0x3B4447, 0x69797E, 0xA0ADB2, 0xCDD6D8),
    fluent("anchor", 0x202427, 0x394146, 0x808A90, 0xBCC3C7),
];

/// Цвета категорий по умолчанию (DESIGN.md «Цвет»): синий, зелёный, фиолетовый, бирюзовый, серый — подобраны так,
/// чтобы любые два и занятые цвета различались на ΔE ≥ 25.
pub const BLUE: &str = "blue";
pub const GREEN: &str = "darkGreen";
pub const VIOLET: &str = "grape";
pub const TEAL: &str = "lightTeal";
/// Нейтральный серый: метки Thunderbird вне таблицы, служебные метки, неизвестный цвет.
pub const NEUTRAL: &str = "mink";

/// Занятые цвета: в категориях их и близких к ним не бывает.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reserved {
    /// Красный — только безопасность.
    Threat,
    /// Янтарный — только сроки и подсветка совпадений поиска.
    Deadline,
}

impl Reserved {
    pub const ALL: [Reserved; 2] = [Reserved::Threat, Reserved::Deadline];

    /// Цвет Fluent, которым занят.
    pub fn key(self) -> &'static str {
        match self {
            Reserved::Threat => "red",
            Reserved::Deadline => "marigold",
        }
    }
}

/// Цвет по имени Fluent (регистр как у Fluent: `darkGreen`).
pub fn color(key: &str) -> Option<&'static Color> {
    PALETTE.iter().find(|c| c.key == key)
}

fn neutral() -> &'static Color {
    // NEUTRAL — из PALETTE (тест `defaults_are_in_the_palette`); запасной — последний цвет.
    color(NEUTRAL).unwrap_or(&PALETTE[PALETTE.len() - 1])
}

/// Имя цвета для отрисовки: неизвестное (таблица из новой версии, правка руками) — нейтральный.
pub fn resolve(key: &str) -> &'static str {
    color(key).map_or(NEUTRAL, |c| c.key)
}

/// Цвета роли в теме; неизвестный цвет — нейтральный.
pub fn swatch(key: &str, theme: Theme) -> Swatch {
    color(key).unwrap_or_else(neutral).swatch(theme)
}

/// Цвета занятого цвета (угроза, срок) в теме.
pub fn reserved(r: Reserved, theme: Theme) -> Swatch {
    swatch(r.key(), theme)
}

/// С чем цвет слишком близок.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clash {
    /// С цветом другой категории.
    Color(&'static str),
    /// С занятым цветом (или это он сам).
    Reserved(Reserved),
    /// Такого цвета в палитре нет.
    Unknown,
}

/// Наименьшая ΔE точек двух цветов по темам.
fn distance(a: &Color, b: &Color) -> f64 {
    Theme::ALL.iter().map(|&t| delta_e(a.swatch(t).dot, b.swatch(t).dot)).fold(f64::INFINITY, f64::min)
}

/// Ближайший занятый цвет, если он ближе [`MIN_DELTA_E`].
fn near_reserved(c: &Color) -> Option<(Reserved, f64)> {
    Reserved::ALL
        .iter()
        .filter_map(|&r| color(r.key()).map(|rc| (r, distance(c, rc))))
        .filter(|&(_, d)| d < MIN_DELTA_E)
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Цвета, которые можно дать категории: далёкие от красного и янтарного в обеих темах (порядок Fluent).
pub fn usable() -> &'static [&'static str] {
    static U: OnceLock<Vec<&'static str>> = OnceLock::new();
    U.get_or_init(|| PALETTE.iter().filter(|c| near_reserved(c).is_none()).map(|c| c.key).collect())
}

/// Можно ли дать категории цвет `key`, когда у других категорий таблицы цвета `others`: первое, к чему он ближе
/// [`MIN_DELTA_E`] в какой-либо теме, и разница; `None` — можно. Тот же цвет в `others` — разница 0.
pub fn clash<S: AsRef<str>>(key: &str, others: &[S]) -> Option<(Clash, f64)> {
    let Some(c) = color(key) else { return Some((Clash::Unknown, 0.0)) };
    if let Some((r, d)) = near_reserved(c) {
        return Some((Clash::Reserved(r), d));
    }
    others
        .iter()
        .filter_map(|o| color(o.as_ref()))
        .map(|o| (o, distance(c, o)))
        .filter(|&(_, d)| d < MIN_DELTA_E)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(o, d)| (Clash::Color(o.key), d))
}

/// Весь список: любые два цвета и занятые цвета различимы. Ошибка — первый цвет, который ближе [`MIN_DELTA_E`]
/// к одному из предыдущих (или к занятому).
pub fn check<S: AsRef<str>>(keys: &[S]) -> Result<(), (String, Clash, f64)> {
    for (i, k) in keys.iter().enumerate() {
        if let Some((c, d)) = clash(k.as_ref(), &keys[..i]) {
            return Err((k.as_ref().to_owned(), c, d));
        }
    }
    Ok(())
}

// --- преобразования цвета ---

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// OKLCH → линейный sRGB (Björn Ottosson, 2020).
fn oklch_to_linear(o: Oklch) -> [f64; 3] {
    let (a, b) = (o.c * o.h.to_radians().cos(), o.c * o.h.to_radians().sin());
    let l = (o.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (o.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (o.l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    ]
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.iter().all(|c| (-1e-6..=1.0 + 1e-6).contains(c))
}

/// OKLCH → sRGB. Вне охвата sRGB — насыщенность уменьшается (L и оттенок те же), пока цвет не войдёт.
pub fn oklch_to_rgb(o: Oklch) -> Rgb {
    let mut rgb = oklch_to_linear(o);
    if !in_gamut(rgb) {
        let (mut lo, mut hi) = (0.0, o.c);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(oklch_to_linear(Oklch { c: mid, ..o })) { lo = mid } else { hi = mid }
        }
        rgb = oklch_to_linear(Oklch { c: lo, ..o });
    }
    let byte = |c: f64| (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u32;
    (byte(rgb[0]) << 16) | (byte(rgb[1]) << 8) | byte(rgb[2])
}

/// sRGB → OKLCH (для проверок и настроек).
pub fn rgb_to_oklch(rgb: Rgb) -> Oklch {
    let [r, g, b] = linear(rgb);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let ll = 0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s;
    Oklch { l: ll, c: a.hypot(bb), h: bb.atan2(a).to_degrees().rem_euclid(360.0) }
}

fn linear(rgb: Rgb) -> [f64; 3] {
    [16, 8, 0].map(|shift| srgb_to_linear(f64::from((rgb >> shift) & 0xff) / 255.0))
}

/// CIELAB (D65).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

pub fn rgb_to_lab(rgb: Rgb) -> Lab {
    let [r, g, b] = linear(rgb);
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    Lab { l: 116.0 * fy - 16.0, a: 500.0 * (fx - fy), b: 200.0 * (fy - fz) }
}

/// Разница цветов CIEDE2000 (Sharma, Wu, Dalal, 2005).
pub fn delta_e2000(p: Lab, q: Lab) -> f64 {
    let c_bar = (p.a.hypot(p.b) + q.a.hypot(q.b)) / 2.0;
    let pow7 = |x: f64| x.powi(7);
    let g = 0.5 * (1.0 - (pow7(c_bar) / (pow7(c_bar) + pow7(25.0))).sqrt());
    let (a1, a2) = ((1.0 + g) * p.a, (1.0 + g) * q.a);
    let (c1, c2) = (a1.hypot(p.b), a2.hypot(q.b));
    let angle = |b: f64, a: f64| if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let (h1, h2) = (angle(p.b, a1), angle(q.b, a2));
    let dl = q.l - p.l;
    let dc = c2 - c1;
    let dh = if c1 * c2 == 0.0 {
        0.0
    } else if (h2 - h1).abs() <= 180.0 {
        h2 - h1
    } else if h2 <= h1 {
        h2 - h1 + 360.0
    } else {
        h2 - h1 - 360.0
    };
    let dh_big = 2.0 * (c1 * c2).sqrt() * (dh / 2.0).to_radians().sin();
    let l_bar = (p.l + q.l) / 2.0;
    let c_bar2 = (c1 + c2) / 2.0;
    let h_bar = if c1 * c2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let cos = |deg: f64| deg.to_radians().cos();
    let t = 1.0 - 0.17 * cos(h_bar - 30.0) + 0.24 * cos(2.0 * h_bar) + 0.32 * cos(3.0 * h_bar + 6.0)
        - 0.20 * cos(4.0 * h_bar - 63.0);
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (pow7(c_bar2) / (pow7(c_bar2) + pow7(25.0))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar2;
    let sh = 1.0 + 0.015 * c_bar2 * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh_big / sh).powi(2) + rt * (dc / sc) * (dh_big / sh)).sqrt()
}

/// ΔE2000 между двумя цветами sRGB.
pub fn delta_e(a: Rgb, b: Rgb) -> f64 {
    delta_e2000(rgb_to_lab(a), rgb_to_lab(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Контрольные пары из статьи Sharma, Wu, Dalal (таблица 1).
    #[test]
    fn ciede2000_matches_the_reference_data() {
        let lab = |l, a, b| Lab { l, a, b };
        let cases = [
            (lab(50.0, 2.6772, -79.7751), lab(50.0, 0.0, -82.7485), 2.0425),
            (lab(50.0, -1.3802, -84.2814), lab(50.0, 0.0, -82.7485), 1.0000),
            (lab(50.0, 2.5, 0.0), lab(73.0, 25.0, -18.0), 27.1492),
            (lab(50.0, 2.5, 0.0), lab(50.0, 0.0, -2.5), 4.3065),
            (lab(60.2574, -34.0099, 36.2677), lab(60.4626, -34.1751, 39.4387), 1.2644),
            (lab(22.7233, 20.0904, -46.6940), lab(23.0331, 14.9730, -42.5619), 2.0373),
            (lab(2.0776, 0.0795, -1.1350), lab(0.9033, -0.0636, -0.5514), 0.9082),
        ];
        for (p, q, want) in cases {
            let got = delta_e2000(p, q);
            assert!((got - want).abs() < 1e-4, "{p:?} {q:?}: {got} != {want}");
            assert!((delta_e2000(q, p) - want).abs() < 1e-4, "симметрия");
        }
    }

    #[test]
    fn oklch_round_trips_through_srgb() {
        for rgb in [0x0078D4, 0x0B6A0B, 0x881798, 0x00B7C3, 0x5D5A58, 0x000000, 0xFFFFFF] {
            assert_eq!(oklch_to_rgb(rgb_to_oklch(rgb)), rgb, "{rgb:06X}");
        }
        // Белый и чёрный — L 1 и 0 без насыщенности.
        let w = rgb_to_oklch(0xFFFFFF);
        assert!((w.l - 1.0).abs() < 1e-3 && w.c < 1e-3);
    }

    /// Все 31 общий цвет Fluent 2, имена без повторов; роли — токены Fluent (светлая и тёмная тема).
    #[test]
    fn palette_is_fluent_shared_colors() {
        assert_eq!(PALETTE.len(), 31);
        for (i, c) in PALETTE.iter().enumerate() {
            assert!(PALETTE[..i].iter().all(|o| o.key != c.key), "{} дважды", c.key);
        }
        let blue = swatch(BLUE, Theme::Light);
        assert_eq!(blue, Swatch { dot: 0x0078D4, text: 0x004377, bg: 0xA9D3F2, border: 0x0078D4 });
        let dark = swatch(BLUE, Theme::Dark);
        assert_eq!(dark, Swatch { dot: 0x0078D4, text: 0xA9D3F2, bg: 0x004377, border: 0x5CAAE5 });
        // Неизвестный цвет — нейтральный.
        assert_eq!(swatch("chartreuse", Theme::Dark), swatch(NEUTRAL, Theme::Dark));
        assert_eq!(resolve("chartreuse"), NEUTRAL);
        assert_eq!(resolve("teal"), "teal");
        assert_eq!(reserved(Reserved::Threat, Theme::Light).dot, 0xD13438);
        assert_eq!(reserved(Reserved::Deadline, Theme::Light).dot, 0xEAA300);
    }

    #[test]
    fn defaults_are_in_the_palette() {
        for key in [BLUE, GREEN, VIOLET, TEAL, NEUTRAL] {
            assert!(color(key).is_some() && usable().contains(&key), "{key}");
        }
    }

    /// Выбор цвета категории: занятые красный и янтарный и близкие к ним выпадают, остальные дальше ΔE 25 от
    /// обоих в обеих темах.
    #[test]
    fn usable_colors_are_far_from_reserved() {
        let usable = usable();
        assert_eq!(usable.len(), 21, "{usable:?}");
        for gone in ["red", "darkRed", "cranberry", "pumpkin", "magenta", "marigold", "peach", "gold", "brass", "brown"]
        {
            assert!(!usable.contains(&gone), "{gone}");
        }
        for key in usable {
            for r in Reserved::ALL {
                for theme in Theme::ALL {
                    let d = delta_e(swatch(key, theme).dot, reserved(r, theme).dot);
                    assert!(d >= MIN_DELTA_E, "{key} — {r:?} {theme:?}: ΔE {d:.1}");
                }
            }
        }
        assert!(matches!(clash("red", &[] as &[&str]), Some((Clash::Reserved(Reserved::Threat), _))));
        assert!(matches!(clash("gold", &[] as &[&str]), Some((Clash::Reserved(Reserved::Deadline), _))));
        assert!(matches!(clash("pumpkin", &[] as &[&str]), Some((Clash::Reserved(Reserved::Threat), _))));
        assert_eq!(clash("chartreuse", &[] as &[&str]), Some((Clash::Unknown, 0.0)));
    }

    /// Категории по умолчанию (DESIGN.md: синий, зелёный, фиолетовый, бирюзовый, серый) различимы между собой и
    /// с красным и янтарным в обеих темах.
    #[test]
    fn default_categories_are_far_apart() {
        let defaults = [BLUE, GREEN, VIOLET, TEAL, NEUTRAL];
        assert_eq!(check(&defaults), Ok(()));
        for theme in Theme::ALL {
            let mut dots: Vec<Rgb> = defaults.iter().map(|&s| swatch(s, theme).dot).collect();
            dots.extend(Reserved::ALL.map(|r| reserved(r, theme).dot));
            for (i, a) in dots.iter().enumerate() {
                for b in &dots[i + 1..] {
                    let d = delta_e(*a, *b);
                    assert!(d >= MIN_DELTA_E, "{theme:?}: {a:06X} {b:06X} ΔE {d:.1}");
                }
            }
        }
    }

    /// Запрет близких цветов: к пяти категориям по умолчанию близкий цвет не добавить, повтор — тоже.
    #[test]
    fn close_colors_are_rejected() {
        let used = [BLUE, GREEN, VIOLET, TEAL, NEUTRAL];
        // Голубой рядом с синим, лес рядом с зелёным, сирень рядом с виноградом, платина — с серым и синим.
        assert!(matches!(clash("royalBlue", &used), Some((Clash::Color(BLUE), _))));
        assert!(matches!(clash("forest", &used), Some((Clash::Color(GREEN), _))));
        assert!(matches!(clash("lilac", &used), Some((Clash::Color(VIOLET), _))));
        assert!(clash("platinum", &used).is_some());
        assert!(matches!(clash(BLUE, &[BLUE]), Some((Clash::Color(BLUE), d)) if d == 0.0));
        // Без синего — можно.
        assert_eq!(clash("royalBlue", &[GREEN]), None);
        assert!(check(&[BLUE, BLUE]).is_err());
        assert!(check(&[BLUE, "steel"]).is_err());
        // Хотя бы одну шестую категорию к пяти по умолчанию добавить можно.
        let sixth: Vec<&str> = usable().iter().copied().filter(|k| clash(k, &used).is_none()).collect();
        assert!(!sixth.is_empty(), "шестой цвет не добавить");
    }

    /// Текст метки читается на её подложке (контраст WCAG ≥ 4,5) в обеих темах — у всех цветов палитры.
    #[test]
    fn chip_text_is_readable() {
        let lum = |rgb: Rgb| {
            let [r, g, b] = linear(rgb);
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        for c in PALETTE {
            for theme in Theme::ALL {
                let s = c.swatch(theme);
                let (a, b) = (lum(s.text), lum(s.bg));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 4.5, "{} {theme:?}: {ratio:.2}", c.key);
            }
        }
    }
}

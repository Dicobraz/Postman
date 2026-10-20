//! Палитра категорий и меток (S5-02, `docs/DESIGN.md` «Цвет»): цвета считаются из OKLCH, а не подбираются руками.
//!
//! Слот палитры — оттенок (угол OKLCH) или нейтральный серый; из него для темы получаются три цвета: точка и
//! полоса (L 0,62 C 0,14), подложка метки (L 0,95) и текст метки того же оттенка (L 0,42). Тёмная тема — свои
//! L и C («яркие цвета приглушены»). Цвет вне sRGB приводится уменьшением насыщенности при том же L и оттенке.
//!
//! В одном списке нет близких цветов: ΔE2000 между точками любых двух категорий и до занятых цветов (красный —
//! безопасность, янтарный — сроки) не меньше [`MIN_DELTA_E`] в обеих темах ([`clash`], [`check`]).

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

/// Три цвета слота в теме.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swatch {
    /// Точка у фильтра и полоса слева в строке письма.
    pub dot: Rgb,
    /// Текст метки.
    pub text: Rgb,
    /// Подложка метки.
    pub bg: Rgb,
}

/// Оттенок слота; `h: None` — нейтральный серый.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hue {
    /// Имя для журнала и настроек (не текст интерфейса).
    pub key: &'static str,
    pub h: Option<f64>,
}

const fn hue(key: &'static str, h: f64) -> Hue {
    Hue { key, h: Some(h) }
}

/// Слоты палитры. Номер слота хранится в таблице категорий — порядок не менять, новые — в конец.
///
/// При одних L и C на круге помещается немного различимых оттенков: синий, зелёный, фиолетовый, бирюзовый и серый
/// по умолчанию плюс красный и янтарный (заняты) уже занимают его целиком — индиго, голубой и изумрудный доступны,
/// только если убрать соседнюю категорию. Оттенки рядом с красным и янтарным (оранжевый, оливковый, малиновый)
/// не берутся вовсе.
pub const HUES: &[Hue] = &[
    Hue { key: "neutral", h: None },
    hue("blue", 255.0),
    hue("green", 135.0),
    hue("violet", 315.0),
    hue("teal", 195.0),
    hue("indigo", 285.0),
    hue("sky", 225.0),
    hue("emerald", 160.0),
];

pub const NEUTRAL: u8 = 0;
pub const BLUE: u8 = 1;
pub const GREEN: u8 = 2;
pub const VIOLET: u8 = 3;
pub const TEAL: u8 = 4;

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

    fn hue(self) -> f64 {
        match self {
            Reserved::Threat => 25.0,
            Reserved::Deadline => 80.0,
        }
    }
}

/// Светлота и насыщенность роли.
#[derive(Clone, Copy)]
struct Tone {
    l: f64,
    c: f64,
}

#[derive(Clone, Copy)]
struct Tones {
    dot: Tone,
    text: Tone,
    bg: Tone,
}

const fn tones(dot: (f64, f64), text: (f64, f64), bg: (f64, f64)) -> Tones {
    Tones { dot: Tone { l: dot.0, c: dot.1 }, text: Tone { l: text.0, c: text.1 }, bg: Tone { l: bg.0, c: bg.1 } }
}

/// Цветные слоты: светлая тема — по DESIGN.md; тёмная — точки светлее и спокойнее, текст светлый, подложка тёмная.
const COLOR: [Tones; 2] =
    [tones((0.62, 0.14), (0.42, 0.10), (0.95, 0.035)), tones((0.68, 0.13), (0.85, 0.07), (0.33, 0.05))];
/// Нейтральный серый: точка светлее (светлая тема) или темнее (тёмная) цветных — иначе он ближе ΔE 25 к ним.
const GRAY: [Tones; 2] = [tones((0.78, 0.0), (0.38, 0.0), (0.96, 0.0)), tones((0.52, 0.0), (0.80, 0.0), (0.35, 0.0))];

fn theme_index(t: Theme) -> usize {
    match t {
        Theme::Light => 0,
        Theme::Dark => 1,
    }
}

fn make(h: Option<f64>, t: Theme) -> Swatch {
    let i = theme_index(t);
    let (set, h) = match h {
        Some(h) => (COLOR[i], h),
        None => (GRAY[i], 0.0),
    };
    let one = |tone: Tone| oklch_to_rgb(Oklch { l: tone.l, c: tone.c, h });
    Swatch { dot: one(set.dot), text: one(set.text), bg: one(set.bg) }
}

struct Table {
    slots: Vec<[Swatch; 2]>,
    reserved: [[Swatch; 2]; 2],
}

/// Цвета считаются один раз на процесс.
fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| Table {
        slots: HUES.iter().map(|h| Theme::ALL.map(|t| make(h.h, t))).collect(),
        reserved: Reserved::ALL.map(|r| Theme::ALL.map(|t| make(Some(r.hue()), t))),
    })
}

/// Цвета слота в теме; неизвестный слот (таблица из новой версии) — нейтральный.
pub fn swatch(slot: u8, theme: Theme) -> Swatch {
    let t = table();
    let row = t.slots.get(usize::from(slot)).unwrap_or(&t.slots[0]);
    row[theme_index(theme)]
}

/// Цвета занятого цвета (угроза, срок) в теме.
pub fn reserved(r: Reserved, theme: Theme) -> Swatch {
    let i = match r {
        Reserved::Threat => 0,
        Reserved::Deadline => 1,
    };
    table().reserved[i][theme_index(theme)]
}

/// С чем цвет слишком близок.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clash {
    Slot(u8),
    Reserved(Reserved),
}

/// Можно ли взять слот `slot` в список, где уже заняты `used`: первое, к чему он ближе [`MIN_DELTA_E`] в какой-либо
/// теме, и разница; `None` — можно. Сам `slot` в `used` не считается.
pub fn clash(slot: u8, used: &[u8]) -> Option<(Clash, f64)> {
    for theme in Theme::ALL {
        let dot = swatch(slot, theme).dot;
        for r in Reserved::ALL {
            let d = delta_e(dot, reserved(r, theme).dot);
            if d < MIN_DELTA_E {
                return Some((Clash::Reserved(r), d));
            }
        }
        for &other in used.iter().filter(|&&u| u != slot) {
            let d = delta_e(dot, swatch(other, theme).dot);
            if d < MIN_DELTA_E {
                return Some((Clash::Slot(other), d));
            }
        }
    }
    None
}

/// Весь список: у одного слота — одна категория, любые два и занятые цвета различимы.
pub fn check(slots: &[u8]) -> Result<(), (u8, Clash, f64)> {
    for (i, &a) in slots.iter().enumerate() {
        if slots[..i].contains(&a) {
            return Err((a, Clash::Slot(a), 0.0));
        }
        // Занятые цвета — и для первого слота (`used` пуст).
        if let Some((c, d)) = clash(a, &slots[..i]) {
            return Err((a, c, d));
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
        for rgb in [0x4687D8, 0x5F983A, 0xA569BF, 0x009F99, 0xB7B7B7, 0x000000, 0xFFFFFF] {
            assert_eq!(oklch_to_rgb(rgb_to_oklch(rgb)), rgb, "{rgb:06X}");
        }
        // Белый и чёрный — L 1 и 0 без насыщенности.
        let w = rgb_to_oklch(0xFFFFFF);
        assert!((w.l - 1.0).abs() < 1e-3 && w.c < 1e-3);
    }

    /// Точки — L 0,62 C 0,14 (DESIGN.md); бирюзовый в sRGB не помещается — насыщенность меньше, L и оттенок те же.
    #[test]
    fn dots_follow_the_design_tones() {
        for slot in [BLUE, GREEN, VIOLET] {
            let o = rgb_to_oklch(swatch(slot, Theme::Light).dot);
            assert!((o.l - 0.62).abs() < 0.01 && (o.c - 0.14).abs() < 0.01, "{slot}: {o:?}");
        }
        let teal = rgb_to_oklch(swatch(TEAL, Theme::Light).dot);
        assert!((teal.l - 0.62).abs() < 0.01 && teal.c < 0.14 && (teal.h - 195.0).abs() < 2.0, "{teal:?}");
        let s = swatch(BLUE, Theme::Light);
        let (bg, text) = (rgb_to_oklch(s.bg), rgb_to_oklch(s.text));
        assert!((bg.l - 0.95).abs() < 0.01 && (text.l - 0.42).abs() < 0.01, "{bg:?} {text:?}");
        // Цвета вычислены, но совпадают с макетом владельца с точностью до единиц ΔE.
        assert!(delta_e(swatch(BLUE, Theme::Light).dot, 0x4687D8) < 1.0);
        assert!(delta_e(swatch(VIOLET, Theme::Light).dot, 0xA569BF) < 1.0);
        let gray = rgb_to_oklch(swatch(NEUTRAL, Theme::Light).dot);
        assert!(gray.c < 1e-3);
        // Неизвестный слот — нейтральный.
        assert_eq!(swatch(200, Theme::Dark), swatch(NEUTRAL, Theme::Dark));
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

    /// Запрет близких цветов: к пяти категориям по умолчанию близкие оттенки не добавить, повтор — тоже.
    #[test]
    fn close_colors_are_rejected() {
        let used = [BLUE, GREEN, VIOLET, TEAL, NEUTRAL];
        // Индиго между синим и фиолетовым, изумрудный между зелёным и бирюзовым, голубой — у синего.
        assert!(matches!(clash(5, &used), Some((Clash::Slot(BLUE | VIOLET), _))));
        assert!(matches!(clash(7, &used), Some((Clash::Slot(GREEN | TEAL), _))));
        assert!(matches!(clash(6, &used), Some((Clash::Slot(BLUE | TEAL | NEUTRAL), _))));
        // Без синего голубой можно.
        assert_eq!(clash(6, &[GREEN, VIOLET, NEUTRAL]), None);
        assert!(check(&[BLUE, BLUE]).is_err());
        assert!(check(&[BLUE, 5]).is_err());
        // Любой слот палитры в одиночку не близок к красному и янтарному.
        for s in 0..HUES.len() as u8 {
            assert_eq!(clash(s, &[]), None, "{}", HUES[usize::from(s)].key);
        }
        // Оранжевый (не в палитре) — близок к занятым.
        let orange = oklch_to_rgb(Oklch { l: 0.62, c: 0.14, h: 50.0 });
        assert!(Reserved::ALL.iter().any(|&r| delta_e(orange, reserved(r, Theme::Light).dot) < MIN_DELTA_E));
    }

    /// Текст метки читается на её подложке (контраст WCAG ≥ 4,5) в обеих темах.
    #[test]
    fn chip_text_is_readable() {
        let lum = |rgb: Rgb| {
            let [r, g, b] = linear(rgb);
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        for slot in 0..HUES.len() as u8 {
            for theme in Theme::ALL {
                let s = swatch(slot, theme);
                let (a, b) = (lum(s.text), lum(s.bg));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 4.5, "{slot} {theme:?}: {ratio:.2}");
            }
        }
    }
}

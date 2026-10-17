//! Не CSS-парсер: из `style=""` берутся только начертание и выравнивание.

use crate::{Align, Marks};

pub(crate) fn apply(style: &str, marks: &mut Marks, align: &mut Align) {
    for decl in style.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
        match name.as_str() {
            "font-weight" => {
                marks.bold = match value.as_str() {
                    "bold" | "bolder" => true,
                    "normal" | "lighter" => false,
                    v => v.parse::<u16>().map(|w| w >= 600).unwrap_or(marks.bold),
                }
            }
            "font-style" => match value.as_str() {
                "italic" | "oblique" => marks.italic = true,
                "normal" => marks.italic = false,
                _ => {}
            },
            "text-decoration" | "text-decoration-line" => {
                if value.contains("none") {
                    marks.underline = false;
                    marks.strike = false;
                }
                if value.contains("underline") {
                    marks.underline = true;
                }
                if value.contains("line-through") {
                    marks.strike = true;
                }
            }
            "text-align" => {
                if let Some(a) = parse_align(&value) {
                    *align = a;
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn parse_align(v: &str) -> Option<Align> {
    Some(match v.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Align::Start,
        "center" | "middle" => Align::Center,
        "right" | "end" => Align::End,
        "justify" => Align::Justify,
        _ => return None,
    })
}

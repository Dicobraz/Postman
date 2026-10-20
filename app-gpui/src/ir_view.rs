//! IR письма → элементы gpui. Абзац — один StyledText с выделениями:
//! перенос строк, выравнивание и настоящий полужирный — силами текстового движка,
//! ссылки — кликабельные диапазоны того же текста.

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use postman_ir::{Align, Block, Inline, Marks};

pub fn blocks(bs: &[Block], key: &str, cx: &mut App) -> Vec<AnyElement> {
    bs.iter().enumerate().map(|(n, b)| block(b, &format!("{key}.{n}"), cx)).collect()
}

fn block(b: &Block, key: &str, cx: &mut App) -> AnyElement {
    match b {
        Block::Paragraph { align, content } => {
            let d = div().child(paragraph(content, key, None));
            match align {
                Align::Center => d.text_center(),
                Align::End => d.text_right(),
                _ => d,
            }
            .into_any_element()
        }
        Block::Heading { level, content } => {
            let size = match level {
                1 => 22.,
                2 => 19.,
                3 => 17.,
                _ => 15.,
            };
            div().text_size(px(size)).font_weight(FontWeight::SEMIBOLD).child(paragraph(content, key, None)).into_any_element()
        }
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(rgb(0xbbbbbb))
            .text_color(rgb(0x555555))
            .children(blocks(inner, key, cx))
            .into_any_element(),
        Block::List { ordered, items } => div()
            .flex()
            .flex_col()
            .gap_1()
            .children(items.iter().enumerate().map(|(n, item)| {
                let marker = if *ordered { format!("{}.", n + 1) } else { "•".into() };
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(16.)).child(marker))
                    .child(div().flex().flex_col().flex_1().children(blocks(item, &format!("{key}.{n}"), cx)))
            }))
            .into_any_element(),
        Block::Code(code) => div().p_2().bg(rgb(0xf0f0f0)).font_family("monospace").child(code.clone()).into_any_element(),
        Block::Rule => div().h(px(1.)).bg(rgb(0xdddddd)).into_any_element(),
        Block::Table(rows) => div()
            .flex()
            .flex_col()
            .children(rows.iter().enumerate().map(|(r, row)| {
                div()
                    .flex()
                    .when(r % 2 == 1, |d| d.bg(rgb(0xf3f3f3)))
                    .children(row.iter().enumerate().map(|(c, cell)| {
                        div().min_w(px(90.)).px_2().py_1().child(paragraph(cell, &format!("{key}.{r}.{c}"), None))
                    }))
            }))
            .into_any_element(),
    }
}

/// Инлайны → (текст, выделения, ссылки).
fn flatten(items: &[Inline], text: &mut String, hl: &mut Vec<(std::ops::Range<usize>, HighlightStyle)>, links: &mut Vec<(std::ops::Range<usize>, String, bool)>) {
    for i in items {
        match i {
            Inline::Text { text: t, marks } => {
                let start = text.len();
                text.push_str(t);
                if let Some(s) = style(*marks) {
                    hl.push((start..text.len(), s));
                }
            }
            Inline::LineBreak => text.push('\n'),
            Inline::Image { alt, .. } => {
                let start = text.len();
                text.push_str(&if alt.is_empty() { tr!("reading-image") } else { tr!("reading-image-alt", alt = alt.as_str()) });
                hl.push((start..text.len(), HighlightStyle { color: Some(rgb(0x888888).into()), ..Default::default() }));
            }
            Inline::Link { href, suspicious, content, .. } => {
                if *suspicious {
                    let s = text.len();
                    text.push('⚠');
                    hl.push((s..text.len(), HighlightStyle { color: Some(rgb(0xcc2222).into()), ..Default::default() }));
                }
                let start = text.len();
                flatten(content, text, hl, links);
                let range = start..text.len();
                hl.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(rgb(0x1a6fd1).into()),
                        underline: Some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
                        ..Default::default()
                    },
                ));
                links.push((range, href.clone(), *suspicious));
            }
        }
    }
}

fn style(m: Marks) -> Option<HighlightStyle> {
    if m == Marks::default() {
        return None;
    }
    Some(HighlightStyle {
        font_weight: m.bold.then_some(FontWeight::BOLD),
        font_style: m.italic.then_some(FontStyle::Italic),
        underline: m.underline.then_some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
        strikethrough: m.strike.then_some(StrikethroughStyle { thickness: px(1.), color: None }),
        background_color: m.code.then(|| rgb(0xeeeeee).into()),
        ..Default::default()
    })
}

fn paragraph(items: &[Inline], key: &str, _size: Option<f32>) -> AnyElement {
    let (mut text, mut hl, mut links) = (String::new(), Vec::new(), Vec::new());
    flatten(items, &mut text, &mut hl, &mut links);
    let styled = StyledText::new(text).with_highlights(normalize(hl));
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<_> = links.iter().map(|(r, _, _)| r.clone()).collect();
    let hrefs: Vec<String> = links.into_iter().map(|(_, h, _)| h).collect();
    InteractiveText::new(SharedString::from(key.to_string()), styled)
        .on_click(ranges, move |ix, _window, cx| {
            if let Some(h) = hrefs.get(ix) {
                cx.open_url(h);
            }
        })
        .into_any_element()
}

/// gpui требует непересекающиеся выделения по возрастанию: ссылка поверх жирного и т.п. сливаем.
fn normalize(mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)>) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut bounds: Vec<usize> = hl.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut out = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut merged: Option<HighlightStyle> = None;
        for (r, s) in &hl {
            if r.start <= a && b <= r.end {
                merged = Some(match merged {
                    None => *s,
                    Some(m) => m.highlight(*s),
                });
            }
        }
        if let Some(s) = merged {
            out.push((a..b, s));
        }
    }
    hl.clear();
    out
}

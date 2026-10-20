//! IR письма → элементы gpui. Абзац — один StyledText с выделениями:
//! перенос строк, выравнивание и настоящий полужирный — силами текстового движка,
//! ссылки — кликабельные диапазоны того же текста. Картинки `cid:` (уже декодированные воркером) — отдельными
//! элементами между кусками текста абзаца; пока картинки нет — её подпись.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use postman_ir::{Align, Block, Document, Inline, Marks};

/// Картинки письма по `cid:`, кадры для gpui.
pub type Images = HashMap<String, Arc<RenderImage>>;

pub fn blocks(bs: &[Block], key: &str, images: &Images, cx: &mut App) -> Vec<AnyElement> {
    bs.iter().enumerate().map(|(n, b)| block(b, &format!("{key}.{n}"), images, cx)).collect()
}

/// Все `cid:` картинок документа, по порядку.
pub fn cids(doc: &Document) -> Vec<String> {
    fn inl(items: &[Inline], out: &mut Vec<String>) {
        for i in items {
            match i {
                Inline::Image { cid, .. } => out.push(cid.clone()),
                Inline::Link { content, .. } => inl(content, out),
                _ => {}
            }
        }
    }
    fn blk(bs: &[Block], out: &mut Vec<String>) {
        for b in bs {
            match b {
                Block::Paragraph { content, .. } | Block::Heading { content, .. } => inl(content, out),
                Block::Quote(inner) => blk(inner, out),
                Block::List { items, .. } => items.iter().for_each(|i| blk(i, out)),
                Block::Table(rows) => rows.iter().flatten().for_each(|c| inl(c, out)),
                Block::Code(_) | Block::Rule => {}
            }
        }
    }
    let mut out = Vec::new();
    blk(&doc.blocks, &mut out);
    out
}

fn block(b: &Block, key: &str, images: &Images, cx: &mut App) -> AnyElement {
    match b {
        Block::Paragraph { align, content } => {
            let d = div().child(paragraph(content, key, images));
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
            div().text_size(px(size)).font_weight(FontWeight::SEMIBOLD).child(paragraph(content, key, images)).into_any_element()
        }
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(rgb(0xbbbbbb))
            .text_color(rgb(0x555555))
            .children(blocks(inner, key, images, cx))
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
                    .child(div().flex().flex_col().flex_1().children(blocks(item, &format!("{key}.{n}"), images, cx)))
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
                        div().min_w(px(90.)).px_2().py_1().child(paragraph(cell, &format!("{key}.{r}.{c}"), images))
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

/// Куски абзаца: текст и показанные картинки верхнего уровня (в ссылке картинка остаётся подписью).
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a [Inline]),
    Image(&'a str),
}

fn pieces<'a>(items: &'a [Inline], shown: impl Fn(&str) -> bool) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, item) in items.iter().enumerate() {
        if let Inline::Image { cid, .. } = item
            && shown(cid)
        {
            out.push(Piece::Text(&items[start..i]));
            out.push(Piece::Image(cid));
            start = i + 1;
        }
    }
    out.push(Piece::Text(&items[start..]));
    // Пустые куски и одни переводы строк вокруг картинки не дают пустых строк.
    out.retain(|p| match p {
        Piece::Text(t) => t.iter().any(|i| match i {
            Inline::Text { text, .. } => !text.trim().is_empty(),
            Inline::LineBreak => false,
            _ => true,
        }),
        Piece::Image(_) => true,
    });
    out
}

fn paragraph(items: &[Inline], key: &str, images: &Images) -> AnyElement {
    let parts = pieces(items, |cid| images.contains_key(cid));
    if !parts.iter().any(|p| matches!(p, Piece::Image(_))) {
        return text(items, key);
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(parts.into_iter().enumerate().map(|(n, p)| match p {
            Piece::Text(t) => text(t, &format!("{key}.{n}")),
            Piece::Image(cid) => match images.get(cid) {
                Some(image) => picture(image),
                None => div().into_any_element(),
            },
        }))
        .into_any_element()
}

/// Картинка в своём размере, не шире области чтения.
fn picture(image: &Arc<RenderImage>) -> AnyElement {
    let size = image.size(0);
    img(image.clone())
        .w(px(size.width.0 as f32))
        .h(px(size.height.0 as f32))
        .max_w_full()
        .object_fit(ObjectFit::ScaleDown)
        .into_any_element()
}

fn text(items: &[Inline], key: &str) -> AnyElement {
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

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{Piece, cids, pieces};
    use postman_ir::{Align, Block, Document, Inline};

    fn image(cid: &str) -> Inline {
        Inline::Image { cid: cid.into(), alt: String::new() }
    }

    #[test]
    fn pictures_split_the_paragraph() {
        let items = [image("logo"), Inline::LineBreak, Inline::text("Коллеги"), image("missing"), image("chart")];
        let p = pieces(&items, |c| c != "missing");
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[0], Piece::Image("logo"));
        // Текст и подпись картинки, которой нет, — одним куском.
        assert_eq!(p[1], Piece::Text(&items[1..4]));
        assert_eq!(p[2], Piece::Image("chart"));
        assert_eq!(pieces(&items[1..3], |_| false), [Piece::Text(&items[1..3])]);
    }

    #[test]
    fn cids_in_reading_order() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph { align: Align::Start, content: vec![image("a")] },
                Block::Quote(vec![Block::Paragraph {
                    align: Align::Start,
                    content: vec![Inline::Link { href: "x".into(), host: None, suspicious: false, content: vec![image("b")] }],
                }]),
                Block::Table(vec![vec![vec![image("c")]]]),
            ],
            remote_images_blocked: 0,
            history: None,
        };
        assert_eq!(cids(&doc), ["a", "b", "c"]);
    }
}

//! Отрисовка IR письма в egui. Никаких сетевых обращений: ссылки открываются
//! только по клику, картинки `cid:` — заглушки до декодирования в воркере.

use eframe::egui::{self, Color32, RichText, Ui};
use postman_ir::{Block, Document, Inline, Marks};

/// Возвращает ссылку, по которой кликнули.
pub fn show(ui: &mut Ui, doc: &Document) -> Option<String> {
    let mut clicked = None;
    blocks(ui, &doc.blocks, &mut clicked, 0);
    clicked
}

fn blocks(ui: &mut Ui, bs: &[Block], clicked: &mut Option<String>, depth: usize) {
    for (n, b) in bs.iter().enumerate() {
        if n > 0 {
            ui.add_space(6.0);
        }
        block(ui, b, clicked, depth);
    }
}

fn block(ui: &mut Ui, b: &Block, clicked: &mut Option<String>, depth: usize) {
    match b {
        // TODO: выравнивание (align) — после перехода абзацев на LayoutJob с hit-test ссылок.
        Block::Paragraph { content, .. } => {
            ui.horizontal_wrapped(|ui| inlines(ui, content, None, clicked));
        }
        Block::Heading { level, content } => {
            let size = match level {
                1 => 22.0,
                2 => 19.0,
                3 => 17.0,
                _ => 15.0,
            };
            ui.horizontal_wrapped(|ui| inlines(ui, content, Some(size), clicked));
        }
        Block::Quote(inner) => {
            let r = ui.indent(("quote", depth), |ui| blocks(ui, inner, clicked, depth + 1));
            let rect = r.response.rect;
            let x = rect.left() + 3.0;
            ui.painter().vline(x, rect.y_range(), (2.0, ui.visuals().weak_text_color()));
        }
        Block::List { ordered, items } => {
            for (n, item) in items.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let marker = if *ordered { format!("{}.", n + 1) } else { "•".into() };
                    ui.add_sized([18.0, 14.0], egui::Label::new(marker));
                    ui.vertical(|ui| blocks(ui, item, clicked, depth + 1));
                });
            }
        }
        Block::Code(code) => {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(6.0)
                .show(ui, |ui| ui.label(RichText::new(code).monospace()));
        }
        Block::Rule => {
            ui.separator();
        }
        Block::Table(rows) => {
            egui::Grid::new(("table", ui.next_auto_id())).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for row in rows {
                    for cell in row {
                        ui.horizontal(|ui| inlines(ui, cell, None, clicked));
                    }
                    ui.end_row();
                }
            });
        }
    }
}

fn inlines(ui: &mut Ui, items: &[Inline], size: Option<f32>, clicked: &mut Option<String>) {
    ui.spacing_mut().item_spacing.x = 0.0;
    for i in items {
        match i {
            Inline::Text { text, marks } => {
                ui.label(rich(text, *marks, size));
            }
            Inline::LineBreak => ui.end_row(),
            Inline::Image { alt, .. } => {
                let label = if alt.is_empty() { "[изображение]".to_string() } else { format!("[изображение: {alt}]") };
                ui.label(RichText::new(label).weak());
            }
            Inline::Link { href, host, suspicious, content } => {
                let text = postman_ir::plain_text(content);
                let mut hover = href.clone();
                if let Some(h) = host {
                    hover = format!("{h}\n{href}");
                }
                if *suspicious {
                    ui.label(RichText::new("⚠").color(Color32::from_rgb(200, 40, 40)));
                    hover = format!("Текст ссылки не совпадает с адресом!\n{hover}");
                }
                let r = ui.link(rich(&text, Marks::default(), size)).on_hover_text(hover);
                if r.clicked() {
                    *clicked = Some(href.clone());
                }
            }
        }
    }
}

fn rich(text: &str, m: Marks, size: Option<f32>) -> RichText {
    let mut r = RichText::new(text);
    if let Some(s) = size {
        r = r.size(s).strong();
    }
    if m.bold {
        r = r.strong();
    }
    if m.italic {
        r = r.italics();
    }
    if m.underline {
        r = r.underline();
    }
    if m.strike {
        r = r.strikethrough();
    }
    if m.code {
        r = r.code();
    }
    r
}

//! Область чтения: шапка письма и тело, разобранное воркером-песочницей.

use std::sync::{Arc, Mutex};

use gpui_kit::*;
use pets_i18n::tr;
use postman_cache::EmailRow;
use postman_ir::Document;
use postman_worker::{Client as Worker, Level, Request};

use crate::*;

pub(crate) struct Reading {
    pub(crate) row: EmailRow,
    pub(crate) body: Option<postman_cache::BodyRow>,
    pub(crate) doc: Option<Document>,
}

// --- область чтения ---

pub(crate) struct ReadingPane {
    pub(crate) worker: Arc<Mutex<Worker>>,
    pub(crate) reading: Option<Reading>,
}

/// Итог разбора: уровень песочницы (строка состояния) и ошибка (тост).
pub(crate) enum ReadingEvent {
    Sandbox(Level),
    Error(String),
}

impl EventEmitter<ReadingEvent> for ReadingPane {}

impl ReadingPane {
    pub(crate) fn open(&mut self, row: EmailRow, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        let body = Some(postman_demo::body(&row.id));
        self.reading = Some(Reading { row, body, doc: None });
        self.render_body(cx);
        cx.notify();
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.reading = None;
        cx.notify();
    }

    /// HTML → IR в воркере-песочнице, в фоновом исполнителе gpui.
    pub(crate) fn render_body(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.reading else { return };
        let Some(body) = r.body.clone() else { return };
        let id = r.row.id.clone();
        let worker = self.worker.clone();
        let task = cx.background_spawn(async move {
            // Отравленный мьютекс (паника в прошлом разборе) не мешает: клиент воркера сам перезапускает процесс.
            let mut w = worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let req = match (body.html, body.text) {
                (Some(h), _) => Request::Html(h),
                (None, Some(t)) => Request::Text(t),
                _ => Request::Text(String::new()),
            };
            (w.render(&req), w.level())
        });
        cx.spawn(async move |this, cx| {
            let (result, level) = task.await;
            this.update(cx, |this, cx| {
                cx.emit(ReadingEvent::Sandbox(level));
                let doc = result.unwrap_or_else(|e| {
                    let text = tr!("reading-error", error = e.to_string());
                    cx.emit(ReadingEvent::Error(text.clone()));
                    postman_ir::from_plain(&text)
                });
                if let Some(r) = this.reading.as_mut().filter(|r| r.row.id == id) {
                    r.doc = Some(doc);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for ReadingPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(r) = &self.reading else {
            return div().flex().size_full().items_center().justify_center().text_color(rgb(0x888888)).child(tr!("reading-empty")).into_any_element();
        };
        let from = format!("{} <{}>", r.row.from_name, r.row.from_addr);
        let to = r
            .body
            .as_ref()
            .map(|b| b.to.iter().map(|a| format!("{} <{}>", a.name, a.email)).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_2()
            .border_b_1()
            .border_color(rgb(0xdddddd))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(r.row.subject.clone()))
            .child(field(tr!("reading-from"), from))
            .child(field(tr!("reading-to"), to))
            .child(field(tr!("reading-date"), format_date(r.row.received_at)));
        let body: AnyElement = match &r.doc {
            None => div().text_color(rgb(0x888888)).child("…").into_any_element(),
            Some(doc) => {
                let mut col = div().flex().flex_col().gap_2();
                if doc.remote_images_blocked > 0 {
                    col = col.child(
                        div()
                            .px_2()
                            .py_1()
                            .bg(rgb(0xfde6dc))
                            .child(tr!("reading-remote-blocked", n = doc.remote_images_blocked)),
                    );
                }
                col.children(ir_view::blocks(&doc.blocks, "doc", cx)).into_any_element()
            }
        };
        div()
            .id("reading")
            .flex()
            .flex_col()
            .size_full()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

pub(crate) fn sandbox_label(level: Level) -> String {
    match level {
        Level::Full => tr!("sandbox-full"),
        Level::Partial => tr!("sandbox-partial"),
        Level::None => tr!("sandbox-none"),
    }
}

pub(crate) fn field(label: String, value: String) -> impl IntoElement {
    div().flex().gap_2().child(div().w(px(48.)).text_color(rgb(0x888888)).child(label)).child(value)
}

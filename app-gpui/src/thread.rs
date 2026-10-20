//! Беседа в области чтения (S5-05, минимальная): переключатель «Беседа (N)» и лента писем цепочки — только новый
//! текст (границу истории ставит воркер-песочница), разделители по дням, свои письма отмечены и сдвинуты вправо;
//! цитаты — переключателем. Ctrl+→ / Ctrl+← — по письмам беседы. Оформление по макетам — S5-01.
//!
//! Письма беседы — из кэша (индекс цепочек, все папки); живой ящик при первом показе ленты просит беседу у сервера
//! (`Command::Thread` → `Event::ThreadReady`): письма из папок за пределами первой страницы и недостающие тела.

use std::collections::HashMap;
use std::sync::PoisonError;

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use postman_cache::{BodyRow, EmailRow};
use postman_core::thread::{self, Conversation, Item};
use postman_ir::Document;
use postman_worker::Request;

use crate::Mailer;
use crate::ir_view::{self, Images};
use crate::reading::{ReadingEvent, ReadingPane, SyncRequest};

const MUTED: u32 = 0x888888;
const ACCENT: u32 = 0x0f6cbd;
/// Подложка своих писем (DESIGN: «свои письма справа на светлой подложке»).
const MINE_BG: u32 = 0xe8f1fb;

/// Лента беседы открытого письма.
pub(crate) struct Feed {
    pub(crate) conv: Conversation,
    /// Разобранные воркером письма (полный IR с границей истории), по ключу письма.
    pub(crate) docs: HashMap<String, Document>,
    /// Разбор в воркере идёт; `again` — за это время пришли новые тела, разобрать и их.
    rendering: bool,
    again: bool,
    /// Беседу уже просили у сервера; `waiting` — ответа ещё нет.
    fetched: bool,
    pub(crate) waiting: bool,
}

impl Feed {
    fn new(conv: Conversation) -> Self {
        Self { conv, docs: HashMap::new(), rendering: false, again: false, fetched: false, waiting: false }
    }

    /// Все письма ленты разобраны (снимок `thread`).
    pub(crate) fn ready(&self) -> bool {
        self.conv.messages.iter().all(|m| self.docs.contains_key(&m.row.id))
    }
}

/// Тело письма — запрос воркеру (HTML, иначе текст).
fn request(body: BodyRow) -> Request {
    match (body.html, body.text) {
        (Some(h), _) => Request::Html(h),
        (None, Some(t)) => Request::Text(t),
        _ => Request::Text(String::new()),
    }
}

impl ReadingPane {
    /// Мои адреса: Identity своего аккаунта и общих ящиков (`Event::Senders`), адрес учётной записи (`Event::Me`).
    fn my_addresses(&self) -> Vec<String> {
        self.mine.iter().cloned().chain(self.me.clone()).collect()
    }

    /// Открыто письмо: беседа — из кэша (в фоне). Та же беседа — лента остаётся, меняется текущее письмо.
    pub(crate) fn sync_thread(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.reading else { return };
        let key = r.row.thread_id.clone();
        if self.feed.as_ref().is_some_and(|f| f.conv.thread == key) {
            return;
        }
        self.feed = None;
        self.reload_thread(key, cx);
    }

    fn reload_thread(&mut self, key: String, cx: &mut Context<Self>) {
        let cache = self.cache.clone();
        let task = cx.background_spawn(async move {
            let rows = cache.thread(&key);
            (key, rows)
        });
        cx.spawn(async move |this, cx| {
            let (key, rows) = task.await;
            this.update(cx, |this, cx| match rows {
                Ok(rows) => this.set_conversation(&key, rows, cx),
                Err(e) => tracing::warn!("thread {key} is not read from the cache: {e}"),
            })
            .ok();
        })
        .detach();
    }

    fn set_conversation(&mut self, key: &str, rows: Vec<EmailRow>, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_none_or(|r| r.row.thread_id != key) {
            return;
        }
        let conv = Conversation::assemble(key, rows, &self.my_addresses(), &TimeZone::system());
        match &mut self.feed {
            Some(f) if f.conv.thread == key => f.conv = conv,
            // Одно письмо — не беседа: переключателя нет.
            _ if conv.len() < 2 => return,
            _ => self.feed = Some(Feed::new(conv)),
        }
        if self.feed_on {
            self.load_feed(cx);
        }
        cx.notify();
    }

    /// «Беседа (N)»: лента вместо одного письма и обратно; выбор помнится для следующих писем.
    pub(crate) fn toggle_feed(&mut self, cx: &mut Context<Self>) {
        if self.feed.is_none() {
            return;
        }
        self.feed_on = !self.feed_on;
        if self.feed_on {
            self.load_feed(cx);
        }
        cx.notify();
    }

    /// «Показать цитаты» в ленте.
    pub(crate) fn toggle_quotes(&mut self, cx: &mut Context<Self>) {
        self.quotes = !self.quotes;
        cx.notify();
    }

    /// Беседа пришла с сервера: перечитать письма из кэша и разобрать новые тела.
    pub(crate) fn thread_ready(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(f) = self.feed.as_mut().filter(|f| f.conv.thread == key) else { return };
        f.waiting = false;
        self.reload_thread(key.to_owned(), cx);
    }

    /// Запрос беседы не ушёл (сервера нет): не ждать.
    pub(crate) fn thread_not_sent(&mut self) {
        if let Some(f) = &mut self.feed {
            f.waiting = false;
        }
    }

    /// Разобрать в воркере письма ленты, у которых есть тело; живой ящик — один раз спросить беседу у сервера.
    fn load_feed(&mut self, cx: &mut Context<Self>) {
        let live = self.live;
        let Some(f) = &mut self.feed else { return };
        if live && !f.fetched {
            f.fetched = true;
            f.waiting = true;
            cx.emit(ReadingEvent::Sync(SyncRequest::Thread(f.conv.thread.clone())));
        }
        if f.rendering {
            f.again = true;
            return;
        }
        let ids: Vec<String> =
            f.conv.messages.iter().map(|m| m.row.id.clone()).filter(|id| !f.docs.contains_key(id)).collect();
        if ids.is_empty() {
            return;
        }
        f.rendering = true;
        let (cache, worker, key) = (self.cache.clone(), self.worker.clone(), f.conv.thread.clone());
        let task = cx.background_spawn(async move {
            let mut out = Vec::new();
            for id in ids {
                let body = if live { cache.body(&id).ok().flatten() } else { Some(postman_demo::body(&id)) };
                // Тела нет — придёт с сервера (`ThreadReady`).
                let Some(body) = body else { continue };
                let mut w = worker.lock().unwrap_or_else(PoisonError::into_inner);
                out.push((id, w.render(&request(body))));
            }
            out
        });
        cx.spawn(async move |this, cx| {
            let docs = task.await;
            this.update(cx, |this, cx| {
                let Some(f) = this.feed.as_mut().filter(|f| f.conv.thread == key) else { return };
                for (id, doc) in docs {
                    let doc = doc.unwrap_or_else(|e| postman_ir::from_plain(&tr!("reading-error", error = e)));
                    f.docs.insert(id, doc);
                }
                f.rendering = false;
                if std::mem::take(&mut f.again) {
                    this.load_feed(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Соседнее письмо беседы от открытого (Ctrl+→ / Ctrl+←).
    pub(crate) fn thread_neighbor(&self, delta: isize) -> Option<EmailRow> {
        let id = &self.reading.as_ref()?.row.id;
        self.feed.as_ref()?.conv.neighbor(id, delta).cloned()
    }

    /// Ряд над письмом: «Беседа (N)», в ленте — «Показать цитаты»; у письма без беседы — ничего.
    pub(crate) fn feed_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.feed.as_ref()?;
        let button = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(rgb(0xdddddd))
                .cursor_pointer()
                .hover(|d| d.bg(rgb(0xececec)))
                .when(on, |d| d.bg(rgb(MINE_BG)).text_color(rgb(ACCENT)))
                .child(label)
        };
        let quotes = if self.quotes { tr!("thread-quotes-hide") } else { tr!("thread-quotes-show") };
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                button("thread-toggle", tr!("thread-toggle", n = f.conv.len()), self.feed_on)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_feed(cx))),
            )
            .when(self.feed_on, |d| {
                d.child(
                    button("thread-quotes", quotes, self.quotes)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_quotes(cx))),
                )
            })
            .when(f.waiting, |d| d.child(div().text_color(rgb(MUTED)).child(tr!("thread-loading"))))
            .child(div().flex_1())
            .child(div().text_color(rgb(MUTED)).child(tr!("thread-keys")));
        Some(bar.into_any_element())
    }

    /// Лента: разделители дней и письма — только новый текст (пересылка и «показать цитаты» — целиком).
    pub(crate) fn feed_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.feed.as_ref().filter(|_| self.feed_on)?;
        let current = self.reading.as_ref().map(|r| r.row.id.as_str());
        let today = jiff::Zoned::now().date();
        // Картинки `cid:` в ленте не грузим: на их месте подписи; письмо целиком — в обычном виде.
        let images = Images::new();
        let mut col = div().flex().flex_col().gap_2();
        for item in &f.conv.items {
            match *item {
                Item::Day(day) => {
                    let label = if day == today {
                        tr!("thread-today")
                    } else if today.yesterday().is_ok_and(|y| y == day) {
                        tr!("thread-yesterday")
                    } else {
                        day.strftime("%d.%m.%Y").to_string()
                    };
                    col = col.child(div().flex().justify_center().pt_1().text_color(rgb(MUTED)).child(label));
                }
                Item::Message(n) => {
                    let Some(m) = f.conv.messages.get(n) else { continue };
                    let who = if m.mine {
                        tr!("thread-me")
                    } else if m.row.from_name.is_empty() {
                        m.row.from_addr.clone()
                    } else {
                        m.row.from_name.clone()
                    };
                    let doc = f.docs.get(&m.row.id);
                    let whole = self.quotes || thread::is_forward(&m.row.subject);
                    let body: AnyElement = match doc {
                        None => div().text_color(rgb(MUTED)).child("…").into_any_element(),
                        Some(d) => {
                            let blocks = if whole { &d.blocks[..] } else { d.new_text() };
                            let hidden = !whole && blocks.len() < d.blocks.len();
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(ir_view::blocks(blocks, &format!("feed{n}"), &images, cx))
                                .when(hidden, |d| {
                                    d.child(div().text_color(rgb(MUTED)).child(tr!("thread-quotes-hidden")))
                                })
                                .into_any_element()
                        }
                    };
                    let row = m.row.clone();
                    let card = div()
                        .id(("feed-msg", n))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .max_w(relative(0.85))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(if current == Some(m.row.id.as_str()) { ACCENT } else { 0xdddddd }))
                        .bg(rgb(if m.mine { MINE_BG } else { 0xffffff }))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ReadingEvent::Open(row.clone()))))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(who))
                                .child(div().text_color(rgb(MUTED)).child(crate::format_date(m.row.received_at))),
                        )
                        .child(body);
                    col = col.child(div().flex().w_full().when(m.mine, |d| d.justify_end()).child(card));
                }
            }
        }
        Some(col.into_any_element())
    }
}

impl Mailer {
    /// Ctrl+→ / Ctrl+←: следующее или предыдущее письмо беседы — в области чтения (список не трогаем).
    pub(crate) fn thread_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(row) = self.reading.read(cx).thread_neighbor(delta) {
            self.open_letter(row, cx);
        }
    }
}

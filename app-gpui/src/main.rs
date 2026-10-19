//! Спайк UI на gpui (gpui-kit): тот же экран, что и в egui-версии, и тот же `--bench`.
//! Ядро (хранилище, IR, воркер-песочница, демо-ящик) — общее; здесь только интерфейс.
//!
//!   postman-gpui [--demo N] [--bench]

mod calendar;
mod ir_view;

use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use postman_cache::{Cache, EmailRow, MailboxRow};
use postman_ir::Document;
use postman_core::bench::{self, Bench, Step};
use postman_worker::{Client as Worker, Level, Request};

actions!(postman, [Next, Prev, First, Last, PageDown, PageUp, MarkRead, MarkUnread, ToggleFlag, FocusSearch, ClearSearch, ShowMail, ShowCalendar, NewMeeting]);

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Mail,
    Calendar,
}

const ROW_HEIGHT: f32 = 44.0;
const WINDOW_MARGIN: usize = 100;

struct Reading {
    row: EmailRow,
    body: Option<postman_cache::BodyRow>,
    doc: Option<Document>,
}

/// Запись флагов в хранилище по очереди в отдельном потоке: поток интерфейса не ждёт диск
/// (каждый коммит redb — fsync, на листании с открытием писем это до 30 мс на кадр).
struct FlagWriter {
    tx: std::sync::mpsc::Sender<(String, bool, bool)>,
    cache: Cache,
}

impl FlagWriter {
    fn new(cache: Cache) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(String, bool, bool)>();
        let writer = cache.clone();
        // Не запустился поток — получатель закрыт, `set` пишет сам (медленнее, но без потерь).
        let _ = std::thread::Builder::new().name("cache-flags".into()).spawn(move || {
            for (id, seen, flagged) in rx {
                let _ = writer.set_flags(&id, seen, flagged);
            }
        });
        Self { tx, cache }
    }

    fn set(&self, id: &str, seen: bool, flagged: bool) {
        if self.tx.send((id.to_owned(), seen, flagged)).is_err() {
            let _ = self.cache.set_flags(id, seen, flagged);
        }
    }
}

struct Mailer {
    cache: Cache,
    flags: FlagWriter,
    focus: FocusHandle,
    mailboxes: Vec<MailboxRow>,
    mailbox: Option<String>,
    /// id результатов поиска вместо папки.
    search_ids: Option<Vec<String>>,
    count: usize,
    rows: (usize, Vec<EmailRow>),
    /// Окно строк, которое читается в фоне, и номер запроса: ответы на устаревшие запросы отбрасываются.
    page_pending: Option<Range<usize>>,
    page_gen: u64,
    /// Письмо выбрано, а его строка ещё не прочитана — открыть, когда страница придёт.
    pending_open: Option<usize>,
    visible: Range<usize>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    reading: Option<Reading>,
    worker: Arc<Mutex<Worker>>,
    sandbox: &'static str,
    /// Последнее нажатие и сработавшее действие — проверка хоткеев в любой раскладке (S1-01).
    last_key: String,
    search: Entity<InputState>,
    bench: Option<Bench>,
    section: Section,
    cal: calendar::CalendarView,
    _subscriptions: Vec<Subscription>,
}

impl Mailer {
    fn new(cache: Cache, bench: Option<Bench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск (Ctrl+E), Enter — искать, Esc — сбросить"));
        let keylog = std::env::var_os("POSTMAN_KEYLOG").is_some();
        let sub = cx.subscribe_in(&search, window, move |this: &mut Self, state, ev: &InputEvent, _window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let q = state.read(cx).value().to_string();
                this.run_search(&q, cx);
            }
            InputEvent::Change if keylog => println!("search \"{}\"", state.read(cx).value()),
            _ => {}
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut m = Self {
            flags: FlagWriter::new(cache.clone()),
            cache,
            focus,
            mailboxes: Vec::new(),
            mailbox: None,
            search_ids: None,
            count: 0,
            rows: (0, Vec::new()),
            page_pending: None,
            page_gen: 0,
            pending_open: None,
            visible: 0..30,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            reading: None,
            worker: Arc::new(Mutex::new(Worker::new(Worker::default_exe(), Duration::from_secs(5)))),
            sandbox: "",
            last_key: String::new(),
            search,
            bench,
            section: Section::Mail,
            cal: calendar::CalendarView::demo(),
            _subscriptions: vec![sub, Self::observe_keys(cx)],
        };
        m.mailboxes = m.cache.mailboxes().unwrap_or_default();
        let inbox = m.mailboxes.iter().find(|b| b.role.as_deref() == Some("inbox")).or(m.mailboxes.first());
        if let Some(id) = inbox.map(|b| b.id.clone()) {
            m.open_mailbox(id);
        }
        m
    }

    /// «Ctrl-e (у) → FocusSearch» в строке состояния; `POSTMAN_KEYLOG=1` — то же строкой `key …` в stdout.
    fn observe_keys(cx: &mut Context<Self>) -> Subscription {
        let to_stdout = std::env::var_os("POSTMAN_KEYLOG").is_some();
        cx.observe_keystrokes(move |this, ev, _window, cx| {
            let k = &ev.keystroke;
            let action = ev.action.as_ref().map_or("—", |a| a.name());
            this.last_key = format!("{} ({}) → {action}", k.unparse(), k.key_char.as_deref().unwrap_or(""));
            if to_stdout {
                println!("key {}", this.last_key);
            }
            cx.notify();
        })
    }

    fn open_mailbox(&mut self, id: String) {
        self.mailbox = Some(id);
        self.search_ids = None;
        self.selected = None;
        self.reading = None;
        self.reload();
    }

    fn reload(&mut self) {
        self.count = match (&self.search_ids, &self.mailbox) {
            (Some(ids), _) => ids.len(),
            (None, Some(mb)) => self.cache.count(mb).unwrap_or(0),
            _ => 0,
        };
        // Первое окно — сразу: первый кадр папки рисуется со строками, а не с заглушками.
        self.page_gen += 1;
        self.page_pending = None;
        self.pending_open = None;
        let (offset, limit) = page_window(&self.visible);
        self.rows = (offset, self.page_source().read(&self.cache, offset, limit));
    }

    /// Окно строк вокруг видимого диапазона. Читается в фоне — поток интерфейса не ждёт хранилище;
    /// следующее окно запрашивается заранее, когда до края загруженного меньше половины запаса.
    fn ensure(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let (off, rows) = (self.rows.0, self.rows.1.len());
        let end = off + rows;
        let loaded = rows > 0 && range.start >= off && range.end <= end;
        let near_edge = (off > 0 && range.start < off + WINDOW_MARGIN / 2)
            || (end < self.count && range.end + WINDOW_MARGIN / 2 > end);
        if loaded && !near_edge {
            return;
        }
        if self.page_pending.as_ref().is_some_and(|p| range.start >= p.start && range.end <= p.end) {
            return;
        }
        let (offset, limit) = page_window(&range);
        self.page_gen += 1;
        let generation = self.page_gen;
        self.page_pending = Some(offset..offset + limit);
        let (source, cache) = (self.page_source(), self.cache.clone());
        let task = cx.background_spawn(async move { source.read(&cache, offset, limit) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |m, cx| {
                if m.page_gen != generation {
                    return;
                }
                m.page_pending = None;
                m.rows = (offset, rows);
                if let Some(row) = m.pending_open.take().and_then(|i| m.row(i).cloned()) {
                    m.open_row(row, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Откуда читать строки списка: папка или результаты поиска.
    fn page_source(&self) -> PageSource {
        match (&self.search_ids, &self.mailbox) {
            (Some(ids), _) => PageSource::Ids(ids.clone()),
            (None, Some(mb)) => PageSource::Mailbox(mb.clone()),
            _ => PageSource::Empty,
        }
    }

    fn row(&self, i: usize) -> Option<&EmailRow> {
        i.checked_sub(self.rows.0).and_then(|j| self.rows.1.get(j))
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.count == 0 {
            return;
        }
        let i = i.min(self.count - 1);
        self.selected = Some(i);
        self.scroll.scroll_to_item(i, ScrollStrategy::Center);
        self.ensure(i..i + 1, cx);
        let Some(row) = self.row(i).cloned() else {
            self.pending_open = Some(i);
            return;
        };
        self.pending_open = None;
        self.open_row(row, cx);
    }

    /// Показать письмо в области чтения; при первом открытии — пометить прочитанным.
    fn open_row(&mut self, mut row: EmailRow, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        if !row.seen {
            row.seen = true;
            self.flags.set(&row.id, true, row.flagged);
            self.patch_row(&row);
        }
        let body = Some(postman_demo::body(&row.id));
        self.reading = Some(Reading { row, body, doc: None });
        self.render_body(cx);
    }

    fn patch_row(&mut self, row: &EmailRow) {
        for r in self.rows.1.iter_mut().filter(|r| r.id == row.id) {
            *r = row.clone();
        }
    }

    /// HTML → IR в воркере-песочнице, в фоновом исполнителе gpui.
    fn render_body(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.reading else { return };
        let Some(body) = r.body.clone() else { return };
        let id = r.row.id.clone();
        let worker = self.worker.clone();
        let task = cx.background_spawn(async move {
            let mut w = worker.lock().expect("воркер");
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
                this.sandbox = match level {
                    Level::Full => "Песочница: полная",
                    Level::Partial => "Песочница: частичная",
                    Level::None => "Песочница: нет",
                };
                if let Some(r) = this.reading.as_mut().filter(|r| r.row.id == id) {
                    r.doc = Some(result.unwrap_or_else(|e| postman_ir::from_plain(&format!("⚠ {e}"))));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_search(&mut self, q: &str, cx: &mut Context<Self>) {
        let q = q.trim();
        if q.is_empty() {
            self.search_ids = None;
        } else {
            self.search_ids = Some(self.cache.search(q, self.mailbox.as_deref(), 1000).unwrap_or_default());
        }
        self.selected = None;
        self.reading = None;
        self.reload();
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let cur = self.selected.map_or(0, |i| i as isize);
        let next = if self.selected.is_none() { 0 } else { (cur + delta).max(0) as usize };
        self.select(next, cx);
        cx.notify();
    }

    fn set_flags(&mut self, seen: Option<bool>, toggle_flag: bool, cx: &mut Context<Self>) {
        let Some(mut row) = self.selected.and_then(|i| self.row(i)).cloned() else { return };
        if let Some(s) = seen {
            row.seen = s;
        }
        if toggle_flag {
            row.flagged = !row.flagged;
        }
        self.flags.set(&row.id, row.seen, row.flagged);
        self.patch_row(&row);
        cx.notify();
    }

    fn page(&self) -> isize {
        self.visible.len().max(1) as isize
    }

    // --- отрисовка ---

    fn folders(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().w(px(200.)).h_full().border_r_1().border_color(rgb(0xdddddd)).bg(rgb(0xf7f7f7));
        if self.search_ids.is_some() {
            col = col.child(div().px_3().py_1().bg(rgb(0xcce4ff)).child(format!("🔍 Результаты ({})", self.count)));
        }
        let mut list = div().flex().flex_col().flex_1().p_1();
        for m in &self.mailboxes {
            let selected = self.search_ids.is_none() && self.mailbox.as_deref() == Some(&m.id);
            let id = m.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("mb-{}", m.id)))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(selected, |d| d.bg(rgb(0xcce4ff)))
                    .hover(|d| d.bg(rgb(0xe8f1fb)))
                    .cursor_pointer()
                    .child(m.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_mailbox(id.clone());
                        cx.notify();
                    })),
            );
        }
        col.child(list).child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(0xdddddd))
                .child(nav("nav-mail", "✉ Почта", self.section == Section::Mail, Section::Mail, cx))
                .child(nav("nav-cal", "📅 Календарь", self.section == Section::Calendar, Section::Calendar, cx))
                .child("👥 Контакты"),
        )
    }

    fn list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.count;
        div().w(px(380.)).h_full().border_r_1().border_color(rgb(0xdddddd)).child(
            uniform_list(
                "messages",
                count,
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    // Одну строку gpui запрашивает, чтобы измерить высоту, — это не видимая область.
                    if range.len() > 1 {
                        this.visible = range.clone();
                        this.ensure(range.clone(), cx);
                    }
                    range
                        .map(|i| match this.row(i).cloned() {
                            Some(r) => list_row(i, &r, this.selected == Some(i), cx),
                            None => empty_row(i),
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h_full(),
        )
    }

    fn reading_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.reading else {
            return div().flex().flex_1().items_center().justify_center().text_color(rgb(0x888888)).child("Выберите письмо").into_any_element();
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
            .child(field("От:", from))
            .child(field("Кому:", to))
            .child(field("Дата:", format_date(r.row.received_at)));
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
                            .child(format!("Внешние картинки не загружаются ({}). Это защищает от отслеживания.", doc.remote_images_blocked)),
                    );
                }
                col.children(ir_view::blocks(&doc.blocks, "doc", cx)).into_any_element()
            }
        };
        div()
            .id("reading")
            .flex()
            .flex_col()
            .flex_1()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

fn nav(id: &'static str, label: &'static str, active: bool, section: Section, cx: &mut Context<Mailer>) -> impl IntoElement {
    div()
        .id(id)
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(rgb(0xcce4ff)))
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.section = section;
            cx.notify();
        }))
}

fn field(label: &'static str, value: String) -> impl IntoElement {
    div().flex().gap_2().child(div().w(px(48.)).text_color(rgb(0x888888)).child(label)).child(value)
}

/// Окно чтения вокруг диапазона: (смещение, число строк).
fn page_window(range: &Range<usize>) -> (usize, usize) {
    (range.start.saturating_sub(WINDOW_MARGIN), range.len() + 2 * WINDOW_MARGIN)
}

enum PageSource {
    Mailbox(String),
    Ids(Vec<String>),
    Empty,
}

impl PageSource {
    fn read(&self, cache: &Cache, offset: usize, limit: usize) -> Vec<EmailRow> {
        match self {
            PageSource::Mailbox(mb) => cache.page(mb, offset, limit).unwrap_or_default(),
            PageSource::Ids(ids) => {
                let end = (offset + limit).min(ids.len());
                cache.rows_by_ids(&ids[offset.min(end)..end]).unwrap_or_default()
            }
            PageSource::Empty => Vec::new(),
        }
    }
}

/// Строка, которая ещё читается из хранилища: та же высота, без текста.
fn empty_row(i: usize) -> AnyElement {
    div().id(i).h(px(ROW_HEIGHT)).border_b_1().border_color(rgb(0xe5e5e5)).into_any_element()
}

fn list_row(i: usize, r: &EmailRow, selected: bool, cx: &mut Context<Mailer>) -> AnyElement {
    let from = if r.from_name.is_empty() { r.from_addr.clone() } else { r.from_name.clone() };
    let mut icons = String::new();
    if r.has_attachment {
        icons.push('📎');
    }
    if r.flagged {
        icons.push('⚑');
    }
    let line2 = format!("{}  —  {}", r.subject, r.preview);
    div()
        .id(i)
        .relative()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_col()
        .justify_center()
        .px_3()
        .line_height(px(18.))
        .border_b_1()
        .border_color(rgb(0xe5e5e5))
        // Метка «не прочитано» — полоса слева, как в Outlook.
        .when(!r.seen, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(rgb(0x1a8cff))))
        .when(selected, |d| d.bg(rgb(0xcce4ff)))
        .hover(|d| d.bg(rgb(0xf0f6fd)))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().truncate().when(!r.seen, |d| d.font_weight(FontWeight::BOLD)).child(from))
                .child(div().text_color(rgb(0xcc3333)).child(icons))
                .child(div().text_sm().text_color(rgb(0x888888)).child(format_date(r.received_at))),
        )
        .child(div().text_sm().truncate().text_color(if r.seen { rgb(0x777777) } else { rgb(0x1a6fd1) }).child(line2))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select(i, cx);
            cx.notify();
        }))
        .into_any_element()
}

fn format_date(ts: i64) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (Ok(t), now) = (jiff::Timestamp::from_second(ts), jiff::Zoned::now()) else { return String::new() };
    let t = t.to_zoned(tz);
    if t.date() == now.date() { t.strftime("%H:%M").to_string() } else { t.strftime("%d.%m %H:%M").to_string() }
}

impl Render for Mailer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        self.drive_bench(window, cx);
        let status = format!("Элементов: {}", self.count);
        let root = div()
            .key_context("Mailer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Next, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &Prev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &First, _, cx| this.step(-(this.count as isize), cx)))
            .on_action(cx.listener(|this, _: &Last, _, cx| this.step(this.count as isize, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.step(this.page(), cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.step(-this.page(), cx)))
            .on_action(cx.listener(|this, _: &MarkRead, _, cx| this.set_flags(Some(true), false, cx)))
            .on_action(cx.listener(|this, _: &MarkUnread, _, cx| this.set_flags(Some(false), false, cx)))
            .on_action(cx.listener(|this, _: &ToggleFlag, _, cx| this.set_flags(None, true, cx)))
            .on_action(cx.listener(|this, _: &ShowMail, _, cx| {
                this.section = Section::Mail;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowCalendar, _, cx| {
                this.section = Section::Calendar;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewMeeting, _, cx| {
                this.section = Section::Calendar;
                this.cal.assistant = !this.cal.assistant;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ClearSearch, window, cx| {
                this.search.update(cx, |s, cx| s.set_value("", window, cx));
                this.run_search("", cx);
                window.focus(&this.focus, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xfafafa))
            .text_color(rgb(0x222222))
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(0xdddddd))
                    .children(["✉ Создать", "↩ Ответить", "↩↩ Ответить всем", "↪ Переслать", "🗑 Удалить", "⚑ Флаг"].map(|l| {
                        div().px_2().py_0p5().rounded_md().bg(rgb(0xececec)).child(l)
                    }))
                    .child(div().flex_1())
                    .child(div().w(px(320.)).child(Input::new(&self.search))),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.folders(cx))
                    .when(self.section == Section::Mail, |d| d.child(self.list(cx)).child(self.reading_pane(cx)))
                    .when(self.section == Section::Calendar, |d| d.child(calendar::render(self, cx))),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_2()
                    .py_0p5()
                    .border_t_1()
                    .border_color(rgb(0xdddddd))
                    .text_color(rgb(0x888888))
                    .child(status)
                    .child(div().flex_1())
                    .child(self.last_key.clone())
                    .child(self.sandbox)
                    .child("gpui"),
            );
        if let Some(b) = &mut self.bench {
            b.ui_time(started.elapsed());
        }
        root
    }
}

// --- замер: общий сценарий из postman_core::bench, тот же, что в egui-версии ---

impl Mailer {
    fn drive_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = &mut self.bench else { return };
        match b.frame(self.count) {
            Step::Wait => {}
            Step::Select(i) => self.select(i, cx),
            Step::Scroll(i) => self.scroll.scroll_to_item(i, ScrollStrategy::Top),
            Step::Resize(w, h) => window.resize(size(px(w), px(h))),
            Step::Done(report) => {
                // Дальше — простой: кадры больше не запрашиваем, процесс завершит замер.
                bench::finish(format!("bench ui=gpui {report}"));
                self.bench = None;
                return;
            }
        }
        window.request_animation_frame();
    }
}

fn data_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| ".".into())
        .join("Postman")
}

fn main() {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n = args.iter().position(|a| a == "--demo").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let bench = args.iter().any(|a| a == "--bench");
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("каталог данных");
    // Бенч — на свежей копии ящика; подготовка данных не входит в first_frame_ms.
    let prepared = Instant::now();
    let path = if bench {
        postman_demo::bench_copy(&dir, n).expect("ящик для замера")
    } else {
        let path = dir.join("demo.redb");
        postman_demo::ensure(&path, n).expect("демо-ящик");
        path
    };
    let start = if bench { start + prepared.elapsed() } else { start };
    let cache = Cache::open(&path).expect("хранилище");

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // Клавиши без модификаторов — только вне поля ввода: в поиске j/k, стрелки, Home/End — это текст и курсор.
        const LIST: Option<&str> = Some("Mailer && !Input");
        cx.bind_keys([
            KeyBinding::new("j", Next, LIST),
            KeyBinding::new("down", Next, LIST),
            KeyBinding::new("k", Prev, LIST),
            KeyBinding::new("up", Prev, LIST),
            KeyBinding::new("home", First, LIST),
            KeyBinding::new("end", Last, LIST),
            KeyBinding::new("pagedown", PageDown, LIST),
            KeyBinding::new("pageup", PageUp, LIST),
            KeyBinding::new("ctrl-q", MarkRead, Some("Mailer")),
            KeyBinding::new("ctrl-u", MarkUnread, Some("Mailer")),
            KeyBinding::new("insert", ToggleFlag, LIST),
            KeyBinding::new("ctrl-e", FocusSearch, None),
            KeyBinding::new("ctrl-1", ShowMail, None),
            KeyBinding::new("ctrl-2", ShowCalendar, None),
            KeyBinding::new("ctrl-shift-q", NewMeeting, None),
            KeyBinding::new("escape", ClearSearch, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| {
            let bench = bench.then(|| Bench::new(start));
            let cache = cache.clone();
            cx.new(|cx| Mailer::new(cache, bench, window, cx))
        })
        .expect("окно");
        cx.activate(true);
    });
}

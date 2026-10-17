//! Спайк UI на gpui (gpui-kit): тот же экран, что и в egui-версии, и тот же `--bench`.
//! Ядро (хранилище, IR, воркер-песочница, демо-ящик) — общее; здесь только интерфейс.
//!
//!   postman-gpui [--demo N] [--bench]

mod ir_view;

use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use postman_cache::{Cache, EmailRow, MailboxRow};
use postman_ir::Document;
use postman_worker::{Client as Worker, Level, Request};

actions!(postman, [Next, Prev, First, Last, PageDown, PageUp, MarkRead, MarkUnread, ToggleFlag, FocusSearch, ClearSearch]);

const ROW_HEIGHT: f32 = 44.0;
const WINDOW_MARGIN: usize = 100;

struct Reading {
    row: EmailRow,
    body: Option<postman_cache::BodyRow>,
    doc: Option<Document>,
}

struct Mailer {
    cache: Cache,
    focus: FocusHandle,
    mailboxes: Vec<MailboxRow>,
    mailbox: Option<String>,
    /// id результатов поиска вместо папки.
    search_ids: Option<Vec<String>>,
    count: usize,
    rows: (usize, Vec<EmailRow>),
    visible: Range<usize>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    reading: Option<Reading>,
    worker: Arc<Mutex<Worker>>,
    sandbox: &'static str,
    search: Entity<InputState>,
    bench: Option<Bench>,
    _subscriptions: Vec<Subscription>,
}

impl Mailer {
    fn new(cache: Cache, bench: Option<Bench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск (Ctrl+E), Enter — искать, Esc — сбросить"));
        let sub = cx.subscribe_in(&search, window, |this: &mut Self, state, ev: &InputEvent, _window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let q = state.read(cx).value().to_string();
                this.run_search(&q, cx);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut m = Self {
            cache,
            focus,
            mailboxes: Vec::new(),
            mailbox: None,
            search_ids: None,
            count: 0,
            rows: (0, Vec::new()),
            visible: 0..30,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            reading: None,
            worker: Arc::new(Mutex::new(Worker::new(Worker::default_exe(), Duration::from_secs(5)))),
            sandbox: "",
            search,
            bench,
            _subscriptions: vec![sub],
        };
        m.mailboxes = m.cache.mailboxes().unwrap_or_default();
        let inbox = m.mailboxes.iter().find(|b| b.role.as_deref() == Some("inbox")).or(m.mailboxes.first());
        if let Some(id) = inbox.map(|b| b.id.clone()) {
            m.open_mailbox(id);
        }
        m
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
        self.rows = (0, Vec::new());
        self.ensure(self.visible.clone());
    }

    /// Окно строк из хранилища вокруг видимого диапазона.
    fn ensure(&mut self, range: Range<usize>) {
        let (off, rows) = &self.rows;
        if range.start >= *off && range.end <= off + rows.len() && !rows.is_empty() {
            return;
        }
        let offset = range.start.saturating_sub(WINDOW_MARGIN);
        let limit = range.len() + 2 * WINDOW_MARGIN;
        let rows = match (&self.search_ids, &self.mailbox) {
            (Some(ids), _) => {
                let end = (offset + limit).min(ids.len());
                self.cache.rows_by_ids(&ids[offset.min(end)..end]).unwrap_or_default()
            }
            (None, Some(mb)) => self.cache.page(mb, offset, limit).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.rows = (offset, rows);
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
        self.ensure(i..i + 1);
        let Some(mut row) = self.row(i).cloned() else { return };
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        if !row.seen {
            row.seen = true;
            let _ = self.cache.set_flags(&row.id, true, row.flagged);
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
        let _ = self.cache.set_flags(&row.id, row.seen, row.flagged);
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
            div().flex().flex_wrap().gap_2().p_2().border_t_1().border_color(rgb(0xdddddd)).child("✉ Почта").child("📅 Календарь").child("👥 Контакты"),
        )
    }

    fn list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.count;
        div().w(px(380.)).h_full().border_r_1().border_color(rgb(0xdddddd)).child(
            uniform_list(
                "messages",
                count,
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    this.visible = range.clone();
                    this.ensure(range.clone());
                    range
                        .filter_map(|i| this.row(i).map(|r| (i, r.clone())))
                        .map(|(i, r)| list_row(i, &r, this.selected == Some(i), cx))
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

fn field(label: &'static str, value: String) -> impl IntoElement {
    div().flex().gap_2().child(div().w(px(48.)).text_color(rgb(0x888888)).child(label)).child(value)
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
                    .child(self.list(cx))
                    .child(self.reading_pane(cx)),
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
                    .child(self.sandbox)
                    .child("gpui"),
            );
        if let Some(b) = &mut self.bench {
            b.ui_cpu.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        root
    }
}

// --- замер, тот же сценарий, что в egui-версии ---

const WARMUP: usize = 30;
const SCROLL_FRAMES: usize = 400;
const STEP: usize = 25;
const RESIZE_FRAMES: usize = 120;

struct Bench {
    start: Instant,
    first_frame: Option<Duration>,
    frame: usize,
    last: Option<Instant>,
    scroll: Vec<f32>,
    resize: Vec<f32>,
    ui_cpu: Vec<f32>,
    rss_peak: usize,
}

impl Mailer {
    fn drive_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = &mut self.bench else { return };
        let now = Instant::now();
        b.first_frame.get_or_insert(now - b.start);
        let dt = b.last.map(|l| (now - l).as_secs_f32() * 1000.0);
        b.last = Some(now);
        b.frame += 1;
        if b.frame.is_multiple_of(20) {
            b.rss_peak = b.rss_peak.max(rss());
        }
        let f = b.frame;
        window.request_animation_frame();
        if f <= WARMUP {
            return;
        }
        let f = f - WARMUP;
        if f <= SCROLL_FRAMES {
            if let Some(dt) = dt.filter(|_| f > 1) {
                b.scroll.push(dt);
            }
            let i = (f * STEP) % self.count.max(1);
            self.select(i, cx);
            return;
        }
        let f = f - SCROLL_FRAMES;
        if f <= RESIZE_FRAMES {
            if let Some(dt) = dt.filter(|_| f > 1) {
                b.resize.push(dt);
            }
            let w = if (f / 4).is_multiple_of(2) { 1000. } else { 1280. };
            window.resize(size(px(w), px(800.)));
            return;
        }
        let stats = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            if v.is_empty() {
                return (0.0, 0.0, 0.0);
            }
            (v[v.len() / 2], v[((v.len() - 1) as f32 * 0.99) as usize], v[v.len() - 1])
        };
        let (s, r, c) = (stats(&mut b.scroll), stats(&mut b.resize), stats(&mut b.ui_cpu));
        println!(
            "bench ui=gpui rows={} first_frame_ms={:.0} scroll_p50={:.2} scroll_p99={:.2} scroll_max={:.2} resize_p50={:.2} resize_p99={:.2} resize_max={:.2} ui_cpu_p50={:.2} ui_cpu_p99={:.2} rss_mb={:.0} rss_peak_mb={:.0}",
            self.count,
            b.first_frame.unwrap_or_default().as_secs_f32() * 1000.0,
            s.0, s.1, s.2, r.0, r.1, r.2, c.0, c.1,
            rss() as f32 / 1_048_576.0,
            b.rss_peak.max(rss()) as f32 / 1_048_576.0,
        );
        self.bench = None;
        cx.quit();
    }
}

fn rss() -> usize {
    memory_stats::memory_stats().map(|m| m.physical_mem).unwrap_or(0)
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
    let path = dir.join("demo.redb");
    postman_demo::ensure(&path, n).expect("демо-ящик");
    let cache = Cache::open(&path).expect("хранилище");

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("j", Next, Some("Mailer")),
            KeyBinding::new("down", Next, Some("Mailer")),
            KeyBinding::new("k", Prev, Some("Mailer")),
            KeyBinding::new("up", Prev, Some("Mailer")),
            KeyBinding::new("home", First, Some("Mailer")),
            KeyBinding::new("end", Last, Some("Mailer")),
            KeyBinding::new("pagedown", PageDown, Some("Mailer")),
            KeyBinding::new("pageup", PageUp, Some("Mailer")),
            KeyBinding::new("ctrl-q", MarkRead, Some("Mailer")),
            KeyBinding::new("ctrl-u", MarkUnread, Some("Mailer")),
            KeyBinding::new("insert", ToggleFlag, Some("Mailer")),
            KeyBinding::new("ctrl-e", FocusSearch, None),
            KeyBinding::new("escape", ClearSearch, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| {
            let bench = bench.then(|| Bench {
                start,
                first_frame: None,
                frame: 0,
                last: None,
                scroll: Vec::new(),
                resize: Vec::new(),
                ui_cpu: Vec::new(),
                rss_peak: 0,
            });
            let cache = cache.clone();
            cx.new(|cx| Mailer::new(cache, bench, window, cx))
        })
        .expect("окно");
        cx.activate(true);
    });
}

//! Спайк UI на gpui (gpui-kit): тот же экран, что и в egui-версии, и тот же `--bench`.
//! Ядро (хранилище, IR, воркер-песочница, демо-ящик) — общее; здесь только интерфейс.
//!
//!   postman-gpui [--demo N] [--bench]

mod calendar;
mod commands;
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

use commands::{mail, search, view};

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

// --- список писем ---

/// Список писем — отдельное view: при листании gpui раскладывает и рисует только его,
/// остальные части экрана берутся из прошлого кадра (S1-06).
struct MailList {
    cache: Cache,
    flags: FlagWriter,
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
}

/// Письмо открыто (строка уже помечена прочитанной).
struct OpenRow(EmailRow);

impl EventEmitter<OpenRow> for MailList {}

impl MailList {
    fn new(cache: Cache) -> Self {
        Self {
            flags: FlagWriter::new(cache.clone()),
            cache,
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
        }
    }

    fn show_mailbox(&mut self, id: String, cx: &mut Context<Self>) {
        self.mailbox = Some(id);
        self.show_search(None, cx);
    }

    fn show_search(&mut self, ids: Option<Vec<String>>, cx: &mut Context<Self>) {
        self.search_ids = ids;
        self.selected = None;
        self.reload();
        cx.notify();
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
        cx.notify();
        let Some(row) = self.row(i).cloned() else {
            self.pending_open = Some(i);
            return;
        };
        self.pending_open = None;
        self.open_row(row, cx);
    }

    /// При первом открытии письмо помечается прочитанным, затем уходит в область чтения.
    fn open_row(&mut self, mut row: EmailRow, cx: &mut Context<Self>) {
        if !row.seen {
            row.seen = true;
            self.flags.set(&row.id, true, row.flagged);
            self.patch_row(&row);
        }
        cx.emit(OpenRow(row));
    }

    fn patch_row(&mut self, row: &EmailRow) {
        for r in self.rows.1.iter_mut().filter(|r| r.id == row.id) {
            *r = row.clone();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let cur = self.selected.map_or(0, |i| i as isize);
        let next = if self.selected.is_none() { 0 } else { (cur + delta).max(0) as usize };
        self.select(next, cx);
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

    /// Замер: строка `i` — первая видимая.
    fn scroll_to_top(&mut self, i: usize, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(i, ScrollStrategy::Top);
        cx.notify();
    }
}

impl Render for MailList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().border_r_1().border_color(rgb(0xdddddd)).child(
            uniform_list(
                "messages",
                self.count,
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
}

// --- папки ---

struct Folders {
    mailboxes: Vec<MailboxRow>,
    current: Option<String>,
    /// Число результатов, пока показан поиск.
    results: Option<usize>,
    section: Section,
}

enum FolderEvent {
    Open(String),
    Section(Section),
}

impl EventEmitter<FolderEvent> for Folders {}

impl Render for Folders {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().size_full().border_r_1().border_color(rgb(0xdddddd)).bg(rgb(0xf7f7f7));
        if let Some(n) = self.results {
            col = col.child(div().px_3().py_1().bg(rgb(0xcce4ff)).child(format!("🔍 Результаты ({n})")));
        }
        let mut list = div().flex().flex_col().flex_1().p_1();
        for m in &self.mailboxes {
            let selected = self.results.is_none() && self.current.as_deref() == Some(&m.id);
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
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderEvent::Open(id.clone())))),
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
}

fn nav(id: &'static str, label: &'static str, active: bool, section: Section, cx: &mut Context<Folders>) -> impl IntoElement {
    div()
        .id(id)
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(rgb(0xcce4ff)))
        .child(label)
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderEvent::Section(section))))
}

// --- область чтения ---

struct ReadingPane {
    worker: Arc<Mutex<Worker>>,
    reading: Option<Reading>,
}

/// Уровень песочницы последнего разбора — для строки состояния.
struct SandboxLevel(&'static str);

impl EventEmitter<SandboxLevel> for ReadingPane {}

impl ReadingPane {
    fn open(&mut self, row: EmailRow, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        let body = Some(postman_demo::body(&row.id));
        self.reading = Some(Reading { row, body, doc: None });
        self.render_body(cx);
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.reading = None;
        cx.notify();
    }

    /// HTML → IR в воркере-песочнице, в фоновом исполнителе gpui.
    fn render_body(&mut self, cx: &mut Context<Self>) {
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
                cx.emit(SandboxLevel(match level {
                    Level::Full => "Песочница: полная",
                    Level::Partial => "Песочница: частичная",
                    Level::None => "Песочница: нет",
                }));
                if let Some(r) = this.reading.as_mut().filter(|r| r.row.id == id) {
                    r.doc = Some(result.unwrap_or_else(|e| postman_ir::from_plain(&format!("⚠ {e}"))));
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
            return div().flex().size_full().items_center().justify_center().text_color(rgb(0x888888)).child("Выберите письмо").into_any_element();
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
            .size_full()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

// --- строка команд ---

/// Высота строки команд: кэшированное view не измеряется по содержимому, размер задаётся явно.
const BAR_HEIGHT: f32 = 40.0;

struct CommandBar {
    search: Entity<InputState>,
}

impl Render for CommandBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(0xdddddd))
            .children(["✉ Создать", "↩ Ответить", "↩↩ Ответить всем", "↪ Переслать", "🗑 Удалить", "⚑ Флаг"].map(|l| {
                div().px_2().py_0p5().rounded_md().bg(rgb(0xececec)).child(l)
            }))
            .child(div().flex_1())
            .child(div().w(px(320.)).child(Input::new(&self.search)))
    }
}

// --- экран ---

/// Экран почты: связывает части, держит хоткеи, поиск, календарь и строку состояния.
/// Части — отдельные кэшируемые view; заново строится та, что получила `notify`.
struct Mailer {
    cache: Cache,
    focus: FocusHandle,
    list: Entity<MailList>,
    folders: Entity<Folders>,
    reading: Entity<ReadingPane>,
    bar: Entity<CommandBar>,
    search: Entity<InputState>,
    sandbox: &'static str,
    /// Последнее нажатие и сработавшее действие — проверка хоткеев в любой раскладке (S1-01).
    last_key: String,
    bench: Option<Bench>,
    section: Section,
    cal: calendar::CalendarView,
    _subscriptions: Vec<Subscription>,
}

impl Mailer {
    fn new(cache: Cache, bench: Option<Bench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск (Ctrl+E), Enter — искать, Esc — сбросить"));
        let keylog = std::env::var_os("POSTMAN_KEYLOG").is_some();
        let on_search = cx.subscribe_in(&search, window, move |this: &mut Self, state, ev: &InputEvent, _window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let q = state.read(cx).value().to_string();
                this.run_search(&q, cx);
            }
            InputEvent::Change if keylog => println!("search \"{}\"", state.read(cx).value()),
            _ => {}
        });
        let mailboxes = cache.mailboxes().unwrap_or_default();
        let inbox = mailboxes.iter().find(|b| b.role.as_deref() == Some("inbox")).or(mailboxes.first()).map(|b| b.id.clone());
        let list = cx.new(|cx| {
            let mut l = MailList::new(cache.clone());
            if let Some(id) = inbox.clone() {
                l.show_mailbox(id, cx);
            }
            l
        });
        let folders = cx.new(|_| Folders { mailboxes, current: inbox, results: None, section: Section::Mail });
        let reading = cx.new(|_| ReadingPane {
            worker: Arc::new(Mutex::new(Worker::new(Worker::default_exe(), Duration::from_secs(5)))),
            reading: None,
        });
        let bar = cx.new(|_| CommandBar { search: search.clone() });
        let subscriptions = vec![
            on_search,
            Self::observe_keys(cx),
            cx.subscribe(&list, |this, _, OpenRow(row), cx| {
                this.reading.update(cx, |r, cx| r.open(row.clone(), cx));
            }),
            cx.subscribe(&folders, |this, _, ev: &FolderEvent, cx| match ev {
                FolderEvent::Open(id) => this.open_mailbox(id.clone(), cx),
                FolderEvent::Section(s) => this.set_section(*s, cx),
            }),
            cx.subscribe(&reading, |this, _, SandboxLevel(level), cx| {
                this.sandbox = level;
                cx.notify();
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            cache,
            focus,
            list,
            folders,
            reading,
            bar,
            search,
            sandbox: "",
            last_key: String::new(),
            bench,
            section: Section::Mail,
            cal: calendar::CalendarView::demo(),
            _subscriptions: subscriptions,
        }
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

    fn open_mailbox(&mut self, id: String, cx: &mut Context<Self>) {
        self.folders.update(cx, |f, cx| {
            f.current = Some(id.clone());
            f.results = None;
            cx.notify();
        });
        self.list.update(cx, |l, cx| l.show_mailbox(id, cx));
        self.reading.update(cx, |r, cx| r.clear(cx));
    }

    fn run_search(&mut self, q: &str, cx: &mut Context<Self>) {
        let q = q.trim();
        let mailbox = self.list.read(cx).mailbox.clone();
        let ids = (!q.is_empty()).then(|| self.cache.search(q, mailbox.as_deref(), 1000).unwrap_or_default());
        let results = ids.as_ref().map(Vec::len);
        self.folders.update(cx, |f, cx| {
            f.results = results;
            cx.notify();
        });
        self.list.update(cx, |l, cx| l.show_search(ids, cx));
        self.reading.update(cx, |r, cx| r.clear(cx));
    }

    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.folders.update(cx, |f, cx| {
            f.section = section;
            cx.notify();
        });
        cx.notify();
    }

    /// Сдвиг выделения; `delta` считается по состоянию списка (страница, число писем).
    fn step(&mut self, delta: impl Fn(&MailList) -> isize, cx: &mut Context<Self>) {
        self.list.update(cx, |l, cx| {
            let d = delta(l);
            l.step(d, cx);
        });
    }
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

fn list_row(i: usize, r: &EmailRow, selected: bool, cx: &mut Context<MailList>) -> AnyElement {
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
        .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
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
        let status = format!("Элементов: {}", self.list.read(cx).count);
        let root = div()
            .key_context("Mailer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &mail::Next, _, cx| this.step(|_| 1, cx)))
            .on_action(cx.listener(|this, _: &mail::Prev, _, cx| this.step(|_| -1, cx)))
            .on_action(cx.listener(|this, _: &mail::First, _, cx| this.step(|l| -(l.count as isize), cx)))
            .on_action(cx.listener(|this, _: &mail::Last, _, cx| this.step(|l| l.count as isize, cx)))
            .on_action(cx.listener(|this, _: &mail::PageDown, _, cx| this.step(MailList::page, cx)))
            .on_action(cx.listener(|this, _: &mail::PageUp, _, cx| this.step(|l| -l.page(), cx)))
            .on_action(cx.listener(|this, _: &mail::MarkRead, _, cx| this.list.update(cx, |l, cx| l.set_flags(Some(true), false, cx))))
            .on_action(cx.listener(|this, _: &mail::MarkUnread, _, cx| this.list.update(cx, |l, cx| l.set_flags(Some(false), false, cx))))
            .on_action(cx.listener(|this, _: &mail::ToggleFlag, _, cx| this.list.update(cx, |l, cx| l.set_flags(None, true, cx))))
            .on_action(cx.listener(|this, _: &view::Mail, _, cx| this.set_section(Section::Mail, cx)))
            .on_action(cx.listener(|this, _: &view::Calendar, _, cx| this.set_section(Section::Calendar, cx)))
            .on_action(cx.listener(|this, _: &commands::calendar::NewMeeting, _, cx| {
                this.cal.assistant = !this.cal.assistant;
                this.set_section(Section::Calendar, cx);
            }))
            .on_action(cx.listener(|this, _: &search::Focus, window, cx| {
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &search::Clear, window, cx| {
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
            .child(self.bar.clone().cached(StyleRefinement::default().w_full().h(px(BAR_HEIGHT)).flex_none()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.folders.clone().cached(StyleRefinement::default().w(px(200.)).h_full().flex_none()))
                    .when(self.section == Section::Mail, |d| {
                        d.child(self.list.clone().cached(StyleRefinement::default().w(px(380.)).h_full().flex_none()))
                            .child(self.reading.clone().cached(StyleRefinement::default().flex_1().min_w_0().h_full()))
                    })
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
        let count = self.list.read(cx).count;
        let Some(b) = &mut self.bench else { return };
        match b.frame(count) {
            Step::Wait => {}
            Step::Select(i) => self.list.update(cx, |l, cx| l.select(i, cx)),
            Step::Scroll(i) => self.list.update(cx, |l, cx| l.scroll_to_top(i, cx)),
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

/// Запуск невозможен (нет каталога данных, хранилище или окно не открылись): сообщение и выход с кодом 1,
/// без паники. В журнал Windows — с S2-06 (pets-shell).
fn fatal(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("Postman не запустился: {what}: {e}");
    std::process::exit(1)
}

fn main() {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n = args.iter().position(|a| a == "--demo").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let bench = args.iter().any(|a| a == "--bench");
    let dir = data_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fatal("каталог данных", e));
    // Бенч — на свежей копии ящика; подготовка данных не входит в first_frame_ms.
    let prepared = Instant::now();
    let path = if bench {
        postman_demo::bench_copy(&dir, n).unwrap_or_else(|e| fatal("ящик для замера", e))
    } else {
        let path = dir.join("demo.redb");
        postman_demo::ensure(&path, n).unwrap_or_else(|e| fatal("демо-ящик", e));
        path
    };
    let start = if bench { start + prepared.elapsed() } else { start };
    let cache = Cache::open(&path).unwrap_or_else(|e| fatal("хранилище", e));

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // Клавиши — из реестра команд (S2-02); в поле ввода клавиши без модификаторов — текст.
        commands::bind(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            let bench = bench.then(|| Bench::new(start));
            let cache = cache.clone();
            cx.new(|cx| Mailer::new(cache, bench, window, cx))
        });
        if let Err(e) = opened {
            fatal("окно", e);
        }
        cx.activate(true);
    });
}

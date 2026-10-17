//! Оболочка в духе classic Outlook: панель команд, дерево папок с навигацией
//! внизу, список писем и область чтения.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui::{
    self, Align2, Color32, FontId, Key, KeyboardShortcut, Modifiers, Rect, Sense, TextStyle, Vec2, pos2,
};
use postman_cache::{AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow};
use postman_ir::Document;
use tokio::sync::mpsc::UnboundedSender;

use postman_compose::{Address, Attachment, Draft, Mode, Original};

use crate::composer::{Action, Composer};
use crate::ir_view::Images;
use crate::render::{Rendered, Renderer};
use crate::{bench, files, ir_view, sync};
use postman_demo as demo;

const ROW_HEIGHT: f32 = 40.0;
/// Сколько строк сверх видимых держать в окне списка.
const WINDOW_MARGIN: usize = 100;

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Mail,
    Calendar,
    Contacts,
}

/// Результаты поиска вместо содержимого папки.
struct SearchView {
    ticket: u64,
    query: String,
    ids: Vec<String>,
    /// Ждём ответа сервера (пока показаны локальные совпадения).
    pending: bool,
    total: Option<u64>,
}

struct ListWindow {
    offset: usize,
    rows: Vec<EmailRow>,
}

struct Reading {
    row: EmailRow,
    body: Option<BodyRow>,
    doc: Option<Document>,
    /// Картинки `cid:` показываются только по разрешению (или для доверенного отправителя).
    show_images: bool,
    images: Images,
    requested: HashSet<String>,
}

#[derive(Clone, Copy)]
enum FileAct {
    Open,
    SaveAs,
}

pub struct App {
    cache: Cache,
    demo: bool,
    section: Section,
    mailboxes: Vec<MailboxRow>,
    mailbox: Option<String>,
    count: usize,
    window: ListWindow,
    visible: Range<usize>,
    selected: Option<usize>,
    scroll_to_selected: bool,
    reading: Option<Reading>,
    search: String,
    focus_search: bool,
    search_all: bool,
    search_view: Option<SearchView>,
    index: sync::Index,
    extractor: Option<crate::extract::Extractor>,
    last_index_commit: std::time::Instant,
    /// Ящик, для которого уже запрошена следующая страница.
    loading_more: Option<String>,
    status: String,
    commands: Option<UnboundedSender<sync::Command>>,
    events: mpsc::Receiver<sync::Event>,
    renderer: Renderer,
    sandbox: &'static str,
    composers: Vec<Composer>,
    /// Действия с вложениями, ждущие скачивания blob'а.
    pending_files: Vec<(AttachmentRow, FileAct)>,
    open_dir: PathBuf,
    next_ticket: u64,
    me: Address,
    signature: String,
    /// Адаптер и API, на которых идёт рендер, — для статус-строки.
    gpu: String,
    bench: Option<bench::Bench>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        db: PathBuf,
        account: Option<sync::Account>,
        demo: bool,
        bench: Option<bench::Bench>,
    ) -> Self {
        let cache = Cache::open(&db).expect("кэш");
        let open_dir = db.parent().map(|d| d.join("open")).unwrap_or_default();
        let index_dir = db.with_extension("search");
        let index = postman_search::SearchIndex::open(&index_dir).ok().map(Arc::new);
        let extractor = index.clone().map(|i| crate::extract::Extractor::spawn(cache.clone(), i));
        if let (Some(i), Some(x)) = (index.clone().filter(|i| i.is_empty()), &extractor) {
            build_index(i, cache.clone(), x);
        }
        let (ev_tx, ev_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        let commands = account.map(|a| sync::spawn(a, cache.clone(), index.clone(), ev_tx, move || ctx.request_repaint()));
        let status = match (&commands, demo) {
            (_, true) => "Демо-режим".to_string(),
            (Some(_), _) => "Запуск синхронизации…".to_string(),
            (None, _) => "Нет учётной записи: задайте POSTMAN_URL/POSTMAN_USER/POSTMAN_PASSWORD или --demo".to_string(),
        };
        configure_style(&cc.egui_ctx);
        let gpu = cc
            .wgpu_render_state
            .as_ref()
            .map(|r| {
                let i = r.adapter.get_info();
                format!("{} · {:?} · {:?}", i.name, i.backend, i.device_type)
            })
            .unwrap_or_default();
        let ctx = cc.egui_ctx.clone();
        let renderer = Renderer::spawn(move || ctx.request_repaint());
        let mut app = Self {
            cache,
            demo,
            section: Section::Mail,
            mailboxes: Vec::new(),
            mailbox: None,
            count: 0,
            window: ListWindow { offset: 0, rows: Vec::new() },
            visible: 0..0,
            selected: None,
            scroll_to_selected: false,
            reading: None,
            search: String::new(),
            focus_search: false,
            search_all: false,
            search_view: None,
            index,
            extractor,
            last_index_commit: std::time::Instant::now(),
            loading_more: None,
            status,
            commands,
            events: ev_rx,
            renderer,
            sandbox: "",
            composers: Vec::new(),
            pending_files: Vec::new(),
            open_dir,
            next_ticket: 1,
            me: if demo { Address::new("Я", "me@corp.example") } else { Address::default() },
            signature: String::new(),
            gpu,
            bench,
        };
        app.reload_mailboxes();
        app
    }

    fn reload_mailboxes(&mut self) {
        self.mailboxes = self.cache.mailboxes().unwrap_or_default();
        if self.mailbox.is_none() {
            let inbox = self.mailboxes.iter().find(|m| m.role.as_deref() == Some("inbox")).or(self.mailboxes.first());
            if let Some(id) = inbox.map(|m| m.id.clone()) {
                self.open_mailbox(id);
            }
        }
    }

    fn open_mailbox(&mut self, id: String) {
        self.search_view = None;
        self.mailbox = Some(id);
        self.selected = None;
        self.reading = None;
        self.reload_list();
    }

    fn reload_list(&mut self) {
        self.count = match (&self.search_view, &self.mailbox) {
            (Some(sv), _) => sv.ids.len(),
            (None, Some(mb)) => self.cache.count(mb).unwrap_or(0),
            (None, None) => return,
        };
        self.window.rows.clear();
        self.ensure_window(self.visible.clone());
    }

    /// Подгрузить из кэша окно строк вокруг видимого диапазона.
    fn ensure_window(&mut self, range: Range<usize>) {
        let w = &self.window;
        let covered = range.start >= w.offset && range.end <= w.offset + w.rows.len();
        if covered && !w.rows.is_empty() {
            return;
        }
        let offset = range.start.saturating_sub(WINDOW_MARGIN);
        let limit = range.len() + 2 * WINDOW_MARGIN;
        let rows = match (&self.search_view, &self.mailbox) {
            (Some(sv), _) => {
                let end = (offset + limit).min(sv.ids.len());
                self.cache.rows_by_ids(&sv.ids[offset.min(end)..end]).unwrap_or_default()
            }
            (None, Some(mb)) => self.cache.page(mb, offset, limit).unwrap_or_default(),
            (None, None) => return,
        };
        self.window = ListWindow { offset, rows };
    }

    /// Поиск: сразу локальные совпадения из кэша, затем — ответ сервера.
    fn start_search(&mut self) {
        let query = self.search.trim().to_string();
        if query.is_empty() {
            self.stop_search();
            return;
        }
        let scope = if self.search_all { None } else { self.mailbox.clone() };
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let ids = match &self.index {
            Some(i) => {
                let _ = i.commit();
                let ids = i.search(&query, 1000).unwrap_or_default();
                match &scope {
                    Some(mb) => self.cache.filter_mailbox(&ids, mb).unwrap_or_default(),
                    None => ids,
                }
            }
            None => self.cache.search(&query, scope.as_deref(), 1000).unwrap_or_default(),
        };
        let pending = match &self.commands {
            Some(tx) => tx.send(sync::Command::Search { query: query.clone(), mailbox: scope, ticket }).is_ok(),
            None => false,
        };
        self.search_view = Some(SearchView { ticket, query, ids, pending, total: None });
        self.selected = None;
        self.reading = None;
        self.reload_list();
    }

    fn stop_search(&mut self) {
        if self.search_view.take().is_some() {
            self.selected = None;
            self.reading = None;
            self.reload_list();
        }
    }

    /// Ближе к концу списка — догрузить следующую страницу ящика с сервера.
    fn maybe_load_more(&mut self) {
        let (Some(mb), None, Some(tx)) = (&self.mailbox, &self.search_view, &self.commands) else { return };
        if self.loading_more.is_some() || self.visible.end + 100 < self.count {
            return;
        }
        let total = self.mailboxes.iter().find(|m| &m.id == mb).map_or(0, |m| m.total as usize);
        if total > self.count {
            let _ = tx.send(sync::Command::LoadMore { mailbox: mb.clone() });
            self.loading_more = Some(mb.clone());
        }
    }

    fn row(&self, i: usize) -> Option<&EmailRow> {
        i.checked_sub(self.window.offset).and_then(|j| self.window.rows.get(j))
    }

    fn select(&mut self, i: usize) {
        if self.count == 0 {
            return;
        }
        let i = i.min(self.count - 1);
        self.selected = Some(i);
        self.scroll_to_selected = true;
        self.ensure_window(i..i + 1);
        let Some(row) = self.row(i).cloned() else { return };
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        // Как в Outlook: письмо помечается прочитанным при открытии.
        if !row.seen {
            self.set_flags(&row, true, row.flagged);
        }
        let body = self.load_body(&row.id);
        let show_images = self.cache.is_trusted(&row.from_addr).unwrap_or(false);
        self.reading = Some(Reading { row, body, doc: None, show_images, images: Images::new(), requested: HashSet::new() });
        self.render_body();
    }

    fn load_body(&self, id: &str) -> Option<BodyRow> {
        if self.demo {
            return Some(demo::body(id));
        }
        let b = self.cache.body(id).ok().flatten();
        if b.is_none()
            && let Some(tx) = &self.commands
        {
            let _ = tx.send(sync::Command::Body(id.to_string()));
        }
        b
    }

    /// HTML -> IR в изолированном процессе.
    fn render_body(&mut self) {
        let Some(r) = &self.reading else { return };
        let Some(body) = r.body.clone() else { return };
        self.renderer.request(r.row.id.clone(), body);
    }

    fn set_flags(&mut self, row: &EmailRow, seen: bool, flagged: bool) {
        let _ = self.cache.set_flags(&row.id, seen, flagged);
        if let Some(tx) = &self.commands {
            if seen != row.seen {
                let _ = tx.send(sync::Command::Keyword { id: row.id.clone(), keyword: "$seen", on: seen });
            }
            if flagged != row.flagged {
                let _ = tx.send(sync::Command::Keyword { id: row.id.clone(), keyword: "$flagged", on: flagged });
            }
        }
        for r in self.window.rows.iter_mut().filter(|r| r.id == row.id) {
            r.seen = seen;
            r.flagged = flagged;
        }
        if let Some(rd) = self.reading.as_mut().filter(|rd| rd.row.id == row.id) {
            rd.row.seen = seen;
            rd.row.flagged = flagged;
        }
    }

    fn selected_row(&self) -> Option<EmailRow> {
        self.selected.and_then(|i| self.row(i)).cloned()
    }

    fn delete_selected(&mut self) {
        let (Some(row), Some(from)) = (self.selected_row(), self.mailbox.clone()) else { return };
        let Some(trash) = self.mailboxes.iter().find(|m| m.role.as_deref() == Some("trash")).map(|m| m.id.clone())
        else {
            return;
        };
        if from == trash {
            self.status = "Окончательное удаление — в следующей итерации".into();
            return;
        }
        let mut moved = row.clone();
        moved.mailbox_ids = vec![trash.clone()];
        let _ = self.cache.upsert_emails(&[moved]);
        if let Some(tx) = &self.commands {
            let _ = tx.send(sync::Command::Move { id: row.id, from, to: trash });
        }
        let keep = self.selected;
        self.reading = None;
        self.reload_list();
        if let Some(i) = keep {
            self.select(i);
        }
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                sync::Event::Status(s) => self.status = s,
                sync::Event::Error(e) => self.status = format!("Ошибка: {e}"),
                sync::Event::BlobReady { blob_id, data } => self.blob_ready(&blob_id, data),
                sync::Event::BodiesStored(ids) => {
                    if let Some(x) = &self.extractor {
                        x.queue(ids);
                    }
                }
                sync::Event::LoadedMore { mailbox } => {
                    if self.loading_more.as_ref() == Some(&mailbox) {
                        self.loading_more = None;
                    }
                }
                sync::Event::SearchResults { ticket, ids, total } => {
                    if let Some(sv) = self.search_view.as_mut().filter(|sv| sv.ticket == ticket) {
                        // Сначала локальные (нечёткие) совпадения, затем то, что нашёл только сервер.
                        let have: HashSet<String> = sv.ids.iter().cloned().collect();
                        sv.ids.extend(ids.into_iter().filter(|id| !have.contains(id)));
                        sv.pending = false;
                        sv.total = Some(total);
                        self.reload_list();
                    }
                }
                sync::Event::BlobFailed { blob_id, error } => {
                    self.pending_files.retain(|(a, _)| a.blob_id != blob_id);
                    self.status = format!("Не удалось скачать вложение: {error}");
                }
                sync::Event::Me { address, signature } => {
                    self.me = address;
                    self.signature = signature;
                }
                sync::Event::Sent { ticket } => {
                    if let Some(c) = self.composer(ticket) {
                        c.sent();
                    }
                    self.status = "Письмо отправлено".into();
                }
                sync::Event::DraftSaved { ticket, id } => {
                    if let Some(c) = self.composer(ticket) {
                        c.saved(id);
                    }
                }
                sync::Event::ComposeFailed { ticket, error } => {
                    if let Some(c) = self.composer(ticket) {
                        c.failed(error);
                    }
                }
                sync::Event::MailboxesChanged => self.reload_mailboxes(),
                sync::Event::EmailsChanged => {
                    self.reload_list();
                    if let Some(r) = &self.reading {
                        let id = r.row.id.clone();
                        self.ensure_window(self.visible.clone());
                        self.selected = self.window.rows.iter().position(|r| r.id == id).map(|p| p + self.window.offset);
                    }
                }
                sync::Event::BodyReady(id) => {
                    if self.reading.as_ref().is_some_and(|r| r.row.id == id) {
                        let body = self.cache.body(&id).ok().flatten();
                        if let Some(r) = self.reading.as_mut() {
                            r.body = body;
                        }
                        self.render_body();
                    }
                }
            }
        }
        while let Ok(done) = self.renderer.rx.try_recv() {
            match done {
                Rendered::Body { id, doc, level } => {
                    self.sandbox = match level {
                        postman_worker::Level::Full => "Песочница: полная",
                        postman_worker::Level::Partial => "Песочница: частичная",
                        postman_worker::Level::None => "Песочница: нет",
                    };
                    if let Some(r) = self.reading.as_mut().filter(|r| r.row.id == id) {
                        if let (Some(i), Some(body)) = (&self.index, &r.body) {
                            crate::indexer::full(i, &r.row, body, &postman_ir::to_markdown(&doc));
                        }
                        r.doc = Some(doc);
                    }
                }
                Rendered::Image { key, result } => {
                    let Some((id, cid)) = key.split_once('\n') else { continue };
                    let Some(r) = self.reading.as_mut().filter(|r| r.row.id == id) else { continue };
                    match result {
                        Ok(img) => {
                            let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
                            let tex = ctx.load_texture(&key, color, egui::TextureOptions::LINEAR);
                            r.images.insert(cid.to_string(), tex);
                        }
                        Err(e) => self.status = format!("Картинка не показана: {e}"),
                    }
                }
            }
        }
    }

    /// Тексты открытых писем попадают в индекс пакетом, не чаще раза в несколько секунд.
    fn commit_index(&mut self) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(5);
        let Some(i) = &self.index else { return };
        if i.dirty() && self.last_index_commit.elapsed() >= EVERY {
            let _ = i.commit();
            self.last_index_commit = std::time::Instant::now();
        }
    }

    /// Байты вложения, если они уже есть локально.
    fn local_blob(&self, blob_id: &str) -> Option<Arc<Vec<u8>>> {
        if self.demo {
            return demo::blob(blob_id).map(Arc::new);
        }
        self.cache.blob(blob_id).ok().flatten().map(Arc::new)
    }

    fn request_blob(&self, a: &AttachmentRow) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(sync::Command::Blob { blob_id: a.blob_id.clone(), name: a.name.clone(), mime: a.mime.clone() });
        }
    }

    /// Запросить декодирование ещё не показанных картинок `cid:` текущего письма.
    fn ensure_images(&mut self) {
        let Some(r) = &self.reading else { return };
        let (Some(doc), Some(body)) = (&r.doc, &r.body) else { return };
        if !r.show_images {
            return;
        }
        let wanted: Vec<(String, AttachmentRow)> = ir_view::cids(doc)
            .into_iter()
            .filter(|cid| !r.requested.contains(cid))
            .filter_map(|cid| {
                let a = body.attachments.iter().find(|a| a.cid.as_deref() == Some(cid.as_str()))?;
                Some((cid, a.clone()))
            })
            .collect();
        let id = r.row.id.clone();
        for (cid, a) in wanted {
            match self.local_blob(&a.blob_id) {
                Some(bytes) => self.renderer.decode(format!("{id}\n{cid}"), bytes),
                None => self.request_blob(&a),
            }
            if let Some(r) = self.reading.as_mut() {
                r.requested.insert(cid);
            }
        }
    }

    fn blob_ready(&mut self, blob_id: &str, data: Arc<Vec<u8>>) {
        if let Some(r) = &self.reading
            && let Some(body) = &r.body
        {
            for a in body.attachments.iter().filter(|a| a.blob_id == blob_id) {
                if let Some(cid) = &a.cid
                    && r.requested.contains(cid)
                {
                    self.renderer.decode(format!("{}\n{cid}", r.row.id), data.clone());
                }
            }
        }
        let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_files).into_iter().partition(|(a, _)| a.blob_id == blob_id);
        self.pending_files = rest;
        for (a, act) in ready {
            self.do_file(&a, act, &data);
        }
    }

    fn file_action(&mut self, a: AttachmentRow, act: FileAct) {
        match self.local_blob(&a.blob_id) {
            Some(data) => self.do_file(&a, act, &data),
            None if self.commands.is_some() => {
                self.status = format!("Скачивание «{}»…", a.name);
                self.request_blob(&a);
                self.pending_files.push((a, act));
            }
            None => self.status = "Вложение недоступно без подключения".into(),
        }
    }

    fn do_file(&mut self, a: &AttachmentRow, act: FileAct, data: &[u8]) {
        let r = match act {
            FileAct::Open => files::open(&self.open_dir, &a.name, data).map(Some),
            FileAct::SaveAs => files::save_as(&a.name, data),
        };
        self.status = match r {
            Ok(Some(p)) => format!("{} — {}", files::safe_name(&a.name), p.display()),
            Ok(None) => String::new(),
            Err(e) => format!("«{}»: {e}", files::safe_name(&a.name)),
        };
    }

    fn hotkeys(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let cmd = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let mut action = None;
        ctx.input_mut(|i| {
            let mut hit = |s: KeyboardShortcut, a: &'static str| {
                if action.is_none() && i.consume_shortcut(&s) {
                    action = Some(a);
                }
            };
            hit(cmd(ctrl, Key::Num1), "mail");
            hit(cmd(ctrl, Key::Num2), "calendar");
            hit(cmd(ctrl, Key::Num3), "contacts");
            hit(cmd(ctrl, Key::E), "search");
            hit(cmd(ctrl_shift, Key::R), "reply_all");
            hit(cmd(ctrl, Key::R), "reply");
            hit(cmd(ctrl, Key::F), "forward");
            hit(cmd(ctrl, Key::N), "new");
            hit(cmd(ctrl, Key::Q), "read");
            hit(cmd(ctrl, Key::U), "unread");
            if !typing {
                hit(cmd(Modifiers::NONE, Key::Delete), "delete");
                hit(cmd(Modifiers::NONE, Key::Insert), "flag");
                hit(cmd(Modifiers::NONE, Key::J), "next");
                hit(cmd(Modifiers::NONE, Key::ArrowDown), "next");
                hit(cmd(Modifiers::NONE, Key::K), "prev");
                hit(cmd(Modifiers::NONE, Key::ArrowUp), "prev");
                hit(cmd(Modifiers::NONE, Key::Home), "first");
                hit(cmd(Modifiers::NONE, Key::End), "last");
                hit(cmd(Modifiers::NONE, Key::PageDown), "page_down");
                hit(cmd(Modifiers::NONE, Key::PageUp), "page_up");
            }
        });
        if let Some(a) = action {
            self.run(a);
        }
    }

    fn run(&mut self, action: &str) {
        let page = self.visible.len().max(1);
        let cur = self.selected;
        match action {
            "mail" => self.section = Section::Mail,
            "calendar" => self.section = Section::Calendar,
            "contacts" => self.section = Section::Contacts,
            "search" => self.focus_search = true,
            "next" => self.select(cur.map_or(0, |i| i + 1)),
            "prev" => self.select(cur.map_or(0, |i| i.saturating_sub(1))),
            "first" => self.select(0),
            "last" => self.select(self.count.saturating_sub(1)),
            "page_down" => self.select(cur.map_or(0, |i| i + page)),
            "page_up" => self.select(cur.map_or(0, |i| i.saturating_sub(page))),
            "read" | "unread" => {
                if let Some(r) = self.selected_row() {
                    self.set_flags(&r, action == "read", r.flagged);
                }
            }
            "flag" => {
                if let Some(r) = self.selected_row() {
                    self.set_flags(&r, r.seen, !r.flagged);
                }
            }
            "delete" => self.delete_selected(),
            "new" => {
                let markdown = if self.signature.is_empty() { String::new() } else { format!("\n\n{}", self.signature) };
                self.open_composer(Draft { from: self.me.clone(), markdown, ..Default::default() });
            }
            "reply" | "reply_all" | "forward" => {
                let mode = match action {
                    "reply" => Mode::Reply,
                    "reply_all" => Mode::ReplyAll,
                    _ => Mode::Forward,
                };
                match self.respond(mode) {
                    Some(d) => self.open_composer(d),
                    None => self.status = "Письмо ещё загружается".into(),
                }
            }
            _ => {}
        }
    }

    fn respond(&self, mode: Mode) -> Option<Draft> {
        let r = self.reading.as_ref()?;
        let (body, doc) = (r.body.as_ref()?, r.doc.as_ref()?);
        let addrs = |v: &[postman_cache::Addr]| v.iter().map(|a| Address::new(&a.name, &a.email)).collect::<Vec<_>>();
        let orig = Original {
            message_id: body.message_id.clone(),
            references: body.references.clone(),
            from: vec![Address::new(&r.row.from_name, &r.row.from_addr)],
            reply_to: addrs(&body.reply_to),
            to: addrs(&body.to),
            cc: addrs(&body.cc),
            subject: r.row.subject.clone(),
            date: format_full_date(r.row.received_at),
        };
        let mut d = postman_compose::respond(&orig, doc, mode, &self.me, &self.signature);
        // Пересылка — все вложения; ответ — только картинки, на которые ссылается цитата.
        let quoted: HashSet<String> = ir_view::cids(doc).into_iter().collect();
        for a in &body.attachments {
            let keep = mode == Mode::Forward || a.cid.as_ref().is_some_and(|c| quoted.contains(c));
            if keep {
                d.attachments.push(Attachment {
                    name: a.name.clone(),
                    mime: a.mime.clone(),
                    data: self.local_blob(&a.blob_id).map(|b| b.to_vec()).unwrap_or_default(),
                    inline_cid: a.cid.clone(),
                    blob_id: Some(a.blob_id.clone()),
                });
            }
        }
        Some(d)
    }

    fn open_composer(&mut self, draft: Draft) {
        self.composers.push(Composer::new(self.next_ticket, draft));
        self.next_ticket += 1;
    }

    fn composer(&mut self, ticket: u64) -> Option<&mut Composer> {
        self.composers.iter_mut().find(|c| c.ticket == ticket)
    }

    fn show_composers(&mut self, ctx: &egui::Context) {
        let mut actions = Vec::new();
        for c in &mut self.composers {
            if let Some(a) = c.show(ctx) {
                actions.push((c.ticket, a));
            }
        }
        for (ticket, a) in actions {
            let cmd = match a {
                Action::Send(draft, replace) => sync::Command::Send { draft, replace, ticket },
                Action::Save(draft, replace) => sync::Command::SaveDraft { draft, replace, ticket },
            };
            match &self.commands {
                Some(tx) => {
                    let _ = tx.send(cmd);
                }
                None => {
                    if let Some(c) = self.composer(ticket) {
                        c.failed("нет подключения к серверу (демо-режим)".into());
                    }
                }
            }
        }
        self.composers.retain(|c| c.open);
    }

    fn command_bar(&mut self, ui: &mut egui::Ui) {
        let mut search = None;
        ui.horizontal(|ui| {
            for (label, action, hint) in [
                ("✉ Создать", "new", "Ctrl+N"),
                ("↩ Ответить", "reply", "Ctrl+R"),
                ("↩↩ Ответить всем", "reply_all", "Ctrl+Shift+R"),
                ("↪ Переслать", "forward", "Ctrl+F"),
                ("🗑 Удалить", "delete", "Del"),
                ("⚑ Флаг", "flag", "Insert"),
            ] {
                if ui.button(label).on_hover_text(hint).clicked() {
                    self.run(action);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.search_all, "Все папки").on_hover_text("Искать во всех папках, а не только в текущей");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Поиск (Ctrl+E), Enter — искать, Esc — сбросить")
                        .desired_width(260.0),
                );
                if self.focus_search {
                    r.request_focus();
                    self.focus_search = false;
                }
                if r.lost_focus() {
                    ui.input(|i| {
                        if i.key_pressed(Key::Enter) {
                            search = Some(true);
                        } else if i.key_pressed(Key::Escape) {
                            search = Some(false);
                        }
                    });
                }
            });
        });
        match search {
            Some(true) => self.start_search(),
            Some(false) => {
                self.search.clear();
                self.stop_search();
            }
            None => {}
        }
    }

    fn folders(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("nav").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, s, hint) in [
                    ("✉ Почта", Section::Mail, "Ctrl+1"),
                    ("📅 Календарь", Section::Calendar, "Ctrl+2"),
                    ("👥 Контакты", Section::Contacts, "Ctrl+3"),
                ] {
                    if ui.selectable_label(self.section == s, label).on_hover_text(hint).clicked() {
                        self.section = s;
                    }
                }
            });
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(sv) = &self.search_view {
                let status = if sv.pending { " …".to_string() } else { sv.total.map(|t| format!(" ({t})")).unwrap_or_default() };
                let _ = ui.selectable_label(true, format!("🔍 «{}»{status}", sv.query));
                ui.separator();
            }
            let mut open = None;
            let roots: Vec<MailboxRow> = self.mailboxes.iter().filter(|m| m.parent_id.is_none()).cloned().collect();
            for m in &roots {
                self.folder(ui, m, 0, &mut open);
            }
            if let Some(id) = open {
                self.open_mailbox(id);
            }
        });
    }

    fn folder(&self, ui: &mut egui::Ui, m: &MailboxRow, depth: usize, open: &mut Option<String>) {
        if depth > 16 {
            return;
        }
        let text = if m.unread > 0 { format!("{}  {}", m.name, m.unread) } else { m.name.clone() };
        let mut rt = egui::RichText::new(text);
        if m.unread > 0 {
            rt = rt.strong();
        }
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            let selected = self.search_view.is_none() && self.mailbox.as_deref() == Some(&m.id);
            if ui.selectable_label(selected, rt).clicked() {
                *open = Some(m.id.clone());
            }
        });
        for child in self.mailboxes.iter().filter(|c| c.parent_id.as_deref() == Some(&m.id)) {
            self.folder(ui, child, depth + 1, open);
        }
    }

    fn message_list(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if self.scroll_to_selected
            && let Some(i) = self.selected
        {
            self.scroll_to_selected = false;
            let (top, bottom) = (self.visible.start, self.visible.end.saturating_sub(1));
            if i < top || i >= bottom {
                let rows_visible = self.visible.len().max(1);
                let first = if i < top { i } else { (i + 2).saturating_sub(rows_visible) };
                area = area.vertical_scroll_offset(first as f32 * ROW_HEIGHT);
            }
        }
        let mut clicked = None;
        area.show_rows(ui, ROW_HEIGHT, self.count, |ui, range| {
            self.visible = range.clone();
            self.ensure_window(range.clone());
            for i in range {
                let Some(row) = self.row(i) else { continue };
                if self.list_row(ui, row, self.selected == Some(i)).clicked() {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.select(i);
            self.scroll_to_selected = false;
        }
        self.maybe_load_more();
    }

    fn list_row(&self, ui: &mut egui::Ui, row: &EmailRow, selected: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
        let v = ui.visuals();
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 0.0, v.selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, 0.0, v.faint_bg_color);
        }
        if !row.seen {
            painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(3.0, ROW_HEIGHT)), 0.0, v.hyperlink_color);
        }
        let strong = v.strong_text_color();
        let weak = v.weak_text_color();
        let font = FontId::proportional(13.0);
        let small = FontId::proportional(12.0);
        let left = rect.left() + 10.0;
        let right = rect.right() - 8.0;
        let date = painter.text(pos2(right, rect.top() + 4.0), Align2::RIGHT_TOP, format_date(row.received_at), small.clone(), weak);
        let mut icons = String::new();
        if row.has_attachment {
            icons.push('📎');
        }
        if row.flagged {
            icons.push('⚑');
        }
        let icons_w = if icons.is_empty() {
            0.0
        } else {
            painter.text(pos2(date.left() - 6.0, rect.top() + 4.0), Align2::RIGHT_TOP, icons, small.clone(), Color32::from_rgb(200, 60, 60)).width() + 6.0
        };
        let sender_clip = Rect::from_min_max(rect.min, pos2(date.left() - 8.0 - icons_w, rect.bottom()));
        let from = if row.from_name.is_empty() { &row.from_addr } else { &row.from_name };
        let sender_color = if row.seen { v.text_color() } else { strong };
        painter.with_clip_rect(sender_clip).text(pos2(left, rect.top() + 4.0), Align2::LEFT_TOP, from, font.clone(), sender_color);
        let line2 = if row.preview.is_empty() { row.subject.clone() } else { format!("{}  —  {}", row.subject, row.preview) };
        painter.with_clip_rect(rect.shrink2(Vec2::new(8.0, 0.0))).text(
            pos2(left, rect.top() + 22.0),
            Align2::LEFT_TOP,
            line2,
            small,
            if row.seen { weak } else { v.hyperlink_color },
        );
        painter.hline(rect.x_range(), rect.bottom() - 0.5, (1.0, v.widgets.noninteractive.bg_stroke.color));
        resp
    }

    fn reading_pane(&mut self, ui: &mut egui::Ui) {
        let Some(r) = &self.reading else {
            ui.centered_and_justified(|ui| ui.weak("Выберите письмо"));
            return;
        };
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&r.row.subject).size(18.0).strong());
        ui.add_space(4.0);
        let from = if r.row.from_name.is_empty() {
            r.row.from_addr.clone()
        } else {
            format!("{} <{}>", r.row.from_name, r.row.from_addr)
        };
        egui::Grid::new("headers").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
            ui.weak("От:");
            ui.label(from);
            ui.end_row();
            if let Some(b) = &r.body {
                ui.weak("Кому:");
                ui.label(addr_list(&b.to));
                ui.end_row();
                if !b.cc.is_empty() {
                    ui.weak("Копия:");
                    ui.label(addr_list(&b.cc));
                    ui.end_row();
                }
            }
            ui.weak("Дата:");
            ui.label(format_full_date(r.row.received_at));
            ui.end_row();
        });
        let mut action = None;
        ui.horizontal(|ui| {
            for (label, a) in [("↩ Ответить", "reply"), ("↩↩ Всем", "reply_all"), ("↪ Переслать", "forward")] {
                if ui.small_button(label).clicked() {
                    action = Some(a);
                }
            }
        });
        // Вложения полосой под шапкой; картинки, показанные в тексте, сюда не попадают.
        let mut file_act = None;
        let shown_cids: HashSet<String> = r.doc.as_ref().map(|d| ir_view::cids(d).into_iter().collect()).unwrap_or_default();
        if let Some(b) = &r.body {
            let listed: Vec<&AttachmentRow> =
                b.attachments.iter().filter(|a| a.cid.as_ref().is_none_or(|c| !shown_cids.contains(c))).collect();
            if !listed.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for a in listed {
                        let label = format!("📎 {} ({})", files::safe_name(&a.name), files::human_size(a.size));
                        ui.menu_button(label, |ui| {
                            let can_open = files::can_open(&a.name);
                            let open = ui.add_enabled(can_open, egui::Button::new("Открыть"));
                            let open = if can_open { open } else { open.on_disabled_hover_text("Этот тип файла опасен: его можно только сохранить") };
                            if open.clicked() {
                                file_act = Some((a.clone(), FileAct::Open));
                            }
                            if ui.button("Сохранить как…").clicked() {
                                file_act = Some((a.clone(), FileAct::SaveAs));
                            }
                        });
                    }
                });
            }
        }
        let mut show_images = false;
        let mut trust_sender = false;
        ui.separator();
        let mut link = None;
        match &r.doc {
            None => {
                ui.spinner();
            }
            Some(doc) => {
                if doc.remote_images_blocked > 0 {
                    egui::Frame::new().fill(ui.visuals().warn_fg_color.gamma_multiply(0.15)).inner_margin(4.0).show(ui, |ui| {
                        ui.label(format!(
                            "Внешние картинки не загружаются ({}). Это защищает от отслеживания.",
                            doc.remote_images_blocked
                        ));
                    });
                    ui.add_space(4.0);
                }
                if !r.show_images && !shown_cids.is_empty() {
                    egui::Frame::new().fill(ui.visuals().faint_bg_color).inner_margin(4.0).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("Картинки из письма скрыты ({}).", shown_cids.len()));
                            show_images = ui.small_button("Показать").clicked();
                            trust_sender = ui.small_button(format!("Всегда показывать от {}", r.row.from_addr)).clicked();
                        });
                    });
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    link = ir_view::show(ui, doc, &r.images);
                });
            }
        }
        if trust_sender {
            let _ = self.cache.trust(&r.row.from_addr);
        }
        if (show_images || trust_sender)
            && let Some(r) = self.reading.as_mut()
        {
            r.show_images = true;
        }
        if let Some((a, act)) = file_act {
            self.file_action(a, act);
        }
        if let Some(a) = action {
            self.run(a);
        }
        if let Some(url) = link {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

impl App {
    fn drive_bench(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.bench else { return };
        match b.frame(self.count) {
            bench::Step::Wait => {}
            bench::Step::Select(i) => self.select(i),
            bench::Step::Resize(w, h) => ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(w, h))),
            bench::Step::Done(report) => {
                println!("bench gpu=\"{}\" rows={} {report}", self.gpu, self.count);
                self.bench = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let frame_start = std::time::Instant::now();
        self.drive_bench(&ctx);
        self.poll_background(&ctx);
        self.ensure_images();
        self.commit_index();
        self.hotkeys(&ctx);
        self.show_composers(&ctx);

        egui::Panel::top("commands").show(ui, |ui| self.command_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let total = match &self.search_view {
                    Some(_) => None,
                    None => self.mailboxes.iter().find(|m| Some(&m.id) == self.mailbox.as_ref()).map(|m| m.total as usize),
                };
                match total {
                    Some(t) if t > self.count => ui.weak(format!("Элементов: {} из {t}", self.count)),
                    _ => ui.weak(format!("Элементов: {}", self.count)),
                };
                ui.separator();
                ui.weak(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(&self.gpu);
                    ui.separator();
                    ui.weak(self.sandbox);
                });
            });
        });
        egui::Panel::left("folders").default_size(200.0).show(ui, |ui| self.folders(ui));
        match self.section {
            Section::Mail => {
                egui::Panel::left("list")
                    .default_size(380.0)
                    .size_range(260.0..=700.0)
                    .show(ui, |ui| self.message_list(ui));
                egui::CentralPanel::default().show(ui, |ui| self.reading_pane(ui));
            }
            Section::Calendar => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.weak("Календарь и «Помощник планирования» — недели 7–12"));
                });
            }
            Section::Contacts => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.weak("Контакты и справочник LDAP — недели 7–12"));
                });
            }
        }
        if let Some(b) = &mut self.bench {
            b.ui_time(frame_start.elapsed());
        }
    }
}

/// Первичное построение индекса по уже накопленному кэшу — в фоне.
fn build_index(index: Arc<postman_search::SearchIndex>, cache: Cache, extractor: &crate::extract::Extractor) {
    let extractor = extractor.clone();
    std::thread::spawn(move || {
        let mut offset = 0;
        loop {
            let rows = cache.all_rows(offset, 5000).unwrap_or_default();
            if rows.is_empty() {
                break;
            }
            offset += rows.len();
            crate::indexer::rows(&index, &rows);
        }
        // Затем тексты уже загруженных тел — строго после строк списка, чтобы краткая
        // версия документа не затёрла полную. HTML разбирается только в воркере.
        let mut offset = 0;
        while let Ok(ids) = cache.body_ids(offset, 5000) {
            if ids.is_empty() {
                break;
            }
            offset += ids.len();
            extractor.queue(ids);
        }
    });
}

fn addr_list(list: &[postman_cache::Addr]) -> String {
    list.iter()
        .map(|a| if a.name.is_empty() { a.email.clone() } else { format!("{} <{}>", a.name, a.email) })
        .collect::<Vec<_>>()
        .join(", ")
}

fn configure_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|s| {
        // Плотная вёрстка, как в Outlook.
        s.spacing.item_spacing = Vec2::new(6.0, 3.0);
        s.spacing.button_padding = Vec2::new(6.0, 2.0);
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
    });
}

fn local(ts: i64) -> Option<jiff::Zoned> {
    Some(jiff::Timestamp::from_second(ts).ok()?.to_zoned(jiff::tz::TimeZone::system()))
}

fn format_date(ts: i64) -> String {
    let (Some(t), Some(now)) = (local(ts), local(jiff::Timestamp::now().as_second())) else {
        return String::new();
    };
    if t.date() == now.date() {
        t.strftime("%H:%M").to_string()
    } else if t.year() == now.year() {
        t.strftime("%d.%m %H:%M").to_string()
    } else {
        t.strftime("%d.%m.%Y").to_string()
    }
}

fn format_full_date(ts: i64) -> String {
    local(ts).map(|t| t.strftime("%d.%m.%Y %H:%M").to_string()).unwrap_or_default()
}

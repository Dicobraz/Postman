//! Окно нового письма / ответа / пересылки — отдельное окно ОС, как в Outlook.
//! Ввод — Markdown; превью строится тем же путём, что и входящие (через воркер).

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, ViewportBuilder, ViewportId};
use postman_cache::BodyRow;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use postman_compose::{Address, Attachment, Draft, parse_addresses};
use postman_ir::Document;

use crate::{files, ir_view};
use crate::render::Renderer;

/// Предупреждение о размере: типичный предел серверов — 25–35 МБ.
const LARGE_TOTAL: usize = 25 * 1024 * 1024;

pub enum Action {
    Send(Box<Draft>, Option<String>),
    Save(Box<Draft>, Option<String>),
}

pub struct Composer {
    pub ticket: u64,
    pub open: bool,
    viewport: ViewportId,
    base: Draft,
    to: String,
    cc: String,
    bcc: String,
    show_bcc: bool,
    subject: String,
    body: String,
    /// id сохранённой на сервере версии черновика.
    server_id: Option<String>,
    status: String,
    busy: bool,
    first_frame: bool,
    /// Свой воркер для превью; поднимается при первом просмотре.
    renderer: Option<Renderer>,
    preview_on: bool,
    preview_doc: Option<Document>,
    /// Слепок содержимого, совпадающий с сохранённой версией (или исходной, если не сохраняли).
    saved_snapshot: u64,
    /// Слепок, отправленный на сохранение и ещё не подтверждённый сервером.
    pending_snapshot: Option<u64>,
    /// Слепок, автосохранение которого не удалось — не повторяем, пока текст не изменится.
    autosave_failed: Option<u64>,
    last_autosave: Instant,
    /// Открыт вопрос «Сохранить изменения?».
    confirm_close: bool,
    /// Закрыть окно, как только сервер подтвердит сохранение.
    close_after_save: bool,
    book: Option<Book>,
    /// Выбранная подсказка адреса.
    suggest_sel: usize,
}

/// Общая адресная книга: строится в фоне, пополняется контактами с сервера.
pub type Book = std::sync::Arc<std::sync::Mutex<postman_core::contacts::AddressBook>>;

/// Поле адресов с подсказками: ↑/↓ — выбор, Enter или Tab — вставить вместо набираемого.
fn address_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    focus: bool,
    book: Option<&postman_core::contacts::AddressBook>,
    sel: &mut usize,
) {
    ui.label(label);
    // От родителя: у каждого окна письма свои поля.
    let id = ui.make_persistent_id(("address-field", label));
    let focused = ui.memory(|m| m.has_focus(id));
    let start = value.rfind([',', ';']).map_or(0, |i| i + 1);
    let token = value[start..].trim().to_string();
    let now = jiff::Timestamp::now().as_second();
    let suggestions: Vec<String> = match book {
        Some(b) if focused && token.chars().count() >= 2 && !token.contains('<') => {
            b.suggest(&token, 6, now).into_iter().map(|e| e.display()).collect()
        }
        _ => Vec::new(),
    };
    *sel = (*sel).min(suggestions.len().saturating_sub(1));
    let mut accept = None;
    if !suggestions.is_empty() {
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                *sel = (*sel + 1).min(suggestions.len() - 1);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                *sel = sel.saturating_sub(1);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::NONE, Key::Tab) {
                accept = Some(*sel);
            }
        });
    }
    let r = ui.add(egui::TextEdit::singleline(value).id(id).desired_width(f32::INFINITY));
    if focus {
        r.request_focus();
    }
    if r.changed() {
        *sel = 0;
    }
    ui.end_row();
    if !suggestions.is_empty() && accept.is_none() {
        ui.label("");
        ui.vertical(|ui| {
            for (n, s) in suggestions.iter().enumerate() {
                if ui.selectable_label(n == *sel, s).clicked() {
                    accept = Some(n);
                }
            }
        });
        ui.end_row();
    }
    if let Some(n) = accept {
        let prefix = value[..start].trim_end().to_string();
        let sep = if prefix.is_empty() { "" } else { " " };
        *value = format!("{prefix}{sep}{}; ", suggestions[n]);
        *sel = 0;
        // Курсор в конец и фокус обратно в поле — можно набирать следующий адрес.
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            let end = egui::text::CCursor::new(value.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), id);
        }
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// Автосохранение изменённого черновика не чаще этого.
const AUTOSAVE: Duration = Duration::from_secs(60);

fn join(list: &[Address]) -> String {
    list.iter()
        .map(|a| if a.name.contains([',', ';', '"']) { format!("\"{}\" <{}>", a.name.replace('"', ""), a.email) } else { a.display() })
        .collect::<Vec<_>>()
        .join("; ")
}

impl Composer {
    pub fn new(ticket: u64, draft: Draft, book: Option<Book>) -> Self {
        let mut c = Self {
            ticket,
            open: true,
            viewport: ViewportId::from_hash_of(("composer", ticket)),
            to: join(&draft.to),
            cc: join(&draft.cc),
            bcc: join(&draft.bcc),
            show_bcc: !draft.bcc.is_empty(),
            subject: draft.subject.clone(),
            body: draft.markdown.clone(),
            base: draft,
            server_id: None,
            status: String::new(),
            busy: false,
            first_frame: true,
            renderer: None,
            preview_on: false,
            preview_doc: None,
            saved_snapshot: 0,
            pending_snapshot: None,
            autosave_failed: None,
            last_autosave: Instant::now(),
            confirm_close: false,
            close_after_save: false,
            book,
            suggest_sel: 0,
        };
        // Нетронутое новое письмо или ответ — не «изменения», закрывается без вопроса.
        c.saved_snapshot = c.snapshot();
        c
    }

    fn snapshot(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.to, &self.cc, &self.bcc, &self.subject, &self.body).hash(&mut h);
        for a in &self.base.attachments {
            (&a.name, a.data.len(), &a.blob_id).hash(&mut h);
        }
        h.finish()
    }

    fn dirty(&self) -> bool {
        self.snapshot() != self.saved_snapshot
    }

    fn attach(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "файл".into());
            match std::fs::read(&p) {
                Ok(data) => self.base.attachments.push(Attachment {
                    mime: files::mime_for(&name).into(),
                    name,
                    data,
                    inline_cid: None,
                    blob_id: None,
                }),
                Err(e) => self.status = format!("«{name}»: {e}"),
            }
        }
        let total: usize = self.base.attachments.iter().map(|a| a.data.len()).sum();
        if total > LARGE_TOTAL {
            self.status = format!("Вложения: {} — сервер может не принять письмо", files::human_size(total as u64));
        }
    }

    pub fn sent(&mut self) {
        self.open = false;
    }

    pub fn saved(&mut self, id: String) {
        self.busy = false;
        self.server_id = Some(id);
        if let Some(s) = self.pending_snapshot.take() {
            self.saved_snapshot = s;
        }
        let time = jiff::Zoned::now().strftime("%H:%M").to_string();
        self.status = format!("Черновик сохранён в {time}");
        if self.close_after_save {
            self.open = false;
        }
    }

    pub fn failed(&mut self, error: String) {
        self.busy = false;
        self.autosave_failed = self.pending_snapshot.take();
        self.close_after_save = false;
        self.status = format!("Ошибка: {error}");
    }

    /// Закрытие: без изменений — сразу, иначе вопрос.
    fn request_close(&mut self) {
        if self.dirty() {
            self.confirm_close = true;
        } else {
            self.open = false;
        }
    }

    fn close_dialog(&mut self, ui: &mut egui::Ui) -> (bool, bool) {
        let (mut save, mut discard) = (false, false);
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::unique(("composer-close", self.ticket))).show(ui.ctx(), |ui| {
            ui.label(egui::RichText::new("Сохранить изменения в черновике?").strong());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = ui.button("Сохранить").clicked();
                discard = ui.button("Не сохранять").clicked();
                cancel = ui.button("Отмена").clicked();
            });
        });
        if cancel || modal.should_close() {
            self.confirm_close = false;
        }
        (save, discard)
    }

    /// Собрать черновик из полей; ошибки адресов — до отправки, а не от сервера.
    fn draft(&self, for_send: bool) -> Result<Draft, String> {
        let mut d = self.base.clone();
        let mut bad = Vec::new();
        for (field, out) in [(&self.to, &mut d.to), (&self.cc, &mut d.cc), (&self.bcc, &mut d.bcc)] {
            let (ok, b) = parse_addresses(field);
            *out = ok;
            bad.extend(b);
        }
        if !bad.is_empty() {
            return Err(format!("не распознаны адреса: {}", bad.join(", ")));
        }
        if for_send && d.recipients().next().is_none() {
            return Err("не указан ни один получатель".into());
        }
        d.subject = self.subject.clone();
        d.markdown = self.body.clone();
        Ok(d)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let title = if self.subject.trim().is_empty() { "Новое сообщение".to_string() } else { self.subject.clone() };
        let builder = ViewportBuilder::default().with_title(title).with_inner_size([780.0, 620.0]).with_min_inner_size([500.0, 360.0]);
        ctx.show_viewport_immediate(self.viewport, builder, |ui, _| self.ui(ui))
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let mut want_send = false;
        let mut want_save = false;
        let mut toggle_preview = false;
        let mut close = false;
        let mut autosave = false;
        let mut attach: Vec<PathBuf> = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        ui.input_mut(|i| {
            if i.viewport().close_requested() {
                close = true;
            }
            if !self.confirm_close && i.consume_key(Modifiers::NONE, Key::Escape) {
                close = true;
            }
            want_send = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter))
                || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, Key::S));
            want_save = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S));
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::B)) {
                self.show_bcc = !self.show_bcc;
            }
            toggle_preview = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::P));
        });

        egui::Panel::top("compose_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.busy, |ui| {
                    want_send |= ui.button("➤ Отправить").on_hover_text("Ctrl+Enter / Alt+S").clicked();
                    want_save |= ui.button("💾 Сохранить").on_hover_text("Ctrl+S").clicked();
                    if ui.button("📎 Вложить").on_hover_text("Или перетащите файлы в окно").clicked() {
                        attach = crate::dialogs::pick();
                    }
                });
                if ui.selectable_label(self.show_bcc, "Скрытая").on_hover_text("Ctrl+Shift+B").clicked() {
                    self.show_bcc = !self.show_bcc;
                }
                toggle_preview |= ui.selectable_label(self.preview_on, "Просмотр").on_hover_text("Ctrl+Shift+P").clicked();
                ui.weak(&self.status);
            });
        });
        if close {
            if ui.input(|i| i.viewport().close_requested()) && self.dirty() {
                // Закрытие окна ОС отменяем до ответа пользователя.
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            self.request_close();
        }
        if self.confirm_close {
            let (save, discard) = self.close_dialog(ui);
            if discard {
                self.open = false;
            }
            if save {
                self.confirm_close = false;
                self.close_after_save = true;
                want_save = true;
            }
        }
        // Автосохранение: изменённый черновик раз в минуту, если прошлая попытка с тем же текстом не провалилась.
        let snap = self.snapshot();
        if !self.busy
            && self.dirty()
            && self.autosave_failed != Some(snap)
            && self.last_autosave.elapsed() >= AUTOSAVE
            && !want_send
            && !want_save
        {
            autosave = true;
            want_save = true;
        }
        ui.ctx().request_repaint_after(AUTOSAVE);
        if !attach.is_empty() {
            self.attach(attach);
        }
        if toggle_preview {
            self.toggle_preview(ui.ctx());
        }
        egui::Panel::top("compose_fields").show(ui, |ui| {
            egui::Grid::new("fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                let book = self.book.clone();
                let book = book.as_ref().and_then(|b| b.lock().ok());
                let focus_to = self.first_frame && self.to.is_empty();
                address_field(ui, "Кому", &mut self.to, focus_to, book.as_deref(), &mut self.suggest_sel);
                address_field(ui, "Копия", &mut self.cc, false, book.as_deref(), &mut self.suggest_sel);
                if self.show_bcc {
                    address_field(ui, "Скрытая", &mut self.bcc, false, book.as_deref(), &mut self.suggest_sel);
                }
                ui.label("Тема");
                ui.add(egui::TextEdit::singleline(&mut self.subject).desired_width(f32::INFINITY));
                ui.end_row();
            });
            let mut remove = None;
            // Картинки цитаты (cid) не показываем: они уходят вместе с ответом автоматически.
            let listed: Vec<(usize, &Attachment)> =
                self.base.attachments.iter().enumerate().filter(|(_, a)| a.inline_cid.is_none()).collect();
            if !listed.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Вложения:");
                    for (i, a) in listed {
                        let size = if a.data.is_empty() { String::new() } else { format!(" ({})", files::human_size(a.data.len() as u64)) };
                        if ui.button(format!("📎 {}{size}  ✕", files::safe_name(&a.name))).on_hover_text("Убрать").clicked() {
                            remove = Some(i);
                        }
                    }
                });
            }
            if let Some(i) = remove {
                self.base.attachments.remove(i);
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(r) = &self.renderer {
                while let Ok(done) = r.rx.try_recv() {
                    if let crate::render::Rendered::Body { doc, .. } = done {
                        self.preview_doc = Some(doc);
                    }
                }
            }
            if self.preview_on {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match &self.preview_doc {
                    Some(doc) => {
                        ir_view::show(ui, doc, &ir_view::Images::new());
                    }
                    None => {
                        ui.spinner();
                    }
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let out = egui::TextEdit::multiline(&mut self.body)
                    .desired_width(f32::INFINITY)
                    .desired_rows(24)
                    .lock_focus(true)
                    .hint_text("Текст письма (Markdown: **жирный**, *курсив*, - список, > цитата)")
                    .show(ui);
                if self.first_frame && !self.to.is_empty() {
                    // Ответ: курсор в начало, над цитатой — как в Outlook.
                    let mut state = out.state.clone();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(0))));
                    state.store(ui.ctx(), out.response.id);
                    out.response.request_focus();
                }
            });
        });
        self.first_frame = false;

        if want_send || want_save {
            if want_save {
                self.last_autosave = Instant::now();
            }
            match self.draft(want_send) {
                Ok(d) => {
                    self.busy = true;
                    self.pending_snapshot = Some(self.snapshot());
                    self.status = match (want_send, autosave) {
                        (true, _) => "Отправка…".into(),
                        (false, true) => "Автосохранение…".into(),
                        (false, false) => "Сохранение…".into(),
                    };
                    let replace = self.server_id.clone();
                    action = Some(if want_send { Action::Send(Box::new(d), replace) } else { Action::Save(Box::new(d), replace) });
                }
                Err(e) => {
                    if autosave {
                        self.autosave_failed = Some(self.snapshot());
                    }
                    self.close_after_save = false;
                    self.status = e;
                }
            }
        }
        action
    }

    fn toggle_preview(&mut self, ctx: &egui::Context) {
        self.preview_on = !self.preview_on;
        if !self.preview_on {
            return;
        }
        self.preview_doc = None;
        let r = self.renderer.get_or_insert_with(|| {
            let ctx = ctx.clone();
            Renderer::spawn(move || ctx.request_repaint())
        });
        let html = postman_ir::markdown_to_html(&self.body);
        r.request(format!("compose{}", self.ticket), BodyRow { html: Some(html), ..Default::default() });
    }
}

//! Окно нового письма / ответа / пересылки — отдельное окно ОС, как в Outlook.
//! Ввод — Markdown; превью строится тем же путём, что и входящие (через воркер).

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, ViewportBuilder, ViewportId};
use postman_cache::BodyRow;
use postman_compose::{Address, Draft, parse_addresses};
use postman_ir::Document;

use crate::ir_view;
use crate::render::Renderer;

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
}

fn join(list: &[Address]) -> String {
    list.iter()
        .map(|a| if a.name.contains([',', ';', '"']) { format!("\"{}\" <{}>", a.name.replace('"', ""), a.email) } else { a.display() })
        .collect::<Vec<_>>()
        .join("; ")
}

impl Composer {
    pub fn new(ticket: u64, draft: Draft) -> Self {
        Self {
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
        }
    }

    pub fn sent(&mut self) {
        self.open = false;
    }

    pub fn saved(&mut self, id: String) {
        self.busy = false;
        self.server_id = Some(id);
        self.status = "Черновик сохранён".into();
    }

    pub fn failed(&mut self, error: String) {
        self.busy = false;
        self.status = format!("Ошибка: {error}");
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
        ui.input_mut(|i| {
            if i.viewport().close_requested() || i.consume_key(Modifiers::NONE, Key::Escape) {
                self.open = false;
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
                });
                if ui.selectable_label(self.show_bcc, "Скрытая").on_hover_text("Ctrl+Shift+B").clicked() {
                    self.show_bcc = !self.show_bcc;
                }
                toggle_preview |= ui.selectable_label(self.preview_on, "Просмотр").on_hover_text("Ctrl+Shift+P").clicked();
                ui.weak(&self.status);
            });
        });
        if toggle_preview {
            self.toggle_preview(ui.ctx());
        }
        egui::Panel::top("compose_fields").show(ui, |ui| {
            egui::Grid::new("fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                let field = |ui: &mut egui::Ui, label: &str, value: &mut String, focus: bool| {
                    ui.label(label);
                    let r = ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                    if focus {
                        r.request_focus();
                    }
                    ui.end_row();
                };
                let focus_to = self.first_frame && self.to.is_empty();
                field(ui, "Кому", &mut self.to, focus_to);
                field(ui, "Копия", &mut self.cc, false);
                if self.show_bcc {
                    field(ui, "Скрытая", &mut self.bcc, false);
                }
                field(ui, "Тема", &mut self.subject, false);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(r) = &self.renderer {
                while let Ok(done) = r.rx.try_recv() {
                    self.preview_doc = Some(done.doc);
                }
            }
            if self.preview_on {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match &self.preview_doc {
                    Some(doc) => {
                        ir_view::show(ui, doc);
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
            match self.draft(want_send) {
                Ok(d) => {
                    self.busy = true;
                    self.status = if want_send { "Отправка…".into() } else { "Сохранение…".into() };
                    let replace = self.server_id.clone();
                    action = Some(if want_send { Action::Send(Box::new(d), replace) } else { Action::Save(Box::new(d), replace) });
                }
                Err(e) => self.status = e,
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

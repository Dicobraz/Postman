//! Окно письма (S3-01): отдельное окно ОС, как в classic Outlook (ADR 0002); основное окно работает.
//!
//! Модель окна ([`Model`]) — без gpui: поля, вложения, грязное/чистое, автосохранение, закрытие; её проверяют
//! тесты. View ([`Composer`]) — поля ввода, подсказки адресов, предпросмотр через воркер-песочницу, вложения
//! (диалог ОС и перетаскивание, файлы читаются в фоне). Сервер — только через шов `Backend::send` с тикетом;
//! итог (`Sent`, `DraftSaved`, `ComposeFailed`) приходит событием синхронизации через `Mailer::on_sync`.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{
    Enter, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_compose::{Address, Attachment, Draft, parse_addresses};
use postman_core::contacts::AddressBook;
use postman_core::sync::{Command, Event};
use postman_ir::Document;
use postman_worker::{Client as Worker, Request};

use crate::commands::compose;
use crate::{Mailer, ir_view, sys};

/// Автосохранение изменённого черновика — не чаще этого.
pub(crate) const AUTOSAVE: Duration = Duration::from_secs(60);
/// Предупреждение о размере вложений: типичный предел серверов — 25–35 МБ.
const LARGE_TOTAL: u64 = 25 * 1024 * 1024;
/// Подсказок адреса под полем.
const SUGGESTIONS: usize = 6;
/// Подсказки — с этого числа набранных букв.
const SUGGEST_FROM: usize = 2;

const LABEL_W: f32 = 64.0;
const ERROR: u32 = 0xc42b1c;
const ACCENT: u32 = 0x0f6cbd;
const MUTED: u32 = 0x666666;
const BORDER: u32 = 0xdddddd;

// --- модель окна (без gpui) ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Field {
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

impl Field {
    fn is_address(self) -> bool {
        matches!(self, Field::To | Field::Cc | Field::Bcc)
    }
}

/// Зачем черновик уходит на сервер.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Purpose {
    Send,
    Save,
    Autosave,
}

/// Строка состояния окна; текст — в view (Fluent).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    None,
    Busy(Purpose),
    Saved {
        at: String,
    },
    BadAddresses(Vec<String>),
    NoRecipients,
    /// Сервера нет (демо) или поток синхронизации остановился: текст остаётся в окне.
    NoServer(Purpose),
    Failed(String),
    Attach {
        name: String,
        error: String,
    },
    Large(u64),
}

/// Ответ на строку «Сохранить черновик? Да / Нет / Отмена».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseAnswer {
    Save,
    Discard,
    Cancel,
}

/// Что делает окно после ответа.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseStep {
    Close,
    Save,
    Stay,
}

#[derive(Debug)]
struct Pending {
    ticket: u64,
    snapshot: u64,
    purpose: Purpose,
}

pub(crate) struct Model {
    /// Отправитель, ответ (In-Reply-To, References), вложения.
    base: Draft,
    pub(crate) to: String,
    pub(crate) cc: String,
    pub(crate) bcc: String,
    pub(crate) subject: String,
    pub(crate) body: String,
    /// id сохранённой на сервере версии черновика (её заменит следующее сохранение или отправка).
    server_id: Option<String>,
    /// Слепок содержимого, совпадающий с сохранённой (или исходной) версией.
    saved: u64,
    pending: Option<Pending>,
    /// Слепок, сохранить который не удалось, — не повторяем автосохранение, пока текст тот же.
    autosave_failed: Option<u64>,
    last_save: Instant,
    pub(crate) status: Status,
    /// Показана строка «Сохранить черновик?».
    pub(crate) confirm_close: bool,
    /// Закрыть окно, когда сервер подтвердит сохранение.
    close_after_save: bool,
}

/// Адреса для поля ввода; имена с запятыми — в кавычках, чтобы разбор вернул то же.
fn join(list: &[Address]) -> String {
    list.iter()
        .map(|a| {
            if a.name.contains([',', ';', '"']) {
                format!("\"{}\" <{}>", a.name.replace('"', ""), a.email)
            } else {
                a.display()
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Подпись нового письма: под пустыми строками для текста, как в Outlook.
fn signature_block(signature: &str) -> String {
    if signature.trim().is_empty() { String::new() } else { format!("\n\n{}", signature.trim_end()) }
}

impl Model {
    pub(crate) fn new(draft: Draft, from: Option<Address>, signature: &str, now: Instant) -> Self {
        let mut base = draft;
        if let Some(from) = from.filter(|_| base.from.email.is_empty()) {
            base.from = from;
        }
        let mut m = Self {
            to: join(&base.to),
            cc: join(&base.cc),
            bcc: join(&base.bcc),
            subject: base.subject.clone(),
            body: if base.markdown.is_empty() { signature_block(signature) } else { base.markdown.clone() },
            base,
            server_id: None,
            saved: 0,
            pending: None,
            autosave_failed: None,
            last_save: now,
            status: Status::None,
            confirm_close: false,
            close_after_save: false,
        };
        // Нетронутое новое письмо — не «изменения»: закрывается без вопроса.
        m.saved = m.snapshot();
        m
    }

    /// Учётная запись с сервера (Identity): отправитель; подпись — в нетронутое пустое письмо.
    /// `true` — текст изменился, его надо показать в поле.
    pub(crate) fn set_identity(&mut self, from: Address, signature: &str) -> bool {
        self.base.from = from;
        let block = signature_block(signature);
        if self.is_dirty() || !self.body.is_empty() || block.is_empty() {
            return false;
        }
        self.body = block;
        self.saved = self.snapshot();
        true
    }

    pub(crate) fn field(&self, f: Field) -> &str {
        match f {
            Field::To => &self.to,
            Field::Cc => &self.cc,
            Field::Bcc => &self.bcc,
            Field::Subject => &self.subject,
            Field::Body => &self.body,
        }
    }

    pub(crate) fn set(&mut self, f: Field, value: String) {
        match f {
            Field::To => self.to = value,
            Field::Cc => self.cc = value,
            Field::Bcc => self.bcc = value,
            Field::Subject => self.subject = value,
            Field::Body => self.body = value,
        }
    }

    fn snapshot(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.to, &self.cc, &self.bcc, &self.subject, &self.body).hash(&mut h);
        for a in &self.base.attachments {
            (&a.name, a.data.len(), &a.blob_id).hash(&mut h);
        }
        h.finish()
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.snapshot() != self.saved
    }

    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Нераспознанные адреса поля — подсвечиваются.
    pub(crate) fn bad(&self, f: Field) -> Vec<String> {
        if f.is_address() { parse_addresses(self.field(f)).1 } else { Vec::new() }
    }

    /// Черновик из полей; ошибки адресов — до отправки, а не от сервера.
    pub(crate) fn draft(&self, for_send: bool) -> Result<Draft, Status> {
        let mut d = self.base.clone();
        let mut bad = Vec::new();
        for (text, out) in [(&self.to, &mut d.to), (&self.cc, &mut d.cc), (&self.bcc, &mut d.bcc)] {
            let (ok, b) = parse_addresses(text);
            *out = ok;
            bad.extend(b);
        }
        if !bad.is_empty() {
            return Err(Status::BadAddresses(bad));
        }
        if for_send && d.recipients().next().is_none() {
            return Err(Status::NoRecipients);
        }
        d.subject = self.subject.clone();
        d.markdown = self.body.clone();
        Ok(d)
    }

    /// Начать отправку или сохранение: команда для `Backend::send` или `None` (ошибка — в `status`).
    pub(crate) fn begin(&mut self, purpose: Purpose, ticket: u64, now: Instant) -> Option<Command> {
        if purpose != Purpose::Send {
            self.last_save = now;
        }
        match self.draft(purpose == Purpose::Send) {
            Ok(d) => {
                self.pending = Some(Pending { ticket, snapshot: self.snapshot(), purpose });
                self.status = Status::Busy(purpose);
                let (draft, replace) = (Box::new(d), self.server_id.clone());
                Some(match purpose {
                    Purpose::Send => Command::Send { draft, replace, ticket },
                    Purpose::Save | Purpose::Autosave => Command::SaveDraft { draft, replace, ticket },
                })
            }
            Err(status) => {
                if purpose == Purpose::Autosave {
                    self.autosave_failed = Some(self.snapshot());
                }
                self.close_after_save = false;
                self.status = status;
                None
            }
        }
    }

    /// Шов не принял команду (демо, синхронизация остановилась): черновик остаётся в окне.
    pub(crate) fn refused(&mut self) {
        if let Some(p) = self.pending.take() {
            // Без сервера тот же текст сохранять снова бессмысленно — ждём изменений.
            self.autosave_failed = Some(p.snapshot);
            self.status = Status::NoServer(p.purpose);
        }
        self.close_after_save = false;
    }

    /// Черновик сохранён на сервере. `true` — закрыть окно (просили «Да» при закрытии).
    pub(crate) fn on_saved(&mut self, ticket: u64, id: String, at: String) -> bool {
        let Some(p) = self.pending.take_if(|p| p.ticket == ticket) else { return false };
        self.server_id = Some(id);
        self.saved = p.snapshot;
        self.status = Status::Saved { at };
        std::mem::take(&mut self.close_after_save)
    }

    /// Письмо ушло. `true` — это наша отправка, окно закрывается.
    pub(crate) fn on_sent(&mut self, ticket: u64) -> bool {
        let Some(p) = self.pending.take_if(|p| p.ticket == ticket) else { return false };
        self.saved = p.snapshot;
        self.status = Status::None;
        true
    }

    /// Отправка или сохранение не удались: строка в окне, окно не закрывается.
    pub(crate) fn on_failed(&mut self, ticket: u64, error: String) -> bool {
        let Some(p) = self.pending.take_if(|p| p.ticket == ticket) else { return false };
        if p.purpose == Purpose::Autosave {
            self.autosave_failed = Some(p.snapshot);
        }
        self.close_after_save = false;
        self.status = Status::Failed(error);
        true
    }

    /// Через сколько автосохранение; `None` — не нужно (нет изменений, идёт отправка, этот текст уже не сохранился).
    pub(crate) fn autosave_wait(&self, now: Instant) -> Option<Duration> {
        let snapshot = self.snapshot();
        if snapshot == self.saved
            || self.pending.is_some()
            || self.confirm_close
            || self.autosave_failed == Some(snapshot)
        {
            return None;
        }
        Some(AUTOSAVE.saturating_sub(now.saturating_duration_since(self.last_save)))
    }

    pub(crate) fn should_autosave(&self, now: Instant) -> bool {
        self.autosave_wait(now).is_some_and(|w| w.is_zero())
    }

    /// Закрытие: без изменений — сразу (`true`), иначе строка «Сохранить черновик?».
    pub(crate) fn request_close(&mut self) -> bool {
        if !self.is_dirty() {
            return true;
        }
        self.confirm_close = true;
        false
    }

    pub(crate) fn answer_close(&mut self, answer: CloseAnswer) -> CloseStep {
        self.confirm_close = false;
        match answer {
            CloseAnswer::Save => {
                self.close_after_save = true;
                CloseStep::Save
            }
            CloseAnswer::Discard => CloseStep::Close,
            CloseAnswer::Cancel => CloseStep::Stay,
        }
    }

    /// Вложения в списке окна (картинки внутри текста — `cid:` — не показываем).
    pub(crate) fn attachments(&self) -> impl Iterator<Item = (usize, &Attachment)> {
        self.base.attachments.iter().enumerate().filter(|(_, a)| a.inline_cid.is_none())
    }

    pub(crate) fn add_attachment(&mut self, a: Attachment) {
        self.base.attachments.push(a);
        let total: u64 = self.base.attachments.iter().map(|a| a.data.len() as u64).sum();
        if total > LARGE_TOTAL {
            self.status = Status::Large(total);
        }
    }

    pub(crate) fn remove_attachment(&mut self, i: usize) {
        if i < self.base.attachments.len() {
            self.base.attachments.remove(i);
        }
    }
}

/// Набираемый адрес — после последнего `,` или `;`.
pub(crate) fn current_token(text: &str) -> &str {
    let start = text.rfind([',', ';']).map_or(0, |i| i + 1);
    text[start..].trim()
}

/// Вставить подсказку вместо набираемого адреса; дальше — разделитель для следующего.
pub(crate) fn accept_suggestion(text: &str, suggestion: &str) -> String {
    let start = text.rfind([',', ';']).map_or(0, |i| i + 1);
    let prefix = text[..start].trim_end();
    let sep = if prefix.is_empty() { "" } else { " " };
    format!("{prefix}{sep}{suggestion}; ")
}

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// Тикет команды окна письма: итог синхронизации находит своё окно по нему.
fn next_ticket() -> u64 {
    NEXT_TICKET.fetch_add(1, Ordering::Relaxed)
}

fn size_text(n: u64) -> String {
    match n {
        n if n < 1024 => tr!("compose-size-b", n = n.to_string()),
        n if n < 1024 * 1024 => tr!("compose-size-kb", n = format!("{:.0}", n as f64 / 1024.0)),
        n => tr!("compose-size-mb", n = format!("{:.1}", n as f64 / 1024.0 / 1024.0)),
    }
}

// --- окна письма у экрана ---

/// Адресная книга для подсказок: строится в фоне при первом окне письма, пополняется контактами с сервера.
pub(crate) type Book = Rc<RefCell<AddressBook>>;

/// Открытые окна письма и то, что им нужно от экрана: отправитель и подпись (Identity), адресная книга.
#[derive(Default)]
pub(crate) struct Composers {
    windows: Vec<WeakEntity<Composer>>,
    me: Option<(Address, String)>,
    /// Общая с окном встречи (S3-05).
    pub(crate) book: Book,
    book_started: bool,
}

impl Composers {
    /// Событие синхронизации: Identity и контакты — себе, итоги отправки — окнам (каждое ищет свой тикет).
    pub(crate) fn on_sync(&mut self, ev: &Event, cx: &mut App) {
        match ev {
            Event::Me { address, signature } => {
                self.me = Some((address.clone(), signature.clone()));
                self.book.borrow_mut().remove(&address.email);
            }
            Event::Contacts(list) => {
                self.book.borrow_mut().add_contacts(list.iter().map(|(n, e)| (n.as_str(), e.as_str())));
            }
            _ => {}
        }
        self.windows.retain(|w| w.update(cx, |c, cx| c.on_sync(ev, cx)).is_ok());
    }

    /// Мой адрес и подпись из Identity (живой ящик), если сервер их уже прислал.
    pub(crate) fn me(&self) -> Option<&(Address, String)> {
        self.me.as_ref()
    }

    /// Последнее открытое окно (снимки `--screens`).
    pub(crate) fn last(&self) -> Option<Entity<Composer>> {
        self.windows.iter().rev().find_map(WeakEntity::upgrade)
    }
}

impl Mailer {
    /// Окно письма с черновиком (`mail.new` — пустой); основное окно продолжает работать.
    pub(crate) fn open_compose(&mut self, draft: Draft, cx: &mut Context<Self>) -> Option<Entity<Composer>> {
        self.load_address_book(cx);
        let open = Open {
            me: self.compose.me.clone(),
            book: self.compose.book.clone(),
            worker: self.reading.read(cx).worker.clone(),
            mailer: cx.entity().downgrade(),
            screens: self.screens.is_some(),
        };
        let bounds = Bounds::centered(None, size(px(820.), px(680.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(tr!("compose-title-new").into()), ..Default::default() }),
            window_min_size: Some(size(px(520.), px(380.))),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Composer::new(draft, open, window, cx))) {
            Ok((_, composer)) => {
                self.compose.windows.retain(|w| w.upgrade().is_some());
                self.compose.windows.push(composer.downgrade());
                Some(composer)
            }
            Err(e) => {
                tracing::error!("compose window: {e}");
                let text = tr!("compose-window-failed", error = e.to_string());
                self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx));
                None
            }
        }
    }

    /// Книга адресов из хранилища — в фоне, один раз; окна видят её, как только она готова.
    pub(crate) fn load_address_book(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.compose.book_started, true) {
            return;
        }
        let cache = self.cache.clone();
        let me = self.compose.me.as_ref().map(|(a, _)| a.email.clone()).unwrap_or_default();
        let task = cx.background_spawn(async move { AddressBook::from_cache(&cache, &me) });
        let book = self.compose.book.clone();
        cx.spawn(async move |_, _| {
            let mut built = task.await;
            let mut b = book.borrow_mut();
            // Контакты с сервера, пришедшие раньше книги, — не теряем.
            built.absorb(std::mem::take(&mut *b));
            *b = built;
        })
        .detach();
    }

    /// Сцена `compose` для `--screens`: окно с заполненными полями и вложением.
    pub(crate) fn open_compose_screen(&mut self, cx: &mut Context<Self>) {
        let draft = Draft {
            to: vec![Address::new("Анна Смирнова", "anna.smirnova@corp.example")], // i18n-ok: образец письма
            cc: vec![Address::new("Иван Петров", "ivan.petrov@corp.example")], // i18n-ok: образец письма
            subject: "Отчёт за третий квартал".into(), // i18n-ok: образец письма
            markdown: "Анна, добрый день!\n\nОтчёт за **третий квартал** во вложении. Коротко:\n\n- выручка +12 %\n- расходы без изменений\n\nС уважением,\nПётр".into(), // i18n-ok: образец письма
            ..Default::default()
        };
        let Some(composer) = self.open_compose(draft, cx) else { return };
        composer.update(cx, |c, cx| {
            // Ошибочный адрес — чтобы снимок показал подсветку.
            c.model.to.push_str("; petr@corp");
            c.sync_inputs = true;
            c.model.add_attachment(Attachment {
                name: "otchet-q3.xlsx".into(),
                mime: postman_core::files::mime_for("otchet-q3.xlsx").into(),
                data: vec![0; 48 * 1024],
                inline_cid: None,
                blob_id: None,
            });
            cx.notify();
        });
    }
}

// --- окно письма ---

/// Что окну нужно от экрана при открытии.
pub(crate) struct Open {
    me: Option<(Address, String)>,
    book: Book,
    worker: Arc<Mutex<Worker>>,
    mailer: WeakEntity<Mailer>,
    screens: bool,
}

enum Preview {
    Off,
    Rendering,
    Shown(Document),
}

/// Подсказки адреса под активным полем.
#[derive(Default)]
struct Suggest {
    field: Option<Field>,
    items: Vec<String>,
    selected: usize,
}

/// Снимок окна для `--screens`: кадры и дескриптор окна.
#[derive(Default)]
struct Screen {
    on: bool,
    frames: u32,
    hwnd: Option<isize>,
}

pub(crate) struct Composer {
    pub(crate) model: Model,
    to: Entity<InputState>,
    cc: Entity<InputState>,
    bcc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    focus: FocusHandle,
    mailer: WeakEntity<Mailer>,
    book: Book,
    worker: Arc<Mutex<Worker>>,
    preview: Preview,
    preview_gen: u64,
    suggest: Suggest,
    /// Таймер автосохранения запущен.
    autosave_armed: bool,
    /// Текст модели поменялся не от ввода (подпись, сцена снимка) — переписать поля при отрисовке.
    sync_inputs: bool,
    /// Закрыть окно при следующей отрисовке (итог пришёл событием, без `Window`).
    close: bool,
    title: String,
    screen: Screen,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    fn new(draft: Draft, open: Open, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (from, signature) = open.me.map_or((None, String::new()), |(a, s)| (Some(a), s));
        let model = Model::new(draft, from, &signature, Instant::now());
        let mut line = |f: Field, placeholder: &str, cx: &mut Context<Self>| {
            let value = model.field(f).to_string();
            let placeholder = tr!(placeholder);
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let to = line(Field::To, "compose-to-placeholder", cx);
        let cc = line(Field::Cc, "compose-cc-placeholder", cx);
        let bcc = line(Field::Bcc, "compose-bcc-placeholder", cx);
        let subject = line(Field::Subject, "compose-subject-placeholder", cx);
        let body_text = model.body.clone();
        let body = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(tr!("compose-body-placeholder")).default_value(body_text)
        });
        let mut subscriptions = Vec::new();
        for (f, state) in [(Field::To, &to), (Field::Cc, &cc), (Field::Bcc, &bcc), (Field::Subject, &subject)] {
            subscriptions.push(cx.subscribe_in(state, window, move |this, state, ev: &InputEvent, _, cx| {
                let value = state.read(cx).value().to_string();
                this.on_input(f, value, ev, cx);
            }));
        }
        subscriptions.push(cx.subscribe_in(&body, window, |this, state, ev: &InputEvent, _, cx| {
            let value = state.read(cx).value().to_string();
            this.on_input(Field::Body, value, ev, cx);
        }));
        // Крестик окна: с несохранённым — строка в окне, окно остаётся.
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |c, cx| {
                let close = c.model.request_close();
                cx.notify();
                close
            })
            .unwrap_or(true)
        });
        // Новое письмо — курсор в «Кому», ответ (адресаты уже есть) — в текст.
        if model.to.is_empty() {
            to.update(cx, |s, cx| s.focus(window, cx));
        } else {
            body.update(cx, |s, cx| s.focus(window, cx));
        }
        Self {
            model,
            to,
            cc,
            bcc,
            subject,
            body,
            focus: cx.focus_handle(),
            mailer: open.mailer,
            book: open.book,
            worker: open.worker,
            preview: Preview::Off,
            preview_gen: 0,
            suggest: Suggest::default(),
            autosave_armed: false,
            sync_inputs: false,
            close: false,
            title: String::new(),
            screen: Screen { on: open.screens, ..Default::default() },
            _subscriptions: subscriptions,
        }
    }

    /// Дескриптор окна для снимка, когда окно уже нарисовано несколько раз.
    pub(crate) fn screen_hwnd(&self) -> Option<isize> {
        self.screen.hwnd.filter(|_| self.screen.frames >= 3)
    }

    fn on_input(&mut self, f: Field, value: String, ev: &InputEvent, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Change => {
                self.model.set(f, value);
                if self.suggest.field == Some(f) {
                    self.refresh_suggestions();
                }
                self.schedule_autosave(cx);
            }
            InputEvent::Focus => {
                self.suggest.field = f.is_address().then_some(f);
                self.refresh_suggestions();
            }
            _ => return,
        }
        cx.notify();
    }

    fn refresh_suggestions(&mut self) {
        self.suggest.selected = 0;
        self.suggest.items.clear();
        let Some(f) = self.suggest.field else { return };
        let token = current_token(self.model.field(f));
        if token.chars().count() < SUGGEST_FROM || token.contains('<') {
            return;
        }
        let now = jiff::Timestamp::now().as_second();
        self.suggest.items =
            self.book.borrow().suggest(token, SUGGESTIONS, now).into_iter().map(|e| e.display()).collect();
    }

    fn suggesting(&self, f: Field) -> bool {
        self.suggest.field == Some(f) && !self.suggest.items.is_empty()
    }

    fn input(&self, f: Field) -> Option<&Entity<InputState>> {
        match f {
            Field::To => Some(&self.to),
            Field::Cc => Some(&self.cc),
            Field::Bcc => Some(&self.bcc),
            Field::Subject => Some(&self.subject),
            Field::Body => None,
        }
    }

    fn accept(&mut self, f: Field, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.suggest.items.get(i) else { return };
        let value = accept_suggestion(self.model.field(f), item);
        if let Some(state) = self.input(f) {
            state.update(cx, |s, cx| {
                s.set_value(value.clone(), window, cx);
                s.set_selected_range(value.len()..value.len(), cx);
                s.focus(window, cx);
            });
        }
        self.model.set(f, value);
        self.refresh_suggestions();
        self.schedule_autosave(cx);
        cx.notify();
    }

    fn move_suggestion(&mut self, f: Field, delta: isize, cx: &mut Context<Self>) -> bool {
        if !self.suggesting(f) {
            return false;
        }
        let last = self.suggest.items.len() - 1;
        self.suggest.selected = self.suggest.selected.saturating_add_signed(delta).min(last);
        cx.notify();
        true
    }

    /// Таймер автосохранения — только при изменениях: в простое окно не просыпается.
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        if self.autosave_armed {
            return;
        }
        let Some(wait) = self.model.autosave_wait(Instant::now()) else { return };
        self.autosave_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |c, cx| {
                c.autosave_armed = false;
                if c.model.should_autosave(Instant::now()) {
                    c.dispatch(Purpose::Autosave, cx);
                } else {
                    c.schedule_autosave(cx);
                }
            });
        })
        .detach();
    }

    /// Черновик — в синхронизацию через шов экрана. Нет сервера — строка в окне (и тост, если просил
    /// пользователь), текст остаётся.
    fn dispatch(&mut self, purpose: Purpose, cx: &mut Context<Self>) {
        if self.model.busy() {
            return;
        }
        let Some(command) = self.model.begin(purpose, next_ticket(), Instant::now()) else {
            cx.notify();
            return;
        };
        let accepted = self
            .mailer
            .update(cx, |m, cx| {
                let ok = m.backend.send(command);
                if !ok && purpose != Purpose::Autosave {
                    m.needs_server(cx);
                }
                ok
            })
            .unwrap_or(false);
        if !accepted {
            self.model.refused();
        }
        cx.notify();
    }

    /// Итог отправки или сохранения — по тикету.
    fn on_sync(&mut self, ev: &Event, cx: &mut Context<Self>) {
        match ev {
            Event::Sent { ticket } => self.close |= self.model.on_sent(*ticket),
            Event::DraftSaved { ticket, id } => {
                let at = jiff::Zoned::now().strftime("%H:%M").to_string();
                self.close |= self.model.on_saved(*ticket, id.clone(), at);
                // Пока сохранялось, могли набрать ещё.
                self.schedule_autosave(cx);
            }
            Event::ComposeFailed { ticket, error } => {
                self.model.on_failed(*ticket, error.clone());
            }
            Event::Me { address, signature } => self.sync_inputs |= self.model.set_identity(address.clone(), signature),
            _ => return,
        }
        cx.notify();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.suggest.items.is_empty() {
            self.suggest.items.clear();
        } else if self.model.confirm_close {
            self.answer(CloseAnswer::Cancel, window, cx);
        } else if self.model.request_close() {
            window.remove_window();
        }
        cx.notify();
    }

    fn answer(&mut self, answer: CloseAnswer, window: &mut Window, cx: &mut Context<Self>) {
        match self.model.answer_close(answer) {
            CloseStep::Close => window.remove_window(),
            CloseStep::Save => self.dispatch(Purpose::Save, cx),
            CloseStep::Stay => {}
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths =
            cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |c, cx| c.attach(paths, cx));
            }
        })
        .detach();
    }

    /// Файлы читаются в фоне: поток окна не ждёт диск.
    fn attach(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|p| {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (postman_core::files::safe_name(&name), std::fs::read(&p))
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let files = task.await;
            let _ = this.update(cx, |c, cx| {
                for (name, data) in files {
                    match data {
                        Ok(data) => c.model.add_attachment(Attachment {
                            mime: postman_core::files::mime_for(&name).into(),
                            name,
                            data,
                            inline_cid: None,
                            blob_id: None,
                        }),
                        Err(e) => c.model.status = Status::Attach { name, error: e.to_string() },
                    }
                }
                c.schedule_autosave(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_attachment(&mut self, i: usize, cx: &mut Context<Self>) {
        self.model.remove_attachment(i);
        self.schedule_autosave(cx);
        cx.notify();
    }

    /// Предпросмотр: Markdown → HTML → IR в воркере-песочнице, как входящие письма.
    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_gen += 1;
        if !matches!(self.preview, Preview::Off) {
            self.preview = Preview::Off;
            self.body.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        self.preview = Preview::Rendering;
        window.focus(&self.focus, cx);
        let generation = self.preview_gen;
        let html = postman_ir::markdown_to_html(&self.model.body);
        let worker = self.worker.clone();
        let task = cx.background_spawn(async move {
            let mut w = worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            w.render(&Request::Html(html))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |c, cx| {
                if c.preview_gen != generation {
                    return;
                }
                let doc = result
                    .unwrap_or_else(|e| postman_ir::from_plain(&tr!("compose-preview-error", error = e.to_string())));
                c.preview = Preview::Shown(doc);
                cx.notify();
            });
        })
        .detach();
    }

    fn status_line(&self) -> Option<(String, bool)> {
        let busy = |p: Purpose| match p {
            Purpose::Send => tr!("compose-sending"),
            Purpose::Save => tr!("compose-saving"),
            Purpose::Autosave => tr!("compose-autosaving"),
        };
        Some(match &self.model.status {
            Status::None => return None,
            Status::Busy(p) => (busy(*p), false),
            Status::Saved { at } => (tr!("compose-saved", time = at.clone()), false),
            Status::BadAddresses(bad) => (tr!("compose-bad-addresses", list = bad.join(", ")), true),
            Status::NoRecipients => (tr!("compose-no-recipients"), true),
            Status::NoServer(Purpose::Send) => (tr!("compose-no-server-send"), true),
            Status::NoServer(_) => (tr!("compose-no-server-save"), true),
            Status::Failed(e) => (tr!("compose-failed", error = e.clone()), true),
            Status::Attach { name, error } => {
                (tr!("compose-attach-failed", name = name.clone(), error = error.clone()), true)
            }
            Status::Large(total) => (tr!("compose-large", size = size_text(*total)), true),
        })
    }

    fn address_row(&self, f: Field, label: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.input(f) else { return div().into_any_element() };
        let bad = self.model.bad(f);
        let label = div()
            .w(px(LABEL_W))
            .flex_none()
            .text_color(rgb(if bad.is_empty() { MUTED } else { ERROR }))
            .child(tr!(label));
        let field = div()
            .flex_1()
            .min_w_0()
            .rounded_md()
            .border_1()
            .border_color(if bad.is_empty() { rgba(0) } else { rgb(ERROR) })
            .child(Input::new(state))
            .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                if this.move_suggestion(f, 1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(move |this, _: &MoveUp, _, cx| {
                if this.move_suggestion(f, -1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(move |this, a: &Enter, window, cx| {
                if !a.secondary && this.suggesting(f) {
                    this.accept(f, this.suggest.selected, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(move |this, _: &IndentInline, window, cx| {
                if this.suggesting(f) {
                    this.accept(f, this.suggest.selected, window, cx);
                    cx.stop_propagation();
                }
            }));
        let mut col =
            div().flex().flex_col().gap_0p5().child(div().flex().items_center().gap_2().child(label).child(field));
        if !bad.is_empty() {
            col = col.child(
                div()
                    .pl(px(LABEL_W + 8.))
                    .text_size(px(12.))
                    .text_color(rgb(ERROR))
                    .child(tr!("compose-bad-addresses", list = bad.join(", "))),
            );
        }
        if self.suggesting(f) {
            let items = self.suggest.items.iter().enumerate().map(|(i, s)| {
                div()
                    .id(ElementId::NamedInteger("suggest".into(), i as u64))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(i == self.suggest.selected, |d| d.bg(rgb(0xdbe9f7)))
                    .hover(|d| d.bg(rgb(0xececec)))
                    .child(s.clone())
                    .on_click(cx.listener(move |this, _, window, cx| this.accept(f, i, window, cx)))
            });
            col = col.child(
                div()
                    .ml(px(LABEL_W + 8.))
                    .p_1()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .children(items),
            );
        }
        col.into_any_element()
    }
}

fn button(id: &'static str, text: String, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .when(primary, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)).hover(|d| d.bg(rgb(0x115ea3))))
        .when(!primary, |d| d.bg(rgb(0xececec)).hover(|d| d.bg(rgb(0xe0e0e0))))
        .child(text)
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.close {
            window.remove_window();
        }
        if std::mem::take(&mut self.sync_inputs) {
            for f in [Field::To, Field::Cc, Field::Bcc, Field::Subject] {
                let value = self.model.field(f).to_string();
                if let Some(state) = self.input(f) {
                    state.update(cx, |s, cx| s.set_value(value, window, cx));
                }
            }
            let body = self.model.body.clone();
            self.body.update(cx, |s, cx| s.set_value(body, window, cx));
        }
        let title =
            if self.model.subject.trim().is_empty() { tr!("compose-title-new") } else { self.model.subject.clone() };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        if self.screen.on {
            self.screen.frames += 1;
            if self.screen.hwnd.is_none() {
                self.screen.hwnd = sys::window_id(window).ok();
            }
            if self.screen.frames < 8 {
                window.request_animation_frame();
            }
        }

        let busy = self.model.busy();
        let previewing = !matches!(self.preview, Preview::Off);
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                button("send", tr!("compose-send"), true)
                    .when(busy, |d| d.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.dispatch(Purpose::Send, cx))),
            )
            .child(
                button("attach", tr!("compose-attach"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
            )
            .child(
                button("preview", tr!("compose-preview"), false)
                    .when(previewing, |d| d.bg(rgb(0xdbe9f7)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
            );

        let attachments: Vec<AnyElement> = self
            .model
            .attachments()
            .map(|(i, a)| {
                let size =
                    if a.data.is_empty() { String::new() } else { format!(" ({})", size_text(a.data.len() as u64)) };
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(rgb(0xf0f0f0))
                    .child(format!("📎 {}{size}", postman_core::files::safe_name(&a.name)))
                    .child(
                        div()
                            .id(ElementId::NamedInteger("remove-attachment".into(), i as u64))
                            .px_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(MUTED))
                            .hover(|d| d.bg(rgb(0xe0e0e0)).text_color(rgb(ERROR)))
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(i, cx))),
                    )
                    .into_any_element()
            })
            .collect();

        let fields = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(self.address_row(Field::To, "compose-to", cx))
            .child(self.address_row(Field::Cc, "compose-cc", cx))
            .child(self.address_row(Field::Bcc, "compose-bcc", cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(LABEL_W)).flex_none().text_color(rgb(MUTED)).child(tr!("compose-subject")))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.subject))),
            )
            .when(!attachments.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div().w(px(LABEL_W)).flex_none().text_color(rgb(MUTED)).child(tr!("compose-attachments")),
                        )
                        .children(attachments),
                )
            });

        let content: AnyElement = match &self.preview {
            Preview::Off => {
                div().flex_1().min_h_0().p_2().child(Textarea::new(&self.body).h(relative(1.))).into_any_element()
            }
            Preview::Rendering => {
                div().flex_1().p_3().text_color(rgb(MUTED)).child(tr!("compose-preview-rendering")).into_any_element()
            }
            Preview::Shown(doc) => div()
                .id("preview")
                .flex_1()
                .min_h_0()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .overflow_y_scroll()
                .children(ir_view::blocks(&doc.blocks, "preview", &ir_view::Images::new(), cx))
                .into_any_element(),
        };

        let confirm = self.model.confirm_close.then(|| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(rgb(0xfff4ce))
                .border_t_1()
                .border_color(rgb(BORDER))
                .child(tr!("compose-confirm-close"))
                .child(
                    button("close-save", tr!("compose-confirm-yes"), true)
                        .on_click(cx.listener(|this, _, window, cx| this.answer(CloseAnswer::Save, window, cx))),
                )
                .child(
                    button("close-discard", tr!("compose-confirm-no"), false)
                        .on_click(cx.listener(|this, _, window, cx| this.answer(CloseAnswer::Discard, window, cx))),
                )
                .child(
                    button("close-cancel", tr!("compose-confirm-cancel"), false)
                        .on_click(cx.listener(|this, _, window, cx| this.answer(CloseAnswer::Cancel, window, cx))),
                )
        });

        let status = self.status_line().map(|(text, error)| {
            div()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(if error { ERROR } else { MUTED }))
                .child(text)
        });

        div()
            .key_context("Composer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &compose::Send, _, cx| this.dispatch(Purpose::Send, cx)))
            .on_action(cx.listener(|this, _: &compose::Attach, _, cx| this.pick_files(cx)))
            .on_action(cx.listener(|this, _: &compose::Preview, window, cx| this.toggle_preview(window, cx)))
            .on_action(cx.listener(|this, _: &compose::Close, window, cx| this.escape(window, cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach(paths.paths().to_vec(), cx)))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(rgb(0xe8f1fb)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xfafafa))
            .text_color(rgb(0x222222))
            .text_size(px(13.))
            .child(toolbar)
            .child(fields)
            .child(content)
            .children(confirm)
            .children(status)
    }
}

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{
        AUTOSAVE, Address, Attachment, CloseAnswer, CloseStep, Command, Draft, Field, Instant, Model, Purpose, Status,
        accept_suggestion, current_token,
    };

    fn model() -> Model {
        Model::new(Draft::default(), Some(Address::new("Я", "me@corp.example")), "", Instant::now())
    }

    fn fill(m: &mut Model) {
        m.set(Field::To, "Анна <anna@corp.example>; petr@corp.example".into());
        m.set(Field::Cc, "ivan@corp.example".into());
        m.set(Field::Bcc, "boss@corp.example".into());
        m.set(Field::Subject, "Отчёт".into());
        m.set(Field::Body, "Привет, **Анна**!".into());
    }

    #[test]
    fn fields_make_a_draft_and_bcc_stays_in_the_envelope() {
        let mut m = model();
        fill(&mut m);
        let d = m.draft(true).unwrap();
        assert_eq!(d.from, Address::new("Я", "me@corp.example"));
        assert_eq!(d.to, vec![Address::new("Анна", "anna@corp.example"), Address::new("", "petr@corp.example")]);
        assert_eq!(d.cc, vec![Address::new("", "ivan@corp.example")]);
        assert_eq!(d.bcc, vec![Address::new("", "boss@corp.example")]);
        assert_eq!((d.subject.as_str(), d.markdown.as_str()), ("Отчёт", "Привет, **Анна**!"));
        // Скрытая копия — в конверте отправки, но не в заголовках письма.
        let envelope: Vec<&str> = d.recipients().map(|a| a.email.as_str()).collect();
        assert!(envelope.contains(&"boss@corp.example"));
        let raw = postman_compose::build(&d, 0, "x@corp.example").unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("boss@corp.example"));
    }

    #[test]
    fn send_and_save_go_through_tickets() {
        let mut m = model();
        fill(&mut m);
        let Some(Command::Send { ticket: 7, replace: None, .. }) = m.begin(Purpose::Send, 7, Instant::now()) else {
            panic!("send command expected")
        };
        assert!(m.busy());
        assert!(!m.on_sent(8), "чужой тикет");
        assert!(m.on_sent(7));
        assert!(!m.is_dirty() && !m.busy());
    }

    #[test]
    fn dirty_and_clean() {
        let mut m = model();
        assert!(!m.is_dirty(), "новое письмо не грязное");
        assert!(m.request_close(), "чистое закрывается сразу");
        m.set(Field::Subject, "Тема".into());
        assert!(m.is_dirty());
        // Сохранено на сервере — снова чистое, следующее сохранение заменит эту версию.
        let Some(Command::SaveDraft { .. }) = m.begin(Purpose::Save, 1, Instant::now()) else {
            panic!("save expected")
        };
        assert!(!m.on_saved(1, "draft-1".into(), "12:00".into()) && !m.is_dirty());
        m.set(Field::Body, "текст".into());
        let Some(Command::SaveDraft { replace: Some(id), .. }) = m.begin(Purpose::Save, 2, Instant::now()) else {
            panic!("save expected")
        };
        assert_eq!(id, "draft-1");
        // Демо: сервера нет — текст остаётся, окно грязное.
        m.refused();
        assert!(m.is_dirty() && !m.busy());
        assert_eq!(m.status, Status::NoServer(Purpose::Save));
        assert_eq!(m.body, "текст");
        // Вложение — тоже изменение.
        let mut m = model();
        m.add_attachment(Attachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            data: b"x".to_vec(),
            inline_cid: None,
            blob_id: None,
        });
        assert!(m.is_dirty());
        m.remove_attachment(0);
        assert!(!m.is_dirty());
    }

    #[test]
    fn closing_with_changes_asks_in_the_window() {
        let mut m = model();
        m.set(Field::Body, "текст".into());
        assert!(!m.request_close() && m.confirm_close);
        assert_eq!(m.answer_close(CloseAnswer::Cancel), CloseStep::Stay);
        assert!(!m.confirm_close);
        m.request_close();
        assert_eq!(m.answer_close(CloseAnswer::Save), CloseStep::Save);
        m.begin(Purpose::Save, 3, Instant::now());
        assert!(m.on_saved(3, "d".into(), "12:00".into()), "после «Да» окно закрывается, когда черновик сохранён");
        m.set(Field::Body, "ещё".into());
        m.request_close();
        assert_eq!(m.answer_close(CloseAnswer::Discard), CloseStep::Close);
    }

    #[test]
    fn bad_address_is_reported() {
        let mut m = model();
        m.set(Field::To, "anna@corp.example, мусор; petr@corp".into());
        assert_eq!(m.bad(Field::To), ["мусор", "petr@corp"]);
        assert!(m.bad(Field::Cc).is_empty());
        assert_eq!(m.draft(false).unwrap_err(), Status::BadAddresses(vec!["мусор".into(), "petr@corp".into()]));
        assert!(m.begin(Purpose::Send, 1, Instant::now()).is_none());
        assert!(!m.busy());
        let mut m = model();
        m.set(Field::Subject, "без адресатов".into());
        assert_eq!(m.draft(true).unwrap_err(), Status::NoRecipients);
        assert!(m.draft(false).is_ok(), "черновик без адресатов сохраняется");
    }

    #[test]
    fn autosave_decision() {
        let t0 = Instant::now();
        let mut m = Model::new(Draft::default(), None, "", t0);
        assert_eq!(m.autosave_wait(t0), None, "без изменений — не нужно");
        m.set(Field::Body, "текст".into());
        assert_eq!(m.autosave_wait(t0), Some(AUTOSAVE));
        assert!(!m.should_autosave(t0 + AUTOSAVE / 2));
        assert!(m.should_autosave(t0 + AUTOSAVE));
        let t1 = t0 + AUTOSAVE;
        m.begin(Purpose::Autosave, 5, t1);
        assert_eq!(m.autosave_wait(t1), None, "идёт сохранение");
        m.refused();
        assert_eq!(m.autosave_wait(t1 + AUTOSAVE), None, "тот же текст не сохранился — не повторяем");
        m.set(Field::Body, "текст 2".into());
        assert!(m.should_autosave(t1 + AUTOSAVE), "текст изменился — снова");
        m.begin(Purpose::Autosave, 6, t1 + AUTOSAVE);
        m.on_failed(6, "сбой".into());
        assert_eq!(m.status, Status::Failed("сбой".into()));
        assert_eq!(m.autosave_wait(t1 + AUTOSAVE * 3), None);
        m.set(Field::Body, "текст 3".into());
        m.request_close();
        assert_eq!(m.autosave_wait(t1 + AUTOSAVE * 3), None, "пока открыт вопрос о закрытии");
    }

    #[test]
    fn signature_goes_into_an_untouched_new_message() {
        let m = Model::new(Draft::default(), None, "С уважением,\nЯ", Instant::now());
        assert_eq!(m.body, "\n\nС уважением,\nЯ");
        assert!(!m.is_dirty());
        let mut m = model();
        assert!(m.set_identity(Address::new("Я", "me@corp.example"), "Подпись"));
        assert!(!m.is_dirty() && m.body == "\n\nПодпись");
        m.set(Field::Body, "моё".into());
        assert!(!m.set_identity(Address::new("Я", "me@corp.example"), "Подпись"), "набранный текст не трогаем");
    }

    #[test]
    fn suggestion_replaces_the_typed_address() {
        assert_eq!(current_token("anna@x.ru; ив"), "ив");
        assert_eq!(current_token("ив"), "ив");
        assert_eq!(
            accept_suggestion("anna@x.ru; ив", "Иван <ivan@corp.example>"),
            "anna@x.ru; Иван <ivan@corp.example>; "
        );
        assert_eq!(accept_suggestion("ив", "Иван <ivan@corp.example>"), "Иван <ivan@corp.example>; ");
    }
}

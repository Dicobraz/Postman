//! Область чтения: шапка письма, вложения, карточка приглашения и тело, разобранное воркером-песочницей.
//!
//! Чужие данные (HTML, картинки, `.ics`) разбирает только воркер. Байты вложений — из демо-ящика, из хранилища
//! или с сервера (`Command::Blob` → `Event::BlobReady` в [`ReadingPane::on_sync`]); диск, воркер и сервер — в фоне.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::{AttachmentRow, BodyRow, Cache, EmailRow};
use postman_calendar::{Invitation, Partstat};
use postman_core::files;
use postman_core::invite::Delivery;
use postman_core::sync::{Command, Event};
use postman_ir::Document;
use postman_worker::{Client as Worker, Level, Request, Rgba};

use crate::attachments::{self, FileAct};
use crate::invite_card::{self, Card, State};
use crate::ir_view::{self, Images};
use crate::*;

/// Наибольшая сторона картинки в области чтения.
const MAX_IMAGE_SIDE: u32 = 1600;
/// Ответ воркера дольше — сбой (воркер перезапускается).
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Reading {
    pub(crate) row: EmailRow,
    pub(crate) body: Option<BodyRow>,
    pub(crate) doc: Option<Document>,
    /// Картинки `cid:`, декодированные воркером.
    pub(crate) images: Images,
    /// `cid:`, за которыми уже пошли (байты, декодирование).
    pub(crate) requested: HashSet<String>,
    pub(crate) invite: Option<Card>,
}

// --- область чтения ---

pub(crate) struct ReadingPane {
    pub(crate) worker: Arc<Mutex<Worker>>,
    pub(crate) cache: Cache,
    pub(crate) reading: Option<Reading>,
    /// Ящик на сервере (ставит S3-04 при входе): байты вложений — из хранилища или командой `Blob`,
    /// ответ на приглашение — командой `RespondInvite`. Иначе — демо-ящик.
    pub(crate) live: bool,
    /// Мой адрес — кто я среди участников приглашения (`Event::Me`; в демо — адрес демо-ящика).
    pub(crate) me: Option<String>,
    /// Действия с вложениями, ждущие байтов.
    pub(crate) pending_files: Vec<(AttachmentRow, FileAct)>,
    /// Блобы, за которыми пошли (хранилище или сервер) и которых ещё нет.
    pub(crate) in_flight: HashSet<String>,
    /// Куда «Открыть» кладёт файл перед запуском системного приложения.
    pub(crate) open_dir: PathBuf,
    /// Картинки закрытого письма: убрать из атласа gpui при следующей отрисовке (S4-08). Сам gpui их не
    /// убирает, пока жив процесс: без этого каждое открытое письмо с картинкой оставляло в атласе (память GPU,
    /// на встроенной графике — память процесса) свой кадр.
    pub(crate) retired: Vec<Arc<RenderImage>>,
    /// Беседа открытого письма (S5-05, `thread.rs`); `feed_on` — показана лента, `quotes` — с цитатами.
    pub(crate) feed: Option<crate::thread::Feed>,
    pub(crate) feed_on: bool,
    pub(crate) quotes: bool,
    /// Мои адреса отправителя (`Event::Senders`): свои письма в ленте беседы.
    pub(crate) mine: Vec<String>,
}

/// Итог разбора и действий: уровень песочницы (строка состояния), тосты, запросы к серверу.
pub(crate) enum ReadingEvent {
    Sandbox(Level),
    Error(String),
    Toast(Toast, String),
    /// Действию нужен сервер, а его нет (демо).
    NeedsServer,
    /// Команда синхронизации: `Mailer` отдаёт её в `Backend`; не ушла — [`ReadingPane::not_sent`].
    Sync(SyncRequest),
    /// Открыть письмо (щелчок по письму в ленте беседы).
    Open(EmailRow),
}

/// Запрос области чтения к серверу: клонируемая форма `Command` (событие gpui отдаётся по ссылке).
#[derive(Clone)]
pub(crate) enum SyncRequest {
    Blob(AttachmentRow),
    RespondInvite { invite: Box<Invitation>, status: Partstat, in_reply_to: Option<String> },
    /// Беседа с сервера (ключ цепочки).
    Thread(String),
}

impl SyncRequest {
    pub(crate) fn command(&self) -> Command {
        match self {
            SyncRequest::Blob(a) => Command::Blob { blob_id: a.blob_id.clone(), name: a.name.clone(), mime: a.mime.clone() },
            SyncRequest::RespondInvite { invite, status, in_reply_to } => {
                Command::RespondInvite { invite: invite.clone(), status: *status, in_reply_to: in_reply_to.clone() }
            }
            SyncRequest::Thread(key) => Command::Thread(key.clone()),
        }
    }
}

impl EventEmitter<ReadingEvent> for ReadingPane {}

impl ReadingPane {
    pub(crate) fn new(cache: Cache) -> Self {
        Self {
            worker: Arc::new(Mutex::new(Worker::new(Worker::default_exe(), WORKER_TIMEOUT))),
            cache,
            reading: None,
            live: false,
            me: Some(postman_demo::ME.to_string()),
            pending_files: Vec::new(),
            in_flight: HashSet::new(),
            open_dir: data_dir().join("open"),
            retired: Vec::new(),
            feed: None,
            feed_on: false,
            quotes: false,
            mine: Vec::new(),
        }
    }

    /// Показать другое письмо (или ничего): картинки прежнего — в [`ReadingPane::retired`].
    fn replace(&mut self, next: Option<Reading>) {
        if let Some(old) = std::mem::replace(&mut self.reading, next) {
            self.retired.extend(old.images.into_values());
        }
    }

    pub(crate) fn open(&mut self, row: EmailRow, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        let body = Some(postman_demo::body(&row.id));
        self.replace(Some(Reading { row, body, doc: None, images: Images::new(), requested: HashSet::new(), invite: None }));
        self.render_body(cx);
        self.ensure_invite(cx);
        self.sync_thread(cx);
        cx.notify();
    }

    /// Живой ящик (S3-04): тело — из кэша или с сервера, приходит позже (`show_body`).
    pub(crate) fn open_live(&mut self, row: EmailRow, cx: &mut Context<Self>) {
        if self.reading.as_ref().is_some_and(|r| r.row.id == row.id) {
            return;
        }
        self.replace(Some(Reading { row, body: None, doc: None, images: Images::new(), requested: HashSet::new(), invite: None }));
        self.sync_thread(cx);
        cx.notify();
    }

    /// Тело открытого письма пришло (кэш, `BodyReady`): разобрать в воркере.
    pub(crate) fn show_body(&mut self, id: &str, body: postman_cache::BodyRow, cx: &mut Context<Self>) {
        let Some(r) = self.reading.as_mut().filter(|r| r.row.id == id) else { return };
        r.body = Some(body);
        r.doc = None;
        self.render_body(cx);
        self.ensure_invite(cx);
        cx.notify();
    }

    /// Событие синхронизации: блоб вложения, ответ на приглашение, мой адрес.
    pub(crate) fn on_sync(&mut self, ev: &Event, cx: &mut Context<Self>) {
        match ev {
            Event::BlobReady { blob_id, data } => self.blob_ready(blob_id, data.clone(), cx),
            Event::BlobFailed { blob_id, error } => self.blob_failed(blob_id, error.clone(), cx),
            Event::InviteAnswered { uid, status, delivery } => {
                let (cache, key, value) = (self.cache.clone(), invite_card::state_key(uid), invite_card::encode(*status));
                cx.background_spawn(async move {
                    if let Err(e) = cache.set_state(&key, value) {
                        tracing::warn!("invite answer is not stored: {e}");
                    }
                })
                .detach();
                if let Some(card) = self.card_for(uid) {
                    card.answer = Some(*status);
                    card.sending = None;
                }
                let text = match delivery {
                    Delivery::Server => tr!("reading-invite-sent-server"),
                    Delivery::Mail { imported: true } => tr!("reading-invite-sent-imported"),
                    Delivery::Mail { imported: false } => tr!("reading-invite-sent-mail"),
                };
                cx.emit(ReadingEvent::Toast(Toast::Success, text));
                cx.notify();
            }
            Event::InviteFailed { uid, error } => {
                if let Some(card) = self.card_for(uid) {
                    card.sending = None;
                }
                cx.emit(ReadingEvent::Toast(Toast::Error, tr!("reading-invite-failed", error = error.as_str())));
                cx.notify();
            }
            Event::Me { address, .. } => {
                self.me = Some(address.email.clone());
                cx.notify();
            }
            Event::Senders(list) => self.mine = list.iter().map(|s| s.address.email.clone()).collect(),
            Event::ThreadReady { thread, .. } => self.thread_ready(thread, cx),
            _ => {}
        }
    }

    /// Запрос не ушёл (сервера нет, поток синхронизации остановился): ничего не ждём.
    pub(crate) fn not_sent(&mut self, req: &SyncRequest, cx: &mut Context<Self>) {
        match req {
            SyncRequest::Blob(a) => {
                self.in_flight.remove(&a.blob_id);
                self.pending_files.retain(|(p, _)| p.blob_id != a.blob_id);
                if let Some(card) = self.reading.as_mut().and_then(|r| r.invite.as_mut()).filter(|c| c.waits_for(&a.blob_id)) {
                    card.state = State::Failed(tr!("reading-blob-unavailable"));
                }
            }
            SyncRequest::RespondInvite { invite, .. } => {
                if let Some(card) = self.card_for(&invite.event.uid) {
                    card.sending = None;
                }
            }
            SyncRequest::Thread(_) => self.thread_not_sent(),
        }
        cx.notify();
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.replace(None);
        self.feed = None;
        cx.notify();
    }

    fn card_for(&mut self, uid: &str) -> Option<&mut Card> {
        self.reading.as_mut().and_then(|r| r.invite.as_mut()).filter(|c| c.uid() == Some(uid))
    }

    /// HTML → IR в воркере-песочнице, в фоновом исполнителе gpui; затем — картинки `cid:`.
    pub(crate) fn render_body(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.reading else { return };
        let Some(body) = r.body.clone() else { return };
        let id = r.row.id.clone();
        let worker = self.worker.clone();
        let task = cx.background_spawn(async move {
            // Отравленный мьютекс (паника в прошлом разборе) не мешает: клиент воркера сам перезапускает процесс.
            let mut w = worker.lock().unwrap_or_else(PoisonError::into_inner);
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
                this.ensure_images(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Картинки `cid:` текущего письма, за которыми ещё не ходили.
    fn ensure_images(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.reading else { return };
        let (Some(doc), Some(body)) = (&r.doc, &r.body) else { return };
        let requested = &mut r.requested;
        let wanted: Vec<AttachmentRow> = ir_view::cids(doc)
            .into_iter()
            .filter(|cid| requested.insert(cid.clone()))
            .filter_map(|cid| body.attachments.iter().find(|a| a.cid.as_deref() == Some(cid.as_str())).cloned())
            .collect();
        for a in wanted {
            self.fetch(&a, cx);
        }
    }

    /// Приглашение во вложениях — на разбор в песочницу (сначала байты).
    fn ensure_invite(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.reading else { return };
        if r.invite.is_some() {
            return;
        }
        let Some(a) = r.body.as_ref().and_then(|b| b.attachments.iter().find(|a| attachments::is_invite(a))).cloned() else {
            return;
        };
        r.invite = Some(Card::loading(a.blob_id.clone()));
        self.fetch(&a, cx);
    }

    /// Байты вложения: демо — сразу; живой ящик — из хранилища (в фоне), иначе с сервера.
    fn fetch(&mut self, a: &AttachmentRow, cx: &mut Context<Self>) {
        if !self.live {
            match postman_demo::blob(&a.blob_id) {
                Some(data) => self.blob_ready(&a.blob_id, Arc::new(data), cx),
                None => self.blob_failed(&a.blob_id, tr!("reading-blob-unavailable"), cx),
            }
            return;
        }
        if !self.in_flight.insert(a.blob_id.clone()) {
            return;
        }
        let (cache, a) = (self.cache.clone(), a.clone());
        let task = cx.background_spawn(async move {
            let local = cache.blob(&a.blob_id).ok().flatten();
            (a, local)
        });
        cx.spawn(async move |this, cx| {
            let (a, local) = task.await;
            this.update(cx, |this, cx| match local {
                Some(data) => this.blob_ready(&a.blob_id, Arc::new(data), cx),
                None => cx.emit(ReadingEvent::Sync(SyncRequest::Blob(a))),
            })
            .ok();
        })
        .detach();
    }

    /// Байты пришли: приглашение — на разбор, картинки — на декодирование, ждущие действия — выполнить.
    fn blob_ready(&mut self, blob_id: &str, data: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.in_flight.remove(blob_id);
        if let Some(r) = &self.reading {
            let id = r.row.id.clone();
            let invite = r.invite.as_ref().is_some_and(|c| c.waits_for(blob_id));
            let cids: Vec<String> = r
                .body
                .iter()
                .flat_map(|b| &b.attachments)
                .filter(|a| a.blob_id == blob_id)
                .filter_map(|a| a.cid.clone())
                .filter(|c| r.requested.contains(c) && !r.images.contains_key(c))
                .collect();
            if invite {
                self.parse_invite(id.clone(), data.clone(), cx);
            }
            for cid in cids {
                self.decode_image(id.clone(), cid, data.clone(), cx);
            }
        }
        let (ready, rest) = std::mem::take(&mut self.pending_files).into_iter().partition(|(a, _)| a.blob_id == blob_id);
        self.pending_files = rest;
        for (a, act) in ready {
            self.do_file(a, act, data.clone(), cx);
        }
        cx.notify();
    }

    fn blob_failed(&mut self, blob_id: &str, error: String, cx: &mut Context<Self>) {
        self.in_flight.remove(blob_id);
        self.pending_files.retain(|(a, _)| a.blob_id != blob_id);
        if let Some(card) = self.reading.as_mut().and_then(|r| r.invite.as_mut()).filter(|c| c.waits_for(blob_id)) {
            card.state = State::Failed(error.clone());
        }
        cx.emit(ReadingEvent::Toast(Toast::Error, tr!("reading-blob-failed", error = error)));
        cx.notify();
    }

    fn decode_image(&mut self, id: String, cid: String, data: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        let worker = self.worker.clone();
        let task = cx.background_spawn(async move {
            let mut w = worker.lock().unwrap_or_else(PoisonError::into_inner);
            w.decode_image(&data, MAX_IMAGE_SIDE).and_then(render_image)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(image) => {
                    if let Some(r) = this.reading.as_mut().filter(|r| r.row.id == id) {
                        r.images.insert(cid, image);
                        cx.notify();
                    }
                }
                Err(e) => tracing::warn!("inline image {cid} is not shown: {e}"),
            })
            .ok();
        })
        .detach();
    }

    /// `.ics` → приглашение в песочнице; запомненный ответ — из хранилища.
    fn parse_invite(&mut self, id: String, data: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        let (worker, cache) = (self.worker.clone(), self.cache.clone());
        let task = cx.background_spawn(async move {
            let parsed = {
                let mut w = worker.lock().unwrap_or_else(PoisonError::into_inner);
                w.parse_calendar(&data)
            };
            let inv = parsed.and_then(|v| Invitation::from_worker(&v, &TimeZone::system()))?;
            let stored = cache.state(&invite_card::state_key(&inv.event.uid)).ok().flatten();
            let answer = stored.as_deref().and_then(invite_card::decode);
            Ok::<_, String>((inv, answer))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(card) = this.reading.as_mut().filter(|r| r.row.id == id).and_then(|r| r.invite.as_mut()) else { return };
                match result {
                    Ok((inv, answer)) => {
                        card.state = State::Ready(Box::new(inv));
                        card.answer = answer;
                    }
                    Err(e) => card.state = State::Failed(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Кнопка карточки приглашения. Демо — «нужен сервер», статус не меняется.
    pub(crate) fn answer_invite(&mut self, status: Partstat, cx: &mut Context<Self>) {
        if !self.live {
            cx.emit(ReadingEvent::NeedsServer);
            return;
        }
        let Some(r) = &mut self.reading else { return };
        let in_reply_to = r.body.as_ref().and_then(|b| b.message_id.clone());
        let Some(card) = &mut r.invite else { return };
        let Some(inv) = card.invitation().cloned() else { return };
        card.sending = Some(status);
        cx.emit(ReadingEvent::Sync(SyncRequest::RespondInvite { invite: Box::new(inv), status, in_reply_to }));
        cx.notify();
    }

    /// «Открыть» / «Сохранить как»: сначала байты, потом действие.
    pub(crate) fn file_action(&mut self, a: AttachmentRow, act: FileAct, cx: &mut Context<Self>) {
        if self.pending_files.iter().any(|(p, x)| p.blob_id == a.blob_id && *x == act) {
            return;
        }
        self.pending_files.push((a.clone(), act));
        self.fetch(&a, cx);
        cx.notify();
    }

    fn do_file(&mut self, a: AttachmentRow, act: FileAct, data: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        let name = files::safe_name(&a.name);
        match act {
            FileAct::Open => {
                let dir = self.open_dir.clone();
                let task = cx.background_spawn(async move { files::open(&dir, &a.name, &data).map(|_| ()) });
                cx.spawn(async move |this, cx| {
                    if let Err(e) = task.await {
                        let text = tr!("reading-file-failed", name = name, error = e.to_string());
                        this.update(cx, |_, cx| cx.emit(ReadingEvent::Toast(Toast::Error, text))).ok();
                    }
                })
                .detach();
            }
            FileAct::SaveAs => {
                // Диалог ОС не держит поток окна: ответ приходит каналом.
                let path = cx.prompt_for_new_path(&downloads_dir(), Some(&name));
                cx.spawn(async move |this, cx| {
                    let path = match path.await {
                        Ok(Ok(Some(p))) => p,
                        Ok(Ok(None)) | Err(_) => return,
                        Ok(Err(e)) => {
                            let text = tr!("reading-file-failed", name = name, error = e.to_string());
                            this.update(cx, |_, cx| cx.emit(ReadingEvent::Toast(Toast::Error, text))).ok();
                            return;
                        }
                    };
                    let written = cx.background_executor().spawn(async move { files::write(&path, &data).map(|()| path) }).await;
                    let event = match written {
                        Ok(p) => ReadingEvent::Toast(Toast::Success, tr!("reading-saved", path = p.display().to_string())),
                        Err(e) => ReadingEvent::Toast(Toast::Error, tr!("reading-file-failed", name = name, error = e.to_string())),
                    };
                    this.update(cx, |_, cx| cx.emit(event)).ok();
                })
                .detach();
            }
        }
    }

    fn attachment_strip(&self, r: &Reading, cx: &mut Context<Self>) -> Option<AnyElement> {
        let body = r.body.as_ref()?;
        let shown: HashSet<String> = r.doc.as_ref().map(|d| ir_view::cids(d).into_iter().collect()).unwrap_or_default();
        let card = r.invite.as_ref().is_some_and(|c| c.invitation().is_some());
        let items = attachments::listed(&body.attachments, &shown, card);
        if items.is_empty() {
            return None;
        }
        let chips = items.into_iter().enumerate().map(|(n, item)| {
            let pending = self.pending_files.iter().any(|(p, _)| p.blob_id == item.row.blob_id);
            let link = |id: &'static str, label: String, act: FileAct, row: AttachmentRow| {
                div()
                    .id((id, n))
                    .px_1()
                    .rounded_sm()
                    .text_color(rgb(0x0f6cbd))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(0xe8f1fb)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.file_action(row.clone(), act, cx)))
            };
            let mut chip = div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(0xdddddd))
                .bg(rgb(0xffffff))
                .child(attachments::icon(item.kind))
                .child(item.name.clone())
                .child(div().text_color(rgb(0x888888)).child(attachments::size_text(item.row.size)));
            if pending {
                chip = chip.child(div().text_color(rgb(0x888888)).child(tr!("reading-att-loading")));
            } else {
                if item.can_open {
                    chip = chip.child(link("att-open", tr!("reading-att-open"), FileAct::Open, item.row.clone()));
                } else {
                    chip = chip.child(div().text_color(rgb(0xb4441c)).child(tr!("reading-att-only-save")));
                }
                chip = chip.child(link("att-save", tr!("reading-att-save"), FileAct::SaveAs, item.row));
            }
            chip
        });
        Some(div().flex().flex_wrap().gap_2().children(chips).into_any_element())
    }
}

/// Пиксели воркера (RGBA) → кадр gpui (BGRA).
pub(crate) fn render_image(rgba: Rgba) -> Result<Arc<RenderImage>, String> {
    let Rgba { width, height, mut pixels } = rgba;
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(width, height, pixels).ok_or_else(|| format!("{width}x{height}: pixel buffer size mismatch"))?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Каталог по умолчанию для «Сохранить как».
fn downloads_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(|| PathBuf::from("."))
}

impl Render for ReadingPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
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
        let bar = self.feed_bar(cx);
        // Лента беседы — на месте письма (вложения и приглашение — у письма в обычном виде).
        if let Some(feed) = self.feed_view(cx) {
            return div()
                .id("reading")
                .flex()
                .flex_col()
                .size_full()
                .p_3()
                .gap_2()
                .overflow_y_scroll()
                .child(header)
                .children(bar)
                .child(feed)
                .into_any_element();
        }
        let strip = self.attachment_strip(r, cx);
        let card = r.invite.as_ref().map(|c| invite_card::render(c, self.me.as_deref(), cx));
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
                col.children(ir_view::blocks(&doc.blocks, "doc", &r.images, cx)).into_any_element()
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
            .children(bar)
            .children(strip)
            .children(card)
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

#[cfg(test)]
mod tests {
    // Не `use super::*`: вместе с gpui пришёл бы атрибут `gpui::test`, он перекрывает `#[test]`.
    use super::{SyncRequest, render_image};
    use postman_cache::AttachmentRow;
    use postman_core::sync::Command;
    use postman_worker::Rgba;

    #[test]
    fn worker_pixels_become_bgra() {
        let image = render_image(Rgba { width: 2, height: 1, pixels: vec![1, 2, 3, 4, 5, 6, 7, 8] }).unwrap();
        assert_eq!(image.as_bytes(0), Some(&[3, 2, 1, 4, 7, 6, 5, 8][..]));
        assert!(render_image(Rgba { width: 2, height: 2, pixels: vec![0; 4] }).is_err());
    }

    #[test]
    fn requests_become_sync_commands() {
        let a = AttachmentRow { blob_id: "b1".into(), name: "a.pdf".into(), mime: "application/pdf".into(), size: 1, cid: None, inline: false };
        match SyncRequest::Blob(a).command() {
            Command::Blob { blob_id, name, mime } => assert_eq!((blob_id.as_str(), name.as_str(), mime.as_str()), ("b1", "a.pdf", "application/pdf")),
            _ => panic!("не Blob"),
        }
    }
}

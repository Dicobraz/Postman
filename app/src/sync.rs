//! Фоновая синхронизация: свой поток с tokio, своё соединение с кэшем.
//! UI шлёт команды, в ответ получает события и перечитывает кэш.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use std::sync::Arc;

use postman_cache::{Addr, AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow};
use postman_compose::{Address, Draft};
use postman_jmap::{Auth, Client, EmailAddress, EmailSummary, Envelope, Identity, Mailbox, utc_to_unix};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Сколько писем на ящик тянуть при первом входе (остальное — по мере прокрутки, позже).
const INITIAL_PAGE: u64 = 500;
const POLL: Duration = Duration::from_secs(30);
/// Догрузка при прокрутке.
const PAGE: u64 = 500;
const SEARCH_LIMIT: u64 = 1000;

#[derive(Clone)]
pub struct Account {
    pub url: String,
    pub auth: Auth,
}

impl Account {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("POSTMAN_URL").ok()?;
        let auth = match std::env::var("POSTMAN_TOKEN") {
            Ok(t) => Auth::Bearer(t),
            Err(_) => Auth::Basic {
                user: std::env::var("POSTMAN_USER").ok()?,
                password: std::env::var("POSTMAN_PASSWORD").ok()?,
            },
        };
        Some(Self { url, auth })
    }
}

pub enum Command {
    Body(String),
    Keyword { id: String, keyword: &'static str, on: bool },
    Move { id: String, from: String, to: String },
    /// `replace` — id прежней версии черновика на сервере.
    Send { draft: Box<Draft>, replace: Option<String>, ticket: u64 },
    SaveDraft { draft: Box<Draft>, replace: Option<String>, ticket: u64 },
    /// Догрузить следующую страницу ящика (после самого старого письма в кэше).
    LoadMore { mailbox: String },
    /// Серверный поиск; `mailbox: None` — во всех папках.
    Search { query: String, mailbox: Option<String>, ticket: u64 },
    /// Скачать вложение/картинку.
    Blob { blob_id: String, name: String, mime: String },
}

pub enum Event {
    Status(String),
    MailboxesChanged,
    EmailsChanged,
    BodyReady(String),
    BlobReady { blob_id: String, data: Arc<Vec<u8>> },
    LoadedMore { mailbox: String },
    SearchResults { ticket: u64, ids: Vec<String>, total: u64 },
    BlobFailed { blob_id: String, error: String },
    /// Учётная запись отправителя и подпись (из Identity).
    Me { address: Address, signature: String },
    Sent { ticket: u64 },
    DraftSaved { ticket: u64, id: String },
    /// Ошибка конкретной отправки/сохранения — показывается в окне письма.
    ComposeFailed { ticket: u64, error: String },
    Error(String),
}

pub fn spawn(
    account: Account,
    db: PathBuf,
    events: mpsc::Sender<Event>,
    repaint: impl Fn() + Send + 'static,
) -> UnboundedSender<Command> {
    let (tx, rx) = unbounded_channel();
    std::thread::Builder::new()
        .name("sync".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
            let notify = |e: Event| {
                let _ = events.send(e);
                repaint();
            };
            rt.block_on(async {
                if let Err(e) = run(account, db, rx, &notify).await {
                    notify(Event::Error(e));
                }
            });
        })
        .expect("поток синхронизации");
    tx
}

async fn run(
    account: Account,
    db: PathBuf,
    mut rx: UnboundedReceiver<Command>,
    notify: &impl Fn(Event),
) -> Result<(), String> {
    let mut cache = Cache::open(&db).map_err(|e| e.to_string())?;
    notify(Event::Status(format!("Подключение к {}…", account.url)));
    let client = Client::connect(&account.url, account.auth).await.map_err(|e| e.to_string())?;
    notify(Event::Status(format!("Вход выполнен: {}", client.session().username)));

    let identities = client.identities().await.unwrap_or_default();
    if let Some(i) = identities.first() {
        notify(Event::Me { address: Address::new(&i.name, &i.email), signature: i.text_signature.clone() });
    }
    let mut email_state = initial_load(&client, &mut cache, notify).await.map_err(|e| e.to_string())?;
    let mut poll = tokio::time::interval(POLL);
    poll.tick().await;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                if let Err(e) = handle(&client, &mut cache, &identities, cmd, notify).await {
                    notify(Event::Error(e.to_string()));
                }
            }
            _ = poll.tick() => {
                match refresh(&client, &mut cache, &email_state).await {
                    Ok(Some(s)) => {
                        email_state = s;
                        notify(Event::MailboxesChanged);
                        notify(Event::EmailsChanged);
                    }
                    Ok(None) => {}
                    Err(e) => notify(Event::Error(e.to_string())),
                }
            }
        }
    }
}

async fn initial_load(client: &Client, cache: &mut Cache, notify: &impl Fn(Event)) -> postman_jmap::Result<String> {
    let (mailboxes, _) = client.mailboxes().await?;
    cache.replace_mailboxes(&mailboxes.iter().map(mailbox_row).collect::<Vec<_>>()).map_err(db_err)?;
    notify(Event::MailboxesChanged);

    // Входящие первыми — их пользователь видит сразу.
    let mut order: Vec<&Mailbox> = mailboxes.iter().collect();
    order.sort_by_key(|m| m.role.as_deref() != Some("inbox"));
    let mut state = String::new();
    for mb in order {
        let page = client.email_page(&mb.id, 0, INITIAL_PAGE).await?;
        cache.upsert_emails(&page.emails.iter().map(email_row).collect::<Vec<_>>()).map_err(db_err)?;
        state = page.state;
        notify(Event::EmailsChanged);
    }
    cache.set_state("Email", &state).map_err(db_err)?;
    notify(Event::Status("Синхронизировано".into()));
    Ok(state)
}

/// Дельта через `Email/changes`; `Some(новое состояние)`, если что-то изменилось.
async fn refresh(client: &Client, cache: &mut Cache, since: &str) -> postman_jmap::Result<Option<String>> {
    let mut state = since.to_string();
    loop {
        let ch = client.changes("Email", &state, 500).await?;
        if ch.new_state == state {
            return Ok(None);
        }
        let ids: Vec<String> = ch.created.into_iter().chain(ch.updated).collect();
        if !ids.is_empty() {
            let (emails, _) = client.email_get(&ids).await?;
            cache.upsert_emails(&emails.iter().map(email_row).collect::<Vec<_>>()).map_err(db_err)?;
        }
        cache.delete_emails(&ch.destroyed).map_err(db_err)?;
        state = ch.new_state;
        if !ch.has_more_changes {
            break;
        }
    }
    let (mailboxes, _) = client.mailboxes().await?;
    cache.replace_mailboxes(&mailboxes.iter().map(mailbox_row).collect::<Vec<_>>()).map_err(db_err)?;
    cache.set_state("Email", &state).map_err(db_err)?;
    Ok(Some(state))
}

async fn handle(
    client: &Client,
    cache: &mut Cache,
    identities: &[Identity],
    cmd: Command,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<()> {
    match cmd {
        Command::Body(id) => {
            let b = client.email_body(&id).await?;
            let row = BodyRow {
                to: addresses(&b.to),
                cc: addresses(&b.cc),
                reply_to: addresses(&b.reply_to),
                message_id: b.message_id,
                references: b.references,
                html: b.html,
                text: b.text,
                attachments: b
                    .attachments
                    .iter()
                    .filter_map(|p| {
                        Some(AttachmentRow {
                            blob_id: p.blob_id.clone()?,
                            name: p.name.clone().unwrap_or_else(|| "вложение".into()),
                            mime: p.mime.clone(),
                            size: p.size,
                            cid: p.cid.as_deref().map(|c| c.trim_matches(['<', '>']).to_string()),
                            inline: p.disposition.as_deref() == Some("inline"),
                        })
                    })
                    .collect(),
            };
            cache.put_body(&id, &row).map_err(db_err)?;
            notify(Event::BodyReady(id));
        }
        Command::Keyword { id, keyword, on } => client.set_keyword(&id, keyword, on).await?,
        Command::Move { id, from, to } => client.move_email(&id, &from, &to).await?,
        Command::Send { draft, replace, ticket } => {
            let r = send(client, cache, identities, &draft, replace.as_deref()).await;
            notify(match r {
                Ok(()) => Event::Sent { ticket },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
        }
        Command::LoadMore { mailbox } => {
            let page = match cache.oldest(&mailbox).map_err(db_err)? {
                Some(anchor) => client.email_page_after(&mailbox, &anchor, PAGE).await,
                None => client.email_page(&mailbox, 0, PAGE).await,
            };
            // Якорь мог исчезнуть на сервере (удалён) — тогда по позиции.
            let page = match page {
                Err(postman_jmap::Error::Method { kind, .. }) if kind == "anchorNotFound" => {
                    client.email_page(&mailbox, cache.count(&mailbox).map_err(db_err)? as u64, PAGE).await?
                }
                p => p?,
            };
            cache.upsert_emails(&page.emails.iter().map(email_row).collect::<Vec<_>>()).map_err(db_err)?;
            notify(Event::LoadedMore { mailbox });
            notify(Event::EmailsChanged);
        }
        Command::Search { query, mailbox, ticket } => {
            let page = client.search(&query, mailbox.as_deref(), SEARCH_LIMIT).await?;
            cache.upsert_emails(&page.emails.iter().map(email_row).collect::<Vec<_>>()).map_err(db_err)?;
            notify(Event::SearchResults { ticket, ids: page.ids, total: page.total });
        }
        Command::Blob { blob_id, name, mime } => {
            let acc = client.mail_account()?.to_string();
            match client.download(&acc, &blob_id, &name, &mime).await {
                Ok(data) => {
                    cache.put_blob(&blob_id, &data).map_err(db_err)?;
                    notify(Event::BlobReady { blob_id, data: Arc::new(data) });
                }
                Err(e) => notify(Event::BlobFailed { blob_id, error: e.to_string() }),
            }
        }
        Command::SaveDraft { draft, replace, ticket } => {
            let r = save_draft(client, cache, &draft, replace.as_deref()).await;
            notify(match r {
                Ok(id) => Event::DraftSaved { ticket, id },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
        }
    }
    Ok(())
}

fn mailbox_by_role(cache: &Cache, role: &str) -> postman_jmap::Result<String> {
    cache
        .mailboxes()
        .map_err(db_err)?
        .into_iter()
        .find(|m| m.role.as_deref() == Some(role))
        .map(|m| m.id)
        .ok_or_else(|| postman_jmap::Error::Protocol(format!("нет папки с ролью {role}")))
}

fn raw_message(draft: &Draft) -> postman_jmap::Result<Vec<u8>> {
    let now = jiff::Timestamp::now().as_second();
    postman_compose::build(draft, now, &postman_compose::new_message_id(&draft.from, now))
        .map_err(|e| postman_jmap::Error::Protocol(format!("сборка письма: {e}")))
}

/// Догрузить вложения оригинала (пересылка), у которых есть только blob_id.
async fn fill_attachments(client: &Client, cache: &Cache, draft: &mut Draft) -> postman_jmap::Result<()> {
    let acc = client.mail_account()?.to_string();
    for a in draft.attachments.iter_mut().filter(|a| a.data.is_empty()) {
        let Some(blob) = a.blob_id.clone() else { continue };
        a.data = match cache.blob(&blob).map_err(db_err)? {
            Some(d) => d,
            None => client.download(&acc, &blob, &a.name, &a.mime).await?,
        };
    }
    Ok(())
}

async fn send(
    client: &Client,
    cache: &Cache,
    identities: &[Identity],
    draft: &Draft,
    replace: Option<&str>,
) -> postman_jmap::Result<()> {
    let mut draft = draft.clone();
    fill_attachments(client, cache, &mut draft).await?;
    let draft = &draft;
    let identity = identities
        .iter()
        .find(|i| i.email.eq_ignore_ascii_case(&draft.from.email))
        .or(identities.first())
        .ok_or_else(|| postman_jmap::Error::Protocol("на сервере нет Identity для отправки".into()))?;
    let envelope = Envelope {
        mail_from: identity.email.clone(),
        rcpt_to: draft.recipients().map(|a| a.email.clone()).collect(),
    };
    let (drafts, sent) = (mailbox_by_role(cache, "drafts")?, mailbox_by_role(cache, "sent")?);
    client.send(raw_message(draft)?, &identity.id, &envelope, &drafts, &sent).await?;
    if let Some(old) = replace {
        client.destroy_email(old).await?;
    }
    Ok(())
}

async fn save_draft(client: &Client, cache: &Cache, draft: &Draft, replace: Option<&str>) -> postman_jmap::Result<String> {
    let mut draft = draft.clone();
    fill_attachments(client, cache, &mut draft).await?;
    let draft = &draft;
    let drafts = mailbox_by_role(cache, "drafts")?;
    client.save_draft(raw_message(draft)?, &drafts, replace).await
}

fn db_err(e: postman_cache::Error) -> postman_jmap::Error {
    postman_jmap::Error::Protocol(format!("кэш: {e}"))
}

fn addresses(list: &[EmailAddress]) -> Vec<Addr> {
    list.iter()
        .filter_map(|a| Some(Addr { name: a.name.clone().unwrap_or_default(), email: a.email.clone()? }))
        .collect()
}

fn mailbox_row(m: &Mailbox) -> MailboxRow {
    MailboxRow {
        id: m.id.clone(),
        parent_id: m.parent_id.clone(),
        name: m.name.clone(),
        role: m.role.clone(),
        sort_order: m.sort_order,
        total: m.total_emails,
        unread: m.unread_emails,
    }
}

fn email_row(e: &EmailSummary) -> EmailRow {
    let from = e.from.as_ref().and_then(|f| f.first());
    EmailRow {
        id: e.id.clone(),
        thread_id: e.thread_id.clone(),
        mailbox_ids: e.mailbox_ids.iter().filter(|(_, v)| **v).map(|(k, _)| k.clone()).collect(),
        received_at: utc_to_unix(&e.received_at).unwrap_or(0),
        subject: e.subject.clone().unwrap_or_default(),
        from_name: from.map(|a| a.display().to_string()).unwrap_or_default(),
        from_addr: from.and_then(|a| a.email.clone()).unwrap_or_default(),
        preview: e.preview.clone(),
        seen: e.has_keyword("$seen"),
        flagged: e.has_keyword("$flagged"),
        has_attachment: e.has_attachment,
        size: e.size,
    }
}

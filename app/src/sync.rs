//! Фоновая синхронизация: свой поток с tokio, своё соединение с кэшем.
//! UI шлёт команды, в ответ получает события и перечитывает кэш.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use postman_cache::{BodyRow, Cache, EmailRow, MailboxRow};
use postman_jmap::{Auth, Client, EmailAddress, EmailSummary, Mailbox, utc_to_unix};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Сколько писем на ящик тянуть при первом входе (остальное — по мере прокрутки, позже).
const INITIAL_PAGE: u64 = 500;
const POLL: Duration = Duration::from_secs(30);

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
}

pub enum Event {
    Status(String),
    MailboxesChanged,
    EmailsChanged,
    BodyReady(String),
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

    let mut email_state = initial_load(&client, &mut cache, notify).await.map_err(|e| e.to_string())?;
    let mut poll = tokio::time::interval(POLL);
    poll.tick().await;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                if let Err(e) = handle(&client, &cache, cmd, notify).await {
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

async fn handle(client: &Client, cache: &Cache, cmd: Command, notify: &impl Fn(Event)) -> postman_jmap::Result<()> {
    match cmd {
        Command::Body(id) => {
            let b = client.email_body(&id).await?;
            let row = BodyRow { to: addresses(&b.to), cc: addresses(&b.cc), html: b.html, text: b.text };
            cache.put_body(&id, &row).map_err(db_err)?;
            notify(Event::BodyReady(id));
        }
        Command::Keyword { id, keyword, on } => client.set_keyword(&id, keyword, on).await?,
        Command::Move { id, from, to } => client.move_email(&id, &from, &to).await?,
    }
    Ok(())
}

fn db_err(e: postman_cache::Error) -> postman_jmap::Error {
    postman_jmap::Error::Protocol(format!("кэш: {e}"))
}

fn addresses(list: &[EmailAddress]) -> String {
    list.iter()
        .map(|a| match (&a.name, &a.email) {
            (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
            _ => a.display().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
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

//! Фоновая синхронизация: свой поток с tokio, своё соединение с кэшем.
//! UI шлёт команды, в ответ получает события и перечитывает кэш.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc;
use std::time::Duration;

use std::sync::Arc;

use postman_cache::{AccountRow, Addr, AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow, Recipients, Rights, scoped, unscoped};
use postman_calendar::{Busy, Invitation, Partstat};
use postman_compose::{Address, Draft};

use crate::categories;
use crate::lifecycle;
use crate::link::{Connection, Link};
use crate::invite::{self, Delivery};
use crate::settings;
use crate::schedule::{self, Meeting};
use crate::send_later::{self, Scheduled};
use crate::tasks;
use postman_search::SearchIndex;
pub use postman_jmap::Release;
use postman_jmap::{Auth, Client, EmailAddress, EmailBody, EmailSummary, Envelope, Filter, Identity, Mailbox, utc_to_unix};
use postman_jmap::{MailboxRights, SharedAccount, UnsendError};
use postman_jmap::{ContactCard, Principal};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
/// Канал команд синхронизации (UI держит отправителя).
pub use tokio::sync::mpsc::UnboundedSender;

/// Сколько писем на ящик тянуть при первом входе (остальное — по мере прокрутки, позже).
const INITIAL_PAGE: u64 = 500;
/// Опрос — только страховка: изменения приходят через push (EventSource).
const POLL: Duration = Duration::from_secs(120);
/// Сервер шлёт ping с этим периодом; тишина втрое дольше — обрыв, переподключаемся.
const PUSH_PING: u32 = 60;
/// Предзагрузка тел: сколько писем за запрос, их размер и общий охват.
const PREFETCH_BATCH: usize = 25;
const PREFETCH_MAX_BYTES: u64 = 256 * 1024;
const PREFETCH_LIMIT: usize = 20_000;
/// Догрузка при прокрутке.
const PAGE: u64 = 500;
const SEARCH_LIMIT: u64 = 1000;
/// Изменения календаря приходят пачкой (событие, уведомление, ответ участника): неделя — одним запросом.
const CALENDAR_DEBOUNCE: Duration = Duration::from_millis(1500);
/// Ошибки кэша идут как `Error::Protocol` с этим началом: так их отличает [`ErrorKind::of`].
const CACHE_ERROR: &str = "cache: ";
/// Справочник целиком — не больше (подсказки адресов и раздел «Контакты»).
const DIRECTORY_LIMIT: u64 = 10_000;
/// Тела писем беседы: части больше — усекаются сервером (как у открытого письма — до 4 МБ, здесь писем много).
const THREAD_BODY_BYTES: u64 = 1024 * 1024;

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
    /// Беседа (ключ цепочки, `EmailRow::thread_id`): письма из всех папок и их тела — в кэш → `ThreadReady`.
    Thread(String),
    Keyword { id: String, keyword: &'static str, on: bool },
    /// Поставить `add` и снять `remove` (ключевые слова — категории, метки, флаг) у писем `ids` (ключи кэша) — по
    /// одному `Email/set` на аккаунт (S5-02). UI уже поменял кэш; письма, которые сервер не изменил, — с сервера
    /// заново.
    SetKeywords { ids: Vec<String>, add: Vec<String>, remove: Vec<String> },
    /// Письма по фильтру на сервере (`hasKeyword`, `notKeyword`…) → `SearchResults`. `mailbox` — ключ папки кэша;
    /// `None` — все папки своего аккаунта («Где искать: Все папки»).
    Query { filter: Filter, mailbox: Option<String>, ticket: u64 },
    /// Число писем по каждому фильтру на сервере (в папке `mailbox` или во всех) → `Counts`.
    Counts { filters: Vec<Filter>, mailbox: Option<String>, ticket: u64 },
    /// Перенести письмо (удаление в «Удалённые»). UI уже перенёс его в кэше; сбой — письмо с сервера заново.
    Move { id: String, from: String, to: String },
    /// Удалить письмо насовсем (из «Удалённых»). UI уже убрал его из кэша; сбой — письмо с сервера заново.
    Destroy(String),
    /// `replace` — id прежней версии черновика на сервере. `release` — когда сервер выпустит письмо (S5-06):
    /// `After` — окно «Отменить» (без FUTURERELEASE у сервера — сразу), `At` — «Отправить позже» → `Sent`.
    Send { draft: Box<Draft>, replace: Option<String>, ticket: u64, release: Release },
    /// Отменить отправку, которую сервер ещё держит, и вернуть письмо в «Черновики» → `Unsent` или `UnsendFailed`,
    /// затем `Scheduled`.
    Unsend { scheduled: Scheduled, ticket: u64 },
    /// Перечитать отложенные отправки → `Scheduled`.
    Scheduled,
    SaveDraft { draft: Box<Draft>, replace: Option<String>, ticket: u64 },
    /// Догрузить следующую страницу ящика (после самого старого письма в кэше).
    LoadMore { mailbox: String },
    /// Серверный поиск; `mailbox: None` — во всех папках.
    Search { query: String, mailbox: Option<String>, ticket: u64 },
    /// Скачать вложение/картинку.
    Blob { blob_id: String, name: String, mime: String },
    /// Ответить на приглашение из письма; `in_reply_to` — Message-ID письма с приглашением.
    RespondInvite { invite: Box<Invitation>, status: Partstat, in_reply_to: Option<String> },
    /// События календаря в `[from, to)` (Unix-секунды), повторения разворачивает сервер → `EventsLoaded`.
    Events { from: i64, to: i64 },
    /// Занятость адресов и моя в `[from, to)` → `Availability`.
    Availability { ticket: u64, emails: Vec<String>, from: i64, to: i64 },
    /// Создать встречу и разослать приглашения → `MeetingCreated` или `MeetingFailed`.
    CreateMeeting { meeting: Box<Meeting>, ticket: u64 },
    /// Открыть коллеге (адрес) только мою занятость → `FreeBusyShared` или `CalendarFailed`.
    ShareFreeBusy { email: String },
    /// Настройки ящика на сервере: автоответ, квота (`settings.rs`) → `Event::Settings`.
    Settings(settings::Request),
    /// Перечитать личные контакты и справочник → `ContactsLoaded`, `DirectoryLoaded` (и `Contacts` для подсказок).
    Contacts,
    /// Создать (`old: None`) или изменить карточку → `ContactSaved` или `ContactFailed`, затем `ContactsLoaded`.
    SaveContact { card: Box<ContactCard>, old: Option<Box<ContactCard>>, ticket: u64 },
    /// Удалить карточку → `ContactDeleted` или `ContactFailed`, затем `ContactsLoaded`.
    DeleteContact { id: String, ticket: u64 },
    /// Таблица категорий аккаунта на сервере (`categories.rs`, S5-02б) → `Event::Categories`.
    Categories(categories::Request),
    /// Жизненный цикл (S5-04, `lifecycle.rs`): правки писем (ключи кэша; папки — ключи кэша или
    /// [`lifecycle::ARCHIVE`] — «Архив» аккаунта, нет — создаётся с именем `archive_name`) — один `Email/set` на
    /// аккаунт. UI уже поменял кэш; письма, которые сервер не изменил, — с сервера заново.
    Patch { patches: Vec<lifecycle::Patch>, archive_name: String },
    /// Рассылки «Входящих» своего аккаунта: прочитанные раньше `cutoff` (Unix-секунды), кроме последней от каждого
    /// отправителя, — в «Архив» → `Event::Newsletters`.
    Newsletters { cutoff: i64, archive_name: String },
    /// «Сообщить о фишинге» (S5-08): письма `ids` (ключи кэша) — вложениями `message/rfc822` одним письмом на адрес
    /// `to` от своего адреса → `Event::Reported`. Без связи — в очередь, как правки писем.
    ReportPhishing { ids: Vec<String>, to: Address, subject: String, text: String, ticket: u64 },
    /// «Повторить сейчас» (S5-08): без связи — попытка сразу, не дожидаясь паузы; со связью — ничего.
    Reconnect,
    /// Задачи (S6-02, `tasks.rs`): загрузка, сверка, правки → `Event::Tasks`. Правка без связи — в кэше, уходит
    /// по возвращении связи (`tasks::Request::Flush`).
    Tasks(tasks::Request),
}

impl Command {
    /// Копия команды, которую можно повторить, когда связь вернётся (`link`): правки писем и сообщение о фишинге.
    /// Остальное (тела, поиск, отправка с окном письма) без связи не ждёт — интерфейс показывает ошибку сам.
    pub fn replayable(&self) -> Option<Command> {
        Some(match self {
            Command::Keyword { id, keyword, on } => Command::Keyword { id: id.clone(), keyword, on: *on },
            Command::SetKeywords { ids, add, remove } => {
                Command::SetKeywords { ids: ids.clone(), add: add.clone(), remove: remove.clone() }
            }
            Command::Move { id, from, to } => Command::Move { id: id.clone(), from: from.clone(), to: to.clone() },
            Command::Destroy(id) => Command::Destroy(id.clone()),
            Command::Patch { patches, archive_name } => {
                Command::Patch { patches: patches.clone(), archive_name: archive_name.clone() }
            }
            Command::Tasks(r) => Command::Tasks(r.replayable()?),
            Command::ReportPhishing { ids, to, subject, text, ticket } => Command::ReportPhishing {
                ids: ids.clone(),
                to: to.clone(),
                subject: subject.clone(),
                text: text.clone(),
                ticket: *ticket,
            },
            _ => return None,
        })
    }
}

/// Что делает синхронизация. Текст — у интерфейса (`sync.ftl`), здесь только код и данные.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connecting { url: String },
    SignedIn { user: String },
    /// Первая загрузка: папка `mailbox` (имя с сервера).
    Loading { mailbox: String },
    Synced,
}

/// Род ошибки: от него зависят текст и поведение интерфейса (`Auth` — снова строка входа).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Сервер отклонил вход (HTTP 401): пароль сменён или сессия отозвана. Синхронизация останавливается.
    Auth,
    /// Сервер недоступен: соединение, DNS, TLS, тайм-аут.
    Network,
    /// Сервер ответил ошибкой (HTTP 4xx/5xx, ошибка метода JMAP).
    Server,
    /// Ответ не тот, что ждали (разбор, нет capability, неверный адрес).
    Protocol,
    /// Локальная копия (кэш) не прочиталась или не записалась.
    Storage,
    /// Синхронизация не запустилась (поток, рантайм).
    Internal,
}

impl ErrorKind {
    pub fn of(e: &postman_jmap::Error) -> Self {
        use postman_jmap::Error as E;
        match e {
            E::Http(h) => match h.status().map(|s| s.as_u16()) {
                Some(401) => Self::Auth,
                Some(_) => Self::Server,
                None if h.is_decode() => Self::Protocol,
                None => Self::Network,
            },
            E::Method { .. } => Self::Server,
            E::Protocol(s) if s.starts_with(CACHE_ERROR) => Self::Storage,
            E::Url(_) | E::Json(_) | E::MissingCapability(_) | E::Protocol(_) => Self::Protocol,
        }
    }
}

/// На каком шаге синхронизации случилась ошибка (для журнала).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Start,
    Connect,
    Load,
    Refresh,
    Prefetch,
    Command,
    Restore,
}

/// Ошибка синхронизации: род, шаг и подробность. `detail` — текст ошибки как есть (может быть ответом сервера),
/// интерфейс показывает его только как подробность после своего текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncError {
    pub kind: ErrorKind,
    pub stage: Stage,
    pub detail: String,
}

impl SyncError {
    pub fn new(stage: Stage, e: &postman_jmap::Error) -> Self {
        Self { kind: ErrorKind::of(e), stage, detail: e.to_string() }
    }

    fn internal(detail: impl std::fmt::Display) -> Self {
        Self { kind: ErrorKind::Internal, stage: Stage::Start, detail: detail.to_string() }
    }
}

/// Для журнала (по-английски); интерфейсу — `kind` и `detail`.
impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} failed ({:?}): {}", self.stage, self.kind, self.detail)
    }
}

/// Для журнала и запасного интерфейса (по-английски).
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Connecting { url } => write!(f, "connecting to {url}"),
            Status::SignedIn { user } => write!(f, "signed in: {user}"),
            Status::Loading { mailbox } => write!(f, "loading {mailbox}"),
            Status::Synced => write!(f, "synced"),
        }
    }
}

/// Отправитель для окна письма: свой адрес или общий ящик (S4-06).
#[derive(Debug, Clone, PartialEq)]
pub struct Sender {
    /// Общий ящик, из которого уходит письмо (id аккаунта, он же префикс ключей кэша); `None` — свой.
    pub account: Option<String>,
    pub address: Address,
    pub signature: String,
}

pub enum Event {
    Status(Status),
    MailboxesChanged,
    EmailsChanged,
    BodyReady(String),
    /// Беседа загружена: ключ цепочки и ключи её писем по времени (старые первыми), тела — в кэше.
    ThreadReady { thread: String, ids: Vec<String> },
    BlobReady { blob_id: String, data: Arc<Vec<u8>> },
    /// Тела загружены в фоне — их текст можно добавить в поиск.
    BodiesStored(Vec<String>),
    /// Догрузка ящика (`LoadMore`) пришла: `added` писем; 0 — на сервере старше нет.
    LoadedMore { mailbox: String, added: usize },
    /// В календаре на сервере что-то изменилось (push, с задержкой на пачку) — открытую неделю перечитать.
    CalendarChanged,
    SearchResults { ticket: u64, ids: Vec<String>, total: u64 },
    /// Ответ на `Counts`: по числу на фильтр; `None` — запрос не удался.
    Counts { ticket: u64, counts: Option<Vec<u64>> },
    BlobFailed { blob_id: String, error: String },
    /// Учётная запись отправителя и подпись (из Identity).
    Me { address: Address, signature: String },
    /// Адреса, от которых можно отправить письмо (S4-06): свои Identity и Identity общих ящиков.
    Senders(Vec<Sender>),
    /// Личные контакты (имя, адрес) — для автодополнения.
    Contacts(Vec<(String, String)>),
    /// Письмо отправлено; `scheduled` — сервер держит его до срока (окно «Отменить» или «Отправить позже»).
    Sent { ticket: u64, scheduled: Option<Scheduled> },
    /// Отправка отменена, письмо — снова черновик в «Черновиках» (`email` — ключ кэша).
    Unsent { ticket: u64, email: String },
    /// Отмена не удалась: `too_late` — письмо уже ушло.
    UnsendFailed { ticket: u64, too_late: bool, error: String },
    /// Отправки, которые сервер держит, и сколько он позволяет задержать письмо (секунды, `send_later::max_delay`;
    /// 0 — отложенной отправки нет).
    Scheduled { list: Vec<Scheduled>, max_delay: u64 },
    DraftSaved { ticket: u64, id: String },
    /// Ошибка конкретной отправки/сохранения — показывается в окне письма.
    ComposeFailed { ticket: u64, error: String },
    InviteAnswered { uid: String, status: Partstat, delivery: Delivery },
    InviteFailed { uid: String, error: String },
    /// События календаря в `[from, to)` — без отменённых, по времени начала.
    EventsLoaded { from: i64, to: i64, events: Vec<postman_calendar::Event> },
    /// Занятость для помощника: моя и по адресам; `None` — нет данных (календарь не открыт мне, сотрудник
    /// не найден): это не «свободен». `error` — запрос не удался целиком.
    Availability { ticket: u64, me: Vec<Busy>, people: Vec<(String, Option<Vec<Busy>>)>, error: Option<String> },
    MeetingCreated { ticket: u64, uid: String },
    MeetingFailed { ticket: u64, error: String },
    FreeBusyShared { email: String },
    /// Ошибка календаря (неделя, открытие занятости) — тостом.
    CalendarFailed(String),
    /// Ответ по настройкам ящика (`settings.rs`).
    Settings(settings::Reply),
    /// Личные контакты целиком (раздел «Контакты»).
    ContactsLoaded(Vec<ContactCard>),
    /// Справочник: сотрудники, переговорные и ресурсы с адресами (раздел «Контакты», подсказки адресов).
    DirectoryLoaded(Vec<Principal>),
    ContactSaved { ticket: u64, id: String },
    ContactDeleted { ticket: u64, id: String },
    /// Ошибка контактов: `ticket` — сохранения или удаления, `0` — загрузки.
    ContactFailed { ticket: u64, error: String },
    /// Таблица категорий: прочитана при входе или по push, записана, снята с писем (`categories.rs`).
    Categories(categories::Reply),
    /// Задачи: из кэша и с сервера, правки (`tasks.rs`, S6-02).
    Tasks(tasks::Reply),
    /// Рассылки перенесены в «Архив» (`Command::Newsletters`): сколько.
    Newsletters { archived: usize },
    /// Связь с сервером (S5-08, `link`): пропала (повтор через N с, правок в очереди) или вернулась.
    Connection(Connection),
    /// Сообщение о фишинге (`Command::ReportPhishing`) отправлено (`error: None`) или не отправлено. Без связи
    /// события нет — команда ждёт в очереди.
    Reported { ticket: u64, error: Option<String> },
    Error(SyncError),
}

/// Поисковый индекс (если открылся); пишут в него и синхронизация, и UI.
pub type Index = Option<Arc<SearchIndex>>;

pub fn spawn(
    account: Account,
    cache: Cache,
    index: Index,
    events: mpsc::Sender<Event>,
    repaint: impl Fn() + Send + 'static,
) -> UnboundedSender<Command> {
    let (tx, rx) = unbounded_channel();
    let failed = events.clone();
    let started = std::thread::Builder::new()
        .name("sync".into())
        .spawn(move || {
            let notify = |e: Event| {
                let _ = events.send(e);
                repaint();
            };
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return notify(Event::Error(SyncError::internal(format!("tokio runtime: {e}")))),
            };
            rt.block_on(async {
                let mut rx = rx;
                let mut link = Link::default();
                // Сервер не ответил при входе или первой загрузке — без связи: повтор с паузой (S5-08). Отказ во входе
                // и прочие ошибки останавливают синхронизацию, как раньше.
                loop {
                    match run(account.clone(), cache.clone(), index.clone(), &mut rx, &mut link, &notify).await {
                        Ok(()) => return,
                        Err(e) if e.kind == ErrorKind::Network => {
                            notify(Event::Error(e));
                            let wait = link.failed();
                            notify(Event::Connection(link.connection(wait)));
                            if !wait_offline(&mut rx, &mut link, wait, &notify).await {
                                return;
                            }
                        }
                        Err(e) => return notify(Event::Error(e)),
                    }
                }
            });
        });
    // Ошибка видна в окне (как любая ошибка синхронизации); команды уходят в никуда, клиент работает с кэшем.
    if let Err(e) = started {
        let _ = failed.send(Event::Error(SyncError::internal(format!("sync thread: {e}"))));
    }
    tx
}

/// Без связи до входа: ждать паузу `wait` (или «Повторить сейчас»); правки писем — в очередь (уйдут после входа),
/// прочие команды без сервера выполнить нельзя. `false` — интерфейс закрыл канал команд: синхронизации конец.
async fn wait_offline(
    rx: &mut UnboundedReceiver<Command>,
    link: &mut Link<Command>,
    wait: Duration,
    notify: &impl Fn(Event),
) -> bool {
    let until = tokio::time::Instant::now() + wait;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(until) => return true,
            cmd = rx.recv() => match cmd {
                None => return false,
                Some(Command::Reconnect) => return true,
                Some(cmd) => {
                    if let Some(c) = cmd.replayable() {
                        link.queue(c);
                        let left = until.saturating_duration_since(tokio::time::Instant::now());
                        notify(Event::Connection(link.connection(left)));
                    }
                }
            },
        }
    }
}

/// Связь пропала (или повтор не удался): пауза до следующей попытки — интерфейсу и циклу.
fn went_down(link: &mut Link<Command>, notify: &impl Fn(Event)) -> tokio::time::Instant {
    let wait = link.failed();
    notify(Event::Connection(link.connection(wait)));
    tokio::time::Instant::now() + wait
}

/// Ждать срока повтора; срока нет — никогда.
async fn retry_at(at: Option<tokio::time::Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Где изменилась почта по событию push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MailChange {
    #[default]
    None,
    /// В этих аккаунтах (id сервера): свой и общие ящики (S4-06) сверяются по отдельности.
    In(BTreeSet<String>),
    /// Неизвестно где (событие не разобралось, связь восстановилась) — сверить все аккаунты.
    All,
}

impl MailChange {
    /// Сверить аккаунт `account`.
    pub fn touches(&self, account: &str) -> bool {
        match self {
            Self::None => false,
            Self::In(set) => set.contains(account),
            Self::All => true,
        }
    }

    fn or(self, o: Self) -> Self {
        match (self, o) {
            (Self::All, _) | (_, Self::All) => Self::All,
            (Self::None, x) | (x, Self::None) => x,
            (Self::In(mut a), Self::In(b)) => {
                a.extend(b);
                Self::In(a)
            }
        }
    }
}

/// Какие данные изменились по событию push (`state`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    pub mail: MailChange,
    pub calendar: bool,
    /// Файлы аккаунтов (FileNode): в них таблица категорий (S5-02б).
    pub files: MailChange,
}

impl Changed {
    const ALL: Self = Self { mail: MailChange::All, calendar: true, files: MailChange::All };

    fn or(self, o: Self) -> Self {
        Self { mail: self.mail.or(o.mail), calendar: self.calendar || o.calendar, files: self.files.or(o.files) }
    }

    /// Разбор `StateChange` (RFC 8620 §7.1): `{"@type":"StateChange","changed":{"<аккаунт>":{"Email":"…",…}}}`.
    /// Не разобралось — считаем, что изменилось всё (лишняя сверка дешевле пропущенного письма).
    pub fn of(data: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return Self::ALL };
        let Some(accounts) = v.get("changed").and_then(|c| c.as_object()) else { return Self::ALL };
        let mut mail = BTreeSet::new();
        let mut files = BTreeSet::new();
        let mut calendar = false;
        for (account, types) in accounts.iter().filter_map(|(a, t)| Some((a, t.as_object()?))) {
            for kind in types.keys() {
                match kind.as_str() {
                    "Email" | "Mailbox" | "Thread" | "EmailDelivery" => {
                        mail.insert(account.clone());
                    }
                    "CalendarEvent" | "Calendar" | "CalendarEventNotification" => calendar = true,
                    "FileNode" => {
                        files.insert(account.clone());
                    }
                    _ => {}
                }
            }
        }
        let set = |s: BTreeSet<String>| if s.is_empty() { MailChange::None } else { MailChange::In(s) };
        Self { mail: set(mail), calendar, files: set(files) }
    }
}

/// Задержка перед перечитыванием календаря: первое изменение пачки ставит срок, следующие до срока — к нему же.
#[derive(Debug, Default)]
pub(crate) struct Debounce {
    due: Option<tokio::time::Instant>,
}

impl Debounce {
    fn poke(&mut self, now: tokio::time::Instant, delay: Duration) {
        self.due.get_or_insert(now + delay);
    }

    /// Срок наступил: сбросить и вернуть `true`.
    fn fire(&mut self, now: tokio::time::Instant) -> bool {
        let due = self.due.is_some_and(|d| d <= now);
        if due {
            self.due = None;
        }
        due
    }

    async fn wait(&self) {
        match self.due {
            Some(d) => tokio::time::sleep_until(d).await,
            None => std::future::pending().await,
        }
    }
}

/// Остановить синхронизацию: сервер отклонил вход — повторять запросы с тем же паролем бессмысленно
/// (и сервер может заблокировать адрес за перебор). Интерфейс показывает строку входа.
fn report(e: postman_jmap::Error, stage: Stage, notify: &impl Fn(Event)) -> bool {
    let e = SyncError::new(stage, &e);
    let stop = e.kind == ErrorKind::Auth;
    notify(Event::Error(e));
    stop
}

/// Почтовый аккаунт синхронизации: свой или общий ящик (S4-06). Письма и папки общего — в кэше с префиксом
/// аккаунта (`postman_cache::scoped`), команды интерфейса приходят с этими ключами.
struct MailAccount {
    /// id аккаунта на сервере.
    id: String,
    /// Префикс ключей кэша: `None` — свой аккаунт.
    scope: Option<String>,
    /// Почтовые методы — в этом аккаунте (`Client::in_account`).
    client: Client,
    /// Состояние `Email` — точка отсчёта `Email/changes`; `None` — первая загрузка ещё не удалась.
    state: Option<String>,
}

impl MailAccount {
    fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    fn key(&self, id: &str) -> String {
        scoped(self.scope(), id)
    }

    fn state_key(&self) -> String {
        self.key("Email")
    }
}

/// Свой аккаунт и общие ящики.
struct Accounts {
    own: MailAccount,
    shared: Vec<MailAccount>,
}

impl Accounts {
    fn all_mut(&mut self) -> impl Iterator<Item = &mut MailAccount> {
        std::iter::once(&mut self.own).chain(self.shared.iter_mut())
    }

    fn by_scope(&self, scope: Option<&str>) -> Option<&MailAccount> {
        std::iter::once(&self.own).chain(&self.shared).find(|a| a.scope() == scope)
    }

    /// Аккаунт и id сервера по ключу кэша.
    fn locate<'a>(&'a self, key: &'a str) -> postman_jmap::Result<(&'a MailAccount, &'a str)> {
        let (scope, id) = unscoped(key);
        self.by_scope(scope).map(|a| (a, id)).ok_or_else(|| postman_jmap::Error::Protocol(format!("no account for {key}")))
    }
}

/// Отправитель для синхронизации: Identity и аккаунт, из которого уходит письмо.
#[derive(Debug, Clone)]
struct SendAs {
    scope: Option<String>,
    identity: Identity,
}

/// Отправители по порядку: свои Identity, затем общих ящиков. Адрес, который есть и у своего аккаунта, и у общего
/// ящика (Stalwart заводит члену группы Identity с адресом группы), — из общего: копия ляжет в его «Отправленные»,
/// где её видят все, кто работает с ящиком.
fn senders(own: &[Identity], shared: &[(String, Vec<Identity>)]) -> Vec<SendAs> {
    let theirs = |email: &str| shared.iter().any(|(_, l)| l.iter().any(|i| i.email.eq_ignore_ascii_case(email)));
    let mut out: Vec<SendAs> =
        own.iter().filter(|i| !theirs(&i.email)).map(|i| SendAs { scope: None, identity: i.clone() }).collect();
    for (account, list) in shared {
        out.extend(list.iter().map(|i| SendAs { scope: Some(account.clone()), identity: i.clone() }));
    }
    out
}

fn account_row(a: &SharedAccount) -> AccountRow {
    AccountRow { id: a.id.clone(), name: a.name.clone(), is_personal: a.is_personal, read_only: a.is_read_only }
}

async fn run(
    account: Account,
    mut cache: Cache,
    index: Index,
    rx: &mut UnboundedReceiver<Command>,
    link: &mut Link<Command>,
    notify: &impl Fn(Event),
) -> Result<(), SyncError> {
    notify(Event::Status(Status::Connecting { url: account.url.clone() }));
    let client = Client::connect(&account.url, account.auth).await.map_err(|e| SyncError::new(Stage::Connect, &e))?;
    notify(Event::Status(Status::SignedIn { user: client.session().username.clone() }));
    let own_id = client.mail_account().map_err(|e| SyncError::new(Stage::Connect, &e))?.to_string();

    // Общие и делегированные ящики (S4-06) — аккаунты сессии, кроме своего.
    let shared = client.session().shared_accounts();
    cache
        .replace_accounts(&shared.iter().map(account_row).collect::<Vec<_>>())
        .map_err(|e| SyncError::new(Stage::Load, &db_err(e)))?;
    let identities = client.identities().await.unwrap_or_default();
    if let Some(i) = identities.first() {
        notify(Event::Me { address: Address::new(&i.name, &i.email), signature: i.text_signature.clone() });
    }
    let mut theirs = Vec::new();
    for a in shared.iter().filter(|a| a.can_submit && !a.is_read_only) {
        // Ящик открыт, но не мой (Stalwart: `forbidden`, «not an owner») — отправлять от него нельзя, это не ошибка.
        if let Ok(list) = client.in_account(&a.id).identities().await {
            theirs.push((a.id.clone(), list));
        }
    }
    let send_as = senders(&identities, &theirs);
    notify(Event::Senders(
        send_as
            .iter()
            .map(|f| Sender {
                account: f.scope.clone(),
                address: Address::new(&f.identity.name, &f.identity.email),
                signature: f.identity.text_signature.clone(),
            })
            .collect(),
    ));
    // Контакты и справочник — до писем: один-два запроса, нужны подсказкам адресов с первого окна письма.
    load_people(&client, notify).await;
    settings::handle(&client, settings::Request::Load, &|r| notify(Event::Settings(r))).await;
    // Таблицы категорий (свои и общих ящиков) — до писем: полосы и метки списка — сразу с ними.
    let shared_ids: Vec<String> = shared.iter().map(|a| a.id.clone()).collect();
    categories::load_all(&client, &own_id, &shared_ids, &|r| notify(Event::Categories(r))).await;
    // Задачи — из кэша сразу, с сервера — дельта; сбой не мешает почте (повтор — по push календаря).
    if let Err(e) = tasks::handle(&tasks::JmapStore::new(&client), &cache, tasks::Request::Load, &|r| notify(Event::Tasks(r))).await {
        tracing::warn!("tasks load failed: {e}");
    }

    let mail = |id: &str, scope: Option<String>, client: Client| MailAccount { id: id.into(), scope, client, state: None };
    let mut accounts = Accounts {
        own: mail(&own_id, None, client.clone()),
        shared: shared.iter().map(|a| mail(&a.id, Some(a.id.clone()), client.in_account(&a.id))).collect(),
    };
    accounts.own.state =
        Some(start_state(&accounts.own, &mut cache, &index, notify).await.map_err(|e| SyncError::new(Stage::Load, &e))?);
    for acc in &mut accounts.shared {
        match start_state(acc, &mut cache, &index, notify).await {
            Ok(s) => acc.state = Some(s),
            // Общий ящик не загрузился — свой работает; повтор — со следующей сверкой.
            Err(e) => {
                if report(e, Stage::Load, notify) {
                    return Ok(());
                }
            }
        }
    }
    notify(Event::Status(Status::Synced));
    notify_scheduled(&accounts, &send_as, notify).await;
    let mut poll = tokio::time::interval(POLL);
    poll.tick().await;
    let (push_tx, mut push_rx) = unbounded_channel::<Changed>();
    tokio::spawn(push_loop(client.clone(), push_tx));
    let mut calendar = Debounce::default();
    // Тела писем без тела — в фоне, свежие первыми: чтение без сети и полнотекстовый поиск.
    let mut prefetch: VecDeque<String> = cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default().into();
    let mut refill = false;
    // Без связи (S5-08): срок следующей попытки; правки писем без связи — в `link`.
    let mut retry: Option<tokio::time::Instant> = None;
    // Вход удался — связь есть: правки, сделанные без связи (в том числе до входа), — на сервер.
    if let Some(queued) = link.ok() {
        notify(Event::Connection(Connection::Online));
        let ctx = Ctx { accounts: &accounts, identities: &identities, send_as: &send_as };
        match replay(&ctx, &mut cache, &index, link, queued, notify).await {
            Replayed::Stop => return Ok(()),
            Replayed::Down(at) => retry = Some(at),
            Replayed::Done => {}
        }
    }
    loop {
        tokio::select! {
            // Команды пользователя важнее фоновой работы.
            biased;
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                if matches!(cmd, Command::Reconnect) {
                    if link.is_down() {
                        retry = Some(tokio::time::Instant::now());
                    }
                    continue;
                }
                // Без связи правки писем — сразу в очередь: каждая попытка ждала бы тайм-аута сети.
                if link.is_down()
                    && let Some(c) = cmd.replayable()
                {
                    link.queue(c);
                    let left = retry.map(|t| t.saturating_duration_since(tokio::time::Instant::now())).unwrap_or_default();
                    notify(Event::Connection(link.connection(left)));
                    continue;
                }
                // Догруженная страница — тоже в предзагрузку тел.
                let more = matches!(cmd, Command::LoadMore { .. });
                let again = cmd.replayable();
                let ctx = Ctx { accounts: &accounts, identities: &identities, send_as: &send_as };
                match handle(ctx.accounts, &mut cache, &index, ctx.identities, ctx.send_as, cmd, notify).await {
                    Ok(()) => {
                        if let Some(queued) = link.ok() {
                            notify(Event::Connection(Connection::Online));
                            retry = None;
                            match replay(&ctx, &mut cache, &index, link, queued, notify).await {
                                Replayed::Stop => return Ok(()),
                                Replayed::Down(at) => retry = Some(at),
                                Replayed::Done => {}
                            }
                        }
                    }
                    Err(e) => {
                        if ErrorKind::of(&e) == ErrorKind::Network {
                            if let Some(c) = again {
                                link.queue(c);
                            }
                            if !link.is_down() {
                                retry = Some(went_down(link, notify));
                            }
                        }
                        if report(e, Stage::Command, notify) {
                            return Ok(());
                        }
                    }
                }
                if more && prefetch.is_empty() {
                    prefetch.extend(cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default());
                }
            }
            () = retry_at(retry) => {
                retry = None;
                let mut outcome = Refreshed::Done;
                for acc in accounts.all_mut() {
                    outcome = outcome.max(apply_refresh(acc, &mut cache, &index, &mut prefetch, notify).await);
                    if outcome != Refreshed::Done {
                        break;
                    }
                }
                match after_refresh(outcome, true, &accounts, &identities, &send_as, &mut cache, &index, link, notify).await {
                    Next::Stop => return Ok(()),
                    Next::Retry(at) => retry = at,
                    Next::Keep => {}
                }
            }
            Some(first) = push_rx.recv() => {
                let mut changed = first;
                while let Ok(c) = push_rx.try_recv() {
                    changed = changed.or(c);
                }
                if changed.calendar {
                    calendar.poke(tokio::time::Instant::now(), CALENDAR_DEBOUNCE);
                }
                // Таблицу категорий поправил другой клиент (или эта же запись вернулась push) — перечитать.
                let files = std::iter::once(&accounts.own).chain(&accounts.shared);
                for acc in files.filter(|a| changed.files.touches(&a.id)) {
                    let request = categories::Request::Load { account: acc.scope.clone() };
                    categories::handle(&client, &own_id, None, request, &|r| notify(Event::Categories(r))).await;
                }
                let mut outcome = Refreshed::Done;
                for acc in accounts.all_mut().filter(|a| changed.mail.touches(&a.id)) {
                    outcome = outcome.max(apply_refresh(acc, &mut cache, &index, &mut prefetch, notify).await);
                    if outcome == Refreshed::Stop {
                        return Ok(());
                    }
                }
                match after_refresh(outcome, false, &accounts, &identities, &send_as, &mut cache, &index, link, notify).await {
                    Next::Stop => return Ok(()),
                    Next::Retry(at) => retry = at,
                    Next::Keep => {}
                }
            }
            () = calendar.wait() => {
                if calendar.fire(tokio::time::Instant::now()) {
                    // Задачи — те же записи календаря (`CalendarEvent`): сверить и их.
                    let store = tasks::JmapStore::new(&accounts.own.client);
                    if let Err(e) = tasks::handle(&store, &cache, tasks::Request::Refresh, &|r| notify(Event::Tasks(r))).await {
                        tracing::warn!("tasks refresh failed: {e}");
                    }
                    notify(Event::CalendarChanged);
                }
            }
            _ = poll.tick() => {
                let mut outcome = Refreshed::Done;
                for acc in accounts.all_mut() {
                    outcome = outcome.max(apply_refresh(acc, &mut cache, &index, &mut prefetch, notify).await);
                    if outcome == Refreshed::Stop {
                        return Ok(());
                    }
                }
                match after_refresh(outcome, false, &accounts, &identities, &send_as, &mut cache, &index, link, notify).await {
                    Next::Stop => return Ok(()),
                    Next::Retry(at) => retry = at,
                    Next::Keep => {}
                }
                // Предзагрузка прервалась ошибкой — остаток со следующего опроса. Полный обход кэша (а в большом
                // ящике это десятки тысяч строк) — только тогда, а не на каждое событие push.
                if std::mem::take(&mut refill) && prefetch.is_empty() {
                    prefetch.extend(cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default());
                }
            }
            // Без связи тела не тянем: каждая попытка — тайм-аут сети.
            _ = std::future::ready(()), if !prefetch.is_empty() && !link.is_down() => {
                let n = prefetch.len().min(PREFETCH_BATCH);
                let keys: Vec<String> = prefetch.drain(..n).collect();
                match prefetch_bodies(&accounts, &cache, &keys).await {
                    Ok(stored) => notify(Event::BodiesStored(stored)),
                    Err(e) => {
                        // Не долбим сервер: остаток подберёт следующий опрос.
                        prefetch.clear();
                        refill = true;
                        if ErrorKind::of(&e) == ErrorKind::Network && !link.is_down() {
                            retry = Some(went_down(link, notify));
                        }
                        if report(e, Stage::Prefetch, notify) {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// Что нужно командам писем помимо кэша: аккаунты и отправители (для `handle` и отправки очереди).
struct Ctx<'a> {
    accounts: &'a Accounts,
    identities: &'a [Identity],
    send_as: &'a [SendAs],
}

/// Итог отправки очереди: всё ушло, связь снова пропала (срок повтора) или синхронизацию остановить.
enum Replayed {
    Done,
    Down(tokio::time::Instant),
    Stop,
}

/// Связь вернулась: правки из очереди — на сервер по порядку. Сеть пропала посреди — остаток снова в очередь.
async fn replay(
    ctx: &Ctx<'_>,
    cache: &mut Cache,
    index: &Index,
    link: &mut Link<Command>,
    queued: Vec<Command>,
    notify: &impl Fn(Event),
) -> Replayed {
    let mut rest = queued.into_iter();
    while let Some(cmd) = rest.next() {
        let again = cmd.replayable();
        let Err(e) = handle(ctx.accounts, cache, index, ctx.identities, ctx.send_as, cmd, notify).await else { continue };
        if ErrorKind::of(&e) == ErrorKind::Network {
            let mut back: Vec<Command> = again.into_iter().collect();
            back.extend(rest);
            link.requeue(back);
            let at = went_down(link, notify);
            report(e, Stage::Command, notify);
            return Replayed::Down(at);
        }
        if report(e, Stage::Command, notify) {
            return Replayed::Stop;
        }
    }
    Replayed::Done
}

/// Что делать со сроком повтора после сверки.
enum Next {
    /// Срок прежний.
    Keep,
    /// Новый срок (или `None` — связь есть, повтора нет).
    Retry(Option<tokio::time::Instant>),
    Stop,
}

/// После сверки (push, опрос, повтор по сроку `timer`): сервер ответил — связь есть, очередь — на сервер; не
/// ответил — без связи (повтор по сроку не удался или связь только что пропала — следующий срок, длиннее).
#[allow(clippy::too_many_arguments)]
async fn after_refresh(
    outcome: Refreshed,
    timer: bool,
    accounts: &Accounts,
    identities: &[Identity],
    send_as: &[SendAs],
    cache: &mut Cache,
    index: &Index,
    link: &mut Link<Command>,
    notify: &impl Fn(Event),
) -> Next {
    match outcome {
        Refreshed::Stop => Next::Stop,
        Refreshed::Network if link.is_down() && !timer => Next::Keep,
        Refreshed::Network => Next::Retry(Some(went_down(link, notify))),
        Refreshed::Done => {
            // Почта сверена: задачи со ссылкой на письмо — по письмам (срок снят — выполнена, S6-03).
            let store = tasks::JmapStore::new(&accounts.own.client);
            if let Err(e) = tasks::handle(&store, cache, tasks::Request::Reconcile, &|r| notify(Event::Tasks(r))).await {
                tracing::warn!("mail tasks reconcile failed: {e}");
            }
            let Some(queued) = link.ok() else { return Next::Keep };
            notify(Event::Connection(Connection::Online));
            let ctx = Ctx { accounts, identities, send_as };
            match replay(&ctx, cache, index, link, queued, notify).await {
                Replayed::Down(at) => Next::Retry(Some(at)),
                Replayed::Done => Next::Retry(None),
                Replayed::Stop => Next::Stop,
            }
        }
    }
}

/// Тела писем по ключам кэша — запросом на аккаунт; ключи аккаунтов, которых больше нет, пропускаются.
async fn prefetch_bodies(accounts: &Accounts, cache: &Cache, keys: &[String]) -> postman_jmap::Result<Vec<String>> {
    let mut by_account: BTreeMap<Option<&str>, Vec<String>> = BTreeMap::new();
    for key in keys {
        let (scope, id) = unscoped(key);
        by_account.entry(scope).or_default().push(id.to_owned());
    }
    let mut stored = Vec::new();
    for (scope, ids) in by_account {
        let Some(acc) = accounts.by_scope(scope) else { continue };
        for (id, b) in acc.client.email_bodies(&ids, PREFETCH_MAX_BYTES).await? {
            let key = acc.key(&id);
            if cache.put_body(&key, &body_row(b, acc.scope())).is_ok() {
                stored.push(key);
            }
        }
    }
    Ok(stored)
}

/// Итог сверки аккаунта; по порядку тяжести (`max` по аккаунтам).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Refreshed {
    Done,
    /// Сервер не ответил (сеть): без связи.
    Network,
    /// Сервер отклонил вход: синхронизацию остановить.
    Stop,
}

/// Ошибка сверки — в интерфейс; что дальше.
fn refresh_failed(e: postman_jmap::Error, notify: &impl Fn(Event)) -> Refreshed {
    let network = ErrorKind::of(&e) == ErrorKind::Network;
    if report(e, Stage::Refresh, notify) {
        Refreshed::Stop
    } else if network {
        Refreshed::Network
    } else {
        Refreshed::Done
    }
}

/// Сверка аккаунта с сервером.
async fn apply_refresh(
    acc: &mut MailAccount,
    cache: &mut Cache,
    index: &Index,
    prefetch: &mut VecDeque<String>,
    notify: &impl Fn(Event),
) -> Refreshed {
    // Первая загрузка общего ящика не удалась — снова целиком.
    let Some(since) = acc.state.clone() else {
        return match start_state(acc, cache, index, notify).await {
            Ok(s) => {
                acc.state = Some(s);
                notify(Event::MailboxesChanged);
                notify(Event::EmailsChanged);
                Refreshed::Done
            }
            Err(e) => refresh_failed(e, notify),
        };
    };
    match refresh(acc, cache, index, &since, notify).await {
        Ok(Some((s, created))) => {
            acc.state = Some(s);
            // Новые письма — в начало очереди предзагрузки.
            for id in created.into_iter().rev() {
                prefetch.push_front(id);
            }
            notify(Event::MailboxesChanged);
            notify(Event::EmailsChanged);
            // Квота — своего ящика (строка состояния); у общего — своя, её не показываем.
            if acc.scope.is_none() {
                settings::handle(&acc.client, settings::Request::Quota, &|r| notify(Event::Settings(r))).await;
            }
            Refreshed::Done
        }
        Ok(None) => Refreshed::Done,
        Err(e) => refresh_failed(e, notify),
    }
}

/// Держит EventSource и будит основной цикл на каждое событие `state` (с тем, что изменилось).
async fn push_loop(client: Client, wake: UnboundedSender<Changed>) {
    let mut backoff = 5;
    // Первое подключение — сразу после загрузки: календарь ещё не открывали, сверять только почту.
    let mut first = true;
    let mail = || Changed { mail: MailChange::All, calendar: false, files: MailChange::None };
    loop {
        match client.event_source(PUSH_PING).await {
            Ok(mut es) => {
                backoff = 5;
                // Сразу после подключения — сверка: изменения могли случиться, пока связи не было.
                let missed = if first { mail() } else { Changed::ALL };
                first = false;
                if wake.send(missed).is_err() {
                    return;
                }
                let silence = Duration::from_secs(u64::from(PUSH_PING) * 3);
                while let Ok(Ok(Some(ev))) = tokio::time::timeout(silence, es.next()).await {
                    if ev.event == "state" && wake.send(Changed::of(&ev.data)).is_err() {
                        return;
                    }
                }
            }
            // Отказ во входе: сверка почты покажет его основному циклу, тот остановит синхронизацию.
            Err(e) if ErrorKind::of(&e) == ErrorKind::Auth => {
                if wake.send(mail()).is_err() {
                    return;
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(300);
    }
}

/// Папки аккаунта — в кэш (ключи с префиксом аккаунта).
async fn load_mailboxes(acc: &MailAccount, cache: &Cache) -> postman_jmap::Result<Vec<Mailbox>> {
    let (mailboxes, _) = acc.client.mailboxes().await?;
    let rows: Vec<MailboxRow> = mailboxes.iter().map(|m| mailbox_row(m, acc.scope())).collect();
    cache.replace_mailboxes(acc.scope(), &rows).map_err(db_err)?;
    Ok(mailboxes)
}

/// Состояние почты аккаунта при запуске. Кэш уже синхронизирован (есть состояние `Email`) — папки и дельта с
/// прошлого раза; сервер дельту не считает (`cannotCalculateChanges`) или кэш пуст — первые страницы папок.
async fn start_state(
    acc: &MailAccount,
    cache: &mut Cache,
    index: &Index,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<String> {
    if let Some(since) = cache.state(&acc.state_key()).map_err(db_err)? {
        load_mailboxes(acc, cache).await?;
        notify(Event::MailboxesChanged);
        match refresh(acc, cache, index, &since, notify).await {
            Ok(changed) => {
                notify(Event::MailboxesChanged);
                notify(Event::EmailsChanged);
                return Ok(changed.map_or(since, |(s, _)| s));
            }
            Err(postman_jmap::Error::Method { kind, .. }) if kind == "cannotCalculateChanges" => {}
            Err(e) => return Err(e),
        }
    }
    initial_load(acc, cache, index, notify).await
}

async fn initial_load(
    acc: &MailAccount,
    cache: &mut Cache,
    index: &Index,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<String> {
    let mailboxes = load_mailboxes(acc, cache).await?;
    notify(Event::MailboxesChanged);

    // Входящие первыми — их пользователь видит сразу.
    let mut order: Vec<&Mailbox> = mailboxes.iter().collect();
    order.sort_by_key(|m| m.role.as_deref() != Some("inbox"));
    let mut state = String::new();
    for mb in order {
        notify(Event::Status(Status::Loading { mailbox: mb.name.clone() }));
        let page = acc.client.email_page(&mb.id, 0, INITIAL_PAGE).await?;
        save_summaries(cache, index, &page.emails, acc.scope(), notify)?;
        state = page.state;
        notify(Event::EmailsChanged);
    }
    // Ни одной папки (общий ящик без открытых мне папок) — состояние всё равно нужно для дельты.
    if state.is_empty() {
        state = acc.client.email_get(&[]).await?.1;
    }
    cache.set_state(&acc.state_key(), &state).map_err(db_err)?;
    Ok(state)
}

/// Дельта через `Email/changes`: `Some((новое состояние, ключи новых писем))`, если что-то изменилось.
async fn refresh(
    acc: &MailAccount,
    cache: &mut Cache,
    index: &Index,
    since: &str,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<Option<(String, Vec<String>)>> {
    let mut state = since.to_string();
    let mut created = Vec::new();
    loop {
        let ch = acc.client.changes("Email", &state, 500).await?;
        if ch.new_state == state {
            return Ok(None);
        }
        created.extend(ch.created.iter().map(|id| acc.key(id)));
        let ids: Vec<String> = ch.created.into_iter().chain(ch.updated).collect();
        if !ids.is_empty() {
            let (emails, _) = acc.client.email_get(&ids).await?;
            save_summaries(cache, index, &emails, acc.scope(), notify)?;
        }
        let destroyed: Vec<String> = ch.destroyed.iter().map(|id| acc.key(id)).collect();
        cache.delete_emails(&destroyed).map_err(db_err)?;
        if let Some(i) = index {
            let _ = i.delete(&destroyed);
            let _ = i.commit();
        }
        state = ch.new_state;
        if !ch.has_more_changes {
            break;
        }
    }
    load_mailboxes(acc, cache).await?;
    cache.set_state(&acc.state_key(), &state).map_err(db_err)?;
    Ok(Some((state, created)))
}

async fn handle(
    accounts: &Accounts,
    cache: &mut Cache,
    index: &Index,
    identities: &[Identity],
    send_as: &[SendAs],
    cmd: Command,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<()> {
    let client = &accounts.own.client;
    match cmd {
        Command::Body(key) => {
            let (acc, id) = accounts.locate(&key)?;
            let row = body_row(acc.client.email_body(id).await?, acc.scope());
            cache.put_body(&key, &row).map_err(db_err)?;
            notify(Event::BodyReady(key));
        }
        Command::Thread(key) => {
            let (acc, id) = accounts.locate(&key)?;
            let ids = load_thread(acc, cache, index, id, notify).await?;
            notify(Event::ThreadReady { thread: key, ids });
        }
        Command::Keyword { id: key, keyword, on } => {
            let (acc, id) = accounts.locate(&key)?;
            acc.client.set_keyword(id, keyword, on).await?;
        }
        Command::SetKeywords { ids, add, remove } => {
            let mut first_error = None;
            for (scope, keys) in by_account(&ids) {
                let Some(acc) = accounts.by_scope(scope) else { continue };
                let server: Vec<String> = keys.iter().map(|k| unscoped(k).1.to_owned()).collect();
                let failed: Vec<String> = match acc.client.set_keywords(&server, &server_words(&add), &server_words(&remove)).await {
                    Ok(f) => f
                        .into_iter()
                        .map(|(id, e)| {
                            first_error.get_or_insert(e);
                            acc.key(&id)
                        })
                        .collect(),
                    Err(e) => {
                        first_error.get_or_insert(e);
                        keys.clone()
                    }
                };
                // Сервер не поставил метки — в кэш письма такими, какие они на сервере.
                for key in &failed {
                    restore(accounts, cache, index, key, notify).await;
                }
            }
            if let Some(e) = first_error {
                return Err(e);
            }
        }
        Command::Query { filter, mailbox, ticket } => {
            let (acc, mb) = query_scope(accounts, mailbox.as_deref())?;
            let page = acc.client.filtered(&in_mailbox(mb, filter), SEARCH_LIMIT).await?;
            save_summaries(cache, index, &page.emails, acc.scope(), notify)?;
            let ids = page.ids.iter().map(|id| acc.key(id)).collect();
            notify(Event::SearchResults { ticket, ids, total: page.total });
        }
        Command::Counts { filters, mailbox, ticket } => {
            let counted = match query_scope(accounts, mailbox.as_deref()) {
                Ok((acc, mb)) => {
                    let filters: Vec<Filter> = filters.into_iter().map(|f| in_mailbox(mb, f)).collect();
                    acc.client.count(&filters).await
                }
                Err(e) => Err(e),
            };
            notify(Event::Counts { ticket, counts: counted.as_ref().ok().cloned() });
            counted?;
        }
        Command::Move { id: key, from, to } => {
            let moved = match accounts.locate(&key) {
                Ok((acc, id)) => acc.client.move_email(id, unscoped(&from).1, unscoped(&to).1).await,
                Err(e) => Err(e),
            };
            if let Err(e) = moved {
                restore(accounts, cache, index, &key, notify).await;
                return Err(e);
            }
        }
        Command::Destroy(key) => {
            let destroyed = match accounts.locate(&key) {
                Ok((acc, id)) => acc.client.destroy_email(id).await,
                Err(e) => Err(e),
            };
            if let Err(e) = destroyed {
                restore(accounts, cache, index, &key, notify).await;
                return Err(e);
            }
            if let Some(i) = index {
                let _ = i.delete(std::slice::from_ref(&key));
                let _ = i.commit();
            }
        }
        Command::Send { draft, replace, ticket, release } => {
            let r = send(accounts, cache, send_as, &draft, replace.as_deref(), release).await;
            let later = matches!((&r, release), (Ok(Some(_)), Release::At(_)));
            notify(match r {
                Ok(scheduled) => Event::Sent { ticket, scheduled },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
            // Запланированное — в список отложенных («Отправится в …»); окно «Отменить» закроется само.
            if later {
                notify_scheduled(accounts, send_as, notify).await;
            }
        }
        Command::Unsend { scheduled, ticket } => {
            notify(match unsend(accounts, cache, &scheduled).await {
                Ok(()) => Event::Unsent { ticket, email: scheduled.email.clone() },
                Err(UnsendError::TooLate) => Event::UnsendFailed { ticket, too_late: true, error: String::new() },
                Err(UnsendError::Jmap(e)) => Event::UnsendFailed { ticket, too_late: false, error: e.to_string() },
            });
            notify_scheduled(accounts, send_as, notify).await;
        }
        Command::Scheduled => notify_scheduled(accounts, send_as, notify).await,
        Command::LoadMore { mailbox } => {
            // Ответ — всегда: интерфейс ждёт его, прежде чем просить следующую страницу.
            let added = match accounts.locate(&mailbox) {
                Ok((acc, _)) => load_more(acc, cache, index, &mailbox, notify).await,
                Err(e) => Err(e),
            };
            notify(Event::LoadedMore { mailbox, added: *added.as_ref().unwrap_or(&0) });
            notify(Event::EmailsChanged);
            added?;
        }
        Command::Search { query, mailbox, ticket } => {
            // В папке — в её аккаунте; во всех папках — в своём (общие ящики ищутся по своему кэшу).
            let (acc, mb) = match &mailbox {
                Some(key) => {
                    let (acc, id) = accounts.locate(key)?;
                    (acc, Some(id))
                }
                None => (&accounts.own, None),
            };
            let page = acc.client.search(&query, mb, SEARCH_LIMIT).await?;
            save_summaries(cache, index, &page.emails, acc.scope(), notify)?;
            let ids = page.ids.iter().map(|id| acc.key(id)).collect();
            notify(Event::SearchResults { ticket, ids, total: page.total });
        }
        Command::Blob { blob_id, name, mime } => {
            let (acc, id) = accounts.locate(&blob_id)?;
            match acc.client.download(&acc.id, id, &name, &mime).await {
                Ok(data) => {
                    cache.put_blob(&blob_id, &data).map_err(db_err)?;
                    notify(Event::BlobReady { blob_id, data: Arc::new(data) });
                }
                Err(e) => notify(Event::BlobFailed { blob_id, error: e.to_string() }),
            }
        }
        Command::RespondInvite { invite, status, in_reply_to } => {
            let uid = invite.event.uid.clone();
            notify(match respond_invite(accounts, cache, identities, send_as, &invite, status, in_reply_to).await {
                Ok(delivery) => Event::InviteAnswered { uid, status, delivery },
                Err(e) => Event::InviteFailed { uid, error: e.to_string() },
            });
        }
        Command::Events { from, to } => {
            notify(match schedule::week(client, from, to).await {
                Ok(events) => Event::EventsLoaded { from, to, events },
                Err(e) => Event::CalendarFailed(e.to_string()),
            });
        }
        Command::Availability { ticket, emails, from, to } => {
            notify(match schedule::availability(client, &emails, from, to).await {
                Ok((me, people)) => Event::Availability { ticket, me, people, error: None },
                Err(e) => Event::Availability {
                    ticket,
                    me: Vec::new(),
                    people: emails.into_iter().map(|e| (e, None)).collect(),
                    error: Some(e.to_string()),
                },
            });
        }
        Command::CreateMeeting { meeting, ticket } => {
            notify(match schedule::create(client, identities, &meeting).await {
                Ok(_) => Event::MeetingCreated { ticket, uid: meeting.uid.clone() },
                Err(e) => Event::MeetingFailed { ticket, error: e.to_string() },
            });
        }
        Command::ShareFreeBusy { email } => {
            notify(match schedule::share_free_busy(client, &email).await {
                Ok(()) => Event::FreeBusyShared { email },
                Err(e) => Event::CalendarFailed(e.to_string()),
            });
        }
        Command::Settings(request) => settings::handle(client, request, &|r| notify(Event::Settings(r))).await,
        Command::Categories(request) => {
            let notify = |r| notify(Event::Categories(r));
            categories::handle(client, &accounts.own.id, Some(&*cache), request, &notify).await;
        }
        Command::Tasks(request) => {
            tasks::handle(&tasks::JmapStore::new(client), cache, request, &|r| notify(Event::Tasks(r))).await?;
        }
        Command::Contacts => load_people(client, notify).await,
        Command::SaveContact { card, old, ticket } => {
            let saved = match &old {
                Some(old) => client.update_contact(old, &card).await.map(|()| old.id.clone()),
                None => client.create_contact(&card).await,
            };
            notify(match saved {
                Ok(id) => Event::ContactSaved { ticket, id },
                Err(e) => Event::ContactFailed { ticket, error: e.to_string() },
            });
            load_cards(client, notify).await;
        }
        Command::DeleteContact { id, ticket } => {
            notify(match client.destroy_contact(&id).await {
                Ok(()) => Event::ContactDeleted { ticket, id },
                Err(e) => Event::ContactFailed { ticket, error: e.to_string() },
            });
            load_cards(client, notify).await;
        }
        Command::Patch { patches, archive_name } => patch(accounts, cache, index, patches, &archive_name, notify).await?,
        Command::Newsletters { cutoff, archive_name } => {
            let archived = newsletters(&accounts.own, cache, cutoff, &archive_name, notify).await;
            notify(Event::Newsletters { archived: *archived.as_ref().unwrap_or(&0) });
            if archived.as_ref().is_ok_and(|n| *n > 0) {
                notify(Event::EmailsChanged);
            }
            archived?;
        }
        Command::SaveDraft { draft, replace, ticket } => {
            let r = save_draft(accounts, cache, send_as, &draft, replace.as_deref()).await;
            notify(match r {
                Ok(id) => Event::DraftSaved { ticket, id },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
        }
        Command::ReportPhishing { ids, to, subject, text, ticket } => {
            let r = report_phishing(accounts, cache, send_as, &ids, to, subject, text).await;
            match &r {
                Ok(()) => notify(Event::Reported { ticket, error: None }),
                // Без связи — ждёт в очереди (`link`), события нет.
                Err(e) if ErrorKind::of(e) == ErrorKind::Network => {}
                Err(e) => notify(Event::Reported { ticket, error: Some(e.to_string()) }),
            }
            r?;
        }
        // «Повторить сейчас» — забота цикла синхронизации.
        Command::Reconnect => {}
    }
    Ok(())
}

/// Имя вложения `.eml` из темы письма: без знаков, которые Windows не пускает в имя файла; пустая — «message».
fn eml_name(subject: Option<&str>) -> String {
    let clean: String = subject
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .take(80)
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    format!("{}.eml", if clean.is_empty() { "message" } else { clean })
}

/// `Command::ReportPhishing`: письма — вложениями `message/rfc822` (байты скачивает отправка по `blobId`) одним
/// письмом на `to` от своего адреса, сразу (без окна «Отменить»).
async fn report_phishing(
    accounts: &Accounts,
    cache: &Cache,
    send_as: &[SendAs],
    keys: &[String],
    to: Address,
    subject: String,
    text: String,
) -> postman_jmap::Result<()> {
    let me = send_as
        .iter()
        .find(|s| s.scope.is_none())
        .or(send_as.first())
        .ok_or_else(|| postman_jmap::Error::Protocol("no Identity on the server to send from".into()))?;
    let mut draft = Draft {
        from: Address::new(&me.identity.name, &me.identity.email),
        to: vec![to],
        subject,
        markdown: text,
        ..Default::default()
    };
    for (scope, keys) in by_account(keys) {
        let Some(acc) = accounts.by_scope(scope) else { continue };
        let ids: Vec<String> = keys.iter().map(|k| unscoped(k).1.to_owned()).collect();
        for e in acc.client.email_blobs(&ids).await? {
            draft.attachments.push(postman_compose::Attachment {
                name: eml_name(e.subject.as_deref()),
                mime: "message/rfc822".into(),
                data: Vec::new(),
                inline_cid: None,
                blob_id: Some(acc.key(&e.blob_id)),
            });
        }
    }
    if draft.attachments.is_empty() {
        return Err(postman_jmap::Error::Protocol("the reported messages are gone from the server".into()));
    }
    send(accounts, cache, send_as, &draft, None, Release::Now).await?;
    Ok(())
}

/// «Архив» аккаунта (id на сервере): из кэша папок; нет — создать папку с ролью `archive` (у Stalwart по умолчанию
/// её нет, `docs/STALWART.md`) и перечитать папки.
async fn ensure_archive(acc: &MailAccount, cache: &Cache, name: &str, notify: &impl Fn(Event)) -> postman_jmap::Result<String> {
    let known = cache
        .mailboxes()
        .map_err(db_err)?
        .into_iter()
        .find(|m| m.role.as_deref() == Some("archive") && postman_cache::account_of(&m.id) == acc.scope());
    if let Some(m) = known {
        return Ok(unscoped(&m.id).1.to_owned());
    }
    let id = acc.client.create_mailbox(name, Some("archive")).await?;
    load_mailboxes(acc, cache).await?;
    notify(Event::MailboxesChanged);
    Ok(id)
}

/// Правка жизненного цикла для сервера: id и папки — без префикса аккаунта; [`lifecycle::ARCHIVE`] — `archive`.
fn server_patch(p: &lifecycle::Patch, archive: Option<&str>) -> (String, postman_jmap::EmailPatch) {
    let mut out = postman_jmap::EmailPatch::default();
    for (m, on) in &p.mailboxes {
        let id = if m == lifecycle::ARCHIVE { archive } else { Some(unscoped(m).1) };
        if let Some(id) = id {
            out = out.mailbox(id, *on);
        }
    }
    for (k, on) in p.keywords.iter().filter(|(k, _)| !k.eq_ignore_ascii_case(postman_cache::ENCRYPTED)) {
        out = out.keyword(k, *on);
    }
    (unscoped(&p.id).1.to_owned(), out)
}

/// `Command::Patch`: по аккаунтам, одним `Email/set` (по 500); не принятые — с сервера заново.
async fn patch(
    accounts: &Accounts,
    cache: &mut Cache,
    index: &Index,
    patches: Vec<lifecycle::Patch>,
    archive_name: &str,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<()> {
    let mut first_error = None;
    let mut by: BTreeMap<Option<String>, Vec<lifecycle::Patch>> = BTreeMap::new();
    for p in patches {
        by.entry(postman_cache::account_of(&p.id).map(str::to_owned)).or_default().push(p);
    }
    for (scope, list) in by {
        let Some(acc) = accounts.by_scope(scope.as_deref()) else { continue };
        let wants_archive = list.iter().any(|p| p.mailboxes.iter().any(|(m, _)| m == lifecycle::ARCHIVE));
        let archive = if wants_archive {
            match ensure_archive(acc, cache, archive_name, notify).await {
                Ok(id) => Some(id),
                Err(e) => {
                    first_error.get_or_insert(e);
                    None
                }
            }
        } else {
            None
        };
        let server: Vec<(String, postman_jmap::EmailPatch)> = list.iter().map(|p| server_patch(p, archive.as_deref())).collect();
        let failed: Vec<String> = match acc.client.patch_emails(&server).await {
            Ok(f) => f
                .into_iter()
                .map(|(id, e)| {
                    first_error.get_or_insert(e);
                    acc.key(&id)
                })
                .collect(),
            Err(e) => {
                first_error.get_or_insert(e);
                list.iter().map(|p| p.id.clone()).collect()
            }
        };
        // Папку «Архив» UI ещё не знал (в кэше письмо без неё) — письма такими, какие они теперь на сервере.
        let refetch: Vec<String> = if wants_archive { list.iter().map(|p| p.id.clone()).collect() } else { failed };
        for key in &refetch {
            restore(accounts, cache, index, key, notify).await;
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `Command::Newsletters`: письма «Входящих» с `List-Id` или `List-Unsubscribe` (сырые заголовки — у сервера) →
/// [`lifecycle::newsletter_sweep`] → в «Архив». Ответ — сколько перенесено.
async fn newsletters(
    acc: &MailAccount,
    cache: &mut Cache,
    cutoff: i64,
    archive_name: &str,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<usize> {
    let mailboxes = cache.mailboxes().map_err(db_err)?;
    let roles = lifecycle::Roles::of(&mailboxes, acc.scope());
    let Some(inbox_key) = roles.inbox else { return Ok(0) };
    let inbox = unscoped(&inbox_key).1.to_owned();
    let seen = || vec![Filter::InMailbox(inbox.clone()), Filter::HasKeyword(postman_cache::SEEN.into())];
    // Свежие — чтобы знать последнюю от отправителя; старые — чтобы дойти до них в большом ящике.
    let mut letters = acc.client.list_letters(&Filter::And(seen()), lifecycle::NEWS_LIMIT).await?;
    let mut old = seen();
    old.push(Filter::Before(postman_jmap::unix_to_utc(cutoff)));
    for l in acc.client.list_letters(&Filter::And(old), lifecycle::NEWS_LIMIT).await? {
        if !letters.iter().any(|x| x.id == l.id) {
            letters.push(l);
        }
    }
    let items: Vec<lifecycle::NewsItem> = letters
        .into_iter()
        .filter(|l| l.list)
        .map(|l| lifecycle::NewsItem { id: l.id, sender: l.from, received_at: l.received_at, seen: l.seen })
        .collect();
    let ids = lifecycle::newsletter_sweep(&items, cutoff);
    if ids.is_empty() {
        return Ok(0);
    }
    let archive = ensure_archive(acc, cache, archive_name, notify).await?;
    let patches: Vec<(String, postman_jmap::EmailPatch)> = ids
        .iter()
        .map(|id| (id.clone(), postman_jmap::EmailPatch::default().mailbox(&inbox, false).mailbox(&archive, true)))
        .collect();
    let failed = acc.client.patch_emails(&patches).await?;
    let moved: Vec<lifecycle::Patch> = ids
        .iter()
        .filter(|id| !failed.iter().any(|(f, _)| f == *id))
        .map(|id| lifecycle::Patch {
            id: acc.key(id),
            keywords: Vec::new(),
            mailboxes: vec![(inbox_key.clone(), false), (acc.key(&archive), true)],
        })
        .collect();
    cache.patch_emails(&moved).map_err(db_err)?;
    Ok(moved.len())
}

/// Ключи кэша по аккаунтам (`None` — свой); порядок внутри аккаунта — как в `keys`.
fn by_account(keys: &[String]) -> BTreeMap<Option<&str>, Vec<String>> {
    let mut out: BTreeMap<Option<&str>, Vec<String>> = BTreeMap::new();
    for key in keys {
        out.entry(postman_cache::account_of(key)).or_default().push(key.clone());
    }
    out
}

/// Где искать на сервере: аккаунт папки `mailbox` (ключ кэша) и её id; без папки — все папки своего аккаунта
/// (общие ящики в «Все папки» — по своему кэшу, как поиск).
fn query_scope<'a>(accounts: &'a Accounts, mailbox: Option<&'a str>) -> postman_jmap::Result<(&'a MailAccount, Option<&'a str>)> {
    match mailbox {
        Some(key) => accounts.locate(key).map(|(acc, id)| (acc, Some(id))),
        None => Ok((&accounts.own, None)),
    }
}

/// Фильтр в папке (`inMailbox` вместе с ним) или как есть.
fn in_mailbox(mailbox: Option<&str>, filter: Filter) -> Filter {
    match mailbox {
        Some(id) => Filter::And(vec![Filter::InMailbox(id.to_owned()), filter]),
        None => filter,
    }
}

/// Беседа с сервера: `Thread/get` и краткие записи писем (из всех папок, в том числе «Отправленных» за пределами
/// первой страницы), затем тела, которых нет в кэше. Ключи писем — по времени. Новые письма в беседе потом приходят
/// обычной дельтой `Email/changes` (в краткой записи есть `threadId`), `Thread/changes` не нужен.
async fn load_thread(
    acc: &MailAccount,
    cache: &mut Cache,
    index: &Index,
    thread: &str,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<Vec<String>> {
    let (ids, emails) = acc.client.thread(thread).await?;
    let keys: Vec<String> = ids.iter().map(|id| acc.key(id)).collect();
    let known = cache.rows_by_ids(&keys).map_err(db_err)?.len();
    save_summaries(cache, index, &emails, acc.scope(), notify)?;
    if known < emails.len() {
        notify(Event::EmailsChanged);
    }
    let mut missing = Vec::new();
    for (id, key) in ids.iter().zip(&keys) {
        if cache.body(key).map_err(db_err)?.is_none() {
            missing.push(id.clone());
        }
    }
    if !missing.is_empty() {
        let mut stored = Vec::new();
        for (id, b) in acc.client.email_bodies(&missing, THREAD_BODY_BYTES).await? {
            let key = acc.key(&id);
            cache.put_body(&key, &body_row(b, acc.scope())).map_err(db_err)?;
            stored.push(key);
        }
        notify(Event::BodiesStored(stored));
    }
    Ok(keys)
}

/// Следующая страница папки (ключ `mailbox`) после самого старого письма в кэше; сколько писем пришло.
async fn load_more(
    acc: &MailAccount,
    cache: &mut Cache,
    index: &Index,
    mailbox: &str,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<usize> {
    let mb = unscoped(mailbox).1;
    let page = match cache.oldest(mailbox).map_err(db_err)? {
        Some(anchor) => acc.client.email_page_after(mb, unscoped(&anchor).1, PAGE).await,
        None => acc.client.email_page(mb, 0, PAGE).await,
    };
    // Якорь мог исчезнуть на сервере (удалён) — тогда по позиции.
    let page = match page {
        Err(postman_jmap::Error::Method { kind, .. }) if kind == "anchorNotFound" => {
            acc.client.email_page(mb, cache.count(mailbox).map_err(db_err)? as u64, PAGE).await?
        }
        p => p?,
    };
    let added = page.emails.len();
    save_summaries(cache, index, &page.emails, acc.scope(), notify)?;
    Ok(added)
}

/// Личные контакты и справочник; чего сервер не объявил — того нет (без ошибки).
async fn load_people(client: &Client, notify: &impl Fn(Event)) {
    load_cards(client, notify).await;
    if !client.has(postman_jmap::PRINCIPALS) {
        return notify(Event::DirectoryLoaded(Vec::new()));
    }
    match client.directory(DIRECTORY_LIMIT).await {
        Ok(people) => notify(Event::DirectoryLoaded(crate::contacts::directory(people))),
        Err(e) => notify(Event::ContactFailed { ticket: 0, error: e.to_string() }),
    }
}

/// Личные контакты — разделу целиком и подсказкам адресов парами (имя, адрес).
async fn load_cards(client: &Client, notify: &impl Fn(Event)) {
    if !client.has(postman_jmap::CONTACTS) {
        // Раздел «Контакты» не ждёт того, чего сервер не умеет.
        return notify(Event::ContactsLoaded(Vec::new()));
    }
    match client.contact_cards().await {
        Ok(cards) => {
            let pairs = cards.iter().flat_map(|c| c.emails.iter().map(|e| (c.name.clone(), e.clone()))).collect();
            notify(Event::Contacts(pairs));
            notify(Event::ContactsLoaded(cards));
        }
        Err(e) => notify(Event::ContactFailed { ticket: 0, error: e.to_string() }),
    }
}

/// Сервер не выполнил перенос или удаление: письмо — в кэш таким, какое оно на сервере (UI уже поменял кэш сам).
async fn restore(accounts: &Accounts, cache: &mut Cache, index: &Index, key: &str, notify: &impl Fn(Event)) {
    let restored = match accounts.locate(key) {
        Ok((acc, id)) => match acc.client.email_get(&[id.to_owned()]).await {
            Ok((emails, _)) => save_summaries(cache, index, &emails, acc.scope(), notify),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    match restored {
        Ok(()) => notify(Event::EmailsChanged),
        Err(e) => {
            report(e, Stage::Restore, notify);
        }
    }
}

/// id сервера папки с ролью `role` в аккаунте `scope`.
fn mailbox_by_role(cache: &Cache, scope: Option<&str>, role: &str) -> postman_jmap::Result<String> {
    cache
        .mailboxes()
        .map_err(db_err)?
        .into_iter()
        .find(|m| m.role.as_deref() == Some(role) && postman_cache::account_of(&m.id) == scope)
        .map(|m| unscoped(&m.id).1.to_owned())
        .ok_or_else(|| postman_jmap::Error::Protocol(format!("no mailbox with role {role}")))
}

fn raw_message(draft: &Draft) -> postman_jmap::Result<Vec<u8>> {
    let now = jiff::Timestamp::now().as_second();
    postman_compose::build(draft, now, &postman_compose::new_message_id(&draft.from, now))
        .map_err(|e| postman_jmap::Error::Protocol(format!("building the message: {e}")))
}

/// Догрузить вложения оригинала (пересылка), у которых есть только blob_id (ключ кэша: оригинал мог быть в
/// общем ящике, а уходит письмо из своего).
async fn fill_attachments(accounts: &Accounts, cache: &Cache, draft: &mut Draft) -> postman_jmap::Result<()> {
    for a in draft.attachments.iter_mut().filter(|a| a.data.is_empty()) {
        let Some(blob) = a.blob_id.clone() else { continue };
        a.data = match cache.blob(&blob).map_err(db_err)? {
            Some(d) => d,
            None => {
                let (acc, id) = accounts.locate(&blob)?;
                acc.client.download(&acc.id, id, &a.name, &a.mime).await?
            }
        };
    }
    Ok(())
}

/// Отправитель черновика: Identity с адресом «От» (свой аккаунт или общий ящик), иначе первая своя.
fn sender_of<'a>(send_as: &'a [SendAs], draft: &Draft) -> postman_jmap::Result<&'a SendAs> {
    send_as
        .iter()
        .find(|f| f.identity.email.eq_ignore_ascii_case(&draft.from.email))
        .or_else(|| send_as.iter().find(|f| f.scope.is_none()))
        .or(send_as.first())
        .ok_or_else(|| postman_jmap::Error::Protocol("no Identity on the server to send from".into()))
}

/// Отправить из аккаунта отправителя: черновик — в его «Черновики», копия — в его «Отправленные». Сервер держит
/// письмо до срока (`release`) — отправка, которую можно отменить.
async fn send(
    accounts: &Accounts,
    cache: &Cache,
    send_as: &[SendAs],
    draft: &Draft,
    replace: Option<&str>,
    release: Release,
) -> postman_jmap::Result<Option<Scheduled>> {
    let mut draft = draft.clone();
    fill_attachments(accounts, cache, &mut draft).await?;
    let draft = &draft;
    let sender = sender_of(send_as, draft)?;
    let acc = accounts
        .by_scope(sender.scope.as_deref())
        .ok_or_else(|| postman_jmap::Error::Protocol("the sender's account is gone".into()))?;
    let envelope = Envelope {
        mail_from: sender.identity.email.clone(),
        rcpt_to: draft.recipients().map(|a| a.email.clone()).collect(),
    };
    let (drafts, sent) = (mailbox_by_role(cache, acc.scope(), "drafts")?, mailbox_by_role(cache, acc.scope(), "sent")?);
    // Без FUTURERELEASE окна «Отменить» нет — письмо уходит сразу; «Отправить позже» — ошибка, а не отправка.
    let release = match release {
        Release::After(_) if send_later::max_delay(acc.client.max_delayed_send()) == 0 => Release::Now,
        Release::At(_) if send_later::max_delay(acc.client.max_delayed_send()) == 0 => {
            return Err(postman_jmap::Error::Protocol("the server does not support delayed sending".into()));
        }
        r => r,
    };
    let raw = raw_message(draft)?;
    let sub = acc.client.submit(raw, &sender.identity.id, &envelope, &drafts, &sent, release, &server_words(&draft.keywords)).await?;
    if let Some(old) = replace {
        let (acc, id) = accounts.locate(old)?;
        acc.client.destroy_email(id).await?;
    }
    Ok((sub.pending && release != Release::Now).then(|| Scheduled {
        submission: acc.key(&sub.submission_id),
        email: acc.key(&sub.email_id),
        send_at: sub.send_at.unwrap_or_else(|| jiff::Timestamp::now().as_second()),
    }))
}

/// Отменить отправку, которую держит сервер: письмо — обратно в «Черновики» аккаунта отправителя.
async fn unsend(accounts: &Accounts, cache: &Cache, s: &Scheduled) -> Result<(), UnsendError> {
    let (acc, submission) = accounts.locate(&s.submission)?;
    let email = unscoped(&s.email).1;
    let (drafts, sent) = (mailbox_by_role(cache, acc.scope(), "drafts")?, mailbox_by_role(cache, acc.scope(), "sent")?);
    acc.client.unsend(submission, email, &drafts, &sent).await
}

/// Отложенные отправки своего аккаунта и общих ящиков, от которых можно отправлять → `Event::Scheduled`.
async fn notify_scheduled(accounts: &Accounts, send_as: &[SendAs], notify: &impl Fn(Event)) {
    let mut list = Vec::new();
    let sending = std::iter::once(&accounts.own)
        .chain(accounts.shared.iter().filter(|a| send_as.iter().any(|f| f.scope.as_deref() == a.scope())));
    for acc in sending {
        // Не прочиталось — списка этого аккаунта нет; письма всё равно уйдут в срок (их держит сервер).
        let Ok(pending) = acc.client.pending_submissions().await else { continue };
        list.extend(pending.into_iter().filter_map(|p| {
            Some(Scheduled { send_at: p.send_at_unix()?, submission: acc.key(&p.id), email: acc.key(&p.email_id) })
        }));
    }
    list.sort_by_key(|s| s.send_at);
    notify(Event::Scheduled { list, max_delay: send_later::max_delay(accounts.own.client.max_delayed_send()) });
}

async fn respond_invite(
    accounts: &Accounts,
    cache: &Cache,
    identities: &[Identity],
    send_as: &[SendAs],
    inv: &Invitation,
    status: Partstat,
    in_reply_to: Option<String>,
) -> postman_jmap::Result<Delivery> {
    let client = &accounts.own.client;
    // Отвечаем от того адреса, на который пришло приглашение (их может быть несколько).
    let identity = identities
        .iter()
        .find(|i| inv.my_status(&i.email).is_some())
        .or(identities.first())
        .ok_or_else(|| postman_jmap::Error::Protocol("no Identity on the server to send from".into()))?;
    let me = Address::new(&identity.name, &identity.email);
    if invite::via_server(client, inv, &me.email, status).await?.is_some() {
        return Ok(Delivery::Server);
    }
    let now = jiff::Timestamp::now().as_second();
    let draft = invite::reply_draft(inv, &me, status, in_reply_to, now)
        .ok_or_else(|| postman_jmap::Error::Protocol("the invitation has no organizer".into()))?;
    send(accounts, cache, send_as, &draft, None, Release::Now).await?;
    // Ответ ушёл — дальше только своя копия; её сбой не повод слать ответ повторно.
    let imported = invite::import(client, inv, &me.email, status).await.unwrap_or(false);
    Ok(Delivery::Mail { imported })
}

/// Черновик — в «Черновики» аккаунта отправителя; id — ключ кэша.
async fn save_draft(
    accounts: &Accounts,
    cache: &Cache,
    send_as: &[SendAs],
    draft: &Draft,
    replace: Option<&str>,
) -> postman_jmap::Result<String> {
    let mut draft = draft.clone();
    fill_attachments(accounts, cache, &mut draft).await?;
    let draft = &draft;
    // Нет Identity (сервер не дал) — черновик всё равно в свой аккаунт.
    let scope = sender_of(send_as, draft).ok().and_then(|f| f.scope.clone());
    let acc = accounts.by_scope(scope.as_deref()).unwrap_or(&accounts.own);
    let drafts = mailbox_by_role(cache, acc.scope(), "drafts")?;
    // Прежняя версия — в том же аккаунте; отправитель сменился — прежнюю убрать отдельно.
    let (same, other): (Option<&str>, Option<&str>) = match replace {
        Some(old) if postman_cache::account_of(old) == acc.scope() => (Some(unscoped(old).1), None),
        Some(old) => (None, Some(old)),
        None => (None, None),
    };
    let id = acc.client.save_draft(raw_message(draft)?, &drafts, same, &server_words(&draft.keywords)).await?;
    if let Some(old) = other
        && let Ok((prev, old_id)) = accounts.locate(old)
    {
        prev.client.destroy_email(old_id).await?;
    }
    Ok(acc.key(&id))
}

/// Записать письма в кэш и в поисковый индекс. У писем с уже загруженным телом краткая
/// запись затёрла бы текст — их индексирует заново извлечение (событие `BodiesStored`).
fn save(cache: &mut Cache, index: &Index, rows: Vec<EmailRow>, notify: &impl Fn(Event)) -> postman_jmap::Result<()> {
    cache.upsert_emails(&rows).map_err(db_err)?;
    if let Some(i) = index {
        let (full, brief): (Vec<EmailRow>, Vec<EmailRow>) =
            rows.into_iter().partition(|r| cache.body(&r.id).ok().flatten().is_some());
        crate::indexer::rows(i, &brief);
        if !full.is_empty() {
            notify(Event::BodiesStored(full.into_iter().map(|r| r.id).collect()));
        }
    }
    Ok(())
}

/// Краткие записи писем с сервера — в кэш: строки и получатели (S5-03, быстрый фильтр «Мне»).
fn save_summaries(
    cache: &mut Cache,
    index: &Index,
    emails: &[EmailSummary],
    scope: Option<&str>,
    notify: &impl Fn(Event),
) -> postman_jmap::Result<()> {
    let recipients: Vec<(String, Recipients)> = emails.iter().map(|e| (scoped(scope, &e.id), recipients(e))).collect();
    save(cache, index, emails.iter().map(|e| email_row(e, scope)).collect(), notify)?;
    cache.set_recipients(&recipients).map_err(db_err)
}

/// Адреса To и Cc письма в нижнем регистре.
fn recipients(e: &EmailSummary) -> Recipients {
    let list = |a: &Option<Vec<EmailAddress>>| {
        a.iter().flatten().filter_map(|a| a.email.as_deref()).map(str::to_lowercase).collect::<Vec<_>>()
    };
    Recipients { to: list(&e.to), cc: list(&e.cc) }
}

pub(crate) fn db_err(e: postman_cache::Error) -> postman_jmap::Error {
    postman_jmap::Error::Protocol(format!("{CACHE_ERROR}{e}"))
}

/// Тело письма для кэша; id блобов вложений — ключи аккаунта `scope`.
fn body_row(b: EmailBody, scope: Option<&str>) -> BodyRow {
    BodyRow {
        to: addresses(&b.to),
        cc: addresses(&b.cc),
        reply_to: addresses(&b.reply_to),
        message_id: b.message_id,
        references: b.references,
        html: b.html,
        text: b.text,
        blob_id: b.blob_id.as_deref().map(|id| scoped(scope, id)),
        attachments: b
            .attachments
            .iter()
            .filter_map(|p| {
                Some(AttachmentRow {
                    blob_id: scoped(scope, p.blob_id.as_deref()?),
                    name: p.name.clone().unwrap_or_else(|| "вложение".into()),
                    mime: p.mime.clone(),
                    size: p.size,
                    cid: p.cid.as_deref().map(|c| c.trim_matches(['<', '>']).to_string()),
                    inline: p.disposition.as_deref() == Some("inline"),
                })
            })
            .collect(),
    }
}

fn addresses(list: &[EmailAddress]) -> Vec<Addr> {
    list.iter()
        .filter_map(|a| Some(Addr { name: a.name.clone().unwrap_or_default(), email: a.email.clone()? }))
        .collect()
}

fn rights(r: &MailboxRights) -> Rights {
    Rights {
        read: r.may_read_items,
        add: r.may_add_items,
        remove: r.may_remove_items,
        seen: r.may_set_seen,
        keywords: r.may_set_keywords,
        submit: r.may_submit,
    }
}

/// Папка для кэша; id и родитель — ключи аккаунта `scope`.
fn mailbox_row(m: &Mailbox, scope: Option<&str>) -> MailboxRow {
    MailboxRow {
        id: scoped(scope, &m.id),
        parent_id: m.parent_id.as_deref().map(|p| scoped(scope, p)),
        name: m.name.clone(),
        role: m.role.clone(),
        sort_order: m.sort_order,
        total: m.total_emails,
        unread: m.unread_emails,
        rights: rights(&m.my_rights),
    }
}

/// Письмо для кэша; id, цепочка и папки — ключи аккаунта `scope`.
fn email_row(e: &EmailSummary, scope: Option<&str>) -> EmailRow {
    let from = e.from.as_ref().and_then(|f| f.first());
    EmailRow {
        id: scoped(scope, &e.id),
        thread_id: scoped(scope, &e.thread_id),
        mailbox_ids: e.mailbox_ids.iter().filter(|(_, v)| **v).map(|(k, _)| scoped(scope, k)).collect(),
        received_at: utc_to_unix(&e.received_at).unwrap_or(0),
        subject: e.subject.clone().unwrap_or_default(),
        from_name: from.map(|a| a.display().to_string()).unwrap_or_default(),
        from_addr: from.and_then(|a| a.email.clone()).unwrap_or_default(),
        preview: e.preview.clone(),
        seen: e.has_keyword("$seen"),
        flagged: e.has_keyword("$flagged"),
        has_attachment: e.has_attachment,
        size: e.size,
        keywords: row_keywords(e),
    }
}

/// Корень письма — шифрованное S/MIME (`application/pkcs7-mime; smime-type=enveloped-data` или
/// `authEnveloped-data`; без `smime-type` — по имени `.p7m` не судим).
fn is_encrypted(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else { return false };
    let ct = ct.to_ascii_lowercase();
    (ct.contains("application/pkcs7-mime") || ct.contains("application/x-pkcs7-mime"))
        && (ct.contains("enveloped-data") || ct.contains("authenveloped-data"))
}

/// Ключевые слова для сервера: без локальных меток строки.
fn server_words(words: &[String]) -> Vec<String> {
    words.iter().filter(|k| !k.eq_ignore_ascii_case(postman_cache::ENCRYPTED)).cloned().collect()
}

/// Ключевые слова письма для кэша: кроме `$seen` и `$flagged` (свои поля), нижний регистр, по алфавиту.
fn row_keywords(e: &EmailSummary) -> Vec<String> {
    let mut out: Vec<String> = e
        .keywords
        .iter()
        .filter(|(_, on)| **on)
        .map(|(k, _)| k.to_ascii_lowercase())
        .filter(|k| k != postman_cache::SEEN && k != postman_cache::FLAGGED && k != postman_cache::ENCRYPTED)
        .collect();
    if is_encrypted(e.content_type.as_deref()) {
        out.push(postman_cache::ENCRYPTED.to_owned());
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_tells_mail_from_calendar() {
        let mail = r#"{"@type":"StateChange","changed":{"a1":{"Email":"s1","Mailbox":"s2"}}}"#;
        let only = |ids: &[&str]| MailChange::In(ids.iter().map(|s| s.to_string()).collect());
        assert_eq!(Changed::of(mail), Changed { mail: only(&["a1"]), calendar: false, files: MailChange::None });
        let cal = r#"{"@type":"StateChange","changed":{"a1":{"CalendarEvent":"s3"}}}"#;
        assert_eq!(Changed::of(cal), Changed { mail: MailChange::None, calendar: true, files: MailChange::None });
        let other = r#"{"@type":"StateChange","changed":{"a1":{"ContactCard":"s4"}}}"#;
        assert_eq!(Changed::of(other), Changed::default());
        // Не разобралось — сверить всё.
        assert_eq!(Changed::of("not json"), Changed::ALL);
        assert_eq!(Changed::of("{}"), Changed::ALL);
        let both = Changed { mail: only(&["a1"]), calendar: true, files: MailChange::None };
        assert_eq!(Changed::of(mail).or(Changed::of(cal)), both);
        // Таблица категорий — файл аккаунта: push называет аккаунт, почта не сверяется.
        let file = r#"{"@type":"StateChange","changed":{"i":{"FileNode":"sem"}}}"#;
        assert_eq!(Changed::of(file), Changed { mail: MailChange::None, calendar: false, files: only(&["i"]) });
        assert_eq!(Changed::ALL.files, MailChange::All);
    }

    /// Общие ящики (S4-06): push называет аккаунты — сверяются только они.
    #[test]
    fn push_names_the_accounts_to_refresh() {
        let both = r#"{"@type":"StateChange","changed":{"e":{"Email":"s1"},"i":{"Mailbox":"s2"},"j":{"Quota":"q"}}}"#;
        let c = Changed::of(both);
        assert!(c.mail.touches("e") && c.mail.touches("i") && !c.mail.touches("j"));
        let shared = Changed::of(r#"{"@type":"StateChange","changed":{"i":{"EmailDelivery":"s"}}}"#);
        assert!(shared.mail.touches("i") && !shared.mail.touches("e"));
        assert!(!MailChange::None.touches("e") && MailChange::All.touches("e"));
        assert_eq!(shared.clone().or(Changed::ALL).mail, MailChange::All);
        let merged = shared.or(Changed::of(r#"{"@type":"StateChange","changed":{"e":{"Email":"s"}}}"#));
        assert!(merged.mail.touches("e") && merged.mail.touches("i"));
    }

    fn identity(id: &str, email: &str) -> Identity {
        Identity { id: id.into(), name: String::new(), email: email.into(), text_signature: String::new() }
    }

    /// Отправители (S4-06): свои — первыми; адрес общего ящика, который сервер дал и своему аккаунту, — из общего.
    #[test]
    fn shared_identity_wins_over_own_copy() {
        let own = [identity("b", "user3@itt.test"), identity("c", "Shared-Sales@itt.test")];
        let shared = [("i".to_string(), vec![identity("b", "shared-sales@itt.test")])];
        let all = senders(&own, &shared);
        let got: Vec<(Option<&str>, &str)> = all.iter().map(|f| (f.scope.as_deref(), f.identity.email.as_str())).collect();
        assert_eq!(got, [(None, "user3@itt.test"), (Some("i"), "shared-sales@itt.test")]);
        let draft = |from: &str| Draft { from: Address::new("", from), ..Default::default() };
        assert_eq!(sender_of(&all, &draft("SHARED-sales@itt.test")).unwrap().scope.as_deref(), Some("i"));
        // Адреса нет среди Identity — своя.
        assert_eq!(sender_of(&all, &draft("x@itt.test")).unwrap().identity.id, "b");
        assert!(sender_of(&[], &draft("x@itt.test")).is_err());
    }

    /// Ключи кэша общего ящика: письмо, папки, цепочка и вложения — с префиксом аккаунта.
    #[test]
    fn shared_rows_are_scoped() {
        let e: EmailSummary = serde_json::from_value(serde_json::json!({
            "id": "eaaaaab", "threadId": "b", "mailboxIds": { "a": true, "d": false },
            "receivedAt": "2026-10-20T18:00:58Z", "subject": "Hi", "from": [{ "name": "S", "email": "s@x" }]
        }))
        .unwrap();
        let row = email_row(&e, Some("i"));
        assert_eq!((row.id.as_str(), row.thread_id.as_str(), row.mailbox_ids.as_slice()), ("i/eaaaaab", "i/b", &["i/a".to_string()][..]));
        assert_eq!(email_row(&e, None).id, "eaaaaab");
        let m: Mailbox = serde_json::from_value(serde_json::json!({
            "id": "c", "name": "Sub", "parentId": "a", "role": null,
            "myRights": { "mayReadItems": true, "mayAddItems": false, "mayRemoveItems": false, "maySetSeen": true, "maySetKeywords": false }
        }))
        .unwrap();
        let mb = mailbox_row(&m, Some("j"));
        assert_eq!((mb.id.as_str(), mb.parent_id.as_deref()), ("j/c", Some("j/a")));
        assert!(mb.rights.seen && !mb.rights.remove && !mb.rights.keywords);
    }

    /// Метки (S5-02): ключевые слова письма — в строку кэша без `$seen`/`$flagged`, в нижнем регистре; команды
    /// меток делятся по аккаунтам, фильтр в папке — с `inMailbox`.
    #[test]
    fn keywords_reach_the_cache_row() {
        let e: EmailSummary = serde_json::from_value(serde_json::json!({
            "id": "m1", "threadId": "t", "mailboxIds": { "a": true },
            "keywords": { "$seen": true, "$flagged": true, "pm.cat.team": true, "$Label1": true, "pm.cat.off": false },
            "receivedAt": "2026-10-20T18:00:58Z"
        }))
        .unwrap();
        let row = email_row(&e, None);
        assert!(row.seen && row.flagged);
        assert_eq!(row.keywords, ["$label1", "pm.cat.team"]);
        // S5-12 Ф2: шифрованное S/MIME — локальная метка по `Content-Type`, на сервер не уходит.
        let enc: EmailSummary = serde_json::from_value(serde_json::json!({
            "id": "m2", "threadId": "t", "mailboxIds": { "a": true }, "receivedAt": "2026-10-20T18:00:58Z",
            "header:Content-Type": " application/pkcs7-mime; smime-type=enveloped-data;
 name=\"smime.p7m\""
        }))
        .unwrap();
        assert_eq!(email_row(&enc, None).keywords, [postman_cache::ENCRYPTED]);
        assert!(!is_encrypted(Some("application/pkcs7-mime; smime-type=signed-data")) && !is_encrypted(None));
        assert!(is_encrypted(Some("application/pkcs7-mime; smime-type=authEnveloped-data")));
        assert_eq!(server_words(&[postman_cache::ENCRYPTED.into(), "pm.conf".into()]), ["pm.conf"]);
        let keys = ["a".to_string(), "i/b".into(), "c".into(), "i/d".into()];
        let groups = by_account(&keys);
        assert_eq!(groups[&None], ["a", "c"]);
        assert_eq!(groups[&Some("i")], ["i/b", "i/d"]);
        let f = Filter::HasKeyword("pm.cat.team".into());
        assert_eq!(in_mailbox(None, f.clone()), f);
        assert_eq!(in_mailbox(Some("x"), f.clone()), Filter::And(vec![Filter::InMailbox("x".into()), f]));
    }

    #[test]
    fn calendar_changes_are_debounced() {
        let t0 = tokio::time::Instant::now();
        let d = Duration::from_millis(1500);
        let mut db = Debounce::default();
        assert!(!db.fire(t0));
        db.poke(t0, d);
        // Следующие изменения пачки срок не сдвигают и второго события не дают.
        db.poke(t0 + Duration::from_millis(500), d);
        assert!(!db.fire(t0 + Duration::from_millis(1000)));
        assert!(db.fire(t0 + d));
        assert!(!db.fire(t0 + d * 2));
    }

    #[test]
    fn errors_have_kinds() {
        use postman_jmap::Error as E;
        assert_eq!(ErrorKind::of(&E::Protocol(format!("{CACHE_ERROR}table"))), ErrorKind::Storage);
        assert_eq!(ErrorKind::of(&E::Protocol("x".into())), ErrorKind::Protocol);
        assert_eq!(ErrorKind::of(&E::MissingCapability(postman_jmap::MAIL)), ErrorKind::Protocol);
        let m = E::Method { method: "Email/get".into(), kind: "serverFail".into(), description: String::new() };
        let e = SyncError::new(Stage::Refresh, &m);
        assert_eq!(e.kind, ErrorKind::Server);
        assert!(e.to_string().starts_with("Refresh failed (Server)"), "{e}");
    }
}

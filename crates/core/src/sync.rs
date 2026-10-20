//! Фоновая синхронизация: свой поток с tokio, своё соединение с кэшем.
//! UI шлёт команды, в ответ получает события и перечитывает кэш.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc;
use std::time::Duration;

use std::sync::Arc;

use postman_cache::{AccountRow, Addr, AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow, Rights, scoped, unscoped};
use postman_calendar::{Busy, Invitation, Partstat};
use postman_compose::{Address, Draft};

use crate::invite::{self, Delivery};
use crate::settings;
use crate::schedule::{self, Meeting};
use postman_search::SearchIndex;
use postman_jmap::{Auth, Client, EmailAddress, EmailBody, EmailSummary, Envelope, Identity, Mailbox, utc_to_unix};
use postman_jmap::{MailboxRights, SharedAccount};
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
    /// Перенести письмо (удаление в «Удалённые»). UI уже перенёс его в кэше; сбой — письмо с сервера заново.
    Move { id: String, from: String, to: String },
    /// Удалить письмо насовсем (из «Удалённых»). UI уже убрал его из кэша; сбой — письмо с сервера заново.
    Destroy(String),
    /// `replace` — id прежней версии черновика на сервере.
    Send { draft: Box<Draft>, replace: Option<String>, ticket: u64 },
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
    BlobReady { blob_id: String, data: Arc<Vec<u8>> },
    /// Тела загружены в фоне — их текст можно добавить в поиск.
    BodiesStored(Vec<String>),
    /// Догрузка ящика (`LoadMore`) пришла: `added` писем; 0 — на сервере старше нет.
    LoadedMore { mailbox: String, added: usize },
    /// В календаре на сервере что-то изменилось (push, с задержкой на пачку) — открытую неделю перечитать.
    CalendarChanged,
    SearchResults { ticket: u64, ids: Vec<String>, total: u64 },
    BlobFailed { blob_id: String, error: String },
    /// Учётная запись отправителя и подпись (из Identity).
    Me { address: Address, signature: String },
    /// Адреса, от которых можно отправить письмо (S4-06): свои Identity и Identity общих ящиков.
    Senders(Vec<Sender>),
    /// Личные контакты (имя, адрес) — для автодополнения.
    Contacts(Vec<(String, String)>),
    Sent { ticket: u64 },
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
                if let Err(e) = run(account, cache, index, rx, &notify).await {
                    notify(Event::Error(e));
                }
            });
        });
    // Ошибка видна в окне (как любая ошибка синхронизации); команды уходят в никуда, клиент работает с кэшем.
    if let Err(e) = started {
        let _ = failed.send(Event::Error(SyncError::internal(format!("sync thread: {e}"))));
    }
    tx
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
}

impl Changed {
    const ALL: Self = Self { mail: MailChange::All, calendar: true };

    fn or(self, o: Self) -> Self {
        Self { mail: self.mail.or(o.mail), calendar: self.calendar || o.calendar }
    }

    /// Разбор `StateChange` (RFC 8620 §7.1): `{"@type":"StateChange","changed":{"<аккаунт>":{"Email":"…",…}}}`.
    /// Не разобралось — считаем, что изменилось всё (лишняя сверка дешевле пропущенного письма).
    pub fn of(data: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return Self::ALL };
        let Some(accounts) = v.get("changed").and_then(|c| c.as_object()) else { return Self::ALL };
        let mut mail = BTreeSet::new();
        let mut calendar = false;
        for (account, types) in accounts.iter().filter_map(|(a, t)| Some((a, t.as_object()?))) {
            for kind in types.keys() {
                match kind.as_str() {
                    "Email" | "Mailbox" | "Thread" | "EmailDelivery" => {
                        mail.insert(account.clone());
                    }
                    "CalendarEvent" | "Calendar" | "CalendarEventNotification" => calendar = true,
                    _ => {}
                }
            }
        }
        Self { mail: if mail.is_empty() { MailChange::None } else { MailChange::In(mail) }, calendar }
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
    mut rx: UnboundedReceiver<Command>,
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
    let mut poll = tokio::time::interval(POLL);
    poll.tick().await;
    let (push_tx, mut push_rx) = unbounded_channel::<Changed>();
    tokio::spawn(push_loop(client.clone(), push_tx));
    let mut calendar = Debounce::default();
    // Тела писем без тела — в фоне, свежие первыми: чтение без сети и полнотекстовый поиск.
    let mut prefetch: VecDeque<String> = cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default().into();
    let mut refill = false;
    loop {
        tokio::select! {
            // Команды пользователя важнее фоновой работы.
            biased;
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                // Догруженная страница — тоже в предзагрузку тел.
                let more = matches!(cmd, Command::LoadMore { .. });
                if let Err(e) = handle(&accounts, &mut cache, &index, &identities, &send_as, cmd, notify).await
                    && report(e, Stage::Command, notify)
                {
                    return Ok(());
                }
                if more && prefetch.is_empty() {
                    prefetch.extend(cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default());
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
                for acc in accounts.all_mut().filter(|a| changed.mail.touches(&a.id)) {
                    if apply_refresh(acc, &mut cache, &index, &mut prefetch, notify).await {
                        return Ok(());
                    }
                }
            }
            () = calendar.wait() => {
                if calendar.fire(tokio::time::Instant::now()) {
                    notify(Event::CalendarChanged);
                }
            }
            _ = poll.tick() => {
                for acc in accounts.all_mut() {
                    if apply_refresh(acc, &mut cache, &index, &mut prefetch, notify).await {
                        return Ok(());
                    }
                }
                // Предзагрузка прервалась ошибкой — остаток со следующего опроса. Полный обход кэша (а в большом
                // ящике это десятки тысяч строк) — только тогда, а не на каждое событие push.
                if std::mem::take(&mut refill) && prefetch.is_empty() {
                    prefetch.extend(cache.missing_bodies(PREFETCH_LIMIT).unwrap_or_default());
                }
            }
            _ = std::future::ready(()), if !prefetch.is_empty() => {
                let n = prefetch.len().min(PREFETCH_BATCH);
                let keys: Vec<String> = prefetch.drain(..n).collect();
                match prefetch_bodies(&accounts, &cache, &keys).await {
                    Ok(stored) => notify(Event::BodiesStored(stored)),
                    Err(e) => {
                        // Не долбим сервер: остаток подберёт следующий опрос.
                        prefetch.clear();
                        refill = true;
                        if report(e, Stage::Prefetch, notify) {
                            return Ok(());
                        }
                    }
                }
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

/// Сверка аккаунта с сервером; `true` — сервер отклонил вход, синхронизацию остановить.
async fn apply_refresh(
    acc: &mut MailAccount,
    cache: &mut Cache,
    index: &Index,
    prefetch: &mut VecDeque<String>,
    notify: &impl Fn(Event),
) -> bool {
    // Первая загрузка общего ящика не удалась — снова целиком.
    let Some(since) = acc.state.clone() else {
        return match start_state(acc, cache, index, notify).await {
            Ok(s) => {
                acc.state = Some(s);
                notify(Event::MailboxesChanged);
                notify(Event::EmailsChanged);
                false
            }
            Err(e) => report(e, Stage::Refresh, notify),
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
            false
        }
        Ok(None) => false,
        Err(e) => report(e, Stage::Refresh, notify),
    }
}

/// Держит EventSource и будит основной цикл на каждое событие `state` (с тем, что изменилось).
async fn push_loop(client: Client, wake: UnboundedSender<Changed>) {
    let mut backoff = 5;
    // Первое подключение — сразу после загрузки: календарь ещё не открывали, сверять только почту.
    let mut first = true;
    let mail = || Changed { mail: MailChange::All, calendar: false };
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
        save(cache, index, page.emails.iter().map(|e| email_row(e, acc.scope())).collect(), notify)?;
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
            save(cache, index, emails.iter().map(|e| email_row(e, acc.scope())).collect(), notify)?;
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
        Command::Keyword { id: key, keyword, on } => {
            let (acc, id) = accounts.locate(&key)?;
            acc.client.set_keyword(id, keyword, on).await?;
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
        Command::Send { draft, replace, ticket } => {
            let r = send(accounts, cache, send_as, &draft, replace.as_deref()).await;
            notify(match r {
                Ok(()) => Event::Sent { ticket },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
        }
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
            save(cache, index, page.emails.iter().map(|e| email_row(e, acc.scope())).collect(), notify)?;
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
        Command::SaveDraft { draft, replace, ticket } => {
            let r = save_draft(accounts, cache, send_as, &draft, replace.as_deref()).await;
            notify(match r {
                Ok(id) => Event::DraftSaved { ticket, id },
                Err(e) => Event::ComposeFailed { ticket, error: e.to_string() },
            });
        }
    }
    Ok(())
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
    save(cache, index, page.emails.iter().map(|e| email_row(e, acc.scope())).collect(), notify)?;
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
            Ok((emails, _)) => save(cache, index, emails.iter().map(|e| email_row(e, acc.scope())).collect(), notify),
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

/// Отправить из аккаунта отправителя: черновик — в его «Черновики», копия — в его «Отправленные».
async fn send(
    accounts: &Accounts,
    cache: &Cache,
    send_as: &[SendAs],
    draft: &Draft,
    replace: Option<&str>,
) -> postman_jmap::Result<()> {
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
    acc.client.send(raw_message(draft)?, &sender.identity.id, &envelope, &drafts, &sent).await?;
    if let Some(old) = replace {
        let (acc, id) = accounts.locate(old)?;
        acc.client.destroy_email(id).await?;
    }
    Ok(())
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
    send(accounts, cache, send_as, &draft, None).await?;
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
    let id = acc.client.save_draft(raw_message(draft)?, &drafts, same).await?;
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

fn db_err(e: postman_cache::Error) -> postman_jmap::Error {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_tells_mail_from_calendar() {
        let mail = r#"{"@type":"StateChange","changed":{"a1":{"Email":"s1","Mailbox":"s2"}}}"#;
        let only = |ids: &[&str]| MailChange::In(ids.iter().map(|s| s.to_string()).collect());
        assert_eq!(Changed::of(mail), Changed { mail: only(&["a1"]), calendar: false });
        let cal = r#"{"@type":"StateChange","changed":{"a1":{"CalendarEvent":"s3"}}}"#;
        assert_eq!(Changed::of(cal), Changed { mail: MailChange::None, calendar: true });
        let other = r#"{"@type":"StateChange","changed":{"a1":{"ContactCard":"s4"}}}"#;
        assert_eq!(Changed::of(other), Changed::default());
        // Не разобралось — сверить всё.
        assert_eq!(Changed::of("not json"), Changed::ALL);
        assert_eq!(Changed::of("{}"), Changed::ALL);
        assert_eq!(Changed::of(mail).or(Changed::of(cal)), Changed { mail: only(&["a1"]), calendar: true });
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

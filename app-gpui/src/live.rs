//! Живой ящик (S3-04): поток синхронизации `postman_core::sync` вместо демо, события — в поток интерфейса.
//!
//! События идут по `std::sync::mpsc`; поток синхронизации после каждого события зовёт `repaint`, а тот будит
//! задачу gpui через канал `futures` (`unbounded_send`). Задача спит на `next().await` — таймеров и опроса нет:
//! в простое процессор не занят, кадры — только когда что-то изменилось. Проснувшись, задача забирает все
//! накопившиеся события разом: каждая часть экрана получает своё (`Mailer::on_sync`), общее — здесь (`plan`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::{Cache, MailboxRow};
use postman_core::categories;
use postman_core::extract::Extractor;
use postman_core::sync::{self, Account, Command, ErrorKind, Event, Index, Status, SyncError};

use crate::Mailer;
use crate::account::{self, Settings};
use crate::backend::Backend;
use crate::list::Flagged;
use crate::signin::{SignIn, Submit};

/// Найти не больше — как у сервера (`SEARCH_LIMIT` синхронизации).
const SEARCH_LIMIT: usize = 1000;

/// Номер запуска синхронизации: события прежней учётной записи (до повторного входа) отбрасываются.
static SESSION: AtomicU64 = AtomicU64::new(1);

/// Состояние живого ящика в `Mailer`. В демо — пустое.
#[derive(Default)]
pub(crate) struct Live {
    session: u64,
    /// Строка состояния: «Подключение…», «Синхронизировано».
    pub(crate) status: String,
    /// Строка входа (нет учётной записи или вход ещё не подтверждён сервером).
    pub(crate) signin: Option<Entity<SignIn>>,
    /// Вход со строки входа: запомнить после первого ответа сервера, при ошибке — вернуть строку входа.
    pending: Option<Account>,
    /// Пароль входа со строки входа — в диспетчер учётных данных (`false` — «Не хранить»).
    pending_store: bool,
    index: Index,
    extractor: Option<Extractor>,
    next_ticket: u64,
    /// Последняя показанная ошибка (её род): тот же род подряд (сервер недоступен при каждом опросе) — без нового
    /// тоста, пока сервер снова не ответит.
    last_error: Option<String>,
    /// Пришло первое `EmailsChanged` (снимки `--screens --live` ждут его).
    pub(crate) synced: bool,
    /// Адрес и логин учётной записи, с которой идёт синхронизация: для строки входа после отказа сервера.
    account: Option<Settings>,
    /// Файл кэша этой учётной записи: повторный вход в неё же — на открытом кэше (второй раз redb его не откроет).
    path: Option<PathBuf>,
    /// Сервер отклонил вход: показать строку входа на следующем кадре (ей нужно окно).
    pub(crate) reauth: Option<Settings>,
    /// Подписка на флаги списка уже есть (повторный вход не дублирует команды `Keyword`).
    flags_watched: bool,
    /// Локальная копия таблиц категорий прочитана; до этого ответы сервера ждут в `labels_early` (иначе копия,
    /// прочитанная позже, затёрла бы таблицу с сервера, а несохранённая правка — потерялась).
    labels_local: bool,
    labels_early: Vec<categories::Reply>,
    /// Запрос фильтров «Все папки» на сервере (S5-03, `Command::Query`): номер последнего.
    pub(crate) filter_ticket: Option<u64>,
}

impl Live {
    /// Номер нового запроса к серверу (поиск, фильтры).
    pub(crate) fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// Индекс поиска учётной записи (демо и до входа — нет: поиск по кэшу).
    pub(crate) fn index(&self) -> Index {
        self.index.clone()
    }
}

/// Что сделать с экраном по пачке событий синхронизации (чистая функция — тестируется без окна).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// Сервер ответил (папки пришли): вход подтверждён.
    pub(crate) signed_in: bool,
    pub(crate) folders: bool,
    pub(crate) list: bool,
    /// Последний статус пачки.
    pub(crate) status: Option<Status>,
    pub(crate) errors: Vec<SyncError>,
    /// Сервер отклонил вход (401): синхронизация остановилась, нужна строка входа.
    pub(crate) auth: bool,
    /// Первая загрузка закончилась (`Status::Synced`).
    pub(crate) synced: bool,
    /// Календарь на сервере изменился (push) — перечитать открытую неделю.
    pub(crate) calendar: bool,
    /// Догрузка папок: (папка, сколько писем пришло).
    pub(crate) more: Vec<(String, usize)>,
    pub(crate) results: Vec<(u64, Vec<String>)>,
    /// Тела, пришедшие по запросу (`BodyReady`).
    pub(crate) bodies: Vec<String>,
    /// Письма с телом — в полнотекстовый индекс.
    pub(crate) index: Vec<String>,
}

pub(crate) fn plan(events: &[Event]) -> Plan {
    let mut p = Plan::default();
    for ev in events {
        match ev {
            Event::MailboxesChanged => {
                p.signed_in = true;
                p.folders = true;
            }
            Event::EmailsChanged => p.list = true,
            Event::Status(s) => {
                p.synced |= *s == Status::Synced;
                p.status = Some(s.clone());
            }
            Event::Error(e) => {
                p.auth |= e.kind == ErrorKind::Auth;
                p.errors.push(e.clone());
            }
            Event::CalendarChanged => p.calendar = true,
            Event::LoadedMore { mailbox, added } => p.more.push((mailbox.clone(), *added)),
            Event::SearchResults { ticket, ids, .. } => p.results.push((*ticket, ids.clone())),
            Event::BodyReady(id) => {
                p.bodies.push(id.clone());
                p.index.push(id.clone());
            }
            Event::BodiesStored(ids) => p.index.extend(ids.iter().cloned()),
            // Остальное берут части экрана (`on_sync`): окно письма, область чтения, календарь.
            _ => {}
        }
    }
    p
}

/// Результаты поиска: сначала локальные (нечёткие) совпадения, затем то, что нашёл только сервер.
pub(crate) fn merge(local: &[String], server: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    local.iter().chain(server).filter(|id| seen.insert(id.as_str())).cloned().collect()
}

/// Ошибки для тостов: тот же род ошибки подряд не показывается (сервер недоступен — один тост, а не на каждый
/// опрос); отказ во входе — не тостом, а строкой входа.
pub(crate) fn fresh_errors(last: &mut Option<String>, errors: Vec<SyncError>) -> Vec<SyncError> {
    errors
        .into_iter()
        .filter(|e| e.kind != ErrorKind::Auth)
        .filter(|e| {
            let key = format!("{:?}", e.kind);
            let fresh = last.as_deref() != Some(key.as_str());
            *last = Some(key);
            fresh
        })
        .collect()
}

/// Текст состояния синхронизации.
pub(crate) fn status_text(s: &Status) -> String {
    match s {
        Status::Connecting { url } => tr!("sync-connecting", url = url.clone()),
        Status::SignedIn { user } => tr!("sync-signed-in", user = user.clone()),
        Status::Loading { mailbox } => tr!("sync-loading", mailbox = mailbox.clone()),
        Status::Synced => tr!("sync-synced"),
    }
}

/// Текст ошибки синхронизации: наш текст по роду, текст сервера — только подробностью.
pub(crate) fn error_text(e: &SyncError) -> String {
    let what = match e.kind {
        ErrorKind::Auth => tr!("sync-error-auth"),
        ErrorKind::Network => tr!("sync-error-network"),
        ErrorKind::Server => tr!("sync-error-server"),
        ErrorKind::Protocol => tr!("sync-error-protocol"),
        ErrorKind::Storage => tr!("sync-error-storage"),
        ErrorKind::Internal => tr!("sync-error-internal"),
    };
    tr!("sync-error", what = what, detail = e.detail.clone())
}

/// Свои «Входящие», иначе первая своя папка, иначе первая (общих ящиков — S4-06).
pub(crate) fn inbox(mailboxes: &[MailboxRow]) -> Option<String> {
    let own = |b: &&MailboxRow| postman_cache::account_of(&b.id).is_none();
    mailboxes
        .iter()
        .filter(own)
        .find(|b| b.role.as_deref() == Some("inbox"))
        .or_else(|| mailboxes.iter().find(own))
        .or(mailboxes.first())
        .map(|b| b.id.clone())
}

/// Кэш и индекс поиска учётной записи (`<кэш>.search`). Индекс не открылся — поиск по кэшу.
pub(crate) fn open_store(path: &Path) -> Result<(Cache, Index), postman_cache::Error> {
    let cache = Cache::open(path)?;
    let index = match postman_search::SearchIndex::open(&path.with_extension("search")) {
        Ok(i) => Some(Arc::new(i)),
        Err(e) => {
            tracing::warn!("search index: {e}");
            None
        }
    };
    Ok((cache, index))
}

/// `--bench --live`: кэш и индекс живого ящика — в каталоге данных бенча (H-01), рабочий кэш учётной записи не
/// трогается. Без `warm` — с чистого листа (замер первой синхронизации); без `keep` — удаляется в конце.
/// Подготовка не входит во время запуска (`start`).
pub(crate) fn bench_store(warm: bool, keep: bool, start: &mut std::time::Instant) -> Result<(Cache, Index), String> {
    let prepared = std::time::Instant::now();
    let dir = postman_demo::testdata_dir().join("live-bench");
    if !warm && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !keep {
        postman_core::bench::remove_on_finish(dir.clone());
    }
    let opened = open_store(&dir.join("cache.redb")).map_err(|e| e.to_string())?;
    *start += prepared.elapsed();
    Ok(opened)
}

/// Поиск по своим данным (в фоне): индекс, пока он пуст (строится) — кэш.
pub(crate) fn local_search(index: &Index, cache: &Cache, query: &str, mailbox: Option<&str>) -> Vec<String> {
    match index.as_ref().filter(|i| !i.is_empty()) {
        Some(i) => {
            // Строки, записанные синхронизацией, видны в поиске после коммита.
            let _ = i.commit();
            let ids = i.search(query, SEARCH_LIMIT).unwrap_or_default();
            match mailbox {
                Some(mb) => cache.filter_mailbox(&ids, mb).unwrap_or_default(),
                None => ids,
            }
        }
        None => cache.search(query, mailbox, SEARCH_LIMIT).unwrap_or_default(),
    }
}

/// Индекс пуст, а в кэше письма есть (кэш старше индекса): построить в фоне — строки, затем тексты тел.
fn build_index(index: Arc<postman_search::SearchIndex>, cache: Cache, extractor: Extractor) {
    let started = std::thread::Builder::new().name("index-build".into()).spawn(move || {
        let mut offset = 0;
        while let Ok(rows) = cache.all_rows(offset, 5000) {
            if rows.is_empty() {
                break;
            }
            offset += rows.len();
            postman_core::indexer::rows(&index, &rows);
        }
        let _ = index.commit();
        let mut offset = 0;
        while let Ok(ids) = cache.body_ids(offset, 5000) {
            if ids.is_empty() {
                break;
            }
            offset += ids.len();
            extractor.queue(ids);
        }
    });
    if let Err(e) = started {
        tracing::warn!("index build thread: {e}");
    }
}

impl Mailer {
    /// Запустить синхронизацию учётной записи на кэше этого экрана. `confirm` — вход со строки входа:
    /// запомнить учётную запись после первого ответа сервера.
    pub(crate) fn start_sync(&mut self, account: Account, index: Index, confirm: bool, cx: &mut Context<Self>) {
        let session = SESSION.fetch_add(1, Ordering::Relaxed);
        let (events, rx) = std::sync::mpsc::channel::<Event>();
        let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
        let wake = wake_tx.clone();
        let repaint = move || {
            let _ = wake.unbounded_send(());
        };
        tracing::info!("sync start: {}", account.url);
        self.live.status = status_text(&Status::Connecting { url: account.url.clone() });
        self.live.account = Some(Settings::of(&account));
        // Свой аккаунт — подписью над папками (метка «нет связи» — у неё, S5-08).
        let own = Settings::of(&account).user;
        self.folders.update(cx, |f, cx| {
            f.own = Some(own);
            cx.notify();
        });
        self.live.path = Some(account::cache_path(&crate::data_dir(), &account));
        let commands = sync::spawn(account.clone(), self.cache.clone(), index.clone(), events, repaint);
        // Поток синхронизации мог не запуститься: его ошибка лежит в канале без пробуждения.
        let _ = wake_tx.unbounded_send(());
        drop(wake_tx);
        self.backend = Backend::Live { commands };
        // Область чтения: блобы и ответы на приглашения — через сервер; мой адрес придёт `Event::Me`.
        self.reading.update(cx, |r, _| {
            r.live = true;
            r.me = None;
        });
        // Календарь — с сервера (S3-05); раздел уже открыт — неделя сразу.
        let calendar_open = self.section == crate::Section::Calendar;
        self.cal.update(cx, |c, cx| {
            c.go_live(cx);
            if calendar_open {
                c.refresh(cx);
            }
        });
        // Кто я для фильтров (упоминания, «Мне», внешние) — из `Event::Me`, не владелец демо-ящика.
        self.list.update(cx, |l, cx| l.forget_me(cx));
        // Контакты и справочник — с сервера (S4-02): поток синхронизации присылает их после входа.
        self.contacts.update(cx, |c, cx| c.go_live(cx));
        self.live.session = session;
        self.live.pending = confirm.then_some(account);
        self.live.extractor = index.clone().map(|i| Extractor::spawn(self.cache.clone(), i));
        if let (Some(i), Some(x)) = (index.clone().filter(|i| i.is_empty()), self.live.extractor.clone()) {
            build_index(i, self.cache.clone(), x);
        }
        self.live.index = index;
        // Флаги, изменённые пользователем, — на сервер (в кэше они уже записаны). Подписка одна на экран.
        if !self.live.flags_watched {
            self.live.flags_watched = true;
            self._subscriptions.push(cx.subscribe(&self.list, |this, _, f: &Flagged, cx| {
                let sent = this.backend.send(Command::Keyword { id: f.id.clone(), keyword: f.keyword, on: f.on });
                if !sent {
                    this.sync_stopped(cx);
                }
            }));
            // Категории (S5-02) — тоже на сервер, одним `Email/set` на аккаунт.
            self._subscriptions.push(cx.subscribe(&self.list, |this, _, l: &crate::list::Labeled, cx| {
                let cmd = Command::SetKeywords { ids: l.ids.clone(), add: l.add.clone(), remove: l.remove.clone() };
                if !this.backend.send(cmd) {
                    this.sync_stopped(cx);
                }
            }));
        }
        self.load_labels(cx);
        self.load_account_prefs(cx);
        cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            // Поток синхронизации закончился — отправитель пробуждений закрыт, задача завершается.
            while wake_rx.next().await.is_some() {
                while wake_rx.try_recv().is_ok() {}
                let batch: Vec<Event> = rx.try_iter().collect();
                if batch.is_empty() {
                    continue;
                }
                let alive = this.update(cx, |m, cx| {
                    if m.live.session == session {
                        m.on_events(&batch, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Пачка событий синхронизации.
    fn on_events(&mut self, events: &[Event], cx: &mut Context<Self>) {
        for ev in events {
            self.on_sync(ev, cx);
            if let Event::Categories(reply) = ev {
                self.on_categories(reply.clone(), cx);
            }
        }
        let plan = plan(events);
        if plan.signed_in
            && let Some(account) = self.live.pending.take()
        {
            let store = self.live.pending_store;
            self.remember(account, store, cx);
        }
        if let Some(s) = plan.status {
            self.live.status = status_text(&s);
            cx.notify();
        }
        if plan.synced
            && let Some(b) = &mut self.bench
        {
            b.synced();
        }
        if !plan.errors.is_empty() {
            self.sync_errors(plan.errors, plan.auth, cx);
        }
        for (mailbox, added) in plan.more {
            self.list.update(cx, |l, cx| {
                if l.mailbox.as_deref() == Some(mailbox.as_str()) {
                    l.more.loaded(added);
                    cx.notify();
                }
            });
        }
        // Неделя открыта — перечитать; раздел закрыт — перечитается при входе в него (`set_section`).
        if plan.calendar && self.section == crate::Section::Calendar {
            self.cal.update(cx, |c, cx| c.refresh(cx));
        }
        if plan.folders {
            // Сервер снова отвечает: следующая ошибка — новым тостом.
            self.live.last_error = None;
            self.reload_folders(cx);
        }
        if plan.list {
            self.live.synced = true;
            self.list.update(cx, |l, cx| l.refresh(cx));
        }
        for (ticket, ids) in plan.results {
            // Поиск командной строки (S5-10, `search.rs`) или фильтры «Все папки».
            if !self.on_search_results(ticket, &ids, cx) && self.live.filter_ticket == Some(ticket) {
                // Фильтры «Все папки»: письма сервера уже в кэше — список пересчитывается с ними.
                self.list.update(cx, |l, cx| l.set_server_hits(ids, cx));
            }
        }
        for id in plan.bodies {
            self.load_body(id, false, cx);
        }
        if let Some(x) = &self.live.extractor
            && !plan.index.is_empty()
        {
            x.queue(plan.index);
        }
    }

    fn sync_errors(&mut self, errors: Vec<SyncError>, auth: bool, cx: &mut Context<Self>) {
        for e in &errors {
            tracing::warn!("sync: {e}");
        }
        // Вход со строки входа не удался: синхронизация остановилась, строка входа — с причиной.
        if self.live.pending.take().is_some() {
            self.backend = Backend::Demo;
            // Синхронизация остановлена: «нет связи» к этой попытке входа больше не относится.
            self.offline = Default::default();
            self.show_offline(String::new(), false, cx);
            self.live.status.clear();
            if let (Some(panel), Some(e)) = (&self.live.signin, errors.last()) {
                let text = error_text(e);
                panel.update(cx, |p, cx| p.failed(&text, cx));
            }
            cx.notify();
            return;
        }
        // Сохранённый вход отклонён (пароль сменили, сессию отозвали): синхронизация остановилась сама —
        // строка входа с адресом и логином, новый пароль заменит прежний в диспетчере (`remember`).
        if auth && self.live.signin.is_none() {
            self.live.status.clear();
            self.live.reauth = Some(self.live.account.clone().unwrap_or_default());
            cx.notify();
        }
        // Нет связи — не тостом, а строкой состояния, меткой у аккаунта и полосой над списком (S5-08, `states.rs`).
        let errors: Vec<SyncError> = errors.into_iter().filter(|e| e.kind != ErrorKind::Network).collect();
        for e in fresh_errors(&mut self.live.last_error, errors) {
            let text = tr!("account-sync-error", error = error_text(&e));
            self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx));
        }
    }

    /// Строка входа после отказа сервера: поля — прежней учётной записи, причина — сразу под ними.
    pub(crate) fn show_reauth(&mut self, prefill: Settings, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("sign-in rejected by the server: asking for the password again");
        self.set_section(crate::Section::Mail, cx);
        self.show_signin(Some(prefill), window, cx);
        if let Some(panel) = &self.live.signin {
            panel.update(cx, |p, cx| p.rejected(tr!("sync-reauth"), cx));
        }
    }

    /// Список дошёл до конца загруженного: следующая страница папки с сервера. Демо — догружать нечего.
    pub(crate) fn load_more(&mut self, mailbox: String, cx: &mut Context<Self>) {
        if !self.backend.is_live() {
            return;
        }
        if !self.backend.send(Command::LoadMore { mailbox }) {
            self.list.update(cx, |l, _| l.more.loaded(0));
            self.sync_stopped(cx);
        }
    }

    /// Число писем открытой папки на сервере — списку (догрузка); демо — без догрузки.
    pub(crate) fn sync_total(&mut self, cx: &mut Context<Self>) {
        let live = self.backend.is_live();
        let mailboxes = &self.folders.read(cx).mailboxes;
        let current = self.list.read(cx).mailbox.clone();
        let total = current
            .and_then(|id| mailboxes.iter().find(|b| b.id == id))
            .map(|b| b.total as usize)
            .filter(|_| live);
        self.list.update(cx, |l, _| l.more.set_total(total));
    }

    /// «Элементов: N» в строке состояния; папка загружена не вся — «N из M» (M — на сервере).
    pub(crate) fn items_status(&self, cx: &App) -> String {
        let list = self.list.read(cx);
        match list.more.total.filter(|&t| t > list.count && list.search_ids.is_none()) {
            Some(total) => tr!("status-items-of", n = list.count, total = total),
            None => tr!("status-items", n = list.count),
        }
    }

    /// Команда не ушла: поток синхронизации остановился (ошибка уже показана).
    pub(crate) fn sync_stopped(&mut self, cx: &mut Context<Self>) {
        let text = tr!("account-sync-stopped");
        if self.live.last_error.as_deref() != Some(text.as_str()) {
            self.live.last_error = Some(text.clone());
            self.toasts.update(cx, |t, cx| t.push(Toast::Warning, text, cx));
        }
    }

    /// Таблицы категорий учётной записи (S5-02) — сначала локальная копия из файла рядом с кэшем (в фоне), затем
    /// с сервера (`Event::Categories` при входе). Копии нет — по умолчанию (список их уже показывает); копия не
    /// читается — тоже по умолчанию, причина — в журнал.
    fn load_labels(&mut self, cx: &mut Context<Self>) {
        use postman_core::labels::{LocalFile, Store as _};
        let Some(path) = self.live.path.clone() else { return };
        self.live.labels_local = false;
        self.live.labels_early.clear();
        let task = cx.background_spawn(async move { LocalFile::beside(&path).load() });
        cx.spawn(async move |this, cx| {
            let book = match task.await {
                Ok(book) => book,
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            };
            let _ = this.update(cx, |m, cx| {
                let book = book.unwrap_or_else(crate::labels::default_book);
                m.list.update(cx, |l, cx| l.set_labels(book, cx));
                m.live.labels_local = true;
                for reply in std::mem::take(&mut m.live.labels_early) {
                    m.on_categories(reply, cx);
                }
            });
        })
        .detach();
    }

    /// Подписи и шаблоны учётной записи (S5-09) — из файла рядом с кэшем (в фоне), для письма.
    fn load_account_prefs(&mut self, cx: &mut Context<Self>) {
        use postman_core::prefs::AccountPrefs;
        let Some(path) = self.live.path.clone() else { return };
        let task = cx.background_spawn(async move { AccountPrefs::load(&AccountPrefs::beside(&path)) });
        cx.spawn(async move |this, cx| {
            let prefs = match task.await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("{e}");
                    return;
                }
            };
            let _ = this.update(cx, |m, _| m.compose.prefs = prefs);
        })
        .detach();
    }

    /// Ответ сервера по таблице категорий (`postman_core::categories`): таблица — в список и в локальную копию.
    /// Своя правка ещё не дошла до сервера — не заменяется, а уходит на сервер (сведётся с серверной).
    fn on_categories(&mut self, reply: categories::Reply, cx: &mut Context<Self>) {
        use categories::Reply;
        if !self.live.labels_local {
            self.live.labels_early.push(reply);
            return;
        }
        let mut book = self.list.read(cx).labels.clone();
        match reply {
            Reply::Loaded { account, table } => {
                if let Some(mine) = book.loaded(account.as_deref(), table, crate::labels::default_table) {
                    let request = categories::Request::Save { account, table: mine };
                    if !self.backend.send(Command::Categories(request)) {
                        self.sync_stopped(cx);
                    }
                    return;
                }
            }
            Reply::Saved { account, table } => book.saved(account.as_deref(), table),
            Reply::Unsupported { account } => {
                tracing::info!("categories: no FileNode for account {account:?}, local table only");
                return;
            }
            Reply::Failed { account, error } => {
                tracing::warn!("categories: account {account:?}: {error}");
                return;
            }
            Reply::Purged { keyword, count, .. } => {
                tracing::info!("categories: {keyword} removed from {count} messages");
                self.list.update(cx, |l, cx| l.refresh(cx));
                return;
            }
            Reply::PurgeFailed { error, .. } => {
                tracing::warn!("categories: purge: {error}");
                return;
            }
        }
        if self.list.read(cx).labels == book {
            return;
        }
        self.save_labels(&book, cx);
        self.list.update(cx, |l, cx| l.set_labels(book, cx));
        // Таблица во вкладке «Настройки → Категории» — та же.
        self.categories_changed(cx);
    }

    /// Локальная копия таблиц — в файл рядом с кэшем (в фоне).
    pub(crate) fn save_labels(&self, book: &postman_core::labels::Book, cx: &mut Context<Self>) {
        use postman_core::labels::{LocalFile, Store as _};
        let Some(path) = self.live.path.clone() else { return };
        let book = book.clone();
        cx.background_spawn(async move {
            if let Err(e) = LocalFile::beside(&path).save(&book) {
                tracing::warn!("{e}");
            }
        })
        .detach();
    }

    /// Вход подтверждён сервером: адрес и логин — в файл, пароль — в диспетчер учётных данных (в фоне).
    fn remember(&mut self, account: Account, store: bool, cx: &mut Context<Self>) {
        self.live.signin = None;
        // Аккаунт добавляли из настроек: карточка добавления там больше не нужна.
        self.signed_in(cx);
        let task = cx.background_spawn(async move { account::remember(&crate::data_dir(), &account, store) });
        cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                tracing::warn!("remember account: {e}");
                let _ = this.update(cx, |m, cx| {
                    let text = tr!("account-remember-failed", error = e);
                    m.toasts.update(cx, |t, cx| t.push(Toast::Warning, text, cx));
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Папки изменились: перечитать в фоне; текущей папки больше нет (или не было) — открыть входящие.
    fn reload_folders(&mut self, cx: &mut Context<Self>) {
        let cache = self.cache.clone();
        let task =
            cx.background_spawn(async move { (cache.mailboxes().unwrap_or_default(), cache.accounts().unwrap_or_default()) });
        cx.spawn(async move |this, cx| {
            let (rows, accounts) = task.await;
            let _ = this.update(cx, |m, cx| {
                let current = m.folders.read(cx).current.clone();
                let open = match current {
                    Some(id) if rows.iter().any(|b| b.id == id) => None,
                    _ => inbox(&rows),
                };
                m.list.update(cx, |l, _| l.set_rights(&rows));
                m.folders.update(cx, |f, cx| {
                    f.mailboxes = rows;
                    f.accounts = accounts;
                    cx.notify();
                });
                if let Some(id) = open {
                    m.open_mailbox(id, cx);
                }
                m.sync_total(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Открыть письмо в области чтения. Живой ящик: тело из кэша (в фоне), нет в кэше — с сервера (`Body`).
    pub(crate) fn open_letter(&mut self, row: postman_cache::EmailRow, cx: &mut Context<Self>) {
        if !self.backend.is_live() {
            self.reading.update(cx, |r, cx| r.open(row, cx));
            return;
        }
        let shown = self.reading.read(cx).reading.as_ref().is_some_and(|r| r.row.id == row.id && r.body.is_some());
        if shown {
            return;
        }
        let id = row.id.clone();
        self.reading.update(cx, |r, cx| r.open_live(row, cx));
        self.load_body(id, true, cx);
    }

    /// Тело письма из кэша — в область чтения, если письмо всё ещё открыто. `fetch` — нет в кэше: запросить.
    fn load_body(&mut self, id: String, fetch: bool, cx: &mut Context<Self>) {
        let open = self.reading.read(cx).reading.as_ref().is_some_and(|r| r.row.id == id);
        if !open {
            return;
        }
        let cache = self.cache.clone();
        let key = id.clone();
        let task = cx.background_spawn(async move { cache.body(&key).ok().flatten() });
        cx.spawn(async move |this, cx| {
            let body = task.await;
            let _ = this.update(cx, |m, cx| {
                if let Some(b) = body {
                    m.reading.update(cx, |r, cx| r.show_body(&id, b, cx));
                } else if fetch && !m.backend.send(Command::Body(id)) {
                    m.sync_stopped(cx);
                }
            });
        })
        .detach();
    }

    /// Нет учётной записи (первый запуск): добавление аккаунта на месте экрана почты.
    pub(crate) fn show_signin(&mut self, prefill: Option<Settings>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.new(|cx| SignIn::new(prefill, true, window, cx));
        self.watch_signin(&panel, window, cx);
        self.live.signin = Some(panel);
        cx.notify();
    }

    pub(crate) fn watch_signin(&mut self, panel: &Entity<SignIn>, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe_in(panel, window, |this, panel, s: &Submit, window, cx| {
            // Из настроек: дальше вход показывает та же карточка на месте экрана почты.
            if this.live.signin.is_none() {
                this.live.signin = Some(panel.clone());
            }
            this.sign_in(s.account.clone(), s.store, window, cx);
        }));
        self._subscriptions.push(cx.subscribe_in(panel, window, |this, _, _: &crate::signin::Cancel, _, cx| {
            this.cancel_add_account(cx);
        }));
    }

    /// Адрес и логин учётной записи, с которой идёт синхронизация (для «Настройки → Аккаунты»).
    pub(crate) fn live_account(&self) -> Option<&Settings> {
        self.live.account.as_ref()
    }

    /// Файл кэша учётной записи (рядом — её настройки: подписи и шаблоны); демо — `None`.
    pub(crate) fn account_cache(&self) -> Option<PathBuf> {
        self.live.path.clone()
    }

    /// Удалить аккаунт («Настройки → Аккаунты»): синхронизация останавливается, адрес и логин — из файла, пароль — из
    /// диспетчера учётных данных (в фоне); кэш на диске остаётся. Экран — первый запуск.
    pub(crate) fn remove_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("account removed by the user");
        cx.background_spawn(async move { account::forget(&crate::data_dir()) }).detach();
        let cache = match Cache::open_in_memory() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("in-memory store: {e}");
                return;
            }
        };
        let screens = self.screens.take();
        // Прежний поток синхронизации закончится сам: канал команд закрывается вместе с экраном.
        SESSION.fetch_add(1, Ordering::Relaxed);
        *self = Mailer { screens, ..Mailer::new(cache, None, window, cx) };
        self.show_signin(None, window, cx);
    }

    /// «Проверить и подключить»: открыть кэш этой учётной записи (в фоне), затем экран на нём и синхронизацию.
    fn sign_in(&mut self, account: Account, store: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.live.pending_store = store;
        let path = account::cache_path(&crate::data_dir(), &account);
        // Снова вход в ту же учётную запись (сервер отклонил пароль): кэш и экран уже её.
        if self.live.path.as_ref() == Some(&path) {
            let index = self.live.index.clone();
            self.live.last_error = None;
            self.start_sync(account, index, true, cx);
            return;
        }
        let task = cx.background_spawn(async move { open_store(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let opened = task.await;
            let _ = this.update_in(cx, |m, window, cx| match opened {
                Ok((cache, index)) => m.switch_account(cache, index, account, window, cx),
                Err(e) => {
                    tracing::warn!("account store: {e}");
                    if let Some(panel) = &m.live.signin {
                        panel.update(cx, |p, cx| p.failed(&e.to_string(), cx));
                    }
                }
            });
        })
        .detach();
    }

    /// Экран заново на кэше учётной записи; строка входа остаётся, пока сервер не ответит.
    fn switch_account(
        &mut self,
        cache: Cache,
        index: Index,
        account: Account,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.live.signin.take();
        let screens = self.screens.take();
        let store = self.live.pending_store;
        *self = Mailer { screens, ..Mailer::new(cache, None, window, cx) };
        self.live.pending_store = store;
        if let Some(panel) = panel {
            self.watch_signin(&panel, window, cx);
            self.live.signin = Some(panel);
        }
        self.start_sync(account, index, true, cx);
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{ErrorKind, Event, MailboxRow, Plan, Status, SyncError, fresh_errors, inbox, merge, plan};
    use postman_core::sync::Stage;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn err(kind: ErrorKind, detail: &str) -> SyncError {
        SyncError { kind, stage: Stage::Refresh, detail: detail.into() }
    }

    fn connecting() -> Event {
        Event::Status(Status::Connecting { url: "https://mail".into() })
    }

    #[test]
    fn events_become_a_plan() {
        let events = vec![
            connecting(),
            Event::MailboxesChanged,
            Event::EmailsChanged,
            Event::EmailsChanged,
            Event::BodyReady("a".into()),
            Event::BodiesStored(ids(&["b", "c"])),
            Event::SearchResults { ticket: 7, ids: ids(&["x"]), total: 1 },
            Event::Error(err(ErrorKind::Network, "offline")),
            Event::Status(Status::Synced),
            Event::Sent { ticket: 1, scheduled: None },
            Event::LoadedMore { mailbox: "in".into(), added: 500 },
            Event::CalendarChanged,
        ];
        let p = plan(&events);
        assert_eq!(
            p,
            Plan {
                signed_in: true,
                folders: true,
                list: true,
                status: Some(Status::Synced),
                errors: vec![err(ErrorKind::Network, "offline")],
                auth: false,
                synced: true,
                calendar: true,
                more: vec![("in".into(), 500)],
                results: vec![(7, ids(&["x"]))],
                bodies: ids(&["a"]),
                index: ids(&["a", "b", "c"]),
            }
        );
        assert_eq!(plan(&[Event::Contacts(Vec::new())]), Plan::default());
        // Ошибка до первого ответа сервера — вход не подтверждён.
        let rejected = plan(&[connecting(), Event::Error(err(ErrorKind::Auth, "401"))]);
        assert!(!rejected.signed_in && rejected.auth);
    }

    #[test]
    fn local_results_first_then_server_only() {
        assert_eq!(merge(&ids(&["a", "b"]), &ids(&["c", "a", "d"])), ids(&["a", "b", "c", "d"]));
        assert_eq!(merge(&[], &ids(&["c", "c"])), ids(&["c"]));
        assert_eq!(merge(&[], &[]), Vec::<String>::new());
    }

    #[test]
    fn same_error_is_not_repeated() {
        let mut last = None;
        let net = |d: &str| err(ErrorKind::Network, d);
        // Сервер недоступен при каждом опросе (подробности разные) — один тост.
        assert_eq!(fresh_errors(&mut last, vec![net("timeout"), net("connect")]), vec![net("timeout")]);
        assert_eq!(fresh_errors(&mut last, vec![net("timeout")]), vec![]);
        let server = err(ErrorKind::Server, "500");
        assert_eq!(fresh_errors(&mut last, vec![server.clone(), net("timeout")]), vec![server, net("timeout")]);
        // Отказ во входе — строкой входа, не тостом.
        assert_eq!(fresh_errors(&mut last, vec![err(ErrorKind::Auth, "401")]), vec![]);
    }

    #[test]
    fn inbox_is_preferred() {
        let mb = |id: &str, role: Option<&str>| MailboxRow {
            id: id.into(),
            name: id.into(),
            role: role.map(str::to_owned),
            parent_id: None,
            sort_order: 0,
            total: 0,
            unread: 0,
            rights: postman_cache::Rights::ALL,
        };
        assert_eq!(inbox(&[mb("sent", Some("sent")), mb("in", Some("inbox"))]), Some("in".into()));
        // «Входящие» общего ящика (S4-06) — не свои.
        assert_eq!(inbox(&[mb("i/in", Some("inbox")), mb("x", None), mb("in", Some("inbox"))]), Some("in".into()));
        assert_eq!(inbox(&[mb("i/in", Some("inbox")), mb("x", None)]), Some("x".into()));
        assert_eq!(inbox(&[mb("a", None), mb("b", None)]), Some("a".into()));
        assert_eq!(inbox(&[]), None);
    }
}

//! Живой ящик (S3-04): поток синхронизации `postman_core::sync` вместо демо, события — в поток интерфейса.
//!
//! События идут по `std::sync::mpsc`; поток синхронизации после каждого события зовёт `repaint`, а тот будит
//! задачу gpui через канал `futures` (`unbounded_send`). Задача спит на `next().await` — таймеров и опроса нет:
//! в простое процессор не занят, кадры — только когда что-то изменилось. Проснувшись, задача забирает все
//! накопившиеся события разом: каждая часть экрана получает своё (`Mailer::on_sync`), общее — здесь (`plan`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::{Cache, MailboxRow};
use postman_core::extract::Extractor;
use postman_core::sync::{self, Account, Command, Event, Index};

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
    index: Index,
    extractor: Option<Extractor>,
    search: Option<LiveSearch>,
    next_ticket: u64,
    /// Последняя показанная ошибка: та же ошибка подряд (сервер недоступен при каждом опросе) — без нового тоста.
    last_error: Option<String>,
    /// Пришло первое `EmailsChanged` (снимки `--screens --live` ждут его).
    pub(crate) synced: bool,
}

impl Live {
    /// Поиск сброшен (другая папка): поздний ответ сервера список не трогает.
    pub(crate) fn clear_search(&mut self) {
        self.search = None;
    }
}

/// Поиск в живом ящике: локальный индекс и сервер, результаты — по мере ответа.
struct LiveSearch {
    ticket: u64,
    query: String,
    local: Option<Vec<String>>,
    server: Option<Vec<String>>,
}

/// Что сделать с экраном по пачке событий синхронизации (чистая функция — тестируется без окна).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// Сервер ответил (папки пришли): вход подтверждён.
    pub(crate) signed_in: bool,
    pub(crate) folders: bool,
    pub(crate) list: bool,
    /// Последний статус пачки.
    pub(crate) status: Option<String>,
    pub(crate) errors: Vec<String>,
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
            Event::Status(s) => p.status = Some(s.clone()),
            Event::Error(e) => p.errors.push(e.clone()),
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

/// Ошибки для тостов: повтор последней подряд не показывается.
pub(crate) fn fresh_errors(last: &mut Option<String>, errors: Vec<String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            let fresh = last.as_deref() != Some(e.as_str());
            *last = Some(e.clone());
            fresh
        })
        .collect()
}

/// Входящие, иначе первая папка.
pub(crate) fn inbox(mailboxes: &[MailboxRow]) -> Option<String> {
    mailboxes.iter().find(|b| b.role.as_deref() == Some("inbox")).or(mailboxes.first()).map(|b| b.id.clone())
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

/// Поиск по своим данным (в фоне): индекс, пока он пуст (строится) — кэш.
fn local_search(index: &Index, cache: &Cache, query: &str, mailbox: Option<&str>) -> Vec<String> {
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
        self.live.status = tr!("account-connecting", url = account.url.clone());
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
        self.live.session = session;
        self.live.pending = confirm.then_some(account);
        self.live.extractor = index.clone().map(|i| Extractor::spawn(self.cache.clone(), i));
        if let (Some(i), Some(x)) = (index.clone().filter(|i| i.is_empty()), self.live.extractor.clone()) {
            build_index(i, self.cache.clone(), x);
        }
        self.live.index = index;
        // Флаги, изменённые пользователем, — на сервер (в кэше они уже записаны).
        self._subscriptions.push(cx.subscribe(&self.list, |this, _, f: &Flagged, cx| {
            let sent = this.backend.send(Command::Keyword { id: f.id.clone(), keyword: f.keyword, on: f.on });
            if !sent {
                this.sync_stopped(cx);
            }
        }));
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
        }
        let plan = plan(events);
        if plan.signed_in
            && let Some(account) = self.live.pending.take()
        {
            self.remember(account, cx);
        }
        if let Some(s) = plan.status {
            self.live.status = s;
            cx.notify();
        }
        if !plan.errors.is_empty() {
            self.sync_errors(plan.errors, cx);
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
            if let Some(s) = self.live.search.as_mut().filter(|s| s.ticket == ticket) {
                s.server = Some(ids);
                self.show_live_results(cx);
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

    fn sync_errors(&mut self, errors: Vec<String>, cx: &mut Context<Self>) {
        for e in &errors {
            tracing::warn!("sync: {e}");
        }
        // Вход со строки входа не удался: синхронизация остановилась, строка входа — с причиной.
        if self.live.pending.take().is_some() {
            self.backend = Backend::Demo;
            self.live.status.clear();
            if let (Some(panel), Some(e)) = (&self.live.signin, errors.last()) {
                panel.update(cx, |p, cx| p.failed(e, cx));
            }
            cx.notify();
            return;
        }
        for e in fresh_errors(&mut self.live.last_error, errors) {
            let text = tr!("account-sync-error", error = e);
            self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx));
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

    /// Вход подтверждён сервером: адрес и логин — в файл, пароль — в диспетчер учётных данных (в фоне).
    fn remember(&mut self, account: Account, cx: &mut Context<Self>) {
        self.live.signin = None;
        let task = cx.background_spawn(async move { account::remember(&crate::data_dir(), &account) });
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
        let task = cx.background_spawn(async move { cache.mailboxes().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |m, cx| {
                let current = m.folders.read(cx).current.clone();
                let open = match current {
                    Some(id) if rows.iter().any(|b| b.id == id) => None,
                    _ => inbox(&rows),
                };
                m.folders.update(cx, |f, cx| {
                    f.mailboxes = rows;
                    cx.notify();
                });
                if let Some(id) = open {
                    m.open_mailbox(id, cx);
                }
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

    /// Поиск в живом ящике: локальный индекс (в фоне) и сервер (`Search`) — результаты в списке по мере ответа.
    pub(crate) fn run_live_search(&mut self, q: &str, cx: &mut Context<Self>) {
        let query = q.trim().to_owned();
        self.reading.update(cx, |r, cx| r.clear(cx));
        if query.is_empty() {
            self.live.search = None;
            self.folders.update(cx, |f, cx| {
                f.results = None;
                cx.notify();
            });
            self.list.update(cx, |l, cx| l.show_search(None, cx));
            return;
        }
        let mailbox = self.list.read(cx).mailbox.clone();
        self.live.next_ticket += 1;
        let ticket = self.live.next_ticket;
        let sent = self.backend.send(Command::Search { query: query.clone(), mailbox: mailbox.clone(), ticket });
        if !sent {
            self.sync_stopped(cx);
        }
        self.live.search =
            Some(LiveSearch { ticket, query: query.clone(), local: None, server: (!sent).then(Vec::new) });
        let (index, cache) = (self.live.index.clone(), self.cache.clone());
        let task = cx.background_spawn(async move { local_search(&index, &cache, &query, mailbox.as_deref()) });
        cx.spawn(async move |this, cx| {
            let ids = task.await;
            let _ = this.update(cx, |m, cx| {
                if let Some(s) = m.live.search.as_mut().filter(|s| s.ticket == ticket) {
                    s.local = Some(ids);
                    m.show_live_results(cx);
                }
            });
        })
        .detach();
    }

    fn show_live_results(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.live.search else { return };
        let ids = merge(s.local.as_deref().unwrap_or_default(), s.server.as_deref().unwrap_or_default());
        if s.local.is_some() && s.server.is_some() && ids.is_empty() {
            let text = tr!("search-nothing", query = s.query.clone());
            self.toasts.update(cx, |t, cx| t.push(Toast::Info, text, cx));
        }
        let n = ids.len();
        self.folders.update(cx, |f, cx| {
            f.results = Some(n);
            cx.notify();
        });
        self.list.update(cx, |l, cx| {
            // Первый ответ — новый список без выделения и сверху (прокрутка папки запомнена); второй дополняет его,
            // выделение остаётся.
            if l.search_ids.is_none() {
                l.begin_search(ids);
            } else {
                l.search_ids = Some(ids);
            }
            l.refresh(cx);
        });
    }

    /// Нет учётной записи: строка входа на месте списка писем.
    pub(crate) fn show_signin(&mut self, prefill: Option<Settings>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.new(|cx| SignIn::new(prefill, window, cx));
        self.watch_signin(&panel, window, cx);
        self.live.signin = Some(panel);
        cx.notify();
    }

    fn watch_signin(&mut self, panel: &Entity<SignIn>, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe_in(panel, window, |this, _, Submit(account), window, cx| {
            this.sign_in(account.clone(), window, cx);
        }));
    }

    /// «Войти»: открыть кэш этой учётной записи (в фоне), затем экран на нём и синхронизацию.
    fn sign_in(&mut self, account: Account, window: &mut Window, cx: &mut Context<Self>) {
        let path = account::cache_path(&crate::data_dir(), &account);
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
        *self = Mailer { screens, ..Mailer::new(cache, None, window, cx) };
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
    use super::{Event, MailboxRow, Plan, fresh_errors, inbox, merge, plan};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn events_become_a_plan() {
        let events = vec![
            Event::Status("connecting".into()),
            Event::MailboxesChanged,
            Event::EmailsChanged,
            Event::EmailsChanged,
            Event::BodyReady("a".into()),
            Event::BodiesStored(ids(&["b", "c"])),
            Event::SearchResults { ticket: 7, ids: ids(&["x"]), total: 1 },
            Event::Error("offline".into()),
            Event::Status("synced".into()),
            Event::Sent { ticket: 1 },
        ];
        let p = plan(&events);
        assert_eq!(
            p,
            Plan {
                signed_in: true,
                folders: true,
                list: true,
                status: Some("synced".into()),
                errors: ids(&["offline"]),
                results: vec![(7, ids(&["x"]))],
                bodies: ids(&["a"]),
                index: ids(&["a", "b", "c"]),
            }
        );
        assert_eq!(plan(&[Event::Contacts(Vec::new())]), Plan::default());
        // Ошибка до первого ответа сервера — вход не подтверждён.
        assert!(!plan(&[Event::Status("connecting".into()), Event::Error("401".into())]).signed_in);
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
        assert_eq!(fresh_errors(&mut last, ids(&["offline", "offline"])), ids(&["offline"]));
        assert_eq!(fresh_errors(&mut last, ids(&["offline"])), Vec::<String>::new());
        assert_eq!(fresh_errors(&mut last, ids(&["401", "offline"])), ids(&["401", "offline"]));
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
        };
        assert_eq!(inbox(&[mb("sent", Some("sent")), mb("in", Some("inbox"))]), Some("in".into()));
        assert_eq!(inbox(&[mb("a", None), mb("b", None)]), Some("a".into()));
        assert_eq!(inbox(&[]), None);
    }
}

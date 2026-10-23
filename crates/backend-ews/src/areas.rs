//! Области Exchange по EWS (ADR 0004, `Areas`): календарь, ответы на встречи, занятость и справочник (GAL) — тем же
//! договором событий, что у JMAP и CalDAV; экраны те же, чего нет (задач, личных контактов) — скрыто по `Features`.
//!
//! - **Неделя** — `FindItem` + `CalendarView` (повторы разворачивает сервер), подробности — `GetItem` пачками; встреча
//!   той же версии (`ChangeKey`) второй раз не запрашивается.
//! - **Встреча** — `CreateItem` с рассылкой сервером (копия — в «Отправленные», как у Outlook); пояс — Windows id.
//! - **Ответ на приглашение** — `AcceptItem`/`TentativelyAcceptItem`/`DeclineItem` по встрече в календаре (её кладёт
//!   туда сам Exchange при доставке приглашения); встречи с этим UID нет — ответ письмом iMIP (почтовый путь).
//! - **Занятость** — `GetUserAvailability` (моя — тем же запросом); ошибка по адресу — «нет данных», не «свободен».
//! - **Справочник** — `ResolveNames` по набранной строке (`Command::FindPeople` → `PeopleFound`): GAL целиком EWS не
//!   отдаёт, `DirectoryLoaded` — пустой.
//! - **Изменения** — опрос `SyncFolderItems` календаря по таймеру синхронизации → `CalendarChanged`.

use std::collections::HashMap;

use postman_calendar::{Busy, Event, Invitation, Partstat, Span};
use postman_core::backend::{AreaCtx, Areas, BoxFuture};
use postman_core::model::{Feature, Features, Principal};
use postman_core::schedule::{Meeting, SEARCH_LIMIT};
use postman_core::sync::{Command, Event as SyncEvent, owned_by};
use postman_ews::{CalendarItem, Client, Error, FolderRef, ItemId, NewItem, Notify, Reply, SyncState, WellKnown};

/// Встреч в окне календаря — не больше (месяц с запасом).
const VIEW_LIMIT: u32 = 1000;
/// Изменений за один `SyncFolderItems`.
const SYNC_PAGE: u32 = 512;
/// Страниц `SyncFolderItems` за один опрос (остальное — в следующий).
const SYNC_PAGES: usize = 20;
/// Людей справочника на одну строку поиска.
pub const PEOPLE_LIMIT: usize = 20;
/// Поиск справочника — со скольки символов.
pub const PEOPLE_FROM: usize = 3;

/// Занятость: моя и по адресам (`None` у адреса — нет данных), как у JMAP и CalDAV.
pub type Availability = (Vec<Busy>, Vec<(String, Option<Vec<Busy>>)>);

/// Области EWS одной учётной записи.
pub struct EwsAreas {
    client: Client,
    me: String,
    state: Option<SyncState>,
    /// Встречи по `ItemId`: версия и событие.
    items: HashMap<String, (Option<String>, Event)>,
}

impl std::fmt::Debug for EwsAreas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsAreas").field("items", &self.items.len()).finish_non_exhaustive()
    }
}

/// Ответ на приглашение → операция EWS; «ждёт ответа» и «передано» — не ответ.
pub fn reply(status: Partstat) -> Option<Reply> {
    match status {
        Partstat::Accepted => Some(Reply::Accept),
        Partstat::Tentative => Some(Reply::Tentative),
        Partstat::Declined => Some(Reply::Decline),
        Partstat::NeedsAction | Partstat::Delegated => None,
    }
}

/// Новая встреча вкладки «Встреча» → `CreateItem`. Организатор — владелец ящика (поле «От» выбрало учётку).
pub fn new_item(meeting: &Meeting) -> Result<NewItem, String> {
    let refs = meeting.attendee_refs();
    NewItem::from_event(&meeting.new_event("", ("", ""), &refs))
}

/// Человек справочника → `Principal` модели (как у JMAP): id — адрес, вид — человек или список рассылки.
pub fn principal(p: postman_ews::Person) -> Principal {
    Principal {
        id: p.email.to_lowercase(),
        name: if p.name.is_empty() { p.email.clone() } else { p.name },
        description: None,
        email: (!p.email.is_empty()).then_some(p.email),
        kind: if p.group { "group".into() } else { "individual".into() },
    }
}

/// Найденное по строке: название, место, описание, участники (как у CalDAV).
pub fn matching(events: Vec<Event>, text: &str) -> Vec<Event> {
    let needle = text.trim().to_lowercase();
    let mut found: Vec<Event> = events
        .into_iter()
        .filter(|e| {
            let people = e.participants.iter().flat_map(|p| [p.name.to_lowercase(), p.email.to_lowercase()]);
            [e.title.to_lowercase(), e.location.to_lowercase(), e.description.to_lowercase()].into_iter().chain(people).any(|s| s.contains(&needle))
        })
        .collect();
    found.truncate(SEARCH_LIMIT);
    found
}

/// Занятость `GetUserAvailability` → (моя, по адресам): первый адрес запроса — мой; ошибка у адреса — `None`.
pub fn split_availability(mut all: Vec<postman_ews::Availability>, emails: &[String]) -> Availability {
    let me = if all.is_empty() { Vec::new() } else { all.remove(0).busy };
    let people = emails
        .iter()
        .enumerate()
        .map(|(i, e)| (e.clone(), all.get(i).and_then(|a| a.error.is_none().then(|| a.busy.clone()))))
        .collect();
    (me, people)
}

impl EwsAreas {
    /// Войти (основной календарь) и запомнить состояние календаря «сейчас».
    pub async fn connect(url: &str, user: &str, password: &str, email: &str) -> Result<EwsAreas, Error> {
        let mut client = Client::new(url, crate::auth(user, password))?;
        client.calendar_folder().await?;
        let mut a = EwsAreas { client, me: email.to_owned(), state: None, items: HashMap::new() };
        a.poll().await?;
        Ok(a)
    }

    /// Календарь, занятость, справочник. Задач, личных контактов, правил и автоответа по EWS у Postman нет.
    pub fn features(&self) -> Features {
        Features::NONE.with(Feature::Calendar, true).with(Feature::FreeBusy, true).with(Feature::Directory, true)
    }

    /// Изменился ли календарь с прошлого раза (`SyncFolderItems`). Первый вызов — только состояние.
    pub async fn poll(&mut self) -> Result<bool, Error> {
        let first = self.state.is_none();
        let mut changed = false;
        for _ in 0..SYNC_PAGES {
            let page = match self.client.sync(&FolderRef::WellKnown(WellKnown::Calendar), self.state.as_ref(), SYNC_PAGE).await {
                Err(e) if e.is_invalid_sync_state() && self.state.is_some() => {
                    // Сервер забыл состояние: начать заново, неделю — перечитать.
                    self.state = None;
                    self.items.clear();
                    changed = true;
                    continue;
                }
                r => r?,
            };
            for id in page.updated.iter().map(|i| &i.id).chain(&page.deleted) {
                self.items.remove(id);
            }
            changed |= !page.created.is_empty() || !page.updated.is_empty() || !page.deleted.is_empty();
            self.state = Some(page.state);
            if page.done {
                break;
            }
        }
        Ok(changed && !first)
    }

    /// Встречи окна (экземпляры серий — отдельно, как разворачивает сервер), по началу.
    pub async fn events(&mut self, from: i64, to: i64) -> Result<Vec<Event>, Error> {
        let (refs, all) = self.client.calendar_view(&FolderRef::WellKnown(WellKnown::Calendar), Span::new(from, to), VIEW_LIMIT).await?;
        if !all {
            tracing::warn!(shown = refs.len(), "exchange: calendar window truncated");
        }
        let missing: Vec<ItemId> = refs
            .iter()
            .filter(|r| self.items.get(&r.id.id).is_none_or(|(ck, _)| *ck != r.id.change_key))
            .map(|r| r.id.clone())
            .collect();
        if !missing.is_empty() {
            for it in self.client.get_items(&missing).await? {
                self.remember(it);
            }
        }
        let mut out: Vec<Event> = refs.iter().filter_map(|r| self.items.get(&r.id.id).map(|(_, e)| e.clone())).collect();
        out.sort_by_key(|e| (e.span.start, e.span.end));
        Ok(out)
    }

    fn remember(&mut self, it: CalendarItem) {
        self.items.insert(it.id.id.clone(), (it.id.change_key.clone(), it.event));
    }

    pub async fn search(&mut self, text: &str, from: i64, to: i64) -> Result<Vec<Event>, Error> {
        Ok(matching(self.events(from, to).await?, text))
    }

    /// Занятость: моя и по адресам одним запросом.
    pub async fn availability(&mut self, emails: &[String], from: i64, to: i64) -> Result<Availability, Error> {
        let mut all: Vec<&str> = vec![self.me.as_str()];
        all.extend(emails.iter().map(String::as_str));
        let got = self.client.availability(&all, Span::new(from, to)).await?;
        Ok(split_availability(got, emails))
    }

    /// Встреча с рассылкой приглашений сервером → её UID (у Exchange UID задаёт сервер).
    pub async fn create_meeting(&mut self, meeting: &Meeting) -> Result<String, Error> {
        let item = new_item(meeting).map_err(|e| Error::Ews { code: "ClientMeeting".into(), message: e })?;
        let notify = if item.required.is_empty() && item.optional.is_empty() { Notify::None } else { Notify::AllAndSaveCopy };
        let id = self.client.create(None, &item, notify).await?;
        let uid = self.client.get_items(std::slice::from_ref(&id)).await?.pop().map(|it| it.event.uid).unwrap_or_default();
        Ok(if uid.is_empty() { meeting.uid.clone() } else { uid })
    }

    /// Ответ на приглашение по встрече с этим UID в календаре около `around`; встречи нет — `false` (ответ письмом).
    pub async fn answer(&mut self, uid: &str, around: Span, status: Partstat) -> Result<bool, Error> {
        let Some(reply) = reply(status) else { return Ok(false) };
        let window = Span::new(around.start - 86_400, around.end.max(around.start) + 86_400);
        self.events(window.start, window.end).await?;
        let found = self
            .items
            .iter()
            .find(|(_, (_, e))| !uid.is_empty() && e.uid.eq_ignore_ascii_case(uid))
            .map(|(id, (ck, _))| ItemId { id: id.clone(), change_key: ck.clone() });
        let Some(id) = found else { return Ok(false) };
        self.client.respond(&id, reply, None, true).await?;
        self.items.remove(&id.id);
        Ok(true)
    }

    /// Люди справочника по строке (`ResolveNames`).
    pub async fn find_people(&mut self, text: &str) -> Result<Vec<Principal>, Error> {
        if text.trim().chars().count() < PEOPLE_FROM {
            return Ok(Vec::new());
        }
        Ok(self.client.resolve_names(text, PEOPLE_LIMIT).await?.into_iter().map(principal).collect())
    }

    /// Команды областей тем же договором событий, что у JMAP; чужая — назад.
    async fn command(&mut self, cmd: Command, ctx: AreaCtx<'_>) -> Option<Command> {
        let notify = ctx.notify;
        match cmd {
            Command::Events { from, to } => notify(match self.events(from, to).await {
                Ok(events) => {
                    let (events, colleagues) = owned_by(&ctx.owner.cloned(), events, Vec::new());
                    SyncEvent::EventsLoaded { from, to, events, colleagues, account: ctx.profile.map(str::to_owned) }
                }
                Err(e) => SyncEvent::CalendarFailed(e.to_string()),
            }),
            Command::SearchEvents { text, from, to } => notify(match self.search(&text, from, to).await {
                Ok(events) => SyncEvent::EventsFound { text, events, error: None },
                Err(e) => SyncEvent::EventsFound { text, events: Vec::new(), error: Some(e.to_string()) },
            }),
            Command::Availability { ticket, emails, from, to, .. } => notify(match self.availability(&emails, from, to).await {
                Ok((me, people)) => SyncEvent::Availability { ticket, me, people, error: None },
                Err(e) => SyncEvent::Availability {
                    ticket,
                    me: Vec::new(),
                    people: emails.into_iter().map(|e| (e, None)).collect(),
                    error: Some(e.to_string()),
                },
            }),
            Command::CreateMeeting { meeting, ticket } => match self.create_meeting(&meeting).await {
                Ok(_) => {
                    notify(SyncEvent::MeetingCreated { ticket, uid: meeting.uid.clone() });
                    notify(SyncEvent::CalendarChanged);
                }
                Err(e) => notify(SyncEvent::MeetingFailed { ticket, error: e.to_string() }),
            },
            Command::Contacts => {
                // Личных контактов по EWS нет; справочник — только поиском (`FindPeople`).
                notify(SyncEvent::ContactsLoaded(Vec::new()));
                notify(SyncEvent::DirectoryLoaded(Vec::new()));
            }
            Command::FindPeople { text } => match self.find_people(&text).await {
                Ok(people) => notify(SyncEvent::PeopleFound { text, people }),
                Err(e) => {
                    tracing::debug!("ews resolve names: {e}");
                    notify(SyncEvent::PeopleFound { text, people: Vec::new() });
                }
            },
            // Занятость в Exchange открывает политика сервера, а не владелец календаря.
            Command::ShareFreeBusy { .. } => notify(SyncEvent::CalendarFailed("Exchange: free/busy sharing is set by the server".into())),
            other => return Some(other),
        }
        None
    }
}

impl Areas for EwsAreas {
    fn features(&self) -> Features {
        EwsAreas::features(self)
    }

    fn handle<'a>(&'a mut self, cmd: Command, ctx: AreaCtx<'a>) -> BoxFuture<'a, Option<Command>> {
        Box::pin(self.command(cmd, ctx))
    }

    fn respond<'a>(&'a mut self, invite: &'a Invitation, status: Partstat) -> BoxFuture<'a, postman_core::model::Result<bool>> {
        Box::pin(async move { self.answer(&invite.event.uid, invite.event.span, status).await.map_err(crate::core_error) })
    }

    /// Задач по EWS у Postman нет (`Features` их не объявляет) — шагу поручения нечего отправлять.
    fn flush_tasks<'a>(&'a mut self, _ctx: AreaCtx<'a>) -> BoxFuture<'a, postman_core::model::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn changed(&mut self) -> BoxFuture<'_, postman_core::model::Result<bool>> {
        Box::pin(async move { self.poll().await.map_err(crate::core_error) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(attendees: Vec<(String, String)>) -> Meeting {
        Meeting {
            uid: "u1".into(),
            title: "Обзор".into(),
            location: "Зал".into(),
            description: String::new(),
            start: "2026-11-02T10:00:00".parse().unwrap(),
            time_zone: "Europe/Moscow".into(),
            duration_secs: 3600,
            attendees,
            optional: vec![("Вера".into(), "vera@example.test".into())],
            from: None,
        }
    }

    #[test]
    fn meeting_becomes_create_item() {
        let it = new_item(&meeting(vec![("Борис".into(), "boris@example.test".into())])).unwrap();
        assert_eq!(it.span.len(), 3600);
        assert_eq!(it.span.start, "2026-11-02T07:00:00Z".parse::<jiff::Timestamp>().unwrap().as_second());
        assert_eq!((it.required[0].email.as_str(), it.optional[0].email.as_str()), ("boris@example.test", "vera@example.test"));
        assert_eq!(it.time_zone.as_deref(), Some("Europe/Moscow"));
    }

    #[test]
    fn replies_and_people() {
        assert_eq!(reply(Partstat::Tentative), Some(Reply::Tentative));
        assert_eq!(reply(Partstat::NeedsAction), None);
        let p = principal(postman_ews::Person { name: "Анна".into(), email: "Anna@example.test".into(), ..Default::default() });
        assert_eq!((p.id.as_str(), p.display_name(), p.email.as_deref(), p.kind.as_str()), ("anna@example.test", "Анна", Some("Anna@example.test"), "individual"));
        let g = principal(postman_ews::Person { email: "dev@example.test".into(), group: true, ..Default::default() });
        assert_eq!((g.display_name(), g.kind.as_str()), ("dev@example.test", "group"));
    }

    #[test]
    fn availability_is_mine_then_theirs() {
        let busy = |s| vec![Busy { span: Span::new(s, s + 60), kind: postman_calendar::BusyKind::Busy }];
        let a = |busy, error: Option<&str>| postman_ews::Availability { email: String::new(), busy, error: error.map(str::to_owned) };
        let (me, people) =
            split_availability(vec![a(busy(1), None), a(busy(2), None), a(Vec::new(), Some("ErrorMailRecipientNotFound"))], &["b@example.test".into(), "x@example.test".into()]);
        assert_eq!(me, busy(1));
        assert_eq!(people, [("b@example.test".to_owned(), Some(busy(2))), ("x@example.test".to_owned(), None)]);
    }
}

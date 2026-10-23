# Exchange (on-prem): учётная запись, проверка, находки

Exchange on-prem по EWS, best effort (ADR 0003, ADR 0004, `docs/specs/S7.md`). Почта, календарь, ответы на встречи,
занятость и справочник — всё по EWS (решение владельца 2026-10-23 вечер, `S0-decisions.md`): у Exchange компании
IMAP4 и приём SMTP от клиентов выключены. Плагин — `postman-backend-ews` (`MailPlugin` — почта, `AreaPlugin` с ключом
`ews` — календарь, занятость, GAL), протокол — крейт `postman-ews`. Вход — Negotiate/NTLM через SSPI Windows, логин и
пароль учётной записи (диспетчер учётных данных Windows). Basic и OAuth не поддерживаются.

**Без адресов, доменов и имён:** здесь и в находках — только версии, коды и счётчики.

## Как Postman заводит учётку Exchange

«Добавить аккаунт» (или первый запуск) — существующий экран: адрес сервера, логин, пароль.

1. Вид сервера по адресу (`Registry::detect`): JMAP нет → EWS по введённому узлу без входа: `https://<узел>/EWS/Exchange.asmx`,
   затем `https://autodiscover.<узел>/EWS/Exchange.asmx` (адрес — домен почты) отвечает 401 с Negotiate или NTLM
   (`postman_ews::probe`) → адрес учётной записи — этот URL EWS. Иначе — IMAP на порту 993, как раньше.
2. При входе — Autodiscover POX с логином и паролем (логин — адрес почты): точный URL EWS (часто другой узел, чем
   `autodiscover.<домен>`); не ответил — адрес учётной записи.
3. Вручную: адрес сервера — сам URL EWS (`https://<узел>/EWS/Exchange.asmx`).

Что видно (`Features`): «Почта», «Календарь» (неделя, поиск, встречи, ответы на приглашения, «Помощник
планирования»), «Контакты» — только раздел «Справочник» (поиск по GAL от 3 символов, подсказки адресов). Задач, личных
контактов, правил, автоответа, квоты, общих ящиков, меток и категорий Postman у Exchange нет — их нет и на экране.

| Функция | EWS | Как |
|---|---|---|
| Дерево папок | `SyncFolderHierarchy`, запасной — `FindFolder` (Deep) под `msgfolderroot` | роли — по стандартным папкам (`GetFolder`: inbox, drafts, sentitems, deleteditems, junkemail, outbox) |
| Письма папки | `FindItem` (новые 500 по дате) + `SyncFolderItems` (изменения, «прочитано») | опрос раз в 2 мин; «снова на связи» — сразу |
| Список | `GetItem` (`Default` + `item:Flag`) | тема, отправитель, дата, размер, вложения, прочитано, флаг |
| Письмо | `GetItem` `MimeContent` | разбор MIME — воркер-песочница; цепочка — по `References` из MIME |
| Прочитано, флаг | `UpdateItem` (`message:IsRead`, `item:Flag`) | других ключевых слов EWS не хранит |
| Перенос, удаление | `MoveItem`, `DeleteItem` | у перенесённого письма — новый id |
| Отправка, черновик | `CreateItem` из MIME: `SendAndSaveCopy`, `SaveOnly` | копию в «Отправленные» кладёт сервер; «Отменить» держит синхронизация; «Отправить позже» нет |
| Поиск, фильтры | `FindItem` с `QueryString` (AQS) | по папкам; даты и размер в AQS — [проверить] |
| Неделя календаря | `FindItem` + `CalendarView`, `GetItem` | повторы разворачивает сервер |
| Новая встреча | `CreateItem`, `SendToAllAndSaveCopy` | пояс — Windows id (`postman_ews::tz`) |
| Ответ на приглашение | `AcceptItem` / `TentativelyAcceptItem` / `DeclineItem` | по встрече с UID приглашения; нет её — письмом iMIP |
| «Помощник планирования» | `GetUserAvailability` | ошибка по адресу — «нет данных», не «свободен» |
| Справочник | `ResolveNames` (GAL) | `Command::FindPeople` → `PeopleFound`; GAL целиком EWS не отдаёт |

Ограничения сервера: не чаще 5 запросов в секунду; `ErrorServerBusy` — пауза `BackOffMilliseconds` и до 2 повторов.

## Проверка

Стенда Exchange нет: проверка — на боевом Exchange компании, только тестовыми учётками, по правилам S7 «Проверка на
боевом Exchange». Без доступа — тесты на корпусе ответов (по примерам Microsoft, обезличен).

| Что | Где | Запуск |
|---|---|---|
| Разбор ответов, сборка запросов, вход (заголовки, привязка к каналу, SSPI NTLM без сети) | `crates/ews` | `cargo test -p postman-ews` |
| Плагин против подменного сервера EWS | `crates/backend-ews/tests/mock.rs` | `cargo test -p postman-backend-ews --test mock` |
| Разведка (только чтение) | `crates/ews/tests/probe.rs` | `cargo test -p postman-ews --test probe -- --ignored --nocapture` |
| Крейт против боевого (`#[ignore]`) | `crates/ews/tests/stand.rs` | `cargo test -p postman-ews --test stand -- --ignored --test-threads=1` |
| Учётка через плагин (`#[ignore]`) | `crates/backend-ews/tests/stand_exchange.rs` | `cargo test -p postman-backend-ews --test stand_exchange -- --ignored --test-threads=1` |
| Руками | `docs/manual-tests/S7-06.md` | владелец |

Окружение — `docs/specs/S7.md` «Проверка на боевом Exchange» (`POSTMAN_EWS_*`); значения — только у владельца, вне
репозитория.

## Находки боевого Exchange (2026-10-23, тестовые учётки)

| № | Что | Итог |
|---|---|---|
| 1 | Версия | Exchange 2013 CU23 (15.0.1497.46), а не 2016+; `RequestServerVersion` `Exchange2013_SP1` принимает |
| 2 | IMAP4 и приём SMTP от клиентов | выключены на узле Exchange (открыт только 25); `mail.<домен>` — другая почтовая система → почта Exchange по EWS (решение владельца) |
| 3 | TLS | rustls не договаривается: сервер берёт ECDHE только с AES-CBC, а GCM — только без ECDHE → TLS EWS — SChannel Windows (`native-tls`) |
| 4 | Autodiscover POX | отвечает на `autodiscover.<домен>` (корень домена — 404); URL EWS — на другом узле домена |
| 5 | Вход | Negotiate (Kerberos или NTLM внутри) — у обеих тестовых учёток; держит ли IIS вход Kerberos на соединении — [проверить] |
| 6 | `GetItem` с `message:InternetMessageId`/`InReplyTo`/`References` | `ErrorInternalServerError` → список — `Default` + `item:Flag`, `Message-ID` и `References` — из MIME |
| 7 | `GetFolder` с `archive` в пачке стандартных папок | роняет весь запрос → «Архив» не запрашиваем |
| 8 | `SyncFolderHierarchy` | `ErrorInternalServerError` (и под `msgfolderroot`) → дерево `FindFolder` (Deep) |
| 9 | `DateTimeReceived` | сразу после отправки себе бывает пуст → дата из `DateTimeSent`/`DateTimeCreated` |
| 10 | `GetUserAvailability` с одинаковыми `StandardTime`/`DaylightTime` | падает внутри (`-2146233088`) → переходы UTC — в разные месяцы |
| 11 | UID встречи EWS = UID iCalendar приглашения из письма (ответ из карточки письма) | [проверить] руками, `docs/manual-tests/S7-06.md` п. 4 |
| 12 | Адреса участников — SMTP или `EX`; у участника после отмены — `IsCancelled` | [проверить] (`ews_meeting_cycle` крейта) |

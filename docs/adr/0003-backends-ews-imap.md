# ADR 0003. Несколько серверов: Exchange (EWS) и IMAP/SMTP рядом с JMAP

Статус: **предложение** (2026-10-23) — ждёт решения владельца. Меняет межкрейтовые контракты (`core` ↔ `jmap`,
`core` ↔ `app-gpui`), поэтому код до решения не трогаем. Спека сессии — `docs/specs/S7.md`.

⚠ отступление от ТЗ: `docs/HANDOFF.md` §1 — «JMAP-only»; решение владельца №5 (2026-10-20) держало IMAP/SMTP в
бэклоге. Новое решение владельца (2026-10-23, `docs/specs/S0-decisions.md`): на переходный период Postman работает
и с Exchange компании (on-prem, 2016/2019/SE) — почта, календарь, контакты, занятость, GAL, приглашения; основной
путь — EWS; IMAP/SMTP — запасной для прочих ящиков, только почта.

## Как сейчас

- `core` зависит от `jmap` в 10 модулях, больше всего — `sync.rs` (61 место, 2 281 строка): цикл синхронизации
  написан под JMAP (`*/changes`, `state`, патчи JSON, `Invocation`).
- Типы JMAP — в публичном API `core` и интерфейса: `sync::Command::Query { filter: postman_jmap::Filter }`,
  `Release`, `Identity`, `Envelope`, `ContactCard`, `Principal`, `BusyPeriod`, `MailboxRights`, `SharedAccount`,
  `Auth`; `app-gpui` зависит от `postman-jmap` напрямую (6 файлов: вход, аккаунт, контакты, настройки, автоответ,
  безопасность).
- Кэш (`cache`, redb) уже почти нейтрален: строки `EmailRow`/`MailboxRow`/`AccountRow`, ключи
  `scoped(аккаунт, id)`, ключевые слова — строки, состояние синхронизации — непрозрачная строка по ключу
  (`sync_state`).
- Функции, опирающиеся на особенности JMAP/Stalwart: таблица категорий в FileNode (S5-02б), правила и автоответ —
  Sieve (S4-07, S5-29), «Отменить» и «Отправить позже» — FUTURERELEASE (S5-06), рассылка приглашений —
  `sendSchedulingMessages`, занятость — `Principal/getAvailability`, общие ящики — общие аккаунты сессии
  (`mail:share`), сроки и метки — `keywords` `pm.*`, квота — `Quota/get`, push — EventSource.

## Варианты

| Вариант | Суть | Итог |
|---|---|---|
| А. JMAP-фасад | Адаптеры EWS и IMAP изображают JMAP-сервер: принимают `Invocation`, отвечают JSON; `core` не меняется | Отвергнут: второй сервер внутри клиента — эмуляция `state`, патчей JSON Pointer, Sieve-скрипта обратно в правила Exchange, ошибок методов; любая новая функция пишется дважды и хрупко |
| **Б. Нейтральная модель в `core`, адаптер на сервер** | `core` говорит с сервером через свои типы и набор областей; на каждый сервер — адаптер в `core` над протокольным крейтом L1 | **Рекомендуется** |
| В. Свой `core` на сервер | Отдельная синхронизация и логика для каждого сервера | Отвергнут: дубли `lifecycle`, `tasks`, `assign`, `categories`, `send_later` и тестов |

## Предложение (вариант Б)

### 1. Слои

| Слой | Крейт | Что |
|---|---|---|
| L1 | `jmap` (есть) | клиент JMAP — без изменений поведения |
| L1 | `ews` (новый) | клиент EWS: SOAP-конверт и разбор ответов (quick-xml, без DTD и сущностей), HTTP/1.1 (hyper + rustls), Autodiscover, вход NTLM/Negotiate через SSPI Windows с привязкой к каналу TLS, Basic по TLS, троттлинг (`BackOffMilliseconds`) |
| L1 | `imap` (новый) | IMAP (`async-imap` + `imap-proto`) и отправка SMTP (`mail-send`) |
| L2 | `core` | нейтральная модель, области-контракты, адаптеры `backend::{jmap, ews, imap}`, общий цикл синхронизации |
| L3 | `app-gpui` | знает только типы `core`; **не зависит ни от одного протокольного крейта** (проверка — тест `layers.rs`) |

Протокольные крейты не знают модели Postman и друг о друге; перевод в модель — только в адаптерах `core`.
`unsafe` (SSPI) — в модуле `ews::sys`, как прочие платформенные модули.

### 2. Нейтральная модель и области

- Типы, которые сейчас приходят из `jmap`, переезжают в `core::model` (поля те же): `Filter`, `Release`, `Envelope`,
  `Identity`, `Principal`, `BusyPeriod`, `ContactCard`, `MailboxRights`, `SharedAccount`, `Auth`, `Mailbox`,
  `EmailSummary`/`EmailBody` (как строки кэша). Адаптер JMAP переводит из `postman_jmap` один к одному.
- Области — трейты с `async fn` (контракт для адаптеров): `Mail` (папки, дельты, тела/MIME, правки, отправка),
  `Calendar` (события, встречи, ответы, занятость), `Directory` (справочник, поиск людей, сертификаты),
  `Tasks`, `Contacts`, `MailboxSettings` (правила, автоответ, квота, таблица категорий).
- Выбор реализации — перечисление `Backend { Jmap(..), Ews(..), Imap(..) }` (enum dispatch): `async fn` в трейтах
  не dyn-совместимы, набор серверов закрыт, вызов — без `Box<dyn Future>`.
- `Features` — что умеет сервер аккаунта: `calendar`, `contacts`, `tasks`, `free_busy`, `directory`,
  `rules: Sieve | ExchangeRules | None`, `autoreply: Sieve | Oof | None`, `hold: Server | Client`,
  `categories: FileNode | UserConfiguration | Local`, `shared`, `quota`, `push: EventSource | Streaming | Pull | Idle
  | Poll`. Интерфейс не показывает отсутствующее (по `docs/DESIGN.md` — без серых кнопок), подписи различий
  (например, «отправится, если Postman запущен») — Fluent.

### 3. Где живут состояние, идентификаторы и push

- **Состояние синхронизации** — в кэше, как сейчас (`sync_state`), ключ `<аккаунт>/<вид>[/<папка>]`, значение —
  непрозрачная строка адаптера:
  - JMAP — `state` типа (как сейчас);
  - EWS — `SyncState` иерархии (`SyncFolderHierarchy`) и каждой папки (`SyncFolderItems`);
  - IMAP — `UIDVALIDITY` + `HIGHESTMODSEQ` (+ `UIDNEXT`) папки; без CONDSTORE — сверка UID и флагов окном.
  Миграции схемы нет: меняются только ключи у новых аккаунтов.
- **Идентификаторы.** Ключ кэша остаётся `scoped(аккаунт, ключ)`. У JMAP ключ = id сервера (как сейчас). У EWS
  `ItemId` длинный (около 150 символов base64) и вместе с `ChangeKey` меняется при правке и переносе; у IMAP UID
  живёт в паре с папкой и `UIDVALIDITY`. Поэтому у EWS и IMAP ключ — короткий локальный, а таблица `remote` в кэше
  хранит ключ → удалённый id (EWS: `ItemId` + `ChangeKey`; IMAP: папка + `UIDVALIDITY` + UID). Перенос на сервере =
  удаление в одной папке и новое письмо в другой; связи между объектами (задача ↔ письмо, поручение) — по
  `Message-ID`/UID iCalendar, как уже сделано в `mailtask` и `assign`.
- **Цепочки:** JMAP — `threadId`; EWS — `ConversationId`; IMAP — клиентом по `References`/`In-Reply-To`
  (`THREAD=REFERENCES`, если объявлен).
- **Push** — внутри адаптера; общий цикл видит только `Changed { виды, аккаунт }`:
  - JMAP — EventSource (как сейчас);
  - EWS — streaming-подписка (`Subscribe` + `GetStreamingEvents`, соединение до 30 мин, затем заново) → pull
    (`GetEvents` по водяному знаку) → опрос; push-подписка (сервер стучится к клиенту) — не нужна;
  - IMAP — IDLE на «Входящих» отдельным соединением, остальные папки — `STATUS` по таймеру.
- **Учётные записи.** `sync::Account { url, auth }` → `Account { kind: Jmap { url } | Ews { email, url } | Imap
  { imap, smtp }, auth }`; у `AccountRow` — поле `backend`. Несколько учётных записей в одном окне — каждая со своим
  адаптером в одном потоке синхронизации (задачи tokio); деревья папок — по аккаунтам, как общие ящики сейчас.
- **Секреты** — как сейчас, Credential Manager Windows (`app-gpui/src/sys/credentials.rs`). Для EWS на машине в
  домене — Negotiate (Kerberos) от учётной записи Windows через SSPI: Postman пароля не видит и не хранит.

### 4. Чужие данные

Как сейчас у JMAP: ответы своего сервера (JSON JMAP, XML EWS, ответы IMAP) разбираются в `core`; тела писем
(MIME из `MimeContent` EWS и `BODY[]` IMAP), HTML, вложения, .ics — только в воркере-песочнице. Разбор XML — без
DTD, внешних сущностей и с пределами размера; входы разбора EWS/IMAP — в фаззинг.

### 5. Как ложатся функции

| Функция Postman | JMAP / Stalwart (сейчас) | Exchange (EWS) | IMAP/SMTP |
|---|---|---|---|
| Папки и роли | `Mailbox`, `role` | `SyncFolderHierarchy`, `DistinguishedFolderId` | `LIST` + SPECIAL-USE, иначе по имени |
| Письма: дельты | `Email/changes` | `SyncFolderItems` (до 512 изменений за вызов) | CONDSTORE/QRESYNC, иначе сверка UID |
| Тело, вложения | `Email/get` части, blob | `GetItem` с `MimeContent` → воркер (тот же разбор MIME); `GetAttachment` для больших | `UID FETCH BODY.PEEK[]` → воркер |
| Прочитано, флаг | `$seen`, `$flagged` | `IsRead`, `Flag.FlagStatus` | `\Seen`, `\Flagged` |
| `$answered`, `$forwarded` | `keywords` | расширенное свойство `PidTagLastVerbExecuted` (0x1081) | `\Answered`, `$Forwarded` |
| Категории `pm.cat.*` | `keywords` + таблица в FileNode | `Categories` писем (имена) + главный список категорий Outlook (`UserConfiguration` `CategoryList`, 25 цветов) — перевод ключ ↔ имя по таблице аккаунта | ключевые слова IMAP (если `PERMANENTFLAGS` содержит `\*`), таблица — локальный файл |
| Сроки и метки `pm.due`, `pm.wait`, `pm.remind`, `pm.expire`, `pm.taken`, `pm.conf` | `keywords` | своё именованное расширенное свойство (свой GUID, многозначная строка) — источник правды; `pm.due` дополнительно — флаг Outlook «К исполнению» с датой (решение владельца, вопрос 5 S7) | ключевые слова IMAP; без `\*` — только в кэше |
| Поиск | `Email/query` + свой индекс tantivy | `FindItem` с `QueryString` (AQS; нужен индекс Exchange Search), иначе `Restriction`; свой индекс — как есть | `UID SEARCH` (TEXT, FROM, SUBJECT) + свой индекс |
| Быстрые фильтры по словам | `Email/query` `hasKeyword` | по индексу ключевых слов кэша (условия по многозначному расширенному свойству на сервере — [проверить]) | `SEARCH KEYWORD` |
| «Отменить» отправку | FUTURERELEASE `HOLDFOR` | держит клиент (окно 10 с, затем `CreateItem` `SendAndSaveCopy`) | держит клиент |
| «Отправить позже» | FUTURERELEASE `HOLDUNTIL` | отложенная доставка `PidTagDeferredSendTime` (0x3FEF) — держит сервер [проверить], иначе клиент | клиент: уйдёт, если Postman запущен |
| Отправка | `EmailSubmission` | `CreateItem` с `MimeContent`, `SendAndSaveCopy` (MIME собирает `compose`, включая S/MIME) | SMTP submission, копия — `APPEND` в «Отправленные» |
| S/MIME | MIME клиента | тот же MIME через `MimeContent` (`ItemClass` `IPM.Note.SMIME*` — [проверить]); сертификаты адресатов — `UserSMIMECertificate` из GAL (`ResolveNames`) | MIME клиента |
| Правила | Sieve (`rules.rs`) | `GetInboxRules`/`UpdateInboxRules` ↔ та же модель «условие → действие»; правила вне модели — только чтение; `RemoveOutlookRuleBlob` — никогда | нет (ManageSieve — после S7) |
| Автоответ | Sieve `vacation` + `VacationResponse` | OOF: `Get/SetUserOofSettings` — «Внутри компании» / «Внешним», `ExternalAudience` `Known` = «только контактам», период; «не отвечать повторно N дней» — не настраивается (Exchange отвечает раз за период) | нет |
| Квота | `Quota/get` | свойства хранилища ящика [проверить] | `GETQUOTAROOT` (RFC 9208) |
| Общие ящики | общие аккаунты сессии (`mail:share`) | ящики из Autodiscover (`AlternativeMailbox`) и добавленные вручную; папки — `DistinguishedFolderId` с `Mailbox`; «От имени» / «Отправить как» — по правам Exchange | нет |
| «Взять в работу» (`pm.taken`) | `keywords` в общем ящике | своё расширенное свойство у письма общего ящика (видно Postman, не Outlook) | — |
| Календарь, встречи | `CalendarEvent`, `sendSchedulingMessages` | `FindItem` `CalendarView` (повторы разворачивает сервер), `CalendarItem` ↔ `Event`, `SendMeetingInvitations`; ответы — `AcceptItem`/`TentativelyAcceptItem`/`DeclineItem`; пояса — Windows ↔ IANA | только iMIP: карточка приглашения и ответ письмом (как для внешних) |
| Занятость | `Principal/getAvailability` | `GetUserAvailability` (до 100 ящиков за запрос, окно до 42 дней) | нет |
| Справочник, подсказки | `Principal/query` | `ResolveNames` (до 100), `FindPeople` постранично; фото — `GetUserPhoto` | нет (личные контакты — локально) |
| Контакты | `ContactCard` | `Contact` в папке «Контакты» | нет |
| Задачи | `CalendarEvent` `@type: Task` | `Task` в папке «Задачи»; `X-POSTMAN-*` → свои расширенные свойства | нет |
| Поручения (S6) | iTIP по VTODO, письма iMIP | те же письма iMIP Postman; Exchange не превращает VTODO в задачу, приём — клиентом (запись `Task` у исполнителя); `TaskRequest` Outlook EWS создавать не умеет | — |
| Делегирование календаря | — | `GetDelegate` (показ); встречи от имени владельца — по правам делегата | — |
| Архив | папка «Архив» | папка «Архив» ящика; онлайн-архив — `archivemsgfolderroot` [проверить] | папка `\Archive` или по имени |

## Цена

- Переделка `sync.rs` и импорта типов в интерфейсе — отдельной задачей (S7-02) **без изменения поведения JMAP**:
  все тесты стенда Stalwart зелёные до и после.
- Размер exe: `ews` (quick-xml, hyper), `imap` (async-imap, imap-proto), `mail-send` — плюс 1,5–3 МиБ [оценка] при
  запасе 0,67 МиБ (`docs/SIZE.md`). Бюджет пересматривается по замеру S7-18 (вопрос владельцу).
- Тесты по трём серверам; у Exchange нет своего стенда — проверка на боевом сервере по правилам
  `docs/specs/S7.md` «Проверка на боевом Exchange».

## Откат

Адаптеры EWS и IMAP — за признаком сборки (`features`) до приёмки S7-18: без них Postman собирается и ведёт себя,
как до S7. Если EWS в компании закрыт (виртуальный каталог выключен или вход не проходит) — Exchange по IMAP/SMTP
(если включён на сервере), только почта.

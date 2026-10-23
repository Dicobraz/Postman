# ADR 0004. Ядро функциональное, протоколы — плагины

Статус: принято (2026-10-23, владелец: «ядро функциональное, а протоколы — плагины»; `S0-decisions.md`). Уточняет
ADR 0003: раздел «Слои» и «Нейтральная модель» (перечисления путей и адаптеры в `core`) — заменены этим ADR; три
вида учётной записи, состояние, идентификаторы, push и секреты — как в ADR 0003. Задача — S7-02б
(`docs/specs/S7.md`).

## Как было

- Адаптеры протоколов жили в `core`: JMAP — `sync.rs` (цикл написан под JMAP) и куски `schedule`, `settings`,
  `categories`, `invite`, `tasks` (`JmapStore`); IMAP — `core::imap`, CalDAV и CardDAV — `core::dav`; перевод типов
  JMAP — `core::backend::jmap`. `core` зависел от `postman-jmap`, `postman-imap`, `postman-dav`.
- ADR 0003 предлагал закрытые перечисления путей (`Mail { Jmap, Imap }`, `Calendar { Jmap, CalDav, Ews }`…) в `core`:
  `async fn` в трейтах не dyn-совместимы. На деле перечисления не понадобились: IMAP (S7-04б) и «стандарты» (S7-05б)
  получили свой поток синхронизации с тем же договором команд и событий, что у JMAP.

## Решение

### Слои

| Слой | Крейты | Правило |
|---|---|---|
| L1 | протоколы `jmap`, `imap`, `dav`, `ews`; `cache`, `compose`, `calendar`, `worker`, `search`, `smime` | без изменений |
| L2 | `core` — модель (`model`), кэш и задачи, общий цикл синхронизации, договор плагинов (`backend`), правило «Нет у сервера — нет на экране» (`Features`), всё без протоколов | **ни одного протокольного крейта** во всём дереве |
| L2b | плагины `backend-jmap`, `backend-imap`, `backend-dav` (позже `backend-ews`) | `core` и только свой протокол L1; друг от друга не зависят |
| L3 | `app-gpui` — композиционный корень: регистрирует плагины (`src/plugins.rs`) | протоколов не знает, только плагины |

Проверяет `crates/core/tests/layers.rs`: `core_does_not_know_protocols`, `plugins_know_core_and_their_own_protocol`,
`interface_does_not_know_protocols`.

### Договор плагина (`postman_core::backend`)

Граница — **поток синхронизации учётной записи за договором команд и событий** (`sync::Command` → `sync::Event`), как
уже жили JMAP и IMAP, плюс **реестр фабрик**:

- **`MailPlugin`** — почтовый путь учётной записи (JMAP; IMAP + SMTP): `spawn(Spawn) -> UnboundedSender<Command>` —
  свой поток с tokio на общем кэше. Ещё: `claims` (своя схема адреса: `imaps://`), `probe` (вид сервера по адресу,
  S7-04в), `area_host` / `has_area` / `with_area` (где искать пути областей и как вписать их адрес),
  `set_tls13_only` (S5-30).
- **`AreaPlugin`** — путь областей: календарь, задачи, занятость, контакты, справочник у учётной записи, где почтовый
  путь их не даёт (IMAP + CalDAV/CardDAV; IMAP + EWS у Exchange). Ключ адреса (`dav` → `imaps://…?dav=https://…`),
  `probe` узла, `connect` → `Box<dyn Areas>`.
- **`Areas`** — подключённые области: `features`, `handle(Command) -> Option<Command>` (не своя команда — назад),
  `respond` (ответ на приглашение своей копией), `flush_tasks` (шаг поручения), `changed` (опрос без push).
- **`Registry`** — список плагинов в порядке проверки: `spawn` по адресу (своя схема, иначе путь по умолчанию —
  первый), `detect` (вид сервера по адресу), `claims`, `set_tls13_only`.
- **Общий цикл** (`postman_core::sync`): `spawn_thread` (поток, рантайм, ошибки старта), `supervise` + трейт
  `Session` (вход; сеть не ответила — «без связи» с паузой, правки — в очередь `link`; отказ во входе — стоп),
  `went_down`, `retry_at`, `report`, `Debounce`, `owned_by`. Сама сессия — у плагина (push JMAP по EventSource,
  IDLE IMAP — разные механизмы).

**Почему так с `async`.** Перебрали: перечисления (ADR 0003) — закрытый набор в `core`, новый протокол правит `core`;
`async fn` в трейтах — не dyn-совместимы, реестр из разнородных плагинов не собрать; `trait-variant` — даёт `Send`-
варианты, но не `dyn`; упакованные футуры (`Pin<Box<dyn Future>>`) — dyn-совместимы, цена — одно выделение на вызов.
Выбрано: почтовый путь — **фабрика потока за каналом команд** (`async` внутри потока, через `dyn` идёт только
синхронный `spawn`); пути областей, которые работают внутри чужого потока, — **упакованные футуры** (`BoxFuture`, без
`Send`: рантайм потока учётной записи однопоточный). Вызовы областей — по команде пользователя или опросу раз в
2 минуты, выделение незаметно. Внутренние трейты, не идущие через `dyn` (`tasks::TaskStore`), остаются с `async fn`.

### Что умеет сервер

`model::Features` — одно на учётную запись. JMAP — по capabilities сессии; IMAP — объединение `Areas::features`
подключённых областей (`Features::union`), без областей — только почта. Сообщает почтовый путь событием
`Event::Features`; правило «Нет у сервера — нет на экране» — у интерфейса (`app-gpui/src/features.rs`), как в S7-03а.

### Ошибки

Общий код (`tasks`, договор областей) — нейтральная `model::Error` (`Method` с кодом JMAP — `cannotCalculateChanges`,
`notFound`…; `Protocol`; `Storage`; `Other` с родом `ErrorKind` и текстом). Плагин переводит ошибки протокола в неё с
тем же родом и текстом; внутри потока плагин работает своими ошибками (`postman-backend-jmap::error`). `ErrorKind`
переехал в `model` (из `sync` — реэкспорт).

### Что не меняется

- Договор интерфейса: `sync::Command`, `sync::Event`, `Status`, `SyncError`, `Target` и маршрутизация по учётным
  записям, `model` — как были. Интерфейс получает канал команд от реестра вместо `sync::spawn` / `imap::spawn`.
- Кэш и его ключи, формат состояния синхронизации, адреса учётных записей (`imaps://…?smtp=…&dav=…`).
- Поведение JMAP, IMAP, CalDAV и CardDAV; тесты стенда — те же, в плагинах (`crates/backend-*/tests`).
- JSContact → `ContactCard` — в модели (`ContactCard::from_jscontact`): формат модели, а не протокола; у JMAP — свой
  разбор того же формата в `postman-jmap`.

## Новый протокол

1. Крейт L1 протокола (как `ews`) — в `PROTOCOL_CRATES` теста слоёв.
2. Плагин `crates/backend-<протокол>`: `MailPlugin` или `AreaPlugin` поверх `postman-core` и своего крейта — в
   `PLUGINS` теста слоёв.
3. Строка в `app-gpui/src/plugins.rs`. `core` и экраны не меняются.

EWS (S7-06г): `postman-backend-ews` — почта Exchange по EWS (решение владельца 2026-10-23 вечер) — `MailPlugin`
(адрес учётной записи — URL EWS), календарь, ответы на встречи, занятость и GAL — `AreaPlugin` с ключом `ews` (почтовый
путь EWS подключает их сам; у IMAP-учётки на Exchange — `imaps://…?ews=https://…`). GAL EWS отдаёт только поиском —
к договору добавлены `Command::FindPeople` → `Event::PeopleFound` (у JMAP — пустой ответ без запроса).

## Цена

- Один крейт на протокол (три сейчас), упакованная футура на вызов области.
- Тесты стенда переехали из `crates/core/tests` в плагины: `cargo test -p postman-backend-jmap --test stand …`,
  `-p postman-backend-imap --test stand_imap`, `--test stand_standards`.

## Откат

Плагины — обычные крейты рабочей области: вернуть модули в `core` и `spawn` в интерфейсе — механически, договор
команд и событий тот же.

# Стенд Stalwart и поведение сервера

Чего сервер не умеет или делает неверно — сводным списком для issues и своих правок: `docs/STALWART-GAPS.md`.

Что проверено на живом сервере и как клиент это учитывает. Стенд: Stalwart v0.16.27 (Docker на ВМ),
домен `itt.test`, пользователи `user1…user3`; доступ, пароли и эталонная коллекция — у владельца
(`2026 ITT Infra\stalwart`, вне репозитория). Внутренние адреса и пароли сюда не пишем.

## Запуск проверок

Доступ (с 2026-10-20): из офисной сети напрямую — только HTTPS 443 (`https://<адрес стенда>`), порт 8080 закрыт;
через туннель владельца (`stalwart-tunnel.ps1`) — `http://localhost:18080`. У стенда сертификат по умолчанию
(rcgen, `CN=rcgen self signed cert`, SAN только `localhost`): ни IP, ни `mail.itt.test` он не покрывает, в доверенные
его не добавить. Поэтому напрямую — с `POSTMAN_INSECURE_TLS=1` (проверка сертификата выключена, в журнале
предупреждение; только стенд). Нужный выход — сертификат сервера на его имя от внутреннего CA (задача стенда).
Прокси: если задан `HTTPS_PROXY`, в `NO_PROXY` нужен `10.0.0.0/8`, иначе reqwest пойдёт к стенду через прокси.

```
POSTMAN_STAND_URL=https://<адрес стенда> POSTMAN_INSECURE_TLS=1   # или http://localhost:18080 через туннель
POSTMAN_STAND_USER1=user1@itt.test POSTMAN_STAND_PASSWORD1=…
POSTMAN_STAND_USER2=user2@itt.test POSTMAN_STAND_PASSWORD2=…
POSTMAN_STAND_USER3=user3@itt.test POSTMAN_STAND_PASSWORD3=…
cargo test -p postman-core --test stand -- --ignored
```

Пароли — только в окружении процесса. Тесты (`crates/core/tests/stand.rs`) идут тем же кодом, что и приложение,
и за собой убирают (письма, события, карточки); в CI не запускаются (`#[ignore]`).

| Тест | Что проверяет |
|---|---|
| `session_follows_connection_origin_and_has_capabilities` | адреса сессии — на адрес подключения; capabilities |
| `mail_is_sent_received_searched_and_flagged` | отправка (blob → черновик → EmailSubmission), поиск у получателя, тело, `$seen`, `Email/changes` |
| `push_delivers_state_change` | EventSource: событие `state` после нового письма |
| `invitation_round_trip` | приглашение → у участника по uid → «принять» → у организатора accepted → удаление → отмена |
| `free_busy_after_sharing` | `Calendar/set shareWith` → `Principal/getAvailability` у коллеги |
| `contacts_are_read` | `ContactCard` → имя и адреса |
| `contacts_are_edited_and_directory_is_listed` | S4-02, user3: `ContactCard/set` create (телефон, организация, должность) → update патчем (адрес добавлен, телефон убран `null`, имя и должность другие) → destroy; `Principal/query` без фильтра — user1…3 типа `individual` |
| `contacts_through_sync` | S4-02, user3, через поток синхронизации: после входа `ContactsLoaded` и `DirectoryLoaded`; `SaveContact` (новая, затем правка) и `DeleteContact` → итог и свежий `ContactsLoaded` |
| `calendar_through_sync` | S3-05, через поток синхронизации: `ShareFreeBusy` (user3 → user1), `CreateMeeting` с приглашением → у user3 по uid, `Events` → встреча в неделе, `Availability` → занятость user3 |
| `event_keywords_sequence_and_principal_kinds` | S5-11: событие с `keywords` и `categories` → оба возвращаются как есть; встреча с user3 → перенос с рассылкой → `sequence` 0 → 1 и у организатора, и у участника; справочник (`Principal/query` без фильтра) — типы на стенде |
| `rejected_password_stops_sync_with_auth_error` | S4-03: неверный пароль → `Error` рода `Auth` на шаге `Connect`, поток синхронизации закрывается без повторов |
| `load_more_through_sync` | S4-03: `LoadMore` после первой страницы → `LoadedMore` с числом писем следующей страницы |
| `calendar_change_arrives_by_push` | S4-03: событие календаря, созданное другим клиентом → push → один `CalendarChanged` (~1,5 с, задержка на пачку), письма не перечитываются |
| `stand_settings::vacation_reply_round_trip` | S4-05: автоответ user2 включён (`settings::Request::SetVacation`) → письмо от user1 → автоответ у user1 → выключен, письма удалены |
| `stand_settings::quota_is_read` | S4-05: `Quota/get` без ошибки (на стенде квот нет — пустой список) |
| `stand_settings::rule_files_by_subject_with_vacation` | S4-07: папка Y (`Mailbox/set`), правило «тема содержит X → Y» (`SaveRules`), автоответ при правилах → письмо от user1 в Y у user2 и автоответ у user1 → правила, папка, письма убраны |
| `stand_settings::invalid_script_is_rejected_by_validate` | S4-07: неверный скрипт — `SaveError::Invalid` с местом ошибки, скрипты сервера не изменились |
| `stand_shared::shared_inbox_is_synced_read_and_flagged` | S4-06, user3: ящик группы `shared-sales` в сессии и в кэше (ключи `<аккаунт>/<id>`); письмо на адрес группы → по push аккаунта группы в кэш (~1 с) → `Body` → `Keyword $flagged` на сервере в аккаунте группы → убрано |
| `stand_shared::mail_is_sent_as_the_shared_mailbox` | S4-06: `Senders` — свой адрес первым, адрес группы — из её аккаунта; `Send` от имени группы → у получателя «От» группы, копия — в «Отправленных» группы, не в своих → убрано |
| `stand_shared::read_only_share_has_rights_but_no_identity` | S4-06: папка `shared-ro`, открытая только для чтения: `myRights` без добавления, удаления и отправки; `Identity/get` — `forbidden` |
| `stand_labels::categories_are_set_queried_and_cleared` | S5-02, user3: письмо создаётся во «Входящих» (`Email/set create`, без SMTP) → категории патчем `keywords/<слово>` (в том числе слово с `~` и `/`) → `hasKeyword` в папке и во всех папках, `notKeyword`, счётчики `limit: 0` → снятие одной категории не трогает другие → неизвестный id — `notUpdated` (`notFound`) → письмо удалено |
| `stand_labels::categories_through_sync` | S5-02, user3, через поток синхронизации: кэш поменян, `SetKeywords` доводит метку до сервера, `Query` → `SearchResults`, `Counts` → `Counts`; снятие — и в кэше, и на сервере |
| `stand_labels::category_table_fits_in_a_file_node` | S5-02: таблица категорий (JSON) — blob → `FileNode/set create` → `FileNode/get` и чтение blob → `destroy` |
| `stand_categories::table_is_shared_between_sessions_last_writer_wins` | S5-02б, user3: таблицы нет → первая запись создаёт `.postman/categories.json` → вторая сессия читает то же → правки двух клиентов последовательно и одновременно (`tokio::join!`) сводятся по категориям (поздняя побеждает, удаление и новая на месте) → запись с прежним `ifInState` — `stateMismatch`, создание существующего — `alreadyExists` → как было |
| `stand_categories::tables_through_sync_and_shared_mailbox` | S5-02б, через поток синхронизации: при входе `Loaded` своей таблицы и таблицы `shared-sales` (своя у ящика), `Unsupported` у папки `shared-ro` → `Save` командой → `Saved` → запись другого клиента приходит push (`FileNode`) → `Loaded` с ней (29 мс – 1 с) → `Save` в аккаунт ящика не трогает свою → как было |
| `stand_categories::purge_removes_the_keyword_from_messages` | S5-02б: удаление категории «и с писем» — `Purge` снимает слово с двух писем во всех папках, `$seen` остаётся → письма удалены |
| `stand_send_later::held_submission_is_canceled_and_not_delivered` | S5-06: user1 → user3 с `HOLDFOR` 30 → `pending`, `sendAt` через 30 с, в списке ждущих → `canceled` → письмо в «Черновиках» с `$draft`, повторная отмена — `cannotUnsend` → после срока у user3 ничего → убрано |
| `stand_send_later::held_submission_is_delivered_after_the_delay` | S5-06: `HOLDUNTIL` через 20 с → `sendAt` тот же; через 8 с у user3 ничего, затем письмо приходит, `receivedAt` ≥ `sendAt`; отмена после выпуска — `cannotUnsend` → убрано |
| `stand_send_later::send_later_through_sync` | S5-06, через поток синхронизации: `Scheduled` при входе (`max_delay` 7 дней) → `Send` с `Release::At` (+2 мин) → `Sent` со сроком → `Scheduled` с ним → `Unsend` → `Unsent`, в `Scheduled` его нет → убрано |
| `stand_threads::reply_chain_is_one_thread_with_new_text_only` | S5-05: user1 → user3 → user1 цепочкой ответов (In-Reply-To, цитата Postman) → у user3 `Command::Thread` (`Thread/get` + `Email/get`) — одна беседа из трёх писем, своё — из «Отправленных», по времени; «только новый текст» — в каждом письме только его строки → письма убраны |

Последний прогон — 2026-10-20, напрямую по HTTPS: `stand_send_later` 3 из 3 (S5-06); `stand_labels` 3 из 3 (S5-02); `stand` 13 из 13 и `stand_shared` 3 из 3 (S4-06); ранее 11 из 11 (S4-03); `contacts_are_edited_and_directory_is_listed` и `contacts_through_sync` (user3, S4-02) — 2026-10-20, отдельно.

## Поведение сервера

1. **Адреса в сессии — внутренние** (даже при входе по HTTPS 443 сессия объявляет `http://…:8080`). `apiUrl`, `downloadUrl`, `uploadUrl`, `eventSourceUrl` абсолютные, на адрес
   из настроек сервера (за туннелем или прокси недоступен). Клиент переносит их на адрес подключения
   (`Session::resolve_urls`): заодно пароль и токен не уходят на другой хост.
2. **Capabilities v0.16.27:** core, mail, submission, vacationresponse, calendars (+parse), contacts (+parse),
   principals, principals:availability, websocket, sieve, blob, quota, filenode, mail:share, emailpush,
   webpush-vapid. `.well-known/jmap` → 307 на `/jmap/session`.
3. **Приглашения — по черновику JMAP Calendars.** Организатор — `organizerCalendarAddress`, участники —
   `participants.*.calendarAddress` (`mailto:`), рассылка — `sendSchedulingMessages: true`. Поля RFC 8984
   `replyTo`/`sendTo` сервер молча игнорирует — приглашения тогда не уходят. Клиент: `NewEvent::to_jmap`.
4. **У участника событие в его аккаунте.** Искать по `uid` (`CalendarEvent/query` с `filter.uid`), себя среди
   участников — по `calendarAddress`. Ответ — патч `participants/<ключ>/participationStatus` с рассылкой;
   организатор видит статус и получает письмо «Accepted: …». Удаление организатором с рассылкой — CANCEL.
5. **Занятость — только после шаринга.** `Principal/getAvailability` отдаёт периоды, если календарь открыт
   запрашивающему (`shareWith/<principal>`: `mayReadFreeBusy` или `mayReadItems`); без шаринга — пустой список
   без ошибки. Для «все видят занятость всех» нужен общий шаринг (группа) — настройка сервера, один раз.
   Клиент: `share_calendar`, `principal_by_email`, `availability` (с capability `principals:availability`).
6. **CalDAV free-busy (Thunderbird, Apple Calendar)** в v0.16.27 требует сразу оба права, `mayReadFreeBusy` и
   `mayReadItems` (баг сервера; JMAP — любое одно). Отсюда пустая «Доступность» у Apple и Thunderbird при
   шаринге «только занятость» — ответ на вопрос `docs/HANDOFF.md` §4 п. 3–4. Postman ходит по JMAP — не затронут.
7. **Контакты:** сервер сам заполняет `name.full` по компонентам в их порядке (`isOrdered`), клиент показывает
   `full` — порядок «Фамилия Имя» задаётся при создании карточки.
8. **Правка контактов:** `name` только с `full` (без компонентов) сервер хранит как есть — клиент создаёт и правит
   карточку одним полем «Имя». Патч `ContactCard/set` по свойствам целиком (`emails`, `phones`, `organizations`,
   `titles`); `null` убирает свойство. Клиент шлёт только изменённые свойства (`ContactCard::patch`).
9. **Справочник:** `Principal/query` без фильтра отдаёт всех (сотрудники и `admin`);
   `name` — логин (адрес), а не ФИО: имя из каталога организации появится, когда сервер возьмёт его из OIDC/LDAP
   [проверить]. Группы и записи без адреса клиент не показывает (`postman_core::contacts::directory`).

10. **Push по типам.** Событие `state` перечисляет изменившиеся типы по аккаунтам (`changed.<аккаунт>.<тип>`):
   почта — `Email`, `Mailbox`, `Thread`, `EmailDelivery`; календарь — `CalendarEvent` (и `Calendar`,
   `CalendarEventNotification`). Клиент (`sync::Changed::of`) сверяет почту только на почтовые типы, на календарные —
   шлёт `CalendarChanged` с задержкой 1,5 с на пачку; не разобралось — сверяет всё.
11. **Отказ во входе — HTTP 401** уже на `/.well-known/jmap`. Клиент останавливает синхронизацию (повторы с тем же
   паролем бессмысленны, а сервер блокирует адрес за перебор) и показывает строку входа с адресом и логином.
12. **Квота загрузки блобов** — 1000 файлов или 50 МБ на учётную запись (`overQuota` у `Blob/upload` и `/upload`),
    импортированные письма в ней остаются, пока не истечёт срок временных блобов. Поэтому большой ящик заполняется
    через `Email/set create` (сервер сам собирает MIME), а не `Email/import`. Для пользователей важно: вложения в
    окне письма загружаются блобами — тысячи вложений подряд упрутся в эту квоту [проверить на рабочем сервере].
13. **Пачка `Email/set create` на 250 писем** проходит (~380 писем/с с рабочей машины по HTTPS).

14. **Автоответ — Sieve-скрипт `vacation`** (S4-05). `VacationResponse/set` пишет скрипт `vacation` (даты —
   `currentdate` в UTC, текст — MIME base64) и с `isEnabled: true` делает его активным, а прежний активный скрипт
   (правила) — неактивным: активен всегда один скрипт. `isEnabled: false` снимает только `vacation`, другой
   активный скрипт не трогает; тема и текст без `isEnabled` активность не меняют. `isEnabled` в ответе
   `VacationResponse/get` — «скрипт `vacation` активен». Автоответ приходит за секунды. Повтор тому же
   отправителю сервер, по RFC 5230, подавляет — тест берёт новую тему на каждый прогон [проверить срок подавления].
15. **Квота** (RFC 9425): у пользователей стенда `Quota/get` — пустой список (ограничений нет), строка состояния без
   заполненности; ящик с квотой — [проверить] (на стенде не задана).
16. **Правила — Sieve** (RFC 9661, S4-07). Скрипты — blob: загрузка, затем `SieveScript/validate` по `blobId`
    (ошибка — `invalidScript` с местом: «… at line 4, column 1») и `SieveScript/set` (неверный скрипт —
    `notCreated`/`invalidScript`); `blobId` записанного скрипта — свой, не загруженный. Активен один скрипт:
    `onSuccessActivateScript` переключает, `onSuccessDeactivateScript` — ни одного; активный не удаляется.
    `fileinto :mailboxid` требует ещё `require "mailbox"` (иначе «Undeclared capability 'mailbox'»). Адрес
    `redirect` сервер не проверяет (пустой принят) — проверяет клиент. `include :personal :optional "vacation"`
    принят, и автоответ из подключённого скрипта работает при активных правилах (тест). Postman пишет скрипт
    `postman` (`crates/core/src/rules.rs`); при включённом автоответе и правилах — `include`, `isEnabled` сервера тогда
    `false`, клиент считает автоответ включённым по своему скрипту. Другие скрипты — только просмотр; замена
    активного — после «Заменить».

17. **Ограничение входящих на получателя** (2026-10-20): после десятков писем одному адресу за короткое время
    Stalwart пишет в журнал `smtp.rate-limit-exceeded` и **молча** не доставляет следующие, в том числе приглашения
    (iTIP): у отправителя `CalendarEvent/set` успешен, у получателя ни события, ни письма. Тесты стенда, которые шлют
    user2 много писем (`stand`, `stand_settings`), лучше не гонять подряд; при сбое `invitation_round_trip` — смотреть
    журнал сервера. Для клиента: «приглашение отправлено» не значит «доставлено» [проверить порог на рабочем сервере].

18. **Общий ящик группы** (S4-06). Группа (`x:Account/set` `@type: Group`; участник — `memberGroupIds` у пользователя)
    даёт участнику в `session.accounts` аккаунт группы: `isPersonal: false`, `isReadOnly: false`, capabilities почты
    и отправки; папки — свои (`Inbox`, `Sent Items`…), `myRights` — полные. Письмо на адрес группы ложится во
    «Входящие» аккаунта группы (участникам не рассылается). id папок и писем — в пределах аккаунта и совпадают с id
    своего ящика (`a`, `b`…): клиент хранит в кэше ключи `<аккаунт>/<id>` (`postman_cache::scoped`), свои — без префикса.
19. **Папка, открытая другому** (`Mailbox/set` `shareWith: { <principal>: { mayReadItems, maySetSeen } }` у владельца):
    у получателя в сессии — аккаунт владельца, в нём только эта папка; `isReadOnly` аккаунта — `false` и при шаринге
    «только чтение», права — только в `myRights` папки. Stalwart ставит и `maySetKeywords: true`, хотя его не давали
    [проверить: правило ACL или баг]; `mayAddItems`, `mayRemoveItems`, `maySubmit` — `false`. Клиент: папка без
    добавления и удаления — «только чтение» (удалять нельзя, флаги — по своим правам), ветка, где так все папки, —
    с подписью «только чтение».
20. **Отправка от имени общего ящика.** В аккаунте группы `Identity/get` отдаёт Identity с адресом группы;
    `EmailSubmission/set` в аккаунте группы с ней проходит: у получателя «От» — адрес группы, копия — в «Отправленных»
    группы. Ещё Stalwart заводит участнику Identity с адресом группы в его собственном аккаунте (отправка оттуда тоже
    проходит, но копия — в свои «Отправленные»); клиент для адреса группы берёт Identity её аккаунта
    (`sync::senders`). Чужой ящик без группы (открыта папка) — `Identity/get` → `forbidden` («You are not an owner of
    account …»): отправлять от его имени нельзя, в «От» его нет. «От имени» с заголовком `Sender` (делегирование через
    право `submit` на папку) — [проверить].
21. **Push по аккаунтам.** EventSource участника присылает и `changed.<аккаунт группы>`: письмо в ящик группы приходит
    в кэш за ~1 с (тест). Для папки, открытой другим пользователем, — тоже по аккаунту владельца [проверить]. Клиент
    сверяет только названные аккаунты (`sync::MailChange`), опрос раз в 2 минуты — все.
22. **Беседы** (S5-05, тест `stand_threads`). Ответ с `In-Reply-To`/`References` и темой «RE: …» Stalwart кладёт в ту же
    цепочку, что и исходное письмо, у обоих пользователей (`threadId` — свой в каждом аккаунте), вместе с копией в
    «Отправленных». `Thread/get` отдаёт `emailIds` из всех папок; `Email/get` со ссылкой
    `#ids: /list/*/emailIds` — одним запросом. `receivedAt` — с точностью до секунды: письма одной секунды идут в
    порядке id. Поиск `Email/query` `text: "RE: …"` не находит ничего — искать без «RE:» и сверять тему на клиенте.

23. **Ключевые слова** (S5-02). Stalwart возвращает их как прислали; заглавные не приводит к нижнему регистру:
    `pm.remind.2026-10-29T0900` и `…t0900` — два разных слова у одного письма (п. 35; RFC 8621 §4.1.1 — без учёта
    регистра; клиент шлёт нижний регистр), `~` и `/` в слове принимает (путь патча — JSON Pointer с `~0`/`~1`). Патч `keywords/<слово>` меняет одно слово,
    остальные (`$seen`, чужие) остаются. `Email/query` с `hasKeyword`/`notKeyword` — и с `inMailbox`, и без него;
    `limit: 0` с `calculateTotal` даёт число без id — счётчики «N из M» одним запросом (до `maxCallsInRequest` = 16
    вызовов; `maxObjectsInSet` = 500). Неизвестный id в `Email/set update` — `notUpdated` с `notFound`, не ошибка запроса.
24. **FileNode** (`urn:ietf:params:jmap:filenode`, черновик IETF): есть у своего аккаунта и у аккаунтов общих ящиков.
    `FileNode/set create` с `blobId` загруженного blob, `parentId: null` — файл в корне; `FileNode/get` отдаёт свой
    `blobId` (не загруженный), имя, тип, права; `destroy` удаляет. Таблица категорий на сервере — здесь
    (`docs/specs/S5.md`, решение №4, S5-02б; п. 29–32). Файл виден в «Файлах» (WebDAV) пользователя — п. 29.
25. **Отложенная отправка — FUTURERELEASE** (S5-06, тест `stand_send_later`). У аккаунта в `submission`:
    `maxDelayedSend: 2592000` (30 дней), `submissionExtensions`: `FUTURERELEASE`, `SIZE`, `DSN`, `DELIVERYBY`,
    `MT-PRIORITY` (`MIXER`), `REQUIRETLS`. Срок задают параметры `envelope.mailFrom.parameters`: `HOLDFOR` (секунды) или
    `HOLDUNTIL` (UTC, `2026-10-21T06:00:00Z`). Свой `sendAt` в `EmailSubmission/set create` сервер не принимает
    (`notCreated`, `invalidProperties` `["sendAt"]`: по RFC 8621 он ставится сервером). В ответе `created` —
    `sendAt` (срок выпуска) и `undoStatus: pending`, `deliveryStatus` — `queued`, «250 2.1.5 Queued».
26. **Предел задержки — 7 дней, а не объявленные 30.** `HOLDFOR` 700000 → `notCreated` `forbiddenMailFrom`
    «501 5.5.4 Requested hold time exceeds maximum of 604800 seconds», 604800 — принят. `HOLDUNTIL` в прошлом →
    `forbiddenMailFrom` «501 5.5.4 HOLDUNTIL must be a date and time in the future». Клиент берёт меньшее из
    `maxDelayedSend` и 7 дней (`send_later::max_delay`) и проверяет срок до отправки.
27. **Отмена — `undoStatus: canceled`** (`EmailSubmission/set update`): `updated`, письмо не доставляется (у получателя
    после срока ничего нет). Повторная отмена и отмена после выпуска — `notUpdated` `cannotUnsend` «The requested
    message is no longer in the queue». После выпуска — `undoStatus: final`, но `deliveryStatus.delivered` остаётся
    `unknown` (доставка на том же сервере). `EmailSubmission/query` с `filter.undoStatus: pending` отдаёт ждущие
    отправки — по нему клиент строит список «Отправится …» после перезапуска. Выпуск — через 0–4 с после срока по
    часам сервера (часы стенда на 1–2 с впереди рабочей машины: сверять `receivedAt` с `sendAt`, а не с часами
    клиента). Переживает ли ждущая отправка перезапуск сервера — [проверить].
28. **`onSuccessUpdateEmail` — при создании отправки, и отложенной тоже:** письмо сразу в «Отправленных», без `$draft`,
    и после отмены остаётся там же. Поэтому клиент после отмены сам возвращает его в «Черновики» (`$draft`), а
    запланированное показывает в «Отправленных» с меткой «⏱ срок»: держать его в «Черновиках» нельзя — после выпуска
    сервер его не перенесёт. Папки «Исходящие» у Stalwart по умолчанию нет. Клиент (S5-06): обычная отправка —
    `HOLDFOR` 13 с (окно «Отменить» 10 с + запас 3 с на ответ сервера), «Отправить позже» — `HOLDUNTIL`; письмо держит
    сервер, поэтому выход из Postman в окне «Отменить» ничего не теряет — письмо уйдёт в срок.
29. **FileNode: папки и имена** (S5-02б, тест `stand_categories`). Узел без `blobId` — папка (`nodeType: directory`);
    имя с точкой (`.postman`) допустимо (запрещены только `.`, `..`, имена устройств Windows и `/<>:"\|?*` —
    `forbiddenNodeNames`, `forbiddenNameChars` в `accountCapabilities`), имена сравниваются с учётом регистра
    (`caseInsensitiveNames: false`). Второй узел с тем же именем в той же папке — `notCreated` `alreadyExists` с
    `existingId`: так клиент узнаёт, что папку или файл только что создал другой. Непустая папка не удаляется
    (`nodeHasChildren`), с `onDestroyRemoveChildren: true` — вместе с содержимым. **Файлы видны пользователю:** папка
    `.postman` и `categories.json` — в WebDAV (`PROPFIND /dav/file/<ящик>/`, Depth 1); скрытых или служебных
    узлов у FileNode нет (`role: null`). Пользователь может удалить таблицу — Postman тогда вернётся к умолчанию.
30. **`FileNode/query`:** фильтр `name` — точное совпадение (без подстроки, с учётом регистра), `parentId: "<id>"` —
    работает; `parentId: null` («в корне») — весь запрос отклоняется (HTTP 400 `notRequest`), `hasParentId: false`
    молча игнорируется (отдаёт и вложенные). Клиент ищет узел в корне фильтром по имени и сам отбирает `parentId: null`
    (`Client::file_node`).
31. **Замена содержимого и `ifInState`.** `FileNode/set update { blobId }` заменяет содержимое (у узла — свой новый
    `blobId`, `modified` меняется); `ifInState` у `FileNode/set` поддержан: не то состояние — ошибка метода
    `stateMismatch`. Состояние — одно на все файлы аккаунта: любая правка в «Файлах» пользователя сдвигает его, и
    запись таблицы тогда перечитывает и сводит заново (`categories::save`, до 5 попыток). Push: `StateChange` несёт
    `changed.<аккаунт>.FileNode` — и своего, и общего ящика; правка таблицы другим клиентом доходит за 29 мс – 1 с.
32. **FileNode в чужих аккаунтах.** Общий ящик группы (`shared-sales`): файлы свои, права полные
    (`mayCreateTopLevelFileNode: true`), загрузка blob в аккаунт ящика (`/jmap/upload/<аккаунт>/`) работает — у ящика
    своя таблица. Аккаунт пользователя, открывшего user3 папку (`shared-ro`): сессия объявляет у него FileNode
    (`mayCreateTopLevelFileNode: false`), но любой метод FileNode — `forbidden` «You do not have access to account j».
    Клиент считает, что таблицы у такого аккаунта нет (`Reply::Unsupported`), и берёт свою.
33. **Метки и версия событий** (S5-11, тест `event_keywords_sequence_and_principal_kinds`). `keywords` и `categories`
    (`CalendarEvent/set create`) хранятся и отдаются как есть, если их запросить в `properties` — по ним клиент
    красит блок недели цветом категории (`Categories::for_event`: `pm.cat.<id>` или имя категории). `sequence`:
    без рассылки правка его не меняет, нулевой сервер не отдаёт (клиент считает 0); правка организатором с
    `sendSchedulingMessages: true` поднимает его на 1 и у организатора, и в копии участника — по нему карточка
    приглашения видит «изменено» и «устарело» (`invite::classify`).
34. **Переговорные и ресурсы.** На стенде `Principal/query` отдаёт только `individual` и `group` (9 записей): типов
    `location`/`resource` нет — строка «Переговорная» во вкладке встречи на стенде не появляется (в демо — есть).
    Бронирование переговорной (участник-ресурс, автопринятие) и её занятость через `Principal/getAvailability` —
    [проверить], когда администратор заведёт такую запись.

35. **Сроки жизненного цикла** (S5-04, тест `stand_lifecycle`). Слова `pm.due.2026-10-29`, `pm.due.2026-10-29t1500`,
    `pm.wait.2026-10-29`, `pm.remind.2026-10-29t0900`, `pm.expire.2026-10-29t2359` и общие `pm.due`, `pm.wait`,
    `pm.remind`, `pm.expire` ставятся одним патчем, читаются тем же текстом и находятся `hasKeyword` (и по слову с датой,
    и по общему — `hasKeyword` ищет слово целиком, поэтому общее слово ставится рядом). Двоеточие (`t09:00`) сервер тоже
    принимает, но формат Postman — без него. Заглавная `T` — отдельное слово (п. 23).
    **Рассылки:** условие `header` в `Email/query` (RFC 8621 §4.4.1) не находит ни одного письма — ни `["List-Unsubscribe"]`,
    ни `["Subject"]`, ни с значением, и через 10 с после загрузки (тест `header_filter_finds_nothing`); ошибки нет.
    `header:List-Id:asText` — `null`, сырой `header:List-Id` и `header:List-Unsubscribe` — есть (и у письма из
    `Email/import`, и из `Email/set create`; у второго `List-Id` не сохраняется вовсе). Поэтому рассылки узнаются по
    сырым заголовкам в `Email/get` (`Client::list_letters`). `before` работает.
    **«Архив»:** у ящиков стенда папки с ролью `archive` нет; `Mailbox/set create` с `role: "archive"` принят — Postman
    создаёт «Архив» при первой правке, которой он нужен («Напомнить», рассылки, «Исчезнет» в архив). Перенос между
    папками и `$seen` — одним `Email/set` патчем (`mailboxIds/<id>`, `keywords/<слово>`).

36. **Поиск «Везде»** (S5-10, тест `stand_search`). Условия запроса командной строки (`postman_core::query`) — `from`
    (адрес и имя), `to`, `subject`, `notKeyword: $seen`, `after`/`before` (UTCDate), `text` — находят ровно ожидаемое.
    Но: **`text` из нескольких слов** ищет все слова в одном поле — «<метка из темы> квартала» (слово из текста письма)
    не находит ничего, а те же слова двумя условиями `text` в `AND` — находят; поэтому Postman шлёт по `text` на слово.
    **Словоформ нет:** «договоры» не находит тему «договор», `subject: договора` — тоже; точная форма находится и в
    теме, и в тексте (`body`). Словоформы находит свой индекс (tantivy, Snowball), сервер дополняет найденное точными
    совпадениями по всему ящику.

## Общие ящики на стенде (S4-06)

- `shared-sales@itt.test` — группа (`x:Account/set` от имени администратора), user3 — участник; пароля нет.
- `shared-ro@itt.test` — пользователь (пароль — в файле владельца рядом с остальными); его «Входящие» открыты user3
  только для чтения (`Mailbox/set` `shareWith` на principal user3).
- Смотрит user3; тест — `cargo test -p postman-core --test stand_shared -- --ignored`, по одному письму на прогон и
  адрес (п. 17).

## Большой ящик (S4-03)

Учётная запись `big@itt.test` (создана `x:Account/set` от имени администратора; пароль — в файле владельца рядом с
остальными) заполнена 25 000 писем примером `fill_stand` (содержимое демо-ящика: отправители, темы, текст; даты — в
прошлое, папки с ролью `archive` у стенда нет — всё во «Входящих»):

```
POSTMAN_URL=https://<стенд> POSTMAN_INSECURE_TLS=1 POSTMAN_USER=big@itt.test POSTMAN_PASSWORD=…
cargo run --release -p postman-core --example fill_stand -- 25000        # дополняет до N, ~65 с на 24 тыс.
cargo run --release -p postman-gpui -- --bench --live [--warm] [--keep]  # замер живого ящика
```

`--bench --live` — тот же сценарий, что и `--bench` (`crates/core/src/bench.rs`), но на учётной записи из окружения;
кэш и индекс — в `%LOCALAPPDATA%\Pets\testdata\postman\live-bench` (рабочий кэш не трогается). Без `--warm` — с
пустого кэша (первая синхронизация), `--keep` оставляет кэш для следующего прогона с `--warm` (запуск с кэша).
Сценарий начинается после первой синхронизации; в строке итога ещё `first_sync_ms` (запуск → «Синхронизировано») и
`list_ms` (запуск → первый кадр со строками). При листании список догружает папку страницами по 500 (`LoadMore`).

Замеры 2026-10-20, release, Windows 11, RTX 5060, стенд по HTTPS из офисной сети; для сравнения — демо 50 тыс. на той
же сборке:

| Что | Бюджет (§2c) | Живой, пустой кэш | Живой, с кэша (`--warm`) | Демо 50 тыс. |
|---|---|---|---|---|
| Первая синхронизация (`first_sync_ms`) | — | 715 мс | 483 мс (дельта `Email/changes`) | — |
| Список на экране (`list_ms`) | < 1 с с кэша | 1152 мс | **424 мс** ✓ (первый кадр) | — |
| Первый кадр | — | 418 мс | 424 мс | 469 мс |
| Писем в списке к концу сценария | — | 5 500 из 25 000 | 6 000 из 25 000 | 35 165 |
| CPU в простое (`idle_cpu_pct`, push подключён) | ≤ 0,5 % | 0,0 % (3 из 5 прогонов), 0,8–1,2 % | 0,0 % (2 из 3), 0,3 % | 0,0 % |
| Рабочий набор: пик / в простое | ≤ 150 МБ | 225 / 163 МБ ✗ | 251 / 171 МБ ✗ | 230 / 195 МБ ✗ |
| Private bytes: пик / в простое | — | 188 / 132 МБ | 214 / 139 МБ | 195 / 166 МБ |
| Листание с открытием: CPU процесса / потока UI | — | 131 % / 52 % | 126 % / 51 % | 77 % / 58 % |
| Листание списка: CPU процесса / потока UI | — | 83 % / 55 % | 135 % / 26 % | 47 % / 38 % |
| Кадр листания p50 / p99 | — | 6,9 / 9,0 мс | 6,9 / 8,1 мс | 6,9 / 7,9 мс |

Выводы:

- Запуск с кэша — в бюджете. Простой с подключённым push — 0,0 %, когда фон закончен: поток синхронизации спит на
  сокете. 0,3–1,2 % — прогоны, где в окно замера простоя (3 с после сценария + 10 с) ещё попала предзагрузка тел
  последней догруженной страницы и их извлечение в индекс; это работа, а не простой, но окно стоит удлинить или
  ждать конца фона [проверить].
- **Память выше бюджета и у демо** (на этой сборке 230 МБ пик, 195 МБ в простое; 88–90 МБ в §2c — замер спайка
  фазы 0, не всего приложения). Живой ящик не хуже демо: в кэше — только загруженные страницы. Разбор, что держит
  память (атлас gpui, шрифты, область чтения, индекс поиска), — отдельной задачей: S4-08, `docs/MEMORY.md`.
- Листание живого ящика нагружает процесс сильнее демо (126–135 % против 47–77 %) за счёт фона, не потока
  интерфейса: догруженные страницы пишутся в кэш и индекс поиска, тела писем предзагружаются (по 25 за запрос) и
  извлекаются в индекс. Кадр от этого не страдает (p99 ≤ 9 мс).
- Исправлено по замерам: запуск с кэша больше не перечитывает первые 500 писем каждой папки (дельта с прошлого
  состояния; 483 мс против 715 мс и меньше трафика); push календаря не вызывает сверку почты; полный обход кэша для
  предзагрузки тел — не на каждый push, а только после догрузки страницы и после сбоя предзагрузки.

## Не проверено

- Вход через внешний OIDC и автосоздание ящика (`docs/IDENTITY.md`) — на стенде только пароли.
- Письма с внешних доменов (iMIP с Gmail/Exchange) — у стенда нет внешнего домена.

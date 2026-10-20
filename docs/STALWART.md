# Стенд Stalwart и поведение сервера

Что проверено на живом сервере и как клиент это учитывает. Стенд: Stalwart v0.16.27 (Docker на ВМ),
домен `itt.test`, пользователи `user1…user3`; доступ, пароли и эталонная коллекция — у владельца
(`2026 ITT Infra\stalwart`, вне репозитория). Внутренние адреса и пароли сюда не пишем.

## Запуск проверок

Туннель к стенду — скрипт владельца (`stalwart-tunnel.ps1`), JMAP — `http://localhost:18080`. Затем:

```
POSTMAN_STAND_URL=http://localhost:18080
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

Последний прогон — 2026-10-20: 6 из 6.

## Поведение сервера

1. **Адреса в сессии — внутренние.** `apiUrl`, `downloadUrl`, `uploadUrl`, `eventSourceUrl` абсолютные, на адрес
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

## Не проверено

- Вход через внешний OIDC и автосоздание ящика (`docs/IDENTITY.md`) — на стенде только пароли.
- Письма с внешних доменов (iMIP с Gmail/Exchange) — у стенда нет внешнего домена.
- Нагрузка: ящик на 20–50 тыс. писем.

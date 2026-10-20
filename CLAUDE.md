# Postman (ThePostman) — правила для Claude Code

Корпоративный клиент почты, календаря и контактов под **Stalwart** по **JMAP**. Чистый Rust, рендер на GPU,
Windows. Приложение семейства «Pets»: общий архитектурный базис — скилл `pets-architecture`
(эталон — Урфин). Всё, что отличается от базиса, — только через ADR в `docs/adr/` с решением владельца
и пометкой «⚠ отступление от базиса».

## С чего начинать сессию

1. `docs/specs/S0-decisions.md` — решения владельца, приоритетнее всех остальных документов.
2. `docs/PLAN.md` — текущая фаза и пункт. Сейчас: **фаза 3, сервер и стабилизация** (сессия S4, `docs/specs/S4.md`);
   фазы 0–2 закрыты (`docs/specs/S1.md`, `S2.md`, `S3.md`).
3. `docs/UI-SPIKE.md` — почему gpui, замеры, риски. `docs/adr/` — принятые отступления.
4. `docs/HANDOFF.md` — исходное ТЗ: MVP, дизайн, бюджеты (§2c), безопасность рендера писем, вехи.
5. Остальное читать по необходимости. Экономить токены: пояснения — по сути.

## Порядок работы

- Сначала исследуем, потом планируем, потом делаем. Новое — концепт в чате, файлы — после согласования.
- Вопросы владельцу — пакетом «Решения для владельца» (№, вопрос, рекомендация).
- Сервер — стенд Stalwart (`docs/STALWART.md`): новое поведение сервера сначала тестом в `crates/core/tests/stand.rs`,
  находки — туда же в документ. Непроверенное на стенде — пометка [проверить].
- Одна задача = ветка `sX/NN-кратко` = один PR; спека сессии — `docs/specs/Sx.md` (цели, задачи `Sx-NN`
  с зависимостями и приёмкой). Соответствие сессий фазам плана — в `S0-decisions.md`. Мерж — владелец.
- Коммиты на русском, суть в первой строке. Перед PR: `cargo clippy --workspace --all-targets -- -D warnings`
  и `cargo test --workspace`. CI (`.github/workflows/ci.yml`, только Windows) собирает и выкладывает
  архив `Postman-<версия>-windows-x64.zip` в Actions (`scripts/windows/package.ps1`; подпись — если задан секрет).
- «Не падать»: вне тестов нет `unwrap`/`expect`/`panic!` (clippy `deny` на всю рабочую область); ошибка — `Result`,
  отравленный мьютекс — `PoisonError::into_inner`, сбой старта — `fatal` (сообщение и выход).
- Платформа: Windows — основа, другие ОС — по запросу. Код вне платформенных модулей не завязываем на Windows,
  чтобы их можно было вернуть.
- Бюджеты — свои, из ТЗ (`docs/HANDOFF.md` §2c), а не из базиса.

## Устройство

| Слой | Крейты | Правило |
|---|---|---|
| L0 | `ir` | HTML → безопасный IR, без I/O |
| L1 | `jmap`, `cache` (redb), `compose`, `calendar`, `search` (tantivy), `worker` | без UI-зависимостей |
| L2 | `core` (синхронизация, рендер через воркер, адресная книга, приглашения, замеры) | без UI-зависимостей |
| L3 | `app-gpui` — интерфейс, исполняемый файл `postman.exe` | только здесь gpui, winit, rfd (диалоги) |
| — | `demo` — синтетический ящик для демо и тестов | без UI; от него зависит только L3 |

Слои проверяет тест `crates/core/tests/layers.rs`; новый крейт — сначала в его таблицу.

- `worker` — отдельный процесс-песочница для разбора чужих данных (HTML, картинки, .ics).
  Windows: AppContainer + Job Object + mitigation policies, запуск без консоли (`DETACHED_PROCESS`).
  Linux (dev): seccomp + Landlock. Чужие данные в UI-процессе не разбираются никогда.
- Патч Windows-платформы gpui (шейдеры в exe, кадры по запросу, живой ресайз) — в репозитории `pets`
  (`crates/pets-gpui-windows`), подключён `[patch]` на ревизию; правки — там, затем поднять `rev` здесь.
  Патчи gpui держим минимальными, предлагаем upstream (Zed, longbridge), на новые версии переносим.
- Действия пользователя — команды из реестра (`app-gpui/src/commands.rs`, модель `pets-commands`): id `область.действие`
  → действие gpui `область::Действие` и хоткеи; новая клавиша или действие — только через таблицу команд;
  название — `cmd-…` в `app-gpui/i18n/{ru,en}/commands.ftl`; затем `POSTMAN_UPDATE_DOCS=1 cargo test -p postman-gpui parity`
  (`docs/PARITY.md`, чек-лист соответствия Outlook) в том же PR.
- Тексты интерфейса — только через Fluent (`pets-i18n`, `tr!`): ключи в `app-gpui/i18n/{ru,en}/<часть>.ftl` (файл на часть экрана, список — `TEXTS` в `main.rs`), оба языка;
  литерал с кириллицей в `app-gpui/src` ловит тест `no_raw_ui_strings`, расхождение наборов — `texts_are_consistent`.
  Журнал (`tracing`) — по-английски. Язык — ОС или `--lang ru|en`.
- Жизнь процесса — `pets-shell`: журнал (`POSTMAN_LOG`, ошибки и паники — ещё в журнал Windows «Приложение»),
  `fatal`, единственный экземпляр (второй запуск показывает окно первого; `--bench` и `--screens` — вне правила).
  Уведомления — тосты `pets-ui` (`Toasts` в `Mailer`), история — 🔔 в строке состояния.
- Экран — набор view со своим состоянием, встроенных через `.cached(...)` с явным размером; изменение —
  `cx.notify()` своего view, связь частей — событиями. Иначе gpui раскладывает всё окно на каждом кадре (S1-06).

## Запуск

```
cargo run --release -p postman-gpui -- --demo 50000     # демо-ящик; exe — target\release\postman.exe
... --bench                                             # замер: строка key=value в stdout
POSTMAN_URL=… POSTMAN_USER=… POSTMAN_PASSWORD=…         # реальный сервер (или POSTMAN_TOKEN)
cargo run --release -p postman-gpui                     # без окружения: сохранённый вход или строка входа; --sign-out — забыть
```

Release-сборка — без консоли (`windows_subsystem`): `--bench`, `--screens`, `--help` пишут в консоль родителя
или в перенаправление (`> файл`, канал). Git Bash и `.cmd` ждут выхода сами; в интерактивном cmd — `start /wait`,
в PowerShell — `| Out-Default`. Ресурсы exe (значок, версия, манифест) — `app-gpui/build.rs`, нужен rc.exe из Windows SDK.
Архив и установка — `scripts/windows/` (`package.ps1`, `install.ps1`, `README.txt`).

Windows: тестам песочницы нужен один раз на каталог сборки (из `cargo metadata`, учитывает `CARGO_TARGET_DIR`):
`icacls (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory /grant "*S-1-15-2-1:(OI)(CI)(RX)" /T /Q`.

### Снимки экранов

```
cargo run --release -p postman-gpui -- --screens <каталог>
```

Агенты проверяют интерфейс снимками, а не нажатиями в окне: демо-ящик (свежая копия на 2000 писем, удаляется
в конце; `--keep` — оставить), PNG через PrintWindow — `list`, `letter`, `attachments`, `invite`, `search`, `calendar`, `assistant`, `event`, `contacts`, `contact-edit`, `compose`, `send-later` (меню «Отправить позже», S5-06), `reply`, `meeting` (окна письма и встречи — свои окна), `vacation` (автоответ), `rules` (правила), `shared` (общий ящик в дереве папок, S4-06), `labels` (полосы категорий и метки, S5-02), `thread` (лента беседы, S5-05);
на каждый файл строка `screen <имя> <путь> <ш>x<в>` в stdout, выход 0; ошибка (в том числе снимок одного цвета) —
stderr и код 1. Только Windows.
`--screens <каталог> --live` — живой ящик из `POSTMAN_URL`… (стенд): `list`, `letter` и `reply` после первой синхронизации,
`calendar` — неделя с сервера, `contacts` — контакты и справочник с сервера, `vacation` и `rules` — автоответ и правила с сервера,
`shared` — «Входящие» первого общего ящика;
без учётной записи в окружении — `signin` (строка входа).

Диск (H-01): каталог сборки и скрипты — без зашитого `target/`; тесты — только `tempfile` с префиксом `postman-`;
данные бенча — `%LOCALAPPDATA%\Pets\testdata\postman` (рабочая копия удаляется, `--keep` — оставить); чистка —
`pets/scripts/stand/clean.ps1 -Repos . -Testdata` (без `-Apply` только показывает).
Стенд Stalwart: `cargo test -p postman-core --test stand -- --ignored` с `POSTMAN_STAND_URL` (напрямую по HTTPS — ещё
`POSTMAN_INSECURE_TLS=1`, самоподписанный сертификат стенда), `POSTMAN_STAND_USER1..3`,
`POSTMAN_STAND_PASSWORD1..3` в окружении (туннель и пароли — у владельца, `docs/STALWART.md`).
Диагностика песочницы (только отладочная сборка):
`cargo test -p postman-worker --test windows_probe -- --ignored --nocapture`.

## Не делать

- Не разбирать письма, вложения и приглашения вне воркера; не ослаблять песочницу без диагностики и ADR.
- Не писать секреты (пароли, токены) на диск.
- Не делать модальных окон: подтверждения — одной строкой в самом окне, уведомления — тостами.
- Не упоминать внутренние системы компании по имени: репозиторий публичный.

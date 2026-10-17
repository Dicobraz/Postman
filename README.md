# Postman

Корпоративный клиент почта + календарь + контакты под **Stalwart** по **JMAP**.
Чистый Rust, GPU-рендер (wgpu → DX12/Vulkan), раскладка и хоткеи classic Outlook.
Исходное ТЗ и решения — [docs/HANDOFF.md](docs/HANDOFF.md).

## Структура

| Крейт | Назначение |
|---|---|
| `crates/ir` | HTML (белый список) / text/plain → IR → Markdown-цитата / исходящий HTML. Без I/O, готов к запуску в песочнице. |
| `crates/jmap` | Тонкий JMAP-клиент: autodiscover `.well-known/jmap`, произвольные вызовы, Mail (ящики, письма, `*/changes`, флаги, перемещение). Свой, чтобы добавить Calendars/Principals. |
| `crates/cache` | SQLite-кэш (WAL): список окнами для виртуального скролла, тела лениво, состояния синхронизации. |
| `app` | egui/eframe (wgpu): папки, список, область чтения, хоткеи, фоновая синхронизация. |

## Запуск

```sh
cargo run --release -p postman -- --demo 50000          # синтетический ящик, без сервера
POSTMAN_URL=https://mail.example.com POSTMAN_USER=u POSTMAN_PASSWORD=p cargo run --release -p postman
# или POSTMAN_TOKEN=… (Bearer) вместо USER/PASSWORD
cargo test --workspace
```

Кэш: `%LOCALAPPDATA%\Postman\` (Windows), `$XDG_DATA_HOME/Postman` (Linux).

## Хоткеи

Ctrl+1/2/3 — почта/календарь/контакты · Ctrl+E — поиск · Ctrl+N/R/Shift+R/F — создать/ответить/всем/переслать ·
Ctrl+Q/U — прочитано/не прочитано · Insert — флаг · Del — в «Удалённые» · ↑↓ / j k, Home/End, PgUp/PgDn — навигация.

## Статус (неделя 1, спайк)

Готово: IR с тестами (вырезание активного содержимого, ссылки только http(s)/mailto + детект подмены и punycode,
внешние картинки не грузятся, вёрсточные таблицы линеаризуются, защита от глубокой вложенности);
JMAP-сессия, ящики, страницы писем, тела, флаги, перемещение, опрос `Email/changes`; SQLite-кэш;
UI-оболочка с виртуальным списком на 50 тыс. писем.

Дальше:
- Воркер-песочница для HTML→IR (сейчас — отдельный поток): AppContainer/Job Object/Win32k-off; seccomp/Landlock на Linux.
- Push (EventSource/WebSocket) вместо опроса; догрузка писем за пределами первых 500 на ящик; серверный поиск.
- Композер (Markdown → multipart/alternative), вложения.
- Рендер: настоящий полужирный шрифт (сейчас `strong` = контрастный цвет), выравнивание абзацев, группировка списка по датам, треды.
- Вопросы из HANDOFF §4 (capabilities Stalwart, availability/free-busy) — проверить на тестовом стенде.
- Замер RSS/FPS на Windows (DX12): в контейнере на программном Vulkan (lavapipe) RSS ≈ 185 МБ — не показатель.

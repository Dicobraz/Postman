//! Postman (`postman.exe`): интерфейс на gpui (gpui-kit). Ядро (хранилище, IR, воркер-песочница, демо-ящик) —
//! в крейтах ниже; здесь только интерфейс.
//!
//!   postman [--demo N] [--bench] [--screens <каталог> [--live]] [--sign-out] [--lang ru|en|auto] [--help] [--version]
//!
//! Без `--demo` — живой ящик: `POSTMAN_URL` + `POSTMAN_USER`/`POSTMAN_PASSWORD` (или `POSTMAN_TOKEN`), иначе
//! сохранённый вход, иначе строка входа в окне. `--sign-out` — забыть сохранённый вход.
//! Release-сборка — без консоли; вывод `--bench`, `--screens`, `--help` — в консоль родителя или в перенаправление.

// Release: без окна консоли (S4-01). Отладочная сборка — с консолью, журнал виден сразу.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod calendar;
mod commands;
mod ir_view;
#[cfg(test)]
mod keys_doc;
mod screens;
mod sys;
mod list;
mod folders;
mod reading;
mod attachments;
mod invite_card;
mod backend;
mod bar;
mod mailer;
mod account;
mod live;
mod signin;
mod compose;
mod actions;
mod meeting;
mod settings;
mod vacation;
mod contacts;
mod rules;
mod labels;
mod thread;
mod send_later;
mod roles;
mod tabs;
mod chrome;

pub(crate) use mailer::{Mailer, Section};

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::Cache;
use postman_core::bench::{self, Bench};

/// Замер кучи Rust (S4-08): `cargo build --release -p postman-gpui --features alloc-count`, поля `heap_…` в бенче.
#[cfg(feature = "alloc-count")]
#[global_allocator]
static ALLOC: sys::alloc::Counting = sys::alloc::Counting;

/// Тексты интерфейса (S2-06): ключи `.ftl`, RU и EN; `pets_ui::FTL` добавляется при запуске.
/// Файл на часть экрана (`i18n/<язык>/<часть>.ftl`): параллельные задачи не правят один файл.
pub const TEXTS: &[pets_i18n::Resource] = &[
    ("ru", include_str!("../i18n/ru/app.ftl")),
    ("ru", include_str!("../i18n/ru/commands.ftl")),
    ("ru", include_str!("../i18n/ru/reading.ftl")),
    ("ru", include_str!("../i18n/ru/compose.ftl")),
    ("ru", include_str!("../i18n/ru/account.ftl")),
    ("ru", include_str!("../i18n/ru/actions.ftl")),
    ("ru", include_str!("../i18n/ru/calendar.ftl")),
    ("ru", include_str!("../i18n/ru/settings.ftl")),
    ("ru", include_str!("../i18n/ru/contacts.ftl")),
    ("ru", include_str!("../i18n/ru/sync.ftl")),
    ("ru", include_str!("../i18n/ru/shared.ftl")),
    ("ru", include_str!("../i18n/ru/labels.ftl")),
    ("ru", include_str!("../i18n/ru/threads.ftl")),
    ("ru", include_str!("../i18n/ru/send.ftl")),
    ("ru", include_str!("../i18n/ru/frame.ftl")),
    ("en", include_str!("../i18n/en/app.ftl")),
    ("en", include_str!("../i18n/en/commands.ftl")),
    ("en", include_str!("../i18n/en/reading.ftl")),
    ("en", include_str!("../i18n/en/compose.ftl")),
    ("en", include_str!("../i18n/en/account.ftl")),
    ("en", include_str!("../i18n/en/actions.ftl")),
    ("en", include_str!("../i18n/en/calendar.ftl")),
    ("en", include_str!("../i18n/en/settings.ftl")),
    ("en", include_str!("../i18n/en/contacts.ftl")),
    ("en", include_str!("../i18n/en/sync.ftl")),
    ("en", include_str!("../i18n/en/shared.ftl")),
    ("en", include_str!("../i18n/en/labels.ftl")),
    ("en", include_str!("../i18n/en/threads.ftl")),
    ("en", include_str!("../i18n/en/send.ftl")),
    ("en", include_str!("../i18n/en/frame.ftl")),
];

pub(crate) fn format_date(ts: i64) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (Ok(t), now) = (jiff::Timestamp::from_second(ts), jiff::Zoned::now()) else { return String::new() };
    let t = t.to_zoned(tz);
    if t.date() == now.date() { t.strftime("%H:%M").to_string() } else { t.strftime("%d.%m %H:%M").to_string() }
}

pub(crate) fn data_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| ".".into())
        .join("Postman")
}

/// Запуск невозможен (нет каталога данных, хранилище или окно не открылись): сообщение в журнал
/// (и журнал Windows «Приложение») и выход с кодом 1, без паники.
fn fatal(what: &str, e: impl std::fmt::Display) -> ! {
    pets_shell::fatal(&tr!(what), e)
}

/// Второй запуск: показать окно первого и сказать об этом тостом.
fn listen_second_instance(listener: pets_shell::Listener, (window, mailer): (AnyWindowHandle, Entity<Mailer>), cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    listener.listen(move |args| {
        let _ = tx.unbounded_send(args);
    });
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while rx.next().await.is_some() {
            let _ = window.update(cx, |_, window, cx| {
                window.activate_window();
                mailer.update(cx, |m, cx| m.toasts.update(cx, |t, cx| t.push(Toast::Info, tr!("second-instance"), cx)));
            });
        }
    })
    .detach();
}

/// `--help`: консольный вывод — по-английски, как журнал.
const USAGE: &str = "Postman - mail, calendar and contacts for Stalwart (JMAP)

Usage: postman [options]
  --demo [N]            demo mailbox with N messages (default 50000), no server
  --lang ru|en|auto     interface language (default: from Windows)
  --theme light|dark|system
                        theme (default: system - light/dark and accent from Windows, live)
  --density compact|normal|comfortable
                        list row density (default: normal)
  --sign-out            forget the saved sign-in
  --bench               measure and exit: one key=value line on stdout
  --screens <dir> [--live] [--keep]
                        save screenshots (PNG) and exit: one 'screen <name> <path> <w>x<h>' line per file;
                        with --theme dark the files are <name>-dark.png
  --help, --version     this text, the version

Live mailbox: POSTMAN_URL and POSTMAN_USER/POSTMAN_PASSWORD (or POSTMAN_TOKEN); otherwise the saved sign-in.
Log: POSTMAN_LOG (e.g. debug); errors also go to the Windows Application log.
cmd waits for postman.exe only with 'start /wait'; PowerShell - with '| Out-Default'; Git Bash and .cmd files wait.
";

fn main() {
    let mut start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Release без консоли: вывод этих режимов — в консоль родителя (если поток не перенаправлен).
    if args.iter().any(|a| ["--bench", "--screens", "--help", "-h", "--version"].contains(&a.as_str())) {
        sys::attach_parent_console();
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return;
    }
    if args.iter().any(|a| a == "--version") {
        println!("postman {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let n = args.iter().position(|a| a == "--demo").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let bench = args.iter().any(|a| a == "--bench");
    let demo = args.iter().any(|a| a == "--demo");
    // `--screens --live`: снимки живого ящика (учётная запись — из окружения).
    let live = args.iter().any(|a| a == "--live");
    // `--keep`: оставить рабочую копию данных бенча (для разбора).
    let keep = args.iter().any(|a| a == "--keep");
    // `--bench --live --warm`: замер живого ящика на кэше прошлого прогона (`--keep`), без — с первой синхронизации.
    let warm = args.iter().any(|a| a == "--warm");
    if bench {
        bench::on_idle(sys::bench_idle_fields);
    }
    let lang = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).map_or("auto", String::as_str);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str);
    let theme = roles::choice(value("--theme"));
    let density = roles::density(value("--density"));
    pets_shell::init_logging("Postman", "POSTMAN_LOG");
    for p in pets_i18n::init(&[TEXTS, pets_ui::FTL], lang) {
        tracing::warn!("i18n: {p}");
    }
    let mut screens = args.iter().position(|a| a == "--screens").and_then(|i| args.get(i + 1)).map(|d| {
        let suffix = if theme == pets_ui::theme::FLUENT_DARK { "-dark" } else { "" };
        screens::Driver::new(d.into(), live, suffix).unwrap_or_else(|e| fatal("fatal-screens-dir", e))
    });
    // Второй запуск отдаёт аргументы первому (тот показывает окно) и выходит; замеры и снимки — всегда отдельно.
    let listener = if bench || screens.is_some() {
        None
    } else {
        match pets_shell::single_instance("Postman", &args) {
            Ok(pets_shell::Instance::First(l)) => Some(l),
            Ok(pets_shell::Instance::Second) => return,
            Err(e) => {
                tracing::warn!("single instance: {e}");
                None
            }
        }
    };
    let dir = data_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fatal("fatal-data-dir", e));
    if args.iter().any(|a| a == "--sign-out") {
        account::forget(&dir);
    }
    let flags = account::Flags { demo, bench, screens: screens.is_some(), live };
    let start_with = account::choose(flags, postman_core::sync::Account::from_env(), || account::load_stored(&dir));
    if bench && live && !matches!(start_with, account::Start::Live(_)) {
        fatal("fatal-bench-account", "POSTMAN_URL");
    }
    if let (Some(d), account::Start::SignIn(_)) = (&mut screens, &start_with) {
        d.sign_in();
    }
    // Живой ящик: кэш учётной записи; нет учётной записи — пустой кэш в памяти до входа.
    let (cache, index) = match &start_with {
        account::Start::Demo => (demo_cache(&dir, bench, screens.is_some(), keep, n, &mut start), None),
        account::Start::Live(_) if bench => live::bench_store(warm, keep, &mut start).unwrap_or_else(|e| fatal("fatal-store", e)),
        account::Start::Live(a) => live::open_store(&account::cache_path(&dir, a)).unwrap_or_else(|e| fatal("fatal-store", e)),
        account::Start::SignIn(_) => (Cache::open_in_memory().unwrap_or_else(|e| fatal("fatal-store", e)), None),
    };

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // Тема (S5-01): «как в ОС» или `--theme`, на лету; до первого окна — чтение реестра, микросекунды.
        roles::init(theme, density, cx);
        // Клавиши — из реестра команд (S2-02); в поле ввода клавиши без модификаторов — текст.
        commands::bind(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        // Свой заголовок окна (S5-01): вкладки, перетаскивание, «─ □ ✕» — `chrome.rs`.
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Postman".into()), appears_transparent: true, ..Default::default() }),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            roles::observe_window(window);
            let bench = bench.then(|| if live { Bench::live(start) } else { Bench::new(start) });
            let cache = cache.clone();
            cx.new(|cx| {
                let mut m = Mailer { screens, ..Mailer::new(cache, bench, window, cx) };
                match start_with {
                    account::Start::Demo => {}
                    account::Start::Live(a) => m.start_sync(a, index, false, cx),
                    account::Start::SignIn(prefill) => m.show_signin(prefill, window, cx),
                }
                m
            })
        });
        match opened {
            Ok(handle) => {
                if let Some(l) = listener {
                    listen_second_instance(l, handle, cx);
                }
            }
            Err(e) => fatal("fatal-window", e),
        }
        cx.activate(true);
    });
}

/// Демо-ящик. Бенч и снимки — на свежей копии; подготовка данных не входит в first_frame_ms.
fn demo_cache(dir: &std::path::Path, bench: bool, screens: bool, keep: bool, n: usize, start: &mut Instant) -> Cache {
    let prepared = Instant::now();
    let path = if bench || screens {
        // Снимки — на небольшой свежей копии: письма в первой строке те же от прогона к прогону.
        let n = if screens { 2000 } else { n };
        let path = postman_demo::bench_copy(&postman_demo::testdata_dir(), n).unwrap_or_else(|e| fatal("fatal-bench-mailbox", e));
        if !keep {
            bench::remove_on_finish(path.clone());
        }
        path
    } else {
        let path = dir.join("demo.redb");
        postman_demo::ensure(&path, n).unwrap_or_else(|e| fatal("fatal-demo-mailbox", e));
        path
    };
    if bench {
        *start += prepared.elapsed();
    }
    Cache::open(&path).unwrap_or_else(|e| fatal("fatal-store", e))
}

#[cfg(test)]
mod tests {
    /// Наборы текстов согласованы: оба языка, одни ключи и переменные, без дублей.
    #[test]
    fn texts_are_consistent() {
        assert_eq!(pets_i18n::check(&[super::TEXTS, pets_ui::FTL]), vec![]);
    }

    /// Строки интерфейса — только через Fluent: в коде экранов нет литералов с кириллицей.
    #[test]
    fn no_raw_ui_strings() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for r in pets_i18n::raw_strings(&source) {
                    found.push(format!("{}:{}: {}", path.display(), r.line, r.text));
                }
            }
        }
        assert!(found.is_empty(), "сырые строки, перенести в i18n/<язык>/*.ftl:\n{}", found.join("\n"));
    }

    /// Цвета — только ролями темы (S5-01, базис Pets): литералы и именованные цвета gpui — лишь в `roles.rs`.
    #[test]
    fn no_color_literals() {
        const CALLS: [&str; 9] = ["rgb(", "rgba(", "hsla(", "white()", "black()", "red()", "green()", "blue()", "yellow()"];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.file_name().is_some_and(|n| n == "roles.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for (n, line) in source.lines().enumerate() {
                for call in CALLS {
                    let hit = line.match_indices(call).any(|(i, _)| {
                        !line[..i].ends_with(|c: char| c.is_alphanumeric() || "_:.\"".contains(c))
                    });
                    if hit {
                        found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "цвет литералом, взять роль темы (`roles.rs`):\n{}", found.join("\n"));
    }
}

//! Спайк UI на gpui (gpui-kit): тот же экран, что и в egui-версии, и тот же `--bench`.
//! Ядро (хранилище, IR, воркер-песочница, демо-ящик) — общее; здесь только интерфейс.
//!
//!   postman-gpui [--demo N] [--bench] [--screens <каталог>] [--lang ru|en|auto]

mod calendar;
mod commands;
mod ir_view;
mod screens;
mod sys;
mod list;
mod folders;
mod reading;
mod backend;
mod bar;
mod mailer;

pub(crate) use mailer::{Mailer, Section};

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::Cache;
use postman_core::bench::{self, Bench};


/// Тексты интерфейса (S2-06): ключи `.ftl`, RU и EN; `pets_ui::FTL` добавляется при запуске.
/// Файл на часть экрана (`i18n/<язык>/<часть>.ftl`): параллельные задачи не правят один файл.
pub const TEXTS: &[pets_i18n::Resource] = &[
    ("ru", include_str!("../i18n/ru/app.ftl")),
    ("ru", include_str!("../i18n/ru/commands.ftl")),
    ("en", include_str!("../i18n/en/app.ftl")),
    ("en", include_str!("../i18n/en/commands.ftl")),
];

pub(crate) fn format_date(ts: i64) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (Ok(t), now) = (jiff::Timestamp::from_second(ts), jiff::Zoned::now()) else { return String::new() };
    let t = t.to_zoned(tz);
    if t.date() == now.date() { t.strftime("%H:%M").to_string() } else { t.strftime("%d.%m %H:%M").to_string() }
}

fn data_dir() -> std::path::PathBuf {
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

fn main() {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n = args.iter().position(|a| a == "--demo").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let bench = args.iter().any(|a| a == "--bench");
    // `--keep`: оставить рабочую копию данных бенча (для разбора).
    let keep = args.iter().any(|a| a == "--keep");
    let lang = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).map_or("auto", String::as_str);
    pets_shell::init_logging("Postman", "POSTMAN_LOG");
    for p in pets_i18n::init(&[TEXTS, pets_ui::FTL], lang) {
        tracing::warn!("i18n: {p}");
    }
    let screens = args.iter().position(|a| a == "--screens").and_then(|i| args.get(i + 1)).map(|d| {
        screens::Driver::new(d.into()).unwrap_or_else(|e| fatal("fatal-screens-dir", e))
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
    // Бенч — на свежей копии ящика; подготовка данных не входит в first_frame_ms.
    let prepared = Instant::now();
    let path = if bench || screens.is_some() {
        // Снимки — на небольшой свежей копии: письма в первой строке те же от прогона к прогону.
        let n = if screens.is_some() { 2000 } else { n };
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
    let start = if bench { start + prepared.elapsed() } else { start };
    let cache = Cache::open(&path).unwrap_or_else(|e| fatal("fatal-store", e));

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // Клавиши — из реестра команд (S2-02); в поле ввода клавиши без модификаторов — текст.
        commands::bind(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            let bench = bench.then(|| Bench::new(start));
            let cache = cache.clone();
            cx.new(|cx| Mailer { screens, ..Mailer::new(cache, bench, window, cx) })
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
}

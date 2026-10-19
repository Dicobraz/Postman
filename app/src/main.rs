//! Postman — спайк недели 1: вход по JMAP, папки, список писем, чтение.
//!
//! Запуск:
//!   postman --demo [N]           — локальный ящик из N синтетических писем (по умолчанию 50 000)
//!   POSTMAN_URL=https://mail.example.com POSTMAN_USER=… POSTMAN_PASSWORD=… postman
//!   (или POSTMAN_TOKEN=… вместо пользователя/пароля)
//!
//! Опции: --gpu dx12|vulkan|gl (или POSTMAN_GPU), --bench (замер и выход),
//! --no-vsync (кадры без ожидания развёртки; в бенче по умолчанию vsync, как у gpui),
//! POSTMAN_GPU_SOFTWARE=1 — разрешить программный адаптер (CI, контейнеры).

mod app;
mod composer;
mod dialogs;
mod gpu;
mod invite_card;
mod ir_view;

use std::path::PathBuf;

use postman_core::{bench, extract, files, indexer, render, sync};
use std::time::Instant;

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Postman")
}

/// Запуск невозможен (нет каталога данных, хранилище или окно не открылись): сообщение и выход с кодом 1,
/// без паники. В журнал Windows — с S2-06 (pets-shell).
pub(crate) fn fatal(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("Postman не запустился: {what}: {e}");
    std::process::exit(1)
}

fn main() -> eframe::Result {
    let mut start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let api = gpu::Api::from_args(&args).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let bench = args.iter().any(|a| a == "--bench");
    let no_vsync = args.iter().any(|a| a == "--no-vsync");
    let software = std::env::var_os("POSTMAN_GPU_SOFTWARE").is_some();
    let demo = args.iter().position(|a| a == "--demo").map(|i| {
        args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(50_000usize)
    });
    let dir = data_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fatal("каталог данных", e));

    let (db_path, account) = match demo {
        Some(n) => {
            // Бенч — на свежей копии ящика; подготовка данных не входит в first_frame_ms.
            let path = if bench {
                let prepared = Instant::now();
                let path = postman_demo::bench_copy(&dir, n).unwrap_or_else(|e| fatal("ящик для замера", e));
                start += prepared.elapsed();
                path
            } else {
                let path = dir.join("demo.redb");
                postman_demo::ensure(&path, n).unwrap_or_else(|e| fatal("демо-кэш", e));
                path
            };
            (path, None)
        }
        None => (dir.join("cache.redb"), sync::Account::from_env()),
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Postman")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: gpu::configuration(api, software, no_vsync),
        ..Default::default()
    };
    eframe::run_native(
        "Postman",
        options,
        Box::new(move |cc| {
            let bench = bench.then(|| bench::Bench::new(start));
            Ok(Box::new(app::App::new(cc, db_path, account, demo.is_some(), bench)))
        }),
    )
}

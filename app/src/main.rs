//! Postman — спайк недели 1: вход по JMAP, папки, список писем, чтение.
//!
//! Запуск:
//!   postman --demo [N]           — локальный ящик из N синтетических писем (по умолчанию 50 000)
//!   POSTMAN_URL=https://mail.example.com POSTMAN_USER=… POSTMAN_PASSWORD=… postman
//!   (или POSTMAN_TOKEN=… вместо пользователя/пароля)
//!
//! Опции: --gpu dx12|vulkan|gl (или POSTMAN_GPU), --bench (замер и выход),
//! POSTMAN_GPU_SOFTWARE=1 — разрешить программный адаптер (CI, контейнеры).

mod app;
mod bench;
mod composer;
mod demo;
mod files;
mod gpu;
mod ir_view;
mod render;
mod sync;

use std::path::PathBuf;
use std::time::Instant;

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Postman")
}

fn main() -> eframe::Result {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let api = gpu::Api::from_args(&args).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let bench = args.iter().any(|a| a == "--bench");
    let software = std::env::var_os("POSTMAN_GPU_SOFTWARE").is_some();
    let demo = args.iter().position(|a| a == "--demo").map(|i| {
        args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(50_000usize)
    });
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("каталог данных");

    let (db_path, account) = match demo {
        Some(n) => {
            let path = dir.join("demo.sqlite");
            demo::ensure(&path, n).expect("демо-кэш");
            (path, None)
        }
        None => (dir.join("cache.sqlite"), sync::Account::from_env()),
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Postman")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: gpu::configuration(api, software, bench),
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

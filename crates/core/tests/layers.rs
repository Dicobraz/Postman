//! Слои рабочей области (S2-01, `docs/specs/S2.md`): ниже интерфейса нет UI-зависимостей,
//! слой ниже не зависит от слоя выше, у каждого крейта есть слой. Проверка по `cargo tree`
//! (обычные зависимости, по Cargo.lock, без сети).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // интеграционный тест: «не падать» — для кода приложения (S2-03)

use std::collections::BTreeSet;
use std::process::Command;

/// Крейт → слой. L0 `ir`; L1 — без UI; L2 `core`; L3 — интерфейс. `demo` — данные для демо и тестов:
/// без UI, от него зависит только интерфейс.
const LAYERS: &[(&str, u8)] = &[
    ("postman-ir", 0),
    ("postman-smime", 1),
    ("postman-jmap", 1),
    ("postman-imap", 1),
    ("postman-dav", 1),
    ("postman-cache", 1),
    ("postman-compose", 1),
    ("postman-calendar", 1),
    ("postman-search", 1),
    ("postman-worker", 1),
    ("postman-core", 2),
    ("postman-demo", 2),
    ("postman-gpui", 3),
];
const UI_LAYER: u8 = 3;

/// Библиотеки интерфейса и системных диалогов: только в L3.
const UI_CRATES: &[&str] = &["gpui", "egui", "eframe", "winit", "rfd"];

/// Протокольные клиенты серверов (ADR 0003): их знает только `core`, интерфейс видит нейтральную модель
/// (`postman_core::model`). Новый путь к серверу — сюда.
const PROTOCOL_CRATES: &[&str] = &["postman-jmap", "postman-imap", "postman-dav", "postman-ews"];

fn cargo_tree(args: &[&str]) -> Vec<String> {
    let out = Command::new(env!("CARGO"))
        .args(["tree", "--offline", "--prefix", "none", "-e", "normal"])
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree");
    assert!(out.status.success(), "cargo tree {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

fn layer(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|&(_, l)| l)
}

fn is_ui(name: &str) -> bool {
    UI_CRATES.iter().any(|u| name == *u || name.starts_with(&format!("{u}-")) || name.starts_with(&format!("{u}_")))
}

#[test]
fn every_crate_has_a_layer() {
    let members: BTreeSet<String> = cargo_tree(&["--workspace", "--depth", "0"]).into_iter().collect();
    let missing: Vec<_> = members.iter().filter(|m| layer(m).is_none()).collect();
    assert!(missing.is_empty(), "крейты без слоя — добавьте в LAYERS и в docs/specs/S2.md: {missing:?}");
}

#[test]
fn no_ui_dependencies_below_the_interface() {
    for &(name, l) in LAYERS.iter().filter(|(_, l)| *l < UI_LAYER) {
        let ui: BTreeSet<String> = cargo_tree(&["-p", name]).into_iter().filter(|d| is_ui(d)).collect();
        assert!(ui.is_empty(), "{name} (L{l}) зависит от библиотек интерфейса: {ui:?}");
    }
}

#[test]
fn lower_layers_do_not_depend_on_higher() {
    for &(name, l) in LAYERS {
        for dep in cargo_tree(&["-p", name, "--depth", "1"]).iter().skip(1) {
            let Some(dl) = layer(dep) else { continue };
            assert!(dl <= l, "{name} (L{l}) зависит от {dep} (L{dl})");
            assert!(dep != "postman-demo" || l == UI_LAYER, "{name} (L{l}) зависит от демо-данных");
        }
    }
}

#[test]
fn interface_does_not_know_protocols() {
    for &(name, _) in LAYERS.iter().filter(|(_, l)| *l == UI_LAYER) {
        let deps: BTreeSet<String> = cargo_tree(&["-p", name, "--depth", "1"]).into_iter().collect();
        let known: Vec<_> = PROTOCOL_CRATES.iter().filter(|p| deps.contains(**p)).collect();
        assert!(known.is_empty(), "{name} зависит от протокольных крейтов (только через postman-core): {known:?}");
    }
}

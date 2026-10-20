//! Слой приложения без UI: всё, что одинаково нужно egui- и gpui-интерфейсу.
//! Интерфейс шлёт команды и получает события; данные читает из хранилища.

pub mod bench;
pub mod contacts;
pub mod extract;
pub mod files;
pub mod indexer;
pub mod invite;
pub mod render;
pub mod rules;
pub mod schedule;
pub mod settings;
pub mod sync;

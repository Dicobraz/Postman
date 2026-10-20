//! Слой приложения без UI: всё, что нужно интерфейсу, кроме самого интерфейса.
//! Интерфейс шлёт команды и получает события; данные читает из хранилища.

pub mod bench;
pub mod contacts;
pub mod extract;
pub mod files;
pub mod indexer;
pub mod invite;
pub mod labels;
pub mod palette;
pub mod render;
pub mod rules;
pub mod schedule;
pub mod settings;
pub mod sync;
pub mod thread;

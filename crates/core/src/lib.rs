//! Слой приложения без UI: всё, что нужно интерфейсу, кроме самого интерфейса.
//! Интерфейс шлёт команды и получает события; данные читает из хранилища.

pub mod attach;
pub mod bulk;
pub mod bench;
pub mod categories;
pub mod checks;
pub mod contacts;
pub mod extract;
pub mod files;
pub mod filters;
pub mod fuzzy;
pub mod indexer;
pub mod invite;
pub mod labels;
pub mod lifecycle;
pub mod link;
pub mod marks;
pub mod palette;
pub mod prefs;
pub mod query;
pub mod render;
pub mod rules;
pub mod schedule;
pub mod send_later;
pub mod settings;
pub mod sync;
pub mod thread;

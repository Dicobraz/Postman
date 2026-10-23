//! Путь JMAP (Stalwart) — плагин ядра (ADR 0004, S7-02б): синхронизация учётной записи по JMAP тем же договором
//! команд и событий (`postman_core::sync`), что у остальных путей. Почта, календарь, задачи, занятость, контакты,
//! справочник и то, что есть только у JMAP: правила (Sieve), автоответ, квота, таблица категорий (FileNode), общие
//! ящики, «Отправить позже» сервером. Что умеет сервер — по capabilities сессии (`Event::Features`).
//!
//! Перевод типов `postman_jmap` ↔ модель — [`convert`]; общий код (кэш, задачи, правила, автоответ) — `postman_core`.

pub mod categories;
pub mod convert;
pub mod error;
pub mod invite;
pub mod schedule;
pub mod settings;
pub mod sync;
pub mod tasks;

use postman_core::backend::{BoxFuture, MailPlugin, Spawn};
use postman_core::sync::{Command, UnboundedSender};

pub use sync::{spawn, spawn_as};

/// Плагин пути JMAP для реестра (`postman_core::backend::Registry`): путь по умолчанию — адрес без своей схемы.
#[derive(Debug, Default, Clone, Copy)]
pub struct JmapPlugin;

impl MailPlugin for JmapPlugin {
    fn name(&self) -> &'static str {
        "jmap"
    }

    /// Есть ли на узле JMAP (`/.well-known/jmap`, S7-04в).
    fn probe<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { postman_jmap::has_jmap(url).await.then(|| url.to_owned()) })
    }

    fn set_tls13_only(&self, on: bool) {
        postman_jmap::set_tls13_only(on);
    }

    fn spawn(&self, s: Spawn) -> UnboundedSender<Command> {
        sync::spawn_as(s.account, s.profile, s.cache, s.index, s.events, s.repaint)
    }
}

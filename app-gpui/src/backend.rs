//! Связь экрана с сервером (шов фазы 2, `docs/specs/S3.md`): живой ящик — поток синхронизации
//! `postman_core::sync` (подключает S3-04), демо — без сервера.
//!
//! Части экрана не держат свой канал к серверу: команды уходят через [`Backend::send`], события синхронизации
//! `Mailer` раздаёт всем частям (`on_sync`) — каждая берёт свои (тело письма, блоб, итог отправки…).
#![expect(dead_code, reason = "phase 2 seam, wired by S3-01, S3-03, S3-04")]

use postman_core::sync::{Command, UnboundedSender};

/// Откуда экран берёт письма и куда отдаёт действия.
pub(crate) enum Backend {
    /// Демо-ящик (`--demo`, бенч, снимки): сервера нет, действия с сервером — тост «нужен сервер».
    Demo,
    /// Учётная запись на сервере: команды — в поток синхронизации.
    Live { commands: UnboundedSender<Command> },
}

impl Backend {
    pub(crate) fn is_live(&self) -> bool {
        matches!(self, Backend::Live { .. })
    }

    /// Команда синхронизации. `false` — сервера нет (демо) или поток синхронизации остановился:
    /// вызывающий говорит об этом тостом и не теряет данные пользователя (черновик остаётся в окне).
    pub(crate) fn send(&self, command: Command) -> bool {
        match self {
            Backend::Demo => false,
            Backend::Live { commands } => commands.send(command).is_ok(),
        }
    }
}

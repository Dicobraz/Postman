//! Платформенная изоляция: `spawn` — на стороне UI, `lockdown` — в самом воркере.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{lockdown, spawn};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{lockdown, spawn};

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn lockdown() -> Result<(), String> {
    Err("платформа не поддерживается".into())
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn spawn(_: &std::path::Path, _relaxed: bool) -> std::io::Result<crate::Process> {
    Err(std::io::Error::other("платформа не поддерживается"))
}

/// Что удалось включить при запуске воркера (для статус-строки и логов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    /// Все механизмы платформы.
    Full,
    /// Часть механизмов недоступна (например, нет AppContainer-профиля или user namespaces).
    Partial,
}

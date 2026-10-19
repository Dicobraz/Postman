//! Системные диалоги выбора файлов — слой интерфейса (S2-01): `core` получает уже выбранный путь.

use std::io;
use std::path::PathBuf;

use postman_core::files;

/// Диалог «Сохранить как…». `Ok(None)` — пользователь отменил.
pub fn save_as(name: &str, data: &[u8]) -> io::Result<Option<PathBuf>> {
    let Some(path) = rfd::FileDialog::new().set_file_name(files::safe_name(name)).save_file() else { return Ok(None) };
    files::write(&path, data)?;
    Ok(Some(path))
}

/// Выбор файлов для вложения.
pub fn pick() -> Vec<PathBuf> {
    rfd::FileDialog::new().pick_files().unwrap_or_default()
}

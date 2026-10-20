//! Платформенный код интерфейса; `unsafe` — только здесь.

use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
mod console;

/// Вывод в консоль родителя, если stdout или stderr не перенаправлены (release без консоли, S4-01):
/// для `--bench`, `--screens`, `--help`, `--version`.
#[cfg(windows)]
pub fn attach_parent_console() {
    console::attach_parent();
}

#[cfg(not(windows))]
pub fn attach_parent_console() {}

/// Кодовая страница ANSI процесса (65001 — манифест exe действует).
#[cfg(all(test, windows))]
pub fn active_code_page() -> u32 {
    console::active_code_page()
}

/// Пароль учётной записи — в диспетчере учётных данных Windows (S3-04).
#[cfg(windows)]
pub mod credentials;

/// Другие ОС: хранилища секретов нет — вход только из окружения или каждый раз строкой входа.
#[cfg(not(windows))]
pub mod credentials {
    pub fn write(_target: &str, _user: &str, _secret: &str) -> Result<(), String> {
        Err("credential store is Windows only".into())
    }

    pub fn read(_target: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub fn delete(_target: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Снимок окна: RGBA, 8 бит на канал, строки сверху вниз.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Дескриптор окна числом — его можно передать в другой поток (сам `Window` туда не передаётся).
pub fn window_id(window: &Window) -> Result<isize, String> {
    match HasWindowHandle::window_handle(window).map_err(|e| format!("window handle: {e}"))?.as_raw() {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get()),
        other => Err(format!("window capture is not supported for {other:?}")),
    }
}

/// Снимок клиентской области окна. Вызывать не из потока окна.
#[cfg(windows)]
pub fn capture(window: isize) -> Result<Image, String> {
    windows::capture(window)
}

#[cfg(not(windows))]
pub fn capture(_window: isize) -> Result<Image, String> {
    Err("window capture is Windows only".into())
}

#[cfg(all(test, windows))]
mod tests {
    /// Манифест из build.rs действует, хотя gpui вшивает свой: кодовая страница процесса — UTF-8.
    #[test]
    fn manifest_is_applied() {
        assert_eq!(super::active_code_page(), 65001);
    }
}

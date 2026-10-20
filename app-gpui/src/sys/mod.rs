//! Платформенный код интерфейса; `unsafe` — только здесь.

use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(windows)]
mod windows;

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

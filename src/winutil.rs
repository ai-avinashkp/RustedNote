//! Small Win32 shim for what GPUI 0.2.2 can't do on Windows with a custom
//! (transparent) titlebar: drag the window, and maximize/restore it.
//!
//! Maximize/restore is done by POSTing `WM_SYSCOMMAND` (`SC_MAXIMIZE` /
//! `SC_RESTORE`). gpui passes `WM_SYSCOMMAND` through to `DefWindowProc`, so
//! this is the *exact* native path the taskbar button and Win+Up use — which
//! renders the frameless maximize correctly (titlebar + buttons stay visible).
//! We `PostMessageW` (queued) rather than `SendMessageW` (synchronous), so it's
//! processed in the normal message loop just like an OS-initiated maximize.
//!
//! On non-Windows platforms these fall back to gpui's own methods.

use gpui::Window;

#[cfg(windows)]
mod imp {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        PostMessageW, ReleaseCapture, SendMessageW, HTCAPTION, SC_MAXIMIZE, SC_RESTORE,
        WM_NCLBUTTONDOWN, WM_SYSCOMMAND,
    };

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn start_drag(window: &mut Window) {
        if let Some(h) = hwnd(window) {
            unsafe {
                ReleaseCapture();
                SendMessageW(h, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
        }
    }

    pub fn maximize(window: &mut Window) {
        if let Some(h) = hwnd(window) {
            unsafe {
                // gpui SetCapture()s on every mouse-down; system commands are
                // ignored while capture is held, so release it first.
                ReleaseCapture();
                PostMessageW(h, WM_SYSCOMMAND, SC_MAXIMIZE as usize, 0);
            }
        }
    }

    pub fn restore(window: &mut Window) {
        if let Some(h) = hwnd(window) {
            unsafe {
                ReleaseCapture();
                PostMessageW(h, WM_SYSCOMMAND, SC_RESTORE as usize, 0);
            }
        }
    }
}

#[cfg(windows)]
pub fn start_drag(window: &mut Window) {
    imp::start_drag(window);
}

#[cfg(windows)]
pub fn maximize(window: &mut Window) {
    imp::maximize(window);
}

#[cfg(windows)]
pub fn restore(window: &mut Window) {
    imp::restore(window);
}

#[cfg(not(windows))]
pub fn start_drag(window: &mut Window) {
    window.start_window_move();
}

#[cfg(not(windows))]
pub fn maximize(window: &mut Window) {
    window.zoom_window();
}

#[cfg(not(windows))]
pub fn restore(window: &mut Window) {
    window.zoom_window();
}

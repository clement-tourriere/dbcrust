//! Hiding and showing the main window without destroying it, through the
//! window's native handle: GPUI has no API for it.
//!
//! - macOS: the `NSWindow` behind GPUI's `NSView` is ordered out / back
//!   front. This is per window, unlike `App::hide()`, which hides the whole
//!   application (⌘H) and would leave the Dock icon as the only way back.
//! - Windows: `ShowWindowAsync(SW_HIDE)` / `SW_SHOW` on the window's `HWND`.
//!   The async variant posts the request instead of re-entering GPUI's window
//!   procedure while it is dispatching the close message.
//! - Anywhere else a window cannot be hidden ([`CAN_HIDE`] is `false`) and
//!   both functions do nothing.

use gpui_kit::Window;
#[cfg_attr(test, allow(unused_imports))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// This build can hide a window and bring it back (tests pretend to).
pub const CAN_HIDE: bool = cfg!(any(test, target_os = "macos", target_os = "windows"));

#[cfg_attr(test, allow(dead_code))]
fn raw_handle(window: &Window) -> Option<RawWindowHandle> {
    // `Window` also has an inherent `window_handle` (GPUI's own handle type)
    HasWindowHandle::window_handle(window)
        .ok()
        .map(|handle| handle.as_raw())
}

/// Hide `window`. Returns whether the platform accepted the request.
pub fn hide(window: &Window) -> bool {
    #[cfg(test)]
    {
        let _ = window;
        fake::set_visible(false)
    }
    #[cfg(not(test))]
    raw_handle(window).is_some_and(imp::hide)
}

/// Un-hide `window`, restore it if minimized, and put it in front.
pub fn show(window: &Window) -> bool {
    #[cfg(test)]
    {
        let _ = window;
        fake::set_visible(true)
    }
    #[cfg(not(test))]
    raw_handle(window).is_some_and(imp::show)
}

/// The test platform has no native window: record what would have happened.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::Cell;

    thread_local! {
        static VISIBLE: Cell<bool> = const { Cell::new(true) };
    }

    pub fn set_visible(visible: bool) -> bool {
        VISIBLE.set(visible);
        true
    }

    pub fn is_visible() -> bool {
        VISIBLE.get()
    }
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod imp {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSView, NSWindow};
    use raw_window_handle::RawWindowHandle;

    fn with_window(handle: RawWindowHandle, act: impl FnOnce(&NSWindow)) -> bool {
        let RawWindowHandle::AppKit(handle) = handle else {
            return false;
        };
        if MainThreadMarker::new().is_none() {
            return false;
        }
        // SAFETY: the handle was just taken from a live GPUI window and points
        // at its `NSView`; we are on the main thread, where GPUI owns it.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let Some(window) = view.window() else {
            return false;
        };
        act(&window);
        true
    }

    pub fn hide(handle: RawWindowHandle) -> bool {
        with_window(handle, |window| window.orderOut(None))
    }

    pub fn show(handle: RawWindowHandle) -> bool {
        with_window(handle, |window| {
            if window.isMiniaturized() {
                window.deminiaturize(None);
            }
            window.makeKeyAndOrderFront(None);
        })
    }
}

#[cfg(target_os = "windows")]
#[cfg_attr(test, allow(dead_code))]
mod imp {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindowAsync,
    };

    fn hwnd(handle: RawWindowHandle) -> Option<HWND> {
        match handle {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn hide(handle: RawWindowHandle) -> bool {
        let Some(hwnd) = hwnd(handle) else {
            return false;
        };
        // SAFETY: `hwnd` is the handle of GPUI's live main window.
        unsafe { ShowWindowAsync(hwnd, SW_HIDE) != 0 }
    }

    pub fn show(handle: RawWindowHandle) -> bool {
        let Some(hwnd) = hwnd(handle) else {
            return false;
        };
        // SAFETY: as above. The show request is queued; GPUI's own
        // `activate_window` (queued after it) then focuses the window.
        unsafe {
            let command = if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            let queued = ShowWindowAsync(hwnd, command) != 0;
            SetForegroundWindow(hwnd);
            queued
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[cfg_attr(test, allow(dead_code))]
mod imp {
    use raw_window_handle::RawWindowHandle;

    pub fn hide(_: RawWindowHandle) -> bool {
        false
    }

    pub fn show(_: RawWindowHandle) -> bool {
        false
    }
}

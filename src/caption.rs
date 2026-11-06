//! The window's handle, its dark frame and its cloak. The widget has no
//! caption, and the theme is always dark; the About window's caption is
//! made dark while that window is still hidden (`ui::OwnWindow`), so that
//! Windows 10 shows it dark from the start.

/// The window's handle, if it is a Win32 window. `eframe::CreationContext`
/// and `eframe::Frame` both implement `HasWindowHandle`.
pub fn hwnd(window: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attribute: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
}

/// Give window `hwnd` the dark caption and frame
/// (`DWMWA_USE_IMMERSIVE_DARK_MODE`) or the light ones.
pub fn set_dark(hwnd: isize, dark: bool) {
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    // The attribute's number before Windows 10 20H1 (1809 to 1909).
    const DWMWA_USE_IMMERSIVE_DARK_MODE_OLD: u32 = 19;
    let value = i32::from(dark);
    let set = |attribute| unsafe {
        DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast(), 4) >= 0
    };
    if !set(DWMWA_USE_IMMERSIVE_DARK_MODE) {
        set(DWMWA_USE_IMMERSIVE_DARK_MODE_OLD);
    }
}

/// Hide window `hwnd` from the screen while it keeps working and painting
/// (`DWMWA_CLOAK`), or show it again. A window shown before its first
/// OpenGL frame reaches DWM flashes white: `App::new` cloaks the window
/// until it has painted a few frames (`App::uncloak`), and the About
/// window is cloaked the same way once found (SKILL.md 3.10).
pub fn cloak(hwnd: isize, on: bool) {
    const DWMWA_CLOAK: u32 = 13;
    let value = i32::from(on);
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, (&raw const value).cast(), 4) };
}

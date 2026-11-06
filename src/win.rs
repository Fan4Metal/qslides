//! Win32 helpers: those shared by the apps of this family (a message box
//! that needs no window, the parent console for the command line modes,
//! the shell's "open", Windows' double-click time, an HTTPS GET through
//! WinHTTP for the update check) and the widget's own: the window's
//! subclass (the tray icon's messages, the proportions kept while the
//! window is sized), its place on the screen, the taskbar button, the Run
//! key for starting with Windows.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};

/// A NUL-terminated UTF-16 string for the Win32 `W` functions.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Show `text` in a system message box with an error icon, waiting until
/// it is closed. It needs no window of the app, so it works from any thread
/// and while the app is failing (the panic hook). user32 is declared by
/// hand, to spare a `windows-sys` feature.
pub fn error_box(title: &str, text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x10000;
    let (text, title) = (wide(text), wide(title));
    unsafe {
        MessageBoxW(
            0,
            text.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// The release build is a GUI-subsystem program with no console of its own,
/// so text printed by the command line modes (`--version`, `--help`)
/// would be lost when started from cmd or PowerShell. Attach to the
/// parent's console in that case; output already redirected (a pipe or a
/// file) is left untouched.
pub fn attach_parent_console() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    if std::io::stdout().as_raw_handle().is_null() {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Open `target` (a file, a folder or a URI such as `https://…`) as
/// Explorer would. eframe opens links only with its `links` feature, which
/// is off, so About, the update button and the menu go through this.
pub fn shell_open(target: impl AsRef<OsStr>) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let file = wide(target);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    result as isize > 32
}

/// Open the folder of `file` in Explorer with the file selected
/// (`SHOpenFolderAndSelectItems`: `explorer /select` leaves the window
/// behind the app's). On a thread of its own with COM, as the shell may
/// take a while; the functions are declared by hand to spare features
/// (metalamp's).
pub fn show_in_folder(file: &std::path::Path) {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHParseDisplayName(
            name: *const u16,
            bind: *mut core::ffi::c_void,
            pidl: *mut *mut core::ffi::c_void,
            sfgao_in: u32,
            sfgao_out: *mut u32,
        ) -> i32;
        fn SHOpenFolderAndSelectItems(
            folder: *const core::ffi::c_void,
            count: u32,
            items: *const *const core::ffi::c_void,
            flags: u32,
        ) -> i32;
        fn ILFree(pidl: *const core::ffi::c_void);
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *const core::ffi::c_void, coinit: u32) -> i32;
        fn CoUninitialize();
    }
    const COINIT_APARTMENTTHREADED: u32 = 2;
    let path = wide(file);
    let spawned = std::thread::Builder::new()
        .name("show in folder".into())
        .spawn(move || unsafe {
            let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED) >= 0;
            let mut pidl = std::ptr::null_mut();
            let parsed = SHParseDisplayName(
                path.as_ptr(),
                std::ptr::null_mut(),
                &mut pidl,
                0,
                std::ptr::null_mut(),
            );
            if parsed >= 0 && !pidl.is_null() {
                let hr = SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0);
                if hr < 0 {
                    log::warn!("cannot show the file in its folder: 0x{hr:08x}");
                }
                ILFree(pidl);
            }
            if com {
                CoUninitialize();
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot show the file in its folder: {e}");
    }
}

/// Windows' double-click time in seconds (Control Panel: Mouse), for
/// egui's `max_double_click_delay`, which is 0.3 s by default.
pub fn double_click_time() -> f64 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDoubleClickTime() -> u32;
    }
    unsafe { GetDoubleClickTime() as f64 / 1000.0 }
}

/// Whether this process runs elevated (as administrator).
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    })
}

// --- The window's place --------------------------------------------------

/// Set a DWM window attribute of 32-bit `value`; whether it was taken.
fn dwm_set(hwnd: isize, attribute: u32, value: i32) -> bool {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    unsafe { DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast(), 4) >= 0 }
}

/// Round the corners of the frameless window as Windows 11 does its own
/// (`DWMWA_WINDOW_CORNER_PREFERENCE` = `DWMWCP_ROUND`); Windows 10 has no
/// such attribute and keeps them square.
pub fn round_corners(hwnd: isize) {
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;
    dwm_set(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
}

/// The window's rectangle on the screen, in pixels.
pub fn window_rect(hwnd: isize) -> Option<[i32; 4]> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut r: RECT = unsafe { std::mem::zeroed() };
    (unsafe { GetWindowRect(hwnd as _, &mut r) } != 0).then_some([r.left, r.top, r.right, r.bottom])
}

/// The window moved and sized to `rect` (pixels, screen coordinates).
pub fn set_rect(hwnd: isize, rect: [i32; 4]) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
    };
    unsafe {
        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            rect[0],
            rect[1],
            rect[2] - rect[0],
            rect[3] - rect[1],
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        );
    }
}

/// The work area (the screen without the taskbar) and the whole screen of
/// the monitor `rect` is mostly on, else of the nearest one, in pixels.
pub fn monitor_at(rect: [i32; 4]) -> Option<([i32; 4], [i32; 4])> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    let r = RECT {
        left: rect[0],
        top: rect[1],
        right: rect[2],
        bottom: rect[3],
    };
    let monitor = unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let (w, m) = (info.rcWork, info.rcMonitor);
    Some((
        [w.left, w.top, w.right, w.bottom],
        [m.left, m.top, m.right, m.bottom],
    ))
}

/// The longer side in pixels of the monitor the window is on: pictures
/// and videos are decoded no larger than that.
pub fn screen_side(hwnd: isize) -> u32 {
    window_rect(hwnd)
        .and_then(monitor_at)
        .map(|(_, m)| (m[2] - m[0]).max(m[3] - m[1]).max(1) as u32)
        .unwrap_or(1920)
}

/// The window, a frameless one, brought to `aspect` (width to height):
/// about as large as it was, about where it was, inside the work area.
pub fn apply_aspect(hwnd: isize, aspect: (u32, u32)) {
    let Some(r) = window_rect(hwnd) else { return };
    let Some((work, _)) = monitor_at(r) else {
        return;
    };
    let new = reshaped(r, work, aspect);
    if new != r {
        set_rect(hwnd, new);
    }
}

/// `rect` with the proportions `aspect` and the same area, the same
/// centre, moved and if need be shrunk into `work`.
pub fn reshaped(rect: [i32; 4], work: [i32; 4], aspect: (u32, u32)) -> [i32; 4] {
    let (w, h) = (
        (rect[2] - rect[0]).max(1) as f64,
        (rect[3] - rect[1]).max(1) as f64,
    );
    let a = aspect.0.max(1) as f64 / aspect.1.max(1) as f64;
    let mut nw = (w * h * a).sqrt();
    let mut nh = nw / a;
    let (ww, wh) = (
        (work[2] - work[0]).max(1) as f64,
        (work[3] - work[1]).max(1) as f64,
    );
    let shrink = (ww / nw).min(wh / nh).min(1.0);
    nw *= shrink;
    nh *= shrink;
    let (nw, nh) = (nw.round() as i32, nh.round() as i32);
    let (cx, cy) = ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2);
    let left = (cx - nw / 2).clamp(work[0], (work[2] - nw).max(work[0]));
    let top = (cy - nh / 2).clamp(work[1], (work[3] - nh).max(work[1]));
    [left, top, left + nw, top + nh]
}

/// `rect` (a window being sized by its `edge`, `WMSZ_*`) with the
/// proportions `aspect`: by a side edge or a corner the width counts, by
/// the top or the bottom edge the height; the edges across from the one
/// dragged stay (metalamp's `squared`, any proportions).
pub fn kept_aspect(rect: [i32; 4], edge: usize, aspect: (u32, u32)) -> [i32; 4] {
    // WMSZ_LEFT 1, RIGHT 2, TOP 3, TOPLEFT 4, TOPRIGHT 5, BOTTOM 6,
    // BOTTOMLEFT 7, BOTTOMRIGHT 8.
    let (aw, ah) = (aspect.0.max(1) as i64, aspect.1.max(1) as i64);
    let [left, mut top, mut right, mut bottom] = rect;
    match edge {
        1 | 2 | 4 | 5 | 7 | 8 => {
            let h = ((right - left) as i64 * ah / aw) as i32;
            if matches!(edge, 4 | 5) {
                top = bottom - h;
            } else {
                bottom = top + h;
            }
        }
        3 | 6 => right = left + ((bottom - top) as i64 * aw / ah) as i32,
        _ => {}
    }
    [left, top, right, bottom]
}

/// Whether `hwnd` is still a window (not destroyed).
pub fn is_window(hwnd: isize) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd as _) != 0 }
}

/// The window in front of the others.
pub fn bring_forward(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    unsafe {
        if IsIconic(hwnd as _) != 0 {
            ShowWindow(hwnd as _, SW_RESTORE);
        }
        SetForegroundWindow(hwnd as _);
    }
}

/// The window of this thread titled `title` (a viewport of the program's:
/// eframe gives no handle for it), hidden or not.
pub fn thread_window(title: &str) -> Option<isize> {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetWindowTextW};
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the pointer to `out` passed below, and the
        // callback runs only during the call.
        let out = unsafe { &mut *(lparam as *mut Vec<isize>) };
        out.push(hwnd as isize);
        1
    }
    let mut all: Vec<isize> = Vec::new();
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect),
            (&raw mut all) as LPARAM,
        )
    };
    all.into_iter().find(|&h| {
        let mut text = [0u16; 128];
        let n = unsafe { GetWindowTextW(h as _, text.as_mut_ptr(), text.len() as i32) };
        n > 0 && String::from_utf16_lossy(&text[..n as usize]) == title
    })
}

/// A window of the program's own (About): owned by `owner`, so it stays
/// over it (even when that one is always on top), and centred over it
/// within its work area.
pub fn adopt(hwnd: isize, owner: isize, centre: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
    unsafe { SetWindowLongPtrW(hwnd as _, GWLP_HWNDPARENT, owner) };
    round_corners(hwnd);
    if !centre {
        return;
    }
    if let (Some(main), Some(own)) = (window_rect(owner), window_rect(hwnd)) {
        let (w, h) = (own[2] - own[0], own[3] - own[1]);
        let x = (main[0] + main[2] - w) / 2;
        let y = (main[1] + main[3] - h) / 2;
        let mut rect = [x, y, x + w, y + h];
        if let Some((work, _)) = monitor_at(main) {
            let dx = (work[2] - rect[2]).min(0) + (work[0] - rect[0]).max(0);
            let dy = (work[3] - rect[3]).min(0) + (work[1] - rect[1]).max(0);
            rect = [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy];
        }
        set_rect(hwnd, rect);
    }
}

/// Where a popup of `size` pixels opened at `at` goes: its top left corner
/// at the point, or flipped left or up where it would leave `work`, kept
/// inside `work`.
pub fn popup_rect(at: (i32, i32), size: (i32, i32), work: [i32; 4]) -> [i32; 4] {
    let (w, h) = size;
    let mut left = if at.0 + w > work[2] { at.0 - w } else { at.0 };
    let mut top = if at.1 + h > work[3] { at.1 - h } else { at.1 };
    left = left.clamp(work[0], (work[2] - w).max(work[0]));
    top = top.clamp(work[1], (work[3] - h).max(work[1]));
    [left, top, left + w, top + h]
}

/// Window `hwnd` (a popup) moved to open at `at` (screen pixels) as
/// [`popup_rect`] says, its size kept; nothing done when it is there.
pub fn place_popup(hwnd: isize, at: (i32, i32)) {
    let Some(r) = window_rect(hwnd) else { return };
    let point = [at.0, at.1, at.0 + 1, at.1 + 1];
    let Some((work, _)) = monitor_at(point) else {
        return;
    };
    let wanted = popup_rect(at, (r[2] - r[0], r[3] - r[1]), work);
    if wanted != r {
        set_rect(hwnd, wanted);
    }
}

/// Let window `hwnd` take the mouse and the keys, or not (the widget while
/// its modal About is open).
pub fn enable(hwnd: isize, on: bool) {
    // Declared by hand, to spare the `Win32_UI_Input_KeyboardAndMouse` feature.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnableWindow(hwnd: isize, enable: i32) -> i32;
    }
    unsafe { EnableWindow(hwnd, i32::from(on)) };
}

/// The pointer on the screen, in pixels.
pub fn cursor_pos() -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

// --- The taskbar button --------------------------------------------------

/// The window's taskbar button shown or taken away ("Only in the tray").
/// winit knows the choice only when the window is made
/// (`with_taskbar`), so later changes go through `ITaskbarList`; Explorer
/// makes the button again when it restarts or the window is shown, which
/// `TaskbarButtonCreated` says (`Hooked::TaskbarButton`). COM is
/// initialised on the UI thread (winit's `OleInitialize`).
pub fn set_taskbar_button(hwnd: isize, shown: bool) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
    let list: windows::core::Result<ITaskbarList> =
        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) };
    let Ok(list) = list else {
        log::warn!("no taskbar list");
        return;
    };
    let hwnd = windows::Win32::Foundation::HWND(hwnd as _);
    let result = unsafe {
        list.HrInit().and_then(|()| {
            if shown {
                list.AddTab(hwnd)
            } else {
                list.DeleteTab(hwnd)
            }
        })
    };
    if let Err(e) = result {
        log::warn!("taskbar button: {e}");
    }
}

// --- Start with Windows --------------------------------------------------

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The command the Run key holds for this exe.
fn run_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

/// The value of `name` in the Run key of the user, if there is one.
fn run_value(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let (key, value) = (wide(RUN_KEY), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 {
        return None;
    }
    let n = (len as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..n]);
    Some(text.trim_end_matches('\0').to_owned())
}

/// Whether Windows starts this copy of the program at sign-in: the Run
/// key has its value, naming this exe (a value naming another copy, an
/// installed one or a portable one, does not count).
pub fn autostart() -> bool {
    let (Some(value), Some(own)) = (run_value(crate::APP_NAME), run_command()) else {
        return false;
    };
    value.trim().eq_ignore_ascii_case(&own)
}

/// Make Windows start this copy at sign-in, or not.
pub fn set_autostart(on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let (key, name) = (wide(RUN_KEY), wide(crate::APP_NAME));
    let status = if on {
        let command = wide(run_command().ok_or("no path of the program")?);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
            // ERROR_FILE_NOT_FOUND: there was none.
            2 => 0,
            s => s,
        }
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("registry error {status}"))
    }
}

// --- The window's subclass -----------------------------------------------

/// What the window's messages asked for, for the app to take each frame
/// ([`take_hooked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hooked {
    /// Exit chosen in the tray icon's menu.
    Exit,
    /// Explorer started (again): the tray icon is to be added anew.
    TrayCreated,
    /// Explorer made the window's taskbar button (again).
    TaskbarButton,
}

static HOOKED: Mutex<Vec<Hooked>> = Mutex::new(Vec::new());
/// Wakes the app when something is hooked.
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// `TaskbarButtonCreated` and `TaskbarCreated`, registered by [`hook_window`].
static TASKBAR_BUTTON: AtomicU32 = AtomicU32::new(0);
static EXPLORER_STARTED: AtomicU32 = AtomicU32::new(0);
/// The proportions kept while the window is sized, width << 16 | height;
/// 0: any.
static ASPECT: AtomicU32 = AtomicU32::new(0);

/// Keep the window at `aspect` while its edges are dragged, or not.
pub fn set_kept_aspect(aspect: Option<(u32, u32)>) {
    let packed = aspect.map_or(0, |(w, h)| (w.min(0xffff) << 16) | h.min(0xffff));
    ASPECT.store(packed, Relaxed);
}

/// What the window's messages asked for since the last call.
pub fn take_hooked() -> Vec<Hooked> {
    HOOKED
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn hook(what: Hooked) {
    if let Ok(mut q) = HOOKED.lock() {
        q.push(what);
    }
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// Subclass the window once: the tray icon's clicks, Explorer's restart
/// and the taskbar button made are queued for [`take_hooked`] and `wake`
/// is called; the proportions are kept while the window is sized.
pub fn hook_window(hwnd: isize, wake: impl Fn() + Send + Sync + 'static) {
    use std::sync::Once;
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static SUBCLASSED: Once = Once::new();
    SUBCLASSED.call_once(|| {
        let button = unsafe { RegisterWindowMessageW(wide("TaskbarButtonCreated").as_ptr()) };
        TASKBAR_BUTTON.store(button, Relaxed);
        let started = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
        EXPLORER_STARTED.store(started, Relaxed);
        let _ = WAKE.set(Box::new(wake));
        // A copy running elevated still hears Explorer and its tray icon
        // (UIPI); one that is not leaves the filter as it is.
        if is_elevated() {
            #[link(name = "user32")]
            unsafe extern "system" {
                fn ChangeWindowMessageFilterEx(
                    hwnd: HWND,
                    message: u32,
                    action: u32,
                    filter: *mut core::ffi::c_void,
                ) -> i32;
            }
            const MSGFLT_ALLOW: u32 = 1;
            for message in [button, started, crate::tray::WM_TRAY] {
                unsafe {
                    ChangeWindowMessageFilterEx(
                        hwnd as _,
                        message,
                        MSGFLT_ALLOW,
                        std::ptr::null_mut(),
                    )
                };
            }
        }
        unsafe { SetWindowSubclass(hwnd as _, Some(window_proc), 1, 0) };
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_SIZING;
    if msg != 0 && msg == TASKBAR_BUTTON.load(Relaxed) {
        hook(Hooked::TaskbarButton);
    }
    if msg != 0 && msg == EXPLORER_STARTED.load(Relaxed) {
        hook(Hooked::TrayCreated);
    }
    // The tray icon's events (`NOTIFYICON_VERSION_4`: the event in the low
    // word of `lparam`, the pointer in `wparam`).
    if msg == crate::tray::WM_TRAY {
        const NIN_SELECT: u32 = 0x400;
        const NIN_KEYSELECT: u32 = 0x401;
        const WM_CONTEXTMENU: u32 = 0x007B;
        match lparam as u32 & 0xFFFF {
            // At once, without waiting for a frame.
            NIN_SELECT | NIN_KEYSELECT => bring_forward(hwnd as isize),
            WM_CONTEXTMENU => {
                let x = (wparam & 0xFFFF) as u16 as i16 as i32;
                let y = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                match crate::tray::menu(hwnd as isize, x, y) {
                    Some(crate::tray::TrayCmd::Show) => bring_forward(hwnd as isize),
                    Some(crate::tray::TrayCmd::Exit) => hook(Hooked::Exit),
                    None => {}
                }
            }
            _ => {}
        }
        return 0;
    }
    let aspect = ASPECT.load(Relaxed);
    if msg == WM_SIZING && lparam != 0 && aspect != 0 {
        // The window has no frame: its rectangle is its client area.
        let r = unsafe { &mut *(lparam as *mut RECT) };
        let [left, top, right, bottom] = kept_aspect(
            [r.left, r.top, r.right, r.bottom],
            wparam,
            (aspect >> 16, aspect & 0xffff),
        );
        *r = RECT {
            left,
            top,
            right,
            bottom,
        };
        return 1;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

// --- HTTPS ---------------------------------------------------------------

/// HTTPS GET of `path` on `host` through WinHTTP, which brings the system's
/// TLS and proxy settings (no HTTP crate); `headers` are extra request
/// lines joined with `\r\n`. Returns the status code and the body, refused
/// past `limit` bytes. Blocks for up to `timeout_ms` per step, so call it
/// off the UI thread.
pub fn https_get(
    host: &str,
    path: &str,
    agent: &str,
    headers: &str,
    timeout_ms: i32,
    limit: usize,
) -> Result<(u32, Vec<u8>), String> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Networking::WinHttp::*;

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
    fn handle(h: *mut c_void) -> Result<Handle, String> {
        if h.is_null() {
            Err(winhttp_error())
        } else {
            Ok(Handle(h))
        }
    }
    fn check(ok: windows_sys::core::BOOL) -> Result<(), String> {
        if ok == 0 {
            Err(winhttp_error())
        } else {
            Ok(())
        }
    }

    let (agent, host, path, headers) = (wide(agent), wide(host), wide(path), wide(headers));
    let verb = wide("GET");
    let t = timeout_ms;
    // Declared in this order, the handles close request first.
    unsafe {
        let session = handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ))?;
        check(WinHttpSetTimeouts(session.0, t, t, t, t))?;
        let connection = handle(WinHttpConnect(
            session.0,
            host.as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = handle(WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        check(WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            null(),
            0,
            0,
            0,
        ))?;
        check(WinHttpReceiveResponse(request.0, null_mut()))?;
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        check(WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&raw mut status).cast(),
            &mut len,
            null_mut(),
        ))?;
        let mut body = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            check(WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            ))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > limit {
                return Err(format!("the answer is larger than {limit} bytes"));
            }
        }
        Ok((status, body))
    }
}

/// The last WinHTTP error in words; the system's message table lacks them.
fn winhttp_error() -> String {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    match code {
        12002 => "the server did not answer in time".into(),
        12007 => "the server name could not be resolved".into(),
        12029 | 12030 => "the connection to the server failed".into(),
        12175 => "the secure connection failed".into(),
        _ => format!("WinHTTP error {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportions_while_sizing() {
        // By the right edge the width counts, the bottom follows.
        assert_eq!(kept_aspect([0, 0, 320, 999], 2, (16, 9)), [0, 0, 320, 180]);
        // By the top left corner the top moves.
        assert_eq!(kept_aspect([0, 0, 300, 999], 4, (3, 2)), [0, 799, 300, 999]);
        // By the bottom edge the height counts, the right edge follows.
        assert_eq!(
            kept_aspect([10, 10, 999, 210], 6, (1, 1)),
            [10, 10, 210, 210]
        );
    }

    #[test]
    fn popups_stay_on_the_screen() {
        let work = [0, 0, 1920, 1040];
        assert_eq!(
            popup_rect((100, 100), (300, 400), work),
            [100, 100, 400, 500]
        );
        // From the tray, at the bottom right: up and to the left.
        assert_eq!(
            popup_rect((1900, 1060), (300, 400), work),
            [1600, 640, 1900, 1040]
        );
        // Taller than the screen: from its top.
        assert_eq!(popup_rect((10, 500), (300, 2000), work)[1], 0);
    }

    #[test]
    fn reshaped_keeps_area_and_screen() {
        let work = [0, 0, 1920, 1040];
        let r = reshaped([100, 100, 500, 400], work, (1, 1));
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        assert_eq!(w, h);
        assert!((w * h - 400 * 300).abs() < 1000);
        // Too tall for the work area: shrunk into it.
        let r = reshaped([0, 0, 1900, 1000], work, (9, 16));
        assert!(
            r[1] >= 0 && r[3] <= 1040 && r[0] >= 0 && r[2] <= 1920,
            "{r:?}"
        );
        // Past the right edge: moved back.
        let r = reshaped([1800, 100, 2200, 400], work, (4, 3));
        assert!(r[2] <= 1920, "{r:?}");
    }
}

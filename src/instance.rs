//! One widget per copy of the program: a second start (from the Start menu
//! while the first was started with Windows) brings the window already
//! open to the front and exits (metalamp's `instance.rs`, without paths).
//!
//! The first start owns a named mutex; once its window exists it also has a
//! message-only window of a class with the same name, which answers a
//! `WM_COPYDATA` with the handle of the main window. Both names are derived
//! from the path of the exe, so an installed copy and a development build
//! do not take each other's starts.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, FindWindowExW, HWND_MESSAGE, RegisterClassW, SMTO_ABORTIFHUNG,
    SMTO_BLOCK, SendMessageTimeoutW, SetForegroundWindow, WM_COPYDATA, WNDCLASSW,
};

use crate::win::wide;

/// Marks a `WM_COPYDATA` as the program's (`COPYDATASTRUCT::dwData`).
const MAGIC: usize = 0x7173_6c64;
/// How long a second start waits for the window of the first one, which
/// appears soon after its start.
const WAIT: Duration = Duration::from_secs(5);
/// How long the first start may take to answer.
const ANSWER_MS: u32 = 3000;

/// The UI to wake when a later start comes, and the main window.
static TARGET: OnceLock<(egui::Context, isize)> = OnceLock::new();

/// Name of the mutex and of the window class, the same for every start of
/// this exe in this session.
fn name() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!(
        "qslides.{:016x}",
        fnv1a(exe.to_string_lossy().to_lowercase().as_bytes())
    )
}

/// FNV-1a, 64 bits: the same in every build, which std's `DefaultHasher`
/// is not promised to be (copies built by different Rust versions would
/// not have known each other).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Called at start-up. If the program is already running from this exe,
/// bring its window forward and return true: this process has nothing
/// more to do. Otherwise this process is the first one and stays so until
/// it exits.
pub fn forward() -> bool {
    let name = name();
    let mutex_name = wide(format!("Local\\{name}"));
    let class = wide(&name);
    let deadline = Instant::now() + WAIT;
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        log::warn!("CreateMutexW failed: {}", std::io::Error::last_os_error());
        return false;
    }
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        // The first start: the handle stays open while the process lives.
        return false;
    }
    unsafe { CloseHandle(mutex) };
    loop {
        let window = unsafe {
            FindWindowExW(
                HWND_MESSAGE,
                std::ptr::null_mut(),
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        if !window.is_null() {
            return send(window);
        }
        // The first start has not made its window yet.
        if Instant::now() >= deadline {
            log::warn!("the program is running but its window was not found");
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Ask the message window `to` for the main window and bring it to the
/// foreground: this process was started by the user, so it may. Whether
/// the first start answered.
fn send(to: HWND) -> bool {
    let data = COPYDATASTRUCT {
        dwData: MAGIC,
        cbData: 0,
        lpData: std::ptr::null_mut(),
    };
    let mut answer = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            to,
            WM_COPYDATA,
            0,
            &raw const data as LPARAM,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            ANSWER_MS,
            &mut answer,
        )
    };
    if ok == 0 || answer == 0 {
        log::warn!("the running program did not answer");
        return false;
    }
    unsafe { SetForegroundWindow(answer as HWND) };
    true
}

/// Make this process the one later starts go to: a message-only window on
/// the UI thread, whose messages the event loop dispatches. `main` is the
/// app's window.
pub fn listen(ctx: &egui::Context, main: isize) {
    if TARGET.set((ctx.clone(), main)).is_err() {
        return;
    }
    let class = wide(name());
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            log::warn!("RegisterClassW failed: {}", std::io::Error::last_os_error());
            return;
        }
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if window.is_null() {
            log::warn!(
                "CreateWindowExW failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Answers a later start with the main window.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg != WM_COPYDATA || lparam == 0 {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let data = unsafe { &*(lparam as *const COPYDATASTRUCT) };
    let Some((ctx, main)) = TARGET.get().filter(|_| data.dwData == MAGIC) else {
        return 0;
    };
    // The later start brings the window forward, from under the others
    // too while it is on the desktop.
    crate::win::mark_raised();
    ctx.request_repaint();
    *main as LRESULT
}

#[cfg(test)]
mod tests {
    /// The known values of FNV-1a 64: the names do not change between
    /// builds.
    #[test]
    fn names_are_stable() {
        assert_eq!(super::fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(super::fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }
}

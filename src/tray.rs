//! The icon in the notification area, there while "Only in the tray" is
//! on (the window then has no taskbar button). A click brings the window
//! in front of the others; the right button opens the same menu as a right
//! click on the window (`menu.rs`). The messages come to the window's
//! subclass (`win::window_proc`), which queues them for the app
//! (metalamp's `tray.rs`).

use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// The message the icon's events come with (`WM_APP` + 0x51).
pub const WM_TRAY: u32 = 0x8000 + 0x51;

pub struct Tray {
    hwnd: isize,
    icon: isize,
    tip: String,
}

impl Tray {
    /// The icon added for `hwnd`; `None` if the shell refused it.
    pub fn new(hwnd: isize, tip: &str) -> Option<Tray> {
        let mut tray = Tray {
            hwnd,
            icon: load_icon(),
            tip: tip.to_owned(),
        };
        tray.add().then_some(tray)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        // SAFETY: a plain struct, all zeros a valid start.
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd as _;
        d.uID = 1;
        d
    }

    /// Adds the icon (again, after Explorer restarted); whether it worked.
    pub fn add(&mut self) -> bool {
        let mut d = self.data();
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = self.icon as _;
        put_tip(&mut d, &self.tip);
        // SAFETY: a filled NOTIFYICONDATAW of this window.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &d) } != 0;
        if added {
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            unsafe { Shell_NotifyIconW(NIM_SETVERSION, &d) };
        }
        added
    }

    /// The tooltip: the name and the file shown.
    pub fn set_tip(&mut self, tip: &str) {
        if tip == self.tip {
            return;
        }
        self.tip = tip.to_owned();
        let mut d = self.data();
        d.uFlags = NIF_TIP | NIF_SHOWTIP;
        put_tip(&mut d, tip);
        // SAFETY: as in `add`.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let d = self.data();
        // SAFETY: the icon of this window, removed once.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
        if self.icon != 0 {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.icon as _) };
        }
    }
}

/// `tip` in the struct's 128 UTF-16 units, cut and NUL-ended.
fn put_tip(d: &mut NOTIFYICONDATAW, tip: &str) {
    let wide: Vec<u16> = tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip.fill(0);
    d.szTip[..wide.len()].copy_from_slice(&wide);
}

/// The exe's icon (resource 1) at the small icon size.
fn load_icon() -> isize {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXSMICON, SM_CYSMICON,
    };
    // SAFETY: the exe's own module and resource; a missing one is null.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        LoadImageW(
            module,
            1 as _,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        ) as isize
    }
}

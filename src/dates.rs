//! The date a photo or a video was taken, for the slide's caption: read
//! through the shell's property system, whatever the format keeps it in
//! (`System.Photo.DateTaken`, the EXIF DateTimeOriginal of a photo;
//! `System.Media.DateEncoded`, the date a camera or a phone wrote into a
//! video). On a thread of its own with COM, and only for the files shown or
//! about to be, kept once read: read for a whole library at its scan, it
//! would take long.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// A day of the calendar, local to this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u16,
    pub day: u16,
}

/// More dates than this kept, and they are all forgotten: a random show
/// of a large library would otherwise keep one for each of its files.
const KEPT: usize = 10_000;

/// The paths sent to the thread, and its answers.
type Worker = (Sender<PathBuf>, Receiver<(PathBuf, Option<Date>)>);

/// The dates of the files asked for, read on a thread started when the
/// first one is asked for.
pub struct Dates {
    ctx: egui::Context,
    worker: Option<Worker>,
    /// The dates read, `None` for a file without one or while it is read.
    known: HashMap<PathBuf, Option<Date>>,
}

impl Dates {
    pub fn new(ctx: egui::Context) -> Dates {
        Dates {
            ctx,
            worker: None,
            known: HashMap::new(),
        }
    }

    /// The date `path` was taken, if it is known; asked for when it was
    /// not asked for yet (the app is asked to repaint when it comes).
    pub fn get(&mut self, path: &Path) -> Option<Date> {
        if let Some((_, rx)) = &self.worker {
            self.known.extend(rx.try_iter());
        }
        if let Some(date) = self.known.get(path) {
            return *date;
        }
        if self.known.len() >= KEPT {
            self.known.clear();
        }
        self.known.insert(path.to_path_buf(), None);
        if self.worker.is_none() {
            self.worker = start(self.ctx.clone());
        }
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(path.to_path_buf());
        }
        None
    }
}

fn start(ctx: egui::Context) -> Option<Worker> {
    let (tx, paths) = channel::<PathBuf>();
    let (done, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("dates".into())
        .spawn(move || {
            let _com = crate::media::Com::init();
            while let Ok(path) = paths.recv() {
                let date = date_taken(&path);
                log::debug!("{}: taken {date:?}", path.display());
                if done.send((path, date)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    match spawned {
        Ok(_) => Some((tx, rx)),
        Err(e) => {
            log::warn!("cannot start reading dates: {e}");
            None
        }
    }
}

/// `System.Photo.DateTaken` and `System.Media.DateEncoded`.
const DATE_TAKEN: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x14b81da1_0135_4d31_96d9_6cbfc9671a99),
        pid: 36867,
    };
const DATE_ENCODED: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x2e4b640d_5019_46d8_8881_55414cc5caa0),
        pid: 100,
    };

/// The day the photo or the video in `path` was taken, as the shell has
/// it; `None` without one. On a thread with COM.
pub fn date_taken(path: &Path) -> Option<Date> {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToFileTime;
    use windows::Win32::System::Variant::PSTF_UTC;
    use windows::Win32::UI::Shell::PropertiesSystem::{
        GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName,
    };
    let name = crate::win::wide(path);
    let store: IPropertyStore = unsafe {
        SHGetPropertyStoreFromParsingName(windows::core::PCWSTR(name.as_ptr()), None, GPS_DEFAULT)
    }
    .ok()?;
    for key in [DATE_TAKEN, DATE_ENCODED] {
        let Ok(value) = (unsafe { store.GetValue(&key) }) else {
            continue;
        };
        // An empty value is not a time.
        let Ok(time) = (unsafe { PropVariantToFileTime(&value, PSTF_UTC) }) else {
            continue;
        };
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        if ticks != 0 {
            return local_date(ticks);
        }
    }
    None
}

/// A time in UTC (`FILETIME` ticks) as the local day, by the time zone's
/// rules at that time: the shell turned the camera's local time into UTC.
fn local_date(ticks: u64) -> Option<Date> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime([u16; 8]);
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToSystemTime(time: *const u64, out: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const core::ffi::c_void,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }
    let (mut utc, mut local) = (SystemTime::default(), SystemTime::default());
    // SAFETY: FILETIME is two DWORDs, low first: a u64 on little-endian
    // Windows; SYSTEMTIME is eight WORDs.
    unsafe {
        if FileTimeToSystemTime(&ticks, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return None;
        }
    }
    // wYear, wMonth, wDayOfWeek, wDay, …
    let [year, month, _, day, ..] = local.0;
    Some(Date { year, month, day })
}

/// The date in words, in the language of the interface: "7 November
/// 2026", "7 ноября 2026".
pub fn label(date: Date) -> String {
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const RU: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    let m = usize::from(date.month.clamp(1, 12)) - 1;
    let Date { day, year, .. } = date;
    tr!(
        format!("{day} {} {year}", EN[m]),
        format!("{day} {} {year}", RU[m])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_utc_time_as_a_day() {
        // 2026-06-15 12:00 UTC is the 15th in any time zone.
        // Unix seconds, from 1601 in 100 ns.
        let ticks = (1_781_524_800u64 + 11_644_473_600) * 10_000_000;
        assert_eq!(
            local_date(ticks),
            Some(Date {
                year: 2026,
                month: 6,
                day: 15
            })
        );
    }

    #[test]
    fn no_date_without_a_file() {
        let _com = crate::media::Com::init();
        assert_eq!(date_taken(Path::new(r"C:\no such folder\1.jpg")), None);
    }

    /// The date of a real file: `QSLIDES_IMAGE` or `QSLIDES_VIDEO`.
    #[test]
    #[ignore]
    fn date_of_a_file() {
        let _com = crate::media::Com::init();
        for var in ["QSLIDES_IMAGE", "QSLIDES_VIDEO"] {
            if let Ok(path) = std::env::var(var) {
                let started = std::time::Instant::now();
                let date = date_taken(Path::new(&path));
                println!("{path}: {date:?} in {:?}", started.elapsed());
            }
        }
    }
}

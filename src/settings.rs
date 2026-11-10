//! Settings kept between runs in eframe's storage
//! (`%APPDATA%\qslides\data\app.ron`, or [`PORTABLE_FILE`] next to the exe,
//! see [`location`]). Window size and position are saved by eframe itself;
//! the library's folders are kept in `library.txt` beside the file
//! (`library.rs`), and starting with Windows in the Run key (`win.rs`).
//!
//! Plain strings through `eframe::Storage`: no serde derives needed, and an
//! unknown or malformed value falls back to its default, so settings of an
//! older or newer version never make the file unreadable.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::border::Border;
use crate::i18n::LangChoice;
use crate::show::{Caption, Fit, Kinds, Layer, Order, VideoLength, aspect_from_name, aspect_name};

const LANGUAGE: &str = "language";
const CHECK_UPDATES: &str = "check_updates";
const LAST_UPDATE_CHECK: &str = "last_update_check";
const INTERVAL: &str = "interval";
const ORDER: &str = "order";
const KINDS: &str = "kinds";
const FIT: &str = "fit";
const VIDEO: &str = "video";
const VIDEO_LIMIT: &str = "video_limit";
const VIDEO_PIECES: &str = "video_pieces";
const VIDEO_BAR: &str = "video_bar";
const SOUND: &str = "sound";
const VOLUME: &str = "volume";
const CAPTION: &str = "caption";
const BORDER: &str = "border";
const BORDER_WIDTH: &str = "border_width";
const ASPECT: &str = "aspect";
/// Read only: before `layer`, `true` was `Layer::Top`.
const ON_TOP: &str = "on_top";
const LAYER: &str = "layer";
const YIELD_FULL_SCREEN: &str = "yield_full_screen";
const TRAY_ONLY: &str = "tray_only";
const LAST_FILE: &str = "last_file";

/// Settings file that makes a copy portable: when it lies next to the exe
/// (even empty) and can be written, the settings are kept in it instead of
/// the user profile. The portable zip ships an empty one
/// (`PORTABLE_SETTINGS` in tools/make_release.py).
pub const PORTABLE_FILE: &str = "app.ron";

/// Where the settings of this run are kept.
#[derive(Debug)]
pub struct Location {
    /// The settings file, `None` if there is nowhere to keep it.
    pub file: Option<PathBuf>,
    /// `file` is [`PORTABLE_FILE`] next to the exe.
    pub portable: bool,
}

impl Location {
    /// Whether settings of an earlier run are there (a portable file starts
    /// empty). `NativeOptions::centered` is set only when there are none:
    /// eframe would otherwise re-centre a restored window.
    pub fn has_saved(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.metadata().is_ok_and(|m| m.len() > 0))
    }
}

/// Where the settings are kept, decided once per run.
pub fn location() -> &'static Location {
    static LOCATION: OnceLock<Location> = OnceLock::new();
    LOCATION.get_or_init(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        match exe_dir.and_then(|d| portable_file_in(&d)) {
            Some(file) => Location {
                file: Some(file),
                portable: true,
            },
            None => Location {
                file: eframe::storage_dir(crate::APP_ID).map(|d| d.join("app.ron")),
                portable: false,
            },
        }
    })
}

/// [`PORTABLE_FILE`] in `dir` if it is there and can be written; a copy on
/// a read-only medium keeps its settings in the profile instead of losing
/// them.
fn portable_file_in(dir: &Path) -> Option<PathBuf> {
    let file = dir.join(PORTABLE_FILE);
    // Opening for appending checks the permission without changing the file.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .ok()
        .map(|_| file)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Interface language, or the one of Windows.
    pub language: LangChoice,
    /// Ask GitHub for a newer release at start-up (off unless enabled).
    pub check_updates: bool,
    /// Unix seconds of the last update check that got an answer.
    pub last_update_check: u64,
    /// Seconds a slide stays (`show::INTERVALS`, though any number reads).
    pub interval: u32,
    pub order: Order,
    /// Pictures, videos or both.
    pub kinds: Kinds,
    pub fit: Fit,
    pub video: VideoLength,
    /// Seconds a video shown to its end may take at most; 0: no limit.
    pub video_limit: u32,
    /// The pieces a video longer than its time is shown in (1 to
    /// `show::MAX_PIECES`; 1: its start only).
    pub video_pieces: u32,
    /// A seek bar over a video shown whole, with the buttons.
    pub video_bar: bool,
    /// Videos are heard (off by default), at `volume` (5 to 100).
    pub sound: bool,
    pub volume: u32,
    pub caption: Caption,
    /// The frame painted round the window's edges.
    pub border: Border,
    /// Its width, 1 to `border::MAX_WIDTH` steps of the shorter side.
    pub border_width: u32,
    /// The window's proportions kept while it is sized; `None`: any.
    pub aspect: Option<(u32, u32)>,
    /// Above all windows, under them, or as any window.
    pub layer: Layer,
    /// Always on top, but not over a full-screen program in front.
    pub yield_full_screen: bool,
    /// No taskbar button; an icon in the notification area instead.
    pub tray_only: bool,
    /// The file shown when the program was closed: the show goes on from
    /// it.
    pub last_file: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: LangChoice::System,
            check_updates: false,
            last_update_check: 0,
            interval: 10,
            order: Order::Name,
            kinds: Kinds::ALL,
            fit: Fit::Fit,
            video: VideoLength::Whole,
            video_limit: 0,
            video_pieces: 1,
            video_bar: false,
            sound: false,
            volume: 50,
            caption: Caption::NONE,
            border: Border::None,
            border_width: 4,
            aspect: None,
            layer: Layer::Normal,
            yield_full_screen: true,
            tray_only: false,
            last_file: None,
        }
    }
}

fn flag(value: Option<&str>, default: bool) -> bool {
    match value {
        Some("true") => true,
        Some("false") => false,
        _ => default,
    }
}

impl Settings {
    /// Read settings; missing or unknown values keep their defaults.
    pub fn load(storage: &dyn eframe::Storage) -> Self {
        let d = Self::default();
        let get = |key| storage.get_string(key);
        Self {
            language: get(LANGUAGE)
                .as_deref()
                .map_or(d.language, LangChoice::from_name),
            check_updates: flag(get(CHECK_UPDATES).as_deref(), d.check_updates),
            last_update_check: get(LAST_UPDATE_CHECK)
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.last_update_check),
            interval: get(INTERVAL)
                .and_then(|v| v.parse().ok())
                .filter(|&n: &u32| (1..=86_400).contains(&n))
                .unwrap_or(d.interval),
            order: get(ORDER)
                .as_deref()
                .and_then(Order::from_name)
                .unwrap_or(d.order),
            kinds: get(KINDS)
                .as_deref()
                .and_then(Kinds::from_name)
                .unwrap_or(d.kinds),
            fit: get(FIT)
                .as_deref()
                .and_then(Fit::from_name)
                .unwrap_or(d.fit),
            video: get(VIDEO)
                .as_deref()
                .and_then(VideoLength::from_name)
                .unwrap_or(d.video),
            video_limit: get(VIDEO_LIMIT)
                .and_then(|v| v.parse().ok())
                .filter(|&n: &u32| n <= 86_400)
                .unwrap_or(d.video_limit),
            video_pieces: get(VIDEO_PIECES)
                .and_then(|v| v.parse().ok())
                .filter(|n: &u32| (1..=crate::show::MAX_PIECES).contains(n))
                .unwrap_or(d.video_pieces),
            video_bar: flag(get(VIDEO_BAR).as_deref(), d.video_bar),
            sound: flag(get(SOUND).as_deref(), d.sound),
            volume: get(VOLUME)
                .and_then(|v| v.parse().ok())
                .filter(|n: &u32| (5..=100).contains(n))
                .unwrap_or(d.volume),
            caption: get(CAPTION)
                .as_deref()
                .and_then(Caption::from_name)
                .unwrap_or(d.caption),
            border: get(BORDER)
                .as_deref()
                .and_then(Border::from_name)
                .unwrap_or(d.border),
            border_width: get(BORDER_WIDTH)
                .and_then(|v| v.parse().ok())
                .filter(|n: &u32| (1..=crate::border::MAX_WIDTH).contains(n))
                .unwrap_or(d.border_width),
            aspect: get(ASPECT)
                .as_deref()
                .and_then(aspect_from_name)
                .unwrap_or(d.aspect),
            layer: match get(LAYER).as_deref().and_then(Layer::from_name) {
                Some(layer) => layer,
                // Saved by a version before `layer`.
                None if flag(get(ON_TOP).as_deref(), false) => Layer::Top,
                None => d.layer,
            },
            yield_full_screen: flag(get(YIELD_FULL_SCREEN).as_deref(), d.yield_full_screen),
            tray_only: flag(get(TRAY_ONLY).as_deref(), d.tray_only),
            last_file: get(LAST_FILE).filter(|v| !v.is_empty()).map(PathBuf::from),
        }
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(LANGUAGE, self.language.name().into());
        storage.set_string(CHECK_UPDATES, self.check_updates.to_string());
        storage.set_string(LAST_UPDATE_CHECK, self.last_update_check.to_string());
        storage.set_string(INTERVAL, self.interval.to_string());
        storage.set_string(ORDER, self.order.name().into());
        storage.set_string(KINDS, self.kinds.name());
        storage.set_string(FIT, self.fit.name().into());
        storage.set_string(VIDEO, self.video.name().into());
        storage.set_string(VIDEO_LIMIT, self.video_limit.to_string());
        storage.set_string(VIDEO_PIECES, self.video_pieces.to_string());
        storage.set_string(VIDEO_BAR, self.video_bar.to_string());
        storage.set_string(SOUND, self.sound.to_string());
        storage.set_string(VOLUME, self.volume.to_string());
        storage.set_string(CAPTION, self.caption.name());
        storage.set_string(BORDER, self.border.name().into());
        storage.set_string(BORDER_WIDTH, self.border_width.to_string());
        storage.set_string(ASPECT, aspect_name(self.aspect));
        storage.set_string(LAYER, self.layer.name().into());
        storage.remove_string(ON_TOP);
        storage.set_string(YIELD_FULL_SCREEN, self.yield_full_screen.to_string());
        storage.set_string(TRAY_ONLY, self.tray_only.to_string());
        let last = self
            .last_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        storage.set_string(LAST_FILE, last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::CaptionPart;
    use eframe::Storage;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemStorage(HashMap<String, String>);

    impl Storage for MemStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn round_trip() {
        let s = Settings {
            language: LangChoice::Ru,
            check_updates: true,
            last_update_check: 1_790_000_000,
            interval: 300,
            order: Order::Random,
            kinds: Kinds::ALL.toggled(crate::library::Kind::Image),
            fit: Fit::Fill,
            video: VideoLength::Interval,
            video_limit: 120,
            video_pieces: 6,
            video_bar: true,
            sound: true,
            volume: 35,
            caption: Caption::NONE
                .toggled(CaptionPart::Folder)
                .toggled(CaptionPart::Date),
            border: Border::Wood,
            border_width: 7,
            aspect: Some((9, 16)),
            layer: Layer::Desktop,
            yield_full_screen: false,
            tray_only: true,
            last_file: Some(PathBuf::from(r"C:\Фото\1.jpg")),
        };
        let mut storage = MemStorage::default();
        s.save(&mut storage);
        assert_eq!(Settings::load(&storage), s);
        let mut storage = MemStorage::default();
        Settings::default().save(&mut storage);
        assert_eq!(Settings::load(&storage), Settings::default());
    }

    #[test]
    fn missing_or_unknown_values_use_defaults() {
        assert_eq!(Settings::load(&MemStorage::default()), Settings::default());
        let mut storage = MemStorage::default();
        for (key, value) in [
            (LANGUAGE, "de"),
            (CHECK_UPDATES, "yes"),
            (LAST_UPDATE_CHECK, "soon"),
            (INTERVAL, "0"),
            (ORDER, "size"),
            (KINDS, ""),
            (FIT, "stretch"),
            (VIDEO, "muted"),
            (VIDEO_LIMIT, "-5"),
            (VIDEO_PIECES, "0"),
            (VIDEO_BAR, "1"),
            (SOUND, "on"),
            (VOLUME, "0"),
            (CAPTION, "exif"),
            (BORDER, "gold"),
            (BORDER_WIDTH, "11"),
            (ASPECT, "7:5"),
            (ON_TOP, "1"),
            (LAYER, "above"),
            (YIELD_FULL_SCREEN, "no"),
            (TRAY_ONLY, ""),
            (LAST_FILE, ""),
        ] {
            storage.set_string(key, value.into());
        }
        assert_eq!(Settings::load(&storage), Settings::default());
    }

    #[test]
    fn always_on_top_of_an_older_version() {
        let mut storage = MemStorage::default();
        storage.set_string(ON_TOP, "true".into());
        assert_eq!(Settings::load(&storage).layer, Layer::Top);
        // Saved again, it is a layer.
        Settings::load(&storage).save(&mut storage);
        assert_eq!(storage.get_string(ON_TOP), None);
        assert_eq!(storage.get_string(LAYER).as_deref(), Some("top"));
    }

    #[test]
    fn portable_file_only_when_present_and_writable() {
        let dir = std::env::temp_dir().join(format!("portable_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PORTABLE_FILE);
        assert_eq!(portable_file_in(&dir), None);
        std::fs::write(&file, "").unwrap();
        assert_eq!(portable_file_in(&dir), Some(file.clone()));
        let mut perm = std::fs::metadata(&file).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&file, perm.clone()).unwrap();
        assert_eq!(portable_file_in(&dir), None);
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&file, perm).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

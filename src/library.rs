//! The library: the folders the slideshow takes its files from, kept one
//! per line in `library.txt` next to the settings file, and the scan that
//! lists the pictures and videos in them and in all their subfolders.
//!
//! The scan runs on a thread of its own (`scan`), started in `main` before
//! the window is made and again when the library changes or the user asks
//! for it; its result comes back through a channel with a repaint request.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

/// File extensions (lowercase) of pictures, decoded by Windows' own codecs
/// (WIC): those of Windows itself, and HEIC, AVIF and WebP when their
/// extensions from the Microsoft Store are installed.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "dib", "tif", "tiff", "webp", "heic",
    "heif", "hif", "avif", "jxr", "wdp", "ico",
];

/// File extensions of videos, decoded by Media Foundation without sound.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "wmv", "asf", "avi", "mkv", "webm", "3gp", "3g2", "mts", "m2ts", "ts",
];

/// What a file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

/// A picture or a video of the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub kind: Kind,
    /// When the file was last written (the order by date).
    pub modified: SystemTime,
    /// The library folder it was found in (its index): the order by name
    /// keeps the folders in the order they were added.
    pub root: usize,
}

/// The kind of `path` by its extension, `None` for other files.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// The file the folders are kept in, next to the settings file.
fn list_file() -> Option<PathBuf> {
    let settings = crate::settings::location().file.as_ref()?;
    Some(settings.with_file_name("library.txt"))
}

/// The library's folders, in the order they were added.
pub fn load() -> Vec<PathBuf> {
    list_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

fn parse(text: &str) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        if !folders.iter().any(|f| same_path(f, &path)) {
            folders.push(path);
        }
    }
    folders
}

/// Keep `folders` for the next run.
pub fn save(folders: &[PathBuf]) {
    let Some(file) = list_file() else { return };
    let mut text = String::new();
    for folder in folders {
        text.push_str(&folder.to_string_lossy());
        text.push_str("\r\n");
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&file, text) {
        log::warn!("cannot write {}: {e}", file.display());
    }
}

/// Paths compared as Windows does: without regard to case.
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .to_lowercase()
        .trim_end_matches('\\')
        == b.as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .trim_end_matches('\\')
}

/// The outcome of a scan.
pub struct Scanned {
    /// Which scan it is (`App` ignores the result of one replaced by a
    /// later scan).
    pub generation: u64,
    pub items: Vec<Item>,
}

/// List the pictures and videos of `folders` on a thread; the result comes
/// through `tx`, and `ctx` (once there is a window) is asked to repaint.
pub fn scan(
    folders: Vec<PathBuf>,
    generation: u64,
    tx: mpsc::Sender<Scanned>,
    ctx: std::sync::Arc<std::sync::OnceLock<egui::Context>>,
) {
    let spawned = std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let items = list(&folders);
            log::info!(
                "scanned {} folders: {} files in {:.0} ms",
                folders.len(),
                items.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            let _ = tx.send(Scanned { generation, items });
            if let Some(ctx) = ctx.get() {
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot start the scan: {e}");
    }
}

/// The pictures and videos of `folders` and their subfolders, each file
/// once: a folder of the library inside another one (`D:\Photos` and
/// `D:\Photos\2024`) would show its files twice. A file found again keeps
/// the first folder it was found in.
fn list(folders: &[PathBuf]) -> Vec<Item> {
    let mut items = Vec::new();
    for (root, folder) in folders.iter().enumerate() {
        walk(folder, root, &mut items);
    }
    let mut seen = std::collections::HashSet::with_capacity(items.len());
    items.retain(|i| seen.insert(i.path.as_os_str().to_string_lossy().to_lowercase()));
    items
}

/// Add the pictures and videos of `dir` and its subfolders to `out`.
/// Hidden and system folders (`$RECYCLE.BIN`, `System Volume Information`)
/// are skipped, and so are folders that are links or junctions, which
/// could lead round in a loop. Only those: OneDrive's folders and files
/// are reparse points too (of the cloud's tags, which are not links), and
/// a file leads nowhere.
fn walk(dir: &Path, root: usize, out: &mut Vec<Item>) {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const DIRECTORY: u32 = 0x10;
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::debug!("cannot read {}", dir.display());
        return;
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let attributes = meta.file_attributes();
        let path = entry.path();
        if attributes & DIRECTORY != 0 {
            // A link: a reparse point of a name-surrogate tag (symbolic
            // links, junctions), which std tells from the cloud's.
            let link = entry.file_type().is_ok_and(|t| t.is_symlink());
            if !link && attributes & (HIDDEN | SYSTEM) == 0 {
                subdirs.push(path);
            }
        } else if let Some(kind) = kind_of(&path) {
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(Item {
                path,
                kind,
                modified,
                root,
            });
        }
    }
    for sub in subdirs {
        walk(&sub, root, out);
    }
}

/// Paths in the order of Explorer: folder by folder, names compared without
/// regard to case and with runs of digits as numbers ("2" before "10").
pub fn path_cmp(a: &Path, b: &Path) -> Ordering {
    let mut ac = a.components();
    let mut bc = b.components();
    loop {
        match (ac.next(), bc.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = natural_cmp(
                    &x.as_os_str().to_string_lossy(),
                    &y.as_os_str().to_string_lossy(),
                );
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// `a` and `b` compared without regard to case, with runs of digits
/// compared as numbers.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (da, db) = (take(&mut a), take(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let o = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| da.len().cmp(&db.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind_of(Path::new(r"C:\a\IMG_1.JPG")), Some(Kind::Image));
        assert_eq!(kind_of(Path::new("clip.mp4")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new("notes.txt")), None);
        assert_eq!(kind_of(Path::new("noext")), None);
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["img10.jpg", "IMG2.jpg", "img1.jpg", "a.jpg", "img02.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            ["a.jpg", "img1.jpg", "IMG2.jpg", "img02.jpg", "img10.jpg"]
        );
        assert_eq!(
            path_cmp(Path::new(r"C:\p\b\1.jpg"), Path::new(r"C:\p\a\9.jpg")),
            Ordering::Greater
        );
        assert_eq!(
            path_cmp(Path::new(r"C:\p\2\x.jpg"), Path::new(r"C:\p\10\a.jpg")),
            Ordering::Less
        );
    }

    #[test]
    fn folders_list_without_repeats() {
        let folders = parse("C:\\Photos\r\n\r\nc:\\photos\\\nD:\\Video\n");
        assert_eq!(
            folders,
            [PathBuf::from(r"C:\Photos"), PathBuf::from(r"D:\Video")]
        );
    }

    #[test]
    fn walks_subfolders() {
        let dir = std::env::temp_dir().join(format!("qslides_walk_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        for f in ["a.jpg", "b.txt", "sub/c.png", "sub/deeper/d.mp4"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut items = Vec::new();
        walk(&dir, 3, &mut items);
        let mut names: Vec<String> = items
            .iter()
            .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "c.png", "d.mp4"]);
        assert!(items.iter().all(|i| i.root == 3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder of the library inside another one: its files once, kept
    /// under the folder that comes first.
    #[test]
    fn nested_folders_list_each_file_once() {
        let dir = std::env::temp_dir().join(format!("qslides_nested_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("2024")).unwrap();
        for f in ["a.jpg", "2024/b.jpg"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        // The inner folder written with other case and a trailing slash.
        let inner = PathBuf::from(format!("{}\\", dir.join("2024").display()).to_uppercase());
        let items = list(&[inner, dir.clone()]);
        let mut found: Vec<(String, usize)> = items
            .iter()
            .map(|i| {
                (
                    i.path.file_name().unwrap().to_string_lossy().to_lowercase(),
                    i.root,
                )
            })
            .collect();
        found.sort();
        assert_eq!(found, [("a.jpg".into(), 1), ("b.jpg".into(), 0)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A junction back to the folder above (made without rights by
    /// `mklink /J`) is not followed round; the folder's files are found.
    #[test]
    fn junctions_are_not_followed() {
        let dir = std::env::temp_dir().join(format!("qslides_junction_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.jpg"), b"x").unwrap();
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(dir.join("sub").join("loop"))
            .arg(&dir)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let mut items = Vec::new();
        walk(&dir, 0, &mut items);
        assert_eq!(items.len(), 1, "{items:?}");
        // The junction itself goes first: removing the folder must not go
        // through it.
        std::fs::remove_dir(dir.join("sub").join("loop")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

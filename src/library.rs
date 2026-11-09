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

/// File extensions of videos, decoded by Media Foundation (their sound played apart, `audio.rs`).
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
    let Some(file) = list_file() else {
        return Vec::new();
    };
    match std::fs::read(&file) {
        Ok(bytes) => parse(&decode(&bytes)),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("cannot read {}: {e}", file.display());
            }
            Vec::new()
        }
    }
}

/// The text of `library.txt` as Notepad may have saved it: UTF-8 with or
/// without its mark, UTF-16 with its mark, or in the system's ANSI code
/// page. It is written back as UTF-8 without a mark.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|&c| unit(c)).collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => ansi(bytes),
        }
    }
}

/// `bytes` in the system's ANSI code page.
fn ansi(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
    let Ok(len) = i32::try_from(bytes.len()) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    // SAFETY: the pointers and lengths are those of `bytes` and `wide`.
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; n as usize];
        let n = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
        wide.truncate(n.max(0) as usize);
        String::from_utf16_lossy(&wide)
    }
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

/// Paths compared as Windows does: without regard to case. Allocates
/// nothing: the file shown is looked for among all the library's.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (
        a.as_os_str().to_string_lossy(),
        b.as_os_str().to_string_lossy(),
    );
    let (a, b) = (a.trim_end_matches('\\'), b.trim_end_matches('\\'));
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// The outcome of a scan.
pub struct Scanned {
    /// Which scan it is (`App` ignores the result of one replaced by a
    /// later scan).
    pub generation: u64,
    pub items: Vec<Item>,
    /// The library's folders (their indexes) that could not be read: a
    /// network drive not connected yet, a removed USB stick. Told from
    /// folders that are only empty.
    pub unreachable: Vec<usize>,
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
            let (items, unreachable) = list(&folders);
            log::info!(
                "scanned {} folders: {} files in {:.0} ms",
                folders.len(),
                items.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            for &i in &unreachable {
                log::info!("cannot read the library's {}", folders[i].display());
            }
            let _ = tx.send(Scanned {
                generation,
                items,
                unreachable,
            });
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
/// the first folder it was found in. Also the folders (their indexes) that
/// could not be read.
fn list(folders: &[PathBuf]) -> (Vec<Item>, Vec<usize>) {
    let mut items = Vec::new();
    let mut unreachable = Vec::new();
    for (root, folder) in folders.iter().enumerate() {
        if !walk(folder, root, &mut items) {
            unreachable.push(root);
        }
    }
    let mut seen = std::collections::HashSet::with_capacity(items.len());
    items.retain(|i| seen.insert(i.path.as_os_str().to_string_lossy().to_lowercase()));
    (items, unreachable)
}

/// Add the pictures and videos of `dir` and its subfolders to `out`.
/// Hidden and system folders (`$RECYCLE.BIN`, `System Volume Information`)
/// are skipped, and so are folders that are links or junctions, which
/// could lead round in a loop. Only those: OneDrive's folders and files
/// are reparse points too (of the cloud's tags, which are not links), and
/// a file leads nowhere. Whether `dir` itself could be read.
fn walk(dir: &Path, root: usize, out: &mut Vec<Item>) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const DIRECTORY: u32 = 0x10;
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::debug!("cannot read {}", dir.display());
        return false;
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
    true
}

/// A path as [`key_cmp`] compares it: its components in lowercase, joined
/// by NUL, which comes before any character, so that a name ends before
/// any longer one and the order stays folder by folder. Made once a sort,
/// not in each of its comparisons (100 000 files took 1.8 s to sort by
/// name on the UI thread, allocating in every comparison).
pub fn sort_key(path: &Path) -> String {
    let mut key = String::new();
    for (n, c) in path.components().enumerate() {
        if n > 0 {
            key.push('\0');
        }
        key.push_str(&c.as_os_str().to_string_lossy().to_lowercase());
    }
    key
}

/// Paths, as their [`sort_key`]s, in the order of Explorer: folder by
/// folder, names compared without regard to case and with runs of digits
/// as numbers ("2" before "10").
pub fn key_cmp(a: &str, b: &str) -> Ordering {
    natural_lower(a, b)
}

/// [`key_cmp`] of two paths.
#[cfg(test)]
fn path_cmp(a: &Path, b: &Path) -> Ordering {
    key_cmp(&sort_key(a), &sort_key(b))
}

/// `a` and `b` compared without regard to case, with runs of digits
/// compared as numbers.
#[cfg(test)]
fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_lower(&a.to_lowercase(), &b.to_lowercase())
}

/// [`natural_cmp`] of strings already in lowercase, allocating nothing.
fn natural_lower(a: &str, b: &str) -> Ordering {
    let digits = |s: &str, from: usize| {
        from + s.as_bytes()[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    // The bytes the two have in common at the start (the library's folders
    // in the paths of a sort) are skipped at once: back to the start of
    // the character and of the run of digits there, they are where the
    // comparison of the rest begins in both, all before it equal.
    let mut start = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while start > 0 && !(a.is_char_boundary(start) && b.is_char_boundary(start)) {
        start -= 1;
    }
    while start > 0 && a.as_bytes()[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let (mut i, mut j) = (start, start);
    loop {
        match (a[i..].chars().next(), b[j..].chars().next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (ie, je) = (digits(a, i), digits(b, j));
                let (da, db) = (&a[i..ie], &b[j..je]);
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let o = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| da.len().cmp(&db.len()));
                if o != Ordering::Equal {
                    return o;
                }
                (i, j) = (ie, je);
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                i += x.len_utf8();
                j += y.len_utf8();
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
        // Folder by folder: a name ends before a longer one whatever
        // follows it ("a" before "a-b", though '\' is after '-').
        assert_eq!(
            path_cmp(Path::new(r"C:\p\a\z.jpg"), Path::new(r"C:\p\a-b\a.jpg")),
            Ordering::Less
        );
        // The common start skipped back to its run of digits: 12 after 9.
        assert_eq!(
            path_cmp(Path::new(r"C:\p\img12.jpg"), Path::new(r"C:\p\img19.jpg")),
            Ordering::Less
        );
        assert_eq!(
            path_cmp(Path::new(r"C:\p\img12.jpg"), Path::new(r"C:\p\img9.jpg")),
            Ordering::Greater
        );
        assert_eq!(natural_cmp("Фото 10", "фото 9"), Ordering::Greater);
        assert_eq!(natural_cmp("Фото", "фото"), Ordering::Equal);
        assert!(same_path(
            Path::new(r"C:\Фото\A.jpg"),
            Path::new(r"c:\фото\a.JPG")
        ));
        assert!(!same_path(
            Path::new(r"C:\Фото\A.jpg"),
            Path::new(r"C:\Фото\B.jpg")
        ));
    }

    #[test]
    fn folders_list_without_repeats() {
        let folders = parse("C:\\Photos\r\n\r\nc:\\photos\\\nD:\\Video\n");
        assert_eq!(
            folders,
            [PathBuf::from(r"C:\Photos"), PathBuf::from(r"D:\Video")]
        );
    }

    /// As Notepad saves it: the first folder is not lost to a byte order
    /// mark, and UTF-16 is read.
    #[test]
    fn folders_list_in_notepads_encodings() {
        let want = [PathBuf::from(r"C:\Фото"), PathBuf::from(r"D:\Video")];
        let text = "C:\\Фото\r\nD:\\Video\r\n";
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(text.as_bytes());
        assert_eq!(parse(&decode(&bom)), want);
        assert_eq!(parse(&decode(text.as_bytes())), want);
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for u in text.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(parse(&decode(&le)), want);
        assert_eq!(parse(&decode(&be)), want);
        // Not UTF-8: the ANSI code page, whatever it is; the ASCII folder
        // is read in any of them.
        let ansi = b"C:\\\xC0\r\nD:\\Video\r\n";
        assert_eq!(parse(&decode(ansi))[1], want[1]);
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
        // A folder that is not there (a network drive not connected yet):
        // told from an empty one.
        let gone = dir.join("not there");
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let (items, unreachable) = list(&[inner, gone, dir.clone(), empty]);
        assert_eq!(unreachable, [1]);
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
        assert_eq!(found, [("a.jpg".into(), 2), ("b.jpg".into(), 0)]);
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

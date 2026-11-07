//! The slideshow's choices (order, fit, videos, the window's shape, the
//! time a slide stays) and the playlist: the library's files in the order
//! chosen and the place in it.

use std::path::Path;

use crate::library::{Item, path_cmp};

/// The order the files are shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// Folder by folder, by name as in Explorer.
    #[default]
    Name,
    /// By the time the file was last written, the oldest first.
    Date,
    /// Shuffled; each file once a round, the next round shuffled anew.
    Random,
}

impl Order {
    pub const ALL: [Order; 3] = [Order::Name, Order::Date, Order::Random];

    pub fn label(self) -> &'static str {
        match self {
            Order::Name => tr!("By name", "По имени"),
            Order::Date => tr!("By date", "По дате"),
            Order::Random => tr!("Random", "Случайно"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Order::Name => "name",
            Order::Date => "date",
            Order::Random => "random",
        }
    }

    pub fn from_name(name: &str) -> Option<Order> {
        Order::ALL.into_iter().find(|o| o.name() == name)
    }
}

/// How a picture of other proportions than the window's fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Whole, with bars of the background.
    #[default]
    Fit,
    /// Over the whole window, its edges cut.
    Fill,
}

impl Fit {
    pub const ALL: [Fit; 2] = [Fit::Fit, Fit::Fill];

    pub fn label(self) -> &'static str {
        match self {
            Fit::Fit => tr!("Fit whole", "Вписать целиком"),
            Fit::Fill => tr!("Fill the window", "Заполнить окно"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fit::Fit => "fit",
            Fit::Fill => "fill",
        }
    }

    pub fn from_name(name: &str) -> Option<Fit> {
        Fit::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// How long a video stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoLength {
    /// To its end, however long that is.
    #[default]
    Whole,
    /// As long as a picture: cut off, or played again from its start.
    Interval,
}

impl VideoLength {
    pub const ALL: [VideoLength; 2] = [VideoLength::Whole, VideoLength::Interval];

    /// As a button of the menu, under the heading "Videos".
    pub fn short_label(self) -> &'static str {
        match self {
            VideoLength::Whole => tr!("To the end", "До конца"),
            VideoLength::Interval => tr!("As long as pictures", "Как картинки"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            VideoLength::Whole => "whole",
            VideoLength::Interval => "interval",
        }
    }

    pub fn from_name(name: &str) -> Option<VideoLength> {
        VideoLength::ALL.into_iter().find(|v| v.name() == name)
    }
}

/// The window's proportions, width to height; `None`: any.
pub const ASPECTS: [Option<(u32, u32)>; 11] = [
    None,
    Some((16, 9)),
    Some((16, 10)),
    Some((3, 2)),
    Some((4, 3)),
    Some((5, 4)),
    Some((1, 1)),
    Some((4, 5)),
    Some((3, 4)),
    Some((2, 3)),
    Some((9, 16)),
];

pub fn aspect_label(aspect: Option<(u32, u32)>) -> String {
    match aspect {
        None => tr!("Any", "Произвольные").into(),
        Some((w, h)) => format!("{w}:{h}"),
    }
}

/// `aspect` as kept in the settings ("16:9", "free").
pub fn aspect_name(aspect: Option<(u32, u32)>) -> String {
    match aspect {
        None => "free".into(),
        Some((w, h)) => format!("{w}:{h}"),
    }
}

pub fn aspect_from_name(name: &str) -> Option<Option<(u32, u32)>> {
    ASPECTS.into_iter().find(|a| aspect_name(*a) == name)
}

/// The times a slide can stay, in seconds: the slider's steps, finer at
/// the short end, where a second is felt, than at the long one.
pub const INTERVALS: [u32; 29] = [
    1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 15, 20, 25, 30, 40, 45, 60, 90, 120, 180, 240, 300, 420, 600,
    900, 1200, 1800, 2700, 3600,
];

/// The step of [`INTERVALS`] nearest to `secs` (a setting written by hand
/// may be between them).
pub fn nearest_interval(secs: u32) -> usize {
    (0..INTERVALS.len())
        .min_by_key(|&i| INTERVALS[i].abs_diff(secs))
        .unwrap_or(0)
}

pub fn interval_label(secs: u32) -> String {
    let (m, s) = (secs / 60, secs % 60);
    match secs {
        _ if secs < 60 => tr!(format!("{secs} s"), format!("{secs} с")),
        _ if secs >= 3600 && secs.is_multiple_of(3600) => {
            tr!(format!("{} h", secs / 3600), format!("{} ч", secs / 3600))
        }
        _ if s == 0 => tr!(format!("{m} min"), format!("{m} мин")),
        _ => tr!(format!("{m} min {s} s"), format!("{m} мин {s} с")),
    }
}

/// A small random number generator (xorshift64*): the shuffle needs no
/// more, and no crate.
struct Rng(u64);

impl Rng {
    fn seeded() -> Rng {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n` (`n` > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The library's files in the order of the show, and the one shown.
pub struct Playlist {
    items: Vec<Item>,
    order: Order,
    /// Indexes into `items`, in the order they are shown.
    sequence: Vec<usize>,
    /// The place in `sequence` of the file shown.
    pos: usize,
    rng: Rng,
}

impl Playlist {
    pub fn new(order: Order) -> Playlist {
        Playlist {
            items: Vec::new(),
            order,
            sequence: Vec::new(),
            pos: 0,
            rng: Rng::seeded(),
        }
    }

    /// Nothing to show: the library has no files, or none of them could be
    /// shown.
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    /// The files in the show (those that could not be shown left out).
    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    /// The library has files, but none of them could be shown.
    pub fn all_failed(&self) -> bool {
        !self.items.is_empty() && self.sequence.is_empty()
    }

    /// The place of the current file in the show, from 1, and the number of
    /// files in it (those that could not be shown are no longer counted);
    /// `None` while it is empty.
    pub fn position(&self) -> Option<(usize, usize)> {
        (!self.sequence.is_empty()).then(|| (self.pos + 1, self.sequence.len()))
    }

    /// The files of a new scan; the show goes on from `keep` (the file
    /// shown, or the one shown when the program was closed) if it is still
    /// there, else from the start. Whether `keep` was found.
    pub fn set_items(&mut self, items: Vec<Item>, keep: Option<&Path>) -> bool {
        self.items = items;
        self.arrange(keep)
    }

    pub fn order(&self) -> Order {
        self.order
    }

    /// Another order; the file shown stays.
    pub fn set_order(&mut self, order: Order) {
        if order != self.order {
            self.order = order;
            let keep = self.current().map(|i| i.path.clone());
            let _ = self.arrange(keep.as_deref());
        }
    }

    fn arrange(&mut self, keep: Option<&Path>) -> bool {
        let items = &self.items;
        let mut sequence: Vec<usize> = (0..items.len()).collect();
        match self.order {
            Order::Name => sequence.sort_by(|&a, &b| {
                items[a]
                    .root
                    .cmp(&items[b].root)
                    .then_with(|| path_cmp(&items[a].path, &items[b].path))
            }),
            Order::Date => sequence.sort_by(|&a, &b| {
                items[a]
                    .modified
                    .cmp(&items[b].modified)
                    .then_with(|| path_cmp(&items[a].path, &items[b].path))
            }),
            Order::Random => self.shuffle(&mut sequence),
        }
        self.sequence = sequence;
        let found = keep.and_then(|k| {
            self.sequence
                .iter()
                .position(|&i| crate::library::same_path(&self.items[i].path, k))
        });
        match found {
            // A shuffled round starts from the file kept, so that the rest
            // of the round is new.
            Some(p) if self.order == Order::Random => {
                self.sequence.swap(0, p);
                self.pos = 0;
            }
            Some(p) => self.pos = p,
            None => self.pos = 0,
        }
        found.is_some()
    }

    /// Fisher–Yates.
    fn shuffle(&mut self, sequence: &mut [usize]) {
        for i in (1..sequence.len()).rev() {
            let j = self.rng.below(i + 1);
            sequence.swap(i, j);
        }
    }

    pub fn current(&self) -> Option<&Item> {
        self.sequence.get(self.pos).map(|&i| &self.items[i])
    }

    /// The file after the current one, without moving on (to decode it
    /// ahead). At the end of a shuffled round it is unknown until the next
    /// round is shuffled: `None`.
    pub fn peek_next(&self) -> Option<&Item> {
        if self.sequence.is_empty() {
            return None;
        }
        if self.pos + 1 < self.sequence.len() {
            return Some(&self.items[self.sequence[self.pos + 1]]);
        }
        match self.order {
            Order::Random if self.sequence.len() > 1 => None,
            _ => Some(&self.items[self.sequence[0]]),
        }
    }

    /// Move on to the next file; after the last, round to the first (a new
    /// shuffle, which does not start with the file just shown).
    pub fn advance(&mut self) -> Option<&Item> {
        if self.sequence.is_empty() {
            return None;
        }
        self.pos += 1;
        if self.pos >= self.sequence.len() {
            self.pos = 0;
            if self.order == Order::Random && self.sequence.len() > 1 {
                let last = *self.sequence.last().expect("not empty");
                let mut sequence = std::mem::take(&mut self.sequence);
                self.shuffle(&mut sequence);
                if sequence[0] == last {
                    let j = 1 + self.rng.below(sequence.len() - 1);
                    sequence.swap(0, j);
                }
                self.sequence = sequence;
            }
        }
        self.current()
    }

    /// Back to the file before (in a shuffled round, the one shown before).
    pub fn back(&mut self) -> Option<&Item> {
        if self.sequence.is_empty() {
            return None;
        }
        self.pos = self.pos.checked_sub(1).unwrap_or(self.sequence.len() - 1);
        self.current()
    }

    /// Drop the current file from the show (it cannot be decoded): the
    /// next one takes its place.
    pub fn remove_current(&mut self) {
        if self.pos < self.sequence.len() {
            self.sequence.remove(self.pos);
            if self.pos >= self.sequence.len() {
                self.pos = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Kind;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn items() -> Vec<Item> {
        ["b10.jpg", "b2.jpg", "a.jpg", "c.mp4"]
            .iter()
            .enumerate()
            .map(|(i, n)| Item {
                path: PathBuf::from(format!(r"C:\p\{n}")),
                kind: Kind::Image,
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(100 - i as u64),
                root: 0,
            })
            .collect()
    }

    fn name(p: &Playlist) -> String {
        p.current()
            .unwrap()
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn by_name_and_round() {
        let mut p = Playlist::new(Order::Name);
        assert_eq!(p.position(), None);
        p.set_items(items(), None);
        assert_eq!(name(&p), "a.jpg");
        assert_eq!(p.position(), Some((1, 4)));
        p.advance();
        assert_eq!(name(&p), "b2.jpg");
        assert_eq!(p.position(), Some((2, 4)));
        assert_eq!(p.peek_next().unwrap().path, PathBuf::from(r"C:\p\b10.jpg"));
        p.advance();
        p.advance();
        assert_eq!(name(&p), "c.mp4");
        p.advance();
        assert_eq!(name(&p), "a.jpg");
        p.back();
        assert_eq!(name(&p), "c.mp4");
    }

    #[test]
    fn by_date_keeps_the_current_file() {
        let mut p = Playlist::new(Order::Name);
        assert!(p.set_items(items(), Some(Path::new(r"c:\P\B10.JPG"))));
        assert_eq!(name(&p), "b10.jpg");
        // A file no longer there: from the start, and said so.
        let mut q = Playlist::new(Order::Name);
        assert!(!q.set_items(items(), Some(Path::new(r"C:\p\gone.jpg"))));
        assert_eq!(name(&q), "a.jpg");
        p.set_order(Order::Date);
        assert_eq!(name(&p), "b10.jpg");
        p.advance();
        // The oldest first: c.mp4 (97), a.jpg (98), b2.jpg (99), b10.jpg (100).
        assert_eq!(name(&p), "c.mp4");
    }

    #[test]
    fn random_shows_each_once_a_round() {
        let mut p = Playlist::new(Order::Random);
        p.set_items(items(), Some(Path::new(r"C:\p\a.jpg")));
        assert_eq!(name(&p), "a.jpg");
        for _ in 0..5 {
            let mut seen = vec![name(&p)];
            for _ in 0..3 {
                p.advance();
                seen.push(name(&p));
            }
            let last = seen.last().unwrap().clone();
            seen.sort();
            assert_eq!(seen, ["a.jpg", "b10.jpg", "b2.jpg", "c.mp4"]);
            p.advance();
            assert_ne!(name(&p), last, "a new round does not repeat the last file");
        }
    }

    #[test]
    fn a_broken_file_is_dropped() {
        let mut p = Playlist::new(Order::Name);
        p.set_items(items(), Some(Path::new(r"C:\p\c.mp4")));
        p.remove_current();
        assert_eq!(name(&p), "a.jpg");
        assert_eq!(p.len(), 3);
        assert!(!p.all_failed());
        // None of them could be shown: empty, though the library is not.
        for _ in 0..3 {
            p.remove_current();
        }
        assert!(p.is_empty() && p.all_failed());
        assert!(p.current().is_none() && p.peek_next().is_none());
        let mut empty = Playlist::new(Order::Name);
        assert!(!empty.all_failed());
        assert!(empty.advance().is_none());
        assert!(empty.peek_next().is_none());
    }

    #[test]
    fn choices_are_stored_by_name() {
        for o in Order::ALL {
            assert_eq!(Order::from_name(o.name()), Some(o));
        }
        for f in Fit::ALL {
            assert_eq!(Fit::from_name(f.name()), Some(f));
        }
        for v in VideoLength::ALL {
            assert_eq!(VideoLength::from_name(v.name()), Some(v));
        }
        for a in ASPECTS {
            assert_eq!(aspect_from_name(&aspect_name(a)), Some(a));
        }
        assert_eq!(aspect_from_name("7:5"), None);
    }

    #[test]
    fn interval_steps() {
        assert!(INTERVALS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(INTERVALS[nearest_interval(10)], 10);
        assert_eq!(INTERVALS[nearest_interval(11)], 10);
        assert_eq!(INTERVALS[nearest_interval(0)], 1);
        assert_eq!(INTERVALS[nearest_interval(1)], 1);
        assert_eq!(INTERVALS[nearest_interval(86_400)], 3600);
        crate::i18n::set_lang(crate::i18n::Lang::En);
        assert_eq!(interval_label(45), "45 s");
        assert_eq!(interval_label(90), "1 min 30 s");
        assert_eq!(interval_label(600), "10 min");
        assert_eq!(interval_label(3600), "1 h");
    }
}

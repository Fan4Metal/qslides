//! The widget's menu, the one control it has besides its buttons: a menu of
//! Windows (`TrackPopupMenuEx`), not of egui, because it opens beyond the
//! edges of a small window. The same menu comes up on a right click on the
//! window and on the tray icon.

use std::path::PathBuf;

use crate::show::{ASPECTS, Fit, INTERVALS, Order, VideoLength, aspect_label, interval_label};

/// What the menu can ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Previous,
    Next,
    Pause,
    OpenFile,
    ShowInFolder,
    AddFolder,
    OpenFolder(usize),
    RemoveFolder(usize),
    Rescan,
    Interval(u32),
    Order(Order),
    Fit(Fit),
    Video(VideoLength),
    Aspect(Option<(u32, u32)>),
    OnTop,
    Autostart,
    TrayOnly,
    About,
    /// Open the page of the newer release.
    Update,
    Exit,
}

/// What the menu shows checked and which items it has.
pub struct State {
    pub has_file: bool,
    pub paused: bool,
    pub folders: Vec<PathBuf>,
    pub interval: u32,
    pub order: Order,
    pub fit: Fit,
    pub video: VideoLength,
    pub aspect: Option<(u32, u32)>,
    pub on_top: bool,
    pub autostart: bool,
    pub tray_only: bool,
    /// The version of a newer release, when one was found.
    pub update: Option<String>,
}

/// One item of the menu as it is built.
enum Entry {
    Item {
        cmd: Cmd,
        label: String,
        checked: bool,
        enabled: bool,
    },
    /// Items checked as one choice (a dot instead of a tick).
    Choice(Vec<(Cmd, String)>, usize),
    Sub(String, Vec<Entry>),
    Separator,
}

fn item(cmd: Cmd, label: impl Into<String>) -> Entry {
    Entry::Item {
        cmd,
        label: label.into(),
        checked: false,
        enabled: true,
    }
}

fn check(cmd: Cmd, label: impl Into<String>, checked: bool) -> Entry {
    Entry::Item {
        cmd,
        label: label.into(),
        checked,
        enabled: true,
    }
}

fn enabled(cmd: Cmd, label: impl Into<String>, on: bool) -> Entry {
    Entry::Item {
        cmd,
        label: label.into(),
        checked: false,
        enabled: on,
    }
}

/// `&` shows the next letter underlined in a menu; a path's own is doubled.
fn escaped(text: &str) -> String {
    text.replace('&', "&&")
}

/// The menu's items for `s`, in their order.
fn entries(s: &State) -> Vec<Entry> {
    let mut folders: Vec<Entry> = s
        .folders
        .iter()
        .enumerate()
        .map(|(i, f)| {
            Entry::Sub(
                escaped(&f.display().to_string()),
                vec![
                    item(
                        Cmd::OpenFolder(i),
                        tr!("Open in Explorer", "Открыть в Проводнике"),
                    ),
                    item(
                        Cmd::RemoveFolder(i),
                        tr!("Remove from the library", "Убрать из библиотеки"),
                    ),
                ],
            )
        })
        .collect();
    if !folders.is_empty() {
        folders.push(Entry::Separator);
    }
    folders.push(item(
        Cmd::AddFolder,
        tr!("Add a folder…\tCtrl+O", "Добавить папку…\tCtrl+O"),
    ));
    folders.push(enabled(
        Cmd::Rescan,
        tr!("Look for new files\tF5", "Найти новые файлы\tF5"),
        !s.folders.is_empty(),
    ));

    let intervals = INTERVALS
        .iter()
        .map(|&n| (Cmd::Interval(n), interval_label(n)))
        .collect();
    let interval_at = INTERVALS
        .iter()
        .position(|&n| n == s.interval)
        .unwrap_or(usize::MAX);
    let orders = Order::ALL
        .iter()
        .map(|&o| (Cmd::Order(o), o.label().to_owned()))
        .collect();
    let order_at = Order::ALL.iter().position(|&o| o == s.order).unwrap_or(0);
    let fits = Fit::ALL
        .iter()
        .map(|&f| (Cmd::Fit(f), f.label().to_owned()))
        .collect();
    let fit_at = Fit::ALL.iter().position(|&f| f == s.fit).unwrap_or(0);
    let videos = VideoLength::ALL
        .iter()
        .map(|&v| (Cmd::Video(v), v.label().to_owned()))
        .collect();
    let video_at = VideoLength::ALL
        .iter()
        .position(|&v| v == s.video)
        .unwrap_or(0);
    let aspects = ASPECTS
        .iter()
        .map(|&a| (Cmd::Aspect(a), aspect_label(a)))
        .collect();
    let aspect_at = ASPECTS.iter().position(|&a| a == s.aspect).unwrap_or(0);

    let mut all = Vec::new();
    if let Some(v) = &s.update {
        all.push(item(
            Cmd::Update,
            tr!(
                format!("Version {v} is available…"),
                format!("Доступна версия {v}…")
            ),
        ));
        all.push(Entry::Separator);
    }
    all.extend([
        enabled(
            Cmd::Previous,
            tr!("Previous\tLeft", "Предыдущий\tВлево"),
            s.has_file,
        ),
        enabled(
            Cmd::Next,
            tr!("Next\tRight", "Следующий\tВправо"),
            s.has_file,
        ),
        Entry::Item {
            cmd: Cmd::Pause,
            label: tr!("Pause\tSpace", "Пауза\tПробел").into(),
            checked: s.paused,
            enabled: s.has_file,
        },
        Entry::Separator,
        enabled(
            Cmd::OpenFile,
            tr!("Open the file\tEnter", "Открыть файл\tEnter"),
            s.has_file,
        ),
        enabled(
            Cmd::ShowInFolder,
            tr!("Show in its folder", "Показать в папке"),
            s.has_file,
        ),
        Entry::Separator,
        Entry::Sub(tr!("Folders", "Папки").into(), folders),
        Entry::Sub(
            tr!("Show each for", "Время показа").into(),
            vec![Entry::Choice(intervals, interval_at)],
        ),
        Entry::Sub(
            tr!("Order", "Порядок").into(),
            vec![Entry::Choice(orders, order_at)],
        ),
        Entry::Sub(
            tr!("Picture", "Картинка").into(),
            vec![
                Entry::Choice(fits, fit_at),
                Entry::Separator,
                Entry::Choice(videos, video_at),
            ],
        ),
        Entry::Sub(
            tr!("Window proportions", "Пропорции окна").into(),
            vec![Entry::Choice(aspects, aspect_at)],
        ),
        Entry::Separator,
        check(
            Cmd::OnTop,
            tr!("Always on top", "Поверх всех окон"),
            s.on_top,
        ),
        check(
            Cmd::Autostart,
            tr!("Start with Windows", "Запускать вместе с Windows"),
            s.autostart,
        ),
        check(
            Cmd::TrayOnly,
            tr!("Only in the tray", "Только в трее"),
            s.tray_only,
        ),
        Entry::Separator,
        item(
            Cmd::About,
            tr!(
                format!("About {}\tF1", crate::APP_NAME),
                format!("О программе {}\tF1", crate::APP_NAME)
            ),
        ),
        item(Cmd::Exit, tr!("Exit", "Выход")),
    ]);
    all
}

/// Show the menu at `(x, y)` (screen pixels) for window `hwnd`, which is
/// put in front first, so that the menu closes when clicked away from;
/// the command chosen. It waits until the menu closes.
pub fn show(hwnd: isize, x: i32, y: i32, state: &State) -> Option<Cmd> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyMenu, PostMessageW, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        TrackPopupMenuEx, WM_NULL,
    };
    let mut cmds = Vec::new();
    // SAFETY: a menu made, shown and destroyed here, on the window's thread.
    unsafe {
        let menu = build(&entries(state), &mut cmds)?;
        SetForegroundWindow(hwnd as _);
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            hwnd as _,
            std::ptr::null(),
        );
        PostMessageW(hwnd as _, WM_NULL, 0, 0);
        DestroyMenu(menu);
        (chosen as usize)
            .checked_sub(1)
            .and_then(|i| cmds.get(i).cloned())
    }
}

/// A popup menu of `entries`; each command's id is its place in `cmds`
/// plus one.
unsafe fn build(
    entries: &[Entry],
    cmds: &mut Vec<Cmd>,
) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HMENU> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CheckMenuRadioItem, CreatePopupMenu, MF_BYCOMMAND, MF_CHECKED, MF_GRAYED,
        MF_POPUP, MF_SEPARATOR, MF_STRING,
    };
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return None;
    }
    for entry in entries {
        match entry {
            Entry::Item {
                cmd,
                label,
                checked,
                enabled,
            } => {
                cmds.push(cmd.clone());
                let mut flags = MF_STRING;
                if *checked {
                    flags |= MF_CHECKED;
                }
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                let label = crate::win::wide(label);
                unsafe { AppendMenuW(menu, flags, cmds.len(), label.as_ptr()) };
            }
            Entry::Choice(choices, at) => {
                let first = cmds.len() + 1;
                for (cmd, label) in choices {
                    cmds.push(cmd.clone());
                    let label = crate::win::wide(label);
                    unsafe { AppendMenuW(menu, MF_STRING, cmds.len(), label.as_ptr()) };
                }
                if *at < choices.len() {
                    let last = cmds.len();
                    unsafe {
                        CheckMenuRadioItem(
                            menu,
                            first as u32,
                            last as u32,
                            (first + at) as u32,
                            MF_BYCOMMAND,
                        )
                    };
                }
            }
            Entry::Sub(label, children) => {
                let Some(sub) = (unsafe { build(children, cmds) }) else {
                    continue;
                };
                let label = crate::win::wide(label);
                unsafe { AppendMenuW(menu, MF_POPUP, sub as usize, label.as_ptr()) };
            }
            Entry::Separator => unsafe {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            },
        }
    }
    Some(menu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            has_file: true,
            paused: false,
            folders: vec![PathBuf::from(r"C:\Photos & Co")],
            interval: 10,
            order: Order::Random,
            fit: Fit::Fill,
            video: VideoLength::Whole,
            aspect: Some((16, 9)),
            on_top: true,
            autostart: false,
            tray_only: false,
            update: Some("0.2.0".into()),
        }
    }

    /// Every command of the menu, in the order of the ids it is given.
    fn commands(entries: &[Entry], out: &mut Vec<Cmd>) {
        for e in entries {
            match e {
                Entry::Item { cmd, .. } => out.push(cmd.clone()),
                Entry::Choice(choices, _) => out.extend(choices.iter().map(|(c, _)| c.clone())),
                Entry::Sub(_, children) => commands(children, out),
                Entry::Separator => {}
            }
        }
    }

    #[test]
    fn the_menu_holds_every_choice() {
        let mut cmds = Vec::new();
        commands(&entries(&state()), &mut cmds);
        for cmd in [
            Cmd::RemoveFolder(0),
            Cmd::Interval(3600),
            Cmd::Order(Order::Date),
            Cmd::Aspect(None),
            Cmd::Exit,
        ] {
            assert!(cmds.contains(&cmd), "{cmd:?}");
        }
        let unique: std::collections::HashSet<String> =
            cmds.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(unique.len(), cmds.len(), "no command twice");
        assert_eq!(escaped(r"C:\Photos & Co"), r"C:\Photos && Co");
    }
}

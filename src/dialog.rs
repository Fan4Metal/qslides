//! The folder dialog, on a thread of its own: shown on the UI thread, the
//! dialog runs its own message loop inside the frame that opened it, and
//! the show stood still until it closed (a video stopped on its frame).
//! The widget is still the dialog's owner, so Windows disables it
//! meanwhile, as for a modal dialog; the show goes on. One dialog at a
//! time; the answer is taken each frame ([`FolderDialog::take`]).
//! metalmpv's `dialog.rs`, with only the folder to pick.

use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};

#[derive(Default)]
pub struct FolderDialog {
    pending: Option<Receiver<Option<PathBuf>>>,
}

impl FolderDialog {
    /// A dialog is open.
    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Ask for a folder, the dialog titled `title` and owned by window
    /// `owner`; `ctx` is woken with the answer. Nothing while one is open.
    pub fn start(&mut self, ctx: &egui::Context, owner: Option<isize>, title: String) {
        if self.is_open() {
            return;
        }
        let (send, receive) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("folder dialog".into())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title(title);
                if let Some(owner) = owner.and_then(NonZeroIsize::new) {
                    dialog = dialog.set_parent(&Owner(owner));
                }
                let _ = send.send(dialog.pick_folder());
                ctx.request_repaint();
            });
        match spawned {
            Ok(_) => self.pending = Some(receive),
            Err(e) => log::warn!("cannot show the folder dialog: {e}"),
        }
    }

    /// The folder chosen, once the dialog is closed with one.
    pub fn take(&mut self) -> Option<PathBuf> {
        let answer = match self.pending.as_ref()?.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        self.pending = None;
        answer
    }
}

/// The widget's window as a dialog's owner, from another thread.
struct Owner(NonZeroIsize);

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // The window lives as long as the app, longer than its dialogs.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

//! Pictures decoded ahead: one thread with COM (`media::Com`) takes the
//! next file of the show while the current slide is up, decodes it with
//! WIC, scaled down to the screen, and hands the texture back, so that the
//! slide changes at once. Only the latest request counts: requests made
//! while one is decoded replace each other.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{TextureHandle, TextureOptions};

struct Job {
    id: u64,
    path: PathBuf,
    max_side: u32,
}

/// A decoded picture, or why there is none.
pub struct Done {
    pub id: u64,
    pub result: Result<TextureHandle, String>,
}

pub struct Loader {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    next_id: u64,
}

impl Loader {
    pub fn new(ctx: egui::Context) -> Loader {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("loader".into())
            .spawn(move || {
                let _com = crate::media::Com::init();
                while let Ok(mut job) = jobs.recv() {
                    // Only the latest request.
                    while let Ok(later) = jobs.try_recv() {
                        job = later;
                    }
                    let started = std::time::Instant::now();
                    let result = crate::media::decode_image(&job.path, job.max_side).map(|image| {
                        let size = image.size;
                        let options = TextureOptions::LINEAR
                            .with_mipmap_mode(Some(egui::TextureFilter::Linear));
                        let texture = ctx.load_texture(job.path.to_string_lossy(), image, options);
                        log::debug!(
                            "{}: {}x{} in {:.0} ms",
                            job.path.display(),
                            size[0],
                            size[1],
                            started.elapsed().as_secs_f64() * 1e3
                        );
                        texture
                    });
                    if done.send(Done { id: job.id, result }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot start the loader: {e}");
        }
        Loader { tx, rx, next_id: 1 }
    }

    /// Ask for the picture in `path`, at most `max_side` pixels on each
    /// side; its id, which [`Done`] comes back with.
    pub fn request(&mut self, path: PathBuf, max_side: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.tx.send(Job { id, path, max_side });
        id
    }

    /// The pictures decoded since the last call.
    pub fn take(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

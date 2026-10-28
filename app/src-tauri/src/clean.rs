//! Source steps (such as AI denoise) for the editor. They run on the
//! full-size photo and can take minutes, so they run on a background thread
//! while the preview keeps showing the last result. The frontend hears
//! about progress through `clean-progress` events and asks for a new
//! preview on `clean-done`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aperio_core::EditDocument;
use aperio_plugin::{Host, Progress};
use aperio_render::{LinearImage, Renderer};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{Session, PREVIEW_EDGE};

#[derive(Default)]
struct Queue {
    /// Whether the worker thread is running.
    running: bool,
    /// The latest steps the editor wants, by key, waiting for the worker.
    wanted: Option<(String, EditDocument)>,
    /// The key the worker is running now.
    current: Option<String>,
}

/// Where a run stands, for the editor.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// Share done, 0 to 1, while running.
    pub done: Option<f32>,
    /// Why the last run's steps couldn't run (e.g. a model that isn't
    /// downloaded).
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Cleaner {
    queue: Mutex<Queue>,
    /// Bumped when the photo changes, so work for the old one stops.
    photo: AtomicU64,
    status: Mutex<Status>,
}

impl Cleaner {
    /// Stop work on the previous photo.
    pub fn new_photo(&self) {
        self.photo.fetch_add(1, Ordering::SeqCst);
        let mut queue = self.queue.lock().unwrap();
        queue.wanted = None;
        queue.current = None;
        drop(queue);
        *self.status.lock().unwrap() = Status::default();
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Ask for the preview after `doc`'s source steps (which have `key`).
    /// A run already going finishes first: its network work is kept by the
    /// plugin, so the next run, with new slider values, is quick.
    pub fn request(
        self: &Arc<Self>,
        key: String,
        doc: EditDocument,
        app: &AppHandle,
        session: &Arc<Mutex<Option<Session>>>,
        host: &Arc<Host>,
    ) {
        let mut queue = self.queue.lock().unwrap();
        if queue.current.as_ref() == Some(&key) {
            // Already running; drop anything queued after it.
            queue.wanted = None;
            return;
        }
        queue.wanted = Some((key, doc));
        if queue.running {
            return;
        }
        queue.running = true;
        let (me, app, session, host) = (self.clone(), app.clone(), session.clone(), host.clone());
        std::thread::spawn(move || me.work(&app, &session, host));
    }

    fn work(&self, app: &AppHandle, session: &Mutex<Option<Session>>, host: Arc<Host>) {
        // A CPU renderer is enough: only plugins run here, and the shared
        // renderer stays free for previews.
        let renderer = Renderer::cpu().with_plugins(host);
        loop {
            let (key, doc) = {
                let mut queue = self.queue.lock().unwrap();
                match queue.wanted.take() {
                    Some(job) => {
                        queue.current = Some(job.0.clone());
                        job
                    }
                    None => {
                        queue.running = false;
                        queue.current = None;
                        return;
                    }
                }
            };
            let photo = self.photo.load(Ordering::SeqCst);
            let Some(mut full) = session.lock().unwrap().as_ref().map(|s| s.full.clone()) else {
                continue;
            };
            self.status.lock().unwrap().done = Some(0.0);
            let report = |done: f32| {
                self.status.lock().unwrap().done = Some(done);
                let _ = app.emit("clean-progress", done);
                self.photo.load(Ordering::SeqCst) == photo
            };
            let result = renderer.run_source_steps(&doc, &mut full, Progress::new(&report));
            if self.photo.load(Ordering::SeqCst) != photo {
                // Another photo was opened: this result is for the old one.
                let _ = app.emit("clean-done", Status::default());
                continue;
            }
            let error = match result {
                Ok(skipped) => {
                    let cleaned = full.downscale(PREVIEW_EDGE);
                    if let Some(s) = session.lock().unwrap().as_mut() {
                        s.cleaned = Some((key, cleaned));
                    }
                    let reasons: Vec<String> = skipped.into_iter().map(|s| s.reason).collect();
                    (!reasons.is_empty()).then(|| reasons.join("; "))
                }
                Err(_) => None,
            };
            *self.status.lock().unwrap() = Status { done: None, error };
            let _ = app.emit("clean-done", self.status());
        }
    }
}

/// What the preview should start from: the original when the document has
/// no source steps; the result for these steps when it's ready; otherwise
/// the last result (or the original) while the background run catches up.
/// Returns the image and, when a run is needed, the steps' key.
pub fn preview_source(
    r: &Renderer,
    session: &Mutex<Option<Session>>,
    doc: &EditDocument,
    preview: LinearImage,
) -> (LinearImage, Option<String>) {
    let steps = r.source_steps(doc);
    if steps.is_empty() {
        return (preview, None);
    }
    let key = format!("{steps:?}");
    match session
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.cleaned.as_ref())
    {
        Some((k, img)) if *k == key => (img.clone(), None),
        Some((_, img)) => (img.clone(), Some(key)),
        None => (preview, Some(key)),
    }
}

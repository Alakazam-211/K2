//! Linux screen capture: the portal picks (it is the only way to choose a
//! screen on Linux), PipeWire streams.

use super::pipewire::{self, Capture};
use super::portal;
use crate::api::{Done, FrameSource, Wake};
use crate::error::{unsupported, VideoError};
use crate::kit::{lock, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use std::sync::{Arc, Mutex, OnceLock};

/// The portal's choice, until `open_screen` takes it (once).
pub(crate) struct Picked(Mutex<Option<portal::Session>>);

/// Can this session share a screen: libpipewire and a ScreenCast portal?
/// (Probed once; a desktop that gains a portal later needs a restart.)
pub(crate) fn supported() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| pipewire::available() && portal::offer().is_ok())
}

/// Window sharing too?
pub(crate) fn windows_supported() -> bool {
    supported() && portal::offer().is_ok_and(|o| o.source_types & portal::SOURCE_WINDOW != 0)
}

/// Show the portal's picker on a thread of its own; `done` gets the
/// choice, `NotFound` when cancelled.
pub(crate) fn pick(opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
    if !pipewire::available() {
        return done(Err(unsupported("screen capture needs PipeWire (libpipewire-0.3)")));
    }
    // `done` runs exactly once: on the thread, or here if it can't start.
    let slot = Arc::new(Mutex::new(Some(done)));
    let s = slot.clone();
    let spawned = std::thread::Builder::new().name("kessel-video-portal".into()).spawn(move || {
        let r = portal::pick(opts).map(|s| ScreenSelection::Picked(Arc::new(Picked(Mutex::new(Some(s))))));
        if let Some(done) = lock(&s).take() {
            done(r);
        }
    });
    if let Err(e) = spawned {
        if let Some(done) = lock(&slot).take() {
            done(Err(VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}"))));
        }
    }
}

struct ScreenControl {
    // Dropped in this order: the stream, then the portal session.
    _capture: Capture,
    _session: portal::Session,
}

impl SourceControl for ScreenControl {}

pub(crate) fn open(sel: &ScreenSelection, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    let ScreenSelection::Picked(any) = sel else {
        // Linux has no list to choose from: only the portal picks.
        return Err(VideoError::NotFound);
    };
    let picked = any.downcast_ref::<Picked>().ok_or(VideoError::NotFound)?;
    let mut session = lock(&picked.0).take().ok_or_else(|| VideoError::Invalid("this portal choice was already opened; pick again".into()))?;
    let remote = session.remote.take().ok_or(VideoError::NotFound)?;
    let fps = c.fps.unwrap_or(30);
    let want = match (c.max_width, c.max_height, session.stream.width, session.stream.height) {
        (_, _, w, h) if w > 0 && h > 0 => (w, h),
        (Some(w), Some(h), _, _) => (w, h),
        _ => (1920, 1080),
    };
    let shared = SourceShared::new(c, fps, wake);
    let capture = Capture::start(remote, session.stream.node_id, fps, want, session.stream.window, shared.clone())?;
    Ok(Box::new(QueuedSource::new(
        shared,
        ScreenControl {
            _capture: capture,
            _session: session,
        },
    )))
}

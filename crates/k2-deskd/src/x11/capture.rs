//! X11 capture (D8b): kessel-video's X11 screen source (MIT-SHM reads,
//! DAMAGE, the XFIXES cursor drawn in), opened without a picker, feeding
//! the engine's software VP8 and JPEG layers.
//!
//! - **One display**, id 0: the X screen deskd is connected to (kessel-video
//!   lists it as `x11:<screen>`).
//! - **Its size is read live** from the root window, so a RandR change
//!   shows at the next plan; the source itself reports the new size on its
//!   next frame.
//! - **Frames only on change.** A still screen gives none; the engine's
//!   keyframe repeat covers a viewer joining then. The source's first frame
//!   comes at once, so a capture opened on a display that was just thawed
//!   (asleep, then woken: nothing on it has drawn since) still starts with
//!   a keyframe.
//! - **The cursor is drawn into the picture**, as on macOS
//!   ([`CURSOR_IN_PICTURE`]).

use std::sync::Arc;

use kessel_video::{CaptureConstraints, FrameSource, ScreenSelection, VideoBackend, VideoError, Wake};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::capture::{Opener, Plan};

/// Draw the pointer into the captured picture (the out-of-band cursor
/// comes later, as on macOS).
pub const CURSOR_IN_PICTURE: bool = true;

/// The tallest capture asked for: the width bounds the picture.
const MAX_HEIGHT: u32 = 8192;

pub struct X11Opener {
    backend: Arc<dyn VideoBackend>,
    conn: Arc<RustConnection>,
    root: Window,
    screen: usize,
}

impl X11Opener {
    /// `conn` is deskd's own X connection (`root` on screen `screen`).
    pub fn new(backend: Arc<dyn VideoBackend>, conn: Arc<RustConnection>, root: Window, screen: usize) -> X11Opener {
        X11Opener { backend, conn, root, screen }
    }

    /// kessel-video's id for this screen.
    pub fn source_id(&self) -> String {
        format!("x11:{}", self.screen)
    }
}

/// The constraints for a plan.
pub fn constraints(plan: &Plan) -> CaptureConstraints {
    let mut c = CaptureConstraints::new(plan.width.clamp(16, 8192), MAX_HEIGHT, plan.fps.clamp(1, crate::capture::MAX_FPS));
    c.cursor = CURSOR_IN_PICTURE;
    c
}

impl Opener for X11Opener {
    fn default_display(&mut self) -> Option<u32> {
        self.native_size(0).map(|_| 0)
    }

    fn native_size(&mut self, display: u32) -> Option<(u32, u32)> {
        if display != 0 {
            return None;
        }
        let g = self.conn.get_geometry(self.root).ok()?.reply().ok()?;
        let _ = self.conn.flush();
        Some((g.width as u32, g.height as u32))
    }

    fn open(&mut self, plan: &Plan, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        if plan.display != 0 {
            return Err(VideoError::NotFound);
        }
        self.backend.open_screen(&ScreenSelection::Listed(self.source_id()), constraints(plan), wake)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_asks_for_its_width_and_rate_with_the_cursor() {
        let c = constraints(&Plan { display: 0, width: 1920, fps: 15 });
        assert_eq!((c.max_width, c.max_height, c.fps), (Some(1920), Some(MAX_HEIGHT), Some(15)));
        assert!(c.cursor);
        assert!(!c.prefer_native, "X11 frames are CPU planes");
    }
}

//! macOS capture (D5): ScreenCaptureKit display capture through
//! kessel-video, native buffers straight into VideoToolbox.
//!
//! - **Native buffers** (`prefer_native`): ScreenCaptureKit's NV12
//!   IOSurface buffers go into the encoder as they are when the layer's
//!   size is the capture's (the widest layer); a narrower layer is scaled
//!   from them.
//! - **`minimumFrameInterval`** is `1 / plan.fps`, the highest layer rate
//!   (the engine reopens the capture when that changes).
//! - **Encode on change:** ScreenCaptureKit hands over a frame only when
//!   the screen changed; idle and blank frames are dropped in kessel-video
//!   (`SCStreamFrameInfoStatus`).
//! - **deskd's own windows** (the in-use banner) are left out of the
//!   capture.
//! - **The cursor is drawn into the picture** until the out-of-band cursor
//!   (`cursor` / `cursor-pos`, PRD §3.6) and the pane's overlay land;
//!   [`CURSOR_IN_PICTURE`] flips with them.
//!
//! Needs Screen Recording for K2 Screen; without it the open fails with
//! `no_screen_permission` and the engine tries again every few seconds.

use std::sync::Arc;

use kessel_video::{CaptureConstraints, FrameSource, ScreenSelection, VideoBackend, VideoError, Wake};

use crate::capture::{Opener, Plan};

/// Draw the pointer into the captured picture (see the module docs).
pub const CURSOR_IN_PICTURE: bool = true;

/// The tallest capture asked for: the width bounds the picture, so the
/// height box only has to be out of the way (kessel-video's limit).
const MAX_HEIGHT: u32 = 8192;

pub struct MacOpener {
    backend: Arc<dyn VideoBackend>,
}

impl MacOpener {
    pub fn new(backend: Arc<dyn VideoBackend>) -> MacOpener {
        MacOpener { backend }
    }
}

/// The constraints for a plan (the SCStreamConfiguration kessel-video
/// builds from them).
pub fn constraints(plan: &Plan) -> CaptureConstraints {
    let mut c = CaptureConstraints::new(plan.width.clamp(16, 8192), MAX_HEIGHT, plan.fps.clamp(1, crate::capture::MAX_FPS));
    c.prefer_native = true;
    c.cursor = CURSOR_IN_PICTURE;
    c.exclude_own_windows = true;
    c
}

impl Opener for MacOpener {
    fn default_display(&mut self) -> Option<u32> {
        let d = super::displays().ok()?;
        d.iter().find(|d| d.main).or_else(|| d.first()).map(|d| d.id)
    }

    fn native_size(&mut self, display: u32) -> Option<(u32, u32)> {
        super::displays().ok()?.into_iter().find(|d| d.id == display).map(|d| (d.width_px, d.height_px))
    }

    fn open(&mut self, plan: &Plan, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        // CG display ids are ScreenCaptureKit's `displayID`s.
        self.backend.open_screen(&ScreenSelection::Listed(format!("display:{}", plan.display)), constraints(plan), wake)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_asks_for_native_buffers_at_its_width_and_rate() {
        let c = constraints(&Plan { display: 1, width: 1920, fps: 60 });
        assert_eq!((c.max_width, c.max_height, c.fps), (Some(1920), Some(MAX_HEIGHT), Some(60)));
        assert!(c.prefer_native && c.exclude_own_windows);
        assert_eq!(c.cursor, CURSOR_IN_PICTURE);
    }
}

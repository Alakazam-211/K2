//! The null backend: no cameras, no screens, no codecs.

use crate::api::{
    Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake,
};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// No cameras, no screens, no codecs: [`Capabilities::codecs`] is empty and
/// every open answers `Unsupported`. For hosts without video and for "video
/// off" builds.
pub fn null() -> Arc<dyn VideoBackend> {
    Arc::new(NullBackend::default())
}

#[derive(Default)]
struct NullBackend {
    events: EventHub,
    shut: AtomicBool,
}

impl NullBackend {
    fn live(&self) -> Result<(), VideoError> {
        if self.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }
}

impl VideoBackend for NullBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities::new("null", crate::platform::os_name())
    }

    fn now_us(&self) -> u64 {
        crate::kit::now_us()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        Vec::new()
    }

    fn camera_permission(&self) -> Permission {
        Permission::Denied
    }

    fn request_camera_permission(&self, done: Done<Permission>) {
        done(Permission::Denied)
    }

    fn open_camera(
        &self,
        _id: Option<&str>,
        _c: CaptureConstraints,
        _wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        Err(unsupported("the null backend has no cameras"))
    }

    fn screen_permission(&self) -> Permission {
        Permission::Denied
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        done(Permission::Denied)
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        Ok(Vec::new())
    }

    fn pick_screen(&self, _opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        done(Err(unsupported("the null backend has no screens")))
    }

    fn open_screen(
        &self,
        _s: &ScreenSelection,
        _c: CaptureConstraints,
        _wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        Err(unsupported("the null backend has no screens"))
    }

    fn encoder(
        &self,
        config: EncoderConfig,
        _wake: Wake,
    ) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!(
            "the null backend has no {:?} encoder",
            config.codec.codec()
        )))
    }

    fn decoder(
        &self,
        config: DecoderConfig,
        _wake: Wake,
    ) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.live()?;
        Err(unsupported(format!(
            "the null backend has no {:?} decoder",
            config.codec
        )))
    }

    fn convert(
        &self,
        frame: &VideoFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<VideoFrame, VideoError> {
        self.live()?;
        crate::convert::convert(frame, width, height, format)
    }

    fn subscribe(&self) -> EventRx {
        self.events.subscribe()
    }

    fn shutdown(&self) {
        self.shut.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;

    #[test]
    fn null_has_nothing_and_says_so() {
        let b = null();
        let caps = b.capabilities();
        assert_eq!(caps.backend, "null");
        assert!(caps.codecs.is_empty() && !caps.camera && !caps.screen.supported);
        assert!(b.cameras().is_empty());
        let e = b
            .encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake())
            .err()
            .expect("no encoder");
        assert!(matches!(e, VideoError::Unsupported(_)), "{e}");
        assert!(matches!(
            b.open_camera(None, CaptureConstraints::default(), no_wake())
                .err(),
            Some(VideoError::Unsupported(_))
        ));
        let got = Arc::new(std::sync::Mutex::new(None));
        let g = got.clone();
        b.request_camera_permission(Box::new(move |p| *g.lock().expect("lock") = Some(p)));
        assert_eq!(*got.lock().expect("lock"), Some(Permission::Denied));
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }
}

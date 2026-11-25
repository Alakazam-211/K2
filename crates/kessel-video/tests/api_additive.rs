//! 0.1.9's additions are additive: code written against 0.1.8's public API,
//! from another crate, still builds and behaves the same. A source and a
//! source control that don't know the new trait methods build (the methods
//! have defaults), and a frame reads as before with `host_time` empty.
//! `scripts/check-semver.sh` compares the whole API with the release on
//! crates.io; this file keeps the two cases that matter to implementers in
//! every `cargo test`.

use kessel_video::kit::{QueuedSource, SourceControl, SourceShared};
use kessel_video::{
    no_wake, CaptureConstraints, FrameSource, HostClock, HostTime, PixelFormat, SourceEvent,
    SourceStats, VideoError, VideoFrame,
};

/// A source written against 0.1.8: only the methods 0.1.8 had.
struct OldSource;

impl FrameSource for OldSource {
    fn try_recv(&mut self) -> Option<VideoFrame> {
        None
    }
    fn try_event(&mut self) -> Option<SourceEvent> {
        None
    }
    fn set_constraints(&mut self, _c: CaptureConstraints) -> Result<(), VideoError> {
        Ok(())
    }
    fn set_paused(&mut self, _paused: bool) {}
    fn stats(&self) -> SourceStats {
        SourceStats::default()
    }
}

/// A source control written against 0.1.8: none of its optional methods.
struct OldControl;

impl SourceControl for OldControl {}

#[test]
fn sources_from_other_crates_build_without_the_new_methods() {
    let mut old: Box<dyn FrameSource> = Box::new(OldSource);
    assert!(old.try_recv().is_none());
    let shared = SourceShared::new(CaptureConstraints::new(640, 360, 30), 30, no_wake());
    let mut queued: Box<dyn FrameSource> = Box::new(QueuedSource::new(shared, OldControl));
    assert!(queued.try_recv().is_none());
    // Where the preview handle exists, the defaults answer None.
    #[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
    {
        assert!(old.apple_preview().is_none());
        assert!(queued.apple_preview().is_none());
    }
}

#[test]
fn a_frame_reads_as_it_did_and_the_host_time_is_empty() {
    let f = VideoFrame::filled(PixelFormat::I420, 4, 4, [16, 128, 128], 5).expect("frame");
    // 0.1.8 code reads the fields it knew; `VideoFrame` was already
    // #[non_exhaustive], so matching it always needed `..`.
    let VideoFrame {
        width,
        height,
        capture_us,
        ..
    } = f.clone();
    assert_eq!((width, height, capture_us), (4, 4, 5));
    assert_eq!(f.host_time, None);
    // A host may set one (a frame-in producer with its own capture times).
    let mut g = f;
    g.host_time = Some(HostTime::new(9, HostClock::MachAbsoluteTime));
    assert_eq!(
        g.host_time.map(|t| (t.nanos, t.clock.as_str())),
        Some((9, "mach_absolute_time"))
    );
}

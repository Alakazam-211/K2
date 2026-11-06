//! The traits: the backend, capture sources, encoders and decoders, and the
//! callbacks and queues between them and the caller.

use crate::error::{DecodeError, VideoError};
use crate::frame::{PixelFormat, VideoFrame};
use crate::types::*;
use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// Called from any thread after something was queued (a frame, an encoded
/// frame, an event). Must be cheap and never block; never call back into
/// kessel-video from it.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A one-shot completion (permission prompts, system pickers). Called
/// exactly once, from any thread; dropped uncalled only after
/// [`VideoBackend::shutdown`].
pub type Done<T> = Box<dyn FnOnce(T) + Send>;

/// A wake that does nothing (for callers that poll on a timer).
pub fn no_wake() -> Wake {
    Arc::new(|| {})
}

/// A running camera or screen source. Frames come at the source's pace;
/// the queue is 2 deep, oldest dropped. Drop it to stop.
pub trait FrameSource: Send {
    /// The next frame, if one is queued.
    fn try_recv(&mut self) -> Option<VideoFrame>;
    /// The next event, if one is queued.
    fn try_event(&mut self) -> Option<SourceEvent>;
    /// Change size and rate without reopening. Applied from the next frame;
    /// emits `Started` with the new values.
    fn set_constraints(&mut self, c: CaptureConstraints) -> Result<(), VideoError>;
    /// Stop delivering frames but keep the device open.
    fn set_paused(&mut self, paused: bool);
    /// Counters since open.
    fn stats(&self) -> SourceStats;
}

/// A video encoder (one layer). Outputs come in submit order, at most one per
/// input (no reordering, no B-frames); rate control may skip an input.
pub trait VideoEncoder: Send {
    /// Queue one frame. Never blocks: `Err(Busy)` when the pipeline or the
    /// output queue is full. A native buffer from another backend is
    /// `Unsupported`; pass CPU planes instead.
    fn submit(&mut self, frame: &VideoFrame, opts: EncodeOptions) -> Result<(), VideoError>;
    /// The next encoded frame or a terminal error. After `Some(Err(_))` the
    /// encoder is dead; open a new one.
    fn poll(&mut self) -> Option<Result<EncodedFrame, VideoError>>;
    /// The next encoded frame is a keyframe. Repeated calls coalesce.
    fn request_keyframe(&mut self);
    /// Bitrate and frame rate now, without a keyframe.
    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError>;
    /// A new coded size. The next output is a keyframe.
    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError>;
    /// What it runs.
    fn info(&self) -> EncoderInfo;
    /// Counters.
    fn stats(&self) -> EncoderStats;
}

/// A video decoder (one remote track).
pub trait VideoDecoder: Send {
    /// Queue one frame. Never blocks.
    fn submit(&mut self, frame: EncodedInput) -> Result<(), DecodeError>;
    /// The next decoded frame (queue of 2, oldest dropped) or an error.
    fn poll(&mut self) -> Option<Result<VideoFrame, DecodeError>>;
    /// Forget all state (and reopen a lost session). The next accepted input
    /// is a keyframe.
    fn reset(&mut self);
    /// Change the output box.
    fn set_fit(&mut self, fit: Option<(u32, u32)>);
    /// Counters.
    fn stats(&self) -> DecoderStats;
}

/// The video side of one app. One per process; shared as
/// `Arc<dyn VideoBackend>`.
pub trait VideoBackend: Send + Sync + 'static {
    /// What this machine can do now.
    fn capabilities(&self) -> Capabilities;
    /// The backend's monotonic clock, µs (the `capture_us` base).
    fn now_us(&self) -> u64;

    /// Cameras now. Doesn't open any; doesn't prompt.
    fn cameras(&self) -> Vec<CameraInfo>;
    /// The camera permission. Never prompts.
    fn camera_permission(&self) -> Permission;
    /// Shows the OS prompt if `NotDetermined`; otherwise calls `done` at once.
    fn request_camera_permission(&self, done: Done<Permission>);
    /// Opens a camera (`None`: the default). Returns when the device is open;
    /// frames follow on `wake`. Denied permission: `Err(NoPermission)`.
    fn open_camera(
        &self,
        id: Option<&str>,
        constraints: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError>;

    /// The screen-recording permission. Never prompts.
    fn screen_permission(&self) -> Permission;
    /// Asks for screen recording; `Granted` at once where there's nothing to
    /// ask.
    fn request_screen_permission(&self, done: Done<Permission>);
    /// Displays and windows (empty when [`ScreenCapability::list`] is false).
    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError>;
    /// Shows the OS picker; `done` gets the choice, or `Err(NotFound)` when
    /// cancelled.
    fn pick_screen(&self, opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>);
    /// Starts capturing a display or window.
    fn open_screen(
        &self,
        source: &ScreenSelection,
        constraints: CaptureConstraints,
        wake: Wake,
    ) -> Result<Box<dyn FrameSource>, VideoError>;

    /// A new encoder.
    fn encoder(
        &self,
        config: EncoderConfig,
        wake: Wake,
    ) -> Result<Box<dyn VideoEncoder>, VideoError>;
    /// A new decoder.
    fn decoder(
        &self,
        config: DecoderConfig,
        wake: Wake,
    ) -> Result<Box<dyn VideoDecoder>, VideoError>;

    /// Scale and convert a frame.
    ///
    /// To `PixelFormat::Native`: the Apple backend gives a CVPixelBuffer
    /// (see `apple_pixel_buffer`), copying CPU planes into an
    /// IOSurface-backed NV12 buffer from a pool; a pixel buffer that
    /// already has the size passes through. The other backends answer
    /// `Invalid`.
    fn convert(
        &self,
        frame: &VideoFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<VideoFrame, VideoError>;

    /// Machine-wide events (hot-plug, thermal, background).
    fn subscribe(&self) -> EventRx;

    /// Stops every source and codec of this backend; later calls answer
    /// `ShutDown`.
    fn shutdown(&self);
}

/// A machine-wide event queue (32 deep, oldest dropped). Each
/// [`VideoBackend::subscribe`] gets every event.
pub struct EventRx {
    q: Arc<ArrayQueue<BackendEvent>>,
    dropped: Arc<AtomicU64>,
}

impl EventRx {
    /// The next event, if any. Never blocks.
    pub fn try_recv(&self) -> Option<BackendEvent> {
        self.q.pop()
    }

    /// Events dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for EventRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRx")
            .field("len", &self.q.len())
            .finish()
    }
}

struct Subscriber {
    q: Weak<ArrayQueue<BackendEvent>>,
    dropped: Weak<AtomicU64>,
}

/// The sending side of every subscriber's queue. Public so backends in
/// other crates fan out events the same way.
#[derive(Default)]
pub struct EventHub {
    subs: Mutex<Vec<Subscriber>>,
}

impl std::fmt::Debug for EventHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHub").finish_non_exhaustive()
    }
}

/// Event queue depth.
pub const EVENT_QUEUE: usize = 32;

impl EventHub {
    /// A new subscriber's queue.
    pub fn subscribe(&self) -> EventRx {
        let rx = EventRx {
            q: Arc::new(ArrayQueue::new(EVENT_QUEUE)),
            dropped: Arc::new(AtomicU64::new(0)),
        };
        self.subs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Subscriber {
                q: Arc::downgrade(&rx.q),
                dropped: Arc::downgrade(&rx.dropped),
            });
        rx
    }

    /// Queue `e` for every live subscriber (oldest dropped when full).
    pub fn publish(&self, e: BackendEvent) {
        let mut subs = self.subs.lock().unwrap_or_else(PoisonError::into_inner);
        subs.retain(|s| {
            let (Some(q), Some(d)) = (s.q.upgrade(), s.dropped.upgrade()) else {
                return false;
            };
            if q.force_push(e.clone()).is_some() {
                d.fetch_add(1, Ordering::Relaxed);
            }
            true
        });
    }

    /// How many subscribers are alive.
    pub fn subscribers(&self) -> usize {
        let subs = self.subs.lock().unwrap_or_else(PoisonError::into_inner);
        subs.iter().filter(|s| s.q.strong_count() > 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_gets_every_event_and_the_oldest_drops() {
        let hub = EventHub::default();
        let a = hub.subscribe();
        let b = hub.subscribe();
        hub.publish(BackendEvent::LowPower(true));
        assert_eq!(a.try_recv(), Some(BackendEvent::LowPower(true)));
        assert_eq!(b.try_recv(), Some(BackendEvent::LowPower(true)));
        drop(b);
        for i in 0..EVENT_QUEUE + 3 {
            hub.publish(BackendEvent::Background(i % 2 == 0));
        }
        assert_eq!(hub.subscribers(), 1);
        assert_eq!(a.dropped(), 3);
        let mut n = 0;
        while a.try_recv().is_some() {
            n += 1;
        }
        assert_eq!(n, EVENT_QUEUE);
    }
}

//! Live frames from the emulator's `streamScreenshot` (S0: ~59 fps RGB888
//! at points resolution, first frame in ~60 ms; a still screen sends
//! almost nothing). One background thread per source runs the stream and
//! keeps only the newest frame; the encoder pulls.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::super::lane::{Frame, FrameData, FrameOpts, FrameSource, LaneError, Orientation};
use super::grpc::{EmuGrpc, Image, ImageFormat, FMT_RGB888};

#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    error: Option<LaneError>,
    ended: bool,
}

pub struct GrpcFrames {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    last_seq: u64,
}

/// Start streaming at `width_pt` x `height_pt` pixels (points
/// resolution, so the frame's scale is 1.0).
pub fn start(g: EmuGrpc, width_pt: u32, height_pt: u32, orientation: Orientation, _opts: FrameOpts) -> Result<GrpcFrames, LaneError> {
    let slot: Arc<(Mutex<Slot>, Condvar)> = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let s2 = slot.clone();
    std::thread::Builder::new()
        .name("k2-sim-frames".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let (m, cv) = &*s2;
                    let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
                    s.error = Some(LaneError::tool(format!("frame runtime: {e}")));
                    s.ended = true;
                    cv.notify_all();
                    return;
                }
            };
            let fmt = ImageFormat { format: FMT_RGB888, width: width_pt, height: height_pt, ..Default::default() };
            let s3 = s2.clone();
            let mut seq = 0u64;
            let res = rt.block_on(async move {
                let fut = g.stream::<_, Image>("streamScreenshot", &fmt, |img| {
                    seq += 1;
                    let (w, h) = img.format.as_ref().map(|f| (f.width, f.height)).unwrap_or((img.width, img.height));
                    if w == 0 || h == 0 || img.image.len() != (w as usize) * (h as usize) * 3 {
                        return true; // a frame we can't use; wait for the next
                    }
                    let f = Frame {
                        width_px: w,
                        height_px: h,
                        scale: 1.0,
                        orientation,
                        seq,
                        captured_at: Instant::now(),
                        data: FrameData::Rgb888(img.image),
                    };
                    let (m, cv) = &*s3;
                    let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
                    s.frame = Some(f);
                    cv.notify_all();
                    true
                });
                tokio::select! {
                    r = fut => r.map_err(|e| e.to_string()),
                    _ = rx => Ok(()),
                }
            });
            let (m, cv) = &*s2;
            let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = res {
                s.error = Some(LaneError::tool(e));
            }
            s.ended = true;
            cv.notify_all();
        })
        .map_err(|e| LaneError::tool(format!("frame thread: {e}")))?;
    Ok(GrpcFrames { slot, stop: Some(tx), last_seq: 0 })
}

impl FrameSource for GrpcFrames {
    fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame>, LaneError> {
        let (m, cv) = &*self.slot;
        let deadline = Instant::now() + wait;
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(f) = s.frame.as_ref().filter(|f| f.seq > self.last_seq) {
                self.last_seq = f.seq;
                return Ok(Some(f.clone()));
            }
            if let Some(e) = s.error.take() {
                return Err(e);
            }
            if s.ended {
                return Err(LaneError::not_running("the emulator's frame stream ended"));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            s = cv.wait_timeout(s, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Drop for GrpcFrames {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

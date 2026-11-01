//! Device I/O shared by the backends: the sample rings between the device
//! threads and the processing thread, the device-thread callback bodies,
//! and the null device.
//!
//! Device callbacks only move samples between the device buffer and a
//! lock-free ring (downmixing capture to mono, copying the mono mix to
//! every playback channel). They never allocate, lock or block.

use crate::events::DeviceInfo;
use crate::resample::{FromPipeline, ToPipeline};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Counters the device threads bump.
#[derive(Default)]
pub(crate) struct IoCounters {
    pub(crate) capture_overruns: AtomicU64,
    pub(crate) playout_underruns: AtomicU64,
}

/// Device thread side, capture: downmix interleaved device samples to mono
/// and push them to the ring. Samples that don't fit are dropped and
/// counted.
pub(crate) fn push_capture<T: Copy>(
    tx: &mut rtrb::Producer<f32>,
    data: &[T],
    channels: usize,
    to_f32: impl Fn(T) -> f32,
    counters: &IoCounters,
) {
    let ch = channels.max(1);
    let frames = data.len() / ch;
    let room = tx.slots();
    let n = frames.min(room);
    let scale = 1.0 / ch as f32;
    for f in 0..n {
        let mut s = 0.0f32;
        for c in 0..ch {
            s += to_f32(data[f * ch + c]);
        }
        // Cannot fail: n is at most the free room.
        let _ = tx.push(s * scale);
    }
    if n < frames {
        counters
            .capture_overruns
            .fetch_add((frames - n) as u64, Ordering::Relaxed);
    }
}

/// Device thread side, playback: pop mono samples from the ring and write
/// each to every channel. Missing samples play as silence and are counted.
pub(crate) fn pop_playback<T: Copy>(
    rx: &mut rtrb::Consumer<f32>,
    data: &mut [T],
    channels: usize,
    from_f32: impl Fn(f32) -> T,
    counters: &IoCounters,
) {
    let ch = channels.max(1);
    let mut missing = 0u64;
    for frame in data.chunks_mut(ch) {
        let v = match rx.pop() {
            Ok(v) => v,
            Err(_) => {
                missing += 1;
                0.0
            }
        };
        let s = from_f32(v);
        for slot in frame.iter_mut() {
            *slot = s;
        }
    }
    if missing > 0 {
        counters
            .playout_underruns
            .fetch_add(missing, Ordering::Relaxed);
    }
}

/// The processing thread's end of an input device.
pub(crate) struct CaptureIo {
    pub(crate) rx: rtrb::Consumer<f32>,
    pub(crate) rs: ToPipeline,
    pub(crate) buf: Vec<f32>,
}

impl CaptureIo {
    pub(crate) fn new(rx: rtrb::Consumer<f32>, rate: u32) -> Result<Self, crate::AudioError> {
        let rs = ToPipeline::new(rate)?;
        let buf = vec![0.0; rs.input_max()];
        Ok(CaptureIo { rx, rs, buf })
    }
}

/// The processing thread's end of an output device.
pub(crate) struct PlayIo {
    pub(crate) tx: rtrb::Producer<f32>,
    pub(crate) rs: FromPipeline,
    /// Keep this many device samples queued ahead of the device.
    pub(crate) target_fill: usize,
}

impl PlayIo {
    pub(crate) fn new(
        tx: rtrb::Producer<f32>,
        rate: u32,
        playout_buffer_ms: u32,
    ) -> Result<Self, crate::AudioError> {
        Ok(PlayIo {
            tx,
            rs: FromPipeline::new(rate)?,
            target_fill: (rate as usize * playout_buffer_ms as usize) / 1000,
        })
    }

    pub(crate) fn filled(&self) -> usize {
        self.tx.buffer().capacity() - self.tx.slots()
    }
}

/// Ring capacity for a device side: 250 ms, and always more than the
/// playout target plus a block.
pub(crate) fn ring_capacity(rate: u32, playout_buffer_ms: u32) -> usize {
    let ms = 250.max(playout_buffer_ms as usize * 2 + 20);
    rate as usize * ms / 1000
}

/// The null device's capture source: fills 10 ms of mono samples.
pub type CaptureSource = Box<dyn FnMut(&mut [f32]) + Send>;
/// The null device's playback sink: receives 10 ms of the mono mix.
pub type PlaybackSink = Box<dyn FnMut(&[f32]) + Send>;

/// The null device: no hardware. A thread of its own plays the device:
/// every `block_period` it takes 10 ms of capture from `input` (silence if
/// `None`) and hands 10 ms of playback to `output` (dropped if `None`).
///
/// Use it for headless tests, servers and agents. With `block_period` of
/// 10 ms it runs in real time; shorter periods run faster than real time.
/// `input` and `output` run on the null device's thread and should not
/// block for long.
pub struct NullDevice {
    /// The device's sample rate (anything rubato supports; 48 kHz avoids
    /// resampling).
    pub sample_rate: u32,
    /// Wall-clock time between 10 ms blocks.
    pub block_period: Duration,
    /// Capture source: fill the slice (10 ms at `sample_rate`, mono).
    pub input: Option<CaptureSource>,
    /// Playback sink: 10 ms of the mix at `sample_rate`, mono.
    pub output: Option<PlaybackSink>,
}

impl Default for NullDevice {
    /// 48 kHz, real time, silent input, discarded output.
    fn default() -> Self {
        NullDevice {
            sample_rate: crate::frame::SAMPLE_RATE,
            block_period: Duration::from_millis(10),
            input: None,
            output: None,
        }
    }
}

impl std::fmt::Debug for NullDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NullDevice")
            .field("sample_rate", &self.sample_rate)
            .field("block_period", &self.block_period)
            .field("input", &self.input.is_some())
            .field("output", &self.output.is_some())
            .finish()
    }
}

pub(crate) const NULL_INPUT_ID: &str = "null:input";
pub(crate) const NULL_OUTPUT_ID: &str = "null:output";

pub(crate) fn null_input_info() -> DeviceInfo {
    DeviceInfo {
        id: NULL_INPUT_ID.into(),
        name: "Null input".into(),
        is_default: true,
    }
}

pub(crate) fn null_output_info() -> DeviceInfo {
    DeviceInfo {
        id: NULL_OUTPUT_ID.into(),
        name: "Null output".into(),
        is_default: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_downmixes_and_counts_overruns() {
        let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(4);
        let c = IoCounters::default();
        // 3 stereo frames of i16.
        let data: [i16; 6] = [16384, 0, -16384, -16384, 32767, 32767];
        push_capture(&mut tx, &data, 2, |s| s as f32 / 32768.0, &c);
        assert_eq!(rx.pop().unwrap(), 0.25);
        assert_eq!(rx.pop().unwrap(), -0.5);
        assert!((rx.pop().unwrap() - 0.99997).abs() < 1e-4);
        // 6 more mono frames into a ring with room for 4.
        push_capture(&mut tx, &[0.1f32; 6], 1, |s| s, &c);
        assert_eq!(c.capture_overruns.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn playback_fans_out_and_counts_underruns() {
        let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(8);
        tx.push(0.5).unwrap();
        let c = IoCounters::default();
        let mut out = [9.0f32; 6];
        pop_playback(&mut rx, &mut out, 3, |v| v, &c);
        assert_eq!(out, [0.5, 0.5, 0.5, 0.0, 0.0, 0.0]);
        assert_eq!(c.playout_underruns.load(Ordering::Relaxed), 1);
    }
}

//! The Mobile tab's device stream as video (0.46.7, `k2/mobile-video-stream`).
//!
//! - [`VideoEncoder`]: one H.264 encoder per viewer. On macOS it is
//!   VideoToolbox ([`open_h264`]); elsewhere there is none and the stream
//!   stays JPEG (`k2_core::sim::stream::negotiate`).
//! - [`annexb`]: AVCC → Annex B, NAL splitting, the WebCodecs codec string.
//! - [`flow`]: the ack window (N frames in flight, sized to the RTT) and the
//!   bitrate controller the daemon's viewer loop uses.
//! - [`fake`]: a deterministic H.264-shaped encoder for tests on any OS.
//!
//! Low-latency settings (VideoToolbox): real-time, no frame reordering (no
//! B-frames), keyframes on join / size change / request only, and a bitrate
//! from the frame's pixel size and fps ([`bitrate_for`]).

pub mod annexb;
pub mod fake;
pub mod flow;
#[cfg(target_os = "macos")]
mod vt;

use std::sync::{OnceLock, RwLock};

/// What an encoder is opened with. Width and height are the frame's pixel
/// size; odd sizes are padded to even inside the encoder (4:2:0 chroma),
/// and the viewer crops back with the header's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

/// One encoded access unit, Annex B (start codes; SPS + PPS before every
/// keyframe, so a viewer can start on any keyframe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub key: bool,
    pub data: Vec<u8>,
}

pub trait VideoEncoder: Send {
    /// Encode one tightly packed RGB888 frame of the configured size.
    /// `Ok(None)` = the encoder dropped it (rate control); the caller frees
    /// its window slot and moves on.
    fn encode_rgb(&mut self, rgb: &[u8], force_key: bool, ts_ms: u32) -> Result<Option<Chunk>, String>;
    /// Change the target rate while running.
    fn set_rate(&mut self, fps: u32, bitrate_bps: u32);
    fn config(&self) -> EncoderConfig;
    /// "videotoolbox", "fake", … (logs).
    fn name(&self) -> &'static str;
}

type Factory = fn(&EncoderConfig) -> Result<Box<dyn VideoEncoder>, String>;

static FACTORY: RwLock<Option<Factory>> = RwLock::new(None);

/// Replace the platform encoder (tests: [`fake::open`]). `None` restores it.
#[doc(hidden)]
pub fn set_h264_factory(f: Option<Factory>) {
    *FACTORY.write().unwrap_or_else(|e| e.into_inner()) = f;
}

fn factory() -> Option<Factory> {
    *FACTORY.read().unwrap_or_else(|e| e.into_inner())
}

/// Open an H.264 encoder for this host.
pub fn open_h264(cfg: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
    if cfg.width == 0 || cfg.height == 0 || cfg.width > 8192 || cfg.height > 8192 {
        return Err(format!("no H.264 encoder for a {}x{} frame", cfg.width, cfg.height));
    }
    if let Some(f) = factory() {
        return f(cfg);
    }
    platform_open(cfg)
}

#[cfg(target_os = "macos")]
fn platform_open(cfg: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
    vt::VtEncoder::open(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

#[cfg(not(target_os = "macos"))]
fn platform_open(_cfg: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
    Err("this host has no H.264 encoder K2 can use (VideoToolbox is macOS only); the stream stays JPEG".into())
}

/// Can this host encode H.264? On macOS: a 64x64 VideoToolbox session opens
/// and encodes one frame (probed once). Elsewhere: no (no copyleft or
/// patent-unclear codec library ships with K2). A test factory says yes.
pub fn h264_available() -> bool {
    if factory().is_some() {
        return true;
    }
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        let cfg = EncoderConfig { width: 64, height: 64, fps: 30, bitrate_bps: 200_000 };
        match platform_open(&cfg) {
            Ok(mut e) => e.encode_rgb(&vec![128u8; 64 * 64 * 3], true, 0).map(|c| c.is_some_and(|c| c.key)).unwrap_or(false),
            Err(_) => false,
        }
    })
}

/// Bits per pixel per frame for the target bitrate. UI screens are mostly
/// flat; 0.12 keeps text sharp at points resolution (bench: ~5–15 KB P
/// frames while scrolling at 411x923).
pub const BITS_PER_PIXEL: f64 = 0.12;
pub const MIN_BITRATE: u32 = 300_000;
pub const MAX_BITRATE_LOCAL: u32 = 12_000_000;
pub const MAX_BITRATE_REMOTE: u32 = 4_000_000;

/// The bitrate for a viewer: tied to the frame (window) size and fps,
/// capped lower for remote viewers (LAN / Connect).
pub fn bitrate_for(width: u32, height: u32, fps: u32, remote: bool) -> u32 {
    let b = (width as f64) * (height as f64) * (fps.max(1) as f64) * BITS_PER_PIXEL;
    let cap = if remote { MAX_BITRATE_REMOTE } else { MAX_BITRATE_LOCAL };
    (b as u32).clamp(MIN_BITRATE, cap)
}

/// Pad an RGB888 frame to even width / height by repeating the last
/// column / row (4:2:0 needs even sizes). Returns the padded size.
pub fn pad_even(rgb: &[u8], width: u32, height: u32) -> Result<(std::borrow::Cow<'_, [u8]>, u32, u32), String> {
    let (w, h) = (width as usize, height as usize);
    if rgb.len() != w * h * 3 {
        return Err(format!("RGB frame of {} bytes is not {width}x{height}", rgb.len()));
    }
    let (pw, ph) = (w + (w & 1), h + (h & 1));
    if pw == w && ph == h {
        return Ok((std::borrow::Cow::Borrowed(rgb), width, height));
    }
    let mut out = Vec::with_capacity(pw * ph * 3);
    for y in 0..ph {
        let row = &rgb[y.min(h - 1) * w * 3..][..w * 3];
        out.extend_from_slice(row);
        if pw != w {
            out.extend_from_slice(&row[(w - 1) * 3..]);
        }
    }
    Ok((std::borrow::Cow::Owned(out), pw as u32, ph as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_follows_size_and_fps_and_caps_remote() {
        assert_eq!(bitrate_for(411, 923, 30, false), (411.0 * 923.0 * 30.0 * BITS_PER_PIXEL) as u32);
        assert_eq!(bitrate_for(10, 10, 1, false), MIN_BITRATE);
        assert_eq!(bitrate_for(2000, 2000, 60, true), MAX_BITRATE_REMOTE);
        assert_eq!(bitrate_for(2000, 2000, 60, false), MAX_BITRATE_LOCAL);
        assert!(bitrate_for(411, 923, 30, false) > bitrate_for(411, 923, 10, false));
    }

    #[test]
    fn pads_odd_sizes_by_repeating_the_edge() {
        let rgb: Vec<u8> = (0..3 * 3 * 3).map(|i| i as u8).collect();
        let (p, w, h) = pad_even(&rgb, 3, 3).expect("pad");
        assert_eq!((w, h), (4, 4));
        assert_eq!(p.len(), 4 * 4 * 3);
        // Row 0: the 3 pixels, then pixel 2 again.
        assert_eq!(&p[..12], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 6, 7, 8]);
        // Row 3 repeats row 2.
        assert_eq!(&p[36..48], &p[24..36]);
        let even = vec![0u8; 2 * 2 * 3];
        assert!(matches!(pad_even(&even, 2, 2).expect("even").0, std::borrow::Cow::Borrowed(_)));
        assert!(pad_even(&even, 3, 3).is_err(), "a wrong size fails loudly");
    }

    #[test]
    fn open_refuses_silly_sizes() {
        assert!(open_h264(&EncoderConfig { width: 0, height: 10, fps: 30, bitrate_bps: 1 }).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_mac_hosts_have_no_h264() {
        let _g = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_h264_factory(None);
        assert!(!h264_available());
        assert!(open_h264(&EncoderConfig { width: 64, height: 64, fps: 30, bitrate_bps: 300_000 }).is_err());
    }
}

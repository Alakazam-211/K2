//! A deterministic H.264-shaped encoder for tests on any OS: real Annex B
//! framing (SPS + PPS + IDR on a keyframe, a non-IDR slice otherwise) with
//! a small made-up payload, so the daemon's pipe, the wire header and the
//! viewer's decoder path can be tested without VideoToolbox.

use std::sync::{Arc, Mutex};

use crate::annexb::push_nal;
use crate::{Chunk, EncoderConfig, VideoEncoder};

/// The fake SPS: Constrained Baseline, level 3.1 → `avc1.42E01F`.
pub const FAKE_SPS: [u8; 4] = [0x67, 0x42, 0xE0, 0x1F];
pub const FAKE_PPS: [u8; 2] = [0x68, 0xCE];

/// What every fake encoder did (tests read it).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FakeLog {
    pub opened: Vec<(u32, u32)>,
    /// (ts_ms, key) per encoded frame.
    pub frames: Vec<(u32, bool)>,
    pub rates: Vec<(u32, u32)>,
}

/// Shared by every fake opened through [`open`] (tests serialise on
/// [`FACTORY_TEST_LOCK`]).
pub static LOG: Mutex<Option<Arc<Mutex<FakeLog>>>> = Mutex::new(None);

/// Tests that swap the process-wide factory take this lock.
pub static FACTORY_TEST_LOCK: Mutex<()> = Mutex::new(());

pub struct FakeH264 {
    cfg: EncoderConfig,
    first: bool,
    log: Option<Arc<Mutex<FakeLog>>>,
}

impl FakeH264 {
    pub fn new(cfg: EncoderConfig) -> Self {
        let log = LOG.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(l) = &log {
            l.lock().unwrap_or_else(|e| e.into_inner()).opened.push((cfg.width, cfg.height));
        }
        FakeH264 { cfg, first: true, log }
    }
}

/// The factory for [`crate::set_h264_factory`].
pub fn open(cfg: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
    Ok(Box::new(FakeH264::new(*cfg)))
}

/// Install the fake as this process's H.264 encoder and return its log.
pub fn install() -> Arc<Mutex<FakeLog>> {
    let log = Arc::new(Mutex::new(FakeLog::default()));
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(log.clone());
    crate::set_h264_factory(Some(open));
    log
}

/// Undo [`install`].
pub fn uninstall() {
    crate::set_h264_factory(None);
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

impl VideoEncoder for FakeH264 {
    fn encode_rgb(&mut self, rgb: &[u8], force_key: bool, ts_ms: u32) -> Result<Option<Chunk>, String> {
        let want = (self.cfg.width as usize) * (self.cfg.height as usize) * 3;
        if rgb.len() != want {
            return Err(format!("fake H.264: {} bytes is not {}x{}", rgb.len(), self.cfg.width, self.cfg.height));
        }
        let key = force_key || self.first;
        self.first = false;
        let mut data = Vec::new();
        // A payload that is never a start code: a marker, then ts.
        let ts = ts_ms.to_be_bytes().map(|b| b | 0x80);
        if key {
            push_nal(&mut data, &FAKE_SPS);
            push_nal(&mut data, &FAKE_PPS);
            push_nal(&mut data, &[0x65, 0x88, ts[0], ts[1], ts[2], ts[3]]);
        } else {
            push_nal(&mut data, &[0x41, 0x9A, ts[0], ts[1], ts[2], ts[3]]);
        }
        if let Some(l) = &self.log {
            l.lock().unwrap_or_else(|e| e.into_inner()).frames.push((ts_ms, key));
        }
        Ok(Some(Chunk { key, data }))
    }

    fn set_rate(&mut self, fps: u32, bitrate_bps: u32) {
        self.cfg.fps = fps;
        self.cfg.bitrate_bps = bitrate_bps;
        if let Some(l) = &self.log {
            l.lock().unwrap_or_else(|e| e.into_inner()).rates.push((fps, bitrate_bps));
        }
    }

    fn config(&self) -> EncoderConfig {
        self.cfg
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annexb::{codec_string, is_keyframe};

    #[test]
    fn first_and_forced_frames_are_keyframes_with_parameter_sets() {
        let mut e = FakeH264::new(EncoderConfig { width: 2, height: 2, fps: 30, bitrate_bps: 1 });
        let px = vec![0u8; 12];
        let a = e.encode_rgb(&px, false, 1).expect("a").expect("frame");
        let b = e.encode_rgb(&px, false, 2).expect("b").expect("frame");
        let c = e.encode_rgb(&px, true, 3).expect("c").expect("frame");
        assert!(a.key && is_keyframe(&a.data));
        assert_eq!(codec_string(&a.data).as_deref(), Some("avc1.42E01F"));
        assert!(!b.key && !is_keyframe(&b.data));
        assert!(c.key && is_keyframe(&c.data));
        assert!(e.encode_rgb(&px[..3], false, 4).is_err(), "wrong size fails loudly");
    }

    #[test]
    fn the_installed_factory_logs_every_encoder() {
        let _g = FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let log = install();
        assert!(crate::h264_available());
        let mut e = crate::open_h264(&EncoderConfig { width: 2, height: 2, fps: 30, bitrate_bps: 1 }).expect("open");
        e.encode_rgb(&[0u8; 12], false, 7).expect("encode");
        e.set_rate(15, 500_000);
        uninstall();
        let l = log.lock().expect("log").clone();
        assert_eq!(l, FakeLog { opened: vec![(2, 2)], frames: vec![(7, true)], rates: vec![(15, 500_000)] });
    }
}

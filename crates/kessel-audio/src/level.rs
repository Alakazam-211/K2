//! Level meters and a simple voice-activity detector.

use crate::events::Level;
use crate::frame::{dbov_from_mean_square, SAMPLE_RATE};

/// Accumulates mean square over fixed windows and keeps a speaking flag
/// with a release hangover.
pub(crate) struct LevelMeter {
    sum: f64,
    n: usize,
    window: usize,
    level: Level,
    threshold_dbov: f32,
    hang_windows: u32,
    hang_left: u32,
}

impl LevelMeter {
    pub(crate) fn new(window_ms: u32, threshold_dbov: f32, release_ms: u32) -> Self {
        let window = (SAMPLE_RATE as usize * window_ms.max(10) as usize) / 1000;
        LevelMeter {
            sum: 0.0,
            n: 0,
            window,
            level: Level::default(),
            threshold_dbov,
            hang_windows: (release_ms / window_ms.max(10)).max(1),
            hang_left: 0,
        }
    }

    /// Feed samples; returns true when at least one window completed.
    pub(crate) fn feed(&mut self, x: &[f32]) -> bool {
        let mut done = false;
        for &v in x {
            self.sum += (v as f64) * (v as f64);
            self.n += 1;
            if self.n >= self.window {
                let l = dbov_from_mean_square(self.sum / self.n as f64);
                self.sum = 0.0;
                self.n = 0;
                if l >= self.threshold_dbov {
                    self.hang_left = self.hang_windows;
                } else if self.hang_left > 0 {
                    self.hang_left -= 1;
                }
                self.level = Level {
                    level_dbov: l,
                    speaking: self.hang_left > 0,
                };
                done = true;
            }
        }
        done
    }

    /// Reset to silence (for example while muted).
    pub(crate) fn silence(&mut self) {
        self.sum = 0.0;
        self.n = 0;
        self.hang_left = 0;
        self.level = Level::default();
    }

    pub(crate) fn level(&self) -> Level {
        self.level
    }
}

/// Energy VAD over 20 ms frames with a tracked noise floor: voice when the
/// frame is 9 dB above the floor and above an absolute minimum, with a
/// short hangover.
pub(crate) struct Vad {
    floor_db: f32,
    hang: u32,
}

impl Vad {
    const MIN_DBOV: f32 = -60.0;
    const MARGIN_DB: f32 = 9.0;
    const HANG_FRAMES: u32 = 10;

    pub(crate) fn new() -> Self {
        Vad {
            floor_db: -70.0,
            hang: 0,
        }
    }

    pub(crate) fn update(&mut self, level_dbov: f32) -> bool {
        // The floor falls fast and rises slowly (about 1.5 dB per second).
        if level_dbov < self.floor_db {
            self.floor_db = 0.7 * self.floor_db + 0.3 * level_dbov;
        } else {
            self.floor_db += 0.03;
        }
        self.floor_db = self.floor_db.clamp(-127.0, -20.0);
        let active = level_dbov > Self::MIN_DBOV && level_dbov > self.floor_db + Self::MARGIN_DB;
        if active {
            self.hang = Self::HANG_FRAMES;
            true
        } else if self.hang > 0 {
            self.hang -= 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn meter_reads_full_scale_sine_at_minus_3_dbov() {
        let mut m = LevelMeter::new(50, -45.0, 300);
        assert!(m.feed(&tone(2400, 1.0)));
        let l = m.level();
        assert!((l.level_dbov + 3.01).abs() < 0.1, "got {}", l.level_dbov);
        assert!(l.speaking);
    }

    #[test]
    fn meter_releases_speaking_after_hangover() {
        let mut m = LevelMeter::new(50, -45.0, 300);
        m.feed(&tone(2400, 0.1));
        assert!(m.level().speaking);
        let silent = vec![0.0f32; 2400];
        let mut windows_until_quiet = 0;
        while m.level().speaking {
            m.feed(&silent);
            windows_until_quiet += 1;
            assert!(windows_until_quiet <= 10, "speaking never released");
        }
        assert_eq!(windows_until_quiet, 6);
        assert_eq!(m.level().level_dbov, -127.0);
    }

    #[test]
    fn vad_follows_speech_over_noise() {
        let mut v = Vad::new();
        for _ in 0..100 {
            assert!(!v.update(-65.0));
        }
        assert!(v.update(-30.0));
        // Hangover, then quiet again.
        let mut frames = 0;
        while v.update(-65.0) {
            frames += 1;
            assert!(frames < 20);
        }
        assert_eq!(frames, Vad::HANG_FRAMES as usize);
    }
}

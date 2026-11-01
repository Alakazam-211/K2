//! Resampling between a device's rate and the pipeline's 48 kHz, in 10 ms
//! blocks, with buffers allocated up front (rubato's FFT resampler).

use crate::error::{AudioError, ErrorCode};
use crate::frame::{BLOCK_SAMPLES, SAMPLE_RATE};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

fn rs_err(e: impl std::fmt::Display) -> AudioError {
    AudioError::new(ErrorCode::UnsupportedConfig, format!("resampler: {e}"))
}

/// Device rate to 48 kHz: always yields exactly 480 samples per call; asks
/// for a varying number of input samples.
pub(crate) struct ToPipeline {
    rs: Option<Fft<f32>>,
}

impl ToPipeline {
    pub(crate) fn new(device_rate: u32) -> Result<Self, AudioError> {
        if device_rate == SAMPLE_RATE {
            return Ok(ToPipeline { rs: None });
        }
        let rs = Fft::<f32>::new(
            device_rate as usize,
            SAMPLE_RATE as usize,
            BLOCK_SAMPLES,
            1,
            FixedSync::Output,
        )
        .map_err(rs_err)?;
        Ok(ToPipeline { rs: Some(rs) })
    }

    /// Device samples needed for the next block.
    pub(crate) fn input_needed(&self) -> usize {
        match &self.rs {
            None => BLOCK_SAMPLES,
            Some(r) => r.input_frames_next(),
        }
    }

    /// The most device samples a block can need (for buffer sizing).
    pub(crate) fn input_max(&self) -> usize {
        match &self.rs {
            None => BLOCK_SAMPLES,
            Some(r) => r.input_frames_max(),
        }
    }

    /// Convert `input` (exactly [`ToPipeline::input_needed`] samples) into
    /// one 480-sample block.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut [f32]) -> Result<(), AudioError> {
        match &mut self.rs {
            None => {
                out.copy_from_slice(input);
                Ok(())
            }
            Some(r) => {
                let n = input.len();
                let a = InterleavedSlice::new(input, 1, n).map_err(rs_err)?;
                let mut b = InterleavedSlice::new_mut(out, 1, BLOCK_SAMPLES).map_err(rs_err)?;
                r.process_into_buffer(&a, &mut b, None).map_err(rs_err)?;
                Ok(())
            }
        }
    }
}

/// 48 kHz to the device rate: takes 480 samples per call, yields a varying
/// number of device samples.
pub(crate) struct FromPipeline {
    rs: Option<Fft<f32>>,
    out: Vec<f32>,
}

impl FromPipeline {
    pub(crate) fn new(device_rate: u32) -> Result<Self, AudioError> {
        if device_rate == SAMPLE_RATE {
            return Ok(FromPipeline {
                rs: None,
                out: vec![0.0; BLOCK_SAMPLES],
            });
        }
        let rs = Fft::<f32>::new(
            SAMPLE_RATE as usize,
            device_rate as usize,
            BLOCK_SAMPLES,
            1,
            FixedSync::Input,
        )
        .map_err(rs_err)?;
        let max = rs.output_frames_max();
        Ok(FromPipeline {
            rs: Some(rs),
            out: vec![0.0; max],
        })
    }

    /// Convert one 480-sample block; returns the device samples.
    pub(crate) fn process(&mut self, block: &[f32]) -> Result<&[f32], AudioError> {
        match &mut self.rs {
            None => {
                self.out.copy_from_slice(block);
                Ok(&self.out[..])
            }
            Some(r) => {
                let a = InterleavedSlice::new(block, 1, BLOCK_SAMPLES).map_err(rs_err)?;
                let cap = self.out.len();
                let mut b = InterleavedSlice::new_mut(&mut self.out[..], 1, cap).map_err(rs_err)?;
                let (_, produced) = r.process_into_buffer(&a, &mut b, None).map_err(rs_err)?;
                Ok(&self.out[..produced])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn device_44k1_to_48k_and_back_keeps_a_tone() {
        let input = tone(44_100, 44_100, 1000.0);
        let mut to = ToPipeline::new(44_100).unwrap();
        let mut pos = 0;
        let mut at48 = Vec::new();
        let mut block = [0.0f32; BLOCK_SAMPLES];
        while pos + to.input_needed() <= input.len() {
            let n = to.input_needed();
            assert!(n <= to.input_max());
            to.process(&input[pos..pos + n], &mut block).unwrap();
            at48.extend_from_slice(&block);
            pos += n;
        }
        // ~1 s in, ~1 s out (minus less than one block of leftover input).
        assert!(at48.len() >= 47_000 && at48.len() <= 48_000, "{}", at48.len());
        let tail = &at48[4800..];
        assert!((rms(tail) - 0.3536).abs() < 0.01, "rms {}", rms(tail));

        let mut from = FromPipeline::new(44_100).unwrap();
        let mut back = Vec::new();
        for b in at48.chunks_exact(BLOCK_SAMPLES) {
            back.extend_from_slice(from.process(b).unwrap());
        }
        let ratio = back.len() as f32 / at48.len() as f32;
        assert!((ratio - 44_100.0 / 48_000.0).abs() < 0.01, "ratio {ratio}");
        assert!((rms(&back[4410..]) - 0.3536).abs() < 0.01);
    }

    #[test]
    fn at_48k_it_passes_through() {
        let input = tone(48_000, BLOCK_SAMPLES, 440.0);
        let mut to = ToPipeline::new(48_000).unwrap();
        let mut out = [0.0f32; BLOCK_SAMPLES];
        to.process(&input, &mut out).unwrap();
        assert_eq!(&out[..], &input[..]);
        let mut from = FromPipeline::new(48_000).unwrap();
        assert_eq!(from.process(&input).unwrap(), &input[..]);
    }
}

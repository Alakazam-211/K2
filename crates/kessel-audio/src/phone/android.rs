//! Android: AAudio (through the `ndk` crate) with the voice-communication
//! input preset and usage, so the platform applies its echo cancellation,
//! noise suppression and gain control, and playback goes to the call volume
//! stream.
//!
//! The app owns the rest: `MODE_IN_COMMUNICATION`, audio focus, the
//! communication device and the `RECORD_AUDIO` permission. Two streams, mono
//! f32 at 48 kHz (AAudio converts from the device's rate); a disconnected
//! stream (a route change) is reported to the state machine, which reopens
//! both. Needs Android 9 (API 28) for the preset and usage.
//!
//! In raw mode the input uses the unprocessed preset (when the app says the
//! device supports it) or the voice-recognition preset, with no audio
//! session, so no `AudioEffect` (a `NoiseSuppressor`, say) can attach.
//! Playback is the same in both modes.
#![allow(unsafe_code)]

use super::{android_input, AndroidPreset, Ends, Opened, PhoneIo, PhoneMode};
use crate::device::{pop_playback, push_capture};
use crate::error::{AudioError, ErrorCode};
use crate::frame::SAMPLE_RATE;
use ndk::audio::{
    AudioCallbackResult, AudioContentType, AudioDirection, AudioError as AaError, AudioFormat,
    AudioInputPreset, AudioPerformanceMode, AudioSharingMode, AudioStream, AudioStreamBuilder,
    AudioUsage, SessionId,
};

fn aa_err(what: &str, e: AaError) -> AudioError {
    let code = match e {
        AaError::Disconnected => ErrorCode::DeviceLost,
        AaError::Unavailable | AaError::NoService | AaError::NoFreeHandles => ErrorCode::DeviceBusy,
        AaError::InvalidFormat
        | AaError::InvalidRate
        | AaError::IllegalArgument
        | AaError::Unimplemented => ErrorCode::UnsupportedConfig,
        _ => ErrorCode::Backend,
    };
    AudioError::new(code, format!("AAudio {what}: {e}"))
}

/// A started stream that stops before it closes.
struct Running(AudioStream);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.request_stop();
    }
}

/// Mono f32 or nothing: the callbacks are written for that.
fn check_shape(s: &AudioStream, what: &str) -> Result<(), AudioError> {
    if s.channel_count() != 1 || s.format() != AudioFormat::PCM_Float {
        return Err(AudioError::new(
            ErrorCode::UnsupportedConfig,
            format!(
                "AAudio {what}: opened {} channels of {:?}, not mono f32",
                s.channel_count(),
                s.format()
            ),
        ));
    }
    Ok(())
}

fn builder(dir: AudioDirection) -> Result<AudioStreamBuilder, AaError> {
    Ok(AudioStreamBuilder::new()?
        .direction(dir)
        .sharing_mode(AudioSharingMode::Shared)
        // The low-latency path can skip the platform's effects on some
        // devices; voice doesn't need it.
        .performance_mode(AudioPerformanceMode::None)
        .format(AudioFormat::PCM_Float)
        .channel_count(1)
        .sample_rate(SAMPLE_RATE as i32))
}

/// AAudio's two streams, set up per mode by [`android_input`].
pub(crate) struct AAudioIo;

impl PhoneIo for AAudioIo {
    fn current_rate(&self) -> Option<u32> {
        // AAudio picks and converts per stream; a route change always
        // reopens.
        None
    }

    fn open(&mut self, ends: Ends, mode: PhoneMode) -> Result<Opened, AudioError> {
        let setup = android_input(mode);
        let (preset, name) = match setup.preset {
            AndroidPreset::VoiceCommunication => {
                (AudioInputPreset::VoiceCommunication, "Voice communication")
            }
            AndroidPreset::Unprocessed => (AudioInputPreset::Unprocessed, "Raw (unprocessed)"),
            AndroidPreset::VoiceRecognition => (
                AudioInputPreset::VoiceRecognition,
                "Raw (voice recognition)",
            ),
        };
        let Ends {
            mut capture,
            mut playback,
            shared,
            faults,
        } = ends;

        let s = shared.clone();
        let f = faults.clone();
        let out = builder(AudioDirection::Output)
            .map_err(|e| aa_err("output", e))?
            .usage(AudioUsage::VoiceCommunication)
            .content_type(AudioContentType::Speech)
            .data_callback(Box::new(move |_, data, frames| {
                // SAFETY: AAudio hands the callback `frames` frames of the
                // stream's format, checked below to be mono f32, valid for
                // the call.
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(data.cast::<f32>(), frames.max(0) as usize)
                };
                pop_playback(&mut playback, buf, 1, |v| v, &s.io);
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(move |_, _| f.raise()))
            .open_stream()
            .map_err(|e| aa_err("output", e))?;
        check_shape(&out, "output")?;
        let playback_rate = out.sample_rate() as u32;

        let mut warnings = Vec::new();
        let s = shared.clone();
        let input = builder(AudioDirection::Input)
            .map(|b| {
                let b = b.input_preset(preset);
                // No session: nothing can attach an AudioEffect (a
                // NoiseSuppressor) to this stream. AAudio's default is the
                // same; saying so keeps it that way.
                let b = if setup.no_session {
                    b.session_id(Some(SessionId::None))
                } else {
                    b
                };
                b.data_callback(Box::new(move |_, data, frames| {
                    // SAFETY: as above, for capture.
                    let buf = unsafe {
                        std::slice::from_raw_parts(
                            data.cast::<f32>().cast_const(),
                            frames.max(0) as usize,
                        )
                    };
                    push_capture(&mut capture, buf, 1, |v| v, &s.io);
                    AudioCallbackResult::Continue
                }))
                .error_callback(Box::new(move |_, _| faults.raise()))
            })
            .and_then(|b| b.open_stream())
            .map_err(|e| aa_err("input", e))
            .and_then(|i| check_shape(&i, "input").map(|()| i));
        // Without the input (no RECORD_AUDIO, or a device that refuses the
        // preset) the call still plays.
        let input = match input {
            Ok(i) => Some(i),
            Err(e) => {
                warnings.push(e);
                None
            }
        };

        out.request_start().map_err(|e| aa_err("output start", e))?;
        let out = Running(out);
        let input = match input {
            Some(i) => match i.request_start() {
                Ok(()) => Some(Running(i)),
                Err(e) => {
                    warnings.push(aa_err("input start", e));
                    None
                }
            },
            None => None,
        };
        let capture_rate = input.as_ref().map(|i| i.0.sample_rate() as u32);
        Ok(Opened {
            stream: Box::new((input, out)),
            capture_rate,
            playback_rate,
            name,
            warnings,
        })
    }
}

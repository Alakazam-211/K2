//! iOS: the VoiceProcessingIO audio unit (Apple's echo cancellation, noise
//! suppression and gain control, the FaceTime path), or, in raw mode or
//! with no OS effects, the plain RemoteIO unit (no OS processing), through
//! coreaudio-rs.
//!
//! The app owns the audio session (category `.playAndRecord`, mode
//! `.voiceChat`, activated by CallKit). This module only reads it: the
//! current sample rate and whether the person refused the microphone. One
//! unit runs capture (element 1) and playback (element 0) at the session's
//! rate, mono f32; the engine resamples to 48 kHz. Both units have the same
//! elements, formats and callbacks; only the subtype and the gain control
//! property differ.
#![allow(unsafe_code)]

use super::{ios_unit, Ends, IosUnit, Opened, OsEffects, PhoneIo, PhoneMode};
use crate::device::{pop_playback, push_capture};
use crate::error::{AudioError, ErrorCode};
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{data, Args};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};
use objc2_audio_toolbox::{
    kAUVoiceIOProperty_VoiceProcessingEnableAGC, kAudioOutputUnitProperty_EnableIO,
};
use objc2_avf_audio::{AVAudioSession, AVAudioSessionRecordPermission};

fn ca_err(what: &str, e: coreaudio::Error) -> AudioError {
    AudioError::new(ErrorCode::Backend, format!("audio unit: {what}: {e}"))
}

/// The session's current hardware rate, or `None` if it reports none.
fn session_rate() -> Option<u32> {
    // SAFETY: the shared session is a process-wide singleton; `sampleRate`
    // is a read-only property that may be read from any thread.
    let rate = unsafe { AVAudioSession::sharedInstance().sampleRate() };
    (rate.is_finite() && rate >= 1.0).then(|| rate.round() as u32)
}

/// The person refused the microphone (a read; this never prompts).
fn mic_refused() -> bool {
    // SAFETY: as above; `recordPermission` is a read-only property. It is
    // deprecated in favour of AVAudioApplication (iOS 17), which older
    // systems lack.
    #[allow(deprecated)]
    let p = unsafe { AVAudioSession::sharedInstance().recordPermission() };
    #[allow(deprecated)]
    let denied = AVAudioSessionRecordPermission::Denied;
    p == denied
}

/// The I/O unit: VoiceProcessingIO or RemoteIO, picked per open by
/// [`ios_unit`].
pub(crate) struct RemoteUnit {
    fx: OsEffects,
}

impl RemoteUnit {
    pub(crate) fn new(fx: OsEffects) -> Self {
        RemoteUnit { fx }
    }
}

impl PhoneIo for RemoteUnit {
    fn current_rate(&self) -> Option<u32> {
        session_rate()
    }

    fn open(&mut self, ends: Ends, mode: PhoneMode) -> Result<Opened, AudioError> {
        let kind = ios_unit(self.fx, mode);
        let Ends {
            mut capture,
            mut playback,
            shared,
            faults: _,
        } = ends;
        let rate = session_rate().ok_or_else(|| {
            AudioError::new(
                ErrorCode::NoDevice,
                "the audio session reports no sample rate",
            )
        })?;
        let mut warnings = Vec::new();
        let mic = !mic_refused();
        if !mic {
            warnings.push(AudioError::new(
                ErrorCode::NoPermission,
                "microphone access is turned off for this app in Settings",
            ));
        }

        let io_type = match kind {
            IosUnit::VoiceProcessing => IOType::VoiceProcessingIO,
            IosUnit::RemoteIo => IOType::RemoteIO,
        };
        let mut unit = AudioUnit::new_uninitialized(io_type).map_err(|e| ca_err("create", e))?;
        let on = 1u32;
        let input = u32::from(mic);
        unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Input,
            Element::Input,
            Some(&input),
        )
        .map_err(|e| ca_err("enable input", e))?;
        unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Output,
            Element::Output,
            Some(&on),
        )
        .map_err(|e| ca_err("enable output", e))?;
        // Only the voice processing unit has gain control to turn off.
        if kind == IosUnit::VoiceProcessing && !self.fx.gain_control {
            let off = 0u32;
            if let Err(e) = unit.set_property(
                kAUVoiceIOProperty_VoiceProcessingEnableAGC,
                Scope::Global,
                Element::Input,
                Some(&off),
            ) {
                warnings.push(ca_err("turn its gain control off", e));
            }
        }

        // What the app side of the unit sees: mono f32 at the session rate.
        let format = StreamFormat {
            sample_rate: rate as f64,
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: 1,
        };
        unit.set_stream_format(format, Scope::Input, Element::Output)
            .map_err(|e| ca_err("playback format", e))?;
        let s = shared.clone();
        unit.set_render_callback(move |args: Args<data::Interleaved<f32>>| {
            let ch = args.data.channels;
            pop_playback(&mut playback, args.data.buffer, ch, |v| v, &s.io);
            Ok(())
        })
        .map_err(|e| ca_err("playback callback", e))?;
        if mic {
            unit.set_stream_format(format, Scope::Output, Element::Input)
                .map_err(|e| ca_err("capture format", e))?;
            let s = shared.clone();
            unit.set_input_callback(move |args: Args<data::Interleaved<f32>>| {
                let ch = args.data.channels;
                push_capture(&mut capture, args.data.buffer, ch, |v| v, &s.io);
                Ok(())
            })
            .map_err(|e| ca_err("capture callback", e))?;
        }
        unit.initialize().map_err(|e| ca_err("initialise", e))?;
        unit.start().map_err(|e| ca_err("start", e))?;
        Ok(Opened {
            // Dropping the unit stops, uninitialises and disposes of it.
            stream: Box::new(unit),
            capture_rate: mic.then_some(rate),
            playback_rate: rate,
            name: match kind {
                IosUnit::VoiceProcessing => "Voice processing",
                IosUnit::RemoteIo => "Raw",
            },
            warnings,
        })
    }
}

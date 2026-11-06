//! H.264 encoding through Media Foundation: a hardware encoder MFT from the
//! GPU's driver when there is one, else Microsoft's software H.264 encoder
//! that ships with Windows. No codec code of our own.
//!
//! Settings: low latency, no B-frames, constant bitrate at the caller's
//! target (changed at run time), Constrained Baseline or Constrained High
//! (plain Baseline or High where the MFT doesn't know the constrained
//! names), keyframes on request plus the configured interval, NV12 input
//! from CPU planes, and Annex B out: one access unit per frame, 4-byte start
//! codes, SPS and PPS before every IDR.

use super::map_error;
use super::mft::{
    self, available, pack, sample_bytes, sample_from, set_codec_value, var_bool, var_u32, Found,
    Mft, NoOutput, Role,
};
use crate::error::{unsupported, VideoError};
use crate::frame::{FrameBuffer, PixelFormat, VideoFrame};
use crate::h264;
use crate::kit::{CoreFrame, EncoderCore};
use crate::types::*;
use windows::Win32::Media::MediaFoundation::*;

/// The MFT profile values to try for a profile, best first.
fn profile_values(p: H264Profile) -> [eAVEncH264VProfile; 2] {
    match p {
        H264Profile::ConstrainedHigh => [
            eAVEncH264VProfile_UCConstrainedHigh,
            eAVEncH264VProfile_High,
        ],
        _ => [eAVEncH264VProfile_ConstrainedBase, eAVEncH264VProfile_Base],
    }
}

/// One access unit in the crate's form: 4-byte start codes, no access unit
/// delimiters, and for a keyframe the given SPS and PPS first (in-band ones
/// are dropped from their place, so each appears once).
pub(super) fn access_unit(nals: &[&[u8]], parameter_sets: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let mut out = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum::<usize>() + 64);
    if let Some((sps, pps)) = parameter_sets {
        h264::push_annexb(&mut out, sps);
        h264::push_annexb(&mut out, pps);
    }
    for nal in nals {
        if !matches!(
            h264::nal_type(nal),
            Some(h264::NAL_SPS | h264::NAL_PPS | h264::NAL_AUD)
        ) {
            h264::push_annexb(&mut out, nal);
        }
    }
    out
}

thread_local! {
    static COM_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// COM and Media Foundation on the calling thread (the encoder's own
/// thread calls this before its first frame).
fn com_on_this_thread() -> Result<(), VideoError> {
    if COM_READY.with(|c| c.get()) {
        return Ok(());
    }
    super::camera::init()?;
    COM_READY.with(|c| c.set(true));
    Ok(())
}

pub(crate) struct MfEncoder {
    pub(super) mft: Mft,
    api: Option<ICodecAPI>,
    cfg: EncoderConfig,
    /// The last SPS and PPS the encoder gave (in band or in its output
    /// type), written before every IDR.
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    last_time: i64,
    low_latency: bool,
    /// The profile value the MFT took.
    profile: eAVEncH264VProfile,
    /// A keyframe was asked for and the MFT gave a delta: ask again.
    owed_key: bool,
}

// SAFETY: the MFT and its ICodecAPI are free-threaded (see `Mft`); one
// thread at a time drives the encoder (&mut self).
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    /// The first H.264 encoder MFT that `cfg.implementation` allows and
    /// that takes `cfg`.
    pub(crate) fn open(cfg: &EncoderConfig) -> Result<MfEncoder, VideoError> {
        let CodecConfig::H264 { .. } = cfg.codec else {
            return Err(unsupported(format!(
                "Media Foundation here encodes H.264, not {:?}",
                cfg.codec.codec()
            )));
        };
        let found = mft::find(Role::Encoder, cfg.implementation)?;
        let mut last = None;
        for f in &found {
            match MfEncoder::open_one(f, cfg) {
                Ok(e) => return Ok(e),
                Err(e) => last = Some((f.name.clone(), e)),
            }
        }
        Err(match last {
            None => mft::none_found(Role::Encoder, cfg.implementation),
            Some((name, e)) if cfg.implementation == ImplPreference::HardwareOnly => {
                unsupported(format!(
                    "no hardware H.264 encoder took {}x{} ({name}: {e})",
                    cfg.width, cfg.height
                ))
            }
            Some((_, e)) => e,
        })
    }

    pub(crate) fn open_one(found: &Found, cfg: &EncoderConfig) -> Result<MfEncoder, VideoError> {
        let CodecConfig::H264 { profile, level } = cfg.codec else {
            return Err(unsupported("H.264 only"));
        };
        let mft = Mft::open(found)?;
        let mut enc = MfEncoder {
            api: mft.codec_api(),
            mft,
            cfg: *cfg,
            sps: None,
            pps: None,
            last_time: -1,
            low_latency: false,
            profile: eAVEncH264VProfile_Base,
            owed_key: false,
        };
        // Rate control and latency before the media types: Microsoft's
        // encoder reads them when its output type is set.
        enc.set_modes();
        let t = enc.mft.transform.clone();
        let t = &t;
        let (out_id, in_id) = (enc.mft.out_id(), enc.mft.in_id());
        let mut taken = None;
        'profiles: for p in profile_values(profile) {
            for with_level in [true, false] {
                let out = mft::video_type(&MFVideoFormat_H264)?;
                // SAFETY: writes to a new media type, then offering it to
                // the MFT.
                let ok = unsafe {
                    out.SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height))
                        .and_then(|_| out.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps, 1)))
                        .and_then(|_| out.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_bps))
                        .and_then(|_| {
                            out.SetUINT32(
                                &MF_MT_INTERLACE_MODE,
                                MFVideoInterlace_Progressive.0 as u32,
                            )
                        })
                        .and_then(|_| out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))
                        .and_then(|_| out.SetUINT32(&MF_MT_MPEG2_PROFILE, p.0 as u32))
                        .and_then(|_| {
                            if with_level {
                                out.SetUINT32(&MF_MT_MPEG2_LEVEL, level.0 as u32)
                            } else {
                                Ok(())
                            }
                        })
                        .and_then(|_| t.SetOutputType(out_id, &out, 0))
                };
                if ok.is_ok() {
                    taken = Some(p);
                    break 'profiles;
                }
            }
        }
        enc.profile = taken.ok_or_else(|| {
            unsupported(format!(
                "{} took no {profile:?} output at {}x{} {} fps",
                found.name, cfg.width, cfg.height, cfg.fps
            ))
        })?;
        let input = available(t, in_id, true, &MFVideoFormat_NV12)
            .map(Ok)
            .unwrap_or_else(|| mft::video_type(&MFVideoFormat_NV12))?;
        // SAFETY: writes to a media type we hold, then offering it.
        unsafe {
            input
                .SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height))
                .and_then(|_| input.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps, 1)))
                .and_then(|_| {
                    input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                })
                .and_then(|_| input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))
                .and_then(|_| t.SetInputType(in_id, &input, 0))
        }
        .map_err(|e| map_error(&e, "SetInputType (NV12)"))?;
        // Some MFTs take codec settings only once their types are set.
        enc.set_modes();
        enc.mft.out_size_hint = (cfg.width * cfg.height * 3 / 2).max(1 << 16);
        enc.mft.begin()?;
        enc.read_sequence_header();
        Ok(enc)
    }

    /// Low latency, rate control, bitrate, no B-frames and the GOP, through
    /// ICodecAPI. Each is best effort: an MFT that lacks one keeps its
    /// default (the output is still checked frame by frame).
    fn set_modes(&mut self) {
        let Some(api) = &self.api else { return };
        let cfg = &self.cfg;
        self.low_latency |= set_codec_value(api, &CODECAPI_AVLowLatencyMode, &var_bool(true))
            || set_codec_value(api, &CODECAPI_AVLowLatencyMode, &var_u32(1));
        set_codec_value(
            api,
            &CODECAPI_AVEncCommonRateControlMode,
            &var_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
        );
        set_codec_value(
            api,
            &CODECAPI_AVEncCommonMeanBitRate,
            &var_u32(cfg.bitrate_bps),
        );
        set_codec_value(
            api,
            &CODECAPI_AVEncCommonMaxBitRate,
            &var_u32(cfg.max_bitrate_bps),
        );
        set_codec_value(api, &CODECAPI_AVEncMPVDefaultBPictureCount, &var_u32(0));
        // Keyframes come on request; the GOP is only a safety net then.
        let gop = cfg
            .keyframe_interval
            .unwrap_or(cfg.fps.saturating_mul(3600));
        if !set_codec_value(api, &CODECAPI_AVEncMPVGOPSize, &var_u32(gop)) {
            set_codec_value(api, &CODECAPI_AVEncMPVGOPSize, &var_u32(gop.min(0xFFFF)));
        }
        if self.cfg.content == ContentHint::Camera {
            // 0 = fastest.
            set_codec_value(api, &CODECAPI_AVEncCommonQualityVsSpeed, &var_u32(0));
        }
    }

    /// SPS and PPS from the output type's sequence header, if it has one.
    fn read_sequence_header(&mut self) {
        // SAFETY: reads of the MFT's current output type.
        let blob = unsafe {
            let Ok(t) = self.mft.transform.GetOutputCurrentType(self.mft.out_id()) else {
                return;
            };
            let Ok(n) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
                return;
            };
            let mut b = vec![0u8; n as usize];
            if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut b, None)
                .is_err()
            {
                return;
            }
            b
        };
        self.remember_parameter_sets(&blob);
    }

    fn remember_parameter_sets(&mut self, annexb: &[u8]) {
        for nal in h264::annexb_nals(annexb) {
            match h264::nal_type(nal) {
                Some(h264::NAL_SPS) => self.sps = Some(nal.to_vec()),
                Some(h264::NAL_PPS) => self.pps = Some(nal.to_vec()),
                _ => {}
            }
        }
    }

    /// The MFT's output as one access unit in the crate's form: 4-byte start
    /// codes, no access unit delimiters, SPS and PPS first on an IDR.
    fn access_unit(&mut self, raw: &[u8]) -> Result<CoreFrame, VideoError> {
        let nals = h264::annexb_nals(raw);
        if nals.is_empty() {
            return Err(VideoError::os(
                0,
                format!(
                    "{} gave {} bytes with no Annex B NAL units",
                    self.mft.name,
                    raw.len()
                ),
            ));
        }
        self.remember_parameter_sets(raw);
        let key = h264::is_keyframe(&nals);
        if !key {
            return Ok(CoreFrame::new(access_unit(&nals, None), false));
        }
        if self.sps.is_none() || self.pps.is_none() {
            self.read_sequence_header();
        }
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Err(VideoError::os(
                0,
                format!("{} gave an IDR with no SPS and PPS", self.mft.name),
            ));
        };
        Ok(CoreFrame::new(access_unit(&nals, Some((sps, pps))), true))
    }

    fn reopen(&mut self, cfg: EncoderConfig) -> Result<(), VideoError> {
        let mut want = cfg;
        // Reopen the same kind it runs now.
        want.implementation = if self.mft.hardware {
            ImplPreference::HardwareOnly
        } else {
            ImplPreference::SoftwareOnly
        };
        let mut fresh = MfEncoder::open(&want)?;
        fresh.cfg.implementation = cfg.implementation;
        *self = fresh;
        Ok(())
    }
}

impl EncoderCore for MfEncoder {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::Nv12
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        keyframe: bool,
    ) -> Result<Option<CoreFrame>, VideoError> {
        com_on_this_thread()?;
        let FrameBuffer::Planes(p) = &frame.buffer else {
            return Err(unsupported("a native buffer from another backend"));
        };
        if p.format != PixelFormat::Nv12
            || (frame.width, frame.height) != (self.cfg.width, self.cfg.height)
        {
            return Err(VideoError::os(
                0,
                "the encoder got a frame it didn't ask for",
            ));
        }
        let tight = p.to_tight(frame.width, frame.height);
        let keyframe = keyframe || self.owed_key;
        if keyframe {
            let forced = !self.owed_key
                && self.api.as_ref().is_some_and(|a| {
                    set_codec_value(a, &CODECAPI_AVEncVideoForceKeyFrame, &var_u32(1))
                });
            if !forced {
                // No way to ask this MFT, or it ignored the last request: a
                // new session starts with an IDR.
                let cfg = self.cfg;
                self.reopen(cfg)?;
            }
        }
        // 100 ns units, strictly increasing.
        let time =
            ((frame.capture_us.min(i64::MAX as u64 / 10) * 10) as i64).max(self.last_time + 1);
        self.last_time = time;
        let duration = 10_000_000 / self.cfg.fps.max(1) as i64;
        let sample = sample_from(&tight.data, time, duration)?;
        let out_id = self.mft.out_id();
        let outs = self.mft.process(&sample, NoOutput::Drain, &mut |t| {
            // A new output type (new parameter sets): take what it offers.
            // SAFETY: queries and a type change on the live MFT.
            unsafe {
                let ty = t
                    .GetOutputAvailableType(out_id, 0)
                    .map_err(|e| map_error(&e, "GetOutputAvailableType"))?;
                t.SetOutputType(out_id, &ty, 0)
                    .map_err(|e| map_error(&e, "SetOutputType"))
            }
        })?;
        if outs.is_empty() {
            // Rate control skipped it.
            return Ok(None);
        }
        let mut raw = Vec::new();
        for s in &outs {
            raw.extend_from_slice(&sample_bytes(s)?);
        }
        let cf = self.access_unit(&raw)?;
        // Ignored once: the next frame comes from a new session.
        self.owed_key = keyframe && !cf.keyframe;
        Ok(Some(cf))
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        let ratio = self.cfg.max_bitrate_bps as f64 / self.cfg.bitrate_bps.max(1) as f64;
        let max = ((rates.bitrate_bps as f64 * ratio.max(1.0)) as u64).min(u32::MAX as u64) as u32;
        let took = self.api.as_ref().is_some_and(|a| {
            set_codec_value(
                a,
                &CODECAPI_AVEncCommonMeanBitRate,
                &var_u32(rates.bitrate_bps),
            )
        });
        if let Some(a) = &self.api {
            set_codec_value(a, &CODECAPI_AVEncCommonMaxBitRate, &var_u32(max));
        }
        let mut cfg = self.cfg;
        cfg.bitrate_bps = rates.bitrate_bps;
        cfg.max_bitrate_bps = max.max(rates.bitrate_bps);
        // The frame rate only sets the sample durations: Media Foundation
        // has no frame-rate change inside a stream.
        cfg.fps = rates.fps;
        if took {
            self.cfg = cfg;
            Ok(())
        } else {
            // An MFT that takes no bitrate while streaming gets a new
            // session at the new rate (whose first frame is an IDR) rather
            // than keep the old one.
            self.reopen(cfg)
        }
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        let mut cfg = self.cfg;
        cfg.width = width;
        cfg.height = height;
        self.reopen(cfg)
    }

    fn implementation(&self) -> Implementation {
        if self.mft.hardware {
            Implementation::Hardware
        } else {
            Implementation::Software
        }
    }

    fn name(&self) -> String {
        let profile = match self.profile {
            p if p == eAVEncH264VProfile_ConstrainedBase => "Constrained Baseline",
            p if p == eAVEncH264VProfile_UCConstrainedHigh => "Constrained High",
            p if p == eAVEncH264VProfile_High => "High",
            _ => "Baseline",
        };
        format!(
            "Media Foundation {} ({}, {profile}{})",
            self.mft.name,
            if self.mft.hardware {
                "hardware"
            } else {
                "software"
            },
            if self.low_latency {
                ", low latency"
            } else {
                ""
            }
        )
    }
}

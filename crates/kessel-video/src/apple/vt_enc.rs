//! H.264 encoding through VideoToolbox (hardware on Apple silicon and on
//! Intel Macs with a T2 or Quick Sync; Apple's software encoder otherwise).
//!
//! Settings: real time, no frame reordering (no B-frames), low-latency rate
//! control when the OS has it, Constrained Baseline or Constrained High,
//! keyframes on request (plus the configured interval), the caller's
//! bitrate with a one-second ceiling, and Annex B out with SPS and PPS
//! before every IDR.

use super::ffi::*;
use super::pixel::{fill_nv12, tag_color, CvFrame};
use crate::error::{unsupported, VideoError};
use crate::frame::{FrameBuffer, PixelFormat, VideoFrame};
use crate::h264;
use crate::kit::{CoreFrame, EncoderCore};
use crate::types::*;
use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

/// What the output callback fills (one entry per frame).
#[derive(Default)]
struct Outbox {
    out: Vec<Result<Option<CoreFrame>, VideoError>>,
}

extern "C" fn on_output(refcon: *mut c_void, _frame: *mut c_void, status: OSStatus, flags: u32, sb: CMSampleBufferRef) {
    // SAFETY: refcon is the encoder's boxed Mutex<Outbox>, alive until the
    // session is invalidated (Drop invalidates first).
    let outbox = unsafe { &*(refcon as *const Mutex<Outbox>) };
    let r = if status != 0 {
        Err(VideoError::os(status as i64, "VideoToolbox encode"))
    } else if sb.is_null() || flags & INFO_FRAME_DROPPED != 0 {
        Ok(None)
    } else {
        // SAFETY: a live sample buffer for the duration of the callback.
        unsafe { sample_to_annexb(sb) }.map(Some)
    };
    outbox.lock().unwrap_or_else(|e| e.into_inner()).out.push(r);
}

/// AVCC sample → one Annex B access unit, SPS + PPS in front of a keyframe.
unsafe fn sample_to_annexb(sb: CMSampleBufferRef) -> Result<CoreFrame, VideoError> {
    let mut key = true;
    let atts = CMSampleBufferGetSampleAttachmentsArray(sb, 0);
    if !atts.is_null() && CFArrayGetCount(atts) > 0 {
        let d = CFArrayGetValueAtIndex(atts, 0);
        if let Some(not_sync) = cf_const(c"kCMSampleAttachmentKey_NotSync") {
            let v = CFDictionaryGetValue(d, not_sync);
            if !v.is_null() && CFBooleanGetValue(v) != 0 {
                key = false;
            }
        }
    }
    let desc = CMSampleBufferGetFormatDescription(sb);
    if desc.is_null() {
        return Err(VideoError::os(0, "an encoded sample with no format description"));
    }
    let mut count = 0usize;
    let mut nal_len = 0i32;
    let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, 0, ptr::null_mut(), ptr::null_mut(), &mut count, &mut nal_len);
    if st != 0 {
        return Err(VideoError::os(st as i64, "H.264 parameter sets unreadable"));
    }
    let mut data = Vec::new();
    if key {
        for i in 0..count {
            let mut p: *const u8 = ptr::null();
            let mut n = 0usize;
            let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, i, &mut p, &mut n, ptr::null_mut(), ptr::null_mut());
            if st != 0 || p.is_null() {
                return Err(VideoError::os(st as i64, format!("H.264 parameter set {i} unreadable")));
            }
            h264::push_annexb(&mut data, std::slice::from_raw_parts(p, n));
        }
    }
    let bb = CMSampleBufferGetDataBuffer(sb);
    if bb.is_null() {
        return Err(VideoError::os(0, "an encoded sample with no data"));
    }
    let len = CMBlockBufferGetDataLength(bb);
    let mut avcc = vec![0u8; len];
    let st = CMBlockBufferCopyDataBytes(bb, 0, len, avcc.as_mut_ptr() as *mut c_void);
    if st != 0 {
        return Err(VideoError::os(st as i64, "encoded sample copy"));
    }
    for nal in h264::avcc_nals(&avcc, nal_len as usize)? {
        // Parameter sets are written once, above, from the format.
        if key && matches!(h264::nal_type(nal), Some(h264::NAL_SPS | h264::NAL_PPS)) {
            continue;
        }
        h264::push_annexb(&mut data, nal);
    }
    Ok(CoreFrame::new(data, key))
}

pub(crate) struct VtEncoder {
    session: VTSessionRef,
    outbox: Box<Mutex<Outbox>>,
    cfg: EncoderConfig,
    hardware: bool,
    low_latency: bool,
    last_pts: i64,
}

// SAFETY: the session is driven from one thread at a time (&mut self);
// VideoToolbox sessions may be used from any thread.
unsafe impl Send for VtEncoder {}

fn profile_keys(p: H264Profile) -> [&'static std::ffi::CStr; 2] {
    match p {
        H264Profile::ConstrainedHigh => [c"kVTProfileLevel_H264_ConstrainedHigh_AutoLevel", c"kVTProfileLevel_H264_High_AutoLevel"],
        _ => [c"kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel", c"kVTProfileLevel_H264_Baseline_AutoLevel"],
    }
}

impl VtEncoder {
    pub(crate) fn open(cfg: &EncoderConfig) -> Result<VtEncoder, VideoError> {
        let CodecConfig::H264 { profile, .. } = cfg.codec else {
            return Err(unsupported(format!("VideoToolbox here encodes H.264, not {:?}", cfg.codec.codec())));
        };
        let outbox: Box<Mutex<Outbox>> = Box::default();
        let attrs = pixel_buffer_attributes(PIX_420V, Some((cfg.width, cfg.height)));
        let spec_for = |low_latency: bool| {
            let spec = dict();
            match cfg.implementation {
                ImplPreference::HardwareOnly => {
                    set_named(&spec, c"kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder", yes());
                }
                ImplPreference::SoftwareOnly => {
                    set_named(&spec, c"kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder", no());
                }
                _ => {
                    set_named(&spec, c"kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder", yes());
                }
            }
            let ll = low_latency && set_named(&spec, c"kVTVideoEncoderSpecification_EnableLowLatencyRateControl", yes());
            (spec, ll)
        };
        let refcon = &*outbox as *const Mutex<Outbox> as *mut c_void;
        let create = |spec: &Cf, out: &mut VTSessionRef| -> OSStatus {
            // SAFETY: valid dictionaries, a live refcon, an out pointer.
            unsafe {
                VTCompressionSessionCreate(
                    ptr::null(),
                    cfg.width as i32,
                    cfg.height as i32,
                    K_CMVIDEO_CODEC_H264,
                    spec.0,
                    attrs.0,
                    ptr::null(),
                    Some(on_output),
                    refcon,
                    out,
                )
            }
        };
        let mut session: VTSessionRef = ptr::null_mut();
        // Low-latency rate control isn't there for every size and mode;
        // without it the session still runs in real time.
        let (spec, mut low_latency) = spec_for(cfg.implementation != ImplPreference::SoftwareOnly);
        let mut st = create(&spec, &mut session);
        if (st != 0 || session.is_null()) && low_latency {
            low_latency = false;
            session = ptr::null_mut();
            st = create(&spec_for(false).0, &mut session);
        }
        if st != 0 || session.is_null() {
            return Err(match cfg.implementation {
                ImplPreference::HardwareOnly => unsupported(format!("no hardware H.264 encoder for {}x{} (OSStatus {st})", cfg.width, cfg.height)),
                _ => VideoError::os(st as i64, format!("VTCompressionSessionCreate {}x{}", cfg.width, cfg.height)),
            });
        }
        let mut enc = VtEncoder {
            session,
            outbox,
            cfg: *cfg,
            hardware: false,
            low_latency,
            last_pts: -1,
        };
        enc.hardware = enc.copy_bool(c"kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder");
        if cfg.implementation == ImplPreference::HardwareOnly && !enc.hardware {
            return Err(unsupported("VideoToolbox gave a software encoder when hardware was required"));
        }
        enc.prop(c"kVTCompressionPropertyKey_RealTime", yes());
        enc.prop(c"kVTCompressionPropertyKey_AllowFrameReordering", no());
        let [want, fallback] = profile_keys(profile);
        let set_profile = |enc: &mut VtEncoder, key| cf_const(key).is_some_and(|p| enc.prop(c"kVTCompressionPropertyKey_ProfileLevel", p));
        if !set_profile(&mut enc, want) && !set_profile(&mut enc, fallback) {
            return Err(unsupported(format!("VideoToolbox refused the {profile:?} profile")));
        }
        match cfg.keyframe_interval {
            Some(n) => {
                enc.prop(c"kVTCompressionPropertyKey_MaxKeyFrameInterval", num_i32(n.min(i32::MAX as u32) as i32).0);
            }
            None => {
                // Keyframes come on request; this is only a safety net.
                enc.prop(c"kVTCompressionPropertyKey_MaxKeyFrameInterval", num_i32(i32::MAX).0);
                enc.prop(c"kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration", num_f64(3600.0).0);
            }
        }
        if cfg.content == ContentHint::Camera {
            enc.prop(c"kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality", yes());
        }
        enc.apply_rates(Rates::new(cfg.bitrate_bps, cfg.fps))?;
        // SAFETY: a live session.
        let st = unsafe { VTCompressionSessionPrepareToEncodeFrames(enc.session) };
        if st != 0 {
            return Err(VideoError::os(st as i64, "VTCompressionSessionPrepareToEncodeFrames"));
        }
        Ok(enc)
    }

    /// Set one property; false when the OS lacks the key or refuses it.
    fn prop(&mut self, key: &std::ffi::CStr, value: CFTypeRef) -> bool {
        let Some(k) = cf_const(key) else { return false };
        // SAFETY: a live session, a framework key, a CF value.
        unsafe { VTSessionSetProperty(self.session, k, value) == 0 }
    }

    fn copy_bool(&self, key: &std::ffi::CStr) -> bool {
        let Some(k) = cf_const(key) else { return false };
        let mut v: CFTypeRef = ptr::null();
        // SAFETY: a live session, a framework key, an out pointer; we own
        // the copied value.
        unsafe {
            if VTSessionCopyProperty(self.session, k, ptr::null(), &mut v) != 0 || v.is_null() {
                return false;
            }
            let owned = Cf(v);
            CFBooleanGetValue(owned.0) != 0
        }
    }

    fn apply_rates(&mut self, r: Rates) -> Result<(), VideoError> {
        // The ceiling keeps its ratio to the target as BWE moves it.
        let ratio = self.cfg.max_bitrate_bps as f64 / self.cfg.bitrate_bps.max(1) as f64;
        let max_bps = (r.bitrate_bps as f64 * ratio.max(1.0)) as i64;
        if !self.prop(c"kVTCompressionPropertyKey_AverageBitRate", num_i32(r.bitrate_bps.min(i32::MAX as u32) as i32).0) {
            return Err(VideoError::os(0, "VideoToolbox refused the bitrate"));
        }
        // [bytes, seconds]: no one-second window over the ceiling.
        let limits = array(&[num_i64(max_bps / 8).0, num_f64(1.0).0]);
        self.prop(c"kVTCompressionPropertyKey_DataRateLimits", limits.0);
        self.prop(c"kVTCompressionPropertyKey_ExpectedFrameRate", num_i32(r.fps as i32).0);
        let max_bitrate = (max_bps as u64).min(u32::MAX as u64) as u32;
        self.cfg.max_bitrate_bps = max_bitrate.max(r.bitrate_bps);
        self.cfg.bitrate_bps = r.bitrate_bps;
        self.cfg.fps = r.fps;
        Ok(())
    }

    fn pixel_buffer(&self) -> Result<Cf, VideoError> {
        let mut pb: CVPixelBufferRef = ptr::null_mut();
        // SAFETY: a live session; an out pointer.
        let st = unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            if pool.is_null() {
                let attrs = pixel_buffer_attributes(PIX_420V, None);
                CVPixelBufferCreate(ptr::null(), self.cfg.width as usize, self.cfg.height as usize, PIX_420V, attrs.0, &mut pb)
            } else {
                CVPixelBufferPoolCreatePixelBuffer(ptr::null(), pool, &mut pb)
            }
        };
        if st != 0 || pb.is_null() {
            return Err(VideoError::os(st as i64, "no pixel buffer for the encoder"));
        }
        Ok(Cf(pb as CFTypeRef))
    }
}

impl EncoderCore for VtEncoder {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::Nv12
    }

    fn accepts_native(&self, frame: &VideoFrame) -> bool {
        match &frame.buffer {
            FrameBuffer::Native(n) => n.0.as_any().downcast_ref::<CvFrame>().is_some(),
            _ => false,
        }
    }

    fn encode(&mut self, frame: &VideoFrame, keyframe: bool) -> Result<Option<CoreFrame>, VideoError> {
        let owned;
        let pb: CVPixelBufferRef = match &frame.buffer {
            FrameBuffer::Native(n) => match n.0.as_any().downcast_ref::<CvFrame>() {
                Some(cv) if cv.size() == (self.cfg.width, self.cfg.height) => cv.raw(),
                _ => return Err(unsupported("a native buffer from another backend")),
            },
            FrameBuffer::Planes(p) => {
                owned = self.pixel_buffer()?;
                let pb = owned.0 as CVPixelBufferRef;
                fill_nv12(pb, p, frame.width, frame.height)?;
                tag_color(pb, frame.color);
                pb
            }
        };
        let props = if keyframe {
            let d = dict();
            set_named(&d, c"kVTEncodeFrameOptionKey_ForceKeyFrame", yes());
            Some(d)
        } else {
            None
        };
        let pts = (frame.capture_us.min(i64::MAX as u64) as i64).max(self.last_pts + 1);
        self.last_pts = pts;
        let mut info = 0u32;
        // SAFETY: a live session and pixel buffer; the options dictionary
        // (or null) outlives the call.
        let st = unsafe {
            let st = VTCompressionSessionEncodeFrame(
                self.session,
                pb,
                cm_time(pts, 1_000_000),
                CM_TIME_INVALID,
                props.as_ref().map_or(ptr::null(), |d| d.0),
                ptr::null_mut(),
                &mut info,
            );
            if st == 0 {
                VTCompressionSessionCompleteFrames(self.session, CM_TIME_INVALID)
            } else {
                st
            }
        };
        if st == VT_INVALID_SESSION {
            return Err(VideoError::os(st as i64, "the VideoToolbox session was lost"));
        }
        if st != 0 {
            return Err(VideoError::os(st as i64, "VTCompressionSessionEncodeFrame"));
        }
        let mut got: Vec<_> = self.outbox.lock().unwrap_or_else(|e| e.into_inner()).out.drain(..).collect();
        // CompleteFrames flushed: this is the frame just given (or nothing if
        // rate control dropped it). More would mean frame delay we turned
        // off; dropping one would break the reference chain, so fail loudly.
        if got.len() > 1 {
            return Err(VideoError::os(0, format!("VideoToolbox gave {} frames for one input", got.len())));
        }
        got.pop().transpose().map(Option::flatten)
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        self.apply_rates(rates)
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        let mut cfg = self.cfg;
        cfg.width = width;
        cfg.height = height;
        *self = VtEncoder::open(&cfg)?;
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        if self.hardware {
            Implementation::Hardware
        } else {
            Implementation::Software
        }
    }

    fn name(&self) -> String {
        format!(
            "VideoToolbox H.264 ({}{})",
            if self.hardware { "hardware" } else { "software" },
            if self.low_latency { ", low-latency rate control" } else { "" }
        )
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        // SAFETY: invalidate before the outbox (the callback's refcon) is
        // freed; then release our reference.
        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
        }
    }
}

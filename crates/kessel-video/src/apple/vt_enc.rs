//! H.264 encoding through VideoToolbox: hardware on iPhones and iPads, on
//! Apple silicon and on Intel Macs with a T2 or Quick Sync; Apple's software
//! encoder on other Macs.
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

autolink!("VideoToolbox", "CoreMedia", "CoreVideo", "CoreFoundation");

/// What the output callback fills (one entry per frame).
#[derive(Default)]
struct Outbox {
    out: Vec<Result<Option<CoreFrame>, VideoError>>,
}

extern "C" fn on_output(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OSStatus,
    flags: u32,
    sb: CMSampleBufferRef,
) {
    // SAFETY: refcon is the encoder's boxed Mutex<Outbox>, alive until the
    // session is invalidated (Drop invalidates first).
    let outbox = unsafe { &*(refcon as *const Mutex<Outbox>) };
    let r = if status == VT_INVALID_SESSION || status == VT_ENCODER_NOT_AVAILABLE_NOW {
        Err(lost_session(status))
    } else if status != 0 {
        Err(VideoError::os(status as i64, "VideoToolbox encode"))
    } else if sb.is_null() || flags & INFO_FRAME_DROPPED != 0 {
        Ok(None)
    } else {
        // SAFETY: a live sample buffer for the duration of the callback.
        unsafe { sample_to_annexb(sb) }.map(Some)
    };
    outbox.lock().unwrap_or_else(|e| e.into_inner()).out.push(r);
}

/// The error for a session VideoToolbox took away. iOS takes the encoder
/// from an app in the background: that is `ShutDown`, as the API says for
/// phones. Otherwise an OS error; either way, open a new encoder.
pub(crate) fn lost_session(st: OSStatus) -> VideoError {
    let phone_gone = cfg!(target_os = "ios") && st == VT_ENCODER_NOT_AVAILABLE_NOW;
    if phone_gone || (st == VT_INVALID_SESSION && super::in_background()) {
        VideoError::ShutDown
    } else {
        VideoError::os(st as i64, "the VideoToolbox session was lost")
    }
}

/// AVCC sample → one Annex B access unit, SPS + PPS in front of a keyframe.
unsafe fn sample_to_annexb(sb: CMSampleBufferRef) -> Result<CoreFrame, VideoError> {
    let mut key = true;
    let atts = CMSampleBufferGetSampleAttachmentsArray(sb, 0);
    if !atts.is_null() && CFArrayGetCount(atts) > 0 {
        let d = CFArrayGetValueAtIndex(atts, 0);
        if let Some(not_sync) = cf_const(c"kCMSampleAttachmentKey_NotSync") {
            if bool_value(dict_value(d, not_sync)) == Some(true) {
                key = false;
            }
        }
    }
    let desc = CMSampleBufferGetFormatDescription(sb);
    if desc.is_null() {
        return Err(VideoError::os(
            0,
            "an encoded sample with no format description",
        ));
    }
    let mut count = 0usize;
    let mut nal_len = 0i32;
    let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc,
        0,
        ptr::null_mut(),
        ptr::null_mut(),
        &mut count,
        &mut nal_len,
    );
    if st != 0 {
        return Err(VideoError::os(st as i64, "H.264 parameter sets unreadable"));
    }
    let mut data = Vec::new();
    if key {
        for i in 0..count {
            let mut p: *const u8 = ptr::null();
            let mut n = 0usize;
            let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                desc,
                i,
                &mut p,
                &mut n,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if st != 0 || p.is_null() {
                return Err(VideoError::os(
                    st as i64,
                    format!("H.264 parameter set {i} unreadable"),
                ));
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
    implementation: Implementation,
    low_latency: bool,
    /// The session's encoder (`kVTCompressionPropertyKey_EncoderID`), when
    /// it names one (`info().name` carries it, for logs).
    encoder_id: Option<String>,
    /// The profile VideoToolbox took (the asked one or its fallback).
    profile_name: &'static str,
    last_pts: i64,
}

// SAFETY: the session is driven from one thread at a time (&mut self);
// VideoToolbox sessions may be used from any thread.
unsafe impl Send for VtEncoder {}

/// A profile key's name for logs ("Constrained High", …).
fn profile_name(key: &std::ffi::CStr) -> &'static str {
    match key.to_bytes() {
        b"kVTProfileLevel_H264_ConstrainedHigh_AutoLevel" => "Constrained High",
        b"kVTProfileLevel_H264_High_AutoLevel" => "High",
        b"kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel" => "Constrained Baseline",
        b"kVTProfileLevel_H264_Baseline_AutoLevel" => "Baseline",
        _ => "?",
    }
}

fn profile_keys(p: H264Profile) -> [&'static std::ffi::CStr; 2] {
    match p {
        H264Profile::ConstrainedHigh => [
            c"kVTProfileLevel_H264_ConstrainedHigh_AutoLevel",
            c"kVTProfileLevel_H264_High_AutoLevel",
        ],
        _ => [
            c"kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel",
            c"kVTProfileLevel_H264_Baseline_AutoLevel",
        ],
    }
}

impl VtEncoder {
    pub(crate) fn open(cfg: &EncoderConfig) -> Result<VtEncoder, VideoError> {
        let CodecConfig::H264 { profile, .. } = cfg.codec else {
            return Err(unsupported(format!(
                "VideoToolbox here encodes H.264, not {:?}",
                cfg.codec.codec()
            )));
        };
        if cfg!(target_os = "ios") && cfg.implementation == ImplPreference::SoftwareOnly {
            return Err(unsupported(
                "iOS has no software-only H.264 encoder to ask for",
            ));
        }
        let attrs = pixel_buffer_attributes(PIX_420V, Some((cfg.width, cfg.height)));
        // The sessions to try, in order: how to ask for hardware, and
        // whether to ask for low-latency rate control (not there for every
        // size and mode; without it the session still runs in real time).
        // Preferring hardware first requires it, and each session is then
        // checked against VideoToolbox's encoder list: a required-hardware
        // ask with low-latency rate control can still be handed the
        // low-latency encoder (`h264.rtvc`, macOS 26 on an M4 Mac mini),
        // which doesn't answer "using hardware" and which the list marks as
        // software.
        let plan: &[(Ask, bool)] = match cfg.implementation {
            ImplPreference::HardwareOnly => &[(Ask::Require, true), (Ask::Require, false)],
            ImplPreference::SoftwareOnly => &[(Ask::Disable, false)],
            _ => &[
                (Ask::Require, true),
                (Ask::Require, false),
                (Ask::Enable, true),
                (Ask::Enable, false),
            ],
        };
        let mut last_status: OSStatus = 0;
        let mut unconfirmed = false;
        let mut chosen = None;
        for &(ask, want_low_latency) in plan {
            let spec = dict();
            let (key, value) = match ask {
                Ask::Require => (
                    c"kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder",
                    yes(),
                ),
                Ask::Enable => (
                    c"kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder",
                    yes(),
                ),
                Ask::Disable => (
                    c"kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder",
                    no(),
                ),
            };
            // A binding ask, where the key is there (macOS, iOS 17.4+):
            // required hardware fails the create rather than fall back to
            // software, and disabled hardware is never used.
            let asked = match (set_named(&spec, key, value), ask) {
                (true, Ask::Require) => Some(true),
                (true, Ask::Disable) => Some(false),
                _ => None,
            };
            let low_latency = want_low_latency
                && set_named(
                    &spec,
                    c"kVTVideoEncoderSpecification_EnableLowLatencyRateControl",
                    yes(),
                );
            if want_low_latency && !low_latency {
                // The same session as the next attempt's.
                continue;
            }
            let outbox: Box<Mutex<Outbox>> = Box::default();
            let refcon = &*outbox as *const Mutex<Outbox> as *mut c_void;
            let mut session: VTSessionRef = ptr::null_mut();
            // SAFETY: valid dictionaries, a live refcon (the outbox moves
            // into the encoder with the session), an out pointer.
            let st = unsafe {
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
                    &mut session,
                )
            };
            if st != 0 || session.is_null() {
                last_status = st;
                continue;
            }
            let mut enc = VtEncoder {
                session,
                outbox,
                cfg: *cfg,
                implementation: Implementation::Unknown,
                low_latency,
                encoder_id: None,
                profile_name: "",
                last_pts: -1,
            };
            let reported =
                enc.copy_bool(c"kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder");
            // The list is read whenever the session doesn't say, even under
            // a binding ask: a Require ask doesn't guarantee the encoder.
            let listed = if reported.is_none() {
                enc.listed_hardware()
            } else {
                None
            };
            enc.implementation = session_implementation(reported, encoder_ask(asked), listed);
            if ask == Ask::Require && enc.implementation != Implementation::Hardware {
                // Asked for hardware and not confirmed: drop it and go on.
                unconfirmed = true;
                continue;
            }
            chosen = Some(enc);
            break;
        }
        let Some(mut enc) = chosen else {
            return Err(match cfg.implementation {
                ImplPreference::HardwareOnly if unconfirmed => unsupported(
                    "VideoToolbox made an H.264 encoder that can't be confirmed as hardware",
                ),
                ImplPreference::HardwareOnly => unsupported(format!(
                    "no hardware H.264 encoder for {}x{} (OSStatus {last_status})",
                    cfg.width, cfg.height
                )),
                _ => VideoError::os(
                    last_status as i64,
                    format!("VTCompressionSessionCreate {}x{}", cfg.width, cfg.height),
                ),
            });
        };
        enc.prop(c"kVTCompressionPropertyKey_RealTime", yes());
        enc.prop(c"kVTCompressionPropertyKey_AllowFrameReordering", no());
        let [want, fallback] = profile_keys(profile);
        let set_profile = |enc: &mut VtEncoder, key| {
            cf_const(key).is_some_and(|p| enc.prop(c"kVTCompressionPropertyKey_ProfileLevel", p))
        };
        if set_profile(&mut enc, want) {
            enc.profile_name = profile_name(want);
        } else if set_profile(&mut enc, fallback) {
            enc.profile_name = profile_name(fallback);
        } else {
            return Err(unsupported(format!(
                "VideoToolbox refused the {profile:?} profile"
            )));
        }
        enc.encoder_id = enc.copy_encoder_id();
        match cfg.keyframe_interval {
            Some(n) => {
                enc.prop(
                    c"kVTCompressionPropertyKey_MaxKeyFrameInterval",
                    num_i32(n.min(i32::MAX as u32) as i32).0,
                );
            }
            None => {
                // Keyframes come on request; this is only a safety net.
                enc.prop(
                    c"kVTCompressionPropertyKey_MaxKeyFrameInterval",
                    num_i32(i32::MAX).0,
                );
                enc.prop(
                    c"kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration",
                    num_f64(3600.0).0,
                );
            }
        }
        if cfg.content == ContentHint::Camera {
            enc.prop(
                c"kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality",
                yes(),
            );
        }
        enc.apply_rates(Rates::new(cfg.bitrate_bps, cfg.fps))?;
        // SAFETY: a live session.
        let st = unsafe { VTCompressionSessionPrepareToEncodeFrames(enc.session) };
        if st != 0 {
            return Err(VideoError::os(
                st as i64,
                "VTCompressionSessionPrepareToEncodeFrames",
            ));
        }
        Ok(enc)
    }

    /// Set one property; false when the OS lacks the key or refuses it.
    fn prop(&mut self, key: &std::ffi::CStr, value: CFTypeRef) -> bool {
        let Some(k) = cf_const(key) else { return false };
        // SAFETY: a live session, a framework key, a CF value.
        unsafe { VTSessionSetProperty(self.session, k, value) == 0 }
    }

    /// A boolean property; `None` when the OS lacks the key or won't say.
    fn copy_bool(&self, key: &std::ffi::CStr) -> Option<bool> {
        let k = cf_const(key)?;
        let mut v: CFTypeRef = ptr::null();
        // SAFETY: a live session, a framework key, an out pointer; we own
        // the copied value.
        unsafe {
            if VTSessionCopyProperty(self.session, k, ptr::null(), &mut v) != 0 || v.is_null() {
                return None;
            }
            let owned = Cf(v);
            bool_value(owned.0)
        }
    }

    /// The session's encoder ID (`com.apple.videotoolbox.videoencoder.…`),
    /// when the session names one.
    fn copy_encoder_id(&self) -> Option<String> {
        let key = cf_const(c"kVTCompressionPropertyKey_EncoderID")?;
        let mut v: CFTypeRef = ptr::null();
        // SAFETY: a live session and a framework key, an out pointer; we
        // own the copy (released by `Cf`) and read it while it lives.
        unsafe {
            if VTSessionCopyProperty(self.session, key, ptr::null(), &mut v) != 0 || v.is_null() {
                return None;
            }
            let owned = Cf(v);
            string_value(owned.0)
        }
    }

    /// Is this session's encoder hardware, as VideoToolbox's encoder list
    /// says? `None` when the session doesn't name its encoder or the list
    /// doesn't have it. A listed encoder without the hardware flag is
    /// software (the list's own rule).
    fn listed_hardware(&self) -> Option<bool> {
        let id_key = cf_const(c"kVTCompressionPropertyKey_EncoderID")?;
        let list_id = cf_const(c"kVTVideoEncoderList_EncoderID")?;
        let list_hw = cf_const(c"kVTVideoEncoderList_IsHardwareAccelerated");
        let mut id: CFTypeRef = ptr::null();
        let mut list: CFTypeRef = ptr::null();
        // SAFETY: a live session and framework keys, out pointers; we own
        // the copied ID and list (released by `Cf`), and read the list's
        // dictionaries only while it lives.
        unsafe {
            if VTSessionCopyProperty(self.session, id_key, ptr::null(), &mut id) != 0
                || id.is_null()
            {
                return None;
            }
            let id = Cf(id);
            if VTCopyVideoEncoderList(ptr::null(), &mut list) != 0 || list.is_null() {
                return None;
            }
            let list = Cf(list);
            if !cf_is(list.0, CFArrayGetTypeID) {
                return None;
            }
            for i in 0..CFArrayGetCount(list.0) {
                let entry = CFArrayGetValueAtIndex(list.0, i);
                let entry_id = dict_value(entry, list_id);
                if entry_id.is_null() || CFEqual(entry_id, id.0) == 0 {
                    continue;
                }
                let hw = list_hw.map_or(ptr::null(), |k| dict_value(entry, k));
                return Some(bool_value(hw) == Some(true));
            }
            None
        }
    }

    fn apply_rates(&mut self, r: Rates) -> Result<(), VideoError> {
        // The ceiling keeps its ratio to the target as BWE moves it.
        let ratio = self.cfg.max_bitrate_bps as f64 / self.cfg.bitrate_bps.max(1) as f64;
        let max_bps = (r.bitrate_bps as f64 * ratio.max(1.0)) as i64;
        if !self.prop(
            c"kVTCompressionPropertyKey_AverageBitRate",
            num_i32(r.bitrate_bps.min(i32::MAX as u32) as i32).0,
        ) {
            return Err(VideoError::os(0, "VideoToolbox refused the bitrate"));
        }
        // [bytes, seconds]: no one-second window over the ceiling.
        let limits = array(&[num_i64(max_bps / 8).0, num_f64(1.0).0]);
        self.prop(c"kVTCompressionPropertyKey_DataRateLimits", limits.0);
        self.prop(
            c"kVTCompressionPropertyKey_ExpectedFrameRate",
            num_i32(r.fps as i32).0,
        );
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
                CVPixelBufferCreate(
                    ptr::null(),
                    self.cfg.width as usize,
                    self.cfg.height as usize,
                    PIX_420V,
                    attrs.0,
                    &mut pb,
                )
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

    fn encode(
        &mut self,
        frame: &VideoFrame,
        keyframe: bool,
    ) -> Result<Option<CoreFrame>, VideoError> {
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
        if st == VT_INVALID_SESSION || st == VT_ENCODER_NOT_AVAILABLE_NOW {
            return Err(lost_session(st));
        }
        if st != 0 {
            return Err(VideoError::os(st as i64, "VTCompressionSessionEncodeFrame"));
        }
        let mut got =
            std::mem::take(&mut self.outbox.lock().unwrap_or_else(|e| e.into_inner()).out);
        // CompleteFrames flushed: this is the frame just given (or nothing if
        // rate control dropped it). More would mean frame delay we turned
        // off; dropping one would break the reference chain, so fail loudly.
        if got.len() > 1 {
            return Err(VideoError::os(
                0,
                format!("VideoToolbox gave {} frames for one input", got.len()),
            ));
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
        self.implementation
    }

    fn name(&self) -> String {
        format!(
            "VideoToolbox H.264 ({}, encoder {}, {}{})",
            match self.implementation {
                Implementation::Hardware => "hardware",
                Implementation::Software => "software",
                _ => "hardware or software: the OS doesn't say",
            },
            self.encoder_id.as_deref().unwrap_or("not named"),
            self.profile_name,
            if self.low_latency {
                ", low-latency rate control"
            } else {
                ""
            }
        )
    }
}

/// How a session asks for hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Require,
    Enable,
    Disable,
}

/// What an encoder's ask proves, failing closed: hardware disabled proves
/// software ("never used"), but hardware required proves nothing. A
/// required-hardware session has been seen running the low-latency
/// software encoder `h264.rtvc` (macOS 26, M4 Mac mini), so an encoder is
/// `Hardware` only when the session or the encoder list says so; with
/// neither, it is `Unknown` (not known to be hardware), never a guess.
pub(crate) fn encoder_ask(asked: Option<bool>) -> Option<bool> {
    asked.filter(|hardware| !hardware)
}

/// What a VideoToolbox session runs, from what the OS tells, in order:
///
/// - `reported`: the session's own "using hardware" property, when it
///   answers (many encoders and decoders don't);
/// - `listed`: what VideoToolbox's encoder list says about the session's
///   encoder (its `EncoderID`), when it lists it. It comes before the ask:
///   a session created with hardware required has been seen running the
///   low-latency encoder `h264.rtvc`, which the list marks as software;
/// - `asked`: a binding ask the session was created under: `Some(true)`,
///   hardware required; `Some(false)`, hardware disabled ("never used",
///   Apple's headers say). Trusted only when neither of the above says.
///   Encoders pass it through [`encoder_ask`], so a required-hardware ask
///   alone never makes an encoder `Hardware`.
///
/// Nothing said: `Unknown`, never a guess.
pub(crate) fn session_implementation(
    reported: Option<bool>,
    asked: Option<bool>,
    listed: Option<bool>,
) -> Implementation {
    match reported.or(listed).or(asked) {
        Some(true) => Implementation::Hardware,
        Some(false) => Implementation::Software,
        None => Implementation::Unknown,
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

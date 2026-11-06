//! H.264 decoding through VideoToolbox, on macOS and iOS.
//!
//! Input is one Annex B access unit per frame. SPS and PPS in band set up
//! (or replace) the session's format; the other NAL units go to
//! VideoToolbox as one AVCC sample. Decoding is synchronous: one frame in,
//! at most one out, as [`DecoderCore`] wants. Outputs are NV12
//! CVPixelBuffers, handed on as native frames; the decoder thread converts
//! them to the caller's format.

use super::ffi::*;
use super::pixel::native_frame;
use crate::error::{unsupported, DecodeError, VideoError};
use crate::frame::VideoFrame;
use crate::h264;
use crate::kit::DecoderCore;
use crate::types::*;
use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

autolink!("VideoToolbox", "CoreMedia", "CoreVideo", "CoreFoundation");

/// What the output callback fills: one entry per decoded frame, the pixel
/// buffer retained.
#[derive(Default)]
struct Outbox {
    out: Vec<Result<Option<Cf>, OSStatus>>,
}

extern "C" fn on_output(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OSStatus,
    flags: u32,
    image: CVPixelBufferRef,
    _pts: CMTime,
    _duration: CMTime,
) {
    // SAFETY: refcon is the decoder's boxed Mutex<Outbox>, alive until the
    // session is invalidated (every path that frees it invalidates first).
    let outbox = unsafe { &*(refcon as *const Mutex<Outbox>) };
    let r = if status != 0 {
        Err(status)
    } else if image.is_null() || flags & DECODE_INFO_FRAME_DROPPED != 0 {
        Ok(None)
    } else {
        // SAFETY: a live pixel buffer for the duration of the callback; we
        // take our own retain.
        Ok(Some(unsafe { Cf::retain(image as CFTypeRef) }))
    };
    outbox.lock().unwrap_or_else(|e| e.into_inner()).out.push(r);
}

/// The decoder's error for a VideoToolbox status.
pub(crate) fn decode_error(st: OSStatus, what: &str) -> DecodeError {
    match st {
        VT_INVALID_SESSION | VT_DECODER_MALFUNCTION | VT_DECODER_NOT_AVAILABLE_NOW => {
            DecodeError::SessionLost
        }
        VT_BAD_DATA => DecodeError::Corrupt(format!("{what}: VideoToolbox rejected the data")),
        _ => DecodeError::Video(VideoError::os(st as i64, what.to_string())),
    }
}

/// The parameter sets and NAL units of one access unit.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AccessUnit<'a> {
    pub sps: Option<&'a [u8]>,
    pub pps: Option<&'a [u8]>,
    /// Slices and SEI, in order (no SPS, PPS or AUD).
    pub nals: Vec<&'a [u8]>,
}

/// Split an Annex B access unit. The last SPS and PPS win.
pub(crate) fn split_access_unit(data: &[u8]) -> AccessUnit<'_> {
    let mut au = AccessUnit::default();
    for nal in h264::annexb_nals(data) {
        match h264::nal_type(nal) {
            Some(h264::NAL_SPS) => au.sps = Some(nal),
            Some(h264::NAL_PPS) => au.pps = Some(nal),
            Some(h264::NAL_AUD) | None => {}
            Some(_) => au.nals.push(nal),
        }
    }
    au
}

pub(crate) struct VtDecoder {
    session: VTSessionRef,
    format: Cf,
    params: Option<(Vec<u8>, Vec<u8>)>,
    outbox: Box<Mutex<Outbox>>,
    pref: ImplPreference,
    hardware: Option<bool>,
}

// SAFETY: the session is driven from one thread at a time (&mut self);
// VideoToolbox sessions may be used from any thread.
unsafe impl Send for VtDecoder {}

impl VtDecoder {
    pub(crate) fn open(config: &DecoderConfig) -> Result<VtDecoder, VideoError> {
        if config.codec != VideoCodec::H264 {
            return Err(unsupported(format!(
                "VideoToolbox here decodes H.264, not {:?}",
                config.codec
            )));
        }
        #[cfg(target_os = "ios")]
        if config.implementation == ImplPreference::SoftwareOnly {
            return Err(unsupported(
                "iOS has no software-only H.264 decoder to ask for",
            ));
        }
        Ok(VtDecoder {
            session: ptr::null_mut(),
            format: Cf::null(),
            params: None,
            outbox: Box::default(),
            pref: config.implementation,
            hardware: None,
        })
    }

    fn close_session(&mut self) {
        if !self.session.is_null() {
            // SAFETY: invalidate before anything the callback uses goes;
            // then release our reference.
            unsafe {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session as CFTypeRef);
            }
            self.session = ptr::null_mut();
        }
        self.outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .out
            .clear();
    }

    /// New SPS and PPS: a new format, and a new session unless the old one
    /// takes it.
    fn set_params(&mut self, sps: &[u8], pps: &[u8]) -> Result<(), DecodeError> {
        if self
            .params
            .as_ref()
            .is_some_and(|(s, p)| s == sps && p == pps)
        {
            return Ok(());
        }
        let ptrs = [sps.as_ptr(), pps.as_ptr()];
        let sizes = [sps.len(), pps.len()];
        let mut desc: CMFormatDescriptionRef = ptr::null();
        // SAFETY: two live byte slices and an out pointer; we own the result.
        let st = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                ptr::null(),
                2,
                ptrs.as_ptr(),
                sizes.as_ptr(),
                4,
                &mut desc,
            )
        };
        if st != 0 || desc.is_null() {
            return Err(DecodeError::Corrupt(format!(
                "VideoToolbox rejected the SPS and PPS (OSStatus {st})"
            )));
        }
        let format = Cf(desc);
        let keep = !self.session.is_null()
            // SAFETY: a live session and format description.
            && unsafe { VTDecompressionSessionCanAcceptFormatDescription(self.session, format.0) }
                != 0;
        if !keep {
            self.close_session();
        }
        self.format = format;
        self.params = Some((sps.to_vec(), pps.to_vec()));
        Ok(())
    }

    fn open_session(&mut self) -> Result<(), DecodeError> {
        let spec = dict();
        match self.pref {
            ImplPreference::HardwareOnly => {
                set_named(
                    &spec,
                    c"kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder",
                    yes(),
                );
            }
            ImplPreference::SoftwareOnly => {
                set_named(
                    &spec,
                    c"kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder",
                    no(),
                );
            }
            _ => {
                set_named(
                    &spec,
                    c"kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder",
                    yes(),
                );
            }
        }
        let attrs = pixel_buffer_attributes(PIX_420V, None);
        let record = VTDecompressionOutputCallbackRecord {
            callback: on_output,
            refcon: &*self.outbox as *const Mutex<Outbox> as *mut c_void,
        };
        let mut session: VTSessionRef = ptr::null_mut();
        // SAFETY: a live format description and dictionaries, a callback
        // record VideoToolbox copies, and an out pointer.
        let st = unsafe {
            VTDecompressionSessionCreate(
                ptr::null(),
                self.format.0,
                spec.0,
                attrs.0,
                &record,
                &mut session,
            )
        };
        if st != 0 || session.is_null() {
            return Err(match (st, self.pref) {
                (VT_DECODER_NOT_AVAILABLE_NOW, _) => DecodeError::SessionLost,
                (_, ImplPreference::HardwareOnly) => DecodeError::Video(unsupported(format!(
                    "no hardware H.264 decoder for this stream (OSStatus {st})"
                ))),
                _ => DecodeError::Video(VideoError::os(st as i64, "VTDecompressionSessionCreate")),
            });
        }
        self.session = session;
        let hw =
            self.copy_bool(c"kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder");
        // iOS decodes H.264 in hardware; an OS without the key doesn't say.
        let hw = hw.or(cfg!(target_os = "ios").then_some(true));
        self.hardware = hw;
        if self.pref == ImplPreference::HardwareOnly && hw == Some(false) {
            self.close_session();
            return Err(DecodeError::Video(unsupported(
                "VideoToolbox gave a software decoder when hardware was required",
            )));
        }
        if let Some(k) = cf_const(c"kVTDecompressionPropertyKey_RealTime") {
            // SAFETY: a live session, a framework key, a CF boolean.
            unsafe { VTSessionSetProperty(self.session, k, yes()) };
        }
        Ok(())
    }

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
            Some(CFBooleanGetValue(owned.0) != 0)
        }
    }

    /// The AVCC sample for `nals`, in a block buffer CoreMedia owns.
    fn sample(&self, nals: &[&[u8]], capture_us: u64) -> Result<Cf, DecodeError> {
        let mut avcc = Vec::new();
        for nal in nals {
            if nal.len() > u32::MAX as usize {
                return Err(DecodeError::Corrupt("a NAL unit over 4 GiB".into()));
            }
            h264::push_avcc(&mut avcc, nal);
        }
        let len = avcc.len();
        let mut bb: CMBlockBufferRef = ptr::null_mut();
        // SAFETY: CoreMedia allocates `len` bytes now (null memory and the
        // default allocator); we copy into them before the buffer is used,
        // and own the block buffer after.
        let st = unsafe {
            CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                len,
                ptr::null(),
                ptr::null(),
                0,
                len,
                BLOCK_BUFFER_ASSURE_MEMORY_NOW,
                &mut bb,
            )
        };
        if st != 0 || bb.is_null() {
            return Err(DecodeError::Video(VideoError::os(
                st as i64,
                "CMBlockBufferCreateWithMemoryBlock",
            )));
        }
        let block = Cf(bb as CFTypeRef);
        // SAFETY: `len` bytes from a live Vec into a block buffer of `len`.
        let st =
            unsafe { CMBlockBufferReplaceDataBytes(avcc.as_ptr() as *const c_void, bb, 0, len) };
        if st != 0 {
            return Err(DecodeError::Video(VideoError::os(
                st as i64,
                "CMBlockBufferReplaceDataBytes",
            )));
        }
        let timing = CMSampleTimingInfo {
            duration: CM_TIME_INVALID,
            pts: cm_time(capture_us.min(i64::MAX as u64) as i64, 1_000_000),
            dts: CM_TIME_INVALID,
        };
        let mut sb: CMSampleBufferRef = ptr::null_mut();
        // SAFETY: a live block buffer and format description, one timing
        // entry and one size; we own the sample buffer.
        let st = unsafe {
            CMSampleBufferCreateReady(
                ptr::null(),
                bb,
                self.format.0,
                1,
                1,
                &timing,
                1,
                &len,
                &mut sb,
            )
        };
        drop(block);
        if st != 0 || sb.is_null() {
            return Err(DecodeError::Video(VideoError::os(
                st as i64,
                "CMSampleBufferCreateReady",
            )));
        }
        Ok(Cf(sb as CFTypeRef))
    }
}

impl DecoderCore for VtDecoder {
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError> {
        if input.codec != VideoCodec::H264 {
            return Err(DecodeError::Video(unsupported(format!(
                "{:?} into the H.264 decoder",
                input.codec
            ))));
        }
        let au = split_access_unit(&input.data);
        match (au.sps, au.pps) {
            (Some(sps), Some(pps)) => self.set_params(sps, pps)?,
            (None, None) => {}
            _ => {
                return Err(DecodeError::Corrupt(
                    "an access unit with an SPS or a PPS but not both".into(),
                ))
            }
        }
        if au.nals.is_empty() {
            return Ok(None);
        }
        if self.format.0.is_null() {
            return Err(if h264::is_keyframe(&au.nals) {
                DecodeError::Corrupt("an IDR with no SPS and PPS before it".into())
            } else {
                DecodeError::NeedKeyframe
            });
        }
        if self.session.is_null() {
            self.open_session()?;
        }
        let sample = self.sample(&au.nals, input.capture_us)?;
        let mut info = 0u32;
        // SAFETY: a live session and sample buffer. Flags 0: synchronous, so
        // the callback has run when this returns; the wait makes sure.
        let st = unsafe {
            let st = VTDecompressionSessionDecodeFrame(
                self.session,
                sample.0 as CMSampleBufferRef,
                0,
                ptr::null_mut(),
                &mut info,
            );
            if st == 0 {
                VTDecompressionSessionWaitForAsynchronousFrames(self.session)
            } else {
                st
            }
        };
        let got = std::mem::take(&mut self.outbox.lock().unwrap_or_else(|e| e.into_inner()).out);
        if st != 0 {
            let e = decode_error(st, "VTDecompressionSessionDecodeFrame");
            if e == DecodeError::SessionLost {
                self.close_session();
            }
            return Err(e);
        }
        if got.len() > 1 {
            return Err(DecodeError::Video(VideoError::os(
                0,
                format!("VideoToolbox gave {} frames for one input", got.len()),
            )));
        }
        match got.into_iter().next() {
            None | Some(Ok(None)) => Ok(None),
            Some(Err(st)) => {
                let e = decode_error(st, "VideoToolbox decode");
                if e == DecodeError::SessionLost {
                    self.close_session();
                }
                Err(e)
            }
            Some(Ok(Some(pb))) => {
                // SAFETY: a pixel buffer we hold a retain on.
                let f = unsafe { native_frame(pb.0 as CVPixelBufferRef, input.capture_us) };
                Ok(Some(f))
            }
        }
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        self.close_session();
        self.format = Cf::null();
        self.params = None;
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        match (self.hardware, self.pref) {
            (Some(true), _) => Implementation::Hardware,
            (Some(false), _) | (None, ImplPreference::SoftwareOnly) => Implementation::Software,
            (None, _) => Implementation::Hardware,
        }
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        self.close_session();
    }
}

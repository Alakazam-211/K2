//! H.264 through VideoToolbox (macOS system framework; hardware on Apple
//! silicon and on Intel Macs with a T2 / Quick Sync, software otherwise).
//!
//! Hand-written FFI, no crates. Every framework *constant* (property keys,
//! profile names) is looked up at run time with `dlsym`, so a key that an
//! older macOS lacks (e.g. `EnableLowLatencyRateControl`, 11.3+) is
//! skipped instead of failing the daemon's launch on a missing symbol.
//!
//! Settings: real-time, low-latency rate control when the OS has it, no
//! frame reordering (no B-frames), Constrained High (Main when the OS has no
//! Constrained High), keyframes only when asked (first frame, size change,
//! a viewer's request) plus a long safety interval, and the caller's
//! bitrate / fps.

use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

use crate::annexb::{avcc_to_annexb, push_nal};
use crate::{pad_even, Chunk, EncoderConfig, VideoEncoder};

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFMutableDictionaryRef = *mut c_void;
type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFBooleanRef = *const c_void;
type VTSessionRef = *mut c_void;
type CVPixelBufferRef = *mut c_void;
type CVPixelBufferPoolRef = *mut c_void;
type CMSampleBufferRef = *mut c_void;
type CMBlockBufferRef = *mut c_void;
type CMFormatDescriptionRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CM_TIME_INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

fn cm_time(value: i64, timescale: i32) -> CMTime {
    CMTime { value, timescale, flags: 1, epoch: 0 }
}

#[repr(C)]
struct CFDictionaryKeyCallBacks {
    _p: [usize; 6],
}
#[repr(C)]
struct CFDictionaryValueCallBacks {
    _p: [usize; 5],
}

type OutputCallback = extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFBooleanRef;
    static kCFBooleanFalse: CFBooleanRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    fn CFDictionaryCreateMutable(a: CFAllocatorRef, cap: isize, k: *const c_void, v: *const c_void) -> CFMutableDictionaryRef;
    fn CFDictionarySetValue(d: CFMutableDictionaryRef, k: *const c_void, v: *const c_void);
    fn CFDictionaryGetValue(d: CFDictionaryRef, k: *const c_void) -> *const c_void;
    fn CFNumberCreate(a: CFAllocatorRef, number_type: isize, value: *const c_void) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFArrayGetCount(a: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> *const c_void;
    fn CFBooleanGetValue(b: CFBooleanRef) -> u8;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: Option<OutputCallback>,
        refcon: *mut c_void,
        session_out: *mut VTSessionRef,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: VTSessionRef, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: VTSessionRef) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: VTSessionRef,
        image: CVPixelBufferRef,
        pts: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: VTSessionRef, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: VTSessionRef);
    fn VTCompressionSessionGetPixelBufferPool(session: VTSessionRef) -> CVPixelBufferPoolRef;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferPoolCreatePixelBuffer(a: CFAllocatorRef, pool: CVPixelBufferPoolRef, out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferCreate(a: CFAllocatorRef, w: usize, h: usize, format: u32, attrs: CFDictionaryRef, out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: CVPixelBufferRef) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pb: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetWidth(pb: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetHeight(pb: CVPixelBufferRef) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetDataBuffer(sb: CMSampleBufferRef) -> CMBlockBufferRef;
    fn CMSampleBufferGetFormatDescription(sb: CMSampleBufferRef) -> CMFormatDescriptionRef;
    fn CMSampleBufferGetSampleAttachmentsArray(sb: CMSampleBufferRef, create: u8) -> CFArrayRef;
    fn CMBlockBufferGetDataLength(bb: CMBlockBufferRef) -> usize;
    fn CMBlockBufferCopyDataBytes(bb: CMBlockBufferRef, offset: usize, len: usize, dest: *mut c_void) -> OSStatus;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: CMFormatDescriptionRef,
        index: usize,
        ptr_out: *mut *const u8,
        size_out: *mut usize,
        count_out: *mut usize,
        nal_header_len_out: *mut i32,
    ) -> OSStatus;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_DEFAULT` on Darwin.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const K_CMVIDEO_CODEC_H264: u32 = u32::from_be_bytes(*b"avc1");
const K_CVPIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const K_CFNUMBER_SINT32: isize = 3;
const K_CFNUMBER_FLOAT64: isize = 6;
/// `kVTEncodeInfo_FrameDropped`.
const INFO_FRAME_DROPPED: u32 = 1 << 1;

/// A framework `const CFStringRef` by name, or None on an OS without it.
pub(crate) fn cf_const(name: &CStr) -> Option<CFStringRef> {
    // SAFETY: dlsym with RTLD_DEFAULT; the symbol, when present, is a
    // `const CFStringRef` variable, so we read one pointer from it.
    unsafe {
        let p = dlsym(RTLD_DEFAULT, name.as_ptr());
        if p.is_null() {
            return None;
        }
        let v = *(p as *const CFStringRef);
        (!v.is_null()).then_some(v)
    }
}

/// An owned CF object (released on drop).
struct Cf(CFTypeRef);
impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one retain on this object.
            unsafe { CFRelease(self.0) }
        }
    }
}

fn dict() -> Cf {
    // SAFETY: standard CF type callbacks.
    Cf(unsafe {
        CFDictionaryCreateMutable(
            ptr::null(),
            0,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        )
    })
}

fn num_i32(v: i32) -> Cf {
    // SAFETY: a pointer to a live i32.
    Cf(unsafe { CFNumberCreate(ptr::null(), K_CFNUMBER_SINT32, &v as *const i32 as *const c_void) })
}

fn num_f64(v: f64) -> Cf {
    // SAFETY: a pointer to a live f64.
    Cf(unsafe { CFNumberCreate(ptr::null(), K_CFNUMBER_FLOAT64, &v as *const f64 as *const c_void) })
}

fn set(d: &Cf, key: CFStringRef, value: CFTypeRef) {
    // SAFETY: d is a mutable dictionary we own; CF retains key and value.
    unsafe { CFDictionarySetValue(d.0 as CFMutableDictionaryRef, key, value) }
}

fn yes() -> CFTypeRef {
    // SAFETY: a framework constant.
    unsafe { kCFBooleanTrue }
}

fn no() -> CFTypeRef {
    // SAFETY: a framework constant.
    unsafe { kCFBooleanFalse }
}

/// What the output callback fills (one entry per encoded frame).
#[derive(Default)]
struct Outbox {
    out: Vec<Result<Option<Chunk>, String>>,
}

extern "C" fn on_output(refcon: *mut c_void, _frame: *mut c_void, status: OSStatus, flags: u32, sb: CMSampleBufferRef) {
    // SAFETY: refcon is the encoder's boxed Mutex<Outbox>, alive until the
    // session is invalidated (Drop invalidates first).
    let outbox = unsafe { &*(refcon as *const Mutex<Outbox>) };
    let r = if status != 0 {
        Err(format!("VideoToolbox encode failed (OSStatus {status})"))
    } else if sb.is_null() || flags & INFO_FRAME_DROPPED != 0 {
        Ok(None)
    } else {
        // SAFETY: a live sample buffer for the duration of the callback.
        unsafe { sample_to_chunk(sb) }.map(Some)
    };
    outbox.lock().unwrap_or_else(|e| e.into_inner()).out.push(r);
}

/// AVCC sample → Annex B, with SPS + PPS in front of a keyframe.
unsafe fn sample_to_chunk(sb: CMSampleBufferRef) -> Result<Chunk, String> {
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
        return Err("encoded sample has no format description".into());
    }
    let mut count = 0usize;
    let mut nal_len = 0i32;
    let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, 0, ptr::null_mut(), ptr::null_mut(), &mut count, &mut nal_len);
    if st != 0 {
        return Err(format!("H.264 parameter sets unreadable (OSStatus {st})"));
    }
    let mut data = Vec::new();
    if key {
        for i in 0..count {
            let mut p: *const u8 = ptr::null();
            let mut n = 0usize;
            let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, i, &mut p, &mut n, ptr::null_mut(), ptr::null_mut());
            if st != 0 || p.is_null() {
                return Err(format!("H.264 parameter set {i} unreadable (OSStatus {st})"));
            }
            push_nal(&mut data, std::slice::from_raw_parts(p, n));
        }
    }
    let bb = CMSampleBufferGetDataBuffer(sb);
    if bb.is_null() {
        return Err("encoded sample has no data".into());
    }
    let len = CMBlockBufferGetDataLength(bb);
    let mut avcc = vec![0u8; len];
    let st = CMBlockBufferCopyDataBytes(bb, 0, len, avcc.as_mut_ptr() as *mut c_void);
    if st != 0 {
        return Err(format!("encoded sample copy failed (OSStatus {st})"));
    }
    avcc_to_annexb(&avcc, nal_len as usize, &mut data)?;
    Ok(Chunk { key, data })
}

pub struct VtEncoder {
    session: VTSessionRef,
    outbox: Box<Mutex<Outbox>>,
    cfg: EncoderConfig,
    padded: (u32, u32),
    last_pts: i64,
    low_latency: bool,
}

// SAFETY: the session is only driven from the thread that owns the encoder
// (one call at a time through &mut self); VideoToolbox sessions may be used
// from any thread.
unsafe impl Send for VtEncoder {}

impl VtEncoder {
    pub fn open(cfg: &EncoderConfig) -> Result<VtEncoder, String> {
        let padded = (cfg.width + (cfg.width & 1), cfg.height + (cfg.height & 1));
        let outbox: Box<Mutex<Outbox>> = Box::default();
        let attrs = dict();
        if let (Some(fmt), Some(w), Some(h), Some(io)) = (
            cf_const(c"kCVPixelBufferPixelFormatTypeKey"),
            cf_const(c"kCVPixelBufferWidthKey"),
            cf_const(c"kCVPixelBufferHeightKey"),
            cf_const(c"kCVPixelBufferIOSurfacePropertiesKey"),
        ) {
            set(&attrs, fmt, num_i32(K_CVPIXEL_FORMAT_BGRA as i32).0);
            set(&attrs, w, num_i32(padded.0 as i32).0);
            set(&attrs, h, num_i32(padded.1 as i32).0);
            set(&attrs, io, dict().0);
        }
        let spec = dict();
        let low_key = cf_const(c"kVTVideoEncoderSpecification_EnableLowLatencyRateControl");
        if let Some(k) = low_key {
            set(&spec, k, yes());
        }
        let refcon = &*outbox as *const Mutex<Outbox> as *mut c_void;
        let mut session: VTSessionRef = ptr::null_mut();
        let create = |spec: CFDictionaryRef, out: &mut VTSessionRef| -> OSStatus {
            // SAFETY: valid dictionaries (or null), a live refcon, an out pointer.
            unsafe {
                VTCompressionSessionCreate(
                    ptr::null(),
                    padded.0 as i32,
                    padded.1 as i32,
                    K_CMVIDEO_CODEC_H264,
                    spec,
                    attrs.0,
                    ptr::null(),
                    Some(on_output),
                    refcon,
                    out,
                )
            }
        };
        let mut low_latency = low_key.is_some();
        let mut st = if low_latency { create(spec.0, &mut session) } else { -1 };
        if st != 0 || session.is_null() {
            low_latency = false;
            session = ptr::null_mut();
            st = create(ptr::null(), &mut session);
        }
        if st != 0 || session.is_null() {
            return Err(format!("VideoToolbox has no H.264 encoder for {}x{} (OSStatus {st})", padded.0, padded.1));
        }
        let mut enc = VtEncoder { session, outbox, cfg: *cfg, padded, last_pts: -1, low_latency };
        enc.prop(c"kVTCompressionPropertyKey_RealTime", yes());
        enc.prop(c"kVTCompressionPropertyKey_AllowFrameReordering", no());
        let profile_set = cf_const(c"kVTProfileLevel_H264_ConstrainedHigh_AutoLevel")
            .is_some_and(|p| enc.prop(c"kVTCompressionPropertyKey_ProfileLevel", p));
        if !profile_set {
            if let Some(p) = cf_const(c"kVTProfileLevel_H264_Main_AutoLevel") {
                enc.prop(c"kVTCompressionPropertyKey_ProfileLevel", p);
            }
        }
        // Keyframes come on request; this is only a safety net (10 min).
        enc.prop(c"kVTCompressionPropertyKey_MaxKeyFrameInterval", num_i32(36_000).0);
        enc.prop(c"kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration", num_f64(600.0).0);
        enc.prop(c"kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality", yes());
        enc.set_rate(cfg.fps, cfg.bitrate_bps);
        // SAFETY: a live session.
        unsafe { VTCompressionSessionPrepareToEncodeFrames(enc.session) };
        Ok(enc)
    }

    /// Set one property; false when the OS lacks the key or refuses it.
    fn prop(&mut self, key: &CStr, value: CFTypeRef) -> bool {
        let Some(k) = cf_const(key) else { return false };
        // SAFETY: a live session, a framework key, a CF value.
        unsafe { VTSessionSetProperty(self.session, k, value) == 0 }
    }

    fn pixel_buffer(&self) -> Result<CVPixelBufferRef, String> {
        let mut pb: CVPixelBufferRef = ptr::null_mut();
        // SAFETY: a live session; out pointers.
        let st = unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            if pool.is_null() {
                CVPixelBufferCreate(ptr::null(), self.padded.0 as usize, self.padded.1 as usize, K_CVPIXEL_FORMAT_BGRA, ptr::null(), &mut pb)
            } else {
                CVPixelBufferPoolCreatePixelBuffer(ptr::null(), pool, &mut pb)
            }
        };
        if st != 0 || pb.is_null() {
            return Err(format!("no pixel buffer for the encoder (CVReturn {st})"));
        }
        Ok(pb)
    }
}

impl VideoEncoder for VtEncoder {
    fn encode_rgb(&mut self, rgb: &[u8], force_key: bool, ts_ms: u32) -> Result<Option<Chunk>, String> {
        let (px, pw, ph) = pad_even(rgb, self.cfg.width, self.cfg.height)?;
        if (pw, ph) != self.padded {
            return Err(format!("frame {pw}x{ph} is not the encoder's {}x{}", self.padded.0, self.padded.1));
        }
        let pb = self.pixel_buffer()?;
        let pb_owned = Cf(pb as CFTypeRef);
        // SAFETY: a locked pixel buffer of at least padded size; rows are
        // written within bytes_per_row.
        unsafe {
            if CVPixelBufferLockBaseAddress(pb, 0) != 0 {
                return Err("pixel buffer lock failed".into());
            }
            let base = CVPixelBufferGetBaseAddress(pb) as *mut u8;
            let bpr = CVPixelBufferGetBytesPerRow(pb);
            let (bw, bh) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb));
            if base.is_null() || bw < pw as usize || bh < ph as usize || bpr < pw as usize * 4 {
                CVPixelBufferUnlockBaseAddress(pb, 0);
                return Err(format!("pixel buffer {bw}x{bh} (row {bpr}) can't hold {pw}x{ph}"));
            }
            for y in 0..ph as usize {
                let src = &px[y * pw as usize * 3..][..pw as usize * 3];
                let dst = std::slice::from_raw_parts_mut(base.add(y * bpr), pw as usize * 4);
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);
        }
        let props = if force_key {
            let d = dict();
            if let Some(k) = cf_const(c"kVTEncodeFrameOptionKey_ForceKeyFrame") {
                set(&d, k, yes());
            }
            Some(d)
        } else {
            None
        };
        let pts = (ts_ms as i64).max(self.last_pts + 1);
        self.last_pts = pts;
        let mut info = 0u32;
        // SAFETY: a live session and pixel buffer; the frame props dict (or
        // null) outlives the call.
        let st = unsafe {
            let st = VTCompressionSessionEncodeFrame(
                self.session,
                pb,
                cm_time(pts, 1000),
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
        drop(pb_owned);
        if st != 0 {
            return Err(format!("VideoToolbox encode failed (OSStatus {st})"));
        }
        let mut ob = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
        let mut got = ob.out.drain(..).collect::<Vec<_>>();
        drop(ob);
        // CompleteFrames flushed, so this is the frame we just gave it (or
        // nothing if rate control dropped it). More than one would mean a
        // frame delay we turned off, and dropping one would corrupt the
        // reference chain: fail loudly (the viewer falls back to JPEG).
        if got.len() > 1 {
            return Err(format!("VideoToolbox emitted {} frames for one input", got.len()));
        }
        Ok(got.pop().transpose()?.flatten())
    }

    fn set_rate(&mut self, fps: u32, bitrate_bps: u32) {
        self.cfg.fps = fps;
        self.cfg.bitrate_bps = bitrate_bps;
        self.prop(c"kVTCompressionPropertyKey_AverageBitRate", num_i32(bitrate_bps.min(i32::MAX as u32) as i32).0);
        self.prop(c"kVTCompressionPropertyKey_ExpectedFrameRate", num_i32(fps.max(1) as i32).0);
    }

    fn config(&self) -> EncoderConfig {
        self.cfg
    }

    fn name(&self) -> &'static str {
        if self.low_latency {
            "videotoolbox, low-latency rate control"
        } else {
            "videotoolbox"
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annexb::{codec_string, is_keyframe, nal_units, nal_type, NAL_PPS, NAL_SPS};

    fn scene(w: u32, h: u32, t: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                let band = ((y + t * 4) / 16) % 4;
                v[i] = (band * 60) as u8;
                v[i + 1] = ((x * 255) / w) as u8;
                v[i + 2] = 200;
            }
        }
        v
    }

    #[test]
    fn framework_constants_are_found() {
        for k in [
            c"kVTCompressionPropertyKey_RealTime",
            c"kVTCompressionPropertyKey_AverageBitRate",
            c"kVTEncodeFrameOptionKey_ForceKeyFrame",
            c"kCMSampleAttachmentKey_NotSync",
            c"kCVPixelBufferPixelFormatTypeKey",
        ] {
            assert!(cf_const(k).is_some(), "{k:?} missing");
        }
        assert!(cf_const(c"kVTNoSuchKey_K2").is_none());
    }

    #[test]
    fn encodes_keyframe_then_deltas_and_forces_keys_on_request() {
        // Odd size on purpose (points sizes are often odd).
        let (w, h) = (411, 923);
        let mut e = VtEncoder::open(&EncoderConfig { width: w, height: h, fps: 30, bitrate_bps: 2_000_000 }).expect("open");
        let first = e.encode_rgb(&scene(w, h, 0), false, 0).expect("first").expect("not dropped");
        assert!(first.key, "the first frame is a keyframe");
        assert!(is_keyframe(&first.data));
        let types: Vec<u8> = nal_units(&first.data).iter().filter_map(|n| nal_type(n)).collect();
        assert!(types.contains(&NAL_SPS) && types.contains(&NAL_PPS), "SPS + PPS in band: {types:?}");
        let codec = codec_string(&first.data).expect("codec");
        assert!(codec.starts_with("avc1.64") || codec.starts_with("avc1.4D"), "{codec}");
        let mut deltas = 0;
        for t in 1..20 {
            let c = e.encode_rgb(&scene(w, h, t), false, t * 33).expect("encode");
            if let Some(c) = c {
                assert_eq!(c.key, is_keyframe(&c.data));
                if !c.key {
                    deltas += 1;
                    assert!(c.data.len() < first.data.len(), "a delta ({}) is smaller than the keyframe ({})", c.data.len(), first.data.len());
                }
            }
        }
        assert!(deltas >= 15, "no B-frames, no surprise keyframes: {deltas} deltas of 19");
        let forced = e.encode_rgb(&scene(w, h, 21), true, 700).expect("forced").expect("not dropped");
        assert!(forced.key && is_keyframe(&forced.data), "a forced keyframe");
        e.set_rate(10, 500_000);
        assert_eq!(e.config().bitrate_bps, 500_000);
        assert!(e.encode_rgb(&scene(w, h - 2, 0), false, 800).is_err(), "a wrong size fails loudly");
        eprintln!("vt: low_latency={} codec={codec} key={} B", e.low_latency, first.data.len());
    }

    #[test]
    fn the_host_probe_says_yes_on_a_mac() {
        assert!(crate::h264_available());
    }
}

//! Hand-written FFI to CoreFoundation, CoreMedia, CoreVideo and
//! VideoToolbox (system frameworks on macOS and iOS; nothing here adds a
//! crate). Framework *constants* are looked up at run time with `dlsym`
//! ([`cf_const`]), so a key an older OS (or the other Apple OS) lacks is
//! skipped instead of failing at launch.

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use objc2::encode::{Encode, Encoding, RefEncode};
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::ptr;

pub type OSStatus = i32;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFBooleanRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type VTSessionRef = *mut c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CVPixelBufferPoolRef = *mut c_void;
pub type CMSampleBufferRef = *mut c_void;
pub type CMBlockBufferRef = *mut c_void;
pub type CMFormatDescriptionRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

// SAFETY: CMTime is `{?=qiIq}` in the Objective-C runtime.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}
// SAFETY: a pointer to the struct above.
unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<CMTime as Encode>::ENCODING);
}

pub const CM_TIME_INVALID: CMTime = CMTime {
    value: 0,
    timescale: 0,
    flags: 0,
    epoch: 0,
};

pub fn cm_time(value: i64, timescale: i32) -> CMTime {
    CMTime {
        value,
        timescale,
        flags: 1,
        epoch: 0,
    }
}

#[repr(C)]
pub struct CMSampleTimingInfo {
    pub duration: CMTime,
    pub pts: CMTime,
    pub dts: CMTime,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CMVideoDimensions {
    pub width: i32,
    pub height: i32,
}

/// A `CMSampleBufferRef` argument with the runtime's encoding
/// (`^{opaqueCMSampleBuffer=}`), for Objective-C delegate methods.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct SampleBufferArg(pub CMSampleBufferRef);
// SAFETY: a pointer to the opaque struct, as the runtime describes it.
unsafe impl Encode for SampleBufferArg {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// A `CMFormatDescriptionRef` return value with the runtime's encoding.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct FormatDescriptionArg(pub CMFormatDescriptionRef);
// SAFETY: a pointer to the opaque struct, as the runtime describes it.
unsafe impl Encode for FormatDescriptionArg {
    const ENCODING: Encoding =
        Encoding::Pointer(&Encoding::Struct("opaqueCMFormatDescription", &[]));
}

#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _p: [usize; 6],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _p: [usize; 5],
}
#[repr(C)]
pub struct CFArrayCallBacks {
    _p: [usize; 5],
}

pub type CompressionOutputCallback =
    extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef);
pub type DecompressionOutputCallback =
    extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CVPixelBufferRef, CMTime, CMTime);

#[repr(C)]
pub struct VTDecompressionOutputCallbackRecord {
    pub callback: DecompressionOutputCallback,
    pub refcon: *mut c_void,
}

// For a static library's link, which never sees `#[link]` (see the
// backend's module docs). `Cf`'s retain and release live in this module.
autolink!("CoreFoundation", "CoreMedia", "CoreVideo", "VideoToolbox");

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFBooleanTrue: CFBooleanRef;
    pub static kCFBooleanFalse: CFBooleanRef;
    pub static kCFAllocatorNull: CFAllocatorRef;
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    pub static kCFTypeArrayCallBacks: CFArrayCallBacks;
    pub fn CFDictionaryCreateMutable(
        a: CFAllocatorRef,
        cap: isize,
        k: *const c_void,
        v: *const c_void,
    ) -> CFMutableDictionaryRef;
    pub fn CFDictionarySetValue(d: CFMutableDictionaryRef, k: *const c_void, v: *const c_void);
    pub fn CFDictionaryGetValue(d: CFDictionaryRef, k: *const c_void) -> *const c_void;
    pub fn CFNumberCreate(
        a: CFAllocatorRef,
        number_type: isize,
        value: *const c_void,
    ) -> CFNumberRef;
    pub fn CFNumberGetValue(n: CFNumberRef, number_type: isize, value: *mut c_void) -> u8;
    pub fn CFArrayCreate(
        a: CFAllocatorRef,
        values: *const *const c_void,
        n: isize,
        cb: *const CFArrayCallBacks,
    ) -> CFArrayRef;
    pub fn CFArrayGetCount(a: CFArrayRef) -> isize;
    pub fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> *const c_void;
    pub fn CFBooleanGetValue(b: CFBooleanRef) -> u8;
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    pub fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: Option<CompressionOutputCallback>,
        refcon: *mut c_void,
        session_out: *mut VTSessionRef,
    ) -> OSStatus;
    pub fn VTSessionSetProperty(
        session: VTSessionRef,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> OSStatus;
    pub fn VTSessionCopyProperty(
        session: VTSessionRef,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        value_out: *mut CFTypeRef,
    ) -> OSStatus;
    pub fn VTCompressionSessionPrepareToEncodeFrames(session: VTSessionRef) -> OSStatus;
    pub fn VTCompressionSessionEncodeFrame(
        session: VTSessionRef,
        image: CVPixelBufferRef,
        pts: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    pub fn VTCompressionSessionCompleteFrames(session: VTSessionRef, until: CMTime) -> OSStatus;
    pub fn VTCompressionSessionInvalidate(session: VTSessionRef);
    pub fn VTCompressionSessionGetPixelBufferPool(session: VTSessionRef) -> CVPixelBufferPoolRef;

    pub fn VTDecompressionSessionCreate(
        allocator: CFAllocatorRef,
        format: CMFormatDescriptionRef,
        decoder_specification: CFDictionaryRef,
        destination_attributes: CFDictionaryRef,
        callback: *const VTDecompressionOutputCallbackRecord,
        session_out: *mut VTSessionRef,
    ) -> OSStatus;
    pub fn VTDecompressionSessionDecodeFrame(
        session: VTSessionRef,
        sample: CMSampleBufferRef,
        flags: u32,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    pub fn VTDecompressionSessionWaitForAsynchronousFrames(session: VTSessionRef) -> OSStatus;
    pub fn VTDecompressionSessionCanAcceptFormatDescription(
        session: VTSessionRef,
        format: CMFormatDescriptionRef,
    ) -> u8;
    pub fn VTDecompressionSessionInvalidate(session: VTSessionRef);
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub fn CVPixelBufferPoolCreatePixelBuffer(
        a: CFAllocatorRef,
        pool: CVPixelBufferPoolRef,
        out: *mut CVPixelBufferRef,
    ) -> i32;
    pub fn CVPixelBufferCreate(
        a: CFAllocatorRef,
        w: usize,
        h: usize,
        format: u32,
        attrs: CFDictionaryRef,
        out: *mut CVPixelBufferRef,
    ) -> i32;
    pub fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferGetBaseAddress(pb: CVPixelBufferRef) -> *mut c_void;
    pub fn CVPixelBufferGetBaseAddressOfPlane(pb: CVPixelBufferRef, plane: usize) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetHeightOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetWidthOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetPlaneCount(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetWidth(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetHeight(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetPixelFormatType(pb: CVPixelBufferRef) -> u32;
    pub fn CVBufferSetAttachment(
        buffer: CVPixelBufferRef,
        key: CFStringRef,
        value: CFTypeRef,
        mode: u32,
    );
    pub fn CVBufferGetAttachment(
        buffer: CVPixelBufferRef,
        key: CFStringRef,
        mode_out: *mut u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub fn CMSampleBufferGetDataBuffer(sb: CMSampleBufferRef) -> CMBlockBufferRef;
    pub fn CMSampleBufferGetImageBuffer(sb: CMSampleBufferRef) -> CVPixelBufferRef;
    pub fn CMSampleBufferGetFormatDescription(sb: CMSampleBufferRef) -> CMFormatDescriptionRef;
    pub fn CMSampleBufferGetSampleAttachmentsArray(sb: CMSampleBufferRef, create: u8)
        -> CFArrayRef;
    pub fn CMBlockBufferGetDataLength(bb: CMBlockBufferRef) -> usize;
    pub fn CMBlockBufferCopyDataBytes(
        bb: CMBlockBufferRef,
        offset: usize,
        len: usize,
        dest: *mut c_void,
    ) -> OSStatus;
    pub fn CMBlockBufferReplaceDataBytes(
        src: *const c_void,
        dest: CMBlockBufferRef,
        offset: usize,
        len: usize,
    ) -> OSStatus;
    pub fn CMBlockBufferCreateWithMemoryBlock(
        a: CFAllocatorRef,
        memory: *mut c_void,
        len: usize,
        block_allocator: CFAllocatorRef,
        custom: *const c_void,
        offset: usize,
        data_len: usize,
        flags: u32,
        out: *mut CMBlockBufferRef,
    ) -> OSStatus;
    pub fn CMSampleBufferCreateReady(
        a: CFAllocatorRef,
        data: CMBlockBufferRef,
        format: CMFormatDescriptionRef,
        num_samples: isize,
        num_timing: isize,
        timing: *const CMSampleTimingInfo,
        num_sizes: isize,
        sizes: *const usize,
        out: *mut CMSampleBufferRef,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: CMFormatDescriptionRef,
        index: usize,
        ptr_out: *mut *const u8,
        size_out: *mut usize,
        count_out: *mut usize,
        nal_header_len_out: *mut i32,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        a: CFAllocatorRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_header_len: i32,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetDimensions(desc: CMFormatDescriptionRef)
        -> CMVideoDimensions;
    pub fn CMFormatDescriptionGetMediaSubType(desc: CMFormatDescriptionRef) -> u32;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
}

pub const RTLD_LAZY: i32 = 1;
/// `RTLD_DEFAULT` on Darwin.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

pub const K_CMVIDEO_CODEC_H264: u32 = u32::from_be_bytes(*b"avc1");
/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`.
pub const PIX_420V: u32 = u32::from_be_bytes(*b"420v");
/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`.
pub const PIX_420F: u32 = u32::from_be_bytes(*b"420f");
pub const PIX_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
pub const K_CFNUMBER_SINT32: isize = 3;
pub const K_CFNUMBER_SINT64: isize = 4;
pub const K_CFNUMBER_FLOAT64: isize = 6;
/// `kVTEncodeInfo_FrameDropped`.
pub const INFO_FRAME_DROPPED: u32 = 1 << 1;
/// `kCVPixelBufferLock_ReadOnly`.
pub const LOCK_READ_ONLY: u64 = 1;
/// `kCVAttachmentMode_ShouldPropagate`.
pub const ATTACH_PROPAGATE: u32 = 1;
/// `kVTInvalidSessionErr`.
pub const VT_INVALID_SESSION: OSStatus = -12903;
/// `kVTVideoDecoderBadDataErr`.
pub const VT_BAD_DATA: OSStatus = -12909;
/// `kVTVideoDecoderMalfunctionErr`.
pub const VT_DECODER_MALFUNCTION: OSStatus = -12911;
/// `kVTVideoDecoderNotAvailableNowErr` (iOS: the app is in the background).
pub const VT_DECODER_NOT_AVAILABLE_NOW: OSStatus = -12913;
/// `kVTVideoEncoderNotAvailableNowErr` (iOS: the app is in the background).
pub const VT_ENCODER_NOT_AVAILABLE_NOW: OSStatus = -12915;
/// `kVTDecodeInfo_FrameDropped`.
pub const DECODE_INFO_FRAME_DROPPED: u32 = 1 << 1;
/// `kCMBlockBufferAssureMemoryNowFlag`.
pub const BLOCK_BUFFER_ASSURE_MEMORY_NOW: u32 = 1;

/// A framework constant (a `const CFStringRef` or other object pointer) by
/// name, or `None` on an OS without it.
pub fn cf_const(name: &CStr) -> Option<CFStringRef> {
    // SAFETY: dlsym with RTLD_DEFAULT; the symbol, when present, is a
    // pointer-sized constant, so we read one pointer from it.
    unsafe {
        let p = dlsym(RTLD_DEFAULT, name.as_ptr());
        if p.is_null() {
            return None;
        }
        let v = *(p as *const CFStringRef);
        (!v.is_null()).then_some(v)
    }
}

/// A C function by name, or `None`.
pub fn symbol(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: dlsym with RTLD_DEFAULT only looks the name up.
    let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// An owned CF object (one retain, released on drop).
pub struct Cf(pub CFTypeRef);

// SAFETY: CF objects here are immutable once built (dictionaries, numbers,
// format descriptions) or thread-safe (pixel buffers with locking).
unsafe impl Send for Cf {}
// SAFETY: as above.
unsafe impl Sync for Cf {}

impl Cf {
    pub fn null() -> Cf {
        Cf(ptr::null())
    }

    /// Take one more retain on `p` (not null).
    ///
    /// # Safety
    /// `p` must be a live CF object.
    pub unsafe fn retain(p: CFTypeRef) -> Cf {
        Cf(CFRetain(p))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one retain on this object.
            unsafe { CFRelease(self.0) }
        }
    }
}

pub fn dict() -> Cf {
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

pub fn num_i32(v: i32) -> Cf {
    // SAFETY: a pointer to a live i32.
    Cf(unsafe {
        CFNumberCreate(
            ptr::null(),
            K_CFNUMBER_SINT32,
            &v as *const i32 as *const c_void,
        )
    })
}

pub fn num_i64(v: i64) -> Cf {
    // SAFETY: a pointer to a live i64.
    Cf(unsafe {
        CFNumberCreate(
            ptr::null(),
            K_CFNUMBER_SINT64,
            &v as *const i64 as *const c_void,
        )
    })
}

pub fn num_f64(v: f64) -> Cf {
    // SAFETY: a pointer to a live f64.
    Cf(unsafe {
        CFNumberCreate(
            ptr::null(),
            K_CFNUMBER_FLOAT64,
            &v as *const f64 as *const c_void,
        )
    })
}

pub fn number_i64(n: CFNumberRef) -> Option<i64> {
    if n.is_null() {
        return None;
    }
    let mut v = 0i64;
    // SAFETY: a CFNumber (the caller's), an out pointer to an i64.
    let ok = unsafe { CFNumberGetValue(n, K_CFNUMBER_SINT64, &mut v as *mut i64 as *mut c_void) };
    (ok != 0).then_some(v)
}

pub fn array(values: &[CFTypeRef]) -> Cf {
    // SAFETY: a slice of live CF objects; CF retains them.
    Cf(unsafe {
        CFArrayCreate(
            ptr::null(),
            values.as_ptr(),
            values.len() as isize,
            &kCFTypeArrayCallBacks,
        )
    })
}

pub fn set(d: &Cf, key: CFStringRef, value: CFTypeRef) {
    // SAFETY: d is a mutable dictionary we own; CF retains key and value.
    unsafe { CFDictionarySetValue(d.0 as CFMutableDictionaryRef, key, value) }
}

/// Set `d[name] = value` when the OS has the key.
pub fn set_named(d: &Cf, name: &CStr, value: CFTypeRef) -> bool {
    match cf_const(name) {
        Some(k) => {
            set(d, k, value);
            true
        }
        None => false,
    }
}

pub fn yes() -> CFTypeRef {
    // SAFETY: a framework constant.
    unsafe { kCFBooleanTrue }
}

pub fn no() -> CFTypeRef {
    // SAFETY: a framework constant.
    unsafe { kCFBooleanFalse }
}

/// Pixel-buffer attributes for `format` at `width` x `height`, backed by
/// IOSurface.
pub fn pixel_buffer_attributes(format: u32, width: Option<(u32, u32)>) -> Cf {
    let attrs = dict();
    set_named(
        &attrs,
        c"kCVPixelBufferPixelFormatTypeKey",
        num_i32(format as i32).0,
    );
    if let Some((w, h)) = width {
        set_named(&attrs, c"kCVPixelBufferWidthKey", num_i32(w as i32).0);
        set_named(&attrs, c"kCVPixelBufferHeightKey", num_i32(h as i32).0);
    }
    set_named(&attrs, c"kCVPixelBufferIOSurfacePropertiesKey", dict().0);
    attrs
}

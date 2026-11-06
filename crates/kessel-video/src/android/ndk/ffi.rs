//! The NDK's C API, declared by hand: Camera2 (`libcamera2ndk`, API 24),
//! `AImageReader` and `AMediaCodec` (`libmediandk`, API 21 to 24), the
//! system properties (`libc`) and `dlsym` (`libdl`). These are OS
//! libraries on every Android device; nothing is built or vendored.
//! Functions newer than API 24 (and `libnativewindow`'s Surface and
//! AHardwareBuffer calls, API 26 and 29) are looked up at run time
//! ([`late`]), so the library still loads on API 24.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_long, c_void, CStr};
use std::sync::OnceLock;

pub(crate) type camera_status_t = c_int;
pub(crate) type media_status_t = c_int;

/// Opaque NDK handles.
macro_rules! opaque {
    ($($name:ident),* $(,)?) => {
        $(
            #[repr(C)]
            pub(crate) struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(
    ACameraManager,
    ACameraDevice,
    ACameraMetadata,
    ACaptureRequest,
    ACameraOutputTarget,
    ACaptureSessionOutput,
    ACaptureSessionOutputContainer,
    ACameraCaptureSession,
    ACameraCaptureSession_captureCallbacks,
    AImageReader,
    AImage,
    ANativeWindow,
    AHardwareBuffer,
    AMediaCodec,
    AMediaFormat,
    AMediaCrypto,
);

#[repr(C)]
pub(crate) struct ACameraIdList {
    pub(crate) numCameras: c_int,
    pub(crate) cameraIds: *const *const c_char,
}

/// `ACameraMetadata_const_entry`: the data pointer is a union of typed
/// pointers; read it as the entry's type says.
#[repr(C)]
pub(crate) struct ACameraMetadata_const_entry {
    pub(crate) tag: u32,
    pub(crate) type_: u8,
    pub(crate) count: u32,
    pub(crate) data: *const c_void,
}

/// `ACAMERA_TYPE_*`.
pub(crate) const ACAMERA_TYPE_BYTE: u8 = 0;
pub(crate) const ACAMERA_TYPE_INT32: u8 = 1;
pub(crate) const ACAMERA_TYPE_INT64: u8 = 3;

/// Metadata tags: `(section << 16) + index`, from `NdkCameraMetadataTags.h`.
pub(crate) const ACAMERA_CONTROL_AE_TARGET_FPS_RANGE: u32 = (1 << 16) + 5;
pub(crate) const ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES: u32 = (1 << 16) + 20;
pub(crate) const ACAMERA_LENS_FACING: u32 = (8 << 16) + 5;
pub(crate) const ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS: u32 = (13 << 16) + 10;
pub(crate) const ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS: u32 = (13 << 16) + 11;
pub(crate) const ACAMERA_SENSOR_ORIENTATION: u32 = (14 << 16) + 14;

/// `TEMPLATE_RECORD`: steady frame rate, continuous video autofocus.
pub(crate) const TEMPLATE_RECORD: c_int = 3;

pub(crate) type DeviceStateCallback = unsafe extern "C" fn(*mut c_void, *mut ACameraDevice);
pub(crate) type DeviceErrorCallback = unsafe extern "C" fn(*mut c_void, *mut ACameraDevice, c_int);

#[repr(C)]
pub(crate) struct ACameraDevice_StateCallbacks {
    pub(crate) context: *mut c_void,
    pub(crate) onDisconnected: Option<DeviceStateCallback>,
    pub(crate) onError: Option<DeviceErrorCallback>,
}

pub(crate) type SessionStateCallback =
    unsafe extern "C" fn(*mut c_void, *mut ACameraCaptureSession);

#[repr(C)]
pub(crate) struct ACameraCaptureSession_stateCallbacks {
    pub(crate) context: *mut c_void,
    pub(crate) onClosed: Option<SessionStateCallback>,
    pub(crate) onReady: Option<SessionStateCallback>,
    pub(crate) onActive: Option<SessionStateCallback>,
}

pub(crate) type ImageCallback = unsafe extern "C" fn(*mut c_void, *mut AImageReader);

#[repr(C)]
pub(crate) struct AImageReader_ImageListener {
    pub(crate) context: *mut c_void,
    pub(crate) onImageAvailable: Option<ImageCallback>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AMediaCodecBufferInfo {
    pub(crate) offset: i32,
    pub(crate) size: i32,
    pub(crate) presentationTimeUs: i64,
    pub(crate) flags: u32,
}

pub(crate) const AMEDIACODEC_CONFIGURE_FLAG_ENCODE: u32 = 1;
pub(crate) const AMEDIACODEC_BUFFER_FLAG_KEY_FRAME: u32 = 1;
pub(crate) const AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
pub(crate) const AMEDIACODEC_INFO_TRY_AGAIN_LATER: isize = -1;
pub(crate) const AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
pub(crate) const AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;

#[link(name = "camera2ndk")]
extern "C" {
    pub(crate) fn ACameraManager_create() -> *mut ACameraManager;
    pub(crate) fn ACameraManager_delete(manager: *mut ACameraManager);
    pub(crate) fn ACameraManager_getCameraIdList(
        manager: *mut ACameraManager,
        list: *mut *mut ACameraIdList,
    ) -> camera_status_t;
    pub(crate) fn ACameraManager_deleteCameraIdList(list: *mut ACameraIdList);
    pub(crate) fn ACameraManager_getCameraCharacteristics(
        manager: *mut ACameraManager,
        camera_id: *const c_char,
        characteristics: *mut *mut ACameraMetadata,
    ) -> camera_status_t;
    pub(crate) fn ACameraManager_openCamera(
        manager: *mut ACameraManager,
        camera_id: *const c_char,
        callback: *mut ACameraDevice_StateCallbacks,
        device: *mut *mut ACameraDevice,
    ) -> camera_status_t;
    pub(crate) fn ACameraMetadata_getConstEntry(
        metadata: *const ACameraMetadata,
        tag: u32,
        entry: *mut ACameraMetadata_const_entry,
    ) -> camera_status_t;
    pub(crate) fn ACameraMetadata_free(metadata: *mut ACameraMetadata);
    pub(crate) fn ACameraDevice_close(device: *mut ACameraDevice) -> camera_status_t;
    pub(crate) fn ACameraDevice_createCaptureRequest(
        device: *const ACameraDevice,
        template_id: c_int,
        request: *mut *mut ACaptureRequest,
    ) -> camera_status_t;
    pub(crate) fn ACameraDevice_createCaptureSession(
        device: *mut ACameraDevice,
        outputs: *const ACaptureSessionOutputContainer,
        callbacks: *const ACameraCaptureSession_stateCallbacks,
        session: *mut *mut ACameraCaptureSession,
    ) -> camera_status_t;
    pub(crate) fn ACaptureSessionOutputContainer_create(
        container: *mut *mut ACaptureSessionOutputContainer,
    ) -> camera_status_t;
    pub(crate) fn ACaptureSessionOutputContainer_free(
        container: *mut ACaptureSessionOutputContainer,
    );
    pub(crate) fn ACaptureSessionOutputContainer_add(
        container: *mut ACaptureSessionOutputContainer,
        output: *const ACaptureSessionOutput,
    ) -> camera_status_t;
    pub(crate) fn ACaptureSessionOutput_create(
        window: *mut ANativeWindow,
        output: *mut *mut ACaptureSessionOutput,
    ) -> camera_status_t;
    pub(crate) fn ACaptureSessionOutput_free(output: *mut ACaptureSessionOutput);
    pub(crate) fn ACameraOutputTarget_create(
        window: *mut ANativeWindow,
        target: *mut *mut ACameraOutputTarget,
    ) -> camera_status_t;
    pub(crate) fn ACameraOutputTarget_free(target: *mut ACameraOutputTarget);
    pub(crate) fn ACaptureRequest_addTarget(
        request: *mut ACaptureRequest,
        target: *const ACameraOutputTarget,
    ) -> camera_status_t;
    pub(crate) fn ACaptureRequest_setEntry_i32(
        request: *mut ACaptureRequest,
        tag: u32,
        count: u32,
        data: *const i32,
    ) -> camera_status_t;
    pub(crate) fn ACaptureRequest_free(request: *mut ACaptureRequest);
    pub(crate) fn ACameraCaptureSession_setRepeatingRequest(
        session: *mut ACameraCaptureSession,
        callbacks: *mut ACameraCaptureSession_captureCallbacks,
        num_requests: c_int,
        requests: *mut *mut ACaptureRequest,
        sequence_id: *mut c_int,
    ) -> camera_status_t;
    pub(crate) fn ACameraCaptureSession_stopRepeating(
        session: *mut ACameraCaptureSession,
    ) -> camera_status_t;
    pub(crate) fn ACameraCaptureSession_close(session: *mut ACameraCaptureSession);
}

#[link(name = "mediandk")]
extern "C" {
    pub(crate) fn AImageReader_new(
        width: i32,
        height: i32,
        format: i32,
        max_images: i32,
        reader: *mut *mut AImageReader,
    ) -> media_status_t;
    pub(crate) fn AImageReader_delete(reader: *mut AImageReader);
    pub(crate) fn AImageReader_getWindow(
        reader: *mut AImageReader,
        window: *mut *mut ANativeWindow,
    ) -> media_status_t;
    pub(crate) fn AImageReader_setImageListener(
        reader: *mut AImageReader,
        listener: *mut AImageReader_ImageListener,
    ) -> media_status_t;
    pub(crate) fn AImageReader_acquireLatestImage(
        reader: *mut AImageReader,
        image: *mut *mut AImage,
    ) -> media_status_t;
    pub(crate) fn AImage_delete(image: *mut AImage);
    pub(crate) fn AImage_getFormat(image: *const AImage, format: *mut i32) -> media_status_t;
    pub(crate) fn AImage_getWidth(image: *const AImage, width: *mut i32) -> media_status_t;
    pub(crate) fn AImage_getHeight(image: *const AImage, height: *mut i32) -> media_status_t;
    pub(crate) fn AImage_getPlaneRowStride(
        image: *const AImage,
        plane: c_int,
        row_stride: *mut i32,
    ) -> media_status_t;
    pub(crate) fn AImage_getPlanePixelStride(
        image: *const AImage,
        plane: c_int,
        pixel_stride: *mut i32,
    ) -> media_status_t;
    pub(crate) fn AImage_getPlaneData(
        image: *const AImage,
        plane: c_int,
        data: *mut *mut u8,
        length: *mut c_int,
    ) -> media_status_t;

    pub(crate) fn AMediaCodec_createEncoderByType(mime: *const c_char) -> *mut AMediaCodec;
    pub(crate) fn AMediaCodec_createCodecByName(name: *const c_char) -> *mut AMediaCodec;
    pub(crate) fn AMediaCodec_delete(codec: *mut AMediaCodec) -> media_status_t;
    pub(crate) fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        surface: *mut ANativeWindow,
        crypto: *mut AMediaCrypto,
        flags: u32,
    ) -> media_status_t;
    pub(crate) fn AMediaCodec_start(codec: *mut AMediaCodec) -> media_status_t;
    pub(crate) fn AMediaCodec_stop(codec: *mut AMediaCodec) -> media_status_t;
    pub(crate) fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64)
        -> isize;
    pub(crate) fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        size: *mut usize,
    ) -> *mut u8;
    pub(crate) fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        offset: c_long,
        size: usize,
        time_us: u64,
        flags: u32,
    ) -> media_status_t;
    pub(crate) fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut AMediaCodecBufferInfo,
        timeout_us: i64,
    ) -> isize;
    pub(crate) fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        size: *mut usize,
    ) -> *mut u8;
    pub(crate) fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        render: bool,
    ) -> media_status_t;

    pub(crate) fn AMediaFormat_new() -> *mut AMediaFormat;
    pub(crate) fn AMediaFormat_delete(format: *mut AMediaFormat) -> media_status_t;
    pub(crate) fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    pub(crate) fn AMediaFormat_setString(
        format: *mut AMediaFormat,
        name: *const c_char,
        value: *const c_char,
    );
    pub(crate) fn AMediaFormat_getInt32(
        format: *mut AMediaFormat,
        name: *const c_char,
        out: *mut i32,
    ) -> bool;
}

extern "C" {
    /// Bionic's system properties (`<sys/system_properties.h>`).
    pub(crate) fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
}

#[link(name = "dl")]
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `PROP_VALUE_MAX`.
pub(crate) const PROP_VALUE_MAX: usize = 92;

/// `AMediaCodec_setParameters` (API 26).
pub(crate) type SetParameters =
    unsafe extern "C" fn(*mut AMediaCodec, *const AMediaFormat) -> media_status_t;
/// `AMediaCodec_getInputFormat` (API 28).
pub(crate) type GetInputFormat = unsafe extern "C" fn(*mut AMediaCodec) -> *mut AMediaFormat;
/// `AMediaCodec_getName` (API 28): the component's name, released with
/// `AMediaCodec_releaseName`.
pub(crate) type GetName =
    unsafe extern "C" fn(*mut AMediaCodec, *mut *mut c_char) -> media_status_t;
/// `AMediaCodec_releaseName` (API 28).
pub(crate) type ReleaseName = unsafe extern "C" fn(*mut AMediaCodec, *mut c_char);

/// `ANativeWindow_toSurface` (API 26): a Java `Surface` (a local reference)
/// for a native window.
pub(crate) type ToSurface = unsafe extern "C" fn(*mut c_void, *mut ANativeWindow) -> *mut c_void;

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AHardwareBuffer_Desc {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) layers: u32,
    pub(crate) format: u32,
    pub(crate) usage: u64,
    /// Row length in pixels.
    pub(crate) stride: u32,
    pub(crate) rfu0: u32,
    pub(crate) rfu1: u64,
}

/// `AHardwareBuffer_Plane`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct AHardwareBuffer_Plane {
    pub(crate) data: *mut c_void,
    pub(crate) pixelStride: u32,
    pub(crate) rowStride: u32,
}

/// `AHardwareBuffer_Planes`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct AHardwareBuffer_Planes {
    pub(crate) planeCount: u32,
    pub(crate) planes: [AHardwareBuffer_Plane; 4],
}

/// `AHARDWAREBUFFER_FORMAT_*`.
pub(crate) const AHB_FORMAT_R8G8B8A8_UNORM: u32 = 1;
pub(crate) const AHB_FORMAT_R8G8B8X8_UNORM: u32 = 2;
pub(crate) const AHB_FORMAT_Y8CB8CR8_420: u32 = 0x23;
/// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN`.
pub(crate) const AHB_USAGE_CPU_READ_OFTEN: u64 = 3;

pub(crate) type AhbAcquire = unsafe extern "C" fn(*mut AHardwareBuffer);
pub(crate) type AhbRelease = unsafe extern "C" fn(*mut AHardwareBuffer);
pub(crate) type AhbDescribe =
    unsafe extern "C" fn(*const AHardwareBuffer, *mut AHardwareBuffer_Desc);
pub(crate) type AhbLock =
    unsafe extern "C" fn(*mut AHardwareBuffer, u64, i32, *const c_void, *mut *mut c_void) -> c_int;
pub(crate) type AhbLockPlanes = unsafe extern "C" fn(
    *mut AHardwareBuffer,
    u64,
    i32,
    *const c_void,
    *mut AHardwareBuffer_Planes,
) -> c_int;
pub(crate) type AhbUnlock = unsafe extern "C" fn(*mut AHardwareBuffer, *mut i32) -> c_int;

/// `libnativewindow`'s AHardwareBuffer calls (API 26; `lock_planes` API 29).
#[derive(Clone, Copy)]
pub(crate) struct Ahb {
    pub(crate) acquire: AhbAcquire,
    pub(crate) release: AhbRelease,
    pub(crate) describe: AhbDescribe,
    pub(crate) lock: AhbLock,
    pub(crate) lock_planes: Option<AhbLockPlanes>,
    pub(crate) unlock: AhbUnlock,
}

/// The functions newer than API 24, where the device has them.
pub(crate) struct Late {
    pub(crate) set_parameters: Option<SetParameters>,
    pub(crate) get_input_format: Option<GetInputFormat>,
    /// `AMediaCodec_getName` and `AMediaCodec_releaseName`, together.
    pub(crate) name: Option<(GetName, ReleaseName)>,
    pub(crate) to_surface: Option<ToSurface>,
    pub(crate) ahb: Option<Ahb>,
}

/// Look up the newer functions once.
pub(crate) fn late() -> &'static Late {
    static LATE: OnceLock<Late> = OnceLock::new();
    LATE.get_or_init(|| {
        // bionic: RTLD_NOW is 2 on 64-bit targets and 0 on 32-bit ones.
        let now = if cfg!(target_pointer_width = "64") {
            2
        } else {
            0
        };
        // SAFETY: a constant, NUL-terminated name. libmediandk is already
        // loaded (this library links it), so this only returns its handle.
        let lib = unsafe { dlopen(c"libmediandk.so".as_ptr(), now) };
        let find = |name: &CStr| -> *mut c_void {
            if lib.is_null() {
                return std::ptr::null_mut();
            }
            // SAFETY: `lib` is a live handle from dlopen and `name` is
            // NUL-terminated.
            unsafe { dlsym(lib, name.as_ptr()) }
        };
        let set = find(c"AMediaCodec_setParameters");
        let get = find(c"AMediaCodec_getInputFormat");
        let get_name = find(c"AMediaCodec_getName");
        let release_name = find(c"AMediaCodec_releaseName");
        // SAFETY: a constant, NUL-terminated name; libnativewindow is a
        // system library (libmediandk loads it).
        let nw = unsafe { dlopen(c"libnativewindow.so".as_ptr(), now) };
        let find_nw = |name: &CStr| -> *mut c_void {
            if nw.is_null() {
                return std::ptr::null_mut();
            }
            // SAFETY: `nw` is a live handle from dlopen and `name` is
            // NUL-terminated.
            unsafe { dlsym(nw, name.as_ptr()) }
        };
        let to_surface = find_nw(c"ANativeWindow_toSurface");
        let lock_planes = find_nw(c"AHardwareBuffer_lockPlanes");
        let ahb = [
            c"AHardwareBuffer_acquire",
            c"AHardwareBuffer_release",
            c"AHardwareBuffer_describe",
            c"AHardwareBuffer_lock",
            c"AHardwareBuffer_unlock",
        ]
        .map(find_nw);
        // SAFETY (each transmute below): a non-null result of dlsym for that
        // name is the NDK function whose C signature the alias spells out.
        let ahb = (!ahb.iter().any(|p| p.is_null())).then(|| unsafe {
            Ahb {
                acquire: std::mem::transmute::<*mut c_void, AhbAcquire>(ahb[0]),
                release: std::mem::transmute::<*mut c_void, AhbRelease>(ahb[1]),
                describe: std::mem::transmute::<*mut c_void, AhbDescribe>(ahb[2]),
                lock: std::mem::transmute::<*mut c_void, AhbLock>(ahb[3]),
                unlock: std::mem::transmute::<*mut c_void, AhbUnlock>(ahb[4]),
                lock_planes: (!lock_planes.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, AhbLockPlanes>(lock_planes)),
            }
        });
        Late {
            // SAFETY: as above.
            to_surface: (!to_surface.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, ToSurface>(to_surface) }),
            ahb,
            // SAFETY: non-null results of dlsym for these names are the
            // NDK functions, whose C signatures the aliases spell out.
            set_parameters: (!set.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, SetParameters>(set) }),
            // SAFETY: as above.
            get_input_format: (!get.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, GetInputFormat>(get) }),
            // SAFETY: as above.
            name: (!get_name.is_null() && !release_name.is_null()).then(|| unsafe {
                (
                    std::mem::transmute::<*mut c_void, GetName>(get_name),
                    std::mem::transmute::<*mut c_void, ReleaseName>(release_name),
                )
            }),
        }
    })
}

//! The [`Platform`] on a device: Camera2 through the NDK, the CAMERA
//! permission through JNI, and the MediaCodec H.264 encoder. Every `unsafe`
//! block of the Android backend is in this module and its children.

mod camera;
mod encoder;
mod ffi;
mod jni;

use super::meta::{self, camera_result, CamDesc, RawCharacteristics};
use super::{Capture, ImageSink, Platform};
use crate::error::VideoError;
use crate::kit::EncoderCore;
use crate::types::*;
use ffi::*;
use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

/// The NDK side of the backend.
pub(crate) struct Ndk {
    /// One `ACameraManager` for the backend's life.
    manager: *mut ACameraManager,
    /// The host's `JavaVM*` and `Context` global reference.
    jni: Option<(usize, usize)>,
}

// SAFETY: an `ACameraManager` may be used from any thread (the NDK guards
// it with its own lock), and the JNI handles are process-wide.
unsafe impl Send for Ndk {}
// SAFETY: as above; `Ndk` holds no Rust state that `&self` mutates.
unsafe impl Sync for Ndk {}

impl Ndk {
    pub(crate) fn new(jni: Option<(usize, usize)>) -> Result<Ndk, VideoError> {
        // SAFETY: no arguments; null on failure, checked below.
        let manager = unsafe { ACameraManager_create() };
        if manager.is_null() {
            return Err(VideoError::os(0, "ACameraManager_create returned null"));
        }
        Ok(Ndk { manager, jni })
    }
}

impl Drop for Ndk {
    fn drop(&mut self) {
        // SAFETY: created in `new`, deleted once here. Open cameras hold
        // their own device handles, which outlive the manager safely.
        unsafe { ACameraManager_delete(self.manager) };
    }
}

/// One metadata entry's data as a slice of `T`, if the entry exists and has
/// type `want`.
///
/// # Safety
///
/// `meta` must be live, and `T` must be the Rust type of `want`.
unsafe fn entry<'a, T>(meta: *const ACameraMetadata, tag: u32, want: u8) -> Option<&'a [T]> {
    let mut e = ACameraMetadata_const_entry {
        tag: 0,
        type_: 0,
        count: 0,
        data: std::ptr::null(),
    };
    // SAFETY: `meta` is live (the caller's promise) and `e` is a valid out
    // pointer.
    let status = unsafe { ACameraMetadata_getConstEntry(meta, tag, &mut e) };
    if status != 0 || e.type_ != want || e.data.is_null() || e.count == 0 {
        return None;
    }
    // SAFETY: the NDK says `data` points at `count` values of the entry's
    // type, owned by `meta`, which outlives the slice (the caller frees
    // `meta` only after it is done with the slices).
    Some(unsafe { std::slice::from_raw_parts(e.data as *const T, e.count as usize) })
}

/// Characteristics, freed on drop.
struct Characteristics(*mut ACameraMetadata);

impl Drop for Characteristics {
    fn drop(&mut self) {
        // SAFETY: from ACameraManager_getCameraCharacteristics, freed once.
        unsafe { ACameraMetadata_free(self.0) };
    }
}

impl Ndk {
    fn describe(&self, id: &CStr) -> Result<Option<CamDesc>, VideoError> {
        let mut meta = std::ptr::null_mut();
        // SAFETY: the manager is live, `id` is NUL-terminated and `meta` a
        // valid out pointer.
        let status = unsafe {
            ACameraManager_getCameraCharacteristics(self.manager, id.as_ptr(), &mut meta)
        };
        camera_result(status, "ACameraManager_getCameraCharacteristics")?;
        if meta.is_null() {
            return Ok(None);
        }
        let meta = Characteristics(meta);
        let m = meta.0 as *const ACameraMetadata;
        // SAFETY: `m` lives until `meta` drops at the end of this function,
        // after the slices' last use; each type matches its tag's NDK type.
        let raw = unsafe {
            RawCharacteristics {
                facing: entry::<u8>(m, ACAMERA_LENS_FACING, ACAMERA_TYPE_BYTE)
                    .and_then(|v| v.first().copied()),
                orientation: entry::<i32>(m, ACAMERA_SENSOR_ORIENTATION, ACAMERA_TYPE_INT32)
                    .and_then(|v| v.first().copied()),
                stream_configs: entry::<i32>(
                    m,
                    ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS,
                    ACAMERA_TYPE_INT32,
                )
                .unwrap_or_default(),
                min_durations: entry::<i64>(
                    m,
                    ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS,
                    ACAMERA_TYPE_INT64,
                )
                .unwrap_or_default(),
                fps_ranges: entry::<i32>(
                    m,
                    ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES,
                    ACAMERA_TYPE_INT32,
                )
                .unwrap_or_default(),
            }
        };
        let name = id.to_string_lossy();
        Ok(meta::describe(&name, &raw))
    }

    fn ids(&self) -> Result<Vec<CString>, VideoError> {
        let mut list: *mut ACameraIdList = std::ptr::null_mut();
        // SAFETY: the manager is live and `list` a valid out pointer.
        let status = unsafe { ACameraManager_getCameraIdList(self.manager, &mut list) };
        camera_result(status, "ACameraManager_getCameraIdList")?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // SAFETY: the NDK filled `list` with `numCameras` NUL-terminated
        // ids; they are copied before the list is freed.
        unsafe {
            let l = &*list;
            for i in 0..l.numCameras.max(0) as usize {
                let p: *const c_char = *l.cameraIds.add(i);
                if !p.is_null() {
                    out.push(CStr::from_ptr(p).to_owned());
                }
            }
            ACameraManager_deleteCameraIdList(list);
        }
        Ok(out)
    }
}

impl Platform for Ndk {
    fn os_name(&self) -> String {
        os_name()
    }

    fn cameras(&self) -> Result<Vec<CamDesc>, VideoError> {
        let mut out = Vec::new();
        for id in self.ids()? {
            // One camera the service can't describe doesn't hide the rest.
            if let Ok(Some(d)) = self.describe(&id) {
                out.push(d);
            }
        }
        meta::mark_default(&mut out);
        Ok(out)
    }

    fn camera_permission(&self) -> Permission {
        match self.jni {
            Some((vm, ctx)) => match jni::camera_granted(vm, ctx) {
                Some(true) => Permission::Granted,
                Some(false) => Permission::Denied,
                None => Permission::NotDetermined,
            },
            None => Permission::NotDetermined,
        }
    }

    fn open_camera(
        &self,
        cam: &CamDesc,
        mode: CaptureMode,
        fps_range: Option<(u32, u32)>,
        sink: Arc<dyn ImageSink>,
    ) -> Result<Box<dyn Capture>, VideoError> {
        let id = CString::new(cam.info.id.as_str())
            .map_err(|_| VideoError::Invalid(format!("camera id {:?}", cam.info.id)))?;
        Ok(Box::new(camera::open(
            self.manager,
            &id,
            mode,
            fps_range,
            sink,
        )?))
    }

    fn h264_encode_capability(&self) -> Option<CodecCapability> {
        Some(encoder::capability())
    }

    fn h264_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn EncoderCore>, VideoError> {
        Ok(Box::new(encoder::McEncoder::open(cfg)?))
    }
}

/// A system property, or empty.
fn property(name: &CStr) -> String {
    let mut buf = [0 as c_char; PROP_VALUE_MAX];
    // SAFETY: `name` is NUL-terminated and `buf` is PROP_VALUE_MAX bytes,
    // the most the call writes (with its NUL).
    let n = unsafe { __system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
    if n <= 0 {
        return String::new();
    }
    // SAFETY: the call wrote a NUL-terminated string into `buf`.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// "Android 14 (API 34)", for logs.
pub(crate) fn os_name() -> String {
    let release = property(c"ro.build.version.release");
    let sdk = property(c"ro.build.version.sdk");
    match (release.is_empty(), sdk.is_empty()) {
        (false, false) => format!("Android {release} (API {sdk})"),
        (false, true) => format!("Android {release}"),
        _ => "Android".into(),
    }
}

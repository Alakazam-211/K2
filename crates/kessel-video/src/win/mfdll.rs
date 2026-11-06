//! Media Foundation's free functions, loaded at run time from the system
//! folder instead of imported.
//!
//! The N and KN editions of Windows have no Media Foundation until the
//! person installs the Media Feature Pack: `mfplat.dll`, `mf.dll` and
//! `mfreadwrite.dll` aren't there. An import of any of them would stop the
//! whole app from starting. Loaded here instead, a missing DLL only means no
//! cameras and no H.264: [`mfplat`] answers `Unsupported`, and VP8, screens
//! and the rest still work. Interface methods are vtable calls and need no
//! imports.

use crate::error::{unsupported, VideoError};
use std::ffi::c_void;
use std::sync::OnceLock;
use windows::core::{Interface, GUID, HRESULT, PCSTR, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaBuffer, IMFMediaSource, IMFMediaType, IMFSample,
    IMFSourceReader, MFT_REGISTER_TYPE_INFO,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

/// Why H.264 and cameras are missing when the DLLs are.
const NO_MF: &str = "Media Foundation isn't installed (an N or KN edition of Windows \
                     without the Media Feature Pack)";

type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateAttributes = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
type CreateOut = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateMemoryBuffer = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;
type TEnumEx = unsafe extern "system" fn(
    GUID,
    u32,
    *const MFT_REGISTER_TYPE_INFO,
    *const MFT_REGISTER_TYPE_INFO,
    *mut *mut *mut c_void,
    *mut u32,
) -> HRESULT;
type EnumDeviceSources =
    unsafe extern "system" fn(*mut c_void, *mut *mut *mut c_void, *mut u32) -> HRESULT;
type CreateDeviceSource = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
type CreateReader =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;

/// `mfplat.dll`: the platform (startup, attributes, media types, samples,
/// buffers and MFT enumeration).
pub(crate) struct MfPlat {
    startup: Startup,
    create_attributes: CreateAttributes,
    create_media_type: CreateOut,
    create_sample: CreateOut,
    create_memory_buffer: CreateMemoryBuffer,
    tenum_ex: TEnumEx,
}

/// `mf.dll` and `mfreadwrite.dll`: capture devices and the source reader.
pub(crate) struct MfCapture {
    enum_device_sources: EnumDeviceSources,
    create_device_source: CreateDeviceSource,
    create_reader: CreateReader,
}

/// A system DLL, by file name, from System32 only.
pub(super) fn load(dll: &str) -> Option<windows::Win32::Foundation::HMODULE> {
    let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated name; System32 only, so nothing planted next
    // to the app can stand in. The module stays loaded for the process.
    unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()
}

/// One export as a function pointer of type `F`.
///
/// # Safety
/// `F` must be the export's real signature (an `extern "system" fn`).
unsafe fn sym<F: Copy>(m: windows::Win32::Foundation::HMODULE, name: &std::ffi::CStr) -> Option<F> {
    // SAFETY: a loaded module and a NUL-terminated name.
    let p = unsafe { GetProcAddress(m, PCSTR(name.as_ptr() as *const u8)) }?;
    debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of_val(&p));
    // SAFETY: the caller names the export's signature; both are function
    // pointers of the same size.
    Some(unsafe { std::mem::transmute_copy::<_, F>(&p) })
}

/// Media Foundation's platform DLL, or `Unsupported` where it isn't
/// installed.
pub(crate) fn mfplat() -> Result<&'static MfPlat, VideoError> {
    static PLAT: OnceLock<Option<MfPlat>> = OnceLock::new();
    PLAT.get_or_init(|| {
        let m = load("mfplat.dll")?;
        // SAFETY: each type is the export's documented signature.
        unsafe {
            Some(MfPlat {
                startup: sym(m, c"MFStartup")?,
                create_attributes: sym(m, c"MFCreateAttributes")?,
                create_media_type: sym(m, c"MFCreateMediaType")?,
                create_sample: sym(m, c"MFCreateSample")?,
                create_memory_buffer: sym(m, c"MFCreateMemoryBuffer")?,
                tenum_ex: sym(m, c"MFTEnumEx")?,
            })
        }
    })
    .as_ref()
    .ok_or_else(|| unsupported(NO_MF))
}

/// Media Foundation's capture DLLs, or `Unsupported`.
pub(crate) fn mfcapture() -> Result<&'static MfCapture, VideoError> {
    static CAP: OnceLock<Option<MfCapture>> = OnceLock::new();
    CAP.get_or_init(|| {
        let mf = load("mf.dll")?;
        let rw = load("mfreadwrite.dll")?;
        // SAFETY: each type is the export's documented signature.
        unsafe {
            Some(MfCapture {
                enum_device_sources: sym(mf, c"MFEnumDeviceSources")?,
                create_device_source: sym(mf, c"MFCreateDeviceSource")?,
                create_reader: sym(rw, c"MFCreateSourceReaderFromMediaSource")?,
            })
        }
    })
    .as_ref()
    .ok_or_else(|| unsupported(NO_MF))
}

/// An interface from an out pointer a call filled.
///
/// # Safety
/// `p` is null or an owned reference to a `T`.
unsafe fn take<T: Interface>(hr: HRESULT, p: *mut c_void, what: &str) -> windows::core::Result<T> {
    hr.ok()?;
    if p.is_null() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_POINTER,
            format!("{what} gave nothing"),
        ));
    }
    // SAFETY: the caller's contract: an owned reference to a T.
    Ok(unsafe { T::from_raw(p) })
}

impl MfPlat {
    /// `MFStartup`.
    pub(crate) fn startup(&self, version: u32, flags: u32) -> windows::core::Result<()> {
        // SAFETY: the export with its signature.
        unsafe { (self.startup)(version, flags) }.ok()
    }

    /// `MFCreateAttributes`.
    pub(crate) fn create_attributes(&self, n: u32) -> windows::core::Result<IMFAttributes> {
        let mut p = std::ptr::null_mut();
        // SAFETY: an out pointer for a new attribute store we then own.
        unsafe { take((self.create_attributes)(&mut p, n), p, "MFCreateAttributes") }
    }

    /// `MFCreateMediaType`.
    pub(crate) fn create_media_type(&self) -> windows::core::Result<IMFMediaType> {
        let mut p = std::ptr::null_mut();
        // SAFETY: an out pointer for a new media type we then own.
        unsafe { take((self.create_media_type)(&mut p), p, "MFCreateMediaType") }
    }

    /// `MFCreateSample`.
    pub(crate) fn create_sample(&self) -> windows::core::Result<IMFSample> {
        let mut p = std::ptr::null_mut();
        // SAFETY: an out pointer for a new sample we then own.
        unsafe { take((self.create_sample)(&mut p), p, "MFCreateSample") }
    }

    /// `MFCreateMemoryBuffer`.
    pub(crate) fn create_memory_buffer(&self, len: u32) -> windows::core::Result<IMFMediaBuffer> {
        let mut p = std::ptr::null_mut();
        // SAFETY: an out pointer for a new buffer we then own.
        unsafe {
            take(
                (self.create_memory_buffer)(len, &mut p),
                p,
                "MFCreateMemoryBuffer",
            )
        }
    }

    /// `MFTEnumEx`: the activation objects of the matching MFTs, in the
    /// order Media Foundation gives them.
    pub(crate) fn tenum_ex(
        &self,
        category: GUID,
        flags: u32,
        input: Option<&MFT_REGISTER_TYPE_INFO>,
        output: Option<&MFT_REGISTER_TYPE_INFO>,
    ) -> windows::core::Result<Vec<IMFActivate>> {
        let mut list: *mut *mut c_void = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: type filters that outlive the call; Media Foundation
        // allocates the array (CoTaskMemAlloc) and gives us one reference
        // per entry, which we take over before freeing the array.
        unsafe {
            (self.tenum_ex)(
                category,
                flags,
                input.map_or(std::ptr::null(), |t| t as *const _),
                output.map_or(std::ptr::null(), |t| t as *const _),
                &mut list,
                &mut count,
            )
            .ok()?;
            Ok(take_array(list, count))
        }
    }
}

/// Take over an array of interface references that Media Foundation
/// allocated, and free the array.
///
/// # Safety
/// `list` is null or a CoTaskMemAlloc'd array of `count` owned references
/// (each null or a `T`).
unsafe fn take_array<T: Interface>(list: *mut *mut c_void, count: u32) -> Vec<T> {
    if list.is_null() {
        return Vec::new();
    }
    let out = (0..count as usize)
        .filter_map(|i| {
            // SAFETY: inside the array; each entry is owned by us now.
            let p = unsafe { *list.add(i) };
            // SAFETY: null or an owned reference to a T.
            (!p.is_null()).then(|| unsafe { T::from_raw(p) })
        })
        .collect();
    // SAFETY: the array itself came from CoTaskMemAlloc.
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(list as *const c_void)) };
    out
}

impl MfCapture {
    /// `MFEnumDeviceSources`.
    pub(crate) fn enum_device_sources(
        &self,
        attrs: &IMFAttributes,
    ) -> windows::core::Result<Vec<IMFActivate>> {
        let mut list: *mut *mut c_void = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: a live attribute store; the array as in `tenum_ex`.
        unsafe {
            (self.enum_device_sources)(attrs.as_raw(), &mut list, &mut count).ok()?;
            Ok(take_array(list, count))
        }
    }

    /// `MFCreateDeviceSource`.
    pub(crate) fn create_device_source(
        &self,
        attrs: &IMFAttributes,
    ) -> windows::core::Result<IMFMediaSource> {
        let mut p = std::ptr::null_mut();
        // SAFETY: a live attribute store; an out pointer for a source we own.
        unsafe {
            take(
                (self.create_device_source)(attrs.as_raw(), &mut p),
                p,
                "MFCreateDeviceSource",
            )
        }
    }

    /// `MFCreateSourceReaderFromMediaSource`.
    pub(crate) fn create_reader(
        &self,
        source: &IMFMediaSource,
        attrs: &IMFAttributes,
    ) -> windows::core::Result<IMFSourceReader> {
        let mut p = std::ptr::null_mut();
        // SAFETY: a live source and attribute store; an out pointer for a
        // reader we own.
        unsafe {
            take(
                (self.create_reader)(source.as_raw(), attrs.as_raw(), &mut p),
                p,
                "MFCreateSourceReaderFromMediaSource",
            )
        }
    }
}

//! Cameras through Media Foundation: the list, and capture through a
//! source reader on a thread of its own that delivers NV12 frames.

use super::map_error;
use crate::api::{FrameSource, Wake};
use crate::error::VideoError;
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

/// An open was refused by the OS's camera privacy setting.
static REFUSED: AtomicBool = AtomicBool::new(false);

/// COM on this thread (multithreaded; an STA thread keeps its apartment,
/// which Media Foundation also works in) and Media Foundation once.
fn init() -> Result<(), VideoError> {
    // SAFETY: COM initialisation for the calling thread; RPC_E_CHANGED_MODE
    // (already an STA) is fine for Media Foundation.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    static STARTED: OnceLock<Result<(), VideoError>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            // SAFETY: MFStartup with the SDK version; never shut down (the
            // process keeps Media Foundation for its lifetime).
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| map_error(&e, "MFStartup"))
        })
        .clone()
}

pub(crate) fn permission() -> Permission {
    if REFUSED.load(Ordering::Acquire) {
        Permission::Denied
    } else {
        Permission::Granted
    }
}

fn attributes(n: u32) -> Result<IMFAttributes, VideoError> {
    let mut a: Option<IMFAttributes> = None;
    // SAFETY: an out pointer for the new attribute store.
    unsafe { MFCreateAttributes(&mut a, n) }.map_err(|e| map_error(&e, "MFCreateAttributes"))?;
    a.ok_or_else(|| VideoError::os(0, "MFCreateAttributes gave nothing"))
}

/// A string attribute.
fn string_attr(a: &IMFAttributes, key: &windows::core::GUID) -> Option<String> {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: Media Foundation allocates the string; we free it with
    // CoTaskMemFree after copying it.
    unsafe {
        a.GetAllocatedString(key, &mut p, &mut len).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s
    }
}

pub(crate) fn cameras() -> Vec<CameraInfo> {
    if init().is_err() {
        return Vec::new();
    }
    let Ok(attrs) = attributes(1) else { return Vec::new() };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: a video-capture source-type attribute; MF allocates the array
    // of activation objects, which we take over one by one and free.
    let devices: Vec<IMFActivate> = unsafe {
        if attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).is_err() {
            return Vec::new();
        }
        if MFEnumDeviceSources(&attrs, &mut list, &mut count).is_err() || list.is_null() {
            return Vec::new();
        }
        let out = (0..count as usize).filter_map(|i| std::ptr::read(list.add(i))).collect();
        CoTaskMemFree(Some(list as *const core::ffi::c_void));
        out
    };
    devices
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let id = string_attr(d, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?;
            let name = string_attr(d, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_else(|| "Camera".into());
            let lower = name.to_lowercase();
            let facing = if lower.contains("integrated") || lower.contains("front") || lower.contains("internal") {
                Facing::Front
            } else {
                Facing::External
            };
            Some(CameraInfo::new(id, name, facing, i == 0))
        })
        .collect()
}

// ───────────────────────────── capture ─────────────────────────────

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// The device's modes, each with the native media type behind it.
fn native_modes(reader: &IMFSourceReader) -> Vec<(CaptureMode, IMFMediaType)> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut out: Vec<(CaptureMode, IMFMediaType)> = Vec::new();
    for i in 0..512u32 {
        // SAFETY: a live reader; MF_E_NO_MORE_TYPES ends the list.
        let Ok(t) = (unsafe { reader.GetNativeMediaType(stream, i) }) else { break };
        // SAFETY: attribute reads on a live media type.
        let (size, rate) = unsafe { (t.GetUINT64(&MF_MT_FRAME_SIZE), t.GetUINT64(&MF_MT_FRAME_RATE)) };
        let (Ok(size), Ok(rate)) = (size, rate) else { continue };
        let (w, h) = unpack(size);
        let (num, den) = unpack(rate);
        if w == 0 || h == 0 || den == 0 {
            continue;
        }
        let fps = (num as f64 / den as f64).round() as u32;
        let mode = CaptureMode::new(w, h, fps.max(1), PixelFormat::Nv12);
        match out.iter_mut().find(|(m, _)| (m.width, m.height) == (w, h)) {
            Some(slot) if slot.0.max_fps < mode.max_fps => *slot = (mode, t),
            Some(_) => {}
            None => out.push((mode, t)),
        }
    }
    out
}

/// Open the device and set it to `c`; gives the reader and the delivered
/// size and rate.
fn open_reader(id: Option<&str>, c: &CaptureConstraints) -> Result<(IMFSourceReader, u32, u32, u32), VideoError> {
    init()?;
    let id = match id {
        Some(id) => id.to_string(),
        None => cameras().into_iter().find(|c| c.is_default).map(|c| c.id).ok_or(VideoError::NotFound)?,
    };
    let attrs = attributes(2)?;
    let link: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: attribute writes on a live store; MFCreateDeviceSource reads
    // them and opens the device.
    let source = unsafe {
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).map_err(|e| map_error(&e, "SetGUID"))?;
        attrs.SetString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, PCWSTR(link.as_ptr())).map_err(|e| map_error(&e, "SetString"))?;
        MFCreateDeviceSource(&attrs)
    }
    .map_err(|e| match map_error(&e, "MFCreateDeviceSource") {
        VideoError::Os { .. } => VideoError::NotFound,
        other => other,
    })?;
    let rattrs = attributes(2)?;
    // SAFETY: attribute writes, then the reader over the live source.
    let reader = unsafe {
        rattrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| map_error(&e, "SetUINT32"))?;
        let _ = rattrs.SetUINT32(&MF_LOW_LATENCY, 1);
        MFCreateSourceReaderFromMediaSource(&source, &rattrs)
    }
    .map_err(|e| map_error(&e, "MFCreateSourceReaderFromMediaSource"))?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let modes = native_modes(&reader);
    let list: Vec<CaptureMode> = modes.iter().map(|(m, _)| *m).collect();
    let chosen = nearest_mode(&list, c);
    if let Some(m) = chosen {
        if let Some((_, native)) = modes.iter().find(|(x, _)| *x == m) {
            // SAFETY: a native type the device offered.
            unsafe { reader.SetCurrentMediaType(stream, None, native) }.map_err(|e| map_error(&e, "SetCurrentMediaType (native)"))?;
        }
    }
    let fps = match (chosen, c.fps) {
        (Some(m), Some(f)) => f.min(m.max_fps).max(1),
        (Some(m), None) => m.max_fps.min(30),
        (None, f) => f.unwrap_or(30),
    };
    // SAFETY: the current type gives the device's size; then an NV12
    // output of that size, converted by the reader's video processing.
    unsafe {
        let current = reader.GetCurrentMediaType(stream).map_err(|e| map_error(&e, "GetCurrentMediaType"))?;
        let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| map_error(&e, "MF_MT_FRAME_SIZE"))?;
        let out = MFCreateMediaType().map_err(|e| map_error(&e, "MFCreateMediaType"))?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| map_error(&e, "SetGUID"))?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| map_error(&e, "SetGUID"))?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| map_error(&e, "SetUINT64"))?;
        if let Ok(rate) = current.GetUINT64(&MF_MT_FRAME_RATE) {
            let _ = out.SetUINT64(&MF_MT_FRAME_RATE, rate);
        } else {
            let _ = out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1));
        }
        reader.SetCurrentMediaType(stream, None, &out).map_err(|e| map_error(&e, "SetCurrentMediaType (NV12)"))?;
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        reader.SetStreamSelection(stream, true).map_err(|e| map_error(&e, "SetStreamSelection"))?;
        let (w, h) = unpack(size);
        Ok((reader, w, h, fps))
    }
}

/// Copy one NV12 sample into planes.
fn copy_sample(sample: &IMFSample, width: u32, height: u32) -> Result<Planes, VideoError> {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::with_capacity(PixelFormat::Nv12.frame_len(width, height));
    // SAFETY: the buffer is locked while we read rows inside the locked
    // region (pitch x (rows of Y + rows of UV)), then unlocked.
    unsafe {
        let buf = sample.ConvertToContiguousBuffer().map_err(|e| map_error(&e, "ConvertToContiguousBuffer"))?;
        if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            b2.Lock2D(&mut p, &mut pitch).map_err(|e| map_error(&e, "Lock2D"))?;
            let ok = !p.is_null() && pitch as usize >= w;
            if ok {
                let pitch = pitch as usize;
                for y in 0..h {
                    out.extend_from_slice(std::slice::from_raw_parts(p.add(y * pitch), w));
                }
                let uv = p.add(pitch * h);
                for y in 0..ch {
                    out.extend_from_slice(std::slice::from_raw_parts(uv.add(y * pitch), cw * 2));
                }
            }
            let _ = b2.Unlock2D();
            if !ok {
                return Err(VideoError::os(0, format!("an NV12 buffer with pitch {pitch} for width {w}")));
            }
        } else {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len as *mut u32)).map_err(|e| map_error(&e, "Lock"))?;
            let need = PixelFormat::Nv12.frame_len(width, height);
            let ok = !p.is_null() && len as usize >= need;
            if ok {
                out.extend_from_slice(std::slice::from_raw_parts(p, need));
            }
            let _ = buf.Unlock();
            if !ok {
                return Err(VideoError::os(0, format!("an NV12 buffer of {len} bytes for {width}x{height}")));
            }
        }
    }
    Planes::tight(PixelFormat::Nv12, width, height, out)
}

fn capture_loop(reader: IMFSourceReader, width: u32, height: u32, shared: Arc<SourceShared>, stop: Arc<AtomicBool>) {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    while !stop.load(Ordering::Acquire) {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        // SAFETY: a synchronous read on our reader, with out pointers.
        let r = unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags as *mut u32), None, Some(&mut sample as *mut Option<IMFSample>)) };
        if stop.load(Ordering::Acquire) {
            return;
        }
        if let Err(e) = r {
            let reason = match map_error(&e, "ReadSample") {
                VideoError::NoPermission => EndReason::Revoked,
                _ => EndReason::DeviceLost,
            };
            shared.event(SourceEvent::Ended(reason));
            return;
        }
        if flags & (MF_SOURCE_READERF_ERROR.0 as u32 | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            shared.event(SourceEvent::Ended(EndReason::DeviceLost));
            return;
        }
        let Some(sample) = sample else { continue };
        if shared.idle() {
            continue;
        }
        match copy_sample(&sample, width, height).and_then(|p| VideoFrame::from_planes(p, width, height, now_us())) {
            Ok(f) => shared.deliver(f),
            Err(e) => shared.event(SourceEvent::Error(e)),
        }
    }
}

struct CameraControl {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SourceControl for CameraControl {}

impl Drop for CameraControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // ReadSample returns within a frame interval, then the thread drops
        // the reader, which closes the device.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn open(id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    let (tx, rx) = sync_channel::<Result<(u32, u32, u32), VideoError>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let shared_slot: Arc<OnceLock<Arc<SourceShared>>> = Arc::new(OnceLock::new());
    let (st, slot, id, c2) = (stop.clone(), shared_slot.clone(), id.map(str::to_string), c);
    // All Media Foundation work stays on this thread, in its own apartment.
    let thread = std::thread::Builder::new()
        .name("kessel-video-camera".into())
        .spawn(move || match open_reader(id.as_deref(), &c2) {
            Ok((reader, w, h, fps)) => {
                let _ = tx.send(Ok((w, h, fps)));
                // Wait for the opener to hand over the queues.
                let shared = loop {
                    if let Some(s) = slot.get() {
                        break s.clone();
                    }
                    if st.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                };
                capture_loop(reader, w, h, shared, st);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        })
        .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?;
    let control = CameraControl {
        stop,
        thread: Some(thread),
    };
    let opened = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| VideoError::os(0, "the camera didn't open in 10 s"))?;
    let (_, _, fps) = match opened {
        Ok(v) => v,
        Err(e) => {
            if e == VideoError::NoPermission {
                REFUSED.store(true, Ordering::Release);
            }
            return Err(e);
        }
    };
    REFUSED.store(false, Ordering::Release);
    let shared = SourceShared::new(c, fps, wake);
    let _ = shared_slot.set(shared.clone());
    Ok(Box::new(QueuedSource::new(shared, control)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_rates_pack_high_then_low() {
        assert_eq!(unpack(pack(1280, 720)), (1280, 720));
        assert_eq!(unpack((30u64 << 32) | 1), (30, 1));
    }
}

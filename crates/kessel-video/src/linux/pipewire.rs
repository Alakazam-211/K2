//! A PipeWire video stream through the system's libpipewire, loaded at run
//! time with dlopen (never linked at build time and never shipped: a system
//! without PipeWire gets `Unsupported`). The format parameters are SPA PODs
//! built and read here by hand.

use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{now_us, SourceShared};
use crate::types::SourceEvent;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::{Arc, Mutex, OnceLock};

// ───────────────────────────── SPA PODs ─────────────────────────────

const SPA_TYPE_ID: u32 = 3;
const SPA_TYPE_RECTANGLE: u32 = 10;
const SPA_TYPE_FRACTION: u32 = 11;
const SPA_TYPE_OBJECT: u32 = 15;
const SPA_TYPE_CHOICE: u32 = 19;
const SPA_TYPE_OBJECT_FORMAT: u32 = 0x40003;
const SPA_PARAM_ENUM_FORMAT: u32 = 3;
const SPA_PARAM_FORMAT: u32 = 4;
const SPA_CHOICE_RANGE: u32 = 1;
const SPA_CHOICE_ENUM: u32 = 3;
const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
const SPA_FORMAT_VIDEO_FORMAT: u32 = 0x20001;
const SPA_FORMAT_VIDEO_SIZE: u32 = 0x20003;
const SPA_FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
const SPA_MEDIA_TYPE_VIDEO: u32 = 2;
const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;

/// `enum spa_video_format` values we read.
pub(crate) const VIDEO_RGBX: u32 = 7;
pub(crate) const VIDEO_BGRX: u32 = 8;
pub(crate) const VIDEO_RGBA: u32 = 11;
pub(crate) const VIDEO_BGRA: u32 = 12;

fn pad8(v: &mut Vec<u8>) {
    while v.len() % 8 != 0 {
        v.push(0);
    }
}

fn pod(ty: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len() + 7);
    v.extend_from_slice(&(body.len() as u32).to_ne_bytes());
    v.extend_from_slice(&ty.to_ne_bytes());
    v.extend_from_slice(body);
    pad8(&mut v);
    v
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_ne_bytes()).collect()
}

/// A choice POD: `kind`, a child type of `child_size` bytes, and its values.
fn choice(kind: u32, child_type: u32, child_size: u32, values: &[u32]) -> Vec<u8> {
    let mut body = words(&[kind, 0, child_size, child_type]);
    body.extend(words(values));
    pod(SPA_TYPE_CHOICE, &body)
}

fn prop(key: u32, value: Vec<u8>) -> Vec<u8> {
    let mut v = words(&[key, 0]);
    v.extend(value);
    v
}

/// The EnumFormat we offer: raw video, 32-bit RGB layouts, any size up to
/// 8192, any rate up to `max_fps`.
pub(crate) fn enum_format(max_fps: u32, want: (u32, u32)) -> Vec<u8> {
    let mut body = words(&[SPA_TYPE_OBJECT_FORMAT, SPA_PARAM_ENUM_FORMAT]);
    body.extend(prop(
        SPA_FORMAT_MEDIA_TYPE,
        pod(SPA_TYPE_ID, &words(&[SPA_MEDIA_TYPE_VIDEO])),
    ));
    body.extend(prop(
        SPA_FORMAT_MEDIA_SUBTYPE,
        pod(SPA_TYPE_ID, &words(&[SPA_MEDIA_SUBTYPE_RAW])),
    ));
    // Enum choice: the default first, then the alternatives.
    body.extend(prop(
        SPA_FORMAT_VIDEO_FORMAT,
        choice(
            SPA_CHOICE_ENUM,
            SPA_TYPE_ID,
            4,
            &[VIDEO_BGRX, VIDEO_BGRX, VIDEO_BGRA, VIDEO_RGBX, VIDEO_RGBA],
        ),
    ));
    body.extend(prop(
        SPA_FORMAT_VIDEO_SIZE,
        choice(
            SPA_CHOICE_RANGE,
            SPA_TYPE_RECTANGLE,
            8,
            &[want.0.max(1), want.1.max(1), 1, 1, 8192, 8192],
        ),
    ));
    body.extend(prop(
        SPA_FORMAT_VIDEO_FRAMERATE,
        choice(
            SPA_CHOICE_RANGE,
            SPA_TYPE_FRACTION,
            8,
            &[max_fps.max(1), 1, 0, 1, max_fps.max(1), 1],
        ),
    ));
    pod(SPA_TYPE_OBJECT, &body)
}

/// The negotiated raw video format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawFormat {
    pub(crate) format: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
}

/// A property value's payload: the value itself, or a choice's first value.
fn value_payload(ty: u32, body: &[u8]) -> Option<(u32, &[u8])> {
    if ty == SPA_TYPE_CHOICE {
        let child_size = u32_at(body, 8)? as usize;
        let child_type = u32_at(body, 12)?;
        Some((child_type, body.get(16..16 + child_size)?))
    } else {
        Some((ty, body))
    }
}

/// Read a Format object (from param_changed).
pub(crate) fn parse_format(p: &[u8]) -> Option<RawFormat> {
    let size = u32_at(p, 0)? as usize;
    if u32_at(p, 4)? != SPA_TYPE_OBJECT {
        return None;
    }
    let body = p.get(8..8 + size)?;
    let mut at = 8; // object type and id
    let (mut format, mut width, mut height) = (None, None, None);
    while at + 16 <= body.len() {
        let key = u32_at(body, at)?;
        let vsize = u32_at(body, at + 8)? as usize;
        let vtype = u32_at(body, at + 12)?;
        let vbody = body.get(at + 16..at + 16 + vsize)?;
        if let Some((ty, v)) = value_payload(vtype, vbody) {
            match (key, ty) {
                (SPA_FORMAT_VIDEO_FORMAT, SPA_TYPE_ID) => format = u32_at(v, 0),
                (SPA_FORMAT_VIDEO_SIZE, SPA_TYPE_RECTANGLE) => {
                    width = u32_at(v, 0);
                    height = u32_at(v, 4);
                }
                _ => {}
            }
        }
        at += 16 + vsize;
        at += (8 - at % 8) % 8;
    }
    Some(RawFormat {
        format: format?,
        width: width?,
        height: height?,
    })
}

/// One mapped buffer's pixels as a frame (BGRx/BGRA as Bgra, RGBx/RGBA as
/// Rgba; the x byte is ignored downstream).
pub(crate) fn to_frame(
    data: &[u8],
    stride: usize,
    f: &RawFormat,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    let format = match f.format {
        VIDEO_BGRX | VIDEO_BGRA => PixelFormat::Bgra,
        VIDEO_RGBX | VIDEO_RGBA => PixelFormat::Rgba,
        other => return Err(unsupported(format!("PipeWire video format {other}"))),
    };
    let row = f.width as usize * 4;
    let stride = if stride == 0 { row } else { stride };
    let planes = Planes::new(
        format,
        f.width,
        f.height,
        data.to_vec(),
        [stride as u32, 0, 0],
        [0, 0, 0],
    )?;
    VideoFrame::from_planes(planes, f.width, f.height, capture_us)
}

// ───────────────────────────── libpipewire ─────────────────────────────

#[repr(C)]
struct SpaHook {
    _p: [*mut c_void; 6],
}

#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

#[repr(C)]
struct SpaData {
    type_: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<extern "C" fn(*mut c_void)>,
    state_changed: Option<extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Option<extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<extern "C" fn(*mut c_void, *mut PwBuffer)>,
    remove_buffer: Option<extern "C" fn(*mut c_void, *mut PwBuffer)>,
    process: Option<extern "C" fn(*mut c_void)>,
    drained: Option<extern "C" fn(*mut c_void)>,
    command: Option<extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<extern "C" fn(*mut c_void)>,
}

const STREAM_STATE_ERROR: c_int = -1;
const STREAM_STATE_UNCONNECTED: c_int = 0;
const SPA_DIRECTION_INPUT: u32 = 0;
const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;

/// The libpipewire functions we call.
struct Lib {
    init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    thread_loop_new: unsafe extern "C" fn(*const c_char, *const c_void) -> *mut c_void,
    thread_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    thread_loop_start: unsafe extern "C" fn(*mut c_void) -> c_int,
    thread_loop_stop: unsafe extern "C" fn(*mut c_void),
    thread_loop_destroy: unsafe extern "C" fn(*mut c_void),
    thread_loop_lock: unsafe extern "C" fn(*mut c_void),
    thread_loop_unlock: unsafe extern "C" fn(*mut c_void),
    context_new: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void,
    context_destroy: unsafe extern "C" fn(*mut c_void),
    context_connect_fd: unsafe extern "C" fn(*mut c_void, c_int, *mut c_void, usize) -> *mut c_void,
    core_disconnect: unsafe extern "C" fn(*mut c_void) -> c_int,
    properties_new_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    stream_new: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> *mut c_void,
    stream_add_listener:
        unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const StreamEvents, *mut c_void),
    stream_connect:
        unsafe extern "C" fn(*mut c_void, u32, u32, u32, *mut *const c_void, u32) -> c_int,
    stream_disconnect: unsafe extern "C" fn(*mut c_void) -> c_int,
    stream_destroy: unsafe extern "C" fn(*mut c_void),
    stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut PwBuffer) -> c_int,
}

// SAFETY: function pointers into a library that stays loaded.
unsafe impl Send for Lib {}
// SAFETY: as above.
unsafe impl Sync for Lib {}

fn load() -> Result<&'static Lib, VideoError> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: dlopen of the system's libpipewire by soname; the handle is
        // never closed, so the symbols stay valid.
        let h = unsafe {
            libc::dlopen(
                c"libpipewire-0.3.so.0".as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            )
        };
        if h.is_null() {
            return Err("libpipewire-0.3.so.0 isn't installed".into());
        }
        let sym = |name: &CStr| -> Result<*mut c_void, String> {
            // SAFETY: a lookup in the handle above.
            let p = unsafe { libc::dlsym(h, name.as_ptr()) };
            if p.is_null() {
                Err(format!("libpipewire has no {}", name.to_string_lossy()))
            } else {
                Ok(p)
            }
        };
        // SAFETY: each symbol is the libpipewire function of that name, with
        // the C signature declared in `Lib`.
        unsafe {
            Ok(Lib {
                init: std::mem::transmute::<*mut c_void, _>(sym(c"pw_init")?),
                thread_loop_new: std::mem::transmute::<*mut c_void, _>(sym(c"pw_thread_loop_new")?),
                thread_loop_get_loop: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_get_loop",
                )?),
                thread_loop_start: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_start",
                )?),
                thread_loop_stop: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_stop",
                )?),
                thread_loop_destroy: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_destroy",
                )?),
                thread_loop_lock: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_lock",
                )?),
                thread_loop_unlock: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_thread_loop_unlock",
                )?),
                context_new: std::mem::transmute::<*mut c_void, _>(sym(c"pw_context_new")?),
                context_destroy: std::mem::transmute::<*mut c_void, _>(sym(c"pw_context_destroy")?),
                context_connect_fd: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_context_connect_fd",
                )?),
                core_disconnect: std::mem::transmute::<*mut c_void, _>(sym(c"pw_core_disconnect")?),
                properties_new_string: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_properties_new_string",
                )?),
                stream_new: std::mem::transmute::<*mut c_void, _>(sym(c"pw_stream_new")?),
                stream_add_listener: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_stream_add_listener",
                )?),
                stream_connect: std::mem::transmute::<*mut c_void, _>(sym(c"pw_stream_connect")?),
                stream_disconnect: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_stream_disconnect",
                )?),
                stream_destroy: std::mem::transmute::<*mut c_void, _>(sym(c"pw_stream_destroy")?),
                stream_dequeue_buffer: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_stream_dequeue_buffer",
                )?),
                stream_queue_buffer: std::mem::transmute::<*mut c_void, _>(sym(
                    c"pw_stream_queue_buffer",
                )?),
            })
        }
    })
    .as_ref()
    .map_err(|e| unsupported(e.clone()))
}

/// Is libpipewire on this system?
pub(crate) fn available() -> bool {
    load().is_ok()
}

/// What the stream's callbacks share (owned by `Capture`, boxed so its
/// address is stable for libpipewire).
struct Ctx {
    lib: &'static Lib,
    stream: *mut c_void,
    shared: Arc<SourceShared>,
    format: Mutex<Option<RawFormat>>,
    window: bool,
}

extern "C" fn on_state(data: *mut c_void, _old: c_int, state: c_int, _error: *const c_char) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: data is our boxed Ctx, alive until the stream is destroyed.
        let ctx = unsafe { &*(data as *const Ctx) };
        if state == STREAM_STATE_ERROR || state == STREAM_STATE_UNCONNECTED {
            // The person stopped sharing, or the window closed.
            ctx.shared.event(SourceEvent::Ended(if ctx.window {
                crate::types::EndReason::SourceClosed
            } else {
                crate::types::EndReason::UserStopped
            }));
        }
    }));
}

extern "C" fn on_param(data: *mut c_void, id: u32, param: *const c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if id != SPA_PARAM_FORMAT || param.is_null() {
            return;
        }
        // SAFETY: data is our Ctx; param is a POD whose first word is its
        // body size, valid for the call.
        let (ctx, bytes) = unsafe {
            let size = *(param as *const u32) as usize;
            (
                &*(data as *const Ctx),
                std::slice::from_raw_parts(param as *const u8, 8 + size),
            )
        };
        let f = parse_format(bytes);
        if f.is_none() {
            ctx.shared.event(SourceEvent::Error(VideoError::os(
                0,
                "an unreadable PipeWire format",
            )));
        }
        *ctx.format.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }));
}

extern "C" fn on_process(data: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: data is our Ctx; buffers come from and go back to our
        // stream on this (the loop's) thread; a buffer's first data plane is
        // mapped (MAP_BUFFERS) for `maxsize` bytes.
        unsafe {
            let ctx = &*(data as *const Ctx);
            let b = (ctx.lib.stream_dequeue_buffer)(ctx.stream);
            if b.is_null() {
                return;
            }
            let format = *ctx.format.lock().unwrap_or_else(|e| e.into_inner());
            let sb = (*b).buffer;
            if let (Some(f), false) = (format, sb.is_null() || ctx.shared.idle()) {
                if (*sb).n_datas > 0 && !(*sb).datas.is_null() {
                    let d = &*(*sb).datas;
                    if !d.data.is_null() && !d.chunk.is_null() {
                        let c = &*d.chunk;
                        let start = (c.offset as usize).min(d.maxsize as usize);
                        let len = (c.size as usize).min(d.maxsize as usize - start);
                        if len > 0 {
                            let bytes =
                                std::slice::from_raw_parts((d.data as *const u8).add(start), len);
                            match to_frame(bytes, c.stride.max(0) as usize, &f, now_us()) {
                                Ok(frame) => ctx.shared.deliver(frame),
                                Err(e) => ctx.shared.event(SourceEvent::Error(e)),
                            }
                        }
                    }
                }
            }
            (ctx.lib.stream_queue_buffer)(ctx.stream, b);
        }
    }));
}

static EVENTS: StreamEvents = StreamEvents {
    version: 2,
    destroy: None,
    state_changed: Some(on_state),
    control_info: None,
    io_changed: None,
    param_changed: Some(on_param),
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

/// A running capture: libpipewire's thread loop, context, core and stream.
/// Dropping it stops the stream and tears everything down.
pub(crate) struct Capture {
    lib: &'static Lib,
    tl: *mut c_void,
    context: *mut c_void,
    core: *mut c_void,
    ctx: Box<Ctx>,
    hook: Box<SpaHook>,
}

// SAFETY: every libpipewire call after start happens under the thread
// loop's lock; the callbacks only touch Ctx's thread-safe parts.
unsafe impl Send for Capture {}

impl Capture {
    /// Connect to the portal's PipeWire remote and stream `node_id`.
    pub(crate) fn start(
        remote: OwnedFd,
        node_id: u32,
        max_fps: u32,
        want: (u32, u32),
        window: bool,
        shared: Arc<SourceShared>,
    ) -> Result<Capture, VideoError> {
        let lib = load()?;
        let params = enum_format(max_fps, want);
        // SAFETY: the documented libpipewire set-up order: init, a thread
        // loop and its context before start; then the core, the stream and
        // its listener under the loop's lock. Every pointer is checked.
        unsafe {
            (lib.init)(std::ptr::null_mut(), std::ptr::null_mut());
            let tl = (lib.thread_loop_new)(c"kessel-video-pw".as_ptr(), std::ptr::null());
            if tl.is_null() {
                return Err(VideoError::os(0, "pw_thread_loop_new"));
            }
            let context =
                (lib.context_new)((lib.thread_loop_get_loop)(tl), std::ptr::null_mut(), 0);
            if context.is_null() {
                (lib.thread_loop_destroy)(tl);
                return Err(VideoError::os(0, "pw_context_new"));
            }
            if (lib.thread_loop_start)(tl) < 0 {
                (lib.context_destroy)(context);
                (lib.thread_loop_destroy)(tl);
                return Err(VideoError::os(0, "pw_thread_loop_start"));
            }
            let mut cap = Capture {
                lib,
                tl,
                context,
                core: std::ptr::null_mut(),
                ctx: Box::new(Ctx {
                    lib,
                    stream: std::ptr::null_mut(),
                    shared,
                    format: Mutex::new(None),
                    window,
                }),
                hook: Box::new(SpaHook {
                    _p: [std::ptr::null_mut(); 6],
                }),
            };
            (lib.thread_loop_lock)(tl);
            // pw_context_connect_fd takes the descriptor.
            cap.core =
                (lib.context_connect_fd)(context, remote.into_raw_fd(), std::ptr::null_mut(), 0);
            if cap.core.is_null() {
                (lib.thread_loop_unlock)(tl);
                return Err(VideoError::os(0, "pw_context_connect_fd"));
            }
            let props = (lib.properties_new_string)(
                c"media.type=Video media.category=Capture media.role=Screen".as_ptr(),
            );
            let stream = (lib.stream_new)(cap.core, c"kessel-video".as_ptr(), props);
            if stream.is_null() {
                (lib.thread_loop_unlock)(tl);
                return Err(VideoError::os(0, "pw_stream_new"));
            }
            cap.ctx.stream = stream;
            let data = &*cap.ctx as *const Ctx as *mut c_void;
            (lib.stream_add_listener)(stream, &mut *cap.hook, &EVENTS, data);
            let mut list = [params.as_ptr() as *const c_void];
            let r = (lib.stream_connect)(
                stream,
                SPA_DIRECTION_INPUT,
                node_id,
                PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS,
                list.as_mut_ptr(),
                1,
            );
            (lib.thread_loop_unlock)(tl);
            if r < 0 {
                return Err(VideoError::os(r as i64, "pw_stream_connect"));
            }
            Ok(cap)
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: the reverse of start, under the lock where libpipewire
        // needs it; the thread loop stops before the context goes.
        unsafe {
            (self.lib.thread_loop_lock)(self.tl);
            if !self.ctx.stream.is_null() {
                (self.lib.stream_disconnect)(self.ctx.stream);
                (self.lib.stream_destroy)(self.ctx.stream);
            }
            if !self.core.is_null() {
                (self.lib.core_disconnect)(self.core);
            }
            (self.lib.thread_loop_unlock)(self.tl);
            (self.lib.thread_loop_stop)(self.tl);
            (self.lib.context_destroy)(self.context);
            (self.lib.thread_loop_destroy)(self.tl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layouts_match_spa() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<SpaHook>(), 48);
        assert_eq!(size_of::<SpaData>(), 40);
        assert_eq!(offset_of!(SpaData, data), 24);
        assert_eq!(offset_of!(SpaData, chunk), 32);
        assert_eq!(size_of::<SpaChunk>(), 16);
        assert_eq!(offset_of!(SpaBuffer, datas), 16);
        assert_eq!(size_of::<StreamEvents>(), 8 + 11 * 8);
    }

    #[test]
    fn the_enum_format_pod_is_well_formed_and_reads_back() {
        let p = enum_format(30, (1920, 1080));
        assert_eq!(p.len() % 8, 0, "PODs are 8-byte padded");
        assert_eq!(
            u32_at(&p, 0),
            Some((p.len() - 8) as u32),
            "the size is the body's"
        );
        assert_eq!(u32_at(&p, 4), Some(SPA_TYPE_OBJECT));
        assert_eq!(u32_at(&p, 8), Some(SPA_TYPE_OBJECT_FORMAT));
        assert_eq!(u32_at(&p, 12), Some(SPA_PARAM_ENUM_FORMAT));
        // The first property: mediaType = Id video, padded to 8.
        assert_eq!(
            &p[16..40],
            &words(&[
                SPA_FORMAT_MEDIA_TYPE,
                0,
                4,
                SPA_TYPE_ID,
                SPA_MEDIA_TYPE_VIDEO,
                0
            ])[..]
        );
        // Reading it back takes each choice's default.
        let f = parse_format(&p).expect("readable");
        assert_eq!(
            f,
            RawFormat {
                format: VIDEO_BGRX,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn a_fixed_format_parses() {
        let mut body = words(&[SPA_TYPE_OBJECT_FORMAT, SPA_PARAM_FORMAT]);
        body.extend(prop(
            SPA_FORMAT_MEDIA_TYPE,
            pod(SPA_TYPE_ID, &words(&[SPA_MEDIA_TYPE_VIDEO])),
        ));
        body.extend(prop(
            SPA_FORMAT_VIDEO_FORMAT,
            pod(SPA_TYPE_ID, &words(&[VIDEO_RGBA])),
        ));
        body.extend(prop(
            SPA_FORMAT_VIDEO_SIZE,
            pod(SPA_TYPE_RECTANGLE, &words(&[1280, 800])),
        ));
        let p = pod(SPA_TYPE_OBJECT, &body);
        assert_eq!(
            parse_format(&p),
            Some(RawFormat {
                format: VIDEO_RGBA,
                width: 1280,
                height: 800
            })
        );
        assert_eq!(parse_format(&p[..12]), None, "a truncated POD fails");
        let no_size = pod(
            SPA_TYPE_OBJECT,
            &words(&[SPA_TYPE_OBJECT_FORMAT, SPA_PARAM_FORMAT]),
        );
        assert_eq!(parse_format(&no_size), None);
    }

    #[test]
    fn buffers_become_frames() {
        // 2x2 BGRx with 12-byte rows.
        let mut data = vec![0u8; 24];
        data[..8].copy_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]);
        data[12..20].copy_from_slice(&[0, 0, 255, 0, 255, 255, 255, 0]);
        let f = to_frame(
            &data,
            12,
            &RawFormat {
                format: VIDEO_BGRX,
                width: 2,
                height: 2,
            },
            9,
        )
        .expect("frame");
        assert_eq!(
            (f.format(), f.width, f.height, f.capture_us),
            (PixelFormat::Bgra, 2, 2, 9)
        );
        assert_eq!(f.planes().expect("planes").strides[0], 12);
        assert!(
            to_frame(
                &data[..10],
                12,
                &RawFormat {
                    format: VIDEO_BGRX,
                    width: 2,
                    height: 2
                },
                0
            )
            .is_err(),
            "short buffers fail"
        );
        assert!(
            to_frame(
                &data,
                12,
                &RawFormat {
                    format: 23,
                    width: 2,
                    height: 2
                },
                0
            )
            .is_err(),
            "NV12 isn't offered"
        );
    }

    #[test]
    fn loading_libpipewire_answers_cleanly() {
        match load() {
            Ok(_) => eprintln!("[pipewire] libpipewire loaded"),
            Err(e) => {
                eprintln!("[pipewire] {e}");
                assert!(matches!(e, VideoError::Unsupported(_)));
            }
        }
    }
}

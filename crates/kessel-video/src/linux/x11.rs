//! X11 screen capture: MIT-SHM reads the screen, DAMAGE says when it
//! changed, and XFIXES gives the cursor image. Over x11rb's pure-Rust
//! connection: no libxcb.
//!
//! - **Listed, no picker.** Each screen of the X display in `DISPLAY` is a
//!   source `x11:<screen>` (`x11:0` on nearly every machine). Opening one
//!   needs only what any X client needs: the display and its cookie
//!   (`XAUTHORITY`). There is no portal and nobody to click Allow, so it
//!   also works on a server with an in-memory X server (Xvfb) and no person
//!   at it.
//! - **Frames only on change.** The source reads the screen when DAMAGE
//!   reported a change (or, with the cursor drawn, the pointer moved), at
//!   most at the asked rate. A still screen costs a poll every ~100 ms and
//!   no reads. The first frame comes at once, whatever the screen does, so
//!   a consumer always has a current picture to start from.
//! - **Size changes** (RandR on the X server) are reported: the root
//!   window's `ConfigureNotify` reallocates the read buffer and the next
//!   frame has the new size, with `SourceEvent::Started` carrying it.
//! - **The cursor** is drawn into the frames when
//!   [`CaptureConstraints::cursor`] is set: XFIXES gives its image (ARGB,
//!   premultiplied) when it changes, and the pointer position is read at
//!   the frame rate.
//! - **Reads** go through a shared-memory segment the X server writes into
//!   (MIT-SHM 1.2, a memfd passed over the socket). Where MIT-SHM is
//!   missing or refuses (a display over ssh), the core `GetImage` request is
//!   used instead: slower, the same pixels.
//!
//! Frames are BGRA (a 24- or 32-bit TrueColor root with the usual masks, in
//! LSB-first byte order); other screen formats are refused as
//! `Unsupported`.

use crate::api::{FrameSource, Wake};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, EventMask, ImageFormat, ImageOrder, Setup,
    VisualClass, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// The prefix of a listed X11 screen's id (`x11:0`).
pub(crate) const PREFIX: &str = "x11:";

/// The longest the capture thread waits before it looks at its stop flag.
const STEP_MS: u64 = 100;

/// The X11 screens of `display` (`None`: `DISPLAY`) whose format this
/// source reads. Empty when there is no X display to connect to.
pub(crate) fn sources_on(display: Option<&str>) -> Vec<ScreenSourceInfo> {
    let Ok((conn, _)) = x11rb::connect(display) else {
        return Vec::new();
    };
    let setup = conn.setup();
    setup
        .roots
        .iter()
        .enumerate()
        .filter(|(i, _)| screen_format(setup, *i).is_ok())
        .map(|(i, s)| {
            ScreenSourceInfo::new(
                format!("{PREFIX}{i}"),
                ScreenKind::Display,
                format!("X11 screen {i}"),
                s.width_in_pixels as u32,
                s.height_in_pixels as u32,
            )
        })
        .collect()
}

/// The screens of the display in `DISPLAY`.
pub(crate) fn sources() -> Vec<ScreenSourceInfo> {
    sources_on(None)
}

/// Is there an X display here whose screen this source can read?
pub(crate) fn available() -> bool {
    !sources().is_empty()
}

/// Is `id` one of ours (`x11:<n>`)?
pub(crate) fn is_x11_id(id: &str) -> bool {
    id.starts_with(PREFIX)
}

fn parse_id(id: &str) -> Option<usize> {
    id.strip_prefix(PREFIX)?.parse().ok()
}

/// Can frames of screen `i` be read as BGRA? (A TrueColor root of depth
/// 24 or 32, 32 bits a pixel, masks 0xff0000 / 0xff00 / 0xff, LSB first.)
fn screen_format(setup: &Setup, i: usize) -> Result<(), String> {
    let s = setup.roots.get(i).ok_or("no such screen")?;
    if setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err("an MSB-first X server".into());
    }
    if !matches!(s.root_depth, 24 | 32) {
        return Err(format!("a screen of depth {}", s.root_depth));
    }
    let bpp = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == s.root_depth)
        .map(|f| f.bits_per_pixel);
    if bpp != Some(32) {
        return Err(format!("{bpp:?} bits a pixel at depth {}", s.root_depth));
    }
    let visual = s
        .allowed_depths
        .iter()
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.visual_id == s.root_visual)
        .ok_or("the root visual isn't listed")?;
    let masks = (visual.red_mask, visual.green_mask, visual.blue_mask);
    if visual.class != VisualClass::TRUE_COLOR || masks != (0xff_0000, 0xff00, 0xff) {
        return Err(format!("a root visual of class {:?}, masks {masks:x?}", visual.class));
    }
    Ok(())
}

fn conn_err(e: impl std::fmt::Display) -> VideoError {
    VideoError::os(0, format!("X11: {e}"))
}

/// The pointer's picture (XFIXES `GetCursorImage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorImage {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) xhot: u16,
    pub(crate) yhot: u16,
    pub(crate) serial: u32,
    /// ARGB, premultiplied, row by row.
    pub(crate) pixels: Vec<u32>,
}

/// The shared-memory segment the X server writes the screen into.
struct Shm {
    seg: shm::Seg,
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is owned by this struct alone and only read through
// `&self` while no X request that writes it is outstanding (the grabber
// waits for the reply of each read first).
unsafe impl Send for Shm {}

impl Shm {
    fn new(conn: &RustConnection, len: usize) -> Result<Shm, String> {
        let name = c"kessel-video-x11";
        // SAFETY: memfd_create with a valid C string; the fd it returns is
        // owned by `fd` from here on.
        let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if raw < 0 {
            return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: `raw` is a fresh descriptor nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: ftruncate on our own descriptor.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: a fresh shared mapping of `len` bytes of our memfd.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let mut shm = Shm { seg: 0, ptr: ptr.cast(), len };
        // The descriptor goes to the X server (and is closed here once
        // sent); the mapping stays.
        let attached = conn
            .generate_id()
            .map_err(|e| e.to_string())
            .and_then(|seg| {
                conn.shm_attach_fd(seg, fd, false)
                    .map_err(|e| e.to_string())?
                    .check()
                    .map_err(|e| e.to_string())?;
                Ok(seg)
            });
        match attached {
            Ok(seg) => {
                shm.seg = seg;
                Ok(shm)
            }
            Err(e) => {
                shm.seg = 0;
                Err(format!("MIT-SHM attach: {e}"))
            }
        }
    }

    fn bytes(&self, n: usize) -> &[u8] {
        // SAFETY: `ptr` maps `len` bytes for as long as `self` lives; the
        // caller reads only after the server's reply (see `Send`).
        unsafe { std::slice::from_raw_parts(self.ptr, n.min(self.len)) }
    }

    fn release(&mut self, conn: &RustConnection) {
        if self.seg != 0 {
            let _ = conn.shm_detach(self.seg);
            let _ = conn.flush();
            self.seg = 0;
        }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        // SAFETY: unmapping our own mapping once.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// One X connection reading one screen.
pub(crate) struct Grabber {
    conn: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    /// MIT-SHM 1.2 is there (a failed attach falls back to `GetImage`).
    shm_ok: bool,
    shm: Option<Shm>,
    /// XFIXES' cursor image works here.
    cursor_ok: bool,
    cursor: Option<CursorImage>,
    /// The cursor image changed since it was read.
    cursor_stale: bool,
    pointer: Option<(i16, i16)>,
    /// The screen changed (DAMAGE), or nothing has been read yet.
    dirty: bool,
    /// A size change waits for the next frame.
    resized: bool,
    damage: damage::Damage,
}

impl Grabber {
    /// Connects to `display` (`None`: `DISPLAY`) and sets up DAMAGE on
    /// screen `screen`'s root. MIT-SHM and XFIXES are used when present.
    pub(crate) fn connect(display: Option<&str>, screen: usize) -> Result<Grabber, VideoError> {
        let (conn, _) = x11rb::connect(display).map_err(|e| {
            VideoError::Unsupported(format!("no X display to capture: {e}"))
        })?;
        let (root, width, height) = {
            let setup = conn.setup();
            let s = setup.roots.get(screen).ok_or(VideoError::NotFound)?;
            screen_format(setup, screen).map_err(|why| unsupported(format!("X11 screen {screen}: {why}")))?;
            (s.root, s.width_in_pixels, s.height_in_pixels)
        };
        // Extensions answer only after their version is asked.
        let has = |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        if !has(damage::X11_EXTENSION_NAME) {
            return Err(unsupported("the X server has no DAMAGE extension"));
        }
        conn.damage_query_version(1, 1)
            .map_err(conn_err)?
            .reply()
            .map_err(conn_err)?;
        let shm_ok = has(shm::X11_EXTENSION_NAME)
            && conn
                .shm_query_version()
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|v| (v.major_version, v.minor_version) >= (1, 2));
        let cursor_ok = has(xfixes::X11_EXTENSION_NAME)
            && conn
                .xfixes_query_version(4, 0)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|v| v.major_version >= 1);
        let damage = conn.generate_id().map_err(conn_err)?;
        conn.damage_create(damage, root, damage::ReportLevel::NON_EMPTY)
            .map_err(conn_err)?
            .check()
            .map_err(conn_err)?;
        // The root's ConfigureNotify: the screen changed size.
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY))
            .map_err(conn_err)?
            .check()
            .map_err(conn_err)?;
        if cursor_ok {
            conn.xfixes_select_cursor_input(root, xfixes::CursorNotifyMask::DISPLAY_CURSOR)
                .map_err(conn_err)?
                .check()
                .map_err(conn_err)?;
        }
        Ok(Grabber {
            conn,
            root,
            width,
            height,
            shm_ok,
            shm: None,
            cursor_ok,
            cursor: None,
            cursor_stale: true,
            pointer: None,
            dirty: true,
            resized: false,
            damage,
        })
    }

    /// The screen's size now.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    /// Reads every queued X event: damage, size changes, cursor changes.
    /// An error means the X connection is gone.
    fn drain_events(&mut self) -> Result<(), ConnectionError> {
        while let Some(ev) = self.conn.poll_for_event()? {
            match ev {
                Event::DamageNotify(_) => self.dirty = true,
                Event::ConfigureNotify(e) if e.window == self.root => {
                    if (e.width, e.height) != (self.width, self.height) {
                        self.width = e.width;
                        self.height = e.height;
                        self.resized = true;
                        self.dirty = true;
                    }
                }
                Event::XfixesCursorNotify(_) => {
                    self.cursor_stale = true;
                    self.dirty = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Waits up to `ms` for the X server to send something.
    fn wait(&self, ms: u64) {
        let mut pfd = libc::pollfd { fd: self.conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd on the connection's own socket.
        unsafe { libc::poll(&mut pfd, 1, ms.min(i32::MAX as u64) as i32) };
    }

    /// The cursor image, read again when XFIXES said it changed. `None`
    /// without XFIXES.
    pub(crate) fn cursor(&mut self) -> Option<&CursorImage> {
        if !self.cursor_ok {
            return None;
        }
        if self.cursor_stale || self.cursor.is_none() {
            let r = self.conn.xfixes_get_cursor_image().ok()?.reply().ok()?;
            self.cursor = Some(CursorImage {
                width: r.width,
                height: r.height,
                xhot: r.xhot,
                yhot: r.yhot,
                serial: r.cursor_serial,
                pixels: r.cursor_image,
            });
            self.cursor_stale = false;
        }
        self.cursor.as_ref()
    }

    /// Where the pointer is on this screen; marks the screen dirty when it
    /// moved since the last look. `None` when it is on another screen.
    fn poll_pointer(&mut self) -> Result<Option<(i16, i16)>, ReplyError> {
        let r = self.conn.query_pointer(self.root)?.reply()?;
        let now = r.same_screen.then_some((r.root_x, r.root_y));
        if now != self.pointer {
            self.pointer = now;
            self.dirty = true;
        }
        Ok(now)
    }

    /// Reads the whole screen as tight BGRA rows.
    fn read(&mut self) -> Result<Vec<u8>, ReplyOrIdError> {
        let (w, h) = (self.width, self.height);
        let len = w as usize * h as usize * 4;
        if self.shm_ok && self.shm.as_ref().is_none_or(|s| s.len != len) {
            if let Some(mut old) = self.shm.take() {
                old.release(&self.conn);
            }
            match Shm::new(&self.conn, len) {
                Ok(s) => self.shm = Some(s),
                // A display over ssh: the server can't map our memory.
                Err(_) => self.shm_ok = false,
            }
        }
        if let Some(s) = self.shm.as_ref() {
            let r = self
                .conn
                .shm_get_image(self.root, 0, 0, w, h, !0, ImageFormat::Z_PIXMAP.into(), s.seg, 0)?
                .reply()?;
            let n = (r.size as usize).min(len);
            let mut out = s.bytes(n).to_vec();
            out.resize(len, 0);
            return Ok(out);
        }
        let r = self.conn.get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, w, h, !0)?.reply()?;
        let mut out = r.data;
        out.resize(len, 0);
        Ok(out)
    }

    /// One frame of the whole screen now, with the cursor drawn in when
    /// `cursor` is set. Clears the damage first, so a change while it reads
    /// marks the screen dirty again.
    pub(crate) fn frame(&mut self, cursor: bool) -> Result<VideoFrame, VideoError> {
        if cursor && self.cursor_ok {
            self.poll_pointer().map_err(conn_err)?;
        }
        self.dirty = false;
        self.resized = false;
        self.conn
            .damage_subtract(self.damage, x11rb::NONE, x11rb::NONE)
            .map_err(conn_err)?;
        let capture_us = now_us();
        let mut data = self.read().map_err(conn_err)?;
        let (w, h) = self.size();
        if cursor {
            if let (Some(pos), Some(img)) = (self.pointer, self.cursor().cloned()) {
                draw_cursor(&mut data, w, h, pos, &img);
            }
        }
        let planes = Planes::tight(PixelFormat::Bgra, w, h, data)?;
        VideoFrame::from_planes(planes, w, h, capture_us)
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        if let Some(mut s) = self.shm.take() {
            s.release(&self.conn);
        }
        let _ = self.conn.damage_destroy(self.damage);
        let _ = self.conn.flush();
    }
}

/// Draws a premultiplied ARGB cursor over BGRA pixels, its hot spot at
/// `pos`, clipped to the frame.
pub(crate) fn draw_cursor(data: &mut [u8], w: u32, h: u32, pos: (i16, i16), img: &CursorImage) {
    let x0 = pos.0 as i64 - img.xhot as i64;
    let y0 = pos.1 as i64 - img.yhot as i64;
    for cy in 0..img.height as i64 {
        let y = y0 + cy;
        if y < 0 || y >= h as i64 {
            continue;
        }
        for cx in 0..img.width as i64 {
            let x = x0 + cx;
            if x < 0 || x >= w as i64 {
                continue;
            }
            let Some(&p) = img.pixels.get((cy * img.width as i64 + cx) as usize) else {
                continue;
            };
            let a = p >> 24;
            if a == 0 {
                continue;
            }
            let at = ((y * w as i64 + x) * 4) as usize;
            let Some(px) = data.get_mut(at..at + 3) else { continue };
            let inv = 255 - a;
            for (i, shift) in [0u32, 8, 16].into_iter().enumerate() {
                let src = (p >> shift) & 0xff;
                px[i] = (src + (px[i] as u32 * inv + 127) / 255).min(255) as u8;
            }
        }
    }
}

struct X11Control {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SourceControl for X11Control {}

impl Drop for X11Control {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens listed screen `id` of `DISPLAY`.
pub(crate) fn open(id: &str, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    open_on(None, id, c, wake)
}

/// Opens listed screen `id` of `display` (`None`: `DISPLAY`).
pub(crate) fn open_on(
    display: Option<&str>,
    id: &str,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    let screen = parse_id(id).ok_or(VideoError::NotFound)?;
    let grabber = Grabber::connect(display, screen)?;
    // The asked rate rules: the screen itself has no rate of its own.
    let shared = SourceShared::new(c, 240, wake);
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (shared.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("kessel-video-x11".into())
        .spawn(move || run(grabber, s, st))
        .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?;
    Ok(Box::new(QueuedSource::new(shared, X11Control { stop, thread: Some(thread) })))
}

/// The capture thread: wait for damage (or a pointer move), read at most
/// once an interval, deliver.
fn run(mut g: Grabber, shared: Arc<SourceShared>, stop: Arc<AtomicBool>) {
    let mut last_us: Option<u64> = None;
    let mut last_pointer_us = 0u64;
    let lost = || shared.event(SourceEvent::Ended(EndReason::DeviceLost));
    while !stop.load(Ordering::Acquire) && !shared.has_ended() {
        if g.drain_events().is_err() {
            return lost();
        }
        let c = shared.constraints();
        let interval = 1_000_000 / c.fps.unwrap_or(30).clamp(1, 240) as u64;
        let now = now_us();
        if c.cursor && g.cursor_ok && now >= last_pointer_us + interval {
            last_pointer_us = now;
            if g.poll_pointer().is_err() {
                return lost();
            }
        }
        let due = last_us.map_or(0, |t| t + interval);
        if g.dirty && !shared.idle() && now >= due {
            match g.frame(c.cursor) {
                Ok(f) => {
                    last_us = Some(f.capture_us);
                    shared.deliver(f);
                }
                Err(VideoError::Os { .. }) => return lost(),
                Err(e) => shared.event(SourceEvent::Error(e)),
            }
            continue;
        }
        // Sleep until the next frame is due, the pointer is due a look,
        // or the X server says something; never longer than a step.
        let mut wait_ms = STEP_MS;
        if g.dirty && !shared.idle() {
            wait_ms = wait_ms.min(due.saturating_sub(now).div_ceil(1000));
        }
        if c.cursor && g.cursor_ok {
            wait_ms = wait_ms.min((last_pointer_us + interval).saturating_sub(now).div_ceil(1000));
        }
        if g.conn.flush().is_err() {
            return lost();
        }
        g.wait(wait_ms.max(1));
    }
}

#[cfg(test)]
#[path = "x11_tests.rs"]
mod tests;

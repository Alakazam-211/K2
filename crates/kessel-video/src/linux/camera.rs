//! Cameras through V4L2: the list (with modes), the permission (file
//! access to the device), and a capture thread that streams mmap buffers
//! and hands out NV12, I420 or YUYV-converted I420 frames.

use super::v4l2::*;
use crate::api::{Done, FrameSource, Wake};
use crate::error::{unsupported, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{now_us, QueuedSource, SourceControl, SourceShared};
use crate::types::*;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// The formats we read, best first.
const FORMATS: [u32; 3] = [PIX_NV12, PIX_YU12, PIX_YUYV];

/// `/dev/videoN` nodes, with a stable id from `/dev/v4l/by-id` when there
/// is one.
fn nodes() -> Vec<(String, String)> {
    let mut by_id = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/dev/v4l/by-id") {
        for e in dir.flatten() {
            if let Ok(target) = std::fs::canonicalize(e.path()) {
                by_id.push((
                    target.to_string_lossy().into_owned(),
                    e.path().to_string_lossy().into_owned(),
                ));
            }
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    let Ok(dir) = std::fs::read_dir("/dev") else {
        return out;
    };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("video") {
            if n.parse::<u32>().is_ok() {
                let path = format!("/dev/{name}");
                let id = by_id
                    .iter()
                    .find(|(t, _)| *t == path)
                    .map_or(path.clone(), |(_, id)| id.clone());
                out.push((path, id));
            }
        }
    }
    out.sort_by_key(|(p, _)| {
        p.trim_start_matches("/dev/video")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    out
}

fn caps(fd: &Fd) -> io::Result<Capability> {
    let mut c: Capability = zeroed();
    fd.ioctl(VIDIOC_QUERYCAP, &mut c)?;
    Ok(c)
}

fn is_capture(c: &Capability) -> bool {
    let caps = if c.capabilities & CAP_DEVICE_CAPS != 0 {
        c.device_caps
    } else {
        c.capabilities
    };
    caps & CAP_VIDEO_CAPTURE != 0 && caps & CAP_STREAMING != 0
}

/// Modes per pixel format we read: (mode, fourcc).
fn modes(fd: &Fd) -> Vec<(CaptureMode, u32)> {
    let mut out: Vec<(CaptureMode, u32)> = Vec::new();
    for i in 0.. {
        let mut d: FmtDesc = zeroed();
        d.index = i;
        d.type_ = BUF_TYPE_VIDEO_CAPTURE;
        if fd.ioctl(VIDIOC_ENUM_FMT, &mut d).is_err() {
            break;
        }
        if !FORMATS.contains(&d.pixelformat) {
            continue;
        }
        for (w, h) in sizes(fd, d.pixelformat) {
            let fps = max_fps(fd, d.pixelformat, w, h);
            if fps == 0 {
                continue;
            }
            let mode = CaptureMode::new(
                w,
                h,
                fps,
                if d.pixelformat == PIX_NV12 {
                    PixelFormat::Nv12
                } else {
                    PixelFormat::I420
                },
            );
            let rank = |f: u32| FORMATS.iter().position(|x| *x == f).unwrap_or(9);
            match out.iter_mut().find(|(m, _)| (m.width, m.height) == (w, h)) {
                Some(slot)
                    if slot.0.max_fps < fps
                        || (slot.0.max_fps == fps && rank(d.pixelformat) < rank(slot.1)) =>
                {
                    *slot = (mode, d.pixelformat)
                }
                Some(_) => {}
                None => out.push((mode, d.pixelformat)),
            }
        }
    }
    out.sort_by_key(|(m, _)| std::cmp::Reverse((m.width as u64 * m.height as u64, m.max_fps)));
    out
}

fn sizes(fd: &Fd, pixfmt: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut s: FrmSizeEnum = zeroed();
        s.index = i;
        s.pixel_format = pixfmt;
        if fd.ioctl(VIDIOC_ENUM_FRAMESIZES, &mut s).is_err() {
            break;
        }
        if s.type_ == FRMSIZE_TYPE_DISCRETE {
            out.push((s.size[0], s.size[1]));
        } else {
            // Stepwise or continuous: the largest, and the common sizes in
            // range.
            let (min_w, max_w, min_h, max_h) = (s.size[0], s.size[1], s.size[3], s.size[4]);
            for (w, h) in [
                (max_w, max_h),
                (1920, 1080),
                (1280, 720),
                (640, 480),
                (640, 360),
                (320, 240),
            ] {
                if (min_w..=max_w).contains(&w)
                    && (min_h..=max_h).contains(&h)
                    && !out.contains(&(w, h))
                {
                    out.push((w, h));
                }
            }
            break;
        }
    }
    out
}

fn max_fps(fd: &Fd, pixfmt: u32, w: u32, h: u32) -> u32 {
    let mut best = 0f64;
    for i in 0.. {
        let mut v: FrmIvalEnum = zeroed();
        v.index = i;
        v.pixel_format = pixfmt;
        v.width = w;
        v.height = h;
        if fd.ioctl(VIDIOC_ENUM_FRAMEINTERVALS, &mut v).is_err() {
            break;
        }
        // Discrete: the interval; stepwise: its minimum interval.
        let (num, den) = (v.ival[0], v.ival[1]);
        if num > 0 {
            best = best.max(den as f64 / num as f64);
        }
        if v.type_ != FRMIVAL_TYPE_DISCRETE {
            break;
        }
    }
    if best == 0.0 {
        // Drivers that don't list intervals: assume 30.
        30
    } else {
        best.round() as u32
    }
}

fn info(path: &str, id: &str, first: bool) -> Option<CameraInfo> {
    let fd = Fd::open(path).ok()?;
    let c = caps(&fd).ok()?;
    if !is_capture(&c) {
        return None;
    }
    let name = text(&c.card);
    let lower = name.to_lowercase();
    let facing =
        if lower.contains("integrated") || lower.contains("built-in") || lower.contains("internal")
        {
            Facing::Front
        } else {
            Facing::External
        };
    let mut cam = CameraInfo::new(
        id,
        if name.is_empty() {
            path.to_string()
        } else {
            name
        },
        facing,
        first,
    );
    cam.modes = modes(&fd).into_iter().map(|(m, _)| m).collect();
    // A metadata node of the same camera lists no image formats.
    (!cam.modes.is_empty()).then_some(cam)
}

pub(crate) fn cameras() -> Vec<CameraInfo> {
    let mut out: Vec<CameraInfo> = Vec::new();
    for (path, id) in nodes() {
        if let Some(c) = info(&path, &id, out.is_empty()) {
            out.push(c);
        }
    }
    out
}

pub(crate) fn permission() -> Permission {
    for (path, _) in nodes() {
        match Fd::open(&path) {
            Ok(_) => return Permission::Granted,
            Err(e)
                if e.raw_os_error() == Some(libc::EACCES)
                    || e.raw_os_error() == Some(libc::EPERM) =>
            {
                return Permission::Denied
            }
            Err(_) => {}
        }
    }
    // No cameras: nothing is refused.
    Permission::Granted
}

pub(crate) fn request_permission(done: Done<Permission>) {
    // Device access comes from the user's groups (video) or the session
    // (logind ACLs); there is no prompt to show.
    done(permission())
}

fn os_err(e: io::Error, what: &str) -> VideoError {
    match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => VideoError::NoPermission,
        Some(libc::EBUSY) => VideoError::DeviceBusy,
        Some(libc::ENODEV) | Some(libc::ENXIO) => VideoError::DeviceLost,
        Some(libc::ENOENT) => VideoError::NotFound,
        Some(n) => VideoError::os(n as i64, format!("{what}: {e}")),
        None => VideoError::os(0, format!("{what}: {e}")),
    }
}

/// The negotiated stream: format, size, row stride.
#[derive(Clone, Copy, Debug)]
struct Negotiated {
    pixfmt: u32,
    width: u32,
    height: u32,
    stride: u32,
}

struct Stream {
    fd: Fd,
    bufs: Vec<Mapped>,
    fmt: Negotiated,
}

fn start_stream(
    path: &str,
    mode: CaptureMode,
    pixfmt: u32,
    fps: u32,
) -> Result<Stream, VideoError> {
    let fd = Fd::open(path).map_err(|e| os_err(e, "open"))?;
    let mut f: Format = zeroed();
    f.type_ = BUF_TYPE_VIDEO_CAPTURE;
    // Writing a union field is safe; reading it back below isn't.
    f.fmt.pix.width = mode.width;
    f.fmt.pix.height = mode.height;
    f.fmt.pix.pixelformat = pixfmt;
    f.fmt.pix.field = FIELD_ANY;
    fd.ioctl(VIDIOC_S_FMT, &mut f)
        .map_err(|e| os_err(e, "VIDIOC_S_FMT"))?;
    // SAFETY: the driver filled the pix member for a capture format.
    let pix = unsafe { f.fmt.pix };
    if !FORMATS.contains(&pix.pixelformat) {
        return Err(unsupported(format!(
            "the camera switched to pixel format {:?}",
            pix.pixelformat.to_le_bytes()
        )));
    }
    let min_stride = if pix.pixelformat == PIX_YUYV {
        pix.width * 2
    } else {
        pix.width
    };
    let fmt = Negotiated {
        pixfmt: pix.pixelformat,
        width: pix.width,
        height: pix.height,
        stride: pix.bytesperline.max(min_stride),
    };
    let mut p: StreamParm = zeroed();
    p.type_ = BUF_TYPE_VIDEO_CAPTURE;
    p.parm.capture.timeperframe = [1, fps.max(1)];
    // Not every driver takes a rate; frames are dropped to it anyway.
    let _ = fd.ioctl(VIDIOC_S_PARM, &mut p);
    let mut rb: RequestBuffers = zeroed();
    rb.count = 4;
    rb.type_ = BUF_TYPE_VIDEO_CAPTURE;
    rb.memory = MEMORY_MMAP;
    fd.ioctl(VIDIOC_REQBUFS, &mut rb)
        .map_err(|e| os_err(e, "VIDIOC_REQBUFS"))?;
    if rb.count < 2 {
        return Err(VideoError::os(
            0,
            format!("the camera gave {} buffers", rb.count),
        ));
    }
    let mut bufs = Vec::new();
    for i in 0..rb.count {
        let mut b: Buffer = zeroed();
        b.index = i;
        b.type_ = BUF_TYPE_VIDEO_CAPTURE;
        b.memory = MEMORY_MMAP;
        fd.ioctl(VIDIOC_QUERYBUF, &mut b)
            .map_err(|e| os_err(e, "VIDIOC_QUERYBUF"))?;
        bufs.push(Mapped::map(&fd, b.m, b.length as usize).map_err(|e| os_err(e, "mmap"))?);
        fd.ioctl(VIDIOC_QBUF, &mut b)
            .map_err(|e| os_err(e, "VIDIOC_QBUF"))?;
    }
    let mut t = BUF_TYPE_VIDEO_CAPTURE as i32;
    fd.ioctl(VIDIOC_STREAMON, &mut t)
        .map_err(|e| os_err(e, "VIDIOC_STREAMON"))?;
    Ok(Stream { fd, bufs, fmt })
}

/// One captured buffer as a tight frame.
pub(crate) fn to_frame(
    data: &[u8],
    pixfmt: u32,
    width: u32,
    height: u32,
    stride: u32,
    capture_us: u64,
) -> Result<VideoFrame, VideoError> {
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let need = |n: usize| {
        if data.len() < n {
            Err(VideoError::os(
                0,
                format!("a {}-byte frame, {n} expected", data.len()),
            ))
        } else {
            Ok(())
        }
    };
    let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
    let format = match pixfmt {
        PIX_NV12 => {
            need(s * h + s * (ch - 1) + cw * 2)?;
            for y in 0..h {
                out.extend_from_slice(&data[y * s..][..w]);
            }
            for y in 0..ch {
                out.extend_from_slice(&data[s * h + y * s..][..cw * 2]);
            }
            PixelFormat::Nv12
        }
        PIX_YU12 => {
            let cs = s / 2;
            need(s * h + 2 * cs * ch)?;
            for y in 0..h {
                out.extend_from_slice(&data[y * s..][..w]);
            }
            for plane in 0..2 {
                let base = s * h + plane * cs * ch;
                for y in 0..ch {
                    out.extend_from_slice(&data[base + y * cs..][..cw]);
                }
            }
            PixelFormat::I420
        }
        PIX_YUYV => {
            need(s * (h - 1) + w * 2)?;
            out.resize(w * h + 2 * cw * ch, 0);
            let (luma, chroma) = out.split_at_mut(w * h);
            let (u, v) = chroma.split_at_mut(cw * ch);
            for y in 0..h {
                let row = &data[y * s..][..w * 2];
                for x in 0..w {
                    luma[y * w + x] = row[2 * x];
                }
                if y % 2 == 0 {
                    // 4:2:2 to 4:2:0: average this row's chroma with the next.
                    let next = if y + 1 < h {
                        &data[(y + 1) * s..][..w * 2]
                    } else {
                        row
                    };
                    for cx in 0..cw {
                        let i = (4 * cx).min(w * 2 - 4);
                        u[(y / 2) * cw + cx] =
                            ((row[i + 1] as u16 + next[i + 1] as u16 + 1) / 2) as u8;
                        v[(y / 2) * cw + cx] =
                            ((row[i + 3] as u16 + next[i + 3] as u16 + 1) / 2) as u8;
                    }
                }
            }
            PixelFormat::I420
        }
        other => {
            return Err(unsupported(format!(
                "pixel format {:?}",
                other.to_le_bytes()
            )))
        }
    };
    VideoFrame::from_planes(
        Planes::tight(format, width, height, out)?,
        width,
        height,
        capture_us,
    )
}

struct CameraControl {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SourceControl for CameraControl {}

impl Drop for CameraControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn capture(stream: Stream, shared: Arc<SourceShared>, stop: Arc<AtomicBool>) {
    let Stream { fd, bufs, fmt } = stream;
    while !stop.load(Ordering::Acquire) {
        match fd.wait(100) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(e) => {
                let _ = e;
                shared.event(SourceEvent::Ended(EndReason::DeviceLost));
                return;
            }
        }
        let mut b: Buffer = zeroed();
        b.type_ = BUF_TYPE_VIDEO_CAPTURE;
        b.memory = MEMORY_MMAP;
        match fd.ioctl(VIDIOC_DQBUF, &mut b) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => continue,
            Err(_) => {
                shared.event(SourceEvent::Ended(EndReason::DeviceLost));
                return;
            }
        }
        if !shared.idle() {
            if let Some(m) = bufs.get(b.index as usize) {
                match to_frame(
                    m.bytes(b.bytesused as usize),
                    fmt.pixfmt,
                    fmt.width,
                    fmt.height,
                    fmt.stride,
                    now_us(),
                ) {
                    Ok(f) => shared.deliver(f),
                    Err(e) => shared.event(SourceEvent::Error(e)),
                }
            }
        }
        if fd.ioctl(VIDIOC_QBUF, &mut b).is_err() {
            shared.event(SourceEvent::Ended(EndReason::DeviceLost));
            return;
        }
    }
    let mut t = BUF_TYPE_VIDEO_CAPTURE as i32;
    let _ = fd.ioctl(VIDIOC_STREAMOFF, &mut t);
    drop(bufs);
}

pub(crate) fn open(
    id: Option<&str>,
    c: CaptureConstraints,
    wake: Wake,
) -> Result<Box<dyn FrameSource>, VideoError> {
    c.validate()?;
    let all = nodes();
    let (path, _) = match id {
        Some(id) => all
            .iter()
            .find(|(p, i)| i == id || p == id)
            .cloned()
            .ok_or(VideoError::NotFound)?,
        None => all
            .iter()
            .find(|(p, _)| {
                Fd::open(p)
                    .ok()
                    .and_then(|fd| caps(&fd).ok())
                    .is_some_and(|c| is_capture(&c))
            })
            .cloned()
            .ok_or(VideoError::NotFound)?,
    };
    let probe = Fd::open(&path).map_err(|e| os_err(e, "open"))?;
    let list = modes(&probe);
    drop(probe);
    let plain: Vec<CaptureMode> = list.iter().map(|(m, _)| *m).collect();
    let mode = nearest_mode(&plain, &c)
        .ok_or_else(|| unsupported("the camera offers no NV12, I420 or YUYV mode"))?;
    let pixfmt = list
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, f)| *f)
        .ok_or(VideoError::NotFound)?;
    let fps = c
        .fps
        .map_or(mode.max_fps.min(30), |f| f.min(mode.max_fps).max(1));
    let stream = start_stream(&path, mode, pixfmt, fps)?;
    let shared = SourceShared::new(c, fps, wake);
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (shared.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("kessel-video-v4l2".into())
        .spawn(move || capture(stream, s, st))
        .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?;
    Ok(Box::new(QueuedSource::new(
        shared,
        CameraControl {
            stop,
            thread: Some(thread),
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_convert_to_frames() {
        // 4x2 YUYV: Y0 U Y1 V per pixel pair, rows of 8 bytes plus 4 of padding.
        let row0 = [10, 100, 20, 200, 30, 110, 40, 210, 0, 0, 0, 0];
        let row1 = [50, 102, 60, 202, 70, 112, 80, 212, 0, 0, 0, 0];
        let data: Vec<u8> = row0.iter().chain(row1.iter()).copied().collect();
        let f = to_frame(&data, PIX_YUYV, 4, 2, 12, 5).expect("yuyv");
        let p = f.planes().expect("planes");
        assert_eq!(p.format, PixelFormat::I420);
        assert_eq!(&p.data[..8], &[10, 20, 30, 40, 50, 60, 70, 80]);
        assert_eq!(&p.data[8..10], &[101, 111], "U averaged over the two rows");
        assert_eq!(&p.data[10..12], &[201, 211], "V averaged over the two rows");
        assert_eq!(f.capture_us, 5);

        // 2x2 NV12 with 4-byte rows.
        let nv12 = [1, 2, 0, 0, 3, 4, 0, 0, 9, 8, 0, 0];
        let f = to_frame(&nv12, PIX_NV12, 2, 2, 4, 0).expect("nv12");
        assert_eq!(&f.planes().expect("p").data[..], &[1, 2, 3, 4, 9, 8]);

        // 2x2 I420 (YU12), tight.
        let yu12 = [1, 2, 3, 4, 7, 9];
        let f = to_frame(&yu12, PIX_YU12, 2, 2, 2, 0).expect("yu12");
        assert_eq!(&f.planes().expect("p").data[..], &yu12);

        assert!(
            to_frame(&[0u8; 5], PIX_YUYV, 4, 2, 8, 0).is_err(),
            "a short buffer fails loudly"
        );
        assert!(to_frame(&[0u8; 64], fourcc(b"MJPG"), 4, 2, 8, 0).is_err());
    }

    #[test]
    fn listing_runs_without_cameras() {
        // Compute nodes have no cameras: the list is empty or real, and the
        // permission is an answer, not a hang.
        let cams = cameras();
        eprintln!("[v4l2] {} cameras", cams.len());
        for c in &cams {
            assert!(!c.modes.is_empty());
        }
        let p = permission();
        assert!(matches!(p, Permission::Granted | Permission::Denied));
        if cams.is_empty() {
            assert_eq!(
                open(None, CaptureConstraints::default(), crate::api::no_wake()).err(),
                Some(VideoError::NotFound)
            );
        }
    }
}

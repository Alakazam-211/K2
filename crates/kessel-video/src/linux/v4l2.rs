//! V4L2 through raw ioctls (the kernel's video capture interface; no
//! library). The structs mirror `<linux/videodev2.h>` for 64-bit targets;
//! tests pin their sizes and the ioctl numbers built from them.

#![allow(dead_code)]

use std::ffi::CString;
use std::io;
use std::os::raw::{c_int, c_ulong, c_void};

pub const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const MEMORY_MMAP: u32 = 1;
pub const FIELD_ANY: u32 = 0;
pub const CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
pub const CAP_STREAMING: u32 = 0x0400_0000;
pub const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
pub const FRMSIZE_TYPE_DISCRETE: u32 = 1;
pub const FRMIVAL_TYPE_DISCRETE: u32 = 1;

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

pub const PIX_NV12: u32 = fourcc(b"NV12");
pub const PIX_YU12: u32 = fourcc(b"YU12");
pub const PIX_YUYV: u32 = fourcc(b"YUYV");

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FmtDesc {
    pub index: u32,
    pub type_: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrmSizeEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub type_: u32,
    /// Discrete: [width, height]; stepwise: [min_w, max_w, step_w, min_h,
    /// max_h, step_h].
    pub size: [u32; 6],
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrmIvalEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub type_: u32,
    /// Discrete: [numerator, denominator]; stepwise: min, max, step.
    pub ival: [u32; 6],
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PixFormat {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub priv_: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

/// `struct v4l2_format`: the union holds pointers (v4l2_window), so it's
/// 8-aligned and the 200-byte body starts at offset 8.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Format {
    pub type_: u32,
    pub fmt: FormatUnion,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union FormatUnion {
    pub pix: PixFormat,
    pub raw: [u8; 200],
    _align: [u64; 25],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RequestBuffers {
    pub count: u32,
    pub type_: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub sec: i64,
    pub usec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timecode {
    pub type_: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Buffer {
    pub index: u32,
    pub type_: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: Timeval,
    pub timecode: Timecode,
    pub sequence: u32,
    pub memory: u32,
    /// The `m` union: the mmap offset for MEMORY_MMAP (an 8-byte union).
    pub m: u64,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CaptureParm {
    pub capability: u32,
    pub capturemode: u32,
    pub timeperframe: [u32; 2],
    pub extendedmode: u32,
    pub readbuffers: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StreamParm {
    pub type_: u32,
    pub parm: StreamParmUnion,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union StreamParmUnion {
    pub capture: CaptureParm,
    pub raw: [u8; 200],
}

const fn ioc(dir: u32, nr: u32, size: usize) -> c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr) as c_ulong
}
const R: u32 = 2;
const W: u32 = 1;
const RW: u32 = 3;

pub const VIDIOC_QUERYCAP: c_ulong = ioc(R, 0, std::mem::size_of::<Capability>());
pub const VIDIOC_ENUM_FMT: c_ulong = ioc(RW, 2, std::mem::size_of::<FmtDesc>());
pub const VIDIOC_G_FMT: c_ulong = ioc(RW, 4, std::mem::size_of::<Format>());
pub const VIDIOC_S_FMT: c_ulong = ioc(RW, 5, std::mem::size_of::<Format>());
pub const VIDIOC_REQBUFS: c_ulong = ioc(RW, 8, std::mem::size_of::<RequestBuffers>());
pub const VIDIOC_QUERYBUF: c_ulong = ioc(RW, 9, std::mem::size_of::<Buffer>());
pub const VIDIOC_QBUF: c_ulong = ioc(RW, 15, std::mem::size_of::<Buffer>());
pub const VIDIOC_DQBUF: c_ulong = ioc(RW, 17, std::mem::size_of::<Buffer>());
pub const VIDIOC_STREAMON: c_ulong = ioc(W, 18, std::mem::size_of::<c_int>());
pub const VIDIOC_STREAMOFF: c_ulong = ioc(W, 19, std::mem::size_of::<c_int>());
pub const VIDIOC_S_PARM: c_ulong = ioc(RW, 22, std::mem::size_of::<StreamParm>());
pub const VIDIOC_ENUM_FRAMESIZES: c_ulong = ioc(RW, 74, std::mem::size_of::<FrmSizeEnum>());
pub const VIDIOC_ENUM_FRAMEINTERVALS: c_ulong = ioc(RW, 75, std::mem::size_of::<FrmIvalEnum>());

/// An all-zero value of a plain C struct.
pub fn zeroed<T: Copy>() -> T {
    // SAFETY: only used with the plain-old-data structs above, for which
    // all-zero bytes are a valid value.
    unsafe { std::mem::zeroed() }
}

/// An open device (closed on drop).
pub struct Fd(pub c_int);

impl Fd {
    pub fn open(path: &str) -> io::Result<Fd> {
        let c = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: a NUL-terminated path; plain flags.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Fd(fd))
        }
    }

    /// One ioctl on `arg`, retried on EINTR.
    pub fn ioctl<T>(&self, req: c_ulong, arg: &mut T) -> io::Result<()> {
        loop {
            // SAFETY: `req` is a V4L2 request whose argument is `T` (the
            // request number encodes `size_of::<T>()`); `arg` is valid for
            // reads and writes of T.
            let r = unsafe { libc::ioctl(self.0, req as _, arg as *mut T as *mut c_void) };
            if r == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(e);
            }
        }
    }

    /// Wait up to `ms` for a frame. True when one is ready.
    pub fn wait(&self, ms: i32) -> io::Result<bool> {
        let mut p = libc::pollfd {
            fd: self.0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut p, 1, ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EINTR) { Ok(false) } else { Err(e) };
        }
        if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 && p.revents & libc::POLLIN == 0 {
            return Err(io::Error::from_raw_os_error(libc::ENODEV));
        }
        Ok(r > 0 && p.revents & libc::POLLIN != 0)
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: we own the descriptor.
        unsafe { libc::close(self.0) };
    }
}

/// A mapped capture buffer (unmapped on drop).
pub struct Mapped {
    pub ptr: *mut u8,
    pub len: usize,
}

// SAFETY: the mapping is only read by the capture thread that owns it.
unsafe impl Send for Mapped {}

impl Mapped {
    pub fn map(fd: &Fd, offset: u64, len: usize) -> io::Result<Mapped> {
        // SAFETY: a shared read/write mapping of the driver's buffer at the
        // offset QUERYBUF gave.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.0, offset as libc::off_t) };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapped { ptr: p as *mut u8, len })
    }

    /// The first `n` bytes.
    pub fn bytes(&self, n: usize) -> &[u8] {
        // SAFETY: a live mapping of `len` bytes.
        unsafe { std::slice::from_raw_parts(self.ptr, n.min(self.len)) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.ptr as *mut c_void, self.len) };
    }
}

/// A NUL-padded C string field as text.
pub fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn struct_sizes_match_the_kernel_abi() {
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<FmtDesc>(), 64);
        assert_eq!(size_of::<FrmSizeEnum>(), 44);
        assert_eq!(size_of::<FrmIvalEnum>(), 52);
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<Format>(), 208);
        assert_eq!(size_of::<RequestBuffers>(), 20);
        assert_eq!(size_of::<Buffer>(), 88);
        assert_eq!(std::mem::offset_of!(Buffer, timestamp), 24);
        assert_eq!(std::mem::offset_of!(Buffer, m), 64);
        assert_eq!(std::mem::offset_of!(Buffer, length), 72);
        assert_eq!(size_of::<StreamParm>(), 204);
    }

    #[test]
    fn ioctl_numbers_match_videodev2() {
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_G_FMT, 0xc0d0_5604);
        assert_eq!(VIDIOC_S_FMT, 0xc0d0_5605);
        assert_eq!(VIDIOC_REQBUFS, 0xc014_5608);
        assert_eq!(VIDIOC_QUERYBUF, 0xc058_5609);
        assert_eq!(VIDIOC_QBUF, 0xc058_560f);
        assert_eq!(VIDIOC_DQBUF, 0xc058_5611);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(VIDIOC_STREAMOFF, 0x4004_5613);
        assert_eq!(VIDIOC_S_PARM, 0xc0cc_5616);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
        assert_eq!(VIDIOC_ENUM_FRAMEINTERVALS, 0xc034_564b);
    }

    #[test]
    fn fourccs_and_text() {
        assert_eq!(PIX_YUYV, 0x5659_5559);
        assert_eq!(PIX_NV12, 0x3231_564e);
        assert_eq!(text(b"UVC Camera\0\0\0"), "UVC Camera");
    }
}

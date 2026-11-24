//! The raw kernel calls the block uses (Linux, x86-64 and aarch64; the
//! numbers below are the same on both).

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::filter::{SeccompData, SockFilter};

const SYS_PIDFD_OPEN: libc::c_long = 434;
const SYS_CLOSE_RANGE: libc::c_long = 436;
const SYS_OPENAT2: libc::c_long = 437;
const SYS_PIDFD_GETFD: libc::c_long = 438;

const SECCOMP_SET_MODE_FILTER: libc::c_uint = 1;
const SECCOMP_GET_ACTION_AVAIL: libc::c_uint = 2;
const SECCOMP_GET_NOTIF_SIZES: libc::c_uint = 3;

pub const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
pub const RESOLVE_NO_SYMLINKS: u64 = 0x04;
pub const RESOLVE_BENEATH: u64 = 0x08;

/// `SECCOMP_USER_NOTIF_FLAG_CONTINUE`.
pub const NOTIF_FLAG_CONTINUE: u32 = 1;

/// `_IOC(dir, '!', nr, size)` (asm-generic encoding, used by both arches).
pub const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'!' as u64) << 8) | nr
}
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;
pub const IOCTL_NOTIF_RECV: u64 = ioc(IOC_READ | IOC_WRITE, 0, std::mem::size_of::<Notif>() as u64);
pub const IOCTL_NOTIF_SEND: u64 = ioc(IOC_READ | IOC_WRITE, 1, std::mem::size_of::<NotifResp>() as u64);
pub const IOCTL_NOTIF_ID_VALID: u64 = ioc(IOC_WRITE, 2, 8);

/// `struct seccomp_notif`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Notif {
    pub id: u64,
    pub pid: u32,
    pub flags: u32,
    pub data: SeccompData,
}

/// `struct seccomp_notif_resp`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NotifResp {
    pub id: u64,
    pub val: i64,
    pub error: i32,
    pub flags: u32,
}

#[repr(C)]
struct SockFprog {
    len: libc::c_ushort,
    filter: *const SockFilter,
}

#[repr(C)]
#[derive(Default)]
struct NotifSizes {
    notif: u16,
    resp: u16,
    data: u16,
}

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn cvt(r: libc::c_long) -> io::Result<libc::c_long> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn owned(r: libc::c_long) -> io::Result<OwnedFd> {
    let fd = cvt(r)?;
    // SAFETY: the kernel just returned this new descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

/// Install `prog` on this thread with `flags`; the listener when
/// `FILTER_FLAG_NEW_LISTENER` is among them.
pub fn seccomp_install(prog: &[SockFilter], flags: u32) -> io::Result<RawFd> {
    let len = libc::c_ushort::try_from(prog.len()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let fprog = SockFprog { len, filter: prog.as_ptr() };
    // SAFETY: fprog and the program it points at outlive the call.
    let r = unsafe { libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, flags, &fprog as *const SockFprog) };
    cvt(r).map(|fd| fd as RawFd)
}

/// Whether the kernel has the seccomp action `action`.
pub fn action_avail(action: u32) -> bool {
    // SAFETY: a pointer to a u32 that outlives the call.
    unsafe { libc::syscall(libc::SYS_seccomp, SECCOMP_GET_ACTION_AVAIL, 0u32, &action as *const u32) == 0 }
}

/// The kernel's `struct seccomp_notif` size.
pub fn notif_size() -> io::Result<usize> {
    let mut s = NotifSizes::default();
    // SAFETY: a sized out-param.
    cvt(unsafe { libc::syscall(libc::SYS_seccomp, SECCOMP_GET_NOTIF_SIZES, 0u32, &mut s as *mut NotifSizes) })?;
    Ok(usize::from(s.notif))
}

pub fn pidfd_open(pid: i32) -> io::Result<OwnedFd> {
    // SAFETY: plain integer arguments.
    owned(unsafe { libc::syscall(SYS_PIDFD_OPEN, pid, 0u32) })
}

pub fn pidfd_getfd(pidfd: &OwnedFd, fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: plain integer arguments.
    owned(unsafe { libc::syscall(SYS_PIDFD_GETFD, pidfd.as_raw_fd(), fd, 0u32) })
}

/// `openat2(dirfd, path, flags, resolve)` (mode 0).
pub fn openat2(dirfd: RawFd, path: &CStr, flags: i32, resolve: u64) -> io::Result<OwnedFd> {
    let how = OpenHow { flags: flags as u32 as u64, mode: 0, resolve };
    // SAFETY: path is NUL-terminated; how outlives the call and its size is given.
    owned(unsafe { libc::syscall(SYS_OPENAT2, dirfd, path.as_ptr(), &how as *const OpenHow, std::mem::size_of::<OpenHow>()) })
}

/// [`openat2`], or where the kernel (or a container's filter) has none
/// (`ENOSYS`), the same rules for `O_PATH` opens by [`walk`]. Only
/// `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS` and `RESOLVE_NO_MAGICLINKS`
/// are understood (the walk follows no magic link: a `/proc` link's text is
/// treated as text).
pub fn open_resolved(dirfd: RawFd, path: &CStr, flags: i32, resolve: u64) -> io::Result<OwnedFd> {
    match openat2(dirfd, path, flags, resolve) {
        Err(e) if e.raw_os_error() == Some(libc::ENOSYS) => walk(dirfd, path.to_bytes(), flags, resolve),
        r => r,
    }
}

fn err(n: i32) -> io::Error {
    io::Error::from_raw_os_error(n)
}

/// One component, never following a link (`O_PATH | O_NOFOLLOW`).
fn open_one(dir: RawFd, name: &[u8]) -> io::Result<OwnedFd> {
    let c = std::ffi::CString::new(name).map_err(|_| err(libc::EINVAL))?;
    // SAFETY: c is NUL-terminated.
    owned(unsafe { libc::openat(dir, c.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) } as libc::c_long)
}

fn link_text(fd: &OwnedFd) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: an empty path reads the link the O_PATH handle is on; buf is sized.
    let n = unsafe { libc::readlinkat(fd.as_raw_fd(), c"".as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(n as usize);
    Ok(buf)
}

/// Resolve `path` from `dirfd` one component at a time, each opened from
/// the handle of the folder before it (so a folder renamed meanwhile
/// changes nothing): `..` goes back to the handle it came from, and with
/// `RESOLVE_BENEATH` never above `dirfd` (`EXDEV`), nor through an absolute
/// link or path; with `RESOLVE_NO_SYMLINKS` any link refuses (`ELOOP`).
pub fn walk(dirfd: RawFd, path: &[u8], flags: i32, resolve: u64) -> io::Result<OwnedFd> {
    use std::collections::VecDeque;
    let beneath = resolve & RESOLVE_BENEATH != 0;
    let no_links = resolve & RESOLVE_NO_SYMLINKS != 0;
    let parts = |p: &[u8]| -> Vec<Vec<u8>> { p.split(|b| *b == b'/').filter(|s| !s.is_empty()).map(<[u8]>::to_vec).collect() };
    let mut cur = if path.first() == Some(&b'/') {
        if beneath {
            return Err(err(libc::EXDEV));
        }
        open_one(libc::AT_FDCWD, b"/")?
    } else {
        open_one(dirfd, b".")?
    };
    let mut up: Vec<OwnedFd> = Vec::new();
    let mut queue: VecDeque<Vec<u8>> = parts(path).into();
    let mut links = 0;
    let mut last: Option<OwnedFd> = None;
    while let Some(name) = queue.pop_front() {
        if last.is_some() {
            return Err(err(libc::ENOTDIR));
        }
        if name == b"." {
            continue;
        }
        if name == b".." {
            match up.pop() {
                Some(parent) => cur = parent,
                None if beneath => return Err(err(libc::EXDEV)),
                None => cur = open_one(cur.as_raw_fd(), b"..")?,
            }
            continue;
        }
        let fd = open_one(cur.as_raw_fd(), &name)?;
        let st = fstat(fd.as_raw_fd())?;
        match st.st_mode & libc::S_IFMT {
            libc::S_IFLNK => {
                links += 1;
                if no_links || links > 40 {
                    return Err(err(libc::ELOOP));
                }
                let t = link_text(&fd)?;
                if t.first() == Some(&b'/') {
                    if beneath {
                        return Err(err(libc::EXDEV));
                    }
                    up.clear();
                    cur = open_one(libc::AT_FDCWD, b"/")?;
                }
                for p in parts(&t).into_iter().rev() {
                    queue.push_front(p);
                }
            }
            libc::S_IFDIR => up.push(std::mem::replace(&mut cur, fd)),
            _ => last = Some(fd),
        }
    }
    let out = last.unwrap_or(cur);
    if flags & libc::O_DIRECTORY != 0 && fstat(out.as_raw_fd())?.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(err(libc::ENOTDIR));
    }
    Ok(out)
}

/// Close every descriptor from `first` up.
pub fn close_from(first: u32) {
    // SAFETY: closes descriptors this process owns; nothing refers to them after.
    let r = unsafe { libc::syscall(SYS_CLOSE_RANGE, first, u32::MAX, 0u32) };
    if r == 0 {
        return;
    }
    // Before 5.9: one by one, from the list.
    let fds: Vec<RawFd> = std::fs::read_dir("/proc/self/fd")
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default();
    for fd in fds.into_iter().filter(|fd| *fd >= first as RawFd) {
        // SAFETY: as above.
        unsafe { libc::close(fd) };
    }
}

/// Receive one notification into a zeroed buffer of the kernel's size.
pub fn notif_recv(listener: RawFd, size: usize) -> io::Result<Notif> {
    let words = size.max(std::mem::size_of::<Notif>()).div_ceil(8);
    let mut buf = vec![0u64; words];
    // SAFETY: the buffer is zeroed, 8-byte aligned and at least the kernel's size.
    cvt(unsafe { libc::ioctl(listener, IOCTL_NOTIF_RECV as _, buf.as_mut_ptr()) } as libc::c_long)?;
    // SAFETY: the buffer holds at least a Notif, aligned for it.
    Ok(unsafe { std::ptr::read(buf.as_ptr() as *const Notif) })
}

pub fn notif_send(listener: RawFd, resp: &NotifResp) -> io::Result<()> {
    let mut r = *resp;
    // SAFETY: a valid response struct.
    cvt(unsafe { libc::ioctl(listener, IOCTL_NOTIF_SEND as _, &mut r as *mut NotifResp) } as libc::c_long).map(|_| ())
}

/// The notification `id` is still pending (its caller still waits).
pub fn notif_id_valid(listener: RawFd, id: u64) -> bool {
    let mut id = id;
    // SAFETY: a pointer to a u64.
    unsafe { libc::ioctl(listener, IOCTL_NOTIF_ID_VALID as _, &mut id as *mut u64) == 0 }
}

/// Copy `len` bytes at `addr` out of process `pid`'s memory, once.
pub fn read_mem(pid: i32, addr: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    if len == 0 {
        return Ok(buf);
    }
    let local = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: len };
    let remote = libc::iovec { iov_base: addr as usize as *mut libc::c_void, iov_len: len };
    // SAFETY: one local iovec over our buffer; the remote one is only read by the kernel.
    let n = unsafe { libc::process_vm_readv(pid, &local, 1, &remote, 1, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != len {
        return Err(io::Error::from_raw_os_error(libc::EFAULT));
    }
    Ok(buf)
}

/// The socket's family (`SO_DOMAIN`).
pub fn so_domain(fd: RawFd) -> io::Result<i32> {
    let mut v: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: a sized out-param.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_DOMAIN, (&mut v as *mut libc::c_int).cast(), &mut len) };
    cvt(r as libc::c_long).map(|_| v)
}

/// `connect(fd, addr)` on a copy the caller can't change; the errno, 0 on
/// success.
pub fn connect_bytes(fd: RawFd, addr: &[u8]) -> i32 {
    // SAFETY: an all-zero sockaddr_storage is valid.
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let n = addr.len().min(std::mem::size_of::<libc::sockaddr_storage>());
    // SAFETY: n is within both buffers.
    unsafe { std::ptr::copy_nonoverlapping(addr.as_ptr(), (&mut ss as *mut libc::sockaddr_storage).cast::<u8>(), n) };
    // SAFETY: ss holds n valid bytes.
    let r = unsafe { libc::connect(fd, (&ss as *const libc::sockaddr_storage).cast(), n as libc::socklen_t) };
    if r == 0 {
        0
    } else {
        io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
    }
}

/// A `sockaddr_un` for `path` (no abstract names), as bytes.
pub fn unix_addr(path: &[u8]) -> Vec<u8> {
    let mut v = (libc::AF_UNIX as u16).to_ne_bytes().to_vec();
    v.extend_from_slice(path);
    v.push(0);
    v
}

pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    // SAFETY: an all-zero stat is valid and fstat fills it.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a sized out-param.
    cvt(unsafe { libc::fstat(fd, &mut st) } as libc::c_long)?;
    Ok(st)
}

/// The process (thread group) a thread `tid` belongs to.
pub fn tgid_of(tid: u32) -> io::Result<i32> {
    let t = std::fs::read_to_string(format!("/proc/{tid}/status"))?;
    t.lines()
        .find_map(|l| l.strip_prefix("Tgid:"))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| io::Error::from_raw_os_error(libc::ESRCH))
}

/// The file description is in blocking mode.
pub fn is_blocking(fd: RawFd) -> bool {
    // SAFETY: F_GETFL on a descriptor we own.
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    fl >= 0 && fl & libc::O_NONBLOCK == 0
}

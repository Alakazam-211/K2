//! deskd's listening socket (macOS): `~/.k2/run/deskd.sock`, 0600, in a
//! 0700 folder this user owns. One connection at a time: an extra one is
//! closed at once with no reply. Each connection's peer is checked
//! (`mac::peer`) before deskd reads or writes a byte; one that fails is
//! closed with no reply.
//!
//! Then: `hello`, one byte carrying the frames pipe's read end
//! (`SCM_RIGHTS`), and requests until the daemon hangs up.

use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use k2_deskd_proto::fdpass::{send_fd, FRAMES_FD_MARKER};
use k2_deskd_proto::wire;

use crate::server::{Deskd, Host};

/// Makes `dir` (and parents) exist, owned by this user, mode 0700.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let md = std::fs::symlink_metadata(dir)?;
    if !md.is_dir() {
        return Err(io::Error::other(format!("{} is not a folder", dir.display())));
    }
    // SAFETY: plain call.
    let me = unsafe { libc::getuid() };
    if md.uid() != me {
        return Err(io::Error::other(format!("{} belongs to uid {}, not {me}", dir.display(), md.uid())));
    }
    if md.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Binds `path` at 0600. A stale socket (nobody answers) is removed; a live
/// one means another deskd is serving, and anything that isn't a socket is
/// left alone: both are errors.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let dir = path.parent().ok_or_else(|| io::Error::other("socket path has no folder"))?;
    prepare_dir(dir)?;
    if let Ok(md) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        if !md.file_type().is_socket() {
            return Err(io::Error::other(format!("{} exists and is not a socket", path.display())));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("another k2-deskd serves {}", path.display())));
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

pub type PeerCheck = dyn Fn(&UnixStream) -> Result<(), String> + Send + Sync;

/// One connection: check, hello, the frames fd, then requests.
pub fn handle_conn<H: Host>(stream: UnixStream, check: &PeerCheck, deskd: &Deskd<H>) -> io::Result<()> {
    if let Err(why) = check(&stream) {
        eprintln!("k2-deskd: refused a connection: {why}");
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return Ok(());
    }
    let (frames_rx, frames_tx) = std::io::pipe()?;
    {
        let mut w = &stream;
        wire::write_json(&mut w, &deskd.hello())?;
        w.flush()?;
    }
    send_fd(&stream, frames_rx.as_raw_fd(), FRAMES_FD_MARKER)?;
    drop(frames_rx); // the daemon holds the only read end now
    deskd.attach_frames(Box::new(frames_tx));
    let reader = stream.try_clone()?;
    let res = deskd.serve(reader, Box::new(stream));
    deskd.detach_frames();
    res
}

/// Accepts for ever on `path`, one connection at a time.
pub fn serve_socket<H: Host>(path: &Path, check: &'static PeerCheck, deskd: &Arc<Deskd<H>>) -> io::Result<()> {
    let listener = bind(path)?;
    serve_listener(listener, check, deskd)
}

pub fn serve_listener<H: Host>(listener: UnixListener, check: &'static PeerCheck, deskd: &Arc<Deskd<H>>) -> io::Result<()> {
    let busy = Arc::new(AtomicBool::new(false));
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if busy.swap(true, Ordering::SeqCst) {
            // One at a time: close the extra one with no reply.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            continue;
        }
        let d = Arc::clone(deskd);
        let b = Arc::clone(&busy);
        std::thread::Builder::new()
            .name("k2-deskd-link".into())
            .spawn(move || {
                if let Err(e) = handle_conn(stream, check, &d) {
                    eprintln!("k2-deskd: link ended: {e}");
                }
                b.store(false, Ordering::SeqCst);
            })
            .map_err(io::Error::other)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hold::LocalHold;
    use crate::inject::tests::Recorder;
    use crate::inject::Injector;
    use k2_deskd_proto::fdpass::recv_fd;
    use k2_deskd_proto::msg::{Display, FramesVia, Tcc};
    use k2_deskd_proto::wire::{Codec, FrameHeader, Message};
    use std::io::Read;
    use std::time::Duration;

    struct H;
    impl Host for H {
        fn os(&self) -> &'static str {
            "test"
        }
        fn backend(&self) -> String {
            "none".into()
        }
        fn frames_via(&self) -> FramesVia {
            FramesVia::Fd
        }
        fn tcc(&self) -> Tcc {
            Tcc::NOT_REQUIRED
        }
        fn request_tcc(&self) -> Tcc {
            Tcc::NOT_REQUIRED
        }
        fn displays(&self) -> Result<Vec<Display>, String> {
            Ok(vec![])
        }
        fn hid_idle_ms(&self) -> Option<u64> {
            None
        }
        fn banner(&self, _t: Option<String>) {}
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        // Short: macOS socket paths are capped at 104 bytes.
        let d = std::env::temp_dir().join(format!("kd{}{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn deskd() -> Arc<Deskd<H>> {
        Arc::new(Deskd::new(Arc::new(H), Injector::new(Box::new(Recorder::default())), Arc::new(LocalHold::default())))
    }

    fn start(dir: &Path, check: &'static PeerCheck) -> (std::path::PathBuf, Arc<Deskd<H>>) {
        let path = dir.join("run").join("deskd.sock");
        let l = bind(&path).unwrap();
        let d = deskd();
        let d2 = Arc::clone(&d);
        std::thread::spawn(move || serve_listener(l, check, &d2));
        (path, d)
    }

    #[test]
    fn socket_and_folder_modes() {
        let dir = tmp("m");
        let (path, _d) = start(&dir, &|_| Ok(()));
        let sm = std::fs::metadata(&path).unwrap().mode() & 0o777;
        let dm = std::fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777;
        assert_eq!((sm, dm), (0o600, 0o700));
        // A second deskd can't take it over while the first one serves.
        assert_eq!(bind(&path).unwrap_err().kind(), io::ErrorKind::AddrInUse);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_refused_peer_gets_no_bytes_at_all() {
        let dir = tmp("r");
        let (path, _d) = start(&dir, &|_| Err("not K2's daemon".into()));
        let mut s = UnixStream::connect(&path).unwrap();
        // Bytes before the check change nothing.
        let _ = s.write_all(b"\0\0\0\x0dJ{\"op\":\"ping\"}");
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let n = s.read_to_end(&mut buf).unwrap_or(0);
        assert_eq!(n, 0, "a refused peer must get nothing, got {buf:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_accepted_peer_gets_hello_then_the_frames_fd_then_replies() {
        let dir = tmp("a");
        let (path, d) = start(&dir, &|_| Ok(()));
        let mut s = UnixStream::connect(&path).unwrap();
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => {
                assert_eq!(v["event"], "hello");
                assert_eq!(v["version"], k2_deskd_proto::PROTOCOL_VERSION);
                assert_eq!(v["frames"], "fd");
            }
            Message::Frame(_) => panic!("frame first"),
        }
        let (marker, fd) = recv_fd(&s).unwrap();
        assert_eq!(marker, FRAMES_FD_MARKER);
        // A frame deskd writes arrives on the passed pipe.
        let h = FrameHeader { codec: Codec::H264, key: true, display: 1, width: 2, height: 2, seq: 1, captured_ns: 5, emitted_ns: 6 };
        let mut frames = std::fs::File::from(fd);
        let d2 = Arc::clone(&d);
        std::thread::spawn(move || d2.send_frame(&h, b"au"));
        match wire::read_message(&mut frames).unwrap().unwrap() {
            Message::Frame(f) => {
                assert_eq!(f.header, h);
                assert_eq!(f.data, b"au");
            }
            Message::Json(_) => panic!("json on the frames pipe"),
        }
        wire::write_json(&mut s, &serde_json::json!({"id": 7, "op": "ping"})).unwrap();
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, serde_json::json!({"id": 7, "ok": true})),
            Message::Frame(_) => panic!(),
        }
        // One at a time: a second client is closed with no reply.
        let mut s2 = UnixStream::connect(&path).unwrap();
        s2.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        assert_eq!(s2.read_to_end(&mut buf).unwrap_or(0), 0);
        // When the daemon hangs up, the frames pipe closes too.
        drop(s);
        let mut rest = Vec::new();
        frames.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_non_socket_at_the_path_is_left_alone() {
        let dir = tmp("n");
        let path = dir.join("run").join("deskd.sock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a socket").unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

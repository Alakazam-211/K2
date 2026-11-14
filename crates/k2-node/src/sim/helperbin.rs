//! The Simulator helper on a node (IN-A10): built here from the source in
//! `k2-node-proto` with this Mac's own Xcode, never shipped by a server.
//! Same cache rule as a Mac daemon's local lane: the name hashes the source
//! and the Xcode version, a sidecar file holds the binary's sha256, and a
//! binary that no longer matches is rebuilt.
//!
//! One helper per booted device, run as the Simulator user: stdin takes
//! one JSON request per line, stdout carries framed replies, and frames
//! come on their own pipe as the child's fd 3 (`--frames-fd 3`).

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use k2_node_proto::crypto::sha256_hex;
use k2_node_proto::sim::HELPER_SOURCE;

use super::exec::Exec;

/// `k2-simhelper-<16 hex of sha256(source ‖ 0 ‖ xcode)>`.
pub fn binary_name(xcode: &str) -> String {
    let mut b = HELPER_SOURCE.as_bytes().to_vec();
    b.push(0);
    b.extend_from_slice(xcode.as_bytes());
    format!("k2-simhelper-{}", &sha256_hex(&b)[..16])
}

/// The helper in `dir` for this Xcode, compiled first if it's missing or
/// doesn't match its recorded hash.
pub fn ensure_built(dir: &Path, exec: &dyn Exec, xcode: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let name = binary_name(xcode);
    let bin = dir.join(&name);
    let marker = dir.join(format!("{name}.sha256"));
    if let (Ok(b), Ok(m)) = (std::fs::read(&bin), std::fs::read_to_string(&marker)) {
        if sha256_hex(&b) == m.trim() {
            return Ok(bin);
        }
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let src = dir.join(format!("{name}.swift"));
    std::fs::write(&src, HELPER_SOURCE).map_err(|e| format!("cannot write the helper source: {e}"))?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let args: Vec<String> = vec!["swiftc".into(), "-O".into(), "-o".into(), tmp.to_string_lossy().into_owned(), src.to_string_lossy().into_owned()];
    let out = exec.run("xcrun", &args, Duration::from_secs(300)).map_err(|e| format!("xcrun swiftc: {e}"))?;
    let _ = std::fs::remove_file(&src);
    if out.status != 0 {
        let _ = std::fs::remove_file(&tmp);
        let tail: Vec<&str> = out.stderr.lines().filter(|l| l.contains("error")).take(3).collect();
        return Err(format!("xcrun swiftc exited {}: {}", out.status, tail.join(" | ")));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| format!("swiftc wrote no binary: {e}"))?;
    std::fs::rename(&tmp, &bin).map_err(|e| format!("cannot place the helper: {e}"))?;
    std::fs::write(&marker, sha256_hex(&bytes)).map_err(|e| format!("cannot write the helper's hash: {e}"))?;
    Ok(bin)
}

/// A running helper's pipes.
pub struct HelperPipes {
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    pub frames: Box<dyn Read + Send>,
    pub kill: Box<dyn FnMut() + Send>,
}

/// Starts helpers (tests play one in memory).
pub trait HelperSpawner: Send + Sync {
    fn spawn(&self, set: &Path, udid: &str) -> Result<HelperPipes, String>;
}

/// The real helper, built for the Mac's Xcode on first use.
pub struct SystemHelpers {
    pub bin_dir: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub exec: std::sync::Arc<dyn Exec>,
}

impl SystemHelpers {
    fn xcode(&self) -> Result<String, String> {
        let o = self.exec.run("xcrun", &["xcodebuild".into(), "-version".into()], Duration::from_secs(60)).map_err(|e| e.to_string())?;
        super::probe::xcode_version(&o.stdout).filter(|_| o.status == 0).ok_or_else(|| "Xcode isn't installed or selected".to_string())
    }

    /// Build (or find) the helper for the selected Xcode.
    pub fn build(&self) -> Result<PathBuf, String> {
        ensure_built(&self.bin_dir, &*self.exec, &self.xcode()?)
    }
}

fn frames_to_fd3(fd: i32) -> std::io::Result<()> {
    // SAFETY: plain fcntl / dup2 on fds this process owns, in the child.
    unsafe {
        if fd == 3 {
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        } else if libc::dup2(fd, 3) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

impl HelperSpawner for SystemHelpers {
    fn spawn(&self, set: &Path, udid: &str) -> Result<HelperPipes, String> {
        let bin = self.build()?;
        let mut cmd = Command::new(&bin);
        cmd.arg("--set")
            .arg(set)
            .arg("--udid")
            .arg(udid)
            .arg("--frames-fd")
            .arg("3")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .current_dir(&self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(d) = std::env::var_os("DEVELOPER_DIR") {
            cmd.env("DEVELOPER_DIR", d);
        }
        let (rx, tx) = std::io::pipe().map_err(|e| format!("frames pipe: {e}"))?;
        let fd = tx.as_raw_fd();
        // SAFETY: the hook only calls `frames_to_fd3` (fcntl / dup2).
        unsafe {
            cmd.pre_exec(move || frames_to_fd3(fd));
        }
        let spawned = cmd.spawn();
        drop(tx);
        let mut child: Child = spawned.map_err(|e| format!("cannot start the helper: {e}"))?;
        let stdin = child.stdin.take().ok_or("no helper stdin")?;
        let stdout = child.stdout.take().ok_or("no helper stdout")?;
        let mut c = Some(child);
        Ok(HelperPipes {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            frames: Box::new(rx),
            kill: Box::new(move || {
                if let Some(mut ch) = c.take() {
                    let _ = ch.kill();
                    let _ = ch.wait();
                }
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::exec::{ExecError, Out};
    use std::sync::Mutex;

    /// "Compiles" by writing the output file, and counts the builds.
    struct FakeSwiftc(Mutex<u32>);
    impl Exec for FakeSwiftc {
        fn run(&self, program: &str, args: &[String], _t: Duration) -> Result<Out, ExecError> {
            assert_eq!((program, args[0].as_str(), args[1].as_str(), args[2].as_str()), ("xcrun", "swiftc", "-O", "-o"));
            let src = std::fs::read_to_string(&args[4]).unwrap();
            assert_eq!(src, HELPER_SOURCE, "the source from k2-node-proto, nothing else");
            std::fs::write(&args[3], b"\xcf\xfa\xed\xfe binary").unwrap();
            *self.0.lock().unwrap() += 1;
            Ok(Out::default())
        }
    }

    #[test]
    fn the_helper_is_built_once_per_xcode_and_rebuilt_when_changed() {
        let d = crate::util::temp_dir("simhelperbin");
        let x = FakeSwiftc(Mutex::new(0));
        let a = ensure_built(&d, &x, "26.6 (17F113)").unwrap();
        assert_eq!(ensure_built(&d, &x, "26.6 (17F113)").unwrap(), a);
        assert_eq!(*x.0.lock().unwrap(), 1, "cached");
        let b = ensure_built(&d, &x, "26.7 (17G1)").unwrap();
        assert_ne!(a, b, "a new Xcode gets its own build");
        std::fs::write(&a, b"tampered").unwrap();
        ensure_built(&d, &x, "26.6 (17F113)").unwrap();
        assert_eq!(*x.0.lock().unwrap(), 3, "a binary that no longer matches its hash is rebuilt");
        assert!(binary_name("26.6").starts_with("k2-simhelper-") && binary_name("26.6").len() == "k2-simhelper-".len() + 16);
    }
}

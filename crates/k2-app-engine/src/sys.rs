//! Small system helpers: atomic writes, child processes with a clean
//! environment, a time limit and an output cap, and `/etc/passwd`.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CHILD_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Temp file in the same dir + fsync + rename. Never follows a link at
/// the target (rename replaces the link itself).
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or("no parent dir")?;
    let name = path.file_name().ok_or("no file name")?.to_string_lossy().to_string();
    let tmp = dir.join(format!(".{name}.k2tmp.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(())
}

/// A finished child.
#[derive(Debug, Clone, Default)]
pub struct Out {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.status == 0 && !self.timed_out
    }
    /// The child's own words for an error line (trimmed, bounded, no
    /// control characters).
    pub fn why(&self) -> String {
        let e = String::from_utf8_lossy(if self.stderr.is_empty() { &self.stdout } else { &self.stderr }).to_string();
        let e: String = e.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
        let e = e.trim();
        let e: String = e.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
        if self.timed_out {
            format!("timed out; {e}")
        } else {
            format!("exit {}: {e}", self.status)
        }
    }
}

/// The engine's environment for every child: cleared, then fixed values
/// (the user manager's runtime dir, `k2app`'s home, C locale).
pub fn base_env(cmd: &mut Command, home: &str, runtime_dir: &str) {
    cmd.env_clear()
        .env("PATH", CHILD_PATH)
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("LC_ALL", "C")
        .env("LANG", "C");
}

/// Run `prog args…` (absolute path), `stdin` piped in from a thread,
/// stdout/stderr each capped at `cap` bytes, killed at `timeout`.
pub fn run(mut cmd: Command, stdin: Option<Vec<u8>>, timeout: Duration, cap: usize) -> Result<Out, String> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let writer = stdin.map(|bytes| {
        let mut sin = child.stdin.take().expect("piped");
        std::thread::spawn(move || {
            let _ = sin.write_all(&bytes);
        })
    });
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let mut chunk = [0u8; 8192];
                loop {
                    match r.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if buf.len() < cap {
                                let take = n.min(cap - buf.len());
                                buf.extend_from_slice(&chunk[..take]);
                            }
                        }
                    }
                }
            }
            buf
        })
    };
    let out_t = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err_t = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(Out { status, stdout: out_t.join().unwrap_or_default(), stderr: err_t.join().unwrap_or_default(), timed_out })
}

/// `(uid, gid, home)` of a user from `/etc/passwd` text.
pub fn passwd_entry(text: &str, user: &str) -> Option<(u32, u32, String)> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() >= 7 && f[0] == user {
            Some((f[2].parse().ok()?, f[3].parse().ok()?, f[5].to_string()))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_lookup() {
        let t = "root:x:0:0:root:/root:/bin/bash\nk2app:x:996:996:K2 Apps:/var/lib/k2app:/usr/sbin/nologin\n";
        assert_eq!(passwd_entry(t, "k2app"), Some((996, 996, "/var/lib/k2app".into())));
        assert_eq!(passwd_entry(t, "k2"), None);
    }

    #[test]
    fn run_caps_times_out_and_pipes_stdin() {
        let mut c = Command::new("/bin/cat");
        base_env(&mut c, "/", "/tmp");
        let o = run(c, Some(b"hello".to_vec()), Duration::from_secs(5), 3).unwrap();
        assert!(o.ok());
        assert_eq!(o.stdout, b"hel");
        let mut c = Command::new("/bin/sleep");
        c.arg("5");
        let o = run(c, None, Duration::from_millis(100), 64).unwrap();
        assert!(o.timed_out && !o.ok());
    }

    #[test]
    fn atomic_write() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.json");
        write_atomic(&p, b"1", 0o600).unwrap();
        write_atomic(&p, b"22", 0o600).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"22");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

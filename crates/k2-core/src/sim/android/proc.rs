//! Bounded tool runs: a timeout, capped output, a clean environment for
//! K2's own SDK (never the person's `ANDROID_*`), and no secrets in argv.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::super::lane::LaneError;

/// The output of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Out {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub truncated: bool,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }
    /// Short, redacted failure text for an error message.
    pub fn failure(&self, what: &str) -> String {
        let mut s = format!("{what} failed (exit {})", self.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()));
        let tail: String = format!("{} {}", self.text().trim(), self.stderr.trim()).chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
        if !tail.trim().is_empty() {
            s.push_str(": ");
            s.push_str(tail.trim());
        }
        crate::redaction::redact(&s)
    }
}

/// Run `program args` with exactly `env` added to a minimal environment,
/// killing it after `timeout`. stdout is capped at `max_out` bytes (the
/// rest is drained and dropped; `truncated` says so).
pub fn run(program: &Path, args: &[&str], env: &HashMap<String, String>, timeout: Duration, max_out: usize) -> Result<Out, LaneError> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    clean_env(&mut cmd, env);
    let mut child = cmd.spawn().map_err(|e| LaneError::tool(format!("start {}: {e}", program.display())))?;
    let mut so = child.stdout.take().ok_or_else(|| LaneError::tool("no stdout"))?;
    let mut se = child.stderr.take().ok_or_else(|| LaneError::tool("no stderr"))?;
    let out_t = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        let mut truncated = false;
        loop {
            match so.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() + n <= max_out {
                        buf.extend_from_slice(&chunk[..n]);
                    } else {
                        let room = max_out.saturating_sub(buf.len());
                        buf.extend_from_slice(&chunk[..room]);
                        truncated = true;
                    }
                }
            }
        }
        (buf, truncated)
    });
    let err_t = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut se).take(64 * 1024).read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).to_string()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(LaneError::tool(format!("wait {}: {e}", program.display()))),
        }
    };
    let (stdout, truncated) = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    match status {
        None => Err(LaneError::timeout(format!(
            "{} {} took longer than {}s",
            program.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            args.first().copied().unwrap_or(""),
            timeout.as_secs()
        ))),
        Some(s) => Ok(Out { code: s.code(), stdout, stderr, truncated }),
    }
}

/// A minimal environment: PATH, HOME, LANG and TMPDIR from the daemon,
/// then `env`. Never the person's `ANDROID_*` / `ADB_*`.
pub fn clean_env(cmd: &mut Command, env: &HashMap<String, String>) {
    cmd.env_clear();
    for k in ["PATH", "HOME", "LANG", "TMPDIR", "USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
}

/// Quote one argument for `adb shell` (the device's sh joins argv into a
/// command line): single quotes, with `'` as `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Is `pid` alive (and not a zombie)?
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) only checks for existence/permission.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if r != 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some(state) = s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().next()) {
                    return state != "Z";
                }
            }
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// The command line of `pid` (Linux `/proc`, else `ps`).
pub fn cmdline(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read(format!("/proc/{pid}/cmdline")).ok().map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let o = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }
}

/// Every pid whose command line contains `needle`.
pub fn pids_matching(needle: &str) -> Vec<u32> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for e in rd.flatten() {
                if let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() {
                    if cmdline(pid).is_some_and(|c| c.contains(needle)) {
                        out.push(pid);
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(o) = Command::new("ps").args(["-axo", "pid=,command="]).output() {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                let l = l.trim();
                if let Some((p, c)) = l.split_once(' ') {
                    if c.contains(needle) {
                        if let Ok(pid) = p.parse() {
                            out.push(pid);
                        }
                    }
                }
            }
        }
    }
    out
}

/// SIGTERM, wait up to `grace`, then SIGKILL.
pub fn terminate(pid: u32, grace: Duration) {
    #[cfg(unix)]
    {
        // SAFETY: plain signals to a pid we started (checked by the caller).
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        let t0 = Instant::now();
        while pid_alive(pid) && t0.elapsed() < grace {
            std::thread::sleep(Duration::from_millis(100));
        }
        if pid_alive(pid) {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_survives_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_quote("x; rm -rf /"), "'x; rm -rf /'");
    }

    #[cfg(unix)]
    #[test]
    fn run_caps_output_and_times_out() {
        let env = HashMap::new();
        let o = run(Path::new("/bin/sh"), &["-c", "yes | head -c 100000"], &env, Duration::from_secs(10), 1000).expect("runs");
        assert!(o.ok());
        assert_eq!(o.stdout.len(), 1000);
        assert!(o.truncated);
        let e = run(Path::new("/bin/sh"), &["-c", "sleep 5"], &env, Duration::from_millis(200), 10).expect_err("times out");
        assert!(matches!(e, LaneError::Timeout { .. }), "{e:?}");
        let o = run(Path::new("/bin/sh"), &["-c", "echo $ANDROID_HOME"], &env, Duration::from_secs(5), 100).expect("runs");
        assert_eq!(o.text().trim(), "", "the person's ANDROID_* never leaks in");
    }
}

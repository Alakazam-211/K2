//! How simhost runs a program: one seam, so the session's rules are tested
//! on any machine with a recording fake, and run on a Mac for real.
//!
//! The real runner gives the child a fixed, small environment (no inherited
//! variables beyond the Developer dir), caps what it reads back, and kills
//! the child at its timeout. Nothing here logs an argument.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use k2_node_proto::sim::ctl::MAX_RUN_OUTPUT;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Out {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    Timeout,
    Failed(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Timeout => write!(f, "timed out"),
            ExecError::Failed(e) => write!(f, "{e}"),
        }
    }
}

pub trait Exec: Send + Sync {
    /// Run `program args` (already allowed by the caller) and wait.
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<Out, ExecError>;
}

/// The Mac's own tools.
pub struct SystemExec {
    pub home: PathBuf,
    pub tmp: PathBuf,
}

/// Read up to `cap` bytes, then drain the rest (so the child never blocks).
fn read_capped(mut r: impl Read, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return out,
            Ok(n) => {
                let room = cap.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

impl Exec for SystemExec {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<Out, ExecError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = std::env::var_os("DEVELOPER_DIR") {
            cmd.env("DEVELOPER_DIR", d);
        }
        let mut child = cmd.spawn().map_err(|e| ExecError::Failed(format!("cannot start {program}: {e}")))?;
        let so = child.stdout.take().map(|o| std::thread::spawn(move || read_capped(o, MAX_RUN_OUTPUT)));
        let se = child.stderr.take().map(|o| std::thread::spawn(move || read_capped(o, MAX_RUN_OUTPUT)));
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ExecError::Timeout);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(ExecError::Failed(format!("wait for {program}: {e}"))),
            }
        };
        let text = |h: Option<std::thread::JoinHandle<Vec<u8>>>| h.and_then(|h| h.join().ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        Ok(Out { status: status.code().unwrap_or(-1), stdout: text(so), stderr: text(se) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_runner_returns_output_and_kills_at_the_timeout() {
        let d = crate::util::temp_dir("simexec");
        let x = SystemExec { home: d.clone(), tmp: d.clone() };
        let o = x.run("/bin/sh", &["-c".into(), "echo out; echo err >&2; exit 3".into()], Duration::from_secs(10)).unwrap();
        assert_eq!(o, Out { status: 3, stdout: "out\n".into(), stderr: "err\n".into() });
        let o = x.run("/usr/bin/env", &[], Duration::from_secs(10)).unwrap();
        let keys: Vec<&str> = o.stdout.lines().filter_map(|l| l.split('=').next()).collect();
        assert!(keys.iter().all(|k| ["PATH", "HOME", "TMPDIR", "DEVELOPER_DIR"].contains(k)), "a small, fixed environment: {keys:?}");
        assert!(o.stdout.contains(&format!("HOME={}", d.display())));
        let t0 = Instant::now();
        assert_eq!(x.run("/bin/sleep", &["30".into()], Duration::from_millis(300)), Err(ExecError::Timeout));
        assert!(t0.elapsed() < Duration::from_secs(10));
    }
}

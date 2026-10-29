//! The host side of the worker (vs-live §3.2–§3.3): spawn `k2-export` for
//! one `describe` or `export`, bound it, read its frames, and say how it
//! ended. The daemon links this; placement, rights, jobs and audit are the
//! daemon's (`docs/export.rs`).
//!
//! **Spawn:** an empty environment (no `K2_*`, no token, no `HOME`), the
//! job folder as cwd, only fds 0/1/2 survive exec (CLOEXEC on the rest).
//!
//! **Limits per OS:**
//!
//! | Limit | Linux | macOS | Windows |
//! |---|---|---|---|
//! | memory | the worker's counting allocator + `RLIMIT_AS` (cap + 256 MiB) | the allocator + a 250 ms watchdog on `ri_phys_footprint` (SIGKILL over the cap; `RLIMIT_AS` isn't enforced on macOS) | the allocator only (no Job Object yet: documented gap) |
//! | time | the wall deadline (kill) + `RLIMIT_CPU` = wall + 5 s | same | the wall deadline (kill) |
//! | files | `RLIMIT_FSIZE` 0, `RLIMIT_NOFILE` 16, `RLIMIT_CORE` 0 | same | — |
//! | processes | `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG` = SIGKILL | the worker exits when its parent changes | — |
//! | output | the host counts `D` bytes: past the cap → kill | same | same |
//!
//! A cancel is a kill. How it ended maps to an error: our watchdog's kill →
//! [`RunError::Limit`] (memory, time, output bytes); SIGXCPU → time; a
//! cancel → [`RunError::Cancelled`]; any other signal, an abort, an exit
//! without a final frame or a protocol breach → [`RunError::Internal`].

use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::proto::{self, Frame, FrameError, Header, Kind};

/// Headroom over the memory cap for `RLIMIT_AS` (address space is more than
/// what is in use: thread stacks, the binary, allocator arenas).
pub const ADDRESS_SPACE_HEADROOM: u64 = 256 << 20;

/// The watchdog's period.
pub const WATCH_EVERY: Duration = Duration::from_millis(250);

/// Most stderr kept per run (for the daemon's debug log only).
pub const STDERR_RING: usize = 64 << 10;

/// How the worker is launched.
#[derive(Debug, Clone)]
pub struct Spawn {
    pub bin: PathBuf,
    /// The job folder (its cwd).
    pub cwd: PathBuf,
}

/// What the host bounds per run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// The wall deadline.
    pub wall: Duration,
    /// The worker's memory cap (the allocator's; `RLIMIT_AS` adds headroom).
    pub memory_bytes: u64,
    /// `D` bytes past this kill the worker.
    pub max_output_bytes: u64,
}

/// The worker's hello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub version: String,
    pub exporters: Vec<String>,
    /// A test build with the hostile exporters (never shipped).
    pub hostile: bool,
}

/// Why a run failed. Nothing it wrote may be kept.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    /// The worker's own final error: the kit's `ErrorReport` JSON.
    Kit(serde_json::Value),
    /// A host limit: `memory`, `time` or `output bytes`.
    Limit(&'static str),
    /// Cancelled (killed on request).
    Cancelled,
    /// A crash, an abort, an exit without a final frame, a protocol breach
    /// or a hello mismatch. `worker` says how it ended (`signal 11`,
    /// `exit 0`, `protocol`).
    Internal { worker: String, message: String },
    /// The worker couldn't be started.
    Spawn(String),
    /// Writing the output failed (the host's side).
    Io(String),
}

/// A clean run: the final `R` payload (JSON) and the `D` bytes written.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub hello: Hello,
    pub report: serde_json::Value,
    pub bytes: u64,
    /// The worker's stderr (debug log only; may quote content).
    pub stderr: String,
}

/// What a run gives back on failure besides the error: the worker's stderr.
#[derive(Debug, Clone, PartialEq)]
pub struct Failed {
    pub error: RunError,
    pub stderr: String,
}

const WHY_NONE: u8 = 0;
const WHY_CANCEL: u8 = 1;
const WHY_TIME: u8 = 2;
const WHY_MEMORY: u8 = 3;
const WHY_OUTPUT: u8 = 4;
const WHY_BREACH: u8 = 5;
const WHY_IO: u8 = 6;

/// `describe`: the targets the worker offers for `kind` (its `R` payload's
/// `targets`), with its hello.
pub fn describe(s: &Spawn, kind: &Kind, bounds: Bounds) -> Result<(Hello, Vec<serde_json::Value>), Failed> {
    let never = AtomicBool::new(false);
    let mut sink = io::sink();
    let done = run(s, &Header::Describe { kind: kind.clone() }, None, bounds, &never, &|_, _| {}, &mut sink)?;
    let targets = done.report.get("targets").and_then(|t| t.as_array()).cloned().ok_or_else(|| Failed {
        error: RunError::Internal { worker: "protocol".into(), message: "describe's report has no targets".into() },
        stderr: done.stderr.clone(),
    })?;
    Ok((done.hello, targets))
}

/// Run one header: the input file's first `size` bytes follow it on stdin
/// (`input`), the `D` bytes go to `out`. `cancel` is polled by the
/// watchdog; `progress` gets each `P` frame.
pub fn run(
    s: &Spawn,
    header: &Header,
    input: Option<(&Path, u64)>,
    bounds: Bounds,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, u64),
    out: &mut dyn Write,
) -> Result<Done, Failed> {
    let mut cmd = Command::new(&s.bin);
    cmd.env_clear()
        .current_dir(&s.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut cmd, bounds);
    let mut child = cmd
        .spawn()
        .map_err(|e| Failed { error: RunError::Spawn(format!("{}: {e}", s.bin.display())), stderr: String::new() })?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let child = Mutex::new(child);
    let why = AtomicU8::new(WHY_NONE);
    let finished = AtomicBool::new(false);
    let mut header_line = header.json().to_string().into_bytes();
    header_line.push(b'\n');
    let deadline = Instant::now() + bounds.wall;

    let kill = |reason: u8| {
        let _ = why.compare_exchange(WHY_NONE, reason, Ordering::AcqRel, Ordering::Acquire);
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
        }
    };

    std::thread::scope(|scope| {
        // stdin: the header, then exactly `size` input bytes, then EOF. A
        // worker that dies first makes this fail (EPIPE); nothing to do.
        if let Some(mut w) = stdin {
            let input = input.map(|(p, n)| (p.to_path_buf(), n));
            let line = header_line.clone();
            scope.spawn(move || {
                if w.write_all(&line).is_err() {
                    return;
                }
                if let Some((path, size)) = input {
                    if let Ok(f) = std::fs::File::open(&path) {
                        let _ = io::copy(&mut f.take(size), &mut w);
                    }
                }
                drop(w);
            });
        }
        // stderr: a ring, for the daemon's debug log.
        let err_thread = scope.spawn(move || {
            let mut ring: Vec<u8> = Vec::new();
            let mut r = stderr;
            let mut buf = [0u8; 8192];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        ring.extend_from_slice(&buf[..n]);
                        if ring.len() > STDERR_RING {
                            let cut = ring.len() - STDERR_RING;
                            ring.drain(..cut);
                        }
                    }
                }
            }
            String::from_utf8_lossy(&ring).into_owned()
        });
        // The watchdog: cancel, the wall deadline, the memory footprint.
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(WATCH_EVERY.min(Duration::from_millis(50)));
                if finished.load(Ordering::Acquire) {
                    break;
                }
                if cancel.load(Ordering::Acquire) {
                    kill(WHY_CANCEL);
                } else if Instant::now() >= deadline {
                    kill(WHY_TIME);
                } else if over_memory(pid, bounds.memory_bytes) {
                    kill(WHY_MEMORY);
                }
            }
        });

        // The frames.
        let mut r = BufReader::with_capacity(256 << 10, stdout);
        let mut hello: Option<Hello> = None;
        let mut fin: Option<Result<Vec<u8>, Vec<u8>>> = None;
        let mut bytes: u64 = 0;
        let mut breach: Option<String> = None;
        let mut io_err: Option<String> = None;
        loop {
            let f = match proto::read_frame(&mut r) {
                Ok(f) => f,
                Err(FrameError::Eof) => break,
                Err(FrameError::Breach(m)) => {
                    breach = Some(m);
                    kill(WHY_BREACH);
                    break;
                }
                Err(FrameError::Io(e)) => {
                    breach = Some(format!("reading the worker: {e}"));
                    kill(WHY_BREACH);
                    break;
                }
            };
            let problem = match (&f, &hello, &fin) {
                (_, _, Some(_)) => Some("a frame after the final one".to_string()),
                (Frame::Hello(p), None, None) => match parse_hello(p) {
                    Ok(h) => {
                        hello = Some(h);
                        None
                    }
                    Err(m) => Some(m),
                },
                (Frame::Hello(_), Some(_), _) => Some("a second hello".to_string()),
                (_, None, _) => Some("the first frame is not a hello".to_string()),
                (Frame::Progress { done, total }, Some(_), None) => {
                    progress(*done, *total);
                    None
                }
                (Frame::Data(d), Some(_), None) => {
                    bytes += d.len() as u64;
                    if bytes > bounds.max_output_bytes {
                        kill(WHY_OUTPUT);
                        break;
                    }
                    if let Err(e) = out.write_all(d) {
                        io_err = Some(e.to_string());
                        kill(WHY_IO);
                        break;
                    }
                    None
                }
                (Frame::Report(p), Some(_), None) => {
                    fin = Some(Ok(p.clone()));
                    None
                }
                (Frame::Error(p), Some(_), None) => {
                    fin = Some(Err(p.clone()));
                    None
                }
            };
            if let Some(m) = problem {
                breach = Some(m);
                kill(WHY_BREACH);
                break;
            }
        }
        drop(r);
        // Wait for the exit (the watchdog still bounds a worker that closed
        // stdout and kept running).
        let status = loop {
            let st = child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten());
            if let Some(st) = st {
                break st;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        finished.store(true, Ordering::Release);
        let stderr = err_thread.join().unwrap_or_default();
        decide(why.load(Ordering::Acquire), status, hello, fin, bytes, breach, io_err, stderr)
    })
}

#[allow(clippy::too_many_arguments)]
fn decide(
    why: u8,
    status: ExitStatus,
    hello: Option<Hello>,
    fin: Option<Result<Vec<u8>, Vec<u8>>>,
    bytes: u64,
    breach: Option<String>,
    io_err: Option<String>,
    stderr: String,
) -> Result<Done, Failed> {
    let fail = |error: RunError| Err(Failed { error, stderr: stderr.clone() });
    match why {
        WHY_CANCEL => return fail(RunError::Cancelled),
        WHY_TIME => return fail(RunError::Limit("time")),
        WHY_MEMORY => return fail(RunError::Limit("memory")),
        WHY_OUTPUT => return fail(RunError::Limit("output bytes")),
        WHY_BREACH => {
            return fail(RunError::Internal {
                worker: "protocol".into(),
                message: breach.unwrap_or_else(|| "protocol breach".into()),
            })
        }
        WHY_IO => return fail(RunError::Io(io_err.unwrap_or_default())),
        _ => {}
    }
    match fin {
        // The worker's own error wins over how it exited (the allocator
        // sends `export_limit` memory, then aborts).
        Some(Err(p)) => match serde_json::from_slice::<serde_json::Value>(&p) {
            Ok(v) if v.get("id").and_then(|i| i.as_str()).is_some() => fail(RunError::Kit(v)),
            _ => fail(RunError::Internal { worker: ended(&status), message: "the error frame is not a report".into() }),
        },
        Some(Ok(p)) if status.code() == Some(proto::EXIT_OK) => match serde_json::from_slice::<serde_json::Value>(&p) {
            Ok(report) => match hello {
                Some(hello) => Ok(Done { hello, report, bytes, stderr }),
                None => fail(RunError::Internal { worker: "protocol".into(), message: "no hello".into() }),
            },
            Err(e) => fail(RunError::Internal { worker: "protocol".into(), message: format!("the report is not JSON: {e}") }),
        },
        Some(Ok(_)) => fail(RunError::Internal { worker: ended(&status), message: "it failed after its report".into() }),
        None if is_sigxcpu(&status) => fail(RunError::Limit("time")),
        None => fail(RunError::Internal { worker: ended(&status), message: "it ended without a final frame".into() }),
    }
}

fn parse_hello(p: &[u8]) -> Result<Hello, String> {
    let v: serde_json::Value = serde_json::from_slice(p).map_err(|e| format!("the hello is not JSON: {e}"))?;
    let proto_v = v.get("proto").and_then(|x| x.as_u64()).ok_or("the hello has no proto")?;
    if proto_v != proto::PROTO {
        return Err(format!("hello mismatch: the worker speaks proto {proto_v}, the daemon {}", proto::PROTO));
    }
    Ok(Hello {
        version: v.get("version").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        exporters: v
            .get("exporters")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        hostile: v.get("hostile").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

/// `signal 11`, `exit 0`.
pub fn ended(st: &ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            return format!("signal {sig}");
        }
    }
    match st.code() {
        Some(c) => format!("exit {c}"),
        None => "ended".to_string(),
    }
}

fn is_sigxcpu(st: &ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        st.signal() == Some(libc::SIGXCPU)
    }
    #[cfg(not(unix))]
    {
        let _ = st;
        false
    }
}

/// macOS: the worker's physical footprint is over `cap` (the watchdog's
/// memory check; Linux has `RLIMIT_AS`, Windows the allocator only).
#[cfg(target_os = "macos")]
fn over_memory(pid: u32, cap: u64) -> bool {
    footprint(pid).is_some_and(|f| f > cap)
}

#[cfg(not(target_os = "macos"))]
fn over_memory(_pid: u32, _cap: u64) -> bool {
    false
}

/// The physical footprint of `pid` (macOS `proc_pid_rusage`, `RUSAGE_INFO_V4`).
#[cfg(target_os = "macos")]
pub fn footprint(pid: u32) -> Option<u64> {
    // SAFETY: rusage_info_v4 is plain data; proc_pid_rusage fills it.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        );
        (rc == 0).then_some(info.ri_phys_footprint)
    }
}

#[cfg(unix)]
fn configure(cmd: &mut Command, bounds: Bounds) {
    use std::os::unix::process::CommandExt;
    let cpu_secs = bounds.wall.as_secs() + 5;
    let memory = bounds.memory_bytes;
    // SAFETY: the closure runs in the child between fork and exec and calls
    // only async-signal-safe functions (setrlimit, prctl, syscall, fcntl).
    unsafe {
        cmd.pre_exec(move || {
            child_limits(cpu_secs, memory);
            Ok(())
        });
    }
}

#[cfg(windows)]
fn configure(cmd: &mut Command, _bounds: Bounds) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn configure(_cmd: &mut Command, _bounds: Bounds) {}

/// In the child, before exec. Failures are ignored: the watchdog and the
/// allocator still bound it.
#[cfg(unix)]
unsafe fn child_limits(cpu_secs: u64, memory: u64) {
    macro_rules! limit {
        ($res:expr, $soft:expr, $hard:expr) => {{
            let rl = libc::rlimit { rlim_cur: $soft as libc::rlim_t, rlim_max: $hard as libc::rlim_t };
            libc::setrlimit($res, &rl);
        }};
    }
    // SIGXCPU at the soft limit, SIGKILL one second later.
    limit!(libc::RLIMIT_CPU, cpu_secs, cpu_secs + 1);
    // It writes no files (pipes aren't affected).
    limit!(libc::RLIMIT_FSIZE, 0, 0);
    limit!(libc::RLIMIT_CORE, 0, 0);
    limit!(libc::RLIMIT_NOFILE, 16, 16);
    #[cfg(target_os = "linux")]
    {
        let as_cap = memory.saturating_add(ADDRESS_SPACE_HEADROOM);
        limit!(libc::RLIMIT_AS, as_cap, as_cap);
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
        // Everything past stdio closes at exec (CLOSE_RANGE_CLOEXEC, 5.11+;
        // an older kernel leaves the daemon's own CLOEXEC flags).
        const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
        libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, CLOSE_RANGE_CLOEXEC);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = memory;
        for fd in 3..4096 {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_with_another_proto_is_a_mismatch() {
        let h = serde_json::json!({ "proto": 99, "version": "x", "exporters": [] }).to_string();
        assert!(parse_hello(h.as_bytes()).expect_err("mismatch").contains("hello mismatch"));
        let ok = serde_json::json!({ "proto": proto::PROTO, "version": "0.1.0", "exporters": ["a"] }).to_string();
        assert_eq!(parse_hello(ok.as_bytes()).expect("hello").exporters, vec!["a".to_string()]);
    }
}

//! The host side: start one `k2-deckrender` per render, bound it, read its
//! response, and say how it ended as a [`Failure`] with its [`FailureCode`].
//! Nothing in the daemon calls this yet (the preview cache is K2's next step).
//!
//! **Spawn:** `posix_spawn` in a session of its own (`POSIX_SPAWN_SETSID`,
//! [`crate::spawn`]: about 1 ms whatever the host's size, where fork + a hook
//! cost 36–54 ms from a ~1 GB daemon; std's `Command` on Windows), an empty environment (no `K2_*`, no token, no `HOME`: nothing
//! inherited; macOS itself adds `__CF_USER_TEXT_ENCODING`), a fresh private temp folder as its cwd (removed afterwards,
//! whatever happened), stdin/stdout/stderr piped. The limits go on argv
//! ([`SelfLimits`]): the worker puts them on itself, and closes every fd past
//! stdio, as the very first thing in `main`, before it reads a byte of its
//! input ([`crate::sandbox`] has the order); a step that fails is a
//! `renderer_error` with the input unread. The wall deadline and the memory
//! watchdog stay here.
//!
//! **Limits per OS:**
//!
//! | Limit | Linux | macOS | Windows |
//! |---|---|---|---|
//! | memory | the worker's counting allocator (a `too_large` line, then abort) + `RLIMIT_AS` (cap + 512 MiB headroom) + the watchdog on RSS (`/proc/<pid>/statm`, every 50 ms; SIGKILL over the cap) | the allocator + the watchdog on the physical footprint (`proc_pid_rusage`; `RLIMIT_AS` isn't enforced on macOS) | the allocator + the worker's Job Object: `JOB_OBJECT_LIMIT_PROCESS_MEMORY` = the `RLIMIT_AS` number (native allocations count; a commit past it fails, the job's memory-limit message → the watchdog kills) |
//! | time | the wall deadline (SIGKILL) + `RLIMIT_CPU` = wall × max(1, threads) + 5 s (SIGXCPU) | same | the wall deadline (kill) + the Job Object's per-process user-mode CPU time, capped at the `RLIMIT_CPU` number (on unix that counts user + system time; Windows ends it; reported as time) |
//! | files | `RLIMIT_FSIZE` 0 (it writes no file), `RLIMIT_NOFILE` 64, `RLIMIT_CORE` 0 | same | — |
//! | network | a seccomp filter: `socket(2)` and `io_uring_setup(2)` fail with `EACCES` (and any syscall of another ABI, x32 included, with `EPERM`) | the worker puts itself in the `no-network` sandbox profile (`sandbox_init`) before it reads its input; best effort: a failure is logged to stderr and it goes on (it fails when the host itself already runs in a sandbox, as K2 Compute's macOS jobs do) | none yet (a documented gap: a Job Object can't close the network; AppContainer is the next step) |
//! | processes | `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG` = SIGKILL (then the parent checked against the host's pid on argv) | the parent checked against the host's pid, then the worker exits when its parent changes | the Job Object: one process (`ACTIVE_PROCESS_LIMIT` = 1: no children), `KILL_ON_JOB_CLOSE` (it dies with the host), `DIE_ON_UNHANDLED_EXCEPTION` (a crash ends at once, no dialog) |
//! | output | the response line at most [`MAX_RESPONSE_HEADER`], the image at most [`Limits::max_output_bytes`]; past either: kill | same | same |
//!
//! How it ended: the worker's own failure line wins (the allocator writes
//! `too_large` before it aborts); then our watchdog's reason (time →
//! `timeout`, memory or output → `too_large`); SIGXCPU → `timeout`; anything
//! else without a response (a crash, an abort, an early exit) or a breach of
//! the protocol → `renderer_error`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::proto::{FailureCode, Header, ImageFormat, Meta, ResponseHeader, SlideRef, EXIT_OK, MAX_RESPONSE_HEADER, PROTO};
use crate::sandbox::SelfLimits;

/// Headroom over the memory cap for `RLIMIT_AS` (address space is more than
/// what is in use: thread stacks, the binary, allocator arenas).
pub const ADDRESS_SPACE_HEADROOM: u64 = 512 << 20;

/// The watchdog's period.
pub const WATCH_EVERY: Duration = Duration::from_millis(50);

/// Most stderr kept per run (debug logs only: it may quote the file).
pub const STDERR_RING: usize = 64 << 10;

/// What the host bounds per render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The wall deadline.
    pub wall: Duration,
    /// The worker's memory cap (its allocator's; the OS limits add headroom).
    pub memory_bytes: u64,
    /// Largest `.pptx` accepted (checked before the worker starts).
    pub max_input_bytes: u64,
    /// Largest image accepted.
    pub max_output_bytes: u64,
    /// Longest image side, in pixels (the width asked for, and the height).
    pub max_side: u32,
    /// Most pixels in the image.
    pub max_pixels: u64,
    /// Render threads (0: the worker's main thread).
    pub threads: u16,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            wall: Duration::from_secs(20),
            memory_bytes: 1 << 30,
            max_input_bytes: 256 << 20,
            max_output_bytes: 256 << 20,
            max_side: 8192,
            max_pixels: 32 << 20,
            threads: 0,
        }
    }
}

/// One render.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub pptx: &'a [u8],
    pub slide: SlideRef,
    /// The image's width in pixels.
    pub width: u32,
    /// WebP lossless (the default) or PNG.
    pub format: ImageFormat,
    /// The text every date field shows (the cached field text); `None`: the file's own.
    pub date_text: Option<&'a str>,
}

impl<'a> Request<'a> {
    /// Slide `slide` at `width` px, WebP, the file's own date text.
    pub fn new(pptx: &'a [u8], slide: SlideRef, width: u32) -> Request<'a> {
        Request { pptx, slide, width, format: ImageFormat::Webp, date_text: None }
    }
}

/// A drawn slide.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The image, flattened onto white, in `format`.
    pub image: Vec<u8>,
    pub format: ImageFormat,
    pub meta: Meta,
    /// The worker's stderr (debug log only; may quote content).
    pub stderr: String,
}

/// A failed render. Nothing it wrote may be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: FailureCode,
    pub detail: String,
    /// The worker's stderr (debug log only; may quote content).
    pub stderr: String,
}

impl Failure {
    fn new(code: FailureCode, detail: impl Into<String>) -> Failure {
        Failure { code, detail: detail.into(), stderr: String::new() }
    }
}

const WHY_NONE: u8 = 0;
const WHY_TIME: u8 = 1;
const WHY_MEMORY: u8 = 2;
const WHY_OUTPUT: u8 = 3;
const WHY_BREACH: u8 = 4;
/// Windows: the Job Object ended the worker at its user-mode CPU time.
const WHY_CPU: u8 = 5;

/// Render `req` with the worker at `bin`, within `limits`.
pub fn render(bin: &Path, req: &Request<'_>, limits: &Limits) -> Result<Rendered, Failure> {
    render_with(bin, req, limits, None)
}

/// [`render`], with the test-only `hostile` field (a `test-hostile` worker
/// does that instead of rendering; any other answers `renderer_error`).
#[doc(hidden)]
pub fn render_with(bin: &Path, req: &Request<'_>, limits: &Limits, hostile: Option<&str>) -> Result<Rendered, Failure> {
    if req.pptx.len() as u64 > limits.max_input_bytes {
        return Err(Failure::new(
            FailureCode::TooLarge,
            format!("the file is {} bytes; at most {}", req.pptx.len(), limits.max_input_bytes),
        ));
    }
    if req.width == 0 {
        return Err(Failure::new(FailureCode::InvalidInput, "the width is 0"));
    }
    if req.width > limits.max_side {
        return Err(Failure::new(FailureCode::TooLarge, format!("the width is {} px; at most {}", req.width, limits.max_side)));
    }
    let header = Header {
        proto: PROTO,
        input_len: req.pptx.len() as u64,
        width: req.width,
        slide: req.slide,
        memory_bytes: limits.memory_bytes,
        max_side: limits.max_side,
        max_pixels: limits.max_pixels,
        threads: limits.threads,
        format: req.format,
        date_text: req.date_text.map(str::to_string),
        hostile: hostile.map(str::to_string),
    };
    let dir = TempDir::new().map_err(|e| Failure::new(FailureCode::RendererError, format!("no temp folder for the worker: {e}")))?;
    run(bin, dir.path(), &header, req.pptx, limits)
}

fn run(bin: &Path, cwd: &Path, header: &Header, input: &[u8], limits: &Limits) -> Result<Rendered, Failure> {
    // An absolute path: std's posix_spawn path wants one (with a cwd set, and
    // with the environment cleared, a bare name or a relative path falls back to fork).
    let bin = if bin.is_absolute() { bin.to_path_buf() } else { std::env::current_dir().map(|d| d.join(bin)).unwrap_or_else(|_| bin.to_path_buf()) };
    let mut cmd = Command::new(&bin);
    cmd.env_clear().current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    configure(&mut cmd, limits);
    // Unix: posix_spawn in the worker's own session (never the daemon's).
    #[cfg(unix)]
    let spawned = crate::spawn::spawn(&cmd);
    #[cfg(not(unix))]
    let spawned = cmd.spawn();
    let mut child = spawned.map_err(|e| Failure::new(FailureCode::RendererError, format!("could not start {}: {e}", bin.display())))?;
    // Windows: the worker's Job Object, before a byte of its input is written.
    #[cfg(windows)]
    let job = match k2_winsec::job::WorkerJob::contain(&child, job_limits(limits)) {
        Ok(j) => j,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::new(FailureCode::RendererError, format!("could not put the worker in its job: {e}")));
        }
    };
    let pid = child.id();
    let stdin = child.stdin.take();
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::new(FailureCode::RendererError, "the worker's pipes are missing"));
    };
    let child = Mutex::new(child);
    let why = AtomicU8::new(WHY_NONE);
    let finished = AtomicBool::new(false);
    let deadline = Instant::now() + limits.wall;
    let header_line = header.line();
    let memory_cap = limits.memory_bytes;

    let kill = |reason: u8| {
        let _ = why.compare_exchange(WHY_NONE, reason, Ordering::AcqRel, Ordering::Acquire);
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
        }
    };

    std::thread::scope(|scope| {
        // stdin: the header, the bytes, EOF. A worker that dies first makes
        // this fail (EPIPE); nothing to do.
        if let Some(mut w) = stdin {
            scope.spawn(move || {
                if w.write_all(&header_line).is_ok() {
                    let _ = w.write_all(input);
                }
                drop(w);
            });
        }
        // stderr: a ring.
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
        // The watchdog: the wall deadline and the memory in use. It parks between checks and is
        // woken when the worker has exited, so a render never waits out its period.
        let watchdog = scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                std::thread::park_timeout(WATCH_EVERY);
                if finished.load(Ordering::Acquire) {
                    break;
                }
                if Instant::now() >= deadline {
                    kill(WHY_TIME);
                } else if memory_cap > 0 && resident(pid).is_some_and(|m| m > memory_cap) {
                    kill(WHY_MEMORY);
                }
                // Windows: a commit the job refused (native allocations too).
                #[cfg(windows)]
                if job.hit_memory() {
                    kill(WHY_MEMORY);
                }
            }
        });

        // The response.
        let mut r = BufReader::with_capacity(256 << 10, stdout);
        let mut response: Option<ResponseHeader> = None;
        let mut image: Vec<u8> = Vec::new();
        let mut breach: Option<String> = None;
        match read_line(&mut r, MAX_RESPONSE_HEADER) {
            Ok(None) => {}
            Ok(Some(line)) => match ResponseHeader::parse(&line) {
                Ok(h) => response = Some(h),
                Err(m) => breach = Some(m),
            },
            Err(m) => breach = Some(m),
        }
        if let Some(h) = &response {
            if let Some(len) = h.image_len {
                if len > limits.max_output_bytes {
                    kill(WHY_OUTPUT);
                } else {
                    // At most `len` bytes, read as they come (never reserved up front).
                    match (&mut r).take(len).read_to_end(&mut image) {
                        Ok(n) if n as u64 == len => {}
                        Ok(n) => breach = Some(format!("the image ended after {n} of {len} bytes")),
                        Err(e) => breach = Some(format!("reading the image: {e}")),
                    }
                }
            }
            if breach.is_none() && why.load(Ordering::Acquire) == WHY_NONE {
                let mut extra = [0u8; 1];
                if matches!(r.read(&mut extra), Ok(n) if n > 0) {
                    breach = Some("bytes after the response".into());
                }
            }
        }
        if breach.is_some() {
            kill(WHY_BREACH);
        }
        drop(r);
        // The exit (the watchdog still bounds a worker that closed stdout and
        // kept running).
        let status = loop {
            let st = child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten());
            if let Some(st) = st {
                break st;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        finished.store(true, Ordering::Release);
        watchdog.thread().unpark();
        // Windows: the job's own limits, when nothing else said why.
        #[cfg(windows)]
        if why.load(Ordering::Acquire) == WHY_NONE {
            job.settle();
            if job.hit_memory() {
                why.store(WHY_MEMORY, Ordering::Release);
            } else if job.hit_cpu() {
                why.store(WHY_CPU, Ordering::Release);
            }
        }
        let stderr = err_thread.join().unwrap_or_default();
        decide(why.load(Ordering::Acquire), status, response, image, breach, limits, stderr)
    })
}

/// One line, without its newline; `None` at EOF before any byte. Longer than
/// `max`, or EOF inside the line: a breach.
fn read_line(r: &mut impl BufRead, max: usize) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    let n = r.take(max as u64 + 1).read_until(b'\n', &mut line).map_err(|e| format!("reading the worker: {e}"))?;
    if n == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(if line.len() > max { "the response line is too long".into() } else { "the response line ended early".into() });
    }
    line.pop();
    Ok(Some(line))
}

fn decide(
    why: u8,
    status: ExitStatus,
    response: Option<ResponseHeader>,
    image: Vec<u8>,
    breach: Option<String>,
    limits: &Limits,
    stderr: String,
) -> Result<Rendered, Failure> {
    let fail = |code: FailureCode, detail: String| Err(Failure { code, detail, stderr: stderr.clone() });
    // The worker's own failure line wins: the allocator writes `too_large`
    // and aborts, and a kill that came after it doesn't change why.
    if let Some(ResponseHeader { ok: false, code: Some(code), detail, .. }) = &response {
        return fail(*code, detail.clone().unwrap_or_default());
    }
    match why {
        WHY_TIME => return fail(FailureCode::Timeout, format!("still running after {} ms", limits.wall.as_millis())),
        WHY_MEMORY => return fail(FailureCode::TooLarge, format!("memory: over {} MiB", limits.memory_bytes >> 20)),
        WHY_OUTPUT => return fail(FailureCode::TooLarge, format!("the image is over {} bytes", limits.max_output_bytes)),
        WHY_CPU => return fail(FailureCode::Timeout, "the CPU limit (user-mode CPU time)".into()),
        _ => {}
    }
    if let Some(m) = breach {
        return fail(FailureCode::RendererError, format!("protocol: {m} ({})", ended(&status)));
    }
    match response {
        Some(ResponseHeader { ok: true, meta: Some(meta), format, .. }) if status.code() == Some(EXIT_OK) => {
            let format = if format.as_deref() == Some("png") { ImageFormat::Png } else { ImageFormat::Webp };
            Ok(Rendered { image, format, meta, stderr })
        }
        Some(_) => fail(FailureCode::RendererError, format!("the worker failed after its image ({})", ended(&status))),
        None if is_sigxcpu(&status) => fail(FailureCode::Timeout, "the CPU limit".into()),
        None => fail(FailureCode::RendererError, format!("the worker ended without a response ({})", ended(&status))),
    }
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

/// The memory `pid` holds: its physical footprint on macOS, its resident set
/// on Linux, nothing elsewhere.
#[cfg(target_os = "macos")]
pub fn resident(pid: u32) -> Option<u64> {
    // SAFETY: rusage_info_v4 is plain data; proc_pid_rusage fills it.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(pid as libc::c_int, libc::RUSAGE_INFO_V4, &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t);
        (rc == 0).then_some(info.ri_phys_footprint)
    }
}

#[cfg(target_os = "linux")]
pub fn resident(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (page > 0).then(|| pages.saturating_mul(page as u64))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn resident(_pid: u32) -> Option<u64> {
    None
}

/// The worker's argv: the limits it puts on itself before it reads its input
/// ([`SelfLimits`]). No pre-exec hook: [`crate::spawn`] starts it with
/// `posix_spawn` and `POSIX_SPAWN_SETSID` (glibc 2.29+ and macOS, with an
/// absolute path and a cwd), never by forking the host.
fn configure(cmd: &mut Command, limits: &Limits) {
    let cpu_secs = cpu_secs(limits);
    let memory = limits.memory_bytes;
    let address_space = address_space(limits);
    cmd.args(SelfLimits { parent: std::process::id(), cpu_secs, memory_bytes: memory, address_space }.to_args());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// The CPU seconds a render may use: wall × max(1, threads) + 5 s
/// (`RLIMIT_CPU` on unix, the Job Object's user-mode CPU time on Windows).
fn cpu_secs(limits: &Limits) -> u64 {
    limits.wall.as_secs().saturating_mul(u64::from(limits.threads.max(1))).saturating_add(5)
}

/// The memory cap plus headroom (`RLIMIT_AS` on unix, the Job Object's
/// committed-memory cap on Windows); 0: none.
fn address_space(limits: &Limits) -> u64 {
    let memory = limits.memory_bytes;
    if memory > 0 {
        memory.saturating_add(ADDRESS_SPACE_HEADROOM)
    } else {
        0
    }
}

/// Windows: the worker's Job Object limits, the unix caps' numbers.
#[cfg(windows)]
fn job_limits(limits: &Limits) -> k2_winsec::job::JobLimits {
    k2_winsec::job::JobLimits { process_memory: address_space(limits), cpu: Duration::from_secs(cpu_secs(limits)) }
}

/// A fresh private folder under the system temp folder, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<TempDir> {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for _ in 0..16 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let p = base.join(format!("k2-deckrender-{}-{n}-{nanos:08x}", std::process::id()));
            // Only unix sets a mode (0700); Windows keeps the default ACL.
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            match b.create(&p) {
                Ok(()) => return Ok(TempDir(p)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free temp folder name"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_folder_is_private_and_goes_away() {
        let d = TempDir::new().expect("temp");
        let p = d.path().to_path_buf();
        assert!(p.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o700);
        }
        std::fs::write(p.join("x"), b"y").expect("write");
        drop(d);
        assert!(!p.exists());
    }

    #[test]
    fn oversized_inputs_and_bad_widths_fail_before_any_worker_starts() {
        let nowhere = Path::new("/nonexistent/k2-deckrender");
        let big = vec![0u8; 2048];
        let limits = Limits { max_input_bytes: 1024, ..Limits::default() };
        let req = Request::new(&big, SlideRef::Index(0), 100);
        assert_eq!(render(nowhere, &req, &limits).expect_err("too large").code, FailureCode::TooLarge);
        let req = Request::new(b"x", SlideRef::Index(0), 0);
        assert_eq!(render(nowhere, &req, &limits).expect_err("zero").code, FailureCode::InvalidInput);
        let req = Request::new(b"x", SlideRef::Index(0), 100_000);
        assert_eq!(render(nowhere, &req, &limits).expect_err("wide").code, FailureCode::TooLarge);
        let req = Request::new(b"x", SlideRef::Index(0), 100);
        assert_eq!(render(nowhere, &req, &limits).expect_err("no binary").code, FailureCode::RendererError);
    }
}

//! The host side: start one `k2-scenerender` per request, bound it, read its
//! response, and say how it ended as a [`Failure`] with its [`FailureCode`].
//!
//! **Spawn:** `posix_spawn`. The host sets no pre-exec hook, so std starts
//! the worker with `posix_spawn` (no fork: the cost doesn't grow with the
//! daemon's size), with an empty environment (nothing inherited; macOS itself
//! adds `__CF_USER_TEXT_ENCODING`), a fresh private (0700) temp folder as its
//! cwd (removed afterwards, whatever happened) and stdin/stdout/stderr piped.
//! The limits travel on argv ([`crate::sandbox::Args`], from [`Limits`]
//! only, never from the input) and the worker puts them on itself before it
//! reads a byte ([`crate::sandbox`]).
//!
//! **Limits per OS:**
//!
//! | Limit | Linux | macOS | Windows |
//! |---|---|---|---|
//! | memory | the worker's counting allocator (a `too_large` line, then abort) + its up-front estimate + `RLIMIT_AS` (cap + 512 MiB headroom) + the watchdog on RSS (`/proc/<pid>/statm`, every 25 ms; SIGKILL over the cap) | the allocator + the estimate + the watchdog on the physical footprint (`proc_pid_rusage`; `RLIMIT_AS` isn't enforced on macOS) | the allocator and the estimate only (documented gap) |
//! | time | the wall deadline (SIGKILL) + `RLIMIT_CPU` = wall × threads + 5 s (SIGXCPU) | same | the wall deadline (kill) |
//! | files | `RLIMIT_FSIZE` 0, `RLIMIT_NOFILE` 64, `RLIMIT_CORE` 0, inherited fds closed | same | — |
//! | network | seccomp: `socket(2)` and `io_uring_setup(2)` fail with `EACCES` (another ABI: `EPERM`) | the `no-network` sandbox profile (`sandbox_init`), best effort (it fails when the host already runs in a sandbox) | none (documented gap) |
//! | processes | `PR_SET_NO_NEW_PRIVS`, `PR_SET_PDEATHSIG` = SIGKILL, the parent's pid checked | the parent's pid checked at start and watched | — |
//! | output | the response line at most [`MAX_RESPONSE_LINE`], the images (and id images) together at most [`Limits::max_output_bytes`]; past either: kill | same | same |
//!
//! How it ended: the worker's own failure line wins (the allocator writes
//! `too_large` before it aborts); then the watchdog's reason (time →
//! `timeout`, memory or output → `too_large`); SIGXCPU → `timeout`; anything
//! else without a response (a crash, an abort, an early exit) or a breach of
//! the protocol → `renderer_error`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::proto::{Ao, Clay, FailureCode, ImageFormat, Request, Response, Timings, View, ViewMeta, EXIT_OK, MAX_RESPONSE_LINE, PROTO};
use crate::sandbox::{Args, Caps, SelfLimits};
use crate::scene::InputLimits;

/// Headroom over the memory cap for `RLIMIT_AS` (address space is more than
/// what is in use: thread stacks, the binary, allocator arenas).
pub const ADDRESS_SPACE_HEADROOM: u64 = 512 << 20;

/// The watchdog's period.
pub const WATCH_EVERY: Duration = Duration::from_millis(25);

/// Most stderr kept per run (debug logs only).
pub const STDERR_RING: usize = 64 << 10;

/// The widest thumbnail; wider is a still.
pub const THUMBNAIL_MAX_WIDTH: u32 = 640;

/// What the host bounds per request. Every value reaches the worker on argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The wall deadline.
    pub wall: Duration,
    /// The worker's memory cap (its allocator's and estimate's; the OS limits add headroom).
    pub memory_bytes: u64,
    /// Render threads (the output is the same for any count).
    pub threads: u32,
    pub max_width: u32,
    pub max_side: u32,
    pub max_pixels: u64,
    pub max_ssaa: u32,
    pub max_views: u32,
    /// All the images of a request together.
    pub max_output_bytes: u64,
    pub input: InputLimits,
}

impl Limits {
    /// Thumbnails: up to 640 px wide, 1 thread, 5 s, 512 MiB.
    pub fn thumbnail() -> Limits {
        Limits {
            wall: Duration::from_secs(5),
            memory_bytes: 512 << 20,
            threads: 1,
            max_width: THUMBNAIL_MAX_WIDTH,
            max_side: 2048,
            max_pixels: 4 << 20,
            max_ssaa: 4,
            max_views: 64,
            max_output_bytes: 16 << 20,
            input: InputLimits::default(),
        }
    }

    /// Stills: up to 1,920 px wide, 4 threads, 10 s, 1 GiB.
    pub fn still() -> Limits {
        Limits {
            wall: Duration::from_secs(10),
            memory_bytes: 1 << 30,
            threads: 4,
            max_width: 1920,
            max_side: 4096,
            max_pixels: 16 << 20,
            max_ssaa: 2,
            max_views: 16,
            max_output_bytes: 64 << 20,
            input: InputLimits::default(),
        }
    }

    /// Thumbnail limits up to [`THUMBNAIL_MAX_WIDTH`], still limits above.
    pub fn for_width(width: u32) -> Limits {
        if width <= THUMBNAIL_MAX_WIDTH {
            Limits::thumbnail()
        } else {
            Limits::still()
        }
    }

    /// The worker's argv for these limits, under the host `parent`.
    pub fn args(&self, parent: u32) -> Args {
        let wall_secs = self.wall.as_secs() + u64::from(self.wall.subsec_nanos() > 0);
        let threads = self.threads.max(1);
        Args {
            limits: SelfLimits {
                parent,
                cpu_secs: wall_secs.saturating_mul(threads as u64).saturating_add(5),
                memory_bytes: self.memory_bytes,
                address_space: if self.memory_bytes > 0 { self.memory_bytes.saturating_add(ADDRESS_SPACE_HEADROOM) } else { 0 },
            },
            caps: Caps {
                max_width: self.max_width,
                max_side: self.max_side,
                max_pixels: self.max_pixels,
                max_ssaa: self.max_ssaa,
                max_threads: threads,
                max_views: self.max_views,
                max_output_bytes: self.max_output_bytes,
                input: self.input,
            },
        }
    }
}

/// One request: a K2SCENE1 and the views to draw from it.
#[derive(Debug, Clone)]
pub struct SceneRequest<'a> {
    pub scene: &'a [u8],
    pub views: Vec<View>,
    pub format: ImageFormat,
    /// sRGB background; `None`: the current neutral.
    pub background: Option<[u8; 3]>,
    /// RGBA with coverage alpha (straight colour) instead of a background.
    pub transparent: bool,
    /// Samples per axis; `None`: by width.
    pub ssaa: Option<u32>,
    /// Clay shading in two tones by class; `None`: base colours.
    pub clay: Option<Clay>,
    /// Screen-space ambient occlusion; `None`: none.
    pub ao: Option<Ao>,
    /// Each view's id image and table too ([`crate::proto::IdsMeta`]).
    pub ids: bool,
}

impl<'a> SceneRequest<'a> {
    /// `views` of `scene`, WebP, the default background and SSAA.
    pub fn new(scene: &'a [u8], views: Vec<View>) -> SceneRequest<'a> {
        SceneRequest { scene, views, format: ImageFormat::Webp, background: None, transparent: false, ssaa: None, clay: None, ao: None, ids: false }
    }
}

/// One drawn view.
#[derive(Debug, Clone)]
pub struct RenderedView {
    pub meta: ViewMeta,
    pub image: Vec<u8>,
    /// Its id image (PNG, see [`crate::proto::IdsMeta`]) when the request asked for ids; the
    /// table is in `meta.ids`.
    pub ids: Option<Vec<u8>>,
}

/// A drawn request.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub format: ImageFormat,
    /// In the request's order.
    pub views: Vec<RenderedView>,
    /// The scene's objects shown without their modifiers.
    pub objects_without_modifiers: Vec<u32>,
    /// The scene's objects hidden in the file.
    pub objects_hidden: Vec<u32>,
    /// The scene's objects drawn at their base transform.
    pub objects_not_evaluated: Vec<u32>,
    pub timings: Timings,
    pub estimate_bytes: u64,
    /// The worker's stderr (debug log only).
    pub stderr: String,
}

/// A failed request. Nothing it wrote may be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: FailureCode,
    pub detail: String,
    /// The worker's stderr (debug log only).
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

/// Draw `req` with the worker at `bin`, within `limits`.
pub fn render(bin: &Path, req: &SceneRequest<'_>, limits: &Limits) -> Result<Rendered, Failure> {
    render_with(bin, req, limits, None)
}

/// [`render`], with the test-only `hostile` field (a `test-hostile` worker
/// does that instead of rendering; any other answers `renderer_error`).
#[doc(hidden)]
pub fn render_with(bin: &Path, req: &SceneRequest<'_>, limits: &Limits, hostile: Option<&str>) -> Result<Rendered, Failure> {
    if req.scene.len() as u64 > limits.input.max_input_bytes {
        return Err(Failure::new(FailureCode::TooLarge, format!("the scene is {} bytes; at most {}", req.scene.len(), limits.input.max_input_bytes)));
    }
    if req.views.is_empty() {
        return Err(Failure::new(FailureCode::InvalidInput, "no views asked for"));
    }
    if req.transparent && req.background.is_some() {
        return Err(Failure::new(FailureCode::InvalidInput, "a transparent image has no background"));
    }
    if let Some(ao) = &req.ao {
        ao.check().map_err(|m| Failure::new(FailureCode::InvalidInput, m))?;
    }
    if req.views.len() as u64 > limits.max_views as u64 {
        return Err(Failure::new(FailureCode::TooLarge, format!("{} views; at most {}", req.views.len(), limits.max_views)));
    }
    for v in &req.views {
        if v.width == 0 {
            return Err(Failure::new(FailureCode::InvalidInput, "a width is 0"));
        }
        if v.width > limits.max_width {
            return Err(Failure::new(FailureCode::TooLarge, format!("a width is {} px; at most {}", v.width, limits.max_width)));
        }
    }
    let request = Request {
        proto: PROTO,
        input_len: req.scene.len() as u64,
        views: req.views.clone(),
        format: req.format,
        background: req.background,
        transparent: req.transparent,
        clay: req.clay,
        ao: req.ao,
        ids: req.ids,
        ssaa: req.ssaa,
        threads: limits.threads.max(1),
        hostile: hostile.map(str::to_string),
    };
    let dir = TempDir::new().map_err(|e| Failure::new(FailureCode::RendererError, format!("no temp folder for the worker: {e}")))?;
    run(bin, dir.path(), &request, req.scene, limits)
}

fn run(bin: &Path, cwd: &Path, request: &Request, input: &[u8], limits: &Limits) -> Result<Rendered, Failure> {
    let args = limits.args(std::process::id()).to_args();
    // No pre_exec, no uid/gid/groups/pgroup: std spawns with posix_spawn.
    let mut cmd = Command::new(bin);
    cmd.args(&args).env_clear().current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| Failure::new(FailureCode::RendererError, format!("could not start {}: {e}", bin.display())))?;
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
    let line = request.line();
    let memory_cap = limits.memory_bytes;
    let wanted = request.views.len();
    let want_ids = request.ids;

    let kill = |reason: u8| {
        let _ = why.compare_exchange(WHY_NONE, reason, Ordering::AcqRel, Ordering::Acquire);
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
        }
    };

    std::thread::scope(|scope| {
        // stdin: the line, the bytes, EOF. A worker that stops reading or dies
        // makes this fail (EPIPE); nothing to do.
        if let Some(mut w) = stdin {
            let line = &line;
            scope.spawn(move || {
                if w.write_all(line).is_ok() {
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
        // The watchdog: the wall deadline and the memory in use.
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
            }
        });

        let mut r = BufReader::with_capacity(256 << 10, stdout);
        let mut response: Option<Response> = None;
        let mut images: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
        let mut breach: Option<String> = None;
        match read_line(&mut r, MAX_RESPONSE_LINE) {
            Ok(None) => {}
            Ok(Some(l)) => match Response::parse(&l) {
                Ok(h) => response = Some(h),
                Err(m) => breach = Some(m),
            },
            Err(m) => breach = Some(m),
        }
        if let Some(Response { ok: true, views: Some(views), .. }) = &response {
            let total = views.iter().fold(0u64, |a, v| a.saturating_add(v.image_len).saturating_add(v.ids.as_ref().map_or(0, |i| i.image_len)));
            if views.len() != wanted {
                breach = Some(format!("{} images for {wanted} views", views.len()));
            } else if views.iter().any(|v| v.ids.is_some() != want_ids) {
                breach = Some(if want_ids { "a view without its id image".into() } else { "an id image no one asked for".into() });
            } else if total > limits.max_output_bytes {
                kill(WHY_OUTPUT);
            } else {
                for v in views {
                    let Some(img) = read_image(&mut r, v.image_len, &mut breach) else { break };
                    let ids = match &v.ids {
                        Some(m) => match read_image(&mut r, m.image_len, &mut breach) {
                            Some(i) => Some(i),
                            None => break,
                        },
                        None => None,
                    };
                    images.push((img, ids));
                }
            }
        }
        if response.is_some() && breach.is_none() && why.load(Ordering::Acquire) == WHY_NONE {
            let mut extra = [0u8; 1];
            if matches!(r.read(&mut extra), Ok(n) if n > 0) {
                breach = Some("bytes after the response".into());
            }
        }
        if breach.is_some() {
            kill(WHY_BREACH);
        }
        drop(r);
        // The exit (the watchdog still bounds a worker that closed stdout and kept running).
        let status = loop {
            let st = child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten());
            if let Some(st) = st {
                break st;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        finished.store(true, Ordering::Release);
        watchdog.thread().unpark();
        let stderr = err_thread.join().unwrap_or_default();
        decide(why.load(Ordering::Acquire), status, response, images, breach, limits, stderr)
    })
}

/// Exactly `len` bytes, read as they come (never reserved up front); `None`, and the breach
/// set, when they end early or fail.
fn read_image(r: &mut impl Read, len: u64, breach: &mut Option<String>) -> Option<Vec<u8>> {
    let mut img = Vec::new();
    match r.take(len).read_to_end(&mut img) {
        Ok(n) if n as u64 == len => Some(img),
        Ok(n) => {
            *breach = Some(format!("an image ended after {n} of {len} bytes"));
            None
        }
        Err(e) => {
            *breach = Some(format!("reading an image: {e}"));
            None
        }
    }
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
    response: Option<Response>,
    images: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    breach: Option<String>,
    limits: &Limits,
    stderr: String,
) -> Result<Rendered, Failure> {
    let fail = |code: FailureCode, detail: String| Err(Failure { code, detail, stderr: stderr.clone() });
    // The worker's own failure line wins: the allocator writes `too_large`
    // and aborts, and a kill that came after it doesn't change why.
    if let Some(Response { ok: false, code: Some(code), detail, .. }) = &response {
        return fail(*code, detail.clone().unwrap_or_default());
    }
    match why {
        WHY_TIME => return fail(FailureCode::Timeout, format!("still running after {} ms", limits.wall.as_millis())),
        WHY_MEMORY => return fail(FailureCode::TooLarge, format!("memory: over {} MiB", limits.memory_bytes >> 20)),
        WHY_OUTPUT => return fail(FailureCode::TooLarge, format!("the images are over {} bytes", limits.max_output_bytes)),
        _ => {}
    }
    if let Some(m) = breach {
        return fail(FailureCode::RendererError, format!("protocol: {m} ({})", ended(&status)));
    }
    match response {
        Some(Response { ok: true, views: Some(metas), format: Some(format), objects_without_modifiers, objects_hidden, objects_not_evaluated, timings, estimate_bytes, .. })
            if status.code() == Some(EXIT_OK) =>
        {
            Ok(Rendered {
                format,
                views: metas.into_iter().zip(images).map(|(meta, (image, ids))| RenderedView { meta, image, ids }).collect(),
                objects_without_modifiers: objects_without_modifiers.unwrap_or_default(),
                objects_hidden: objects_hidden.unwrap_or_default(),
                objects_not_evaluated: objects_not_evaluated.unwrap_or_default(),
                timings: timings.unwrap_or_default(),
                estimate_bytes: estimate_bytes.unwrap_or(0),
                stderr,
            })
        }
        Some(_) => fail(FailureCode::RendererError, format!("the worker failed after its images ({})", ended(&status))),
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
            let p = base.join(format!("k2-scenerender-{}-{n}-{nanos:08x}", std::process::id()));
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
    use crate::camera::ViewSpec;

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
    fn the_profiles_are_k2s_limits() {
        let t = Limits::thumbnail();
        assert_eq!((t.max_width, t.threads, t.wall, t.memory_bytes), (640, 1, Duration::from_secs(5), 512 << 20));
        let s = Limits::still();
        assert_eq!((s.max_width, s.threads, s.wall, s.memory_bytes), (1920, 4, Duration::from_secs(10), 1 << 30));
        for l in [t, s] {
            assert_eq!(l.input.max_input_bytes, 256 << 20);
            assert_eq!((l.input.max_drawn_triangles, l.input.max_drawn_vertices), (8_000_000, 8_000_000));
        }
        assert_eq!(Limits::for_width(640), t);
        assert_eq!(Limits::for_width(641), s);
        let a = s.args(77);
        assert_eq!(a.limits.cpu_secs, 10 * 4 + 5);
        assert_eq!(a.limits.address_space, (1 << 30) + ADDRESS_SPACE_HEADROOM);
        assert_eq!(Args::parse(&a.to_args()), Ok(a), "argv round-trips");
        let ms = Limits { wall: Duration::from_millis(1500), ..t };
        assert_eq!(ms.args(1).limits.cpu_secs, 2 + 5, "a part second rounds up");
    }

    #[test]
    fn oversized_inputs_and_bad_views_fail_before_any_worker_starts() {
        let nowhere = Path::new("/nonexistent/k2-scenerender");
        let big = vec![0u8; 2048];
        let limits = Limits { input: InputLimits { max_input_bytes: 1024, ..InputLimits::default() }, ..Limits::thumbnail() };
        let one = || vec![View::new(ViewSpec::Auto, 100)];
        assert_eq!(render(nowhere, &SceneRequest::new(&big, one()), &limits).expect_err("big").code, FailureCode::TooLarge);
        assert_eq!(render(nowhere, &SceneRequest::new(b"x", vec![]), &limits).expect_err("none").code, FailureCode::InvalidInput);
        assert_eq!(render(nowhere, &SceneRequest::new(b"x", vec![View::new(ViewSpec::Auto, 0)]), &limits).expect_err("zero").code, FailureCode::InvalidInput);
        assert_eq!(render(nowhere, &SceneRequest::new(b"x", vec![View::new(ViewSpec::Auto, 641)]), &limits).expect_err("wide").code, FailureCode::TooLarge);
        assert_eq!(render(nowhere, &SceneRequest::new(b"x", vec![View::new(ViewSpec::Auto, 8); 65]), &limits).expect_err("many").code, FailureCode::TooLarge);
        let both = SceneRequest { transparent: true, background: Some([0, 0, 0]), ..SceneRequest::new(b"x", one()) };
        assert_eq!(render(nowhere, &both, &limits).expect_err("both").code, FailureCode::InvalidInput);
        let dark = SceneRequest { ao: Some(Ao { strength: 2.0, ..Ao::default() }), ..SceneRequest::new(b"x", one()) };
        assert_eq!(render(nowhere, &dark, &limits).expect_err("ao").code, FailureCode::InvalidInput);
        assert_eq!(render(nowhere, &SceneRequest::new(b"x", one()), &limits).expect_err("no binary").code, FailureCode::RendererError);
    }
}

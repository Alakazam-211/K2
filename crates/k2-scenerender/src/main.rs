//! `k2-scenerender`: the 3D stills renderer, one process per request, started
//! by the host (`k2_scenerender::host`) with `posix_spawn`, an empty
//! environment and its limits on argv. See `k2_scenerender::proto` for the wire.
//!
//! First thing in `main`, before a byte of stdin is read: the argv limits are
//! parsed and put on the process (`k2_scenerender::sandbox`: the parent check
//! and PDEATHSIG, the signal mask, inherited fds, rlimits, NO_NEW_PRIVS, the
//! seccomp no-network filter), the counting allocator gets its cap, and on
//! macOS the `no-network` sandbox profile goes on (best effort) and a thread
//! watches the parent. Then the request line, the header (every bound checked,
//! the memory estimated) and only then the mesh data.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use k2_scenerender::proto::{self, FailureCode, Request, Response, Timings};
use k2_scenerender::render::{self, Failure};
use k2_scenerender::sandbox::{Args, Caps};
use k2_scenerender::scene::Decoder;

struct Counting;

static USED: AtomicU64 = AtomicU64::new(0);
/// 0: no cap yet (before the argv limits).
static CAP: AtomicU64 = AtomicU64::new(0);
static LINE_PTR: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static LINE_LEN: AtomicUsize = AtomicUsize::new(0);
/// Windows: stdout's handle, taken with the cap so [`Counting::die`] needn't
/// allocate to reach it.
#[cfg(windows)]
static OUT_HANDLE: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

impl Counting {
    fn over(&self, add: usize) -> bool {
        let cap = CAP.load(Ordering::Relaxed);
        let now = USED.fetch_add(add as u64, Ordering::Relaxed) + add as u64;
        if cap != 0 && now > cap {
            USED.fetch_sub(add as u64, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Write the prebuilt `too_large` line (no allocation) and abort.
    fn die(&self) -> ! {
        let p = LINE_PTR.load(Ordering::Acquire);
        let n = LINE_LEN.load(Ordering::Acquire);
        if !p.is_null() {
            #[cfg(unix)]
            // SAFETY: p/n describe a leaked, immutable Vec<u8>; write(2) doesn't allocate.
            unsafe {
                let mut off = 0usize;
                while off < n {
                    let w = libc::write(1, p.add(off) as *const libc::c_void, n - off);
                    if w <= 0 {
                        break;
                    }
                    off += w as usize;
                }
            }
            #[cfg(windows)]
            {
                use std::io::Write as _;
                use std::os::windows::io::FromRawHandle as _;
                let h = OUT_HANDLE.load(Ordering::Acquire);
                if !h.is_null() {
                    // SAFETY: stdout's handle, borrowed (ManuallyDrop never closes
                    // it); p/n describe a leaked, immutable Vec<u8>; a File write is
                    // WriteFile, which doesn't allocate.
                    let mut out = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(h) });
                    let _ = out.write_all(unsafe { std::slice::from_raw_parts(p, n) });
                }
            }
        }
        // Windows: abort goes through Windows Error Reporting, which holds the
        // process for seconds (past a thumbnail's wall); the answer is written,
        // so end now.
        #[cfg(windows)]
        unsafe {
            libc::_exit(3)
        }
        #[cfg(not(windows))]
        std::process::abort()
    }
}

// SAFETY: forwards to System; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if self.over(l.size()) {
            self.die();
        }
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if self.over(l.size()) {
            self.die();
        }
        System.alloc_zeroed(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        USED.fetch_sub(l.size() as u64, Ordering::Relaxed);
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new > l.size() {
            if self.over(new - l.size()) {
                self.die();
            }
        } else {
            USED.fetch_sub((l.size() - new) as u64, Ordering::Relaxed);
        }
        System.realloc(p, l, new)
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn set_memory_cap(cap: u64) {
    let line = Response::failed(FailureCode::TooLarge, format!("memory: the renderer needed more than {} MiB", cap >> 20)).line();
    let line: &'static mut [u8] = Box::leak(line.into_boxed_slice());
    LINE_LEN.store(line.len(), Ordering::Release);
    LINE_PTR.store(line.as_mut_ptr(), Ordering::Release);
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        OUT_HANDLE.store(std::io::stdout().as_raw_handle(), Ordering::Release);
    }
    CAP.store(cap, Ordering::Release);
}

/// The worker exits when its parent changes (Linux has PDEATHSIG too).
fn watch_parent(parent: u32) {
    #[cfg(unix)]
    {
        let _ = std::thread::Builder::new().name("parent".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            // SAFETY: getppid has no preconditions.
            if u32::try_from(unsafe { libc::getppid() }).ok() != Some(parent) {
                std::process::exit(proto::EXIT_FAILED);
            }
        });
    }
    #[cfg(not(unix))]
    let _ = parent;
}

/// macOS: the `no-network` sandbox profile, before any input is read. Best
/// effort: `Err` says why it isn't on (logged; the worker goes on).
#[cfg(target_os = "macos")]
fn no_network() -> Result<(), String> {
    use std::ffi::{c_char, c_int, CStr};
    extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
        fn sandbox_free_error(errorbuf: *mut c_char);
    }
    const SANDBOX_NAMED: u64 = 0x0001;
    // kSBXProfileNoNetwork.
    let profile = c"no-network";
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: a NUL-terminated profile name; err is freed with sandbox_free_error.
    let rc = unsafe { sandbox_init(profile.as_ptr(), SANDBOX_NAMED, &mut err) };
    if rc == 0 {
        return Ok(());
    }
    let why = if err.is_null() {
        format!("sandbox_init returned {rc}")
    } else {
        // SAFETY: sandbox_init set err to a NUL-terminated message.
        let m = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
        // SAFETY: as above.
        unsafe { sandbox_free_error(err) };
        m
    };
    Err(why)
}

#[cfg(not(target_os = "macos"))]
fn no_network() -> Result<(), String> {
    // Linux: the seccomp filter (sandbox.rs). Elsewhere: none (documented gap).
    Ok(())
}

/// A test build carries this; nothing that ships may.
#[cfg(feature = "test-hostile")]
const HOSTILE_MARKER: &str = "K2_SCENERENDER_TEST_HOSTILE_ENABLED";

/// TEST ONLY: the limits in force just before the first byte of stdin was read.
#[cfg(feature = "test-hostile")]
static AT_FIRST_READ: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn respond(out: &mut impl Write, line: &[u8], images: &[&[u8]]) -> i32 {
    let mut r = out.write_all(line);
    for i in images {
        r = r.and_then(|_| out.write_all(i));
    }
    if r.and_then(|_| out.flush()).is_err() {
        return proto::EXIT_FAILED;
    }
    proto::EXIT_OK
}

fn fail(out: &mut impl Write, f: Failure) -> i32 {
    let _ = respond(out, &Response::failed(f.code, f.detail).line(), &[]);
    proto::EXIT_FAILED
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") && args.len() == 1 {
        #[cfg(feature = "test-hostile")]
        let extra = format!(", test-hostile {HOSTILE_MARKER}");
        #[cfg(not(feature = "test-hostile"))]
        let extra = String::new();
        println!("k2-scenerender {} (proto {}{extra})", proto::VERSION, proto::PROTO);
        return;
    }
    let parsed = match Args::parse(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("k2-scenerender: the host runs this worker (stdin/stdout): {e}");
            std::process::exit(proto::EXIT_USAGE);
        }
    };
    // The limits, before anything is read.
    if let Err(e) = parsed.limits.apply() {
        let out = std::io::stdout();
        std::process::exit(fail(&mut out.lock(), Failure::new(FailureCode::RendererError, e.to_string())));
    }
    if parsed.limits.memory_bytes > 0 {
        set_memory_cap(parsed.limits.memory_bytes);
    }
    let sandbox = no_network();
    if let Err(e) = &sandbox {
        eprintln!("k2-scenerender: no network sandbox: {e}");
    }
    if !cfg!(target_os = "linux") {
        watch_parent(parsed.limits.parent);
    }
    let sandbox = match (&sandbox, cfg!(target_os = "macos")) {
        (Ok(()), true) => "on",
        (Err(_), true) => "off",
        _ => "n/a (seccomp on Linux)",
    };
    std::process::exit(run(&parsed.caps, parsed.limits.memory_bytes, sandbox));
}

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

fn run(caps: &Caps, memory: u64, sandbox: &str) -> i32 {
    #[cfg(feature = "test-hostile")]
    let _ = AT_FIRST_READ.set(k2_scenerender::sandbox::observe());
    let t0 = Instant::now();
    let stdin = std::io::stdin();
    let mut input = std::io::BufReader::with_capacity(256 << 10, stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut line = Vec::new();
    match (&mut input).take(proto::MAX_REQUEST_LINE as u64 + 1).read_until(b'\n', &mut line) {
        Ok(_) if line.last() == Some(&b'\n') => {
            line.pop();
        }
        Ok(_) => return fail(&mut out, Failure::new(FailureCode::InvalidInput, "the request line is missing or too long")),
        Err(e) => return fail(&mut out, Failure::new(FailureCode::InvalidInput, format!("reading the request: {e}"))),
    }
    let req = match Request::parse(&line) {
        Ok(r) => r,
        Err(m) => return fail(&mut out, Failure::new(FailureCode::InvalidInput, m)),
    };
    drop(line);
    if let Some(h) = req.hostile.as_deref() {
        return hostile(&mut out, h, sandbox);
    }
    if let Err(f) = render::check_request(&req, caps) {
        return fail(&mut out, f);
    }
    // The input alone must fit before anything of it is read.
    if memory > 0 && req.input_len.saturating_add(16 << 20) > memory {
        return fail(&mut out, Failure::new(FailureCode::TooLarge, format!("memory: a {} byte scene needs more than the {} MiB cap", req.input_len, memory >> 20)));
    }
    let mut dec = Decoder::new((&mut input).take(req.input_len), req.input_len, caps.input);
    let header = match dec.header() {
        Ok(h) => h,
        Err(e) => return fail(&mut out, Failure::new(FailureCode::from_input(&e), e.to_string())),
    };
    let plans = match render::plan(&render::header_counts(&header), &header.objects, &header.cameras, &req, caps) {
        Ok(p) => p,
        Err(f) => return fail(&mut out, f),
    };
    let header_json = dec.header_bytes();
    let data_bytes: u64 = header.meshes.iter().map(k2_scenerender::scene::data_len).sum();
    let est = render::estimate_bytes(data_bytes, header_json, &plans, render::threads(&req, caps), caps.max_output_bytes);
    if memory > 0 && est > memory {
        return fail(&mut out, Failure::new(FailureCode::TooLarge, format!("memory: this render needs about {} MiB, at most {} MiB", est >> 20, memory >> 20)));
    }
    let header_ms = ms(t0);
    let t1 = Instant::now();
    let scene = match dec.data(header) {
        Ok(s) => s,
        Err(e) => return fail(&mut out, Failure::new(FailureCode::from_input(&e), e.to_string())),
    };
    drop(input);
    let data_ms = ms(t1);
    let t2 = Instant::now();
    let o = match render::render_planned(&scene, &plans, &req, caps) {
        Ok(o) => o,
        Err(f) => return fail(&mut out, f),
    };
    drop(scene);
    let resp = Response {
        proto: proto::PROTO,
        ok: true,
        code: None,
        detail: None,
        format: Some(o.format),
        views: Some(o.views.iter().map(|v| v.0.clone()).collect()),
        objects_without_modifiers: Some(o.objects_without_modifiers),
        objects_hidden: Some(o.objects_hidden).filter(|v| !v.is_empty()),
        objects_not_evaluated: Some(o.objects_not_evaluated).filter(|v| !v.is_empty()),
        timings: Some(Timings { header_ms, data_ms, views_ms: ms(t2) }),
        estimate_bytes: Some(est),
    };
    // Each view's image, then its id image when the request asked for ids.
    let mut images: Vec<&[u8]> = Vec::with_capacity(o.views.len() + o.ids.len());
    for (i, v) in o.views.iter().enumerate() {
        images.push(&v.1);
        if let Some(ids) = o.ids.get(i) {
            images.push(ids);
        }
    }
    respond(&mut out, &resp.line(), &images)
}

#[cfg(not(feature = "test-hostile"))]
fn hostile(out: &mut impl Write, _what: &str, _sandbox: &str) -> i32 {
    fail(out, Failure::new(FailureCode::RendererError, "this worker has no test hooks"))
}

/// TEST ONLY: what the limit tests make the worker do.
#[cfg(feature = "test-hostile")]
fn hostile(out: &mut impl Write, what: &str, sandbox: &str) -> i32 {
    fn report(out: &mut impl Write, m: String) -> i32 {
        fail(out, Failure::new(FailureCode::RendererError, m))
    }
    match what {
        "sleep" => {
            std::thread::sleep(std::time::Duration::from_secs(3600));
            0
        }
        "spin" => {
            let mut x = 0u64;
            loop {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        }
        // Through the counting allocator: its too_large line, then abort.
        "alloc" => {
            let mut keep: Vec<Vec<u8>> = Vec::new();
            loop {
                keep.push(vec![1u8; 1 << 20]);
                std::hint::black_box(&keep);
            }
        }
        // Around the allocator (malloc and touch): only the OS limits and the
        // host's watchdog can stop it.
        "alloc_raw" => alloc_raw(out),
        "abort" => std::process::abort(),
        "net" => {
            let r = std::net::UdpSocket::bind("127.0.0.1:0").and_then(|s| s.send_to(b"x", "127.0.0.1:9"));
            let net = match r {
                Ok(_) => "open".to_string(),
                Err(e) => format!("blocked: {e}"),
            };
            report(out, format!("net: {net}; sandbox: {sandbox}"))
        }
        "env" => {
            let names: Vec<String> = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
            let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
            report(out, format!("env: [{}]; cwd: {cwd}", names.join(",")))
        }
        // The limits as they were just before the first byte of stdin was read.
        "limits" => {
            let at = AT_FIRST_READ.get().cloned().unwrap_or_default();
            report(out, format!("limits: {at}; sandbox: {sandbox}"))
        }
        // A file write: RLIMIT_FSIZE 0 (and the cwd is the host's private folder).
        "write" => {
            // SIGXFSZ ignored, so the write reports EFBIG instead of killing the worker.
            #[cfg(unix)]
            // SAFETY: plain signal(2) call.
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            }
            let r = std::fs::write("probe", b"x");
            report(out, format!("write: {}", match r {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("blocked: {e}"),
            }))
        }
        _ => report(out, format!("unknown hostile mode {what}")),
    }
}

#[cfg(all(feature = "test-hostile", unix))]
fn alloc_raw(_out: &mut impl Write) -> i32 {
    loop {
        // SAFETY: a fresh block of 1 MiB, one volatile write per page (so
        // neither the stores nor the malloc can be optimised away), never freed.
        unsafe {
            let p = std::hint::black_box(libc::malloc(1 << 20) as *mut u8);
            if p.is_null() {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            } else {
                for off in (0..1usize << 20).step_by(4096) {
                    p.add(off).write_volatile(1);
                }
            }
        }
    }
}

#[cfg(all(feature = "test-hostile", not(unix)))]
fn alloc_raw(out: &mut impl Write) -> i32 {
    fail(out, Failure::new(FailureCode::RendererError, "alloc_raw: unix only"))
}

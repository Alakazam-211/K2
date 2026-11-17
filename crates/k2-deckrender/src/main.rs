//! `k2-deckrender`: the `.pptx` slide renderer, one process per render,
//! started by the host (`k2_deckrender::host`) with an empty environment. See
//! `k2_deckrender::proto` for the wire.
//!
//! The host starts it with `posix_spawn` and the limits on argv
//! (`--sandbox parent=… cpu=… memory=… as=…`, [`k2_deckrender::sandbox`]). The
//! worker puts them on itself as the very first thing in `main`, before it
//! reads a byte of its input, in this order:
//!
//! 1. the argv parsed (nothing else is: no env, no stdin);
//! 2. [`SelfLimits::apply`]: `PR_SET_PDEATHSIG` and the parent checked
//!    against the host's pid, the signal mask, inherited fds closed, the
//!    rlimits, `PR_SET_NO_NEW_PRIVS` and the seccomp no-network filter
//!    (Linux);
//! 3. the allocator's cap;
//! 4. macOS: the `no-network` sandbox profile (best effort);
//! 5. the parent watch (exits when the parent changes; macOS has no
//!    parent-death signal);
//! 6. the bundled fonts; then the request is read.
//!
//! A step of 1–3 that fails answers a typed `renderer_error` line and exits,
//! with the input unread.
//!
//! The memory cap lives here, in a counting global allocator: an allocation
//! that would pass the cap writes a prebuilt `too_large` line and aborts.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use k2_deckrender::proto::{self, FailureCode, Header, ResponseHeader};
use k2_deckrender::sandbox::SelfLimits;
use k2_deckrender::worker;

struct Counting;

static USED: AtomicU64 = AtomicU64::new(0);
/// 0: no cap (before [`set_memory_cap`], or a host that set none).
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
    let line = ResponseHeader::failed(FailureCode::TooLarge, format!("memory: the renderer needed more than {} MiB", cap >> 20)).line();
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

/// macOS has no parent-death signal: a thread exits the worker when its
/// parent is no longer the host (whose pid [`SelfLimits::apply`] already
/// checked). Linux also has `PR_SET_PDEATHSIG`.
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
/// effort: `Err` says why it isn't on (the caller logs it and goes on).
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
    // Linux: the seccomp filter (SelfLimits::apply). Elsewhere: none (documented gap).
    Ok(())
}

/// A test build carries this; nothing that ships may.
#[cfg(feature = "test-hostile")]
const HOSTILE_MARKER: &str = "K2_DECKRENDER_TEST_HOSTILE_ENABLED";

fn respond(out: &mut impl Write, line: &[u8], body: &[u8]) -> i32 {
    if out.write_all(line).and_then(|_| out.write_all(body)).and_then(|_| out.flush()).is_err() {
        return proto::EXIT_FAILED;
    }
    proto::EXIT_OK
}

fn fail(out: &mut impl Write, code: FailureCode, detail: impl Into<String>) -> i32 {
    let _ = respond(out, &ResponseHeader::failed(code, detail).line(), &[]);
    proto::EXIT_FAILED
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        #[cfg(feature = "test-hostile")]
        let extra = format!(", test-hostile {HOSTILE_MARKER}");
        #[cfg(not(feature = "test-hostile"))]
        let extra = String::new();
        println!("k2-deckrender {} (proto {}{extra})", proto::VERSION, proto::PROTO);
        return;
    }
    // The limits first: nothing is read before they are on (the input stays
    // unread when a step fails).
    let limits = match SelfLimits::parse(&args) {
        Ok(l) => l,
        Err(m) => {
            eprintln!("k2-deckrender: the host runs this worker (stdin/stdout); the other flag is --version: {m}");
            let _ = fail(&mut std::io::stdout(), FailureCode::RendererError, format!("usage: {m}"));
            std::process::exit(proto::EXIT_USAGE);
        }
    };
    if let Err(e) = limits.apply() {
        std::process::exit(fail(&mut std::io::stdout(), FailureCode::RendererError, e.to_string()));
    }
    if limits.memory_bytes > 0 {
        set_memory_cap(limits.memory_bytes);
    }
    let sandbox = no_network();
    if let Err(e) = &sandbox {
        eprintln!("k2-deckrender: no network sandbox: {e}");
    }
    watch_parent(limits.parent);
    // The bundled fonts, every face loaded, before anything is read or drawn.
    let _ = k2_deckrender::fonts::init();
    let rc = run(
        limits.memory_bytes,
        match (&sandbox, cfg!(target_os = "macos")) {
            (Ok(()), true) => "on",
            (Err(_), true) => "off",
            _ => "n/a (the seccomp filter on Linux)",
        },
    );
    std::process::exit(rc);
}

/// TEST ONLY: the limits in force just before the first byte of input is
/// read (the `limits` hostile mode answers it).
#[cfg(feature = "test-hostile")]
static AT_FIRST_READ: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn run(memory_cap: u64, sandbox: &str) -> i32 {
    #[cfg(feature = "test-hostile")]
    let _ = AT_FIRST_READ.set(format!("{};alloc_cap={}", k2_deckrender::sandbox::observe(), CAP.load(Ordering::Acquire)));
    let stdin = std::io::stdin();
    let mut input = std::io::BufReader::with_capacity(256 << 10, stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut line = Vec::new();
    match (&mut input).take(proto::MAX_HEADER as u64 + 1).read_until(b'\n', &mut line) {
        Ok(_) if line.last() == Some(&b'\n') => {
            line.pop();
        }
        Ok(_) => return fail(&mut out, FailureCode::InvalidInput, "the request header is missing or too long"),
        Err(e) => return fail(&mut out, FailureCode::InvalidInput, format!("reading the request: {e}")),
    }
    let header = match Header::parse(&line) {
        Ok(h) => h,
        Err(m) => return fail(&mut out, FailureCode::InvalidInput, m),
    };
    drop(line);
    // The cap is argv's, set before anything was read; the header's copy must agree.
    if header.memory_bytes != memory_cap {
        return fail(&mut out, FailureCode::RendererError, format!("the header's memory cap {} isn't the worker's {memory_cap}", header.memory_bytes));
    }
    if memory_cap > 0 && header.input_len >= memory_cap {
        return fail(&mut out, FailureCode::TooLarge, format!("the file is {} bytes; the memory cap is {memory_cap}", header.input_len));
    }
    let Ok(len) = usize::try_from(header.input_len) else {
        return fail(&mut out, FailureCode::TooLarge, "the file is too large for this machine");
    };
    let mut bytes = Vec::with_capacity(len);
    match (&mut input).take(header.input_len).read_to_end(&mut bytes) {
        Ok(n) if n == len => {}
        Ok(n) => return fail(&mut out, FailureCode::InvalidInput, format!("the input ended after {n} of {len} bytes")),
        Err(e) => return fail(&mut out, FailureCode::InvalidInput, format!("reading the input: {e}")),
    }
    drop(input);

    if let Some(h) = header.hostile.as_deref() {
        return hostile(&mut out, h, sandbox);
    }

    let opts = worker::Opts {
        max_side: header.max_side,
        max_pixels: header.max_pixels,
        threads: header.threads,
        format: header.format,
        date_text: header.date_text.clone(),
    };
    match worker::render(&bytes, header.slide, header.width, &opts) {
        Ok(o) => {
            drop(bytes);
            let head = ResponseHeader::ok(o.meta, o.format, o.image.len()).line();
            if respond(&mut out, &head, &o.image) == proto::EXIT_OK {
                proto::EXIT_OK
            } else {
                proto::EXIT_FAILED
            }
        }
        Err(f) => fail(&mut out, f.code, f.detail),
    }
}

#[cfg(not(feature = "test-hostile"))]
fn hostile(out: &mut impl Write, _what: &str, _sandbox: &str) -> i32 {
    fail(out, FailureCode::RendererError, "this worker has no test hooks")
}

/// TEST ONLY: what the limit tests make the worker do.
#[cfg(feature = "test-hostile")]
fn hostile(out: &mut impl Write, what: &str, sandbox: &str) -> i32 {
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
            fail(out, FailureCode::RendererError, format!("net: {net}; sandbox: {sandbox}"))
        }
        // The limits in force just before the first input byte was read.
        "limits" => fail(out, FailureCode::RendererError, format!("limits: {}; sandbox: {sandbox}", AT_FIRST_READ.get().map(String::as_str).unwrap_or("none"))),
        "env" => {
            let names: Vec<String> = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
            let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
            fail(out, FailureCode::RendererError, format!("env: [{}]; cwd: {cwd}", names.join(",")))
        }
        _ => fail(out, FailureCode::RendererError, format!("unknown hostile mode {what}")),
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
    fail(out, FailureCode::RendererError, "alloc_raw: unix only")
}

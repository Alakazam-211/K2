//! `k2-deckrender`: the `.pptx` slide renderer, one process per render,
//! started by the host (`k2_deckrender::host`) with an empty environment. See
//! `k2_deckrender::proto` for the wire.
//!
//! The memory cap lives here, in a counting global allocator: an allocation
//! that would pass the cap writes a prebuilt `too_large` line and aborts. The
//! host adds the OS limits (`RLIMIT_AS` on Linux, a footprint watchdog on
//! macOS). On macOS the worker also puts itself in the `no-network` sandbox
//! profile before it reads its input (Linux: the host's seccomp filter).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use k2_deckrender::proto::{self, FailureCode, Header, ResponseHeader};
use k2_deckrender::worker;

struct Counting;

static USED: AtomicU64 = AtomicU64::new(0);
/// 0: no cap yet (before the header).
static CAP: AtomicU64 = AtomicU64::new(0);
static LINE_PTR: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static LINE_LEN: AtomicUsize = AtomicUsize::new(0);

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
        }
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
    CAP.store(cap, Ordering::Release);
}

/// macOS has no parent-death signal: a thread exits the worker when its
/// parent is gone. Linux uses `PR_SET_PDEATHSIG` (the host).
fn watch_parent() {
    #[cfg(unix)]
    {
        // SAFETY: getppid has no preconditions.
        let parent = unsafe { libc::getppid() };
        let _ = std::thread::Builder::new().name("parent".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            // SAFETY: as above.
            if unsafe { libc::getppid() } != parent {
                std::process::exit(proto::EXIT_FAILED);
            }
        });
    }
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
    // Linux: the host's seccomp filter. Elsewhere: none (documented gap).
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
    if !args.is_empty() {
        eprintln!("k2-deckrender: the host runs this worker (stdin/stdout); the only flag is --version");
        std::process::exit(proto::EXIT_USAGE);
    }
    watch_parent();
    let sandbox = no_network();
    if let Err(e) = &sandbox {
        eprintln!("k2-deckrender: no network sandbox: {e}");
    }
    // The bundled fonts, every face loaded, before anything is read or drawn.
    let _ = k2_deckrender::fonts::init();
    let rc = run(match (&sandbox, cfg!(target_os = "macos")) {
        (Ok(()), true) => "on",
        (Err(_), true) => "off",
        _ => "n/a (the host's seccomp filter on Linux)",
    });
    std::process::exit(rc);
}

fn run(sandbox: &str) -> i32 {
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
    if header.memory_bytes > 0 {
        set_memory_cap(header.memory_bytes);
    }
    if header.memory_bytes > 0 && header.input_len >= header.memory_bytes {
        return fail(&mut out, FailureCode::TooLarge, format!("the file is {} bytes; the memory cap is {}", header.input_len, header.memory_bytes));
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

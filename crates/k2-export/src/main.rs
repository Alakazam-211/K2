//! `k2-export`: the export worker (vs-live §3). Spawned per job by the
//! daemon with an empty environment; see `k2_export::worker` for the
//! protocol and `k2_export::host` for how it is run and bounded.
//!
//! The memory cap lives here, in a counting global allocator: an allocation
//! that would pass the cap writes a prebuilt `E` frame (`MPC-E003`,
//! `limit: memory`) and aborts. On Linux the host also sets `RLIMIT_AS`; on
//! macOS (where `RLIMIT_AS` isn't enforced) it watches the footprint.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

struct Counting;

static USED: AtomicU64 = AtomicU64::new(0);
/// 0: no cap yet (before the header).
static CAP: AtomicU64 = AtomicU64::new(0);
static FRAME_PTR: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static FRAME_LEN: AtomicUsize = AtomicUsize::new(0);

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

    /// Write the prebuilt `E` frame (no allocation) and abort.
    fn die(&self) -> ! {
        let p = FRAME_PTR.load(Ordering::Acquire);
        let n = FRAME_LEN.load(Ordering::Acquire);
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

fn set_memory_cap(cap: u64, frame: Vec<u8>) {
    let frame: &'static mut [u8] = Box::leak(frame.into_boxed_slice());
    FRAME_LEN.store(frame.len(), Ordering::Release);
    FRAME_PTR.store(frame.as_mut_ptr(), Ordering::Release);
    CAP.store(cap, Ordering::Release);
}

/// macOS has no parent-death signal: a thread exits the worker when its
/// parent (the daemon) is gone. Linux uses `PR_SET_PDEATHSIG` (the host).
fn watch_parent() {
    #[cfg(unix)]
    {
        // SAFETY: getppid has no preconditions.
        let parent = unsafe { libc::getppid() };
        let _ = std::thread::Builder::new().name("parent".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            // SAFETY: as above.
            if unsafe { libc::getppid() } != parent {
                std::process::exit(k2_export::proto::EXIT_ERROR);
            }
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!(
            "k2-export {} (proto {}{})",
            k2_export::proto::VERSION,
            k2_export::proto::PROTO,
            if cfg!(feature = "test-hostile") { ", test-hostile" } else { "" }
        );
        return;
    }
    if !args.is_empty() {
        eprintln!("k2-export: the daemon runs this worker (stdin/stdout); the only flag is --version");
        std::process::exit(2);
    }
    watch_parent();
    let stdin = std::io::stdin();
    let mut input = std::io::BufReader::with_capacity(256 << 10, stdin.lock());
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let rc = k2_export::worker::run(&mut input, &mut output, &set_memory_cap);
    std::process::exit(rc);
}

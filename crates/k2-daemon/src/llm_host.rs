//! Phase 2 Unit 2 — daemon-side LLM subprocess supervisor.
//!
//! Owns the model path, max-concurrency gate, RSS watchdog, timeout,
//! and crash isolation policy. Inference itself happens in a child
//! process (`k2so-daemon --llm-worker <payload>`) so a Metal/ggml
//! abort never takes the daemon down.
//!
//! # Architectural pillars (from PRD § "Unit 2 — LLM inference to daemon")
//!
//! 1. **Subprocess per request** — mirror the legacy Tauri pattern. A
//!    one-shot worker loads the GGUF, runs one inference pass, prints
//!    the result to stdout, and exits. Tearing the worker down between
//!    requests means a runaway model can never persist live across
//!    multiple calls — the OS reclaims everything when the worker
//!    exits.
//! 2. **Max-concurrency = 1** — only one inference at a time per
//!    daemon. Two simultaneous Metal sessions on the same GPU thrash
//!    or panic. A bounded queue (depth 4) absorbs short bursts; the
//!    5th in-flight gets a `429 Too Many Requests`.
//! 3. **Timeout (60s default)** — if the worker hasn't produced output
//!    after 60s, kill it and return a `504 Gateway Timeout` to the
//!    caller. Prevents pathological models / wedged Metal contexts
//!    from holding the queue forever.
//! 4. **RSS watchdog** — sample the running worker's RSS every 2s via
//!    Darwin `proc_pidinfo PROC_PIDTASKINFO` (same kernel call the
//!    `commands/memory_watcher.rs` uses for this process). If the
//!    worker exceeds the cap (default 3 GB) we send SIGKILL and the
//!    request fails with a clear error. The cap is intentionally
//!    above any sane Qwen2.5-1.5B load (which sits ~1.6 GB) so the
//!    watchdog only fires on pathological cases.
//! 5. **Crash isolation** — the worker can SIGABRT, segfault,
//!    panic, exit non-zero, or get killed externally. None of those
//!    affect the daemon process; the supervisor surfaces a clean
//!    error string to the caller and the next request lazy-respawns
//!    a fresh worker.
//! 6. **Autorestart on next request** — there's no persistent worker
//!    process to "restart". Every request gets a fresh subprocess
//!    that loads the model, runs once, and exits. The model_path
//!    cached here is the contract.
//!
//! # Persistent state
//!
//! Singleton — initialized at daemon boot, lives the daemon's
//! lifetime. Holds:
//! - `model_path` — the currently-loaded model (set by
//!   `load_model`, `download_default_model`, or first-boot auto-pick).
//! - `downloading` — atomic flag so two callers can't kick off
//!   two parallel downloads of the same model.
//! - `inflight` — counter of currently-running workers (0..=1 in
//!   normal operation).
//! - `queued` — counter of requests waiting on the gate. When
//!   `inflight + queued >= MAX_QUEUED`, new requests get 429.

use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use k2_core::log_debug;

/// Default per-request timeout. Worker is SIGKILLed if it hasn't
/// produced output in this time and the caller gets a clean error.
/// Tunable via `K2SO_LLM_TIMEOUT_SECS` env var.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Resident-set-size cap for the worker process. 3 GB leaves head-
/// room above Qwen2.5-1.5B's ~1.6 GB load while still catching
/// pathological models or memory leaks.
/// Tunable via `K2SO_LLM_RSS_MAX_MB` env var.
const DEFAULT_RSS_CAP_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// How often the RSS watchdog polls the worker process. 2s gives
/// us bounded reaction time without busy-looping a syscall.
const RSS_POLL_INTERVAL: Duration = Duration::from_millis(2000);

/// Maximum stdout bytes accepted from the worker. 10 MB protects the
/// daemon against runaway models that dump unbounded output.
const MAX_WORKER_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Max concurrent in-flight requests. Hard cap; second concurrent
/// request blocks on the gate (it's a tokio task waiting for the
/// in-flight counter to fall back to 0).
const MAX_INFLIGHT: u32 = 1;

/// Max queued+inflight. When `inflight + queued >= MAX_QUEUED`,
/// new requests get a 429. 4 = single in-flight + 3 waiting.
const MAX_QUEUED: u32 = 4;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    /// True when a model_path is set AND the file exists on disk.
    /// We don't load the model in-process (Metal lives in the
    /// worker), so "loaded" here means "we have a ready-to-spawn
    /// model file".
    pub loaded: bool,
    /// Currently configured model path, or None.
    pub model_path: Option<String>,
    /// True while a download is in progress.
    pub downloading: bool,
    /// Currently running inferences (0 or 1 in normal operation).
    pub inflight: u32,
    /// Queued (not yet running) inferences.
    pub queued: u32,
}

/// Process-wide singleton.
static SHARED: OnceLock<Arc<LlmHost>> = OnceLock::new();

pub fn shared() -> Arc<LlmHost> {
    SHARED
        .get_or_init(|| Arc::new(LlmHost::new()))
        .clone()
}

pub struct LlmHost {
    inner: Mutex<Inner>,
    inflight: AtomicU32,
    queued: AtomicU32,
    downloading: AtomicBool,
}

struct Inner {
    model_path: Option<String>,
}

impl LlmHost {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner { model_path: None }),
            inflight: AtomicU32::new(0),
            queued: AtomicU32::new(0),
            downloading: AtomicBool::new(false),
        }
    }

    /// Set the active model path. Used by `load_model` and by the
    /// first-boot autodiscovery pass.
    pub fn set_model_path(&self, path: String) {
        self.inner.lock().model_path = Some(path);
    }

    /// Read-only snapshot of the active model path.
    pub fn model_path(&self) -> Option<String> {
        self.inner.lock().model_path.clone()
    }

    pub fn status(&self) -> LlmStatus {
        let path = self.model_path();
        let loaded = match &path {
            Some(p) => std::path::Path::new(p).exists(),
            None => false,
        };
        LlmStatus {
            loaded,
            model_path: path,
            downloading: self.downloading.load(Ordering::Relaxed),
            inflight: self.inflight.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
        }
    }

    #[allow(dead_code)]
    pub fn is_downloading(&self) -> bool {
        self.downloading.load(Ordering::Relaxed)
    }

    /// Try to mark "download in progress". Returns true on success;
    /// false if another download is already running.
    pub fn try_begin_download(&self) -> bool {
        self.downloading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    pub fn end_download(&self) {
        self.downloading.store(false, Ordering::Relaxed);
    }

    /// Synchronous inference call. Spawns a `k2so-daemon --llm-worker`
    /// child, writes a JSON payload to a temp file, reads stdout for
    /// up to `timeout`, and returns the trimmed output.
    ///
    /// Enforces the max-concurrency + queue policy. If `inflight +
    /// queued >= MAX_QUEUED` the call returns `Err(429)` immediately.
    pub fn generate(
        &self,
        system_prompt: &str,
        user_message: &str,
        timeout: Duration,
    ) -> Result<String, GenerateError> {
        // Admission control: queue-depth check happens BEFORE we wait
        // on the inflight gate, so a deeply-saturated daemon fails
        // fast with 429 instead of accumulating an unbounded backlog.
        let total = self.inflight.load(Ordering::Acquire)
            + self.queued.load(Ordering::Acquire);
        if total >= MAX_QUEUED {
            return Err(GenerateError::TooManyRequests(format!(
                "LLM busy: {} requests in flight or queued (max {})",
                total, MAX_QUEUED
            )));
        }

        // Capture the model path BEFORE entering the gate so a missing
        // model fails fast (no wait penalty for a clearly-broken
        // request).
        let model_path = match self.model_path() {
            Some(p) => p,
            None => {
                return Err(GenerateError::NoModel(
                    "No model loaded — call /cli/llm/load-model or \
                     /cli/llm/download-default first"
                        .to_string(),
                ));
            }
        };

        // Tick up the queued counter; tick down once we acquire the
        // inflight slot. This lets the status endpoint report
        // accurate live numbers.
        self.queued.fetch_add(1, Ordering::AcqRel);

        // Busy-wait on the inflight slot. The wait is bounded by the
        // queue admission check above (max 3 waiters) and the per-
        // request timeout below.
        let acquire_deadline = Instant::now() + timeout;
        loop {
            // Try to swap inflight 0→1
            if self
                .inflight
                .compare_exchange(0, MAX_INFLIGHT, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            if Instant::now() > acquire_deadline {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                return Err(GenerateError::Timeout(format!(
                    "Timed out waiting in queue (>{}s)",
                    timeout.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.queued.fetch_sub(1, Ordering::AcqRel);

        // RAII guard that releases the inflight slot on every exit
        // path (Ok, error, or panic).
        let _inflight_guard = InflightGuard {
            counter: &self.inflight,
        };

        let result = run_worker_subprocess(&model_path, system_prompt, user_message, timeout);

        result
    }
}

/// Releases the inflight counter on Drop, even if the worker panics.
struct InflightGuard<'a> {
    counter: &'a AtomicU32,
}
impl<'a> Drop for InflightGuard<'a> {
    fn drop(&mut self) {
        self.counter.store(0, Ordering::Release);
    }
}

/// Cleanup helper for the worker payload temp file.
struct TempFileGuard {
    path: std::path::PathBuf,
}
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// All the failure modes generate() can hand back. Translated by
/// `llm_routes` into appropriate HTTP status codes.
#[derive(Debug)]
pub enum GenerateError {
    /// 429 — admission control rejected the request.
    TooManyRequests(String),
    /// 503 — no model loaded; caller needs to load one first.
    NoModel(String),
    /// 504 — worker exceeded the timeout (or queue wait did).
    Timeout(String),
    /// 502 — worker exited abnormally (signal, abort, panic, OOM).
    /// Caller can safely retry; next request lazy-respawns.
    WorkerCrashed(String),
    /// 500 — anything else (spawn failed, IO error, etc.).
    Internal(String),
}

impl GenerateError {
    pub fn message(&self) -> &str {
        match self {
            Self::TooManyRequests(m)
            | Self::NoModel(m)
            | Self::Timeout(m)
            | Self::WorkerCrashed(m)
            | Self::Internal(m) => m,
        }
    }
}

/// Spawn the worker subprocess, write the payload, watch its RSS,
/// enforce the timeout, capture stdout. Pure function — the
/// concurrency gate has already been entered by the caller.
fn run_worker_subprocess(
    model_path: &str,
    system_prompt: &str,
    user_message: &str,
    timeout: Duration,
) -> Result<String, GenerateError> {
    let exe = std::env::current_exe()
        .map_err(|e| GenerateError::Internal(format!("current_exe: {e}")))?;

    // Payload via temp file so we don't blow argv length limits on
    // long system prompts.
    let tmp = std::env::temp_dir().join(format!(
        "k2so-llm-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let _tmp_guard = TempFileGuard { path: tmp.clone() };
    let payload = serde_json::json!({
        "model": model_path,
        "system": system_prompt,
        "message": user_message,
    });
    std::fs::write(&tmp, payload.to_string())
        .map_err(|e| GenerateError::Internal(format!("write worker payload: {e}")))?;

    let mut child = std::process::Command::new(&exe)
        .args(["--llm-worker", tmp.to_string_lossy().as_ref()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| GenerateError::Internal(format!("spawn worker: {e}")))?;

    let pid = child.id();
    let rss_cap = rss_cap_bytes();
    let start = Instant::now();
    let mut last_rss_sample = Instant::now() - RSS_POLL_INTERVAL;

    // Poll loop: check exit, then watchdog, then timeout. 100 ms
    // sleep matches the cadence Tauri used. Tighter polling buys
    // millisecond-scale latency for short generations but burns CPU.
    let output = loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                break child.wait_with_output().map_err(|e| {
                    GenerateError::Internal(format!("wait_with_output: {e}"))
                })?;
            }
            Ok(None) => {
                // Still running. Apply guards.
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GenerateError::Timeout(format!(
                        "LLM inference timed out after {}s",
                        timeout.as_secs()
                    )));
                }
                if last_rss_sample.elapsed() >= RSS_POLL_INTERVAL {
                    last_rss_sample = Instant::now();
                    if let Ok(rss) = read_process_rss(pid as i32) {
                        if rss > rss_cap {
                            log_debug!(
                                "[llm-host] worker RSS {} bytes exceeds cap {} bytes — killing",
                                rss,
                                rss_cap
                            );
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(GenerateError::WorkerCrashed(format!(
                                "LLM worker killed: RSS {} MB exceeded cap {} MB",
                                rss / (1024 * 1024),
                                rss_cap / (1024 * 1024)
                            )));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(GenerateError::Internal(format!("try_wait: {e}")));
            }
        }
    };

    let exit_code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        if line.contains("Prompt:") || line.contains("tokens") {
            log_debug!("[llm-host/worker] {}", line.trim());
        }
    }

    if output.status.success() {
        if output.stdout.len() > MAX_WORKER_OUTPUT_BYTES {
            return Err(GenerateError::Internal(format!(
                "LLM output too large ({} bytes, max {})",
                output.stdout.len(),
                MAX_WORKER_OUTPUT_BYTES
            )));
        }
        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
        log_debug!("[llm-host/worker] output: {} bytes", result.len());
        if result.is_empty() {
            Err(GenerateError::WorkerCrashed(
                "LLM produced empty output".to_string(),
            ))
        } else {
            Ok(result)
        }
    } else {
        let trimmed = stderr.trim().to_string();
        let last_lines: String = trimmed
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .join(" | ");
        log_debug!(
            "[llm-host/worker] failed: exit={:?} stderr_tail={}",
            exit_code,
            last_lines
        );
        if exit_code.is_none() || trimmed.contains("abort") {
            // Signal kill or abort() — classic Metal abort path.
            Err(GenerateError::WorkerCrashed(format!(
                "LLM crashed (signal). Last stderr: {last_lines}"
            )))
        } else {
            Err(GenerateError::WorkerCrashed(format!(
                "LLM error: {}",
                if trimmed.is_empty() { "unknown" } else { &trimmed }
            )))
        }
    }
}

/// Resolve the worker RSS cap, honoring an env override.
fn rss_cap_bytes() -> u64 {
    if let Ok(mb) = std::env::var("K2SO_LLM_RSS_MAX_MB") {
        if let Ok(n) = mb.parse::<u64>() {
            return n.saturating_mul(1024 * 1024);
        }
    }
    DEFAULT_RSS_CAP_BYTES
}

/// Resolve the per-request timeout, honoring an env override.
pub fn default_timeout() -> Duration {
    let secs = std::env::var("K2SO_LLM_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Read a foreign process's RSS via `proc_pidinfo PROC_PIDTASKINFO`.
/// Mirrors `src-tauri/src/commands/memory_watcher.rs` but on any pid,
/// not just `std::process::id()`. On non-macOS targets this returns
/// 0 — the watchdog never fires and the worker only gets killed by
/// the timeout or by exiting on its own.
#[cfg(target_os = "macos")]
fn read_process_rss(pid: i32) -> Result<u64, String> {
    use libc::{c_int, c_void};
    const PROC_PIDTASKINFO: c_int = 4;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ProcTaskinfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let mut info = ProcTaskinfo::default();
    let size = std::mem::size_of::<ProcTaskinfo>() as c_int;
    let ret = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if ret != size {
        // Worker may have exited between try_wait and the sample —
        // treat as "no reading available" so the watchdog doesn't
        // SIGKILL a dead process.
        return Err(format!("proc_pidinfo returned {ret} (expected {size})"));
    }
    Ok(info.pti_resident_size)
}

#[cfg(not(target_os = "macos"))]
fn read_process_rss(_pid: i32) -> Result<u64, String> {
    Ok(0)
}

/// Entry point invoked when the daemon is launched with
/// `--llm-worker <payload_path>`. Loads the model, runs one
/// inference, prints to stdout, exits via `_exit` to skip C++
/// static destructors (ggml_metal's static cleanup races macOS
/// Metal device teardown if normal exit runs).
///
/// Mirrors `src-tauri/src/lib.rs::llm_worker_main` byte-for-byte.
pub fn worker_main(payload_path: &str) -> ! {
    let raw = match std::fs::read_to_string(payload_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to read payload: {e}");
            std::process::exit(1);
        }
    };

    let payload: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to parse payload: {e}");
            std::process::exit(1);
        }
    };

    let model_path = payload["model"].as_str().unwrap_or("");
    let system_prompt = payload["system"].as_str().unwrap_or("");
    let user_message = payload["message"].as_str().unwrap_or("");

    let mut manager = k2_core::llm::LlmManager::new();
    if let Err(e) = manager.load_model(model_path) {
        eprintln!("Failed to load model: {e}");
        std::process::exit(1);
    }

    match manager.generate(system_prompt, user_message) {
        Ok(output) => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(output.as_bytes());
            let _ = std::io::stdout().flush();
            // Force-exit to skip ggml_metal static destructors that
            // race macOS Metal device teardown.
            unsafe { libc::_exit(0); }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// First-boot model-discovery pass. Mirrors the pre-Phase-2 Tauri
/// behavior in `src-tauri/src/lib.rs` (see the `auto-download AI
/// model` block, ~line 933):
///
///   1. Clear any stale `.tmp` files from interrupted downloads.
///   2. If the default model file exists at `~/.k2so/models/...`,
///      cache its path on the host.
///   3. Otherwise kick off the download in a background thread.
///      When it completes, cache the path.
///
/// Errors are logged and skipped — the daemon must boot whether or
/// not the LLM is reachable. The /cli/llm/check route lets clients
/// poll readiness.
pub fn maybe_first_boot_discover() {
    // Cheap; safe to call on every boot.
    k2_core::llm::download::cleanup_stale_downloads();

    let host = shared();
    match k2_core::llm::download::default_model_exists() {
        Ok(true) => {
            match k2_core::llm::download::default_model_path() {
                Ok(p) => {
                    let path_str = p.to_string_lossy().to_string();
                    host.set_model_path(path_str.clone());
                    log_debug!("[llm-host] default model found at {path_str}");
                }
                Err(e) => log_debug!("[llm-host] WARN: default_model_path: {e}"),
            }
        }
        Ok(false) => {
            log_debug!(
                "[llm-host] default model not found — clients can call \
                 /cli/llm/download-default to fetch"
            );
        }
        Err(e) => log_debug!("[llm-host] WARN: default_model_exists: {e}"),
    }
}

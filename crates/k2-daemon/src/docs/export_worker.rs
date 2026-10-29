//! The export worker as the daemon sees it (vs-live §3): which `k2-export`
//! runs, whether it passed its hello, the limits of one job, and how many
//! jobs run at once. Spawning, the per-OS limits, the watchdog and cancel
//! are `k2_export::host`.
//!
//! **Which binary** (the k2-ptyd order, `ptyd/client.rs`): `K2_EXPORT_BIN`
//! (debug builds only: an env var must never pick a release daemon's
//! worker), then the copy embedded in the daemon (standalone downloads;
//! authoritative, hash-checked, placed at
//! `~/.k2/bin/export/<ver>-<sha12>/k2-export`, 0700, never overwritten in
//! place), then `k2-export` beside the daemon (`K2 Daemon.app`, packages).
//!
//! **Feature `docs-export-v1`** is listed while live docs are up and the
//! worker resolved and passed its hello (checked once when the docs
//! runtime starts). Without a worker, `as-is` still runs in the daemon.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_export::host::{self, Bounds, Hello, Spawn};
use k2_export::proto::{Kind, Limits};

include!(concat!(env!("OUT_DIR"), "/export_embed.rs"));

/// Defaults (vs-live §2 step 7). Each is an `AppSettings` field (0 = this).
pub const DEFAULT_WALL_SECS: u64 = 300;
pub const DEFAULT_MEMORY_MIB: u64 = 1024;
pub const DEFAULT_MAX_OUTPUT_MIB: u64 = 512;
pub const DEFAULT_WORKERS: u64 = 2;
/// Jobs waiting for a slot before `429 export_busy`.
pub const QUEUE: usize = 8;

/// Debug builds only: override the wall time (ms), memory (MiB) and output
/// cap (bytes), for the headless limit cases.
pub const ENV_WALL_MS: &str = "K2_EXPORT_WALL_MS";
pub const ENV_MEMORY_MIB: &str = "K2_EXPORT_MEMORY_MIB";
pub const ENV_MAX_OUTPUT_BYTES: &str = "K2_EXPORT_MAX_OUTPUT_BYTES";
/// Debug builds only: the worker binary.
pub const ENV_BIN: &str = "K2_EXPORT_BIN";
/// Debug builds only: run `as-is` through the worker too (tests).
pub const ENV_AS_IS_IN_WORKER: &str = "K2_EXPORT_AS_IS_IN_WORKER";

fn debug_env(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The limits of one export.
pub fn limits() -> Limits {
    let s = k2_core::app_settings::load();
    let or = |v: u64, d: u64| if v == 0 { d } else { v };
    let wall_ms = debug_env(ENV_WALL_MS)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(or(s.docs_export_wall_secs, DEFAULT_WALL_SECS).saturating_mul(1000));
    let memory = debug_env(ENV_MEMORY_MIB)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(or(s.docs_export_memory_mib, DEFAULT_MEMORY_MIB))
        .saturating_mul(1 << 20);
    let out = debug_env(ENV_MAX_OUTPUT_BYTES)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(or(s.docs_export_max_output_mib, DEFAULT_MAX_OUTPUT_MIB).saturating_mul(1 << 20));
    let kit = k2_mpc_base::export::ExportLimits::default();
    Limits {
        // Bigger files aren't docs (`blob_max_bytes`), so no export reads more.
        max_input_bytes: super::blob_max_bytes(),
        max_output_bytes: out,
        max_pixels: kit.max_pixels,
        max_pages: kit.max_pages,
        max_wall_ms: wall_ms,
        memory_bytes: memory,
    }
}

pub fn bounds(l: &Limits) -> Bounds {
    Bounds { wall: Duration::from_millis(l.max_wall_ms), memory_bytes: l.memory_bytes, max_output_bytes: l.max_output_bytes }
}

/// Debug builds only: `as-is` goes through the worker as well.
pub fn as_is_in_worker() -> bool {
    debug_env(ENV_AS_IS_IN_WORKER).is_some_and(|v| v == "1")
}

/// The worker binary to run, or why there is none (`no_binary`,
/// `bin_env_missing`, `embed_hash_mismatch`, `copy_bin:<kind>`).
pub fn binary() -> Result<PathBuf, String> {
    if let Some(p) = debug_env(ENV_BIN) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err("bin_env_missing".into()) };
    }
    if let Some((bytes, sha)) = EXPORT_EMBED {
        if sha256_hex(bytes) != sha {
            return Err("embed_hash_mismatch".into());
        }
        static PLACED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
        return PLACED
            .get_or_init(|| versioned_copy(&k2_core::paths::k2_home(), bytes, sha).map_err(|e| format!("copy_bin:{}", e.kind())))
            .clone();
    }
    let exe = std::env::current_exe().map_err(|_| "no_current_exe".to_string())?;
    let sib = exe.parent().map(|d| d.join(k2_export::proto::BINARY_NAME)).ok_or("no_exe_dir")?;
    if sib.is_file() {
        Ok(sib)
    } else {
        Err("no_binary".into())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `<home>/bin/export/<ver>-<sha12>/k2-export` (0700), reused on a hash
/// match, never overwritten in place (a temp name, then a rename).
pub fn versioned_copy(home: &Path, bytes: &[u8], sha: &str) -> std::io::Result<PathBuf> {
    let dir = home
        .join("bin")
        .join("export")
        .join(format!("{}-{}", k2_export::proto::VERSION, &sha[..12.min(sha.len())]));
    let target = dir.join(k2_export::proto::BINARY_NAME);
    if let Ok(existing) = std::fs::read(&target) {
        if sha256_hex(&existing) == sha {
            return Ok(target);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let tmp = dir.join(format!(".k2-export.{}.tmp", std::process::id()));
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o700).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &target)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".k2-export.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &target)?;
    }
    Ok(target)
}

/// The worker's hello and targets, per kind, for the binary that answered
/// (cached per binary path; `--list` and the target check read it).
struct Known {
    bin: PathBuf,
    hello: Hello,
    targets: HashMap<String, Vec<serde_json::Value>>,
}

fn known() -> &'static Mutex<Option<Known>> {
    static K: OnceLock<Mutex<Option<Known>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(None))
}

static READY: AtomicBool = AtomicBool::new(false);

/// Whether the worker resolved and passed its hello (`docs-export-v1`).
pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

fn kind_key(k: &Kind) -> String {
    match k {
        Kind::Pack(p) => format!("pack:{p}"),
        Kind::Format(f) => format!("format:{f}"),
    }
}

/// The worker's targets for `kind`, or why it isn't available. A release
/// daemon refuses a hostile (test) worker.
pub fn targets(kind: &Kind) -> Result<(Hello, Vec<serde_json::Value>), String> {
    let bin = binary()?;
    {
        let k = known().lock().expect("export known");
        if let Some(k) = k.as_ref().filter(|k| k.bin == bin) {
            if let Some(t) = k.targets.get(&kind_key(kind)) {
                return Ok((k.hello.clone(), t.clone()));
            }
        }
    }
    let dir = jobs_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("jobs_dir:{}", e.kind()))?;
    let l = limits();
    let b = Bounds { wall: Duration::from_secs(10), memory_bytes: l.memory_bytes, max_output_bytes: 1 << 20 };
    let (hello, t) = host::describe(&Spawn { bin: bin.clone(), cwd: dir }, kind, b).map_err(|f| {
        log_debug!("[docs] WARN: export worker describe failed: {:?} (stderr: {})", f.error, f.stderr.trim());
        match f.error {
            host::RunError::Spawn(m) => format!("spawn: {m}"),
            host::RunError::Internal { message, .. } => message,
            other => format!("{other:?}"),
        }
    })?;
    if hello.hostile && !cfg!(debug_assertions) {
        return Err("hostile_worker".into());
    }
    let mut k = known().lock().expect("export known");
    if k.as_ref().is_none_or(|k| k.bin != bin) {
        *k = Some(Known { bin, hello: hello.clone(), targets: HashMap::new() });
    }
    if let Some(k) = k.as_mut() {
        k.targets.insert(kind_key(kind), t.clone());
    }
    READY.store(true, Ordering::Release);
    Ok((hello, t))
}

/// At docs start: resolve the worker and run its hello (off the boot path).
pub fn check_in_background() {
    let _ = std::thread::Builder::new().name("k2-export-hello".into()).spawn(|| match targets(&Kind::Format(String::new())) {
        Ok((h, _)) => log_debug!("[docs] export worker ready: k2-export {} ({} exporter(s))", h.version, h.exporters.len()),
        Err(why) => {
            READY.store(false, Ordering::Release);
            log_debug!("[docs] export worker not available ({why}); as-is exports still run in the daemon");
        }
    });
}

/// `~/.k2/exports/jobs` (beside the docs store, so tests stay in their home).
pub fn jobs_root() -> PathBuf {
    let store = k2_core::docs_store::store_path();
    store.parent().map(|d| d.join("exports").join("jobs")).unwrap_or_else(|| PathBuf::from("exports/jobs"))
}

// ── concurrency (vs-live §3.3) ──────────────────────────────────────────

struct SlotState {
    running: usize,
    queued: usize,
    pids: HashSet<String>,
}

fn slots() -> &'static (Mutex<SlotState>, Condvar) {
    static S: OnceLock<(Mutex<SlotState>, Condvar)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(SlotState { running: 0, queued: 0, pids: HashSet::new() }), Condvar::new()))
}

fn workers() -> usize {
    let w = k2_core::app_settings::load().docs_export_workers;
    (if w == 0 { DEFAULT_WORKERS } else { w }).clamp(1, 64) as usize
}

/// A running job's slot; freed on drop.
pub struct Slot {
    pid: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (m, cv) = slots();
        let mut s = m.lock().expect("export slots");
        s.running = s.running.saturating_sub(1);
        s.pids.remove(&self.pid);
        cv.notify_all();
    }
}

/// A place in line, or `None` when the line is full (`429 export_busy`).
pub struct Ticket {
    pid: String,
    taken: bool,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.taken {
            let (m, cv) = slots();
            let mut s = m.lock().expect("export slots");
            s.queued = s.queued.saturating_sub(1);
            cv.notify_all();
        }
    }
}

/// Join the line: at most `workers()` running and [`QUEUE`] waiting.
pub fn enqueue(pid: &str) -> Option<Ticket> {
    let (m, _) = slots();
    let mut s = m.lock().expect("export slots");
    if s.queued >= QUEUE {
        return None;
    }
    s.queued += 1;
    Some(Ticket { pid: pid.to_string(), taken: false })
}

/// Wait for a slot: one running job per caller, `workers()` in all. `None`
/// when `cancel` is set while waiting.
pub fn wait_slot(mut t: Ticket, cancel: &AtomicBool) -> Option<Slot> {
    let (m, cv) = slots();
    let mut s = m.lock().expect("export slots");
    loop {
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        if s.running < workers() && !s.pids.contains(&t.pid) {
            s.queued = s.queued.saturating_sub(1);
            s.running += 1;
            s.pids.insert(t.pid.clone());
            t.taken = true;
            return Some(Slot { pid: t.pid.clone() });
        }
        s = cv.wait_timeout(s, Duration::from_millis(200)).expect("export slots").0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_copy_is_versioned_reused_and_replaced_on_a_hash_change() {
        let home = std::env::temp_dir().join(format!("k2-export-vc-{}", uuid::Uuid::new_v4()));
        let a = b"worker-a".to_vec();
        let ha = sha256_hex(&a);
        let p = versioned_copy(&home, &a, &ha).expect("copy");
        assert!(p.to_string_lossy().contains(&format!("bin/export/{}-{}", k2_export::proto::VERSION, &ha[..12])), "{p:?}");
        assert_eq!(std::fs::read(&p).expect("read"), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(versioned_copy(&home, &a, &ha).expect("again"), p, "a hash match reuses the copy");
        let b = b"worker-b".to_vec();
        let hb = sha256_hex(&b);
        let q = versioned_copy(&home, &b, &hb).expect("copy b");
        assert_ne!(p, q, "another hash, another folder");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_line_holds_at_most_queue_waiting() {
        // Isolated from other tests' slots: count only what this test adds.
        let base = {
            let (m, _) = slots();
            m.lock().expect("slots").queued
        };
        let mut held = Vec::new();
        while let Some(t) = enqueue("person:test-line") {
            held.push(t);
            assert!(held.len() <= QUEUE, "more than {QUEUE} waiting");
        }
        assert_eq!(held.len() + base, QUEUE, "the line fills to {QUEUE}");
        drop(held);
        assert!(enqueue("person:test-line").is_some(), "dropped tickets free the line");
    }
}

//! Pending-live delivery durability.
//!
//! F3 of Phase 3.1. When a `Delivery::Live` signal targets an
//! offline agent, the bus needs to wake the target and inject on
//! session-ready. Without durability, a daemon crash between wake
//! and session-boot would drop the signal. This module persists
//! the signal to `~/.k2so/daemon.pending-live/<agent>/<ts>-<uuid>.json`
//! so it survives restart.
//!
//! The flow:
//!
//!   1. `DaemonWakeProvider::wake` gets called for offline target.
//!      It persists the signal here.
//!   2. (Deferred to a real scheduler-wake primitive: fire the
//!      actual wake. MVP: no wake fires; we rely on the agent's
//!      session being spawned by some other path.)
//!   3. When a session is spawned for the target agent (via
//!      `/cli/sessions/spawn`), `drain_for_agent(name)` reads all
//!      queued signals for that agent, deletes each file, and
//!      returns the signals so the spawn path can inject them.
//!   4. At daemon boot, `boot_sweep()` scans the queue. Queued
//!      entries stay on disk until either a session spawns for
//!      that agent (drain + inject) or they pass an hour
//!      ([`MAX_AGE_SECS`]: dropped with a log line, never delivered).
//!
//! **Root path.** `~/.k2so/daemon.pending-live/<agent>/`. Lives
//! under the daemon's own `~/.k2so/` — this is the daemon's own
//! queue state, not project-scoped. One daemon, one queue root.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use k2_core::awareness::AgentSignal;
use k2_core::fs_atomic::atomic_write;
use k2_core::log_debug;

/// In-memory pending-signal counters keyed by agent name. Populated
/// once at first access by scanning the queue directory; updated on
/// every `enqueue` and cleared on every successful `drain_for_agent`.
///
/// **Why this exists.** The pre-L1.1 `drain_for_agent` hit the disk
/// with `fs::read_dir(...)` on EVERY session spawn, whether the queue
/// was empty or not. On a fresh machine with no pending signals, that
/// was ~2-5ms of pure I/O on every Cmd+T for zero benefit. The counter
/// lets us short-circuit the common case (no signals queued) with a
/// single mutex acquire + hashmap lookup — ~100ns amortized.
///
/// **Consistency.** enqueue AND drain both go through the mutex so
/// counter state never lags disk state. The window where a file is
/// visible on disk but not yet in the counter (or vice-versa) never
/// exists outside the lock. Correctness > raw throughput for this
/// state — enqueue/drain are low-frequency operations (user-driven,
/// not per-frame) so lock contention is a non-issue.
static PENDING_STATE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

fn pending_state() -> &'static Mutex<HashMap<String, usize>> {
    PENDING_STATE.get_or_init(|| {
        // First-access initialization: scan the queue root once,
        // populating the counter from whatever's already on disk.
        // Any boot-time replay that predated us is reflected here.
        let root = queue_root();
        let mut map = HashMap::new();
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.filter_map(|r| r.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let count = match fs::read_dir(&path) {
                    Ok(it) => it
                        .filter_map(|r| r.ok())
                        .filter(|e| {
                            e.path()
                                .extension()
                                .and_then(|x| x.to_str())
                                == Some("json")
                        })
                        .count(),
                    Err(_) => 0,
                };
                if count > 0 {
                    map.insert(name.to_string(), count);
                }
            }
        }
        Mutex::new(map)
    })
}

/// The names the queue root's override is read under, newest first.
const ROOT_NAMES: &[&str] = k2_core::env_names::PENDING_LIVE_ROOT;

/// Resolve the queue root. Daemon's `~/.k2so/daemon.pending-live/`.
/// Tests override via env var; production uses `k2_core::paths::home_dir()`.
pub fn queue_root() -> PathBuf {
    if let Some((_, override_path)) = k2_core::env_names::first_set(ROOT_NAMES) {
        return PathBuf::from(override_path);
    }
    k2_core::paths::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".k2so/daemon.pending-live")
}

/// Persist a signal for eventual inject. Filename uses
/// ns-timestamp + signal uuid so sorted-lex = sorted-by-time and
/// concurrent writers don't collide. Path-traversal-safe: agent
/// name with `/` or `..` is neutralized.
pub fn enqueue(signal: &AgentSignal, target_agent: &str) -> io::Result<PathBuf> {
    let root = queue_root();
    let safe_agent = sanitize(target_agent);
    let dir = root.join(&safe_agent);
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{ns:020}-{}.json", signal.id));
    let json = serde_json::to_vec_pretty(signal).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}"))
    })?;

    // Take the lock around both the file write AND the counter
    // bump so a concurrent drain can never observe an inconsistent
    // (file-on-disk, counter-says-zero) state. See PENDING_STATE
    // doc for rationale.
    let mut state = pending_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // One file per signal id: a second enqueue of the same signal (a
    // retried wake) keeps the first file.
    if let Some(existing) = queued_file_for(&dir, &signal.id.to_string()) {
        drop(state);
        log_debug!(
            "[daemon/pending-live] signal id={} for agent={} is already queued at {:?}; not queued twice",
            signal.id,
            target_agent,
            existing
        );
        return Ok(existing);
    }
    atomic_write(&path, &json)?;
    *state.entry(target_agent.to_string()).or_insert(0) += 1;
    drop(state);

    log_debug!(
        "[daemon/pending-live] queued signal id={} for agent={} at {:?}",
        signal.id,
        target_agent,
        path
    );
    Ok(path)
}

/// Drain every queued signal for `agent`, deleting each file
/// after successful parse. Used by the spawn path to flush
/// anything that was queued while the agent was offline.
///
/// Hot path: when no signals are queued (by far the common case on
/// every Cmd+T / Cmd+Shift+T), returns `Vec::new()` after a single
/// mutex acquire + hashmap lookup. No disk I/O. This is the
/// primary win of L1.1.
pub fn drain_for_agent(agent: &str) -> Vec<AgentSignal> {
    let mut state = pending_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Fast path. Empty (or missing) counter = no files on disk.
    // Locked access to the counter means enqueue can't sneak a
    // file between this check and any subsequent drain_directory
    // call — if a file exists, the counter contains it.
    if state.get(agent).copied().unwrap_or(0) == 0 {
        return Vec::new();
    }
    // Slow path. Actually drain, clear the counter. Still inside
    // the lock so enqueue waits — keeps state coherent.
    let root = queue_root();
    let dir = root.join(sanitize(agent));
    let signals = drain_directory(&dir);
    state.remove(agent);
    signals
}

/// Boot-time replay — scan every agent directory under the queue
/// root. Called by the daemon at startup BEFORE the accept loop
/// begins. Returns a `Vec<(agent_name, Vec<AgentSignal>)>` so
/// callers can decide what to do (daemon-at-boot: log + drain as
/// sessions come online; tests: just inspect the contents).
/// 0.37.5 migration — merge legacy `<sanitized_pid>_<agent>/` queue
/// dirs into the new bare `<sanitized_pid>/` shape.
///
/// Pre-0.37.5 the queue was keyed `<workspace_id>:<agent>`, which
/// `sanitize()` mapped to `<sanitized_pid>_<agent>` (colon → underscore).
/// Post-0.37.5 the canonical key is bare `workspace_id`, so the dir
/// is just `<sanitized_pid>`. Existing queued signals must move to
/// the new dir or they'll never drain.
///
/// **Lossless.** Per-file move (`fs::rename`), atomic on the same
/// filesystem. If a target file already exists (statistically
/// impossible — every queued filename embeds a ns timestamp +
/// signal UUID), the source file stays in place and a debug log
/// fires.
///
/// **Idempotent.** Subsequent boots find no `_<agent>` dirs and no-op.
///
/// **Ordering.** This MUST run before any other `pending_live` access
/// (`boot_sweep`, `enqueue`, `drain_for_agent`) so the in-memory
/// `pending_state` counter is built from the post-migration shape.
/// Caller is responsible for the order; in practice `main.rs`'s boot
/// sequence calls this immediately after queue_root setup, before
/// `boot_sweep`.
///
/// Worktree dirs (no underscore in name) and bare-pid dirs (no
/// underscore prefix that decomposes into a UUID-shape) are left alone.
pub fn migrate_legacy_dirs_to_bare_pid() {
    let root = queue_root();
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut migrated = 0usize;
    let mut skipped = 0usize;
    for entry in entries.filter_map(|r| r.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Look for `<sanitized_uuid>_<rest>` pattern. The sanitized
        // UUID has hyphens preserved but colons replaced by `_`, so
        // the pattern is: 36 chars of UUID-shape, then `_`, then a
        // suffix.
        if dir_name.len() <= 37 || !dir_name.is_char_boundary(36) {
            skipped += 1;
            continue;
        }
        let (prefix, sep_and_rest) = dir_name.split_at(36);
        if !is_uuid_shape(prefix) {
            skipped += 1;
            continue;
        }
        if !sep_and_rest.starts_with('_') {
            skipped += 1;
            continue;
        }
        // Found a legacy dir. Target dir is just <prefix>.
        let target = root.join(prefix);
        if !target.exists() {
            // Simple rename — atomic on same fs.
            match fs::rename(&path, &target) {
                Ok(()) => {
                    migrated += 1;
                    log_debug!(
                        "[pending-live/migrate] renamed {dir_name} → {prefix}"
                    );
                }
                Err(e) => {
                    log_debug!(
                        "[pending-live/migrate] rename {dir_name} → {prefix} failed: {e}"
                    );
                }
            }
            continue;
        }
        // Target dir exists — move queued files in, then remove old dir.
        let files = match fs::read_dir(&path) {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let mut moved_files = 0usize;
        for f in files.filter_map(|r| r.ok()) {
            let src = f.path();
            let fname = match src.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let dst = target.join(&fname);
            if dst.exists() {
                log_debug!(
                    "[pending-live/migrate] file {fname} already exists in target {prefix}; \
                     leaving source"
                );
                continue;
            }
            if let Err(e) = fs::rename(&src, &dst) {
                log_debug!(
                    "[pending-live/migrate] move {fname} from {dir_name} → {prefix} failed: {e}"
                );
                continue;
            }
            moved_files += 1;
        }
        // Best-effort cleanup of the now-empty (or partially-empty) source dir.
        let _ = fs::remove_dir(&path);
        migrated += 1;
        log_debug!(
            "[pending-live/migrate] merged {dir_name} → {prefix} ({moved_files} files)"
        );
    }
    if migrated > 0 || skipped > 0 {
        log_debug!(
            "[pending-live/migrate] complete: migrated={migrated} skipped={skipped}"
        );
    }
}

fn is_uuid_shape(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[13] == b'-'
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[23] == b'-'
}

/// Queued signals older than this are dropped, never delivered: an
/// agent that comes back hours later must not act on a stale nudge.
pub const MAX_AGE_SECS: i64 = 60 * 60;

/// Boot sweep (replaces the old read-delete-requeue replay). Each queued
/// file stays where it is unless it is a second file for a signal id
/// already queued (deleted) or older than [`MAX_AGE_SECS`] (deleted with
/// a log line). Rebuilds the in-memory counters from what is left and
/// returns `(agent, kept)` for the boot log. Never writes a file, so
/// boots no longer multiply or re-date the queue.
pub fn boot_sweep() -> Vec<(String, usize)> {
    let mut state = pending_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let summary = sweep_root(&queue_root(), now_secs());
    state.clear();
    for (agent, kept) in &summary {
        state.insert(agent.clone(), *kept);
    }
    summary.into_iter().filter(|(_, kept)| *kept > 0).collect()
}

/// [`boot_sweep`]'s disk half, for one queue root (tests pass their own).
fn sweep_root(root: &Path, now: i64) -> Vec<(String, usize)> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(|r| r.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(agent) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        let kept = sweep_dir(&path, now, false).len();
        out.push((agent, kept));
    }
    out
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The queued file for signal `id` in `dir`, if there is one.
fn queued_file_for(dir: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.json");
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix)))
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Take every deliverable signal in `dir` (deleting each file once read).
fn drain_directory(dir: &Path) -> Vec<AgentSignal> {
    sweep_dir(dir, now_secs(), true)
}

/// Walk `dir` oldest first. A second file for a signal id is deleted; a
/// signal older than [`MAX_AGE_SECS`] is deleted with a log line naming
/// its sender. `take`: the rest are deleted and returned (delivery);
/// otherwise they stay on disk and are returned (boot count).
fn sweep_dir(dir: &Path, now: i64, take: bool) -> Vec<AgentSignal> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(files.len());
    for p in files {
        let signal = match fs::read(&p).and_then(|b| {
            serde_json::from_slice::<AgentSignal>(&b).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })
        }) {
            Ok(signal) => signal,
            Err(e) => {
                log_debug!(
                    "[daemon/pending-live] parse {:?} failed: {} — leaving in place",
                    p,
                    e
                );
                continue;
            }
        };
        let why_drop = if !seen.insert(signal.id.to_string()) {
            Some("a second file for this signal id".to_string())
        } else {
            let age = now.saturating_sub(signal.at.timestamp());
            (age > MAX_AGE_SECS).then(|| format!("queued {} min ago, over the {} min limit", age / 60, MAX_AGE_SECS / 60))
        };
        if let Some(why) = why_drop {
            remove_logged(&p);
            log_debug!(
                "[daemon/pending-live] dropped signal id={} from {:?} in {:?}: {why}",
                signal.id,
                signal.from,
                dir
            );
            continue;
        }
        if take {
            remove_logged(&p);
        }
        out.push(signal);
    }
    out
}

fn remove_logged(p: &Path) {
    if let Err(e) = fs::remove_file(p) {
        log_debug!("[daemon/pending-live] remove {:?} failed: {}", p, e);
    }
}

fn sanitize(name: &str) -> String {
    if name.is_empty() || name == "." || name == ".." {
        return "_invalid".to_string();
    }
    let clean: String = name
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '/' | '\\' | ':' | '\0' => '_',
            _ => c,
        })
        .collect();
    if clean.starts_with('.') {
        format!("_{}", clean)
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L1.1 fast path: when no signals have been queued for an agent,
    /// `drain_for_agent` must NOT touch the disk. We verify this by
    /// pointing the queue root at a non-existent path and confirming
    /// `drain_for_agent` returns quickly with an empty vec instead
    /// of erroring out on `read_dir`. If the fast path was bypassed,
    /// we'd still get an empty vec (the read_dir error is swallowed)
    /// but any bug that re-introduces the disk hit would still work
    /// "correctly" — so the meaningful check is a benchmark, not a
    /// unit assertion. This unit test just pins down the behavior.
    /// The root's override is read as `K5_PENDING_LIVE_ROOT` first and
    /// `K2SO_PENDING_LIVE_ROOT` after it.
    #[test]
    fn queue_root_is_read_under_the_k5_name_first_and_the_old_name_after() {
        assert_eq!(ROOT_NAMES, &["K5_PENDING_LIVE_ROOT", "K2SO_PENDING_LIVE_ROOT"]);
        let _new_unset = k2_core::test_env::EnvVar::remove("K5_PENDING_LIVE_ROOT");
        let _old = k2_core::test_env::EnvVar::set("K2SO_PENDING_LIVE_ROOT", "/old/root");
        assert_eq!(queue_root(), PathBuf::from("/old/root"));
        let _new = k2_core::test_env::EnvVar::set("K5_PENDING_LIVE_ROOT", "/new/root");
        assert_eq!(queue_root(), PathBuf::from("/new/root"), "the new name wins when both are set");
    }

    /// With neither name set the root is under the home.
    #[test]
    fn queue_root_without_an_override_is_under_the_home() {
        let _a = k2_core::test_env::EnvVar::remove("K5_PENDING_LIVE_ROOT");
        let _b = k2_core::test_env::EnvVar::remove("K2SO_PENDING_LIVE_ROOT");
        assert!(queue_root().ends_with(".k2so/daemon.pending-live"), "{:?}", queue_root());
    }


    fn signal(age_secs: i64) -> AgentSignal {
        use k2_core::awareness::{AgentAddress, SignalKind, WorkspaceId};
        let mut s = AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId("ws-from".into()), name: "sender".into() },
            AgentAddress::Agent { workspace: WorkspaceId("ws-to".into()), name: "target".into() },
            SignalKind::Msg { text: "hello".into() },
        );
        s.at = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        s
    }

    fn write_queued(dir: &Path, ns: u128, s: &AgentSignal) -> PathBuf {
        fs::create_dir_all(dir).expect("dir");
        let p = dir.join(format!("{ns:020}-{}.json", s.id));
        fs::write(&p, serde_json::to_vec(s).expect("json")).expect("write");
        p
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Field report: each boot read, deleted and re-wrote every queued
    /// signal, and the queue ended up with two files per id. A boot now
    /// leaves a pending file exactly where it is, boot after boot.
    #[test]
    fn two_boots_leave_one_pending_file_untouched() {
        let root = k2_core::test_env::unique_temp_path("pending-boots");
        let dir = root.join("agent-a");
        let s = signal(60);
        let file = write_queued(&dir, 1, &s);
        let before = names(&dir);
        assert_eq!(sweep_root(&root, now_secs()), vec![("agent-a".to_string(), 1)]);
        assert_eq!(sweep_root(&root, now_secs()), vec![("agent-a".to_string(), 1)]);
        assert_eq!(names(&dir), before, "boots must not rename, re-date or copy a queued signal");
        assert!(file.exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// Two files already on disk for one signal id (left by older boots):
    /// the boot sweep keeps the oldest and deletes the other.
    #[test]
    fn boot_clears_a_second_file_for_one_signal_id() {
        let root = k2_core::test_env::unique_temp_path("pending-dupes");
        let dir = root.join("agent-a");
        let s = signal(60);
        let first = write_queued(&dir, 1, &s);
        let second = write_queued(&dir, 2, &s);
        assert_eq!(sweep_root(&root, now_secs()), vec![("agent-a".to_string(), 1)]);
        assert!(first.exists(), "the oldest file stays");
        assert!(!second.exists(), "the duplicate goes");
        assert_eq!(queued_file_for(&dir, &s.id.to_string()), Some(first));
        let _ = fs::remove_dir_all(&root);
    }

    /// Over an hour old: dropped at boot and never handed to a session.
    #[test]
    fn a_stale_signal_is_dropped_not_delivered() {
        let root = k2_core::test_env::unique_temp_path("pending-stale");
        let dir = root.join("agent-a");
        let stale = signal(MAX_AGE_SECS + 120);
        let fresh = signal(30);
        let stale_file = write_queued(&dir, 1, &stale);
        write_queued(&dir, 2, &fresh);
        assert_eq!(sweep_root(&root, now_secs()), vec![("agent-a".to_string(), 1)]);
        assert!(!stale_file.exists(), "a stale signal is deleted at boot");
        // A stale file that appears after boot is dropped by the drain too.
        let late_stale = signal(MAX_AGE_SECS + 120);
        write_queued(&dir, 3, &late_stale);
        let got = drain_directory(&dir);
        assert_eq!(got.iter().map(|s| s.id).collect::<Vec<_>>(), vec![fresh.id]);
        assert!(names(&dir).is_empty(), "{:?}", names(&dir));
        let _ = fs::remove_dir_all(&root);
    }

    /// A genuinely pending signal is delivered once, and only once, with
    /// its own id; a later boot and drain find nothing to send again.
    #[test]
    fn a_pending_signal_is_delivered_once() {
        let root = k2_core::test_env::unique_temp_path("pending-once");
        let dir = root.join("agent-a");
        let s = signal(30);
        write_queued(&dir, 1, &s);
        write_queued(&dir, 2, &s);
        let got = drain_directory(&dir);
        assert_eq!(got.len(), 1, "one delivery per signal id");
        assert_eq!(got[0].id, s.id, "delivered under its own id");
        assert!(names(&dir).is_empty());
        assert!(sweep_root(&root, now_secs()).iter().all(|(_, kept)| *kept == 0), "a restart after delivery re-sends nothing");
        assert!(drain_directory(&dir).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn drain_for_unknown_agent_is_fast_path_empty() {
        // Use a deliberately nonexistent path so any disk read would
        // fail. The result should still be Vec::new() courtesy of the
        // fast path.
        let missing = k2_core::test_env::unique_temp_path("pending-live-missing");
        let _root = k2_core::test_env::EnvVar::set(
            "K2SO_PENDING_LIVE_ROOT",
            &missing,
        );
        // First access initializes PENDING_STATE from disk. The
        // nonexistent path means the initial map is empty. Subsequent
        // drain calls for any agent should hit the fast path.
        let result = drain_for_agent("agent-that-was-never-enqueued");
        assert!(result.is_empty());
    }
}

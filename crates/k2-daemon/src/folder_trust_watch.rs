//! Re-scan on change (prd-security-sweep-0464-v1 SS11, folded U0 finding 1).
//!
//! Claude watches its settings files and reloads `hooks`, `permissions` and
//! `apiKeyHelper` mid-session, so a scan at spawn time isn't enough: a
//! `git pull` that brings new hooks reaches a running trusted session with
//! no new spawn. This watcher follows every folder where K2 runs an AI
//! session (a live `claude`/`codex`/`grok`/`gemini`/`cursor-agent` in
//! `v2_session_map`):
//!
//! - non-recursive notify watches on the folder, its git work tree top and
//!   main checkout, and the config folders the scan reads there (`.claude`,
//!   `.claude/hooks`, `.cursor`, `.gemini`, `.codex`, `.grok`, `.vscode`,
//!   `.git`, `.git/hooks`); an event marks the folder dirty;
//! - a full re-scan of every live folder every [`RESCAN`] as the net for
//!   what non-recursive watches miss (a script deep under `.claude/`, a
//!   nested clone);
//! - a dirty folder is re-evaluated after [`DEBOUNCE`]; when its
//!   fingerprint differs from the last one seen, the gate runs again
//!   ([`k2_core::cli_folder_trust::gate::ensure`]): a `once` folder whose
//!   settings changed (or a clean one that became risky) loses K2's grants
//!   and the owner gets the card, worded "changed while <agent> was
//!   running", and K2's sessions in that scope stop (review A-M1). An
//!   `always` folder is left alone.
//!
//! What it can't do: Claude may load a new hook before K2 sees the change
//! (both watch the same files). For the watched config folders K2 notices
//! within about a second; for a file deeper under them (a script under
//! `.claude/…`, a nested clone) it can take up to [`RESCAN`]. In that
//! window a running session can run the new settings.
//! The honest limits are in `k2 study folder-trust`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use k2_core::cli_folder_trust::gate::{self, Actor, Door};
use k2_core::log_debug;

use crate::notify_bound::{should_observe, DroppingHandler, NOTIFY_CHANNEL_BOUND};

/// How often the live-folder list is refreshed.
const TICK: Duration = Duration::from_secs(2);
/// A full re-scan of every live folder.
const RESCAN: Duration = Duration::from_secs(60);
/// Wait for a burst of writes (a checkout) to settle.
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Config folders watched (non-recursively) under each root.
const CONFIG_DIRS: &[&str] = &[".claude", ".claude/hooks", ".cursor", ".gemini", ".codex", ".grok", ".vscode", ".git", ".git/hooks"];

static STARTED: AtomicBool = AtomicBool::new(false);

/// Start the watcher thread once (boot, after ready).
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("k2-folder-trust-watch".into()).spawn(|| {
        if let Err(e) = watch_loop() {
            log_debug!("[folder-trust-watch] exited: {e}");
        }
        STARTED.store(false, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        STARTED.store(false, Ordering::SeqCst);
        log_debug!("[folder-trust-watch] failed to spawn: {e}");
    }
}

/// One live folder: the harness running there and its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Live {
    pub cwd: String,
    pub program: Option<String>,
    pub project_id: Option<String>,
}

/// Every folder with a live K2 harness session.
pub(crate) fn live_folders() -> Vec<Live> {
    let mut seen: BTreeMap<String, Live> = BTreeMap::new();
    for (key, s) in crate::v2_session_map::snapshot() {
        if !crate::folder_trust::session_is_gated(&key, s.program.as_deref(), &s.args) || !s.is_child_alive() {
            continue;
        }
        let Some(cwd) = s.cwd.as_ref() else { continue };
        let cwd = dunce::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone()).to_string_lossy().to_string();
        seen.entry(cwd.clone()).or_insert_with(|| Live {
            project_id: crate::folder_trust::project_for_path(&cwd).map(|(id, _)| id),
            cwd,
            program: s.program.clone(),
        });
    }
    seen.into_values().collect()
}

/// The folders to watch for one live cwd (that exist).
fn watch_dirs(cwd: &str) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(cwd)];
    if let Some(e) = gate::evaluate(cwd) {
        for r in e.roots {
            let r = PathBuf::from(r);
            if !roots.contains(&r) {
                roots.push(r);
            }
        }
    }
    let mut out = Vec::new();
    for r in roots {
        if r.is_dir() {
            out.push(r.clone());
        }
        for d in CONFIG_DIRS {
            let p = r.join(d);
            if p.is_dir() && !std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(true) {
                out.push(p);
            }
        }
    }
    out
}

/// Re-check one live folder. `last`: the fingerprint last seen (outer
/// `None` = first sighting). Returns the fingerprint now.
pub(crate) fn recheck(live: &Live, last: Option<&Option<String>>) -> Option<String> {
    let Some(eval) = gate::evaluate(&live.cwd) else { return None };
    let now = eval.fingerprint.clone();
    if last == Some(&now) {
        return now;
    }
    let changed_live = last.is_some();
    let actor = Actor {
        project_id: live.project_id.clone(),
        by: "watch".into(),
        changed_live,
        tool: live.program.clone(),
        ..Default::default()
    };
    match gate::ensure(&live.cwd, Door::Spawn, &actor, None) {
        gate::Outcome::Held(r) | gate::Outcome::Denied(r) => {
            // Held again (a live change on a `once` folder, a clean folder
            // that became risky) or denied: K2's sessions in that scope stop
            // (review A-M1), so a running Claude doesn't keep working under
            // settings nobody has trusted. The card says so.
            let n = crate::folder_trust::stop_sessions_in(&r.scope);
            log_debug!("[folder-trust-watch] {} {}: stopped {n} session(s) (card {:?})", live.cwd, r.error, r.card);
        }
        _ => {}
    }
    now
}

fn watch_loop() -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel::<notify::Result<Event>>(NOTIFY_CHANNEL_BOUND);
    let mut watcher = RecommendedWatcher::new(DroppingHandler::new(tx), Config::default()).map_err(|e| format!("create watcher: {e}"))?;
    // dir → the live cwds it belongs to
    let mut watched: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    let mut lives: Vec<Live> = Vec::new();
    let mut last_fp: HashMap<String, Option<String>> = HashMap::new();
    let mut dirty: HashMap<String, Instant> = HashMap::new();
    let mut last_tick = Instant::now() - TICK;
    let mut last_full = Instant::now();
    loop {
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            let now_lives = live_folders();
            if now_lives != lives {
                // New folders: first sighting re-check; gone folders: unwatch.
                let mut want: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
                for l in &now_lives {
                    for d in watch_dirs(&l.cwd) {
                        want.entry(d).or_default().insert(l.cwd.clone());
                    }
                    if !last_fp.contains_key(&l.cwd) {
                        dirty.insert(l.cwd.clone(), Instant::now());
                    }
                }
                for d in watched.keys().filter(|d| !want.contains_key(*d)).cloned().collect::<Vec<_>>() {
                    let _ = watcher.unwatch(&d);
                    watched.remove(&d);
                }
                for (d, owners) in want {
                    if !watched.contains_key(&d) && watcher.watch(&d, RecursiveMode::NonRecursive).is_err() {
                        continue;
                    }
                    watched.insert(d, owners);
                }
                last_fp.retain(|k, _| now_lives.iter().any(|l| &l.cwd == k));
                lives = now_lives;
            }
        }
        if last_full.elapsed() >= RESCAN {
            last_full = Instant::now();
            for l in &lives {
                dirty.entry(l.cwd.clone()).or_insert_with(Instant::now);
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(ev)) if should_observe(ev.kind) => {
                for p in &ev.paths {
                    for (d, owners) in &watched {
                        if p.starts_with(d) || p.parent() == Some(d.as_path()) {
                            for o in owners {
                                dirty.insert(o.clone(), Instant::now());
                            }
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("notify channel closed".into()),
        }
        let due: Vec<String> = dirty.iter().filter(|(_, at)| at.elapsed() >= DEBOUNCE).map(|(k, _)| k.clone()).collect();
        for cwd in due {
            dirty.remove(&cwd);
            let Some(l) = lives.iter().find(|l| l.cwd == cwd).cloned() else { continue };
            let fp = recheck(&l, last_fp.get(&cwd));
            last_fp.insert(cwd.clone(), fp);
            // New config folders (a pull created `.claude/`): watch them.
            for d in watch_dirs(&cwd) {
                if !watched.contains_key(&d) && watcher.watch(&d, RecursiveMode::NonRecursive).is_ok() {
                    watched.entry(d).or_default().insert(cwd.clone());
                }
            }
        }
    }
}

/// One re-check of `cwd` as the watcher does it (`last`: the fingerprint
/// it saw before; `None` = first sighting). For tests and tools.
pub fn recheck_path(cwd: &std::path::Path, last: Option<&Option<String>>, project_id: Option<&str>) -> Option<String> {
    recheck(
        &Live { cwd: cwd.to_string_lossy().to_string(), program: Some("claude".into()), project_id: project_id.map(str::to_string) },
        last,
    )
}

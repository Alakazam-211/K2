//! Env, file and agent delivery at spawn (§9.4, KV19, VS10; agent:
//! prd-vault-gaps-v1 item 3).
//!
//! Every agent spawn path (`spawn.rs`, `v2_spawn.rs`, the legacy
//! terminal create) calls [`materialize`] right after the passport is
//! minted. Only secrets whose delivery is `file` or `env` for this
//! session arrive; fetch/proxy secrets never touch the environment.
//! `agent` secrets (ssh keys) never arrive either: the session gets
//! `SSH_AUTH_SOCK`, a socket in its secrets dir that lists and signs with
//! them ([`super::ssh_agent`]).
//!
//! Files go to a memory-backed directory, never `/tmp` or the home folder.
//! Linux, the same order as `k2_core::runtime_dir` ([`linux_secrets_candidates`]):
//! `$K2_RUNTIME_DIR/secrets`, else the daemon unit's own dir
//! (`RuntimeDirectory=k2` → `$RUNTIME_DIRECTORY/secrets`, `/run/k2/secrets`;
//! systemd-logind never deletes it, akzm 2026-10-24), else
//! `$XDG_RUNTIME_DIR/k2/secrets`, `/run/k2/secrets`,
//! `/dev/shm/k2-<uid>/secrets`. macOS: the per-user `$TMPDIR/k2-secrets`
//! (private, 0700). No memory filesystem → file delivery is skipped
//! (`no_memory_fs`) and status says so.
//! Directories are made 0700 and must not exist yet; files are created
//! `O_CREAT|O_EXCL|O_NOFOLLOW` 0600. Removed when the session ends and
//! swept at boot.
//!
//! Live delivery (0.46.5, field papercut 8): every agent session that can
//! hold files gets its dir at spawn, even an empty one, so a `file` secret
//! granted (or set) while it runs can still reach it: [`top_up_all`] writes
//! the files a live session is now granted and doesn't have yet, right
//! after a grant or a set and on the vault's minute tick; `k2 vault sync`
//! runs [`top_up`] for the caller's own session at once. The same grant
//! check and audit as at spawn (`spawn_items` + `resolve(Use::File)`). Env
//! and ssh-agent items can't reach a running process: they are listed as
//! "restart your session to receive it".
//!
//! Taking back (0.46.8): the same pass ([`sweep_live`]) removes a file K2
//! delivered once its secret is no longer this session's: at once after a
//! destroy, a disable or a revoke, on the minute tick when a grant or a
//! secret's time ran out. A rotation doesn't touch a delivered file (a new
//! value arrives at restart). Env can't be taken back from a running process.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use std::io::Write;
use std::path::{Path, PathBuf};

use k2_core::private_fs;

use k2_vault::{Delivery, Requester, SecretStore, Selector, Use};
use parking_lot::Mutex;

pub const SECRETS_DIR_ENV: &str = "K2_SECRETS_DIR";
/// Set only for a session with `agent` items (a grant can never set it:
/// `SSH_*` is on the protected list, `k2_vault::delivery`).
pub const SSH_AUTH_SOCK_ENV: &str = "SSH_AUTH_SOCK";

/// Live session → its secrets dir (removed at session end).
static DIRS: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());

/// What live delivery needs to know about a session with a dir: its
/// workspace, the env names it got at spawn (`None`: unknown, a session
/// kept across a restart), secrets whose live delivery failed (not
/// retried for [`RETRY_AFTER`], so a refusal isn't audited every minute),
/// and the files K2 wrote into its dir ([`Delivered`]).
struct LiveSession {
    workspace_id: String,
    env_names: Option<Vec<String>>,
    failed: HashMap<String, Instant>,
    delivered: Delivered,
}

/// The files K2 itself wrote into a session's dir (file name → secret id),
/// so a file goes when its secret stops being this session's and nothing
/// else in the dir is ever touched. Kept in a file beside the dir
/// (`<dir>.delivered`, names and ids only, in the memory-backed root), so a
/// session that survives a daemon restart is known from boot. The dir and
/// this file are writable by any same-user process: names are checked on
/// load, and a wrong entry can only reach that session's own dir.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Delivered {
    /// False for a session from before this record existed (one kept across
    /// the update): K2 can't tell what it wrote there earlier.
    complete: bool,
    files: HashMap<String, String>,
}

const DELIVERED_SUFFIX: &str = ".delivered";

fn delivered_path(dir: &Path) -> Option<PathBuf> {
    let name = dir.file_name()?.to_str()?;
    Some(dir.with_file_name(format!("{name}{DELIVERED_SUFFIX}")))
}

fn save_delivered(dir: &Path, d: &Delivered) {
    let write = || -> std::io::Result<()> {
        let path = delivered_path(dir).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no name"))?;
        let tmp = path.with_extension("delivered.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = private_fs::create_new_private_file(&tmp)?;
        f.write_all(&serde_json::to_vec(d).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        k2_core::log_debug!("[vault] WARN: can't record delivered files for {}: {}", dir.display(), e.kind());
    }
}

/// The record beside `dir`. Missing or unreadable: not complete, so K2 says
/// it can't tell rather than guessing from what is in the folder.
fn load_delivered(dir: &Path) -> Delivered {
    let read = || -> Option<Delivered> {
        let path = delivered_path(dir)?;
        let m = std::fs::symlink_metadata(&path).ok()?;
        if !m.is_file() || !private_fs::owned_by_me(&path, &m) || m.len() > 1024 * 1024 {
            return None;
        }
        serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
    };
    let mut d = read().unwrap_or_default();
    d.files.retain(|name, _| k2_vault::delivery::validate_file_name(name).is_ok());
    d
}

/// A session that survived a daemon restart: live delivery and taking back
/// work for it from now on, not from the first tick.
fn register_survivor(session_id: &str, workspace_id: &str, dir: &Path) {
    LIVE.lock().insert(
        session_id.to_string(),
        LiveSession { workspace_id: workspace_id.to_string(), env_names: None, failed: HashMap::new(), delivered: load_delivered(dir) },
    );
}

#[cfg(test)]
thread_local! {
    /// Tests: the vault can't say what a session gets.
    static FAIL_SPAWN_ITEMS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn items_for(vault: &k2_vault::LocalVault, who: &Requester) -> Result<Vec<k2_vault::SpawnItem>, k2_vault::VaultError> {
    #[cfg(test)]
    if FAIL_SPAWN_ITEMS.with(std::cell::Cell::get) {
        return Err(k2_vault::VaultError::internal("spawn items failed"));
    }
    vault.spawn_items(who)
}

static LIVE: LazyLock<Mutex<HashMap<String, LiveSession>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Running `k2 job` runs' files: (the run's dir, file name, secret id). They
/// aren't sessions (a job runner owns its dir), so taking back doesn't reach
/// them; [`job_runs_holding`] lets the answer say so.
static JOB_FILES: Mutex<Vec<(PathBuf, String, String)>> = Mutex::new(Vec::new());

/// Live delivery and session end take turns: a session that ends while
/// its files are topped up never gets its dir made again.
static TOPUP: Mutex<()> = Mutex::new(());

/// A failed live delivery is tried again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(600);

/// The words an agent reads when a secret can't reach its running session.
pub const RESTART_TO_RECEIVE: &str = "restart your session to receive it";

/// The memory-backed root for secrets dirs, or `None` (`no_memory_fs`).
pub fn secrets_root() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::geteuid() };
        let env = |k: &str| std::env::var_os(k).filter(|x| !x.is_empty()).map(PathBuf::from);
        let candidates = linux_secrets_candidates(
            env(k2_core::runtime_dir::RUNTIME_DIR_ENV).as_deref(),
            k2_core::runtime_dir::systemd_runtime_directory().as_deref(),
            env("XDG_RUNTIME_DIR").as_deref(),
            uid,
            k2_core::runtime_dir::looks_like_test_process(),
        );
        for c in candidates {
            let parent_ok = c.parent().is_some_and(|p| p.is_dir());
            if !parent_ok {
                continue;
            }
            if ensure_private(&c).is_ok() {
                return Some(c.join("secrets"));
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let t = std::env::var_os("TMPDIR").filter(|x| !x.is_empty())?;
        let dir = PathBuf::from(t).join("k2-secrets");
        Some(dir)
    }
    // Windows: like macOS, the per-user temp dir (TMP / TEMP); the dirs in
    // it are owner-only DACLs (k2_core::private_fs).
    #[cfg(windows)]
    {
        Some(std::env::temp_dir().join("k2-secrets"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// Linux: the runtime dir and the secrets root's dir, when systemd-logind
/// can delete them under the running daemon (no linger; see
/// `k2_core::runtime_dir::logind_risk`). `k2 vault doctor` and the boot
/// WARN read it.
pub fn runtime_logind_risk() -> Option<k2_core::runtime_dir::LogindRisk> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let extra: Vec<PathBuf> = secrets_root().and_then(|r| r.parent().map(Path::to_path_buf)).into_iter().collect();
    k2_core::runtime_dir::logind_risk(&extra)
}

/// Boot: one WARN line when [`runtime_logind_risk`] finds a dir at risk.
pub fn warn_logind_risk_at_boot() {
    if let Some(r) = runtime_logind_risk() {
        k2_core::log_debug!("[runtime] WARN: {}", r.message());
    }
}

/// Pure Linux order for the dir that holds `secrets/`: the override alone,
/// else the unit's `$RUNTIME_DIRECTORY` (never for a test process: an
/// agent inherits it from the daemon unit), then the pre-0.46.4 chain.
pub fn linux_secrets_candidates(
    override_dir: Option<&Path>,
    systemd_dir: Option<&Path>,
    xdg_runtime_dir: Option<&Path>,
    uid: u32,
    is_test: bool,
) -> Vec<PathBuf> {
    if let Some(d) = override_dir {
        return vec![d.to_path_buf()];
    }
    let mut v = Vec::new();
    if let Some(s) = systemd_dir.filter(|_| !is_test) {
        v.push(s.to_path_buf());
    }
    if let Some(x) = xdg_runtime_dir {
        v.push(x.join("k2"));
    }
    v.push(PathBuf::from("/run/k2"));
    v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    v
}

/// Make `dir` 0700 owned by us (create if missing); refuse symlinks and
/// foreign owners.
///
/// Create first, then check what is there. "Look, then create" lost a
/// race: two spawns (or a spawn and a live top-up) that both saw no dir
/// both created it, the second got `AlreadyExists`, and its session
/// silently had no secrets dir (soak, shuffled seed 4).
fn ensure_private(dir: &Path) -> std::io::Result<()> {
    match private_fs::create_private_dir(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    // A symlink (even a dangling one) is `AlreadyExists` too: refused here.
    let m = std::fs::symlink_metadata(dir)?;
    if m.file_type().is_symlink() || !m.is_dir() || !private_fs::owned_by_me(dir, &m) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe secrets dir"));
    }
    if !private_fs::is_private(dir, &m) {
        private_fs::make_private_dir(dir)?;
    }
    Ok(())
}

/// Write one file `O_CREAT|O_EXCL|O_NOFOLLOW` 0600.
pub fn write_secret_file(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    k2_vault::delivery::validate_file_name(name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.hint))?;
    let mut f = private_fs::create_new_private_file(&dir.join(name))?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// The file in a session's secrets dir that holds a passport K2 re-minted
/// for it ([`deliver_passport`]). The CLI and `notify.sh` read it when the
/// session's env token is no longer the valid one.
pub const PASSPORT_FILE: &str = ".k2-passport";

/// Put a re-minted passport into THIS session's own secrets dir (`recorded`:
/// the dir its spawn recorded in the ptyd meta). The dir must be a real,
/// private, owned, direct child of the secrets root, named for the session;
/// anything else is refused, so a wrong meta can't carry a token elsewhere.
/// The file is 0600, swapped in by rename; a link planted at the name is
/// replaced, never followed. The token is never logged.
pub fn deliver_passport(session_id: &str, recorded: &Path, token: &str) -> std::io::Result<PathBuf> {
    let refused = |why: &str| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_string());
    let root = secrets_root().ok_or_else(|| refused("no secrets root"))?;
    let tag: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    let own_name = recorded.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{tag}-")));
    if recorded.parent() != Some(root.as_path()) || !own_name {
        return Err(refused("not this session's secrets dir"));
    }
    let m = std::fs::symlink_metadata(recorded)?;
    if !m.is_dir() || m.file_type().is_symlink() || !private_fs::owned_by_me(recorded, &m) || !private_fs::is_private(recorded, &m) {
        return Err(refused("secrets dir check failed"));
    }
    let tmp = recorded.join(format!("{PASSPORT_FILE}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let path = recorded.join(PASSPORT_FILE);
    let write = || -> std::io::Result<()> {
        let mut f = private_fs::create_new_private_file(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(path)
}

/// Make a fresh per-session dir under `root` (must not exist yet).
pub fn make_session_dir(root: &Path, session: &str) -> std::io::Result<PathBuf> {
    ensure_private(root)?;
    let tag: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    let dir = root.join(format!("{tag}-{}", k2_vault::crypto::base62(8)));
    private_fs::create_private_dir(&dir)?;
    let m = std::fs::symlink_metadata(&dir)?;
    if m.file_type().is_symlink() || !private_fs::owned_by_me(&dir, &m) || !private_fs::is_private(&dir, &m) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secrets dir check failed"));
    }
    Ok(dir)
}

/// What a spawn received (names only; for logs and tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Materialized {
    pub env_names: Vec<String>,
    pub files: Vec<String>,
    pub dir: Option<PathBuf>,
    pub skipped: Vec<String>,
    /// Item 3: the session's ssh-agent socket (`SSH_AUTH_SOCK`), and the
    /// names of the keys it serves at spawn.
    pub agent_sock: Option<PathBuf>,
    pub agent_keys: Vec<String>,
    /// (secret id, version) of everything delivered: the `/v1` cell audit
    /// line ([`SpawnRefs::via`]). Ids only.
    pub delivered: Vec<(String, u32)>,
    /// (file name, secret id) of each file written.
    pub file_secrets: Vec<(String, String)>,
}

/// Where a spawn's env came from, for its `k2v://` references (R8,
/// prd-env-to-vault-v1).
#[derive(Debug, Default, Clone)]
pub struct SpawnRefs {
    /// The part of the env an Owner or Admin set (an agent preset's env, an
    /// Owner/Admin v2 spawn body): a reference there resolves as the
    /// owner's choice. Counts only when the same key holds the same
    /// reference here.
    pub owner_env: HashMap<String, String>,
    /// The rest of the env is data, never a reference (a `/v1` caller's
    /// prompt, host-curated `/v1` env): left exactly as it is, unresolved
    /// (review fix 2). `false`: other references resolve only for secrets
    /// the session may have (an AGENT.md launch block).
    pub others_are_data: bool,
    /// An external `/v1` caller drives this cell (`v1:<principal>`): what
    /// it received is also written to the vault audit under that label, so
    /// it reads apart from the workspace's own agent (review fix 10).
    pub via: Option<String>,
}

/// Add this session's env/file secrets to `env`, and `SSH_AUTH_SOCK` when
/// it has `agent` items. `sandboxed` cells get env delivery only (a host
/// directory or socket isn't visible inside the cell).
///
/// `k2v://` references in `env` ([`SpawnRefs`]): an owner-set one resolves
/// as the owner's choice (audited under this session); with
/// `others_are_data` every other value stays as it is; otherwise another
/// reference (an AGENT.md launch block the agent can edit) resolves only
/// when this session may have the secret: its own workspace's or chat's at
/// env default delivery, or one granted to it or its workspace at env
/// delivery. A refused reference is removed from the env.
pub fn materialize(env: &mut HashMap<String, String>, workspace_id: &str, session_id: &str, sandboxed: bool, refs_from: &SpawnRefs) -> Materialized {
    let mut out = Materialized::default();
    let ws = workspace_id.trim();
    if ws.is_empty() {
        return out;
    }
    let Ok(vault) = super::get() else { return out };
    let who = Requester::Session { workspace_id: ws.to_string(), session_id: Some(session_id.to_string()) };
    let refs: Vec<(String, String)> = env
        .iter()
        .filter(|(_, v)| k2_vault::refs::VaultRef::is_ref(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, r) in refs {
        let owner_set = refs_from.owner_env.get(&k) == Some(&r);
        if !owner_set && refs_from.others_are_data {
            continue; // data, not a reference
        }
        match resolve_env_ref(&vault, &r, owner_set, &who) {
            Ok((v, id, version)) => {
                env.insert(k.clone(), v);
                out.env_names.push(k);
                out.delivered.push((id, version));
            }
            Err(e) => {
                env.remove(&k);
                k2_core::log_debug!("WARN [vault] spawn {session_id}: {k} not delivered: {}", e.code);
                out.skipped.push(format!("{k}: {}", e.code));
            }
        }
    }
    deliver_items(&vault, env, &who, session_id, sandboxed, &mut out);
    if let (Some(via), false) = (refs_from.via.as_deref(), out.delivered.is_empty()) {
        vault.audit_carry("v1_cell_delivery", &k2_vault::Actor::System(via.to_string()), &who.label(), &out.delivered);
    }
    // Papercut 8: a session with no file secrets yet still gets its (empty)
    // dir, so a file granted while it runs can reach it ([`top_up`]).
    // A sandboxed cell can't see a host dir: none.
    if out.dir.is_none() && !sandboxed {
        if let Some(root) = secrets_root() {
            match make_session_dir(&root, session_id) {
                Ok(d) => {
                    env.insert(SECRETS_DIR_ENV.to_string(), d.display().to_string());
                    out.dir = Some(d);
                }
                Err(e) => k2_core::log_debug!("[vault] spawn {session_id}: no secrets dir ({})", e.kind()),
            }
        }
    }
    if let Some(d) = &out.dir {
        // The session registers only after its PTY is up (seconds when
        // k2-ptyd launches); the 60 s sweep must not take the dir first.
        crate::session_liveness::note_spawning(session_id);
        DIRS.lock().push((session_id.to_string(), d.clone()));
        let delivered = Delivered { complete: true, files: out.file_secrets.iter().cloned().collect() };
        save_delivered(d, &delivered);
        LIVE.lock().insert(
            session_id.to_string(),
            LiveSession { workspace_id: ws.to_string(), env_names: Some(out.env_names.clone()), failed: HashMap::new(), delivered },
        );
    }
    out
}

/// Env and file delivery for one `k2 job` run (prd-vault-gaps-v1 item 5):
/// the same code path as a session ([`deliver_items`]), for
/// `Requester::Job`, so a run gets exactly its `job:<name>` grants (Q13).
///
/// The run's dir is NOT in the session list: the 60 s `sweep_dead` (which
/// keys on live v2 sessions) can never remove it mid-run. The job runner
/// owns its lifecycle and calls [`job_ended`] when the run ends; a daemon
/// restart sweeps it at boot like every other dir.
///
/// Item 3 (`agent` delivery): a run with `agent` grants gets its own
/// ssh-agent socket (keyed by the run, so its rate limit is its own),
/// registered with `out.dir`; [`job_ended`] stops it and removes it with
/// the dir.
pub fn materialize_job(env: &mut HashMap<String, String>, workspace_id: &str, job_id: &str, job_name: &str, run_sid: &str) -> Materialized {
    let mut out = Materialized::default();
    let Ok(vault) = super::get() else { return out };
    let who = Requester::Job { workspace_id: workspace_id.to_string(), job_id: job_id.to_string(), name: job_name.to_string() };
    deliver_items(&vault, env, &who, run_sid, false, &mut out);
    if let Some(d) = &out.dir {
        JOB_FILES.lock().extend(out.file_secrets.iter().map(|(f, id)| (d.clone(), f.clone(), id.clone())));
    }
    out
}

/// How many running `k2 job` runs still have a file of one of these secrets.
pub fn job_runs_holding(secret_ids: &[String]) -> usize {
    let mut dirs: Vec<PathBuf> = JOB_FILES
        .lock()
        .iter()
        .filter(|(d, f, id)| secret_ids.contains(id) && std::fs::symlink_metadata(d.join(f)).is_ok())
        .map(|(d, _, _)| d.clone())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs.len()
}

/// A job run ended: its secrets dir (files, and its agent socket) goes.
pub fn job_ended(m: &Materialized) {
    if let Some(d) = &m.dir {
        JOB_FILES.lock().retain(|(dir, _, _)| dir != d);
        remove_dir(d);
    }
}

/// The shared delivery loop: every env/file item `who` gets, into `env`
/// and a fresh private dir tagged `tag`, and an ssh-agent socket in that
/// dir for its `agent` items. Sessions and job runs both come through here.
fn deliver_items(
    vault: &k2_vault::LocalVault,
    env: &mut HashMap<String, String>,
    who: &Requester,
    tag: &str,
    sandboxed: bool,
    out: &mut Materialized,
) {
    let session_id = tag;
    let items = match vault.spawn_items(who) {
        Ok(i) => i,
        Err(e) => {
            k2_core::log_debug!("[vault] spawn items for {}: {}", who.label(), e.code);
            return;
        }
    };
    if items.is_empty() {
        return;
    }
    let mut dir: Option<PathBuf> = None;
    let mut agent_items: Vec<String> = vec![];
    for it in items {
        if it.delivery == Delivery::Agent {
            // Never resolved here: the agent socket signs at use time.
            if sandboxed {
                out.skipped.push(format!("{}: agent delivery isn't available in a sandboxed cell", it.name));
            } else {
                agent_items.push(it.name.clone());
            }
            continue;
        }
        let use_ = if it.delivery == Delivery::Env { Use::Env } else { Use::File };
        let r = match vault.resolve(&Selector::Id(it.secret_id.clone()), use_, who) {
            Ok(r) => {
                out.delivered.push((r.meta.id.clone(), r.version));
                r
            }
            Err(e) => {
                out.skipped.push(format!("{}: {}", it.name, e.code));
                continue;
            }
        };
        match it.delivery {
            Delivery::Env => {
                let Some(name) = it.env_name.clone() else { continue };
                match std::str::from_utf8(r.value.expose()) {
                    Ok(s) if !s.contains('\0') => {
                        env.insert(name.clone(), s.to_string());
                        out.env_names.push(name);
                    }
                    _ => out.skipped.push(format!("{}: not text, use file delivery", it.name)),
                }
            }
            _ => {
                if sandboxed {
                    out.skipped.push(format!("{}: file delivery isn't available in a sandboxed cell", it.name));
                    continue;
                }
                if dir.is_none() {
                    let Some(root) = secrets_root() else {
                        out.skipped.push(format!("{}: no_memory_fs", it.name));
                        continue;
                    };
                    match make_session_dir(&root, session_id) {
                        Ok(d) => dir = Some(d),
                        Err(e) => {
                            out.skipped.push(format!("{}: secrets dir ({})", it.name, e.kind()));
                            continue;
                        }
                    }
                }
                let d = dir.as_ref().expect("dir set above");
                let fname = it.file_name.clone().unwrap_or_else(|| it.name.clone());
                match write_secret_file(d, &fname, r.value.expose()) {
                    Ok(()) => {
                        out.file_secrets.push((fname.clone(), it.secret_id.clone()));
                        out.files.push(fname);
                    }
                    Err(e) => out.skipped.push(format!("{}: {}", it.name, e.kind())),
                }
            }
        }
    }
    if !agent_items.is_empty() {
        deliver_agent(vault, env, who, session_id, &agent_items, &mut dir, out);
    }
    if let Some(d) = &dir {
        env.insert(SECRETS_DIR_ENV.to_string(), d.display().to_string());
        out.dir = Some(d.clone());
    }
    if !out.env_names.is_empty() || !out.files.is_empty() || !out.skipped.is_empty() || !out.agent_keys.is_empty() {
        k2_core::log_debug!(
            "[vault] spawn {session_id}: env {:?} files {:?} agent {:?} skipped {:?}",
            out.env_names,
            out.files,
            out.agent_keys,
            out.skipped
        );
    }
}

/// Item 3: one ssh-agent socket for `who`'s `agent` items, in the secrets
/// dir (made here when no file made it yet). Keys the agent can't serve
/// (a rotated value that isn't an ssh key any more) are skipped by name.
fn deliver_agent(
    vault: &k2_vault::LocalVault,
    env: &mut HashMap<String, String>,
    who: &Requester,
    tag: &str,
    names: &[String],
    dir: &mut Option<PathBuf>,
    out: &mut Materialized,
) {
    let served: Vec<String> = match vault.agent_identities(who) {
        Ok(ids) => ids.into_iter().map(|i| i.name).collect(),
        Err(e) => {
            out.skipped.extend(names.iter().map(|n| format!("{n}: {}", e.code)));
            return;
        }
    };
    for n in names.iter().filter(|n| !served.contains(n)) {
        out.skipped.push(format!("{n}: not an ssh key the agent can serve"));
    }
    if served.is_empty() {
        return;
    }
    if dir.is_none() {
        let Some(root) = secrets_root() else {
            out.skipped.extend(served.iter().map(|n| format!("{n}: no_memory_fs")));
            return;
        };
        match make_session_dir(&root, tag) {
            Ok(d) => *dir = Some(d),
            Err(e) => {
                out.skipped.extend(served.iter().map(|n| format!("{n}: secrets dir ({})", e.kind())));
                return;
            }
        }
    }
    let d = dir.as_ref().expect("dir set above");
    match super::ssh_agent::start(vault, who.clone(), tag, d) {
        Ok(sock) => {
            env.insert(SSH_AUTH_SOCK_ENV.to_string(), sock.display().to_string());
            out.agent_sock = Some(sock);
            out.agent_keys = served;
        }
        Err(e) => out.skipped.extend(served.iter().map(|n| format!("{n}: agent socket ({})", e.kind()))),
    }
}

/// A `k2v://` reference in a session's env → (value, secret id, version).
/// Owner-set (`owner_set`): the owner chose it, so it resolves whenever the
/// secret's ceiling allows env; the audit names this session as the
/// consumer (`daemon:preset-env:<ws>/<sid>`, review fix 10). Otherwise the
/// session itself is the requester and needs the secret to be its own or
/// granted to it ([`k2_vault::LocalVault::resolve_ref_env`]).
fn resolve_env_ref(vault: &k2_vault::LocalVault, r: &str, owner_set: bool, who: &Requester) -> Result<(String, String, u32), k2_vault::VaultError> {
    let sel = super::selector_for_ref(r)?;
    let not_text = || k2_vault::VaultError::bad_request("not_text", "env values must be text");
    if !owner_set {
        let v = vault.resolve_ref_env(&sel, who)?;
        let s = String::from_utf8(v.value.expose().to_vec()).map_err(|_| not_text())?;
        return Ok((s, v.meta.id.clone(), v.version));
    }
    let m = vault.meta(&sel, &k2_vault::Actor::System("preset-env".into()))?;
    if m.delivery_ceiling.level() < Delivery::Env.level() {
        return Err(k2_vault::VaultError::forbidden("delivery_above_ceiling", "this secret can't be delivered as env"));
    }
    let consumer = who.label();
    let label = format!("preset-env:{}", consumer.strip_prefix("session:").unwrap_or(&consumer));
    let v = vault.resolve(&Selector::Id(m.id), Use::System, &Requester::System(label))?;
    let s = String::from_utf8(v.value.expose().to_vec()).map_err(|_| not_text())?;
    Ok((s, v.meta.id.clone(), v.version))
}

/// Session ended: its secrets dir and placeholders go.
pub fn session_ended(session_id: &str) {
    let _turn = TOPUP.lock();
    crate::session_liveness::forget_spawning(session_id);
    LIVE.lock().remove(session_id);
    let mine: Vec<PathBuf> = {
        let mut g = DIRS.lock();
        let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|(s, _)| s == session_id);
        *g = keep;
        gone.into_iter().map(|(_, d)| d).collect()
    };
    for d in mine {
        remove_dir(&d);
    }
    if let Ok(v) = super::get() {
        let _ = v.placeholders_drop_session(session_id);
    }
}

fn remove_dir(d: &Path) {
    // Item 3: the agent stops before its socket and dir go.
    super::ssh_agent::stop_dir(d);
    if dir_is_safe_to_empty(d) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                wipe_entry(&e.path());
            }
        }
        let _ = std::fs::remove_dir(d);
    } else {
        // A link swapped in for the dir: the link goes, what it points at stays.
        let _ = std::fs::remove_file(d);
    }
    if let Some(p) = delivered_path(d) {
        let _ = std::fs::remove_file(&p);
    }
}

/// The dir is really a dir (never a link to one, `lstat`), or isn't there.
/// Any same-user process can swap it, so removal looks before it reaches in.
fn dir_is_safe_to_empty(d: &Path) -> bool {
    match std::fs::symlink_metadata(d) {
        Ok(m) => m.is_dir() && !m.file_type().is_symlink() && private_fs::owned_by_me(d, &m),
        Err(_) => true,
    }
}

/// Remove one entry of a secrets dir; a regular file we own is zeroed
/// first. Never blocks and never follows a link: the dir is writable by
/// any same-user process, so a FIFO, socket or link planted there (even
/// swapped in after the stat) is only unlinked. The open is
/// `O_WRONLY|O_NOFOLLOW|O_NONBLOCK` and the zeroing re-checks the open
/// file (`fstat`): a regular file, ours, with one link (a hard link to
/// another file of ours is unlinked, never zeroed), at most 16 MiB. A
/// planted subdirectory is removed with `std::fs::remove_dir_all`, which
/// on Unix walks with `openat`/`O_NOFOLLOW` and never follows a symlink,
/// even one swapped in mid-walk; nothing in it is zeroed.
pub(crate) fn wipe_entry(p: &Path) {
    let Ok(lm) = std::fs::symlink_metadata(p) else { return };
    if lm.file_type().is_dir() {
        let _ = std::fs::remove_dir_all(p);
        return;
    }
    if lm.file_type().is_file() {
        if let Ok(mut f) = private_fs::open_write_nofollow(p) {
            if let Ok(m) = f.metadata() {
                if m.is_file()
                    && private_fs::owned_by_me(p, &m)
                    && private_fs::link_count(&f).ok() == Some(1)
                    && m.len() <= 16 * 1024 * 1024
                {
                    let _ = f.write_all(&vec![0u8; m.len() as usize]);
                    let _ = f.sync_all();
                }
            }
        }
    }
    let _ = std::fs::remove_file(p);
}

/// The secrets dir the boot sweep kept for a surviving session (item 3:
/// its agent socket is re-served only into this dir).
pub fn kept_dir(session_id: &str) -> Option<PathBuf> {
    DIRS.lock().iter().find(|(s, _)| s == session_id).map(|(_, d)| d.clone())
}

/// Boot: remove every secrets dir except those of agent sessions that
/// survived the restart in k2-ptyd (PRD prd-k2-ptyd-v1 P36: B0's running
/// set, each with the `secrets_dir` its spawn recorded). Those are kept
/// and re-registered in `DIRS`, so they go when their session ends.
/// Without k2-ptyd (or with nothing running in it) nothing survives a
/// restart and every dir goes, as before.
///
/// Item 3: agent socket records of sessions that didn't survive are
/// dropped (with their sockets), and orphaned short-path socket dirs go
/// ([`super::ssh_agent::boot_sweep`]). A survivor's socket is re-served
/// only once k2-ptyd adoption has registered the session
/// ([`super::ssh_agent::on_adopted`]), from the daemon's own record.
pub fn sweep_at_boot() {
    let survivors = surviving_dirs();
    let keep: Vec<(String, PathBuf)> = survivors.iter().map(|(s, d, _)| (s.clone(), d.clone())).collect();
    let kept = sweep_at_boot_keeping(&keep);
    // Live delivery for the survivors (papercut 8): their workspace from
    // the ptyd meta; the env they got at spawn isn't known; the files K2
    // delivered from their record.
    for (sid, dir) in &kept {
        if let Some((_, _, Some(ws))) = survivors.iter().find(|(s, _, _)| s == sid) {
            register_survivor(sid, ws, dir);
        }
    }
    if let Ok(v) = super::get() {
        super::ssh_agent::boot_sweep(&v, &kept);
    }
}

/// The `(session id, secrets dir, workspace)` B0 found running in ptyd.
fn surviving_dirs() -> Vec<(String, PathBuf, Option<String>)> {
    #[cfg(unix)]
    {
        k2_core::terminal::ptyd::adopt::running()
            .into_iter()
            .filter_map(|s| {
                let dir = s.meta.get("secrets_dir")?.as_str()?.to_string();
                let ws = s.meta.get("workspace_uuid").and_then(|v| v.as_str()).filter(|w| !w.is_empty()).map(str::to_string);
                Some((s.sid, PathBuf::from(dir), ws))
            })
            .collect()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// [`sweep_at_boot`] with an explicit keep set. A kept dir must be a
/// direct child of the secrets root named for its session (`<tag>-<8>`,
/// [`make_session_dir`]); anything else in `keep` is ignored. Returns the
/// dirs kept.
pub fn sweep_at_boot_keeping(keep: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let Some(root) = secrets_root() else { return vec![] };
    sweep_root_keeping(&root, keep)
}

fn sweep_root_keeping(root: &Path, keep: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let root = root.to_path_buf();
    let kept: Vec<(String, PathBuf)> = keep
        .iter()
        .filter(|(sid, dir)| {
            let tag: String = sid.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
            dir.parent() == Some(root.as_path())
                && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{tag}-")))
        })
        .cloned()
        .collect();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) {
                let p = e.path();
                if kept.iter().any(|(_, d)| d == &p) {
                    continue;
                }
                remove_dir(&p);
            } else if e.file_name().to_str().is_some_and(|n| n.ends_with(DELIVERED_SUFFIX) || n.ends_with(".delivered.tmp")) {
                // A record whose session is gone.
                let p = e.path();
                if !kept.iter().any(|(_, d)| delivered_path(d).as_ref() == Some(&p)) {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
    let mut g = DIRS.lock();
    let mut out = vec![];
    for (sid, dir) in kept {
        if dir.is_dir() {
            if !g.iter().any(|(s, d)| s == &sid && d == &dir) {
                g.push((sid.clone(), dir.clone()));
            }
            out.push((sid, dir));
        }
    }
    out
}

/// What live delivery did for one session (names only).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TopUp {
    /// The session's secrets dir (`None`: it has none: it started before
    /// 0.46.5, runs sandboxed, or the machine has no memory filesystem).
    pub dir: Option<PathBuf>,
    /// Files written into the dir now.
    pub delivered: Vec<String>,
    /// Granted files already in the dir.
    pub present: Vec<String>,
    /// Granted, but they can't reach a running session: `NAME (how): why`.
    pub restart: Vec<String>,
    /// Granted, but delivery failed: `NAME: code`.
    pub skipped: Vec<String>,
    /// Files taken out now, their secret no longer this session's:
    /// `(file name, secret id)`.
    pub removed: Vec<(String, String)>,
    /// Files that should have gone and are still there (kept on the record,
    /// tried again on the next pass): `(file name, secret id)`.
    pub not_removed: Vec<(String, String)>,
    /// The vault couldn't say what this session gets: nothing was checked.
    pub unchecked: Option<String>,
}

/// What one pass over every live session did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LiveSweep {
    /// Files written.
    pub written: usize,
    /// One `(file name, secret id)` per session a file was taken from.
    pub removed: Vec<(String, String)>,
    /// One `(file name, secret id)` per session a file wouldn't come out of.
    pub not_removed: Vec<(String, String)>,
    /// One per session the vault couldn't check: the error code, and the
    /// secrets on that session's record.
    pub unchecked: Vec<(String, Vec<String>)>,
    /// One per session holding files K2 has no record of (from before the
    /// record existed, or with no workspace known): those file names. K2
    /// can't tell whose they are.
    pub untracked: Vec<Vec<String>>,
}

/// Live delivery for one session: the caller's own (`k2 vault sync`).
/// `workspace_id` is the caller's: a session K2 recorded under another
/// workspace gets nothing.
pub fn top_up(vault: &k2_vault::LocalVault, session_id: &str, workspace_id: &str) -> TopUp {
    let _turn = TOPUP.lock();
    let dir = DIRS.lock().iter().find(|(s, _)| s == session_id).map(|(_, d)| d.clone());
    let mut live = LIVE.lock();
    match live.get_mut(session_id) {
        Some(r) if r.workspace_id != workspace_id => TopUp::default(),
        Some(r) => top_up_one(vault, session_id, workspace_id, dir.as_deref(), r.env_names.as_deref(), &mut r.failed, &mut r.delivered, true),
        None => {
            let mut delivered = dir.as_deref().map(load_delivered).unwrap_or_default();
            top_up_one(vault, session_id, workspace_id, dir.as_deref(), None, &mut HashMap::new(), &mut delivered, true)
        }
    }
}

/// Regular files in `dir` K2 has no record of. Never through a swapped link.
fn unrecorded_files(dir: &Path, d: &Delivered) -> Vec<String> {
    if !dir_is_safe_to_empty(dir) {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| !d.files.contains_key(n) && n != PASSPORT_FILE)
        .collect()
}

/// Live delivery for every live session with a dir (after a grant or a
/// set, and on the vault's minute tick). Returns how many files it wrote.
pub fn top_up_all(vault: &k2_vault::LocalVault) -> usize {
    sweep_live(vault).written
}

/// The vault's minute pass for running sessions: a session card's secret
/// follows its chat to the chat's new session (after a restart or
/// `--resume`), then every session gets what it is granted now.
pub fn follow_and_top_up(vault: &k2_vault::LocalVault) -> usize {
    crate::overlay_routes::follow_session_cards(vault);
    top_up_all(vault)
}

/// One pass over every live session with a dir: files it now gets go in,
/// files it no longer gets come out. After a destroy, a disable or a revoke
/// this is what takes the file back at once; the vault's minute tick catches
/// a grant or a secret whose time ran out.
///
/// Only the served vault ([`super::get`]) sweeps: every live session got
/// its secrets from it ([`materialize`]), so another vault can't say what
/// a session may keep. A vault that isn't served (one a restore just
/// replaced, or a test's own) taking files back would take what the
/// served vault still grants: route tests on their own vaults took live
/// sessions' files from other tests (soak, shuffled vault runs). The
/// minute ticker already stops for a vault that isn't served.
pub fn sweep_live(vault: &k2_vault::LocalVault) -> LiveSweep {
    if !super::get().is_ok_and(|served| std::ptr::eq(&*served, vault)) {
        k2_core::log_debug!("[vault] live sweep skipped: this vault isn't the one served");
        return LiveSweep::default();
    }
    let _turn = TOPUP.lock();
    let dirs: Vec<(String, PathBuf)> = DIRS.lock().clone();
    let mut out = LiveSweep::default();
    for (sid, dir) in dirs {
        let mut live = LIVE.lock();
        let Some(r) = live.get_mut(&sid) else {
            drop(live);
            let names = unrecorded_files(&dir, &Delivered::default());
            if !names.is_empty() {
                out.untracked.push(names);
            }
            continue;
        };
        let ws = r.workspace_id.clone();
        let t = top_up_one(vault, &sid, &ws, Some(&dir), r.env_names.as_deref(), &mut r.failed, &mut r.delivered, false);
        if !r.delivered.complete {
            let names = unrecorded_files(&dir, &r.delivered);
            if !names.is_empty() {
                out.untracked.push(names);
            }
        }
        if let Some(code) = &t.unchecked {
            out.unchecked.push((code.clone(), r.delivered.files.values().cloned().collect()));
        }
        drop(live);
        if !t.delivered.is_empty() || !t.skipped.is_empty() || !t.removed.is_empty() || !t.not_removed.is_empty() {
            let names = |v: &[(String, String)]| v.iter().map(|(f, _)| f.clone()).collect::<Vec<_>>();
            k2_core::log_debug!(
                "[vault] live delivery {sid}: files {:?} skipped {:?} removed {:?} couldn't remove {:?}",
                t.delivered,
                t.skipped,
                names(&t.removed),
                names(&t.not_removed)
            );
        }
        out.written += t.delivered.len();
        out.removed.extend(t.removed);
        out.not_removed.extend(t.not_removed);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn top_up_one(
    vault: &k2_vault::LocalVault,
    session_id: &str,
    workspace_id: &str,
    dir: Option<&Path>,
    env_names: Option<&[String]>,
    failed: &mut HashMap<String, Instant>,
    delivered: &mut Delivered,
    retry_now: bool,
) -> TopUp {
    let mut out = TopUp { dir: dir.map(Path::to_path_buf), ..Default::default() };
    let who = Requester::Session { workspace_id: workspace_id.to_string(), session_id: Some(session_id.to_string()) };
    let items = match items_for(vault, &who) {
        Ok(i) => i,
        Err(e) => {
            out.skipped.push(format!("*: {}", e.code));
            out.unchecked = Some(e.code.to_string());
            return out;
        }
    };
    let mut record_changed = false;
    let now = Instant::now();
    failed.retain(|_, t| now.duration_since(*t) < RETRY_AFTER);
    // A file K2 delivered whose secret this session no longer gets (destroyed,
    // disabled, unshared, expired) goes first, so a secret of the same name it
    // does get can take its place below. Only files on K2's record: a file
    // already in the folder under a secret's name was never K2's. A secret
    // it still gets at another delivery (a grant moved it to env) keeps the
    // file. One that won't come out stays on the record for the next pass.
    if let Some(d) = dir {
        let safe = dir_is_safe_to_empty(d);
        delivered.files.retain(|fname, id| {
            if items.iter().any(|it| it.secret_id == *id) {
                return true;
            }
            if !safe {
                out.not_removed.push((fname.clone(), id.clone()));
                return true;
            }
            let p = d.join(fname.as_str());
            if std::fs::symlink_metadata(&p).is_ok() {
                wipe_entry(&p);
                if std::fs::symlink_metadata(&p).is_ok() {
                    out.not_removed.push((fname.clone(), id.clone()));
                    return true;
                }
                out.removed.push((fname.clone(), id.clone()));
            }
            record_changed = true;
            false
        });
    }
    let mut dir_ok = false;
    for it in items {
        match it.delivery {
            Delivery::Env => {
                let Some(env) = it.env_name.as_deref() else { continue };
                if env_names.is_some_and(|got| !got.iter().any(|g| g == env)) {
                    out.restart.push(format!("{} (env ${env}): {RESTART_TO_RECEIVE}", it.name));
                }
            }
            Delivery::Agent => {
                if dir.and_then(super::ssh_agent::socket_for).is_none() {
                    out.restart.push(format!("{} (ssh key, $SSH_AUTH_SOCK): {RESTART_TO_RECEIVE}", it.name));
                }
            }
            _ => {
                let fname = it.file_name.clone().unwrap_or_else(|| it.name.clone());
                let Some(d) = dir else {
                    out.restart.push(format!("{} (file; this session has no $K2_SECRETS_DIR): {RESTART_TO_RECEIVE}", it.name));
                    continue;
                };
                if !dir_ok {
                    if let Err(e) = ensure_session_dir(d) {
                        out.skipped.push(format!("{}: secrets dir ({})", it.name, e.kind()));
                        continue;
                    }
                    dir_ok = true;
                }
                if std::fs::symlink_metadata(d.join(&fname)).is_ok() {
                    out.present.push(fname);
                    continue;
                }
                if !retry_now && failed.contains_key(&it.secret_id) {
                    continue;
                }
                let r = match vault.resolve(&Selector::Id(it.secret_id.clone()), Use::File, &who) {
                    Ok(r) => r,
                    Err(e) => {
                        failed.insert(it.secret_id.clone(), now);
                        out.skipped.push(format!("{}: {}", it.name, e.code));
                        continue;
                    }
                };
                match write_secret_file(d, &fname, r.value.expose()) {
                    Ok(()) => {
                        failed.remove(&it.secret_id);
                        delivered.files.insert(fname.clone(), it.secret_id.clone());
                        record_changed = true;
                        out.delivered.push(fname);
                    }
                    // Made by someone else since the check: it's there.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => out.present.push(fname),
                    Err(e) => {
                        failed.insert(it.secret_id.clone(), now);
                        out.skipped.push(format!("{}: {}", it.name, e.kind()));
                    }
                }
            }
        }
    }
    if let (true, Some(d)) = (record_changed, dir) {
        save_delivered(d, delivered);
    }
    out
}

/// A live session's dir is there, private and ours. A dir that went
/// missing under a live session (field report 10-24) is made again at the
/// same path, which its `$K2_SECRETS_DIR` still names, but only directly
/// under this machine's secrets root.
fn ensure_session_dir(d: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(d) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || !private_fs::owned_by_me(d, &m) || !private_fs::is_private(d, &m) {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secrets dir check failed"));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let root = secrets_root().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no_memory_fs"))?;
            if d.parent() != Some(root.as_path()) {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not under the secrets root"));
            }
            ensure_private(&root)?;
            private_fs::create_private_dir(d)?;
            k2_core::log_debug!("[vault] WARN: secrets dir {} was gone under a live session; made it again", d.display());
            let m = std::fs::symlink_metadata(d)?;
            if m.file_type().is_symlink() || !m.is_dir() || !private_fs::owned_by_me(d, &m) {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secrets dir check failed"));
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Periodic: drop dirs whose session is gone. `live` is
/// [`crate::session_liveness`]'s answer (registered, waiting for adoption,
/// or still being spawned).
pub fn sweep_dead(live: &dyn Fn(&str) -> bool) {
    let dead: Vec<String> = DIRS.lock().iter().filter(|(s, _)| !live(s)).map(|(s, _)| s.clone()).collect();
    for s in dead {
        session_ended(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// 0700 dir / 0600 file on Unix; an owner-only DACL on Windows.
    #[track_caller]
    fn assert_private(p: &Path, unix_mode: u32) {
        #[cfg(unix)]
        assert_eq!(std::fs::metadata(p).expect("m").permissions().mode() & 0o777, unix_mode, "{}", p.display());
        #[cfg(windows)]
        {
            let _ = unix_mode;
            let m = std::fs::metadata(p).expect("m");
            assert!(private_fs::is_private(p, &m), "{} is not owner-only", p.display());
        }
    }

    /// A re-minted passport reaches only the session's own private dir, as a
    /// 0600 file, replaces an older one, and a link at its name isn't followed.
    #[cfg(unix)]
    #[test]
    fn a_passport_goes_only_to_the_sessions_own_private_dir() {
        let _home = crate::test_support::TempHome::short();
        let root = secrets_root().expect("root");
        let sid = "11111111-1111-4111-8111-111111111111";
        let dir = make_session_dir(&root, sid).expect("dir");
        let path = deliver_passport(sid, &dir, "sid.one").expect("delivered");
        assert_eq!(path, dir.join(PASSPORT_FILE));
        assert_private(&path, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sid.one\n");
        deliver_passport(sid, &dir, "sid.two").expect("replaced");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sid.two\n");

        // Planted link: replaced, its target untouched.
        let bait = dir.join("bait");
        std::fs::write(&bait, "kept").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&bait, &path).unwrap();
        deliver_passport(sid, &dir, "sid.three").expect("over a link");
        assert_eq!(std::fs::read_to_string(&bait).unwrap(), "kept");
        assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());

        // Another session's dir, a dir outside the root, a link, an open dir.
        let other = make_session_dir(&root, "22222222-2222-4222-8222-222222222222").expect("other");
        assert!(deliver_passport(sid, &other, "x").is_err(), "another session's dir");
        assert!(!other.join(PASSPORT_FILE).exists());
        let t = k2_vault::fsutil::TestDir::new("passport-outside");
        let outside = t.path().join(format!("{sid}-abc"));
        std::fs::create_dir_all(&outside).unwrap();
        assert!(deliver_passport(sid, &outside, "x").is_err(), "outside the root");
        let link = root.join(format!("{sid}-link"));
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(deliver_passport(sid, &link, "x").is_err(), "a link to a dir");
        let open = root.join(format!("{sid}-open"));
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(deliver_passport(sid, &open, "x").is_err(), "an open dir");

        // K2's own bookkeeping doesn't count it as a stray file.
        assert!(!unrecorded_files(&dir, &Delivered::default()).contains(&PASSPORT_FILE.to_string()));
    }

    /// Many callers making the same missing dir at once all get it (one
    /// creates it, the rest find it), and the dir is 0700. "Look, then
    /// create" gave the losers `AlreadyExists`, so a spawn racing another
    /// one had no secrets dir. A symlink there is still refused.
    #[test]
    fn ensure_private_holds_when_callers_race_to_make_the_dir() {
        let t = k2_vault::fsutil::TestDir::new("ensure-race");
        for round in 0..50 {
            let dir = t.path().join(format!("root-{round}"));
            let start = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (dir, start) = (dir.clone(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        ensure_private(&dir).map_err(|e| e.kind())
                    })
                })
                .collect();
            for h in handles {
                assert_eq!(h.join().expect("thread"), Ok(()), "round {round}: every caller gets the dir");
            }
            assert_private(&dir, 0o700);
        }
        #[cfg(unix)]
        {
            let link = t.path().join("link");
            std::os::unix::fs::symlink(t.path().join("elsewhere"), &link).expect("symlink");
            assert_eq!(ensure_private(&link).map_err(|e| e.kind()), Err(std::io::ErrorKind::PermissionDenied), "a dangling symlink is refused");
            std::fs::create_dir(t.path().join("elsewhere")).expect("target");
            assert_eq!(ensure_private(&link).map_err(|e| e.kind()), Err(std::io::ErrorKind::PermissionDenied), "a symlink to a dir is refused");
        }
    }

    /// k2-ptyd P36: boot keeps the secrets dir of a session that survived
    /// in k2-ptyd (and re-registers it so it goes with the session); every
    /// other dir goes; a keep entry outside the root, or not named for its
    /// session, is ignored.
    #[test]
    fn boot_sweep_keeps_only_surviving_sessions_dirs() {
        let root = std::env::temp_dir().join(format!("k2-secrets-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sid = "4f6e9a1c-1111-4222-8333-944455556666";
        let kept = make_session_dir(&root, sid).unwrap();
        write_secret_file(&kept, "token", b"x").unwrap();
        let gone = make_session_dir(&root, "0b4d1f7e-dead-4c66-9d11-3a3e8f6a7c21").unwrap();
        let foreign = make_session_dir(&root, "other-session").unwrap();
        let outside = std::env::temp_dir().join(format!("k2-secrets-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        sweep_root_keeping(
            &root,
            &[(sid.to_string(), kept.clone()), (sid.to_string(), foreign.clone()), (sid.to_string(), outside.clone())],
        );
        assert!(kept.join("token").is_file(), "the survivor's dir and file stay");
        assert!(!gone.exists(), "a dead session's dir goes");
        assert!(!foreign.exists(), "a dir not named for the kept session goes");
        assert!(outside.exists(), "nothing outside the root is touched");
        assert!(DIRS.lock().iter().any(|(s, d)| s == sid && d == &kept), "re-registered for session end");
        session_ended(sid);
        assert!(!kept.exists(), "goes when the session ends");
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[cfg(unix)]
    fn mkfifo(p: &Path) {
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).expect("path");
        // SAFETY: a valid C path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {}", p.display());
    }

    // pending Windows vault ssh-agent (named pipe): a Unix-socket client.
    #[cfg(unix)]
    fn list_ids(sock: &Path) -> Vec<String> {
        use std::io::{Read, Write};
        let mut s = std::os::unix::net::UnixStream::connect(sock).expect("connect");
        s.set_read_timeout(Some(std::time::Duration::from_secs(10))).expect("timeout");
        s.write_all(&k2_vault::ssh_agent::frame(&k2_vault::ssh_agent::client::identities())).expect("write");
        let mut len = [0u8; 4];
        s.read_exact(&mut len).expect("len");
        let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
        s.read_exact(&mut buf).expect("body");
        k2_vault::ssh_agent::client::parse_identities(&buf).expect("identities").into_iter().map(|(_, n)| n).collect()
    }

    /// Item 3 + k2-ptyd: after a restart, a surviving session's dir is kept
    /// but its agent socket is re-served only after adoption, only from
    /// the daemon's own record, and only when the ptyd meta agrees with
    /// it. A dead session's dir, socket and record go; a record that points
    /// at another session's socket is never followed; a FIFO planted in a
    /// secrets dir never hangs the sweep.
    // pending Windows vault ssh-agent (named pipe): serves a Unix socket.
    #[cfg(unix)]
    #[test]
    fn boot_re_serves_an_agent_socket_only_from_the_daemons_record() {
        use super::super::ssh_agent::{self as agent, ResumeRefused};
        use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes};
        let _env = k2_core::test_env::lock();
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), "BOOT_KEY", Kind::SshKey);
        r.default_delivery = Some(Delivery::Agent);
        v.put(r, SecretBytes::from_slice(k2_vault::ssh_agent::testkeys::ed25519().as_bytes()), &Actor::Owner).expect("put");
        let t = k2_vault::fsutil::TestDir::new("boot-agent");
        let root = t.path().join("secrets");
        let live = uuid::Uuid::new_v4().to_string();
        let dead = uuid::Uuid::new_v4().to_string();
        let who = |s: &str| Requester::Session { workspace_id: ws.clone(), session_id: Some(s.to_string()) };
        let live_dir = make_session_dir(&root, &live).expect("dir");
        let dead_dir = make_session_dir(&root, &dead).expect("dir");
        let live_sock = agent::start(&v, who(&live), &live, &live_dir).expect("start");
        let dead_sock = agent::start(&v, who(&dead), &dead, &dead_dir).expect("start");
        assert!(v.agent_socket_get(&live).expect("rec").is_some(), "the daemon recorded the socket");
        // k2-ptyd spawned `live`: its record is bound to that instance and
        // process (a real, live process: the OS is checked too).
        let (mut child, cpid, cstart, ckey) = agent::live_child_for_tests();
        assert!(v.agent_socket_bind(&live, "boot-1", cpid, cstart, ckey).expect("bind"));
        assert!(!v.agent_socket_bind(&live, "boot-1", cpid + 1, cstart, ckey).expect("rebind"), "bound once, for the session's life");
        assert!(!v.agent_socket_bind(&live, "boot-1", cpid, cstart, ckey + 1).expect("rebind"), "bound once, to one start key");
        let proc = agent::Adopted { boot_id: "boot-1", pid: cpid, started_at: cstart };
        // Planted FIFOs (a reader-less FIFO blocks a plain open for write).
        mkfifo(&dead_dir.join("a.sock.path"));
        mkfifo(&dead_dir.join("token"));
        mkfifo(&live_dir.join("a.sock.path"));
        // The daemon goes away; k2-ptyd keeps `live` running.
        agent::abandon_for_tests(&live_dir);
        agent::abandon_for_tests(&dead_dir);
        std::thread::sleep(std::time::Duration::from_millis(700));
        let t0 = std::time::Instant::now();
        let kept = sweep_root_keeping(&root, &[(live.clone(), live_dir.clone())]);
        agent::boot_sweep(&v, &kept);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "the boot sweep must not block on a FIFO");
        assert!(!dead_dir.exists() && !dead_sock.exists(), "a dead session's dir and socket go");
        assert!(v.agent_socket_get(&dead).expect("rec").is_none(), "and its record");
        assert!(agent::socket_for(&live_dir).is_none(), "nothing is served before adoption");
        assert_eq!(kept_dir(&live).as_deref(), Some(live_dir.as_path()));

        // Adoption with a ptyd meta whose workspace was rewritten: refused.
        let other_ws = uuid::Uuid::new_v4().to_string();
        let e = agent::resume_adopted(&v, &live, proc, Some(&other_ws), Some(&live_dir), Some(&live_dir)).expect_err("rewritten meta");
        assert_eq!(e, ResumeRefused::WorkspaceMismatch);
        // Another process under the same sid (a forged spawn), or another ptyd: refused.
        let forged = agent::Adopted { pid: cpid + 100_000, ..proc };
        let e = agent::resume_adopted(&v, &live, forged, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("forged spawn");
        assert_eq!(e, ResumeRefused::ProcessMismatch);
        let other_ptyd = agent::Adopted { boot_id: "boot-2", ..proc };
        let e = agent::resume_adopted(&v, &live, other_ptyd, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("other ptyd");
        assert_eq!(e, ResumeRefused::ProcessMismatch);
        let e = agent::resume_adopted(&v, &live, proc, Some(&ws), Some(&dead_dir), Some(&live_dir)).expect_err("rewritten dir");
        assert_eq!(e, ResumeRefused::DirMismatch);
        assert!(agent::socket_for(&live_dir).is_none());

        // A record that points at the live session's socket from another
        // session's dir is ignored: not served, and its stop leaves the
        // live socket alone.
        let x = uuid::Uuid::new_v4().to_string();
        let x_dir = make_session_dir(&root, &x).expect("dir");
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket {
            tag: x.clone(),
            kind: "session".into(),
            workspace_id: ws.clone(),
            secrets_dir: x_dir.to_string_lossy().into_owned(),
            sock_path: live_sock.to_string_lossy().into_owned(),
            short_root: v.agent_socket_get(&live).expect("rec").expect("live rec").short_root,
            created_at: "2026-10-24T00:00:00Z".into(),
            ptyd_boot: "boot-1".into(),
            pid: 5151,
            started_at: 1_700_000_001,
            proc_start: 4242,
        })
        .expect("forged record");
        let xp = agent::Adopted { boot_id: "boot-1", pid: 5151, started_at: 1_700_000_001 };
        let e = agent::resume_adopted(&v, &x, xp, Some(&ws), Some(&x_dir), Some(&x_dir)).expect_err("points elsewhere");
        assert_eq!(e, ResumeRefused::BadSocketPath);
        agent::stop_dir(&x_dir);
        assert!(std::fs::symlink_metadata(&live_sock).is_ok(), "another record never removes this socket");
        assert!(v.agent_socket_get(&x).expect("rec").is_none());

        // The real adoption: same workspace, same dir: re-served at the same path.
        let t0 = std::time::Instant::now();
        let saved = v.agent_socket_get(&live).expect("rec").expect("live rec");
        let sock = agent::resume_adopted(&v, &live, proc, Some(&ws), Some(&live_dir), Some(&live_dir)).expect("re-served");
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(sock, live_sock, "SSH_AUTH_SOCK in the session stays valid");
        assert_eq!(agent::socket_for(&live_dir), Some(live_sock.clone()));
        assert_eq!(list_ids(&live_sock), vec!["BOOT_KEY".to_string()]);
        let t0 = std::time::Instant::now();
        session_ended(&live);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "session end must not block on a FIFO");
        assert!(!live_dir.exists() && std::fs::symlink_metadata(&live_sock).is_err(), "and the survivor's go at its end");
        assert!(v.agent_socket_get(&live).expect("rec").is_none());

        // Replay: the saved (validly MAC'd) record put back after the
        // session ended, plus a forged ptyd spawn under the same sid and
        // dir: the forged process isn't the bound one.
        v.agent_socket_put_unchecked_for_tests(&saved).expect("replayed record");
        let forged = agent::Adopted { boot_id: "boot-1", pid: 7777, started_at: 1_700_000_500 };
        let e = agent::resume_adopted(&v, &live, forged, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("replayed record");
        assert_eq!(e, ResumeRefused::ProcessMismatch);
        // A record never bound to ptyd is never re-served.
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket { ptyd_boot: String::new(), pid: 0, started_at: 0, proc_start: 0, ..saved.clone() })
            .expect("unbound record");
        let e = agent::resume_adopted(&v, &live, proc, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("not ptyd");
        assert_eq!(e, ResumeRefused::NotPtyd);
        // Bound to ptyd but without a kernel start key (a record from a
        // build before the key): never re-served.
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket { proc_start: 0, ..saved.clone() }).expect("keyless record");
        let e = agent::resume_adopted(&v, &live, proc, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("no key");
        assert_eq!(e, ResumeRefused::NotPtyd);
        // A fake ptyd repeating a replayed record's values: the OS says the
        // pid is dead, or isn't the process bound at spawn.
        let mut dead = std::process::Command::new("true").spawn().expect("spawn true");
        let dead_pid = i64::from(dead.id());
        dead.wait().expect("reap");
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket { pid: dead_pid, started_at: cstart, ..saved.clone() })
            .expect("dead-pid record");
        let fake = agent::Adopted { boot_id: "boot-1", pid: dead_pid, started_at: cstart };
        let e = agent::resume_adopted(&v, &live, fake, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("dead pid");
        assert_eq!(e, ResumeRefused::ProcessGone);
        // The live pid with another start key (a reused pid).
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket { proc_start: ckey + 1, ..saved.clone() })
            .expect("wrong-key record");
        let e = agent::resume_adopted(&v, &live, proc, Some(&ws), Some(&live_dir), Some(&live_dir)).expect_err("wrong start key");
        assert_eq!(e, ResumeRefused::ProcessGone);
        // A wall-clock step between spawn and adoption: ptyd's recorded
        // start time no longer lines up with the wall clock, the kernel's
        // tick count is the same. Linux re-serves (exact ticks, no wall
        // time); macOS keeps its start-time check (its start timeval is
        // stored at fork, so a step doesn't move it either).
        let stepped = agent::Adopted { boot_id: "boot-1", pid: cpid, started_at: cstart - 100 };
        v.agent_socket_put_unchecked_for_tests(&k2_vault::vault::AgentSocket { started_at: cstart - 100, ..saved.clone() })
            .expect("stepped record");
        // (The session's dir again: it went at the session's end above.)
        std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).create(&live_dir).expect("dir again");
        let r = agent::resume_adopted(&v, &live, stepped, Some(&ws), Some(&live_dir), Some(&live_dir));
        #[cfg(target_os = "linux")]
        {
            assert_eq!(r.expect("a clock step still matches on Linux"), live_sock);
            assert_eq!(list_ids(&live_sock), vec!["BOOT_KEY".to_string()], "re-served");
            agent::stop_dir(&live_dir);
        }
        #[cfg(target_os = "macos")]
        assert_eq!(r.expect_err("macOS compares ptyd's time with the kernel's"), ResumeRefused::ProcessGone);
        std::fs::remove_dir_all(&live_dir).expect("rm dir");
        v.agent_socket_remove(&live).expect("cleanup");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Item 3: short-path socket dirs in this vault's roots that no record
    /// names (a crash) go at boot, with no connect probe: a planted
    /// never-accepting listener (backlog 0) can't stall it. Named ones and
    /// other namespaces' stay; a symlinked dir is never followed. Runs
    /// under a temp `$TMPDIR` (env lock), never the real short-path root.
    // pending Windows vault ssh-agent (named pipe): Unix socket dirs.
    #[cfg(unix)]
    #[test]
    fn boot_removes_orphaned_short_path_socket_dirs() {
        use super::super::ssh_agent as agent;
        let _env = k2_core::test_env::lock();
        // A short temp TMPDIR (socket paths must fit sun_path), removed after:
        // what this test made in it, then the folder if that left it empty.
        // A test reading temp_dir() without the lock may have put files here
        // while TMPDIR pointed at it; deleting them failed that test
        // (`mail::secrets`, soak 10-30, seed 6).
        struct Rm(PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                for entry in std::fs::read_dir(&self.0).into_iter().flatten().flatten() {
                    let name = entry.file_name();
                    if name == "elsewhere" || name.to_string_lossy().starts_with("k2a-") {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let tmpdir = PathBuf::from(format!("/tmp/k2t-{}", k2_vault::crypto::base62(8)));
        std::fs::create_dir(&tmpdir).expect("tmpdir");
        let _rm = Rm(tmpdir.clone());
        let _t = k2_core::test_env::EnvVar::set("TMPDIR", &tmpdir);
        let v = super::super::ensure_test_vault();
        let ns = agent::namespace(&v);
        let root = agent::fallback_roots(&ns).into_iter().next().expect("a root");
        assert!(root.starts_with(&tmpdir), "{}", root.display());
        std::fs::create_dir_all(&root).expect("root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let orphan = root.join(k2_vault::crypto::base62(8));
        std::fs::create_dir(&orphan).expect("orphan");
        let l = std::os::unix::net::UnixListener::bind(orphan.join("a.sock")).expect("bind");
        drop(l);
        // A planted listener that never accepts, backlog 0 (a blocking
        // connect to it would hang on Linux).
        let stall = root.join(k2_vault::crypto::base62(8));
        std::fs::create_dir(&stall).expect("stall");
        let stall_fd = {
            let path = std::ffi::CString::new(stall.join("a.sock").as_os_str().as_encoded_bytes()).expect("path");
            // SAFETY: a fresh socket, a zeroed sockaddr_un filled within bounds.
            unsafe {
                let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                assert!(fd >= 0, "socket");
                let mut addr: libc::sockaddr_un = std::mem::zeroed();
                addr.sun_family = libc::AF_UNIX as _;
                let bytes = path.as_bytes_with_nul();
                assert!(bytes.len() <= addr.sun_path.len());
                for (i, b) in bytes.iter().enumerate() {
                    addr.sun_path[i] = *b as libc::c_char;
                }
                let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
                assert_eq!(libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len), 0, "bind");
                assert_eq!(libc::listen(fd, 0), 0, "listen");
                fd
            }
        };
        // A symlinked 8-char dir pointing at another dir with a socket.
        let elsewhere = tmpdir.join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("elsewhere");
        let el = std::os::unix::net::UnixListener::bind(elsewhere.join("a.sock")).expect("bind elsewhere");
        drop(el);
        let linked = root.join(k2_vault::crypto::base62(8));
        k2_core::test_env::symlink(&elsewhere, &linked).expect("symlink");
        // Another namespace's root: never looked at.
        let other_root = tmpdir.join("k2a-0badc0de");
        let other = other_root.join(k2_vault::crypto::base62(8));
        std::fs::create_dir_all(&other).expect("other ns");
        let ol = std::os::unix::net::UnixListener::bind(other.join("a.sock")).expect("bind other");
        drop(ol);
        // A long secrets dir: the live socket uses a short path and is recorded.
        let t = k2_vault::fsutil::TestDir::new("boot-orphan");
        let long = t.path().join("x".repeat(110));
        std::fs::create_dir_all(&long).expect("long");
        let tag = uuid::Uuid::new_v4().to_string();
        let ws = uuid::Uuid::new_v4().to_string();
        let sock = agent::start(&v, Requester::Session { workspace_id: ws, session_id: Some(tag.clone()) }, &tag, &long).expect("start");
        assert!(sock.as_os_str().len() <= agent::MAX_SOCK_PATH, "{}", sock.display());
        let t0 = std::time::Instant::now();
        agent::boot_sweep(&v, &[(tag.clone(), long.clone())]);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "a never-accepting listener must not stall boot");
        assert!(!orphan.exists(), "an orphaned short-path dir goes");
        assert!(!stall.exists(), "so does an unrecorded never-accepting one (no probe)");
        assert!(sock.exists(), "a recorded one stays");
        assert!(std::fs::symlink_metadata(&linked).is_ok_and(|m| m.file_type().is_symlink()), "a symlinked dir is left alone");
        assert!(std::fs::symlink_metadata(elsewhere.join("a.sock")).is_ok(), "and never followed");
        assert!(other.join("a.sock").exists(), "another K2 home's namespace is never swept");
        // SAFETY: closing the test's own fd.
        unsafe { libc::close(stall_fd) };
        agent::stop_dir(&long);
        assert!(!sock.exists() && !sock.parent().expect("dir").exists());
    }

    /// KV19: every production spawn path that mints a passport also
    /// delivers vault secrets. A new spawn entry point that skips
    /// `materialize` fails here.
    #[test]
    fn every_spawn_path_materializes_vault_secrets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut checked = 0;
        let mut missing = vec![];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read dir") {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let rel = p.strip_prefix(&root).expect("rel").to_string_lossy().to_string();
                if rel == "session_token.rs" {
                    continue;
                }
                let src = std::fs::read_to_string(&p).expect("read");
                // Whole file: only session_token.rs's own tests mint in tests.
                let prod = src.as_str();
                let mints = prod.matches("prepare_agent_spawn_env(").count();
                if mints == 0 {
                    continue;
                }
                checked += 1;
                let delivers = prod.matches("vault::spawn::materialize(").count();
                if delivers < mints {
                    missing.push(format!("{rel}: {mints} passport mint(s), {delivers} materialize call(s)"));
                }
            }
        }
        assert!(checked >= 3, "expected spawn.rs, v2_spawn.rs and the legacy terminal path; saw {checked}");
        assert!(missing.is_empty(), "spawn paths without vault delivery: {missing:?}");
    }

    #[test]
    fn env_and_file_secrets_reach_a_spawn_and_fetch_ones_do_not() {
        use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes};
        // The secrets root follows env: see secrets_root_follows_the_env.
        let _env = k2_core::test_env::lock();
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let put = |name: &str, d: Delivery, val: &str| {
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), name, Kind::Generic);
            r.default_delivery = Some(d);
            r.hosts = vec!["api.example.com".into()];
            v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        };
        put("AS_ENV", Delivery::Env, "env-value-1");
        put("AS_FILE", Delivery::File, "file-value-2");
        put("AS_FETCH", Delivery::Fetch, "fetch-value-3");
        let mut env = HashMap::new();
        let sid = uuid::Uuid::new_v4().to_string();
        let out = materialize(&mut env, &ws, &sid, false, &SpawnRefs::default());
        assert_eq!(env.get("AS_ENV").map(String::as_str), Some("env-value-1"));
        assert!(!env.values().any(|x| x.contains("fetch-value-3")), "fetch secrets never reach env");
        assert!(!env.contains_key("AS_FETCH"));
        if let Some(dir) = &out.dir {
            assert_eq!(std::fs::read(dir.join("AS_FILE")).expect("file"), b"file-value-2");
            assert_eq!(env.get(SECRETS_DIR_ENV).map(String::as_str), Some(dir.display().to_string().as_str()));
            session_ended(&sid);
            assert!(!dir.exists(), "removed at session end");
        } else {
            assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}");
        }
        // Another workspace gets nothing: no value, only its own (empty)
        // secrets dir for live delivery (papercut 8).
        let mut env2 = HashMap::new();
        let out2 = materialize(&mut env2, &uuid::Uuid::new_v4().to_string(), "s2", false, &SpawnRefs::default());
        assert!(out2.files.is_empty() && out2.env_names.is_empty(), "{out2:?}");
        assert!(env2.keys().all(|k| k == SECRETS_DIR_ENV), "{env2:?}");
        assert!(!env2.values().any(|x| x.contains("value-")), "{env2:?}");
        if let Some(d2) = &out2.dir {
            assert_eq!(std::fs::read_dir(d2).expect("dir").count(), 0, "an empty dir");
        }
        session_ended("s2");
        // A sandboxed cell gets env only.
        let mut env3 = HashMap::new();
        let out3 = materialize(&mut env3, &ws, "s3", true, &SpawnRefs::default());
        assert!(out3.dir.is_none());
        assert_eq!(env3.get("AS_ENV").map(String::as_str), Some("env-value-1"));
    }

    /// Item 5: a job run gets its `job:` grants (env + file) through the
    /// same delivery path, nothing from the workspace's implicit grants,
    /// and the 60 s sweep (live v2 sessions) never touches its dir.
    #[test]
    fn a_job_run_gets_its_own_grants_and_the_session_sweep_leaves_it_alone() {
        use k2_vault::{Actor, Consumer, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes};
        // The secrets root follows env: see secrets_root_follows_the_env.
        let _env = k2_core::test_env::lock();
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let job_id = uuid::Uuid::new_v4().to_string();
        let put = |name: &str, d: Delivery, val: &str| {
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), name, Kind::Generic);
            r.default_delivery = Some(d);
            r.hosts = vec!["api.example.com".into()];
            v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        };
        put("JOB_ENV", Delivery::Env, "job-env-value-1");
        put("JOB_FILE", Delivery::File, "job-file-value-2");
        put("WS_ONLY", Delivery::Env, "ws-only-value-3");
        let grant = |name: &str, d: Delivery| {
            v.grant(
                GrantRequest {
                    selector: k2_vault::Selector::user(Scope::workspace(ws.clone()), name),
                    consumer: Consumer::job(&ws, &job_id),
                    delivery: d,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &Actor::Owner,
            )
            .expect("grant")
        };
        grant("JOB_ENV", Delivery::Env);
        grant("JOB_FILE", Delivery::File);
        let mut env = HashMap::new();
        let run_sid = format!("job-{}", uuid::Uuid::new_v4());
        let out = materialize_job(&mut env, &ws, &job_id, "collect", &run_sid);
        assert_eq!(env.get("JOB_ENV").map(String::as_str), Some("job-env-value-1"));
        assert!(!env.contains_key("WS_ONLY"), "no implicit workspace grant for a job: {env:?}");
        assert!(!env.values().any(|x| x.contains("ws-only-value-3")));
        match &out.dir {
            Some(dir) => {
                assert_eq!(std::fs::read(dir.join("JOB_FILE")).expect("file"), b"job-file-value-2");
                // The session sweep, told this run's id isn't a live session,
                // leaves its dir: a run is never in the session list. (Only
                // this id is "dead", so parallel tests' sessions stay.)
                sweep_dead(&|s| s != run_sid);
                assert!(dir.join("JOB_FILE").exists(), "the session sweep never removes a job run's dir");
                job_ended(&out);
                assert!(!dir.exists(), "removed at run end");
            }
            None => assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}"),
        }
        // Another job of the workspace gets nothing.
        let mut env2 = HashMap::new();
        let out2 = materialize_job(&mut env2, &ws, &uuid::Uuid::new_v4().to_string(), "other", "job-x");
        assert!(env2.is_empty() && out2.dir.is_none(), "{out2:?}");
    }

    #[test]
    fn secrets_root_order_matches_the_runtime_dir() {
        let p = PathBuf::from;
        let xdg = p("/run/user/1001");
        let unit = p("/run/k2");
        assert_eq!(
            linux_secrets_candidates(Some(Path::new("/x/run")), Some(&unit), Some(&xdg), 1001, false),
            vec![p("/x/run")],
            "override wins alone"
        );
        assert_eq!(
            linux_secrets_candidates(None, Some(&unit), Some(&xdg), 1001, false),
            vec![p("/run/k2"), p("/run/user/1001/k2"), p("/run/k2"), p("/dev/shm/k2-1001")],
            "the unit's dir first, then the old chain"
        );
        assert_eq!(
            linux_secrets_candidates(None, Some(Path::new("/run/other")), Some(&xdg), 1001, true),
            vec![p("/run/user/1001/k2"), p("/run/k2"), p("/dev/shm/k2-1001")],
            "a test process never takes the inherited unit dir"
        );
        assert_eq!(
            linux_secrets_candidates(None, None, None, 1001, false),
            vec![p("/run/k2"), p("/dev/shm/k2-1001")],
            "unset: the pre-0.46.4 fallbacks"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn secrets_root_follows_the_env() {
        // This test points the secrets root at a temp dir it deletes. The
        // env lock is taken first, so it is released only after `t` is
        // gone: no test that takes the lock (every test that makes secrets
        // dirs) can create one under the temp root, or see the root move.
        let _env = k2_core::test_env::lock();
        let t = k2_vault::fsutil::TestDir::new("spawn-root");
        let over = t.path().join("over");
        let _o = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, &over);
        assert_eq!(secrets_root(), Some(over.join("secrets")), "K2_RUNTIME_DIR set");
        assert!(over.is_dir());
        assert_private(&&over, 0o700);
        drop(_o);
        let _u = k2_core::test_env::EnvVar::remove(k2_core::runtime_dir::RUNTIME_DIR_ENV);
        let unit = t.path().join("unit");
        let xdg = t.path().join("xdg");
        std::fs::create_dir_all(&xdg).expect("mkdir xdg");
        let _x = k2_core::test_env::EnvVar::set("XDG_RUNTIME_DIR", &xdg);
        let _r = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::SYSTEMD_RUNTIME_DIRECTORY_ENV, &unit);
        let got = secrets_root();
        if k2_core::runtime_dir::looks_like_test_process() {
            assert_eq!(got, Some(xdg.join("k2").join("secrets")), "a test process skips the unit dir");
        } else {
            assert_eq!(got, Some(unit.join("secrets")), "RUNTIME_DIRECTORY set");
        }
    }

    /// Round 3 (6): removing a secrets dir never zeroes another file of
    /// ours through a hard link, and removes a planted subdirectory (FIFOs,
    /// links inside are only unlinked, never followed), without blocking.
    #[test]
    fn wiping_a_secrets_dir_never_zeroes_through_a_hard_link_and_removes_planted_dirs() {
        let t = k2_vault::fsutil::TestDir::new("spawn-wipe");
        let precious = t.path().join("precious");
        std::fs::write(&precious, b"keep-me").expect("precious");
        let d = make_session_dir(&t.path().join("secrets"), "s-wipe").expect("dir");
        std::fs::hard_link(&precious, d.join("linked")).expect("hard link");
        write_secret_file(&d, "TOKEN", b"secret-value").expect("token");
        let sub = d.join("planted").join("deeper");
        std::fs::create_dir_all(&sub).expect("planted dirs");
        std::fs::write(sub.join("f"), b"x").expect("file");
        // Windows filesystems have no FIFOs.
        #[cfg(unix)]
        mkfifo(&sub.join("fifo"));
        k2_core::test_env::symlink(&precious, sub.join("link")).expect("symlink");
        let t0 = std::time::Instant::now();
        remove_dir(&d);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(std::fs::read(&precious).expect("precious"), b"keep-me", "never zeroed through a hard link or a symlink");
        assert!(!d.exists(), "the dir and the planted tree are gone");
    }

    #[test]
    fn session_dir_and_files_are_private_and_exclusive() {
        let t = k2_vault::fsutil::TestDir::new("spawn-dir");
        let root = t.path().join("secrets");
        let d = make_session_dir(&root, "sess-1").expect("dir");
        assert_private(&&d, 0o700);
        write_secret_file(&d, "STRIPE_KEY", b"v").expect("write");
        assert_private(&d.join("STRIPE_KEY"), 0o600);
        assert!(write_secret_file(&d, "STRIPE_KEY", b"v").is_err(), "no overwrite (O_EXCL)");
        assert!(write_secret_file(&d, "../x", b"v").is_err(), "no traversal");
        // A pre-made symlink in place of a file is refused.
        k2_core::test_env::symlink(t.path().join("elsewhere"), d.join("LINKED")).expect("symlink");
        assert!(write_secret_file(&d, "LINKED", b"v").is_err());
        assert!(!t.path().join("elsewhere").exists(), "nothing written through the link");
        // A symlinked root is refused.
        let link_root = t.path().join("linkroot");
        k2_core::test_env::symlink(t.path(), &link_root).expect("symlink");
        assert!(make_session_dir(&link_root, "s").is_err());
    }

    /// Papercut 8 (0.46.5) test setup: a temp secrets root (Linux: the
    /// `K2_RUNTIME_DIR` override; macOS uses `$TMPDIR`), the test vault, and
    /// a secret of ANOTHER workspace (`other`) that `ws` doesn't get until
    /// it is granted. Hold the returned guards for the whole test.
    fn late_grant_setup(
        tag: &str,
        names: &[(&str, Delivery, &str)],
    ) -> (k2_core::test_env::EnvLock, k2_vault::fsutil::TestDir, k2_core::test_env::EnvVar, std::sync::Arc<k2_vault::LocalVault>, String, String) {
        use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes};
        let env = k2_core::test_env::lock();
        let t = k2_vault::fsutil::TestDir::new(tag);
        let over = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, t.path().join("run"));
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        for (name, d, val) in names {
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(other.clone()), *name, Kind::Generic);
            r.default_delivery = Some(*d);
            r.delivery_ceiling = Some(Delivery::Env);
            v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        }
        (env, t, over, v, ws, other)
    }

    fn grant_to_ws(v: &k2_vault::LocalVault, other: &str, ws: &str, name: &str, d: Delivery) {
        use k2_vault::{Actor, Consumer, GrantRequest, Scope};
        v.grant(
            GrantRequest {
                selector: k2_vault::Selector::user(Scope::workspace(other.to_string()), name),
                consumer: Consumer::workspace(ws),
                delivery: d,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant");
    }

    /// Papercut 8 (agent-acme, agent-acme-vanilla 10-26): a file secret
    /// granted while a session runs never reached its `$K2_SECRETS_DIR`
    /// (only a restart did). Now: every session gets its dir at spawn even
    /// with nothing in it; after a grant, live delivery writes the file;
    /// an env grant is listed as "restart your session to receive it"; a
    /// vanished dir is made again at the same path; a session of another
    /// workspace gets nothing; an ended session gets nothing and is told
    /// to restart.
    #[test]
    fn a_file_granted_mid_session_reaches_the_live_session() {
        let (_env, t, _over, v, ws, other) =
            late_grant_setup("spawn-late", &[("LATE_FILE", Delivery::File, "late-file-value-1"), ("LATE_ENV", Delivery::Env, "late-env-value-2")]);
        let sid = uuid::Uuid::new_v4().to_string();
        let mut env = HashMap::new();
        let out = materialize(&mut env, &ws, &sid, false, &SpawnRefs::default());
        let dir = out.dir.clone().unwrap_or_else(|| {
            let vault = match super::super::get() {
                Ok(v) => format!("open at {}", v.dir().display()),
                Err(e) => format!("closed ({})", e.code),
            };
            panic!(
                "a session with nothing to deliver still gets its secrets dir; vault {vault}, secrets root {:?}, {}={:?}, test dir exists {}, out {out:?}",
                secrets_root(),
                k2_core::runtime_dir::RUNTIME_DIR_ENV,
                std::env::var_os(k2_core::runtime_dir::RUNTIME_DIR_ENV),
                t.path().is_dir(),
            )
        });
        assert_eq!(env.get(SECRETS_DIR_ENV).map(String::as_str), Some(dir.display().to_string().as_str()));
        assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 0, "empty at spawn");
        assert_private(&&dir, 0o700);
        // A bystander session of a third workspace.
        let by_ws = uuid::Uuid::new_v4().to_string();
        let by_sid = uuid::Uuid::new_v4().to_string();
        let by = materialize(&mut HashMap::new(), &by_ws, &by_sid, false, &SpawnRefs::default());
        let by_dir = by.dir.clone().expect("bystander dir");

        // Nothing granted yet: nothing to deliver.
        let t0 = top_up(&v, &sid, &ws);
        assert_eq!((t0.delivered.len(), t0.present.len(), t0.restart.len(), t0.skipped.len()), (0, 0, 0, 0), "{t0:?}");

        grant_to_ws(&v, &other, &ws, "LATE_FILE", Delivery::File);
        grant_to_ws(&v, &other, &ws, "LATE_ENV", Delivery::Env);
        let n = top_up_all(&v);
        assert!(n >= 1, "live delivery wrote the granted file (wrote {n})");
        assert_eq!(std::fs::read(dir.join("LATE_FILE")).expect("the file arrived live"), b"late-file-value-1");
        assert_private(&dir.join("LATE_FILE"), 0o600);
        assert!(!by_dir.join("LATE_FILE").exists(), "another workspace's session gets nothing");

        let t1 = top_up(&v, &sid, &ws);
        assert_eq!(t1.present, vec!["LATE_FILE".to_string()], "{t1:?}");
        assert!(t1.delivered.is_empty() && t1.skipped.is_empty(), "{t1:?}");
        assert_eq!(t1.restart, vec![format!("LATE_ENV (env $LATE_ENV): {RESTART_TO_RECEIVE}")], "env can't reach a running session");

        // The caller's workspace must match the session's.
        assert_eq!(top_up(&v, &sid, &by_ws), TopUp::default(), "another workspace's caller tops up nothing");

        // The dir vanished under the live session: made again, same path.
        std::fs::remove_dir_all(&dir).expect("rm");
        let t2 = top_up(&v, &sid, &ws);
        assert_eq!(t2.delivered, vec!["LATE_FILE".to_string()], "{t2:?}");
        assert_private(&&dir, 0o700);
        assert_eq!(std::fs::read(dir.join("LATE_FILE")).expect("file"), b"late-file-value-1");

        // Ended: the dir goes and nothing is made again.
        session_ended(&sid);
        assert!(!dir.exists(), "removed at session end");
        top_up_all(&v);
        assert!(!dir.exists(), "an ended session's dir is never made again");
        let t3 = top_up(&v, &sid, &ws);
        assert!(t3.dir.is_none(), "{t3:?}");
        assert!(
            t3.restart.contains(&format!("LATE_FILE (file; this session has no $K2_SECRETS_DIR): {RESTART_TO_RECEIVE}")),
            "{t3:?}"
        );
        assert!(!dir.exists());
        session_ended(&by_sid);
        assert!(!by_dir.exists());

        // A sandboxed cell has no dir (a host dir isn't visible inside).
        let mut env4 = HashMap::new();
        let out4 = materialize(&mut env4, &ws, &uuid::Uuid::new_v4().to_string(), true, &SpawnRefs::default());
        assert!(out4.dir.is_none() && !env4.contains_key(SECRETS_DIR_ENV), "{out4:?}");
    }

    /// Papercut 8, the routes: `POST /cli/vault/sync` (k2 vault sync)
    /// writes the caller's own session's granted files now; env-file on a
    /// file-delivered secret says where the file is instead of a bare
    /// delivery_not_granted; a grant's answer says what running sessions
    /// get; sync is POST-only and needs an agent session.
    #[test]
    fn vault_sync_route_and_copy_for_a_mid_session_grant() {
        use crate::vault::actor::{Channel, VaultCaller};
        use crate::vault_use_routes::{ENV_FILE, SYNC};
        use k2_vault::{Actor, AgentActor, Consumer};
        let (_env, _t, _over, v, ws, _other) = late_grant_setup("spawn-sync", &[]);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        let me = VaultCaller {
            actor: Actor::Agent(AgentActor { workspace_id: ws.clone(), session_id: Some(sid.clone()), shell: false, vault_manage: false }),
            channel: Channel::Plain,
            session_id: Some(sid.clone()),
        };
        let call = |path: &str, c: &VaultCaller, body: serde_json::Value| -> (u16, serde_json::Value) {
            let r = crate::vault_routes::handle_on(&v, path, Ok(c.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None);
            let code: u16 = r.status[..3].parse().expect("status");
            (code, serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json body for {path}: {}", r.body)))
        };
        assert!(crate::vault_routes::is_route(SYNC) && crate::vault_routes::is_post_route(SYNC), "sync is a POST-only vault route");

        // The workspace's own file secret, set after the session started
        // (straight into the vault, so nothing tops it up yet).
        {
            use k2_vault::{Kind, Namespace, PutRequest, Scope, SecretBytes};
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), "SYNC_FILE", Kind::Generic);
            r.default_delivery = Some(Delivery::File);
            r.delivery_ceiling = Some(Delivery::Env);
            v.put(r, SecretBytes::from_slice(b"sync-file-value-1"), &Actor::Owner).expect("put");
        }
        assert!(!dir.join("SYNC_FILE").exists());

        // env-file on a file secret: where the file is, and how to get it.
        let (st, e) = call(ENV_FILE, &me, serde_json::json!({ "secrets": ["SYNC_FILE"] }));
        assert_eq!((st, e["error"]["code"].as_str()), (403, Some("delivery_not_granted")), "{e}");
        let hint = e["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains("$K2_SECRETS_DIR/SYNC_FILE") && hint.contains("k2 vault sync") && hint.contains(RESTART_TO_RECEIVE), "{hint}");

        // sync: delivered now.
        let (st, s1) = call(SYNC, &me, serde_json::json!({}));
        assert_eq!(st, 200, "{s1}");
        assert_eq!(s1["delivered"], serde_json::json!(["SYNC_FILE"]), "{s1}");
        assert_eq!(s1["dir"].as_str(), Some(dir.display().to_string().as_str()), "{s1}");
        assert_eq!(s1["hint"], "every file you are granted is in $K2_SECRETS_DIR", "{s1}");
        assert_eq!(std::fs::read(dir.join("SYNC_FILE")).expect("file"), b"sync-file-value-1");
        let (_, s2) = call(SYNC, &me, serde_json::json!({}));
        assert_eq!((s2["delivered"].clone(), s2["present"].clone()), (serde_json::json!([]), serde_json::json!(["SYNC_FILE"])), "{s2}");

        // Only an agent session can sync (the owner has no session dir).
        let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
        let (st, e) = call(SYNC, &owner, serde_json::json!({}));
        assert_eq!(st, 403, "{e}");

        // The grant answer's words for running sessions.
        let file_reach = crate::vault_routes::grant_reach(&v, &Consumer::workspace(ws.clone()), Delivery::File).expect("file reach");
        assert!(file_reach.starts_with("running sessions get the file in $K2_SECRETS_DIR now"), "{file_reach}");
        let env_reach = crate::vault_routes::grant_reach(&v, &Consumer::workspace(ws.clone()), Delivery::Env).expect("env reach");
        assert_eq!(env_reach, format!("env arrives in new sessions only: {RESTART_TO_RECEIVE}"));
        assert!(crate::vault_routes::grant_reach(&v, &Consumer::workspace(ws.clone()), Delivery::Fetch).is_none());

        // A session that ended (or never had a dir): told to restart.
        session_ended(&sid);
        let (st, s3) = call(SYNC, &me, serde_json::json!({}));
        assert_eq!(st, 200, "{s3}");
        assert!(s3["dir"].is_null(), "{s3}");
        assert!(s3["hint"].as_str().expect("hint").ends_with(RESTART_TO_RECEIVE), "{s3}");
        assert_eq!(s3["restart"], serde_json::json!([format!("SYNC_FILE (file; this session has no $K2_SECRETS_DIR): {RESTART_TO_RECEIVE}")]), "{s3}");
        assert!(!dir.exists(), "sync never makes an ended session's dir");
    }

    /// A destroyed, disabled or unshared secret's file stayed readable in
    /// every session it had reached until that session ended, and no output
    /// said so. Now it goes at once from every live session, by spawn or by
    /// live delivery; the answer counts them and says a value already read
    /// or in an environment can't be taken back. Files K2 didn't deliver,
    /// and other secrets' files, stay.
    #[test]
    fn destroying_disabling_or_unsharing_takes_the_file_from_live_sessions() {
        use crate::vault::actor::{Channel, VaultCaller};
        use crate::vault_routes::{DESTROY, DISABLE, ENABLE, REVOKE};
        use k2_vault::{Actor, Scope, Selector};
        let (_env, _t, _over, v, ws, other) = late_grant_setup(
            "spawn-recall",
            &[
                ("GONE_FILE", Delivery::File, "gone-value-1"),
                ("PAUSED_FILE", Delivery::File, "paused-value-2"),
                ("UNSHARED_FILE", Delivery::File, "unshared-value-3"),
                ("KEPT_FILE", Delivery::File, "kept-value-4"),
            ],
        );
        // This session starts first, so it gets them by live delivery.
        let live_sid = uuid::Uuid::new_v4().to_string();
        let live = materialize(&mut HashMap::new(), &ws, &live_sid, false, &SpawnRefs::default()).dir.expect("dir");
        for n in ["GONE_FILE", "PAUSED_FILE", "UNSHARED_FILE", "KEPT_FILE"] {
            grant_to_ws(&v, &other, &ws, n, Delivery::File);
        }
        let id_of = |n: &str| v.meta(&Selector::user(Scope::workspace(other.clone()), n), &Actor::Owner).expect("meta").id;
        let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
        let call = |path: &str, body: serde_json::Value| -> serde_json::Value {
            let r = crate::vault_routes::handle_on(&v, path, Ok(owner.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None);
            assert_eq!(r.status, "200 OK", "{path}: {}", r.body);
            serde_json::from_str(&r.body).expect("json")
        };

        // This one gets them at spawn.
        let spawned_sid = uuid::Uuid::new_v4().to_string();
        let spawned = materialize(&mut HashMap::new(), &ws, &spawned_sid, false, &SpawnRefs::default());
        assert!(spawned.files.contains(&"GONE_FILE".to_string()), "{spawned:?}");
        let spawned = spawned.dir.expect("dir");
        top_up_all(&v);
        // A file nobody delivered stays, whatever happens to the secrets.
        std::fs::write(live.join("mine.txt"), b"not a secret file").expect("write");
        // A session kept across a daemon restart, with no tick since: what K2
        // delivered is known from its record alone.
        let kept_sid = uuid::Uuid::new_v4().to_string();
        let kept = materialize(&mut HashMap::new(), &ws, &kept_sid, false, &SpawnRefs::default()).dir.expect("dir");
        daemon_restarted(&kept_sid, &ws, &kept);
        for d in [&live, &spawned, &kept] {
            assert_eq!(std::fs::read(d.join("GONE_FILE")).expect("delivered"), b"gone-value-1");
        }

        // Destroy: gone from all three now, not on the tick.
        let a = call(DESTROY, serde_json::json!({ "id": id_of("GONE_FILE"), "yes": true }));
        assert_eq!(a["filesRemoved"], 3, "{a}");
        let words = a["recall"].as_str().expect("recall words");
        assert!(words.contains("removed its file from 3 running sessions"), "{words}");
        assert!(words.contains("environment variable") && words.contains("can't take"), "{words}");
        for d in [&live, &spawned, &kept] {
            assert!(!d.join("GONE_FILE").exists(), "{}", d.display());
            assert_eq!(std::fs::read(d.join("KEPT_FILE")).expect("kept"), b"kept-value-4", "another secret's file stays");
        }
        assert_eq!(std::fs::read(live.join("mine.txt")).expect("mine"), b"not a secret file");

        // Disable stops it; enable brings it back on the next delivery.
        let a = call(DISABLE, serde_json::json!({ "id": id_of("PAUSED_FILE") }));
        assert_eq!(a["filesRemoved"], 3, "{a}");
        assert!(!live.join("PAUSED_FILE").exists());
        call(ENABLE, serde_json::json!({ "id": id_of("PAUSED_FILE") }));
        top_up_all(&v);
        assert_eq!(std::fs::read(live.join("PAUSED_FILE")).expect("back"), b"paused-value-2");

        // Revoke, by grant id.
        let grants = v.grants(&Selector::user(Scope::workspace(other.clone()), "UNSHARED_FILE"), &Actor::Owner).expect("grants");
        let a = call(REVOKE, serde_json::json!({ "grantId": grants[0].id }));
        assert_eq!(a["filesRemoved"], 3, "{a}");
        let words = a["recall"].as_str().expect("recall words");
        assert!(words.contains("removed its file from 3 running sessions that no longer get it"), "{a}");
        assert!(words.contains("Sessions it is still shared with keep their file."), "{a}");
        assert!(!spawned.join("UNSHARED_FILE").exists());

        // Nothing held as a file: the answer says that, not a count of zero files gone.
        let quiet = call(DESTROY, serde_json::json!({ "id": id_of("UNSHARED_FILE"), "yes": true }));
        assert_eq!(quiet["filesRemoved"], 0, "{quiet}");
        assert!(quiet["recall"].as_str().expect("words").starts_with("No running session had it as a file."), "{quiet}");

        for s in [&live_sid, &spawned_sid, &kept_sid] {
            session_ended(s);
        }
    }

    /// What a daemon restart leaves: nothing in memory but the session.
    fn daemon_restarted(sid: &str, ws: &str, dir: &Path) {
        LIVE.lock().remove(sid);
        register_survivor(sid, ws, dir);
    }

    fn destroy_as_owner(v: &std::sync::Arc<k2_vault::LocalVault>, id: &str) -> serde_json::Value {
        use crate::vault::actor::{Channel, VaultCaller};
        let owner = VaultCaller { actor: k2_vault::Actor::Owner, channel: Channel::Plain, session_id: None };
        let body = serde_json::json!({ "id": id, "yes": true }).to_string();
        let r = crate::vault_routes::handle_on(v, crate::vault_routes::DESTROY, Ok(owner), body.as_bytes(), &HashMap::new(), "test", None);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        serde_json::from_str(&r.body).expect("json")
    }

    fn secret_id(v: &k2_vault::LocalVault, other: &str, name: &str) -> String {
        use k2_vault::{Actor, Scope, Selector};
        v.meta(&Selector::user(Scope::workspace(other.to_string()), name), &Actor::Owner).expect("meta").id
    }

    /// A file already in the folder under a secret's name was never K2's:
    /// the session's own file survives the destroy, across a restart too.
    #[test]
    fn destroy_leaves_a_file_k2_did_not_deliver() {
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-mine", &[("MINE_FILE", Delivery::File, "vault-value-1")]);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        std::fs::write(dir.join("MINE_FILE"), b"the agent's own").expect("write");
        grant_to_ws(&v, &other, &ws, "MINE_FILE", Delivery::File);
        assert_eq!(top_up(&v, &sid, &ws).present, vec!["MINE_FILE".to_string()]);
        top_up_all(&v);
        daemon_restarted(&sid, &ws, &dir);
        top_up_all(&v);
        let a = destroy_as_owner(&v, &secret_id(&v, &other, "MINE_FILE"));
        assert_eq!(a["filesRemoved"], 0, "{a}");
        assert_eq!(std::fs::read(dir.join("MINE_FILE")).expect("still there"), b"the agent's own");
        session_ended(&sid);
    }

    /// The vault can't say what sessions get: the answer says K2 couldn't
    /// check, never that no session had it, and the next pass removes it.
    #[test]
    fn an_unchecked_session_is_reported_and_retried() {
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-unchecked", &[("LOCKED_FILE", Delivery::File, "locked-value-1")]);
        grant_to_ws(&v, &other, &ws, "LOCKED_FILE", Delivery::File);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        assert!(dir.join("LOCKED_FILE").exists());
        let id = secret_id(&v, &other, "LOCKED_FILE");
        FAIL_SPAWN_ITEMS.with(|f| f.set(true));
        let a = destroy_as_owner(&v, &id);
        FAIL_SPAWN_ITEMS.with(|f| f.set(false));
        assert_eq!(a["filesRemoved"], 0, "{a}");
        assert!(a["sessionsUnchecked"].as_u64().expect("count") >= 1, "{a}");
        let words = a["recall"].as_str().expect("words");
        assert!(words.contains("couldn't check") && words.contains("may still be in $K2_SECRETS_DIR"), "{words}");
        assert!(!words.contains("No running session"), "never a claim it didn't check: {words}");
        assert!(dir.join("LOCKED_FILE").exists(), "not removed while unchecked");
        top_up_all(&v);
        assert!(!dir.join("LOCKED_FILE").exists(), "the next pass removes it");
        session_ended(&sid);
    }

    /// A session from before the record existed: K2 can't tell whose its
    /// files are, so it touches none and says so.
    #[test]
    fn a_session_with_no_record_is_left_alone_and_reported() {
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-norecord", &[("OLD_FILE", Delivery::File, "old-value-1")]);
        grant_to_ws(&v, &other, &ws, "OLD_FILE", Delivery::File);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        std::fs::remove_file(delivered_path(&dir).expect("path")).expect("rm record");
        daemon_restarted(&sid, &ws, &dir);
        let a = destroy_as_owner(&v, &secret_id(&v, &other, "OLD_FILE"));
        assert_eq!(a["filesRemoved"], 0, "{a}");
        assert_eq!(a["sessionsUntracked"], 1, "{a}");
        let words = a["recall"].as_str().expect("words");
        assert!(words.contains("has no record of the files in 1 running session"), "{words}");
        assert!(!words.contains("No running session"), "{words}");
        assert!(dir.join("OLD_FILE").exists());
        session_ended(&sid);
    }

    /// The record: names are checked on load, it goes with its session, and
    /// the boot sweep clears one whose session is gone.
    #[test]
    fn the_delivered_record_is_checked_and_cleaned_up() {
        let (_env, t, _over, v, ws, _other) = late_grant_setup("spawn-record", &[]);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        let record = delivered_path(&dir).expect("path");
        assert_eq!(load_delivered(&dir), Delivered { complete: true, files: HashMap::new() });
        assert_private(&record, 0o600);
        // A same-user process can write it; a path in it never loads.
        let forged = serde_json::json!({ "complete": true, "files": { "../../outside": "id-1", "OK_NAME": "id-2" } });
        std::fs::write(&record, forged.to_string()).expect("forge");
        assert_eq!(load_delivered(&dir).files.keys().collect::<Vec<_>>(), vec!["OK_NAME"]);
        std::fs::write(&record, b"not json").expect("garbage");
        assert!(!load_delivered(&dir).complete, "unreadable: not complete");
        session_ended(&sid);
        assert!(!record.exists(), "goes with its session");
        let _ = &v;

        let root = t.path().join("sweep-root");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("gone-12345678.delivered"), b"{}").expect("stale record");
        std::fs::write(root.join("not-ours.txt"), b"x").expect("other file");
        sweep_root_keeping(&root, &[]);
        assert!(!root.join("gone-12345678.delivered").exists(), "a dead session's record is swept");
        assert!(root.join("not-ours.txt").exists());
    }

    /// A secret that stops being this session's by other means (its grant
    /// or its own time ran out) goes on the minute tick.
    #[test]
    fn the_tick_removes_a_file_whose_secret_this_session_no_longer_gets() {
        use k2_vault::{Actor, Consumer, GrantRequest, Scope, Selector};
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-expire", &[("BRIEF_FILE", Delivery::File, "brief-value-1")]);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        let sel = Selector::user(Scope::workspace(other.clone()), "BRIEF_FILE");
        v.grant(
            GrantRequest {
                selector: sel.clone(),
                consumer: Consumer::workspace(&ws),
                delivery: Delivery::File,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: Some((chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant");
        top_up_all(&v);
        assert!(dir.join("BRIEF_FILE").exists(), "delivered while the grant lasts");
        std::thread::sleep(Duration::from_millis(3200));
        top_up_all(&v);
        assert!(!dir.join("BRIEF_FILE").exists(), "gone once the grant ran out");
        session_ended(&sid);
    }

    /// The session's dir is writable by anything running as the same user. A
    /// link swapped in for it never turns a removal on another folder.
    #[cfg(unix)]
    #[test]
    fn a_swapped_secrets_dir_never_turns_removal_on_another_folder() {
        let (_env, t, _over, v, ws, other) = late_grant_setup("spawn-swap", &[("SWAP_TARGET", Delivery::File, "value-1")]);
        grant_to_ws(&v, &other, &ws, "SWAP_TARGET", Delivery::File);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        assert!(dir.join("SWAP_TARGET").exists());
        let victim = t.path().join("victim");
        std::fs::create_dir_all(&victim).expect("victim");
        std::fs::write(victim.join("SWAP_TARGET"), b"precious").expect("write");
        std::fs::write(victim.join("other.txt"), b"precious too").expect("write");
        std::fs::rename(&dir, t.path().join("moved")).expect("move the real dir");
        std::os::unix::fs::symlink(&victim, &dir).expect("swap in a link");

        let a = destroy_as_owner(&v, &secret_id(&v, &other, "SWAP_TARGET"));
        assert_eq!(std::fs::read(victim.join("SWAP_TARGET")).expect("still there"), b"precious", "{a}");
        assert_eq!(a["filesNotRemoved"], 1, "{a}");
        assert!(a["recall"].as_str().expect("words").contains("couldn't remove"), "{a}");

        session_ended(&sid);
        assert_eq!(std::fs::read(victim.join("SWAP_TARGET")).expect("still there"), b"precious");
        assert_eq!(std::fs::read(victim.join("other.txt")).expect("still there"), b"precious too");
        assert!(std::fs::symlink_metadata(&dir).is_err(), "the link itself goes");
    }

    /// A running `k2 job` keeps its file until the run ends, so the answer
    /// never says nothing has it, and it names the copies that stay a while.
    #[test]
    fn destroy_says_when_a_running_job_still_holds_the_file() {
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-job", &[("JOB_FILE", Delivery::File, "value-1")]);
        let job_id = uuid::Uuid::new_v4().to_string();
        v.grant(
            k2_vault::GrantRequest {
                selector: k2_vault::Selector::user(k2_vault::Scope::workspace(other.clone()), "JOB_FILE"),
                consumer: k2_vault::Consumer::job(&ws, &job_id),
                delivery: Delivery::File,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &k2_vault::Actor::Owner,
        )
        .expect("grant");
        let run = format!("job-{}", uuid::Uuid::new_v4());
        let m = materialize_job(&mut HashMap::new(), &ws, &job_id, "nightly", &run);
        assert!(m.dir.as_ref().expect("job dir").join("JOB_FILE").exists(), "{m:?}");
        let id = secret_id(&v, &other, "JOB_FILE");
        let a = destroy_as_owner(&v, &id);
        let words = a["recall"].as_str().expect("words");
        assert!(!words.contains("No running session"), "{words}");
        assert!(words.contains("1 running k2 job"), "{words}");
        assert!(words.contains("k2 vault file"), "{words}");
        job_ended(&m);
    }

    /// What the answer counts is about this secret: other sessions on the
    /// server, with files K2 can't place, aren't this secret's business.
    #[test]
    fn recall_counts_are_about_this_secret() {
        let (_env, _t, _over, v, ws, other) = late_grant_setup("spawn-scope", &[("SCOPED_A", Delivery::File, "value-a"), ("SCOPED_B", Delivery::File, "value-b")]);
        grant_to_ws(&v, &other, &ws, "SCOPED_A", Delivery::File);
        let sid = uuid::Uuid::new_v4().to_string();
        let dir = materialize(&mut HashMap::new(), &ws, &sid, false, &SpawnRefs::default()).dir.expect("dir");
        std::fs::remove_file(delivered_path(&dir).expect("path")).expect("rm record");
        daemon_restarted(&sid, &ws, &dir);
        let a = destroy_as_owner(&v, &secret_id(&v, &other, "SCOPED_B"));
        assert_eq!(a["sessionsUntracked"], 0, "{a}");
        assert!(a["recall"].as_str().expect("words").starts_with("No running session had it as a file."), "{a}");
        let a = destroy_as_owner(&v, &secret_id(&v, &other, "SCOPED_A"));
        assert_eq!(a["sessionsUntracked"], 1, "{a}");
        session_ended(&sid);
    }
}

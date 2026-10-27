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
/// kept across a restart), and secrets whose live delivery failed (not
/// retried for [`RETRY_AFTER`], so a refusal isn't audited every minute).
struct LiveSession {
    workspace_id: String,
    env_names: Option<Vec<String>>,
    failed: HashMap<String, Instant>,
}

static LIVE: LazyLock<Mutex<HashMap<String, LiveSession>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
fn ensure_private(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || !private_fs::owned_by_me(dir, &m) {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe secrets dir"));
            }
            if !private_fs::is_private(dir, &m) {
                private_fs::make_private_dir(dir)?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => private_fs::create_private_dir(dir),
        Err(e) => Err(e),
    }
}

/// Write one file `O_CREAT|O_EXCL|O_NOFOLLOW` 0600.
pub fn write_secret_file(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    k2_vault::delivery::validate_file_name(name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.hint))?;
    let mut f = private_fs::create_new_private_file(&dir.join(name))?;
    f.write_all(bytes)?;
    f.sync_all()
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
        LIVE.lock().insert(
            session_id.to_string(),
            LiveSession { workspace_id: ws.to_string(), env_names: Some(out.env_names.clone()), failed: HashMap::new() },
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
    out
}

/// A job run ended: its secrets dir (files, and its agent socket) goes.
pub fn job_ended(m: &Materialized) {
    if let Some(d) = &m.dir {
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
                    Ok(()) => out.files.push(fname),
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
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            wipe_entry(&e.path());
        }
    }
    let _ = std::fs::remove_dir(d);
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
    // the ptyd meta; the env they got at spawn isn't known.
    {
        let mut live = LIVE.lock();
        for (sid, _) in &kept {
            if let Some((_, _, Some(ws))) = survivors.iter().find(|(s, _, _)| s == sid) {
                live.insert(sid.clone(), LiveSession { workspace_id: ws.clone(), env_names: None, failed: HashMap::new() });
            }
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
        Some(r) => top_up_one(vault, session_id, workspace_id, dir.as_deref(), r.env_names.as_deref(), &mut r.failed, true),
        None => top_up_one(vault, session_id, workspace_id, dir.as_deref(), None, &mut HashMap::new(), true),
    }
}

/// Live delivery for every live session with a dir (after a grant or a
/// set, and on the vault's minute tick). Returns how many files it wrote.
pub fn top_up_all(vault: &k2_vault::LocalVault) -> usize {
    let _turn = TOPUP.lock();
    let dirs: Vec<(String, PathBuf)> = DIRS.lock().clone();
    let mut n = 0;
    for (sid, dir) in dirs {
        let mut live = LIVE.lock();
        let Some(r) = live.get_mut(&sid) else { continue };
        let ws = r.workspace_id.clone();
        let t = top_up_one(vault, &sid, &ws, Some(&dir), r.env_names.as_deref(), &mut r.failed, false);
        drop(live);
        if !t.delivered.is_empty() || !t.skipped.is_empty() {
            k2_core::log_debug!("[vault] live delivery {sid}: files {:?} skipped {:?}", t.delivered, t.skipped);
        }
        n += t.delivered.len();
    }
    n
}

fn top_up_one(
    vault: &k2_vault::LocalVault,
    session_id: &str,
    workspace_id: &str,
    dir: Option<&Path>,
    env_names: Option<&[String]>,
    failed: &mut HashMap<String, Instant>,
    retry_now: bool,
) -> TopUp {
    let mut out = TopUp { dir: dir.map(Path::to_path_buf), ..Default::default() };
    let who = Requester::Session { workspace_id: workspace_id.to_string(), session_id: Some(session_id.to_string()) };
    let items = match vault.spawn_items(&who) {
        Ok(i) => i,
        Err(e) => {
            out.skipped.push(format!("*: {}", e.code));
            return out;
        }
    };
    let now = Instant::now();
    failed.retain(|_, t| now.duration_since(*t) < RETRY_AFTER);
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
        // A short temp TMPDIR (socket paths must fit sun_path), removed after.
        struct Rm(PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
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

    /// Papercut 8 (agent-nsi, agent-nsi-vanilla 10-26): a file secret
    /// granted while a session runs never reached its `$K2_SECRETS_DIR`
    /// (only a restart did). Now: every session gets its dir at spawn even
    /// with nothing in it; after a grant, live delivery writes the file;
    /// an env grant is listed as "restart your session to receive it"; a
    /// vanished dir is made again at the same path; a session of another
    /// workspace gets nothing; an ended session gets nothing and is told
    /// to restart.
    #[test]
    fn a_file_granted_mid_session_reaches_the_live_session() {
        let (_env, _t, _over, v, ws, other) =
            late_grant_setup("spawn-late", &[("LATE_FILE", Delivery::File, "late-file-value-1"), ("LATE_ENV", Delivery::Env, "late-env-value-2")]);
        let sid = uuid::Uuid::new_v4().to_string();
        let mut env = HashMap::new();
        let out = materialize(&mut env, &ws, &sid, false, &SpawnRefs::default());
        let dir = out.dir.clone().expect("a session with nothing to deliver still gets its secrets dir");
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
}

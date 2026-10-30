//! R8 upgrade (prd-env-to-vault-v1): `.env.k2` references need a grant.
//!
//! Before 0.46.4 a published service (or App) resolved any
//! `k2v://server/NAME` in its `.env.k2` as the daemon, with no grant, and
//! its own workspace's secrets whatever their default delivery. Now the
//! service itself is the requester ([`super::publish_env`]): a server
//! secret needs a grant to the service or its workspace, and an own
//! workspace secret needs an env default delivery (or a grant).
//!
//! So that services in real use keep working, this pass runs **once**, at
//! the first boot with the new rule, before any service starts. For each
//! reference in a service's current `.env.k2` that the old rule resolved
//! and the new rule refuses, it grants **that one service** (`service:<id>`,
//! env, the `.env.k2` key as the variable) **for [`UPGRADE_GRANT_DAYS`]
//! days** (Rosson's default, review round 1), only with evidence that this
//! very service received it:
//!
//! - **Live env** (strong): the service is running and its process
//!   environment holds `KEY=<the secret's value>` (Linux `/proc/<pid>/environ`,
//!   macOS `ps -E` of the user's own process). Compared inside the daemon;
//!   the value is never logged or stored.
//! - **Only referrer, audited**: no other service's `.env.k2` names the
//!   secret (so the old path's `daemon:publish` audit rows can only be this
//!   service's), the service is running or wanted running or started in the
//!   last [`WINDOW_DAYS`] days, its `.env.k2` didn't change after that start
//!   (a `--cmd` service; an App's Seed is content-addressed), and the vault
//!   audit shows the old path resolved the secret in the window.
//!
//! Everything else is refused (listed). Agent launch-block references (an
//! AGENT.md `launch:` env, which the agent can edit) are never granted.
//!
//! The owner gets one notice (names and service ids only, never values)
//! and, per workspace, one Thread card: "Keep all", a "Keep" per row, or
//! "Let them lapse". Unkept grants lapse after [`UPGRADE_GRANT_DAYS`]; the
//! service then shows the refused reference in its status.
//!
//! Done flag: an `upgrade_r8_done` row in the vault's own MAC'd audit chain
//! (not a file an agent could delete). A pass that couldn't read a
//! service's `.env.k2` or Seed doesn't set it, so it runs again next boot.
//! A human-readable report is kept at `~/.k2/vault/r8-server-refs.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use k2_vault::{
    Actor, Consumer, Delivery, GrantRequest, LocalVault, Namespace, Requester, ScopeKind, SecretStatus, SecretStore,
    Selector, Use,
};
use serde::{Deserialize, Serialize};

/// "Used lately": the look-back for the audit evidence.
pub const WINDOW_DAYS: i64 = 30;

/// How long an upgrade grant lasts unless the owner keeps it (the one
/// constant to change).
pub const UPGRADE_GRANT_DAYS: i64 = 14;

/// The audit actor of the old `.env.k2` resolve path (`Requester::System("publish")`).
const OLD_PUBLISH_ACTOR: &str = "daemon:publish";

const ACTOR: &str = "upgrade-r8";

/// The audit action that marks the pass done.
pub const DONE_ACTION: &str = "upgrade_r8_done";

/// The Thread card's action id ([`crate::action_cards`]).
pub const KEEP_ACTION: &str = "vault.r8.keep";
pub const KEEP_ALL: &str = "Keep all";
pub const LET_LAPSE: &str = "Let them lapse";
/// At most this many per-row "Keep" buttons on one card.
const MAX_ROW_OPTIONS: usize = 6;

fn actor() -> Actor {
    Actor::System(ACTOR.into())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefLine {
    /// The service id (unique on the box; names aren't).
    pub service_id: String,
    /// The service's name as the agent chose it, made safe to print.
    pub service: String,
    pub workspace_id: String,
    pub workspace: String,
    /// The env name in `.env.k2` or the launch block.
    pub key: String,
    /// The secret's name (never its value) and id.
    pub secret: String,
    pub secret_id: String,
    /// `server` | `workspace`.
    pub scope: String,
    pub reason: String,
    /// The upgrade grant, when one was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub done_at: String,
    pub window_days: i64,
    pub grant_days: i64,
    /// A rerun after an unreadable first pass (only those services).
    #[serde(default)]
    pub rerun: bool,
    /// Granted to that one service, time-boxed.
    pub granted: Vec<RefLine>,
    /// Service references now refused.
    pub refused: Vec<RefLine>,
    /// Agent launch-block references now refused (never granted).
    pub launch_refused: Vec<RefLine>,
    /// What couldn't be read (a service's `.env.k2` or Seed, or the list of
    /// services itself): the pass is partial and those are checked again.
    pub unreadable: Vec<String>,
    /// The service ids behind `unreadable` (`*` = the list of services).
    #[serde(default)]
    pub unreadable_ids: Vec<String>,
    /// Services read in full this pass.
    #[serde(default)]
    pub scanned_ids: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty() && self.refused.is_empty() && self.launch_refused.is_empty() && self.unreadable.is_empty()
    }
    /// Anything the owner must hear about: something granted or refused.
    pub fn has_decisions(&self) -> bool {
        !self.granted.is_empty() || !self.refused.is_empty() || !self.launch_refused.is_empty()
    }
}

/// Audit actions the pass keeps its state in (all by `system:upgrade-r8`).
const PASS_ACTION: &str = "upgrade_r8_pass";
const DECIDED_ACTION: &str = "upgrade_r8_decided";
const UNREADABLE_ACTION: &str = "upgrade_r8_unreadable";
const SCANNED_ACTION: &str = "upgrade_r8_scanned";

fn report_path() -> PathBuf {
    super::vault_dir(&k2_core::paths::k2_home()).join("r8-server-refs.json")
}

/// Has the pass completed on this vault?
pub fn done(vault: &LocalVault) -> bool {
    vault.audit_has(DONE_ACTION, &actor()).unwrap_or(false)
}

/// What a pass looks at.
#[derive(Debug, Default, Clone)]
pub struct PassScope {
    /// `None`: every service (the first pass). `Some`: only these service
    /// ids, the ones an earlier pass couldn't read (round 2 fix 1).
    pub only: Option<BTreeSet<String>>,
    /// `<service id>/<secret id>` pairs an earlier pass decided: never again.
    pub decided: BTreeSet<String>,
}

fn pair(service_id: &str, secret_id: &str) -> String {
    format!("{service_id}/{secret_id}")
}

/// The once-only boot step (from [`super::boot`], before services start).
///
/// State lives in the vault's audit chain, verified before anything is
/// read from it. Forging a row there needs the audit key; a row written
/// straight into `vault.db` fails the chain check and the pass doesn't run
/// (no grants: fail closed). The first pass is stamped (`upgrade_r8_pass`);
/// each decided (service, secret) pair is recorded and never decided again;
/// unreadable services are recorded and are the only ones a rerun reads; a
/// rerun accepts only the live-env evidence (no audit window, no mtime) and
/// posts a notice and cards only when it decided something new.
pub fn upgrade_once(vault: &LocalVault) {
    if let Err(e) = vault.verify_audit_chain() {
        k2_core::log_debug!("WARN [vault] R8 upgrade skipped: the vault audit chain doesn't verify ({}); no grants made", e.code);
        // Fail closed, but say so: the owner hears what is refused and how
        // to fix it (listed from the services' files and the grants table,
        // never from the chain).
        let r = chain_broken_report(vault, &e.code);
        let (title, body) = chain_broken_notice(&e.code, &r);
        file_notice(&title, &body);
        return;
    }
    let state = (|| -> Result<(bool, Option<String>, Vec<String>, Vec<String>, Vec<String>), k2_vault::VaultError> {
        Ok((
            vault.audit_has(DONE_ACTION, &actor())?,
            vault.audit_first_ts(PASS_ACTION, &actor())?,
            vault.audit_consumers(DECIDED_ACTION, &actor())?,
            vault.audit_consumers(UNREADABLE_ACTION, &actor())?,
            vault.audit_consumers(SCANNED_ACTION, &actor())?,
        ))
    })();
    let (is_done, first, decided, unreadable, scanned) = match state {
        Ok(s) => s,
        Err(e) => {
            k2_core::log_debug!("WARN [vault] R8 upgrade skipped: the audit couldn't be read ({})", e.code);
            return;
        }
    };
    if is_done {
        return;
    }
    let scope = PassScope {
        only: first.as_ref().map(|_| {
            let scanned: BTreeSet<String> = scanned.into_iter().collect();
            unreadable.into_iter().filter(|u| !scanned.contains(u)).collect()
        }),
        decided: decided.into_iter().collect(),
    };
    let report = run_scoped(vault, chrono::Utc::now(), &scope);
    if let Ok(body) = serde_json::to_vec_pretty(&report) {
        let path = report_path();
        if let Some(dir) = path.parent() {
            let _ = k2_vault::fsutil::ensure_private_dir(dir);
        }
        if let Err(e) = k2_vault::fsutil::write_private_atomic(&path, &body) {
            k2_core::log_debug!("[vault] R8 upgrade report not saved: {}", e.hint);
        }
    }
    let note = |action: &str, consumer: &str| {
        if let Err(e) = vault.audit_note(action, &actor(), consumer) {
            k2_core::log_debug!("WARN [vault] R8 upgrade: {action} not recorded ({})", e.code);
        }
    };
    if first.is_none() {
        note(PASS_ACTION, &report.done_at);
    }
    for l in report.granted.iter().chain(report.refused.iter()) {
        note(DECIDED_ACTION, &pair(&l.service_id, &l.secret_id));
    }
    for id in &report.scanned_ids {
        note(SCANNED_ACTION, id);
    }
    for id in &report.unreadable_ids {
        note(UNREADABLE_ACTION, id);
    }
    if report.unreadable.is_empty() {
        let summary = format!("granted {} refused {} launch {}", report.granted.len(), report.refused.len(), report.launch_refused.len());
        note(DONE_ACTION, &summary);
    } else {
        k2_core::log_debug!("WARN [vault] R8 upgrade partial ({} unreadable); only those are read again next boot", report.unreadable.len());
    }
    k2_core::log_debug!(
        "[vault] R8 upgrade{}: granted {} refused {} launch-block refused {} unreadable {}",
        if report.rerun { " (rerun)" } else { "" },
        report.granted.len(),
        report.refused.len(),
        report.launch_refused.len(),
        report.unreadable.len()
    );
    // The first pass always tells the owner; a rerun only when it decided
    // something new (an agent can't make it nag by keeping a file unreadable).
    if (!report.rerun && !report.is_empty()) || report.has_decisions() {
        notify_owner(&report);
        present_cards(&report);
    }
}

struct Service {
    id: String,
    workspace_id: String,
    name: String,
    is_app: bool,
    dirs: Vec<PathBuf>,
    /// The recorded pid, only when it is alive AND provably this service's
    /// process (started when K2 says the service started; round 2 fix 2).
    pid: Option<i64>,
    /// Running, wanted running, or started in the window.
    recent: bool,
    last_started_at: Option<i64>,
}

#[allow(clippy::type_complexity)]
fn services(cutoff_unix: i64) -> Result<Vec<Service>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let sql = "SELECT ps.id, ps.project_id, ps.name, ps.cmd, ps.cwd, ps.last_started_at, ps.pid, ps.desired, \
               a.live_seed, a.preview_seed, a.state, a.updated_at \
               FROM published_services ps LEFT JOIN apps a ON a.service_id = ps.id";
    let mut st = conn.prepare(sql).map_err(|e| format!("the list of services couldn't be read ({e})"))?;
    let rows = st
        .query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?),
                (r.get::<_, Option<i64>>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, String>(7)?),
                (r.get::<_, Option<String>>(8)?, r.get::<_, Option<String>>(9)?, r.get::<_, Option<String>>(10)?, r.get::<_, Option<i64>>(11)?),
            ))
        })
        .map_err(|e| format!("the list of services couldn't be read ({e})"))?;
    let mut out = vec![];
    for row in rows {
        // Round 2 fix 5: one bad row makes the pass partial, never "done".
        let ((id, ws, name, cmd, cwd), (started, pid, desired), (live, preview, state, app_updated)) =
            row.map_err(|e| format!("a service row couldn't be read ({e})"))?;
        let is_app = cmd == k2_core::published_services::CMD_APP_SENTINEL;
        let live_pid = pid.filter(|p| pid_alive(*p));
        let pid = live_pid.filter(|p| started.is_some_and(|s| pid_started_near(*p, s)));
        let dirs: Vec<PathBuf> = if is_app {
            // An App resolves `.env.k2` inside its Seed (the engine's workdir).
            [live, preview].into_iter().flatten().filter(|h| !h.is_empty()).map(|h| k2_core::deploy::seed::seed_path(&h)).collect()
        } else {
            vec![PathBuf::from(cwd)]
        };
        let recent = live_pid.is_some()
            || desired == k2_core::published_services::DESIRED_RUNNING
            || started.is_some_and(|t| t >= cutoff_unix)
            || (is_app && (state.as_deref() == Some("running") || app_updated.is_some_and(|t| t >= cutoff_unix)));
        out.push(Service { id, workspace_id: ws, name, is_app, dirs, pid, recent, last_started_at: started });
    }
    Ok(out)
}

fn pid_alive(pid: i64) -> bool {
    let Ok(p) = i32::try_from(pid) else { return false };
    #[cfg(unix)]
    {
        p > 0 && unsafe { libc::kill(p, 0) == 0 }
    }
    #[cfg(windows)]
    {
        p > 0 && crate::publish_runtime::win_job::pid_alive(p as u32)
    }
}

/// How far a process's start may be from `last_started_at` and still be
/// that start (K2 stamps the time right after the spawn returns).
const START_SLACK_SECS: i64 = 10;

/// Did process `pid` start within [`START_SLACK_SECS`] of `started_at`?
/// After a reboot the recorded pid can be some other process: this tells
/// them apart. `false` when K2 can't tell.
pub fn pid_started_near(pid: i64, started_at: i64) -> bool {
    match process_start_unix(pid) {
        Some(t) => (t - started_at).abs() <= START_SLACK_SECS,
        None => false,
    }
}

/// When process `pid` started (unix seconds), from `ps -o etime=`
/// (`[[dd-]hh:]mm:ss`, on Linux and macOS).
pub fn process_start_unix(pid: i64) -> Option<i64> {
    let out = k2_core::bg_proc::command("ps").args(["-o", "etime=", "-p", &pid.to_string()]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let elapsed = parse_etime(std::str::from_utf8(&out.stdout).ok()?.trim())?;
    Some(chrono::Utc::now().timestamp() - elapsed)
}

/// `[[dd-]hh:]mm:ss` → seconds.
pub fn parse_etime(s: &str) -> Option<i64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<i64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<i64> = rest.split(':').map(|p| p.parse::<i64>().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    Some(((days * 24 + h) * 60 + m) * 60 + sec)
}

/// Does process `pid`'s environment hold exactly `key=value`? `None` when
/// K2 can't read it (another user's process, gone, unsupported OS), never
/// `Some(false)`. Every buffer that held the value is zeroed before it's
/// dropped.
pub fn process_env_has(pid: i64, key: &str, value: &[u8]) -> Option<bool> {
    use zeroize::Zeroizing;
    let mut want = Zeroizing::new(Vec::with_capacity(key.len() + 1 + value.len()));
    want.extend_from_slice(key.as_bytes());
    want.push(b'=');
    want.extend_from_slice(value);
    #[cfg(target_os = "linux")]
    {
        // Empty: a zombie, a kernel thread, or mid-exec (the env isn't set
        // up yet), so "can't tell", not "lacks it".
        let env = Zeroizing::new(std::fs::read(format!("/proc/{pid}/environ")).ok()?);
        if env.is_empty() {
            return None;
        }
        Some(env.split(|b| *b == 0).any(|kv| kv == want.as_slice()))
    }
    #[cfg(target_os = "macos")]
    {
        // Our own user's processes, by `KERN_PROCARGS2` (no `ps` spawn).
        // macOS 27 leaves the env out for Apple platform binaries
        // (`/bin/sleep`, `/bin/sh`, in `ps -E` too): no env strings at all
        // means "hidden", so `None`, not "lacks it".
        let args = Zeroizing::new(macos_procargs2(pid)?);
        let mut env = procargs2_env(&args)?;
        Some(env.any(|kv| kv == want.as_slice()))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (&want, pid);
        None
    }
}

/// The raw `KERN_PROCARGS2` block of `pid`; `None` when the sysctl refuses
/// (another user's process, EPERM, gone). The caller zeroes it.
#[cfg(target_os = "macos")]
fn macos_procargs2(pid: i64) -> Option<Vec<u8>> {
    use zeroize::Zeroize;
    let pid = libc::c_int::try_from(pid).ok().filter(|p| *p > 0)?;
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` and `argmax` are valid for the call; `size` is its size.
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buf` holds `size` writable bytes; the kernel writes at most that.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 {
        buf.zeroize();
        return None;
    }
    // Zero the tail before shortening, so no byte is dropped unzeroed.
    let len = size.min(buf.len());
    buf[len..].zeroize();
    buf.truncate(len);
    Some(buf)
}

/// The env strings of a `KERN_PROCARGS2` block: `argc` (i32), the exec
/// path, NUL padding, `argc` argv strings, then env strings up to an empty
/// one. `None` when the block is malformed or holds no `key=value` env
/// string (the OS hid the environment).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn procargs2_env(buf: &[u8]) -> Option<impl Iterator<Item = &[u8]>> {
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let mut rest = &buf[4..];
    // The exec path, then its NUL padding.
    rest = &rest[rest.iter().position(|b| *b == 0)?..];
    rest = &rest[rest.iter().position(|b| *b != 0)?..];
    // argv: exactly `argc` strings (an argv entry may be empty).
    for _ in 0..argc {
        rest = &rest[rest.iter().position(|b| *b == 0)? + 1..];
    }
    let env = || rest.split(|b| *b == 0).take_while(|s| !s.is_empty());
    if !env().any(|s| s.contains(&b'=')) {
        return None;
    }
    Some(env())
}

/// A printable name an agent chose: letters, digits, `.`, `_`, `-`, at
/// most 40 characters; anything else becomes `?` (review fix 7).
pub fn safe_label(s: &str) -> String {
    let mut out: String =
        s.chars().take(40).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '?' }).collect();
    if s.chars().count() > 40 {
        out.push('…');
    }
    if out.is_empty() {
        out.push('?');
    }
    out
}

struct Line {
    svc: usize,
    key: String,
    sel: Selector,
}

enum Read {
    Lines(Vec<(String, String)>),
    Absent,
    Unreadable(String),
}

/// `(key, ref)` pairs of references in a service's env file.
fn refs_in(dir: &Path, env_file: &str) -> Read {
    let Ok(p) = crate::vault_use_routes::jailed_path(dir, env_file) else { return Read::Absent };
    match std::fs::read_to_string(&p) {
        Ok(text) => Read::Lines(
            super::publish_env::parse_env_k2(&text)
                .into_iter()
                // `granted()` skips a key it can't set before resolving anything.
                .filter(|(k, v)| k2_vault::delivery::validate_env_name(k).is_ok() && k2_vault::refs::VaultRef::is_ref(v))
                .collect(),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unreadable(format!("{}: {:?}", p.display(), e.kind())),
    }
}

/// The env file's modification time (unix seconds).
fn mtime(dir: &Path, env_file: &str) -> Option<i64> {
    let p = crate::vault_use_routes::jailed_path(dir, env_file).ok()?;
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// What the pre-R8 rule resolved for a service of `workspace_id`: a server
/// secret or its own workspace's, active, unexpired, ceiling ≥ env.
fn old_rule_resolves(vault: &LocalVault, sel: &Selector, workspace_id: Option<&str>) -> Option<k2_vault::SecretMeta> {
    let m = vault.meta(sel, &actor()).ok()?;
    let scope_ok = match m.scope.kind {
        ScopeKind::Server => true,
        ScopeKind::Workspace => workspace_id.is_none_or(|w| m.scope.id == w),
        _ => workspace_id.is_none(),
    };
    let expired = m
        .expires_at
        .as_deref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
        .is_some_and(|x| x < chrono::Utc::now());
    (scope_ok
        && m.namespace == Namespace::User
        && m.status == SecretStatus::Active
        && !expired
        && m.delivery_ceiling.level() >= Delivery::Env.level())
    .then_some(m)
}

/// Why this service may keep it, or why not.
#[allow(clippy::too_many_arguments)]
fn evidence(
    vault: &LocalVault,
    svc: &Service,
    key: &str,
    m: &k2_vault::SecretMeta,
    referrers: usize,
    env_mtime: Option<i64>,
    since: &str,
    rerun: bool,
) -> Result<&'static str, String> {
    if let Some(pid) = svc.pid {
        let value = vault.resolve(&Selector::Id(m.id.clone()), Use::System, &Requester::System(ACTOR.into())).map_err(|e| e.code.to_string())?;
        if process_env_has(pid, key, value.value.expose()) == Some(true) {
            return Ok("the running service has it in its environment");
        }
        // Not found (or unreadable): a process may rewrite its environ for
        // its title (nginx, postgres, node), so this is no proof against.
    }
    if rerun {
        return Err("checked again after its .env.k2 couldn't be read: only a running service holding it in its environment counts".into());
    }
    if referrers != 1 {
        return Err(format!("{referrers} services name it, so K2 can't tell which one used it"));
    }
    if !svc.recent {
        return Err(format!("not running and not started in the last {WINDOW_DAYS} days"));
    }
    if !svc.is_app {
        match (env_mtime, svc.last_started_at) {
            (Some(m), Some(s)) if m <= s => {}
            (_, None) => return Err("K2 has no record of when it last started".into()),
            _ => return Err("its .env.k2 changed after it last started".into()),
        }
    }
    let used = vault.audit_seen(&m.id, "resolve", OLD_PUBLISH_ACTOR, None, Some(since), None).map_err(|e| e.code.to_string())?;
    if !used {
        return Err(format!("the vault shows no use in the last {WINDOW_DAYS} days"));
    }
    Ok("the only service that names it, and the vault shows it was used")
}

/// The first pass over every service (tests call it with their own clock).
pub fn run(vault: &LocalVault, now: chrono::DateTime<chrono::Utc>) -> Report {
    run_scoped(vault, now, &PassScope::default())
}

/// A pass over `scope` (round 2 fix 1: a rerun reads only the services an
/// earlier pass couldn't, and never decides a pair twice).
pub fn run_scoped(vault: &LocalVault, now: chrono::DateTime<chrono::Utc>, scope: &PassScope) -> Report {
    vault.flush_rollups();
    let rerun = scope.only.is_some();
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let since = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let expires = (now + chrono::Duration::days(UPGRADE_GRANT_DAYS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut report = Report {
        done_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        window_days: WINDOW_DAYS,
        grant_days: UPGRADE_GRANT_DAYS,
        rerun,
        ..Default::default()
    };
    if let Err(e) = vault.verify_audit_chain() {
        report.unreadable.push(format!("the vault audit chain doesn't verify ({})", e.code));
        report.unreadable_ids.push("*".into());
        return report;
    }
    let all = match services(cutoff.timestamp()) {
        Ok(s) => s,
        Err(why) => {
            report.unreadable.push(why);
            report.unreadable_ids.push("*".into());
            return report;
        }
    };
    let wanted = |id: &str| match &scope.only {
        None => true,
        Some(only) => only.contains(id) || only.contains("*"),
    };
    // Referrer counts look at every service (a rerun too), so a secret two
    // services name is never pinned on one of them.
    let mut lines: Vec<Line> = vec![];
    let mut mtimes: BTreeMap<usize, Option<i64>> = BTreeMap::new();
    let mut referrers: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (i, svc) in all.iter().enumerate() {
        let env_file = super::publish_env::get(&svc.id).env_file.unwrap_or_else(|| ".env.k2".into());
        let mine = wanted(&svc.id);
        if svc.is_app && svc.dirs.iter().any(|d| !d.is_dir()) {
            if mine {
                report.unreadable.push(format!("App service {}: its Seed folder is missing", svc.id));
                report.unreadable_ids.push(svc.id.clone());
            }
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut readable = true;
        for dir in &svc.dirs {
            if !svc.is_app {
                mtimes.insert(i, mtime(dir, &env_file));
            }
            let found = match refs_in(dir, &env_file) {
                Read::Lines(l) => l,
                Read::Absent => continue,
                Read::Unreadable(why) => {
                    if mine {
                        report.unreadable.push(format!("service {}: {why}", svc.id));
                        report.unreadable_ids.push(svc.id.clone());
                    }
                    readable = false;
                    continue;
                }
            };
            for (key, r) in found {
                if !seen.insert((key.clone(), r.clone())) {
                    continue;
                }
                let Ok(sel) = super::selector_for_ref(&r) else { continue };
                if let Ok(m) = vault.meta(&sel, &actor()) {
                    referrers.entry(m.id).or_default().insert(i);
                }
                if mine {
                    lines.push(Line { svc: i, key, sel });
                }
            }
        }
        if mine && readable {
            report.scanned_ids.push(svc.id.clone());
        }
    }
    for l in lines {
        let svc = &all[l.svc];
        let Some(m) = old_rule_resolves(vault, &l.sel, Some(&svc.workspace_id)) else { continue };
        if scope.decided.contains(&pair(&svc.id, &m.id)) {
            continue; // an earlier pass decided this pair
        }
        // Never a second upgrade grant for a pair that has (or had) one,
        // even expired, revoked or lapsed.
        let consumer = Consumer::published_service(svc.id.clone()).label();
        if vault.audit_seen(&m.id, "grant", &actor().stamp(), Some(&consumer), None, None).unwrap_or(true) {
            continue;
        }
        let who = Requester::PublishedService { id: svc.id.clone(), workspace_id: svc.workspace_id.clone() };
        if vault.ref_would_resolve(&l.sel, &who).unwrap_or(false) {
            continue; // the new rule allows it
        }
        let mut line = RefLine {
            service_id: svc.id.clone(),
            service: safe_label(&svc.name),
            workspace_id: svc.workspace_id.clone(),
            workspace: safe_label(&super::workspace_label(&svc.workspace_id)),
            key: l.key.clone(),
            secret: m.name.clone(),
            secret_id: m.id.clone(),
            scope: if m.scope.kind == ScopeKind::Server { "server".into() } else { "workspace".into() },
            ..Default::default()
        };
        let n = referrers.get(&m.id).map(BTreeSet::len).unwrap_or(0);
        match evidence(vault, svc, &l.key, &m, n, mtimes.get(&l.svc).copied().flatten(), &since, rerun) {
            Err(why) => {
                line.reason = why;
                report.refused.push(line);
            }
            Ok(why) => {
                let g = GrantRequest {
                    selector: Selector::Id(m.id.clone()),
                    consumer: Consumer::published_service(svc.id.clone()),
                    delivery: Delivery::Env,
                    env_name: Some(l.key.clone()),
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: Some(expires.clone()),
                    version: None,
                };
                match vault.grant(g, &actor()) {
                    Ok(g) => {
                        line.reason = why.to_string();
                        line.grant_id = Some(g.id);
                        line.expires_at = Some(expires.clone());
                        report.granted.push(line);
                    }
                    Err(e) => {
                        line.reason = format!("the grant failed ({})", e.code);
                        report.refused.push(line);
                    }
                }
            }
        }
    }
    if !rerun {
        report.launch_refused = launch_block_refusals(vault);
    }
    report
}

/// Agent launch-block env (AGENT.md) references the new rule refuses.
/// Reported, never granted: the agent can edit that file.
fn launch_block_refusals(vault: &LocalVault) -> Vec<RefLine> {
    let projects: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT id, path FROM projects") else { return vec![] };
        let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else { return vec![] };
        rows.flatten().collect()
    };
    let mut out = vec![];
    for (id, path) in projects {
        // The workspace agent (any name finds it when present) plus named
        // agents under `.k2/agents/`.
        let mut names: BTreeSet<String> = BTreeSet::from(["-".to_string()]);
        if let Ok(rd) = std::fs::read_dir(k2_core::workspace::agent_identity::agents_dir(&path)) {
            names.extend(rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)));
        }
        let who = Requester::Session { workspace_id: id.clone(), session_id: None };
        let mut seen = BTreeSet::new();
        for name in names {
            let Ok(Some(profile)) = k2_core::workspace::launch_profile::load_launch_profile(&path, &name) else { continue };
            for (key, r) in profile.env.unwrap_or_default() {
                if !k2_vault::refs::VaultRef::is_ref(&r) || !seen.insert((key.clone(), r.clone())) {
                    continue;
                }
                let Ok(sel) = super::selector_for_ref(&r) else { continue };
                let Some(m) = old_rule_resolves(vault, &sel, None) else { continue };
                if vault.ref_would_resolve(&sel, &who).unwrap_or(false) {
                    continue;
                }
                let label = safe_label(&super::workspace_label(&id));
                out.push(RefLine {
                    service_id: String::new(),
                    service: String::new(),
                    workspace_id: id.clone(),
                    workspace: label,
                    key: safe_label(&key),
                    secret: m.name.clone(),
                    secret_id: m.id.clone(),
                    scope: m.scope.kind.as_str().to_string(),
                    reason: "in an agent launch block (AGENT.md), which the agent can edit; needs a grant to the workspace".into(),
                    ..Default::default()
                });
            }
        }
    }
    out
}

/// The `--scope` a printed command needs: the workspace **id** (round 2
/// fix 7: a sanitized label may not name the workspace).
fn scope_flag(l: &RefLine) -> String {
    if l.scope == "server" {
        "--scope server".into()
    } else {
        format!("--scope ws:{}", l.workspace_id)
    }
}

pub fn notice_body(r: &Report) -> String {
    let mut b = String::from(
        "K2 0.46.4 closes a gap: a published service or App could read any server-wide secret through a k2v:// line in \
         its .env.k2 with no grant, and an agent's launch env could read any secret. Now a reference needs a grant \
         to that service or workspace, like any shared secret. Names and service ids only below; no value was read \
         out or moved.\n",
    );
    if !r.granted.is_empty() {
        b.push_str(&format!(
            "\nKept working for {} days (K2 saw each service use it, so it granted each to that one service, as env). \
             Keep them on the card in the workspace's Thread, or they lapse:\n",
            r.grant_days
        ));
        for l in &r.granted {
            b.push_str(&format!(
                "- service {} (\"{}\", workspace {}): {} = {} ({} secret). Keep for good: k2 vault grant {} {} --to service:{} --as env --env-name {}\n",
                l.service_id, l.service, l.workspace, l.key, l.secret, l.scope, l.secret, scope_flag(l), l.service_id, l.key
            ));
        }
    }
    if !r.refused.is_empty() {
        b.push_str("\nNow refused (the service starts without it):\n");
        for l in &r.refused {
            b.push_str(&format!(
                "- service {} (\"{}\", workspace {}): {} = {} ({} secret): {}. To allow: k2 vault grant {} {} --to service:{} --as env --env-name {}\n",
                l.service_id, l.service, l.workspace, l.key, l.secret, l.scope, l.reason, l.secret, scope_flag(l), l.service_id, l.key
            ));
        }
    }
    if !r.launch_refused.is_empty() {
        b.push_str("\nAgent launch blocks now refused (the agent's next session starts without it):\n");
        for l in &r.launch_refused {
            b.push_str(&format!(
                "- workspace {} ({}): {} = {}: {}. To allow: k2 vault grant {} {} --to ws:{} --as env\n",
                l.workspace, l.workspace_id, l.key, l.secret, l.reason, l.secret, scope_flag(l), l.workspace_id
            ));
        }
    }
    if !r.unreadable.is_empty() {
        b.push_str("\nK2 couldn't read these, so it will check again at the next start:\n");
        for u in &r.unreadable {
            b.push_str(&format!("- {}\n", safe_text(u)));
        }
    }
    b
}

/// Free text for the notice: control characters and backticks removed.
fn safe_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && *c != '`').take(300).collect()
}

/// What the new rule refuses, listed without the audit chain: every
/// reference in a service's `.env.k2` that the old rule resolved and the new
/// one refuses. No grant is made and nothing is recorded.
fn chain_broken_report(vault: &LocalVault, code: &str) -> Report {
    let mut report = Report { done_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true), ..Default::default() };
    let cutoff = chrono::Utc::now().timestamp() - WINDOW_DAYS * 86_400;
    let svcs = match services(cutoff) {
        Ok(s) => s,
        Err(why) => {
            report.unreadable.push(why);
            return report;
        }
    };
    for svc in &svcs {
        let env_file = super::publish_env::get(&svc.id).env_file.unwrap_or_else(|| ".env.k2".into());
        let mut seen = BTreeSet::new();
        for dir in &svc.dirs {
            let found = match refs_in(dir, &env_file) {
                Read::Lines(l) => l,
                Read::Absent => continue,
                Read::Unreadable(why) => {
                    report.unreadable.push(format!("service {}: {why}", svc.id));
                    continue;
                }
            };
            for (key, r) in found {
                if !seen.insert((key.clone(), r.clone())) {
                    continue;
                }
                let Ok(sel) = super::selector_for_ref(&r) else { continue };
                let Some(m) = old_rule_resolves(vault, &sel, Some(&svc.workspace_id)) else { continue };
                let who = Requester::PublishedService { id: svc.id.clone(), workspace_id: svc.workspace_id.clone() };
                if vault.ref_would_resolve(&sel, &who).unwrap_or(false) {
                    continue;
                }
                report.refused.push(RefLine {
                    service_id: svc.id.clone(),
                    service: safe_label(&svc.name),
                    workspace_id: svc.workspace_id.clone(),
                    workspace: safe_label(&super::workspace_label(&svc.workspace_id)),
                    key,
                    secret: m.name.clone(),
                    secret_id: m.id.clone(),
                    scope: if m.scope.kind == ScopeKind::Server { "server".into() } else { "workspace".into() },
                    reason: format!("the vault audit chain doesn't verify ({code}), so K2 made no upgrade grants"),
                    ..Default::default()
                });
            }
        }
    }
    report
}

/// The owner notice when the audit chain doesn't verify (the pass stays
/// off: fail closed).
pub fn chain_broken_notice(code: &str, r: &Report) -> (String, String) {
    let title = "Vault: the audit log doesn't verify, so .env.k2 references weren't carried over".to_string();
    let mut b = format!(
        "K2 0.46.4 asks for a grant before a service reads a secret through a k2v:// line in .env.k2. On update K2 would \
         grant references that services were really using, but it decides that from the vault's audit log, and that log \
         doesn't verify ({code}). So K2 made no upgrade grants. Services using k2v://server/… (or fetch-only workspace) \
         references will start without them.\n\nCheck the vault: k2 vault doctor\n"
    );
    if r.refused.is_empty() {
        b.push_str("\nK2 found no such references in your services' .env.k2 files.\n");
    } else {
        b.push_str("\nTo let a service keep one, grant it (names and service ids only):\n");
        for l in &r.refused {
            b.push_str(&format!(
                "- service {} (\"{}\", workspace {}): {} = {} ({} secret): k2 vault grant {} {} --to service:{} --as env --env-name {}\n",
                l.service_id, l.service, l.workspace, l.key, l.secret, l.scope, l.secret, scope_flag(l), l.service_id, l.key
            ));
        }
    }
    if !r.unreadable.is_empty() {
        b.push_str("\nK2 couldn't read these, so they may hold more references:\n");
        for u in &r.unreadable {
            b.push_str(&format!("- {}\n", safe_text(u)));
        }
    }
    (title, b)
}

/// File one owner notice (tests record it instead).
fn file_notice(title: &str, body: &str) {
    #[cfg(not(test))]
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-vault", title, body) {
        k2_core::log_debug!("WARN [vault] R8 upgrade notice: {e}");
    }
    #[cfg(test)]
    {
        NOTICES.with(|n| n.set(n.get() + 1));
        LAST_NOTICE.with(|l| *l.borrow_mut() = format!("{title}\n{body}"));
    }
}

fn notify_owner(r: &Report) {
    let n = r.granted.len() + r.refused.len() + r.launch_refused.len();
    let title = format!("Vault: {n} secret reference{} now need a grant", if n == 1 { "" } else { "s" });
    let body = notice_body(r);
    crate::vault::auth_audit(
        "vault-r8-upgrade",
        "daemon",
        &format!("granted {} refused {} launch {} unreadable {}", r.granted.len(), r.refused.len(), r.launch_refused.len(), r.unreadable.len()),
        "-",
    );
    file_notice(&title, &body);
}

#[cfg(test)]
thread_local! {
    /// Notices filed on this test's thread.
    pub static NOTICES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The last one (title, newline, body).
    pub static LAST_NOTICE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// One card row (the action's params).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CardRow {
    pub grant_id: String,
    pub secret_id: String,
    pub service_id: String,
    pub key: String,
    pub secret: String,
}

/// The per-row button: unique on the card (service id prefix + key).
pub fn row_label(r: &CardRow) -> String {
    let short: String = r.service_id.chars().take(8).collect();
    format!("Keep {} for service {short}", r.key)
}

/// The card's words and buttons for these rows.
pub fn card_for(rows: &[CardRow]) -> (String, Vec<String>) {
    let mut prompt = format!(
        "K2 0.46.4 now asks for a grant before a service reads a secret through .env.k2. These kept working because K2 saw each \
         service use it; each lasts {UPGRADE_GRANT_DAYS} days unless you keep it:"
    );
    for r in rows {
        let short: String = r.service_id.chars().take(8).collect();
        prompt.push_str(&format!("\n- {} = {} (service {short})", r.key, r.secret));
    }
    let mut options = vec![KEEP_ALL.to_string()];
    if rows.len() > 1 {
        options.extend(rows.iter().take(MAX_ROW_OPTIONS).map(row_label));
    }
    options.push(LET_LAPSE.to_string());
    (prompt, options)
}

/// Post one keep card per workspace that has upgrade grants. Where a
/// workspace has no Thread, the notice's commands are the way.
pub fn present_cards(r: &Report) {
    let mut by_ws: BTreeMap<String, Vec<CardRow>> = BTreeMap::new();
    for l in &r.granted {
        let Some(g) = &l.grant_id else { continue };
        by_ws.entry(l.workspace_id.clone()).or_default().push(CardRow {
            grant_id: g.clone(),
            secret_id: l.secret_id.clone(),
            service_id: l.service_id.clone(),
            key: l.key.clone(),
            secret: l.secret.clone(),
        });
    }
    let expires_at = chrono::Utc::now().timestamp() + UPGRADE_GRANT_DAYS * 86_400;
    for (ws, rows) in by_ws {
        present_card(&ws, &rows, expires_at);
    }
}

/// Post (or replace) the keep card for `rows` on `workspace_id`'s Thread.
pub fn present_card(workspace_id: &str, rows: &[CardRow], expires_at: i64) {
    if rows.is_empty() {
        return;
    }
    let (prompt, options) = card_for(rows);
    let action = k2_core::overlay::CardAction {
        id: KEEP_ACTION.to_string(),
        params: serde_json::json!({ "workspaceId": workspace_id, "rows": rows, "expiresAt": expires_at }),
        dedupe: "r8-upgrade".to_string(),
        result: None,
        view: None,
    };
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    #[cfg(not(test))]
    match crate::overlay_routes::present_action_card(
        None,
        workspace_id,
        &prompt,
        &opts,
        action,
        true,
        expires_at,
        crate::overlay_routes::CardAudience::AgentPost,
    ) {
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("WARN [vault] R8 keep card for {workspace_id}: {e}"),
    }
    #[cfg(test)]
    let _ = (prompt, opts, action, expires_at);
}

/// Make one upgrade grant permanent, as the person who tapped. The grant
/// must still be the one the card names (same id, secret and service), and
/// still an upgrade grant with an expiry (round 2 fix 4): a card never
/// turns some other grant into a permanent one. A version pin is kept.
fn keep_one(vault: &LocalVault, row: &CardRow, person: &Actor) -> Result<(), String> {
    let grants = vault.grants_to(&Consumer::published_service(row.service_id.clone())).map_err(|e| e.code.to_string())?;
    let Some((g, _)) = grants.into_iter().find(|(g, _)| g.id == row.grant_id && g.secret_id == row.secret_id) else {
        return Err(format!("{} for service {} is no longer there", row.key, row.service_id));
    };
    if g.granted_by != actor().stamp() || g.expires_at.is_none() {
        return Err(format!("{} for service {} isn't a time-boxed upgrade grant any more", row.key, row.service_id));
    }
    vault
        .grant(
            GrantRequest {
                selector: Selector::Id(g.secret_id.clone()),
                consumer: g.consumer.clone(),
                delivery: g.delivery,
                env_name: g.env_name.clone(),
                file_name: g.file_name.clone(),
                hosts_narrowed: g.hosts_narrowed.clone(),
                expires_at: None,
                version: g.version_selector.parse::<u32>().ok(),
            },
            person,
        )
        .map(|_| ())
        .map_err(|e| format!("{} wasn't kept: {}", row.key, e.code))
}

/// The card action ([`crate::action_cards::run`]). `by` is the answering
/// person. The action-card answer route lets only the server's **owner**
/// answer (the owner token or an Owner login; agents, terminals, widgets,
/// App guests and Admins are refused there), and this checks the same
/// again: only the owner keeps (round 2 fix 3).
pub fn keep_action(params: &serde_json::Value, answer: &str, by: &str) -> Result<serde_json::Value, String> {
    let rows: Vec<CardRow> = serde_json::from_value(params["rows"].clone()).map_err(|_| "the card names no grants".to_string())?;
    let ws = params["workspaceId"].as_str().unwrap_or_default().to_string();
    let expires_at = params["expiresAt"].as_i64().unwrap_or(0);
    if answer == LET_LAPSE {
        return Ok(serde_json::json!({
            "kept": 0,
            "message": format!("They lapse after {UPGRADE_GRANT_DAYS} days; each service then shows the refused reference."),
        }));
    }
    let caller = if by == "owner" {
        crate::sidecar_routes::Caller::Owner
    } else {
        crate::sidecar_routes::Caller::Login { username: by.to_string() }
    };
    let c = crate::vault::actor::from_caller(caller, None).map_err(|e| e.hint)?;
    if !matches!(c.actor, Actor::Owner | Actor::Login { role: k2_vault::Role::Owner, .. }) {
        return Err("only the server's owner keeps a vault grant from this card".into());
    }
    let vault = crate::vault::get().map_err(|e| e.hint)?;
    let (chosen, rest): (Vec<CardRow>, Vec<CardRow>) = if answer == KEEP_ALL {
        (rows, vec![])
    } else {
        match rows.iter().position(|r| row_label(r) == answer) {
            Some(i) => {
                let mut rest = rows.clone();
                let one = rest.remove(i);
                (vec![one], rest)
            }
            None => return Err(format!("'{answer}' isn't an answer to this card")),
        }
    };
    let mut kept = 0;
    let mut problems = vec![];
    for r in &chosen {
        match keep_one(&vault, r, &c.actor) {
            Ok(()) => kept += 1,
            Err(e) => problems.push(e),
        }
    }
    if !rest.is_empty() {
        // The rest still wait: a fresh card for them (posted after this
        // answer closes, off the answer route's locks).
        let ws2 = ws.clone();
        std::thread::spawn(move || present_card(&ws2, &rest, expires_at));
    }
    let mut message = format!("Kept {kept} for good.");
    if !problems.is_empty() {
        message.push_str(&format!(" Not kept: {}.", problems.join("; ")));
    }
    Ok(serde_json::json!({ "kept": kept, "message": message }))
}

#[cfg(test)]
#[path = "server_refs_tests.rs"]
mod tests;

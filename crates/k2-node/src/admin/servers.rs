//! One node, many servers: the node owner's decisions
//! (prd-compute-shared-queue-v1 §3, §4, §9, §11.1).
//!
//! `/etc/k2-node/servers.toml` holds only what the node owner decided. It is
//! written here: by the root executor (approved `servers.*` verbs, and the
//! node's unsigned `servers.prune` op) or by the machine owner
//! (`k2-node servers …`). **Never by the node** (V45); the node's own
//! runtime state per server lives in its pin files.
//!
//! Beside it, in the config dir (root-written, node-readable):
//! - [`ENROLL_FILE`]: the one pending add's enrol credential (the code's
//!   binding, never the code; 10 minutes; group-readable by the node only);
//! - [`STOP_FILE`]: revoked slots whose running jobs stop now (`--now`).
//!
//! In the executor's root-only state dir, [`BOOK_FILE`]: when each revoke
//! happened (the wipe is 24 h later, or at once with `--wipe`).
//!
//! The user, group and per-tenant wall are root's `tenant.prepare` /
//! `tenant.wipe` (`admin/tenants.rs`, which also hands out tenant uids,
//! never reused, V64): [`prepare_tenant`] and [`wipe_slot`] call them as
//! root and write the uid into the server's line. A `--dev` test node
//! (no root) gets only its files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_node_proto::admin::{code, AdminRefusal, Class};
use k2_node_proto::share::{
    self, codes, AddArgs, LimitsArgs, Network, RevokeArgs, ServerArgs, ServerEntry, ServerListing, ServerStatus, ServersFile, ServersList, ShareLimits,
    ShareState, Slot, Trust, OWNER_SLOT, PENDING_SECS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::Layout;

/// The pending add's enrol credential (config dir, 0640 root:<node group>).
pub const ENROLL_FILE: &str = "server-enroll.toml";
/// What the node did with it (`run/`, 0600, node-written: it carries the
/// SAS; review B5).
pub const ENROLL_RESULT_FILE: &str = "server-enroll-result.json";
/// Pending servers' SAS, slot → SAS (`run/`, 0600, node-written; the
/// status file, 0644, never carries one).
pub const SAS_FILE: &str = "servers-sas.json";
/// Revoked slots whose running jobs stop at once (config dir, 0644).
pub const STOP_FILE: &str = "servers-stop.toml";
/// Root-only bookkeeping (executor state dir, 0600).
pub const BOOK_FILE: &str = "servers-book.json";
/// A revoked slot is wiped this long after the revoke (or at once).
pub const WIPE_AFTER_SECS: i64 = 24 * 3600;

const GIB: u64 = 1 << 30;

/// One pending add's enrol, for the node to run (§4.1 step 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEnroll {
    /// Tried once per id.
    pub id: String,
    pub slot: Slot,
    /// The new server's base URL.
    pub url: String,
    /// `pairing::code_binding` of the code: the credential (V79).
    pub code_binding: String,
    /// The code's 16 hex: the server's key must start with it.
    pub fp_prefix: String,
    /// Node clock; ignored after (the code's 10 minutes).
    pub expires_at: i64,
}

/// What the node did with a [`ServerEnroll`] (`run/`, readable by the
/// machine owner's tools).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollOutcome {
    pub id: String,
    pub slot: Slot,
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sas: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopFile {
    #[serde(default)]
    pub slots: Vec<Slot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Book {
    /// slot → (revoked at, wipe at once).
    #[serde(default)]
    revoked: BTreeMap<String, (i64, bool)>,
}

/// Where and as whom a `servers.*` change runs.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub config_dir: PathBuf,
    /// Root-only state (the executor's).
    pub state_dir: PathBuf,
    pub home: PathBuf,
    pub now: i64,
    pub macos: bool,
    /// Jobs here run behind the job wall (`job-wall`; V59).
    pub walled: bool,
    /// The per-tenant wall is known to be ready (tests). `false` = ask
    /// root's executor when it matters (a `team` add, Linux, as root).
    pub tenant_wall: bool,
    /// The owner server's fingerprint as root pinned it (`admin.toml`).
    pub pinned_owner: Option<String>,
    pub node_user: String,
    /// `owner-approval:<cmd id>` or `local:<user>`.
    pub by: String,
    /// Root's own tenant ops run here (`tenant.prepare` / `tenant.wipe`):
    /// root's executor and `sudo k2-node servers`. Off for a `--dev` test
    /// node's files-only path and for tests.
    pub root: bool,
}

impl Ctx {
    pub fn layout(&self) -> Layout {
        Layout::new(&self.home, &self.config_dir)
    }

    /// The executor's context. The wall facts come from root's own files
    /// where it has them (Linux: `jobs.toml`) and the node's status file
    /// otherwise.
    pub fn for_executor(e: &super::exec::Executor, by: &str) -> Self {
        let cfg = super::config::read(&e.paths.admin()).ok().flatten();
        let macos = e.platform.os() == "macos";
        let layout = e.paths.layout();
        Self {
            config_dir: e.paths.config_dir.clone(),
            state_dir: e.paths.state_dir.clone(),
            home: e.paths.home.clone(),
            now: e.now(),
            macos,
            walled: node_walled(&layout, macos, false),
            tenant_wall: false,
            pinned_owner: cfg.as_ref().and_then(|c| c.controller.as_ref().map(|x| x.fp.clone())),
            node_user: cfg.map(|c| c.node_user).unwrap_or_else(|| super::config::default_node_user().to_string()),
            by: by.to_string(),
            // The executor is root in life; its unit tests drive it in
            // process (as root too in the Linux container) with fake
            // platforms and no users: they keep the files-only path.
            root: is_root() && !cfg!(test),
        }
    }
}

/// Does this node wall its jobs? Linux: root's `jobs.toml` names a job
/// user. macOS: Seatbelt is there and the running node says it uses it.
/// `allow_dev` (the machine owner's own CLI only, never the executor: the
/// status file is the node user's): a `--dev` test node, which has no wall
/// by design, may be shared for tests.
pub fn node_walled(layout: &Layout, macos: bool, allow_dev: bool) -> bool {
    let status = crate::status::read(&layout.status()).ok();
    if allow_dev && crate::node::dev_test_exemption(status.as_ref().is_some_and(|s| s.dev), &layout.config) {
        return true;
    }
    let status_wall = status.map(|s| s.wall).unwrap_or_default();
    use k2_node_proto::health::{WALL_JOB_USER, WALL_SEATBELT};
    if macos {
        crate::wall::seatbelt_available() && status_wall == WALL_SEATBELT
    } else {
        matches!(super::jobs::read_jobs_file(&layout.config), Ok(Some(_))) && status_wall == WALL_JOB_USER
    }
}

impl Ctx {
    /// Root's tenant ops (`tenant.prepare` / `tenant.wipe`) for this
    /// context: the same files the root executor reads.
    fn executor(&self) -> super::exec::Executor {
        super::exec::Executor::new(
            super::exec::ExecPaths {
                config_dir: self.config_dir.clone(),
                state_dir: self.state_dir.clone(),
                home: self.home.clone(),
                binary: std::env::current_exe().unwrap_or_default(),
            },
            std::sync::Arc::new(super::exec::SystemPlatform),
        )
    }

    /// A wall between tenants that takes other people's servers (`team`):
    /// the per-tenant users on Linux, as root's executor finds them ready
    /// (job wall, systemd ≥ 247, the tools, its runtime self-test). Never
    /// on macOS in 0.46.3 (§8.2), nor without root.
    fn team_wall_ready(&self) -> Result<(), AdminRefusal> {
        if self.tenant_wall {
            return Ok(());
        }
        if self.macos || !self.root {
            return Err(refuse(codes::TEAM_NEEDS_TENANT_WALL, "other people's servers need the per-tenant wall on this node, which isn't set up"));
        }
        self.executor().tenant_wall_ready()
    }
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

fn failed(m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(code::FAILED, m)
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// ── files ────────────────────────────────────────────────────────────

pub fn servers_path(config_dir: &Path) -> PathBuf {
    config_dir.join(share::SERVERS_FILE)
}

/// `Ok(None)` = no file = solo mode. A file that doesn't parse or validate
/// is an error (callers never treat it as solo: fail closed).
pub fn read_file(config_dir: &Path) -> Result<Option<ServersFile>, String> {
    let p = servers_path(config_dir);
    let text = match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", p.display())),
    };
    let f: ServersFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
    f.validate().map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(f))
}

/// Write `servers.toml` (0664; as root: root and the config dir's group,
/// `k2nodectl`). A file that wouldn't validate is never written.
pub fn write_file(config_dir: &Path, f: &ServersFile) -> Result<(), String> {
    f.validate()?;
    let body = toml::to_string(f).map_err(|e| format!("encode servers.toml: {e}"))?;
    let text = format!(
        "# K2 node servers (prd-compute-shared-queue-v1 §11.1): the node owner's decisions only.\n\
         # Change it with `sudo k2-node servers …` here, or `k2 compute node servers …` from the owner server.\n{body}"
    );
    crate::util::atomic_write_owned(&servers_path(config_dir), text.as_bytes(), 0o664, dir_group(config_dir))
}

/// As root: root and the config dir's group (`k2nodectl`), set on the new
/// file's fd before it's renamed in (never a chown by path, review A M5).
fn dir_group(dir: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    if !is_root() {
        return None;
    }
    std::fs::symlink_metadata(dir).ok().filter(|m| m.is_dir()).map(|m| (0, m.gid()))
}

/// The node user's uid and primary gid.
fn user_ids(name: &str) -> Option<(u32, u32)> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getpwnam with a valid C string; fields read before any other
    // passwd call.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() {
            None
        } else {
            Some(((*pw).pw_uid, (*pw).pw_gid))
        }
    }
}

/// A credential file: 0600 (never briefly world-readable); as root,
/// root's and the node user's group's, 0640 (set on its fd).
fn write_private(p: &Path, bytes: &[u8], node_user: &str) -> Result<(), String> {
    match user_ids(node_user).filter(|_| is_root()) {
        Some((_, gid)) => crate::util::atomic_write_owned(p, bytes, 0o640, Some((0, gid))),
        None => crate::util::atomic_write(p, bytes, 0o600),
    }
}

pub fn enroll_path(config_dir: &Path) -> PathBuf {
    config_dir.join(ENROLL_FILE)
}

pub fn read_enroll(config_dir: &Path) -> Result<Option<ServerEnroll>, String> {
    let p = enroll_path(config_dir);
    match std::fs::read_to_string(&p) {
        Ok(s) => toml::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

pub fn stop_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STOP_FILE)
}

/// Revoked slots whose running jobs stop now (missing = none).
pub fn read_stop(config_dir: &Path) -> Result<StopFile, String> {
    let p = stop_path(config_dir);
    match std::fs::read_to_string(&p) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StopFile::default()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

fn set_stop(config_dir: &Path, slot: Slot, on: bool) -> Result<(), String> {
    let mut s = read_stop(config_dir).unwrap_or_default();
    let had = s.slots.contains(&slot);
    if had == on {
        return Ok(());
    }
    s.slots.retain(|x| *x != slot);
    if on {
        s.slots.push(slot);
        s.slots.sort();
    }
    let p = stop_path(config_dir);
    if s.slots.is_empty() {
        return match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("remove {}: {e}", p.display())),
            _ => Ok(()),
        };
    }
    let body = toml::to_string(&s).map_err(|e| e.to_string())?;
    crate::util::atomic_write_owned(&p, body.as_bytes(), 0o644, dir_group(config_dir))
}

fn book_path(state_dir: &Path) -> PathBuf {
    state_dir.join(BOOK_FILE)
}

fn read_book(state_dir: &Path) -> Result<Book, String> {
    let p = book_path(state_dir);
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Book::default()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

fn write_book(state_dir: &Path, b: &Book) -> Result<(), String> {
    crate::util::mkdir_mode(state_dir, 0o700)?;
    let body = serde_json::to_vec_pretty(b).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&book_path(state_dir), &body, 0o600)
}

// ── shared helpers ───────────────────────────────────────────────────

/// A plain label from a URL's host (`https://baden.k2.dev:8443/` →
/// `baden.k2.dev-8443`).
pub fn label_of(url: &str) -> String {
    let host = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    let host = host.split('/').next().unwrap_or(host);
    let host = host.rsplit('@').next().unwrap_or(host);
    let mut s: String = host.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    s.truncate(64);
    if s.is_empty() {
        "server".into()
    } else {
        s
    }
}

/// The owner line the first add writes (§11.1): from `controller.json`
/// (which must match root's pinned controller) and `policy.toml`.
fn owner_file(ctx: &Ctx) -> Result<ServersFile, AdminRefusal> {
    let layout = ctx.layout();
    let pin = crate::identity::read_pin(&layout.pin())
        .map_err(failed)?
        .filter(|p| !p.revoked)
        .ok_or_else(|| refuse(code::NOT_CONFIGURED, "this node has no owner server yet: enrol it first"))?;
    if let Some(fp) = &ctx.pinned_owner {
        if *fp != pin.controller_fp {
            return Err(refuse(
                code::NOT_CONFIGURED,
                "the enrolled owner server isn't the controller root pinned (sudo k2-node admin pin after a re-enrol)",
            ));
        }
    }
    let facts = crate::node::machine_facts(&layout);
    let policy = crate::config::read_policy_file(&layout.policy())
        .and_then(|f| crate::config::Policy::resolve(&f, &facts))
        .map_err(|e| refuse(code::NOT_CONFIGURED, format!("policy.toml: {e}")))?;
    let cpus = policy.cpu_cap_millis(facts.cores).div_ceil(1000).clamp(1, 1024) as u32;
    let mem = (policy.mem_cap_bytes / GIB).clamp(1, 4096) as u32;
    let disk = (policy.disk_budget_bytes / GIB).clamp(1, 65536) as u32;
    let limits = ShareLimits::owner(policy.max_parallel.clamp(1, 64), cpus, mem, disk, Network::of_policy(&policy.network));
    let max = share::DEFAULT_MAX_SERVERS;
    Ok(ServersFile::new(&pin.controller_fp, &label_of(pin.routes.first().map(String::as_str).unwrap_or("owner")), limits, max, ctx.now, &ctx.by))
}

/// Does the node hold a confirmed pin for this line?
fn pin_confirmed(layout: &Layout, e: &ServerEntry) -> bool {
    matches!(crate::identity::read_tenant_pin(&layout.tenant(e.slot).pin()), Ok(Some((p, _))) if p.confirmed && !p.revoked && crate::node::tenant::pin_serves(e, &p))
}

fn find_mut<'a>(f: &'a mut ServersFile, server: &str) -> Result<&'a mut ServerEntry, AdminRefusal> {
    let slot = f.find(server).map(|e| e.slot).ok_or_else(|| refuse(code::BAD_ARGS, format!("no server {server:?} on this node")))?;
    Ok(f.servers.iter_mut().find(|e| e.slot == slot).expect("found above"))
}

fn args<T: serde::de::DeserializeOwned>(verb: &str, v: &Value) -> Result<T, AdminRefusal> {
    share::check_verb_args(verb, v).map_err(|m| refuse(code::BAD_ARGS, m))?;
    serde_json::from_value(v.clone()).map_err(|e| refuse(code::BAD_ARGS, format!("{verb}: {e}")))
}

// ── verbs ────────────────────────────────────────────────────────────

/// Run one `servers.*` verb (the executor after its checks, or the machine
/// owner's CLI).
pub fn run(ctx: &Ctx, verb: &str, a: &Value) -> Result<Value, AdminRefusal> {
    let _g = if verb == share::VERB_LIST { None } else { Some(servers_lock(ctx)?) };
    match verb {
        share::VERB_ADD => add(ctx, &args(verb, a)?),
        share::VERB_LIMITS => limits(ctx, &args(verb, a)?),
        share::VERB_PAUSE => pause(ctx, &args::<ServerArgs>(verb, a)?.server, true),
        share::VERB_RESUME => pause(ctx, &args::<ServerArgs>(verb, a)?.server, false),
        share::VERB_REVOKE => revoke(ctx, &args(verb, a)?),
        share::VERB_LIST => {
            args::<Value>(verb, a)?;
            Ok(list_file(ctx))
        }
        _ => Err(refuse(code::VERB_NOT_ALLOWED, format!("{verb} is not a servers verb"))),
    }
}

/// `servers.add` (§4.1): a pending line plus the node's enrol credential.
pub fn add(ctx: &Ctx, a: &AddArgs) -> Result<Value, AdminRefusal> {
    if !ctx.walled {
        return Err(refuse(
            codes::TENANT_NEEDS_JOB_WALL,
            "this node runs jobs without the job wall; a second server needs it (Linux: sudo k2-node admin job-user k2job; macOS: the node's Seatbelt profile)",
        ));
    }
    if a.trust == Trust::Team && ctx.macos && !ctx.tenant_wall {
        return Err(refuse(
            codes::TEAM_NEEDS_TENANT_WALL,
            "other people's servers need per-tenant users, which come to macOS nodes in 0.46.4; add your own servers with --trust own",
        ));
    }
    if a.trust == Trust::Team {
        ctx.team_wall_ready()?;
    }
    let mut f = match read_file(&ctx.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))? {
        Some(f) => f,
        None => owner_file(ctx)?,
    };
    if let Some(fp) = &ctx.pinned_owner {
        if *fp != f.owner {
            return Err(refuse(code::NOT_CONFIGURED, "servers.toml names another owner than the controller root pinned"));
        }
    }
    let mut pruned = Vec::new();
    prune_file(ctx, &mut f, &mut pruned);
    wipe_failed_now(ctx, &pruned)?;
    if f.live_pending(ctx.now).is_some() {
        return Err(refuse(codes::SERVER_ADD_PENDING, "another server is waiting for its SAS; confirm it or wait 10 minutes"));
    }
    let pending = f.servers.iter().filter(|e| e.state == ShareState::Pending).count();
    if f.active_count(ctx.now) + pending >= f.max_servers as usize {
        return Err(refuse(codes::MAX_SERVERS_REACHED, format!("this node takes at most {} servers (max_servers)", f.max_servers)));
    }
    let label = label_of(&a.url);
    let clash = f.servers.iter().any(|e| {
        e.state != ShareState::Revoked && (e.label == label || e.fp_prefix.starts_with(&a.fp_prefix) || a.fp_prefix.starts_with(&e.fp_prefix))
    });
    if clash {
        return Err(refuse(codes::SERVER_EXISTS, format!("{label} (or a server with that key) is already on this node")));
    }
    let slot = f.free_slot().ok_or_else(|| refuse(code::BAD_ARGS, "no free slot (wipe a revoked server first)"))?;
    let limits = a.limits.apply(&ShareLimits::new_server(a.trust, ctx.macos));
    limits.validate().map_err(|m| refuse(code::BAD_ARGS, m))?;
    let mut e = f.servers[0].clone();
    e.fp_prefix = a.fp_prefix.clone();
    e.label = label.clone();
    e.state = ShareState::Pending;
    e.trust = a.trust;
    e.slot = slot;
    // Root's tenant.prepare hands it out (below), never reused (V64).
    e.uid = None;
    e.set_limits(&limits);
    e.expires_at = match (a.for_secs, a.trust) {
        (Some(s), _) => Some(ctx.now + s),
        (None, Trust::Team) => Some(ctx.now + 7 * 24 * 3600),
        (None, Trust::Own) => None,
    };
    e.pending_until = ctx.now + PENDING_SECS;
    e.added_at = ctx.now;
    e.added_by = ctx.by.clone();
    f.servers.push(e.clone());
    f.validate().map_err(|m| refuse(code::BAD_ARGS, m))?;
    // The pending line first (lane S3's tenant.prepare acts only on a slot
    // servers.toml has), then the tenant's user and root; a failed prepare
    // takes the line back out (and the file, if this add created it).
    let had_file = servers_path(&ctx.config_dir).exists();
    set_stop(&ctx.config_dir, slot, false).map_err(failed)?;
    write_file(&ctx.config_dir, &f).map_err(failed)?;
    let uid = match prepare_tenant(ctx, &e) {
        Ok(uid) => uid,
        Err(r) => {
            if had_file {
                f.servers.retain(|x| x.slot != slot);
                let _ = write_file(&ctx.config_dir, &f);
            } else {
                let _ = std::fs::remove_file(servers_path(&ctx.config_dir));
            }
            return Err(r);
        }
    };
    // The line records the uid its user got (Linux; one uid on macOS).
    if uid.is_some() {
        if let Some(x) = f.servers.iter_mut().find(|x| x.slot == slot) {
            x.uid = uid;
        }
        write_file(&ctx.config_dir, &f).map_err(failed)?;
    }
    // Last: the credential the node enrols with.
    let enroll = ServerEnroll {
        id: format!("add-{slot}-{}", k2_node_proto::crypto::random_id().map_err(failed)?),
        slot,
        url: a.url.trim_end_matches('/').to_string(),
        code_binding: a.code_binding.clone(),
        fp_prefix: a.fp_prefix.clone(),
        expires_at: e.pending_until,
    };
    let body = toml::to_string(&enroll).map_err(|x| failed(x.to_string()))?;
    write_private(&enroll_path(&ctx.config_dir), body.as_bytes(), &ctx.node_user).map_err(failed)?;
    Ok(json!({
        "slot": slot,
        "label": label,
        "state": "pending",
        "trust": a.trust.as_str(),
        "uid": uid,
        "pending_until": e.pending_until,
        "expires_at": e.expires_at,
        "limits": limits,
    }))
}

/// The share `servers.limits` would leave, and its end date.
fn limits_after(ctx_now: i64, e: &ServerEntry, a: &LimitsArgs) -> (ShareLimits, Option<i64>) {
    let l = a.set.apply(&e.limits());
    let exp = match a.for_secs {
        None => e.expires_at,
        Some(0) => None,
        Some(s) => Some(ctx_now + s),
    };
    (l, exp)
}

/// What a `servers.limits` would widen (empty = a pure narrowing). A later
/// or cleared end date is a widening too.
pub fn widens(f: &ServersFile, a: &LimitsArgs, now: i64) -> Vec<&'static str> {
    let Some(e) = f.find(&a.server) else { return Vec::new() };
    let (l, exp) = limits_after(now, e, a);
    let mut w = l.widens(&e.limits());
    let later = match (e.expires_at, exp) {
        (Some(_), None) => true,
        (Some(old), Some(new)) => new > old,
        _ => false,
    };
    if later {
        w.push("expires_at");
    }
    w
}

/// The executor's own class for `servers.limits`: Operate when it only
/// narrows, Privileged (owner approval) when it widens (§4.3).
pub fn classify_limits(config_dir: &Path, a: &Value, now: i64) -> Result<Class, AdminRefusal> {
    let a: LimitsArgs = args(share::VERB_LIMITS, a)?;
    let f = read_file(config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))?;
    Ok(match f {
        Some(f) if !widens(&f, &a, now).is_empty() => Class::Privileged,
        _ => Class::Operate,
    })
}

pub fn limits(ctx: &Ctx, a: &LimitsArgs) -> Result<Value, AdminRefusal> {
    let mut f = read_file(&ctx.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))?.ok_or_else(|| refuse(code::BAD_ARGS, "this node isn't shared"))?;
    let widened = widens(&f, a, ctx.now);
    let e = find_mut(&mut f, &a.server)?;
    if e.state == ShareState::Revoked {
        return Err(refuse(code::BAD_ARGS, "that server is revoked"));
    }
    let (l, exp) = limits_after(ctx.now, e, a);
    l.validate().map_err(|m| refuse(code::BAD_ARGS, m))?;
    if e.slot == OWNER_SLOT && exp.is_some() {
        return Err(refuse(code::BAD_ARGS, "the owner server has no end date"));
    }
    e.set_limits(&l);
    e.expires_at = exp;
    let (slot, label) = (e.slot, e.label.clone());
    write_file(&ctx.config_dir, &f).map_err(failed)?;
    Ok(json!({"slot": slot, "label": label, "limits": l, "expires_at": exp, "widened": widened}))
}

/// `servers.pause` / `servers.resume`: queued work waits; running jobs
/// finish (§4.3).
pub fn pause(ctx: &Ctx, server: &str, on: bool) -> Result<Value, AdminRefusal> {
    let mut f = read_file(&ctx.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))?.ok_or_else(|| refuse(code::BAD_ARGS, "this node isn't shared"))?;
    let e = find_mut(&mut f, server)?;
    if e.slot == OWNER_SLOT {
        return Err(refuse(code::BAD_ARGS, "pause the whole node instead (k2-node pause, or k2 compute node pause)"));
    }
    let (from, to) = if on { (ShareState::Active, ShareState::Paused) } else { (ShareState::Paused, ShareState::Active) };
    if e.state != from {
        return Err(refuse(code::BAD_ARGS, format!("{} is {}, not {}", e.label, e.state.as_str(), from.as_str())));
    }
    e.state = to;
    let (slot, label) = (e.slot, e.label.clone());
    write_file(&ctx.config_dir, &f).map_err(failed)?;
    Ok(json!({"slot": slot, "label": label, "state": to.as_str()}))
}

/// `servers.revoke` (§4.3): unusable at once; the node ends the session;
/// running jobs finish (or stop with `now`); the slot is wiped after 24 h
/// (or at once with `wipe`).
pub fn revoke(ctx: &Ctx, a: &RevokeArgs) -> Result<Value, AdminRefusal> {
    let mut f = read_file(&ctx.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))?.ok_or_else(|| refuse(code::BAD_ARGS, "this node isn't shared"))?;
    let e = find_mut(&mut f, &a.server)?;
    if e.slot == OWNER_SLOT {
        return Err(refuse(code::BAD_ARGS, "the owner server can't be revoked here; re-enrol the node with --force at the machine"));
    }
    if e.state == ShareState::Revoked && !a.now && !a.wipe {
        return Err(refuse(code::BAD_ARGS, format!("{} is already revoked", e.label)));
    }
    e.state = ShareState::Revoked;
    let (slot, label) = (e.slot, e.label.clone());
    let mut b = read_book(&ctx.state_dir).map_err(failed)?;
    let at = b.revoked.get(&slot.to_string()).map(|r| r.0).unwrap_or(ctx.now);
    b.revoked.insert(slot.to_string(), (at, a.wipe));
    write_book(&ctx.state_dir, &b).map_err(failed)?;
    // A wipe at once takes the files from under running jobs: they stop too.
    if a.now || a.wipe {
        set_stop(&ctx.config_dir, slot, true).map_err(failed)?;
    }
    if read_enroll(&ctx.config_dir).ok().flatten().is_some_and(|x| x.slot == slot) {
        let _ = std::fs::remove_file(enroll_path(&ctx.config_dir));
    }
    write_file(&ctx.config_dir, &f).map_err(failed)?;
    let mut wiped = Vec::new();
    if a.wipe {
        let r = prune_locked(ctx)?;
        wiped = r.get("wiped").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if let Some(e) = r["wipe_failed"].as_array().and_then(|v| v.iter().find(|x| x["slot"] == slot)) {
            return Err(failed(format!("{label} is revoked, but wiping slot {slot} failed: {} (retried on every prune)", e["error"].as_str().unwrap_or("?"))));
        }
    }
    Ok(json!({"slot": slot, "label": label, "state": "revoked", "now": a.now, "wipe_at": if a.wipe { ctx.now } else { at + WIPE_AFTER_SECS }, "wiped": wiped}))
}

/// Due housekeeping on `f` (no write): confirmed pending lines become
/// active; pending lines past their 10 minutes go. Returns what changed.
fn prune_file(ctx: &Ctx, f: &mut ServersFile, changed: &mut Vec<String>) {
    let layout = ctx.layout();
    let enroll = read_enroll(&ctx.config_dir).ok().flatten();
    let mut keep = Vec::new();
    for mut e in std::mem::take(&mut f.servers) {
        if e.state == ShareState::Pending {
            if pin_confirmed(&layout, &e) && e.expires_at.is_none_or(|x| ctx.now < x) {
                e.state = ShareState::Active;
                e.pending_until = 0;
                changed.push(format!("promoted:{}", e.slot));
            } else if ctx.now >= e.pending_until {
                if enroll.as_ref().is_some_and(|x| x.slot == e.slot) {
                    let _ = std::fs::remove_file(enroll_path(&ctx.config_dir));
                }
                // It never took a job: its unconfirmed pin, user and empty
                // dirs go, so a re-add enrols afresh. A wipe that failed
                // keeps the line (expired by time, holding its slot) and is
                // tried again on the next prune, as a revoked line's is: a
                // slot whose user wasn't removed is never handed out again
                // (review B10, V64).
                match wipe_slot(ctx, &e) {
                    Ok(()) => {
                        changed.push(format!("expired:{}", e.slot));
                        continue;
                    }
                    Err(x) => {
                        crate::nlog!("servers: wipe expired slot {}: {} (kept as revoked; retried on every prune)", e.slot, x.message);
                        changed.push(format!("wipe_failed:{}", e.slot));
                        // An ended line nobody may use, holding its slot.
                        e.state = ShareState::Revoked;
                    }
                }
            }
        }
        keep.push(e);
    }
    f.servers = keep;
}

/// `servers.prune` (the node's ticker, and before every `servers.*` write):
/// promote confirmed adds, drop expired pending lines, wipe revoked slots
/// that are due. Idempotent.
pub fn prune(ctx: &Ctx) -> Result<Value, AdminRefusal> {
    let _g = servers_lock(ctx)?;
    prune_locked(ctx)
}

/// One `servers.*` change at a time: root's executor (signed verbs on its
/// connection threads, the node's `servers.prune`) and `sudo k2-node
/// servers …` read-modify-write the same `servers.toml`, book and stop
/// file. Lock order: the executor's `busy`, then this, then the tenant
/// ops lock, then the audit log's.
pub const SERVERS_LOCK_FILE: &str = ".servers.lock";

fn servers_lock(ctx: &Ctx) -> Result<crate::util::FileLock, AdminRefusal> {
    // Beside servers.toml: the machine owner's group (pause, resume,
    // limits without sudo) and root both reach it.
    std::fs::create_dir_all(&ctx.config_dir).map_err(|x| failed(format!("create {}: {x}", ctx.config_dir.display())))?;
    crate::util::FileLock::exclusive(&ctx.config_dir.join(SERVERS_LOCK_FILE)).map_err(failed)
}

fn prune_locked(ctx: &Ctx) -> Result<Value, AdminRefusal> {
    let Some(mut f) = read_file(&ctx.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))? else {
        return Ok(json!({"changed": [], "wiped": []}));
    };
    let before = f.clone();
    let mut changed = Vec::new();
    prune_file(ctx, &mut f, &mut changed);
    wipe_failed_now(ctx, &changed)?;
    let mut b = read_book(&ctx.state_dir).map_err(failed)?;
    let mut wiped = Vec::new();
    let mut wipe_failed = Vec::new();
    let revoked: Vec<ServerEntry> = f.servers.iter().filter(|e| e.state == ShareState::Revoked).cloned().collect();
    for e in revoked {
        let key = e.slot.to_string();
        let (at, now) = *b.revoked.entry(key.clone()).or_insert((ctx.now, false));
        if now || ctx.now >= at + WIPE_AFTER_SECS {
            // A wipe that fails keeps the line (and its slot) for the next
            // prune; what else this pass decided is still written.
            if let Err(x) = wipe_slot(ctx, &e) {
                crate::nlog!("servers: wipe slot {}: {} (kept; retried on the next prune)", e.slot, x.message);
                wipe_failed.push(json!({"slot": e.slot, "error": x.message}));
                continue;
            }
            f.servers.retain(|x| x.slot != e.slot);
            b.revoked.remove(&key);
            set_stop(&ctx.config_dir, e.slot, false).map_err(failed)?;
            wiped.push(e.slot);
        }
    }
    write_book(&ctx.state_dir, &b).map_err(failed)?;
    if f != before {
        write_file(&ctx.config_dir, &f).map_err(failed)?;
    }
    Ok(json!({"changed": changed, "wiped": wiped, "wipe_failed": wipe_failed}))
}

/// Expired adds whose wipe failed (now revoked lines): wiped again on
/// every prune, not 24 h later.
fn wipe_failed_now(ctx: &Ctx, changed: &[String]) -> Result<(), AdminRefusal> {
    let slots: Vec<&str> = changed.iter().filter_map(|c| c.strip_prefix("wipe_failed:")).collect();
    if slots.is_empty() {
        return Ok(());
    }
    let mut b = read_book(&ctx.state_dir).map_err(failed)?;
    for s in slots {
        b.revoked.insert(s.to_string(), (ctx.now, true));
    }
    write_book(&ctx.state_dir, &b).map_err(failed)
}

/// Put the pending servers' SAS into the status lines, from the node's
/// 0600 SAS file, when this user may read it (the node user, root).
pub fn fill_sas(layout: &Layout, lines: &mut [crate::status::ServerLine]) {
    let Ok(b) = std::fs::read(layout.run().join(SAS_FILE)) else { return };
    let Ok(m) = serde_json::from_slice::<BTreeMap<String, String>>(&b) else { return };
    for l in lines {
        if let Some(s) = m.get(&l.slot.to_string()) {
            l.sas = Some(s.clone());
        }
    }
}

/// The lines as the file says them (the machine CLI adds the node's view).
pub fn list_file(ctx: &Ctx) -> Value {
    match read_file(&ctx.config_dir) {
        Ok(None) => json!({"shared": false, "servers": []}),
        Ok(Some(f)) => json!({
            "shared": f.shared(ctx.now),
            "owner": f.owner,
            "max_servers": f.max_servers,
            "servers": f.servers.iter().map(|e| json!({
                "slot": e.slot,
                "label": e.label,
                "role": if e.slot == OWNER_SLOT { "owner" } else { "tenant" },
                "state": e.status(ctx.now).as_str(),
                "trust": e.trust.as_str(),
                "limits": e.limits(),
                "expires_at": e.expires_at,
                "pending_until": (e.state == ShareState::Pending).then_some(e.pending_until),
                "added_at": e.added_at,
                "added_by": e.added_by,
            })).collect::<Vec<_>>(),
        }),
        Err(e) => json!({"error": e}),
    }
}

/// `servers.list` (a read verb the node answers for its owner server, §9):
/// the pinned [`ServersList`]: every line with what the node knows
/// (status by time and attach, the SAS while pending, online, running and
/// queued). The owner server mirrors it into `compute_node_servers`.
/// `Err` = `servers.toml` is broken (the node serves on with its last good
/// copy; the owner is told, never a fake "solo").
pub fn node_list(node: &crate::node::Node) -> Result<ServersList, String> {
    let now = node.now();
    let owner = node.owner();
    let running = node.running_by_tenant();
    let queued_of = |t: &crate::node::Tenant| t.ledger.lock().unwrap().queued().map(|q| q.len() as u32).unwrap_or(0);
    let Some(f) = read_file(&node.layout().config)? else {
        // Solo: the owner alone, implied by controller.json.
        let pin = owner.pin.lock().unwrap().clone();
        let servers = pin
            .map(|p| ServerListing {
                slot: OWNER_SLOT,
                label: label_of(p.routes.first().map(String::as_str).unwrap_or("owner")),
                state: if p.revoked { ServerStatus::Revoked } else { ServerStatus::Active },
                trust: Trust::Own,
                fp: Some(p.controller_fp.clone()),
                fp_prefix: p.controller_fp.clone(),
                limits: node.fairq.share(OWNER_SLOT, node).map(|s| s.limits).unwrap_or_else(|| ShareLimits::owner(1, 1, 1, 1, Network::On)),
                expires_at: None,
                expires_in_secs: None,
                added_at: p.enrolled_at,
                added_by: "enrol".into(),
                running: running.get(&OWNER_SLOT).map(|v| v.len() as u32).unwrap_or(0),
                queued: queued_of(&owner),
                cpu_secs_today: 0,
                sas: None,
                refusal: None,
                online: *owner.hub.connected.borrow() != 0,
            })
            .into_iter()
            .collect();
        return Ok(ServersList { max_servers: share::DEFAULT_MAX_SERVERS, solo: true, servers });
    };
    let servers = f
        .servers
        .iter()
        .map(|e| {
            // The open tenant serving this very line (a reused slot's old
            // tenant doesn't count).
            let t = node.tenant(e.slot).filter(|t| t.is_owner() || t.share().is_some_and(|i| i.entry.fp_prefix == e.fp_prefix && i.entry.added_at == e.added_at));
            let pin = t.as_ref().and_then(|t| t.pin.lock().unwrap().clone());
            let state = t.as_ref().and_then(|t| t.status_at(now)).unwrap_or_else(|| e.status(now));
            let refusal = t.as_ref().and_then(|t| t.flags.lock().unwrap().refused.clone());
            ServerListing {
                slot: e.slot,
                label: e.label.clone(),
                state,
                trust: e.trust,
                fp: pin.as_ref().map(|p| p.controller_fp.clone()),
                fp_prefix: e.fp_prefix.clone(),
                limits: e.limits(),
                expires_at: e.expires_at,
                expires_in_secs: e.expires_at.map(|x| (x - now).max(0)),
                added_at: e.added_at,
                added_by: e.added_by.clone(),
                running: running.get(&e.slot).map(|v| v.len() as u32).unwrap_or(0),
                queued: t.as_ref().map(|t| queued_of(t)).unwrap_or(0),
                // Per-day CPU lives in the queue store (lane S2): not exposed yet.
                cpu_secs_today: 0,
                sas: pin.as_ref().filter(|p| !p.confirmed && !p.revoked).map(|p| p.sas.replace(' ', "")),
                refusal: (state == ServerStatus::Refused).then_some(refusal).flatten(),
                online: t.as_ref().is_some_and(|t| *t.hub.connected.borrow() != 0),
            }
        })
        .collect();
    Ok(ServersList { max_servers: f.max_servers, solo: false, servers })
}

// ── the tenant wall (root's tenant.prepare / tenant.wipe) ───────────

/// The new tenant's user, group, slice and job-facing root: root's
/// `tenant.prepare` (V54, V64, V75), which reads the pending line this add
/// just wrote. Returns the uid it handed out (Linux; `None` on macOS,
/// where 0.46.3 tenants share the node's uid behind Seatbelt). A `--dev`
/// test node (not root) gets only a plain root dir.
pub fn prepare_tenant(ctx: &Ctx, e: &ServerEntry) -> Result<Option<u32>, AdminRefusal> {
    let root = ctx.layout().tenants_root();
    let dir = root.join(e.slot.to_string());
    if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(failed(format!("{} is a symlink", dir.display())));
    }
    if ctx.root {
        let ask = share::TenantPrepare { slot: e.slot, trust: e.trust, weight: e.weight, network: e.network };
        let r = ctx.executor().tenant_prepare(&ask)?;
        return Ok(r.get("uid").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()));
    }
    std::fs::create_dir_all(&dir).map_err(|x| failed(format!("create {}: {x}", dir.display())))?;
    Ok(None)
}

/// Wipe a revoked (or never-confirmed) slot: as root, `tenant.wipe` (its
/// units and processes, files, user and group; the uid stays burned); a
/// `--dev` test node's files only: the node-only state (pin, ledger,
/// journals, artifacts) and its job-facing root.
pub fn wipe_slot(ctx: &Ctx, e: &ServerEntry) -> Result<(), AdminRefusal> {
    if e.slot == OWNER_SLOT {
        return Err(failed("never wipe the owner"));
    }
    if ctx.root {
        return ctx.executor().tenant_wipe(&share::TenantWipe { slot: e.slot }).map(|_| ());
    }
    let t = ctx.layout().tenant(e.slot);
    for p in [t.state(), t.pin(), t.root()] {
        remove_tree(&p).map_err(failed)?;
    }
    Ok(())
}

/// Remove a file or tree without following a symlink at `p` (a link is
/// removed itself; `remove_dir_all` never follows links inside).
fn remove_tree(p: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", p.display())),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p).map_err(|e| format!("remove {}: {e}", p.display())),
        Ok(_) => std::fs::remove_file(p).map_err(|e| format!("remove {}: {e}", p.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node_proto::crypto::SigningKey;

    struct T {
        ctx: Ctx,
        owner: SigningKey,
    }

    fn setup(macos: bool) -> T {
        let d = crate::util::temp_dir("servers");
        let layout = Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        std::fs::write(layout.policy(), "max_parallel = 3\ncpu_cap_percent = 50\nnetwork = \"full\"\n").unwrap();
        let owner = SigningKey::generate().unwrap();
        let pin = crate::identity::Pin {
            controller_fp: owner.fingerprint(),
            controller_public_key_pem: owner.spki_pem(),
            routes: vec!["https://rosson.k2.dev".into()],
            node_id: "n0".into(),
            name: "mini-1".into(),
            enrolled_at: 1,
            sas: "000 000".into(),
            revoked: false,
            revoked_reason: None,
            labels: Default::default(),
            confirmed: true,
        };
        crate::identity::write_pin(&layout.pin(), &pin).unwrap();
        let ctx = Ctx {
            config_dir: layout.config.clone(),
            state_dir: d.join("admin-state"),
            home: layout.home.clone(),
            now: 1_000_000,
            macos,
            walled: true,
            tenant_wall: false,
            pinned_owner: Some(owner.fingerprint()),
            node_user: "nobody".into(),
            by: "local:test".into(),
            root: false,
        };
        T { ctx, owner }
    }

    fn add_args(url: &str, prefix: &str, trust: Trust) -> AddArgs {
        AddArgs { url: url.into(), code_binding: "binding-b64".into(), fp_prefix: prefix.into(), trust, for_secs: None, limits: Default::default() }
    }

    /// The first add writes the owner line from controller.json and the
    /// policy, then a pending line with the code's binding for the node.
    #[test]
    fn first_add_writes_the_owner_and_a_pending_line() {
        let t = setup(true);
        let r = add(&t.ctx, &add_args("https://z3thon.k2.dev", "0123456789abcdef", Trust::Own)).unwrap();
        assert_eq!((r["slot"].as_u64(), r["state"].as_str()), (Some(1), Some("pending")));
        let f = read_file(&t.ctx.config_dir).unwrap().unwrap();
        assert_eq!(f.owner, t.owner.fingerprint());
        assert_eq!(f.max_servers, share::DEFAULT_MAX_SERVERS, "three servers per node by default");
        let o = f.entry(0).unwrap();
        assert_eq!((o.label.as_str(), o.weight, o.gate, o.max_running), ("rosson.k2.dev", 1, true, 3));
        let b = f.entry(1).unwrap();
        assert_eq!((b.label.as_str(), b.state, b.trust, b.expires_at), ("z3thon.k2.dev", ShareState::Pending, Trust::Own, None));
        assert_eq!(b.pending_until, t.ctx.now + PENDING_SECS);
        assert_eq!(b.uid, None, "not root (a test): root's tenant.prepare hands out the uid (tenant_wall_linux S1)");
        let en = read_enroll(&t.ctx.config_dir).unwrap().unwrap();
        assert_eq!((en.slot, en.url.as_str(), en.code_binding.as_str()), (1, "https://z3thon.k2.dev", "binding-b64"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(enroll_path(&t.ctx.config_dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o007, 0, "the credential is never world-readable: {mode:o}");
        assert!(t.ctx.layout().tenants_root().join("1").is_dir(), "the tenant root is made");
    }

    #[test]
    fn adds_need_the_wall_and_team_needs_the_tenant_wall() {
        let mut t = setup(true);
        t.ctx.walled = false;
        let e = add(&t.ctx, &add_args("https://z3thon.k2.dev", "0123456789abcdef", Trust::Own)).unwrap_err();
        assert_eq!(e.code, codes::TENANT_NEEDS_JOB_WALL);
        assert!(read_file(&t.ctx.config_dir).unwrap().is_none(), "nothing written");
        t.ctx.walled = true;
        let e = add(&t.ctx, &add_args("https://baden.k2.dev", "0123456789abcdef", Trust::Team)).unwrap_err();
        assert_eq!(e.code, codes::TEAM_NEEDS_TENANT_WALL, "Macs take own servers only in 0.46.3");
        let mut l = setup(false);
        l.ctx.tenant_wall = false;
        assert_eq!(add(&l.ctx, &add_args("https://baden.k2.dev", "0123456789abcdef", Trust::Team)).unwrap_err().code, codes::TEAM_NEEDS_TENANT_WALL);
    }

    /// Review B10: an expired pending add whose wipe failed keeps its line
    /// (as revoked, holding its slot) until a later prune finishes the wipe.
    #[test]
    fn an_expired_add_whose_wipe_failed_keeps_its_slot() {
        use std::os::unix::fs::PermissionsExt;
        let mut t = setup(false);
        add(&t.ctx, &add_args("https://a.k2.dev", "0123456789abcdef", Trust::Own)).unwrap();
        // Something of slot 1 can't be removed (as a userdel that failed
        // would leave its user behind).
        let stuck = t.ctx.layout().tenant(1).root().join("stuck");
        std::fs::create_dir_all(&stuck).unwrap();
        std::fs::write(stuck.join("f"), "x").unwrap();
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o500)).unwrap();
        t.ctx.now += PENDING_SECS;
        let r = prune(&t.ctx).unwrap();
        assert_eq!(r["changed"], json!(["wipe_failed:1"]), "{r}");
        assert_eq!(r["wipe_failed"].as_array().unwrap().len(), 1, "retried at once in the same pass, still failing: {r}");
        let f = read_file(&t.ctx.config_dir).unwrap().unwrap();
        assert!(f.entry(1).is_some_and(|e| e.state == ShareState::Revoked), "the line stays, ended (revoked)");
        // The next server never gets that slot (nor its leftovers).
        let b = add(&t.ctx, &add_args("https://b.k2.dev", "1123456789abcdef", Trust::Own)).unwrap();
        assert_eq!(b["slot"].as_u64(), Some(2), "{b}");
        // Once the wipe can finish, the line goes and the slot is free.
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o700)).unwrap();
        let r = prune(&t.ctx).unwrap();
        assert_eq!(r["wiped"], json!([1]), "the next prune finishes the wipe (not 24 h later): {r}");
        assert!(read_file(&t.ctx.config_dir).unwrap().unwrap().entry(1).is_none());
        assert!(!t.ctx.layout().tenant(1).root().exists());
    }

    /// One pending add at a time (P3); after 10 minutes it's dropped by
    /// time and another may come; max_servers holds.
    #[test]
    fn one_pending_add_at_a_time() {
        let mut t = setup(false);
        add(&t.ctx, &add_args("https://a.k2.dev", "0123456789abcdef", Trust::Own)).unwrap();
        let e = add(&t.ctx, &add_args("https://b.k2.dev", "1123456789abcdef", Trust::Own)).unwrap_err();
        assert_eq!(e.code, codes::SERVER_ADD_PENDING);
        t.ctx.now += PENDING_SECS;
        let r = add(&t.ctx, &add_args("https://b.k2.dev", "1123456789abcdef", Trust::Own)).unwrap();
        assert_eq!(r["slot"].as_u64(), Some(1), "the expired pending line was pruned and its slot reused");
        let f = read_file(&t.ctx.config_dir).unwrap().unwrap();
        assert_eq!(f.servers.len(), 2);
        assert_eq!(read_enroll(&t.ctx.config_dir).unwrap().unwrap().fp_prefix, "1123456789abcdef");
        let e = add(&t.ctx, &add_args("https://a.k2.dev", "1123456789abcdef", Trust::Own));
        assert!(e.is_err(), "same key twice while it's pending");
        // Once active: the same key again is `server_exists`.
        let mut f = read_file(&t.ctx.config_dir).unwrap().unwrap();
        for e in f.servers.iter_mut() {
            e.state = ShareState::Active;
        }
        crate::util::atomic_write(&t.ctx.config_dir.join(share::SERVERS_FILE), toml::to_string(&f).unwrap().as_bytes(), 0o664).unwrap();
        let e = add(&t.ctx, &add_args("https://a.k2.dev", "1123456789abcdef", Trust::Own)).unwrap_err();
        assert_eq!(e.code, codes::SERVER_EXISTS, "same key twice");
        // Owner + two = the default three: a fourth is refused by count.
        let mut g = read_file(&t.ctx.config_dir).unwrap().unwrap();
        let mut third = g.servers[1].clone();
        third.slot = 2;
        third.label = "c.k2.dev".into();
        third.fp_prefix = "2123456789abcdef".into();
        g.servers.push(third);
        crate::util::atomic_write(&t.ctx.config_dir.join(share::SERVERS_FILE), toml::to_string(&g).unwrap().as_bytes(), 0o664).unwrap();
        let e = add(&t.ctx, &add_args("https://d.k2.dev", "3123456789abcdef", Trust::Own)).unwrap_err();
        assert_eq!(e.code, codes::MAX_SERVERS_REACHED);
    }

    fn confirm_pin(t: &T, slot: Slot, prefix: &str) {
        let k = SigningKey::generate().unwrap();
        let fp = format!("{prefix}{}", &k.fingerprint()[16..]);
        let pin = crate::identity::Pin {
            controller_fp: fp,
            controller_public_key_pem: k.spki_pem(),
            routes: vec!["https://x".into()],
            node_id: "n1".into(),
            name: "mini-1".into(),
            enrolled_at: t.ctx.now,
            sas: "1".into(),
            revoked: false,
            revoked_reason: None,
            labels: Default::default(),
            confirmed: true,
        };
        crate::identity::write_tenant_pin(&t.ctx.layout().tenant(slot).pin(), &pin, &Default::default()).unwrap();
    }

    /// Prune promotes a pending line once the node's pin is confirmed;
    /// revoke wipes after 24 h (or at once with --wipe); a reused slot
    /// starts fresh. (The uid a reused slot gets, never an old one, is
    /// root's tenant.prepare: tenant_wall_linux.)
    #[test]
    fn prune_promotes_and_revoke_wipes() {
        let mut t = setup(false);
        add(&t.ctx, &add_args("https://a.k2.dev", "0123456789abcdef", Trust::Own)).unwrap();
        prune(&t.ctx).unwrap();
        assert_eq!(read_file(&t.ctx.config_dir).unwrap().unwrap().entry(1).unwrap().state, ShareState::Pending, "no pin yet");
        confirm_pin(&t, 1, "0123456789abcdef");
        let r = prune(&t.ctx).unwrap();
        assert_eq!(r["changed"], json!(["promoted:1"]));
        let f = read_file(&t.ctx.config_dir).unwrap().unwrap();
        assert_eq!(f.entry(1).unwrap().state, ShareState::Active);
        assert!(f.shared(t.ctx.now));

        pause(&t.ctx, "a.k2.dev", true).unwrap();
        assert_eq!(read_file(&t.ctx.config_dir).unwrap().unwrap().entry(1).unwrap().state, ShareState::Paused);
        assert!(pause(&t.ctx, "a.k2.dev", true).is_err());
        pause(&t.ctx, "slot:1", false).unwrap();
        assert!(pause(&t.ctx, "slot:0", true).is_err(), "the owner isn't paused per server");

        // Revoke (no wipe): the line stays revoked until the 24 h wipe.
        let led = t.ctx.layout().tenant(1);
        led.ensure_state().unwrap();
        std::fs::write(led.state().join("ledger.sqlite"), b"x").unwrap();
        let r = revoke(&t.ctx, &RevokeArgs { server: "a.k2.dev".into(), now: true, wipe: false }).unwrap();
        assert_eq!(r["state"], "revoked");
        assert_eq!(read_stop(&t.ctx.config_dir).unwrap().slots, vec![1]);
        assert!(revoke(&t.ctx, &RevokeArgs { server: "slot:0".into(), now: false, wipe: false }).is_err());
        prune(&t.ctx).unwrap();
        assert!(read_file(&t.ctx.config_dir).unwrap().unwrap().entry(1).is_some(), "not due yet");
        t.ctx.now += WIPE_AFTER_SECS;
        let r = prune(&t.ctx).unwrap();
        assert_eq!(r["wiped"], json!([1]));
        assert!(read_file(&t.ctx.config_dir).unwrap().unwrap().entry(1).is_none());
        assert!(!led.state().exists() && !led.pin().exists() && !led.root().exists(), "the slot's files are gone");
        assert!(read_stop(&t.ctx.config_dir).unwrap().slots.is_empty());

        // The slot is reused, fresh.
        let r = add(&t.ctx, &add_args("https://b.k2.dev", "2123456789abcdef", Trust::Own)).unwrap();
        assert_eq!(r["slot"].as_u64(), Some(1));
        let b = read_file(&t.ctx.config_dir).unwrap().unwrap();
        assert_eq!((b.entry(1).unwrap().label.as_str(), b.entry(1).unwrap().state), ("b.k2.dev", ShareState::Pending));
    }

    /// Narrowing is Operate; any widening (or a later end date) is
    /// Privileged, so it needs the owner's approval.
    #[test]
    fn limits_classify_by_widening() {
        let t = setup(false);
        add(&t.ctx, &add_args("https://a.k2.dev", "0123456789abcdef", Trust::Own)).unwrap();
        let cd = &t.ctx.config_dir;
        let narrow = json!({"server": "a.k2.dev", "set": {"max_running": 1, "cpu_hours_day": 4}});
        assert_eq!(classify_limits(cd, &narrow, t.ctx.now).unwrap(), Class::Operate);
        let wide = json!({"server": "a.k2.dev", "set": {"max_running": 2}});
        assert_eq!(classify_limits(cd, &wide, t.ctx.now).unwrap(), Class::Privileged);
        let r = run(&t.ctx, share::VERB_LIMITS, &json!({"server": "a.k2.dev", "set": {"max_running": 1}, "for_secs": 7200})).unwrap();
        assert_eq!(r["expires_at"].as_i64(), Some(t.ctx.now + 7200));
        assert_eq!(classify_limits(cd, &json!({"server": "a.k2.dev", "set": {"max_running": 1}, "for_secs": 0}), t.ctx.now).unwrap(), Class::Privileged, "clearing the end date widens");
        assert_eq!(classify_limits(cd, &json!({"server": "a.k2.dev", "set": {"max_running": 1}, "for_secs": 3600}), t.ctx.now).unwrap(), Class::Operate, "an earlier end date narrows");
        assert!(run(&t.ctx, share::VERB_LIMITS, &json!({"server": "nope.k2.dev", "set": {"max_running": 1}})).is_err());
        assert!(run(&t.ctx, share::VERB_LIMITS, &json!({"server": "slot:0", "set": {"max_running": 1}, "for_secs": 3600})).is_err());
    }

    #[test]
    fn labels_are_plain() {
        assert_eq!(label_of("https://baden.k2.dev/"), "baden.k2.dev");
        assert_eq!(label_of("http://127.0.0.1:4100"), "127.0.0.1-4100");
        assert_eq!(label_of("https://u@h.example/x"), "h.example");
        assert!(share::valid_label(&label_of("https://a b")));
    }

    /// A broken servers.toml is an error, never solo (fail closed).
    #[test]
    fn a_broken_file_is_an_error() {
        let t = setup(false);
        std::fs::write(servers_path(&t.ctx.config_dir), "version = 1\nowner = \"x\"\n").unwrap();
        assert!(read_file(&t.ctx.config_dir).is_err());
        assert!(add(&t.ctx, &add_args("https://a.k2.dev", "0123456789abcdef", Trust::Own)).is_err());
    }

    /// The executor path: signed `servers.*` commands, with the owner's
    /// approval for an add and a widening (not for a narrowing, pause or
    /// revoke), the binding kept out of the audit, and `servers.prune` from
    /// the node. (macOS: the Seatbelt wall needs no root; the Linux wall
    /// needs root's jobs.toml and a job user.)
    #[cfg(target_os = "macos")]
    #[test]
    fn the_executor_runs_servers_verbs_with_approval_where_due() {
        use crate::admin::tests::Rig;
        let r = Rig::new("full");
        let layout = r.exec.paths.layout();
        std::fs::write(layout.policy(), "max_parallel = 2\n").unwrap();
        let pin = crate::identity::Pin {
            controller_fp: r.ctl.fingerprint(),
            controller_public_key_pem: r.ctl.spki_pem(),
            routes: vec!["https://rosson.k2.dev".into()],
            node_id: "n0".into(),
            name: "mini-1".into(),
            enrolled_at: 1,
            sas: "0".into(),
            revoked: false,
            revoked_reason: None,
            labels: Default::default(),
            confirmed: true,
        };
        crate::identity::write_pin(&layout.pin(), &pin).unwrap();
        let add = json!({"url": "https://z3thon.k2.dev", "code_binding": "SECRET-BINDING", "fp_prefix": "0123456789abcdef", "trust": "own"});

        // No wall (the node's status says none): refused, nothing written.
        let res = r.run(&r.approve(r.cmd("servers.add", add.clone())));
        assert_eq!(res.code.as_deref(), Some(codes::TENANT_NEEDS_JOB_WALL), "{res:?}");
        assert!(read_file(&layout.config).unwrap().is_none());
        let st = crate::status::Status { wall: "seatbelt".into(), ..Default::default() };
        crate::status::write(&layout.status(), &st).unwrap();

        // An add needs the owner's approval.
        let res = r.run(&r.sign(&r.cmd("servers.add", add.clone())));
        assert_eq!(res.code.as_deref(), Some(code::APPROVAL_REQUIRED), "{res:?}");
        let res = r.run(&r.approve(r.cmd("servers.add", add.clone())));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(res.result["slot"], 1);
        let f = read_file(&layout.config).unwrap().unwrap();
        assert_eq!((f.owner.as_str(), f.entry(1).unwrap().added_by.starts_with("owner-approval:")), (r.ctl.fingerprint().as_str(), true));
        let audit = std::fs::read_to_string(r.exec.paths.audit()).unwrap();
        assert!(!audit.contains("SECRET-BINDING"), "the binding never reaches the audit");
        assert_eq!(read_enroll(&layout.config).unwrap().unwrap().code_binding, "SECRET-BINDING");

        // Narrowing runs on the controller's word; widening needs approval.
        let narrow = json!({"server": "z3thon.k2.dev", "set": {"cpu_hours_day": 4}});
        assert_eq!(r.run(&r.sign(&r.cmd("servers.limits", narrow))).state, "done");
        let wide = json!({"server": "z3thon.k2.dev", "set": {"max_running": 3}});
        assert_eq!(r.run(&r.sign(&r.cmd("servers.limits", wide.clone()))).code.as_deref(), Some(code::APPROVAL_REQUIRED));
        assert_eq!(read_file(&layout.config).unwrap().unwrap().entry(1).unwrap().max_running, 1, "nothing widened");
        assert_eq!(r.run(&r.approve(r.cmd("servers.limits", wide))).state, "done");
        assert_eq!(read_file(&layout.config).unwrap().unwrap().entry(1).unwrap().max_running, 3);

        // The node's prune: nothing due yet.
        let pr = r.exec.handle_servers_op(crate::admin::exec::ServersOp::Prune);
        assert_eq!(pr.state, "done", "{pr:?}");
        // Revoke on the controller's word (operate), wiped at once.
        let res = r.run(&r.sign(&r.cmd("servers.revoke", json!({"server": "slot:1", "wipe": true}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(res.result["wiped"], json!([1]));
        assert!(read_file(&layout.config).unwrap().unwrap().entry(1).is_none());
        assert!(read_enroll(&layout.config).unwrap().is_none(), "the pending credential went with it");
        // A bad verb shape is refused before anything runs.
        let bad = r.run(&r.sign(&r.cmd("servers.pause", json!({"server": "slot:9"}))));
        assert_eq!(bad.code.as_deref(), Some(code::BAD_ARGS), "{bad:?}");
    }
}

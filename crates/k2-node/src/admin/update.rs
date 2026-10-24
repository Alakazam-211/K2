//! Signed self-update and rollback (prd-k2-node-admin-v1 §8, S5;
//! prd-compute-update-continuity-v1 §5.9, U1).
//!
//! The executor reads the offered build **into memory** (it arrives over
//! the local socket; there is no spool path a job could swap), verifies
//! the minisign signature with the root-helper keys compiled into the
//! running binary, the asset name, the binary format and the marker,
//! checks `version > running` and `protocol ≥ running`, and only then
//! writes `<bin>.new`: `fchmod` 0755 on the fd whatever the umask (the
//! units run with `UMask=0077`; 0.46.3 and older staged 0700 builds the
//! node user couldn't exec), fsync, and `fstat` must show 0755 and our own
//! uid. Preflight: the staged build's `version` runs as the node user
//! (supplementary groups dropped) and must name the staged version.
//! Swap: `<bin>` → `<bin>.prev` (hard link), `.new` → `<bin>` (rename).
//! Restart the node, the update mark written first (v2: the node's pid,
//! C3). Health gate: up to 120 s for `<home>/run/healthy` naming the new
//! version (and a controller handshake when the node is enrolled). No mark
//! → automatic rollback (`<bin>` → `<bin>.bad`, `.prev` → `<bin>`), the bad
//! version goes on a refuse list, restart. Manual rollback only to the
//! local `.prev` (never a download of an older build). Every step logs one
//! `admin: update step=…` line and lands in `update.json.steps` (Bug 3).
//! After a healthy gate the executor sets `reexec` and returns: `serve`
//! finishes the requests in flight and exits (never `process::exit`, C5).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use k2_node_proto::admin::{code, AdminRefusal, UpdateStep};
use k2_root_proto::node_build::{self, verify_node_build};
use k2_root_proto::version::Semver;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::{only_keys, str_arg, Checked, Executor};
use super::store::AuditEntry;

/// How long a new build has to say it's healthy.
pub const HEALTH_GATE_SECS: i64 = 120;

/// How long a shared node's update, rollback or k2-node restart waits for
/// running jobs before it goes ahead anyway (V55).
pub const DRAIN_MAX_SECS: i64 = k2_node_proto::admin::DRAIN_MAX_SECS;

/// `node-update.json` in [`mark_dir`]: written by the executor just before every
/// node restart it makes (update, rollback, `unit.restart`, `wall.enable`),
/// so the jobs that restart interrupts end `interrupted/node_update` (infra,
/// no node-health strike on the server; V55) instead of `node_shutdown` /
/// `node_restart`. v2 (0.46.4) names the node pid it is about (C3).
pub const UPDATE_MARK_FILE: &str = "node-update.json";
/// A mark older than this is stale (a restart that never came).
pub const UPDATE_MARK_SECS: i64 = 600;

/// The first k2-node whose executor writes mark v2 and step logs. U4's
/// `decide` keeps jobs only when the running and the incoming build are at
/// least this (prd-compute-update-continuity-v1 §5.5, `ADOPT_MIN`).
pub const ADOPT_MIN: &str = "0.46.4";

/// The macOS Developer ID team that signs K2 (release.sh).
pub const APPLE_TEAM_ID: &str = "36B8R93HXV";

/// `<state>/update.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRecord {
    /// `swapped` (gate running) | `healthy` | `rolled_back` | `failed`.
    pub phase: String,
    pub from: String,
    pub to: String,
    pub at: i64,
    #[serde(default)]
    pub deadline: i64,
    #[serde(default)]
    pub cmd_id: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Versions rolled back from: never installed again.
    #[serde(default)]
    pub refused: Vec<String>,
    /// This update's (or rollback's) steps, in order (0.46.4). Reaches the
    /// node through `Status.last_update` and the controller in
    /// `UpdateState.steps`.
    #[serde(default)]
    pub steps: Vec<UpdateStep>,
}

pub fn load(e: &Executor) -> UpdateRecord {
    std::fs::read(e.paths.update_state()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(e: &Executor, r: &UpdateRecord) -> Result<(), AdminRefusal> {
    let b = serde_json::to_vec_pretty(r).map_err(|x| AdminRefusal::new(code::FAILED, x.to_string()))?;
    crate::util::atomic_write(&e.paths.update_state(), &b, 0o644).map_err(|x| AdminRefusal::new(code::FAILED, x))
}

fn sib(bin: &Path, suffix: &str) -> PathBuf {
    let mut s = bin.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn is_regular(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())
}

fn fsync_dir(p: &Path) {
    if let Some(d) = p.parent() {
        if let Ok(f) = std::fs::File::open(d) {
            let _ = f.sync_all();
        }
    }
}

fn fail(m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(code::FAILED, m)
}

/// The version the installed `<bin>` says it is (its marker), if any.
pub fn installed_version(bin: &Path) -> Option<String> {
    let b = std::fs::read(bin).ok()?;
    let m = node_build::find_markers(&b).ok()?;
    (m.len() == 1).then(|| m[0].version.to_string())
}

/// The healthy mark the node writes (`<home>/run/healthy`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthyMark {
    pub version: String,
    pub at: i64,
    /// True after a controller accepted the attach (or when the node has
    /// no controller pin, so there is nothing to shake hands with).
    pub handshake: bool,
}

pub fn healthy_path(home: &Path) -> PathBuf {
    home.join("run").join("healthy")
}

/// Node side: write the mark (best effort).
pub fn mark_healthy(home: &Path, handshake: bool) {
    let m = HealthyMark { version: crate::PRODUCT_VERSION.to_string(), at: crate::util::now(), handshake };
    if let Ok(b) = serde_json::to_vec(&m) {
        let _ = crate::util::atomic_write(&healthy_path(home), &b, 0o644);
    }
}

/// The gate's check. The node writes the mark; root reads it FIFO-safe
/// and fails closed: anything but a small regular file is "not healthy",
/// so the gate times out into a rollback instead of hanging (round 4, 3).
fn mark_ok(e: &Executor, to: &str, since: i64) -> bool {
    let Some(m) = crate::util::read_regular(&healthy_path(&e.paths.home)).ok().and_then(|b| serde_json::from_str::<HealthyMark>(&b).ok()) else {
        return false;
    };
    m.version == to && m.at >= since && m.handshake
}

/// macOS: the staged build must carry K2's Developer ID signature.
fn verify_codesign(staged: &Path) -> Result<(), AdminRefusal> {
    let req = format!("=anchor apple generic and certificate leaf[subject.OU] = \"{APPLE_TEAM_ID}\"");
    let out = super::fdpass::spawn_output(std::process::Command::new("/usr/bin/codesign").args(["--verify", "--strict", "-R"]).arg(&req).arg(staged).env_clear())
        .map_err(|x| AdminRefusal::new(code::BAD_BUILD, format!("codesign: {x}")))?;
    if !out.status.success() {
        return Err(AdminRefusal::new(code::BAD_BUILD, "the build is not signed by K2's Developer ID"));
    }
    Ok(())
}

fn codesign_applies(e: &Executor) -> bool {
    e.platform.os() == "macos" && !e.test_keys
}

// ── step logs (Bug 3, §5.9) ──────────────────────────────────────────

/// One `admin: update step=…` line (journald / `/Library/Logs/k2-node-admin.log`).
pub fn log_step(step: &str, to: &str, from: &str, cmd: &str, ok: bool, detail: &str) {
    crate::nlog!("admin: update step={step} to={to} from={from} cmd={cmd} ok={ok} detail={detail}");
}

/// The steps of an update before its record exists (received → decide).
pub struct Steps {
    pub to: String,
    pub from: String,
    pub cmd: String,
    pub lines: Vec<UpdateStep>,
}

impl Steps {
    pub fn new(to: &str, cmd: &str) -> Self {
        Self { to: to.to_string(), from: crate::PRODUCT_VERSION.to_string(), cmd: cmd.to_string(), lines: Vec::new() }
    }

    fn push(&mut self, e: &Executor, step: &str, ok: bool, detail: String) {
        log_step(step, &self.to, &self.from, &self.cmd, ok, &detail);
        self.lines.push(UpdateStep { step: step.to_string(), ok, at: e.now(), detail: (!detail.is_empty()).then_some(detail) });
    }

    pub fn ok(&mut self, e: &Executor, step: &str, detail: impl Into<String>) {
        self.push(e, step, true, detail.into());
    }

    /// Log the refusal as a failed step and hand it back.
    pub fn failed(&mut self, e: &Executor, step: &str, r: AdminRefusal) -> AdminRefusal {
        self.push(e, step, false, format!("{}: {}", r.code, r.message));
        r
    }
}

/// A step after the record exists: log it, append it, save (best effort;
/// callers that must not go on without the record save themselves).
fn rec_step(e: &Executor, rec: &mut UpdateRecord, step: &str, ok: bool, detail: impl Into<String>) {
    let detail = detail.into();
    log_step(step, &rec.to, &rec.from, rec.cmd_id.as_deref().unwrap_or("-"), ok, &detail);
    rec.steps.push(UpdateStep { step: step.to_string(), ok, at: e.now(), detail: (!detail.is_empty()).then_some(detail) });
    if let Err(x) = save(e, rec) {
        crate::nlog!("admin: update.json: {}", x.message);
    }
}

// ── staging (Bug 1) ──────────────────────────────────────────────────

/// Write a verified build to `<bin>.new`. The file is created 0700 and
/// then given 0755 with `fchmod` on the open fd, so the result never
/// depends on the umask (the executor runs under `UMask=0077` / plist
/// `Umask 63`); fsync; `fstat` must then show a regular file, mode 0755,
/// owned by this process's uid (root in the executor), else `.new` is
/// removed and the stage refused.
pub fn stage_build(bin: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let new = sib(bin, ".new");
    if std::fs::symlink_metadata(&new).is_ok() {
        if !is_regular(&new) {
            return Err(".new is not a regular file".into());
        }
        std::fs::remove_file(&new).map_err(|x| format!("remove stale .new: {x}"))?;
    }
    let r = (|| -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&new)
            .map_err(|x| format!("create: {x}"))?;
        f.write_all(bytes).map_err(|x| format!("write: {x}"))?;
        // fchmod on the fd: the umask can't touch an explicit chmod.
        f.set_permissions(std::fs::Permissions::from_mode(0o755)).map_err(|x| format!("fchmod 0755: {x}"))?;
        f.sync_all().map_err(|x| format!("fsync: {x}"))?;
        let m = f.metadata().map_err(|x| format!("fstat: {x}"))?;
        let uid = unsafe { libc::geteuid() };
        if !m.file_type().is_file() || m.mode() & 0o7777 != 0o755 || m.uid() != uid {
            return Err(format!("staged build is mode {:o} uid {}, not 0755 uid {uid}", m.mode() & 0o7777, m.uid()));
        }
        Ok(())
    })();
    if let Err(x) = r {
        let _ = std::fs::remove_file(&new);
        return Err(format!("stage: {x}"));
    }
    fsync_dir(&new);
    Ok(new)
}

/// A binary a rollback puts back must be 0755 too (fchmod on the fd).
fn ensure_exec_mode(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(p).map_err(|x| format!("open {}: {x}", p.display()))?;
    let m = f.metadata().map_err(|x| format!("fstat {}: {x}", p.display()))?;
    if !m.file_type().is_file() {
        return Err(format!("{} is not a regular file", p.display()));
    }
    if m.mode() & 0o7777 != 0o755 {
        f.set_permissions(std::fs::Permissions::from_mode(0o755)).map_err(|x| format!("fchmod {}: {x}", p.display()))?;
    }
    Ok(())
}

/// The staged build's `version` must name the version that was verified
/// (`k2-node <v> (protocol <p>)`).
fn preflight_says(out: &str, want: &str) -> bool {
    let mut w = out.split_whitespace();
    w.next() == Some("k2-node") && w.next() == Some(want)
}

/// `node.update` (operate; the controller signature + the build
/// signature + the version rule are the gate).
pub fn apply(e: &Executor, c: &Checked, sig: &str, bytes: &[u8]) -> Result<Value, AdminRefusal> {
    let args = &c.opened.cmd.args;
    only_keys(args, &["version", "sha256", "trigger", "now"])?;
    let now_flag = now_arg(args)?;
    let want_v = str_arg(args, "version")?;
    let want_sha = str_arg(args, "sha256")?;
    let trigger = args.get("trigger").and_then(|v| v.as_str()).unwrap_or("explicit");
    if trigger != "auto" && trigger != "explicit" {
        return Err(AdminRefusal::new(code::BAD_ARGS, "trigger must be auto or explicit"));
    }
    let cmd_id = c.opened.cmd.cmd_id.clone();
    let mut st = Steps::new(&want_v, &cmd_id);
    st.ok(e, "received", format!("trigger={trigger} bytes={} now={now_flag}", bytes.len()));
    if trigger == "auto" && !c.cfg.auto_update {
        return Err(st.failed(e, "received", AdminRefusal::new(code::AUTO_UPDATE_OFF, "this machine's owner turned automatic updates off")));
    }
    let sha = k2_node_proto::crypto::sha256_hex(bytes);
    if !k2_node_proto::crypto::ct_eq(sha.as_bytes(), want_sha.as_bytes()) {
        return Err(st.failed(e, "verify_sig", AdminRefusal::new(code::BAD_BUILD, "build bytes differ from the build the controller signed for")));
    }
    let keys = k2_root_proto::keys::trusted_keys(e.test_keys);
    let built = verify_node_build(bytes, sig, &keys, e.platform.os(), e.platform.arch()).map_err(|m| st.failed(e, "verify_sig", AdminRefusal::new(code::BAD_BUILD, m)))?;
    if built.version.to_string() != want_v {
        return Err(st.failed(e, "verify_sig", AdminRefusal::new(code::BAD_BUILD, "signed version differs from the command's")));
    }
    let running = Semver::parse(crate::PRODUCT_VERSION).ok_or_else(|| st.failed(e, "verify_sig", fail("running version unreadable")))?;
    if built.version == running {
        return Err(st.failed(e, "verify_sig", AdminRefusal::new(code::SAME_VERSION, "already running that version")));
    }
    if built.version < running || built.protocol < k2_node_proto::PROTOCOL {
        return Err(st.failed(e, "verify_sig", AdminRefusal::new(code::DOWNGRADE, "older than the running k2-node (roll back instead)")));
    }
    let rec = load(e);
    if rec.refused.iter().any(|v| *v == built.version.to_string()) {
        return Err(st.failed(e, "verify_sig", AdminRefusal::new(code::ROLLED_BACK_VERSION, "that version was rolled back on this node")));
    }
    st.ok(e, "verify_sig", format!("key={} protocol={}", built.key_id, built.protocol));
    let bin = e.paths.binary.clone();
    if !is_regular(&bin) {
        return Err(st.failed(e, "stage", fail("installed k2-node is not a regular file")));
    }
    // Stage from memory: 0755 whatever the umask.
    let new = stage_build(&bin, bytes).map_err(|x| st.failed(e, "stage", fail(x)))?;
    st.ok(e, "stage", format!("{} 0755", new.display()));
    if codesign_applies(e) {
        if let Err(r) = verify_codesign(&new) {
            let _ = std::fs::remove_file(&new);
            return Err(st.failed(e, "verify_codesign", r));
        }
        st.ok(e, "verify_codesign", APPLE_TEAM_ID);
    }
    // Preflight: the node user runs it before it goes in (203/EXEC never
    // reaches the health gate again).
    let node_user = e.node_user();
    match e.platform.preflight(&new, &node_user) {
        Ok(out) if preflight_says(&out, &want_v) => st.ok(e, "preflight", format!("as {node_user}: {out}")),
        Ok(out) => {
            let _ = std::fs::remove_file(&new);
            return Err(st.failed(e, "preflight", AdminRefusal::new(code::BAD_BUILD, format!("the staged build says {out:?}, not k2-node {want_v}"))));
        }
        Err(x) => {
            let _ = std::fs::remove_file(&new);
            return Err(st.failed(e, "preflight", AdminRefusal::new(code::BAD_BUILD, format!("the staged build doesn't run as {node_user}: {x}"))));
        }
    }
    let d = decide(e, now_flag, false);
    st.ok(e, "decide", format!("{} running={}", d.0.as_str(), d.1));
    let staged = Staged { to: built.version.to_string(), key: built.key_id.to_string(), cmd_id: cmd_id.clone(), sha, steps: st };
    // A shared node lets running jobs finish first (V55); `.new` waits.
    run_decided(e, d, DrainOpts::default(), "node.update", &cmd_id, move |ex| swap_and_restart(ex, staged))
}

/// A verified build waiting in `<bin>.new`.
struct Staged {
    to: String,
    key: String,
    cmd_id: String,
    /// The verified build's sha256: `.new` must still be it at the swap.
    sha: String,
    steps: Steps,
}

/// Swap `.new` in, restart the node, start the health gate.
fn swap_and_restart(e: &Executor, mut s: Staged) -> Result<Value, AdminRefusal> {
    let bin = e.paths.binary.clone();
    let new = sib(&bin, ".new");
    if !is_regular(&new) {
        return Err(s.steps.failed(e, "swap", fail("the staged build is gone")));
    }
    let mut rec = load(e);
    // A drained swap runs later: the version may have been rolled back
    // meanwhile, or `.new` replaced (review A L7).
    if rec.refused.iter().any(|v| *v == s.to) {
        let _ = std::fs::remove_file(&new);
        return Err(s.steps.failed(e, "swap", AdminRefusal::new(code::ROLLED_BACK_VERSION, "that version was rolled back on this node")));
    }
    let staged_sha = std::fs::read(&new).map(|b| k2_node_proto::crypto::sha256_hex(&b)).map_err(|x| s.steps.failed(e, "swap", fail(format!("read the staged build: {x}"))))?;
    if !k2_node_proto::crypto::ct_eq(staged_sha.as_bytes(), s.sha.as_bytes()) {
        let _ = std::fs::remove_file(&new);
        return Err(s.steps.failed(e, "swap", AdminRefusal::new(code::BAD_BUILD, "the staged build changed since it was verified")));
    }
    // Record first (B-L2): `swapping` is on disk before the rename, so an
    // executor that dies between the two never leaves a build that hasn't
    // passed the gate in place unrecorded; `resume_pending` finishes it.
    let from = crate::PRODUCT_VERSION.to_string();
    rec.phase = "swapping".into();
    rec.from = from.clone();
    rec.to = s.to.clone();
    rec.at = e.now();
    rec.deadline = 0;
    rec.cmd_id = Some(s.cmd_id.clone());
    rec.reason = None;
    rec.steps = std::mem::take(&mut s.steps.lines);
    save(e, &rec)?;
    // Swap.
    let prev = sib(&bin, ".prev");
    let swap = (|| -> std::io::Result<()> {
        if std::fs::symlink_metadata(&prev).is_ok() {
            std::fs::remove_file(&prev)?;
        }
        std::fs::hard_link(&bin, &prev)?;
        std::fs::rename(&new, &bin)?;
        Ok(())
    })();
    if let Err(x) = swap {
        let _ = std::fs::remove_file(&new);
        rec.phase = "failed".into();
        rec.reason = Some(format!("swap: {x}"));
        rec_step(e, &mut rec, "swap", false, x.to_string());
        return Err(fail(format!("swap: {x}")));
    }
    fsync_dir(&bin);
    let now = e.now();
    rec.phase = "swapped".into();
    rec.at = now;
    rec.deadline = now + HEALTH_GATE_SECS;
    rec_step(e, &mut rec, "swap", true, format!(".prev = {from}"));
    // A crash from here on resumes the gate.
    save(e, &rec)?;
    restart_into_gate(e, &mut rec)?;
    Ok(json!({"phase": "swapped", "from": from, "to": rec.to, "key": s.key, "health_gate_secs": HEALTH_GATE_SECS}))
}

/// After the swap (recorded `swapped`): restart the node with the mark and
/// start the health gate; a failed restart rolls back.
fn restart_into_gate(e: &Executor, rec: &mut UpdateRecord) -> Result<(), AdminRefusal> {
    let _ = std::fs::remove_file(healthy_path(&e.paths.home));
    let cmd = rec.cmd_id.clone().unwrap_or_else(|| "-".into());
    let named = mark_restart(e, "node.update", &cmd);
    rec_step(e, rec, "mark", true, mark_detail(named));
    if let Err(x) = e.platform.restart_node() {
        rec_step(e, rec, "restart", false, x.clone());
        let r = rollback_inner(e, &format!("restart failed: {x}"), true);
        return Err(fail(format!("restart after swap failed ({x}); rolled back: {}", r.map(|_| "yes".to_string()).unwrap_or_else(|z| z.message))));
    }
    rec_step(e, rec, "restart", true, "");
    rec_step(e, rec, "health_wait", true, format!("up to {HEALTH_GATE_SECS} s"));
    start_gate(e);
    Ok(())
}

// ── the drain on shared nodes (V55) ──────────────────────────────────

/// The optional `now` arg (skip the drain).
pub fn now_arg(args: &Value) -> Result<bool, AdminRefusal> {
    k2_node_proto::admin::now_arg(args)
}

/// Two or more active servers (§2): restarts drain first.
pub fn shared_node(e: &Executor) -> bool {
    super::servers::read_file(&e.paths.config_dir).ok().flatten().is_some_and(|f| f.shared(e.now()))
}

/// Running jobs, every server's: the larger of what root counts itself
/// (live job units, Linux with `jobs.toml`; review A L7: a node can't
/// hide a unit) and what the node's status file says. The node's word can
/// only make a drain wait longer (it is capped), never shorter; and a node
/// that isn't walled yet (`jobs.toml` just written, or a fallback to no
/// wall) runs its jobs as its own children, which no unit count sees
/// (security review A-H1 / B-M1).
pub fn running_jobs(e: &Executor) -> usize {
    let mut units = 0;
    // Root counts its units unless jobs.toml is certainly absent: a read
    // error never zeroes the count (round 4, item 5).
    if cfg!(target_os = "linux") && !matches!(super::jobs::read_jobs_file(&e.paths.config_dir), Ok(None)) {
        units = e.platform.running_job_units().unwrap_or(0);
    }
    // No status file: the node isn't running anything it reported. Any
    // other failure (a FIFO, a symlink, garbage) fails closed: busy.
    let status = e.paths.layout().status();
    let reported = match std::fs::symlink_metadata(&status) {
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => 0,
        _ => running_in_status(&status).unwrap_or_else(|x| {
            crate::nlog!("admin: {x}; counting the node as busy");
            1
        }),
    };
    units.max(reported)
}

/// The node's running count from its status file, reading only that
/// field: a new required field in `Status` never makes the node look busy
/// forever (round 5, item 4).
fn running_in_status(p: &Path) -> Result<usize, String> {
    #[derive(Deserialize)]
    struct RunningOnly {
        #[serde(default)]
        running: Vec<String>,
    }
    let s = crate::util::read_regular_max(p, 4 << 20).map_err(|x| format!("read {}: {x}", p.display()))?;
    serde_json::from_str::<RunningOnly>(&s).map(|r| r.running.len()).map_err(|x| format!("{}: {x}", p.display()))
}

/// How a node restart goes (§5.5). U4 adds `Adopt` (keep running jobs and
/// restart at once) beside these; until then running jobs either wait
/// (`Drain`) or are stopped by the restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// A solo node, or nothing running: restart now (today's path).
    Restart,
    /// `now: true` on a node that would drain.
    Now,
    /// Running jobs finish first (no new starts), up to the drain limit.
    Drain,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Restart => "restart",
            Decision::Now => "now",
            Decision::Drain => "drain",
        }
    }
}

/// The decision and the running count it saw. `always_drain`: wait for
/// running jobs on a solo node too (`wall.enable`).
pub fn decide(e: &Executor, now: bool, always_drain: bool) -> (Decision, usize) {
    if !always_drain && !shared_node(e) {
        return (Decision::Restart, 0);
    }
    let running = running_jobs(e);
    if running == 0 {
        (Decision::Restart, 0)
    } else if now {
        (Decision::Now, running)
    } else {
        (Decision::Drain, running)
    }
}

/// The mark's format: 2 = names the node by pid and start time (C3, B-M4).
/// A mark without `v` comes from a 0.46.3 executor.
pub const MARK_V: u32 = 2;

/// The update mark v2 (C3). The executor writes it before every node
/// restart it makes, naming the node's main process (pid + start time)
/// read just before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMark {
    #[serde(default)]
    pub v: u32,
    pub what: String,
    pub at: i64,
    /// The stopped node may leave its jobs running for the next node to
    /// adopt (U3/U4). Always false until U4's `decide` sets it.
    #[serde(default)]
    pub keep: bool,
    #[serde(default)]
    pub cmd_id: Option<String>,
    /// The node process this restart stops (`None` when the service manager
    /// didn't say: then the mark names nobody).
    #[serde(default)]
    pub node_pid: Option<u32>,
    /// That process's start time ([`crate::util::process_start_time`]), so a
    /// reused pid is never the same process (B-M4).
    #[serde(default)]
    pub node_start: Option<u64>,
    /// The restart this mark belonged to is over (healthy gate, rollback,
    /// failed restart): it keeps nothing any more.
    #[serde(default)]
    pub ended: bool,
}

impl UpdateMark {
    pub fn fresh(&self, now: i64) -> bool {
        now >= self.at && now - self.at < UPDATE_MARK_SECS
    }

    /// Was the process (`pid`, started at `start`) stopped by this K2-made
    /// restart? A v2 mark names exactly one process (no pid = nobody,
    /// A-L3); a 0.46.3 mark goes by time alone, as it did there.
    pub fn names(&self, pid: u32, start: Option<u64>, now: i64) -> bool {
        if !self.fresh(now) {
            return false;
        }
        if self.v < MARK_V {
            return true;
        }
        self.node_pid == Some(pid) && self.node_start.is_none_or(|s| Some(s) == start)
    }

    /// U3's detach test (§5.4): only the process the executor restarted
    /// (pid and start time both known and equal), only while the mark is
    /// fresh and its restart not over, and only with `keep`. An owner's
    /// `systemctl stop` later is another process (Q2).
    pub fn keeps_jobs_of(&self, pid: u32, start: Option<u64>, now: i64) -> bool {
        self.v >= MARK_V
            && self.keep
            && !self.ended
            && self.fresh(now)
            && self.node_pid == Some(pid)
            && self.node_start.is_some()
            && self.node_start == start
    }
}

/// Where the mark lives: a directory only root can write and the node can
/// read (`/run/k2-node-admin`, `/var/run/k2-node-admin` on macOS), never the
/// group-writable config dir, where `k2nodectl` members could stash a
/// genuine mark and put it back later, or plant a FIFO (A-N1, A-N3 / B-R1).
/// A dev or test node with its own config dir keeps it in `<config>/admin-run`.
pub fn mark_dir(config_dir: &Path) -> PathBuf {
    if config_dir == Path::new(crate::paths::DEFAULT_CONFIG_DIR) {
        PathBuf::from(if cfg!(target_os = "macos") { "/var/run/k2-node-admin" } else { "/run/k2-node-admin" })
    } else {
        config_dir.join("admin-run")
    }
}

pub fn update_mark_path(config_dir: &Path) -> PathBuf {
    mark_dir(config_dir).join(UPDATE_MARK_FILE)
}

/// Root side: make the mark's directory (0755) when it's missing. Only
/// creates it; every check and write goes through the directory fd
/// ([`write_owned_in`]). The mode is set with `fchmod` on a directory fd
/// opened `O_NOFOLLOW`, never `chmod` on the path: on macOS `/var/run` is
/// writable by the `daemon` group, which could swap the new directory for
/// a symlink between the `mkdir` and a path `chmod` (round-5 review, L1).
pub(crate) fn ensure_mark_dir(config_dir: &Path) -> Result<(), String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let d = mark_dir(config_dir);
    if std::fs::symlink_metadata(&d).is_err() {
        std::fs::create_dir(&d).map_err(|x| format!("create {}: {x}", d.display()))?;
        let dc = std::ffi::CString::new(d.as_os_str().as_encoded_bytes()).map_err(|x| x.to_string())?;
        // SAFETY: valid C path; the fd is owned right below.
        let dfd = unsafe { libc::open(dc.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if dfd < 0 {
            return Err(format!("open {}: {}", d.display(), std::io::Error::last_os_error()));
        }
        // SAFETY: a fresh fd we own.
        let dfd = unsafe { OwnedFd::from_raw_fd(dfd) };
        // SAFETY: fchmod on our own directory fd.
        if unsafe { libc::fchmod(dfd.as_raw_fd(), 0o755) } != 0 {
            return Err(format!("fchmod {}: {}", d.display(), std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// Write `name` in `dir` the way [`read_owned_in`] reads it: open the
/// directory once (`O_DIRECTORY|O_NOFOLLOW`), check on that fd that
/// `owner` owns it and nobody else may write it, then create a temp file
/// with `openat` (`O_CREAT|O_EXCL|O_NOFOLLOW`), write, `fchmod`, fsync,
/// `renameat` it over `name` in the same directory fd, fsync the
/// directory. Nothing can swap the directory between the check and the
/// write (round 5, item 2).
pub(crate) fn write_owned_in(dir: &Path, name: &str, bytes: &[u8], mode: u32, owner: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::PermissionsExt;
    let cs = |s: &[u8]| std::ffi::CString::new(s).map_err(|x| x.to_string());
    let dc = cs(dir.as_os_str().as_encoded_bytes())?;
    // SAFETY: valid C path; the fd is owned right below.
    let dfd = unsafe { libc::open(dc.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if dfd < 0 {
        return Err(format!("open {}: {}", dir.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh fd we own.
    let dfd = unsafe { OwnedFd::from_raw_fd(dfd) };
    // SAFETY: zeroed out-struct; fstat on our fd.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(dfd.as_raw_fd(), &mut st) } != 0 {
        return Err(format!("fstat {}: {}", dir.display(), std::io::Error::last_os_error()));
    }
    if st.st_uid != owner || (st.st_mode as u32) & 0o022 != 0 {
        return Err(format!("{} isn't a directory only uid {owner} writes", dir.display()));
    }
    let id = k2_node_proto::crypto::random_id().map_err(|x| x.to_string())?;
    let tmp = format!(".{name}.{id}");
    let tc = cs(tmp.as_bytes())?;
    let nc = cs(name.as_bytes())?;
    // SAFETY: openat relative to the checked directory fd.
    let fd = unsafe { libc::openat(dfd.as_raw_fd(), tc.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if fd < 0 {
        return Err(format!("create {}/{tmp}: {}", dir.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh fd we own.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let written = (|| -> std::io::Result<()> {
        f.write_all(bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()
    })();
    let unlink_tmp = || {
        // SAFETY: unlinkat our own temp name in our directory fd.
        unsafe { libc::unlinkat(dfd.as_raw_fd(), tc.as_ptr(), 0) };
    };
    if let Err(x) = written {
        unlink_tmp();
        return Err(format!("write {}/{tmp}: {x}", dir.display()));
    }
    // SAFETY: rename within the same checked directory fd.
    if unsafe { libc::renameat(dfd.as_raw_fd(), tc.as_ptr(), dfd.as_raw_fd(), nc.as_ptr()) } != 0 {
        let x = std::io::Error::last_os_error();
        unlink_tmp();
        return Err(format!("rename into {}/{name}: {x}", dir.display()));
    }
    // SAFETY: fsync on the directory fd.
    unsafe { libc::fsync(dfd.as_raw_fd()) };
    Ok(())
}

/// Who must own the mark: root for the installed node (`/etc/k2-node` is
/// group-writable for `k2nodectl`, so its members, the human and their
/// agents, could rename their own file in; A-M1 / B-M3). A dev or test
/// node with its own config dir runs everything as one user.
pub fn mark_owner(config_dir: &Path) -> u32 {
    if config_dir == Path::new(crate::paths::DEFAULT_CONFIG_DIR) {
        0
    } else {
        unsafe { libc::geteuid() }
    }
}

/// Open `name` inside `dir` the safe way, as `owner` wrote it: the
/// directory once (`O_DIRECTORY|O_NOFOLLOW`; with `check_dir`, `fstat`
/// must show `owner` and no group/other write), then the file with
/// `openat` (`O_NOFOLLOW|O_NONBLOCK`): a regular file with one link, owned
/// by `owner`, writable by nobody else, at most 64 KiB. Nothing between the
/// check and the read can swap the directory (macOS `/var/run` is group
/// `daemon`-writable; review round 4, item 9).
pub(crate) fn read_owned_in(dir: &Path, name: &str, owner: u32, check_dir: bool) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let dc = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    let nc = std::ffi::CString::new(name).ok()?;
    // SAFETY: valid C path; the fd is owned right below.
    let dfd = unsafe { libc::open(dc.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if dfd < 0 {
        return None;
    }
    // SAFETY: a fresh fd we own.
    let dfd = unsafe { OwnedFd::from_raw_fd(dfd) };
    // SAFETY: zeroed out-struct; fstat on our fd.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(dfd.as_raw_fd(), &mut st) } != 0 {
        return None;
    }
    if check_dir && (st.st_uid != owner || (st.st_mode as u32) & 0o022 != 0) {
        crate::nlog!("ignoring {}: not a directory only uid {owner} writes", dir.display());
        return None;
    }
    // SAFETY: openat relative to the directory fd we checked.
    let fd = unsafe { libc::openat(dfd.as_raw_fd(), nc.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: a fresh fd we own.
    let f = unsafe { std::fs::File::from_raw_fd(fd) };
    let m = f.metadata().ok()?;
    use std::os::unix::fs::MetadataExt;
    // nlink 0: root just renamed a new mark over the one we opened (still
    // root's, still whole); 2+: a hard link someone else made.
    if !m.file_type().is_file() || m.nlink() > 1 || m.uid() != owner || m.mode() & 0o022 != 0 || m.len() > crate::util::READ_REGULAR_MAX {
        crate::nlog!("ignoring {}/{name}: not uid {owner}'s own small regular file", dir.display());
        return None;
    }
    let mut b = Vec::new();
    f.take(crate::util::READ_REGULAR_MAX).read_to_end(&mut b).ok()?;
    Some(b)
}

/// Read the mark only when [`mark_owner`] wrote it, in a directory only it
/// can write ([`read_owned_in`]). `O_NONBLOCK`: a FIFO never wedges the
/// reader (the node at an owner's stop, the executor under its lock; A-N1).
///
/// TODO(0.46.5): drop the legacy fallback. During the 0.46.3 → 0.46.4
/// rollout only, a root-owned v1 mark in the config dir (where 0.46.3's
/// executor wrote it) still counts, so jobs a 0.46.3 executor's restart
/// stopped close `node_update`, not `node_shutdown` (round 4, item 7).
pub fn read_update_mark(config_dir: &Path) -> Option<UpdateMark> {
    let owner = mark_owner(config_dir);
    if let Some(b) = read_owned_in(&mark_dir(config_dir), UPDATE_MARK_FILE, owner, true) {
        return serde_json::from_slice::<UpdateMark>(&b).ok();
    }
    let b = read_owned_in(config_dir, UPDATE_MARK_FILE, owner, false)?;
    serde_json::from_slice::<UpdateMark>(&b).ok().filter(|m| m.v < MARK_V)
}

/// On executor start: remove a mark left in the config dir by 0.46.3
/// (`unlinkat` on the directory fd; never follows a link). TODO(0.46.5): drop.
pub fn remove_legacy_mark(config_dir: &Path) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let (Ok(dc), Ok(nc)) = (std::ffi::CString::new(config_dir.as_os_str().as_encoded_bytes()), std::ffi::CString::new(UPDATE_MARK_FILE)) else { return };
    // SAFETY: valid C path; the fd is owned right below.
    let dfd = unsafe { libc::open(dc.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if dfd < 0 {
        return;
    }
    // SAFETY: a fresh fd we own.
    let dfd = unsafe { OwnedFd::from_raw_fd(dfd) };
    // SAFETY: unlinkat a plain name in our directory fd (a symlink itself
    // is removed, never followed).
    if unsafe { libc::unlinkat(dfd.as_raw_fd(), nc.as_ptr(), 0) } == 0 {
        crate::nlog!("admin: removed the 0.46.3 restart mark from {}", config_dir.display());
    }
}

fn write_update_mark(e: &Executor, m: &UpdateMark) {
    if let Err(x) = ensure_mark_dir(&e.paths.config_dir) {
        crate::nlog!("admin: update mark: {x}");
        return;
    }
    if let Ok(b) = serde_json::to_vec(m) {
        let cfg = &e.paths.config_dir;
        if let Err(x) = write_owned_in(&mark_dir(cfg), UPDATE_MARK_FILE, &b, 0o644, mark_owner(cfg)) {
            crate::nlog!("admin: update mark: {x}");
        }
    }
}

/// Write the mark for a restart about to happen; returns the process it names.
fn mark_restart(e: &Executor, what: &str, cmd_id: &str) -> (Option<u32>, Option<u64>) {
    let node_pid = e.platform.node_pid();
    let node_start = node_pid.and_then(crate::util::process_start_time);
    if node_pid.is_none() {
        crate::nlog!("admin: the node's main pid is unknown; the restart mark names no process");
    }
    write_update_mark(
        e,
        &UpdateMark { v: MARK_V, what: what.to_string(), at: e.now(), keep: false, cmd_id: Some(cmd_id.to_string()), node_pid, node_start, ended: false },
    );
    (node_pid, node_start)
}

fn mark_detail((pid, start): (Option<u32>, Option<u64>)) -> String {
    let s = |v: Option<String>| v.unwrap_or_else(|| "unknown".into());
    format!("node_pid={} node_start={} keep=false", s(pid.map(|p| p.to_string())), s(start.map(|p| p.to_string())))
}

/// A restart is over (a healthy gate, a rollback's restart, a failed
/// restart): the mark keeps nothing (`ended`). It isn't deleted: the
/// restarted node's tenants reconcile after the attach and still read it
/// for the `node_update` reason until it goes stale.
fn retire_update_mark(e: &Executor) {
    let m = match read_update_mark(&e.paths.config_dir) {
        // An old-format mark names every process while fresh: its retired
        // form is a v2 mark naming none, same time (round 5, item 3).
        Some(m) if m.v < MARK_V => UpdateMark { v: MARK_V, what: m.what, at: m.at, keep: false, cmd_id: m.cmd_id, node_pid: None, node_start: None, ended: true },
        Some(mut m) => {
            m.keep = false;
            m.ended = true;
            m
        }
        // Unreadable or gone: still leave an ended mark, which names no
        // process and is stale (B-R1: retirement never depends on a read).
        None => UpdateMark { v: MARK_V, what: "retired".into(), at: 0, keep: false, cmd_id: None, node_pid: None, node_start: None, ended: true },
    };
    write_update_mark(e, &m);
}

/// Restart the node service the K2 way: the mark first (every K2-made
/// restart, solo nodes too, C3), then `restart`. A failed restart retires
/// the mark at once: the process it names is still running (B-M4).
pub fn restart_marked(e: &Executor, what: &str, cmd_id: &str, restart: impl FnOnce(&Executor) -> Result<(), String>) -> Result<Option<u32>, String> {
    let named = mark_restart(e, what, cmd_id);
    crate::nlog!("admin: restart k2-node what={what} cmd={cmd_id} {}", mark_detail(named));
    if let Err(x) = restart(e) {
        retire_update_mark(e);
        return Err(x);
    }
    Ok(named.0)
}

/// Node side: was this process (`pid`, started at `start`) stopped for a
/// restart the executor made in the last few minutes? Decides
/// `node_update` vs `node_shutdown` for the jobs it stops.
pub fn update_mark_for(config_dir: &Path, pid: u32, start: Option<u64>, now: i64) -> Option<UpdateMark> {
    read_update_mark(config_dir).filter(|m| m.names(pid, start, now))
}

/// Node side at start: did the executor restart a node in the last few
/// minutes (whose rows this node now closes)? By time: the mark names the
/// node that was stopped, never this one.
pub fn recent_update_mark(config_dir: &Path, now: i64) -> bool {
    read_update_mark(config_dir).is_some_and(|m| m.fresh(now))
}

/// See [`Executor::drain_settle`](super::exec::Executor::drain_settle).
pub const DRAIN_SETTLE_SECS: u64 = 12;

/// How a drain behaves at its limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainOpts {
    /// Jobs still running when `drain_max` passes: give up (put control
    /// back, never act) instead of acting anyway. `wall.enable` never
    /// kills a job (A-H1 / B-M1); updates and owner restarts still go ahead.
    pub give_up_at_limit: bool,
    /// Once the count reaches 0, keep control at `draining` for
    /// `drain_settle` and count again before acting (round 4, item 1).
    pub settle: bool,
    /// Don't wait for running jobs: once control says `draining`, restart
    /// k2-node the marked way, so its running jobs stop as
    /// `interrupted/node_update` (infra, no strike), and the restarted node
    /// starts nothing; then count and settle as usual (`servers.owner
    /// --now`, which must act with nothing running).
    pub stop_running: bool,
}

/// `<state>/drain.json`: a drain waiting to act, so an executor that exits
/// (re-exec after an update) or crashes mid-drain never leaves the owner's
/// control at `draining` (B-M2): the next executor puts it back.
pub const DRAIN_FILE: &str = "drain.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainRecord {
    pub what: String,
    pub cmd_id: String,
    /// The `by` the drain wrote on control.toml (only that is undone).
    pub by: String,
    pub prior_control: k2_node_proto::frames::Control,
    pub prior_by: Option<String>,
    pub prior_at: Option<i64>,
    pub at: i64,
}

fn drain_path(e: &Executor) -> PathBuf {
    e.paths.state_dir.join(DRAIN_FILE)
}

/// Put the owner's control back as it was before a drain, unless someone
/// changed it since (it no longer says `draining` by this drain).
fn restore_control(e: &Executor, d: &DrainRecord) {
    let control_path = e.paths.layout().control();
    if let Ok(c) = crate::config::read_control(&control_path) {
        if c.control == k2_node_proto::frames::Control::Draining && c.by.as_deref() == Some(d.by.as_str()) {
            let back_by = d.prior_by.clone().unwrap_or_else(|| "k2-node".into());
            if let Err(x) = crate::config::write_control(&control_path, d.prior_control, &back_by, d.prior_at.unwrap_or_else(|| e.now())) {
                crate::nlog!("admin: can't restore control after the drain: {x}");
            }
        }
    }
}

/// On executor start: a drain the last executor didn't finish (it exited
/// to re-exec, or crashed) never acts now; control goes back to the owner's.
pub fn recover_drain(e: &Executor) {
    let p = drain_path(e);
    let Some(d) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<DrainRecord>(&b).ok()) else {
        let _ = std::fs::remove_file(&p);
        return;
    };
    restore_control(e, &d);
    let _ = std::fs::remove_file(&p);
    crate::nlog!("admin: drain for {} (cmd {}) ended with the last executor; control is back to {:?}", d.what, d.cmd_id, d.prior_control);
    let _ = e.audit().append(
        AuditEntry { event: "drain".into(), cmd_id: Some(d.cmd_id.clone()), code: Some("drain_abandoned".into()), detail: json!({"what": d.what, "phase": "abandoned"}), ..Default::default() },
        e.now(),
    );
}

/// Run `act` (which restarts the node) now on a solo node, with `now`, or
/// when nothing runs. On a shared node with running jobs: set the node
/// draining (no new starts), answer at once, and run `act` in the
/// background once every running job ended or `drain_max` passed; then
/// put the machine owner's control back as it was.
pub fn drain_then<F>(e: &Executor, now: bool, what: &str, cmd_id: &str, act: F) -> Result<Value, AdminRefusal>
where
    F: FnOnce(&Executor) -> Result<Value, AdminRefusal> + Send + 'static,
{
    run_decided(e, decide(e, now, false), DrainOpts::default(), what, cmd_id, act)
}

/// Act on a [`decide`] result.
pub fn run_decided<F>(e: &Executor, (d, running): (Decision, usize), opts: DrainOpts, what: &str, cmd_id: &str, act: F) -> Result<Value, AdminRefusal>
where
    F: FnOnce(&Executor) -> Result<Value, AdminRefusal> + Send + 'static,
{
    // Any update or rollback that acts (now, or a new drain) supersedes a
    // drain still waiting to act; a refused one changes nothing.
    let supersede = || e.drain_gen.fetch_add(1, Ordering::SeqCst) + 1;
    if d != Decision::Drain {
        supersede();
        return act(e);
    }
    if e.draining.swap(true, Ordering::SeqCst) {
        return Err(AdminRefusal::new(code::BUSY, "this node is already draining for an update or restart"));
    }
    let gen = supersede();
    let control_path = e.paths.layout().control();
    let prior = match crate::config::read_control(&control_path) {
        Ok(c) => c,
        Err(x) => {
            e.draining.store(false, Ordering::SeqCst);
            return Err(fail(x));
        }
    };
    let rec = DrainRecord {
        what: what.to_string(),
        cmd_id: cmd_id.to_string(),
        by: format!("k2-node {what} (draining)"),
        prior_control: prior.control,
        prior_by: prior.by.clone(),
        prior_at: prior.at,
        at: e.now(),
    };
    // Recorded before control changes: a crash after it is undone at start.
    let saved = serde_json::to_vec(&rec).map_err(|x| x.to_string()).and_then(|b| crate::util::atomic_write(&drain_path(e), &b, 0o600));
    if let Err(x) = saved {
        e.draining.store(false, Ordering::SeqCst);
        return Err(fail(format!("drain record: {x}")));
    }
    if let Err(x) = crate::config::write_control(&control_path, k2_node_proto::frames::Control::Draining, &rec.by, e.now()) {
        let _ = std::fs::remove_file(drain_path(e));
        e.draining.store(false, Ordering::SeqCst);
        return Err(fail(x));
    }
    let _ = e.audit().append(
        AuditEntry { event: "drain".into(), cmd_id: Some(cmd_id.to_string()), detail: json!({"what": what, "running": running}), ..Default::default() },
        e.now(),
    );
    let ex = e.bg_handle();
    let resp = json!({"phase": "draining", "what": what, "running": running, "max_secs": e.drain_max.as_secs(), "gives_up_at_limit": opts.give_up_at_limit});
    let stop_cmd = cmd_id.to_string();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + ex.drain_max;
        if opts.stop_running && running_jobs(&ex) > 0 {
            // Control says `draining` already: the restarted node takes
            // nothing new, and the jobs it stops end `node_update`.
            if let Err(x) = restart_marked(&ex, &rec.what, &stop_cmd, |e| e.platform.restart_unit("k2-node")) {
                crate::nlog!("admin: {}: stopping the running jobs: {x}", rec.what);
            }
        }
        loop {
            while running_jobs(&ex) > 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(if cfg!(test) { 20 } else { 2000 }));
            }
            if !opts.settle || std::time::Instant::now() >= deadline {
                break;
            }
            // Control has said `draining` all along; give the node one
            // status write to report a job it accepted just before.
            std::thread::sleep(ex.drain_settle);
            if running_jobs(&ex) == 0 {
                break;
            }
        }
        let left = running_jobs(&ex);
        let r = {
            let _g = ex.busy.lock().unwrap_or_else(|p| p.into_inner());
            let r = if ex.reexec.load(Ordering::SeqCst) {
                // The executor is on its way out (re-exec after an update):
                // never start an action it can't finish (B-R2).
                Err(AdminRefusal::new("drain_abandoned", "the executor is restarting; nothing was done"))
            } else if ex.drain_gen.load(Ordering::SeqCst) != gen {
                // A later update or rollback (`now`) came first: this
                // drained action is stale and never runs.
                Err(AdminRefusal::new(code::BUSY, "superseded by a later update or rollback"))
            } else if opts.give_up_at_limit && left > 0 {
                crate::nlog!("admin: {} gave up: {left} job(s) still running at the drain limit; nothing restarted", rec.what);
                Err(AdminRefusal::new(DRAIN_GAVE_UP, format!("{left} job(s) still running after {} s; nothing was restarted", ex.drain_max.as_secs())))
            } else {
                act(&ex)
            };
            restore_control(&ex, &rec);
            let _ = std::fs::remove_file(drain_path(&ex));
            r
        };
        let _ = ex.audit().append(
            AuditEntry {
                event: "drain".into(),
                cmd_id: Some(rec.cmd_id.clone()),
                code: r.as_ref().err().map(|x| x.code.clone()),
                detail: json!({"what": rec.what, "phase": "done", "left_running": left}),
                ..Default::default()
            },
            ex.now(),
        );
        ex.draining.store(false, Ordering::SeqCst);
    });
    Ok(resp)
}

/// The audit code of a drain that gave up at its limit.
pub const DRAIN_GAVE_UP: &str = "drain_gave_up";

/// Run the health gate in the background (holding the busy lock so no
/// other root action interleaves). Healthy → record; else roll back.
pub fn start_gate(e: &Executor) {
    let ex = e.bg_handle();
    std::thread::spawn(move || {
        // Wait until the request that started us released the lock.
        std::thread::sleep(Duration::from_millis(50));
        let _g = ex.busy.lock().unwrap_or_else(|p| p.into_inner());
        gate(&ex);
    });
}

/// The gate itself (blocking). Public for tests.
pub fn gate(e: &Executor) {
    let mut rec = load(e);
    if rec.phase != "swapped" {
        return;
    }
    loop {
        if mark_ok(e, &rec.to, rec.at) {
            rec.phase = "healthy".into();
            rec_step(e, &mut rec, "healthy", true, "");
            let _ = e.audit().append(AuditEntry { event: "update".into(), detail: json!({"phase": "healthy", "from": rec.from, "to": rec.to}), cmd_id: rec.cmd_id.clone(), ..Default::default() }, e.now());
            retire_update_mark(e);
            if e.exit_after_update {
                // Re-exec on the new binary once the requests in flight are
                // answered (C5): `serve` sees the flag, stops accepting,
                // joins its connections and returns; the service manager
                // starts the new binary on the next connection.
                rec_step(e, &mut rec, "reexec", true, "after the requests in flight");
                e.reexec.store(true, Ordering::SeqCst);
            }
            return;
        }
        if e.now() >= rec.deadline {
            let _ = rollback_inner(e, "health_gate", true);
            return;
        }
        std::thread::sleep(Duration::from_millis(if cfg!(test) { 20 } else { 1000 }));
    }
}

/// On executor start: a gate interrupted by an executor crash resumes; a
/// swap interrupted between its record and the rename is finished (the
/// new build is in: restart into the gate) or dropped (it isn't: nothing
/// changed). B-L2.
pub fn resume_pending(e: &Executor) {
    let mut rec = load(e);
    match rec.phase.as_str() {
        "swapped" => start_gate(e),
        "swapping" => {
            let bin = e.paths.binary.clone();
            if installed_version(&bin).as_deref() == Some(rec.to.as_str()) {
                let now = e.now();
                rec.phase = "swapped".into();
                rec.at = now;
                rec.deadline = now + HEALTH_GATE_SECS;
                rec_step(e, &mut rec, "swap", true, "found in place after an executor restart");
                if save(e, &rec).is_ok() {
                    if let Err(x) = restart_into_gate(e, &mut rec) {
                        crate::nlog!("admin: resuming the update to {}: {}", rec.to, x.message);
                    }
                }
            } else {
                let _ = std::fs::remove_file(sib(&bin, ".new"));
                rec.phase = "failed".into();
                rec.reason = Some("the executor stopped before the swap; nothing changed".into());
                rec_step(e, &mut rec, "swap", false, "interrupted before the swap; nothing changed");
            }
        }
        _ => {}
    }
}

/// The oldest build a manual rollback may put back: the first with the
/// updater fix (0.46.4). Anything older can't take a remote update again,
/// so going back there needs a person at the machine (A-M2 / B-L4).
pub const ROLLBACK_FLOOR: &str = ADOPT_MIN;

/// `node.rollback` (operate): to the local `.prev` only, and never below
/// [`ROLLBACK_FLOOR`] or to a build without a marker.
pub fn rollback(e: &Executor, why: &str) -> Result<Value, AdminRefusal> {
    let prev = sib(&e.paths.binary, ".prev");
    if is_regular(&prev) {
        let floor = Semver::parse(ROLLBACK_FLOOR).ok_or_else(|| fail("rollback floor unreadable"))?;
        match installed_version(&prev).as_deref().and_then(Semver::parse) {
            Some(v) if v >= floor => {}
            Some(v) => {
                return Err(AdminRefusal::new(
                    code::DOWNGRADE,
                    format!("the previous build here is {v}, older than {ROLLBACK_FLOOR} (the updater fix): going back to it needs an install at the machine (sudo k2-node setup)"),
                ))
            }
            None => return Err(AdminRefusal::new(code::DOWNGRADE, "the previous build here has no version marker: install at the machine instead")),
        }
    }
    let r = rollback_inner(e, why, false)?;
    if e.exit_after_update {
        e.reexec.store(true, Ordering::SeqCst);
    }
    Ok(r)
}

/// `gate_steps`: a rollback that ends this update's gate (its steps go on);
/// otherwise (a manual rollback) the steps start fresh. A gate rollback
/// always puts back the build that ran before the update, floor or not.
fn rollback_inner(e: &Executor, why: &str, gate_steps: bool) -> Result<Value, AdminRefusal> {
    let bin = e.paths.binary.clone();
    let (prev, bad) = (sib(&bin, ".prev"), sib(&bin, ".bad"));
    if !is_regular(&prev) {
        return Err(AdminRefusal::new(code::NO_PREVIOUS, "no previous k2-node to roll back to"));
    }
    if !is_regular(&bin) {
        return Err(fail("installed k2-node is not a regular file"));
    }
    // What goes back in must be executable by the node user (Bug 1).
    ensure_exec_mode(&prev).map_err(|x| fail(format!("rollback: {x}")))?;
    let current = installed_version(&bin).unwrap_or_else(|| "unknown".into());
    let back_to = installed_version(&prev).unwrap_or_else(|| "unknown".into());
    // Record first: a crash after the swap must not let the bad build back.
    let mut rec = load(e);
    if current != "unknown" && !rec.refused.contains(&current) {
        rec.refused.push(current.clone());
    }
    if !gate_steps {
        rec.steps.clear();
    }
    rec.phase = "rolled_back".into();
    rec.from = current.clone();
    rec.to = back_to.clone();
    rec.at = e.now();
    rec.reason = Some(why.to_string());
    save(e, &rec)?;
    rec_step(e, &mut rec, "rollback", true, format!("why={why}"));
    // Atomic (B-L1): `<bin>` is replaced in one rename, so there is always
    // a k2-node at that path; `.bad` is a hard link of the build going out.
    let swap = (|| -> std::io::Result<()> {
        if std::fs::symlink_metadata(&bad).is_ok() {
            std::fs::remove_file(&bad)?;
        }
        std::fs::hard_link(&bin, &bad)?;
        std::fs::rename(&prev, &bin)?;
        Ok(())
    })();
    if let Err(x) = swap {
        rec_step(e, &mut rec, "swap", false, x.to_string());
        return Err(fail(format!("rollback swap: {x}")));
    }
    fsync_dir(&bin);
    let named = mark_restart(e, "node.rollback", rec.cmd_id.as_deref().unwrap_or("-"));
    rec_step(e, &mut rec, "mark", true, mark_detail(named));
    let restarted = e.platform.restart_node();
    rec_step(e, &mut rec, "restart", restarted.is_ok(), restarted.as_ref().err().cloned().unwrap_or_default());
    // The restart is over either way: the mark keeps nothing now (B-M4).
    retire_update_mark(e);
    let _ = e.audit().append(
        AuditEntry { event: "update".into(), code: restarted.as_ref().err().map(|_| code::FAILED.to_string()), detail: json!({"phase": "rolled_back", "from": current, "to": back_to, "why": why}), cmd_id: rec.cmd_id.clone(), ..Default::default() },
        e.now(),
    );
    restarted.map_err(|x| fail(format!("rolled back, but restarting the node failed: {x}")))?;
    Ok(json!({"phase": "rolled_back", "from": current, "to": back_to, "why": why}))
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use crate::admin::tests::Rig;
    use k2_node_proto::frames::Control;
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, ShareState, Trust};

    fn share_node(r: &Rig, servers: usize) {
        let mut f = ServersFile::new(&"a".repeat(64), "owner.k2.dev", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, r.exec.now() - 10, "local:t");
        for slot in 1..servers {
            let mut e = f.servers[0].clone();
            e.slot = slot as u8;
            e.label = format!("s{slot}.k2.dev");
            e.fp_prefix = format!("{slot:016x}");
            e.trust = Trust::Own;
            e.state = ShareState::Active;
            e.set_limits(&ShareLimits::new_server(Trust::Own, false));
            f.servers.push(e);
        }
        crate::admin::servers::write_file(&r.exec.paths.config_dir, &f).unwrap();
    }

    fn running(r: &Rig, jobs: &[&str]) {
        let st = crate::status::Status { running: jobs.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        crate::status::write(&r.exec.paths.layout().status(), &st).unwrap();
    }

    fn control(r: &Rig) -> Control {
        crate::config::read_control(&r.exec.paths.layout().control()).unwrap().control
    }

    fn calls(r: &Rig) -> Vec<String> {
        r.plat.calls.lock().unwrap().clone()
    }

    fn wait(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..300 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    /// V55: a k2-node restart on a shared node with running jobs drains
    /// first (no new starts), restarts once they end, puts the owner's
    /// control back and marks the restart an update. Solo and `now` restart
    /// at once. Every K2-made restart is marked, solo too (C3, 0.46.4).
    #[test]
    fn shared_restarts_drain_and_solo_ones_restart_at_once() {
        // Solo: at once, and marked with the node's pid.
        let solo = Rig::new("full");
        running(&solo, &["j1"]);
        let res = solo.run(&solo.approve(solo.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(calls(&solo), vec!["restart-unit k2-node".to_string()]);
        let m = read_update_mark(&solo.exec.paths.config_dir).expect("a solo restart is marked too (C3)");
        assert_eq!((m.what.as_str(), m.node_pid, m.keep, m.ended), ("unit.restart", Some(crate::admin::tests::node_pid()), false, false));
        assert_eq!(m.cmd_id.as_deref(), Some(res.cmd_id.as_str()));

        // Shared with a running job: drains, then restarts.
        let r = Rig::new("full");
        share_node(&r, 2);
        running(&r, &["j1", "t1-0123456789abcdef"]);
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(res.result["phase"], "draining");
        assert_eq!(res.result["running"], 2);
        assert_eq!(control(&r), Control::Draining, "no new starts while draining");
        std::thread::sleep(Duration::from_millis(200));
        assert!(calls(&r).is_empty(), "nothing restarts while jobs run");
        assert!(read_update_mark(&r.exec.paths.config_dir).is_none(), "the mark is written at the restart, not at the drain");
        let again = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(again.code.as_deref(), Some(code::BUSY), "one drain at a time: {again:?}");
        running(&r, &[]);
        wait("the restart", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        wait("control back", || control(&r) == Control::Active);
        assert!(recent_update_mark(&r.exec.paths.config_dir, r.exec.now()), "the restart is marked an update");
        // The drain's end line is written after the control is put back.
        wait("drain start and end audited", || r.exec.audit().verify().unwrap().iter().filter(|l| l.event == "drain").count() == 2);

        // Shared with `now`: at once.
        let n = Rig::new("full");
        share_node(&n, 3);
        running(&n, &["j1"]);
        let res = n.run(&n.approve(n.cmd("unit.restart", json!({"unit": "k2-node", "now": true}))));
        assert_eq!((res.state.as_str(), calls(&n)), ("done", vec!["restart-unit k2-node".to_string()]), "{res:?}");
        assert_eq!(control(&n), Control::Active);
    }

    /// C3/T5, B-M4: the mark names one process, by pid and start time. Only
    /// that process is "stopped by K2"; a later stop of the next node (an
    /// owner's `systemctl stop`, another pid, or the same pid reused) is
    /// not. Only a `keep` mark of an open restart lets the named process
    /// leave its jobs (U3), and only while fresh.
    #[test]
    fn the_update_mark_names_one_process() {
        let r = Rig::new("full");
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.state, "done", "{res:?}");
        let cfg = &r.exec.paths.config_dir;
        let now = r.exec.now();
        let pid = crate::admin::tests::node_pid();
        let start = crate::util::process_start_time(pid);
        assert!(start.is_some(), "this platform reads a process's start time");
        let m = read_update_mark(cfg).expect("the mark");
        assert_eq!((m.v, m.node_pid, m.node_start), (MARK_V, Some(pid), start));
        assert!(update_mark_for(cfg, pid, start, now).is_some(), "the restarted node is named");
        assert!(update_mark_for(cfg, pid + 1, start, now).is_none(), "the next node (another pid) is not");
        assert!(update_mark_for(cfg, pid, start.map(|s| s + 1), now).is_none(), "a reused pid (another start time) is not");
        assert!(update_mark_for(cfg, pid, start, now + UPDATE_MARK_SECS).is_none(), "a stale mark names nobody");
        assert!(recent_update_mark(cfg, now + 5), "the next node's start-time reconcile still reads the reason");
        assert!(!m.keeps_jobs_of(pid, start, now), "U1 never keeps jobs (keep=false until U4)");
        let keep = UpdateMark { keep: true, ..m.clone() };
        assert!(keep.keeps_jobs_of(pid, start, now));
        assert!(!keep.keeps_jobs_of(pid + 1, start, now), "an owner stop of another process stops its jobs (Q2)");
        assert!(!keep.keeps_jobs_of(pid, start.map(|s| s + 1), now), "a reused pid keeps nothing");
        assert!(!keep.keeps_jobs_of(pid, start, now + UPDATE_MARK_SECS));
        assert!(!UpdateMark { ended: true, ..keep.clone() }.keeps_jobs_of(pid, start, now), "an ended restart keeps nothing");
        assert!(!UpdateMark { node_start: None, ..keep.clone() }.keeps_jobs_of(pid, start, now), "no start time, no keep");
        let nobody = UpdateMark { node_pid: None, ..keep };
        assert!(!nobody.keeps_jobs_of(pid, start, now) && !nobody.names(pid, start, now), "a v2 mark with no pid names nobody (A-L3)");
        // A 0.46.3 executor's mark (no `v`, no pid): by time, as before.
        let legacy: UpdateMark = serde_json::from_str(&format!(r#"{{"what":"node.update","at":{now}}}"#)).unwrap();
        assert!(legacy.names(pid + 7, None, now) && !legacy.keeps_jobs_of(pid + 7, None, now));
    }

    /// A-M1 / B-M3: a mark root didn't write (here: not the expected
    /// owner's mode, a symlink, a second link) is ignored.
    #[test]
    fn a_forged_mark_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let r = Rig::new("full");
        r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        let cfg = r.exec.paths.config_dir.clone();
        let p = update_mark_path(&cfg);
        assert!(read_update_mark(&cfg).is_some());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(read_update_mark(&cfg).is_none(), "group/other-writable: ignored");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = cfg.join("copy.json");
        std::fs::hard_link(&p, &link).unwrap();
        assert!(read_update_mark(&cfg).is_none(), "a second hard link: ignored");
        std::fs::remove_file(&link).unwrap();
        let real = cfg.join("real.json");
        std::fs::rename(&p, &real).unwrap();
        std::os::unix::fs::symlink(&real, &p).unwrap();
        assert!(read_update_mark(&cfg).is_none(), "a symlink: ignored");
        assert_eq!(mark_owner(std::path::Path::new(crate::paths::DEFAULT_CONFIG_DIR)), 0, "the installed node trusts root's marks only");
    }

    fn mkfifo(p: &std::path::Path) {
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: valid C path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo: {}", std::io::Error::last_os_error());
    }

    /// Run `f` on a thread; fail loudly if it doesn't return within 5 s.
    fn within_5s<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("{what} blocked (a FIFO wedged the reader)"))
    }

    /// A-N1: a FIFO planted as the mark, as jobs.toml, admin.toml, servers.toml
    /// or control.toml never wedges a reader: it is refused at once.
    #[test]
    fn a_planted_fifo_never_blocks_a_reader() {
        let r = Rig::new("full");
        let cfg = r.exec.paths.config_dir.clone();
        r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        let p = update_mark_path(&cfg);
        std::fs::remove_file(&p).unwrap();
        mkfifo(&p);
        let c2 = cfg.clone();
        assert!(within_5s("read_update_mark", move || read_update_mark(&c2)).is_none());
        let c2 = cfg.clone();
        assert!(within_5s("update_mark_for", move || update_mark_for(&c2, 1, None, 0)).is_none());
        // The executor's retire never hangs under its lock either: it
        // replaces the FIFO with an ended mark.
        let ex = r.exec.bg_handle();
        within_5s("retire_update_mark", move || retire_update_mark(&ex));
        assert!(read_update_mark(&cfg).is_some_and(|m| m.ended && !m.names(1, None, 0)));
        for f in [crate::admin::jobs::JOBS_FILE, crate::admin::config::ADMIN_FILE] {
            let p = cfg.join(f);
            let _ = std::fs::remove_file(&p);
            mkfifo(&p);
            let p2 = p.clone();
            let e = within_5s(f, move || crate::admin::jobs::read_root_trusted(&p2)).unwrap_err();
            assert!(e.to_string().contains("not a regular file"), "{f}: {e}");
            std::fs::remove_file(&p).unwrap();
        }
        let sp = crate::admin::servers::servers_path(&cfg);
        mkfifo(&sp);
        let c2 = cfg.clone();
        let e = within_5s("servers.toml", move || crate::admin::servers::read_file(&c2)).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
        let cp = r.exec.paths.layout().control();
        let _ = std::fs::remove_file(&cp);
        mkfifo(&cp);
        assert!(within_5s("control.toml", move || crate::config::read_control(&cp)).is_err());
    }

    /// A-N3 / B-R1: the mark lives in its own directory, which must be
    /// writable by its owner only; a group-writable one is ignored.
    #[test]
    fn the_mark_lives_in_a_directory_only_root_writes() {
        use std::os::unix::fs::PermissionsExt;
        let r = Rig::new("full");
        let cfg = r.exec.paths.config_dir.clone();
        r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(update_mark_path(&cfg).parent().unwrap(), mark_dir(&cfg));
        assert_ne!(mark_dir(&cfg), cfg, "never the group-writable config dir");
        assert_eq!(
            mark_dir(std::path::Path::new(crate::paths::DEFAULT_CONFIG_DIR)),
            std::path::PathBuf::from(if cfg!(target_os = "macos") { "/var/run/k2-node-admin" } else { "/run/k2-node-admin" })
        );
        assert!(read_update_mark(&cfg).is_some());
        std::fs::set_permissions(mark_dir(&cfg), std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(read_update_mark(&cfg).is_none(), "a group-writable mark dir: ignored");
        let raw_before = std::fs::read(update_mark_path(&cfg)).unwrap();
        let ran = std::sync::atomic::AtomicBool::new(false);
        restart_marked(&r.exec, "unit.restart", "c2", |_| {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(ran.load(Ordering::SeqCst), "the restart itself still runs");
        let raw_after = std::fs::read(update_mark_path(&cfg)).unwrap();
        assert_eq!(raw_before, raw_after, "no mark is written into an untrusted dir");
        let on_disk: UpdateMark = serde_json::from_slice(&raw_after).unwrap();
        assert_ne!(on_disk.cmd_id.as_deref(), Some("c2"));
    }

    /// Round 4 item 7 (TODO(0.46.5): drop with the fallback): a v1 mark a
    /// 0.46.3 executor left in the config dir still names the stop, only
    /// when its owner wrote it and only as v1; the executor removes it.
    #[test]
    fn a_legacy_mark_counts_during_the_rollout_only() {
        use std::os::unix::fs::PermissionsExt;
        let r = Rig::new("full");
        let cfg = r.exec.paths.config_dir.clone();
        let legacy = cfg.join(UPDATE_MARK_FILE);
        let now = r.exec.now();
        std::fs::write(&legacy, format!(r#"{{"what":"node.update","at":{now}}}"#)).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o644)).unwrap();
        let m = read_update_mark(&cfg).expect("the 0.46.3 mark counts");
        assert!(m.v < MARK_V && m.names(4321, None, now));
        assert!(recent_update_mark(&cfg, now));
        // A v2 mark there is never trusted (only root's own dir holds those).
        std::fs::write(&legacy, format!(r#"{{"v":2,"what":"node.update","at":{now},"keep":true,"node_pid":1}}"#)).unwrap();
        assert!(read_update_mark(&cfg).is_none());
        // A group-writable one isn't either.
        std::fs::write(&legacy, format!(r#"{{"what":"node.update","at":{now}}}"#)).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o664)).unwrap();
        assert!(read_update_mark(&cfg).is_none());
        // The executor removes it at start.
        remove_legacy_mark(&cfg);
        assert!(std::fs::symlink_metadata(&legacy).is_err());
    }

    /// Round 4 item 2: root's reads of node- or owner-writable files never
    /// follow a link (a secret's contents never reach an error), are
    /// capped at 64 KiB, and a parse error never quotes the file.
    #[test]
    fn root_reads_refuse_links_and_big_files_and_never_echo_content() {
        let r = Rig::new("full");
        let cfg = r.exec.paths.config_dir.clone();
        let secret = r.dir.join("secret.txt");
        std::fs::write(&secret, "TOPSECRET-4f1c").unwrap();
        let cp = r.exec.paths.layout().control();
        let _ = std::fs::remove_file(&cp);
        std::os::unix::fs::symlink(&secret, &cp).unwrap();
        let e = crate::config::read_control(&cp).unwrap_err();
        assert!(!e.contains("TOPSECRET"), "{e}");
        let e = crate::util::read_regular(&cp).unwrap_err().to_string();
        assert!(e.contains("symlink") && !e.contains("TOPSECRET"), "{e}");
        std::fs::remove_file(&cp).unwrap();
        // Too big.
        std::fs::write(&cp, vec![b'#'; (crate::util::READ_REGULAR_MAX + 1) as usize]).unwrap();
        let e = crate::util::read_regular(&cp).unwrap_err().to_string();
        assert!(e.contains("larger than"), "{e}");
        // A parse error names the file, not its content.
        std::fs::write(&cp, "state = TOPSECRET-4f1c [[[").unwrap();
        let e = crate::config::read_control(&cp).unwrap_err();
        assert!(!e.contains("TOPSECRET"), "{e}");
        std::fs::write(crate::admin::servers::servers_path(&cfg), "x = TOPSECRET-4f1c [[[").unwrap();
        let e = crate::admin::servers::read_file(&cfg).unwrap_err();
        assert!(!e.contains("TOPSECRET"), "{e}");
    }

    /// Round 4 item 3: a FIFO at the node's status file or its healthy
    /// mark never wedges root: running jobs count as busy, the gate's
    /// check fails closed (so it times out into a rollback).
    #[test]
    fn a_fifo_at_status_or_healthy_fails_closed() {
        let r = Rig::new("full");
        let sp = r.exec.paths.layout().status();
        let _ = std::fs::remove_file(&sp);
        mkfifo(&sp);
        let ex = r.exec.bg_handle();
        assert!(within_5s("running_jobs", move || running_jobs(&ex)) >= 1, "unreadable status = busy");
        let hp = healthy_path(&r.exec.paths.home);
        std::fs::create_dir_all(hp.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&hp);
        mkfifo(&hp);
        let ex = r.exec.bg_handle();
        assert!(!within_5s("mark_ok", move || mark_ok(&ex, "99.0.0", 0)), "not healthy");
        // No status file at all: nothing running.
        std::fs::remove_file(&sp).unwrap();
        assert_eq!(running_jobs(&r.exec), 0);
    }

    /// Round 4 item 1: a job the node takes between the count and the
    /// restart (here: while the drain settles, nothing running before) is
    /// never stopped; the restart comes once it ended.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_job_starting_during_the_settle_is_never_killed() {
        let mut r = Rig::new("policy");
        // A 2 s settle: the late job lands 300 ms in, well inside it.
        r.exec.drain_settle = Duration::from_secs(2);
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(res.result["restart"]["phase"], "draining", "always through the drain, even with nothing running: {res:?}");
        std::thread::sleep(Duration::from_millis(300));
        running(&r, &["late-job"]);
        // Past the settle's recount (2 s), which saw the late job.
        std::thread::sleep(Duration::from_millis(2600));
        assert!(calls(&r).is_empty(), "the late job is never stopped");
        assert_eq!(control(&r), Control::Draining, "and nothing new starts meanwhile");
        running(&r, &[]);
        wait("the restart", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        wait("control back", || control(&r) == Control::Active);
        assert!(!r.exec.paths.state_dir.join(crate::admin::jobs::WALL_RESTART_PENDING_FILE).exists(), "the restart clears the owed flag");
    }

    /// Round 4 item 1: a wall.enable that couldn't restart (here: it gave
    /// up at the drain limit) leaves the restart owed, across an executor
    /// restart; asking again finishes it even though the files are in place.
    #[cfg(target_os = "linux")]
    #[test]
    fn asking_again_finishes_an_owed_wall_restart() {
        let mut r = Rig::new("policy");
        r.exec.drain_max = Duration::from_millis(300);
        running(&r, &["stuck"]);
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(res.result["changed"], true, "{res:?}");
        wait("the give-up", || !r.exec.draining.load(Ordering::SeqCst));
        assert!(calls(&r).is_empty());
        let pending = r.exec.paths.state_dir.join(crate::admin::jobs::WALL_RESTART_PENDING_FILE);
        assert!(pending.exists(), "the restart is still owed");
        // A new executor over the same files (it re-exec'd meanwhile).
        let mut next = Executor::new(r.exec.paths.clone(), r.plat.clone());
        next.drain_settle = Duration::from_millis(60);
        running(&r, &[]);
        let again = next.handle_command(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(again.state, "done", "{again:?}");
        assert_eq!((again.result["changed"].as_bool(), again.result["restart_pending"].as_bool()), (Some(false), Some(true)), "{again:?}");
        assert_eq!(again.result["restart"]["phase"], "draining", "{again:?}");
        wait("the restart", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        wait("owed flag cleared", || !pending.exists());
        // Nothing owed any more: a third call restarts nothing.
        let third = next.handle_command(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert!(third.result.get("restart").is_none(), "{third:?}");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls(&r).len(), 1);
    }

    /// B-R2: a drain thread that gets the lock as the executor re-execs
    /// never starts its action; control goes back.
    #[test]
    fn a_drain_never_acts_once_the_executor_is_reexecing() {
        let r = Rig::new("full");
        share_node(&r, 2);
        running(&r, &["j1", "t1-0123456789abcdef"]);
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.result["phase"], "draining", "{res:?}");
        r.exec.reexec.store(true, Ordering::SeqCst);
        running(&r, &[]);
        wait("the drain to end", || !r.exec.draining.load(Ordering::SeqCst));
        assert!(calls(&r).is_empty(), "nothing restarted while the executor leaves");
        assert_eq!(control(&r), Control::Active);
        assert!(r.exec.audit().verify().unwrap().iter().any(|l| l.event == "drain" && l.code.as_deref() == Some("drain_abandoned")));
    }

    /// Round 5 item 1 (Linux): the node's word (status.json says walled)
    /// never clears root's owed-restart flag, and restarts nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_status_saying_walled_never_clears_the_owed_restart() {
        let r = Rig::new("policy");
        let pending = r.exec.paths.state_dir.join(crate::admin::jobs::WALL_RESTART_PENDING_FILE);
        std::fs::write(&pending, b"c0").unwrap();
        let st = crate::status::Status { wall: "job_user".into(), ..Default::default() };
        crate::status::write(&r.exec.paths.layout().status(), &st).unwrap();
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert!(pending.exists(), "only a restart the executor made clears it");
        assert_eq!(res.result["restart_pending"], true, "{:?}", res.result);
        assert!(res.result.get("restart").is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert!(calls(&r).is_empty());
    }

    /// Round 5 item 2: the writer checks the mark directory on the fd it
    /// writes through: a group-writable directory or a symlinked one gets
    /// no mark, a good one gets it atomically (no temp file left).
    #[test]
    fn the_mark_writer_checks_the_directory_it_writes_through() {
        use std::os::unix::fs::PermissionsExt;
        let r = Rig::new("full");
        let me = unsafe { libc::geteuid() };
        let good = r.dir.join("good");
        std::fs::create_dir(&good).unwrap();
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_owned_in(&good, "m.json", b"{}", 0o644, me).unwrap();
        let names: Vec<String> = std::fs::read_dir(&good).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["m.json".to_string()], "no temp file left");
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(good.join("m.json")).unwrap().mode() & 0o7777, 0o644);
        let open = r.dir.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let e = write_owned_in(&open, "m.json", b"{}", 0o644, me).unwrap_err();
        assert!(e.contains("only uid"), "{e}");
        assert!(!open.join("m.json").exists());
        let link = r.dir.join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(write_owned_in(&link, "n.json", b"{}", 0o644, me).is_err(), "a symlinked directory is never written through");
        assert!(!good.join("n.json").exists());
        assert!(write_owned_in(&good, "m.json", b"{}", 0o644, me + 1).is_err(), "another owner's directory");
    }

    /// Round 5 item 3: retiring an old-format mark writes a v2 mark that
    /// names no process, keeping its time (reasons still read it).
    #[test]
    fn retiring_a_v1_mark_writes_v2_naming_nobody() {
        let r = Rig::new("full");
        let cfg = r.exec.paths.config_dir.clone();
        let now = r.exec.now();
        std::fs::write(cfg.join(UPDATE_MARK_FILE), format!(r#"{{"what":"node.update","at":{now}}}"#)).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(cfg.join(UPDATE_MARK_FILE), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let before = read_update_mark(&cfg).unwrap();
        assert!(before.v < MARK_V && before.names(777, None, now), "a v1 mark names any process");
        retire_update_mark(&r.exec);
        let after = read_update_mark(&cfg).unwrap();
        assert_eq!((after.v, after.ended, after.at, after.node_pid), (MARK_V, true, now, None), "{after:?}");
        assert!(!after.names(777, None, now), "retired: names nobody");
        assert!(recent_update_mark(&cfg, now + 1), "its time still gives the reason");
    }

    /// Round 5 item 4: the running count reads only `running` from the
    /// status file, so a file without the other fields (a future shape)
    /// still counts right and never reads as busy forever.
    #[test]
    fn the_running_count_reads_only_running() {
        let r = Rig::new("full");
        let sp = r.exec.paths.layout().status();
        std::fs::write(&sp, r#"{"running":["a","b"],"some_future_field":{"x":1}}"#).unwrap();
        assert!(crate::status::read(&sp).is_err(), "the full Status doesn't parse this");
        assert_eq!(running_jobs(&r.exec), 2);
        std::fs::write(&sp, r#"{"version":"9.9.9"}"#).unwrap();
        assert_eq!(running_jobs(&r.exec), 0, "no `running` field: nothing running");
        std::fs::write(&sp, "not json").unwrap();
        assert_eq!(running_jobs(&r.exec), 1, "garbage: busy");
    }

    /// Round 5 item 7: the executor reports root's owed-restart flag, and
    /// the health note then says the restart is owed, not "run setup".
    #[test]
    fn health_reports_an_owed_wall_restart() {
        let r = Rig::new("full");
        assert!(!r.exec.status().wall_restart_pending);
        std::fs::write(r.exec.paths.state_dir.join(crate::admin::jobs::WALL_RESTART_PENDING_FILE), b"c0").unwrap();
        assert!(r.exec.status().wall_restart_pending);
        use crate::wall::Wall;
        let owed = crate::admin::health::security_fact_with(&Wall::None { why: "not restarted yet".into() }, "k2node", vec![], true);
        let note = owed.notes.join(" ");
        assert!(note.contains(crate::admin::health::WALL_RESTART_OWED) && !note.contains("k2-node setup"), "{note}");
        let fact = k2_node_proto::health::ExecutorFact { reachable: true, wall_restart_pending: true, ..Default::default() };
        assert_eq!(serde_json::to_value(&fact).unwrap()["wall_restart_pending"], true);
        let quiet = k2_node_proto::health::ExecutorFact { reachable: true, ..Default::default() };
        assert!(serde_json::to_value(&quiet).unwrap().get("wall_restart_pending").is_none(), "off the wire when false");
    }

    /// B-M4: a restart that fails leaves no live mark naming the running node.
    #[test]
    fn a_failed_restart_retires_its_mark() {
        let r = Rig::new("full");
        let err = restart_marked(&r.exec, "unit.restart", "c1", |_| Err("systemctl said no".into())).unwrap_err();
        assert_eq!(err, "systemctl said no");
        let m = read_update_mark(&r.exec.paths.config_dir).unwrap();
        assert!(m.ended && !m.keep, "{m:?}");
    }

    /// A-H1 / B-M1 (Linux): `wall.enable` on a node whose jobs still run
    /// unwalled counts them (jobs.toml written or not) and drains; it
    /// restarts once they end; a repeat call changes nothing and never
    /// restarts again.
    #[cfg(target_os = "linux")]
    #[test]
    fn wall_enable_drains_unwalled_jobs_and_restarts_once() {
        let r = Rig::new("policy");
        running(&r, &["j1"]);
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(*r.plat.walls.lock().unwrap(), vec![crate::admin::jobs::DEFAULT_JOB_USER.to_string()]);
        assert!(matches!(crate::admin::jobs::read_jobs_file(&r.exec.paths.config_dir), Ok(Some(_))), "jobs.toml written");
        assert_eq!(res.result["changed"], true);
        assert_eq!(res.result["restart"]["phase"], "draining", "{:?}", res.result);
        assert_eq!(res.result["restart"]["gives_up_at_limit"], true);
        // With jobs.toml in place the count still sees the unwalled job.
        assert!(running_jobs(&r.exec) >= 1);
        std::thread::sleep(Duration::from_millis(150));
        assert!(calls(&r).is_empty(), "a running job is never stopped to turn the wall on");
        running(&r, &[]);
        wait("the restart", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        wait("control back", || control(&r) == Control::Active);
        assert_eq!(read_update_mark(&r.exec.paths.config_dir).unwrap().what, "wall.enable");
        // Again: the files are in place, nothing changes, no restart.
        let again = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": true}))));
        assert_eq!(again.state, "done", "{again:?}");
        assert_eq!(again.result["changed"], false);
        assert!(again.result.get("restart").is_none(), "{:?}", again.result);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls(&r), vec!["restart-unit k2-node".to_string()], "one restart, ever");
    }

    /// B-M1: a drain that may give up (wall.enable) never acts past its
    /// limit while a job still runs; control goes back; the audit says so.
    #[test]
    fn a_drain_that_gives_up_never_restarts() {
        let mut r = Rig::new("full");
        r.exec.drain_max = Duration::from_millis(300);
        running(&r, &["stuck"]);
        let d = decide(&r.exec, false, true);
        assert_eq!(d, (Decision::Drain, 1));
        let res = run_decided(&r.exec, d, DrainOpts { give_up_at_limit: true, settle: false, stop_running: false }, "wall.enable", "c1", |e| {
            restart_marked(e, "wall.enable", "c1", |e| e.platform.restart_unit("k2-node")).map_err(fail)?;
            Ok(json!({"restarted": "k2-node"}))
        })
        .unwrap();
        assert_eq!(res["phase"], "draining");
        wait("the drain to end", || !r.exec.draining.load(Ordering::SeqCst));
        assert!(calls(&r).is_empty(), "nothing restarted at the limit");
        assert_eq!(control(&r), Control::Active, "control back");
        assert!(!r.exec.paths.state_dir.join(DRAIN_FILE).exists(), "the drain record is gone");
        let audit = r.exec.audit().verify().unwrap();
        assert!(audit.iter().any(|l| l.event == "drain" && l.code.as_deref() == Some(DRAIN_GAVE_UP)), "{audit:?}");
        // Nothing running: no drain at all.
        let n = Rig::new("full");
        assert_eq!(decide(&n.exec, false, true), (Decision::Restart, 0));
        assert_eq!(decide(&n.exec, false, false).0, Decision::Restart, "solo updates restart at once");
    }

    /// B-M2: an executor that exits (re-exec) or dies mid-drain never
    /// leaves the owner's control at `draining`: the next one puts it back.
    #[test]
    fn a_drain_left_by_an_exiting_executor_is_undone_at_start() {
        let r = Rig::new("full");
        share_node(&r, 2);
        running(&r, &["j1", "t1-0123456789abcdef"]);
        crate::config::write_control(&r.exec.paths.layout().control(), Control::Paused, "rosson", r.exec.now()).unwrap();
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.result["phase"], "draining", "{res:?}");
        assert_eq!(control(&r), Control::Draining);
        assert!(r.exec.paths.state_dir.join(DRAIN_FILE).exists(), "the drain is recorded");
        // The old executor's drain thread never acts (as if it died with it).
        r.exec.drain_gen.fetch_add(1, Ordering::SeqCst);
        // The next executor over the same files.
        let next = Executor::new(r.exec.paths.clone(), r.plat.clone());
        recover_drain(&next);
        let c = crate::config::read_control(&r.exec.paths.layout().control()).unwrap();
        assert_eq!((c.control, c.by.as_deref()), (Control::Paused, Some("rosson")), "the owner's pause is back");
        assert!(!r.exec.paths.state_dir.join(DRAIN_FILE).exists());
        assert!(next.audit().verify().unwrap().iter().any(|l| l.code.as_deref() == Some("drain_abandoned")));
        // A second start finds nothing to do.
        recover_drain(&next);
        assert!(calls(&r).is_empty());
    }

    /// Q5: `wall.enable` is operate at `policy` (no owner signature); `off`
    /// refuses it; `restart` must be a boolean. The owner's own choice
    /// stands (A-L2 / B-L8): after `--no-job-wall`, or with a custom job
    /// user, it needs the owner's approval, and the approval clears the opt-out.
    #[test]
    fn wall_enable_is_operate_at_policy_unless_the_owner_chose() {
        let r = Rig::new("policy");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": "yes"}))));
        assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"user": "root"}))));
        assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({}))));
        if cfg!(target_os = "macos") {
            assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "macOS has Seatbelt, nothing to enable: {res:?}");
        } else {
            assert_eq!(res.state, "done", "{res:?}");
            assert_eq!(res.result["restart_needed"], "k2-node", "no restart asked: {:?}", res.result);
        }
        let off = Rig::new("off");
        let res = off.run(&off.sign(&off.cmd("wall.enable", json!({}))));
        assert_eq!(res.code.as_deref(), Some(code::REMOTE_ADMIN_OFF), "{res:?}");

        // The owner's --no-job-wall: an operator can't undo it.
        let o = Rig::new("policy");
        std::fs::write(o.exec.paths.state_dir.join(crate::admin::jobs::WALL_OPT_OUT_FILE), b"").unwrap();
        let res = o.run(&o.sign(&o.cmd("wall.enable", json!({}))));
        assert_eq!(res.code.as_deref(), Some(code::APPROVAL_REQUIRED), "{res:?}");
        let res = o.run(&o.approve(o.cmd("wall.enable", json!({}))));
        if cfg!(target_os = "linux") {
            assert_eq!(res.state, "done", "{res:?}");
            assert!(!o.exec.paths.state_dir.join(crate::admin::jobs::WALL_OPT_OUT_FILE).exists(), "the owner's approval clears the opt-out");
        }
        // A custom job user the owner set: the same.
        let u = Rig::new("policy");
        let f = crate::admin::jobs::JobsFile { job_user: "buildbot".into(), tenants: Default::default() };
        crate::admin::jobs::write_jobs(&u.exec.paths.config_dir, &f).unwrap();
        let res = u.run(&u.sign(&u.cmd("wall.enable", json!({}))));
        assert_eq!(res.code.as_deref(), Some(code::APPROVAL_REQUIRED), "{res:?}");
    }

    /// Review A L7: a later `now` supersedes a drain still waiting; the
    /// drained action never runs after it (no second restart, no stale
    /// swap after a rollback), and the drain still puts control back.
    #[test]
    fn a_later_now_supersedes_a_waiting_drain() {
        let r = Rig::new("full");
        share_node(&r, 2);
        running(&r, &["j1", "t1-0123456789abcdef"]);
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.result["phase"], "draining", "{res:?}");
        let now = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node", "now": true}))));
        assert_eq!(now.state, "done", "{now:?}");
        assert_eq!(calls(&r), vec!["restart-unit k2-node".to_string()], "the `now` restart ran at once");
        running(&r, &[]);
        wait("the drain to end", || !r.exec.draining.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls(&r), vec!["restart-unit k2-node".to_string()], "the superseded drained restart never ran");
        wait("control back", || control(&r) == Control::Active);
    }

    /// The drain gives up after `drain_max` and restarts anyway; a machine
    /// owner's pause set during the drain is never overwritten.
    #[test]
    fn a_drain_ends_at_its_limit_and_keeps_the_owners_pause() {
        let mut r = Rig::new("full");
        r.exec.drain_max = Duration::from_millis(300);
        share_node(&r, 2);
        running(&r, &["stuck"]);
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.result["phase"], "draining", "{res:?}");
        crate::config::write_control(&r.exec.paths.layout().control(), Control::Paused, "rosson", r.exec.now()).unwrap();
        wait("the restart at the limit", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(control(&r), Control::Paused, "the owner's pause stands");
        let bad = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node", "now": "yes"}))));
        assert_eq!(bad.code.as_deref(), Some(code::BAD_ARGS));
    }
}

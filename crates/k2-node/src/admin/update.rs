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

/// `<config>/node-update.json`: written by the executor just before every
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

fn mark_ok(e: &Executor, to: &str, since: i64) -> bool {
    let Some(m) = std::fs::read(healthy_path(&e.paths.home)).ok().and_then(|b| serde_json::from_slice::<HealthyMark>(&b).ok()) else {
        return false;
    };
    m.version == to && m.at >= since && m.handshake
}

/// macOS: the staged build must carry K2's Developer ID signature.
fn verify_codesign(staged: &Path) -> Result<(), AdminRefusal> {
    let req = format!("=anchor apple generic and certificate leaf[subject.OU] = \"{APPLE_TEAM_ID}\"");
    let out = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R"])
        .arg(&req)
        .arg(staged)
        .env_clear()
        .output()
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
    run_decided(e, d, "node.update", &cmd_id, move |ex| swap_and_restart(ex, staged))
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
        return Err(s.steps.failed(e, "swap", fail(format!("swap: {x}"))));
    }
    fsync_dir(&bin);
    let now = e.now();
    let from = crate::PRODUCT_VERSION.to_string();
    rec.phase = "swapped".into();
    rec.from = from.clone();
    rec.to = s.to.clone();
    rec.at = now;
    rec.deadline = now + HEALTH_GATE_SECS;
    rec.cmd_id = Some(s.cmd_id.clone());
    rec.reason = None;
    rec.steps = std::mem::take(&mut s.steps.lines);
    rec_step(e, &mut rec, "swap", true, format!(".prev = {from}"));
    // Record first: a crash from here on must resume the gate.
    save(e, &rec)?;
    let _ = std::fs::remove_file(healthy_path(&e.paths.home));
    let pid = mark_restart(e, "node.update", &s.cmd_id);
    rec_step(e, &mut rec, "mark", true, mark_detail(pid));
    if let Err(x) = e.platform.restart_node() {
        rec_step(e, &mut rec, "restart", false, x.clone());
        let r = rollback_inner(e, &format!("restart failed: {x}"), true);
        return Err(fail(format!("restart after swap failed ({x}); rolled back: {}", r.map(|_| "yes".to_string()).unwrap_or_else(|z| z.message))));
    }
    rec_step(e, &mut rec, "restart", true, "");
    rec_step(e, &mut rec, "health_wait", true, format!("up to {HEALTH_GATE_SECS} s"));
    start_gate(e);
    Ok(json!({"phase": "swapped", "from": from, "to": rec.to, "key": s.key, "health_gate_secs": HEALTH_GATE_SECS}))
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

/// Running jobs, every server's: behind the Linux job wall, root counts
/// the live job units itself (review A L7: never the node's word);
/// otherwise (macOS, no wall: jobs are the node's children) the node's
/// status file.
pub fn running_jobs(e: &Executor) -> usize {
    if cfg!(target_os = "linux") && matches!(super::jobs::read_jobs_file(&e.paths.config_dir), Ok(Some(_))) {
        let pattern = format!("{}*", super::jobs::UNIT_PREFIX);
        if let Ok(list) = super::jobs::systemctl(&["list-units", "--plain", "--no-legend", "--state=active,activating,deactivating", &pattern]) {
            return list.lines().filter(|l| l.trim_start().starts_with(super::jobs::UNIT_PREFIX)).count();
        }
    }
    crate::status::read(&e.paths.layout().status()).map(|s| s.running.len()).unwrap_or(0)
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

/// The update mark v2 (C3). The executor writes it before every node
/// restart it makes, naming the node's main pid read just before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMark {
    pub what: String,
    pub at: i64,
    /// The stopped node may leave its jobs running for the next node to
    /// adopt (U3/U4). Always false until U4's `decide` sets it.
    #[serde(default)]
    pub keep: bool,
    #[serde(default)]
    pub cmd_id: Option<String>,
    /// The node process this restart stops (`None` from a 0.46.3 executor,
    /// or when the service manager didn't say).
    #[serde(default)]
    pub node_pid: Option<u32>,
    /// The gate this mark belonged to ended: it keeps nothing any more.
    #[serde(default)]
    pub ended: bool,
}

impl UpdateMark {
    pub fn fresh(&self, now: i64) -> bool {
        now >= self.at && now - self.at < UPDATE_MARK_SECS
    }

    /// Was the process `pid` stopped by this K2-made restart? A mark
    /// without a pid (older executor) goes by time alone, as before.
    pub fn names(&self, pid: u32, now: i64) -> bool {
        self.fresh(now) && self.node_pid.is_none_or(|p| p == pid)
    }

    /// U3's detach test (§5.4): only the process the executor restarted,
    /// only while the mark is fresh and its gate is open, and only with
    /// `keep`. An owner's `systemctl stop` later has another pid (Q2).
    pub fn keeps_jobs_of(&self, pid: u32, now: i64) -> bool {
        self.keep && !self.ended && self.fresh(now) && self.node_pid == Some(pid)
    }
}

/// In the config dir (root's; the node reads it): root never writes into
/// the node's home (review A, H1).
pub fn update_mark_path(config_dir: &Path) -> PathBuf {
    config_dir.join(UPDATE_MARK_FILE)
}

pub fn read_update_mark(config_dir: &Path) -> Option<UpdateMark> {
    std::fs::read(update_mark_path(config_dir)).ok().and_then(|b| serde_json::from_slice::<UpdateMark>(&b).ok())
}

fn write_update_mark(e: &Executor, m: &UpdateMark) {
    if let Ok(b) = serde_json::to_vec(m) {
        if let Err(x) = crate::util::atomic_write(&update_mark_path(&e.paths.config_dir), &b, 0o644) {
            crate::nlog!("admin: update mark: {x}");
        }
    }
}

/// Write the mark for a restart about to happen; returns the node pid it names.
fn mark_restart(e: &Executor, what: &str, cmd_id: &str) -> Option<u32> {
    let node_pid = e.platform.node_pid();
    write_update_mark(e, &UpdateMark { what: what.to_string(), at: e.now(), keep: false, cmd_id: Some(cmd_id.to_string()), node_pid, ended: false });
    node_pid
}

fn mark_detail(pid: Option<u32>) -> String {
    format!("node_pid={} keep=false", pid.map(|p| p.to_string()).unwrap_or_else(|| "unknown".into()))
}

/// At a healthy gate's end the mark keeps nothing (`ended`). It isn't
/// deleted: the restarted node's tenants reconcile after the attach and
/// still read it for the `node_update` reason until it goes stale.
fn retire_update_mark(e: &Executor) {
    if let Some(mut m) = read_update_mark(&e.paths.config_dir) {
        m.keep = false;
        m.ended = true;
        write_update_mark(e, &m);
    }
}

/// Restart the node service the K2 way: the mark first (every K2-made
/// restart, solo nodes too, C3), then `restart`. Returns the pid it named.
pub fn restart_marked(e: &Executor, what: &str, cmd_id: &str, restart: impl FnOnce(&Executor) -> Result<(), String>) -> Result<Option<u32>, String> {
    let pid = mark_restart(e, what, cmd_id);
    crate::nlog!("admin: restart k2-node what={what} cmd={cmd_id} {}", mark_detail(pid));
    restart(e)?;
    Ok(pid)
}

/// Node side: was this process stopped for a restart the executor made in
/// the last few minutes (the restart's own pid, or any pid for a mark
/// from an older executor)? Decides `node_update` vs `node_shutdown` for
/// the jobs it stops.
pub fn update_mark_for(config_dir: &Path, pid: u32, now: i64) -> Option<UpdateMark> {
    read_update_mark(config_dir).filter(|m| m.names(pid, now))
}

/// Node side at start: did the executor restart a node in the last few
/// minutes (whose rows this node now closes)? By time: the mark names the
/// node that was stopped, never this one.
pub fn recent_update_mark(config_dir: &Path, now: i64) -> bool {
    read_update_mark(config_dir).is_some_and(|m| m.fresh(now))
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
    run_decided(e, decide(e, now, false), what, cmd_id, act)
}

/// [`drain_then`] that waits for running jobs on any node, solo too.
pub fn drain_always_then<F>(e: &Executor, what: &str, cmd_id: &str, act: F) -> Result<Value, AdminRefusal>
where
    F: FnOnce(&Executor) -> Result<Value, AdminRefusal> + Send + 'static,
{
    run_decided(e, decide(e, false, true), what, cmd_id, act)
}

/// Act on a [`decide`] result.
pub fn run_decided<F>(e: &Executor, (d, running): (Decision, usize), what: &str, cmd_id: &str, act: F) -> Result<Value, AdminRefusal>
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
    let by = format!("k2-node {what} (draining)");
    if let Err(x) = crate::config::write_control(&control_path, k2_node_proto::frames::Control::Draining, &by, e.now()) {
        e.draining.store(false, Ordering::SeqCst);
        return Err(fail(x));
    }
    let _ = e.audit().append(
        AuditEntry { event: "drain".into(), cmd_id: Some(cmd_id.to_string()), detail: json!({"what": what, "running": running}), ..Default::default() },
        e.now(),
    );
    let ex = e.bg_handle();
    let resp = json!({"phase": "draining", "what": what, "running": running, "max_secs": e.drain_max.as_secs()});
    let (what, cmd_id) = (what.to_string(), cmd_id.to_string());
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + ex.drain_max;
        while running_jobs(&ex) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(if cfg!(test) { 20 } else { 2000 }));
        }
        let left = running_jobs(&ex);
        let r = {
            let _g = ex.busy.lock().unwrap_or_else(|p| p.into_inner());
            let r = if ex.drain_gen.load(Ordering::SeqCst) != gen {
                // A later update or rollback (`now`) came first: this
                // drained action is stale and never runs.
                Err(AdminRefusal::new(code::BUSY, "superseded by a later update or rollback"))
            } else {
                act(&ex)
            };
            // Put the owner's switch back, unless someone changed it since.
            if let Ok(c) = crate::config::read_control(&control_path) {
                if c.control == k2_node_proto::frames::Control::Draining && c.by.as_deref() == Some(by.as_str()) {
                    let back_by = prior.by.clone().unwrap_or_else(|| "k2-node".into());
                    if let Err(x) = crate::config::write_control(&control_path, prior.control, &back_by, prior.at.unwrap_or_else(|| ex.now())) {
                        crate::nlog!("admin: can't restore control after the drain: {x}");
                    }
                }
            }
            r
        };
        let _ = ex.audit().append(
            AuditEntry {
                event: "drain".into(),
                cmd_id: Some(cmd_id),
                code: r.as_ref().err().map(|x| x.code.clone()),
                detail: json!({"what": what, "phase": "done", "left_running": left}),
                ..Default::default()
            },
            ex.now(),
        );
        ex.draining.store(false, Ordering::SeqCst);
    });
    Ok(resp)
}

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

/// On executor start: a gate interrupted by an executor crash resumes.
pub fn resume_pending(e: &Executor) {
    if load(e).phase == "swapped" {
        start_gate(e);
    }
}

/// `node.rollback` (operate): to the local `.prev` only.
pub fn rollback(e: &Executor, why: &str) -> Result<Value, AdminRefusal> {
    let r = rollback_inner(e, why, false)?;
    if e.exit_after_update {
        e.reexec.store(true, Ordering::SeqCst);
    }
    Ok(r)
}

/// `gate_steps`: a rollback that ends this update's gate (its steps go on);
/// otherwise (a manual rollback) the steps start fresh.
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
    let swap = (|| -> std::io::Result<()> {
        if std::fs::symlink_metadata(&bad).is_ok() {
            std::fs::remove_file(&bad)?;
        }
        std::fs::rename(&bin, &bad)?;
        std::fs::rename(&prev, &bin)?;
        Ok(())
    })();
    if let Err(x) = swap {
        rec_step(e, &mut rec, "swap", false, x.to_string());
        return Err(fail(format!("rollback swap: {x}")));
    }
    fsync_dir(&bin);
    // The mark names the bad build's pid; it isn't retired here (no gate
    // follows a rollback, and the pid it names is gone after the restart).
    let pid = mark_restart(e, "node.rollback", rec.cmd_id.as_deref().unwrap_or("-"));
    rec_step(e, &mut rec, "mark", true, mark_detail(pid));
    let restarted = e.platform.restart_node();
    rec_step(e, &mut rec, "restart", restarted.is_ok(), restarted.as_ref().err().cloned().unwrap_or_default());
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
        assert_eq!((m.what.as_str(), m.node_pid, m.keep, m.ended), ("unit.restart", Some(crate::admin::tests::NODE_PID), false, false));
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

    /// C3/T5: the mark names the node pid it stops. Only that process is
    /// "stopped by K2"; a later stop of the next node (an owner's
    /// `systemctl stop`, another pid) is not. Only a `keep` mark of an open
    /// gate lets the named process leave its jobs (U3), and only while fresh.
    #[test]
    fn the_update_mark_names_one_process() {
        let r = Rig::new("full");
        let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.state, "done", "{res:?}");
        let cfg = &r.exec.paths.config_dir;
        let now = r.exec.now();
        let pid = crate::admin::tests::NODE_PID;
        assert!(update_mark_for(cfg, pid, now).is_some(), "the restarted node is named");
        assert!(update_mark_for(cfg, pid + 1, now).is_none(), "the next node (another pid) is not");
        assert!(update_mark_for(cfg, pid, now + UPDATE_MARK_SECS).is_none(), "a stale mark names nobody");
        assert!(recent_update_mark(cfg, now + 5), "the next node's start-time reconcile still reads the reason");
        let m = read_update_mark(cfg).unwrap();
        assert!(!m.keeps_jobs_of(pid, now), "U1 never keeps jobs (keep=false until U4)");
        let keep = UpdateMark { keep: true, ..m.clone() };
        assert!(keep.keeps_jobs_of(pid, now));
        assert!(!keep.keeps_jobs_of(pid + 1, now), "an owner stop of another process stops its jobs (Q2)");
        assert!(!keep.keeps_jobs_of(pid, now + UPDATE_MARK_SECS));
        assert!(!UpdateMark { ended: true, ..keep.clone() }.keeps_jobs_of(pid, now), "an ended gate keeps nothing");
        assert!(!UpdateMark { node_pid: None, ..keep }.keeps_jobs_of(pid, now), "a mark with no pid never keeps jobs");
        // A 0.46.3 executor's mark (no pid): by time, as before.
        let legacy: UpdateMark = serde_json::from_str(&format!(r#"{{"what":"node.update","at":{now}}}"#)).unwrap();
        assert!(legacy.names(pid + 7, now) && !legacy.keep && legacy.node_pid.is_none());
    }

    /// `wall.enable` restarts wait for running jobs on a solo node too.
    #[test]
    fn drain_always_waits_on_a_solo_node() {
        let r = Rig::new("full");
        running(&r, &["j1"]);
        assert_eq!(decide(&r.exec, false, false).0, Decision::Restart, "solo restarts at once for updates");
        assert_eq!(decide(&r.exec, false, true), (Decision::Drain, 1));
        assert_eq!(decide(&r.exec, true, true), (Decision::Now, 1));
        let res = drain_always_then(&r.exec, "wall.enable", "c1", |e| {
            restart_marked(e, "wall.enable", "c1", |e| e.platform.restart_unit("k2-node")).map_err(fail)?;
            Ok(json!({"restarted": "k2-node"}))
        })
        .unwrap();
        assert_eq!(res["phase"], "draining", "{res}");
        std::thread::sleep(Duration::from_millis(150));
        assert!(calls(&r).is_empty(), "a running job is never stopped to turn the wall on");
        running(&r, &[]);
        wait("the restart", || calls(&r) == vec!["restart-unit k2-node".to_string()]);
        wait("control back", || control(&r) == Control::Active);
        assert_eq!(read_update_mark(&r.exec.paths.config_dir).unwrap().what, "wall.enable");
        // Nothing running: at once.
        let n = Rig::new("full");
        assert_eq!(decide(&n.exec, false, true), (Decision::Restart, 0));
    }

    /// Q5: `wall.enable` is operate at `policy` (no owner signature); `off`
    /// refuses it; `restart` must be a boolean.
    #[test]
    fn wall_enable_is_operate_at_policy() {
        let r = Rig::new("policy");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"restart": "yes"}))));
        assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({"user": "root"}))));
        assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
        let res = r.run(&r.sign(&r.cmd("wall.enable", json!({}))));
        for refused in [code::APPROVAL_REQUIRED, code::REMOTE_ADMIN_OFF, code::VERB_NOT_ALLOWED] {
            assert_ne!(res.code.as_deref(), Some(refused), "{res:?}");
        }
        if cfg!(target_os = "macos") {
            assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "macOS has Seatbelt, nothing to enable: {res:?}");
        }
        let off = Rig::new("off");
        let res = off.run(&off.sign(&off.cmd("wall.enable", json!({}))));
        assert_eq!(res.code.as_deref(), Some(code::REMOTE_ADMIN_OFF), "{res:?}");
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

//! Signed self-update and rollback (prd-k2-node-admin-v1 §8, S5).
//!
//! The executor reads the offered build **into memory** (it arrives over
//! the local socket; there is no spool path a job could swap), verifies
//! the minisign signature with the root-helper keys compiled into the
//! running binary, the asset name, the binary format and the marker,
//! checks `version > running` and `protocol ≥ running`, and only then
//! writes `<bin>.new` (root 0755, fsync). Swap: `<bin>` → `<bin>.prev`
//! (hard link), `.new` → `<bin>` (rename). Restart the node. Health gate:
//! up to 120 s for `<home>/run/healthy` naming the new version (and a
//! controller handshake when the node is enrolled). No mark → automatic
//! rollback (`<bin>` → `<bin>.bad`, `.prev` → `<bin>`), the bad version
//! goes on a refuse list, restart. Manual rollback only to the local
//! `.prev` (never a download of an older build).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use k2_node_proto::admin::{code, AdminRefusal};
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

/// `<home>/run/node-update.json`: written by the executor just before it
/// restarts the node for an update, rollback or restart, so the jobs that
/// restart interrupts end `interrupted/node_update` (infra, no node-health
/// strike on the server; V55) instead of `node_shutdown` / `node_restart`.
pub const UPDATE_MARK_FILE: &str = "node-update.json";
/// A mark older than this is stale (a restart that never came).
pub const UPDATE_MARK_SECS: i64 = 600;

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
fn verify_codesign(e: &Executor, staged: &Path) -> Result<(), AdminRefusal> {
    if e.platform.os() != "macos" || e.test_keys {
        return Ok(());
    }
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
    if trigger == "auto" && !c.cfg.auto_update {
        return Err(AdminRefusal::new(code::AUTO_UPDATE_OFF, "this machine's owner turned automatic updates off"));
    }
    let sha = k2_node_proto::crypto::sha256_hex(bytes);
    if !k2_node_proto::crypto::ct_eq(sha.as_bytes(), want_sha.as_bytes()) {
        return Err(AdminRefusal::new(code::BAD_BUILD, "build bytes differ from the build the controller signed for"));
    }
    let keys = k2_root_proto::keys::trusted_keys(e.test_keys);
    let built = verify_node_build(bytes, sig, &keys, e.platform.os(), e.platform.arch()).map_err(|m| AdminRefusal::new(code::BAD_BUILD, m))?;
    if built.version.to_string() != want_v {
        return Err(AdminRefusal::new(code::BAD_BUILD, "signed version differs from the command's"));
    }
    let running = Semver::parse(crate::PRODUCT_VERSION).ok_or_else(|| fail("running version unreadable"))?;
    if built.version == running {
        return Err(AdminRefusal::new(code::SAME_VERSION, "already running that version"));
    }
    if built.version < running || built.protocol < k2_node_proto::PROTOCOL {
        return Err(AdminRefusal::new(code::DOWNGRADE, "older than the running k2-node (roll back instead)"));
    }
    let rec = load(e);
    if rec.refused.iter().any(|v| *v == built.version.to_string()) {
        return Err(AdminRefusal::new(code::ROLLED_BACK_VERSION, "that version was rolled back on this node"));
    }
    let bin = e.paths.binary.clone();
    if !is_regular(&bin) {
        return Err(fail("installed k2-node is not a regular file"));
    }
    // Stage from memory.
    let new = sib(&bin, ".new");
    if std::fs::symlink_metadata(&new).is_ok() {
        if !is_regular(&new) {
            return Err(fail(".new is not a regular file"));
        }
        std::fs::remove_file(&new).map_err(|x| fail(format!("remove stale .new: {x}")))?;
    }
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&new)
            .map_err(|x| fail(format!("stage: {x}")))?;
        f.write_all(bytes).map_err(|x| fail(format!("stage: {x}")))?;
        f.sync_all().map_err(|x| fail(format!("stage: {x}")))?;
    }
    if let Err(r) = verify_codesign(e, &new) {
        let _ = std::fs::remove_file(&new);
        return Err(r);
    }
    let staged = Staged { to: built.version.to_string(), key: built.key_id.to_string(), cmd_id: c.opened.cmd.cmd_id.clone(), sha };
    // A shared node lets running jobs finish first (V55); `.new` waits.
    drain_then(e, now_flag, "node.update", &c.opened.cmd.cmd_id, move |ex| swap_and_restart(ex, &staged))
}

/// A verified build waiting in `<bin>.new`.
struct Staged {
    to: String,
    key: String,
    cmd_id: String,
    /// The verified build's sha256: `.new` must still be it at the swap.
    sha: String,
}

/// Swap `.new` in, restart the node, start the health gate.
fn swap_and_restart(e: &Executor, s: &Staged) -> Result<Value, AdminRefusal> {
    let bin = e.paths.binary.clone();
    let new = sib(&bin, ".new");
    if !is_regular(&new) {
        return Err(fail("the staged build is gone"));
    }
    let mut rec = load(e);
    // A drained swap runs later: the version may have been rolled back
    // meanwhile, or `.new` replaced (review A L7).
    if rec.refused.iter().any(|v| *v == s.to) {
        let _ = std::fs::remove_file(&new);
        return Err(AdminRefusal::new(code::ROLLED_BACK_VERSION, "that version was rolled back on this node"));
    }
    let staged_sha = std::fs::read(&new).map(|b| k2_node_proto::crypto::sha256_hex(&b)).map_err(|x| fail(format!("read the staged build: {x}")))?;
    if !k2_node_proto::crypto::ct_eq(staged_sha.as_bytes(), s.sha.as_bytes()) {
        let _ = std::fs::remove_file(&new);
        return Err(AdminRefusal::new(code::BAD_BUILD, "the staged build changed since it was verified"));
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
        return Err(fail(format!("swap: {x}")));
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
    save(e, &rec)?;
    let _ = std::fs::remove_file(healthy_path(&e.paths.home));
    if let Err(x) = e.platform.restart_node() {
        let r = rollback_inner(e, &format!("restart failed: {x}"));
        return Err(fail(format!("restart after swap failed ({x}); rolled back: {}", r.map(|_| "yes".to_string()).unwrap_or_else(|z| z.message))));
    }
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

/// The mark the node reads when the restart interrupts jobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMark {
    pub what: String,
    pub at: i64,
}

/// In the config dir (root's; the node reads it): root never writes into
/// the node's home (review A, H1).
pub fn update_mark_path(config_dir: &Path) -> PathBuf {
    config_dir.join(UPDATE_MARK_FILE)
}

fn write_update_mark(e: &Executor, what: &str) {
    let m = UpdateMark { what: what.to_string(), at: e.now() };
    if let Ok(b) = serde_json::to_vec(&m) {
        if let Err(x) = crate::util::atomic_write(&update_mark_path(&e.paths.config_dir), &b, 0o644) {
            crate::nlog!("admin: update mark: {x}");
        }
    }
}

/// Node side: was this process (or its jobs) stopped for an update the
/// executor started in the last few minutes?
pub fn recent_update_mark(config_dir: &Path, now: i64) -> bool {
    std::fs::read(update_mark_path(config_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<UpdateMark>(&b).ok())
        .is_some_and(|m| now >= m.at && now - m.at < UPDATE_MARK_SECS)
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
    // Any update or rollback that acts (now, or a new drain) supersedes a
    // drain still waiting to act; a refused one changes nothing.
    let supersede = || e.drain_gen.fetch_add(1, Ordering::SeqCst) + 1;
    if !shared_node(e) {
        // Solo: today's path, today's reasons.
        supersede();
        return act(e);
    }
    let running = running_jobs(e);
    if now || running == 0 {
        supersede();
        write_update_mark(e, what);
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
    let ex = Executor {
        paths: e.paths.clone(),
        platform: e.platform.clone(),
        test_keys: e.test_keys,
        busy: e.busy.clone(),
        exit_after_update: e.exit_after_update,
        drain_max: e.drain_max,
        draining: e.draining.clone(),
        drain_gen: e.drain_gen.clone(),
    };
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
                write_update_mark(&ex, &what);
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
    let ex = Executor {
        paths: e.paths.clone(),
        platform: e.platform.clone(),
        test_keys: e.test_keys,
        busy: e.busy.clone(),
        exit_after_update: e.exit_after_update,
        drain_max: e.drain_max,
        draining: e.draining.clone(),
        drain_gen: e.drain_gen.clone(),
    };
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
            let _ = save(e, &rec);
            let _ = e.audit().append(AuditEntry { event: "update".into(), detail: json!({"phase": "healthy", "from": rec.from, "to": rec.to}), cmd_id: rec.cmd_id.clone(), ..Default::default() }, e.now());
            if e.exit_after_update {
                // Re-exec on the new binary: the service manager starts it
                // again on the next socket connection.
                super::cli::REEXEC.store(true, Ordering::SeqCst);
                crate::nlog!("admin: update to {} healthy; exiting to re-exec", rec.to);
                std::process::exit(0);
            }
            return;
        }
        if e.now() >= rec.deadline {
            let _ = rollback_inner(e, "health_gate");
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
    let r = rollback_inner(e, why)?;
    if e.exit_after_update {
        super::cli::REEXEC.store(true, Ordering::SeqCst);
    }
    Ok(r)
}

fn rollback_inner(e: &Executor, why: &str) -> Result<Value, AdminRefusal> {
    let bin = e.paths.binary.clone();
    let (prev, bad) = (sib(&bin, ".prev"), sib(&bin, ".bad"));
    if !is_regular(&prev) {
        return Err(AdminRefusal::new(code::NO_PREVIOUS, "no previous k2-node to roll back to"));
    }
    if !is_regular(&bin) {
        return Err(fail("installed k2-node is not a regular file"));
    }
    let current = installed_version(&bin).unwrap_or_else(|| "unknown".into());
    let back_to = installed_version(&prev).unwrap_or_else(|| "unknown".into());
    // Record first: a crash after the swap must not let the bad build back.
    let mut rec = load(e);
    if current != "unknown" && !rec.refused.contains(&current) {
        rec.refused.push(current.clone());
    }
    rec.phase = "rolled_back".into();
    rec.from = current.clone();
    rec.to = back_to.clone();
    rec.at = e.now();
    rec.reason = Some(why.to_string());
    save(e, &rec)?;
    let swap = (|| -> std::io::Result<()> {
        if std::fs::symlink_metadata(&bad).is_ok() {
            std::fs::remove_file(&bad)?;
        }
        std::fs::rename(&bin, &bad)?;
        std::fs::rename(&prev, &bin)?;
        Ok(())
    })();
    swap.map_err(|x| fail(format!("rollback swap: {x}")))?;
    fsync_dir(&bin);
    let restarted = e.platform.restart_node();
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
    /// at once; solo leaves no mark (today's reasons).
    #[test]
    fn shared_restarts_drain_and_solo_ones_dont() {
        // Solo: at once, no mark.
        let solo = Rig::new("full");
        running(&solo, &["j1"]);
        let res = solo.run(&solo.approve(solo.cmd("unit.restart", json!({"unit": "k2-node"}))));
        assert_eq!(res.state, "done", "{res:?}");
        assert_eq!(calls(&solo), vec!["restart-unit k2-node".to_string()]);
        assert!(!update_mark_path(&solo.exec.paths.config_dir).exists(), "solo keeps today's reasons");

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

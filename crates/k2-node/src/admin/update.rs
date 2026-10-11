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
    only_keys(args, &["version", "sha256", "trigger"])?;
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
    let mut rec = load(e);
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
    rec.to = built.version.to_string();
    rec.at = now;
    rec.deadline = now + HEALTH_GATE_SECS;
    rec.cmd_id = Some(c.opened.cmd.cmd_id.clone());
    rec.reason = None;
    save(e, &rec)?;
    let _ = std::fs::remove_file(healthy_path(&e.paths.home));
    if let Err(x) = e.platform.restart_node() {
        let r = rollback_inner(e, &format!("restart failed: {x}"));
        return Err(fail(format!("restart after swap failed ({x}); rolled back: {}", r.map(|_| "yes".to_string()).unwrap_or_else(|z| z.message))));
    }
    start_gate(e);
    Ok(json!({"phase": "swapped", "from": from, "to": rec.to, "key": built.key_id, "health_gate_secs": HEALTH_GATE_SECS}))
}

/// Run the health gate in the background (holding the busy lock so no
/// other root action interleaves). Healthy → record; else roll back.
pub fn start_gate(e: &Executor) {
    let ex = Executor { paths: e.paths.clone(), platform: e.platform.clone(), test_keys: e.test_keys, busy: e.busy.clone(), exit_after_update: e.exit_after_update };
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

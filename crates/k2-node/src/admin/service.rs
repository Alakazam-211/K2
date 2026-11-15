//! The node's own service file, kept as this build renders it (macOS:
//! `/Library/LaunchDaemons/dev.k2.node.plist`).
//!
//! The installer writes the plist once; a remote update swaps only the
//! binary. So a change to the rendered plist (0.46.13:
//! `ProcessType=Interactive`, which stops launchd coalescing every job's
//! timers) never reached installed nodes. Now, on executor start (after an
//! update's healthy gate the executor re-execs on the new build), a plist
//! that differs from this build's rendering is rewritten and the node is
//! loaded again from it. launchd reads a plist only when the job loads, so
//! that is `bootout` + `bootstrap`: a node restart. It runs at a drained
//! moment: at once when nothing runs, otherwise after a drain that gives up
//! at its limit (nothing is restarted then; the next executor start tries
//! again). A reload that fails puts the old file back and loads it again;
//! the same rendering is not tried again (`service_refresh.json`), so a
//! bad plist never loops. Linux's unit isn't kept this way yet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::admin::{code, AdminRefusal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::Executor;
use super::store::AuditEntry;
use super::update::{self, DrainOpts};

/// The drain/audit name of a service-file refresh.
pub const WHAT: &str = "service.refresh";
/// `<state>/service_refresh.json`: the last refresh's outcome.
pub const STATE_FILE: &str = "service_refresh.json";
/// How long a reloaded node has to show a pid.
pub const UP_SECS: u64 = 30;

/// The service file this build wants: where, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFile {
    pub path: PathBuf,
    pub want: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshRecord {
    /// sha256 of the rendering this record is about.
    pub sha: String,
    /// `ok` | `failed`.
    pub result: String,
    pub at: i64,
    #[serde(default)]
    pub detail: String,
}

fn state_path(e: &Executor) -> PathBuf {
    e.paths.state_dir.join(STATE_FILE)
}

fn read_record(e: &Executor) -> Option<RefreshRecord> {
    std::fs::read(state_path(e)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_record(e: &Executor, r: &RefreshRecord) {
    if let Ok(b) = serde_json::to_vec(r) {
        if let Err(x) = crate::util::atomic_write(&state_path(e), &b, 0o600) {
            crate::nlog!("admin: {WHAT}: can't record the outcome: {x}");
        }
    }
}

/// What needs doing, if anything: the wanted file and the bytes on disk
/// now. `None`: nothing to keep on this OS, the file matches, it is
/// missing or unreadable (not installed this way: left alone), an update
/// or an owner change is under way, or this rendering already failed.
pub fn pending(e: &Executor) -> Option<(ServiceFile, Vec<u8>)> {
    let f = e.platform.node_service_file(&e.paths, &e.node_user())?;
    let have = match crate::util::read_regular(&f.path) {
        Ok(s) => s.into_bytes(),
        Err(x) => {
            crate::nlog!("admin: {WHAT}: {} unreadable ({x}); left alone", f.path.display());
            return None;
        }
    };
    if have == f.want.as_bytes() {
        return None;
    }
    let phase = update::load(e).phase;
    if phase == "swapping" || phase == "swapped" {
        return None;
    }
    if update::control_held(e) {
        return None;
    }
    let sha = k2_node_proto::crypto::sha256_hex(f.want.as_bytes());
    if read_record(e).is_some_and(|r| r.result == "failed" && r.sha == sha) {
        return None;
    }
    Some((f, have))
}

/// On executor start: refresh the node's service file when it differs,
/// in the background (the executor serves meanwhile).
pub fn refresh_on_start(e: &Executor) {
    let Some((f, old)) = pending(e) else { return };
    crate::nlog!("admin: {WHAT}: {} differs from this build's; reloading the node at a drained moment", f.path.display());
    let ex = e.bg_handle();
    std::thread::spawn(move || {
        let _ = schedule(&ex, f, old);
    });
}

/// Run the refresh now when nothing runs, or after a drain that gives up
/// at its limit.
pub fn schedule(e: &Executor, f: ServiceFile, old: Vec<u8>) -> Result<Value, AdminRefusal> {
    let decided = update::decide(e, false, true);
    let opts = DrainOpts { settle: true, give_up_at_limit: true, stop_running: false };
    let _g = (decided.0 != update::Decision::Drain).then(|| e.busy.lock().unwrap_or_else(|p| p.into_inner()));
    let r = update::run_decided(e, decided, opts, WHAT, "-", move |ex| apply(ex, &f, &old));
    if let Err(x) = &r {
        crate::nlog!("admin: {WHAT}: {}", x.message);
    }
    r
}

/// Write the wanted file and load the node from it; on failure put the
/// old file back, load that, and record the rendering as failed.
pub fn apply(e: &Executor, f: &ServiceFile, old: &[u8]) -> Result<Value, AdminRefusal> {
    let sha = k2_node_proto::crypto::sha256_hex(f.want.as_bytes());
    let fail = |e: &Executor, detail: String| -> AdminRefusal {
        let back = crate::util::atomic_write(&f.path, old, 0o644).and_then(|_| e.platform.reload_node(&f.path));
        let detail = match back {
            Ok(()) => format!("{detail}; the old file is back and loaded"),
            Err(x) => format!("{detail}; putting the old file back failed too: {x}"),
        };
        crate::nlog!("admin: {WHAT}: {detail}");
        write_record(e, &RefreshRecord { sha: sha.clone(), result: "failed".into(), at: e.now(), detail: detail.clone() });
        audit(e, Some(code::FAILED), json!({"path": f.path.display().to_string(), "phase": "failed", "detail": detail}));
        AdminRefusal::new(code::FAILED, detail)
    };
    if let Err(x) = crate::util::atomic_write(&f.path, f.want.as_bytes(), 0o644) {
        // Nothing changed on disk (the write is atomic): no reload.
        write_record(e, &RefreshRecord { sha, result: "failed".into(), at: e.now(), detail: format!("write: {x}") });
        return Err(AdminRefusal::new(code::FAILED, format!("write {}: {x}", f.path.display())));
    }
    if let Err(x) = update::restart_marked(e, WHAT, "-", |e| e.platform.reload_node(&f.path)) {
        return Err(fail(e, format!("reload: {x}")));
    }
    if !node_up(e) {
        return Err(fail(e, format!("the node didn't start within {UP_SECS} s")));
    }
    write_record(e, &RefreshRecord { sha, result: "ok".into(), at: e.now(), detail: String::new() });
    audit(e, None, json!({"path": f.path.display().to_string(), "phase": "done"}));
    crate::nlog!("admin: {WHAT}: {} rewritten and the node reloaded", f.path.display());
    Ok(json!({"phase": "done", "what": WHAT, "path": f.path.display().to_string()}))
}

fn node_up(e: &Executor) -> bool {
    let step = Duration::from_millis(if cfg!(test) { 10 } else { 500 });
    let tries = if cfg!(test) { 20 } else { UP_SECS * 2 };
    for _ in 0..tries {
        if e.platform.node_pid().is_some() {
            return true;
        }
        std::thread::sleep(step);
    }
    false
}

fn audit(e: &Executor, code: Option<&str>, detail: Value) {
    let _ = e.audit().append(AuditEntry { event: WHAT.into(), code: code.map(str::to_string), detail, ..Default::default() }, e.now());
}

/// macOS: the node's LaunchDaemon plist as this build's installer writes it.
pub fn macos_service_file(paths: &super::exec::ExecPaths, node_user: &str, at: &Path) -> ServiceFile {
    let s = |p: &Path| p.display().to_string();
    let (binary, home, config) = (s(&paths.binary), s(&paths.home), s(&paths.config_dir));
    let want = crate::install::render_plist(&crate::install::InstallParams { binary: &binary, home: &home, config: &config, user: node_user, group: node_user });
    ServiceFile { path: at.to_path_buf(), want }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::tests::Rig;

    /// A rig whose platform keeps a service file in its temp dir.
    fn rig_with(have: &str) -> (Rig, PathBuf) {
        let r = Rig::new("policy");
        let path = r.dir.join("LaunchDaemons/dev.k2.node.plist");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, have).unwrap();
        *r.plat.service.lock().unwrap() = Some(macos_service_file(&r.exec.paths, &r.exec.node_user(), &path));
        (r, path)
    }

    fn want(r: &Rig) -> String {
        r.plat.service.lock().unwrap().clone().unwrap().want
    }

    fn reloads(r: &Rig) -> usize {
        r.plat.calls.lock().unwrap().iter().filter(|c| c.starts_with("reload-node")).count()
    }

    #[test]
    fn the_rendering_is_the_installers_plist_with_processtype_interactive() {
        let (r, _) = rig_with("");
        let w = want(&r);
        assert!(w.contains("<key>ProcessType</key>\n\t<string>Interactive</string>"), "{w}");
        assert!(w.contains(&format!("<string>{}</string>", r.exec.paths.binary.display())));
    }

    #[test]
    fn a_matching_plist_is_left_alone() {
        let (r, path) = rig_with("");
        std::fs::write(&path, want(&r)).unwrap();
        assert!(pending(&r.exec).is_none());
        refresh_on_start(&r.exec);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(reloads(&r), 0);
    }

    #[test]
    fn an_old_plist_is_rewritten_and_the_node_reloaded_when_nothing_runs() {
        let old = "<plist>old, no ProcessType</plist>\n";
        let (r, path) = rig_with(old);
        let (f, have) = pending(&r.exec).expect("differs");
        assert_eq!(have, old.as_bytes());
        let out = schedule(&r.exec, f, have).unwrap();
        assert_eq!(out["phase"], "done");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), want(&r));
        assert_eq!(reloads(&r), 1);
        assert!(r.plat.calls.lock().unwrap().iter().any(|c| c == &format!("reload-node {}", path.display())));
        assert_eq!(read_record(&r.exec).unwrap().result, "ok");
        assert!(pending(&r.exec).is_none(), "done: nothing left");
    }

    #[test]
    fn a_failed_reload_puts_the_old_plist_back_and_is_not_retried() {
        let old = "<plist>old</plist>\n";
        let (r, path) = rig_with(old);
        *r.plat.fail_reload.lock().unwrap() = Some(1);
        let (f, have) = pending(&r.exec).unwrap();
        let e = schedule(&r.exec, f, have).unwrap_err();
        assert!(e.message.contains("the old file is back and loaded"), "{}", e.message);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert_eq!(reloads(&r), 2, "the new file, then the old one again");
        assert_eq!(read_record(&r.exec).unwrap().result, "failed");
        assert!(pending(&r.exec).is_none(), "the same rendering is not tried again");
    }

    #[test]
    fn running_jobs_drain_first_and_a_drain_that_hits_its_limit_restarts_nothing() {
        let old = "<plist>old</plist>\n";
        let (mut r, path) = rig_with(old);
        r.exec.drain_max = Duration::from_millis(150);
        // The node reports one running job (its status file).
        let st = r.exec.paths.layout().status();
        std::fs::create_dir_all(st.parent().unwrap()).unwrap();
        std::fs::write(&st, r#"{"running":["j1"]}"#).unwrap();
        assert_eq!(update::running_jobs(&r.exec), 1, "the rig's status file counts one job");
        let (f, have) = pending(&r.exec).unwrap();
        let out = schedule(&r.exec, f, have).unwrap();
        assert_eq!(out["phase"], "draining");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while r.exec.draining.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(reloads(&r), 0, "a job still runs at the limit: nothing reloaded");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert!(pending(&r.exec).is_some(), "tried again at the next executor start");
    }

    #[test]
    fn no_service_file_on_this_os_means_nothing_to_do() {
        let r = Rig::new("policy");
        assert!(pending(&r.exec).is_none());
    }
}

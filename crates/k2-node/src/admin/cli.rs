//! `k2-node admin …` (prd-k2-node-admin-v1 §13.2, "at the node machine").

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use k2_node_proto::admin::Level;
use k2_node_proto::crypto;

use super::config::{self, AdminFile};
use super::exec::{ExecPaths, Executor, LocalRequest, LocalResponse, Platform, SystemPlatform};
use super::server;
use super::units::{self, AdminUnitParams};

fn need_root(what: &str) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(format!("{what} runs as root at this machine (sudo k2-node admin …)"));
    }
    Ok(())
}

fn platform() -> Arc<dyn Platform> {
    #[cfg(feature = "test-keys")]
    if let Some(h) = std::env::var_os("K2_NODE_ADMIN_TEST_HOOK") {
        return Arc::new(super::exec::HookPlatform(PathBuf::from(h)));
    }
    Arc::new(SystemPlatform)
}

/// `admin serve` (root; started by systemd/launchd).
pub fn serve(paths: ExecPaths, socket: &Path) -> Result<(), String> {
    need_root("admin serve")?;
    super::exec::ensure_state_dir(&paths.state_dir)?;
    let node_user = config::read(&paths.config_dir.join(config::ADMIN_FILE))
        .ok()
        .flatten()
        .map(|c| c.node_user)
        .unwrap_or_else(|| config::default_node_user().to_string());
    let rule = server::PeerRule::for_node_user(&node_user);
    let gid = server::user_ids(&node_user).map(|(_, g)| g);
    let l = server::listener(socket, gid)?;
    let mut exec = Executor::new(paths, platform());
    exec.exit_after_update = true;
    // A health gate that was running when the executor died resumes.
    super::update::resume_pending(&exec);
    // A drain the last executor left behind never keeps the owner's
    // control at `draining` (B-M2).
    super::update::recover_drain(&exec);
    // TODO(0.46.5): drop with the legacy mark read.
    super::update::remove_legacy_mark(&exec.paths.config_dir);
    crate::nlog!("admin executor {} serving (peers: root, {node_user})", crate::PRODUCT_VERSION);
    let reexec = exec.reexec.clone();
    server::serve(&exec, &rule, l, &|| reexec.load(Ordering::SeqCst))?;
    crate::nlog!("admin executor: exiting so the service manager starts the updated binary");
    Ok(())
}

/// The controller key from the node's own pin, as root sees it now (the
/// person at the machine is confirming it by running this).
fn controller_from_node_pin(home: &Path) -> Result<Option<(String, Vec<u8>)>, String> {
    let layout = crate::paths::Layout::new(home, "/nonexistent");
    match crate::identity::read_pin(&layout.pin())? {
        Some(p) if !p.revoked => Ok(Some((p.controller_fp.clone(), p.controller_spki()?))),
        _ => Ok(None),
    }
}

/// `admin init` (installer): create admin.toml when missing, set the
/// level when given, pin the controller when the node is enrolled.
pub fn init(config_dir: &Path, home: &Path, level: Option<Level>, auto_update: Option<&str>) -> Result<String, String> {
    need_root("admin init")?;
    let path = config_dir.join(config::ADMIN_FILE);
    let mut f = match config::read(&path)? {
        Some(c) => c.file,
        None => AdminFile::default(),
    };
    if let Some(l) = level {
        f.remote_admin = l.as_str().into();
    }
    if let Some(a) = auto_update {
        f.auto_update = a.to_string();
    }
    if let Some((fp, der)) = controller_from_node_pin(home)? {
        f.controller_fp = Some(fp);
        f.controller_key = Some(crypto::b64(&der));
    }
    config::write(&path, &f)?;
    Ok(format!(
        "remote admin: {}; controller: {}; approval keys: {}; auto update: {}",
        f.remote_admin,
        f.controller_fp.as_deref().map(|s| &s[..16]).unwrap_or("not pinned yet (enroll, then sudo k2-node admin pin)"),
        f.approval_keys.len(),
        f.auto_update
    ))
}

/// `admin pin`: copy the enrolled controller's key into root's admin.toml.
pub fn pin(config_dir: &Path, home: &Path) -> Result<String, String> {
    need_root("admin pin")?;
    let path = config_dir.join(config::ADMIN_FILE);
    let mut f = config::read(&path)?.map(|c| c.file).unwrap_or_default();
    let (fp, der) = controller_from_node_pin(home)?.ok_or("this node is not enrolled (k2-node status); enroll first")?;
    if f.controller_fp.as_deref().is_some_and(|old| old != fp) {
        // A new controller: the old approval keys belonged to the old one.
        f.approval_keys.clear();
    }
    f.controller_fp = Some(fp.clone());
    f.controller_key = Some(crypto::b64(&der));
    config::write(&path, &f)?;
    Ok(format!("pinned controller {fp}"))
}

/// `admin level off|policy|full` (raising is only possible here).
pub fn level(config_dir: &Path, l: Level) -> Result<String, String> {
    need_root("admin level")?;
    let path = config_dir.join(config::ADMIN_FILE);
    let mut f = config::read(&path)?.map(|c| c.file).unwrap_or_default();
    let was = f.remote_admin.clone();
    f.remote_admin = l.as_str().into();
    config::write(&path, &f)?;
    let _ = super::store::Audit::new(&config::default_state_dir().join("audit.log")).append(
        super::store::AuditEntry { event: "level".into(), detail: serde_json::json!({"from": was, "to": l.as_str(), "by": "local"}), ..Default::default() },
        crate::util::now(),
    );
    Ok(format!("remote admin: {was} → {}", l.as_str()))
}

/// `admin audit`: check the chain, print the tail.
pub fn audit(state_dir: &Path, tail: usize, json: bool) -> Result<(), String> {
    let lines = super::store::Audit::new(&state_dir.join("audit.log")).verify()?;
    let start = lines.len().saturating_sub(tail);
    for l in &lines[start..] {
        if json {
            println!("{}", serde_json::to_string(l).map_err(|e| e.to_string())?);
        } else {
            println!(
                "{:>5} {} {:<8} {} {} {}",
                l.seq,
                l.at,
                l.event,
                l.verb.as_deref().unwrap_or("-"),
                l.cmd_id.as_deref().unwrap_or("-"),
                l.code.as_deref().unwrap_or("")
            );
        }
    }
    eprintln!("audit chain ok ({} lines)", lines.len());
    Ok(())
}

/// `admin status`: what the executor says about itself.
pub fn status(socket: &Path) -> Result<(), String> {
    match server::request(socket, &LocalRequest::Status, None, Duration::from_secs(5)) {
        Ok(LocalResponse::Status(s)) => {
            println!("{}", serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?);
            Ok(())
        }
        Ok(other) => Err(format!("unexpected answer: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// `admin install-files`: render the executor's service files (installers).
pub fn install_files(os: &str, p: &AdminUnitParams<'_>, out: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let files: Vec<(&str, String)> = match os {
        "linux" => vec![(units::LINUX_SOCKET_UNIT, units::render_socket_unit(p)), (units::LINUX_SERVICE_UNIT, units::render_service_unit(p))],
        "macos" => vec![("dev.k2.node-admin.plist", units::render_plist(p))],
        o => return Err(format!("--os {o:?}: use linux or macos")),
    };
    let mut written = Vec::new();
    for (n, b) in files {
        let path = out.join(n);
        std::fs::write(&path, b).map_err(|e| format!("write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

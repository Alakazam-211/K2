//! The daemon unit's hand-off drop-in (runtime stage H, vs-live AR3/K37)
//! and the `/cli/daemon/handoff*` routes.
//!
//! The drop-in (`k2_root_proto::templates::handoff_dropin`) gives the
//! systemd unit `KillMode=process` (frpc, published services and skin
//! helpers survive a stop, restart or crash and are re-adopted),
//! `NotifyAccess=main` (a hand-off can move the unit's main pid),
//! `ExecReload=` SIGUSR2 (`systemctl reload` = a no-downtime hand-off) and
//! `OOMPolicy=continue` (an OOM-killed agent in the unit's cgroup no longer
//! makes systemd stop and restart the whole daemon) and, since 0.46.4,
//! `RuntimeDirectory=k2` (0700, preserved): the runtime dir systemd-logind
//! can't delete (`k2_core::runtime_dir`; from the unit's next start).
//!
//! - **System unit** (provisioned boxes): through the root helper's
//!   `daemon handoff-dropin install` (protocol 5). The unit comes from the
//!   helper's `daemon.toml`, never from us.
//! - **User unit** (Arch package, `k2 daemon install`): the daemon writes
//!   `~/.config/systemd/user/<unit>.d/k2-handoff.conf` itself and runs
//!   `systemctl --user daemon-reload` — no root.
//!
//! [`ensure_on_boot`] converges it once per boot (Linux, under systemd;
//! `K2_HANDOFF_DROPIN=off` opts out). A changed drop-in takes effect for
//! the unit's next stop; `NotifyAccess` applies at once, so the very next
//! hand-off works without a restart (the notify socket is the manager's
//! well-known one).

use serde_json::{json, Value};

use k2_core::log_debug;
use k2_root_proto::templates::handoff_dropin;
use k2_root_proto::version::HANDOFF_MIN_PROTOCOL;

/// Opt-out for the boot-time convergence.
pub const OPT_OUT_ENV: &str = "K2_HANDOFF_DROPIN";

fn under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

/// `~/.config/systemd/user/<unit>.d/k2-handoff.conf`.
fn user_dropin_path(unit: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::config_dir()?
            .join("systemd")
            .join("user")
            .join(format!("{unit}.d"))
            .join(k2_root_proto::paths::HANDOFF_DROPIN_FILE),
    )
}

/// The drop-in path for `unit` in `scope` ("user" or "system").
fn dropin_path(unit: &str, scope: &str) -> Option<std::path::PathBuf> {
    if scope == "user" {
        user_dropin_path(unit)
    } else {
        Some(
            std::path::Path::new(k2_root_proto::paths::SYSTEMD_SYSTEM_DIR)
                .join(format!("{unit}.d"))
                .join(k2_root_proto::paths::HANDOFF_DROPIN_FILE),
        )
    }
}

/// Whether the file at `path` holds exactly this build's drop-in bytes.
fn holds_current_dropin(path: Option<&std::path::Path>) -> bool {
    path.and_then(|p| std::fs::read(p).ok())
        .is_some_and(|b| b == handoff_dropin().as_bytes())
}

/// Where the drop-in for this daemon's unit lives, and whether it holds
/// the canned bytes.
pub fn dropin_state() -> Value {
    if !cfg!(target_os = "linux") || !under_systemd() {
        return json!({ "applies": false, "why": "not a systemd unit" });
    }
    let (_user, unit, scope) = crate::root_helper::own_unit();
    let path = dropin_path(&unit, &scope);
    let current = holds_current_dropin(path.as_deref());
    json!({
        "applies": true,
        "unit": unit,
        "scope": scope,
        "path": path.map(|p| p.display().to_string()),
        "installed": current,
    })
}

/// Install (or re-assert) the drop-in. `Ok(changed)`.
pub fn install() -> Result<bool, String> {
    if !cfg!(target_os = "linux") {
        return Err("the hand-off drop-in is for systemd units (Linux)".into());
    }
    if !under_systemd() {
        return Err("this daemon is not running under systemd".into());
    }
    let (_user, unit, scope) = crate::root_helper::own_unit();
    if scope == "user" {
        let path = user_dropin_path(&unit).ok_or("no config dir for the user unit")?;
        let dir = path.parent().ok_or("drop-in path has no parent")?;
        if std::fs::read(&path).ok().as_deref() == Some(handoff_dropin().as_bytes()) {
            return Ok(false);
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        let tmp = dir.join(format!(".k2-handoff.conf.tmp.{}", std::process::id()));
        std::fs::write(&tmp, handoff_dropin()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        let st = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()
            .map_err(|e| format!("systemctl --user daemon-reload: {e}"))?;
        if !st.success() {
            return Err(format!("systemctl --user daemon-reload exited {st}"));
        }
        return Ok(true);
    }
    let st = crate::root_helper::state();
    if !st.usable(HANDOFF_MIN_PROTOCOL) {
        return Err(crate::root_helper::helper_needed(
            "the daemon unit's hand-off drop-in",
            HANDOFF_MIN_PROTOCOL,
            &st,
        ));
    }
    let out = crate::root_helper::call_text(
        &[
            "daemon".to_string(),
            "handoff-dropin".to_string(),
            "install".to_string(),
        ],
        None,
    )?;
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("root helper answer: {e}"))?;
    Ok(v["changed"].as_bool().unwrap_or(false))
}

/// Boot: converge the drop-in once, in the background. Never fails boot.
pub fn ensure_on_boot() {
    if !cfg!(target_os = "linux") || !under_systemd() {
        return;
    }
    if std::env::var(OPT_OUT_ENV).map(|v| v == "off" || v == "0").unwrap_or(false) {
        log_debug!("[daemon/handoff] {OPT_OUT_ENV}=off — not touching the unit's drop-in");
        return;
    }
    // Retries cover a box whose root helper is still protocol 4 at boot:
    // the helper auto-heal installs this version's helper ~2 min in. They
    // also cover an older helper that answers but renders its own (older)
    // drop-in, e.g. a 0.46.0 helper without `OOMPolicy=continue`: the
    // system unit counts as done only once the file holds this build's
    // bytes, which it does after the helper self-updates.
    std::thread::Builder::new()
        .name("k2-handoff-dropin".into())
        .spawn(|| {
            for wait in [0u64, 240, 900] {
                std::thread::sleep(std::time::Duration::from_secs(wait));
                if k2_core::handoff_state::draining() {
                    return;
                }
                match install() {
                    Ok(changed) => {
                        let (_user, unit, scope) = crate::root_helper::own_unit();
                        if !holds_current_dropin(dropin_path(&unit, &scope).as_deref()) {
                            log_debug!(
                                "[daemon/handoff] unit drop-in differs from this build's \
                                 (older root helper?); retrying after the helper updates"
                            );
                            continue;
                        }
                        if changed {
                            log_debug!(
                                "[daemon/handoff] unit drop-in installed (KillMode=process, \
                                 NotifyAccess=main, ExecReload=SIGUSR2, OOMPolicy=continue): \
                                 frpc and Apps survive a daemon restart, and an OOM-killed \
                                 agent no longer stops the daemon"
                            );
                        }
                        return;
                    }
                    Err(e) => log_debug!("[daemon/handoff] unit drop-in not installed (yet): {e}"),
                }
            }
        })
        .ok();
}

/// `GET /cli/daemon/handoff/status`.
pub fn status_json() -> Value {
    let capable = crate::handoff::capable();
    json!({
        "capable": capable.is_ok(),
        "why": capable.err(),
        "handedOff": crate::handoff::handed_off(),
        "bootedFromHandoff": crate::handoff::booted_from_handoff(),
        "liveSplices": crate::handoff::live_splices(),
        "dropin": dropin_state(),
    })
}

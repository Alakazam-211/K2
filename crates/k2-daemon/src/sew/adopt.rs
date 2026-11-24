//! `k2 hostchat adopt` (`POST /cli/sew/server/adopt`, the owner): take a
//! Sew that systemd and Sew itself report healthy back from an `error`
//! record, which before only an enable cleared.

use serde_json::{json, Value};

use k2_root_proto::sew::{UNIT_DB, UNIT_SEW};

use super::store;
use super::supervisor::{
    begin, box_status, changed, helper_check, mgmt, need_row, no, on_disk, on_disk_upgrade_only, status_body, unsupported,
    Env, R,
};
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::{cli_name, product_name};

/// `POST /cli/sew/server/adopt` (the owner): take a Sew that systemd and
/// Sew itself report healthy back from an `error` record. Only an enable
/// cleared `error`, and enable refuses a box whose binary is not this
/// build's pin; this step records what is on the box instead. It runs only
/// on an `error` record, with both units active, Sew's mgmt health OK and
/// the binary on disk readable. It writes the installed version (the one on
/// disk), the live verdict (running / degraded) and clears `last_error`.
/// It never installs, migrates, restarts or touches data.
pub fn adopt(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(unsupported());
    }
    let row = need_row(false)?;
    if row.status != "error" {
        return Err(no(
            "not_in_error",
            409,
            format!(
                "nothing changed: adopt only clears an error record, and this one says '{}' ({CLI_NAME} hostchat status)",
                row.status
            ),
        ));
    }
    helper_check(env)?;
    let _l = begin(env)?;
    let down: Vec<String> = [UNIT_DB, UNIT_SEW]
        .iter()
        .filter_map(|u| {
            let i = env.ops.unit(u);
            (!i.is_active()).then(|| format!("{u} is '{}'", i.active))
        })
        .collect();
    if !down.is_empty() {
        return Err(no(
            "units_not_active",
            409,
            format!(
                "nothing changed: systemd reports {}. Bring Sew back first ({CLI_NAME} hostchat restart), or run \
                 {CLI_NAME} hostchat enable to repair the install",
                down.join(" and ")
            ),
        ));
    }
    if !mgmt(env, "GET", "/mgmt/v1/health", None).is_ok_and(|v| v["ok"] == true) {
        return Err(no(
            "sew_unhealthy",
            409,
            concat!("nothing changed: Sew's units run, but its mgmt health check does not answer OK. \
             ", cli_name!(), " hostchat doctor shows why; ", cli_name!(), " hostchat restart, then adopt again"),
        ));
    }
    let st = box_status(env).ok_or_else(|| no("helper_error", 500, "engine sew status failed"))?;
    let disk = match on_disk(&st) {
        (true, Some(v), _) => v,
        (true, None, _) => on_disk_upgrade_only(&st).map(|(v, _)| v).ok_or_else(|| {
            no(
                "binary_unreadable",
                409,
                concat!("nothing changed: ", product_name!(), " can't read the version of /usr/local/bin/sew (not a build ", product_name!(), " knows, or not \
                 root-owned). ", cli_name!(), " hostchat upgrade names it; enable or upgrade repairs it"),
            )
        })?,
        (false, ..) => {
            return Err(no(
                "binary_unreadable",
                409,
                concat!("nothing changed: /usr/local/bin/sew is missing, so there is nothing to adopt (", cli_name!(), " hostchat enable)"),
            ))
        }
    };
    let was = (row.installed_version.clone(), row.last_error.clone());
    // Judge it as a running record would be judged (the same verdict the
    // health loop uses), then keep running or degraded only.
    store::update(env.ops.now(), |r| {
        r.status = "running".into();
        r.installed_version = Some(disk.clone());
        r.last_error = None;
    })
    .map_err(|e| no("db_error", 500, e))?;
    let live = status_body(env, false);
    let state = live["state"].as_str().unwrap_or("").to_string();
    if !matches!(state.as_str(), "running" | "degraded") {
        // It went down between the checks and the verdict: put it back.
        let _ = store::update(env.ops.now(), |r| {
            r.status = row.status.clone();
            r.installed_version = was.0.clone();
            r.last_error = was.1.clone();
        });
        return Err(no(
            "units_not_active",
            409,
            format!("nothing changed: Sew stopped while {PRODUCT_NAME} checked it ({state}). {CLI_NAME} hostchat status"),
        ));
    }
    store::update(env.ops.now(), |r| r.status = state.clone()).map_err(|e| no("db_error", 500, e))?;
    changed("state");
    Ok(json!({
        "ok": true,
        "state": state,
        "installedVersion": {"from": was.0, "to": disk},
        "clearedError": was.1,
        "alarms": live["alarms"],
    }))
}

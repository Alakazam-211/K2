//! `/cli/sew/*` — the Sew server on this box (S29 §20, re-gated per
//! plan-sew-s30-s32 K19).
//!
//! Who may do what:
//! - **Reads** (`status`, `preflight`, `backups`, `job`): any credential
//!   except app passes / API keys (`Who::Wrong`).
//! - **Manage Sew** (enable, disable, uninstall, upgrade, rollback, backup
//!   run, restore, doctor, rotate-master start): the owner token, an
//!   Owner/Admin Connect login, or an agent passport whose workspace has
//!   "Manage Sew" (`chat_manage`) on. A K2 shell tab counts as agent-level
//!   here (plan S30 amend 1): it needs the switch too. Turning the switch on
//!   is `/cli/agent-access/set` (Owner/Admin), never an agent verb.
//! - **Person-only** (purge, the recovery key, `rotate-master drop-prev`):
//!   the owner (token or Owner login) AND a fresh vault confirm bound to
//!   this install (`confirmToken` from `/cli/vault/confirm/finish`, action
//!   `sew-purge` / `sew-recovery-key` / `sew-drop-prev`, `secretId` = the
//!   install id or `none`). Never a passport, even with Manage Sew on. On
//!   boxes where agents have root this is evidence, not prevention (KM13
//!   §19a).
//!
//! Route floors (Connect logins) live in `route_policy::ROUTES`; every
//! mutation is POST-only (405 on GET before auth).

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::connect_users::Role;
use serde_json::{json, Value};

use super::supervisor::{self as sup, no, Env, Fail, R};
use crate::cli_response::CliResponse;
pub use crate::compute_routes::Who;

pub const MAX_BODY: usize = 64 * 1024;

pub const ROUTES: &[&str] = &[
    "/cli/sew/backups",
    "/cli/sew/job",
    "/cli/sew/preflight",
    "/cli/sew/server/backup",
    "/cli/sew/server/disable",
    "/cli/sew/server/doctor",
    "/cli/sew/server/enable",
    "/cli/sew/server/recovery-key",
    "/cli/sew/server/restore",
    "/cli/sew/server/rollback",
    "/cli/sew/server/rotate-master",
    "/cli/sew/server/uninstall",
    "/cli/sew/server/upgrade",
    "/cli/sew/status",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/sew/server/backup",
    "/cli/sew/server/disable",
    "/cli/sew/server/doctor",
    "/cli/sew/server/enable",
    "/cli/sew/server/recovery-key",
    "/cli/sew/server/restore",
    "/cli/sew/server/rollback",
    "/cli/sew/server/rotate-master",
    "/cli/sew/server/uninstall",
    "/cli/sew/server/upgrade",
];

/// Vault confirm actions (`vault_routes::CONFIRM_ACTIONS`).
pub const CONFIRM_PURGE: &str = "sew-purge";
pub const CONFIRM_RECOVERY: &str = "sew-recovery-key";
pub const CONFIRM_DROP_PREV: &str = "sew-drop-prev";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── responses ────────────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        202 => "202 Accepted",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

pub fn refusal(f: &Fail) -> CliResponse {
    let mut err = json!({"code": f.code, "hint": f.hint});
    if let Some(Value::Object(m)) = &f.extra {
        for (k, v) in m {
            err[k] = v.clone();
        }
    }
    CliResponse { status: status_line(f.status), content_type: "application/json", body: json!({"ok": false, "error": err}).to_string() }
}

fn ok(v: Value) -> CliResponse {
    CliResponse { status: "200 OK", content_type: "application/json", body: v.to_string() }
}

pub fn too_large() -> CliResponse {
    refusal(&no("bad_request", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)))
}

// ── gates ────────────────────────────────────────────────────────────────

fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Err(no("wrong_credential", 403, "App passes, API keys and API sessions can't manage Sew")),
        _ => Ok(()),
    }
}

fn owner_only() -> Fail {
    no(
        "owner_only",
        403,
        "only the server's owner or an admin can do this (or an agent whose workspace has \"Manage Sew\" on)",
    )
}

/// The Manage Sew gate (CM): owner / admin, or a passport whose workspace
/// has `chat_manage` on.
pub fn need_manage(who: &Who) -> R<()> {
    need_credential(who)?;
    match who {
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Who::Login { .. } => Err(owner_only()),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            if k2_core::workspace::settings::agents_can_manage_chat(project_id) {
                Ok(())
            } else {
                Err(no(
                    "chat_manage_off",
                    403,
                    "\"Manage Sew\" is off for this workspace: ask your owner or an admin to run \
                     k2 agent access set <workspace> chat_manage on",
                ))
            }
        }
        _ => Err(owner_only()),
    }
}

/// Person-only: the owner AND a fresh vault confirm for `action` on this
/// install. Returns the confirm method (audit).
pub fn need_person(who: &Who, body: &Value, action: &str, secret_id: &str) -> R<String> {
    need_credential(who)?;
    if who.passport().is_some() {
        return Err(no(
            "person_only",
            403,
            "a person does this, never an agent or a K2 terminal (Manage Sew does not cover it): Settings → Sew, or \
             k2 hostchat from a terminal outside K2",
        ));
    }
    if !who.is_owner() {
        return Err(no("owner_only", 403, "only the server's owner does this, with a confirm"));
    }
    let token = body
        .get("confirmToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| no("owner_confirm_required", 403, "this needs a fresh owner confirm (Touch ID, your login or the vault password)"))?;
    let stamp = match who {
        Who::Owner => "owner".to_string(),
        Who::Login { username, .. } => format!("connect:{username}"),
        _ => return Err(owner_only()),
    };
    let vault = vault().map_err(|e| no("vault_closed", 503, e))?;
    let proof = vault
        .confirm
        .take(token, action, Some(secret_id), &stamp)
        .map_err(|_| no("owner_confirm_required", 403, "confirm again: a confirm works once, for this install, within 60 seconds"))?;
    match proof {
        k2_vault::HumanProof::Confirmed { method } => Ok(method),
        k2_vault::HumanProof::None => Err(no("owner_confirm_required", 403, "no owner confirm")),
    }
}

fn vault() -> Result<std::sync::Arc<k2_vault::LocalVault>, String> {
    #[cfg(test)]
    return Ok(crate::vault::ensure_test_vault());
    #[cfg(not(test))]
    crate::vault::get().map_err(|e| format!("the vault isn't open ({}): {}", e.code, e.hint))
}

/// The id a person confirm is bound to.
pub fn confirm_secret_id() -> String {
    super::store::load().and_then(|r| r.install_id).unwrap_or_else(|| "none".into())
}

// ── bodies ───────────────────────────────────────────────────────────────

fn json_body(body: &[u8]) -> R<Value> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| no("bad_request", 400, format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(no("bad_request", 400, "expected a JSON object"));
    }
    Ok(v)
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn abs_path(v: &Value, k: &str) -> R<Option<PathBuf>> {
    match s(v, k) {
        None => Ok(None),
        Some(p) if p.starts_with('/') && !p.split('/').any(|seg| seg == "..") => Ok(Some(PathBuf::from(p))),
        Some(_) => Err(no("bad_request", 400, format!("{k} must be an absolute path"))),
    }
}

// ── entry ────────────────────────────────────────────────────────────────

/// Route a Sew request (blocking). Machines run on a background thread.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    handle_env(sup::live(), path, &who, body, params, true)
}

/// [`handle`] over any environment. `background = false` runs the enable /
/// upgrade machines inline (tests).
pub fn handle_env(env: Env<'static>, path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, background: bool) -> CliResponse {
    match route(env, path, who, body, params, background) {
        Ok(v) => ok(v),
        Err(f) => refusal(&f),
    }
}

fn route(env: Env<'static>, path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, background: bool) -> R<Value> {
    match path {
        "/cli/sew/status" => {
            need_credential(who)?;
            Ok(sup::status(&env, params.get("health").is_some_and(|h| h == "1" || h == "true")))
        }
        "/cli/sew/preflight" => {
            need_credential(who)?;
            let req = sup::EnableRequest {
                tarball_path: params.get("tarballPath").filter(|p| p.starts_with('/')).map(PathBuf::from),
                recovery_key_file: None,
            };
            let checks = sup::preflight(&env, &req);
            let ready = !checks.iter().any(|c| c["status"] == "fail");
            Ok(json!({"ok": true, "ready": ready, "checks": checks}))
        }
        "/cli/sew/backups" => {
            need_credential(who)?;
            sup::backups(&env)
        }
        "/cli/sew/job" => {
            need_credential(who)?;
            sup::job(&env, params.get("id").map(String::as_str).unwrap_or(""))
        }
        "/cli/sew/server/enable" => {
            need_manage(who)?;
            let v = json_body(body)?;
            let req = sup::EnableRequest { tarball_path: abs_path(&v, "tarballPath")?, recovery_key_file: abs_path(&v, "recoveryKeyFile")? };
            match sup::enable_begin(&env, &req)? {
                Err(done) => Ok(done),
                Ok(latch) if background => {
                    std::thread::Builder::new()
                        .name("sew-enable".into())
                        .spawn(move || {
                            let _l = latch;
                            if let Err(e) = sup::enable_machine(&env, &req) {
                                k2_core::log_debug!("[sew] enable: {e}");
                            }
                        })
                        .map_err(|e| no("spawn", 500, e.to_string()))?;
                    Ok(json!({"ok": true, "state": "installing", "hint": "poll GET /cli/sew/status (k2 hostchat status)"}))
                }
                Ok(latch) => {
                    let r = sup::enable_machine(&env, &req);
                    drop(latch);
                    match r {
                        Ok(()) => Ok(json!({"ok": true, "state": "running"})),
                        Err(e) => Err(no("enable_failed", 500, e)),
                    }
                }
            }
        }
        "/cli/sew/server/disable" => {
            need_manage(who)?;
            sup::disable(&env)
        }
        "/cli/sew/server/uninstall" => {
            let v = json_body(body)?;
            if b(&v, "purge") {
                // Person-only, and the hostname typed back.
                let method = need_person(who, &v, CONFIRM_PURGE, &confirm_secret_id())?;
                let host = s(&v, "confirmHostname").ok_or_else(|| no("bad_request", 400, "purge needs confirmHostname"))?;
                let mut r = sup::purge(&env, host, b(&v, "keepBackups"))?;
                r["confirmedBy"] = json!(method);
                Ok(r)
            } else {
                need_manage(who)?;
                sup::uninstall(&env, b(&v, "dropGrants"))
            }
        }
        "/cli/sew/server/upgrade" => {
            need_manage(who)?;
            let v = json_body(body)?;
            let req = sup::UpgradeRequest { tarball_path: abs_path(&v, "tarballPath")?, dry_run: b(&v, "dryRun") };
            match sup::upgrade_begin(&env, &req)? {
                Err(done) => Ok(done),
                Ok((latch, from)) if background => {
                    std::thread::Builder::new()
                        .name("sew-upgrade".into())
                        .spawn(move || {
                            let _l = latch;
                            let _ = sup::upgrade_machine(&env, &req, &from);
                        })
                        .map_err(|e| no("spawn", 500, e.to_string()))?;
                    Ok(json!({"ok": true, "state": "upgrading", "hint": "poll GET /cli/sew/status (upgradeProgress)"}))
                }
                Ok((latch, from)) => {
                    let p = sup::upgrade_machine(&env, &req, &from);
                    drop(latch);
                    Ok(json!({"ok": p["result"] == "done", "upgrade": p}))
                }
            }
        }
        "/cli/sew/server/rollback" => {
            need_manage(who)?;
            sup::rollback(&env)
        }
        "/cli/sew/server/backup" => {
            need_manage(who)?;
            sup::backup_run(&env)
        }
        "/cli/sew/server/restore" => {
            need_manage(who)?;
            let v = json_body(body)?;
            if !b(&v, "confirm") {
                return Err(no(
                    "confirm_required",
                    400,
                    "restore replaces ALL Sew data with that backup (Sew takes a safety backup first): pass --confirm",
                ));
            }
            let id = s(&v, "bundleId").ok_or_else(|| no("bad_request", 400, "name the backup (bundleId)"))?;
            sup::restore(&env, id)
        }
        "/cli/sew/server/doctor" => {
            need_manage(who)?;
            sup::doctor(&env)
        }
        "/cli/sew/server/rotate-master" => {
            let v = json_body(body)?;
            match s(&v, "phase").unwrap_or("start") {
                "start" => {
                    need_manage(who)?;
                    sup::rotate_start(&env)
                }
                "drop-prev" | "finish" => {
                    let method = need_person(who, &v, CONFIRM_DROP_PREV, &confirm_secret_id())?;
                    let mut r = sup::rotate_drop_prev(&env)?;
                    r["confirmedBy"] = json!(method);
                    Ok(r)
                }
                _ => Err(no("bad_request", 400, "phase is start or drop-prev")),
            }
        }
        "/cli/sew/server/recovery-key" => {
            let v = json_body(body)?;
            let method = need_person(who, &v, CONFIRM_RECOVERY, &confirm_secret_id())?;
            let r = sup::recovery_key(&env)?;
            crate::vault::auth_audit("sew-recovery-key", &who.label(), &format!("shown via {method}"), "cli");
            Ok(r)
        }
        _ => Err(no("not_found", 404, "no such Sew route")),
    }
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;

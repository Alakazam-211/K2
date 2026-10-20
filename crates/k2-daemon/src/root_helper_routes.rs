//! `/cli/root-helper/*` (`prd-k2-root-helper-v1` §8.3).
//!
//! - `GET  /cli/root-helper/status` — Member floor; agents may read it
//!   (`is_agent_verb`). `?events=1` adds the helper's last journal lines
//!   (`helper audit-tail`).
//! - `POST /cli/root-helper/update` — Admin; passports refused. Runs the
//!   auto-heal now. Body `{"fromDir": "<dir>"}` installs from local signed
//!   assets (air-gap) — the signature still gates it, no root needed.
//! - `POST /cli/root-helper/rollback` — Admin; passports refused.
//!
//! `setup` (the box-sudo install button) is a later slice with the
//! Settings card.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::root_helper::{self, heal};
use crate::sidecar_routes::Caller;

pub const STATUS: &str = "/cli/root-helper/status";
pub const UPDATE: &str = "/cli/root-helper/update";
pub const ROLLBACK: &str = "/cli/root-helper/rollback";

pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    matches!(path, STATUS | UPDATE | ROLLBACK)
}

pub fn is_post_route(path: &str) -> bool {
    matches!(path, UPDATE | ROLLBACK)
}

fn refuse(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({"error": {"code": code, "hint": hint.into()}}).to_string(),
    }
}

pub fn too_large() -> CliResponse {
    refuse(
        "413 Payload Too Large",
        "body_too_large",
        "request bodies here are at most 4 KiB",
    )
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn human(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Owner => Some("owner".into()),
        Caller::Login { username } => Some(username.clone()),
        _ => None,
    }
}

pub fn handle(
    path: &str,
    caller: Caller,
    body: &[u8],
    params: &HashMap<String, String>,
) -> CliResponse {
    if matches!(caller, Caller::Invalid | Caller::WrongCredential) {
        return refuse(
            "401 Unauthorized",
            "unauthorized",
            "owner token, a Connect login or a K2 passport",
        );
    }
    match path {
        STATUS => {
            if params.get("refresh").map(String::as_str) == Some("1") {
                root_helper::invalidate();
            }
            let st = root_helper::state();
            let legacy_mail = crate::mail::supervisor::mail_helper_legacy_state().as_str();
            let events = if params.get("events").map(String::as_str) == Some("1")
                && st.kind == root_helper::Kind::Root
            {
                root_helper::call_text(&["helper".into(), "audit-tail".into()], None)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            } else {
                None
            };
            ok(root_helper::status_json(
                &st,
                legacy_mail,
                root_helper::legacy_pg_present(),
                events,
            ))
        }
        UPDATE | ROLLBACK => {
            let Some(who) = human(&caller) else {
                return refuse(
                    "403 Forbidden",
                    "owner_only",
                    "updating or rolling back the root helper is an owner/admin step: run `k2 root-helper` outside a K2 terminal",
                );
            };
            if !cfg!(target_os = "linux") {
                return refuse(
                    "409 Conflict",
                    "linux_only",
                    "the root helper exists on Linux daemons only",
                );
            }
            if path == ROLLBACK {
                root_helper::invalidate();
                let r = root_helper::call_text(&["helper".into(), "self-rollback".into()], None);
                root_helper::invalidate();
                k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
                    "root_helper_rollback",
                    &who,
                    match &r {
                        Ok(_) => "ok".to_string(),
                        Err(e) => format!("failed: {e}"),
                    },
                    "cli",
                    "-",
                    "cli",
                ));
                return match r {
                    Ok(t) => ok(serde_json::from_str(&t).unwrap_or_else(|_| json!({"result": t}))),
                    Err(e) => refuse("409 Conflict", "rollback_failed", e),
                };
            }
            let from_dir = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v["fromDir"].as_str().map(str::to_string))
                .filter(|s| !s.is_empty());
            let out = match from_dir {
                Some(dir) => {
                    let p = std::path::PathBuf::from(&dir);
                    if !p.is_absolute() || !p.is_dir() {
                        return refuse(
                            "400 Bad Request",
                            "bad_dir",
                            "fromDir must be an existing absolute directory",
                        );
                    }
                    // Air-gap: never the network; the signature still gates it.
                    heal::run_once(
                        &heal::DirOps { dir: p },
                        &heal::live_plan(Some(false)),
                        &who,
                    )
                }
                None => heal::run_once(&heal::LiveOps, &heal::live_plan(None), &who),
            };
            let cli = root_helper::cli_refresh::run_once(
                &root_helper::cli_refresh::LiveCliOps,
                &root_helper::cli_refresh::daemon_cli_version(),
                &who,
            );
            ok(
                json!({"heal": out, "cli": cli, "status": root_helper::status_json(&root_helper::state(), crate::mail::supervisor::mail_helper_legacy_state().as_str(), root_helper::legacy_pg_present(), None)}),
            )
        }
        _ => refuse("404 Not Found", "not_found", "unknown root-helper route"),
    }
}

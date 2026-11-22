//! `k2 app user import <app> --file users.jsonl [--apply] [--role R]`
//! (prd-app-supabase-compat-v1 §9.2, S2): App users brought over from
//! another user store with their own ids and passwords
//! ([`k2_core::skin::import`] does the work).
//!
//! `POST /cli/app/user/import` `{app, lines, apply?, role?}`: `lines` is the
//! file's JSONL text (one user per line, Supabase's `auth.users` column
//! names or the short ones). A dry run unless `apply`. The answer is the
//! report: counts, derived usernames, every clash; an apply with clashes is
//! refused (409 `import_clashes`) and writes nothing.
//!
//! **Who.** The owner (a K2 shell tab counts as the owner, as everywhere in
//! `k2 app`), or the App's own agent with "Let this agent manage its Apps'
//! users"; an agent's new logins are stamped and tied to its workspace and
//! it never touches a login it didn't make. Not an Admin or Member login,
//! and never the App key (PRD §9.2). Hashes are never logged or echoed.

use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::deploy::codes;
use k2_core::skin;
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_credential, no, respond, s, Who, R};
use crate::cli_response::CliResponse;

pub const IMPORT: &str = "/cli/app/user/import";
pub const ROUTES: &[&str] = &[IMPORT];
/// The file rides in the body: 16 MiB (some thousands of users with
/// metadata).
pub const MAX_BODY: usize = 16 * 1024 * 1024;

pub fn handle(who: &Who, body: &[u8]) -> CliResponse {
    match run(who, body) {
        Ok(Ok(report)) => respond(Ok(report)),
        Ok(Err(report)) => CliResponse { status: "409 Conflict", content_type: "application/json", body: report.to_string() },
        Err(e) => respond(Err(e)),
    }
}

fn run(who: &Who, body: &[u8]) -> R<Result<J, J>> {
    need_credential(who)?;
    let v = json_body(body)?;
    let agent_ws = match who {
        Who::Owner | Who::Shell { .. } | Who::Login { role: k2_core::connect_users::Role::Owner, .. } => None,
        Who::Agent { project_id, .. } => {
            let ws = super::routes::ws_by_id(project_id).ok_or_else(|| no(codes::FORBIDDEN, 403, "your workspace isn't on this box"))?;
            if !crate::skin_routes::agent_may_manage_app_users(&ws.path) {
                return Err(no(
                    crate::skin_routes::APP_USERS_OFF,
                    403,
                    format!(
                        "importing your App's users needs \"Let this agent manage its Apps' users\" (off). Ask your owner to turn it on \
                         ({CLI_NAME} app access <ws> --users on from a terminal outside {PRODUCT_NAME}), or ask them to run the import"
                    ),
                ));
            }
            Some(project_id.clone())
        }
        _ => return Err(no(codes::FORBIDDEN, 403, "importing App users is the owner's, or the App's own agent's (with its users switch)")),
    };
    let app = app_for(who, s(&v, "app"))?;
    let lines = v.get("lines").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, "send lines: the users file's JSONL text"))?;
    let apply = v.get("apply").and_then(J::as_bool).unwrap_or(false);
    let role = s(&v, "role");
    // Read before the user store is locked (an agent's names only).
    let reserved = match agent_ws {
        Some(_) => reserved_names().map_err(|e| no("import_failed", 500, e))?,
        None => Vec::new(),
    };
    let (rows, bad) = skin::import::parse_jsonl(lines, now_secs());
    if rows.is_empty() && bad.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "the file has no users (one JSON object per line)"));
    }
    let label = actor(who);
    let opts = skin::import::Options {
        app: &app.service_id,
        role,
        apply,
        agent_workspace: agent_ws.as_deref(),
        actor: &label,
        reserved: &reserved,
    };
    let out = skin::import::import(&rows, &bad, opts);
    let mut out = match out {
        Err(e) if e.get("clashes").is_none() => {
            let msg = e["error"].as_str().unwrap_or("the import failed").to_string();
            return Err(no("import_failed", 500, msg));
        }
        other => other,
    };
    match &mut out {
        Ok(r) | Err(r) => r["app"] = json!(app.app_id),
    }
    if let Ok(r) = &out {
        if r["written"] == json!(true) {
            let line = json!({
                "route": IMPORT, "app": app.app_id, "new": r["new"], "updated": r["updated"],
                "unchanged": r["unchanged"], "disabled": r["disabled"],
            });
            k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("app_users_imported", &label, line.to_string(), "-", "-", "cli"));
        }
    }
    Ok(out)
}

/// The names an agent may not give a login (`skin::creators::name_reserved`
/// for every candidate would lock the user store once per row).
fn reserved_names() -> Result<Vec<String>, String> {
    let mut out: Vec<String> = skin::creators::RESERVED_WORDS.iter().map(|s| s.to_string()).collect();
    out.extend(k2_core::connect_users::list_users()?.into_iter().map(|u| u.username.to_ascii_lowercase()));
    out.push(k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken).to_ascii_lowercase());
    Ok(out)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
#[path = "user_import_tests.rs"]
mod tests;

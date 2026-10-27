//! `/cli/zen/downloads*`: the widgets each Garden lets save files to
//! Downloads without K2's card asking again (`k2.download`,
//! prd-garden-board-needs-v1; slice-1 security review #1). The store is
//! `k2_core::zen::downloads`: one answer for every client.
//!
//! ```text
//! GET  /cli/zen/downloads[?garden=<id>]          → {ok, gardens: {<id>: [widgets]}}
//!                                                   (with garden: {ok, garden, widgets})
//! POST /cli/zen/downloads/allow  {garden, widget} → {ok, garden, widget, added}
//! POST /cli/zen/downloads/forget {garden, widget} → {ok, garden, widget, removed}
//! ```
//!
//! Every route takes this computer's owner token only (the app, a terminal
//! outside K2): "Always" is the person's answer on K2's own card, and only
//! the person lists or revokes it. An agent or K2 terminal gets 403
//! `owner_only`; a Connect login, an App pass or an API key 403
//! `zen_local_only`.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::{downloads, ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

pub const GET_ROUTES: &[&str] = &["/cli/zen/downloads"];
pub const POST_ROUTES: &[&str] = &["/cli/zen/downloads/allow", "/cli/zen/downloads/forget"];

pub const OWNER_ONLY: &str = "Only the person decides which widgets save files to Downloads without asking (K2's card, the first time). Agents can't read or change that list.";

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetBody {
    garden: String,
    widget: String,
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn refuse(status: &'static str, code: &str, message: &str) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message }))
}

fn zen_err(e: ZenError) -> CliResponse {
    match e {
        ZenError::BadRequest(m) => refuse("400 Bad Request", "bad_request", &m),
        other => refuse("500 Internal Server Error", "internal", &other.to_string()),
    }
}

/// The route, or `None` when `path` isn't one of ours.
pub fn handle(path: &str, owner: bool, caller: &Caller, params: &HashMap<String, String>, body: &[u8]) -> Option<CliResponse> {
    if !is_route(path) {
        return None;
    }
    Some(run(&ZenFiles::local().root().to_path_buf(), path, owner, caller, params, body))
}

/// [`handle`] on a given Zen folder (tests use a temp one).
pub fn run(zen_dir: &Path, path: &str, owner: bool, caller: &Caller, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let owner_token = owner && *caller == Caller::Owner;
    if !owner_token && !crate::zen_routes::is_local_passport(caller) {
        return crate::zen_routes::local_only();
    }
    if !owner_token {
        return refuse("403 Forbidden", "owner_only", OWNER_ONLY);
    }
    match path {
        "/cli/zen/downloads" => match params.get("garden").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(g) => match downloads::widgets(zen_dir, g) {
                Ok(w) => resp("200 OK", json!({ "ok": true, "garden": g, "widgets": w })),
                Err(e) => zen_err(e),
            },
            None => resp("200 OK", json!({ "ok": true, "gardens": downloads::read(zen_dir).gardens })),
        },
        "/cli/zen/downloads/allow" | "/cli/zen/downloads/forget" => {
            let b: WidgetBody = match serde_json::from_slice(body) {
                Ok(b) => b,
                Err(e) => return refuse("400 Bad Request", "bad_request", &format!("takes {{garden, widget}}: {e}")),
            };
            let (g, w) = (b.garden.trim(), b.widget.trim());
            if path.ends_with("/allow") {
                match downloads::allow(zen_dir, g, w) {
                    Ok(added) => resp("200 OK", json!({ "ok": true, "garden": g, "widget": w, "added": added })),
                    Err(e) => zen_err(e),
                }
            } else {
                match downloads::forget(zen_dir, g, w) {
                    Ok(removed) => resp("200 OK", json!({ "ok": true, "garden": g, "widget": w, "removed": removed })),
                    Err(e) => zen_err(e),
                }
            }
        }
        _ => refuse("404 Not Found", "not_found", "unknown zen downloads route"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar_routes::AgentCaller;

    fn call(d: &Path, path: &str, owner: bool, caller: &Caller, q: &[(&str, &str)], body: J) -> (String, J) {
        let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = run(d, path, owner, caller, &params, body.to_string().as_bytes());
        (r.status.to_string(), serde_json::from_str(&r.body).expect("json"))
    }

    #[test]
    fn one_answer_for_every_client_owner_only_and_revocable() {
        let d = std::env::temp_dir().join(format!("k2-zen-downloads-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir");
        let w = json!({"garden": "g-1", "widget": "board"});
        // Client A (one window) answers Always.
        let (s, v) = call(&d, "/cli/zen/downloads/allow", true, &Caller::Owner, &[], w.clone());
        assert_eq!((s.as_str(), v["added"].as_bool()), ("200 OK", Some(true)), "{v}");
        // Client B (another window, the web client) reads the same answer.
        let (_, v) = call(&d, "/cli/zen/downloads", true, &Caller::Owner, &[("garden", "g-1")], J::Null);
        assert_eq!(v["widgets"], json!(["board"]));
        // Agents can neither read nor change it; logins never reach it.
        let agent = Caller::Agent(AgentCaller { project_id: "w-1".into(), map_key: None, args: vec![], sandboxed: false });
        for (p, body) in [("/cli/zen/downloads", J::Null), ("/cli/zen/downloads/allow", w.clone()), ("/cli/zen/downloads/forget", w.clone())] {
            let (s, v) = call(&d, p, false, &agent, &[("garden", "g-1")], body);
            assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{p}: {v}");
        }
        let (_, v) = call(&d, "/cli/zen/downloads", false, &Caller::Login { username: "adam".into() }, &[], J::Null);
        assert_eq!(v["error"].as_str(), Some("zen_local_only"));
        // Revoking brings the card back for every client.
        let (_, v) = call(&d, "/cli/zen/downloads/forget", true, &Caller::Owner, &[], w);
        assert_eq!(v["removed"].as_bool(), Some(true));
        let (_, v) = call(&d, "/cli/zen/downloads", true, &Caller::Owner, &[("garden", "g-1")], J::Null);
        assert_eq!(v["widgets"], json!([]));
        let (s, _) = call(&d, "/cli/zen/downloads/allow", true, &Caller::Owner, &[], json!({"garden": "g-1", "widget": "../x"}));
        assert_eq!(s, "400 Bad Request");
        for p in POST_ROUTES {
            assert!(crate::zen_routes::POST_ROUTES.contains(p), "{p} is a zen POST row (405 on GET)");
            assert!(crate::routes::route_policy::lookup(p).is_some_and(|r| r.post.is_some()), "{p}: policy POST row");
        }
        for p in GET_ROUTES {
            assert!(crate::zen_routes::GET_ROUTES.contains(p), "{p} is a zen GET row");
            assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a policy row");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

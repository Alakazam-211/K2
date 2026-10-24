//! `/cli/zen/links*`: the sites each Garden may open in the person's
//! browser without asking again (`k2.open`, catalog `links.open`, cap
//! `links:open`; 0.46.4). The store is `k2_core::zen::links`.
//!
//! ```text
//! GET  /cli/zen/links[?garden=<id>]     → {ok, gardens: {<id>: [sites]}}
//!                                          (with garden: {ok, garden, sites})
//! POST /cli/zen/links/allow  {garden, site} → {ok, garden, site, added}
//! POST /cli/zen/links/forget {garden, site} → {ok, garden, site, removed}
//! ```
//!
//! - **Read:** this computer's owner token (the app, a terminal outside K2)
//!   and passports on this computer (agents and K2 terminals): an agent may
//!   see what the person allowed.
//! - **Write:** this computer's owner token only. "Always open" is the
//!   person's answer on K2's own card; a widget can't reach these routes and
//!   an agent gets 403 `owner_only`, so nothing but the person adds a site.
//!   A Connect login, an App pass or an API key gets 403 `zen_local_only`.
//!
//! Routing: `zen_routes::handle` calls [`handle`] first (one hook line);
//! the rows are in `zen_routes::POST_ROUTES` / `GET_ROUTES` (405 on a GET
//! to a POST row) and the policy table.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::{links, ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

pub const GET_ROUTES: &[&str] = &["/cli/zen/links"];
pub const POST_ROUTES: &[&str] = &["/cli/zen/links/allow", "/cli/zen/links/forget"];

/// What an agent hears on a write.
pub const OWNER_ONLY: &str = "Only the person opens a site for a Garden: K2 asks them on its own card the first time a widget opens a link there. Agents can read the list (k2 zen links) but can't change it.";

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteBody {
    garden: String,
    site: String,
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
    let reader = owner_token || crate::zen_routes::is_local_passport(caller);
    if !reader {
        return crate::zen_routes::local_only();
    }
    if POST_ROUTES.contains(&path) && !owner_token {
        return refuse("403 Forbidden", "owner_only", OWNER_ONLY);
    }
    match path {
        "/cli/zen/links" => {
            let garden = params.get("garden").map(|s| s.trim()).filter(|s| !s.is_empty());
            match garden {
                Some(g) => match links::sites(zen_dir, g) {
                    Ok(sites) => resp("200 OK", json!({ "ok": true, "garden": g, "sites": sites })),
                    Err(e) => zen_err(e),
                },
                None => resp("200 OK", json!({ "ok": true, "gardens": links::read(zen_dir).gardens })),
            }
        }
        "/cli/zen/links/allow" | "/cli/zen/links/forget" => {
            let b: SiteBody = match serde_json::from_slice(body) {
                Ok(b) => b,
                Err(e) => return refuse("400 Bad Request", "bad_request", &format!("takes {{garden, site}}: {e}")),
            };
            if path.ends_with("/allow") {
                match links::allow(zen_dir, b.garden.trim(), b.site.trim()) {
                    Ok(added) => resp("200 OK", json!({ "ok": true, "garden": b.garden.trim(), "site": b.site.trim(), "added": added })),
                    Err(e) => zen_err(e),
                }
            } else {
                match links::forget(zen_dir, b.garden.trim(), b.site.trim()) {
                    Ok(removed) => resp("200 OK", json!({ "ok": true, "garden": b.garden.trim(), "site": b.site.trim(), "removed": removed })),
                    Err(e) => zen_err(e),
                }
            }
        }
        _ => refuse("404 Not Found", "not_found", "unknown zen links route"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar_routes::AgentCaller;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("k2-zen-links-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst) + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos() as u64
    }

    fn agent() -> Caller {
        Caller::Agent(AgentCaller { project_id: "w-1".into(), map_key: None, args: vec![], sandboxed: false })
    }

    fn call(d: &Path, path: &str, owner: bool, caller: &Caller, q: &[(&str, &str)], body: J) -> (String, J) {
        let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = run(d, path, owner, caller, &params, body.to_string().as_bytes());
        (r.status.to_string(), serde_json::from_str(&r.body).expect("json"))
    }

    #[test]
    fn the_person_allows_and_forgets_agents_only_read() {
        let d = dir();
        let site = json!({"garden": "g-1", "site": "https://example.com"});
        let (s, v) = call(&d, "/cli/zen/links/allow", false, &agent(), &[], site.clone());
        assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "{v}");
        let (s, v) = call(&d, "/cli/zen/links/allow", true, &Caller::Owner, &[], site.clone());
        assert_eq!((s.as_str(), v["added"].as_bool()), ("200 OK", Some(true)), "{v}");
        let (s, v) = call(&d, "/cli/zen/links", false, &agent(), &[("garden", "g-1")], J::Null);
        assert_eq!((s.as_str(), v["sites"].clone()), ("200 OK", json!(["https://example.com"])), "an agent reads: {v}");
        let (_, v) = call(&d, "/cli/zen/links", true, &Caller::Owner, &[], J::Null);
        assert_eq!(v["gardens"], json!({"g-1": ["https://example.com"]}));
        let (s, v) = call(&d, "/cli/zen/links/forget", false, &agent(), &[], site.clone());
        assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("owner_only")), "an agent can't forget either: {v}");
        let (_, v) = call(&d, "/cli/zen/links/forget", true, &Caller::Owner, &[], site);
        assert_eq!(v["removed"].as_bool(), Some(true), "{v}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn other_credentials_are_local_only_and_bodies_are_checked() {
        let d = dir();
        for c in [Caller::Login { username: "adam".into() }, Caller::WrongCredential, Caller::Invalid] {
            let (s, v) = call(&d, "/cli/zen/links", false, &c, &[], J::Null);
            assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{c:?}: {v}");
        }
        let sandboxed = Caller::Agent(AgentCaller { project_id: "w-1".into(), map_key: None, args: vec![], sandboxed: true });
        let (_, v) = call(&d, "/cli/zen/links", false, &sandboxed, &[], J::Null);
        assert_eq!(v["error"].as_str(), Some("zen_local_only"), "a sandboxed agent never reaches Zen");
        for bad in [
            json!({"garden": "g-1", "site": "https://example.com/path"}),
            json!({"garden": "g-1", "site": "javascript:alert(1)"}),
            json!({"garden": "../x", "site": "https://example.com"}),
            json!({"garden": "g-1"}),
            json!({"garden": "g-1", "site": "https://example.com", "extra": 1}),
        ] {
            let (s, v) = call(&d, "/cli/zen/links/allow", true, &Caller::Owner, &[], bad.clone());
            assert_eq!((s.as_str(), v["error"].as_str()), ("400 Bad Request", Some("bad_request")), "{bad}: {v}");
        }
        assert!(!d.join(links::LINKS_FILE).exists(), "nothing refused was written");
        for p in POST_ROUTES {
            assert!(crate::zen_routes::POST_ROUTES.contains(p), "{p} is a zen POST row (405 on GET)");
            let row = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no policy row"));
            assert!(row.post.is_some(), "{p}: policy POST row");
        }
        for p in GET_ROUTES {
            assert!(crate::zen_routes::GET_ROUTES.contains(p), "{p} is a zen GET row");
            assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a policy row");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

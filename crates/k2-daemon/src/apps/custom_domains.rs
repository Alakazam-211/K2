//! Custom domains that actually serve an App (0.46.6,
//! `prd-app-custom-domains-v1` + its vs-live CD-A1…CD-A22).
//!
//! A hostname bound with role `app:<id>` (`k2 app domain add <app>
//! app.customer.com`, or the low-level `k2 domain name add <host> --role
//! app:<id>`) walks `waiting_dns` → `verifying` → `issuing` → `live`
//! (+ `failing`); see `k2_core::domains::AppDomain`.
//!
//! This module is the box side's single home for:
//! - **who may bind** (CD-A2): `app:<id>` must name a runtime App or a
//!   published service on this box, and an agent may bind only its own
//!   workspace's, with "Build and run Apps" on. A hostname already bound to
//!   another workspace's App is never an agent's to re-role or remove.
//! - **the honest status** (CD-A5): what the name's state is, which server
//!   really answers for it, and the DNS to set.

use serde_json::{json, Value as J};

use k2_core::domains::{self, AppDomain};

/// `app:<id>` names no App or published service here (or, for an agent,
/// none in its workspace: the same answer, so no existence leak).
pub const UNKNOWN_APP: &str = "unknown_app";
/// The hostname is bound to another workspace's App.
pub const BOUND_ELSEWHERE: &str = "hostname_bound_elsewhere";

/// A refusal with its HTTP status, code and plain-words hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
}

fn refuse(status: u16, code: &'static str, hint: impl Into<String>) -> Refusal {
    Refusal { status, code, hint: hint.into() }
}

/// The workspaces that hold `name` as a runtime App (its id) or as a
/// published service (its name). Empty = no such App on this box.
pub fn holders(name: &str) -> Vec<String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Vec::new();
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<String> = Vec::new();
    if let Ok(Some(app)) = k2_core::deploy::store::get_app(&conn, &name) {
        out.push(app.owner_workspace);
    }
    if let Ok(mut st) = conn.prepare("SELECT DISTINCT project_id FROM published_services WHERE name = ?1") {
        if let Ok(rows) = st.query_map(rusqlite::params![name], |r| r.get::<_, String>(0)) {
            for ws in rows.flatten() {
                if !out.contains(&ws) {
                    out.push(ws);
                }
            }
        }
    }
    out
}

/// CD-A2: may this caller bind a hostname to App `name`? `caller_ws` is
/// the agent's workspace id (`None` = the owner or an Admin, who may bind
/// any App on the box).
///
/// - No App or published service called `name` here → `400 unknown_app`.
/// - An agent naming another workspace's App → the same `400 unknown_app`
///   ("…in your workspace"): an agent never learns other workspaces' Apps.
/// - An agent without "Build and run Apps" → `403 apps_manage_off`
///   (`k2 app domain add` turns that into a held request for the owner).
pub fn may_bind(caller_ws: Option<&str>, name: &str) -> Result<(), Refusal> {
    let held_by = holders(name);
    if held_by.is_empty() {
        return Err(refuse(
            400,
            UNKNOWN_APP,
            format!("no App or published service '{name}' on this box (k2 app list): a custom domain binds to an App that exists"),
        ));
    }
    let Some(ws) = caller_ws else { return Ok(()) };
    if !held_by.iter().any(|h| h == ws) {
        return Err(refuse(
            400,
            UNKNOWN_APP,
            format!("no App or published service '{name}' in your workspace (k2 app list): an agent binds names only to its own workspace's Apps"),
        ));
    }
    if super::access::level(ws) == super::access::Level::Off {
        return Err(refuse(
            403,
            k2_core::deploy::codes::APPS_MANAGE_OFF,
            "\"Build and run Apps\" is off for this agent: use `k2 app domain add <app> <host>`, which asks your owner, \
             or ask them to turn it on (Workspace settings → Agent → Apps)",
        ));
    }
    Ok(())
}

/// May this caller re-role or remove a hostname whose current role is
/// `existing_role`? The owner may; an agent may not touch a name bound to
/// another workspace's App (or to an App that no longer exists and that
/// it can't show is its own).
pub fn may_change_existing(caller_ws: Option<&str>, hostname: &str, existing_role: &str) -> Result<(), Refusal> {
    let Some(ws) = caller_ws else { return Ok(()) };
    let Some(app) = domains::app_of_role(existing_role) else { return Ok(()) };
    if holders(app).iter().any(|h| h == ws) {
        return Ok(());
    }
    Err(refuse(
        409,
        BOUND_ELSEWHERE,
        format!("{hostname} is bound to an App outside your workspace: only the owner can move or remove it"),
    ))
}

/// What one name's served-by probe found (CD-A5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedBy {
    /// The name's public :443 presents this box's stored leaf.
    ThisBox,
    /// It presents another certificate: another server answers for it.
    Elsewhere,
    /// No TLS answer at all.
    Nothing,
}

impl ServedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            ServedBy::ThisBox => "this_box",
            ServedBy::Elsewhere => "elsewhere",
            ServedBy::Nothing => "nothing",
        }
    }
}

/// Which server really answers for `hostname` on :443: the served leaf
/// compared with the one in this box's store (`~/.k2/certs/<host>/`).
/// A leaf with no stored copy to compare is `Elsewhere`.
pub fn served_by(hostname: &str) -> ServedBy {
    use crate::domains::status::{pem_leaf_sha256, served_leaf, Served};
    let ours = crate::domains::store::load(hostname).and_then(|p| pem_leaf_sha256(&p.chain_pem));
    match served_leaf(hostname, 443) {
        Served::Nothing => ServedBy::Nothing,
        Served::Leaf(l) if ours.as_deref() == Some(l.as_str()) => ServedBy::ThisBox,
        Served::Leaf(_) => ServedBy::Elsewhere,
    }
}

/// The plain-words line for a state (CLI and Settings).
pub fn state_words(d: &AppDomain) -> String {
    if !d.apex_attached {
        return format!("not served yet: its apex {} is no longer attached (k2 domain add {})", d.apex, d.apex);
    }
    match d.state.as_str() {
        domains::STATE_LIVE => "live: this box serves it".to_string(),
        domains::STATE_ISSUING => "not served yet: proven, getting its certificate".to_string(),
        domains::STATE_VERIFYING => "not served yet: checking the DNS records".to_string(),
        domains::STATE_FAILING => "not served (failing): the DNS records stopped pointing here".to_string(),
        _ => "not served yet: waiting for the DNS records".to_string(),
    }
}

/// The `app` block K2 adds to a bound name's JSON (`k2 cert list`,
/// `k2 domain list`, Settings → Domains, `k2 app domains`): the state,
/// whether it is served, which server answers, the TXT proof to set.
/// `probe`: also ask which server answers on :443 (a network probe).
pub fn status_json(d: &AppDomain, probe: bool) -> J {
    let (txt_name, txt_value) = d.txt();
    let served = probe.then(|| served_by(&d.hostname));
    let live = d.is_live();
    let mut v = json!({
        "app": d.app,
        "state": d.state,
        "live": live,
        // Vanilla's small part: say plainly when nothing serves it yet.
        "notServedYet": !live,
        "words": state_words(d),
        "primary": d.is_primary,
        "apexAttached": d.apex_attached,
        "verifiedAt": d.verified_at,
        "checkedAt": d.checked_at,
        "path": d.path,
        "lastError": d.last_error,
        "failingSince": d.failing_since,
        "txt": { "type": "TXT", "name": txt_name, "value": txt_value },
        "appExists": !holders(&d.app).is_empty(),
    });
    if let Some(s) = served {
        v["served"] = json!(s.as_str());
        if live && s != ServedBy::ThisBox {
            v["warning"] = json!(match s {
                ServedBy::Elsewhere => "state is live, but :443 for this name presents another certificate (DNS may point elsewhere)",
                _ => "state is live, but nothing answered TLS for this name on :443 from this box",
            });
        }
    }
    if !v["appExists"].as_bool().unwrap_or(false) {
        v["words"] = json!(format!("not served: no App '{}' on this box ({UNKNOWN_APP})", d.app));
        v["error"] = json!(UNKNOWN_APP);
    }
    v
}

/// [`status_json`] for `hostname` when it is an `app:` name (`None`
/// otherwise).
pub fn status_json_for(hostname: &str, probe: bool) -> Option<J> {
    let d = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::app_domain(&conn, hostname).ok().flatten()
    }?;
    Some(status_json(&d, probe))
}

#[cfg(test)]
pub(crate) mod test_util {
    //! Shared fixtures: an App row (with its published service) in a
    //! workspace, and the agent switch.

    pub fn workspace(id: &str) {
        let path = std::env::temp_dir().join("k2-cd-test").join(id);
        std::fs::create_dir_all(&path).unwrap();
        let path = std::fs::canonicalize(&path).unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?1, ?2)",
            rusqlite::params![id, path.to_string_lossy()],
        )
        .unwrap();
    }

    /// Run `f` as the agent passport of workspace `ws`.
    pub fn as_agent<T>(ws: &str, f: impl FnOnce() -> T) -> T {
        crate::caller_workspace::with_request_principal(
            Some(crate::session_token::HookPrincipal { workspace_uuid: ws.to_string(), agent_address: ws.to_string() }),
            f,
        )
    }

    pub fn build_switch(ws: &str, on: bool) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agents_can_manage_skin = ?2 WHERE id = ?1", rusqlite::params![ws, on as i64])
            .unwrap();
    }

    pub fn dns_manage(ws: &str, on: bool) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET dns_manage_enabled = ?2 WHERE id = ?1", rusqlite::params![ws, on as i64])
            .unwrap();
    }

    /// A runtime App `app` owned by `ws` (and its published service).
    pub fn runtime_app(ws: &str, app: &str) {
        workspace(ws);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, ws, app, "app", "/x", 0, "tunnel", "running", "cmd", "").unwrap();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: app,
                service_id: &svc.id,
                seed_id: None,
                owner_workspace: ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn unknown_apps_are_refused_for_everyone() {
        let _db = k2_core::db::scoped_for_test();
        let e = may_bind(None, "ghost").unwrap_err();
        assert_eq!((e.status, e.code), (400, UNKNOWN_APP), "{e:?}");
        workspace("ws-a");
        build_switch("ws-a", true);
        let e = may_bind(Some("ws-a"), "ghost").unwrap_err();
        assert_eq!((e.status, e.code), (400, UNKNOWN_APP), "{e:?}");
    }

    /// CD-A2: an agent binds only its own workspace's Apps, and only with
    /// "Build and run Apps"; the owner binds any App on the box.
    #[test]
    fn an_agent_binds_only_its_own_workspaces_apps() {
        let _db = k2_core::db::scoped_for_test();
        runtime_app("ws-a", "jobs");
        runtime_app("ws-b", "crm");
        build_switch("ws-a", true);
        build_switch("ws-b", true);

        assert_eq!(may_bind(None, "jobs"), Ok(()), "the owner may bind any App");
        assert_eq!(may_bind(None, "crm"), Ok(()));
        assert_eq!(may_bind(Some("ws-a"), "jobs"), Ok(()));
        let e = may_bind(Some("ws-a"), "crm").unwrap_err();
        assert_eq!((e.status, e.code), (400, UNKNOWN_APP), "another workspace's App reads as unknown: {e:?}");
        assert!(e.hint.contains("in your workspace"), "{}", e.hint);

        build_switch("ws-a", false);
        let e = may_bind(Some("ws-a"), "jobs").unwrap_err();
        assert_eq!((e.status, e.code), (403, k2_core::deploy::codes::APPS_MANAGE_OFF), "{e:?}");
    }

    /// A plain published service (a `--cmd` service, no runtime App row)
    /// counts as an App for the binding, in its own workspace only.
    #[test]
    fn a_published_service_counts_in_its_workspace() {
        let _db = k2_core::db::scoped_for_test();
        workspace("ws-a");
        workspace("ws-b");
        build_switch("ws-b", true);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::published_services::insert(&conn, "ws-a", "legacy", "cmd", "/x", 3000, "tunnel", "running", "cmd", "").unwrap();
        }
        assert_eq!(holders("legacy"), vec!["ws-a".to_string()]);
        assert_eq!(may_bind(None, "legacy"), Ok(()));
        assert_eq!(may_bind(Some("ws-b"), "legacy").unwrap_err().code, UNKNOWN_APP);
    }

    #[test]
    fn a_name_bound_to_another_workspaces_app_is_never_an_agents() {
        let _db = k2_core::db::scoped_for_test();
        runtime_app("ws-a", "jobs");
        runtime_app("ws-b", "crm");
        assert_eq!(may_change_existing(None, "x.example.com", "app:crm"), Ok(()));
        assert_eq!(may_change_existing(Some("ws-a"), "x.example.com", "app:jobs"), Ok(()));
        let e = may_change_existing(Some("ws-a"), "x.example.com", "app:crm").unwrap_err();
        assert_eq!((e.status, e.code), (409, BOUND_ELSEWHERE));
        let e = may_change_existing(Some("ws-a"), "x.example.com", "app:gone").unwrap_err();
        assert_eq!(e.code, BOUND_ELSEWHERE, "an App that no longer exists isn't the agent's either");
        assert_eq!(may_change_existing(Some("ws-a"), "x.example.com", "other"), Ok(()), "non-App roles: dns_manage as before");
    }

    /// CD-A5 + "not served yet": a bound name reads not served until it is
    /// live; `served` says which server answers, compared by leaf.
    #[test]
    fn status_is_honest_about_serving() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        runtime_app("ws-a", "jobs");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            domains::upsert_binding(&conn, "example.com", None, false).unwrap();
            domains::upsert_name(&conn, "app.example.com", "example.com", "app:jobs").unwrap();
        }
        // Nothing answers: not served yet.
        crate::domains::status::set_test_served("app.example.com", Some(crate::domains::status::Served::Nothing));
        let v = status_json_for("app.example.com", true).expect("an app: name");
        assert_eq!(v["state"], "waiting_dns");
        assert_eq!(v["notServedYet"], true);
        assert_eq!(v["served"], "nothing");
        assert!(v["words"].as_str().unwrap().starts_with("not served yet"), "{v}");
        assert_eq!(v["txt"]["name"], "_k2-domain.app.example.com");
        assert!(v["txt"]["value"].as_str().unwrap().starts_with("k2-verify="));

        // Another server's certificate (e.g. the old host) is not "issued
        // here": served elsewhere, and still not served by this box.
        let pem = crate::domains::acme::test_le_pem("app.example.com");
        crate::domains::store::install("app.example.com", &pem.chain_pem, &pem.key_pem).unwrap();
        let other = crate::domains::acme::test_le_pem("app.example.com");
        let other_leaf = crate::domains::status::pem_leaf_sha256(&other.chain_pem).unwrap();
        crate::domains::status::set_test_served("app.example.com", Some(crate::domains::status::Served::Leaf(other_leaf)));
        assert_eq!(served_by("app.example.com"), ServedBy::Elsewhere);
        let v = status_json_for("app.example.com", true).unwrap();
        assert_eq!((v["served"].as_str(), v["notServedYet"].as_bool()), (Some("elsewhere"), Some(true)));

        // Our leaf, but the state isn't live: still "not served yet".
        let ours = crate::domains::status::pem_leaf_sha256(&pem.chain_pem).unwrap();
        crate::domains::status::set_test_served("app.example.com", Some(crate::domains::status::Served::Leaf(ours)));
        assert_eq!(served_by("app.example.com"), ServedBy::ThisBox);
        assert_eq!(status_json_for("app.example.com", true).unwrap()["notServedYet"], true);

        // Live and ours: served.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            domains::set_app_state(&conn, "app.example.com", domains::STATE_LIVE, None).unwrap();
        }
        let v = status_json_for("app.example.com", true).unwrap();
        assert_eq!((v["notServedYet"].as_bool(), v["served"].as_str()), (Some(false), Some("this_box")));
        assert!(v.get("warning").is_none(), "{v}");

        // Live, but :443 shows nothing from here: a warning, not silence.
        crate::domains::status::set_test_served("app.example.com", Some(crate::domains::status::Served::Nothing));
        let v = status_json_for("app.example.com", true).unwrap();
        assert!(v["warning"].as_str().unwrap().contains("nothing answered"), "{v}");
        crate::domains::status::set_test_served("app.example.com", None);

        // A non-App name has no App block.
        assert!(status_json_for("nope.example.com", false).is_none());
    }

    fn add(host: &str, role: &str) -> crate::cli_response::CliResponse {
        let mut p = std::collections::HashMap::new();
        p.insert("hostname".to_string(), host.to_string());
        p.insert("role".to_string(), role.to_string());
        crate::domains::routes::handle_names_add(&p)
    }

    fn rm(host: &str) -> crate::cli_response::CliResponse {
        let mut p = std::collections::HashMap::new();
        p.insert("hostname".to_string(), host.to_string());
        crate::domains::routes::handle_names_remove(&p)
    }

    fn role_of(host: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::get_name(&conn, host).unwrap().map(|n| n.role)
    }

    /// PRD test 1 + CD-A2 through the real route (`POST /cli/domains/names`):
    /// an unknown App is 400 `unknown_app` for the owner too; an agent with
    /// `dns_manage` can't bind, re-role or remove another workspace's App
    /// name, and nothing is written when it is refused.
    #[test]
    fn the_names_route_refuses_unknown_and_foreign_apps() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        crate::domains::status::set_test_served("a.example.com", Some(crate::domains::status::Served::Nothing));
        crate::domains::status::set_test_served("b.example.com", Some(crate::domains::status::Served::Nothing));
        let _probe = crate::domains::status::set_test_probe(Some(crate::domains::status::ProbeResult::missing("x")));
        runtime_app("ws-a", "jobs");
        runtime_app("ws-b", "crm");
        for ws in ["ws-a", "ws-b"] {
            build_switch(ws, true);
            dns_manage(ws, true);
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            domains::upsert_binding(&conn, "example.com", None, false).unwrap();
        }

        // Owner: unknown App refused, nothing written.
        let r = add("a.example.com", "app:ghost");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains(UNKNOWN_APP), "{}", r.body);
        assert_eq!(role_of("a.example.com"), None);

        // Agent ws-a → ws-b's App: refused as unknown, nothing written.
        let r = as_agent("ws-a", || add("a.example.com", "app:crm"));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains(UNKNOWN_APP), "{}", r.body);
        assert_eq!(role_of("a.example.com"), None);

        // Agent ws-a → its own App: bound, waiting_dns, not served yet.
        let r = as_agent("ws-a", || add("a.example.com", "app:jobs"));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["name"]["app"]["state"], "waiting_dns", "{v}");
        assert_eq!(v["name"]["app"]["notServedYet"], true, "{v}");

        // ws-b's agent binds its own; ws-a then can't re-role or remove it.
        let r = as_agent("ws-b", || add("b.example.com", "app:crm"));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let r = as_agent("ws-a", || add("b.example.com", "app:jobs"));
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert!(r.body.contains(BOUND_ELSEWHERE), "{}", r.body);
        let r = as_agent("ws-a", || add("b.example.com", "other"));
        assert_eq!(r.status, "409 Conflict", "re-roling to a plain name is a steal too: {}", r.body);
        let r = as_agent("ws-a", || rm("b.example.com"));
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
        assert_eq!(role_of("b.example.com").as_deref(), Some("app:crm"), "untouched");

        // Without "Build and run Apps" the agent can't bind its own either.
        build_switch("ws-a", false);
        let r = as_agent("ws-a", || add("c.example.com", "app:jobs"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(role_of("c.example.com"), None);

        // The owner may move and remove any of them.
        assert_eq!(add("b.example.com", "app:jobs").status, "200 OK");
        assert_eq!(rm("b.example.com").status, "200 OK");
        for h in ["a.example.com", "b.example.com"] {
            crate::domains::status::set_test_served(h, None);
        }
    }

    /// A row naming an App that no longer exists says so.
    #[test]
    fn a_row_for_an_unknown_app_says_unknown_app() {
        let _db = k2_core::db::scoped_for_test();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            domains::upsert_binding(&conn, "example.com", None, false).unwrap();
            domains::upsert_name(&conn, "old.example.com", "example.com", "app:ghost").unwrap();
        }
        let v = status_json_for("old.example.com", false).unwrap();
        assert_eq!(v["error"], UNKNOWN_APP);
        assert_eq!(v["appExists"], false);
    }
}

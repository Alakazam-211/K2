//! Agent hire doors: recipe hire, idempotency, rollback with zero residue,
//! key ceiling and quotas, links, reach, `/v1/w` T19, the hire-key mint
//! confirm and the `k2 msg who` sender check. Real shared test DB, temp
//! hire root, rolled-back folders to a temp trash dir (never the OS Trash).

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};
use uuid::Uuid;

use super::test_support::ensure_test_root;
use super::*;
use crate::routes::http::V1Principal;
use k2_core::api_keys::ApiCapabilities;

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..10])
}

fn call(path: &str, who: &V1Principal, post: bool, body: Value, idem: Option<&str>) -> (u16, Value) {
    let r = handle_v1(path, who, post, body.to_string().as_bytes(), &HashMap::new(), idem);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn get(path: &str, who: &V1Principal, params: &[(&str, &str)]) -> (u16, Value) {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle_v1(path, who, false, b"", &p, None);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn status_code(s: &str) -> u16 {
    s.split_whitespace().next().and_then(|n| n.parse().ok()).expect("status")
}

fn owner() -> V1Principal {
    ensure_test_root();
    V1Principal::Owner
}

/// Mint a real hire key (DB-backed) and resolve it.
fn hire_key(caps: ApiCapabilities, limits: Value, grant: &str) -> V1Principal {
    ensure_test_root();
    let l = k2_core::hire::limits::HireLimits::from_request(&limits, caps.db).expect("limits");
    let caps = ApiCapabilities { hire: true, ..caps };
    let (_id, raw) = k2_core::api_keys::create_api_key_ex(&uniq("hk"), None, Some(grant), None, None, Some(caps), Some(&l.to_json()))
        .expect("mint");
    V1Principal::Api(k2_core::api_keys::resolve_api_key(&raw).expect("resolve"))
}

fn hire_only() -> ApiCapabilities {
    ApiCapabilities::none()
}

fn project_by_handle(handle: &str) -> Option<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, path FROM projects WHERE handle = ?1 COLLATE NOCASE",
        rusqlite::params![handle],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

fn count(sql: &str, arg: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(sql, rusqlite::params![arg], |r| r.get(0)).expect("count")
}

// ── recipe hire ──────────────────────────────────────────────────────

#[test]
fn owner_hires_one_agent_with_the_whole_recipe() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let handle = uniq("solo");
    let body = json!({
        "agent": {
            "role": "librarian",
            "handle": handle,
            "name": "Plans Librarian",
            "color": "#4F46E5",
            "persona": "You index plan sets.",
            "preset": "claude",
            "model": "opus",
            "wikiNotes": [{ "id": "Plans.md", "body": "plan notes" }],
            "context": ["wiki:hygiene"],
            "layers": [{ "label": "desk", "markdown": "# Desk\n" }],
            "skills": [{ "name": "plan-index", "body": "---\nname: plan-index\n---\n# Plan index\n" }],
            "heartbeats": [{ "name": "reindex", "frequency": "daily", "spec": { "time": "02:00" }, "instructions": "re-index" }],
        },
        "onBehalfOf": "alex (job J1)",
    });
    let (st, v) = call(V1_HIRE, &who, true, body, Some(&uniq("idem")));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["changed"], true);
    assert_eq!(v["status"], "active");
    assert_eq!(v["onBehalfOf"]["verified"], false, "onBehalfOf is app-asserted: {v}");
    let m = &v["members"][0];
    assert_eq!(m["handle"], handle.as_str());
    let path = m["path"].as_str().expect("path").to_string();
    assert!(Path::new(&path).starts_with(ensure_test_root()), "under the hire root: {path}");
    let persona = std::fs::read_to_string(k2_core::workspace::agent_identity::persona_md_in(
        k2_core::workspace::agent_identity::workspace_agent_path(&path),
    ))
    .expect("persona");
    assert!(persona.contains("You index plan sets."), "{persona}");
    assert_eq!(std::fs::read_to_string(Path::new(&path).join(".k2/wiki/Plans.md")).expect("note"), "plan notes");
    assert!(Path::new(&path).join(".k2/wiki/Home.md").is_file(), "wiki seeded by default");
    assert!(Path::new(&path).join(".k2/context/desk.md").is_file());
    let skill = k2_core::skills::crud::profile(&path, "plan-index").expect("skill");
    assert!(skill.contains("# Plan index"), "{skill}");
    assert!(
        k2_core::heartbeats::k2_heartbeat_list(path.clone()).expect("hb list").iter().any(|h| h.name == "reindex"),
        "heartbeat added"
    );
    assert_eq!(k2_core::workspace::display::agent_display_name(&path), "Plans Librarian");
    let (ws_id, _) = project_by_handle(&handle).expect("registered");
    assert_eq!(count("SELECT COUNT(*) FROM projects WHERE id = ?1 AND color = '#4f46e5'", &ws_id), 1);
    assert_eq!(
        count("SELECT COUNT(*) FROM hire_events WHERE instance_id = ?1 AND action = 'agent-hire' AND result = 'ok'", v["instance"].as_str().unwrap()),
        1,
        "ledgered"
    );
    let sink = AUDIT_SINK.lock().unwrap();
    assert!(sink.iter().any(|(e, a, o)| e == "agent-hire" && a == "owner-token" && o.contains(&handle)), "audited: {sink:?}");
}

#[test]
fn idempotency_key_replays_conflicts_and_scopes() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let handle = uniq("idem");
    let body = json!({ "agent": { "handle": handle, "role": "worker" } });
    let k = uniq("key");
    let (st, first) = call(V1_HIRE, &who, true, body.clone(), Some(&k));
    assert_eq!(st, 200, "{first}");
    let (st, again) = call(V1_HIRE, &who, true, body.clone(), Some(&k));
    assert_eq!(st, 200, "{again}");
    assert_eq!(again["changed"], false, "{again}");
    assert_eq!(again["instance"], first["instance"]);
    assert_eq!(count("SELECT COUNT(*) FROM projects WHERE handle LIKE ?1", &format!("{handle}%")), 1, "no second agent");
    // Body key works the same as the header.
    let (st, via_body) = call(V1_HIRE, &who, true, json!({ "key": k, "agent": { "handle": handle, "role": "worker" } }), None);
    assert_eq!(st, 200, "{via_body}");
    assert_eq!(via_body["instance"], first["instance"]);
    // Same key, different request → 409.
    let (st, conflict) = call(V1_HIRE, &who, true, json!({ "agent": { "handle": handle, "role": "other" } }), Some(&k));
    assert_eq!(st, 409, "{conflict}");
    assert_eq!(conflict["error"]["code"], "key_conflict");
    // A different key makes a second instance (handle suffixed).
    let (st, second) = call(V1_HIRE, &who, true, body, Some(&uniq("key")));
    assert_eq!(st, 200, "{second}");
    assert_ne!(second["instance"], first["instance"]);
    assert_eq!(second["members"][0]["handle"], format!("{handle}-2"));
}

#[test]
fn team_hire_makes_group_poc_and_star_edges_and_dry_run_predicts() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let team = uniq("Desk");
    let body = json!({
        "team": { "name": team },
        "members": [
            { "role": "sme" },
            { "role": "librarian", "lead": true },
            { "role": "field" },
        ],
    });
    let mut dry = body.clone();
    dry["dryRun"] = json!(true);
    let (st, plan) = call(V1_HIRE, &who, true, dry, None);
    assert_eq!(st, 200, "{plan}");
    assert_eq!(plan["dryRun"], true);
    let predicted: Vec<String> = plan["plan"]["members"].as_array().unwrap().iter().map(|m| m["handle"].as_str().unwrap().to_string()).collect();
    for h in &predicted {
        assert!(project_by_handle(h).is_none(), "dry run wrote {h}");
    }
    assert_eq!(count("SELECT COUNT(*) FROM project_groups WHERE name = ?1", &team), 0);

    let (st, v) = call(V1_HIRE, &who, true, body, Some(&uniq("k")));
    assert_eq!(st, 200, "{v}");
    let got: Vec<String> = v["members"].as_array().unwrap().iter().map(|m| m["handle"].as_str().unwrap().to_string()).collect();
    assert_eq!(got, predicted, "the plan names the handles");
    let slug = team.to_lowercase();
    assert_eq!(got[0], format!("{slug}-librarian"), "lead first, seeded {{team}}-{{role}}");
    assert_eq!(v["members"][0]["lead"], true);
    let gid = v["team"]["groupId"].as_str().expect("group").to_string();
    let lead_id = v["members"][0]["workspaceId"].as_str().unwrap().to_string();
    assert_eq!(count("SELECT COUNT(*) FROM project_groups WHERE id = ?1 AND poc_workspace_id IS NOT NULL", &gid), 1);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let poc: String = conn.query_row("SELECT poc_workspace_id FROM project_groups WHERE id = ?1", rusqlite::params![gid], |r| r.get(0)).unwrap();
    assert_eq!(poc, lead_id);
    let edges: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_relations WHERE origin_link_id = ?1 AND source_project_id = ?2",
            rusqlite::params![format!("inst:{}", v["instance"].as_str().unwrap()), lead_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(edges, 2, "star: lead ↔ each member");
    drop(conn);
    // Team name is unique on the box.
    let (st, dup) = call(V1_HIRE, &who, true, json!({ "team": { "name": team }, "members": [{ "role": "a" }, { "role": "b" }] }), None);
    assert_eq!(st, 409, "{dup}");
    assert_eq!(dup["error"]["code"], "group_name_taken");
}

#[test]
fn a_failure_at_every_step_leaves_zero_residue() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    for fail_at in 0..=3usize {
        let team = uniq("Roll");
        let body = json!({ "team": { "name": team }, "members": [{ "role": "lead", "lead": true }, { "role": "b" }, { "role": "c" }] });
        let mut dry = body.clone();
        dry["dryRun"] = json!(true);
        let (_, plan) = call(V1_HIRE, &who, true, dry, None);
        let planned: Vec<(String, String)> = plan["plan"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["handle"].as_str().unwrap().to_string(), m["path"].as_str().unwrap().to_string()))
            .collect();
        k2_core::hire::apply::set_fail_at(Some((&team, fail_at)));
        let (st, v) = call(V1_HIRE, &who, true, body, Some(&uniq("k")));
        k2_core::hire::apply::set_fail_at(None);
        assert_eq!(st, 500, "fail_at={fail_at}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("nothing was kept"), "{v}");
        for (i, (h, p)) in planned.iter().enumerate() {
            assert!(project_by_handle(h).is_none(), "fail_at={fail_at}: {h} still registered");
            assert!(!Path::new(p).exists(), "fail_at={fail_at}: folder {p} left behind");
            let retired = count("SELECT COUNT(*) FROM retired_workspace_handles WHERE handle = ?1", h);
            // Members registered before the failure burn their handle (0159).
            assert_eq!(retired, (i < fail_at) as i64, "fail_at={fail_at} {h}: retired={retired}");
        }
        assert_eq!(count("SELECT COUNT(*) FROM project_groups WHERE name = ?1", &team), 0, "group left behind");
        assert_eq!(
            count("SELECT COUNT(*) FROM agent_instances WHERE team_name = ?1 AND status = 'failed' AND idem_key IS NULL", &team),
            1,
            "ledger records the failure and releases the key"
        );
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM workspace_relations WHERE origin_link_id IN (SELECT 'inst:' || id FROM agent_instances WHERE team_name = ?1)",
                &team
            ),
            0
        );
    }
}

#[test]
fn shape_refusals() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    // Folder lines on app logins aren't built (T17): unknown field → 400.
    let (st, v) = call(V1_HIRE, &who, true, json!({ "agent": { "appLogin": { "app": "x", "folders": ["plans/"] } } }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (400, Some("usage")), "{v}");
    let (st, v) = call(V1_HIRE, &who, true, json!({ "agent": {}, "templateId": "plans-desk" }), None);
    assert_eq!(st, 400, "unknown field refuses: {v}");
    let (st, v) = call(V1_HIRE, &who, true, json!({ "members": [{}, {}] }), None);
    assert_eq!(st, 400, "a team needs a name: {v}");
    let r = handle_v1(V1_HIRE, &who, false, b"", &HashMap::new(), None);
    assert_eq!(r.status, "405 Method Not Allowed");
}

// ── API key ceiling ──────────────────────────────────────────────────

#[test]
fn a_key_without_the_hire_scope_gets_the_uniform_404() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let (_id, raw) = k2_core::api_keys::create_api_key("no-hire", None, Some("*"), None, None, Some(ApiCapabilities::all())).unwrap();
    let k = V1Principal::Api(k2_core::api_keys::resolve_api_key(&raw).unwrap());
    for p in V1_PATHS {
        let post = V1_POST_PATHS.contains(p);
        let r = handle_v1(p, &k, post, b"{}", &HashMap::new(), None);
        assert_eq!(r.status, "404 Not Found", "{p}: {}", r.body);
    }
    // A hire door with unreadable limits is no door either (fail closed).
    let (id2, raw2) = k2_core::api_keys::create_api_key_ex("bad-limits", None, None, None, None, Some(ApiCapabilities { hire: true, ..ApiCapabilities::none() }), Some("{\"x\":1}")).unwrap();
    let k2 = V1Principal::Api(k2_core::api_keys::resolve_api_key(&raw2).unwrap());
    let r = handle_v1(V1_HIRE, &k2, true, b"{}", &HashMap::new(), None);
    assert_eq!(r.status, "404 Not Found", "{id2}: {}", r.body);
}

#[test]
fn key_hires_inside_its_folder_and_ceiling() {
    let _home = k2_core::test_env::TempHome::new();
    let folder = uniq("acme");
    let k = hire_key(hire_only(), json!({ "folder": folder }), "[]");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": { "role": "plans" } , "onBehalfOf": "app-user" }), Some("k1"));
    assert_eq!(st, 200, "{v}");
    let path = v["members"][0]["path"].as_str().unwrap();
    assert!(Path::new(path).starts_with(ensure_test_root().join(&folder)), "pinned to the key's folder: {path}");
    // The same idempotency key under ANOTHER key is a different instance.
    let k_other = hire_key(hire_only(), json!({}), "[]");
    let (st, v2) = call(V1_HIRE, &k_other, true, json!({ "agent": { "role": "plans" }, "onBehalfOf": "app-user" }), Some("k1"));
    assert_eq!(st, 200, "{v2}");
    assert_ne!(v2["instance"], v["instance"]);
    // Audit names the key.
    let inst = v["instance"].as_str().unwrap();
    assert_eq!(
        count("SELECT COUNT(*) FROM hire_events WHERE instance_id = ?1 AND actor = 'api-key:' || api_key_id AND on_behalf_of = 'app-user'", inst),
        1
    );
}

#[test]
fn key_ceiling_refusals_write_nothing() {
    let _home = k2_core::test_env::TempHome::new();
    let k = hire_key(hire_only(), json!({ "maxMembersPerTeam": 2 }), "[]");
    let handle = uniq("ceil");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": { "handle": handle, "dbAccess": "read" } }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("exceeds_key")), "{v}");
    let (st, v) = call(
        V1_HIRE,
        &k,
        true,
        json!({ "team": { "name": uniq("T") }, "members": [{ "handle": handle }, {}, {}] }),
        None,
    );
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("exceeds_key")), "{v}");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": { "handle": handle }, "linkTo": [{ "workspace": "anything" }] }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("exceeds_key")), "no link scope: {v}");
    assert!(project_by_handle(&handle).is_none(), "nothing written");
    // db read is fine when the key has the db door and the ceiling allows it.
    let kd = hire_key(ApiCapabilities { db: true, ..hire_only() }, json!({ "db": "read" }), "[]");
    let (st, v) = call(V1_HIRE, &kd, true, json!({ "agent": { "dbAccess": "read" } }), None);
    assert_eq!(st, 200, "{v}");
    let path = v["members"][0]["path"].as_str().unwrap();
    assert_eq!(k2_core::workspace::settings::db_agent_access_for_path(path), "read");
}

#[test]
fn quotas_are_counted_from_the_ledger() {
    let _home = k2_core::test_env::TempHome::new();
    let k = hire_key(hire_only(), json!({ "maxMembersPerDay": 2, "maxLiveMembers": 5 }), "[]");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": {} }), None);
    assert_eq!(st, 200, "{v}");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": {} }), None);
    assert_eq!(st, 200, "{v}");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": {} }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (429, Some("quota_exceeded")), "{v}");
    let k_live = hire_key(hire_only(), json!({ "maxLiveMembers": 1 }), "[]");
    let (st, _) = call(V1_HIRE, &k_live, true, json!({ "agent": {} }), None);
    assert_eq!(st, 200);
    let (st, v) = call(V1_HIRE, &k_live, true, json!({ "agent": {} }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (429, Some("quota_exceeded")), "{v}");
}

#[test]
fn key_retire_needs_the_scope_and_its_own_instance() {
    let _home = k2_core::test_env::TempHome::new();
    let k = hire_key(hire_only(), json!({}), "[]");
    let (_, v) = call(V1_HIRE, &k, true, json!({ "agent": {} }), None);
    let inst = v["instance"].as_str().unwrap().to_string();
    let (st, r) = call(V1_RETIRE, &k, true, json!({ "instance": inst }), None);
    assert_eq!((st, r["error"]["code"].as_str()), (403, Some("exceeds_key")), "{r}");
    let other = hire_key(hire_only(), json!({ "retire": true }), "[]");
    let (st, r) = call(V1_RETIRE, &other, true, json!({ "instance": inst }), None);
    assert_eq!(st, 404, "never another key's instance: {r}");
    let (st, list) = get(V1_INSTANCES, &other, &[]);
    assert_eq!(st, 200);
    assert!(list["instances"].as_array().unwrap().iter().all(|i| i["id"] != inst.as_str()), "list is own only");
}

#[test]
fn reach_defaults_to_none_even_with_a_star_grant() {
    let _home = k2_core::test_env::TempHome::new();
    let k = hire_key(ApiCapabilities { canonical_message: true, host_sessions: true, ..hire_only() }, json!({}), "*");
    let (_, v) = call(V1_HIRE, &k, true, json!({ "agent": {} }), None);
    let handle = v["members"][0]["handle"].as_str().unwrap().to_string();
    use k2_core::hire::limits::ReachDoor;
    for door in [ReachDoor::Message, ReachDoor::HostSessions, ReachDoor::Other] {
        let r = crate::v1_sandboxes::resolve_authorized_workspace_door(&k, &handle, door);
        assert_eq!(r.err().map(|e| e.status), Some("404 Not Found"), "{door:?}");
    }
    let km = hire_key(ApiCapabilities { canonical_message: true, ..hire_only() }, json!({ "reach": "message" }), "[]");
    let (_, v) = call(V1_HIRE, &km, true, json!({ "agent": {} }), None);
    let h2 = v["members"][0]["handle"].as_str().unwrap().to_string();
    assert!(crate::v1_sandboxes::resolve_authorized_workspace_door(&km, &h2, ReachDoor::Message).is_ok(), "reach=message opens the message door");
    assert!(crate::v1_sandboxes::resolve_authorized_workspace_door(&km, &h2, ReachDoor::HostSessions).is_err());
}

// ── links ────────────────────────────────────────────────────────────

fn connections(path: &str, action: &str, target: &str) -> Result<String, String> {
    k2_core::connections::connections_for_actor(path, action, Some(target), None, true)
}

#[test]
fn links_add_and_remove_only_their_own_edges() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let (_, desk) = call(V1_HIRE, &who, true, json!({ "team": { "name": uniq("Desk") }, "members": [{ "role": "lib", "lead": true }, { "role": "sme" }] }), None);
    let (_, job) = call(V1_HIRE, &who, true, json!({ "agent": { "role": "joblead" } }), None);
    let job_ws = job["members"][0]["workspaceId"].as_str().unwrap().to_string();
    let job_path = job["members"][0]["path"].as_str().unwrap().to_string();
    let lead_handle = desk["members"][0]["handle"].as_str().unwrap().to_string();
    let lead_path = desk["members"][0]["path"].as_str().unwrap().to_string();
    let (st, l) = call(V1_LINK, &who, true, json!({ "team": desk["instance"], "workspace": job_ws, "scope": { "planSets": ["ps-1"] } }), None);
    assert_eq!(st, 200, "{l}");
    let link_id = l["link"]["id"].as_str().unwrap().to_string();
    // Linked agents may now message each other (one served_by row).
    assert_eq!(count("SELECT COUNT(*) FROM workspace_relations WHERE origin_link_id = ?1 AND relation_type = 'served_by'", &link_id), 1);
    // A hand-made connection next to it is written for real (HT10 add).
    let added = connections(&job_path, "add", &lead_handle).expect("manual add");
    assert!(!added.contains("\"noop\":true"), "a link row must not swallow a manual add: {added}");
    // Manual remove takes the hand-made row; then refuses the link row.
    connections(&job_path, "remove", &lead_handle).expect("manual remove");
    let err = connections(&job_path, "remove", &lead_handle).expect_err("link row refused");
    assert!(err.starts_with("managed_by_link"), "{err}");
    let rel_id: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM workspace_relations WHERE origin_link_id = ?1", rusqlite::params![link_id], |r| r.get(0)).unwrap()
    };
    let r = crate::agents_routes::handle_relations_delete(json!({ "id": rel_id }).to_string().as_bytes(), true);
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    // Lookups both ways.
    let (_, view) = get(V1_LINKS, &who, &[("workspace", &job_ws)]);
    assert_eq!(view["servedBy"][0]["id"], link_id.as_str(), "{view}");
    let (_, desk_view) = get(V1_LINKS, &who, &[("workspace", &lead_path)]);
    assert_eq!(desk_view["serves"][0]["id"], link_id.as_str(), "{desk_view}");
    assert_eq!(desk_view["serves"][0]["scope"]["planSets"][0], "ps-1");
    // Hand-made connection survives the unlink; link rows are gone.
    connections(&job_path, "add", &lead_handle).expect("manual add again");
    let (st, u) = call(V1_UNLINK, &who, true, json!({ "link": link_id }), None);
    assert_eq!(st, 200, "{u}");
    assert_eq!(count("SELECT COUNT(*) FROM workspace_relations WHERE origin_link_id = ?1", &link_id), 0);
    assert_eq!(
        count("SELECT COUNT(*) FROM workspace_relations WHERE source_project_id = ?1 AND origin_link_id IS NULL", &job_ws),
        1,
        "the manual connection survives"
    );
}

#[test]
fn key_links_only_what_it_created_or_was_named() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let (_, owners_desk) = call(V1_HIRE, &who, true, json!({ "agent": { "role": "desk" } }), None);
    let desk_ws = owners_desk["members"][0]["workspaceId"].as_str().unwrap().to_string();
    let k = hire_key(hire_only(), json!({ "link": true }), "[]");
    let (st, v) = call(V1_HIRE, &k, true, json!({ "agent": { "role": "job" }, "linkTo": [{ "workspace": desk_ws, "kind": "helper_of" }] }), None);
    assert_eq!((st, v["error"]["code"].as_str()), (403, Some("exceeds_key")), "not created by the key: {v}");
    let named = hire_key(hire_only(), json!({ "link": true, "linkTargets": [desk_ws] }), "[]");
    let (st, v) = call(V1_HIRE, &named, true, json!({ "agent": { "role": "job" }, "linkTo": [{ "workspace": desk_ws }] }), None);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["links"][0]["consumerWorkspaceId"], desk_ws.as_str());
    // Agents never link (passport on /cli).
    let r = handle_cli(
        CLI_LINK,
        Who::Agent { project_id: desk_ws.clone(), session_id: "s".into() },
        true,
        b"{}",
        &HashMap::new(),
        None,
    );
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
}

// ── /cli converge (k2 agent hire) ────────────────────────────────────

/// An agent's own workspace (made by the owner), with "Let this agent hire
/// agents" on and the passport's session counted as its main session.
fn hiring_agent(prefix: &str) -> (String, Who) {
    let path = ensure_test_root().join(uniq(prefix)).to_string_lossy().into_owned();
    let r = handle_cli(CLI_HIRE, Who::Owner, true, json!({ "path": path, "name": "Hirer" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    let id = v["id"].as_str().expect("id").to_string();
    k2_core::workspace::settings::set_agents_can_hire_agents(&id, true).expect("hire on");
    let sid = format!("main-{id}");
    test_main_session(&sid);
    (id.clone(), Who::Agent { project_id: id, session_id: sid })
}

fn code_of(r: &crate::cli_response::CliResponse) -> String {
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body));
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {}", r.body)).to_string()
}

/// 0.46.12 (Rosson): hiring and sidecars are two switches. An agent hires
/// only with "Let this agent hire agents" on (the sidecars switch doesn't
/// open it), only from its main session (a sidecar can't, switch or not),
/// and a template dry run needs only the main session. A shell tab is a
/// person typing.
#[test]
fn an_agent_hires_only_with_its_own_switch_and_only_from_its_main_session() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (ws, agent) = hiring_agent("hg");
    // Hiring off, sidecars on: refused, nothing written, with the switch's name.
    k2_core::workspace::settings::set_agents_can_hire_agents(&ws, false).unwrap();
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws, true).unwrap();
    let path = root.join(uniq("off")).to_string_lossy().into_owned();
    let r = handle_cli(CLI_HIRE, agent.clone(), true, json!({ "path": path, "name": "Nope" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "hire_off"), "{}", r.body);
    assert!(r.body.contains("Let this agent hire agents"), "{}", r.body);
    assert!(!Path::new(&path).exists(), "refused before any write");
    let name = uniq("Tpl");
    let r = handle_cli(CLI_HIRE, agent.clone(), true, json!({ "template": "writer", "name": name, "launch": false }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "hire_off"), "a template hire too: {}", r.body);
    // A template dry run writes nothing: the main session may show it.
    let (st, plan) = cli(CLI_HIRE, agent.clone(), true, json!({ "template": "writer", "name": name, "dryRun": true }), &[]);
    assert_eq!((st, plan["dryRun"].as_bool()), (200, Some(true)), "{plan}");
    assert!(!Path::new(plan["path"].as_str().unwrap()).exists(), "a dry run writes nothing");
    // Hiring on, sidecars off: the hire goes through.
    k2_core::workspace::settings::set_agents_can_hire_agents(&ws, true).unwrap();
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws, false).unwrap();
    let r = handle_cli(CLI_HIRE, agent.clone(), true, json!({ "path": path, "name": "Yes" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(Path::new(&path).is_dir());
    // A sidecar's passport (not the main session) never hires, switch on.
    let side = Who::Agent { project_id: ws.clone(), session_id: "tab-side-session".into() };
    let other = root.join(uniq("side")).to_string_lossy().into_owned();
    let r = handle_cli(CLI_HIRE, side.clone(), true, json!({ "path": other, "name": "Side" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "main_only"), "{}", r.body);
    assert!(!Path::new(&other).exists());
    let (st, e) = cli(CLI_HIRE, side, true, json!({ "template": "writer", "name": uniq("S"), "dryRun": true }), &[]);
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("main_only")), "{e}");
    // A plain shell tab is a person typing: no switch needed.
    k2_core::workspace::settings::set_agents_can_hire_agents(&ws, false).unwrap();
    let shell = Who::Shell { project_id: ws.clone(), session_id: "shell-session".into() };
    let by_hand = root.join(uniq("sh")).to_string_lossy().into_owned();
    let r = handle_cli(CLI_HIRE, shell, true, json!({ "path": by_hand, "name": "ByHand" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    // Every refusal is audited.
    let sink = super::AUDIT_SINK.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(sink.iter().any(|(e, _, o)| e == "agent-hire" && o == "refused:hire_off"), "{sink:?}");
    assert!(sink.iter().any(|(e, _, o)| e == "agent-hire" && o == "refused:main_only"), "{sink:?}");
}

/// 0.46.17 (an agent that had hired before 0.46.12 got `hire_off` on its
/// real run after a dry run that said "would apply", with no Ask line):
/// a plain dry run (no template) answers as the real hire would, writes
/// nothing either way, and is audited as a dry run; the `hire_off` hint
/// says how a person turns cards on while they're off for the agent, and
/// with cards on the refusal wrapper ends it with `agents.hire`'s Ask line.
#[test]
fn a_plain_dry_run_tells_the_truth_and_the_refusal_teaches_the_card() {
    let _home = k2_core::test_env::TempHome::new();
    let _env = k2_core::test_env::EnvVar::remove("K2_PERMISSION_CARDS");
    k2_core::permission::gate::set_server_switch(false);
    let root = ensure_test_root();
    let (ws, agent) = hiring_agent("dr");
    k2_core::workspace::settings::set_agents_can_hire_agents(&ws, false).unwrap();
    let path = root.join(uniq("dry")).to_string_lossy().into_owned();
    let dry = |who: &Who, body: Value| {
        let r = handle_cli(CLI_HIRE, who.clone(), true, body.to_string().as_bytes(), &HashMap::new(), None);
        let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body));
        (r, v)
    };

    // Switch off, cards off: the dry run is refused as the hire would be
    // (with or without a path), and the hint says how cards go on.
    for body in [json!({ "dryRun": true }), json!({ "path": path, "name": "Dry", "dryRun": true })] {
        let (r, v) = dry(&agent, body.clone());
        assert_eq!((r.status, v["error"]["code"].as_str()), ("403 Forbidden", Some("hire_off")), "{body}: {}", r.body);
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains("Let this agent hire agents"), "{hint}");
        assert!(hint.contains("Permission cards are off for you"), "cards off names it: {hint}");
        assert!(hint.contains(crate::apps::user_hold::PERMISSION_CARDS_ON_PATH), "and where they go on: {hint}");
        assert!(hint.ends_with("you ask with k2 permission request agents.hire."), "{hint}");
        assert!(v.get("permission").is_none(), "no card object while cards are off: {v}");
    }
    assert!(!Path::new(&path).exists(), "a dry run writes nothing");
    // The same refusal on the real run.
    let (r, v) = dry(&agent, json!({ "path": path, "name": "Real" }));
    assert_eq!((r.status, v["error"]["code"].as_str()), ("403 Forbidden", Some("hire_off")), "{}", r.body);
    assert!(!Path::new(&path).exists());
    let sink = super::AUDIT_SINK.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(sink.iter().any(|(e, _, o)| e == "agent-hire" && o == "dry-run:hire_off"), "{sink:?}");

    // A template dry run still skips the switch, and says the real hire
    // needs it (no card to ask with while cards are off).
    let (r, v) = dry(&agent, json!({ "template": "writer", "name": uniq("Tpl"), "dryRun": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(
        (v["dryRun"].as_bool(), v["hireSwitch"]["on"].as_bool(), v["hireSwitch"]["code"].as_str()),
        (Some(true), Some(false), Some("hire_off")),
        "{v}"
    );
    assert!(v["hireSwitch"]["hint"].as_str().expect("hint").contains("Permission cards are off for you"), "{v}");
    assert!(v["hireSwitch"].get("command").is_none(), "{v}");

    // Cards on for this agent (both switches): the hint is the plain one and
    // the wrapper ends the refusal with agents.hire's Ask line, dry run or not.
    let _cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws, true).expect("agent cards switch");
    let ask = "k2 permission request agents.hire --why \"<what you need it for>\"";
    for body in [json!({ "dryRun": true }), json!({ "path": path, "name": "Real" })] {
        let r = handle_cli(CLI_HIRE, agent.clone(), true, body.to_string().as_bytes(), &HashMap::new(), None);
        assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "hire_off"), "{}", r.body);
        assert!(!r.body.contains("Permission cards are off"), "{}", r.body);
        let taught = crate::permission::walls::teach_cell(r.status, "POST", CLI_HIRE, &HashMap::new(), body.to_string().as_bytes(), &ws, &r.body)
            .unwrap_or_else(|| panic!("no permission object: {}", r.body));
        let t: Value = serde_json::from_str(&taught).expect("json");
        assert_eq!((t["permission"]["action"].as_str(), t["permission"]["command"].as_str()), (Some("agents.hire"), Some(ask)), "{t}");
    }
    let (r, v) = dry(&agent, json!({ "template": "writer", "name": uniq("Tpl"), "dryRun": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(v["hireSwitch"]["command"].as_str(), Some(ask), "{v}");
    assert!(!v["hireSwitch"]["hint"].as_str().expect("hint").contains("Permission cards are off"), "{v}");

    // Switch on: the plain dry run passes the gate and still writes nothing;
    // the template dry run has nothing to add.
    k2_core::workspace::settings::set_agents_can_hire_agents(&ws, true).unwrap();
    let (r, v) = dry(&agent, json!({ "path": path, "name": "Dry", "dryRun": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!((v["dryRun"].as_bool(), v["hire"].as_str()), (Some(true), Some("allowed")), "{v}");
    assert!(!Path::new(&path).exists(), "a dry run that would pass writes nothing either");
    let (r, v) = dry(&agent, json!({ "template": "writer", "name": uniq("Tpl"), "dryRun": true }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(v.get("hireSwitch").is_none(), "{v}");
    // A sidecar's plain dry run: main_only, as its real hire.
    let side = Who::Agent { project_id: ws.clone(), session_id: "tab-dry-side".into() };
    let (r, v) = dry(&side, json!({ "dryRun": true }));
    assert_eq!((r.status, v["error"]["code"].as_str()), ("403 Forbidden", Some("main_only")), "{}", r.body);
    // The owner's plain dry run: the gate is an agent's only.
    let (r, v) = dry(&Who::Owner, json!({ "dryRun": true }));
    assert_eq!((r.status, v["hire"].as_str()), ("200 OK", Some("allowed")), "{}", r.body);
}

/// A plain dry run with a preset runs the real hire's preset check: a label
/// comes back as the id the hire would save, an id or command word as
/// given, and a preset this server doesn't have (a part of a label too) is
/// the same usage refusal with the server's presets. Nothing is written.
#[test]
fn a_plain_dry_run_checks_the_preset_as_the_hire_would() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("dp");
    let cursor_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM agent_presets WHERE label = 'Cursor Agent' AND is_built_in = 1", [], |r| r.get::<_, String>(0)).expect("built-in Cursor Agent")
    };
    for who in [agent.clone(), Who::Owner] {
        for (preset, want) in [("cursor agent", cursor_id.as_str()), (cursor_id.as_str(), cursor_id.as_str()), ("codex", "codex")] {
            let (st, v) = hire_as(&who, &json!({ "dryRun": true, "preset": preset }));
            assert_eq!((st, v["hire"].as_str(), v["preset"].as_str()), (200, Some("allowed"), Some(want)), "{preset}: {v}");
        }
        let (st, v) = hire_as(&who, &json!({ "dryRun": true }));
        assert_eq!((st, v["hire"].as_str()), (200, Some("allowed")), "{v}");
        assert!(v.get("preset").is_none(), "no preset asked, none answered: {v}");
        for bad in ["Cursor", NO_SUCH_PRESET] {
            let path = root.join(uniq("dp-bad")).to_string_lossy().into_owned();
            let (st, v) = hire_as(&who, &json!({ "path": path, "dryRun": true, "preset": bad }));
            let real = hire_as(&who, &json!({ "path": path, "preset": bad }));
            assert_eq!((st, v["error"]["code"].as_str()), (400, Some("usage")), "{bad}: {v}");
            assert_eq!((st, &v), (real.0, &real.1), "{bad}: the dry run refuses as the hire does");
            assert!(v["error"]["hint"].as_str().expect("hint").contains(&format!("preset {bad} no longer exists on this server; pick one of: ")), "{v}");
            assert!(!Path::new(&path).exists() && !registered(&path), "{bad}: nothing written");
        }
    }
}

#[test]
fn passport_converge_never_raises_db_access() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let path = ensure_test_root().join(uniq("cli")).to_string_lossy().into_owned();
    let (_, agent) = hiring_agent("cliw");
    let r = handle_cli(CLI_HIRE, agent.clone(), true, json!({ "path": path, "dbAccess": "write" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("owner_only"), "{}", r.body);
    assert!(!Path::new(&path).exists(), "refused before any write");
    let r = handle_cli(CLI_HIRE, agent, true, json!({ "path": path, "dbAccess": "off", "name": "Helper" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert!(v["steps"].as_array().unwrap().iter().any(|s| s["step"] == "register-workspace" && s["status"] == "done"), "{v}");
    let r = handle_cli(CLI_HIRE, Who::Owner, true, json!({ "path": path, "dbAccess": "write" }).to_string().as_bytes(), &HashMap::new(), None);
    assert_eq!(r.status, "200 OK", "the owner may: {}", r.body);
    assert_eq!(k2_core::workspace::settings::db_agent_access_for_path(&path), "write");
}

#[test]
fn people_only_routes_refuse_passports_and_members() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let agent = Who::Agent { project_id: "w".into(), session_id: "s".into() };
    for p in [CLI_INSTANCES_HIRE, CLI_INSTANCES_RETIRE, CLI_UNLINK] {
        let r = handle_cli(p, agent.clone(), true, b"{}", &HashMap::new(), None);
        assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
    }
    let member = Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member };
    let r = handle_cli(CLI_INSTANCES_HIRE, member, true, b"{}", &HashMap::new(), None);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = handle_cli(CLI_HIRE_ROOT, Who::Login { username: "a".into(), role: k2_core::connect_users::Role::Admin }, true, b"{\"root\":\"/tmp/x\"}", &HashMap::new(), None);
    assert_eq!(r.status, "403 Forbidden", "only the owner sets the root: {}", r.body);
}

// ── hire-key mint ────────────────────────────────────────────────────

#[test]
fn minting_a_hire_key_needs_a_fresh_owner_confirm() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let vault = crate::vault::ensure_test_vault();
    let body = json!({ "label": uniq("mint"), "hire": { "maxLiveMembers": 7 } });
    let r = crate::misc_routes::handle_api_key_create(body.to_string().as_bytes(), "owner-token");
    assert_eq!(r.status, "403 Forbidden", "owner token alone is not enough: {}", r.body);
    assert!(r.body.contains("vault_confirm_required"), "{}", r.body);

    let tok = vault.confirm.mint("hire-key-mint", None, "owner", "local-auth");
    let mut b = body.clone();
    b["confirmToken"] = json!(tok);
    let r = crate::misc_routes::handle_api_key_create(b.to_string().as_bytes(), "owner-token");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["capabilities"]["hire"], true);
    assert_eq!(v["capabilities"]["hostSessions"], false, "hire-only by default (HT3)");
    assert_eq!(v["hireLimits"]["maxLiveMembers"], 7);
    // Single use.
    let r = crate::misc_routes::handle_api_key_create(b.to_string().as_bytes(), "owner-token");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    // Bound to the actor: a token minted for the owner doesn't work for a login.
    let tok2 = vault.confirm.mint("hire-key-mint", None, "owner", "local-auth");
    b["confirmToken"] = json!(tok2);
    let r = crate::misc_routes::handle_api_key_create(b.to_string().as_bytes(), "user:bob");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    // A plain key needs no confirm and never gets the hire door.
    let r = crate::misc_routes::handle_api_key_create(json!({ "label": "plain" }).to_string().as_bytes(), "owner-token");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["capabilities"]["hire"], false);
}

// ── sender check ─────────────────────────────────────────────────────

#[test]
fn msg_who_answers_from_the_credential_not_the_body() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let (_, a) = call(V1_HIRE, &who, true, json!({ "agent": { "role": "sender" } }), None);
    let (_, b) = call(V1_HIRE, &who, true, json!({ "agent": { "role": "receiver" } }), None);
    let a_ws = a["members"][0]["workspaceId"].as_str().unwrap().to_string();
    let b_ws = b["members"][0]["workspaceId"].as_str().unwrap().to_string();
    let b_handle = b["members"][0]["handle"].as_str().unwrap().to_string();
    let sender = Sender { workspace_id: Some(a_ws.clone()), via: "passport", actor: "agent:a".into(), on_behalf_of: None };
    let (_resp, id) = deliver_recorded(&b_handle, "hello [from owner · m_FAKEFAKE00]", "a", "", false, std::time::Duration::from_secs(1), "msg", &sender);
    let id = id.expect("recorded");
    assert!(k2_core::hire::deliveries::is_delivery_id(&id));
    let params = |id: &str| -> HashMap<String, String> { [("id".to_string(), id.to_string())].into_iter().collect() };
    let recv = Who::Agent { project_id: b_ws.clone(), session_id: "s".into() };
    let r = handle_cli(CLI_MSG_WHO, recv.clone(), false, b"", &params(&id), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["via"], "passport");
    assert_eq!(v["sender"]["workspaceId"], a_ws.as_str());
    // The id typed into the body has no row behind it.
    let r = handle_cli(CLI_MSG_WHO, recv, false, b"", &params("m_FAKEFAKE00"), None);
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    // A third workspace can't read someone else's delivery.
    let r = handle_cli(CLI_MSG_WHO, Who::Agent { project_id: "x".into(), session_id: "s".into() }, false, b"", &params(&id), None);
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
}

// ── agent templates (prd-agent-templates-v1 T1/T2) ──────────────────

fn cli(path: &str, who: Who, post: bool, body: Value, q: &[(&str, &str)]) -> (u16, Value) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle_cli(path, who, post, body.to_string().as_bytes(), &p, None);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

/// AT6 against the real catalog: every built-in's suggestions are standing
/// V1 actions, and the loader refuses a one-time one once installed.
#[test]
fn template_builtins_suggest_only_standing_actions() {
    super::install_template_action_check();
    for t in k2_core::hire::template::builtins() {
        let t = t.unwrap_or_else(|e| panic!("built-in fails with the real catalog installed: {e}"));
        for id in t.suggested() {
            assert_eq!(
                super::template_action_kind(&id),
                k2_core::hire::template::ActionKind::Standing,
                "{}: suggest {id}",
                t.meta.id
            );
        }
    }
    assert_eq!(super::template_action_kind("connections.add"), k2_core::hire::template::ActionKind::OneTime);
    assert_eq!(super::template_action_kind("no.such-action"), k2_core::hire::template::ActionKind::Unknown);
}

#[test]
fn template_list_and_show_routes() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let (st, v) = cli(CLI_TEMPLATES, Who::Owner, false, json!({}), &[]);
    assert_eq!(st, 200, "{v}");
    let ids: Vec<&str> = v["templates"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids.first(), Some(&"general"), "General first: {ids:?}");
    assert_eq!(ids.len(), 10, "the ten built-ins: {ids:?}");
    // An agent passport reads them too (Kessel and leads hire).
    let agent = Who::Agent { project_id: "w".into(), session_id: "s".into() };
    let (st, v) = cli(CLI_TEMPLATES_SHOW, agent, false, json!({}), &[("id", "qa"), ("files", "1")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["template"]["id"], "reviewer", "qa is the Reviewer: {v}");
    assert!(v["template"]["files"]["ROLE.md"].as_str().unwrap().contains("You check work"));
    let (st, v) = cli(CLI_TEMPLATES_SHOW, Who::Owner, false, json!({}), &[("id", "boss")]);
    assert_eq!((st, v["error"]["code"].as_str()), (404, Some("unknown_template")), "{v}");
    let (st, _) = cli(CLI_TEMPLATES_SHOW, Who::Owner, false, json!({}), &[]);
    assert_eq!(st, 400);
    let r = handle_cli(CLI_TEMPLATES, Who::Owner, true, b"{}", &HashMap::new(), None);
    assert_eq!(r.status, "405 Method Not Allowed");
}

#[test]
fn template_hire_through_the_cli_door() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let name = uniq("Max");
    // A dry run with no folder: the review rows, nothing written.
    let (st, plan) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "preset": "codex", "dryRun": true }), &[]);
    assert_eq!(st, 200, "{plan}");
    assert_eq!(plan["dryRun"], true);
    let path = plan["path"].as_str().unwrap().to_string();
    assert!(Path::new(&path).starts_with(&root), "{path} under {}", root.display());
    assert!(!Path::new(&path).exists(), "a dry run writes nothing");
    assert!(plan["rows"].as_array().unwrap().iter().any(|r| r["step"] == "role" && r["status"] == "write"), "{plan}");
    // The hire.
    let (st, out) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "preset": "codex", "launch": false }), &[]);
    assert_eq!(st, 200, "{out}");
    assert_eq!(out["path"], path.as_str());
    assert_eq!(out["created"], true);
    assert!(out["hello"].as_str().is_some_and(|h| h.contains("Say hello")), "{out}");
    assert!(out["steps"].as_array().unwrap().iter().any(|s| s["step"] == "write-persona" && s["status"] == "done"), "{out}");
    assert_eq!(project_by_handle(out["handle"].as_str().unwrap()).map(|(id, _)| id), out["id"].as_str().map(str::to_string));
    // Again: nothing changes.
    let (_, again) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "preset": "codex" }), &[]);
    assert_eq!(again["changed"], false, "{again}");
    // Another template on the same agent is a conflict.
    let (st, e) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "ops", "path": path }), &[]);
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some("template_conflict")), "{e}");
    // Template and persona together are refused.
    let (st, _) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "ops", "persona": "x", "path": format!("{path}-x") }), &[]);
    assert_eq!(st, 400);
}

/// First-conversation questions: `show` lists them for a hiring agent, the
/// CLI door takes `answers` and reports which stay open, and an unknown
/// question id is refused with the valid ids.
#[test]
fn template_hire_answers_through_the_cli_door() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let agent = Who::Agent { project_id: "w".into(), session_id: "s".into() };
    let (st, v) = cli(CLI_TEMPLATES_SHOW, agent, false, json!({}), &[("id", "writer")]);
    assert_eq!(st, 200, "{v}");
    let ids: Vec<&str> = v["template"]["questions"].as_array().expect("questions").iter().map(|q| q["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["audience", "samples", "avoid", "drafts"], "{v}");
    assert!(v["template"]["questions"][0]["ask"].as_str().is_some_and(|a| a.ends_with('?')), "{v}");

    let name = uniq("Ada");
    let (st, e) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "answers": { "tone": "warm" } }), &[]);
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap().contains("audience, samples, avoid, drafts"), "{e}");

    let (st, out) = cli(
        CLI_HIRE,
        Who::Owner,
        true,
        json!({ "template": "writer", "name": name, "answers": { "audience": "Founders", "drafts": "The drafts folder" } }),
        &[],
    );
    assert_eq!(st, 200, "{out}");
    let open: Vec<&str> = out["questions"].as_array().expect("questions").iter().filter(|q| q["answered"] == false).map(|q| q["id"].as_str().unwrap()).collect();
    assert_eq!(open, vec!["samples", "avoid"], "{out}");
    let hello = out["hello"].as_str().expect("hello");
    assert!(hello.contains("samples or links") && !hello.contains("Who do you write for?"), "{hello}");
    let role = std::fs::read_to_string(Path::new(out["path"].as_str().unwrap()).join(".k2/agent/ROLE.md")).expect("ROLE.md");
    assert!(role.contains("- **Who do you write for?** Founders") && role.contains("The drafts folder"), "{role}");
}

/// A recipe member takes any template id (AT11); an API key can't hire a
/// system template (checked on the plan, before anything is written).
#[test]
fn template_recipe_member_takes_a_template() {
    let _home = k2_core::test_env::TempHome::new();
    let who = owner();
    let handle = uniq("rev");
    let (st, v) = call(V1_HIRE, &who, true, json!({ "agent": { "handle": handle, "template": "qa", "launch": false } }), None);
    assert_eq!(st, 200, "{v}");
    let (_, path) = project_by_handle(&handle).expect("hired");
    let lock = std::fs::read_to_string(Path::new(&path).join(".k2/agent/template.lock")).expect("lock");
    assert!(lock.contains("id = \"reviewer\""), "{lock}");
    assert!(Path::new(&path).join(".k2/wiki/Review notes.md").is_file());
    let (st, v) = call(V1_HIRE, &who, true, json!({ "agent": { "template": "boss" } }), None);
    assert_eq!(st, 400, "{v}");
}

fn hellos_to(handle: &str) -> Vec<(String, Option<String>)> {
    crate::template_hello::TEST_HELLOS
        .lock()
        .unwrap()
        .iter()
        .filter(|(h, _, _)| h == handle)
        .map(|(_, hello, onboard)| (hello.clone(), onboard.clone()))
        .collect()
}

/// T9: a hire that didn't launch keeps its hello in the lock; the agent's
/// first start (any door) sends it once, and later starts send nothing.
#[test]
fn template_hello_waits_for_the_first_start() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    // Hire without launch, then the agent starts: exactly one hello.
    let (st, out) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Late") }), &[]);
    assert_eq!(st, 200, "{out}");
    assert_eq!(out["helloDelivered"], false, "{out}");
    let handle = out["handle"].as_str().unwrap().to_string();
    let path = out["path"].as_str().unwrap().to_string();
    assert!(hellos_to(&handle).is_empty(), "nothing starts at the hire");
    assert!(k2_core::hire::template::lock::has_pending_hello(&path), "the lock holds the hello");
    assert!(crate::template_hello::launch_hello(&handle), "the first start sends it");
    let got = hellos_to(&handle);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].0.contains("Say hello"), "{got:?}");
    let lock = k2_core::hire::template::lock::read(&path).unwrap().unwrap();
    assert!(lock.hello_pending.is_none() && lock.hello_sent_at.is_some(), "{lock:?}");
    // Started again: nothing.
    assert!(!crate::template_hello::launch_hello(&handle));
    // A re-run with launch: nothing either.
    let (_, again) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": out["name"], "launch": true }), &[]);
    assert_eq!(again["helloDelivered"], false, "{again}");
    assert_eq!(hellos_to(&handle).len(), 1, "the hello goes once");

    // Hire without launch, then a re-run WITH launch: that run sends it,
    // then the onboard message; a later start sends nothing.
    let (_, quiet) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Rerun") }), &[]);
    let rh = quiet["handle"].as_str().unwrap().to_string();
    let (st, run) = cli(
        CLI_HIRE,
        Who::Owner,
        true,
        json!({ "template": "writer", "name": quiet["name"], "launch": true, "onboard": "Start with the blog." }),
        &[],
    );
    assert_eq!(st, 200, "{run}");
    assert_eq!((run["helloDelivered"].as_bool(), run["onboardQueued"].as_bool()), (Some(true), Some(true)), "{run}");
    let got = hellos_to(&rh);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1.as_deref(), Some("Start with the blog."));
    assert!(!crate::template_hello::launch_hello(&rh), "then nothing on a start");

    // Hire WITH launch: one hello at the hire, none on a later start.
    let (_, now) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Now"), "launch": true }), &[]);
    let nh = now["handle"].as_str().unwrap().to_string();
    assert_eq!(hellos_to(&nh).len(), 1);
    assert!(!crate::template_hello::launch_hello(&nh));
    assert_eq!(hellos_to(&nh).len(), 1);

    // A lock from before the field (no hello_pending): never a hello.
    let (_, old) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Old") }), &[]);
    let (oh, op) = (old["handle"].as_str().unwrap().to_string(), old["path"].as_str().unwrap().to_string());
    let text = std::fs::read_to_string(Path::new(&op).join(".k2/agent/template.lock")).unwrap();
    let mut l = k2_core::hire::template::lock::read(&op).unwrap().unwrap();
    l.hello_pending = None;
    k2_core::hire::template::lock::write(&op, &l).unwrap();
    let stripped = std::fs::read_to_string(Path::new(&op).join(".k2/agent/template.lock")).unwrap();
    assert!(text.contains("hello_pending") && !stripped.contains("hello_pending") && !stripped.contains("hello_sent_at"), "{stripped}");
    assert!(!crate::template_hello::launch_hello(&oh), "an old lock sends nothing");
    assert!(hellos_to(&oh).is_empty());
}

/// Daemon-first hello (PRD §3 Hello row): with `launch: true` a first
/// template hire starts the agent and delivers its hello once (then the
/// `onboard` message after it). `launch` absent (an older CLI) or false
/// starts nothing; a re-apply and a dry run send nothing.
#[test]
fn template_hire_delivers_the_hello_from_the_daemon_once() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let sent = |handle: &str| -> Vec<(String, Option<String>)> {
        crate::template_hello::TEST_HELLOS
            .lock()
            .unwrap()
            .iter()
            .filter(|(h, _, _)| h == handle)
            .map(|(_, hello, onboard)| (hello.clone(), onboard.clone()))
            .collect()
    };
    // `launch` absent: the old rule, nothing starts; the hello is returned.
    let (st, quiet) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Quiet") }), &[]);
    assert_eq!(st, 200, "{quiet}");
    assert_eq!(quiet["helloDelivered"], false, "{quiet}");
    assert!(quiet["hello"].is_string(), "{quiet}");
    assert!(sent(quiet["handle"].as_str().unwrap()).is_empty());
    // `launch: false`: the same.
    let (_, off) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": uniq("Off"), "launch": false }), &[]);
    assert_eq!(off["helloDelivered"], false, "{off}");
    assert!(sent(off["handle"].as_str().unwrap()).is_empty());

    let name = uniq("Hal");
    // A dry run sends nothing, even with launch.
    let (st, plan) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "launch": true, "dryRun": true }), &[]);
    assert_eq!(st, 200, "{plan}");
    assert_ne!(plan["helloDelivered"], true, "{plan}");
    assert!(sent(plan["handle"].as_str().unwrap()).is_empty());
    // `launch: true`, first create: delivered once, the onboard message after it.
    let (st, out) = cli(
        CLI_HIRE,
        Who::Owner,
        true,
        json!({ "template": "writer", "name": name, "launch": true, "onboard": "Start with the blog." }),
        &[],
    );
    assert_eq!(st, 200, "{out}");
    assert_eq!((out["helloDelivered"].as_bool(), out["onboardQueued"].as_bool()), (Some(true), Some(true)), "{out}");
    let handle = out["handle"].as_str().unwrap().to_string();
    let got = sent(&handle);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].0.contains("Say hello") && Some(got[0].0.as_str()) == out["hello"].as_str(), "{got:?}");
    assert_eq!(got[0].1.as_deref(), Some("Start with the blog."));
    // A re-apply sends nothing, even with launch.
    let (st, again) = cli(CLI_HIRE, Who::Owner, true, json!({ "template": "writer", "name": name, "launch": true }), &[]);
    assert_eq!(st, 200, "{again}");
    assert_eq!(again["helloDelivered"], false, "{again}");
    assert_eq!(sent(&handle).len(), 1, "the hello goes out once");
}

// ── a passport's hire only ever creates ─────────────────────────────

/// Everything a hire could change on a workspace: every file under its
/// folder (bytes), its `projects` row (every column) and its context layers.
fn workspace_state(path: &str) -> (Vec<(String, Vec<u8>)>, String, String) {
    let dir = Path::new(path);
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display())) {
            let p = e.expect("dir entry").path();
            let m = std::fs::symlink_metadata(&p).expect("metadata");
            let rel = p.strip_prefix(dir).expect("under the folder").to_string_lossy().into_owned();
            if m.file_type().is_symlink() {
                files.push((format!("{rel} ->"), std::fs::read_link(&p).expect("link").to_string_lossy().into_owned().into_bytes()));
            } else if m.is_dir() {
                files.push((format!("{rel}/"), Vec::new()));
                stack.push(p);
            } else {
                files.push((rel, std::fs::read(&p).expect("read file")));
            }
        }
    }
    files.sort();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = conn.prepare("SELECT * FROM projects WHERE path = ?1").expect("prepare");
        let n = st.column_count();
        st.query_row(rusqlite::params![path], |r| {
            let mut cols = Vec::new();
            for i in 0..n {
                cols.push(format!("{:?}", r.get_ref(i)?));
            }
            Ok(cols.join("|"))
        })
        .expect("a registered workspace")
    };
    let layers = format!("{:?}", k2_core::workspace::context_layers::list_layers(path).expect("layers"));
    (files, row, layers)
}

fn registered(path: &str) -> bool {
    count("SELECT COUNT(*) FROM projects WHERE path = ?1", path) > 0
}

/// What a hijack would send: a new name, another tool, context and a layer.
fn takeover(path: &str) -> Value {
    json!({
        "path": path,
        "name": "Taken Over",
        "preset": "codex",
        "defaultModel": "other-model",
        "context": ["wiki:hygiene"],
        "layers": [{ "label": "takeover", "markdown": "# Takeover\n" }],
    })
}

fn hire_as(who: &Who, body: &Value) -> (u16, Value) {
    let r = handle_cli(CLI_HIRE, who.clone(), true, body.to_string().as_bytes(), &HashMap::new(), None);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn assert_refused(who: &Who, body: Value, status: u16, code: &str) {
    let (st, v) = hire_as(who, &body);
    assert_eq!((st, v["error"]["code"].as_str()), (status, Some(code)), "{body}: {v}");
}

/// The owner's workspace at a fresh path under the hire root, with a name,
/// a tool and no context: what an agent must not change.
fn owners_workspace(tag: &str) -> String {
    let path = ensure_test_root().join(uniq(tag)).to_string_lossy().into_owned();
    let (st, v) = hire_as(&Who::Owner, &json!({ "path": path, "name": "Owned", "preset": "claude" }));
    assert_eq!(st, 200, "{v}");
    path
}

/// Steward ruling: a hire made with a passport only ever creates. An
/// existing workspace, by its path or any other spelling of it, is refused
/// with 403 and nothing about it changes (name, tool, model, context,
/// files). The template door too, dry run included.
#[test]
fn passport_hire_refuses_an_existing_workspace_in_every_spelling() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("oc");
    let theirs = owners_workspace("theirs");
    let before = workspace_state(&theirs);
    let base = Path::new(&theirs).file_name().unwrap().to_string_lossy().into_owned();
    let root_s = root.to_string_lossy().into_owned();
    let mut spellings = vec![
        theirs.clone(),
        format!("{theirs}/"),
        format!("{theirs}/."),
        format!("{root_s}/./{base}"),
        format!("{root_s}//{base}"),
    ];
    #[cfg(unix)]
    {
        let link = root.join(uniq("link-to-theirs"));
        std::os::unix::fs::symlink(&theirs, &link).expect("symlink");
        spellings.push(link.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "macos")]
    spellings.push(format!("{root_s}/{}", base.to_uppercase()));
    for p in &spellings {
        assert_refused(&agent, takeover(p), 403, "owner_only");
        assert_refused(&agent, json!({ "template": "writer", "path": p, "name": "Taken Over", "launch": false }), 403, "owner_only");
        assert_refused(&agent, json!({ "template": "writer", "path": p, "name": "Taken Over", "dryRun": true }), 403, "owner_only");
        assert_eq!(workspace_state(&theirs), before, "{p}: nothing about the workspace changed");
    }
    // One registration (LIKE matches ASCII case variants too), none for the link.
    assert_eq!(count("SELECT COUNT(*) FROM projects WHERE path LIKE '%' || ?1", &base), 1);
    assert!(!spellings.iter().any(|p| p.contains("link-to-theirs") && registered(p)), "the link is not registered");
    let (_, v) = hire_as(&agent, &takeover(&theirs));
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains("only makes a new workspace") && hint.contains("A person changes an existing workspace"), "{hint}");
    // A shell tab's passport is a passport too.
    let shell = match &agent {
        Who::Agent { project_id, .. } => Who::Shell { project_id: project_id.clone(), session_id: "shell-oc".into() },
        _ => unreachable!(),
    };
    assert_refused(&shell, takeover(&theirs), 403, "owner_only");
    assert_eq!(workspace_state(&theirs), before);
    // The owner converges an existing workspace, as before.
    let (st, v) = hire_as(&Who::Owner, &json!({ "path": theirs, "name": "Renamed By Owner" }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(k2_core::workspace::display::agent_display_name(&theirs), "Renamed By Owner");
}

/// A folder that is already there is never adopted: with files in it,
/// empty, a file in its place, or a registered workspace whose folder is
/// gone. No `..`, and no symlink on the way to the new folder.
#[test]
fn passport_hire_refuses_any_path_that_is_already_there() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("ot");
    let full = root.join(uniq("full"));
    std::fs::create_dir_all(&full).unwrap();
    std::fs::write(full.join("notes.txt"), b"mine").unwrap();
    let empty = root.join(uniq("empty"));
    std::fs::create_dir_all(&empty).unwrap();
    let file = root.join(uniq("file"));
    std::fs::write(&file, b"a file").unwrap();
    for p in [&full, &empty, &file] {
        let s = p.to_string_lossy().into_owned();
        assert_refused(&agent, takeover(&s), 403, "owner_only");
        assert_refused(&agent, json!({ "template": "writer", "path": s, "name": "Taken Over", "dryRun": true }), 403, "owner_only");
        assert!(!registered(&s), "{s}: not registered");
    }
    assert_eq!(std::fs::read_dir(&full).unwrap().count(), 1, "the full folder is as it was");
    assert_eq!(std::fs::read(full.join("notes.txt")).unwrap(), b"mine");
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "the empty folder is as it was");
    assert_eq!(std::fs::read(&file).unwrap(), b"a file");
    // A registered workspace whose folder was removed: the agent can't
    // make the folder again and take the registration.
    let gone = owners_workspace("gone");
    std::fs::remove_dir_all(&gone).unwrap();
    assert_refused(&agent, takeover(&gone), 403, "owner_only");
    assert_refused(&agent, json!({ "template": "writer", "path": gone, "name": "Taken Over", "dryRun": true }), 403, "owner_only");
    assert!(!Path::new(&gone).exists(), "nothing made at {gone}");
    // `..` is refused by its shape.
    let dotdot = format!("{}/x/../{}", root.display(), uniq("dd"));
    assert_refused(&agent, takeover(&dotdot), 400, "usage");
    // A symlink on the way: the new folder would land inside another
    // workspace.
    #[cfg(unix)]
    {
        let theirs = owners_workspace("inside");
        let link = root.join(uniq("via"));
        std::os::unix::fs::symlink(&theirs, &link).expect("symlink");
        let sub = link.join("new-agent");
        assert_refused(&agent, takeover(&sub.to_string_lossy()), 403, "owner_only");
        assert!(!Path::new(&theirs).join("new-agent").exists(), "nothing made inside the other workspace");
    }
}

/// A fresh path works as today, and the new workspace gets the name, tool
/// and context; running the same hire again is refused (it exists now).
#[test]
fn passport_hire_on_a_fresh_path_creates_it() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("of");
    let path = root.join(uniq("fresh")).join("deeper").to_string_lossy().into_owned();
    let (st, v) = hire_as(&agent, &json!({ "path": format!("{path}/"), "name": "Fresh Hire", "preset": "codex", "context": ["wiki:hygiene"] }));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["created"].as_bool(), v["path"].as_str()), (Some(true), Some(path.as_str())), "{v}");
    let steps = v["steps"].as_array().expect("steps");
    for step in ["create-dir", "register-workspace", "set-name", "set-default-agent"] {
        assert!(steps.iter().any(|s| s["step"] == step && s["status"] == "done"), "{step}: {v}");
    }
    assert_eq!(k2_core::workspace::display::agent_display_name(&path), "Fresh Hire");
    assert_eq!(count("SELECT COUNT(*) FROM projects WHERE path = ?1 AND default_agent = 'codex'", &path), 1);
    let layers = k2_core::workspace::context_layers::list_layers(&path).expect("layers");
    assert!(!layers.is_empty(), "context added: {layers:?}");
    let after = workspace_state(&path);
    assert_refused(&agent, takeover(&path), 403, "owner_only");
    assert_eq!(workspace_state(&path), after, "a re-run changes nothing");
    // A template hire at a fresh path, too.
    let tpl = root.join(uniq("fresh-tpl")).to_string_lossy().into_owned();
    let (st, v) = hire_as(&agent, &json!({ "template": "writer", "path": tpl, "name": "Tpl Hire", "launch": false }));
    assert_eq!((st, v["created"].as_bool()), (200, Some(true)), "{v}");
}

/// The check and the create are one step: a folder or link that appears
/// after the check is refused by the create, never adopted.
#[test]
fn passport_hire_claims_the_folder_in_one_step() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let late = root.join(uniq("late")).to_string_lossy().into_owned();
    let checked = k2_core::hire::converge::create_only_path(&late).expect("free at check time");
    std::fs::create_dir(&checked).unwrap();
    std::fs::write(Path::new(&checked).join("theirs.txt"), b"x").unwrap();
    let e = k2_core::hire::converge::claim_new_folder(&checked).unwrap_err();
    assert_eq!((e.status, e.code), (403, "owner_only"), "{e:?}");
    assert_eq!(std::fs::read_dir(&checked).unwrap().count(), 1, "the folder that appeared is untouched");
    #[cfg(unix)]
    {
        let other = owners_workspace("racer");
        let link = root.join(uniq("late-link")).to_string_lossy().into_owned();
        let checked = k2_core::hire::converge::create_only_path(&link).expect("free at check time");
        std::os::unix::fs::symlink(&other, &checked).expect("symlink");
        let e = k2_core::hire::converge::claim_new_folder(&checked).unwrap_err();
        assert_eq!((e.status, e.code), (403, "owner_only"), "{e:?}");
    }
    let fresh = root.join(uniq("claim")).to_string_lossy().into_owned();
    k2_core::hire::converge::claim_new_folder(&k2_core::hire::converge::create_only_path(&fresh).expect("free")).expect("claimed");
    assert!(Path::new(&fresh).is_dir());
}

// ── the hire's preset must exist ─────────────────────────────────────

/// Well formed, but no such preset.
const NO_SUCH_PRESET: &str = "00000000-0000-4000-8000-0000000000ff";

fn assert_no_such_preset(st: u16, v: &Value) {
    assert_eq!((st, v["error"]["code"].as_str()), (400, Some("usage")), "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains(&format!("preset {NO_SUCH_PRESET} no longer exists on this server; pick one of: ")), "{hint}");
    // The fix is named: the server's presets, by command word and id, in
    // `k2 preset list` order.
    let list: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentPreset::list(&conn)
            .expect("presets")
            .iter()
            .map(|p| format!("{} ({})", p.command.split_whitespace().next().expect("a command"), p.id))
            .collect()
    };
    assert!(list.iter().any(|c| c.starts_with("codex (")), "the built-ins are seeded: {list:?}");
    assert!(hint.ends_with(&format!("pick one of: {}", list.join(", "))), "{hint}");
}

/// K2's ruling: every hire door refuses a preset that doesn't exist (400,
/// nothing saved, nothing created), for every caller. A preset id or its
/// command (`codex`) still works.
#[test]
fn preset_hire_refuses_a_preset_that_does_not_exist() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("np");
    // A new workspace, plain and template, dry run too: owner and passport.
    for who in [Who::Owner, agent.clone()] {
        let path = root.join(uniq("np-new")).to_string_lossy().into_owned();
        for body in [
            json!({ "path": path, "name": "No Preset", "preset": NO_SUCH_PRESET }),
            json!({ "template": "writer", "path": path, "name": "No Preset", "preset": NO_SUCH_PRESET, "launch": false }),
            json!({ "template": "writer", "path": path, "name": "No Preset", "preset": NO_SUCH_PRESET, "dryRun": true }),
        ] {
            let (st, v) = hire_as(&who, &body);
            assert_no_such_preset(st, &v);
            assert!(!Path::new(&path).exists() && !registered(&path), "{body}: nothing created");
        }
    }
    // The owner on an existing workspace: nothing saved.
    let theirs = owners_workspace("np-old");
    let before = workspace_state(&theirs);
    let (st, v) = hire_as(&Who::Owner, &json!({ "path": theirs, "name": "Renamed", "preset": NO_SUCH_PRESET }));
    assert_no_such_preset(st, &v);
    assert_eq!(workspace_state(&theirs), before, "nothing saved");
    // The recipe door: the owner and an API key.
    for who in [owner(), hire_key(hire_only(), json!({}), "[]")] {
        let handle = uniq("np-key");
        let (st, v) = call(V1_HIRE, &who, true, json!({ "agent": { "handle": handle, "preset": NO_SUCH_PRESET } }), None);
        assert_no_such_preset(st, &v);
        assert!(v["error"]["hint"].as_str().unwrap().starts_with("members[0].preset: "), "{v}");
        assert!(project_by_handle(&handle).is_none(), "nothing created");
    }
    // A real preset by its id, and by its command.
    let codex_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM agent_presets WHERE label = 'Codex' AND is_built_in = 1", [], |r| r.get::<_, String>(0)).expect("built-in Codex")
    };
    for preset in [codex_id.as_str(), "codex"] {
        let path = root.join(uniq("np-ok")).to_string_lossy().into_owned();
        let (st, v) = hire_as(&agent, &json!({ "path": path, "preset": preset }));
        assert_eq!(st, 200, "{preset}: {v}");
        let saved: Option<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT default_agent FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get(0)).expect("registered")
        };
        assert_eq!(saved.as_deref(), Some(preset), "saved as given");
    }
}

fn saved_default_agent(path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT default_agent FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get(0)).expect("registered")
}

/// Labels are what people type: a preset's label, in full and in any case,
/// hires and saves the preset's id (the launch resolver reads ids and
/// command words, not labels). Part of a label is not a label: refused with
/// the server's presets, nothing made. A label two presets share is refused
/// with their ids. The plain door, the template door and the recipe alike.
#[test]
fn preset_hire_takes_a_label_and_saves_its_id() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("pl");
    let cursor_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id FROM agent_presets WHERE label = 'Cursor Agent' AND is_built_in = 1", [], |r| r.get::<_, String>(0)).expect("built-in Cursor Agent")
    };
    for who in [Who::Owner, agent.clone()] {
        for label in ["Cursor Agent", "cursor agent", "CURSOR AGENT"] {
            let path = root.join(uniq("pl-ok")).to_string_lossy().into_owned();
            let (st, v) = hire_as(&who, &json!({ "path": path, "preset": label }));
            assert_eq!(st, 200, "{label}: {v}");
            assert_eq!(saved_default_agent(&path).as_deref(), Some(cursor_id.as_str()), "{label}: the id is saved");
        }
        let path = root.join(uniq("pl-tpl")).to_string_lossy().into_owned();
        let (st, v) = hire_as(&who, &json!({ "template": "writer", "path": path, "name": "Label Tpl", "preset": "Cursor Agent", "launch": false }));
        assert_eq!(st, 200, "template: {v}");
        assert_eq!(saved_default_agent(&path).as_deref(), Some(cursor_id.as_str()), "template: the id is saved");
        // A prefix, or a label with a word missing, is no label.
        for part in ["Cursor", "cursor ag", "Agent"] {
            let path = root.join(uniq("pl-part")).to_string_lossy().into_owned();
            let (st, v) = hire_as(&who, &json!({ "path": path, "preset": part }));
            assert_eq!((st, v["error"]["code"].as_str()), (400, Some("usage")), "{part}: {v}");
            let hint = v["error"]["hint"].as_str().expect("hint");
            assert!(hint.contains(&format!("preset {part} no longer exists on this server; pick one of: ")), "{hint}");
            assert!(hint.contains(&format!("cursor-agent ({cursor_id})")), "the list names it: {hint}");
            assert!(!Path::new(&path).exists() && !registered(&path), "{part}: nothing created");
        }
    }
    // The recipe door saves the id too.
    let handle = uniq("pl-key");
    let (st, v) = call(V1_HIRE, &owner(), true, json!({ "agent": { "handle": handle, "preset": "cursor agent" } }), None);
    assert_eq!(st, 200, "{v}");
    let (_, path) = project_by_handle(&handle).expect("hired");
    assert_eq!(saved_default_agent(&path).as_deref(), Some(cursor_id.as_str()), "recipe: the id is saved");
    // Two presets with one label: refused with both ids, nothing made.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, order) in [("twin-label-a", 90), ("twin-label-b", 91)] {
            k2_core::db::schema::AgentPreset::create(&conn, id, "Twin Tool", &format!("{id} --go"), None, 1, order, 0).expect("custom preset");
        }
    }
    let path = root.join(uniq("pl-twin")).to_string_lossy().into_owned();
    let (st, v) = hire_as(&Who::Owner, &json!({ "path": path, "preset": "twin tool" }));
    assert_eq!((st, v["error"]["code"].as_str()), (400, Some("usage")), "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.ends_with("preset twin tool is the label of more than one preset; pass one of their ids: twin-label-a, twin-label-b"), "{hint}");
    assert!(!Path::new(&path).exists() && !registered(&path), "twin: nothing created");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM agent_presets WHERE id IN ('twin-label-a', 'twin-label-b')", []).expect("remove the twins");
}

/// K2's ruling: an agent hires only inside the hire root. Outside it the
/// hire is refused (403 `outside_hire_root`, saying where agents may hire)
/// with nothing made, the template door and its dry run too; inside it
/// works; the owner still hires anywhere.
#[test]
fn passport_hire_stays_inside_the_hire_root() {
    let home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    let (_, agent) = hiring_agent("hr");
    let outside = home.path().join(uniq("outside")).to_string_lossy().into_owned();
    for body in [
        json!({ "path": outside, "name": "Out" }),
        json!({ "template": "writer", "path": outside, "name": "Out", "launch": false }),
        json!({ "template": "writer", "path": outside, "name": "Out", "dryRun": true }),
    ] {
        let (st, v) = hire_as(&agent, &body);
        assert_eq!((st, v["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{body}: {v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains(&format!("An agent hires only inside the hire root {}", root.display())), "{hint}");
        assert!(!Path::new(&outside).exists() && !registered(&outside), "{body}: nothing made");
    }
    // The hire root itself is not a new workspace's folder.
    assert_refused(&agent, json!({ "path": root.to_string_lossy() }), 403, "owner_only");
    let inside = root.join(uniq("inside")).to_string_lossy().into_owned();
    let (st, v) = hire_as(&agent, &json!({ "path": inside, "name": "In" }));
    assert_eq!((st, v["created"].as_bool()), (200, Some(true)), "{v}");
    let (st, v) = hire_as(&Who::Owner, &json!({ "path": outside, "name": "Owner Out" }));
    assert_eq!((st, v["created"].as_bool()), (200, Some(true)), "the owner hires anywhere: {v}");
}

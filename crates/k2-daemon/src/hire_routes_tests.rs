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
        k2_core::heartbeats::k2so_heartbeat_list(path.clone()).expect("hb list").iter().any(|h| h.name == "reindex"),
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

#[test]
fn passport_converge_never_raises_db_access() {
    ensure_test_root();
    let path = ensure_test_root().join(uniq("cli")).to_string_lossy().into_owned();
    let agent = Who::Agent { project_id: "some-ws".into(), session_id: "s".into() };
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

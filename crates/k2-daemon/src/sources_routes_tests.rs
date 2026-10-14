//! B1 route behaviour, called the way both transports call it (the role
//! floor is `role_gate`'s and is tested in route_policy).

use super::*;
use k2_core::sources::extract::{ProjectedItem, Projection};
use k2_core::sources::Health;
use serde_json::{json, Value};

fn agent(p: &str) -> Who {
    Who::Agent { project_id: p.into(), session_id: format!("sid-{p}") }
}

fn login(u: &str, role: Role) -> Who {
    Who::Login { username: u.into(), role }
}

fn call(path: &str, who: &Who, body: Value) -> (u16, Value) {
    let r = handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    (code(r.status), serde_json::from_str(&r.body).expect("json"))
}

fn get(path: &str, who: &Who, params: &[(&str, &str)]) -> (u16, Value) {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(path, who.clone(), b"", &p);
    (code(r.status), serde_json::from_str(&r.body).expect("json"))
}

fn code(status: &str) -> u16 {
    status[..3].parse().unwrap()
}

fn err(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn fleet(name: &str) -> Value {
    json!({"name": name, "label": "Fleet", "everySecs": 60,
           "fetch": {"targets": [{"key": "a", "url": "https://a.example.com/boot-status"},
                                 {"key": "b", "url": "https://b.example.com/boot-status"}]},
           "extract": {"items": {"itemFields": {"version": "/version"}}, "aggregates": {"n": {"op": "count"}}, "trend": ["n"]}})
}

fn deploys(name: &str) -> Value {
    json!({"name": name, "label": "Deploys", "everySecs": 180, "secret": "VERCEL_RO",
           "fetch": {"url": "https://api.vercel.com/v6/deployments?teamId=team_x&limit=5"},
           "extract": {"items": {"pointer": "/deployments", "itemKey": "/uid", "itemFields": {"state": "/state"}}}})
}

struct Env {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
}

fn env() -> Env {
    let home = crate::test_support::TempHome::new();
    Env { _db: k2_core::db::scoped_for_test(), _home: home }
}

#[test]
fn people_add_agents_propose_and_list_shows_both() {
    let _e = env();
    let (s, v) = call("/cli/sources/add", &Who::Owner, fleet("fleet"));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active");
    assert_eq!(v["approval"], "one_click");
    let (s, v) = call("/cli/sources/propose", &agent("p1"), fleet("fleet2"));
    assert_eq!((s, v["status"].as_str()), (200, Some("proposed")), "{v}");
    let (s, v) = call("/cli/sources/add", &agent("p1"), fleet("fleet3"));
    assert_eq!((s, err(&v)), (403, "person_only"));
    let (s, v) = call("/cli/sources/propose", &agent("p1"), deploys("deploys"));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["approval"], "needs_confirm");
    let (s, v) = call("/cli/sources/add", &Who::Owner, deploys("deploys2"));
    assert_eq!((v["status"].as_str(), v["approval"].as_str()), (Some("proposed"), Some("needs_confirm")), "{s} {v}");

    let (s, v) = get("/cli/sources/list", &Who::Owner, &[]);
    assert_eq!(s, 200);
    let names: Vec<&str> = v["sources"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names[0], "fleet", "active first");
    assert_eq!(names.len(), 4);
    assert!(v["serverNow"].is_i64() && v["rev"].as_i64().unwrap() > 0);
    let fleet_row = v["sources"].as_array().unwrap().iter().find(|r| r["name"] == "fleet").unwrap();
    assert_eq!(fleet_row["targets"], 2);
    assert_eq!(fleet_row["view"]["display"], "pending");
    let dep = v["sources"].as_array().unwrap().iter().find(|r| r["name"] == "deploys").unwrap();
    assert_eq!(dep["secret"], "VERCEL_RO");
    assert_eq!((dep["viewers"].as_str(), dep["agentAccess"].as_str()), (Some("admin"), Some("shape")));
}

#[test]
fn lan_and_http_sources_are_owner_only() {
    let _e = env();
    let lan = json!({"name": "nas", "label": "NAS", "everySecs": 60, "fetch": {"url": "http://192.168.1.5:8080/status"}});
    for who in [login("adm", Role::Admin), agent("p1")] {
        let path = if matches!(who, Who::Login { .. }) { "/cli/sources/add" } else { "/cli/sources/propose" };
        let (s, v) = call(path, &who, lan.clone());
        assert_eq!((s, err(&v)), (403, "lan_owner_only"), "{who:?}");
    }
    let mut meta = lan.clone();
    meta["fetch"]["url"] = "http://169.254.169.254/latest/meta-data/".into();
    meta["name"] = "meta".into();
    assert_eq!(err(&call("/cli/sources/add", &login("adm", Role::Admin), meta).1), "lan_owner_only");
    let (s, v) = call("/cli/sources/add", &login("boss", Role::Owner), lan.clone());
    assert_eq!(s, 200, "{v}");
    let mut l2 = lan;
    l2["name"] = "nas2".into();
    let (s, v) = call("/cli/sources/add", &Who::Owner, l2);
    assert_eq!(s, 200, "{v}");
    let (_, v) = call("/cli/sources/add", &Who::Owner, json!({"name": "h", "label": "H", "everySecs": 60, "fetch": {"url": "http://example.com/"}}));
    assert_eq!((err(&v), v["error"]["specCode"].as_str()), ("bad_spec", Some("https_only")));
}

#[test]
fn viewers_floor_masking_and_ids() {
    let _e = env();
    call("/cli/sources/add", &Who::Owner, deploys("deploys"));
    let db = k2_core::db::shared();
    {
        let conn = db.lock();
        let row = store::get_by_name(&conn, "deploys").unwrap().unwrap();
        let (mut st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
        st.phase = k2_core::sources::Phase::Live;
        st.health = Health::Ok;
        let p = Projection {
            fields: Default::default(),
            items: vec![ProjectedItem { key: "d1".into(), fields: [("state".to_string(), json!("READY"))].into() }],
        };
        store::save_state(&conn, &st, Some(&p)).unwrap();
    }
    let (s, v) = get("/cli/sources/read", &login("m", Role::Member), &[("name", "deploys")]);
    assert_eq!((s, err(&v), v["error"]["role"].as_str()), (403, "cap_not_granted", Some("admin")));
    let (_, v) = get("/cli/sources/read", &login("a", Role::Admin), &[("name", "deploys")]);
    assert_eq!(v["items"][0]["fields"]["state"], "READY");
    let (_, v) = get("/cli/sources/read", &agent("p1"), &[("name", "deploys")]);
    assert_eq!((v["masked"].as_bool(), v["items"][0]["fields"]["state"].as_str()), (Some(true), Some("•••")));
    // A person's unmasked read of a shape source is audited once an hour.
    get("/cli/sources/read", &Who::Owner, &[("name", "deploys")]);
    get("/cli/sources/read", &Who::Owner, &[("name", "deploys")]);
    {
        let conn = db.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM source_audit WHERE action = 'unmasked_read' AND actor = 'owner'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
    let (_, v) = get("/cli/sources/show", &Who::Owner, &[("name", "deploys")]);
    assert!(v["source"]["id"].is_string(), "persons get the id (vault confirm secretId)");
    assert!(v["source"]["fetch"]["url"].as_str().unwrap().contains("team_x"));
    let (_, v) = get("/cli/sources/show", &agent("p1"), &[("name", "deploys")]);
    assert!(v["source"].get("id").is_none());
    assert!(!v["source"]["fetch"]["url"].as_str().unwrap().contains("team_x"), "agents see masked query values");
    let (_, v) = get("/cli/sources/list", &login("m", Role::Member), &[]);
    assert!(v["sources"].as_array().unwrap().is_empty(), "unreadable rows are left out");
}

#[test]
fn names_tombstones_and_holds() {
    let _e = env();
    call("/cli/sources/add", &Who::Owner, fleet("fleet"));
    assert_eq!(err(&call("/cli/sources/add", &Who::Owner, fleet("fleet")).1), "name_taken");
    let (s, v) = call("/cli/sources/remove", &Who::Owner, json!({"name": "fleet"}));
    assert_eq!(s, 200, "{v}");
    assert!(v["heldUntilAt"].as_i64().unwrap() > now());
    let (s, v) = get("/cli/sources/read", &Who::Owner, &[("name", "fleet")]);
    assert_eq!((s, err(&v)), (410, "removed"));
    assert!(v["error"]["removedAt"].is_i64());
    let (s, v) = call("/cli/sources/add", &Who::Owner, fleet("fleet"));
    assert_eq!((s, err(&v)), (409, "name_held"));
    assert!(v["error"]["untilAt"].is_i64());
    let (s, v) = get("/cli/sources/read", &Who::Owner, &[("name", "nope")]);
    assert_eq!((s, err(&v)), (404, "source_not_found"));
}

#[test]
fn agents_change_only_their_own_unpinned_sources() {
    let _e = env();
    call("/cli/sources/propose", &agent("p1"), fleet("mine"));
    call("/cli/sources/add", &Who::Owner, fleet("theirs"));
    let set = json!({"label": "Renamed"});
    assert_eq!(err(&call("/cli/sources/edit", &agent("p1"), json!({"name": "theirs", "set": set})).1), "not_own");
    assert_eq!(err(&call("/cli/sources/remove", &agent("p1"), json!({"name": "theirs"})).1), "not_own");
    let (s, v) = call("/cli/sources/edit", &agent("p1"), json!({"name": "mine", "set": set}));
    assert_eq!(s, 200, "{v}");
    for f in ["viewers", "agentAccess", "keep", "secret", "pinned"] {
        let (s, v) = call("/cli/sources/edit", &agent("p1"), json!({"name": "mine", "set": {f: "admin"}}));
        assert_eq!((s, err(&v)), (403, "field_person_only"), "{f}");
    }
    assert_eq!(err(&call("/cli/sources/edit", &Who::Owner, json!({"name": "mine", "set": {"name": "x"}})).1), "bad_request");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = store::get_by_name(&conn, "mine").unwrap().unwrap();
        store::set_pin(&conn, &row.id, true, "owner", now()).unwrap();
    }
    let (s, v) = call("/cli/sources/edit", &agent("p1"), json!({"name": "mine", "set": set}));
    assert_eq!((s, err(&v)), (423, "source_pinned"));
    assert_eq!(err(&call("/cli/sources/remove", &agent("p1"), json!({"name": "mine"})).1), "source_pinned");
    let (s, _) = call("/cli/sources/edit", &Who::Owner, json!({"name": "mine", "set": set}));
    assert_eq!(s, 200, "a person still edits a pinned source");
}

#[test]
fn an_agent_edit_of_a_secret_bound_source_sends_it_back_and_unbinds() {
    let _e = env();
    call("/cli/sources/propose", &agent("p1"), deploys("deploys"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = store::get_by_name(&conn, "deploys").unwrap().unwrap();
        store::mark_approved(&conn, &row.id, "owner", &row.spec_hash, now()).unwrap();
        store::set_secret_bound(&conn, &row.id, true, now()).unwrap();
    }
    // A label change keeps it running.
    let (_, v) = call("/cli/sources/edit", &agent("p1"), json!({"name": "deploys", "set": {"label": "D"}}));
    assert_eq!((v["status"].as_str(), v["unbound"].as_bool()), (Some("active"), Some(false)), "{v}");
    // What it fetches: back to proposed, secret unbound [GV53].
    let (s, v) = call("/cli/sources/edit", &agent("p1"), json!({"name": "deploys", "set": {"extract": {}}}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["status"].as_str(), v["unbound"].as_bool()), (Some("proposed"), Some(true)));
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = store::get_by_name(&conn, "deploys").unwrap().unwrap();
    assert!(!row.secret_bound);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM source_audit WHERE action = 'edit_unbound'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn mute_limits_by_caller() {
    let _e = env();
    call("/cli/sources/add", &Who::Owner, fleet("fleet"));
    let m = |who: &Who, secs: u32, reason: Option<&str>| call("/cli/sources/mute", who, json!({"name": "fleet", "forSecs": secs, "reason": reason}));
    assert_eq!(err(&m(&agent("p1"), 1800, None).1), "reason_required");
    assert_eq!(err(&m(&agent("p1"), 3 * 3600, Some("roll")).1), "too_long");
    assert_eq!(err(&m(&Who::Owner, 25 * 3600, None).1), "too_long");
    let (s, v) = m(&Who::Owner, 3600, None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["muted"]["by"], "owner");
    assert_eq!(err(&call("/cli/sources/unmute", &agent("p1"), json!({"name": "fleet"})).1), "not_own");
    let (s, v) = m(&agent("p1"), 1800, Some("daemon roll"));
    assert_eq!((s, v["muted"]["by"].as_str()), (200, Some("agent:p1")));
    let (s, _) = call("/cli/sources/unmute", &agent("p1"), json!({"name": "fleet"}));
    assert_eq!(s, 200);
    let (_, v) = get("/cli/sources/read", &Who::Owner, &[("name", "fleet")]);
    assert!(v["muted"].is_null());
}

#[test]
fn pause_resume_status_changes_history_export() {
    let _e = env();
    call("/cli/sources/add", &Who::Owner, fleet("fleet"));
    assert_eq!(err(&call("/cli/sources/pause", &agent("p1"), json!({"name": "fleet"})).1), "person_only");
    let (_, v) = call("/cli/sources/pause", &Who::Owner, json!({"name": "fleet"}));
    assert_eq!(v["status"], "paused");
    let (_, v) = get("/cli/sources/read", &Who::Owner, &[("name", "fleet")]);
    assert_eq!(v["display"], "paused");
    let (_, v) = call("/cli/sources/resume", &Who::Owner, json!({"name": "fleet"}));
    assert_eq!(v["status"], "active");

    let (_, v) = get("/cli/sources/changes", &Who::Owner, &[("since", "0")]);
    assert_eq!(v["full"], true);
    let rev = v["rev"].as_i64().unwrap();
    let (_, v) = get("/cli/sources/changes", &Who::Owner, &[("since", &rev.to_string())]);
    assert_eq!((v["full"].as_bool(), v["changed"].as_array().unwrap().len()), (Some(false), 0));
    call("/cli/sources/mute", &Who::Owner, json!({"name": "fleet", "forSecs": 60}));
    let (_, v) = get("/cli/sources/changes", &Who::Owner, &[("since", &rev.to_string())]);
    assert_eq!(v["changed"][0]["name"], "fleet");
    call("/cli/sources/remove", &Who::Owner, json!({"name": "fleet"}));
    let (_, v) = get("/cli/sources/changes", &Who::Owner, &[("since", &rev.to_string())]);
    assert_eq!(v["removed"], json!(["fleet"]));

    call("/cli/sources/add", &Who::Owner, json!({"name": "s2", "label": "S2", "everySecs": 60,
        "fetch": {"url": "https://status.example.com/x.json"}, "extract": {"fields": {"open": "/open"}, "trend": ["open"]}, "keep": "states"}));
    let (s, v) = get("/cli/sources/history", &Who::Owner, &[("name", "s2"), ("field", "open")]);
    assert_eq!((s, err(&v)), (409, "no_history"));
    let (_, v) = get("/cli/sources/export", &Who::Owner, &[("name", "s2")]);
    assert_eq!(v["fetch"]["url"], "https://status.example.com/x.json");
    assert_eq!(v["keep"], "states");
    let (s, v) = get("/cli/sources/status", &Who::Owner, &[]);
    assert_eq!(s, 200);
    assert_eq!(v["active"], 1);
    assert!(v["parkedOrigins"].is_array());
}

#[test]
fn per_server_caps() {
    let _e = env();
    let targets: Vec<Value> = (0..50).map(|i| json!({"key": format!("t{i}"), "url": format!("https://t{i}.example.com/")})).collect();
    for n in 0..10 {
        let (s, v) = call("/cli/sources/add", &Who::Owner, json!({"name": format!("f{n}"), "label": "F", "everySecs": 60, "fetch": {"targets": targets}}));
        assert_eq!(s, 200, "{n}: {v}");
    }
    let (s, v) = call("/cli/sources/add", &Who::Owner, fleet("one-more"));
    assert_eq!((s, err(&v)), (409, "too_many_targets"), "500 targets per server");
}

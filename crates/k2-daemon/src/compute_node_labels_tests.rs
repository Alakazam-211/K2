//! This server's node labels over the routes (0.46.18): who reads, who
//! changes, the POST-only table, and `--label WORD` at submit.

use super::*;
use k2_core::connect_users::Role;

fn body(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).expect("json")
}

/// Two workspaces (agents may use compute), three nodes; `w-a` is
/// granted node-a and node-b, `w-b` only node-c.
fn seed() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for (id, h) in [("w-a", "wa"), ("w-b", "wb")] {
        conn.execute(
            "INSERT INTO projects (id, name, path, handle, agents_can_use_compute) VALUES (?1, ?2, ?3, ?2, 1)",
            rusqlite::params![id, h, format!("/tmp/{id}")],
        )
        .expect("project");
    }
    for n in ["node-a", "node-b", "node-c"] {
        store::insert_pending_node(&conn, n, n, &format!("{n:0>64}"), "pem", &Default::default(), "owner", "123456", 1).expect("node");
    }
    for (n, ws) in [("node-a", "w-a"), ("node-b", "w-a"), ("node-c", "w-b")] {
        store::set_grant(&conn, n, ws, &store::GrantLimits::default(), "owner", 2).expect("grant");
    }
}

fn labels_of(v: &serde_json::Value, node: &str) -> Vec<String> {
    let rows = v["nodes"].as_array().unwrap_or_else(|| panic!("no nodes: {v}"));
    let row = rows.iter().find(|r| r["node"] == node).unwrap_or_else(|| panic!("no {node} in {v}"));
    row["labels"].as_array().unwrap_or_else(|| panic!("labels: {row}")).iter().map(|l| l.as_str().expect("str").to_string()).collect()
}

fn node_names(v: &serde_json::Value) -> Vec<String> {
    v["nodes"].as_array().expect("nodes").iter().map(|r| r["node"].as_str().expect("name").to_string()).collect()
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("s-{ws}") }
}

fn manager(ws: &str) -> Manager {
    Manager { project_id: ws.into(), session_id: format!("s-{ws}"), label: format!("agent:{ws}"), handle: ws.into() }
}

#[test]
fn the_owner_adds_and_removes_and_everyone_reads() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    seed();
    let none = HashMap::new();
    let v = handle(SET, &Who::Owner, &body(serde_json::json!({"node": "node-a", "add": ["Release", "for:0.46.18"]})), &none, None).expect("add");
    assert_eq!((v["changed"].as_bool(), v["labels"].clone()), (Some(true), serde_json::json!(["for:0.46.18", "release"])), "{v}");
    let v = handle(SET, &Who::Owner, &body(serde_json::json!({"node": "node-a", "add": ["release"]})), &none, None).expect("again");
    assert_eq!(v["changed"].as_bool(), Some(false));

    // The owner and an Admin see every node.
    let admin = Who::Login { username: "a".into(), role: Role::Admin };
    for who in [Who::Owner, admin.clone()] {
        let v = handle(LIST, &who, b"", &none, None).expect("list");
        assert_eq!(node_names(&v), ["node-a", "node-b", "node-c"], "{who:?}");
        assert_eq!(labels_of(&v, "node-a"), ["for:0.46.18", "release"]);
        assert!(labels_of(&v, "node-b").is_empty());
    }
    // An agent reads its granted nodes' labels, and only those.
    let v = handle(LIST, &agent("w-a"), b"", &none, None).expect("agent reads");
    assert_eq!(node_names(&v), ["node-a", "node-b"]);
    assert_eq!(labels_of(&v, "node-a"), ["for:0.46.18", "release"]);
    let one = HashMap::from([("node".to_string(), "node-a".to_string())]);
    assert_eq!(node_names(&handle(LIST, &agent("w-a"), b"", &one, None).expect("one node")), ["node-a"]);
    let e = handle(LIST, &agent("w-b"), b"", &one, None).expect_err("not its node");
    assert_eq!((e.code, e.status), (codes::NODE_NOT_FOUND, 404));

    // Remove.
    let v = handle(SET, &admin, &body(serde_json::json!({"node": "node-a", "remove": ["release"]})), &none, None).expect("admin removes");
    assert_eq!(v["labels"], serde_json::json!(["for:0.46.18"]));
    // A bad label, an empty ask, an unknown node.
    for (b, code, status) in [
        (serde_json::json!({"node": "node-a", "add": ["os=linux"]}), codes::BAD_REQUEST, 400),
        (serde_json::json!({"node": "node-a"}), codes::BAD_REQUEST, 400),
        (serde_json::json!({"node": "node-a", "add": "release"}), codes::BAD_REQUEST, 400),
        (serde_json::json!({"node": "node-z", "add": ["x"]}), codes::NODE_NOT_FOUND, 404),
    ] {
        let e = handle(SET, &Who::Owner, &body(b.clone()), &none, None).expect_err("refused");
        assert_eq!((e.code, e.status), (code, status), "{b}");
    }
}

/// Changing labels: the owner, an Admin, or an agent under "Allow
/// managing compute nodes" (a manager); a plain agent, a K2 shell tab and
/// a Member login are refused owner_only.
#[test]
fn changing_needs_the_owner_an_admin_or_the_manage_switch() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    seed();
    let none = HashMap::new();
    let ask = body(serde_json::json!({"node": "node-b", "add": ["nightly"]}));
    let member = Who::Login { username: "m".into(), role: Role::Member };
    let shell = Who::Shell { project_id: "w-a".into(), session_id: "s-sh".into() };
    for who in [agent("w-a"), shell, member] {
        let e = handle(SET, &who, &ask, &none, None).expect_err("refused");
        assert_eq!((e.code, e.status), (codes::OWNER_ONLY, 403), "{who:?}");
    }
    let still = handle(LIST, &Who::Owner, b"", &none, None).expect("list");
    assert!(labels_of(&still, "node-b").is_empty(), "nothing written by a refused caller");
    // A manager agent may, on any node.
    let m = manager("w-b");
    let v = handle(SET, &agent("w-b"), &ask, &none, Some(&m)).expect("manager");
    assert_eq!(v["labels"], serde_json::json!(["nightly"]));
    let e = handle(SET, &Who::Invalid, &ask, &none, None).expect_err("no token");
    assert_eq!(e.status, 403);
}

#[test]
fn the_change_route_is_post_only_and_the_read_is_an_agent_verb() {
    for p in ROUTES {
        let r = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no route_policy row"));
        let post_only = POST_ROUTES.contains(p);
        assert!(crate::compute_routes::is_route(p), "{p} on the compute router");
        assert_eq!(crate::compute_routes::is_post_route(p), post_only, "{p}: the dispatcher's 405 guard");
        assert_eq!(r.get.is_none(), post_only, "{p}: a mutating route admits no GET");
    }
    assert!(crate::compute_routes::is_post_route(SET) && !crate::compute_routes::is_post_route(LIST));
    assert!(crate::session_token::is_agent_verb(LIST), "agents read labels");
    assert!(!crate::session_token::is_agent_verb(SET), "changing them is not an agent verb (a manager passes in the handler, as node pause)");
}

#[test]
fn run_labels_are_normalized_and_only_for_auto_placement() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    seed();
    let v = serde_json::json!({"serverLabels": ["Release", "for:0.46.18", "release"]});
    assert_eq!(from_run(&v, true, None).expect("auto"), ["for:0.46.18", "release"]);
    assert!(from_run(&serde_json::json!({}), false, None).expect("none asked").is_empty());
    let e = from_run(&v, false, None).expect_err("a named node");
    assert_eq!((e.code, e.status), (codes::BAD_REQUEST, 400));
    assert!(e.hint.contains("auto-placement"), "{}", e.hint);
    let e = from_run(&serde_json::json!({"serverLabels": ["a b"]}), true, None).expect_err("bad label");
    assert_eq!(e.status, 400);
    // `--cache` pinned to a node without the labels: refused; with: fine.
    let node_b = node_or_404(Some("node-b")).expect("node-b");
    let e = from_run(&v, true, Some(&node_b)).expect_err("cache node lacks them");
    assert!(e.hint.contains("--cache pins this job to node-b"), "{}", e.hint);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        server_labels::change(&conn, "node-b", &["release".to_string(), "for:0.46.18".to_string()], &[], "owner", 3).expect("label");
    }
    assert_eq!(from_run(&v, true, Some(&node_b)).expect("cache node has them"), ["for:0.46.18", "release"]);
}

/// `--for <version>`: checked, a leading v dropped, and kept on the job
/// (`k2ctl.forVersion` in the stored plan; none: no key at all).
#[test]
fn for_version_is_checked_and_kept_on_the_job() {
    let f = |x: serde_json::Value| for_from(&serde_json::json!({ "forVersion": x }));
    assert_eq!(f("0.46.19".into()).expect("ok").as_deref(), Some("0.46.19"));
    assert_eq!(f("v0.47.0-rc1".into()).expect("v dropped").as_deref(), Some("0.47.0-rc1"));
    assert_eq!(for_from(&serde_json::json!({})).expect("none"), None);
    for bad in [serde_json::json!(""), serde_json::json!("next"), serde_json::json!("0.46 19"), serde_json::json!(46), serde_json::json!("1".repeat(FOR_MAX + 1))] {
        let e = f(bad.clone()).expect_err("refused");
        assert_eq!((e.code, e.status), (codes::BAD_REQUEST, 400), "{bad}");
    }
    let ctl = store::JobCtl { for_version: Some("0.46.19".into()), ..Default::default() };
    let v = serde_json::to_value(&ctl).expect("json");
    assert_eq!(v["forVersion"], "0.46.19", "{v}");
    let back: store::JobCtl = serde_json::from_value(v).expect("back");
    assert_eq!(back.for_version.as_deref(), Some("0.46.19"));
    assert!(serde_json::to_value(store::JobCtl::default()).expect("json").get("forVersion").is_none());
}

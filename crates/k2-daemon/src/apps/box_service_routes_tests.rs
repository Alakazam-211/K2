//! `/cli/app/box-service*` (0.46.0): who may propose and revoke, approval
//! through the one held-request flow (the owner in person, a confirm bound
//! to the request), the deny-list, presets read from Sew's config, and the
//! forwards and env a start renders.

use k2_core::deploy::store;
use serde_json::json;

use super::*;
use crate::apps::approvals;
use crate::apps::egress_routes::test_seams as vault_seams;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                rusqlite::params![ws, format!("/tmp/k2-boxsvc-test/{ws}")],
            )
            .unwrap();
        }
        for (app, ws, iso) in [("hearth", &ws_a, "container"), ("other", &ws_b, "container"), ("plain", &ws_a, "process")] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}-{tag}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
    }
    super::super::alerts::set_sink(Some(Box::new(|_ws: &str, _text: &str| {})));
    test_seams::set_sew_toml(Some(None));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn call(path: &str, who: &Who, body: J) -> Result<J, (u16, &'static str, String)> {
    let r = if approvals::ROUTES.contains(&path) {
        approvals::handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    } else {
        handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    };
    r.map_err(|r| (r.status, r.code, r.hint))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> Result<J, (u16, &'static str, String)> {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    handle(path, who, b"", &p).map_err(|r| (r.status, r.code, r.hint))
}

fn approve(id: &str) -> J {
    let tok = vault_seams::vault_handle().confirm.mint(approvals::CONFIRM_ACTION, Some(id), "owner", "local-auth");
    call("/cli/app/approve", &Who::Owner, json!({"request": id, "confirmToken": tok})).unwrap()
}

#[test]
fn an_agent_asks_the_owner_approves_in_person_and_revoke_is_immediate() {
    let f = fx("flow");
    let a = agent(&f.ws_a);
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "sew-api", "reason": "Hearth talks to Sew"})).unwrap();
    assert_eq!(r["status"], "held");
    assert_eq!(r["kind"], "box_service");
    let id = r["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(r["services"], json!([{"name": "sew-api", "hostPort": 8470, "appPort": 8470, "proto": "tcp", "preset": "sew-api"}]));
    assert!(r["hint"].as_str().unwrap().contains(&format!("k2 app approve {id}")));
    // Held: forwarded at the next start, door shut, env set.
    assert!(allowed_ports("hearth").is_empty());
    let (fw, skipped) = forwards_for("hearth", 41_000);
    assert_eq!(fw, vec![BoxForward { app_port: 8470, host_port: 8470 }]);
    assert!(skipped.is_empty());
    assert_eq!(env_for("hearth", false).get("K2_BOX_SEW_API").map(String::as_str), Some("127.0.0.1:8470"));
    let shown = get("/cli/app/box-service", &a, &[("app", "hearth")]).unwrap();
    assert_eq!(shown["wall"], "container");
    assert_eq!(shown["services"][0]["status"], "pending");
    assert_eq!(shown["services"][0]["door"], "shut (waiting on the owner)");
    assert_eq!(shown["pending"][0]["id"], id.as_str());
    assert_eq!(shown["presets"][1]["hostPort"], 7880);

    // Another workspace's agent doesn't see the App; an agent never approves.
    assert_eq!(get("/cli/app/box-service", &agent(&f.ws_b), &[("app", "hearth")]).unwrap_err().0, 404);
    let tok = vault_seams::vault_handle().confirm.mint(approvals::CONFIRM_ACTION, Some(&id), "owner", "local-auth");
    let e = call("/cli/app/approve", &a, json!({"request": id, "confirmToken": tok})).unwrap_err();
    assert_eq!(e.1, "owner_only");
    let e = call("/cli/app/approve", &Who::Owner, json!({"request": id})).unwrap_err();
    assert_eq!(e.1, "vault_confirm_required", "the owner token alone is not an approval");

    // The owner, confirmed.
    let r = approve(&id);
    assert_eq!(r["request"]["status"], "approved", "{r}");
    assert_eq!(r["request"]["services"][0]["name"], "sew-api");
    assert_eq!(r["result"]["note"], "approved; it opens when the App starts", "{r}");
    assert_eq!(allowed_ports("hearth"), vec![8470]);
    assert_eq!(get("/cli/app/box-service", &a, &[("app", "hearth")]).unwrap()["services"][0]["door"], "open");
    // Asking again: already allowed (exit 0 in the CLI).
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "sew-api"})).unwrap();
    assert_eq!(r["status"], "allowed");

    // A second one after the App started: approving says a restart opens
    // its forward.
    record_live("hearth", &json!({"boxForwards": [{"appPort": 8470, "hostPort": 8470}]}));
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "sew-signal"})).unwrap();
    let id2 = r["request"]["id"].as_str().unwrap().to_string();
    let r = approve(&id2);
    assert_eq!(r["result"]["restartNeeded"], true, "{r}");
    assert!(r["result"]["note"].as_str().unwrap().contains("k2 app restart hearth"), "{r}");
    let shown = get("/cli/app/box-service", &a, &[("app", "hearth")]).unwrap();
    let sig = shown["services"].as_array().unwrap().iter().find(|s| s["name"] == "sew-signal").unwrap().clone();
    assert_eq!(sig["forwarded"], false);
    forget_live("hearth");

    // Revoke: at once, by the requester's side too (narrowing never waits).
    let r = call("/cli/app/box-service/revoke", &a, json!({"app": "hearth", "name": "sew-api"})).unwrap();
    assert_eq!(r["revoked"], true);
    assert_eq!(allowed_ports("hearth"), vec![7880]);
    let r = call("/cli/app/box-service/revoke", &a, json!({"app": "hearth", "name": "sew-api"})).unwrap();
    assert_eq!(r["revoked"], false);
    // Audited.
    let h = k2_core::db::shared();
    let conn = h.lock();
    let events: Vec<String> = conn
        .prepare("SELECT event FROM deploy_audit WHERE subject_id = 'hearth' ORDER BY seq")
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap();
    for e in ["box_service_requested", "widening_approved", "box_service_revoked"] {
        assert!(events.iter().any(|x| x == e), "{e} missing: {events:?}");
    }
}

#[test]
fn the_deny_list_and_the_daemons_own_ports_are_refused() {
    let f = fx("deny");
    let a = agent(&f.ws_a);
    for (port, why) in [(22, "ssh"), (5432, "Postgres"), (5433, "Postgres"), (5449, "Postgres"), (38_601, "gateway"), (38_950, "gateway"), (2019, "Caddy"), (38_472, "box router"), (80, "system")] {
        let e = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "x", "hostPort": port})).unwrap_err();
        assert_eq!((e.0, e.1), (403, codes::BOX_SERVICE_REFUSED), "{port}");
        assert!(e.2.contains(why), "{port}: {}", e.2);
    }
    // The daemon's own main port (read from ~/.k2/daemon.port).
    let home = k2_core::paths::k2_home();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("daemon.port"), "41999").unwrap();
    let e = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "x", "hostPort": 41999})).unwrap_err();
    assert!(e.2.contains("this K2 daemon's own port"), "{}", e.2);
    // In-wall ports: never 8730, never K2's range.
    for ap in [8730, 38_700, 80] {
        let e = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "x", "hostPort": 9000, "appPort": ap})).unwrap_err();
        assert_eq!(e.1, codes::BOX_SERVICE_INVALID, "{ap}");
    }
    // Names and unknown names without a port.
    assert_eq!(call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "Bad_Name", "hostPort": 9000})).unwrap_err().1, codes::BOX_SERVICE_INVALID);
    let e = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "redis"})).unwrap_err();
    assert!(e.2.contains("sew-api and sew-signal"), "{}", e.2);
    assert_eq!(call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "x", "hostPort": 9000, "proto": "udp"})).unwrap_err().1, codes::BOX_SERVICE_INVALID);
    // Nothing was filed.
    assert!(get("/cli/app/box-service", &a, &[("app", "hearth")]).unwrap()["services"].as_array().unwrap().is_empty());
    // A doctored row (say, an old build's) is never forwarded or admitted.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO app_box_services (app_id, name, host_port, app_port, proto, source, status, added_by, added_at) \
             VALUES ('hearth', 'ssh', 41999, 2222, 'tcp', 'request', 'allowed', 'x', 1)",
            [],
        )
        .unwrap();
    }
    assert!(allowed_ports("hearth").is_empty());
    let (fw, skipped) = forwards_for("hearth", 40_000);
    assert!(fw.is_empty());
    assert_eq!(skipped, vec!["ssh".to_string()]);
}

#[test]
fn presets_follow_sews_config_and_remaps_stay_inside_the_wall() {
    let f = fx("preset");
    let a = agent(&f.ws_a);
    test_seams::set_sew_toml(Some(Some("[server]\nlisten = \"127.0.0.1:8471\"\n[voice.ops]\nsignal_listen = \"127.0.0.1:7881\"\n".into())));
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "sew-api"})).unwrap();
    assert_eq!(r["services"][0]["hostPort"], 8471);
    assert_eq!(r["services"][0]["appPort"], 8471);
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "sew-signal", "appPort": 7880})).unwrap();
    assert_eq!(r["services"].as_array().unwrap().len(), 2, "joins the open request: {r}");
    let (fw, _) = forwards_for("hearth", 41_000);
    assert!(fw.contains(&BoxForward { app_port: 7880, host_port: 7881 }), "{fw:?}");
    assert_eq!(env_for("hearth", false)["K2_BOX_SEW_SIGNAL"], "127.0.0.1:7880");
    assert_eq!(env_for("hearth", true)["K2_BOX_SEW_SIGNAL"], "127.0.0.1:7881", "the process exception dials the host port");
    // Sew bound off loopback: refused.
    test_seams::set_sew_toml(Some(Some("[server]\nlisten = \"0.0.0.0:8470\"\n".into())));
    let e = call("/cli/app/box-service/request", &agent(&f.ws_b), json!({"app": "other", "name": "sew-api"})).unwrap_err();
    assert_eq!(e.1, codes::BOX_SERVICE_REFUSED);
    // A forward never sits on the instance's own port.
    let (fw, skipped) = forwards_for("hearth", 8471);
    assert!(!fw.iter().any(|f| f.app_port == 8471));
    assert_eq!(skipped, vec!["sew-api".to_string()]);
    // The unwalled preview engine: nothing enforced, said so.
    let shown = get("/cli/app/box-service", &agent(&f.ws_a), &[("app", "plain")]).unwrap();
    assert_eq!(shown["wall"], "unwalled");
    test_seams::set_sew_toml(None);
}

#[test]
fn denying_releases_the_held_rows() {
    let f = fx("denyreq");
    let a = agent(&f.ws_a);
    let r = call("/cli/app/box-service/request", &a, json!({"app": "hearth", "name": "x", "hostPort": 9000})).unwrap();
    let id = r["request"]["id"].as_str().unwrap().to_string();
    let r = call("/cli/app/deny", &Who::Owner, json!({"request": id, "reason": "no"})).unwrap();
    assert_eq!(r["status"], "denied");
    assert!(get("/cli/app/box-service", &a, &[("app", "hearth")]).unwrap()["services"].as_array().unwrap().is_empty());
    assert!(forwards_for("hearth", 41_000).0.is_empty());
}

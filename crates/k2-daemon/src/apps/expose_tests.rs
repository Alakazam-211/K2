//! `k2 app expose` (App-governed Spaces S7): a public URL on or off for an
//! installed App with no re-create. The control-plane write is the seam
//! (`expose::test_seam`); every check before it is the real one.

use serde_json::json;

use super::custom_domains::test_util::{build_switch, runtime_app};
use super::expose::{self, test_seam};
use super::routes::Who;
use k2_core::deploy::codes;
use k2_core::published_services as ps;

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: "s1".into() }
}

fn call(who: &Who, body: serde_json::Value) -> super::routes::R<serde_json::Value> {
    expose::route(who, body.to_string().as_bytes())
}

/// A local-only runtime App (installed without `--tunnel`); its service id.
fn local_app(ws: &str, app: &str) -> String {
    runtime_app(ws, app);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::deploy::store::get_app(&conn, app).unwrap().expect("App row");
    ps::set_expose(&conn, &row.service_id, ps::EXPOSE_LOCAL).unwrap();
    row.service_id
}

fn expose_of(service_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ps::get_by_id(&conn, service_id).unwrap().expect("service row").expose
}

/// The owner turns the URL on: the same App and service id, its users and
/// grants untouched, `expose = tunnel`, one hostname write, a sweep asked
/// for (Sew's origin follows), and the answer says to restart for
/// `K2_APP_ORIGIN`. A second call changes nothing.
#[test]
fn the_owner_exposes_an_installed_app_without_recreating_it() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let svc = local_app("ws-xp1", "xpjobs1");
    let dana = k2_core::skin::add_principal_for_app("xpdana", None, &svc, None, "test").expect("an App user");
    let grants_before = k2_core::skin::list_grants().unwrap().into_iter().filter(|g| g.target_id == svc).count();
    assert_eq!(grants_before, 1, "the App user's login");
    test_seam::set(Some(41_101));
    let _ = k2_core::tool_hook::take_reconcile_request();

    let v = call(&Who::Owner, json!({ "app": "xpjobs1", "tunnel": true })).expect("exposed");
    assert_eq!((v["expose"].as_str(), v["changed"].as_bool(), v["restartNeeded"].as_bool()), (Some("tunnel"), Some(true), Some(true)), "{v}");
    assert!(v["next"].as_str().unwrap().contains("k2 app restart xpjobs1"), "{v}");
    assert_eq!(test_seam::log(), vec!["point xpjobs1 ws-xp1 41101".to_string()]);
    assert_eq!(expose_of(&svc), ps::EXPOSE_TUNNEL);
    assert!(k2_core::tool_hook::take_reconcile_request(), "a sweep now, so Sew's origin follows");
    // No re-create: same App, same service id, same login.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::deploy::store::get_app(&conn, "xpjobs1").unwrap().unwrap().service_id, svc);
    }
    let after: Vec<_> = k2_core::skin::list_grants().unwrap().into_iter().filter(|g| g.target_id == svc).collect();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].subject_id, dana.id);
    // The App's origins name the label from its next start.
    assert_eq!(
        super::origins::nested_origin("xpjobs1", &expose_of(&svc), "box").as_deref(),
        Some(format!("https://xpjobs1.box.{}", k2_core::tunnel::config::SUBDOMAIN_HOST).as_str())
    );

    // Again: nothing to do.
    test_seam::set(Some(41_101));
    let v = call(&Who::Owner, json!({ "app": "xpjobs1", "tunnel": true })).unwrap();
    assert_eq!(v["changed"], json!(false), "{v}");
    assert!(test_seam::log().is_empty(), "no second hostname write");
    test_seam::set(None);
}

/// `--local` removes the hostname only while it is still this App's (its
/// workspace holds it, it points at the App's front port), then sets
/// `expose = local`; a hostname that moved on is left as it is.
#[test]
fn local_drops_the_hostname_only_while_it_is_the_apps() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let svc = local_app("ws-xp2", "xpjobs2");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &svc, ps::EXPOSE_TUNNEL).unwrap();
        conn.execute("UPDATE apps SET front_port = 41102 WHERE app_id = 'xpjobs2'", []).unwrap();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, "xpjobs2", "ws-xp2", Some("127.0.0.1:41102")).unwrap();
    }
    test_seam::set(Some(41_102));
    let _ = k2_core::tool_hook::take_reconcile_request();
    let v = call(&Who::Owner, json!({ "app": "xpjobs2", "tunnel": false })).expect("local");
    assert_eq!((v["expose"].as_str(), v["changed"].as_bool()), (Some("local"), Some(true)), "{v}");
    assert!(v.get("hostnameLeft").is_none(), "{v}");
    assert_eq!(test_seam::log(), vec!["drop xpjobs2".to_string()]);
    assert_eq!(expose_of(&svc), ps::EXPOSE_LOCAL);
    assert!(k2_core::tool_hook::take_reconcile_request(), "a sweep now: Sew's origin is cleared");

    // A hostname another workspace holds now is never dropped.
    let svc3 = local_app("ws-xp3", "xpjobs3");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &svc3, ps::EXPOSE_TUNNEL).unwrap();
        conn.execute("UPDATE apps SET front_port = 41103 WHERE app_id = 'xpjobs3'", []).unwrap();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, "xpjobs3", "ws-xp2", Some("127.0.0.1:41103")).unwrap();
    }
    test_seam::set(Some(41_103));
    let v = call(&Who::Owner, json!({ "app": "xpjobs3", "tunnel": false })).unwrap();
    assert!(v["hostnameLeft"].as_str().unwrap().contains("left as it is"), "{v}");
    assert!(test_seam::log().is_empty(), "nothing dropped: {:?}", test_seam::log());
    assert_eq!(expose_of(&svc3), ps::EXPOSE_LOCAL);
    test_seam::set(None);
}

/// Who: the App's own agent with "Build and run Apps" (inside its default
/// limit of one public hostname; past it, held for the owner); another
/// workspace's agent never sees the App; `tunnel` is required.
#[test]
fn the_apps_agent_exposes_within_its_limits() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let svc_a = local_app("ws-xp4", "xpjobs4");
    let svc_b = local_app("ws-xp4", "xpjobs5");
    local_app("ws-xp5", "xpother");
    build_switch("ws-xp4", true);
    build_switch("ws-xp5", true);
    test_seam::set(Some(41_104));

    let e = call(&agent("ws-xp5"), json!({ "app": "xpjobs4", "tunnel": true })).unwrap_err();
    assert_eq!((e.status, e.code), (404, codes::APP_NOT_FOUND), "{e:?}");
    let e = call(&agent("ws-xp4"), json!({ "app": "xpjobs4" })).unwrap_err();
    assert_eq!(e.status, 400, "{e:?}");
    assert!(e.hint.contains("tunnel is required"), "{e:?}");

    let v = call(&agent("ws-xp4"), json!({ "app": "xpjobs4", "tunnel": true })).expect("its own App, inside the limit");
    assert_eq!(v["changed"], json!(true), "{v}");
    assert_eq!(expose_of(&svc_a), ps::EXPOSE_TUNNEL);
    // A second public hostname is past the agent's default limit: held.
    assert!(super::limits::public_hostname_overage("ws-xp4").unwrap().is_some());
    let v = call(&agent("ws-xp4"), json!({ "app": "xpjobs5", "tunnel": true })).expect("held, not refused");
    assert_eq!(v["status"], json!("held"), "{v}");
    assert_eq!(expose_of(&svc_b), ps::EXPOSE_LOCAL, "nothing changed while held");
    assert_eq!(test_seam::log(), vec!["point xpjobs4 ws-xp4 41104".to_string()], "one hostname write only");
    // Turning it off narrows: always the agent's.
    let v = call(&agent("ws-xp4"), json!({ "app": "xpjobs4", "tunnel": false })).unwrap();
    assert_eq!(v["expose"], json!("local"), "{v}");
    test_seam::set(None);

    assert!(super::routes::is_post_route("/cli/app/expose"), "POST only: 405 on GET");
    let row = crate::routes::route_policy::lookup("/cli/app/expose").expect("classified");
    assert!(row.get.is_none());
    assert_eq!(row.post.expect("POST").as_wire(), "admin");
    assert!(super::approvals::MANAGE_HELD.contains(&"/cli/app/expose"), "held without switch 1");
}

fn installed(app: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::get_app(&conn, app).unwrap().is_some()
}

/// PRD §9.2 (Q4: refuse and list): a plain uninstall of an App that still
/// has people's logins is refused with the counts, the `k2 app expose`
/// pointer and the way through (`dropAccess`); nothing is removed. With
/// `dropAccess` it goes ahead. An App with nothing to lose uninstalls as
/// before.
#[test]
fn uninstall_refuses_to_drop_access_silently() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let svc = local_app("ws-xp6", "xpjobs6");
    k2_core::skin::add_principal_for_app("xpera", None, &svc, None, "test").unwrap();
    let uninstall = |body: serde_json::Value| super::routes::uninstall(&Who::Owner, body.to_string().as_bytes());

    let e = uninstall(json!({ "app": "xpjobs6" })).unwrap_err();
    assert_eq!((e.status, e.code), (409, super::routes::UNINSTALL_DROPS_ACCESS), "{e:?}");
    assert!(e.hint.contains("would drop 1 App login"), "{}", e.hint);
    assert!(e.hint.contains("k2 app expose xpjobs6 --tunnel"), "{}", e.hint);
    assert!(e.hint.contains("k2 app uninstall xpjobs6 --drop-access"), "{}", e.hint);
    let would = e.extra.as_ref().expect("counts")["wouldDrop"].clone();
    assert_eq!((would["logins"].as_u64(), would["total"].as_u64()), (Some(1), Some(1)), "{would}");
    assert!(installed("xpjobs6"), "nothing removed");
    assert!(k2_core::skin::list_grants().unwrap().iter().any(|g| g.target_id == svc), "the login stays");

    let v = uninstall(json!({ "app": "xpjobs6", "dropAccess": true })).expect("dropAccess goes ahead");
    assert_eq!(v["ok"], json!(true), "{v}");
    assert!(!installed("xpjobs6"));

    // Nothing to lose: a plain uninstall, as before.
    local_app("ws-xp6", "xpjobs7");
    uninstall(json!({ "app": "xpjobs7" })).expect("nothing to drop");
    assert!(!installed("xpjobs7"));
}

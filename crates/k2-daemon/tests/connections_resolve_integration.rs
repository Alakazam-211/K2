//! `GET /cli/connections?action=resolve&target=` through the real
//! dispatcher (0.46.4; `k2 agent hire --dry-run` resolves --connect with
//! it). Security review: another server's roster is for the owner, an
//! admin or an agent; a Member gets 403 for a remote target and, for a
//! local one, found / not found without the workspace's path or id. The
//! privilege stamp is the dispatcher's: a Member can't send its own.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;

fn session(user: &str, role: k2_core::connect_users::Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-resolve-int-7").expect("add_user");
    if role != k2_core::connect_users::Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_reads_another_server_only_for_the_owner_an_admin_or_an_agent() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "resolvews");
    let path = ws.path.to_string_lossy().to_string();
    let remote = format!("action=resolve&target={}", enc("k2::nowhere.k2.dev"));
    let local = format!("action=resolve&target={}", enc(&path));

    let member = session("resmember", k2_core::connect_users::Role::Member);
    // Neither a privilege stamp nor an agent stamp in the request counts:
    // `principal_bound=1&project_id=<ws>` used to read as that workspace's
    // agent (verification 0.46.4).
    let spoof = format!("principal_bound=1&project_id={}", enc(&ws.id));
    for q in [remote.clone(), format!("{remote}&actor_privileged=1"), format!("{remote}&{spoof}")] {
        let r = get(d.port, "/cli/connections", &member, &q).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{q}: {}", r.body);
    }
    for q in [local.clone(), format!("{local}&{spoof}")] {
        let r = get(d.port, "/cli/connections", &member, &q).await;
        assert_eq!(r.status, 200, "{q}: {}", r.body);
        let v = r.json();
        assert_eq!(v["kind"], "local", "{v}");
        assert!(v.get("path").is_none() && v.get("projectId").is_none(), "a Member gets no path or id: {v}");
    }
    // A miss is a bare not_found for a Member: no did-you-mean handles.
    let r = get(d.port, "/cli/connections", &member, &format!("action=resolve&target={}", enc("resolvewz"))).await;
    assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "{}", r.body);
    assert!(!r.body.contains("resolvews"), "no handles for a Member: {}", r.body);

    // The owner (and an admin) get the full answer; an agent's passport too.
    let admin = session("resadmin", k2_core::connect_users::Role::Admin);
    for who in [OWNER.to_string(), admin, passport(&ws)] {
        let r = get(d.port, "/cli/connections", &who, &local).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["path"], path.as_str(), "{}", r.body);
        let r = get(d.port, "/cli/connections", &who, &remote).await;
        assert_eq!((r.status, r.code()), (404, "not_found".to_string()), "federation off: {}", r.body);
    }
}

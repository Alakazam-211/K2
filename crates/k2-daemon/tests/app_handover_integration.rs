//! App handover, headless (prd-app-handoff-v1 rev 2; `k2 app handover`):
//! the real dispatcher in-process on a temp `$HOME`.
//!
//! - T1.2: the running App's gateway link re-binds to B: the App gets a
//!   new pass whose room is B, every pass from before is revoked, and
//!   `/gw/me` names B.
//! - T1.3: a guest's live session resolves at every instant across the
//!   approval (a loop of resolves while the card applies sees no gap), and
//!   its rooms are B's after.
//! - T1.8: every mutating handover route is POST-only (405 on GET).

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

fn confirm(action: &str, bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint(action, Some(bound), "owner", "local-auth")
}

/// An App row of `ws` with its service row (as `install` makes them).
fn add_app(app: &str, ws: &Ws) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let svc = k2_core::published_services::insert(
        &conn,
        &ws.id,
        app,
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-hov-int/apps",
        0,
        k2_core::published_services::EXPOSE_TUNNEL,
        k2_core::published_services::DESIRED_STOPPED,
        k2_core::published_services::KIND_CMD,
        "",
    )
    .unwrap();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: app,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "process",
            memory_limit_bytes: Some(256 << 20),
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE apps SET approved_caps_json = ?2 WHERE app_id = ?1",
        rusqlite::params![app, serde_json::to_string(&["thread:read"]).unwrap()],
    )
    .unwrap();
    k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, app, &ws.id, None).unwrap();
    svc.id
}

async fn gw_me(port: u16, pass: &str) -> (u16, Value) {
    let pq = format!("/gw/me?token={}", enc(pass));
    let r = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.write_all(format!("GET {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).to_string()
    })
    .await
    .unwrap();
    let status: u16 = r.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap();
    let body = r.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_link_and_live_sessions_move_to_b_with_no_gap() {
    let env = setup(&[]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    // A vault on a file key under the temp home (secrets and the confirms).
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let a = seed_ws(&env, "hova");
    let b = seed_ws(&env, "hovb");
    let svc = add_app("hovapp", &a);

    // The App runs walled: its link in A's room, a pass delivered.
    let delivered: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = delivered.clone();
    let deliver: gateway::Deliver = Arc::new(move |p: &str| {
        sink.lock().unwrap().push(p.to_string());
        Ok(())
    });
    let old_pass = gateway::open(
        LinkSpec {
            app_id: "hovapp".into(),
            service_id: svc.clone(),
            workspace_id: a.id.clone(),
            port: 0,
            caps: vec!["thread:read".into()],
            rooms: vec![a.id.clone()],
        },
        Some(deliver),
    )
    .expect("open link");
    let port = gateway::port_of("hovapp").expect("link port");
    let (st, me) = gw_me(port, &old_pass).await;
    assert_eq!(st, 200, "{me}");
    assert_eq!((me["rooms"].clone(), me["workspace"].clone()), (json!([a.id]), json!(a.id)), "{me}");

    // A guest signed in before the handover.
    let p = k2_core::skin::add_principal("ann").unwrap();
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id,
        kind: "app".into(),
        target_id: svc.clone(),
        role: None,
        scope: Some(json!([a.id]).to_string()),
        created_by: Some("owner-token".into()),
    })
    .unwrap();
    let session = k2_core::skin::create_app_session_token("ann", &svc).unwrap();

    // The owner asks; nothing moves yet.
    let r = post(d.port, "/cli/app/handover", OWNER, json!({ "app": "hovapp", "to": b.id })).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let req = r.json()["request"]["id"].as_str().unwrap().to_string();

    // T1.3: resolve the guest's session in a loop across the approval.
    let stop = Arc::new(AtomicBool::new(false));
    let misses = Arc::new(AtomicUsize::new(0));
    let rounds = Arc::new(AtomicUsize::new(0));
    let looper = {
        let (stop, misses, rounds, raw) = (stop.clone(), misses.clone(), rounds.clone(), session.raw.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if k2_core::skin::resolve_skin_token(&raw).is_none() {
                    misses.fetch_add(1, Ordering::SeqCst);
                }
                rounds.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let r = post(d.port, "/cli/app/approve", OWNER, json!({ "request": req, "confirmToken": confirm("app-approve", &req) })).await;
    stop.store(true, Ordering::SeqCst);
    looper.join().unwrap();
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    assert!(rounds.load(Ordering::SeqCst) > 0, "the loop ran");
    assert_eq!(misses.load(Ordering::SeqCst), 0, "the guest's session resolved at every instant");
    let pass = k2_core::skin::resolve_skin_token(&session.raw).expect("live after");
    assert_eq!(pass.room_policy.keys().cloned().collect::<Vec<_>>(), vec![b.id.clone()]);

    // T1.2: a new pass reached the App, in B's room; the old one is dead.
    let new_pass = delivered.lock().unwrap().last().cloned().expect("a new pass was delivered");
    assert_ne!(new_pass, old_pass);
    let (st, me) = gw_me(port, &new_pass).await;
    assert_eq!(st, 200, "{me}");
    assert_eq!((me["rooms"].clone(), me["workspace"].clone()), (json!([b.id]), json!(b.id)), "{me}");
    let (st, me) = gw_me(port, &old_pass).await;
    assert!(st == 401 || st == 403, "the pass from before is revoked: {st} {me}");
    assert_eq!(k2_core::skin::link::live_link_passes(&svc).unwrap(), 1, "only the new pass lives");
    let link = get(d.port, "/cli/app/link", OWNER, "app=hovapp").await;
    assert_eq!(link.json()["link"]["rooms"], json!([b.id]), "{}", link.body);

    // T1.8: the mutating handover routes refuse GET.
    for path in ["/cli/app/handover", "/cli/app/unpark", "/cli/app/release"] {
        let r = get(d.port, path, OWNER, "app=hovapp&to=x").await;
        assert_eq!(r.status, 405, "{path}: {}", r.body);
    }
    gateway::close("hovapp", Some(&svc), true);
}

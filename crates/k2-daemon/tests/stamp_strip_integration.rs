//! 0.46.4 stamp-strip: the server-set identity keys (`principal_bound`
//! with `project_id`, and `cell_session_id`) come only from a validated
//! token, never from a request's query or body. A Member that sends them is
//! still a Member: not the stamped workspace's agent, not the owner, and not
//! the session it names. One check per arm that used to take them:
//! `/cli/thread/*` POST (post, ask, secret), `/cli/workspace/msg` POST and
//! `/cli/folder-trust/*`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

fn member_session(user: &str) -> String {
    k2_core::connect_users::add_user(user, "password-for-stamp-strip-3").expect("add_user");
    k2_core::connect_users::create_session(user)
}

fn pin_chat(ws: &Ws) {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
}

async fn post_q(port: u16, pq: String, body: Value) -> Resp {
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get_q(port: u16, pq: String) -> Resp {
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_sending_the_server_stamps_is_still_a_member() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let a = seed_ws(&env, "stampa");
    let b = seed_ws(&env, "stampb");
    pin_chat(&a);
    let member = member_session("stampmember");
    let tok = enc(&member);
    let stamp = format!("principal_bound=1&project_id={}", enc(&a.id));

    // 1. /cli/thread/post and /cli/thread/ask: `from=owner` beside a forged
    //    stamp used to post as "owner" (a bound caller's `from` is kept).
    let r = post_q(d.port, format!("/cli/thread/post?token={tok}&{stamp}&from=owner"), json!({ "addr": a.handle, "text": "hello" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["from"], "owner", "a Member's post is not the owner's: {}", r.body);
    let r = post_q(
        d.port,
        format!("/cli/thread/ask?token={tok}&{stamp}&from=owner"),
        json!({ "addr": a.handle, "prompt": "pick one", "options": "yes,no" }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["from"], "owner", "a Member's card is not the owner's: {}", r.body);
    // The stamp in the body is ignored the same way.
    let r = post_q(
        d.port,
        format!("/cli/thread/post?token={tok}"),
        json!({ "addr": a.handle, "text": "hi", "from": "owner", "principal_bound": "1", "project_id": a.id }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["from"], "owner", "{}", r.body);

    // 1b. /cli/thread/secret: a `cell_session_id` in the query or body used
    //     to name the session the card's answer is for.
    for (pq, body) in [
        (format!("/cli/thread/secret?token={tok}&cell_session_id=forged-q"), json!({ "addr": a.handle, "name": "API_KEY_Q" })),
        (format!("/cli/thread/secret?token={tok}"), json!({ "addr": a.handle, "name": "API_KEY_B", "cell_session_id": "forged-b" })),
    ] {
        let r = post_q(d.port, pq.clone(), body).await;
        assert_eq!(r.status, 200, "{pq}: {}", r.body);
        let card = r.json()["id"].as_str().expect("card id").to_string();
        let req = k2_daemon::vault::get().expect("vault").request_get(&card).expect("the card's request");
        assert!(
            !req.session_id.as_deref().unwrap_or("").starts_with("forged"),
            "{pq}: the request names no session the caller made up: {:?}",
            req.session_id
        );
    }

    // 2. /cli/workspace/msg: a forged stamp used to record the delivery as
    //    sent by workspace A's passport (`k2 msg who`), and to put the
    //    Member under A's peer gate. A Member is recorded as a person.
    let pq = format!("/cli/workspace/msg?token={tok}&{stamp}&workspace={}&text=hello&wake=false", enc(&b.handle));
    let r = post_q(d.port, pq, json!({})).await;
    assert_ne!(r.status, 403, "not A's peer gate: {}", r.body);
    let id = r.json()["deliveryId"].as_str().unwrap_or_else(|| panic!("a delivery is recorded: {}", r.body)).to_string();
    let r = get(d.port, "/cli/msg/who", OWNER, &format!("id={}", enc(&id))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let w = r.json();
    assert_ne!(w["via"], "passport", "not A's passport: {w}");
    assert!(w["sender"]["workspaceId"].is_null(), "no sender workspace for a Member: {w}");

    // 3. /cli/folder-trust/*: a forged stamp (or cell_session_id) used to
    //    read as an agent of A (the card's "asked by", and "only your own
    //    workspace's folder"). A Member reads B's folder like any person.
    let path = enc(&b.path.to_string_lossy());
    for extra in [stamp.clone(), "cell_session_id=forged".to_string()] {
        let r = get_q(d.port, format!("/cli/folder-trust/status?token={tok}&path={path}&{extra}")).await;
        assert_ne!(r.status, 403, "{extra}: not read as A's agent: {}", r.body);
        assert!(!r.body.contains("not_your_workspace"), "{extra}: {}", r.body);
    }
}

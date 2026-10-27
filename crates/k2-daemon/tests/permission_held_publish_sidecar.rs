//! NSI papercut 12 (0.46.4 field report, fixed for 0.46.6), headless: the
//! real dispatcher in process on a temp `$HOME`, permission cards on.
//!
//! An agent working in a **sidecar** (`<ws>/<name>`, a second harness tab
//! of the workspace) runs `k2 publish run` and is held for the owner (a
//! `publish_run` App request):
//! - the hold raises the `apps.approve` card at once (it used to raise
//!   none: `/cli/publish/run` never ran `held_note`), on the **sidecar's**
//!   Thread (it used to go on the main one), and says where;
//! - the agent's own `k2 permission request apps.approve request=w_…`
//!   finds that same card (`already_pending`), on the sidecar's Thread;
//! - the owner answers Allow on the sidecar's Thread with the app-approve
//!   confirm: the permission request and the App request are approved by
//!   the owner, the service runs, and the `[permission:]` turn goes to the
//!   sidecar;
//! - a hold whose card can't go up (no Thread) carries the Ask-with
//!   `permission` object (`/cli/publish/run` had no wall row).
//!
//! NSI papercut 13: one owner approval covers identical re-runs of the
//! same App (command, folder, port, tunnel); any change asks again.
//!
//! No real agent CLI runs (a `claude` that is `cat`), no real `~/.k2`, no
//! keychain.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::Arc;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession, LabelSource};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use serde_json::json;

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

/// The sidecar Thread `<ws>/<slug>`: provider conversation `conv` in a
/// `tab-pane-<conv>` harness tab.
fn sidecar(ws: &Ws, conv: &str, slug: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::allocate_ordinal(&conn, &ws.id, conv).expect("ordinal");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) VALUES ('claude', ?1, ?2, 0, unixepoch())",
        rusqlite::params![conv, slug],
    )
    .expect("name");
    conn.execute(
        "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
        rusqlite::params![ws.id, format!("pane-{conv}"), format!("tab-pane-{conv}"), conv],
    )
    .expect("tab");
}

/// The sidecar's live PTY (`claude`, resolved to the `exec cat` test shim
/// the caller installed) under its tab's agent name, and a passport bound
/// to that PTY's daemon session id (what a real sidecar's `k2` carries).
fn live_sidecar(ws: &Ws, conv: &str) -> (String, Arc<DaemonPtySession>, String) {
    let sid = SessionId::new();
    let session = DaemonPtySession::spawn(DaemonPtyConfig {
        session_id: sid,
        cols: 80,
        rows: 24,
        cwd: Some(ws.path.clone()),
        program: Some("claude".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: LabelSource::Pty,
        sandbox: k2_core::terminal::SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        survive: false,
        ptyd_meta: None,
    })
    .expect("spawn the fake claude");
    let agent_name = format!("tab-pane-{conv}");
    k2_daemon::v2_session_map::register(agent_name.clone(), session.clone());
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    (agent_name, session, passport)
}

fn cards_on(ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn.execute("UPDATE projects SET agents_can_request_permissions = 1 WHERE id = ?1", [&ws.id]).expect("agent switch");
    assert_eq!(n, 1);
}

fn permission_row(held: &str) -> (String, Option<String>, Option<String>, String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT state, conversation_id, addr, card_id, answered_by FROM permission_requests \
         WHERE held_ref = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
        [held],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<String>>(3)?.unwrap_or_default(), r.get(4)?)),
    )
    .unwrap_or_else(|e| panic!("no permission request for {held}: {e}"))
}

fn confirm(bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint("app-approve", Some(bound), "owner", "local-auth")
}

fn publish_run_requests(ws: &Ws, name: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn
        .prepare("SELECT id FROM app_widenings WHERE kind = 'publish_run' AND workspace_id = ?1 AND app_label = ?2 ORDER BY requested_at, id")
        .expect("prepare");
    let ids = st.query_map(rusqlite::params![ws.id, name], |r| r.get::<_, String>(0)).expect("query");
    ids.collect::<rusqlite::Result<Vec<_>>>().expect("rows")
}

/// NSI papercut 13: one owner approval covers identical re-runs of the
/// same App (same command, folder, port, tunnel); any change asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_identical_publish_runs_take_one_approval_and_a_change_asks_again() {
    require_python3();
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000"), ("K2_OPEN_APPS_GATE", "enforce")]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "nsirerun");
    let agent = passport(&ws);
    let port = k2_core::test_env::loopback_test_port();
    let cmd = "exec python3 -m http.server $PORT --bind 127.0.0.1";
    let run = |port: u16| json!({"name": "rerun-sales", "cmd": cmd, "port": port, "noTunnel": true});

    // Run 1: held; the owner approves it once.
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "{}", r.body);
    let first = r.json()["request"]["id"].as_str().expect("held id").to_string();
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": first, "confirmToken": confirm(&first)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);

    // Runs 2 and 3, identical: they run, naming the approval they repeat.
    for n in 2..=3 {
        let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
        assert_eq!(r.status, 200, "stop before run {n}: {}", r.body);
        let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
        assert_eq!(r.status, 200, "identical run {n} goes through: {}", r.body);
        assert_eq!(r.json()["approvedRun"]["request"], first.as_str(), "run {n}: {}", r.body);
    }
    assert_eq!(publish_run_requests(&ws, "rerun-sales"), vec![first.clone()], "three identical runs, one approval");

    // A different port asks again (a new request), and so does a new command.
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run(k2_core::test_env::loopback_test_port())).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "a new port is held: {}", r.body);
    let changed = json!({"name": "rerun-sales", "cmd": "exec python3 -m http.server $PORT --bind 0.0.0.0", "port": port, "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &agent, changed).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "a new command is held: {}", r.body);
    assert_eq!(publish_run_requests(&ws, "rerun-sales").len(), 3, "each change is its own request");

    let r = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "rerun-sales", "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sidecars_held_publish_run_gets_its_card_on_the_sidecar_and_allow_runs_it() {
    require_python3();
    // The owner's gate choice: an agent's new --cmd service is held even
    // where this harness has no open path.
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000"), ("K2_OPEN_APPS_GATE", "enforce")]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());

    let ws = seed_ws(&env, "nsiside");
    let main = pin_chat(&ws);
    let conv = uuid::Uuid::new_v4().to_string();
    sidecar(&ws, &conv, "sales");
    let side = format!("{}/sales", ws.handle);
    // The shim only for the sidecar's own spawn (the App's python3 later is
    // no agent CLI).
    let (agent_name, session, agent) = {
        let _agents = k2_core::test_env::AgentShim::install();
        live_sidecar(&ws, &conv)
    };
    k2_core::permission::gate::set_server_switch(true);
    cards_on(&ws);

    // 1. The hold raises the card, on the sidecar's Thread.
    let port = k2_core::test_env::loopback_test_port();
    let run = json!({"name": "nsi-sales", "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": port, "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &agent, run).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "publish_run", "{v}");
    let held = v["request"]["id"].as_str().expect("held id").to_string();
    assert_eq!((v["card"]["raised"].as_bool(), v["card"]["new"].as_bool()), (Some(true), Some(true)), "the hold raises its card: {v}");
    assert_eq!(v["card"]["thread"], side.as_str(), "on the sidecar's Thread: {v}");
    let card = v["card"]["id"].as_str().expect("card id").to_string();
    let (state, on_conv, addr, card_row, by) = permission_row(&held);
    assert_eq!(state, "pending");
    assert_eq!(on_conv.as_deref(), Some(conv.as_str()), "the sidecar's conversation, not the main one ({main})");
    assert_eq!(addr.as_deref(), Some(side.as_str()));
    assert_eq!(card_row, card);
    assert_eq!(by, None);

    // 2. The agent's own ask finds that card.
    let r = post(
        d.port,
        "/cli/permission/request",
        &agent,
        json!({"action": "apps.approve", "params": {"request": held}, "why": "start the sales App"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let a = r.json();
    assert_eq!((a["status"].as_str(), a["card"].as_str(), a["addr"].as_str()), (Some("already_pending"), Some(card.as_str()), Some(side.as_str())), "{a}");

    // 3. The owner answers Allow on the sidecar's Thread, with the confirm.
    let r = post(
        d.port,
        "/cli/thread/answer",
        OWNER,
        json!({"addr": side, "id": card, "answer": "Allow", "client": "desktop", "confirmToken": confirm(&held)}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);

    // 4. Resolved: both requests approved by the owner, and the service runs.
    let (state, _, _, _, by) = permission_row(&held);
    assert_eq!((state.as_str(), by.as_deref()), ("approved", Some("owner")));
    let r = get(d.port, "/cli/app/approvals", OWNER, "state=all").await;
    let reqs = r.json()["requests"].as_array().cloned().unwrap_or_default();
    let app_req = reqs.iter().find(|x| x["id"] == held.as_str()).unwrap_or_else(|| panic!("{held} listed: {}", r.body)).clone();
    assert_eq!(app_req["status"], "approved", "{app_req}");
    wait_until("the approved service answers", 20, || http_get(port, "/").is_some_and(|(s, _)| s == 200)).await;

    let r = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "nsi-sales", "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // 5. No Thread at all: the card can't go up, so the hold teaches.
    let lone = seed_ws(&env, "nsilone");
    cards_on(&lone);
    let lone_agent = passport(&lone);
    let run = json!({"name": "lone-svc", "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": k2_core::test_env::loopback_test_port(), "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &lone_agent, run).await;
    k2_core::permission::gate::set_server_switch(false);
    k2_daemon::v2_session_map::unregister(&agent_name);
    session.kill();
    k2_daemon::apps::alerts::set_sink(None);
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    let lone_held = v["request"]["id"].as_str().expect("held id");
    assert_eq!(v["card"]["raised"], false, "{v}");
    assert_eq!(v["permission"]["action"], "apps.approve", "the hold ends with its Ask-with object: {v}");
    assert_eq!(
        v["permission"]["command"],
        format!("k2 permission request apps.approve request={lone_held} --why \"<what you need it for>\""),
        "{v}"
    );
}

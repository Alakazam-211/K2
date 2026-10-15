//! Sew S27 grant hook, end to end against a fake Sew mgmt socket.
//!
//! - Delivery (K3): success, retryable (transport, 5xx, 401, 429),
//!   permanent (4xx → blocked, one subject only), lost response replayed
//!   under the same `Idempotency-Key`, restart resumes the outbox, A8
//!   profile round-trip (fill once before the first send).
//! - Reconcile (K4): drift repair, install-id change, rev 147.
//! - Routes: owner-only, POST-only (405), hook state on `GET grants`.
//!
//! Tests drive `tool_hook_worker::{pass_once, reconcile_once}` directly
//! (S27-A3: `test_harness` does not run `daemon_main` tasks); one test
//! spawns the real loop and wakes it with `kick()`.
//!
//! Isolation: `TempHome::short()` (holds the one env lock; short path so
//! the socket fits `SUN_LEN`). Never the real `~/.k2` or `/run/sew`.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::skin::{self, SkinGrantWrite};
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, sew_wire::MemberKind, CHAT_KIND};
use k2_daemon::test_harness;
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
use serde_json::Value;

const OWNER: &str = "owner-token-grant-hook-s27";

fn cfg() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    }
}

struct Env {
    _home: TempHome,
    sew: FakeSew,
}

async fn setup() -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    Env { _home: home, sew }
}

fn enable(env: &Env) {
    hook::set_target(
        CHAT_KIND,
        true,
        Some(env.sew.socket.to_str().unwrap()),
        Some(env.sew.token_file.to_str().unwrap()),
        "owner-token",
    )
    .expect("enable chat target");
}

fn chat(subject_kind: &str, subject: &str, scope: Option<&str>, actor: &str) -> skin::SkinGrant {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: subject_kind.into(),
        subject_id: subject.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: scope.map(str::to_string),
        created_by: Some(actor.into()),
    })
    .expect("chat grant")
}

fn add_ws(prefix: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{prefix}{}", &id[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            id,
            format!("Agent {handle}"),
            format!("/tmp/k2-gh-{id}"),
            handle
        ],
    )
    .expect("insert project");
    (id, handle)
}

fn drop_ws(id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])
        .expect("delete project");
}

fn rows() -> Vec<k2_core::tool_hook::outbox::OutboxRow> {
    let mut r = hook::log(500, None).expect("log");
    r.reverse();
    r
}

fn open_rows() -> Vec<k2_core::tool_hook::outbox::OutboxRow> {
    rows()
        .into_iter()
        .filter(|r| r.state == "pending" || r.state == "blocked")
        .collect()
}

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

/// Run passes (forcing backoff due) until nothing is pending, ≤ 40 passes.
async fn drain(env: &Env) {
    for _ in 0..40 {
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.expect("pass");
        if rows()
            .iter()
            .all(|r| r.state != "pending" || r.op == "sew_session_revoke")
        {
            return;
        }
    }
    panic!(
        "outbox never drained: {:?}\nfake log:\n{}",
        open_rows(),
        env.sew.log()
    );
}

// ── K3: delivery ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_create_provisions_member_and_owner_with_actor_attribution() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "user:boss");
    drain(&env).await;
    let m = env.sew.member(&pext(&ann.id)).expect("ann is a Sew member");
    assert_eq!(
        (m.kind, m.guest, m.username.as_str()),
        (MemberKind::Person, false, "ann")
    );
    assert!(!m.deactivated);
    assert_eq!(
        env.sew.owner().as_deref(),
        Some("k2:owner"),
        "owner provisioned then set"
    );
    let posts = env.sew.posts();
    let ann_post = posts
        .iter()
        .find(|c| c.body["external_id"] == pext(&ann.id))
        .unwrap();
    assert_eq!(
        ann_post.actor.as_deref(),
        Some("user:boss"),
        "Sew-Actor = outbox actor (attribution only)"
    );
    let row = rows()
        .into_iter()
        .find(|r| r.subject_key == pext(&ann.id))
        .unwrap();
    assert_eq!(ann_post.key.as_deref(), Some(row.idem_key.as_str()));
    assert_eq!(row.state, "done");
    let owner_posts: Vec<&str> = posts
        .iter()
        .filter(|c| c.body["external_id"] == "k2:owner")
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(
        owner_posts,
        ["/mgmt/v1/principals/provision", "/mgmt/v1/owner/set"]
    );
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    assert!(t.healthy && t.last_ok_at.is_some());
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guest_scope_update_and_delete_reach_sew() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    let g = chat("principal", "ann", Some("guest"), "owner-token");
    drain(&env).await;
    assert!(env.sew.member(&pext(&ann.id)).unwrap().guest);
    skin::update_grant_scope(&g.id, "member", "owner-token").unwrap();
    drain(&env).await;
    assert!(
        !env.sew.member(&pext(&ann.id)).unwrap().guest,
        "scope update upserts"
    );
    skin::set_principal_full_name_as("ann", Some("Ann Lee"), "owner-token").unwrap();
    skin::set_principal_email_as("ann", Some("ann@example.com"), "owner-token").unwrap();
    drain(&env).await;
    let m = env.sew.member(&pext(&ann.id)).unwrap();
    assert_eq!(
        (m.display_name.as_str(), m.email.as_deref()),
        ("Ann Lee", Some("ann@example.com"))
    );
    skin::delete_grant_by_id(&g.id).unwrap();
    drain(&env).await;
    assert!(
        env.sew.member(&pext(&ann.id)).unwrap().deactivated,
        "grant delete deprovisions"
    );
    env.sew.assert_clean();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assistant_grant_waits_for_person_then_provisions() {
    let env = setup().await;
    enable(&env);
    drain(&env).await; // owner first
    let ann = skin::add_principal("ann").unwrap();
    let (wid, handle) = add_ws("asst");
    chat("principal", "ann", None, "owner-token");
    chat("workspace", &handle, Some("assistant:ann"), "owner-token");
    // The person's provision fails (503) → the assistant is NOT sent.
    env.sew.fail_next(1, 503, "unavailable", "");
    worker::pass_once(&cfg()).await.unwrap();
    let ws_ext = format!("k2:workspace:{wid}");
    assert!(
        env.sew
            .posts()
            .iter()
            .all(|c| c.body["external_id"] != ws_ext),
        "{}",
        env.sew.log()
    );
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    assert!(!t.healthy, "5xx marks the target unhealthy");
    drain(&env).await;
    let a = env
        .sew
        .member(&ws_ext)
        .expect("assistant provisioned after the person");
    assert_eq!(a.assistant_of.as_deref(), Some(pext(&ann.id).as_str()));
    assert_eq!(a.username, handle);
    let order: Vec<String> = env
        .sew
        .posts()
        .iter()
        .filter(|c| (200..300).contains(&c.status))
        .map(|c| c.body["external_id"].as_str().unwrap_or("").to_string())
        .collect();
    let pi = order.iter().position(|e| *e == pext(&ann.id)).unwrap();
    let ai = order.iter().position(|e| *e == ws_ext).unwrap();
    assert!(pi < ai, "person before assistant: {order:?}");
    drop_ws(&wid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sew_down_retries_then_succeeds_exactly_once() {
    let mut env = setup().await;
    enable(&env);
    env.sew.down();
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    for _ in 0..3 {
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.unwrap();
    }
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    assert!(!t.healthy);
    assert!(
        t.last_error.as_deref().unwrap_or("").starts_with("connect"),
        "{t:?}"
    );
    let pending: Vec<_> = rows()
        .into_iter()
        .filter(|r| r.state == "pending")
        .collect();
    assert_eq!(
        pending.len(),
        3,
        "owner ×2 + ann stay pending, never blocked: {pending:?}"
    );
    assert!(pending
        .iter()
        .any(|r| r.attempts >= 1 && r.last_status.is_none()));
    env.sew.up();
    drain(&env).await;
    assert_eq!(
        env.sew
            .applied("/mgmt/v1/principals/provision", &pext(&ann.id)),
        1
    );
    assert!(hook::get_target(CHAT_KIND).unwrap().unwrap().healthy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_restart_resumes_outbox() {
    let mut env = setup().await;
    enable(&env);
    env.sew.down();
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    // "Restart": the rows were committed with no worker running; a fresh
    // worker (the boot pass of the real loop) against the same HOME
    // delivers them once Sew is back.
    env.sew.up();
    let h = worker::spawn_with(cfg());
    env.sew
        .wait_for("ann provisioned by the spawned worker", |s| {
            s.member(&pext(&ann.id)).is_some_and(|m| !m.deactivated)
        })
        .await;
    // And a later grant is picked up by a kick, not a manual pass.
    let bob = skin::add_principal("bob").unwrap();
    chat("principal", "bob", None, "owner-token");
    env.sew
        .wait_for("bob provisioned after kick()", |s| {
            s.member(&pext(&bob.id)).is_some()
        })
        .await;
    h.abort();
    let _ = h.await;
    assert_eq!(
        env.sew
            .applied("/mgmt/v1/principals/provision", &pext(&ann.id)),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_response_is_replayed_not_duplicated() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    env.sew.apply_then_drop_connection_next();
    worker::pass_once(&cfg()).await.unwrap();
    let row = rows()
        .into_iter()
        .find(|r| r.subject_key == pext(&ann.id))
        .unwrap();
    assert_eq!(
        (row.state.as_str(), row.attempts),
        ("pending", 1),
        "EOF = transient: {row:?}"
    );
    drain(&env).await;
    let calls: Vec<_> = env
        .sew
        .posts()
        .into_iter()
        .filter(|c| c.body["external_id"] == pext(&ann.id))
        .collect();
    assert_eq!(calls.len(), 2, "{}", env.sew.log());
    assert_eq!(
        calls[0].key, calls[1].key,
        "same Idempotency-Key on the retry"
    );
    assert!(calls[1].replayed, "Sew replayed the stored response");
    assert_eq!(
        env.sew
            .applied("/mgmt/v1/principals/provision", &pext(&ann.id)),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_subject_order_preserved_through_errors() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    let g = chat("principal", "ann", None, "owner-token");
    skin::delete_grant_by_id(&g.id).unwrap();
    env.sew.fail_next(2, 503, "unavailable", "");
    drain(&env).await;
    let ok: Vec<String> = env
        .sew
        .posts()
        .iter()
        .filter(|c| c.body["external_id"] == pext(&ann.id) && c.status == 200)
        .map(|c| c.path.clone())
        .collect();
    assert_eq!(
        ok,
        [
            "/mgmt/v1/principals/provision",
            "/mgmt/v1/principals/deprovision"
        ]
    );
    assert!(env.sew.member(&pext(&ann.id)).unwrap().deactivated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permanent_error_blocks_one_subject_retry_rearms_skip_then_reconcile_repairs() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    let bob = skin::add_principal("bob").unwrap();
    chat("principal", "ann", None, "owner-token");
    env.sew.fail_next(1, 400, "validation_failed", "");
    chat("principal", "bob", None, "owner-token");
    drain(&env).await;
    let ann_row = rows()
        .into_iter()
        .find(|r| r.subject_key == pext(&ann.id))
        .unwrap();
    assert_eq!(ann_row.state, "blocked", "{ann_row:?}");
    assert_eq!(
        ann_row.last_error.as_deref(),
        Some("validation_failed: injected")
    );
    assert!(
        env.sew.member(&pext(&bob.id)).is_some(),
        "another subject still delivers"
    );
    let blocked = hook::status().unwrap()["blocked"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(blocked.len(), 1);
    assert!(
        blocked[0]["payload"].is_object(),
        "blocked rows carry their payload"
    );
    // Retry re-arms it.
    hook::retry(&ann_row.id).unwrap();
    drain(&env).await;
    assert!(env.sew.member(&pext(&ann.id)).is_some());
    // Skip, then reconcile repairs: block a deprovision, skip it, reconcile deprovisions.
    let g = skin::list_grants()
        .unwrap()
        .into_iter()
        .find(|g| g.subject_id == bob.id && g.kind == "chat")
        .unwrap();
    env.sew.fail_next(1, 403, "missing_permission", "");
    skin::delete_grant_by_id(&g.id).unwrap();
    drain(&env).await;
    let dep = rows()
        .into_iter()
        .rev()
        .find(|r| r.subject_key == pext(&bob.id))
        .unwrap();
    assert_eq!(
        (dep.op.as_str(), dep.state.as_str()),
        ("deprovision", "blocked")
    );
    assert!(!env.sew.member(&pext(&bob.id)).unwrap().deactivated);
    hook::skip(&dep.id).unwrap();
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert!(
        env.sew.member(&pext(&bob.id)).unwrap().deactivated,
        "reconcile repaired the skip"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn username_conflict_uses_suffix_once() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let (wid, handle) = add_ws("sales");
    env.sew
        .seed("local:taken", MemberKind::Person, &handle, "Local", false);
    chat("workspace", &handle, Some("coworker"), "owner-token");
    drain(&env).await;
    let ext = format!("k2:workspace:{wid}");
    let m = env.sew.member(&ext).expect("provisioned with a suffix");
    assert_eq!(m.username, hook::chat::suffixed_username(&handle, &ext));
    let keys: Vec<String> = env
        .sew
        .posts()
        .iter()
        .filter(|c| c.body["external_id"] == ext)
        .filter_map(|c| c.key.clone())
        .collect();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[1], format!("{}_u", keys[0]));
    // Reconcile accepts the suffixed handle as correct.
    let before = env.sew.posts().len();
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert_eq!(env.sew.posts().len(), before, "no drift: {}", env.sew.log());
    drop_ws(&wid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_honors_retry_after_and_bad_token_keeps_rows_pending() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    env.sew.rate_limit_next(30);
    let now = k2_core::tool_hook::outbox::now_secs();
    worker::pass_once(&cfg()).await.unwrap();
    let r = rows()
        .into_iter()
        .find(|r| r.subject_key == pext(&ann.id))
        .unwrap();
    assert_eq!((r.state.as_str(), r.last_status), ("pending", Some(429)));
    assert!(r.next_attempt_at >= now + 30, "Retry-After honoured: {r:?}");
    worker::pass_once(&cfg()).await.unwrap();
    assert!(
        env.sew.member(&pext(&ann.id)).is_none(),
        "not retried before Retry-After"
    );
    // Bad token: rows stay pending, target unhealthy.
    env.sew.set_token("rotated");
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.unwrap();
    let r = rows()
        .into_iter()
        .find(|r| r.subject_key == pext(&ann.id))
        .unwrap();
    assert_eq!((r.state.as_str(), r.last_status), ("pending", Some(401)));
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    assert_eq!(
        (t.healthy, t.last_error.as_deref()),
        (false, Some("mgmt token rejected"))
    );
    std::fs::write(&env.sew.token_file, "rotated\n").unwrap();
    drain(&env).await;
    assert!(env.sew.member(&pext(&ann.id)).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_target_pauses_outbox() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    let g = chat("principal", "ann", None, "owner-token");
    hook::set_target(CHAT_KIND, false, None, None, "owner-token").unwrap();
    skin::delete_grant_by_id(&g.id).unwrap(); // delete is allowed while disabled
    let n = env.sew.calls().len();
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.unwrap();
    worker::reconcile_once().await.unwrap();
    assert_eq!(env.sew.calls().len(), n, "paused: no calls");
    assert_eq!(
        open_rows().len(),
        2,
        "provision + deprovision wait, not blocked"
    );
    enable(&env);
    drain(&env).await;
    assert!(env.sew.member(&pext(&ann.id)).unwrap().deactivated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assistant_person_inactive_blocks_at_once() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let (wid, handle) = add_ws("ina");
    skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    drain(&env).await;
    chat("workspace", &handle, Some("assistant:ann"), "owner-token");
    env.sew
        .fail_next(1, 422, "invalid_state", "assistant_person_inactive");
    drain(&env).await;
    let r = rows()
        .into_iter()
        .rev()
        .find(|r| r.subject_key == format!("k2:workspace:{wid}"))
        .unwrap();
    assert_eq!(r.state, "blocked");
    drop_ws(&wid);
}

// ── A8: provision is declarative; K2 round-trips what it does not own ──

/// `GET /mgmt/v1/principals` calls (lookups during delivery passes; the
/// fake records the path without its query).
fn principal_gets(env: &Env) -> usize {
    env.sew
        .calls()
        .iter()
        .filter(|c| c.method == "GET" && c.path == "/mgmt/v1/principals" && c.status == 200)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a8_guest_flip_and_repin_keep_sew_owned_profile_fields() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    skin::set_principal_email_as("ann", Some("ann@k2.io"), "owner-token").unwrap();
    let g = chat("principal", "ann", None, "owner-token");
    let (wid, handle) = add_ws("pin");
    chat("workspace", &handle, Some("assistant:ann"), "owner-token");
    drain(&env).await;
    // People edit their Sew-local profile; an admin gives the agent a Sew email.
    let ws_ext = format!("k2:workspace:{wid}");
    env.sew.set_sew_profile(
        &pext(&ann.id),
        Some("ann@k2.io"),
        Some("CTO"),
        Some("she/her"),
    );
    env.sew
        .set_sew_profile(&ws_ext, Some("bot@sew.io"), Some("Scheduler"), None);
    // Re-pin the assistant to the owner, then flip... the person can't go
    // guest while pinned, so re-pin first.
    let wg = skin::list_grants()
        .unwrap()
        .into_iter()
        .find(|x| x.subject_id == wid)
        .unwrap();
    skin::update_grant_scope(&wg.id, "assistant:owner", "owner-token").unwrap();
    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    drain(&env).await;
    let a = env.sew.member(&pext(&ann.id)).unwrap();
    assert!(a.guest, "guest flip applied");
    assert_eq!(
        (
            a.title.as_deref(),
            a.pronouns.as_deref(),
            a.email.as_deref()
        ),
        (Some("CTO"), Some("she/her"), Some("ann@k2.io")),
        "a re-provision must not clear the Sew-local profile: {}",
        env.sew.log()
    );
    let w = env.sew.member(&ws_ext).unwrap();
    assert_eq!(w.assistant_of.as_deref(), Some("k2:owner"));
    assert_eq!(
        (w.email.as_deref(), w.title.as_deref()),
        (Some("bot@sew.io"), Some("Scheduler")),
        "an agent's email is Sew's: round-tripped"
    );
    // The bodies Sew received carried the fields (declarative provision).
    let flip = env
        .sew
        .posts()
        .into_iter()
        .filter(|c| c.body["external_id"] == pext(&ann.id))
        .last()
        .unwrap();
    assert_eq!(flip.body["profile"]["title"], "CTO");
    assert_eq!(flip.body["guest"], true);
    env.sew.assert_clean();
    drop_ws(&wid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a8_fill_happens_once_before_the_first_send_and_retries_reuse_it() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    let g = chat("principal", "ann", None, "owner-token");
    drain(&env).await;
    env.sew
        .set_sew_profile(&pext(&ann.id), None, Some("CTO"), None);
    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    let gets_before = principal_gets(&env);
    env.sew.fail_next(2, 503, "unavailable", "");
    drain(&env).await;
    let gets = principal_gets(&env) - gets_before;
    assert_eq!(
        gets,
        1,
        "one lookup, then the persisted body: {}",
        env.sew.log()
    );
    let sends: Vec<_> = env
        .sew
        .posts()
        .into_iter()
        .filter(|c| c.body["external_id"] == pext(&ann.id) && c.body["guest"] == true)
        .collect();
    assert_eq!(sends.len(), 3, "503, 503, 200: {}", env.sew.log());
    assert!(
        sends.iter().all(|c| c.key == sends[0].key),
        "same key on every retry"
    );
    assert!(
        sends.iter().all(|c| c.body == sends[0].body),
        "same bytes on every retry"
    );
    assert_eq!(sends[0].body["profile"]["title"], "CTO");
    let row = rows()
        .into_iter()
        .filter(|r| r.subject_key == pext(&ann.id))
        .last()
        .unwrap();
    assert!(row.filled_at.is_some());
    assert_eq!(
        env.sew.member(&pext(&ann.id)).unwrap().title.as_deref(),
        Some("CTO")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a8_pre_sw14_list_still_provisions_and_lookup_failure_retries() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    let g = chat("principal", "ann", None, "owner-token");
    drain(&env).await;
    // An older Sew: the list has no profile fields → unknown; the flip
    // still goes through (Sew clears the title: nothing K2 can see).
    env.sew.set_list_profile_fields(false);
    env.sew
        .set_sew_profile(&pext(&ann.id), None, Some("CTO"), None);
    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    drain(&env).await;
    let m = env.sew.member(&pext(&ann.id)).unwrap();
    assert!(m.guest);
    assert_eq!(
        m.title, None,
        "pre-SW14: unknown fields cannot be round-tripped"
    );
    // A lookup that cannot reach Sew sends nothing and stays pending.
    let mut env = env;
    skin::update_grant_scope(&g.id, "member", "owner-token").unwrap();
    env.sew.down();
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.unwrap();
    let r = rows()
        .into_iter()
        .filter(|r| r.subject_key == pext(&ann.id))
        .last()
        .unwrap();
    assert_eq!((r.state.as_str(), r.filled_at), ("pending", None), "{r:?}");
    assert!(
        r.last_error
            .as_deref()
            .unwrap_or("")
            .starts_with("member lookup:"),
        "{r:?}"
    );
    env.sew.up();
    drain(&env).await;
    assert!(!env.sew.member(&pext(&ann.id)).unwrap().guest);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a8_profile_drift_is_repaired_with_update_and_keeps_title() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    skin::set_principal_email_as("ann", Some("ann@k2.io"), "owner-token").unwrap();
    chat("principal", "ann", None, "owner-token");
    drain(&env).await;
    // Drift in Sew: wrong email and name; a Sew-local title K2 must keep.
    env.sew
        .set_sew_profile(&pext(&ann.id), Some("wrong@x.io"), Some("CTO"), None);
    env.sew.set_display_name(&pext(&ann.id), "Wrong");
    let provisions_before = env
        .sew
        .applied("/mgmt/v1/principals/provision", &pext(&ann.id));
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    let m = env.sew.member(&pext(&ann.id)).unwrap();
    assert_eq!(
        (
            m.email.as_deref(),
            m.display_name.as_str(),
            m.title.as_deref()
        ),
        (Some("ann@k2.io"), "ann", Some("CTO"))
    );
    assert_eq!(
        env.sew
            .applied("/mgmt/v1/principals/provision", &pext(&ann.id)),
        provisions_before,
        "profile-only drift never re-provisions"
    );
    assert_eq!(
        env.sew
            .applied("/mgmt/v1/principals/update", &pext(&ann.id)),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hc2_provision_has_no_skipped_so_s27_never_holds_a_member() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    drain(&env).await;
    assert!(rows().iter().all(|r| r.state != "held"), "{:?}", rows());
    assert!(env.sew.member(&pext(&ann.id)).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_chat_grants_do_not_call_sew() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let n = env.sew.calls().len();
    let ann = skin::add_principal("ann").unwrap();
    for kind in ["mailbox", "database", "cli"] {
        let g = skin::create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ann.id.clone(),
            kind: kind.into(),
            target_id: "t".into(),
            role: None,
            scope: Some("s".into()),
            created_by: None,
        })
        .unwrap();
        skin::delete_grant_by_id(&g.id).unwrap();
    }
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.unwrap();
    assert_eq!(env.sew.calls().len(), n, "{}", env.sew.log());
}

// ── K4: reconcile ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_fixes_drift_and_leaves_local_members_alone() {
    let env = setup().await;
    enable(&env);
    drain(&env).await;
    let ann = skin::add_principal("ann").unwrap();
    let bob = skin::add_principal("bob").unwrap();
    let cat = skin::add_principal("cat").unwrap();
    chat("principal", "ann", None, "owner-token");
    chat("principal", "bob", None, "owner-token");
    chat("principal", "cat", Some("guest"), "owner-token");
    drain(&env).await;
    // Drift: ann missing, bob renamed in Sew, cat no longer a guest, an
    // extra k2:principal member, and a local member.
    env.sew.with_members(|m| {
        m.remove(&pext(&ann.id));
        m.get_mut(&pext(&bob.id)).unwrap().display_name = "Wrong".into();
        m.get_mut(&pext(&cat.id)).unwrap().guest = false;
    });
    let stray = uuid::Uuid::new_v4().to_string();
    env.sew
        .seed(&pext(&stray), MemberKind::Person, "stray", "Stray", false);
    env.sew
        .seed("local:solo", MemberKind::Person, "solo", "Solo", false);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert!(env
        .sew
        .member(&pext(&ann.id))
        .is_some_and(|m| !m.deactivated));
    assert_eq!(env.sew.member(&pext(&bob.id)).unwrap().display_name, "bob");
    assert!(env.sew.member(&pext(&cat.id)).unwrap().guest);
    assert!(env.sew.member(&pext(&stray)).unwrap().deactivated);
    assert!(
        !env.sew.member("local:solo").unwrap().deactivated,
        "local members untouched"
    );
    assert_eq!(
        env.sew.applied("/mgmt/v1/owner/set", "k2:owner"),
        1,
        "reconcile never calls owner_set again"
    );
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    let last = t.last_reconcile.expect("summary stored");
    assert_eq!(last["ignoredLocal"], 1);
    // Standalone Sew: reconcile refuses and marks the target unhealthy.
    env.sew.set_provider("standalone");
    let rep = worker::reconcile_once().await.unwrap();
    assert_eq!(rep.failed.len(), 1);
    let t = hook::get_target(CHAT_KIND).unwrap().unwrap();
    assert_eq!(t.last_error.as_deref(), Some("Sew is in standalone mode"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_removed_reconcile_deprovisions() {
    let env = setup().await;
    enable(&env);
    let (wid, handle) = add_ws("gone");
    chat("workspace", &handle, Some("coworker"), "owner-token");
    drain(&env).await;
    let ext = format!("k2:workspace:{wid}");
    assert!(env.sew.member(&ext).is_some());
    drop_ws(&wid);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert!(env.sew.member(&ext).unwrap().deactivated);
    assert!(skin::list_grants()
        .unwrap()
        .iter()
        .all(|g| g.subject_id != wid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_id_change_reprovisions_owner_and_members() {
    let env = setup().await;
    enable(&env);
    let ann = skin::add_principal("ann").unwrap();
    chat("principal", "ann", None, "owner-token");
    worker::reconcile_once().await.unwrap(); // records install id 1001
    drain(&env).await;
    env.sew.reinstall("2002");
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert_eq!(
        env.sew.owner().as_deref(),
        Some("k2:owner"),
        "owner set again on the new install"
    );
    assert!(env
        .sew
        .member(&pext(&ann.id))
        .is_some_and(|m| !m.deactivated));
    assert_eq!(
        hook::get_target(CHAT_KIND)
            .unwrap()
            .unwrap()
            .install_id
            .as_deref(),
        Some("2002")
    );
    let causes: Vec<String> = rows()
        .into_iter()
        .filter(|r| r.subject_key == "k2:owner")
        .map(|r| r.cause)
        .collect();
    assert!(
        causes.contains(&"install_changed".to_string()),
        "{causes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rev147_connect_owner_and_admin_never_become_members() {
    let env = setup().await;
    k2_core::connect_users::add_user("boss", "pw-boss-long-1").unwrap();
    k2_core::connect_users::set_role("boss", k2_core::connect_users::Role::Owner).unwrap();
    k2_core::connect_users::add_user("adm", "pw-adm-long-1").unwrap();
    k2_core::connect_users::set_role("adm", k2_core::connect_users::Role::Admin).unwrap();
    let boss = k2_core::connect_users::create_session("boss");
    let d = test_harness::start(OWNER).await;
    // Enable + grant through the routes as the Owner-role Connect user.
    let body = format!(
        r#"{{"kind":"chat","enabled":true,"socket":"{}","tokenFile":"{}"}}"#,
        env.sew.socket.display(),
        env.sew.token_file.display()
    );
    let r = http(
        d.port,
        "POST",
        &format!("/cli/skin/grant-hooks/target?token={boss}"),
        Some(&body),
    );
    assert_eq!(r.0, 200, "{}", r.1);
    let ann = skin::add_principal("ann").unwrap();
    let r = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants?token={boss}"),
        Some(r#"{"kind":"chat","subjectKind":"principal","subjectId":"ann"}"#),
    );
    assert_eq!(r.0, 200, "{}", r.1);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    let exts: Vec<String> = env
        .sew
        .members()
        .into_iter()
        .map(|m| m.external_id)
        .collect();
    assert_eq!(exts.len(), 2, "{exts:?}");
    assert!(exts.contains(&"k2:owner".to_string()));
    assert!(exts.contains(&pext(&ann.id)));
    let ann_post = env
        .sew
        .posts()
        .into_iter()
        .find(|c| c.body["external_id"] == pext(&ann.id))
        .unwrap();
    assert_eq!(
        ann_post.actor.as_deref(),
        Some("user:boss"),
        "Connect owner is attribution, not a member"
    );
}

// ── routes ───────────────────────────────────────────────────────────

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_string();
            let len = head.lines().find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
            });
            if let Some(len) = len {
                if raw.len() >= end + 4 + len {
                    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
                    return (
                        status,
                        String::from_utf8_lossy(&raw[end + 4..end + 4 + len]).to_string(),
                    );
                }
            }
        }
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) => panic!("read: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_string(),
    )
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("not JSON ({e}): {s}"))
}

const POST_ROUTES: &[&str] = &[
    "/cli/skin/grants/update",
    "/cli/skin/grant-hooks/target",
    "/cli/skin/grant-hooks/retry",
    "/cli/skin/grant-hooks/skip",
    "/cli/skin/grant-hooks/reconcile",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_hook_mutations_are_post_only() {
    let _env = setup().await;
    let d = test_harness::start(OWNER).await;
    for p in POST_ROUTES {
        let (st, body) = http(d.port, "GET", &format!("{p}?token={OWNER}"), None);
        assert_eq!(st, 405, "{p}: {body}");
        assert_eq!(json(&body)["error"], "POST required", "{p}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_hook_routes_owner_only() {
    let _env = setup().await;
    k2_core::connect_users::add_user("adm", "pw-adm-long-1").unwrap();
    k2_core::connect_users::set_role("adm", k2_core::connect_users::Role::Admin).unwrap();
    k2_core::connect_users::add_user("mem", "pw-mem-long-1").unwrap();
    let adm = k2_core::connect_users::create_session("adm");
    let mem = k2_core::connect_users::create_session("mem");
    let d = test_harness::start(OWNER).await;
    for tok in [adm.as_str(), mem.as_str(), "k2skn_notreal", ""] {
        for p in POST_ROUTES {
            let (st, body) = http(d.port, "POST", &format!("{p}?token={tok}"), Some("{}"));
            assert_eq!(st, 403, "{p} as {tok:?}: {body}");
        }
        for p in ["/cli/skin/grant-hooks", "/cli/skin/grant-hooks/log"] {
            let (st, body) = http(d.port, "GET", &format!("{p}?token={tok}"), None);
            assert_eq!(st, 403, "{p} as {tok:?}: {body}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routes_drive_the_hook_end_to_end() {
    let env = setup().await;
    let d = test_harness::start(OWNER).await;
    let ann = skin::add_principal("ann").unwrap();
    let (wid, handle) = add_ws("rt");
    // Before enable: a chat grant is a loud 400.
    let (st, body) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants?token={OWNER}"),
        Some(r#"{"kind":"chat","subjectKind":"principal","subjectId":"ann"}"#),
    );
    assert_eq!(
        (st, json(&body)["error"].as_str()),
        (400, Some(hook::CHAT_NOT_ENABLED))
    );
    let body = format!(
        r#"{{"kind":"chat","enabled":true,"socket":"{}","tokenFile":"{}"}}"#,
        env.sew.socket.display(),
        env.sew.token_file.display()
    );
    let (st, b) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grant-hooks/target?token={OWNER}"),
        Some(&body),
    );
    assert_eq!(st, 200, "{b}");
    assert_eq!(json(&b)["enabled"], true);
    let (st, b) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants?token={OWNER}"),
        Some(r#"{"kind":"chat","subjectKind":"principal","subjectId":"ann"}"#),
    );
    assert_eq!(st, 200, "{b}");
    let person = json(&b);
    assert_eq!(
        (person["targetId"].as_str(), person["scope"].as_str()),
        (Some("sew"), Some("member"))
    );
    let (st, b) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants?token={OWNER}"),
        Some(&format!(
            r#"{{"kind":"chat","subjectKind":"workspace","subjectId":"{handle}","scope":"assistant:ann"}}"#
        )),
    );
    assert_eq!(st, 200, "{b}");
    // GET grants shows hook state; pending before delivery.
    let (_, b) = http(
        d.port,
        "GET",
        &format!("/cli/skin/grants?token={OWNER}"),
        None,
    );
    let g = json(&b);
    let chat_rows: Vec<&Value> = g["grants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["kind"] == "chat")
        .collect();
    assert_eq!(chat_rows.len(), 2);
    assert!(chat_rows.iter().all(|x| x["hookState"] == "pending"), "{b}");
    drain(&env).await;
    let (_, b) = http(
        d.port,
        "GET",
        &format!("/cli/skin/grants?token={OWNER}"),
        None,
    );
    assert!(
        json(&b)["grants"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["kind"] == "chat")
            .all(|x| x["hookState"] == "synced"),
        "{b}"
    );
    // Status + log.
    let (st, b) = http(
        d.port,
        "GET",
        &format!("/cli/skin/grant-hooks?token={OWNER}"),
        None,
    );
    assert_eq!(st, 200, "{b}");
    let s = json(&b);
    assert_eq!(s["targets"][0]["kind"], "chat");
    assert_eq!(s["outbox"]["pending"], 0);
    let subj = format!("k2:principal%3A{}", ann.id);
    let (st, b) = http(
        d.port,
        "GET",
        &format!("/cli/skin/grant-hooks/log?token={OWNER}&limit=5&subject={subj}"),
        None,
    );
    assert_eq!(st, 200, "{b}");
    let log = json(&b);
    let rows = log["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{b}");
    assert!(
        rows[0].get("payload").is_none(),
        "done rows never carry the payload"
    );
    let (st, _) = http(
        d.port,
        "GET",
        &format!("/cli/skin/grant-hooks/log?token={OWNER}&limit=9999"),
        None,
    );
    assert_eq!(st, 400);
    // Delete the person: the response lists the cascaded assistant grant.
    let (st, b) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants/delete?token={OWNER}"),
        Some(&format!(r#"{{"id":"{}"}}"#, person["id"].as_str().unwrap())),
    );
    assert_eq!(st, 200, "{b}");
    let del = json(&b);
    assert_eq!(del["cascaded"].as_array().unwrap().len(), 1, "{b}");
    assert_eq!(del["cascaded"][0]["subjectId"], wid);
    // Update on a non-chat grant is a 400; retry of an unknown row is a 400.
    let (st, _) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grants/update?token={OWNER}"),
        Some(r#"{"id":"nope","scope":"guest"}"#),
    );
    assert_eq!(st, 400);
    let (st, _) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grant-hooks/retry?token={OWNER}"),
        Some(r#"{"id":"nope"}"#),
    );
    assert_eq!(st, 400);
    let (st, b) = http(
        d.port,
        "POST",
        &format!("/cli/skin/grant-hooks/reconcile?token={OWNER}"),
        Some("{}"),
    );
    assert_eq!((st, json(&b)["started"].as_bool()), (200, Some(true)));
    drop_ws(&wid);
}

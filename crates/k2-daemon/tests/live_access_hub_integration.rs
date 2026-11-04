//! The hub: an open socket hears about an access change at once
//! (`prd-live-access-changes-v1` S2).
//!
//! `live_access_integration.rs` proves what a socket does when access goes, however
//! it learns. These prove it learns in well under a tick, idle, from every kind of
//! writer; that people whose access didn't change hear nothing; and what's left with
//! the hub off.

#[path = "support/app_fixture.rs"]
mod app_fixture;
#[path = "support/live_sockets.rs"]
mod live_sockets;

use std::time::{Duration, Instant};

use app_fixture::*;
use live_sockets::*;
use serde_json::json;

/// Well under the 5 s tick, and room for a busy node.
const AT_ONCE_MS: u64 = 2_500;

struct Four {
    overlay: Sock,
    activity: Sock,
    files: Sock,
    project: Sock,
}

async fn four(port: u16, ws: &Ws, conv: &str, group: &str, pass: &str) -> Four {
    let t = enc(pass);
    let out = Four {
        overlay: open(port, &format!("/cli/overlay/events?conversation={conv}&token={t}")).await,
        activity: open(port, &format!("/cli/activity/events?workspace={}&token={t}", ws.handle)).await,
        files: open(port, &format!("/cli/fs/events?workspace={}&token={t}", ws.handle)).await,
        project: open(port, &format!("/cli/skin/projects/events?group={group}&token={t}")).await,
    };
    // A socket registers with the hub just after it opens.
    tokio::time::sleep(Duration::from_millis(300)).await;
    out
}

async fn all_close(s: &mut Four, want: u16, why: &str) {
    let t0 = Instant::now();
    let (a, b, c, d) = tokio::join!(watch(&mut s.overlay, AT_ONCE_MS), watch(&mut s.activity, AT_ONCE_MS), watch(&mut s.files, AT_ONCE_MS), watch(&mut s.project, AT_ONCE_MS));
    for (what, seen) in [("overlay", &a), ("activity", &b), ("files", &c), ("project chat", &d)] {
        assert_eq!(seen.close, Some(want), "{why}: {what} closes well inside a tick: {seen:?}");
    }
    eprintln!("[hub] {why}: closed within {:?}", t0.elapsed());
}

/// Idle sockets, no frame caused: the hub closes them, well before the tick.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_socket_closes_at_once() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "hubidle");
    let conv = pin_chat(&ws);
    let group = project(port, "hubidleproj", &[&ws]).await;

    let (_, roomless) = guest_in(port, &ws, "hubroomless", ALL_CAPS).await;
    let (_, revoked) = guest_in(port, &ws, "hubrevoked", ALL_CAPS).await;
    let (_, kept) = guest_in(port, &ws, "hubkept", ALL_CAPS).await;
    let mut lost_s = four(port, &ws, &conv, &group, &roomless).await;
    let mut gone_s = four(port, &ws, &conv, &group, &revoked).await;
    let mut kept_s = four(port, &ws, &conv, &group, &kept).await;

    owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": "hubroomless" })).await;
    all_close(&mut lost_s, 4013, "the room went").await;

    let id = k2_core::skin::resolve_skin_token(&revoked).expect("live").id;
    assert!(k2_core::skin::revoke_token(&id).expect("revoke"));
    all_close(&mut gone_s, 4003, "the session went").await;

    // Past both changes: someone whose access didn't change is still there, and
    // heard nothing but the usual.
    let (a, b, c, e) = tokio::join!(watch(&mut kept_s.overlay, 500), watch(&mut kept_s.activity, 500), watch(&mut kept_s.files, 500), watch(&mut kept_s.project, 500));
    for (what, seen) in [("overlay", &a), ("files", &c), ("project chat", &e)] {
        assert!(!seen.ended && seen.frames.is_empty(), "{what}: {seen:?}");
    }
    // Activity keeps its usual news: someone left the room's member list.
    assert!(!b.ended && b.frames.iter().all(|f| f.contains("room_members_changed")), "activity: {b:?}");
    drop(kept_s);
    let deadline = Instant::now() + Duration::from_secs(5);
    while k2_daemon::access_live::open_passes() != 0 {
        assert!(Instant::now() < deadline, "the hub lets go of a pass when its last socket ends: {}", k2_daemon::access_live::open_passes());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A project's members live in the main database (V11): no skin.db write happens,
/// and access can still go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_project_growing_past_a_guest_closes_its_socket_at_once() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "hubproj");
    let other = seed_ws(&env, "hubprojother");
    let group = project(port, "hubprojgrows", &[&ws]).await;
    let (_, pass) = guest_in(port, &ws, "hubprojguest", ALL_CAPS).await;
    let mut s = open(port, &format!("/cli/skin/projects/events?group={group}&token={}", enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    owner_ok(port, "/cli/project-group/add-member", json!({ "group": group, "workspace": other.id })).await;
    let seen = watch(&mut s, AT_ONCE_MS).await;
    assert_eq!(seen.close, Some(4013), "{seen:?}");
}

/// A write the hook can't see (another connection: the other daemon during a
/// handoff): the sweep finds it. Set short here; 30 s in production.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sweep_finds_a_write_the_hook_cant_see() {
    let env = setup(&[("K2_ACCESS_SWEEP_MS", "400")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "hubsweep");
    let (_, pass) = guest_in(port, &ws, "hubsweepguest", ALL_CAPS).await;
    let mut s = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let generation = k2_core::skin::write_generation();

    let id = k2_core::skin::resolve_skin_token(&pass).expect("live").id;
    let other = rusqlite::Connection::open(env.path().join(".k2/skin.db")).expect("another connection");
    let n = other.execute("UPDATE tokens SET revoked_at = 1 WHERE id = ?1", [&id]).expect("write");
    assert_eq!(n, 1, "rig");
    drop(other);

    let seen = watch(&mut s, 3_000).await;
    assert_eq!(seen.close, Some(4003), "{seen:?}");
    assert_eq!(k2_core::skin::write_generation(), generation, "rig: the hook really didn't see it");
}

/// An outsider can't move the generation or wake the hub: a failed login writes
/// only `login_lockouts`, and a sign-in only a new `tokens` row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_login_or_a_new_sign_in_moves_nothing() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "hubquiet");
    let _ = guest_in(port, &ws, "hubquietguest", ALL_CAPS).await;
    let _ = guest_in(port, &ws, "hubquietlocked", ALL_CAPS).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = k2_core::skin::write_generation();
    // Enough to lock the account out too: that's only `login_lockouts`.
    for pw in ["wrong-horse-1", "wrong-horse-2", "wrong-horse-3", "wrong-horse-4", "wrong-horse-5", "wrong-horse-6"] {
        let body = json!({ "username": "hubquietlocked", "password": pw }).to_string();
        let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
        assert!(r.status == 401 || r.status == 423, "{}: {}", r.status, r.body);
    }
    let body = json!({ "username": "nobody-at-all", "password": "wrong-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 401, "{}", r.body);
    assert_eq!(k2_core::skin::write_generation(), before, "failed logins");
    let body = json!({ "username": "hubquietguest", "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "{}", r.body);
    let pass = r.json()["token"].as_str().expect("token").to_string();
    assert_eq!(k2_core::skin::write_generation(), before, "a sign-in");
    let me = get(port, "/cli/skin/me", &pass, "x=1").await;
    assert_eq!(me.status, 200, "{}", me.body);
    assert_eq!(k2_core::skin::write_generation(), before, "a read");
}

/// `K2_ACCESS_HUB=off`: no hub, and the tick is what closes an idle socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_hub_off_the_tick_still_closes_it() {
    let env = setup(&[("K2_ACCESS_HUB", "off")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "huboff");
    let (_, pass) = guest_in(port, &ws, "huboffguest", ALL_CAPS).await;
    let mut s = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    // A socket from the test before may still be ending with its runtime.
    let deadline = Instant::now() + Duration::from_secs(3);
    while k2_daemon::access_live::open_passes() != 0 {
        assert!(Instant::now() < deadline, "no hub, so nothing holds this pass: {}", k2_daemon::access_live::open_passes());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    owner_ok(port, "/cli/skin/roles/unassign", json!({ "username": "huboffguest" })).await;
    let early = watch(&mut s, 1_500).await;
    assert!(!early.ended, "no hub, so nothing this soon: {early:?}");
    let seen = watch(&mut s, CLOSE_WITHIN_MS).await;
    assert_eq!(seen.close, Some(4013), "{seen:?}");
}

/// Security review A1, on the tick and frame path, with no hub to help. A guest's
/// files socket sits on a room; the room goes while the role still names it; the same
/// folder comes back under a new id. That folder's changes must not reach this pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_hub_off_a_removed_rooms_socket_closes_and_a_new_room_in_its_folder_stays_dark() {
    let env = setup(&[("K2_ACCESS_HUB", "off")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "hubretire");
    let (_, pass) = guest_in(port, &ws, "hubretireguest", ALL_CAPS).await;
    let quiet = seed_ws(&env, "hubretirequiet");
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": "role-hubretireguest", "handle": quiet.handle, "caps": ALL_CAPS })).await;
    let mut idle = open(port, &format!("/cli/fs/events?workspace={}&token={}", quiet.handle, enc(&pass))).await;
    let mut busy = open(port, &format!("/cli/fs/events?workspace={}&token={}", ws.handle, enc(&pass))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    file_changed(&ws, "before.txt");
    let b = watch(&mut busy, 1_000).await;
    assert!(b.has("before.txt"), "rig: {b:?}");

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::delete(&conn, &ws.id).expect("delete");
        k2_core::db::schema::Project::delete(&conn, &quiet.id).expect("delete");
    }
    let again = seed_ws(&env, "hubretire");
    assert_ne!(again.id, ws.id, "rig: a new id");
    assert_eq!(again.path, ws.path, "rig: the same folder");

    // The next frame is checked first: no file name, and the close.
    file_changed(&again, "after-secret.txt");
    let seen = watch(&mut busy, CLOSE_WITHIN_MS).await;
    assert!(!seen.has("after-secret.txt"), "a new room's file name reached a pass with no rights there: {seen:?}");
    assert_eq!(seen.close, Some(4013), "{seen:?}");
    // A socket with nothing to send, on another removed room: the tick closes it.
    let seen = watch(&mut idle, CLOSE_WITHIN_MS).await;
    assert_eq!(seen.close, Some(4013), "{seen:?}");
}

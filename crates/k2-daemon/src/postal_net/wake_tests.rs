//! The loop's wake: after a failed pass the loop waits out its backoff (a
//! minute, then longer); a person's `postal claim` while the network is
//! down makes it try again at once, so a plane that was fixed meanwhile
//! connects on that claim, not up to 15 minutes later. The claim's answer
//! says when the last try ran and that it is retrying now. An agent's ask
//! never wakes it, and people wake it at most once per [`WAKE_EVERY`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::postal::sdk::plane::Request;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::postal_net::plane::{Answer, Plane, Transport};

/// A plane that answers like a broken host (no `Postal-Api: 1`) until it
/// is fixed, then like an empty Postal plane v1.
#[derive(Default)]
struct Mendable {
    fixed: AtomicBool,
    calls: AtomicUsize,
}

impl Transport for Arc<Mendable> {
    fn call(&self, _bearer: &str, req: &Request) -> Result<Answer, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let path = req.path.split('?').next().unwrap_or("").to_string();
        if !self.fixed.load(Ordering::SeqCst) {
            return Err(format!("postal plane {path}: answered 404 without Postal-Api: 1 (not a Postal plane v1)"));
        }
        let out = if path.ends_with("/session") {
            json!({ "token": "pst_0123456789abcdef0123456789abcdef", "label": "initech", "expires_at": "x" })
        } else if path.ends_with("/me") {
            json!({})
        } else if path.ends_with("/pairs") {
            json!({ "pairs": [] })
        } else if path.ends_with("/hold") {
            json!({ "items": [], "next_after": null })
        } else if path.ends_with("/events") {
            // A long poll's stand-in, so a connected loop doesn't spin.
            std::thread::sleep(Duration::from_millis(50));
            json!({ "cursor": "c1", "events": [] })
        } else {
            return Ok((404, br#"{"error":{"code":"not_found","message":""}}"#.to_vec()));
        };
        Ok((200, out.to_string().into_bytes()))
    }

    fn post_open(&self, _path: &str, _body: &Value) -> Result<Answer, String> {
        Err("not in this test".into())
    }
}

/// Wait up to `limit` for `f`; how long it took, or `None` if it never held.
fn within(limit: Duration, f: impl Fn() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if f() {
            return Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// A workspace: its id and path.
fn seed_ws() -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("w{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let path = format!("/tmp/k2-postal-{handle}");
    let db = k2_core::db::shared();
    db.lock()
        .execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("seed project");
    (id, path)
}

fn call(path: &str, who: Who, body: Value, q: &[(&str, &str)]) -> (u16, Value) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let post = !body.is_null();
    let b = if post { body.to_string().into_bytes() } else { Vec::new() };
    let r = crate::postal_routes::handle(path, who, post, &b, &params);
    let code: u16 = r.status.split_whitespace().next().and_then(|s| s.parse().ok()).expect("status");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

/// The loop's bell and the network mark as a fresh process has them, now
/// and when the test ends (pass or fail): under `cargo test` every test
/// shares one process, and a ring left unheard or a "connected" left set
/// leaks into the next Postal test. Taken after a `TempHome` (its env lock
/// keeps other Postal tests, which ring on a claim, out meanwhile).
struct Fresh;

impl Fresh {
    fn new() -> Fresh {
        Fresh.reset();
        Fresh
    }
    fn reset(&self) {
        *WAKE.lock().unwrap_or_else(|p| p.into_inner()) = false;
        *LAST_WAKE.lock().unwrap_or_else(|p| p.into_inner()) = None;
        k2_core::postal::gate::set_network_available(false);
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        self.reset();
    }
}

#[test]
fn a_fixed_plane_connects_on_the_next_claim_without_waiting_out_the_backoff() {
    let _home = crate::test_support::TempHome::new();
    let _fresh = Fresh::new();
    k2_core::db::init_for_tests();
    k2_core::postal::keys::install(Arc::new(k2_core::postal::keys::MemorySecrets::default()));
    let _on = crate::postal_routes::switch_for_test(true);
    let plane = Arc::new(Mendable::default());
    install(Arc::new(Plane::new(Box::new(plane.clone()), Box::new(|| Ok(("k2c_test".to_string(), "initech".to_string()))))));
    let (ws, ws_path) = seed_ws();

    let stop = Arc::new(AtomicBool::new(false));
    let runner = {
        let stop = stop.clone();
        std::thread::spawn(move || run_until(&stop))
    };

    // The first pass fails; the loop now waits a minute before the next.
    within(Duration::from_secs(10), || last_problem().is_some()).expect("the first pass records the problem");
    let p = last_problem().expect("problem");
    assert_eq!(p.code, "postal_unreachable");
    let next = next_try().expect("the loop says when it tries next");
    assert!(next >= p.at + 59, "a minute's backoff: tried {} next {next}", p.at);
    assert!(!k2_core::postal::gate::network_available());

    // An agent asking for status doesn't wake it: the answer names the
    // next try, and no call reaches the plane.
    let calls = plane.calls.load(Ordering::SeqCst);
    let agent = Who::Agent { project_id: ws.clone(), session_id: "sess-wake".into() };
    let (s, v) = call(crate::postal_routes::STATUS, agent, Value::Null, &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["plane"]["retrying"], json!(false), "{v}");
    assert_eq!(v["plane"]["nextTryAt"], json!(next), "{v}");
    let note = v["plane"]["note"].as_str().expect("note");
    assert!(note.contains("(last tried ") && note.contains("; next try ") && !note.contains("retrying now"), "{note}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(plane.calls.load(Ordering::SeqCst), calls, "an agent's status never wakes the loop");

    // The plane is fixed. A person claims an address: the answer says when
    // the last try ran and that it is retrying now.
    plane.fixed.store(true, Ordering::SeqCst);
    // An address of its own: the test DB is shared by every test in the
    // process, so a fixed one can already be held by another workspace.
    let address = format!("w{}::initech.postal.bot", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let (s, v) = call(crate::postal_routes::CLAIM, Who::Owner, json!({ "workspace": ws_path, "address": address }), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["plane"]["retrying"], json!(true), "{v}");
    assert_eq!(v["plane"]["lastTriedAt"], json!(p.at), "{v}");
    assert_eq!(v["plane"]["nextTryAt"], Value::Null, "{v}");
    assert_eq!(v["plane"]["problem"], json!("postal_unreachable"), "{v}");
    let note = v["plane"]["note"].as_str().expect("note");
    let when = crate::overlay_routes::card_time_words(p.at);
    assert!(note.contains("not a Postal plane v1") && note.ends_with(&format!("(last tried {when}; retrying now)")), "{note}");

    // ... and it connects now, well inside the minute's backoff.
    let took = within(Duration::from_secs(10), k2_core::postal::gate::network_available)
        .expect("the claim woke the loop: connected without waiting out the backoff");
    assert!(took < Duration::from_secs(10));
    assert_eq!(last_problem(), None);
    within(Duration::from_secs(10), || {
        k2_core::db::read(|c| k2_core::postal::store::handle_for_workspace(c, &ws)).is_some_and(|h| h.status == "claimed")
    })
    .expect("the woken pass registered the claimed address");

    stop.store(true, Ordering::SeqCst);
    ring();
    runner.join().expect("loop thread");
}

#[test]
fn people_wake_it_at_most_once_per_wake_every() {
    let t = Instant::now();
    assert!(wake_allowed(None, t));
    assert!(!wake_allowed(Some(t), t));
    assert!(!wake_allowed(Some(t), t + WAKE_EVERY - Duration::from_millis(1)));
    assert!(wake_allowed(Some(t), t + WAKE_EVERY));
}

#[test]
fn a_wake_ends_the_wait_and_an_unwoken_wait_runs_out() {
    let _home = crate::test_support::TempHome::new();
    let _fresh = Fresh::new();
    assert!(!pause(Duration::from_millis(50)), "nobody woke it");
    let waiter = std::thread::spawn(|| {
        let start = Instant::now();
        (pause(Duration::from_secs(600)), start.elapsed())
    });
    std::thread::sleep(Duration::from_millis(100));
    ring();
    let (woken, took) = waiter.join().expect("waiter");
    assert!(woken && took < Duration::from_secs(10), "woken {woken} after {took:?}");
}

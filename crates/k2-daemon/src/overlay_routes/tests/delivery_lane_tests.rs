//! Send-delay fix 2 (0.46.8): a Thread post answers once it is stored and
//! its frame is out; the agent inject runs after, on the agent's delivery
//! lane (`crate::delivery_queue`). Through the real handler
//! (`dispatch_post`). Each test arms its own address slow
//! ([`arm_test_delivery_delay`]), which sends its deliveries through the
//! real lane (other unit tests deliver inline).

use super::*;
use rusqlite::params;
use std::time::{Duration, Instant};

fn seed(label: &str) -> (String, String, String) {
    crate::vault::ensure_test_vault();
    let handle = format!("{label}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/ovl-lane-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    (id, handle, conv)
}

fn post(body: serde_json::Value) -> CliResponse {
    dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes())
}

/// Wait (bounded) until `addr` has `n` recorded deliveries; returns them.
fn wait_deliveries(addr: &str, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got = recorded_deliveries(addr);
        if got.len() >= n {
            return got;
        }
        assert!(Instant::now() < deadline, "{addr}: {} of {n} deliveries arrived: {got:?}", got.len());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait (bounded) until a workspace's lane has nothing running or queued.
fn wait_lane_idle(project_id: &str) {
    let lane = workspace_delivery_lane(project_id);
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::delivery_queue::busy(&lane) {
        assert!(Instant::now() < deadline, "lane {lane} never went idle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The answer does not wait for a slow agent; the line still reaches it.
#[test]
fn a_post_answers_before_its_slow_delivery_and_the_agent_still_gets_it() {
    let (id, handle, _) = seed("laneslow");
    let delay = Duration::from_millis(1500);
    arm_test_delivery_delay(&handle, delay);
    let text = format!("hello {}", uuid::Uuid::new_v4());

    let t0 = Instant::now();
    let r = post(serde_json::json!({ "addr": handle, "text": text, "via": "compose" }));
    let took = t0.elapsed();
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(took < delay / 2, "the answer waited for the delivery: {took:?} (delivery takes {delay:?})");
    assert!(
        recorded_deliveries(&handle).is_empty(),
        "nothing is delivered yet when the post answers: {:?}",
        recorded_deliveries(&handle)
    );
    let body: serde_json::Value = serde_json::from_str(&r.body).expect("answer JSON");
    assert_eq!(body["ok"], true, "{body}");
    assert!(body.get("delivered").is_none(), "a Thread answer carries no delivery outcome: {body}");

    let got = wait_deliveries(&handle, 1);
    assert_eq!(got, vec![format!("[thread:{handle}] {text}")], "delivered once, after the answer");
    wait_lane_idle(&id);
}

/// Two quick posts (and a third right behind them) reach the agent in the
/// order they were sent, though each delivery is slow.
#[test]
fn quick_posts_reach_the_agent_in_order() {
    let (id, handle, _) = seed("laneorder");
    arm_test_delivery_delay(&handle, Duration::from_millis(300));
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let texts: Vec<String> = (1..=3).map(|i| format!("msg {i} {tag}")).collect();
    let t0 = Instant::now();
    for t in &texts {
        let r = post(serde_json::json!({ "addr": handle, "text": t, "via": "compose" }));
        assert_eq!(r.status, "200 OK", "{}", r.body);
    }
    assert!(
        t0.elapsed() < Duration::from_millis(600),
        "three posts answered in {:?}: they waited for 3 × 300 ms of delivery",
        t0.elapsed()
    );
    let got = wait_deliveries(&handle, 3);
    let want: Vec<String> = texts.iter().map(|t| format!("[thread:{handle}] {t}")).collect();
    assert_eq!(got, want, "in the order sent");
    wait_lane_idle(&id);
}

/// post-idempotency-v1 still holds with the delivery after the answer: a
/// repeat with the same clientMsgId (sent while the first is still being
/// delivered) queues nothing, so the agent gets the line once.
#[test]
fn a_repeat_while_the_first_is_still_delivering_delivers_once() {
    let _keys = crate::client_msg_id::test_lock();
    let (id, handle, _) = seed("lanecmid");
    arm_test_delivery_delay(&handle, Duration::from_millis(400));
    let cmid = format!("cm_{}", uuid::Uuid::new_v4().simple());
    let text = format!("once {}", uuid::Uuid::new_v4());
    let body = serde_json::json!({ "addr": handle, "text": text, "via": "compose", "clientMsgId": cmid });

    let first = post(body.clone());
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let again = post(body);
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let again: serde_json::Value = serde_json::from_str(&again.body).expect("answer JSON");
    assert_eq!(again["duplicate"], true, "{again}");

    wait_deliveries(&handle, 1);
    wait_lane_idle(&id);
    let got = recorded_deliveries(&handle);
    assert_eq!(got, vec![format!("[thread:{handle}] {text}")], "the agent gets the line once");
}

/// One lane per agent: a slow delivery to one never delays another's.
#[test]
fn the_lane_is_per_agent_a_slow_one_never_holds_another() {
    let (slow_id, slow, _) = seed("laneslowa");
    let (fast_id, fast, _) = seed("lanefastb");
    arm_test_delivery_delay(&slow, Duration::from_millis(1500));
    arm_test_delivery_delay(&fast, Duration::from_millis(10));
    let r = post(serde_json::json!({ "addr": slow, "text": "slow one", "via": "compose" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let t0 = Instant::now();
    let r = post(serde_json::json!({ "addr": fast, "text": "fast one", "via": "compose" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    wait_deliveries(&fast, 1);
    assert!(
        t0.elapsed() < Duration::from_millis(1000),
        "another agent's slow delivery held this one: {:?}",
        t0.elapsed()
    );
    assert!(recorded_deliveries(&slow).is_empty(), "the slow agent's line is still on its way");
    wait_deliveries(&slow, 1);
    wait_lane_idle(&slow_id);
    wait_lane_idle(&fast_id);
}

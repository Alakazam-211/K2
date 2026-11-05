//! 0.46.9: Thread delivery visibility.
//!
//! - Every queued Thread delivery is in the delivery journal (migration
//!   0267) until its lane is done with it; boot queues again what never
//!   started, in order, and tells the Thread about a line cut off
//!   mid-delivery or too old, never typing it in twice.
//! - One `[thread-delivery]` log line per delivery.
//! - The take check's retry and its notices.
//!
//! Each test arms its own address slow ([`arm_test_delivery_delay`]), so
//! its deliveries take the real lane (other unit tests deliver inline).

use super::*;
use k2_core::delivery_journal::{self as store, JournalRow, KIND_THREAD};
use rusqlite::params;
use std::time::{Duration, Instant};

fn seed(label: &str) -> (String, String, String) {
    crate::vault::ensure_test_vault();
    let handle = format!("{label}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/ovl-journal-{handle}-{id}");
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

fn rows_on(lane: &str) -> Vec<JournalRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::pending(&conn).expect("journal").into_iter().filter(|r| r.lane == lane).collect()
}

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

fn wait_lane_idle(lane: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::delivery_queue::busy(lane) {
        assert!(Instant::now() < deadline, "lane {lane} never went idle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A journal row for a Thread line to `handle`, as a post would write it.
fn journal_row(handle: &str, text: &str, queued_at: i64, started_at: Option<i64>) -> JournalRow {
    let resolved = resolve_addr(handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let job = ThreadDeliveryJob::new(&resolved, format!("[thread:{handle}] {text}"), "owner", "compose", "", None);
    let row = JournalRow {
        id: uuid::Uuid::new_v4().to_string(),
        lane: job.lane.clone(),
        kind: KIND_THREAD.to_string(),
        body: serde_json::to_string(&job).expect("job JSON"),
        queued_at,
        started_at,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record(&conn, &row.id, &row.lane, &row.kind, &row.body, row.queued_at).expect("record");
    if let Some(at) = started_at {
        assert!(store::mark_started(&conn, &row.id, at).expect("start"));
    }
    row
}

fn thread_bodies(conv: &str) -> Vec<String> {
    k2_core::overlay::read_thread(conv, 0)
        .expect("thread")
        .into_iter()
        .filter_map(|i| i.doc.body)
        .collect()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A post's delivery is in the journal while it waits, and gone once its
/// lane is done with it.
#[test]
fn a_queued_post_is_journaled_until_its_lane_is_done() {
    let (id, handle, _) = seed("jrnl");
    arm_test_delivery_delay(&handle, Duration::from_millis(1200));
    let lane = workspace_delivery_lane(&id);
    let text = format!("journal me {}", uuid::Uuid::new_v4());

    let r = post(serde_json::json!({ "addr": handle, "text": text, "via": "compose" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let rows = rows_on(&lane);
    assert_eq!(rows.len(), 1, "one journal row while it waits: {rows:?}");
    assert_eq!(rows[0].kind, KIND_THREAD);
    let job: serde_json::Value = serde_json::from_str(&rows[0].body).expect("body JSON");
    assert_eq!(job["payload"], format!("[thread:{handle}] {text}"), "{job}");
    assert_eq!(job["via"], "compose");
    assert_eq!(job["retry"], false);
    // The lane picks it up (and marks it) while the slow delivery runs.
    std::thread::sleep(Duration::from_millis(400));
    let rows = rows_on(&lane);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].started_at.is_some(), "started rows are marked: {rows:?}");

    wait_deliveries(&handle, 1);
    wait_lane_idle(&lane);
    assert!(rows_on(&lane).is_empty(), "a finished delivery leaves no row: {:?}", rows_on(&lane));
}

/// Boot: rows that never started go back on their lane, in the order they
/// were queued, and each reaches the agent once.
#[test]
fn boot_sends_unstarted_rows_again_in_order() {
    let (id, handle, _) = seed("jrnlboot");
    arm_test_delivery_delay(&handle, Duration::from_millis(100));
    let lane = workspace_delivery_lane(&id);
    let t = now();
    let first = journal_row(&handle, "first", t - 3_000, None);
    let second = journal_row(&handle, "second", t - 2_000, None);

    let sum = crate::delivery_journal::replay(vec![first, second], t);
    assert_eq!(sum.requeued, 2, "{sum:?}");
    let got = wait_deliveries(&handle, 2);
    assert_eq!(got, vec![format!("[thread:{handle}] first"), format!("[thread:{handle}] second")], "in queue order");
    wait_lane_idle(&lane);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(recorded_deliveries(&handle).len(), 2, "each line once");
    assert!(rows_on(&lane).is_empty(), "replayed rows finish like any other: {:?}", rows_on(&lane));
}

/// Boot: a row cut off mid-delivery is never typed in again; the Thread
/// says it may not have reached the agent.
#[test]
fn boot_never_retypes_a_row_cut_off_mid_delivery() {
    let (id, handle, conv) = seed("jrnlcut");
    arm_test_delivery_delay(&handle, Duration::from_millis(50));
    let lane = workspace_delivery_lane(&id);
    let t = now();
    let row = journal_row(&handle, "deploy the site", t - 5_000, Some(t - 4_000));

    let sum = crate::delivery_journal::replay(vec![row], t);
    assert_eq!(sum.interrupted, 1, "{sum:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(recorded_deliveries(&handle).is_empty(), "never typed in twice: {:?}", recorded_deliveries(&handle));
    assert!(rows_on(&lane).is_empty(), "its row is gone");
    let bodies = thread_bodies(&conv);
    assert!(
        bodies.iter().any(|b| b.contains("K2 itself restarted") && b.contains("deploy the site")),
        "the Thread is told: {bodies:?}"
    );
}

/// Boot: a row over an hour old is not sent; the Thread says so.
#[test]
fn boot_does_not_send_a_row_over_an_hour_old() {
    let (id, handle, conv) = seed("jrnlold");
    arm_test_delivery_delay(&handle, Duration::from_millis(50));
    let lane = workspace_delivery_lane(&id);
    let t = now();
    let row = journal_row(&handle, "yesterday's ask", t - crate::delivery_journal::REPLAY_MAX_AGE_MS - 1_000, None);

    let sum = crate::delivery_journal::replay(vec![row], t);
    assert_eq!(sum.too_old, 1, "{sum:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(recorded_deliveries(&handle).is_empty());
    assert!(rows_on(&lane).is_empty());
    let bodies = thread_bodies(&conv);
    assert!(bodies.iter().any(|b| b.contains("over an hour old")), "the Thread is told: {bodies:?}");
}

/// A job written to the journal reads back the same, as a live delivery
/// (a replay never runs on the test sink by accident in production).
#[test]
fn a_journaled_job_reads_back_whole() {
    let (_, handle, _) = seed("jrnlserde");
    let resolved = resolve_addr(&handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let turn = ThreadTurn::new(&resolved, "doc-1", 1_234);
    let job = ThreadDeliveryJob::new(&resolved, format!("[thread:{handle}] hi"), "Ann", "compose", "/loop", Some(turn)).from_guest();
    let back: ThreadDeliveryJob = serde_json::from_str(&serde_json::to_string(&job).expect("JSON")).expect("read back");
    assert_eq!(back.addr, job.addr);
    assert_eq!(back.payload, job.payload);
    assert_eq!(back.command, "/loop");
    assert!(back.guest);
    assert_eq!(back.turn.as_ref().map(|t| (t.turn_id.as_str(), t.started_at)), Some(("doc-1", 1_234)));
    assert_eq!(back.sink, DeliverySink::Live, "the sink is never stored");
    assert!(!back.retry);

    let retry = back.into_retry();
    assert!(retry.retry, "the retry is marked");
    assert!(retry.turn.is_none(), "the retry starts no Thread turn of its own");
}

/// One log line per delivery, with where it went and how.
#[test]
fn the_delivery_log_line_names_session_path_and_attempts() {
    let ok = workspace_msg::MsgResponse {
        success: true,
        target_session_id: Some("sess-1".into()),
        attempts: 2,
        reason: None,
        hint: None,
        woke: true,
        wake_ms: Some(900),
        branch: Some("send_message".into()),
        addr: None,
        moved_from: None,
    };
    assert_eq!(
        thread_delivery_log_line("sales", "compose", &ok, 1234, false),
        "[thread-delivery] delivered addr=sales via=compose session=sess-1 path=send_message attempts=2 woke=true ms=1234 retry=false"
    );
    let failed = workspace_msg::MsgResponse {
        success: false,
        target_session_id: None,
        attempts: 3,
        reason: Some("pty_stalled".into()),
        hint: None,
        woke: false,
        wake_ms: None,
        branch: None,
        addr: None,
        moved_from: None,
    };
    assert_eq!(
        thread_delivery_log_line("sales/rev", "thread", &failed, 8000, true),
        "[thread-delivery] not delivered reason=pty_stalled addr=sales/rev via=thread session=- path=- attempts=3 woke=false ms=8000 retry=true"
    );
}

/// The notice quotes the sender's words (short) and says what happened.
#[test]
fn not_taken_notices_quote_the_message_and_say_what_to_do() {
    assert_eq!(notice_quote("[thread:sales] run the tests"), "run the tests");
    assert_eq!(notice_quote("[thread:sales] > a guest\n> line two"), "a guest");
    let long = format!("[thread:s] {}", "x".repeat(100));
    assert_eq!(notice_quote(&long), format!("{}...", "x".repeat(60)));

    let lost = not_taken_text("sales", "run the tests", NotTaken::NeverReadIn);
    assert!(lost.starts_with("K2: sales didn't take the message \"run the tests\"."), "{lost}");
    assert!(lost.contains("twice") && lost.contains("send the message again"), "{lost}");
    let failed = not_taken_text("sales", "", NotTaken::RetryFailed);
    assert!(failed.contains("didn't take a message") && failed.contains("couldn't type it in again"), "{failed}");
    for why in [NotTaken::NeverReadIn, NotTaken::RetryFailed, NotTaken::RestartMidDelivery, NotTaken::RestartTooOld] {
        let t = not_taken_text("sales", "hi", why);
        assert!(!t.contains('\u{2014}'), "no em dashes: {t}");
    }
}

/// A retry that can't be typed in tells the Thread at once.
#[test]
fn a_retry_that_fails_tells_the_thread() {
    let (_, handle, conv) = seed("jrnlretry");
    let resolved = resolve_addr(&handle).unwrap_or_else(|e| panic!("resolve: {}", e.body));
    let mut job = ThreadDeliveryJob::new(&resolved, format!("[thread:{handle}] ping"), "owner", "compose", "", None).into_retry();
    // The live path's outcome check, with a failed retry.
    job.sink = DeliverySink::Live;
    job.check_taken(&ThreadDelivery::default(), now());
    let bodies = thread_bodies(&conv);
    assert!(
        bodies.iter().any(|b| b.contains("couldn't type it in again") && b.contains("ping")),
        "the Thread is told: {bodies:?}"
    );
}

//! `compute_ws`'s unit tests, in their own file so compute_ws.rs stays under the
//! ratchet's 4000 lines (moved unchanged).

use super::*;
use k2_core::compute::proto::frames::{JobLimits, Plan};

pub(super) fn job_on(conn: &rusqlite::Connection, id: &str, node: &str) -> store::JobRow {
    let plan = Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: id.into(),
        node_id: node.into(),
        workspace_id: "ws".into(),
        workspace_label: "ws".into(),
        requested_by: "owner".into(),
        argv: vec!["true".into()],
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
        src: None,
        exclusive: false,
        created_at: 10,
        needs: Vec::new(),
        artifacts: Vec::new(),
        stall_secs: None,
        secret_env: Vec::new(),
        tier: None,
        cache: None,
        src_path: None, submitter: None, submitter_max_running: None, packages: Vec::new(), git_access: Vec::new(), receipt_v: None, gpu: None, gpu_mem: None, cache_commit: None, cache_ref: None, setup: Vec::new(), cpus_burst: None, provenance: None,
        network: None,
        with: Vec::new(),
    };
    store::insert_job(
        conn,
        &store::NewJob { plan: &plan, client_job_id: id, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: Default::default() },
    )
    .expect("insert job")
}

/// §6: a shared node gets this server's jobs in its own order:
/// workspaces take turns (after the last served), FIFO inside each.
#[test]
fn shared_nodes_get_jobs_in_workspace_rotation() {
    let mk = |id: &str, ws: &str, at: i64| store::JobRow { id: id.into(), workspace_id: ws.into(), created_at: compute::now() - 100 + at, ..Default::default() };
    let jobs = vec![mk("a1", "a", 1), mk("a2", "a", 2), mk("b1", "b", 3), mk("c1", "c", 4)];
    let ids = |v: Vec<store::JobRow>| v.into_iter().map(|j| j.id).collect::<Vec<_>>();
    assert_eq!(ids(rotation_order(jobs.clone(), &scheduler::Turns::default())), ["a1", "b1", "c1", "a2"]);
    assert_eq!(ids(rotation_order(jobs, &scheduler::Turns::ws(Some("a")))), ["b1", "c1", "a1", "a2"]);
}

/// Job tiers T2: the send order into a shared node is tier-first: CI,
/// then Priority, each oldest first, then the workspace rotation. A
/// share's room (`max_running + max_queued`) is cut from the front of
/// this order, so a guest with two Normal jobs waiting and room for two
/// sends a new CI job first instead of leaving it `tenant_queue_full`.
#[test]
fn the_send_order_into_a_shared_node_is_tier_first() {
    let mk = |id: &str, ws: &str, at: i64, tier: &str| store::JobRow {
        id: id.into(),
        workspace_id: ws.into(),
        created_at: compute::now() - 100 + at,
        tier: tier.into(),
        ..Default::default()
    };
    let ids = |v: Vec<store::JobRow>| v.into_iter().map(|j| j.id).collect::<Vec<_>>();
    let jobs = vec![mk("n1", "a", 1, "normal"), mk("n2", "b", 2, "normal"), mk("p1", "a", 3, "priority"), mk("c1", "b", 4, "ci"), mk("c0", "a", 5, "ci")];
    let order = ids(rotation_order(jobs, &scheduler::Turns::ws(Some("a"))));
    assert_eq!(order, ["c1", "c0", "p1", "n2", "n1"], "Normal still takes turns after the last-served workspace");
    assert_eq!(&order[..2], ["c1", "c0"], "room for two: both CI jobs go first");
}

/// r8 (2026-10-22): a job its requester cancelled sends no `[compute]`
/// completion message (they just did it, and wait on nothing); a job
/// someone else cancelled still tells its requester.
#[test]
fn a_job_its_requester_cancelled_sends_no_completion_message() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let mk = |id: &str, sid: &str, by_requester: bool| {
        let conn = db.lock();
        let j = job_on(&conn, id, "n1");
        conn.execute("UPDATE compute_jobs SET session_id = ?2, detach = 1 WHERE id = ?1", [&j.id, &sid.to_string()]).expect("session");
        if by_requester {
            let ctl = store::JobCtl { cancelled_by_requester: true, ..Default::default() };
            store::set_job_ctl(&conn, &j.id, &ctl).expect("ctl");
        }
        let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("cancelled"), None, 30);
        j.id
    };
    {
        let conn = db.lock();
        store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
    }
    let (mine, theirs) = ("sess-requester-cancel", "sess-owner-cancel");
    test_sink_register(mine);
    test_sink_register(theirs);
    let own = mk("eeeeeeee0123456789abcdef01234567", mine, true);
    let other = mk("ffffffff0123456789abcdef01234567", theirs, false);
    finalize(&own);
    finalize(&other);
    let t0 = std::time::Instant::now();
    while test_sink_peek(theirs).is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(10), "the requester of a job someone else cancelled is told");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(test_sink_take(theirs)[0].contains("cancelled"));
    assert_eq!(test_sink_take(mine), Vec::<String>::new(), "no message for a job its requester cancelled");
}

/// k2n4, 2026-10-21: a running job whose cancel was asked for, that
/// then comes back `interrupted/node_restart` (a full disk lost its
/// end; the node was restarted), ends `cancelled` and is never sent
/// again, even with retries and `retry_interrupted` on. Without a
/// pending cancel the same interruption is retried as before.
#[test]
fn a_cancel_requested_job_that_comes_back_interrupted_ends_cancelled() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let mk = |id: &str, cancel: bool| {
        let conn = db.lock();
        let j = job_on(&conn, id, "n1");
        assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
        let ctl = store::JobCtl { retries: 2, ..Default::default() };
        store::set_job_ctl(&conn, &j.id, &ctl).expect("ctl");
        conn.execute("UPDATE compute_jobs SET retry_interrupted = 1 WHERE id = ?1", [&j.id]).expect("retry_interrupted");
        if cancel {
            store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
        }
        store::job_by_id(&conn, &j.id).expect("row")
    };
    {
        let conn = db.lock();
        store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
    }
    let job = mk("cccccccc0123456789abcdef01234567", true);
    apply_observation("n1", &job.id, job.generation as u32, JobState::Running, None, None, None, None, None, NodeTimes::default());
    let row = store::job_by_id(&db.lock(), &job.id).expect("row");
    assert_eq!((row.state.as_str(), row.reason.as_deref()), ("running", Some("cancel_requested")), "the cancel stays owed while it runs");
    apply_observation("n1", &job.id, job.generation as u32, JobState::Interrupted, Some("node_restart"), None, None, None, None, NodeTimes::default());
    let row = store::job_by_id(&db.lock(), &job.id).expect("row");
    assert_eq!((row.state.as_str(), row.reason.as_deref()), ("cancelled", Some("cancelled")), "{row:?}");
    assert_eq!(row.generation, job.generation, "not re-dispatched");
    assert!(row.detail.as_deref().is_some_and(|d| d.contains("interrupted/node_restart")), "{:?}", row.detail);
    assert!(row.ended_at.is_some());
    // The control: no cancel owed, the interruption is retried.
    let other = mk("dddddddd0123456789abcdef01234567", false);
    apply_observation("n1", &other.id, other.generation as u32, JobState::Interrupted, Some("node_restart"), None, None, None, None, NodeTimes::default());
    let row = store::job_by_id(&db.lock(), &other.id).expect("row");
    assert_eq!(row.state.as_str(), "queued", "retried: {row:?}");
    assert!(row.attempt > other.attempt, "a new attempt: {row:?}");
}

/// CE-10: the warning goes out 5 min before the limit (half of a
/// short one), once, in words that teach the flag.
#[test]
fn stall_warning_timing_and_words() {
    assert_eq!(stall_warn_after(1800), 1500);
    assert_eq!(stall_warn_after(240), 120);
    assert_eq!(
        stall_warning_text("ab12cd34ffff", 1560, 1800, false),
        "[compute] job ab12cd34 has printed nothing for about 26m00s; it ends at 30m00s of silence. A quiet job that is still working: pass --stall-timeout off (or a longer one) next time. k2 compute get ab12cd34"
    );
    assert!(stall_warning_text("ab12cd34ffff", 1560, 1800, true).contains("unless it is using CPU"));
}

/// C11: a job whose cancel was asked for while assigned, and that the
/// node then refused, ends `cancelled` instead of going back to the
/// queue (where it would run later).
#[test]
fn a_cancel_requested_job_the_node_refuses_ends_cancelled() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let job = {
        let conn = db.lock();
        store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
        let j = job_on(&conn, "aaaaaaaa0123456789abcdef01234567", "n1");
        assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
        store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
        j
    };
    apply_observation("n1", &job.id, job.generation as u32, JobState::Queued, Some("parallel_full"), None, None, None, None, NodeTimes::default());
    let row = store::job_by_id(&db.lock(), &job.id).expect("row");
    assert_eq!((row.state.as_str(), row.reason.as_deref()), ("cancelled", Some("cancelled")));
    assert!(row.ended_at.is_some());
    // Without a pending cancel, a refusal re-queues as before.
    let other = {
        let conn = db.lock();
        let j = job_on(&conn, "bbbbbbbb0123456789abcdef01234567", "n1");
        assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
        j
    };
    apply_observation("n1", &other.id, other.generation as u32, JobState::Queued, Some("parallel_full"), None, None, None, None, NodeTimes::default());
    let row = store::job_by_id(&db.lock(), &other.id).expect("row");
    assert_eq!((row.state.as_str(), row.reason.as_deref()), ("queued", Some("parallel_full")));
}

/// G1 (plan-ci-release-on-k2-compute-v1): the release nodes reach
/// z3mbpZ only through the relay and the Linux AppImage is 105 MB.
/// The relay keeps its 100 MB default; `over_relay` lifts it to the
/// node's own 500 MB collection cap, never past it. LAN, loopback and
/// unknown routes were never relay-capped.
#[test]
fn relay_pulls_keep_the_100_mb_cap_unless_the_caller_says_over_relay() {
    let mb = 1u64 << 20;
    let appimage = 105 * mb;
    let (code, hint) = pull_cap_refusal(Some("relay"), false, appimage).expect("105 MB over the relay is refused by default");
    assert_eq!(code, "relay_cap");
    assert!(hint.contains("--over-relay") && hint.contains("105 MB") && hint.contains("100 MB"), "{hint}");
    assert_eq!(pull_cap_refusal(Some("relay"), false, 100 * mb), None, "exactly the cap passes");
    assert!(pull_cap_refusal(Some("relay"), false, 100 * mb + 1).is_some());
    assert_eq!(pull_cap_refusal(Some("relay"), true, appimage), None, "--over-relay lets the AppImage through");
    assert_eq!(pull_cap_refusal(Some("relay"), true, 500 * mb), None);
    let (code, hint) = pull_cap_refusal(Some("relay"), true, 500 * mb + 1).expect("never past the LAN cap");
    assert_eq!(code, "relay_cap");
    assert!(!hint.contains("--over-relay"), "the flag can't help here: {hint}");
    for route in [Some("lan"), Some("loopback"), None] {
        for over in [false, true] {
            assert_eq!(pull_cap_refusal(route, over, appimage), None, "{route:?}");
            assert_eq!(pull_cap_refusal(route, over, 500 * mb), None, "{route:?}");
            assert!(pull_cap_refusal(route, over, 500 * mb + 1).is_some(), "{route:?}");
        }
    }
    assert_eq!(artifact_pull_cap(Some("relay"), false), results::RELAY_MAX_FILE_BYTES);
    assert_eq!(artifact_pull_cap(Some("relay"), true), results::MAX_FILE_BYTES);
}

/// A background pull (`wait=`) hands its error to the next ask once,
/// keeps nothing behind, and a later ask starts a new pull.
#[test]
fn a_background_pull_reports_its_error_once_and_leaves_nothing_behind() {
    // The cache path comes from $HOME: hold it still (and off the real ~/.k2).
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        job_on(&conn, "abababab0123456789abcdef01234567", "n-gone")
    };
    let info = ArtifactInfo { name: "dist/k2.AppImage".into(), sha256: "c".repeat(64), bytes: 105 << 20 };
    let key = artifact_cache_path(&j.id, j.generation, &info.sha256);
    for round in 0..2 {
        let r = fetch_artifact_wait(&j, &info, true, Duration::from_secs(5), Duration::from_secs(10));
        match r {
            Err((code, _)) => assert_eq!(code, "node_offline", "round {round}"),
            Ok(other) => panic!("round {round}: an offline node can't send anything: {other:?}"),
        }
        assert!(with_bg(|m| !m.contains_key(&key)), "round {round}: the finished pull is gone once reported");
        assert!(with_pulls(|m| m.values().all(|p| p.dst != key)), "round {round}: no transfer left open");
    }
}

// ── GH#80: artifacts from a relay node ──────────────────────────

/// A node offer with `features` (the rest is a plain idle Linux node).
fn offer_with(features: &[&str]) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: frames::Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 8000, mem_bytes: 8 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: frames::Free { cpu_millis: 8000, mem_bytes: 8 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 1,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: BTreeMap::new(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.8".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

/// A live session for `node_id` on the relay without a socket (no
/// offer handler, no placement): what this server sends comes out of
/// the receiver.
fn relay_conn(node_id: &str, features: &[&str]) -> mpsc::UnboundedReceiver<Out> {
    let (tx, rx) = mpsc::unbounded_channel::<Out>();
    with_engine(|e| {
        e.next_conn += 1;
        let conn_id = e.next_conn;
        e.conns.insert(
            node_id.to_string(),
            Conn {
                conn_id,
                tx,
                route: "relay",
                offer: Some(offer_with(features)),
                statement: None,
                lineages: None,
                bytes_in: Arc::new(AtomicU64::new(0)),
                bytes_out: Arc::new(AtomicU64::new(0)),
            },
        );
    });
    rx
}

/// The next frame this server sends the fake node that `pick` takes.
fn next_sent<T>(rx: &mut mpsc::UnboundedReceiver<Out>, mut pick: impl FnMut(&Frame) -> Option<T>) -> T {
    let t0 = Instant::now();
    loop {
        match rx.try_recv() {
            Ok(Out::Frame(f)) => {
                if let Some(t) = pick(&f) {
                    return t;
                }
            }
            Ok(Out::Close) => panic!("the session was closed"),
            Err(_) => {
                assert!(t0.elapsed() < Duration::from_secs(10), "no such frame within 10 s");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn chunk(request_id: &str, seq: u32, data: &[u8], last: bool, info: &ArtifactInfo) -> Frame {
    Frame::ArtifactChunk(ArtifactChunk { request_id: request_id.into(), seq, data: crypto::b64(data), last, total_bytes: info.bytes, sha256: info.sha256.clone() })
}

/// GH#80: over a slow relay a file arrives slower than its fixed
/// deadline allowed (60 s + 1 s per MB assumed about 1 MB/s). A pull
/// that keeps getting bytes is not cut off; only one that stops
/// moving for its whole timeout fails.
///
/// The artifact tests hold a temp home (the cache path comes from
/// $HOME, which other tests change) and run their pulls on a manual
/// clock, so a busy test machine can't make a pull time out.
#[test]
fn a_slow_pull_that_keeps_moving_is_not_cut_off() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let j = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        job_on(&conn, "slowslow0123456789abcdef01234567", "n-slow")
    };
    let body: Vec<u8> = (0..6 * 1024u32).map(|i| (i % 251) as u8).collect();
    let info = ArtifactInfo { name: "dist/app.tar.gz".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
    let _ = std::fs::remove_dir_all(artifact_cache_path(&j.id, j.generation, &info.sha256).parent().unwrap());
    let mut rx = relay_conn("n-slow", &[results::FEATURE_ARTIFACTS]);
    let clock = PullClock::manual();
    let (job, inf, c) = (j.clone(), info.clone(), clock.clone());
    let pull = std::thread::spawn(move || fetch_artifact_on(&job, &inf, false, Duration::from_secs(2), c));
    let get = next_sent(&mut rx, |f| match f {
        Frame::ArtifactGet(g) => Some(g.clone()),
        _ => None,
    });
    // Six pieces, one every 1.9 s: 11.4 s in all, over five times the
    // timeout, but never 2 s without a byte.
    let t0 = clock.now();
    for (i, piece) in body.chunks(1024).enumerate() {
        clock.advance(Duration::from_millis(1900));
        assert!(!pull.is_finished(), "piece {i}: the pull is still waiting for bytes");
        test_frame("n-slow", chunk(&get.request_id, i as u32, piece, i == 5, &info));
    }
    let r = pull.join().expect("the pull thread");
    assert!(clock.now() - t0 > Duration::from_secs(2) * 5, "the transfer took far longer than the timeout");
    let path = r.unwrap_or_else(|(c, h)| panic!("a moving pull failed: {c}: {h}"));
    assert_eq!(std::fs::read(&path).expect("cached"), body);
    test_detach("n-slow");
}

/// GH#80: the server keeps reading a node's frames while its own
/// frames to the node wait (a node, or a relay hop, slow to read).
/// Before, one `select!` did both: a write that waited stopped every
/// read, the node's sends backed up behind it (rwnd-limited, its
/// artifact stuck at 0 bytes on the server).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_keeps_reading_while_its_writes_wait() {
    use tokio_tungstenite::tungstenite::protocol::Role;
    let (a, b) = tokio::io::duplex(64 * 1024);
    let server_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let node_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(b, Role::Client, None).await;
    let node_key = SigningKey::generate().unwrap();
    let session = "gh80-session".to_string();
    let keys = SessionKeys { key: SigningKey::generate().unwrap(), session: session.clone(), node_spki: node_key.spki_der().to_vec(), node_id: "n-reads-nothing".into() };
    // 16 big frames for the node, which never reads them: far more
    // than the pipe holds.
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    for seq in 0..16u32 {
        let t = Transfer { job_id: "j".into(), generation: 1, kind: TransferKind::Dirty, seq, data: "A".repeat(200 * 1024), last: false, total_bytes: 0, sha256: String::new() };
        tx.send(Out::Frame(Box::new(Frame::Transfer(t)))).unwrap();
    }
    let (work_tx, work_rx) = std::sync::mpsc::channel::<Frame>();
    let (bytes_in, bytes_out) = (AtomicU64::new(0), AtomicU64::new(0));
    let (mut sink, mut source) = server_ws.split();
    const SENT: u32 = 24;
    let (mut node_sink, _node_source) = node_ws.split();
    let node = async move {
        for seq in 0..SENT {
            let f = Frame::ArtifactChunk(ArtifactChunk {
                request_id: "r".into(),
                seq,
                data: crypto::b64(&vec![7u8; frames::TRANSFER_CHUNK]),
                last: false,
                total_bytes: 0,
                sha256: String::new(),
            });
            let text = frames::seal(&node_key, Direction::NodeToController, &session, u64::from(seq), &f).unwrap();
            node_sink.send(Message::Text(text)).await.expect("node send");
        }
        std::future::pending::<()>().await
    };
    let counted = tokio::task::spawn_blocking(move || {
        let mut n = 0;
        while n < SENT {
            match work_rx.recv_timeout(Duration::from_secs(15)) {
                Ok(Frame::ArtifactChunk(_)) => n += 1,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        n
    });
    let server = pump(&mut sink, &mut source, &mut rx, &work_tx, &keys, &bytes_in, &bytes_out, Duration::from_secs(3600));
    tokio::select! {
        why = server => panic!("the session ended early: {why}"),
        _ = node => unreachable!("the node never stops on its own"),
        n = counted => assert_eq!(n.unwrap(), SENT, "every frame the node sent was read while the server's own writes waited"),
    }
}

fn flow_job(id: &str, node: &str, body: &[u8]) -> (store::JobRow, ArtifactInfo) {
    let j = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        job_on(&conn, id, node)
    };
    let info = ArtifactInfo { name: "dist/app.tar.gz".into(), sha256: crypto::sha256_hex(body), bytes: body.len() as u64 };
    let _ = std::fs::remove_dir_all(artifact_cache_path(&j.id, j.generation, &info.sha256).parent().unwrap());
    (j, info)
}

/// GH#80, on a node with `artifact-flow`: the pull asks for a window,
/// acks every chunk it wrote, and when the node goes quiet it fails,
/// cancels the node's sending and keeps its bytes; the next fetch asks
/// from there and finishes the file.
#[test]
fn a_flow_pull_acks_cancels_when_it_stops_and_the_next_one_continues() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let body: Vec<u8> = (0..3 * 1024u32).map(|i| (i % 253) as u8).collect();
    let (j, info) = flow_job("flowflow0123456789abcdef01234567", "n-flow", &body);
    let mut rx = relay_conn("n-flow", &[results::FEATURE_ARTIFACTS, results::FEATURE_ARTIFACT_FLOW]);
    let clock = PullClock::manual();
    let (job, inf, c) = (j.clone(), info.clone(), clock.clone());
    let pull = std::thread::spawn(move || fetch_artifact_on(&job, &inf, false, Duration::from_secs(1), c));
    let get = next_sent(&mut rx, |f| match f {
        Frame::ArtifactGet(g) => Some(g.clone()),
        _ => None,
    });
    assert_eq!((get.offset, get.window), (None, Some(frames::ARTIFACT_WINDOW)), "a fresh pull, paced");
    test_frame("n-flow", chunk(&get.request_id, 0, &body[..1024], false, &info));
    let ack = next_sent(&mut rx, |f| match f {
        Frame::ArtifactAck(a) => Some(a.clone()),
        _ => None,
    });
    assert_eq!((ack.request_id.as_str(), ack.next), (get.request_id.as_str(), 1024));
    assert!(!pull.is_finished(), "a pull whose clock hasn't moved can't have timed out");
    // The node goes quiet for the whole limit: the pull fails, the node
    // is told to stop.
    clock.advance(Duration::from_secs(1));
    let (code, hint) = pull.join().unwrap().expect_err("a quiet node fails the pull");
    assert_eq!(code, "artifact_timeout", "{hint}");
    assert!(hint.contains("the next fetch continues"), "{hint}");
    let cancel = next_sent(&mut rx, |f| match f {
        Frame::ArtifactCancel(c) => Some(c.clone()),
        _ => None,
    });
    assert_eq!(cancel.request_id, get.request_id);
    let dst = artifact_cache_path(&j.id, j.generation, &info.sha256);
    assert_eq!(std::fs::read(part_path(&dst)).expect("the partial file stays"), body[..1024]);

    // The next fetch continues at byte 1024 and finishes the file.
    let (job, inf, c) = (j.clone(), info.clone(), PullClock::manual());
    let pull = std::thread::spawn(move || fetch_artifact_on(&job, &inf, false, Duration::from_secs(1), c));
    let get = next_sent(&mut rx, |f| match f {
        Frame::ArtifactGet(g) => Some(g.clone()),
        _ => None,
    });
    assert_eq!(get.offset, Some(1024), "continues where the last pull stopped");
    test_frame("n-flow", chunk(&get.request_id, 0, &body[1024..2048], false, &info));
    test_frame("n-flow", chunk(&get.request_id, 1, &body[2048..], true, &info));
    let path = pull.join().unwrap().unwrap_or_else(|(c, h)| panic!("{c}: {h}"));
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(!part_path(&dst).exists(), "the partial file became the cached one");
    let sent: Vec<Frame> = std::iter::from_fn(|| match rx.try_recv() {
        Ok(Out::Frame(f)) => Some(*f),
        _ => None,
    })
    .collect();
    assert!(sent.iter().all(|f| !matches!(f, Frame::ArtifactCancel(_))), "a finished pull cancels nothing: {sent:?}");
    test_detach("n-flow");
}

/// GH#80: a reconnect ends the pull at once (the old session's chunks
/// won't come on the new one) instead of waiting out its timeout; a
/// second fetch of a file being pulled joins that pull.
#[test]
fn a_reconnect_ends_the_pull_and_a_second_fetch_joins_a_running_one() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let body: Vec<u8> = (0..2048u32).map(|i| (i % 249) as u8).collect();
    let (j, info) = flow_job("dropdrop0123456789abcdef01234567", "n-drop", &body);
    let mut rx = relay_conn("n-drop", &[results::FEATURE_ARTIFACTS, results::FEATURE_ARTIFACT_FLOW]);
    // A clock that never moves: this pull can't time out, so it ends
    // only because its session did.
    let (job, inf, c) = (j.clone(), info.clone(), PullClock::manual());
    let pull = std::thread::spawn(move || fetch_artifact_on(&job, &inf, false, Duration::from_secs(30), c));
    let get = next_sent(&mut rx, |f| match f {
        Frame::ArtifactGet(g) => Some(g.clone()),
        _ => None,
    });
    test_frame("n-drop", chunk(&get.request_id, 0, &body[..1024], false, &info));
    // The node's session drops and it comes back on a new one.
    let mut rx2 = relay_conn("n-drop", &[results::FEATURE_ARTIFACTS, results::FEATURE_ARTIFACT_FLOW]);
    let (code, hint) = pull.join().unwrap().expect_err("the pull ends with its session");
    assert_eq!(code, "node_offline", "{hint}");
    assert!(hint.contains("the next fetch continues"), "{hint}");

    // Two fetches at once: one request to the node, both get the file.
    let spawn = || {
        let (job, inf, c) = (j.clone(), info.clone(), PullClock::manual());
        std::thread::spawn(move || fetch_artifact_on(&job, &inf, false, Duration::from_secs(10), c))
    };
    let first = spawn();
    let get = next_sent(&mut rx2, |f| match f {
        Frame::ArtifactGet(g) => Some(g.clone()),
        _ => None,
    });
    assert_eq!(get.offset, Some(1024));
    let second = spawn();
    std::thread::sleep(Duration::from_millis(300));
    test_frame("n-drop", chunk(&get.request_id, 0, &body[1024..], true, &info));
    for h in [first, second] {
        let p = h.join().unwrap().unwrap_or_else(|(c, h)| panic!("{c}: {h}"));
        assert_eq!(std::fs::read(p).unwrap(), body);
    }
    let more: Vec<Frame> = std::iter::from_fn(|| match rx2.try_recv() {
        Ok(Out::Frame(f)) => Some(*f),
        _ => None,
    })
    .collect();
    assert!(more.iter().all(|f| !matches!(f, Frame::ArtifactGet(_))), "the second fetch sent no request of its own: {more:?}");
    test_detach("n-drop");
    drop(rx);
}

/// 0.46.17: an auto Priority or CI job waits on this server until a node
/// can start it (never into a shared node's queue); Standard and Batch
/// only for the hold (V71); a pinned job, or one that starts now, never.
#[test]
fn priority_and_ci_hold_on_the_server_until_a_node_can_start_them() {
    for tier in [Tier::Priority, Tier::Ci] {
        let r = any_hold(tier, true, 86_400, 60, false).expect("held however long it waited");
        assert_eq!(r.code, "any_hold", "the mover counts it as only busy");
        assert!(r.message.contains("waits on this server until a node can start it now"), "{}", r.message);
        assert!(any_hold(tier, true, 86_400, 60, true).is_none(), "a node starts it now: it goes");
        assert!(any_hold(tier, false, 86_400, 60, false).is_none(), "pinned: into its node's queue");
    }
    assert!(any_hold(Tier::Ci, true, 0, 0, false).unwrap().message.starts_with("CI: "));
    for tier in [Tier::Normal, Tier::Batch] {
        assert_eq!(any_hold(tier, true, 30, 60, false).map(|r| r.message), Some("waiting up to 60s on this server for a node that can start it now".to_string()));
        assert!(any_hold(tier, true, 60, 60, false).is_none(), "after the hold: into the queue");
        assert!(any_hold(tier, true, 0, 0, false).is_none(), "no hold");
    }
}

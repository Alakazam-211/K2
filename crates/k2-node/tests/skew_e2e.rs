//! Clock skew is re-measured from the server's clock ping (0.46.15), not
//! only at attach. A test node, 2026-11-16: a flat battery reset its RTC, the
//! node attached on the wrong clock, NTP then stepped the clock 17 h, and
//! `k2-node status` / `k2 compute nodes` kept showing -61935 s until the
//! session reconnected.
#![cfg(unix)]

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::*;
use k2_node_proto::frames::{self, Frame};
use k2_node_proto::share::{QueueGet, OWNER_SLOT};

const STEP: i64 = 61_935;

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Everything the node read before this frame has been handled: the
/// reader takes pings and frames in order, and answers a `queue_get`.
async fn barrier(c: &mut Conn, id: &str) {
    c.send(Frame::QueueGet(QueueGet { request_id: id.into() }));
    tokio::time::timeout(Duration::from_secs(20), c.until(|f| matches!(f, Frame::QueueView(v) if v.request_id == id)))
        .await
        .expect("no queue_view for the barrier");
}

fn skews(n: &TestNode) -> (Option<i64>, Option<i64>) {
    let link = n.node.tenant(OWNER_SLOT).expect("owner tenant").link.lock().unwrap().skew_secs;
    let status = n.node.status.lock().unwrap().clock_skew_secs;
    (link, status)
}

fn near(got: Option<i64>, want: i64) -> bool {
    got.is_some_and(|g| (g - want).abs() <= 5)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_attach_skew_is_remeasured_from_the_servers_clock_ping() {
    let mut fake = Fake::start().await;
    // The server is 17 h ahead of the node at attach: the node is behind.
    fake.welcome_time.store(unix_now() + STEP, std::sync::atomic::Ordering::SeqCst);
    let n = start_node(&fake, "skew-ping", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let _ = tenant_opened(&n, OWNER_SLOT).await;
    let (link, status) = skews(&n);
    assert!(near(link, -STEP) && near(status, -STEP), "measured at attach: link {link:?}, status {status:?}");

    // An old server: its ping is empty (and anything that isn't a clock is
    // ignored). The attach number stays, and the session goes on.
    c.tx.send(Out::Ping(Vec::new())).unwrap();
    c.tx.send(Out::Ping(b"hello".to_vec())).unwrap();
    barrier(&mut c, "old-server").await;
    let (link, status) = skews(&n);
    assert!(near(link, -STEP) && near(status, -STEP), "an old server's ping leaves the attach skew: link {link:?}, status {status:?}");

    // The node's clock was fixed (here: the server's own clock is the real
    // one now). The next clock ping re-measures; before 0.46.15 the
    // -61935 stayed until a reconnect.
    c.tx.send(Out::Ping(frames::clock_ping(unix_now()))).unwrap();
    barrier(&mut c, "new-server").await;
    let (link, status) = skews(&n);
    assert!(near(link, 0) && near(status, 0), "re-measured from the ping: link {link:?}, status {status:?}");
    assert!(!fake_closed(&c), "the session stayed up through every ping");
}

fn fake_closed(c: &Conn) -> bool {
    c.is_closed.load(std::sync::atomic::Ordering::SeqCst)
}

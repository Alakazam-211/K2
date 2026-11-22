//! The server's ping carries its clock (0.46.15, `frames::clock_ping`) and a
//! node from before that keeps its session. An old node's reader opens text
//! frames in order and skips anything else, and its tungstenite echoes each
//! ping in a pong, so the frames after the clock pings still open in
//! sequence and the server reads the pongs without ending the session.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_servers_ping_carries_its_clock_and_an_old_node_reads_on() {
    use tokio_tungstenite::tungstenite::protocol::Role;
    let (a, b) = tokio::io::duplex(64 * 1024);
    let server_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut node_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(b, Role::Client, None).await;
    let node_key = SigningKey::generate().unwrap();
    let server_key = SigningKey::generate().unwrap();
    let server_spki = server_key.spki_der().to_vec();
    let session = "ping-clock-session".to_string();
    let keys = SessionKeys { key: server_key, session: session.clone(), node_spki: node_key.spki_der().to_vec(), node_id: "n-old".into() };
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let (work_tx, _work_rx) = std::sync::mpsc::channel::<Frame>();
    let (bytes_in, bytes_out) = (AtomicU64::new(0), AtomicU64::new(0));
    let (mut sink, mut source) = server_ws.split();
    let last_in = AtomicI64::new(0);
    let server = pump(&mut sink, &mut source, &mut rx, &work_tx, &keys, &bytes_in, &bytes_out, &last_in, Duration::from_millis(50));
    let old_node = async move {
        let mut pings: Vec<Vec<u8>> = Vec::new();
        let mut opened: Vec<Frame> = Vec::new();
        let mut expected = 0u64;
        while opened.len() < 2 {
            let msg = tokio::time::timeout(Duration::from_secs(15), node_ws.next())
                .await
                .expect("the server went quiet")
                .expect("the server closed")
                .expect("read");
            match msg {
                Message::Text(t) => {
                    let f = frames::open(&server_spki, Direction::ControllerToNode, &session, expected, &t).expect("a frame after the clock pings opens in sequence");
                    expected += 1;
                    opened.push(f);
                }
                Message::Ping(p) => {
                    pings.push(p);
                    // Two clock pings in (and their pongs on the way back):
                    // now the frames.
                    if pings.len() == 2 {
                        for id in ["q1", "q2"] {
                            tx.send(Out::Frame(Box::new(Frame::QueueGet(pshare::QueueGet { request_id: id.into() })))).unwrap();
                        }
                    }
                }
                _ => {}
            }
        }
        (pings, opened)
    };
    tokio::select! {
        why = server => panic!("the session ended: {why}"),
        (pings, opened) = old_node => {
            for p in &pings {
                assert!(p.len() <= 125, "a control frame's payload: {} bytes", p.len());
                let t = frames::ping_clock(p).unwrap_or_else(|| panic!("no clock in the ping: {:?}", String::from_utf8_lossy(p)));
                assert!((t - compute::now()).abs() <= 5, "the server's own clock: {t}");
            }
            let ids: Vec<&str> = opened.iter().map(|f| match f {
                Frame::QueueGet(g) => g.request_id.as_str(),
                other => panic!("unexpected frame {}", other.kind()),
            }).collect();
            assert_eq!(ids, ["q1", "q2"]);
        }
    }
}

/// One server side of a session over an in-memory pipe, and the node's end.
async fn pipe_session(
    name: &str,
) -> (tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>, tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>, SessionKeys) {
    use tokio_tungstenite::tungstenite::protocol::Role;
    let (a, b) = tokio::io::duplex(64 * 1024);
    let server_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let node_ws = tokio_tungstenite::WebSocketStream::from_raw_socket(b, Role::Client, None).await;
    let node_key = SigningKey::generate().unwrap();
    let keys = SessionKeys { key: SigningKey::generate().unwrap(), session: format!("{name}-session"), node_spki: node_key.spki_der().to_vec(), node_id: name.into() };
    (server_ws, node_ws, keys)
}

/// A half-open session (0.46.17): nothing comes in (no frame, no pong),
/// though our writes still succeed. It ends `idle` after three pings, so
/// the node goes offline here and the node-lost rule and the reattach's
/// re-asks work. Before, the reader waited forever and the node looked
/// online for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_brings_nothing_in_ends_idle() {
    let (server_ws, node_ws, keys) = pipe_session("n-silent").await;
    let (_tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let (work_tx, _work_rx) = std::sync::mpsc::channel::<Frame>();
    let (bytes_in, bytes_out) = (AtomicU64::new(0), AtomicU64::new(0));
    let last_in = AtomicI64::new(0);
    let (mut sink, mut source) = server_ws.split();
    let t0 = Instant::now();
    let why = tokio::time::timeout(
        Duration::from_secs(10),
        pump(&mut sink, &mut source, &mut rx, &work_tx, &keys, &bytes_in, &bytes_out, &last_in, Duration::from_millis(50)),
    )
    .await
    .expect("a silent session must end, not hang");
    assert_eq!(why, "idle");
    assert!(t0.elapsed() >= Duration::from_millis(150), "not before three pings: {:?}", t0.elapsed());
    // The node's end never read anything (a dead leg): kept open to here.
    drop(node_ws);
}

/// A live node that sends nothing but its pongs (any node: its websocket
/// library answers our pings) keeps its session for many idle windows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_that_only_answers_pings_stays_connected() {
    let (server_ws, mut node_ws, keys) = pipe_session("n-pongs").await;
    let (_tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let (work_tx, _work_rx) = std::sync::mpsc::channel::<Frame>();
    let (bytes_in, bytes_out) = (AtomicU64::new(0), AtomicU64::new(0));
    let last_in = AtomicI64::new(0);
    let (mut sink, mut source) = server_ws.split();
    let server = pump(&mut sink, &mut source, &mut rx, &work_tx, &keys, &bytes_in, &bytes_out, &last_in, Duration::from_millis(50));
    // The node only reads: its library answers each ping with a pong.
    let node = async move {
        let mut pings = 0u32;
        while let Some(msg) = node_ws.next().await {
            if let Message::Ping(_) = msg.expect("read") {
                pings += 1;
            }
        }
        pings
    };
    tokio::select! {
        why = server => panic!("the session ended though the node answered every ping: {why}"),
        pings = node => panic!("the node's end closed after {pings} pings"),
        _ = tokio::time::sleep(Duration::from_millis(1500)) => {}
    }
    assert_eq!(last_in.load(Ordering::Relaxed), 0, "pongs are not frames");
}

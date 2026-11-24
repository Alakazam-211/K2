//! The per-inbox IMAP guard against a stub IMAP server, through the real
//! login path (`RealImapOps`, plaintext only in tests):
//! - G1: 24 parallel reads of one linked inbox open at most
//!   `MAX_SESSIONS_PER_INBOX` sessions at once, and every read still lands.
//! - G2: a throttle answer starts a cool-down: the next read answers
//!   `inbox_throttled` without connecting, and a re-link doesn't lift it.
//! - G3 (0.46.18): reads the provider never answers time out, free their
//!   places (no LOGOUT is tried on a timed-out socket), start no cool-down,
//!   and the next read lands.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::mail::external::{ImapOps, ManageOp};
use crate::mail::external_imap::RealImapOps;

struct Stub {
    port: u16,
    connections: Arc<AtomicUsize>,
    max_open: Arc<AtomicUsize>,
}

/// A stub IMAP server: every connection greets, answers LOGIN (OK, or the
/// Gmail throttle NO), LIST after `hold`, LOGOUT. It counts connections and
/// the most open at once.
fn spawn_stub(throttle: bool, hold: Duration) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let connections = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let max_open = Arc::new(AtomicUsize::new(0));
    let (c, o, m) = (connections.clone(), open.clone(), max_open.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            c.fetch_add(1, Ordering::SeqCst);
            let now = o.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(now, Ordering::SeqCst);
            let o = o.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut w = stream;
                let _ = w.write_all(b"* OK stub IMAP4rev1 ready\r\n");
                // Counted closed BEFORE the LOGOUT answer goes out, so a
                // client can't free its place while this still counts open.
                let mut counted = true;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let mut parts = line.trim_end().splitn(3, ' ');
                    let tag = parts.next().unwrap_or("*").to_string();
                    let verb = parts.next().unwrap_or("").to_ascii_uppercase();
                    let reply = match verb.as_str() {
                        "LOGIN" if throttle => format!(
                            "{tag} NO [ALERT] Too many simultaneous connections. (Failure)\r\n"
                        ),
                        "LOGIN" => format!("{tag} OK LOGIN done\r\n"),
                        "LIST" => {
                            std::thread::sleep(hold);
                            format!("* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n{tag} OK LIST done\r\n")
                        }
                        "LOGOUT" => format!("* BYE bye\r\n{tag} OK LOGOUT done\r\n"),
                        _ => format!("{tag} BAD unexpected\r\n"),
                    };
                    let last = verb == "LOGOUT" || (throttle && verb == "LOGIN");
                    if last {
                        o.fetch_sub(1, Ordering::SeqCst);
                        counted = false;
                    }
                    if w.write_all(reply.as_bytes()).is_err() || last {
                        break;
                    }
                }
                if counted {
                    o.fetch_sub(1, Ordering::SeqCst);
                }
            });
        }
    });
    crate::mail::external_imap::plaintext_ports().lock().unwrap_or_else(|p| p.into_inner()).insert(port);
    Stub { port, connections, max_open }
}

/// A password-linked inbox row pointing at the stub.
fn seed(port: u16) -> k2_core::db::schema::MailExternalInbox {
    let id = uuid::Uuid::new_v4().to_string();
    let address = format!("guard-{}@ext.example", &id[..8]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, port, username, created_at) \
             VALUES (?1, ?2, ?3, '127.0.0.1', ?4, ?3, 100)",
            rusqlite::params![id, uuid::Uuid::new_v4().to_string(), address, port],
        )
        .expect("seed");
    }
    crate::mail::external::inbox_for_address(&address).expect("row")
}

fn list(inbox: &k2_core::db::schema::MailExternalInbox) -> Result<(), String> {
    RealImapOps.manage(inbox, "pw", &ManageOp::FolderList).map(|_| ()).map_err(|e| format!("{e:?}"))
}

#[test]
fn g1_twenty_four_parallel_reads_open_at_most_four_sessions() {
    let stub = spawn_stub(false, Duration::from_millis(150));
    let inbox = seed(stub.port);
    let handles: Vec<_> = (0..24)
        .map(|_| {
            let inbox = inbox.clone();
            std::thread::spawn(move || list(&inbox))
        })
        .collect();
    let results: Vec<Result<(), String>> = handles.into_iter().map(|h| h.join().expect("join")).collect();
    for r in &results {
        assert!(r.is_ok(), "every queued read lands: {r:?}");
    }
    let max = stub.max_open.load(Ordering::SeqCst);
    assert!(
        max <= crate::mail::imap_guard::MAX_SESSIONS_PER_INBOX,
        "at most {} sessions at once, saw {max}",
        crate::mail::imap_guard::MAX_SESSIONS_PER_INBOX
    );
    assert!(max >= 2, "the reads did run side by side ({max})");
    assert_eq!(stub.connections.load(Ordering::SeqCst), 24, "one connection per read, no extras");
    crate::mail::imap_guard::reset_for_test(&inbox.id);
}

#[test]
fn g2_a_throttle_answer_starts_a_cooldown_that_a_relink_does_not_lift() {
    let stub = spawn_stub(true, Duration::ZERO);
    let inbox = seed(stub.port);
    let first = list(&inbox).expect_err("the provider throttles");
    assert!(first.contains("Too many simultaneous connections"), "{first}");
    assert_eq!(stub.connections.load(Ordering::SeqCst), 1);
    let left = crate::mail::imap_guard::cooldown_left(&inbox.id).expect("a cool-down runs");
    assert!(left > 290, "{left}");

    let second = list(&inbox).expect_err("paused");
    assert_eq!(crate::mail::imap_guard::parse_tag(&second).map(|t| t.0), Some("inbox_throttled"), "{second}");
    assert_eq!(stub.connections.load(Ordering::SeqCst), 1, "nothing reached the provider");
    // The catalog says throttled (wait), not "needs re-link".
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(crate::mail::relink::relink_reason(&conn, &inbox.id), Some("throttled"));
    }

    crate::mail::relink::after_relink(&inbox.id);
    let third = list(&inbox).expect_err("still paused after a re-link");
    assert_eq!(crate::mail::imap_guard::parse_tag(&third).map(|t| t.0), Some("inbox_throttled"), "{third}");
    assert_eq!(stub.connections.load(Ordering::SeqCst), 1);
    crate::mail::imap_guard::reset_for_test(&inbox.id);
}

struct StallingStub {
    port: u16,
    /// LOGOUT lines that arrived on a connection after its stalled LIST.
    logout_after_stall: Arc<AtomicUsize>,
}

/// A stub whose first `stall` connections answer LOGIN and then never
/// answer LIST (they keep reading until the client hangs up); later
/// connections answer everything.
fn spawn_stalling_stub(stall: usize) -> StallingStub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let logout_after_stall = Arc::new(AtomicUsize::new(0));
    let after = logout_after_stall.clone();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(stream) = stream else { break };
            let after = after.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut w = stream;
                let _ = w.write_all(b"* OK stub IMAP4rev1 ready\r\n");
                let mut stalled = false;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let mut parts = line.trim_end().splitn(3, ' ');
                    let tag = parts.next().unwrap_or("*").to_string();
                    let verb = parts.next().unwrap_or("").to_ascii_uppercase();
                    if stalled {
                        if verb == "LOGOUT" {
                            after.fetch_add(1, Ordering::SeqCst);
                        }
                        continue;
                    }
                    let reply = match verb.as_str() {
                        "LOGIN" => format!("{tag} OK LOGIN done\r\n"),
                        "LIST" if n < stall => {
                            stalled = true;
                            continue;
                        }
                        "LIST" => format!("* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n{tag} OK LIST done\r\n"),
                        "LOGOUT" => format!("* BYE bye\r\n{tag} OK LOGOUT done\r\n"),
                        _ => format!("{tag} BAD unexpected\r\n"),
                    };
                    if w.write_all(reply.as_bytes()).is_err() || verb == "LOGOUT" {
                        break;
                    }
                }
            });
        }
    });
    crate::mail::external_imap::plaintext_ports().lock().unwrap_or_else(|p| p.into_inner()).insert(port);
    crate::mail::external_imap::stub_timeouts().lock().unwrap_or_else(|p| p.into_inner()).insert(
        port,
        crate::mail::imap_tls::Timeouts { connect: Duration::from_secs(2), io: Duration::from_secs(1) },
    );
    StallingStub { port, logout_after_stall }
}

#[test]
fn g3_stalled_reads_time_out_and_free_their_places() {
    use crate::mail::imap_guard::{acquire_within, cooldown_left, MAX_SESSIONS_PER_INBOX};
    let stub = spawn_stalling_stub(MAX_SESSIONS_PER_INBOX);
    let inbox = seed(stub.port);
    let (tx, rx) = std::sync::mpsc::channel();
    for _ in 0..MAX_SESSIONS_PER_INBOX {
        let (inbox, tx) = (inbox.clone(), tx.clone());
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let r = list(&inbox);
            let _ = tx.send((r, t0.elapsed()));
        });
    }
    for _ in 0..MAX_SESSIONS_PER_INBOX {
        let (r, took) = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("a read the provider never answers must return (it blocked with no timeout)");
        let err = r.expect_err("an unanswered read fails");
        assert!(err.contains("did not answer within 1s"), "{err}");
        assert!(took < Duration::from_secs(6), "bounded by the read timeout: {took:?}");
    }
    assert!(cooldown_left(&inbox.id).is_none(), "a timeout is not the provider pushing back");
    // Every place is free again: the cap's worth are taken at once.
    let held: Vec<_> = (0..MAX_SESSIONS_PER_INBOX)
        .map(|_| acquire_within(&inbox.id, Duration::ZERO).expect("the place was freed"))
        .collect();
    drop(held);
    // And the next read lands (the stub answers from here on).
    list(&inbox).expect("the read after the timeouts lands");
    // Give a late LOGOUT time to arrive before counting.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(stub.logout_after_stall.load(Ordering::SeqCst), 0, "nothing is sent on a timed-out connection");
    crate::mail::imap_guard::reset_for_test(&inbox.id);
}

//! The client's tries and rules that the parity table can't show with
//! one stub: a daemon that isn't there yet, one that moved ports, a
//! migrating daemon that never gets ready, a second refusal after a heal,
//! and that a passport never sends the owner token (PRD 13.9 #4).

#![cfg(unix)]

mod common;

use common::stub::{Reply, Stub};
use k5_cli::transport::{Auth, Call, Channel, Client, Error, Passport, SecretsDir};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const PAUSE: Duration = Duration::from_millis(100);

fn scratch(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("k5-tc-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stub(tag: &str) -> Stub {
    static N: AtomicUsize = AtomicUsize::new(0);
    Stub::start(&PathBuf::from(format!("/tmp/k5tc-{tag}-{}-{}.sock", std::process::id(), N.fetch_add(1, Ordering::SeqCst))))
}

/// A loopback port nothing listens on.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn owner(port: u16) -> Client {
    let mut c = Client::new("127.0.0.1", port, "/p", Auth::Owner("owner-tok".into()));
    c.pause = PAUSE;
    c
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn nothing_listening_is_tried_four_times_then_unreachable() {
    let mut c = owner(free_port());
    let t0 = Instant::now();
    let err = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap_err();
    assert!(matches!(err, Error::Unreachable { .. }), "{err:?}");
    // Three pauses between four tries.
    let took = t0.elapsed();
    assert!(took >= PAUSE * 3 && took < PAUSE * 3 + Duration::from_secs(3), "{took:?}");
}

#[test]
fn a_body_from_stdin_is_never_tried_twice() {
    let mut c = owner(free_port());
    c.pause = Duration::from_secs(5);
    let t0 = Instant::now();
    let err = c.post_json_stdin("/cli/x", b"{}").unwrap_err();
    assert!(matches!(err, Error::Unreachable { .. }), "{err:?}");
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
}

#[test]
fn a_daemon_that_comes_up_between_tries_answers() {
    let port = free_port();
    let up = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let (mut s, _) = l.accept().unwrap();
        use std::io::{Read, Write};
        let mut buf = [0u8; 4096];
        let mut got = Vec::new();
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        got
    });
    let mut c = owner(port);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
    let got = text(&up.join().unwrap());
    assert!(got.starts_with("GET /cli/x?token=owner-tok&project=%2fp HTTP/1.1\r\n"), "{got}");
}

#[test]
fn a_refused_loopback_port_follows_daemon_port() {
    let s = stub("port");
    let dir = scratch("port");
    let file = dir.join("daemon.port");
    std::fs::write(&file, format!("{}\n", s.port)).unwrap();
    let mut c = owner(free_port());
    c.port_file = Some(file);
    s.reset(&[]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::RequestPost(&["a=1"])).unwrap();
    assert_eq!(r.status, 200);
    let log = s.log();
    assert_eq!(log.len(), 1);
    let got = text(&log[0].bytes);
    assert!(got.contains(&format!("\r\nHost: 127.0.0.1:{}\r\n", s.port)), "{got}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn migrating_four_times_gives_the_last_answer() {
    let s = stub("mig");
    let migrating = Reply::Answer(503, r#"{"state":"migrating"}"#.into());
    s.reset(&[migrating.clone(), migrating.clone(), migrating.clone(), migrating.clone(), Reply::Answer(200, "{}".into())]);
    let mut c = owner(s.port);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!((r.status, text(&r.body)), (503, r#"{"state":"migrating"}"#.to_string()));
    assert_eq!(s.log().len(), 4);
    // The status decides: a 200 that says migrating is final.
    s.reset(&[Reply::Answer(200, r#"{"state":"migrating"}"#.into())]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(s.log().len(), 1);
    // Not retried by a call that doesn't (cli_post_json).
    s.reset(&[migrating]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::PostJson(b"{}")).unwrap();
    assert_eq!(r.status, 503);
    assert_eq!(s.log().len(), 1);
}

#[test]
fn a_heal_happens_once() {
    let s = stub("heal");
    let dir = scratch("heal");
    let refused = r#"{"error":"Invalid or missing auth token"}"#;
    std::fs::write(dir.join(".k2-passport"), "sessA.new1\n").unwrap();
    let mut c = Client::new("127.0.0.1", s.port, "/p", Auth::Passport { passport: Passport::new("sessA.old1").unwrap(), sock: None });
    c.pause = PAUSE;
    c.healer = Some(Box::new(SecretsDir(dir.clone())));
    s.reset(&[Reply::Answer(401, refused.into()), Reply::Answer(401, refused.into()), Reply::Answer(401, refused.into())]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.status, 401);
    let log: Vec<String> = s.log().iter().map(|l| text(&l.bytes)).collect();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].starts_with("GET /cli/x?token=sessA.old1&"), "{}", log[0]);
    assert!(log[1].starts_with("GET /cli/x?token=sessA.new1&"), "{}", log[1]);
    assert_eq!(c.passport().map(Passport::as_str), Some("sessA.new1"));
    // Healed already: the next refusal is the answer.
    std::fs::write(dir.join(".k2-passport"), "sessA.new2\n").unwrap();
    s.reset(&[Reply::Answer(401, refused.into())]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(s.log().len(), 1);
    // The owner never heals.
    s.reset(&[Reply::Answer(401, refused.into())]);
    let mut o = owner(s.port);
    o.healer = Some(Box::new(SecretsDir(dir.clone())));
    assert_eq!(o.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap().status, 401);
    assert_eq!(s.log().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// PRD 13.9 #4: a client with a passport has no owner token to send, on
/// any channel or retry: the passport's refusal is the answer.
#[test]
fn a_passport_never_sends_the_owner_token() {
    let s = stub("owner");
    let mut c = Client::new("127.0.0.1", s.port, "/p", Auth::Passport { passport: Passport::new("sessA.pp").unwrap(), sock: Some(s.sock.clone()) });
    c.pause = PAUSE;
    let refused = Reply::Answer(403, r#"{"error":"owner_only"}"#.into());
    let not_served = Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.into());
    // In order: not served then TCP, TCP, the cell socket, not served then
    // TCP, TCP.
    s.reset(&[not_served.clone(), refused.clone(), refused.clone(), refused.clone(), not_served, refused.clone(), refused]);
    let calls: Vec<Result<_, Error>> = vec![
        c.send("/cli/inbox/list", Call::Request(&[])),
        c.send("/cli/agents/list", Call::RequestPost(&[])),
        c.send_json(true, "/cli/x", "{}"),
        c.send_json(false, "/cli/x", ""),
        c.post_json_stdin("/cli/x", b"{}"),
    ];
    for r in calls {
        assert_eq!(r.unwrap().status, 403);
    }
    let log = s.log();
    assert_eq!(log.len(), 7);
    for l in &log {
        let t = text(&l.bytes);
        assert!(!t.contains("owner"), "{t}");
        let carries = t.contains("token=sessA.pp") || t.contains("\r\nAuthorization: Bearer sessA.pp\r\n");
        assert!(carries, "no passport in {t}");
        assert!(!t.contains("Bearer \r\n"), "an empty Bearer: {t}");
    }
}

#[test]
fn a_cell_socket_that_is_gone_goes_to_tcp() {
    let s = stub("gone");
    let gone = PathBuf::from(format!("/tmp/k5tc-gone-{}.sock", std::process::id()));
    let mut c = Client::new("127.0.0.1", s.port, "/p", Auth::Passport { passport: Passport::new("sessA.pp").unwrap(), sock: Some(gone) });
    c.pause = PAUSE;
    s.reset(&[]);
    c.send("/cli/inbox/list", Call::Request(&[])).unwrap();
    let log = s.log();
    assert_eq!(log.iter().map(|l| l.channel).collect::<Vec<_>>(), vec!["tcp"]);
    // A remote host forces TCP too.
    let mut c = Client::new("127.0.0.1", s.port, "/p", Auth::Passport { passport: Passport::new("sessA.pp").unwrap(), sock: Some(s.sock.clone()) });
    c.force_tcp = true;
    s.reset(&[]);
    c.send_json(false, "/cli/x", "").unwrap();
    assert_eq!(s.log().iter().map(|l| l.channel).collect::<Vec<_>>(), vec!["tcp"]);
}

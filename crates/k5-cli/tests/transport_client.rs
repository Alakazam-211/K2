//! The client's tries and rules that the parity table can't show with
//! one stub: a daemon that isn't there yet, one that moved ports, a
//! migrating daemon that never gets ready, a second refusal after a heal,
//! that a passport never sends the owner token (PRD 13.9 #4), and that a
//! request that went out on the cell socket is never sent again as a
//! POST over TCP.

#![cfg(unix)]

mod common;

use common::stub::{dead_port, Reply, Stub};
use k5_cli::transport::{Auth, Call, Channel, Client, Error, Passport, SecretsDir};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PAUSE: Duration = Duration::from_millis(100);

fn scratch(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k5-tc-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stub(tag: &str) -> Stub {
    static N: AtomicUsize = AtomicUsize::new(0);
    Stub::start(&PathBuf::from(format!(
        "/tmp/k5tc-{tag}-{}-{}.sock",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )))
}

/// A loopback port free now, for a listener the test starts on it later.
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
    let mut c = owner(dead_port());
    let t0 = Instant::now();
    let err = c
        .call(Channel::Tcp, "/cli/x", Call::Request(&[]))
        .unwrap_err();
    assert!(matches!(err, Error::Unreachable { .. }), "{err:?}");
    // Three pauses between four tries.
    let took = t0.elapsed();
    assert!(
        took >= PAUSE * 3 && took < PAUSE * 3 + Duration::from_secs(3),
        "{took:?}"
    );
}

#[test]
fn a_body_from_stdin_is_never_tried_twice() {
    let mut c = owner(dead_port());
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
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        got
    });
    let mut c = owner(port);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
    let got = text(&up.join().unwrap());
    assert!(
        got.starts_with("GET /cli/x?project=%2Fp HTTP/1.1\r\n")
            && got.contains("\r\nAuthorization: Bearer owner-tok\r\n"),
        "{got}"
    );
}

#[test]
fn a_refused_loopback_port_follows_daemon_port() {
    let s = stub("port");
    let dir = scratch("port");
    let file = dir.join("daemon.port");
    std::fs::write(&file, format!("{}\n", s.port)).unwrap();
    let mut c = owner(dead_port());
    c.port_file = Some(file);
    s.reset(&[]);
    let r = c
        .call(Channel::Tcp, "/cli/x", Call::RequestPost(&["a=1"]))
        .unwrap();
    assert_eq!(r.status, 200);
    let log = s.log();
    assert_eq!(log.len(), 1);
    let got = text(&log[0].bytes);
    assert!(
        got.contains(&format!("\r\nHost: 127.0.0.1:{}\r\n", s.port)),
        "{got}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn migrating_four_times_gives_the_last_answer() {
    let s = stub("mig");
    let migrating = Reply::Answer(503, r#"{"state":"migrating"}"#.into());
    s.reset(&[
        migrating.clone(),
        migrating.clone(),
        migrating.clone(),
        migrating.clone(),
        Reply::Answer(200, "{}".into()),
    ]);
    let mut c = owner(s.port);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(
        (r.status, text(&r.body)),
        (503, r#"{"state":"migrating"}"#.to_string())
    );
    assert_eq!(s.log().len(), 4);
    // The status decides: a 200 that says migrating is final.
    s.reset(&[Reply::Answer(200, r#"{"state":"migrating"}"#.into())]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(s.log().len(), 1);
    // Not retried by a call that doesn't (cli_post_json).
    s.reset(&[migrating]);
    let r = c
        .call(Channel::Tcp, "/cli/x", Call::PostJson(b"{}"))
        .unwrap();
    assert_eq!(r.status, 503);
    assert_eq!(s.log().len(), 1);
}

#[test]
fn a_heal_happens_once() {
    let s = stub("heal");
    let dir = scratch("heal");
    let refused = r#"{"error":"Invalid or missing auth token"}"#;
    std::fs::write(dir.join(".k2-passport"), "sessA.new1\n").unwrap();
    let mut c = Client::new(
        "127.0.0.1",
        s.port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.old1").unwrap(),
            sock: None,
        },
    );
    c.pause = PAUSE;
    c.healer = Some(Box::new(SecretsDir(dir.clone())));
    s.reset(&[
        Reply::Answer(401, refused.into()),
        Reply::Answer(401, refused.into()),
        Reply::Answer(401, refused.into()),
    ]);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.status, 401);
    let log: Vec<String> = s.log().iter().map(|l| text(&l.bytes)).collect();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(
        log[0].contains("\r\nAuthorization: Bearer sessA.old1\r\n"),
        "{}",
        log[0]
    );
    assert!(
        log[1].contains("\r\nAuthorization: Bearer sessA.new1\r\n"),
        "{}",
        log[1]
    );
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
    assert_eq!(
        o.call(Channel::Tcp, "/cli/x", Call::Request(&[]))
            .unwrap()
            .status,
        401
    );
    assert_eq!(s.log().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// PRD 13.9 #4: a client with a passport has no owner token to send, on
/// any channel or retry: the passport's refusal is the answer.
#[test]
fn a_passport_never_sends_the_owner_token() {
    let s = stub("owner");
    let mut c = Client::new(
        "127.0.0.1",
        s.port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.pp").unwrap(),
            sock: Some(s.sock.clone()),
        },
    );
    c.pause = PAUSE;
    let refused = Reply::Answer(403, r#"{"error":"owner_only"}"#.into());
    let not_served = Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.into());
    // In order: not served then TCP, TCP, the cell socket, not served then
    // TCP, TCP.
    s.reset(&[
        not_served.clone(),
        refused.clone(),
        refused.clone(),
        refused.clone(),
        not_served,
        refused.clone(),
        refused,
    ]);
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
        let carries =
            t.contains("token=sessA.pp") || t.contains("\r\nAuthorization: Bearer sessA.pp\r\n");
        assert!(carries, "no passport in {t}");
        assert!(!t.contains("Bearer \r\n"), "an empty Bearer: {t}");
    }
}

/// `cli_send cli_post`: the cell socket first (`cli_post_uds`: the pairs
/// alone, the passport as Bearer); its answer is the answer. "Not served
/// on cell socket" sends it again over TCP (`cli_post`: `project=` first),
/// with the passport, never the owner token.
#[test]
fn cli_send_cli_post_is_socket_first_then_tcp_when_not_served() {
    let s = stub("qpost");
    let mut c = Client::new(
        "127.0.0.1",
        s.port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.pp").unwrap(),
            sock: Some(s.sock.clone()),
        },
    );
    c.pause = PAUSE;
    let not_served = Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.into());
    let ok = Reply::Answer(200, r#"{"success":true}"#.into());
    s.reset(&[ok.clone(), not_served, ok]);
    let ep = "/cli/publish/subdomain/claim";
    let pairs = ["label=acme", "check=1"];
    let r = c.send(ep, Call::QueryPost(&pairs)).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Uds));
    let r = c.send(ep, Call::QueryPost(&pairs)).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(text(&r.body), r#"{"success":true}"#);
    let log = s.log();
    assert_eq!(
        log.iter().map(|l| l.channel).collect::<Vec<_>>(),
        vec!["uds", "uds", "tcp"]
    );
    let lines: Vec<String> = log
        .iter()
        .map(|l| text(&l.bytes).lines().next().unwrap().to_string())
        .collect();
    assert_eq!(
        lines,
        [
            "POST /cli/publish/subdomain/claim?label=acme&check=1 HTTP/1.1",
            "POST /cli/publish/subdomain/claim?label=acme&check=1 HTTP/1.1",
            "POST /cli/publish/subdomain/claim?project=%2Fp&label=acme&check=1 HTTP/1.1",
        ]
    );
    for l in &log {
        let t = text(&l.bytes);
        assert!(t.contains("\r\nAuthorization: Bearer sessA.pp\r\n"), "{t}");
        assert!(!t.contains("owner"), "{t}");
    }
}

/// `_connect_curl_daemon`'s requests: the JSON body POSTed to
/// /cli/connect/proxy as it is (no `project=`), the cell socket first.
/// Its answer is the answer whatever the status; "not served on cell
/// socket" goes on to TCP. A socket that read a non-GET request and gave
/// no answer: never sent again. A GET goes on to TCP.
#[test]
fn the_connect_proxy_requests() {
    let s = stub("proxy");
    let mut c = with_passport(s.port, &s.sock);
    let not_served = Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.into());
    let relayed = Reply::Answer(200, r#"{"status":201,"body":"{}"}"#.into());
    s.reset(&[
        Reply::Answer(403, r#"{"code":"forbidden"}"#.into()),
        not_served,
        relayed,
    ]);
    let req = br#"{"method": "POST", "endpoint": "/subdomains", "body": "{}"}"#;
    let r = c.connect_proxy(false, req).unwrap();
    assert_eq!((r.status, r.channel), (403, Channel::Uds));
    let r = c.connect_proxy(false, req).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let log = s.log();
    assert_eq!(
        log.iter().map(|l| l.channel).collect::<Vec<_>>(),
        vec!["uds", "uds", "tcp"]
    );
    for l in &log {
        let t = text(&l.bytes);
        assert!(t.starts_with("POST /cli/connect/proxy HTTP/1.1\r\n"), "{t}");
        assert!(t.contains("\r\nAuthorization: Bearer sessA.pp\r\n"), "{t}");
        assert!(t.contains("\r\nContent-Type: application/json\r\n"), "{t}");
        assert!(t.ends_with(std::str::from_utf8(req).unwrap()), "{t}");
    }

    let d = door("proxy", Act::CloseNoAnswer, Act::Answer);
    let mut c = with_passport(d.port, &d.sock);
    let err = c.connect_proxy(false, req).unwrap_err();
    assert!(matches!(err, Error::MaybeApplied { .. }), "{err:?}");
    assert!(
        err.to_string().contains("didn't answer /cli/connect/proxy"),
        "{err}"
    );
    assert_eq!(d.counts(), (1, 0), "(cell socket, TCP) requests");
    let r = c.connect_proxy(true, req).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(d.counts(), (2, 1));

    // No daemon on TCP: curl's 7 comes back, for the caller's fallback.
    let mut o = owner(dead_port());
    o.pause = Duration::ZERO;
    let err = o.connect_proxy(false, req).unwrap_err();
    assert_eq!(err.curl_code(), 7, "{err:?}");
}

/// `_connect_curl`'s own call to the control plane: the tunnel token is
/// the `Authorization` header and appears nowhere else (not the request
/// line, no other header, not the body); no hand-over headers (it is not
/// the daemon); a JSON body with its type; PUT for the repoint.
#[test]
fn the_control_plane_call_sends_the_token_only_as_a_header() {
    let s = stub("cplane");
    s.reset(&[
        Reply::Answer(201, r#"{"label":"acme"}"#.into()),
        Reply::Answer(200, "{}".into()),
    ]);
    let base = k5_cli::transport::remote::base(&format!("http://127.0.0.1:{}", s.port)).unwrap();
    let token = "tun-tok-secret".to_string();
    let mut c = Client::new("127.0.0.1", 1, "/p", Auth::Owner(token.clone()));
    k5_cli::transport::remote::apply(&mut c, &base, token.clone());
    let body = br#"{"label": "acme", "target": "127.0.0.1:7"}"#;
    let r = c.control_plane(false, "/subdomains", body).unwrap();
    assert_eq!((r.status, r.channel), (201, Channel::Tcp));
    let r = c
        .control_plane(true, "/subdomains/acme", br#"{"target": "x"}"#)
        .unwrap();
    assert_eq!(r.status, 200);
    let log = s.log();
    assert_eq!(log.len(), 2);
    for (l, line) in log
        .iter()
        .zip(["POST /subdomains HTTP/1.1", "PUT /subdomains/acme HTTP/1.1"])
    {
        let t = text(&l.bytes);
        let (head, rest) = t.split_once("\r\n\r\n").unwrap();
        let mut lines = head.split("\r\n");
        assert_eq!(lines.next(), Some(line), "{t}");
        let with_token: Vec<&str> = head.lines().filter(|h| h.contains(&token)).collect();
        assert_eq!(
            with_token,
            [format!("Authorization: Bearer {token}").as_str()],
            "{t}"
        );
        assert!(!rest.contains(&token), "{t}");
        assert!(!head.contains("X-K5-"), "{t}");
        assert!(head.contains("\r\nContent-Type: application/json"), "{t}");
    }
}

#[test]
fn a_cell_socket_that_is_gone_goes_to_tcp() {
    let s = stub("gone");
    let gone = PathBuf::from(format!("/tmp/k5tc-gone-{}.sock", std::process::id()));
    let mut c = Client::new(
        "127.0.0.1",
        s.port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.pp").unwrap(),
            sock: Some(gone),
        },
    );
    c.pause = PAUSE;
    s.reset(&[]);
    c.send("/cli/inbox/list", Call::Request(&[])).unwrap();
    let log = s.log();
    assert_eq!(
        log.iter().map(|l| l.channel).collect::<Vec<_>>(),
        vec!["tcp"]
    );
    // A remote host forces TCP too.
    let mut c = Client::new(
        "127.0.0.1",
        s.port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.pp").unwrap(),
            sock: Some(s.sock.clone()),
        },
    );
    c.force_tcp = true;
    s.reset(&[]);
    c.send_json(false, "/cli/x", "").unwrap();
    assert_eq!(
        s.log().iter().map(|l| l.channel).collect::<Vec<_>>(),
        vec!["tcp"]
    );
}

/// What a [`door`] does with each request it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    /// `200` with `{}`, then a close.
    Answer,
    /// The same whole answer, then a reset (TCP: `SO_LINGER` 0; the cell
    /// socket: shut down both ways).
    AnswerThenReset,
    /// A close with no answer at all.
    CloseNoAnswer,
}

const ANSWER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";

/// Reads one request (its head and its `Content-Length` body).
fn read_request<S: std::io::Read>(s: &mut S) {
    let mut got = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "the request ended inside its head: {:?}", text(&got));
        got.extend_from_slice(&chunk[..n]);
    };
    let head = text(&got[..head_end]);
    let length: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .map(|v| v.trim().parse().unwrap())
        .unwrap_or(0);
    while got.len() < head_end + length {
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "the request ended inside its body");
        got.extend_from_slice(&chunk[..n]);
    }
}

/// Closes a TCP connection with a reset.
fn reset_tcp(s: std::net::TcpStream) {
    use std::os::unix::io::AsRawFd;
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    // SAFETY: a valid socket and a `linger` of the size passed.
    let rc = unsafe {
        libc::setsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    assert_eq!(rc, 0, "SO_LINGER: {}", std::io::Error::last_os_error());
    drop(s);
}

/// A daemon on loopback TCP and a cell socket, each doing `tcp` and
/// `uds` with every request, and counting the requests it read.
struct Door {
    port: u16,
    sock: PathBuf,
    tcp: Arc<AtomicUsize>,
    uds: Arc<AtomicUsize>,
}

impl Door {
    fn counts(&self) -> (usize, usize) {
        (
            self.uds.load(Ordering::SeqCst),
            self.tcp.load(Ordering::SeqCst),
        )
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock);
    }
}

fn door(tag: &str, uds_act: Act, tcp_act: Act) -> Door {
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    static N: AtomicUsize = AtomicUsize::new(0);
    let sock = PathBuf::from(format!(
        "/tmp/k5tcd-{tag}-{}-{}.sock",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&sock);
    let ul = UnixListener::bind(&sock).unwrap();
    let tl = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = tl.local_addr().unwrap().port();
    let (tcp, uds) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let n = Arc::clone(&uds);
    std::thread::spawn(move || {
        for conn in ul.incoming() {
            let mut s = conn.unwrap();
            read_request(&mut s);
            n.fetch_add(1, Ordering::SeqCst);
            match uds_act {
                Act::Answer => s.write_all(ANSWER).unwrap(),
                Act::AnswerThenReset => {
                    s.write_all(ANSWER).unwrap();
                    s.shutdown(std::net::Shutdown::Both).unwrap();
                }
                Act::CloseNoAnswer => {}
            }
        }
    });
    let n = Arc::clone(&tcp);
    std::thread::spawn(move || {
        for conn in tl.incoming() {
            let mut s = conn.unwrap();
            read_request(&mut s);
            n.fetch_add(1, Ordering::SeqCst);
            match tcp_act {
                Act::Answer => s.write_all(ANSWER).unwrap(),
                Act::AnswerThenReset => {
                    s.write_all(ANSWER).unwrap();
                    reset_tcp(s);
                }
                Act::CloseNoAnswer => {}
            }
        }
    });
    Door {
        port,
        sock,
        tcp,
        uds,
    }
}

fn with_passport(port: u16, sock: &std::path::Path) -> Client {
    let mut c = Client::new(
        "127.0.0.1",
        port,
        "/p",
        Auth::Passport {
            passport: Passport::new("sessA.pp").unwrap(),
            sock: Some(sock.to_path_buf()),
        },
    );
    c.pause = PAUSE;
    c
}

/// One call on a client.
type CallFn = fn(&mut Client) -> Result<k5_cli::transport::Response, Error>;

/// The cell socket read a POST and closed with no answer: the POST may
/// have run, so it is never sent again over TCP. A GET is, as the shell's
/// cli_send and cli_send_json do.
#[test]
fn a_post_the_cell_socket_read_is_never_sent_again_over_tcp() {
    let d = door("post", Act::CloseNoAnswer, Act::Answer);
    let mut c = with_passport(d.port, &d.sock);
    let posts: [(&str, CallFn); 5] = [
        ("cli_request_post", |c| {
            c.send("/cli/inbox/claim", Call::RequestPost(&["id=7"]))
        }),
        ("cli_post", |c| {
            c.send(
                "/cli/publish/subdomain/claim",
                Call::QueryPost(&["label=a"]),
            )
        }),
        ("cli_post_form", |c| {
            c.send("/cli/workspace/msg", Call::PostForm(&["text=x"]))
        }),
        ("cli_post_json", |c| {
            c.send("/cli/awareness/publish", Call::PostJson(b"{}"))
        }),
        ("cli_send_json POST", |c| {
            c.send_json(true, "/cli/compute/run", "{}")
        }),
    ];
    for (i, (name, call)) in posts.iter().enumerate() {
        let err = call(&mut c).unwrap_err();
        assert!(matches!(err, Error::MaybeApplied { .. }), "{name}: {err:?}");
        assert_eq!(
            d.counts(),
            (i + 1, 0),
            "{name}: (cell socket, TCP) requests"
        );
    }
    let err = Error::MaybeApplied {
        endpoint: "/cli/workspace/msg".into(),
        waited: 0,
    };
    assert_eq!(
        err.to_string(),
        "Error: K2's server didn't answer /cli/workspace/msg (0 s); the request may have been applied. Check before running it again."
    );
    let r = c.send("/cli/inbox/list", Call::Request(&[])).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(d.counts(), (6, 1), "a GET: (cell socket, TCP) requests");
    let r = c.send_json(false, "/cli/compute/nodes", "").unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(
        d.counts(),
        (7, 2),
        "a cli_send_json GET: (cell socket, TCP) requests"
    );
}

/// `_compute_send`: a POST the cell socket read and closed with no answer
/// is never sent again over TCP; a GET is; `tcp` never tries the socket.
#[test]
fn compute_send_keeps_the_send_once_rule() {
    let d = door("cp", Act::CloseNoAnswer, Act::Answer);
    let mut c = with_passport(d.port, &d.sock);
    let err = c
        .compute_send(true, "/cli/compute/cancel", "{}", false)
        .unwrap_err();
    assert!(matches!(err, Error::MaybeApplied { .. }), "{err:?}");
    assert_eq!(err.to_string(), "Error: K2's server didn't answer /cli/compute/cancel (0 s); the request may have been applied. Check before running it again.");
    assert_eq!(d.counts(), (1, 0), "(cell socket, TCP) requests");
    let r = c
        .compute_send(false, "/cli/compute/stats", "job=abcd", false)
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(d.counts(), (2, 1));
    let r = c
        .compute_send(false, "/cli/compute/stats", "job=abcd", true)
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let r = c
        .compute_send(true, "/cli/compute/cancel", "{}", true)
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(d.counts(), (2, 3), "tcp: never the cell socket");
    assert!(!c.force_tcp, "tcp is for the one call");
}

/// A whole answer followed by a reset is the answer, on both channels,
/// and nothing is sent again.
#[test]
fn a_whole_answer_then_a_reset_is_the_answer() {
    let d = door("reset", Act::AnswerThenReset, Act::AnswerThenReset);
    let mut c = with_passport(d.port, &d.sock);
    let r = c
        .send("/cli/workspace/msg", Call::PostForm(&["text=x"]))
        .unwrap();
    assert_eq!(
        (r.status, r.channel, text(&r.body)),
        (200, Channel::Uds, "{}".to_string())
    );
    let r = c.send("/cli/inbox/list", Call::Request(&[])).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Uds));
    let r = c.send_json(true, "/cli/compute/run", "{}").unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Uds));
    assert_eq!(d.counts(), (3, 0), "(cell socket, TCP) requests");
    let r = c
        .call(
            Channel::Tcp,
            "/cli/inbox/claim",
            Call::RequestPost(&["id=7"]),
        )
        .unwrap();
    assert_eq!(
        (r.status, r.channel, text(&r.body)),
        (200, Channel::Tcp, "{}".to_string())
    );
    let r = c
        .call(Channel::Tcp, "/cli/users/add", Call::PostJson(b"{}"))
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(d.counts(), (3, 2), "(cell socket, TCP) requests");
}

/// Over TCP a request read and closed with no answer is sent once: the
/// no-answer line, never a second try.
#[test]
fn a_close_with_no_answer_over_tcp_is_sent_once() {
    let d = door("tcpclose", Act::Answer, Act::CloseNoAnswer);
    let mut c = with_passport(d.port, &d.sock);
    let err = c
        .call(
            Channel::Tcp,
            "/cli/inbox/claim",
            Call::RequestPost(&["id=7"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::NoAnswer(ref n) if n.empty), "{err:?}");
    assert_eq!(d.counts(), (0, 1));
    let err = c
        .call(Channel::Tcp, "/cli/users/add", Call::PostJson(b"{}"))
        .unwrap_err();
    assert!(matches!(err, Error::NoAnswer(ref n) if n.empty), "{err:?}");
    assert_eq!(d.counts(), (0, 2));
    let err = c
        .call(Channel::Tcp, "/cli/inbox/list", Call::Request(&[]))
        .unwrap_err();
    assert!(matches!(err, Error::NoAnswer(ref n) if n.empty), "{err:?}");
    assert_eq!(d.counts(), (0, 3));
}

/// A cell socket nothing listens on took no byte of the request: the
/// call goes to TCP, POST or GET.
#[test]
fn a_refused_cell_socket_sends_nothing_and_tcp_answers() {
    use std::os::unix::net::UnixListener;
    let d = door("refused", Act::Answer, Act::Answer);
    let dead = PathBuf::from(format!("/tmp/k5tcd-dead-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&dead);
    drop(UnixListener::bind(&dead).unwrap());
    let mut c = with_passport(d.port, &dead);
    let r = c
        .send("/cli/workspace/msg", Call::PostForm(&["text=x"]))
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let r = c.send_json(true, "/cli/compute/run", "{}").unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let r = c.send("/cli/inbox/list", Call::Request(&[])).unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let r = c
        .compute_send(true, "/cli/compute/cancel", "{}", false)
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let r = c
        .send(
            "/cli/publish/subdomain/claim",
            Call::QueryPost(&["label=a"]),
        )
        .unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    assert_eq!(d.counts(), (0, 5));
    std::fs::remove_file(&dead).unwrap();
}

/// The daemon's version comes back with the answer when it is shaped like
/// one, on every kind of call; anything else is no version.
#[test]
fn the_server_version_comes_back_with_the_answer() {
    let s = stub("ver");
    let v = |value: &str| {
        Reply::Headed(
            200,
            vec![("X-K5-Server-Version".into(), value.into())],
            r#"{"ok":true}"#.into(),
        )
    };
    s.reset(&[
        v("0.46.16"),
        v("0.46.17"),
        v("not a version"),
        Reply::Answer(200, "{}".into()),
    ]);
    let mut c = owner(s.port);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.server_version.as_deref(), Some("0.46.16"));
    let r = c
        .call(Channel::Tcp, "/cli/x", Call::PostJson(b"{}"))
        .unwrap();
    assert_eq!(r.server_version.as_deref(), Some("0.46.17"));
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.server_version, None);
    let r = c.call(Channel::Tcp, "/cli/x", Call::Request(&[])).unwrap();
    assert_eq!(r.server_version, None);
    assert_eq!(s.left(), 0);
}

/// The public GET: the path as given, no token, no project, nothing of
/// the owner's, on a client with no credential.
#[test]
fn the_public_get_carries_no_credential() {
    let s = stub("pub");
    s.reset(&[Reply::Answer(200, r#"{"version":"x"}"#.into())]);
    let mut c = Client::new("127.0.0.1", s.port, "/p", Auth::None);
    c.pause = PAUSE;
    let r = c.get_public("/boot-status").unwrap();
    assert_eq!((r.status, r.channel), (200, Channel::Tcp));
    let log = s.log();
    assert_eq!(log.len(), 1);
    let want = format!(
        "GET /boot-status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUser-Agent: k5/{v}\r\nAccept: */*\r\nX-K5-Cli-Version: {v}\r\nX-K5-Cli-Via: native\r\n\r\n",
        s.port,
        v = k5_cli::embedded::version()
    );
    assert_eq!(text(&log[0].bytes), want);
    assert!(matches!(
        c.call(Channel::Tcp, "/cli/x", Call::Request(&[])),
        Err(Error::NoCredential { .. })
    ));
    assert_eq!(s.log().len(), 1, "nothing more was sent");
}

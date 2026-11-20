use super::*;
use crate::transport::client::{Auth, Client, Passport};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// What a scripted server does with one connection.
#[derive(Clone)]
enum Act {
    /// Reads the request, writes these bytes, closes.
    Write(&'static [u8]),
    /// Reads the request, waits, closes.
    Wait(Duration),
}

/// A loopback server that answers each connection with the next act and
/// keeps each request's bytes.
fn server(acts: Vec<Act>) -> (u16, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = Arc::clone(&log);
    std::thread::spawn(move || {
        for act in acts {
            let (mut conn, _) = listener.accept().expect("accept");
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..at]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|x| x.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.parse().expect("length"));
                    if got.len() >= at + 4 + len {
                        break;
                    }
                }
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            l.lock().expect("lock").push(got);
            match act {
                Act::Write(bytes) => {
                    let _ = conn.write_all(bytes);
                }
                Act::Wait(d) => std::thread::sleep(d),
            }
        }
    });
    (port, log)
}

fn opener(port: u16) -> Opener {
    let mut o = Opener::from_client(&Client::new("127.0.0.1", port, "/p", Auth::None));
    o.pause = Duration::ZERO;
    o
}

fn get(route: &str) -> Request {
    Request::new(Method::Get, route, Duration::from_secs(5))
}

#[test]
fn urlencode_is_python_s() {
    assert_eq!(
        urlencode(&[("project", "/Users/a b/c"), ("q", "x&y=z~é")]),
        "project=%2FUsers%2Fa+b%2Fc&q=x%26y%3Dz~%C3%A9"
    );
    assert_eq!(urlencode::<&str, &str>(&[]), "");
    assert_eq!(quote_plus("  "), "++");
    assert_eq!(quote_plus("a+b"), "a%2Bb");
}

#[test]
fn the_request_is_http_client_s() {
    let mut client = Client::new(
        "127.0.0.1",
        4100,
        "/p",
        Auth::Passport {
            passport: Passport::new("pp").expect("a passport"),
            sock: None,
        },
    );
    client.would_refuse = true;
    let o = Opener::from_client(&client);
    assert_eq!(o.token(), Some("pp"));
    let req = Request::new(Method::Post, "/cli/db/create", Duration::from_secs(60))
        .query([("project", "/w s")])
        .data(b"{}".to_vec())
        .header("Authorization", "Bearer pp")
        .header("Content-Type", "application/json");
    let v = &client.version;
    assert_eq!(
        String::from_utf8(o.encode(&req)).expect("utf-8"),
        format!(
            "POST /cli/db/create?project=%2Fw+s HTTP/1.1\r\n\
             Accept-Encoding: identity\r\n\
             Content-Length: 2\r\n\
             Host: 127.0.0.1:4100\r\n\
             User-Agent: k5/{v}\r\n\
             Authorization: Bearer pp\r\n\
             Content-Type: application/json\r\n\
             X-K5-Would-Refuse: 1\r\n\
             X-K5-Cli-Version: {v}\r\n\
             X-K5-Cli-Via: native\r\n\
             Connection: close\r\n\r\n{{}}"
        )
    );
}

#[test]
fn an_empty_post_gets_urllib_s_form_type_and_the_owner_s_source() {
    let mut client = Client::new("h", 1, "/p", Auth::Owner("ow".into()));
    client.owner_from_disk = true;
    let o = Opener::from_client(&client);
    let req = Request::new(Method::Post, "/r", Duration::from_secs(1)).data(Vec::new());
    let text = String::from_utf8(o.encode(&req)).expect("utf-8");
    let v = &client.version;
    assert_eq!(
        text,
        format!(
            "POST /r HTTP/1.1\r\nAccept-Encoding: identity\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\n\
             Host: h:1\r\nUser-Agent: k5/{v}\r\nX-K5-Cli-Version: {v}\r\n\
             X-K5-Cli-Via: native\r\nX-K5-Owner-Source: disk\r\nConnection: close\r\n\r\n"
        )
    );
    assert_eq!(
        Opener::from_client(&Client::new("h", 1, "/p", Auth::None)).token(),
        None
    );
    // The port as the shell typed it.
    let mut o = Opener::from_client(&Client::new("h", 4100, "/p", Auth::None));
    o.netloc = "h:04100".into();
    let text =
        String::from_utf8(o.encode(&Request::new(Method::Get, "/r", Duration::from_secs(1))))
            .expect("utf-8");
    assert!(text.contains("\r\nHost: h:04100\r\n"), "{text}");
    assert_eq!(o.base(), "http://h:04100");
}

#[test]
fn a_status_line_python_refuses() {
    let read = |b: &[u8]| read_status(&mut &b[..]);
    assert_eq!(read(b"HTTP/1.1 200 OK\r\n"), Ok(200));
    assert_eq!(read(b"HTTP/1.0 404\r\n"), Ok(404));
    let bad = read(b"garbage\r\n").expect_err("not HTTP");
    assert_eq!(
        (bad.class, bad.text()),
        (Class::BadStatusLine, "garbage\r\n".to_string())
    );
    assert_eq!(
        read(b"HTTP/1.1 99 X\r\n").expect_err("too low").class,
        Class::BadStatusLine
    );
    let none = read(b"").expect_err("nothing");
    assert_eq!(
        (none.class, none.kind, none.text().as_str()),
        (
            Class::RemoteDisconnected,
            Kind::Dropped,
            "Remote end closed connection without response"
        )
    );
}

#[test]
fn python_s_int() {
    assert_eq!(py_int(" 12 "), Some(12));
    assert_eq!(py_int("+1_0"), Some(10));
    for bad in ["", "1__0", "_1", "1_", "x", "0x10", "1 2"] {
        assert_eq!(py_int(bad), None, "{bad:?}");
    }
}

#[test]
fn answers_and_http_errors() {
    let (port, log) = server(vec![
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}"),
        Act::Write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\n{}"),
        Act::Write(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\n\r\nto the end"),
        Act::Write(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n"),
    ]);
    let o = opener(port);
    assert_eq!(
        o.open(&get("/a")),
        Outcome::Answer {
            status: 200,
            body: Ok(b"{\"ok\":true}".to_vec())
        }
    );
    assert_eq!(
        o.open(&get("/b")),
        Outcome::HttpError {
            code: 404,
            body: Ok(b"{}".to_vec())
        }
    );
    assert_eq!(
        o.open(&get("/c")),
        Outcome::Answer {
            status: 201,
            body: Ok(b"to the end".to_vec())
        }
    );
    assert_eq!(
        o.open(&get("/d")),
        Outcome::Answer {
            status: 200,
            body: Ok(b"abcde".to_vec())
        }
    );
    assert_eq!(log.lock().expect("lock").len(), 4);
}

#[test]
fn a_body_cut_short_is_incomplete_read() {
    let (port, _) = server(vec![
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"),
        Act::Write(b"HTTP/1.1 500 X\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n9\r\nde"),
        Act::Write(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"),
    ]);
    let o = opener(port);
    let want = |status: u16, text: &str| (status, text.to_string(), Kind::Dropped);
    let got = |out: Outcome| match out {
        Outcome::Answer {
            status,
            body: Err(r),
        }
        | Outcome::HttpError {
            code: status,
            body: Err(r),
        } => {
            assert_eq!(r.class, Class::IncompleteRead);
            (status, r.text(), r.kind)
        }
        other => panic!("not cut short: {other:?}"),
    };
    assert_eq!(
        got(o.open(&get("/a"))),
        want(200, "IncompleteRead(3 bytes read, 7 more expected)")
    );
    assert_eq!(
        got(o.open(&get("/b"))),
        want(500, "IncompleteRead(3 bytes read)")
    );
    assert_eq!(
        got(o.open(&get("/c"))),
        want(200, "IncompleteRead(0 bytes read)")
    );
}

#[test]
fn no_answer_is_python_s_exception() {
    let (port, _) = server(vec![
        Act::Write(b""),
        Act::Write(b"garbage\r\n\r\n"),
        Act::Wait(Duration::from_secs(3)),
    ]);
    let o = opener(port);
    let raised = |out: Outcome| match out {
        Outcome::Raised(r) => r,
        other => panic!("not raised: {other:?}"),
    };
    let r = raised(o.open(&get("/a")));
    assert_eq!(
        r.traceback_line(),
        "http.client.RemoteDisconnected: Remote end closed connection without response"
    );
    let r = raised(o.open(&get("/b")));
    assert_eq!(
        (r.class, r.kind, r.text()),
        (Class::BadStatusLine, Kind::Other, "garbage\r\n".into())
    );
    let mut slow = get("/c");
    slow.timeout = Duration::from_millis(300);
    let r = raised(o.open(&slow));
    assert_eq!(
        (r.class, r.kind, r.text()),
        (Class::TimeoutError, Kind::Timeout, "timed out".into())
    );
}

#[test]
fn a_refused_connect_is_url_error() {
    // Nothing listens on port 1 (see tests/common/stub.rs, dead_port).
    let r = match opener(1).open(&get("/a")) {
        Outcome::Raised(r) => r,
        other => panic!("not refused: {other:?}"),
    };
    let errno = if cfg!(target_os = "macos") { 61 } else { 111 };
    assert_eq!(r.kind, Kind::Refused);
    assert_eq!(
        r.reason_text(),
        format!("[Errno {errno}] Connection refused")
    );
    assert_eq!(
        r.text(),
        format!("<urlopen error [Errno {errno}] Connection refused>")
    );
    assert_eq!(
        r.traceback_line(),
        format!("urllib.error.URLError: <urlopen error [Errno {errno}] Connection refused>")
    );
}

#[test]
fn the_restart_loop() {
    let migrating: &[u8] = b"HTTP/1.1 503 X\r\nContent-Length: 20\r\n\r\n{\"state\":\"migrating\"}";
    let (port, log) = server(vec![
        Act::Write(migrating),
        Act::Write(migrating),
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"),
        // A dropped GET goes again once, then the drop stands.
        Act::Write(b""),
        Act::Write(b""),
        // A dropped POST is never sent again.
        Act::Write(b""),
        // Four 503s: the last is the answer.
        Act::Write(migrating),
        Act::Write(migrating),
        Act::Write(migrating),
        Act::Write(migrating),
    ]);
    let o = opener(port);
    let count = || log.lock().expect("lock").len();
    assert_eq!(
        o.open_retrying(&get("/a")),
        Outcome::Answer {
            status: 200,
            body: Ok(b"{}".to_vec())
        }
    );
    assert_eq!(count(), 3);
    assert!(matches!(o.open_retrying(&get("/b")), Outcome::Raised(r) if r.kind == Kind::Dropped));
    assert_eq!(count(), 5);
    let post = Request::new(Method::Post, "/c", Duration::from_secs(5)).data(b"{}".to_vec());
    assert!(matches!(o.open_retrying(&post), Outcome::Raised(r) if r.kind == Kind::Dropped));
    assert_eq!(count(), 6);
    assert!(matches!(
        o.open_retrying(&get("/d")),
        Outcome::HttpError { code: 503, .. }
    ));
    assert_eq!(count(), 10);
}

#[test]
fn a_refused_connect_is_tried_four_times() {
    let mut o = opener(1);
    o.pause = Duration::from_millis(50);
    let t0 = std::time::Instant::now();
    assert!(matches!(o.open_retrying(&get("/a")), Outcome::Raised(r) if r.kind == Kind::Refused));
    // Three pauses: four tries.
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(150), "{took:?}");
}

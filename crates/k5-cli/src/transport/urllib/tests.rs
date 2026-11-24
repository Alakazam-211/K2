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
    /// Reads the request, writes these bytes, waits, closes.
    WriteHold(&'static [u8], Duration),
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
                Act::WriteHold(bytes, d) => {
                    let _ = conn.write_all(bytes);
                    std::thread::sleep(d);
                }
            }
        }
    });
    (port, log)
}

/// The proxy variables the shell's Python reads, spelled so that only
/// the transport's own files name them in full (the reader test in
/// transport/client/tunnel.rs).
const HTTPS_PROXY: &str = concat!("https", "_proxy");
const NO_PROXY: &str = concat!("no", "_proxy");

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

/// `HOST=https://<name>`: TLS to the name, the `Host` header the URL's
/// netloc with no scheme (a family's own `HOST` text too). A URL the
/// shared transport doesn't read, or a proxy setting the shell's Python
/// reading doesn't take: nothing is sent at all, never a plain request in
/// its place.
#[test]
fn an_https_daemon() {
    let mut c = Client::new("box.example", 4711, "/p", Auth::Owner("t".into()));
    c.tls = true;
    let mut o = Opener::from_client(&c);
    assert!(o.tls && o.unsendable.is_none(), "{o:?}");
    assert_eq!(o.base(), "https://box.example:4711");
    for (netloc, want) in [
        ("box.example:4711", "box.example:4711"),
        ("https://box.example:4711", "box.example:4711"),
        ("HTTPS://box.example/:04711", "box.example:04711"),
    ] {
        o.netloc = netloc.into();
        assert_eq!(o.host_header(), want, "{netloc}");
    }
    // A plain client's netloc is written as it is.
    let mut plain = Opener::from_client(&Client::new("h", 1, "/p", Auth::None));
    plain.netloc = "https://h:1".into();
    assert!(!plain.tls);
    assert_eq!(plain.host_header(), "https://h:1");
    // The transport left a URL unsent only for a proxy setting curl's
    // reading doesn't take: the Python reading is this opener's own.
    let mut curl_left = Client::new("https://Box.example/", 4711, "/p", Auth::None);
    curl_left.tls = true;
    curl_left.unsendable = Some("https://Box.example:4711".into());
    let o = Opener::from_client(&curl_left);
    assert_eq!(
        (o.host.as_str(), o.port, o.unsendable.as_deref()),
        ("Box.example", 4711, None)
    );
    // Nothing sent: a URL the transport doesn't read, and a proxy URL the
    // binary doesn't read as Python does.
    let act = Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    let (port, log) = server(vec![act]);
    let mut unread = Client::new("127.0.0.1", port, "/p", Auth::Owner("t".into()));
    unread.tls = true;
    unread.unsendable = Some("https://[::1]:1".into());
    let mut socks = Opener::from_client(&Client::new("box.example", 1, "/p", Auth::None));
    socks.tls = true;
    socks.py_proxy =
        crate::transport::proxy::python::env([(HTTPS_PROXY, format!("socks5://127.0.0.1:{port}"))]);
    for o in [Opener::from_client(&unread), socks] {
        match o.open(&Request::new(Method::Get, "/r", Duration::from_secs(2))) {
            Outcome::Raised(r) => {
                assert_eq!((r.class, r.kind), (Class::UrlError, Kind::Other), "{r:?}");
                assert!(
                    r.text()
                        .starts_with("<urlopen error won't send to https://"),
                    "{r:?}"
                );
            }
            other => panic!("sent: {other:?}"),
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        log.lock().expect("log").is_empty(),
        "a request reached the server"
    );
}

/// Through the person's proxy as the shell's Python goes: `http.client`'s
/// CONNECT head (the proxy's own credential, never the token), and a
/// refusal raised as urllib raises it, once, nothing else sent. A proxy
/// that answers nothing, or not HTTP, is what `http.client` raises.
#[test]
fn an_https_daemon_through_the_proxy() {
    let proxied = |port: u16| {
        let mut c = Client::new("box.example", 4711, "/p", Auth::Owner("tok-9".into()));
        c.tls = true;
        let mut o = Opener::from_client(&c);
        o.py_proxy = crate::transport::proxy::python::env([(
            HTTPS_PROXY,
            format!("http://acme:pw@127.0.0.1:{port}"),
        )]);
        o
    };
    let req = Request::new(Method::Get, "/r", Duration::from_secs(5));
    for (answer, want) in [
        (
            &b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\n\r\n"[..],
            (
                Class::UrlError,
                "<urlopen error Tunnel connection failed: 407 Proxy Authentication Required>",
            ),
        ),
        (
            &b"HTTP/1.1 201 Created\r\n\r\n"[..],
            (
                Class::UrlError,
                "<urlopen error Tunnel connection failed: 201 Created>",
            ),
        ),
        (
            &b""[..],
            (
                Class::UrlError,
                "<urlopen error Remote end closed connection without response>",
            ),
        ),
        (
            &b"garbage\r\n\r\n"[..],
            (Class::BadStatusLine, "garbage\r\n"),
        ),
    ] {
        let (port, log) = server(vec![Act::Write(answer)]);
        match proxied(port).open(&req) {
            Outcome::Raised(r) => assert_eq!((r.class, r.text().as_str()), want, "{r:?}"),
            other => panic!("{other:?}"),
        }
        std::thread::sleep(Duration::from_millis(100));
        let got = log.lock().expect("log").clone();
        assert_eq!(
            got,
            [b"CONNECT box.example:4711 HTTP/1.1\r\nProxy-Authorization: Basic YWNtZTpwdw==\r\nHost: box.example:4711\r\n\r\n".to_vec()],
            "the proxy got the CONNECT head alone, once"
        );
    }
    // A proxy nothing listens on: the refused connect, as for the daemon.
    let dead = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = dead.local_addr().expect("addr").port();
    drop(dead);
    match proxied(port).open(&req) {
        Outcome::Raised(r) => {
            assert_eq!((r.class, r.kind), (Class::UrlError, Kind::Refused), "{r:?}");
        }
        other => panic!("{other:?}"),
    }
    // no_proxy names the host: straight there (nothing listens at
    // box.example, so the connect fails; the proxy gets nothing).
    let (port, log) = server(vec![]);
    let mut o = proxied(port);
    o.py_proxy = crate::transport::proxy::python::env([
        (HTTPS_PROXY, format!("http://127.0.0.1:{port}")),
        (NO_PROXY, "box.example".to_string()),
    ]);
    assert!(matches!(o.route(), Ok(None)), "{:?}", o.route());
    assert!(log.lock().expect("log").is_empty());
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
    let want = if cfg!(windows) {
        "[WinError 10061] No connection could be made because the target machine actively refused it.".to_string()
    } else {
        format!("[Errno {errno}] Connection refused")
    };
    assert_eq!(r.reason_text(), want);
    assert_eq!(r.text(), format!("<urlopen error {want}>"));
    assert_eq!(
        r.traceback_line(),
        format!("urllib.error.URLError: <urlopen error {want}>")
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

#[test]
fn a_request_with_its_own_headers_is_http_client_s() {
    let req = Request::new(
        Method::Post,
        "/base/auth/v1/token?grant_type=password",
        Duration::from_secs(30),
    )
    .data(b"{}".to_vec())
    .header("Apikey", "k")
    .header("User-Agent", "k2-cli/1.2.3")
    .header("Content-Type", "application/json");
    assert_eq!(
        String::from_utf8(encode_plain("db.example", &req)).unwrap(),
        "POST /base/auth/v1/token?grant_type=password HTTP/1.1\r\nAccept-Encoding: identity\r\nContent-Length: 2\r\nHost: db.example\r\nApikey: k\r\nUser-Agent: k2-cli/1.2.3\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}"
    );
    let get = Request::new(
        Method::Get,
        "/rest/v1/x?select=a,b",
        Duration::from_secs(30),
    )
    .header("Authorization", "Bearer t");
    assert_eq!(
        String::from_utf8(encode_plain("127.0.0.1:9", &get)).unwrap(),
        "GET /rest/v1/x?select=a,b HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: 127.0.0.1:9\r\nAuthorization: Bearer t\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn open_url_reads_a_plain_answer_and_names_a_failed_handshake() {
    let (port, log) = server(vec![
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]"),
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]"),
    ]);
    let place = |tls| Place {
        host: "127.0.0.1".into(),
        port,
        netloc: format!("127.0.0.1:{port}"),
        tls,
    };
    let req = Request::new(Method::Get, "/x", Duration::from_secs(5));
    assert_eq!(
        open_url(&place(false), &req),
        Outcome::Answer {
            status: 200,
            body: Ok(b"[]".to_vec())
        }
    );
    // A server that speaks no TLS (it waits for an HTTP head that never
    // comes): a URLError, and no request sent.
    let short = Request::new(Method::Get, "/x", Duration::from_secs(1));
    match open_url(&place(true), &short) {
        Outcome::Raised(r) => {
            assert_eq!(r.class, Class::UrlError);
            assert!(r.text().starts_with("<urlopen error "), "{}", r.text());
        }
        other => panic!("{other:?}"),
    }
    let got = log.lock().unwrap();
    assert!(String::from_utf8_lossy(&got[0]).starts_with("GET /x HTTP/1.1\r\n"));
    // The handshake's bytes, if the server logged them yet, hold no request.
    for g in &got[1..] {
        assert!(!String::from_utf8_lossy(g).contains("GET /x"));
    }
}

/// The line `_K2NoRedirect` writes, with this build's product name.
fn refused(here: &str, code: u16, there: &str) -> Refused {
    let p = k2_client::product_name!();
    Refused {
        line: format!(
            "refused: {here} answered {code}, sending {p} to {there}; {p} does not follow a redirect with your token or password, so nothing was sent there"
        ),
    }
}

/// urllib's redirect handler and the rule's line, case by case: the
/// codes it takes, `Location` joined to the request's URL, user parts
/// off, the new URL quoted as ISO-8859-1 bytes, and what it hands back
/// as the 3xx itself.
#[test]
fn redirect_refusal_is_the_rule_s() {
    let here = "127.0.0.1:4100";
    let r = |code, to: Option<&str>| redirect_refusal("http://127.0.0.1:4100", code, to);
    for code in [301, 302, 303, 307, 308] {
        assert_eq!(
            r(code, Some("http://127.0.0.1:9/elsewhere")),
            Some(refused(here, code, "127.0.0.1:9")),
            "{code}"
        );
    }
    // Not a code the handler takes, or nothing to go to: urlopen's HTTPError.
    assert_eq!(r(300, Some("http://127.0.0.1:9/")), None);
    assert_eq!(r(304, Some("http://127.0.0.1:9/")), None);
    assert_eq!(r(302, None), None);
    // A scheme it won't follow is its own HTTPError.
    assert_eq!(r(302, Some("mailto:a@x.example")), None);
    assert_eq!(r(302, Some("file:///etc/passwd")), None);
    // https, ftp, and the scheme in any case.
    assert_eq!(
        r(307, Some("https://h.example:8443/x")),
        Some(refused(here, 307, "h.example:8443"))
    );
    assert_eq!(
        r(302, Some("FTP://h.example/x")),
        Some(refused(here, 302, "h.example"))
    );
    // Relative: the request's own place.
    assert_eq!(r(303, Some("/moved?x=1")), Some(refused(here, 303, here)));
    assert_eq!(r(302, Some("")), Some(refused(here, 302, here)));
    assert_eq!(r(302, Some("http:/x")), Some(refused(here, 302, here)));
    // Scheme-relative takes its netloc.
    assert_eq!(
        r(302, Some("//other.example:1/p")),
        Some(refused(here, 302, "other.example:1"))
    );
    // A user part is never shown, on either side.
    assert_eq!(
        redirect_refusal(
            "http://u@127.0.0.1:4100",
            302,
            Some("http://user:pw@h.example/")
        ),
        Some(refused(here, 302, "h.example"))
    );
    // Quoted as ISO-8859-1 bytes: a space and a byte past ASCII as %XX
    // (the header was read as Latin-1, so `é` sent as UTF-8 is two).
    assert_eq!(
        r(302, Some("http://a b\u{c3}\u{a9}.example/")),
        Some(refused(here, 302, "a%20b%C3%A9.example"))
    );
    // Another scheme with no netloc: the whole new URL; the same scheme
    // as the request's (https to a remote K2): the request's place.
    assert_eq!(
        r(302, Some("https:no-netloc")),
        Some(refused(here, 302, "https:no-netloc"))
    );
    assert_eq!(
        redirect_refusal("https://k2.example:4100", 302, Some("https:/x")),
        Some(refused("k2.example:4100", 302, "k2.example:4100"))
    );
}

/// The ruled opener refuses a redirect before its body (a server that
/// holds the connection open after the head doesn't hold it up), never
/// connects to the place it names, and leaves every other answer as
/// `open` gives it. `open` still hands the 3xx back.
#[test]
fn the_ruled_opener_refuses_a_redirect_and_sends_nothing_there() {
    let there = TcpListener::bind("127.0.0.1:0").expect("bind");
    let tp = there.local_addr().expect("addr").port();
    there.set_nonblocking(true).expect("nonblocking");
    let leak = |s: String| -> &'static [u8] { Box::leak(s.into_boxed_str()).as_bytes() };
    let redirect = leak(format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{tp}/x\r\nContent-Length: 100\r\n\r\n"
    ));
    let uri = leak(format!(
        "HTTP/1.1 301 Moved\r\nURI: http://127.0.0.1:{tp}/y\r\nContent-Length: 0\r\n\r\n"
    ));
    let (port, log) = server(vec![
        Act::WriteHold(redirect, Duration::from_secs(3)),
        Act::Write(redirect),
        Act::Write(uri),
        Act::Write(b"HTTP/1.1 302 Found\r\nContent-Length: 2\r\n\r\n{}"),
        Act::Write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"),
        Act::Write(redirect),
    ]);
    let o = opener(port);
    let here = format!("127.0.0.1:{port}");
    let t0 = std::time::Instant::now();
    assert_eq!(
        o.open_ruled(&get("/a")),
        Err(refused(&here, 302, &format!("127.0.0.1:{tp}")))
    );
    // The body (100 bytes promised, the connection held) was never
    // waited for.
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    // The retrying loop ends at once on it: one request.
    assert_eq!(
        o.open_retrying_ruled(&get("/b")),
        Err(refused(&here, 302, &format!("127.0.0.1:{tp}")))
    );
    assert_eq!(log.lock().expect("lock").len(), 2);
    // `URI` when there is no `Location`.
    assert_eq!(
        o.open_ruled(&get("/c")),
        Err(refused(&here, 301, &format!("127.0.0.1:{tp}")))
    );
    // A 3xx with nowhere to go, and a 200: as `open` gives them.
    assert_eq!(
        o.open_ruled(&get("/d")),
        Ok(Outcome::HttpError {
            code: 302,
            body: Ok(b"{}".to_vec())
        })
    );
    assert_eq!(
        o.open_ruled(&get("/e")),
        Ok(Outcome::Answer {
            status: 200,
            body: Ok(b"{}".to_vec())
        })
    );
    // The plain opener still hands a redirect back (its body cut short).
    assert!(matches!(
        o.open(&get("/f")),
        Outcome::HttpError {
            code: 302,
            body: Err(_)
        }
    ));
    assert_eq!(log.lock().expect("lock").len(), 6);
    std::thread::sleep(Duration::from_millis(200));
    match there.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok((_, from)) => panic!("the redirect was followed: a connection from {from}"),
        Err(e) => panic!("accept: {e}"),
    }
}

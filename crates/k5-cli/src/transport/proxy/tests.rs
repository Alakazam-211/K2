use super::*;
use std::collections::HashMap;
use std::net::TcpListener;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let m: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |n: &str| m.get(n).cloned()
}

/// The proxy [`decide`] picked, as `scheme://host:port`, or the exit code
/// curl gives for a setting it refuses.
fn shown(v: Option<Via>) -> Option<Result<String, i32>> {
    v.map(|v| v.map(|p| p.to_string()).map_err(|e| e.curl_exit_code()))
}

#[test]
fn base64_is_the_standard_alphabet_with_padding() {
    for (raw, want) in [
        (&b""[..], ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"\xff\xfe\xfd", "//79"),
    ] {
        assert_eq!(base64(raw), want);
    }
}

/// The shell's `_k2_direct_url` for `https://<host>`.
/// The host part of an `https://` URL, and whether the shell's
/// `_k2_direct_url` calls it this computer (no proxy). Read from the bash
/// (cli/src/cli-tools-policy.sh), and checked against it below.
const LOOPBACK_ROWS: &[(&str, bool)] = &[
    ("localhost", true),
    ("LOCALHOST", true),
    ("localhost.", true),
    ("LocalHost.", true),
    ("localhost:8443", true),
    ("LOCALHOST.:8443", true),
    ("localhost#x", true),
    ("a.localhost", true),
    ("A.LOCALHOST.", true),
    (".localhost", true),
    ("127.0.0.1", true),
    ("127.9.9.9", true),
    ("127.255.255.255", true),
    ("127.0.0.1.", true),
    ("127.0.0.1:8443", true),
    ("127.0.0.1#x", true),
    ("[::1]", true),
    ("[::1]:8443", true),
    // A 127 address counts only as four numbers: these are names.
    ("127.x", false),
    ("127.evil.example", false),
    ("127.0.0.1.example", false),
    ("127.01.0.1", false),
    ("127.256.0.1", false),
    ("127.0.0", false),
    ("127.0.0.1.#x", false),
    ("box.example", false),
    ("10.0.0.5", false),
    ("localhost.example", false),
    ("128.0.0.1", false),
    ("localhost..", false),
    ("x.localhost..", false),
    ("a.localhost#x", false),
    ("", false),
    // IPv6: only [::1] as written. A bare ::1, an IPv4-mapped address
    // and any other spelling of ::1 are cut at their first ':'.
    ("::1", false),
    ("::ffff:127.0.0.1", false),
    ("[::ffff:127.0.0.1]", false),
    ("[::ffff:127.0.0.1]:8443", false),
    ("[::ffff:7f00:1]", false),
    ("[0:0:0:0:0:0:0:1]", false),
    ("[::1]x", false),
    ("[::1].", false),
];

#[test]
fn loopback_is_the_shells_direct_rule() {
    for &(h, want) in LOOPBACK_ROWS {
        assert_eq!(loopback(h), want, "{h}");
    }
}

/// The shell's own `_k2_direct_url` (and the `_k2_url_is_literal_loopback`
/// it calls), cut from cli/src/cli-tools-policy.sh and run by bash in the
/// C locale, answers every row as the table does: the binary and the
/// shell can't drift apart.
#[cfg(unix)]
#[test]
fn the_shells_direct_url_answers_the_table() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh");
    let src = std::fs::read_to_string(&path).expect("cli/src/cli-tools-policy.sh");
    let cut = |name: &str| {
        let head = format!("{name}() {{\n");
        let start = src
            .find(&head)
            .unwrap_or_else(|| panic!("{name} in {path:?}"));
        let end = src[start..].find("\n}\n").expect("the function's end") + start + 3;
        src[start..end].to_string()
    };
    let script = format!(
        "set -u\n{}{}for h in \"$@\"; do if _k2_direct_url \"https://$h\"; then echo 1; else echo 0; fi; done\n",
        cut("_k2_url_is_literal_loopback"),
        cut("_k2_direct_url"),
    );
    let o = std::process::Command::new("bash")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("a PATH"))
        .env("LC_ALL", "C")
        .arg("-c")
        .arg(&script)
        .arg("bash")
        .args(LOOPBACK_ROWS.iter().map(|(h, _)| *h))
        .output()
        .expect("run bash");
    assert!(o.status.success(), "bash: {o:?}");
    let said: Vec<bool> = String::from_utf8(o.stdout)
        .expect("bash's answers")
        .lines()
        .map(|l| match l {
            "1" => true,
            "0" => false,
            other => panic!("bash printed {other:?}"),
        })
        .collect();
    assert_eq!(said.len(), LOOPBACK_ROWS.len(), "one answer per row");
    for (&(h, want), got) in LOOPBACK_ROWS.iter().zip(said) {
        assert_eq!(got, want, "the shell's _k2_direct_url for https://{h}");
    }
}

/// The route for a host: this computer and plain http never use a
/// proxy; otherwise curl's rule (`curl-proxy-env`, its own tables and its
/// curl oracle hold the detail), every scheme and form read by the
/// binary, and a setting curl refuses carried to the connect (never
/// direct).
#[test]
fn the_route_for_a_host() {
    let all = [
        ("http_proxy", "http://p.example:1"),
        ("HTTP_PROXY", "http://p.example:2"),
        ("https_proxy", "http://p.example:3"),
        ("HTTPS_PROXY", "http://p.example:4"),
        ("all_proxy", "http://p.example:5"),
        ("ALL_PROXY", "http://p.example:6"),
    ];
    for (host, https) in [
        ("127.0.0.1", true),
        ("localhost", true),
        ("box.example", false),
        ("10.0.0.5", false),
    ] {
        assert_eq!(decide(host, https, env(&all)), None, "{host}");
    }
    let ok = |s: &str| Some(Ok(s.to_string()));
    assert_eq!(
        shown(decide("box.example", true, env(&all))),
        ok("http://p.example:3")
    );
    assert_eq!(decide("box.example", true, env(&[])), None);
    assert_eq!(
        decide("box.example", true, env(&[("HTTP_PROXY", "p.example")])),
        None
    );
    let named = [
        ("https_proxy", "p.example:3128"),
        ("no_proxy", "box.example"),
    ];
    assert_eq!(decide("box.example", true, env(&named)), None);
    assert_eq!(decide("a.box.example", true, env(&named)), None);
    assert_eq!(
        shown(decide("other.example", true, env(&named))),
        ok("http://p.example:3128")
    );
    let star = [("https_proxy", "p.example"), ("NO_PROXY", "*")];
    assert_eq!(decide("box.example", true, env(&star)), None);
    for (setting, want) in [
        ("https://p.example", ok("https://p.example:443")),
        ("socks5://p.example", ok("socks5://p.example:1080")),
        ("socks5h://u:pw@p.example:9", ok("socks5h://p.example:9")),
        ("socks4a://p.example", ok("socks4a://p.example:1080")),
        ("[::1]:3128", ok("http://[::1]:3128")),
        ("u@p.example:3", ok("http://p.example:3")),
        ("ftp://p.example", Some(Err(7))),
        ("http://p.example:99999", Some(Err(5))),
    ] {
        assert_eq!(
            shown(decide(
                "box.example",
                true,
                env(&[("https_proxy", setting)])
            )),
            want,
            "{setting}"
        );
    }
    // A target with a capital letter or a trailing dot, a no_proxy with
    // blanks: the binary's too.
    let p = [("https_proxy", "p.example")];
    assert_eq!(
        shown(decide("Box.example", true, env(&p))),
        ok("http://p.example:1080")
    );
    assert_eq!(
        shown(decide("box.example.", true, env(&p))),
        ok("http://p.example:1080")
    );
    let blanks = [
        ("https_proxy", "p.example"),
        ("no_proxy", "a.example box.example"),
    ];
    assert_eq!(decide("box.example", true, env(&blanks)), None);
}

/// A proxy for one connection: records what came before the head's end,
/// answers `answer`, then echoes what follows (the tunnel).
fn fake_proxy(answer: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut got = Vec::new();
        let mut b = [0u8; 1];
        while !got.ends_with(b"\r\n\r\n") {
            match s.read(&mut b) {
                Ok(0) | Err(_) => return got,
                Ok(_) => got.push(b[0]),
            }
        }
        if answer.is_empty() {
            return got;
        }
        s.write_all(answer).unwrap();
        let mut buf = [0u8; 64];
        if let Ok(n) = s.read(&mut buf) {
            let _ = s.write_all(&buf[..n]);
        }
        got
    });
    (port, h)
}

fn plain_tcp(host: &str, port: u16, _limit: Instant) -> Result<TcpStream, Failure> {
    TcpStream::connect((host, port)).map_err(Failure::Connect)
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn via(url: &str) -> Via {
    curl_proxy_env::ProxyUrl::parse(url.as_bytes())
}

#[test]
fn a_2xx_opens_the_tunnel_and_nothing_of_the_answer_is_left_behind() {
    let (port, h) = fake_proxy(b"HTTP/1.1 200 Connection established\r\n\r\n");
    let v = via(&format!("http://u:pw@127.0.0.1:{port}"));
    let mut s = through(&v, "box.example", 443, "k5/t", soon()).unwrap();
    s.write_all(b"hello").unwrap();
    let mut back = [0u8; 5];
    s.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"hello", "the tunnel carries bytes both ways");
    let head = String::from_utf8(h.join().unwrap()).unwrap();
    assert_eq!(
        head,
        "CONNECT box.example:443 HTTP/1.1\r\nHost: box.example:443\r\nProxy-Authorization: Basic dTpwdw==\r\nUser-Agent: k5/t\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
}

#[test]
fn anything_but_a_2xx_is_curl_56_with_nothing_sent() {
    for answer in [
        &b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n"[..],
        b"HTTP/1.1 403 Forbidden\r\n\r\n",
        b"HTTP/1.0 502 Bad Gateway\r\n\r\n",
        b"SSH-2.0-nope\r\n\r\n",
        b"",
    ] {
        let (port, h) = fake_proxy(answer);
        let v = via(&format!("http://127.0.0.1:{port}"));
        let Err(f) = through(&v, "box.example", 443, "k5/t", soon()) else {
            panic!("a tunnel through {answer:?}");
        };
        assert!(matches!(f, Failure::Tunnel(_)), "{f:?}");
        assert_eq!(f.curl_code(), curl::RECV_ERROR);
        assert!(!f.sent());
        h.join().unwrap();
    }
}

/// curl's codes for a proxy that fails, as `_k2_curl` reads them, and
/// nothing of the request sent: a refused connect is 7 (and tried again
/// as any is), a name that doesn't resolve 5, a SOCKS handshake that
/// fails 97, a setting curl refuses its 7 or 5 with no connection made.
#[test]
fn a_proxy_that_fails_is_curl_s_code() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    for url in [
        format!("http://127.0.0.1:{port}"),
        format!("socks5h://127.0.0.1:{port}"),
        format!("socks4a://127.0.0.1:{port}"),
    ] {
        let Err(f) = through(&via(&url), "box.example", 443, "k5/t", soon()) else {
            panic!("a tunnel through a closed port: {url}");
        };
        assert!(matches!(f, Failure::Connect(_)), "{url}: {f:?}");
        assert!(!f.sent());
    }
    let Err(f) = through(
        &via("http://proxy.example.invalid:3128"),
        "box.example",
        443,
        "k5/t",
        soon(),
    ) else {
        panic!("a tunnel through a proxy with no address");
    };
    assert_eq!(f.curl_code(), COULDNT_RESOLVE_PROXY, "{f:?}");
    assert!(!f.sent());
    // A proxy that takes the SOCKS5 greeting and closes.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let closer = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut greeting = [0u8; 3];
        s.read_exact(&mut greeting).unwrap();
        greeting
    });
    let Err(f) = through(
        &via(&format!("socks5h://127.0.0.1:{port}")),
        "box.example",
        443,
        "k5/t",
        soon(),
    ) else {
        panic!("a SOCKS5 handshake with an HTTP stub");
    };
    assert_eq!(f.curl_code(), PROXY_HANDSHAKE, "{f:?}");
    assert!(!f.sent());
    assert_eq!(closer.join().unwrap(), [5, 1, 0]);
    for (url, code) in [
        ("ftp://p.example", curl::COULDNT_CONNECT),
        ("http://p.example:99999", COULDNT_RESOLVE_PROXY),
    ] {
        let Err(f) = through(&via(url), "box.example", 443, "k5/t", soon()) else {
            panic!("{url} sent");
        };
        assert_eq!(f.curl_code(), code, "{url}: {f:?}");
        assert!(!f.sent());
    }
}

#[test]
fn a_proxy_that_never_answers_is_a_connect_timeout() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let held = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        std::thread::sleep(Duration::from_millis(800));
        drop(s);
    });
    let limit = Instant::now() + Duration::from_millis(300);
    let v = via(&format!("http://127.0.0.1:{port}"));
    let Err(f) = through(&v, "box.example", 443, "k5/t", limit) else {
        panic!("a tunnel through a silent proxy");
    };
    assert!(matches!(f, Failure::ConnectTimedOut), "{f:?}");
    held.join().unwrap();
}

/// The shell's Python reads the variables as urllib's
/// `getproxies_environment` does:
/// any spelling ending in `_proxy`, the lower-case one winning and an
/// empty lower-case one taking the key away; `all_proxy` never serves an
/// https request.
#[test]
fn python_reads_the_variables_as_urllib_does() {
    use python::Env;
    let ok = |v: &str| Some(Ok(v.to_string()));
    let rows: &[(&[(&str, &str)], Env)] = &[
        (&[], Env::default()),
        (
            &[("HTTPS_PROXY", "http://b:2"), ("ALL_PROXY", "http://c:3")],
            Env {
                https: ok("http://b:2"),
                no: None,
                any: true,
            },
        ),
        (
            &[("all_proxy", "http://c:3")],
            Env {
                https: None,
                no: None,
                any: true,
            },
        ),
        (
            &[("https_proxy", "http://a:1"), ("HTTPS_PROXY", "http://b:2")],
            Env {
                https: ok("http://a:1"),
                no: None,
                any: true,
            },
        ),
        (
            &[
                ("Https_Proxy", "http://a:1"),
                ("HTTPS_PROXY", "http://b:2"),
                ("https_proxy", ""),
                ("NO_PROXY", "x"),
            ],
            Env {
                https: None,
                no: ok("x"),
                any: true,
            },
        ),
        (
            &[("HTTPS_proxy", "http://a:1"), ("https_PROXY", "http://b:2")],
            Env {
                https: ok("http://a:1"),
                no: None,
                any: true,
            },
        ),
        (
            &[("HTTPS_PROXY", ""), ("no_proxy", "a"), ("NO_PROXY", "b")],
            Env {
                https: None,
                no: ok("a"),
                any: true,
            },
        ),
        (&[("PROXY", "x"), ("_proxyx", "y")], Env::default()),
    ];
    for (vars, want) in rows {
        assert_eq!(&python::env(vars.iter().copied()), want, "{vars:?}");
    }
    let bad = python::env([(&b"https_proxy"[..], &b"http://\xff:1"[..])]);
    assert_eq!(bad.https, Some(Err(NotText)));
    assert_eq!(python::decide("h:1", &bad), Route::Shell);
    let other = python::env([(&b"x\xff_proxy"[..], &b"v"[..])]);
    assert_eq!((other.https, other.any), (None, true));
}

/// `proxy_bypass_environment`: the answers python3 gives for each row.
#[test]
fn python_no_proxy_matches_as_urllib_does() {
    for (host, no, want) in [
        ("box.example:4100", "*", true),
        ("box.example:4100", "box.example", true),
        ("box.example:4100", ".example", true),
        ("box.example:4100", "example", true),
        ("box.example:4100", "ample", false),
        ("box.example:4100", " Box.Example , other", true),
        ("box.example:4100", "box.example:4100", true),
        ("box.example:4100", "box.example:4200", false),
        ("Box.Example:4100", "..box.example", true),
        ("10.1.2.3:4100", "10.1.2.3", true),
        ("10.1.2.3:4100", "10.1.0.0/16", false),
        ("10.1.2.3:4100", "2.3", true),
        ("box.example:4100", "", false),
        ("box.example:4100", ",,", false),
        ("box.example:4100", "*,other", false),
        ("box.example:4100", "x.box.example", false),
        ("box.example:04100", "box.example:4100", false),
        ("box.example.:4100", ".", true),
    ] {
        assert_eq!(python::bypass(host, no), want, "{host:?} {no:?}");
    }
}

/// `_parse_proxy` and `HTTPSConnection`'s host and port, as python3 reads
/// each row; the user and password only when both are there.
#[test]
fn python_proxy_urls() {
    let auth = |u: &str, p: &str| {
        Some(Credentials {
            user: u.as_bytes().to_vec(),
            pass: p.as_bytes().to_vec(),
        })
    };
    let px = |host: &str, port: u16, auth: Option<Credentials>| {
        Some(Proxy {
            host: host.into(),
            port,
            auth,
        })
    };
    for (raw, want) in [
        ("http://proxy.example:3128", px("proxy.example", 3128, None)),
        ("http://proxy.example", px("proxy.example", 443, None)),
        ("proxy.example:8080", px("proxy.example", 8080, None)),
        ("proxy.example", px("proxy.example", 443, None)),
        (
            "http://u:p@proxy.example:3128/",
            px("proxy.example", 3128, auth("u", "p")),
        ),
        (
            "http://u@proxy.example:3128",
            px("proxy.example", 3128, None),
        ),
        (
            "http://:p@proxy.example:3128",
            px("proxy.example", 3128, None),
        ),
        (
            "http://u%40x:p%3Ab@proxy.example:3128",
            px("proxy.example", 3128, auth("u@x", "p:b")),
        ),
        (
            "u:p@proxy.example:3128",
            px("proxy.example", 3128, auth("u", "p")),
        ),
        ("http://[::1]:3128", px("::1", 3128, None)),
        ("http://proxy.example:/", px("proxy.example", 443, None)),
        ("HTTP://proxy.example:3128", px("proxy.example", 3128, None)),
        (
            "http://a@b@proxy.example:3128/x",
            px("proxy.example", 3128, None),
        ),
        (
            "http://proxy.example:3128/path?q",
            px("proxy.example", 3128, None),
        ),
        // What the binary leaves unsent.
        ("https://proxy.example:3128", None),
        ("socks5://proxy.example:1080", None),
        ("http:/proxy.example", None),
        ("http://proxy.example:99999", None),
        ("http://proxy.example:x", None),
        ("http://pro xy:1", None),
        ("http://", None),
    ] {
        assert_eq!(python::parse(raw), want, "{raw:?}");
    }
}

/// `k2_direct` first, then urllib's reading; `http.client`'s CONNECT head
/// carries the proxy's own credential and never anything else.
#[test]
fn python_route_and_connect_head() {
    for (host, direct) in [
        ("localhost", true),
        ("LOCALHOST.", true),
        ("a.localhost", true),
        ("127.0.0.9", true),
        ("127.example", false),
        ("box.example", false),
    ] {
        assert_eq!(python::direct(host), direct, "{host}");
    }
    let env = python::env([
        ("https_proxy", "http://u:p@proxy.example:3128"),
        ("no_proxy", "skip.example"),
    ]);
    assert_eq!(python::decide("skip.example:4100", &env), Route::Direct);
    let Route::Via(p) = python::decide("box.example:4100", &env) else {
        panic!("no proxy")
    };
    assert_eq!(
        String::from_utf8(python::connect_head(&p, "box.example", 4100)).unwrap(),
        "CONNECT box.example:4100 HTTP/1.1\r\nProxy-Authorization: Basic dTpw\r\nHost: box.example:4100\r\n\r\n"
    );
    let bare = Proxy {
        host: "proxy.example".into(),
        port: 443,
        auth: None,
    };
    assert_eq!(
        python::connect_head(&bare, "box.example", 4100),
        b"CONNECT box.example:4100 HTTP/1.1\r\nHost: box.example:4100\r\n\r\n"
    );
    assert_eq!(
        python::decide(
            "box.example:4100",
            &python::env([("https_proxy", "socks5://s:1")])
        ),
        Route::Shell
    );
}

/// The shared exchange with Python's rule: 200 alone opens it; a 2xx
/// that curl would take is a failed tunnel, with the answer handed back.
#[test]
fn a_tunnel_head_opens_on_the_callers_rule() {
    let p = |port| Proxy {
        host: "127.0.0.1".into(),
        port,
        auth: None,
    };
    let (port, sent) = fake_proxy(b"HTTP/1.1 201 Created\r\n\r\n");
    let head = b"CONNECT box.example:1 HTTP/1.1\r\nHost: box.example:1\r\n\r\n";
    match tunnel_head(&p(port), head, |c| c == 200, soon(), plain_tcp) {
        Err((Failure::Tunnel(_), got)) => assert_eq!(got, b"HTTP/1.1 201 Created\r\n\r\n"),
        other => panic!("{other:?}"),
    }
    assert_eq!(sent.join().unwrap(), head);
    let (port, sent) = fake_proxy(b"HTTP/1.1 200 OK\r\n\r\n");
    assert!(tunnel_head(&p(port), head, |c| c == 200, soon(), plain_tcp).is_ok());
    assert_eq!(sent.join().unwrap(), head);
}

/// A proxy URL with another scheme: its scheme, and the one line that
/// names it (never the rest of the URL, which may hold a password).
#[test]
fn python_other_scheme_line() {
    for (raw, want) in [
        ("socks5://u:secret@s:1", Some("socks5")),
        ("HTTPS://p:3128", Some("https")),
        ("http://p:3128", None),
        ("p:3128", None),
        ("u:p@p:3128", None),
        ("we ird://p", None),
    ] {
        assert_eq!(python::other_scheme(raw).as_deref(), want, "{raw}");
    }
    let env = python::env([("https_proxy", "socks5://u:secret@s:1")]);
    let line = python::unsupported_line(&env).expect("a line");
    assert_eq!(
        line,
        "Error: proxy socks5://\u{2026} is not supported: only http:// proxies are"
    );
    assert!(!line.contains("secret"));
    assert_eq!(
        python::unsupported_line(&python::env([("https_proxy", "http://p:1")])),
        None
    );
}

/// `http.client` names an IPv6 target in brackets on the CONNECT line
/// and bare in `Host` (Python 3.12's `_wrap_ipv6` and `set_tunnel`).
#[test]
fn python_connect_head_for_an_ipv6_target() {
    let p = Proxy {
        host: "proxy.example".into(),
        port: 3128,
        auth: None,
    };
    assert_eq!(
        python::connect_head(&p, "fd00::5", 4100),
        b"CONNECT [fd00::5]:4100 HTTP/1.1\r\nHost: fd00::5:4100\r\n\r\n"
    );
}

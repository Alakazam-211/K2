use super::*;
use std::collections::HashMap;
use std::net::TcpListener;

/// The variables set, whether the target is https, and the proxy
/// setting curl uses.
type EnvRow = (
    &'static [(&'static str, &'static str)],
    bool,
    Option<&'static str>,
);

/// A proxy URL as read: host, port, and whether it has a user part.
type Parsed = (String, u16, bool);

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Lookup {
    let m: HashMap<String, String> = pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |n: &str| Ok(m.get(n).cloned())
}

fn via(host: &str, port: u16) -> Route {
    Route::Via(Proxy {
        host: host.into(),
        port,
        auth: None,
    })
}

/// curl's `detect_proxy` order, and an empty value counts as unset.
#[test]
fn which_variable_names_the_proxy() {
    let rows: &[EnvRow] = &[
        (
            &[
                ("https_proxy", "a"),
                ("HTTPS_PROXY", "b"),
                ("all_proxy", "c"),
            ],
            true,
            Some("a"),
        ),
        (&[("HTTPS_PROXY", "b"), ("all_proxy", "c")], true, Some("b")),
        (
            &[("https_proxy", ""), ("HTTPS_PROXY", "b")],
            true,
            Some("b"),
        ),
        (&[("all_proxy", "c"), ("ALL_PROXY", "d")], true, Some("c")),
        (&[("all_proxy", ""), ("ALL_PROXY", "d")], true, Some("d")),
        (&[("http_proxy", "e"), ("HTTP_PROXY", "f")], true, None),
        (
            &[("http_proxy", "e"), ("https_proxy", "a")],
            false,
            Some("e"),
        ),
        // HTTP_PROXY is never read (curl: the CGI `Proxy:` header).
        (&[("HTTP_PROXY", "f")], false, None),
        (&[("HTTP_PROXY", "f"), ("ALL_PROXY", "d")], false, Some("d")),
        (&[("https_proxy", "a")], false, None),
        (&[], true, None),
    ];
    for (vars, https, want) in rows {
        let got = proxy_setting(*https, &env(vars)).unwrap();
        assert_eq!(got.as_deref(), *want, "{vars:?} https={https}");
    }
    let e = env(&[("no_proxy", ""), ("NO_PROXY", "x")]);
    assert_eq!(no_proxy_setting(&e).unwrap().as_deref(), Some("x"));
    let e = env(&[("no_proxy", "y"), ("NO_PROXY", "x")]);
    assert_eq!(no_proxy_setting(&e).unwrap().as_deref(), Some("y"));
}

/// curl 8.5's `Curl_check_noproxy`, row by row.
#[test]
fn no_proxy_matches_as_curl_does() {
    use NoProxy::*;
    let rows = [
        ("box.example", "*", Skip),
        ("box.example", "box.example", Skip),
        ("box.example", "BOX.Example", Skip),
        ("box.example", ".box.example", Skip),
        ("box.example", "box.example.", Skip),
        ("box.example.", "box.example", Skip),
        ("a.box.example", "box.example", Skip),
        ("a.box.example", ".box.example", Skip),
        ("nobox.example", "box.example", Use),
        ("box.example", "a.box.example", Use),
        ("box.example", "other.example,box.example", Skip),
        ("box.example", ",,other.example,,", Use),
        ("box.example", "example", Skip),
        ("box.example", ".", Use),
        ("box.example", "*.example", Use),
        ("box.example", "*,x", Use),
        ("box.example", "other.example box.example", Shell),
        ("box.example", "box.example\tother", Shell),
        ("10.1.2.3", "10.1.2.3", Skip),
        ("10.1.2.3", "10.1.2.4", Use),
        ("10.1.2.3", "10.0.0.0/8", Skip),
        ("10.1.2.3", "10.1.0.0/16", Skip),
        ("10.1.2.3", "10.2.0.0/16", Use),
        ("10.1.2.3", "10.1.2.0/24x", Skip),
        ("10.1.2.3", "10.1.2.3/32", Skip),
        ("10.1.2.3", "10.1.2.0/0", Use),
        ("10.1.2.3", "10.1.2.0/33", Use),
        ("10.1.2.3", "10.1.2.0/-1", Use),
        ("10.1.2.3", "0.0.0.0/1", Skip),
        ("10.1.2.3", "box.example", Use),
        ("10.1.2.3", "2.3", Use),
    ];
    for (host, list, want) in rows {
        assert_eq!(no_proxy(host, list), want, "{host} in {list:?}");
    }
}

#[test]
fn proxy_urls_the_binary_reads_and_the_ones_it_leaves() {
    let p = |h: &str, port| Some((h.to_string(), port, false));
    let rows: &[(&str, Option<Parsed>)] = &[
        ("http://proxy.example:3128", p("proxy.example", 3128)),
        ("HTTP://proxy.example:3128/", p("proxy.example", 3128)),
        ("http://proxy.example", p("proxy.example", 1080)),
        ("proxy.example", p("proxy.example", 1080)),
        ("proxy.example:8080", p("proxy.example", 8080)),
        ("10.0.0.9:3128", p("10.0.0.9", 3128)),
        (
            "http://proxy.example:3128/any/path",
            p("proxy.example", 3128),
        ),
        (
            "http://u:p@proxy.example:3128",
            Some(("proxy.example".into(), 3128, true)),
        ),
        ("https://proxy.example:3128", None),
        ("socks5://proxy.example:1080", None),
        ("socks5h://proxy.example", None),
        ("ftp://proxy.example", None),
        ("http://[::1]:3128", None),
        ("[::1]:3128", None),
        ("ftp.example:3128", None),
        ("dict.example", None),
        ("LDAP.example", None),
        ("imap.example", None),
        ("smtp.example", None),
        ("pop3.example", None),
        ("u:p@proxy.example:3128", None),
        ("http://a@b@proxy.example", None),
        ("http://proxy.example:", None),
        ("http://proxy.example:0", None),
        ("http://proxy.example:99999", None),
        ("http://proxy.example:31x8", None),
        ("http://proxy.example?x", None),
        ("http://proxy.example#x", None),
        ("http://pro xy.example", None),
        ("http://proxy%2eexample", None),
        ("http://u:p%zz@proxy.example", None),
        ("http://u:p%0a@proxy.example", None),
        // The path starts at the `/`: the host would be `u`, with `p`
        // for a port.
        ("http://u:p/x@proxy.example", None),
        ("http://", None),
        ("", None),
    ];
    for (raw, want) in rows {
        let got = parse_proxy(raw).map(|p| (p.host, p.port, p.auth.is_some()));
        assert_eq!(&got, want, "{raw:?}");
    }
}

#[test]
fn credentials_are_percent_decoded_and_never_printed() {
    let p = parse_proxy("http://ac%40me:p%3As%2Fs%20w@proxy.example:3128").unwrap();
    let c = p.auth.clone().unwrap();
    assert_eq!(c.user, b"ac@me");
    assert_eq!(c.pass, b"p:s/s w");
    // base64("ac@me:p:s/s w")
    assert_eq!(c.basic(), "Basic YWNAbWU6cDpzL3Mgdw==");
    assert_eq!(format!("{c:?}"), "Credentials(<redacted>)");
    assert_eq!(format!("{c}"), "<redacted>");
    let shown = format!("{p:?} {:?}", Route::Via(p.clone()));
    for secret in ["ac@me", "ac%40me", "p:s", "YWNA", "s w"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    // A user with no password: an empty one, as curl sends it.
    let p = parse_proxy("http://only@proxy.example").unwrap();
    assert_eq!(
        p.auth.unwrap().basic(),
        format!("Basic {}", base64(b"only:"))
    );
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

#[test]
fn the_connect_head_is_curls() {
    let p = parse_proxy("http://u:pw@proxy.example:3128").unwrap();
    let head = connect_head(&p, "box.example", 443, "k5/1.2.3");
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "CONNECT box.example:443 HTTP/1.1\r\nHost: box.example:443\r\nProxy-Authorization: Basic dTpwdw==\r\nUser-Agent: k5/1.2.3\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    let p = parse_proxy("proxy.example").unwrap();
    let head = connect_head(&p, "10.0.0.5", 8443, "k5/1");
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "CONNECT 10.0.0.5:8443 HTTP/1.1\r\nHost: 10.0.0.5:8443\r\nUser-Agent: k5/1\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
}

/// The shell's `_k2_direct_url` for `https://<host>`.
#[test]
fn loopback_is_the_shells_direct_rule() {
    for h in [
        "localhost",
        "LOCALHOST",
        "localhost.",
        "a.localhost",
        "127.0.0.1",
        "127.9.9.9",
        "127.255.255.255",
        "127.0.0.1.",
        "[::1]",
    ] {
        assert!(loopback(h), "{h}");
    }
    // A 127 address counts only as four numbers: these are names.
    for h in [
        "127.x",
        "127.evil.example",
        "127.0.0.1.example",
        "127.01.0.1",
        "127.256.0.1",
        "127.0.0",
        "box.example",
        "10.0.0.5",
        "localhost.example",
        "128.0.0.1",
        "localhost..",
        "",
    ] {
        assert!(!loopback(h), "{h}");
    }
}

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
    // Loopback and plain http never use a proxy, whatever is set.
    for (host, https) in [
        ("127.0.0.1", true),
        ("localhost", true),
        ("box.example", false),
        ("10.0.0.5", false),
    ] {
        assert_eq!(decide(host, https, env(&all)), Route::Direct, "{host}");
    }
    assert_eq!(decide("box.example", true, env(&all)), via("p.example", 3));
    assert_eq!(decide("box.example", true, env(&[])), Route::Direct);
    assert_eq!(
        decide("box.example", true, env(&[("HTTP_PROXY", "p.example")])),
        Route::Direct
    );
    let named = [
        ("https_proxy", "p.example:3128"),
        ("no_proxy", "box.example"),
    ];
    assert_eq!(decide("box.example", true, env(&named)), Route::Direct);
    assert_eq!(decide("a.box.example", true, env(&named)), Route::Direct);
    assert_eq!(
        decide("other.example", true, env(&named)),
        via("p.example", 3128)
    );
    let star = [("https_proxy", "p.example"), ("NO_PROXY", "*")];
    assert_eq!(decide("box.example", true, env(&star)), Route::Direct);
    let blanks = [
        ("https_proxy", "p.example"),
        ("no_proxy", "a.example b.example"),
    ];
    assert_eq!(decide("box.example", true, env(&blanks)), Route::Shell);
    // No proxy at all: a no_proxy with blanks doesn't matter.
    let blanks = [("no_proxy", "a.example b.example")];
    assert_eq!(decide("box.example", true, env(&blanks)), Route::Direct);
    for odd in ["https://p.example", "socks5://p.example", "[::1]:3128"] {
        assert_eq!(
            decide("box.example", true, env(&[("https_proxy", odd)])),
            Route::Shell,
            "{odd}"
        );
    }
    let p = [("https_proxy", "p.example")];
    assert_eq!(decide("Box.example", true, env(&p)), Route::Shell);
    assert_eq!(decide("box.example.", true, env(&p)), Route::Shell);
    let not_text = |n: &str| -> Lookup {
        if n == "https_proxy" {
            Err(NotText)
        } else {
            Ok(None)
        }
    };
    assert_eq!(decide("box.example", true, not_text), Route::Shell);
    let not_text = |n: &str| -> Lookup {
        match n {
            "https_proxy" => Ok(Some("p.example".into())),
            "no_proxy" => Err(NotText),
            _ => Ok(None),
        }
    };
    assert_eq!(decide("box.example", true, not_text), Route::Shell);
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

#[test]
fn a_2xx_opens_the_tunnel_and_nothing_of_the_answer_is_left_behind() {
    let (port, h) = fake_proxy(b"HTTP/1.1 200 Connection established\r\n\r\n");
    let p = Proxy {
        host: "127.0.0.1".into(),
        port,
        auth: parse_proxy("http://u:pw@x.example").unwrap().auth,
    };
    let mut s = tunnel(&p, "box.example", 443, "k5/t", soon(), plain_tcp).unwrap();
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
    ] {
        let (port, h) = fake_proxy(answer);
        let p = Proxy {
            host: "127.0.0.1".into(),
            port,
            auth: None,
        };
        let Err(f) = tunnel(&p, "box.example", 443, "k5/t", soon(), plain_tcp) else {
            panic!("a tunnel through {answer:?}");
        };
        assert!(matches!(f, Failure::Tunnel(_)), "{f:?}");
        assert_eq!(f.curl_code(), curl::RECV_ERROR);
        assert!(!f.sent());
        h.join().unwrap();
    }
    // Closed with no answer.
    let (port, h) = fake_proxy(b"");
    let p = Proxy {
        host: "127.0.0.1".into(),
        port,
        auth: None,
    };
    let Err(f) = tunnel(&p, "box.example", 443, "k5/t", soon(), plain_tcp) else {
        panic!("a tunnel through a proxy that closed");
    };
    assert_eq!(f.curl_code(), curl::RECV_ERROR, "{f:?}");
    h.join().unwrap();
}

#[test]
fn a_proxy_that_is_down_is_curl_7_and_a_name_that_doesnt_resolve_curl_5() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let p = Proxy {
        host: "127.0.0.1".into(),
        port,
        auth: None,
    };
    let Err(f) = tunnel(&p, "box.example", 443, "k5/t", soon(), plain_tcp) else {
        panic!("a tunnel through a closed port");
    };
    assert_eq!(f.curl_code(), curl::COULDNT_CONNECT, "{f:?}");
    let unresolved = |_: &str, _: u16, _: Instant| -> Result<TcpStream, Failure> {
        Err(Failure::Broken {
            curl: curl::COULDNT_RESOLVE_HOST,
            why: "no such name".into(),
        })
    };
    let Err(f) = tunnel(&p, "box.example", 443, "k5/t", soon(), unresolved) else {
        panic!("a tunnel through a proxy with no address");
    };
    assert_eq!(f.curl_code(), COULDNT_RESOLVE_PROXY, "{f:?}");
    assert!(!f.sent());
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
    let p = Proxy {
        host: "127.0.0.1".into(),
        port,
        auth: None,
    };
    let limit = Instant::now() + Duration::from_millis(300);
    let Err(f) = tunnel(&p, "box.example", 443, "k5/t", limit, plain_tcp) else {
        panic!("a tunnel through a silent proxy");
    };
    assert!(matches!(f, Failure::ConnectTimedOut), "{f:?}");
    held.join().unwrap();
}

/// The shell's Python reads the variables as urllib's
/// `getproxies_environment` does (K2's ruling for the urllib families):
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

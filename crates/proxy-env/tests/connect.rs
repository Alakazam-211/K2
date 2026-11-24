//! `connect` against stub proxies on loopback: each kind of proxy opens a
//! tunnel that carries bytes, sends what its protocol says and nothing
//! of the caller's, and fails with curl's code (never going direct).

mod common;

#[cfg(feature = "rustls")]
use common::Pki;
use common::{closed_port, shown, Mode, Stub};
use proxy_env::{connect, Error, Options, ProxyStream, ProxyUrl};
use std::io::{Read, Write};
use std::time::Duration;

fn proxy(url: &str) -> ProxyUrl {
    ProxyUrl::parse(url.as_bytes()).unwrap()
}

fn opts() -> Options {
    Options::new()
        .user_agent("test/1")
        .timeout(Duration::from_secs(10))
}

/// `ping` through the tunnel gets the stub's `pong`.
fn ping(mut s: ProxyStream) {
    s.write_all(b"ping").unwrap();
    let mut b = [0u8; 4];
    s.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"pong");
}

fn one(stub: &Stub) -> Vec<u8> {
    std::thread::sleep(Duration::from_millis(100));
    let mut got = stub.take();
    assert_eq!(got.len(), 1, "one connection: {got:?}");
    got.pop().unwrap()
}

#[test]
fn http_connect_opens_a_tunnel() {
    let stub = Stub::start(Mode::Http("200 Connection established"));
    let p = proxy(&format!("http://acme:s3cret@127.0.0.1:{}", stub.port()));
    let s = connect(Some(&p), "box.example.invalid", 8443, &opts()).unwrap();
    assert!(!s.is_tls());
    ping(s);
    let head = one(&stub);
    assert_eq!(
        shown(&head),
        "CONNECT box.example.invalid:8443 HTTP/1.1\\r\\nHost: box.example.invalid:8443\\r\\nProxy-Authorization: Basic YWNtZTpzM2NyZXQ=\\r\\nUser-Agent: test/1\\r\\nProxy-Connection: Keep-Alive\\r\\n\\r\\n"
    );
}

#[test]
fn a_refused_connect_is_curl_56_and_closed_is_56() {
    let stub = Stub::start(Mode::Http("407 Proxy Authentication Required"));
    let p = proxy(&format!("http://127.0.0.1:{}", stub.port()));
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert!(matches!(e, Error::TunnelRefused(407)), "{e:?}");
    assert_eq!(e.curl_exit_code(), 56);
    let stub = Stub::start(Mode::Raw);
    let p = proxy(&format!("http://127.0.0.1:{}", stub.port()));
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert!(matches!(e, Error::HttpProxy(_)), "{e:?}");
    assert_eq!(e.curl_exit_code(), 56);
}

#[cfg(feature = "rustls")]
#[test]
fn an_https_proxy_is_tls_with_its_certificate_checked() {
    let pki = Pki::new();
    let stub = Stub::start(Mode::https("200 OK", &pki));
    for host in ["127.0.0.1", "localhost"] {
        let p = proxy(&format!("https://u:p@{host}:{}", stub.port()));
        let s = connect(
            Some(&p),
            "box.example.invalid",
            443,
            &opts().proxy_tls(pki.client(true)),
        )
        .unwrap();
        assert!(s.is_tls());
        ping(s);
        let head = one(&stub);
        assert!(
            head.starts_with(b"CONNECT box.example.invalid:443 HTTP/1.1\r\n"),
            "{}",
            shown(&head)
        );
        assert!(
            head.windows(6).any(|w| w == b"Basic "),
            "the credentials, inside TLS: {}",
            shown(&head)
        );
    }
    // A certificate the CA signed for another name (`localhost` only,
    // the proxy reached as 127.0.0.1): the handshake fails, curl's 60,
    // and no byte of CONNECT went out. The check can't be turned off.
    let other = Pki::for_names(&["localhost"]);
    let named = Stub::start(Mode::https("200 OK", &other));
    let p = proxy(&format!("https://u:p@127.0.0.1:{}", named.port()));
    let e = connect(
        Some(&p),
        "box.example.invalid",
        443,
        &opts().proxy_tls(other.client(true)),
    )
    .unwrap_err();
    assert!(
        matches!(
            e,
            Error::Tls {
                certificate: true,
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(e.curl_exit_code(), 60);
    assert_eq!(one(&named), b"", "nothing decrypted reached the proxy");
    // A CA nothing trusts: curl's 60, and no byte of CONNECT went out.
    let p = proxy(&format!("https://u:p@127.0.0.1:{}", stub.port()));
    let e = connect(
        Some(&p),
        "box.example.invalid",
        443,
        &opts().proxy_tls(pki.client(false)),
    )
    .unwrap_err();
    assert_eq!(e.curl_exit_code(), 60, "{e:?}");
    assert_eq!(one(&stub), b"", "nothing decrypted reached the proxy");
    // No TLS config: refused before connecting.
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert!(matches!(e, Error::TlsNotConfigured), "{e:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert!(stub.take().is_empty());
}

#[test]
fn socks5_without_and_with_a_password() {
    let stub = Stub::start(Mode::Socks5(None, 0));
    let p = proxy(&format!("socks5h://127.0.0.1:{}", stub.port()));
    ping(connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap());
    let mut want = vec![5, 1, 0, 5, 1, 0, 3, 19];
    want.extend_from_slice(b"box.example.invalid");
    want.extend_from_slice(&443u16.to_be_bytes());
    assert_eq!(shown(&one(&stub)), shown(&want));
    // socks5:// sends the address, resolved here.
    let p = proxy(&format!("socks5://127.0.0.1:{}", stub.port()));
    ping(connect(Some(&p), "127.0.0.5", 8443, &opts()).unwrap());
    assert_eq!(one(&stub), [5, 1, 0, 5, 1, 0, 1, 127, 0, 0, 5, 0x20, 0xfb]);
    // A name that doesn't resolve here fails the handshake after the
    // greeting, as curl's does: 97, and no request went out.
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert!(matches!(e, Error::Socks(_)), "{e:?}");
    assert_eq!(e.curl_exit_code(), 97);
    assert_eq!(one(&stub), [5, 1, 0]);

    let stub = Stub::start(Mode::Socks5(Some((b"acme".to_vec(), b"pw".to_vec())), 0));
    let p = proxy(&format!("socks5h://acme:pw@127.0.0.1:{}", stub.port()));
    ping(connect(Some(&p), "[::1]", 443, &opts()).unwrap());
    let mut want = vec![5, 2, 0, 2, 1, 4];
    want.extend_from_slice(b"acme");
    want.push(2);
    want.extend_from_slice(b"pw");
    want.extend_from_slice(&[5, 1, 0, 4]);
    want.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
    want.extend_from_slice(&443u16.to_be_bytes());
    assert_eq!(shown(&one(&stub)), shown(&want));
    for url in [
        format!("socks5h://acme:wrong@127.0.0.1:{}", stub.port()),
        format!("socks5h://127.0.0.1:{}", stub.port()),
    ] {
        let e = connect(Some(&proxy(&url)), "box.example.invalid", 443, &opts()).unwrap_err();
        assert!(matches!(e, Error::Socks(_)), "{e:?}");
        assert_eq!(e.curl_exit_code(), 97);
        one(&stub);
    }
    let stub = Stub::start(Mode::Socks5(None, 5));
    let p = proxy(&format!("socks5h://127.0.0.1:{}", stub.port()));
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert_eq!(e.curl_exit_code(), 97, "{e:?}");
}

#[test]
fn socks4_and_socks4a() {
    let stub = Stub::start(Mode::Socks4(90));
    let p = proxy(&format!("socks4://someone@127.0.0.1:{}", stub.port()));
    ping(connect(Some(&p), "127.0.0.5", 8443, &opts()).unwrap());
    let mut want = vec![4, 1, 0x20, 0xfb, 127, 0, 0, 5];
    want.extend_from_slice(b"someone\0");
    assert_eq!(shown(&one(&stub)), shown(&want));
    let p = proxy(&format!("socks4a://127.0.0.1:{}", stub.port()));
    ping(connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap());
    let mut want = vec![4, 1, 1, 0xbb, 0, 0, 0, 1, 0];
    want.extend_from_slice(b"box.example.invalid\0");
    assert_eq!(shown(&one(&stub)), shown(&want));
    // SOCKS4a sends an address as written, for the proxy to read.
    ping(connect(Some(&p), "127.0.0.5", 443, &opts()).unwrap());
    let mut want = vec![4, 1, 1, 0xbb, 0, 0, 0, 1, 0];
    want.extend_from_slice(b"127.0.0.5\0");
    assert_eq!(shown(&one(&stub)), shown(&want));
    // SOCKS4 for a name that doesn't resolve: the proxy is connected,
    // nothing sent, 97.
    let p = proxy(&format!("socks4://127.0.0.1:{}", stub.port()));
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert_eq!(e.curl_exit_code(), 97, "{e:?}");
    assert_eq!(one(&stub), b"");
    let stub = Stub::start(Mode::Socks4(91));
    let p = proxy(&format!("socks4a://127.0.0.1:{}", stub.port()));
    let e = connect(Some(&p), "box.example.invalid", 443, &opts()).unwrap_err();
    assert_eq!(e.curl_exit_code(), 97, "{e:?}");
}

#[test]
fn a_proxy_that_fails_is_curl_s_code_never_a_direct_connection() {
    let p = proxy(&format!("http://127.0.0.1:{}", closed_port()));
    let e = connect(Some(&p), "127.0.0.1", 443, &opts()).unwrap_err();
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
    assert_eq!(e.curl_exit_code(), 7);
    let p = proxy("http://proxy.example.invalid:3128");
    let e = connect(Some(&p), "127.0.0.1", 443, &opts()).unwrap_err();
    assert_eq!(e.curl_exit_code(), 5, "{e:?}");
    let stub = Stub::start(Mode::Silent);
    for url in [
        format!("http://127.0.0.1:{}", stub.port()),
        format!("socks5h://127.0.0.1:{}", stub.port()),
    ] {
        let t = std::time::Instant::now();
        let e = connect(
            Some(&proxy(&url)),
            "box.example.invalid",
            443,
            &Options::new().timeout(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert!(matches!(e, Error::TimedOut), "{e:?}");
        assert_eq!(e.curl_exit_code(), 28);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }
}

#[test]
fn no_proxy_is_a_direct_connection() {
    let target = Stub::start(Mode::Raw);
    let s = connect(None, "127.0.0.1", target.port(), &opts()).unwrap();
    drop(s);
    one(&target);
    let e = connect(None, "box.example.invalid", 443, &opts()).unwrap_err();
    assert_eq!(e.curl_exit_code(), 6, "{e:?}");
}

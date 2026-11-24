use super::*;

#[test]
fn the_connect_head() {
    let p = ProxyUrl::parse(b"http://u:pw@proxy.example:3128").unwrap();
    let head = connect_head(&p, &Host::parse("box.example"), 443, Some("curl/8.5.0"));
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "CONNECT box.example:443 HTTP/1.1\r\nHost: box.example:443\r\nProxy-Authorization: Basic dTpwdw==\r\nUser-Agent: curl/8.5.0\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    let p = ProxyUrl::parse(b"proxy.example").unwrap();
    let head = connect_head(&p, &Host::parse("10.0.0.5"), 8443, None);
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "CONNECT 10.0.0.5:8443 HTTP/1.1\r\nHost: 10.0.0.5:8443\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    let head = connect_head(&p, &Host::parse("2001:db8::1"), 443, None);
    assert!(
        head.starts_with(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\nHost: [2001:db8::1]:443\r\n"),
        "{}",
        String::from_utf8_lossy(&head)
    );
}

#[test]
fn the_status_line() {
    for (head, want) in [
        (
            &b"HTTP/1.1 200 Connection established\r\n\r\n"[..],
            Some(200),
        ),
        (b"HTTP/1.0 204\r\n\r\n", Some(204)),
        (
            b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n",
            Some(407),
        ),
        (b"HTTP/2 200\r\n\r\n", None),
        (b"HTTP/1.1 20 x\r\n\r\n", None),
        (b"SSH-2.0-x\r\n\r\n", None),
    ] {
        assert_eq!(status(head), want, "{:?}", String::from_utf8_lossy(head));
    }
}

#[test]
fn socks5_reply_words() {
    assert_eq!(socks5_reply(2), "not allowed by the rules");
    assert_eq!(socks5_reply(42), "code 42");
}

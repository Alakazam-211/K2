use super::*;

/// What a URL reads as: scheme, host, port and the user part, or the
/// error. `tests/curl_oracle.rs` checks the same forms against curl.
#[test]
fn proxy_urls() {
    type Want = Result<
        (
            Scheme,
            &'static str,
            u16,
            Option<(&'static [u8], &'static [u8])>,
        ),
        ConfigError,
    >;
    let ok = |s, h, p| Ok((s, h, p, None));
    let bad = |why| Err(ConfigError::BadSyntax(why));
    let unsupported = |s: &str| Err(ConfigError::UnsupportedScheme(s.to_string()));
    let rows: &[(&[u8], Want)] = &[
        (
            b"http://proxy.example:3128",
            ok(Scheme::Http, "proxy.example", 3128),
        ),
        (
            b"HTTP://proxy.example:3128/",
            ok(Scheme::Http, "proxy.example", 3128),
        ),
        (
            b"http://proxy.example",
            ok(Scheme::Http, "proxy.example", 1080),
        ),
        (b"proxy.example", ok(Scheme::Http, "proxy.example", 1080)),
        (
            b"proxy.example:8080",
            ok(Scheme::Http, "proxy.example", 8080),
        ),
        (b"10.0.0.9:3128", ok(Scheme::Http, "10.0.0.9", 3128)),
        (
            b"http://proxy.example:3128/any/path",
            ok(Scheme::Http, "proxy.example", 3128),
        ),
        (
            b"http://proxy.example:3128/\xff",
            ok(Scheme::Http, "proxy.example", 3128),
        ),
        (
            b"http://proxy.example?x",
            ok(Scheme::Http, "proxy.example", 1080),
        ),
        (
            b"http://proxy.example#x",
            ok(Scheme::Http, "proxy.example", 1080),
        ),
        (
            b"http://proxy.example:",
            ok(Scheme::Http, "proxy.example", 1080),
        ),
        (
            b"http://proxy.example:0080",
            ok(Scheme::Http, "proxy.example", 80),
        ),
        (
            b"https://proxy.example",
            ok(Scheme::Https, "proxy.example", 443),
        ),
        (
            b"HTTPS://proxy.example:8443",
            ok(Scheme::Https, "proxy.example", 8443),
        ),
        (
            b"socks4://proxy.example",
            ok(Scheme::Socks4, "proxy.example", 1080),
        ),
        (
            b"socks://proxy.example",
            ok(Scheme::Socks4, "proxy.example", 1080),
        ),
        (
            b"socks4a://proxy.example:9",
            ok(Scheme::Socks4a, "proxy.example", 9),
        ),
        (
            b"socks5://proxy.example:1080",
            ok(Scheme::Socks5, "proxy.example", 1080),
        ),
        (
            b"SOCKS5H://proxy.example",
            ok(Scheme::Socks5h, "proxy.example", 1080),
        ),
        (b"http://[::1]:3128", ok(Scheme::Http, "[::1]", 3128)),
        (b"[::1]:3128", ok(Scheme::Http, "[::1]", 3128)),
        (
            b"socks5://[2001:db8::7]",
            ok(Scheme::Socks5, "[2001:db8::7]", 1080),
        ),
        (b"ftp://proxy.example", unsupported("ftp")),
        (b"Gopher://proxy.example", unsupported("gopher")),
        (b"ftp.example:3128", unsupported("ftp")),
        (b"dict.example", unsupported("dict")),
        (b"LDAP.example", unsupported("ldap")),
        (b"imap.example", unsupported("imap")),
        (b"smtp.example", unsupported("smtp")),
        (b"pop3.example", unsupported("pop3")),
        (b"http://ftp.example", ok(Scheme::Http, "ftp.example", 1080)),
        (
            b"http://proxy.example:0",
            ok(Scheme::Http, "proxy.example", 0),
        ),
        (b"http://proxy.example:99999", bad("a port out of range")),
        (b"http://proxy.example:65536", bad("a port out of range")),
        (
            b"http://proxy.example:31x8",
            bad("a port that is not a number"),
        ),
        (b"http://pro xy.example", bad("a blank or control byte")),
        (b"http://proxy.example:9\t", bad("a blank or control byte")),
        (
            b"http://proxy%2eexample",
            ok(Scheme::Http, "proxy.example", 1080),
        ),
        (b"http://127%2e0.0.1:9", ok(Scheme::Http, "127.0.0.1", 9)),
        (
            b"http://pro%20xy.example",
            bad("a byte a host name can't hold"),
        ),
        (b"http://a@b@proxy.example", bad("a second @")),
        (
            b"http://u:p%0a@proxy.example",
            Err(ConfigError::ControlInCredentials),
        ),
        (
            b"socks5://u%00:p@proxy.example",
            Err(ConfigError::ControlInCredentials),
        ),
        (
            b"http://u%7f@proxy.example",
            Err(ConfigError::ControlInCredentials),
        ),
        (
            b"http://pr\xc3\xb6xy.example",
            bad("a byte a host name can't hold"),
        ),
        (b"http://[::1", bad("an IPv6 address with no ]")),
        (b"http://[::1]x", bad("bytes after an IPv6 address")),
        (b"http://[nope]", bad("an IPv6 address that doesn't parse")),
        (b"http://", bad("no host")),
        (b"", bad("no host")),
        (b"http://:3128", bad("no host")),
        // The path starts at the `/`: the host would be `u`, with `p`
        // for a port.
        (
            b"http://u:p/x@proxy.example",
            bad("a port that is not a number"),
        ),
    ];
    for (raw, want) in rows {
        let got = ProxyUrl::parse(raw).map(|p| {
            let c = p
                .credentials()
                .map(|c| (c.user().to_vec(), c.password().to_vec()));
            (p.scheme(), p.host().to_string(), p.port(), c)
        });
        let want = want.clone().map(|(s, h, p, c)| {
            (
                s,
                h.to_string(),
                p,
                c.map(|(u, pw): (&[u8], &[u8])| (u.to_vec(), pw.to_vec())),
            )
        });
        assert_eq!(got, want, "{:?}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn user_parts_are_percent_decoded() {
    let rows: &[(&[u8], &[u8], &[u8])] = &[
        (b"http://u:p@proxy.example:3128", b"u", b"p"),
        (b"http://only@proxy.example", b"only", b""),
        (b"http://u:@proxy.example", b"u", b""),
        (b"http://:p@proxy.example", b"", b"p"),
        (
            b"http://ac%40me:p%3As%2Fs%20w@proxy.example",
            b"ac@me",
            b"p:s/s w",
        ),
        (b"http://u:a:b@proxy.example", b"u", b"a:b"),
        (b"http://u:p%zz@proxy.example", b"u", b"p%zz"),
        (b"http://u:p%4@proxy.example", b"u", b"p%4"),
        (b"user@127.0.0.1:9", b"user", b""),
        (b"http://u%41:p%2@proxy.example", b"uA", b"p%2"),
    ];
    for (raw, user, pass) in rows {
        let p = ProxyUrl::parse(raw).unwrap();
        let c = p.credentials().expect("a user part");
        assert_eq!(
            (c.user(), c.password()),
            (*user, *pass),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn credentials_are_never_printed() {
    let p = ProxyUrl::parse(b"http://ac%40me:p%3As%2Fs%20w@proxy.example:3128").unwrap();
    let c = p.credentials().unwrap();
    // base64("ac@me:p:s/s w")
    assert_eq!(c.basic(), "Basic YWNAbWU6cDpzL3Mgdw==");
    let shown = format!("{p:?} {p} {c:?}");
    for secret in ["ac@me", "ac%40me", "p:s", "YWNA", "s w"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    assert_eq!(p.to_string(), "http://proxy.example:3128");
    assert_eq!(format!("{c:?}"), "Credentials(<redacted>)");
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
fn hosts() {
    assert_eq!(Host::parse("[::1]"), Host::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(Host::parse("::1"), Host::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(
        Host::parse("10.0.0.1"),
        Host::V4(Ipv4Addr::new(10, 0, 0, 1))
    );
    assert_eq!(Host::parse("10.1"), Host::Name("10.1".into()));
    assert_eq!(
        Host::parse("Box.example."),
        Host::Name("Box.example.".into())
    );
    assert_eq!(Host::V6(Ipv6Addr::LOCALHOST).to_string(), "[::1]");
}

use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `cmd_connect_status` as a block.
fn status_fn() -> String {
    let text = source("connect-account.sh");
    let start = text
        .find("\ncmd_connect_status() {\n")
        .expect("cmd_connect_status");
    let body = &text[start..];
    body[..body.find("\n}\n").expect("its end") + 3].to_string()
}

/// Each line `run` follows, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let f = status_fn();
    for line in [
        r#"            *) echo "Usage: k2 connect status" >&2; return 2 ;;"#,
        r#"    if [ -f "$(_connect_account_file)" ]; then"#,
        r#"        _connect_account_read"#,
        r#"        acc_email="$_CONNECT_ACC_EMAIL""#,
        r#"    if [ -f "${K2_HOME}/tunnel.json" ]; then"#,
        r#"        { IFS= read -r -d '' sub && IFS= read -r -d '' token_set; } < <(python3 -c '"#,
        r#"sub = (d.get("subdomain") or "").strip().replace("\0", "")"#,
        r#"out = "%s\0%s\0" % (sub, "yes" if tok else "no")"#,
        r#"sys.stdout.buffer.write(out.encode("utf-8", "replace"))"#,
        r#"' "${K2_HOME}/tunnel.json" 2>/dev/null) || { sub=""; token_set="no"; }"#,
        r#"        _cli_use_healed_passport || true"#,
        r#"    if [ "$token_set" = "no" ] && _connect_require_daemon 2>/dev/null; then"#,
        r#"        token_set="$(cli_send cli_request "/cli/tunnel/config" | python3 -c '"#,
        r#"' 2>/dev/null)" || token_set="no""#,
        r#"    echo "K2 Connect account""#,
        r#"    echo "  Account session: $acc_present$([ -n "$acc_email" ] && echo "  ($acc_email)")""#,
        r#"        printf '  Paired subdomain: %s → https://%s.k2.dev\n' "$sub" "$sub""#,
        r#"        printf '%s\n' "  Paired subdomain: (none)""#,
        r#"    echo "  Tunnel token:     $token_set""#,
        r#"    if _connect_require_daemon 2>/dev/null; then"#,
        r#"        raw=$(cli_send cli_request "/cli/tunnel/status") || raw="""#,
        r#"' 2>/dev/null || echo "  Tunnel status:   (daemon unreachable)""#,
        r#"        echo "  Tunnel status:   (daemon not running — start with: k2 daemon start)""#,
        r#"    print("unknown (the K2 vault could not be read)")"#,
        r#"    print("  Tunnel status:   (unparseable daemon response)")"#,
        r#"    print("  Tunnel status:   (error: %s)" % (err or "unknown"))"#,
        r#"running = "yes" if d.get("running") else "no""#,
        r#"print("  Tunnel running:  %s" % running)"#,
        r#"    print("  Public URL:      %s" % (d.get("public_url") or "(assigned by server)"))"#,
        r#"    print("  State:           RELEASED — re-pair with: k2 connect login")"#,
        r#"elif d.get("enabled") is False:"#,
        r#"    print("  State:           DISABLED — re-enable with: k2 tunnel enable")"#,
        r#"    print("  State:           stopped")"#,
        r#"tok = (d.get("token") or "").strip()"#,
        r#"    d = json.loads(sys.stdin.read())"#,
        r#"if isinstance(d, dict) and d.get("tokenSet"):"#,
        r#"elif isinstance(d, dict) and d.get("tokenUnreadable"):"#,
        r#"    d = json.loads(raw) if raw.strip() else {}"#,
        r#"if not isinstance(d, dict) or d.get("error"):"#,
        r#"    err = d.get("error") if isinstance(d, dict) else None"#,
    ] {
        assert!(
            f.lines().any(|l| l == line),
            "cmd_connect_status no longer has {line:?}"
        );
    }
    let text = source("connect-account.sh");
    for line in [
        r#"_connect_account_file() { echo "${K2_HOME}/connect-account.json"; }"#,
        r#"    { IFS= read -r -d '' _CONNECT_ACC_ACCESS &&"#,
        r#"        IFS= read -r -d '' _CONNECT_ACC_REFRESH &&"#,
        r#"        IFS= read -r -d '' _CONNECT_ACC_EMAIL; } < <(python3 -c '"#,
        r#"    sys.exit(0)"#,
        r#"for key in ("access_token", "refresh_token", "email"):"#,
        r#"    value = (d.get(key) or "").strip().replace("\0", "")"#,
        r#"    sys.stdout.buffer.write(value.encode("utf-8", "replace") + b"\0")"#,
        r#"' "$(_connect_account_file)" 2>/dev/null) || true"#,
        r#"        if [ -z "${PORT:-}" ] && [ -f "${K2_HOME}/daemon.port" ]; then"#,
        r#"            PORT="$(cat "${K2_HOME}/daemon.port" 2>/dev/null)""#,
        r#"    if [ -z "${PORT:-}" ] || [ -z "${TOKEN:-}" ]; then"#,
        r#"        status)"#,
        r#"            cmd_connect_status "$@""#,
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "connect-account.sh no longer has {line:?}"
        );
    }
    // The gate is skipped for connect; the guard and the tool policy stay.
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains("\n[ \"${1:-}\" = \"connect\" ] && _skip_conn_gate=1\n"));
    assert!(policy.contains("        connect) echo \"connect-account\" ;;"));
    assert!(policy.contains("|publish|connect-account|sessions-spawn|"));
    let router = source("router.sh");
    assert!(router.contains(
        "    connect)\n        # Headless Connect account pairing (login → pick subdomain →\n        # tunnel bind/start). Nested Pro verbs live under `k2 publish`.\n        if _wants_help \"${@:2}\"; then\n            _connect_usage\n            exit 0\n        fi\n        shift; cmd_connect \"$@\"\n"
    ));
    // /cli/tunnel/ is not a cell-socket route.
    assert!(!crate::transport::client::uds_eligible(CONFIG));
    assert!(!crate::transport::client::uds_eligible(STATUS));
}

fn obj(text: &str) -> Option<Value> {
    Some(json::parse(text).expect("json"))
}

#[test]
fn the_account_s_email() {
    assert_eq!(account_email(&None), "");
    assert_eq!(
        account_email(&obj(
            r#"{"access_token":"a","refresh_token":"r","email":" me@x.example "}"#
        )),
        "me@x.example"
    );
    // No refresh token: an empty field, and the email is still the third.
    assert_eq!(
        account_email(&obj(r#"{"access_token":"a","email":"me"}"#)),
        "me"
    );
    // Not a dict, or a value that is not a string: Python stops there and
    // the shell's three reads stop with it.
    assert_eq!(account_email(&obj("[1]")), "");
    assert_eq!(
        account_email(&obj(
            r#"{"access_token":"a","refresh_token":5,"email":"me"}"#
        )),
        ""
    );
    assert_eq!(
        account_email(&obj(
            r#"{"access_token":"a","refresh_token":"r","email":7}"#
        )),
        ""
    );
    assert_eq!(
        account_email(&obj(
            r#"{"access_token":0,"refresh_token":[],"email":"me"}"#
        )),
        "me"
    );
    // A token with a newline is one field (0.46.17): the email stays.
    assert_eq!(
        account_email(&obj(
            r#"{"access_token":"a\nb","refresh_token":"r","email":"me"}"#
        )),
        "me"
    );
    // A NUL is dropped; half a surrogate pair is a `?`.
    assert_eq!(
        account_email(&obj(r#"{"access_token":"a\u0000","email":"m\u0000e"}"#)),
        "me"
    );
    assert_eq!(account_email(&obj(r#"{"email":"x\ud800y"}"#)), "x?y");
}

#[test]
fn the_pairing() {
    assert_eq!(tunnel(&None), (String::new(), false));
    assert_eq!(
        tunnel(&obj(r#"{"subdomain":" acme ","token":"t"}"#)),
        ("acme".into(), true)
    );
    assert_eq!(
        tunnel(&obj(r#"{"subdomain":"acme","token":"  "}"#)),
        ("acme".into(), false)
    );
    assert_eq!(tunnel(&obj("[]")), (String::new(), false));
    assert_eq!(
        tunnel(&obj(r#"{"subdomain":"acme","token":5}"#)),
        (String::new(), false)
    );
    // Read as data (0.46.17): quotes, `$`, a backslash, a tab and a word
    // that is not ASCII print as they are saved.
    for (raw, want) in [
        (r#"{"subdomain":"a b\"$x"}"#, "a b\"$x"),
        (r#"{"subdomain":"it's"}"#, "it's"),
        (r#"{"subdomain":"a\\b"}"#, "a\\b"),
        (r#"{"subdomain":"é"}"#, "é"),
        (r#"{"subdomain":"a\tb"}"#, "a\tb"),
        (r#"{"subdomain":"a\u0000b"}"#, "ab"),
        (r#"{"subdomain":"\ud800"}"#, "?"),
    ] {
        assert_eq!(tunnel(&obj(raw)), (want.to_string(), false), "{raw}");
    }
}

#[test]
fn the_daemon_s_token() {
    assert_eq!(token_flag(br#"{"tokenSet":true}"#), "yes");
    assert_eq!(
        token_flag(br#"{"tokenSet":false,"tokenUnreadable":1}"#),
        UNREADABLE
    );
    assert_eq!(token_flag(b"{}\n\n"), "no");
    assert_eq!(token_flag(b"[1]"), "no");
    assert_eq!(token_flag(b"x"), "no");
    assert_eq!(token_flag(b""), "no");
}

#[test]
fn the_tunnel_s_status() {
    assert_eq!(
        status_lines(b""),
        "  Tunnel running:  no\n  State:           stopped\n"
    );
    assert_eq!(
        status_lines(" \u{a0} ".as_bytes()),
        "  Tunnel running:  no\n  State:           stopped\n"
    );
    assert_eq!(
        status_lines(b"x"),
        "  Tunnel status:   (unparseable daemon response)\n"
    );
    assert_eq!(
        status_lines(b"[1]"),
        "  Tunnel status:   (error: unknown)\n"
    );
    assert_eq!(
        status_lines(br#"{"error":{"code":"x"}}"#),
        "  Tunnel status:   (error: {'code': 'x'})\n"
    );
    assert_eq!(
        status_lines(br#"{"running":true}"#),
        "  Tunnel running:  yes\n  Public URL:      (assigned by server)\n"
    );
    assert_eq!(
        status_lines(br#"{"running":1,"public_url":"https://acme.k2.dev"}"#),
        "  Tunnel running:  yes\n  Public URL:      https://acme.k2.dev\n"
    );
    assert_eq!(
        status_lines(br#"{"released":true,"enabled":false}"#),
        "  Tunnel running:  no\n  State:           RELEASED \u{2014} re-pair with: k2 connect login\n"
    );
    assert_eq!(
        status_lines(br#"{"enabled":false}"#),
        "  Tunnel running:  no\n  State:           DISABLED \u{2014} re-enable with: k2 tunnel enable\n"
    );
    assert_eq!(
        status_lines(br#"{"enabled":0}"#),
        "  Tunnel running:  no\n  State:           stopped\n"
    );
}

#[test]
fn the_four_lines() {
    let f = Facts {
        account: true,
        email: "me@x.example".into(),
        sub: "acme".into(),
        token_on_disk: false,
        daemon: None,
    };
    assert_eq!(
        account_lines(&f, "yes"),
        "K2 Connect account\n  Account session: yes  (me@x.example)\n  Paired subdomain: acme \u{2192} https://acme.k2.dev\n  Tunnel token:     yes\n"
    );
    let none = Facts {
        account: false,
        email: String::new(),
        sub: String::new(),
        ..f
    };
    assert_eq!(
        account_lines(&none, "no"),
        "K2 Connect account\n  Account session: no\n  Paired subdomain: (none)\n  Tunnel token:     no\n"
    );
}

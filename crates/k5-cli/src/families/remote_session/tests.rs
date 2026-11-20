use super::*;

fn script() -> String {
    let p =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/remote-session.sh");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The line `python3 -c` numbers `utcfromtimestamp` at, in the reader
/// whose first lines (after `-c '`) start with `starts`.
fn utc_line(starts: &str) -> u32 {
    let text = script();
    for block in text.split("python3 -c '").skip(1) {
        let body = &block[..block.find("\n'").unwrap_or(block.len())];
        if body.trim_start().starts_with(starts) {
            let at = body
                .split('\n')
                .position(|l| l.contains("utcfromtimestamp"))
                .expect("utcfromtimestamp in the reader");
            return at as u32 + 1;
        }
    }
    panic!("no reader starting {starts:?}");
}

#[test]
fn the_warning_s_lines_are_the_readers() {
    assert_eq!(
        STATUS_UTC_LINE,
        utc_line(
            "import json,sys,datetime\nd=json.loads(sys.stdin.read())\nprint(\"Remote Sessions"
        )
    );
    assert_eq!(
        GRANT_UTC_LINE,
        utc_line("import json,sys,datetime\nd=json.loads(sys.stdin.read())\ng = d.get(\"grant\")")
    );
    assert_eq!(
        GRANTS_UTC_LINE,
        utc_line("import json,sys,datetime\nd=json.loads(sys.stdin.read())\ngrants")
    );
}

#[test]
fn the_grants_width_line_is_the_readers() {
    let text = script();
    let block = text
        .split("python3 -c '")
        .find(|b| b.contains("w_st = max"))
        .expect("the grants reader");
    let at = block
        .split('\n')
        .position(|l| l.starts_with("w_st = max"))
        .unwrap();
    assert_eq!(GRANTS_WIDTH_LINE, at as u32 + 1);
}

#[test]
fn the_lines_are_the_shell_s() {
    let text = script();
    for l in [BOTH, NO_TOKEN, REVOKE_USAGE, WRITE_USAGE, READ_USAGE] {
        // In the shell's `echo "…"` a quote is `\"`.
        assert!(text.contains(&l.replace('"', "\\\"")), "{l:?}");
    }
    for code in [
        "REMOTE_SESSIONS_DISABLED",
        "NO_GRANT",
        "GRANT_EXPIRED",
        "GRANT_REVOKED",
        "gated",
    ] {
        let t = teach(code).unwrap();
        let quoted = t.replace('"', "\\\"");
        assert!(
            text.contains(&format!("\"{code}\": \"{quoted}\"")),
            "{code}: {t}"
        );
    }
}

#[test]
fn the_ttl_as_bash_reads_it() {
    assert_eq!(parse_ttl(""), Some(1800));
    assert_eq!(parse_ttl("30m"), Some(1800));
    assert_eq!(parse_ttl("1H"), Some(3600));
    assert_eq!(parse_ttl("90s"), Some(90));
    assert_eq!(parse_ttl("0042"), Some(42));
    for bad in ["h", "1mh", "1.5h", "-1", "1d", " 1"] {
        assert_eq!(parse_ttl(bad), None, "{bad}");
    }
    assert_eq!(parse_ttl("9223372036854775808"), Some(i64::MIN));
}

#[test]
fn the_words() {
    let p = |w: &[&str]| parse(w).0;
    assert_eq!(p(&[]), Form::Help);
    assert_eq!(p(&["status", "--json"]), Form::Status { json: true });
    assert!(matches!(p(&["status", "x"]), Form::Stop { page: true, .. }));
    assert!(matches!(
        p(&["enable", "x"]),
        Form::Stop { page: false, .. }
    ));
    assert_eq!(
        p(&["status", "--host"]),
        Form::Stop {
            lines: vec![],
            page: false,
            exit: Exit::Error
        }
    );
    assert_eq!(
        p(&["grant", "--ttl", "2h", "--label", " a ", "--scope", ""]),
        Form::Grant {
            scope: String::new(),
            ttl: 7200,
            label: " a ".into()
        }
    );
    assert!(matches!(p(&["grant", "--ttl", "x"]), Form::Stop { .. }));
    assert_eq!(p(&["revoke", "g1"]), Form::Revoke { id: "g1".into() });
    assert!(matches!(p(&["revoke", "g1", "g2"]), Form::Stop { .. }));
    assert_eq!(
        p(&["write", "s1", "ls -l", "--token-stdin"]),
        Form::Write {
            sid: "s1".into(),
            text: "ls -l".into()
        }
    );
    assert!(matches!(p(&["write", "s1"]), Form::Stop { .. }));
    assert_eq!(
        p(&["read", "s1", "--lines", "9"]),
        Form::Read {
            sid: "s1".into(),
            lines: "9".into()
        }
    );
    assert!(matches!(p(&["nope"]), Form::Stop { page: true, .. }));
    let (_, t) = parse(&["shell", "--token", "k2rs_x", "--host", "h"]);
    assert_eq!((t.token.as_str(), t.host.as_str()), ("k2rs_x", "h"));
}

#[test]
fn times_as_python_prints_them() {
    assert_eq!(civil(0).unwrap(), "1970-01-01 00:00:00Z");
    assert_eq!(civil(1_763_600_000).unwrap(), "2025-11-20 00:53:20Z");
    assert_eq!(civil(-1).unwrap(), "1969-12-31 23:59:59Z");
    assert_eq!(civil(253_402_300_800), None);
    let mut o = Out::default();
    let mut warned = false;
    assert_eq!(utc(&mut o, Some(&s("x")), 9, &mut warned), "x");
    assert!(o.stderr.is_empty());
    assert_eq!(
        utc(&mut o, Some(&Value::Float(1.9)), 9, &mut warned),
        "1970-01-01 00:00:01Z"
    );
    assert_eq!(
        utc(&mut o, Some(&s(" 2 ")), 9, &mut warned),
        "1970-01-01 00:00:02Z"
    );
    assert_eq!(o.stderr, format!("<string>:9: {UTC_WARNING}\n"));
}

fn shown(r: Shown) -> (String, String, Exit) {
    (r.stdout, r.stderr, r.exit)
}

#[test]
fn the_error_printer() {
    assert_eq!(shown(print_err(b" ")).2, Exit::Error);
    assert_eq!(shown(print_err(b"<x>")).1, "Error: <x>\n");
    assert_eq!(
        shown(print_err(b"[1]")).1,
        "Error: unexpected response: '[1]'\n"
    );
    assert_eq!(
        shown(print_err(
            br#"{"error":{"code":"REMOTE_SESSIONS_DISABLED"}}"#
        )),
        (
            String::new(),
            "Error: Remote Sessions are OFF. Enable with: k2 remote-session enable\n  \u{2192} k2 remote-session enable\n".into(),
            Exit::Refused
        )
    );
    assert_eq!(
        shown(print_err(br#"{"error":{"code":"REMOTE_SESSIONS_DISABLED","hint":"Off here"}}"#)).1,
        "Error: Off here\n  \u{2192} Remote Sessions are OFF. Enable with: k2 remote-session enable\n  \u{2192} k2 remote-session enable\n"
    );
    assert_eq!(
        shown(print_err(br#"{"ok":false,"code":"usage","message":"bad"}"#)),
        (String::new(), "Error: bad\n".into(), Exit::Usage)
    );
    assert_eq!(shown(print_err(br#"{"error":{}}"#)).1, "Error: {}\n");
    assert_eq!(
        shown(print_err(br#"{"error":"Invalid token"}"#)).2,
        Exit::Refused
    );
    assert!(shown(print_err(br#"{"error":{"code":5}}"#))
        .1
        .ends_with("AttributeError: 'int' object has no attribute 'strip'\n"));
}

#[test]
fn the_readers() {
    let d = json::parse(r#"{"ok":true,"enabled":true,"activeGrants":2,"activeSessions":[{"sessionId":"s1"},"x"],"recentDenials":[{"createdAt":60,"code":"NO_GRANT","payload":"p"},{"createdAt":"bad"}]}"#).unwrap();
    let mut o = Out::default();
    status_lines(&d, &mut o).unwrap();
    assert_eq!(
        o.stdout,
        "Remote Sessions: ON\nActive grants:   2\nActive shells:   2\n  session=s1  grant=?\nRecent denials (2):\n  [1970-01-01 00:01:00Z] NO_GRANT  ?  p\n  [bad] -  ?  \n"
    );
    assert_eq!(o.stderr, format!("<string>:19: {UTC_WARNING}\n"));
    let d = json::parse(r#"{"ok":true,"grants":[{"id":"g1","expiresAt":0,"label":"L"},{"id":"g22","revokedAt":5,"scope":"x"}]}"#).unwrap();
    let mut o = Out::default();
    let r = grants_lines(&d, &mut o).unwrap_err();
    assert_eq!(r.exception, "IndexError: tuple index out of range");
    assert_eq!(
        (o.stdout.as_str(), o.stderr),
        ("", format!("<string>:9: {UTC_WARNING}\n"))
    );
    let mut o = Out::default();
    grants_lines(&json::parse(r#"{"grants":[1]}"#).unwrap(), &mut o).unwrap();
    assert_eq!(o.stdout, "No grants.\n");
    assert_eq!(shown(write_reader(b"", "s")).0, "wrote to s\n");
    assert_eq!(shown(write_reader(b"plain", "s")).0, "plain\n");
    assert_eq!(shown(write_reader(br#"{"lines":[]}"#, "s")).0, "");
    assert_eq!(
        shown(write_reader(br#"{"success":false}"#, "s")).0,
        "wrote to s\n"
    );
    assert_eq!(
        shown(write_reader(br#"{"error":"nope"}"#, "s")).1,
        "Error: nope\n"
    );
    assert_eq!(shown(read_reader(br#"{"lines":["a",1]}"#)).0, "a\n1\n");
    assert_eq!(shown(read_reader(b"")).0, "");
    assert_eq!(shown(read_reader(br#"{"error":"x"}"#)).1, "Error: x\n");
}

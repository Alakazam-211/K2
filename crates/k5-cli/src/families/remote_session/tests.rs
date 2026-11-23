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
    for l in [
        BOTH,
        NO_TOKEN,
        TOKEN_FORMS,
        REVOKE_USAGE,
        WRITE_USAGE,
        READ_USAGE,
        TEXT_WAYS,
        KEEP_WORD,
        BOTH_STDIN,
    ] {
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
            lines: vec!["Error: --host takes a value".into()],
            page: false,
            exit: Exit::Usage
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
            text: Said::Word("ls -l".into()),
            keep: false
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

/// The shell's Python lines for `--token-file` and `write`'s text, as
/// [`token_file`] and [`said`] print them.
#[test]
fn the_secret_input_lines_are_the_shell_s() {
    let text = script();
    for l in [
        "sys.stderr.write(\"k2 remote-session: --token-file %s: %s\\n\" % (p, msg))",
        "die(\"its group or others can read it (mode %04o); run: chmod 600 %s\" % (mode & 0o777, p))",
        "if os.name == \"posix\" and mode & 0o044:",
        "tok = data.split(b\"\\n\", 1)[0].strip(b\" \\t\\r\\x0b\\x0c\")",
        "die(\"no token in the file\")",
        "sys.stderr.write(\"k2 remote-session write: --text-file %s: %s\\n\" % (p, e.strerror))",
        "sys.stderr.write(\"k2 remote-session write: --text-file %s: no text in the file\\n\" % p)",
        "    if data.endswith(b\"\\r\\n\"):\n        data = data[:-2]\n    elif data.endswith(b\"\\n\"):\n        data = data[:-1]",
        "sys.stderr.write(\"k2 remote-session write: -: no text on stdin\\n\")",
        "_cli_read_secret_stdin \"Text: \"",
        "raw=$(cli_post_form \"/cli/terminal/write\"",
    ] {
        assert!(text.contains(l), "{l}");
    }
}

#[test]
fn write_s_text_forms() {
    let p = |w: &[&str]| parse(w).0;
    let w = |text: Said| Form::Write {
        sid: "s1".into(),
        text,
        keep: false,
    };
    assert_eq!(p(&["write", "s1", "-"]), w(Said::Stdin));
    assert_eq!(
        p(&["write", "s1", "--text-file", "pw"]),
        w(Said::File("pw".into()))
    );
    // An empty word leaves its slot open, as the shell's `[ -z ]` does.
    assert_eq!(p(&["write", "s1", "", "x"]), w(Said::Word("x".into())));
    let stop = |l: &str| Form::Stop {
        lines: vec![l.to_string()],
        page: false,
        exit: Exit::Usage,
    };
    assert_eq!(p(&["write", "-"]), stop("Unknown flag: -"));
    assert_eq!(
        p(&["write", "s1", "x", "-"]),
        stop("Unexpected argument: -")
    );
    assert_eq!(
        p(&["write", "s1", "-", "x"]),
        stop("Unexpected argument: x")
    );
    assert_eq!(
        p(&["write", "s1", "-", "-"]),
        stop("Unexpected argument: -")
    );
    assert_eq!(
        p(&["write", "s1", "x", "--text-file", "f"]),
        stop(TEXT_WAYS)
    );
    assert_eq!(
        p(&["write", "s1", "-", "--text-file", "f"]),
        stop(TEXT_WAYS)
    );
    assert_eq!(p(&["write", "s1", "-", "--token-stdin"]), stop(BOTH_STDIN));
    assert_eq!(
        p(&["write", "s1", "--keep-newline", "-"]),
        Form::Write {
            sid: "s1".into(),
            text: Said::Stdin,
            keep: true
        }
    );
    assert_eq!(p(&["write", "s1", "x", "--keep-newline"]), stop(KEEP_WORD));
    assert_eq!(p(&["write", "s1", "--keep-newline"]), stop(WRITE_USAGE));
    // --keep-newline is write's only.
    assert!(matches!(
        p(&["status", "--keep-newline"]),
        Form::Stop { page: true, .. }
    ));
    assert_eq!(p(&["write", "s1", "--text-file", ""]), stop(WRITE_USAGE));
    assert_eq!(
        p(&["write", "s1", "-", "--text-file"]),
        stop("Error: --text-file takes a value")
    );
    // --text-file is write's only.
    assert!(matches!(
        p(&["status", "--text-file", "f"]),
        Form::Stop { page: true, .. }
    ));
    let (_, t) = parse(&["status", "--token-file", "tok"]);
    assert_eq!(t.file, "tok");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("k5-rs-secret-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(unix)]
fn file_with(dir: &std::path::Path, name: &str, body: &[u8], mode: u32) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    p.to_string_lossy().into_owned()
}

#[cfg(unix)]
#[test]
fn the_token_file_s_mode_and_line() {
    let d = scratch("token");
    for mode in [0o600, 0o400] {
        let p = file_with(&d, &format!("t{mode:o}"), b"  k2rs_tok \r\nsecond\n", mode);
        assert_eq!(token_file(&p), Ok("k2rs_tok".to_string()), "{mode:o}");
    }
    for mode in [0o644, 0o640, 0o604, 0o660, 0o606] {
        let p = file_with(&d, &format!("t{mode:o}"), b"k2rs_tok\n", mode);
        assert_eq!(token_file(&p), Err(Exit::Usage), "{mode:o}");
    }
    let p = file_with(&d, "blank", b" \t\n", 0o600);
    assert_eq!(token_file(&p), Err(Exit::Usage));
    let missing = d.join("missing").to_string_lossy().into_owned();
    assert_eq!(token_file(&missing), Err(Exit::Usage));
    let t = Target {
        token: "k2rs_argv".into(),
        file: file_with(&d, "both", b"k2rs_tok\n", 0o600),
        ..Target::default()
    };
    assert_eq!(apply_target(&t), Err(Exit::Usage));
    let t = Target {
        file: t.file.clone(),
        ..Target::default()
    };
    assert_eq!(apply_target(&t), Ok(Some("k2rs_tok".to_string())));
    std::fs::remove_dir_all(&d).unwrap();
}

#[cfg(unix)]
#[test]
fn a_text_file_is_sent_byte_for_byte() {
    let d = scratch("text");
    let body = b"pw \"q\" & = % + \xc3\xa9 \r\x00 end\n\n";
    let p = file_with(&d, "pw", body, 0o600);
    // By default one newline goes; of two, one stays.
    let got = said(&Said::File(p.clone()), false).unwrap();
    assert_eq!(got.expose(), &body[..body.len() - 1]);
    let got = said(&Said::File(p), true).unwrap();
    assert_eq!(got.expose(), body);
    let crlf = file_with(&d, "crlf", b"pw\r\n", 0o600);
    assert_eq!(said(&Said::File(crlf), false).unwrap().expose(), b"pw");
    let nl = file_with(&d, "nl", b"\n", 0o600);
    assert!(said(&Said::File(nl.clone()), false).is_err());
    assert_eq!(said(&Said::File(nl), true).unwrap().expose(), b"\n");
    let empty = file_with(&d, "empty", b"", 0o600);
    assert!(said(&Said::File(empty), true).is_err());
    let missing = d.join("missing").to_string_lossy().into_owned();
    assert!(said(&Said::File(missing), false).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn one_trailing_newline_comes_off() {
    let strip = |b: &[u8], keep: bool| {
        let mut v = b.to_vec();
        strip_newline(&mut v, keep);
        v
    };
    assert_eq!(strip(b"pw\n", false), b"pw");
    assert_eq!(strip(b"pw\n\n", false), b"pw\n");
    assert_eq!(strip(b"pw\r\n", false), b"pw");
    assert_eq!(strip(b"pw\r\n\r\n", false), b"pw\r\n");
    assert_eq!(strip(b"pw\r", false), b"pw\r");
    assert_eq!(strip(b"pw", false), b"pw");
    assert_eq!(strip(b"\n", false), b"");
    assert_eq!(strip(b"pw\r\n", true), b"pw\r\n");
}

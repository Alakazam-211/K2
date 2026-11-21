use super::*;

fn here() -> Here {
    Here {
        pwd: "/srv/proj".into(),
        stdin_tty: false,
        stdout_tty: false,
    }
}

fn stop_of(words: &[&str]) -> (Vec<String>, Exit) {
    match parse(words, &here()) {
        Some(Ask::Stop(s)) => (s.lines, s.exit),
        other => panic!("{words:?}: not a stop: {other:?}"),
    }
}

#[test]
fn workspace_alone_and_an_unknown_verb_are_the_usage_line() {
    assert_eq!(stop_of(&[]), (vec![USAGE.to_string()], Exit::Error));
    assert_eq!(stop_of(&["bogus"]), (vec![USAGE.to_string()], Exit::Error));
    assert!(USAGE.starts_with(concat!("Usage: ", cli_name!(), " workspace <list|")));
}

#[test]
fn the_shell_keeps_identity_help_and_apps() {
    for words in [
        &["set-agent-identity", "a"][..],
        &["api-guest-policy", "help"],
        &["mobile-tab", "help"],
        &["trust", "--help"],
    ] {
        assert_eq!(parse(words, &here()), None, "{words:?}");
    }
    // `trust` alone and `trust help` print the trust page here.
    for words in [&["trust"][..], &["trust", "help"], &["trust", ""]] {
        assert!(
            matches!(parse(words, &here()), Some(Ask::Page(p)) if p.contains("workspace trust")),
            "{words:?}"
        );
    }
    // --drop-access with another --apps is answered here, after the lookup.
    assert!(matches!(
        parse(&["remove", "x", "--apps", "park", "--drop-access"], &here()),
        Some(Ask::Other(other::Ask::Remove(other::Remove {
            drop_misused: true,
            ..
        })))
    ));
}

#[test]
fn remove_apps_releases_first_and_reads_json_on_the_line() {
    let Some(Ask::Other(o)) = parse(
        &["remove", "x", "--mode", "--json", "--apps", "park"],
        &here(),
    ) else {
        panic!("remove --apps is answered here");
    };
    assert!(o.releases());
    assert!(matches!(
        o,
        other::Ask::Remove(other::Remove {
            args_json: true,
            ..
        })
    ));
    let Some(Ask::Other(o)) = parse(&["remove", "x"], &here()) else {
        panic!("remove is answered here");
    };
    assert!(!o.releases());
    assert!(matches!(
        o,
        other::Ask::Remove(other::Remove {
            args_json: false,
            ..
        })
    ));
}

#[test]
fn switch_words_stop_before_anything_is_sent() {
    let (l, e) = stop_of(&["api-skip-permissions", "set", "ws", "maybe"]);
    assert_eq!(l, ["Error: expected on|off|1|0, got: maybe"]);
    assert_eq!(e, Exit::Usage);
    let (l, _) = stop_of(&["host-session-cell-cap", "set", "ws", "513"]);
    assert_eq!(
        l,
        ["Error: host_session_cell_cap max is 512 (daemon ceiling), got: 513"]
    );
    let (l, _) = stop_of(&["host-session-cell-cap", "set", "ws", "012"]);
    assert_eq!(
        l,
        ["Error: expected positive integer 1..512 or 'default', got: 012"]
    );
    let (l, _) = stop_of(&["default-model", "frob"]);
    assert_eq!(
        l,
        [
            "Unknown default-model subcommand: frob".to_string(),
            format!(
                "Usage: {} workspace default-model get|set <workspace> …",
                cli_name!()
            )
        ]
    );
    // The tabs read their on|off word after the lookup.
    assert!(matches!(
        parse(&["design-tab", "set", "ws", "maybe"], &here()),
        Some(Ask::Switch(_))
    ));
}

#[test]
fn named_verbs_stop_as_the_shell_does() {
    let (l, e) = stop_of(&["set-handle", "ws", "new"]);
    assert_eq!(l.len(), 4);
    assert_eq!(e, Exit::Error);
    let (l, e) = stop_of(&["swap-canonical", "ws"]);
    assert_eq!(l.len(), 1);
    assert_eq!(e, Exit::Usage);
    let (l, _) = stop_of(&["swap-canonical", "ws", "--agent", "x", "--mode", "half"]);
    assert_eq!(l, ["mode must be recent or full"]);
    let (l, e) = stop_of(&["api-key"]);
    assert_eq!((l.len(), e), (2, Exit::Error));
    let (l, e) = stop_of(&["api-key", "ws", "k", "--key-stdin"]);
    assert_eq!(
        (l, e),
        (
            vec!["Error: give the key on stdin (--key-stdin) or not at all, not both.".to_string()],
            Exit::Usage
        )
    );
}

#[test]
fn remove_with_a_flag_and_no_value() {
    for flag in ["--apps", "--mode"] {
        assert_eq!(
            stop_of(&["remove", "x", flag]),
            (vec![format!("Error: {flag} takes a value")], Exit::Usage)
        );
    }
    assert_eq!(stop_of(&["remove"]).0.len(), 20);
}

#[test]
fn normpath_is_pythons() {
    for (p, want) in [
        ("/a/./b/../c", "/a/c"),
        ("//a//b/", "//a/b"),
        ("///a", "/a"),
        ("/..", "/"),
        ("/a/b/../../..", "/"),
    ] {
        assert_eq!(other::normpath(p), want, "{p}");
    }
}

#[test]
fn quoting_is_shlex_and_bash_printf_q() {
    assert_eq!(trust::shlex_quote("/a/b-c_d.e"), "/a/b-c_d.e");
    assert_eq!(trust::shlex_quote("a b'c"), "'a b'\"'\"'c'");
    assert_eq!(trust::shlex_quote(""), "''");
    assert_eq!(trust::bash_q("/a b/c"), "/a\\ b/c");
    assert_eq!(trust::bash_q("~x#y"), "\\~x#y");
    assert_eq!(trust::bash_q("a\nb"), "$'a\\nb'");
    assert_eq!(trust::bash_q("é"), "$'\\303\\251'");
}

#[test]
fn decode_errors_are_pythons() {
    assert_eq!(
        decode_error(""),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        decode_error("<html>"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        decode_error("{\"a\":1} x"),
        "Extra data: line 1 column 9 (char 8)"
    );
    assert_eq!(
        decode_error("{\"a\":1\n,}"),
        "Expecting property name enclosed in double quotes: line 2 column 2 (char 8)"
    );
}

#[test]
fn the_lookup_reads_path_or_nothing() {
    assert_eq!(resolved_of(br#"{"path":"/srv/a"}"#), "/srv/a");
    assert_eq!(resolved_of(br#"{"path":"/srv/a\n\n"}"#), "/srv/a");
    assert_eq!(resolved_of(br#"{"name":"a"}"#), "");
    assert_eq!(resolved_of(br#"["path"]"#), "");
    assert_eq!(resolved_of(b"<html>"), "");
    assert_eq!(resolved_of(br#"{"path":5}"#), "5");
}

#[test]
fn a_switch_alone_is_its_help_page_on_stderr() {
    for (verb, page, exit) in [
        ("api-guest-policy", "api-guest-policy", Exit::Ok),
        ("api-skip-permissions", "api-skip-permissions", Exit::Ok),
        ("default-model", "default-model", Exit::Ok),
        ("force-model-on-resume", "force-model-on-resume", Exit::Ok),
        ("host-session-cell-cap", "host-session-cell-cap", Exit::Ok),
        ("api-host-session-cap", "host-session-cell-cap", Exit::Ok),
        ("design-tab", "design-tab", Exit::Usage),
        ("design-mode", "design-tab", Exit::Usage),
        ("mobile-tab", "mobile-tab", Exit::Usage),
    ] {
        let key = format!("workspace/{page}");
        let text = crate::text::help(&key).expect("an embedded page");
        let want = (vec![text.strip_suffix('\n').unwrap().to_string()], exit);
        assert_eq!(stop_of(&[verb]), want, "{verb}");
        assert_eq!(stop_of(&[verb, ""]), want, "{verb} ''");
    }
}

#[test]
fn remove_with_apps_is_answered_here() {
    let remove = |words: &[&str]| match parse(words, &here()) {
        Some(Ask::Other(other::Ask::Remove(r))) => r,
        x => panic!("{words:?}: not a remove: {x:?}"),
    };
    let r = remove(&["remove", "x", "--apps", "handover:ws2"]);
    assert_eq!(r.apps, "handover:ws2");
    assert!(!r.drop_access && !r.drop_misused);
    let r = remove(&["remove", "x", "--apps=uninstall", "--drop-access"]);
    assert_eq!(r.apps, "uninstall");
    assert!(r.drop_access && !r.drop_misused);
    // Its release tries the cell socket.
    assert!(Ask::Other(other::Ask::Remove(r)).sends_on_socket());
}

#[test]
fn set_agent_identity_s_note_names_the_brand() {
    assert_eq!(
        SET_AGENT_IDENTITY_NOTE,
        format!(
            "note: '{0} workspace set-agent-identity' is now '{0} agent identity' (same flags).",
            cli_name!()
        )
    );
}

use super::screen::*;
use super::*;
use k2_client::brand::CLI_NAME;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src/talk-and-workspace.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Each line `talk` prints, as `cmd_talk` and its helpers have it (`k2`
/// is the default build's command name; `$target` stands for the target).
#[test]
fn the_lines_are_the_shells() {
    let src = source();
    let shell = |s: &str| s.replace("TARGET", "$target");
    for line in [
        "Error: workspace required",
        "Usage: k2 talk <workspace> \\\"text\\\" [--read-only] [--from <name>] [--lines N] [--agent <name>]",
        "       k2 talk <workspace>@<host> \\\"text\\\" [--token-stdin | --token <tok>] ...",
        "       Run 'k2 talk --help' for the full BETA protocol.",
        "k2 talk: --token-stdin or --token, not both",
        "k2 talk: --token-stdin: no token on stdin",
        ONE_OF,
        NO_ANSWERS,
        "Error: message text required (or pass --read-only to just peek)",
        "Error: unknown flag '$1' (try 'k2 talk --help')",
        RULE,
        "── talk(BETA): TARGET's live screen ──────────────────────────",
        "── talk(BETA): driving TARGET — sending '$key' ───────────────",
        "── talk(BETA): TARGET's screen after '$key' ──────────────────",
        "── talk(BETA) scout: walking TARGET's form (arrow-only, non-destructive) ──",
        "── scout parsed summary (best-effort; raw screens above are authoritative) ──",
        "── to answer: k2 talk TARGET --answers <a,b,c>  (N digits + auto final 1 to submit) ──",
        "── talk(BETA) answers: TARGET's screen after submit ──",
        "talk(BETA): --read-only — nothing sent. Re-run without it to deliver.",
        "talk(BETA): TARGET is in a human-in-the-loop prompt — message HELD, NOT delivered:",
        "talk(BETA): answer the form above with  k2 talk TARGET --answers <a,b,c>  (or single-step --answer <N> / --respond <y|n>),",
        "            then re-run  k2 talk TARGET \\\"…\\\"  once it's READY to deliver.",
        "talk(BETA): delivering to ${target}…",
        "=== Question $idx of ${total} — ${qname} ===",
        "=== Question $idx${total:+ of $total} (raw screen) ===",
    ] {
        assert!(src.contains(&shell(line)), "not in the shell: {line}");
    }
    // The judging notes, line by line.
    let notes = judge("$target").replace(&format!("'{CLI_NAME} "), "'k2 ");
    for line in notes.lines().skip(1) {
        assert!(
            src.contains(&format!("\n{line}\n")),
            "not in the shell: {line}"
        );
    }
    // The texts built here say the same, with the brand's name.
    let k = |s: &str| s.replace("k2 ", &format!("{CLI_NAME} "));
    assert!(usage().contains(&k("Run 'k2 talk --help' for the full BETA protocol.\n")));
    assert_eq!(
        unknown_flag("--x"),
        k("Error: unknown flag '--x' (try 'k2 talk --help')")
    );
    assert_eq!(
        BOTH_TOKENS,
        k("k2 talk: --token-stdin or --token, not both")
    );
    assert_eq!(
        NO_STDIN_TOKEN,
        k("k2 talk: --token-stdin: no token on stdin")
    );
}

#[test]
fn the_words_are_read_as_cmd_talk_reads_them() {
    let Parsed::Ask(a) = parse(&[
        "",
        "ws",
        "one",
        "two",
        "--lines",
        "9",
        "--no-wake",
        "--peek",
    ]) else {
        panic!("not an ask");
    };
    assert_eq!(
        (
            a.target.as_str(),
            a.text.as_str(),
            a.lines.as_str(),
            a.wake,
            a.read_only
        ),
        ("ws", "two", "9", false, true)
    );
    assert_eq!(
        parse(&["ws", "--zap", "--lines"]),
        Parsed::Stop(unknown_flag("--zap"), Exit::Error)
    );
    assert_eq!(
        parse(&["ws", "--lines"]),
        Parsed::Stop(needs_value("--lines"), Exit::Usage)
    );
    let Parsed::Ask(a) = parse(&["--answers", "1,2", "--respond", "y", "--token-stdin", "x"])
    else {
        panic!("not an ask");
    };
    assert_eq!(
        (
            a.answers.as_str(),
            a.respond.as_str(),
            a.token_stdin,
            a.target.as_str()
        ),
        ("1,2", "y", true, "x")
    );
}

#[test]
fn the_tuning_takes_what_sleep_and_test_take() {
    assert_eq!(seconds("0.35"), Some(Duration::from_millis(350)));
    assert_eq!(seconds("2"), Some(Duration::from_secs(2)));
    for bad in ["", ".", "1.2.3", "1s", "-1", "1e2"] {
        assert_eq!(seconds(bad), None, "{bad}");
    }
    assert_eq!(scout_cap("2"), Some(2));
    assert_eq!(scout_cap("0"), Some(0));
    for not_digits in ["", "-3", "2+2", "a[$(id)]", "x", " 2"] {
        assert_eq!(scout_cap(not_digits), Some(20), "{not_digits}");
    }
    for shell in ["08", "010", "99999999999999999999"] {
        assert_eq!(scout_cap(shell), None, "{shell}");
    }
}

#[test]
fn the_screen_readers_do_what_the_pythons_do() {
    assert_eq!(screen::read_raw(br#"{"lines":["a","b","",""]}"#), "a\nb");
    assert_eq!(screen::read_raw(br#"{"lines":"ab"}"#), "a\nb");
    assert_eq!(screen::read_raw(br#"{"lines":["a",1]}"#), "");
    assert_eq!(screen::read_raw(b"[1]"), "");
    assert_eq!(screen::read_raw(b"{}"), "");
    assert_eq!(
        terminal_lines(br#"{"lines":["a",null,true]}"#).unwrap(),
        "a\nNone\nTrue\n"
    );
    assert_eq!(terminal_lines(br#"{"lines":3}"#), None);
    assert_eq!(terminal_lines(b"nope"), None);
    // A capped read's note: on stderr after the lines; only a string is one.
    assert_eq!(
        screen::terminal_note(br#"{"lines":["a"],"note":"capped at 5000"}"#),
        Some(Some(format!(
            "{}capped at 5000",
            crate::families::terminal::READ_NOTE
        )))
    );
    assert_eq!(screen::terminal_note(br#"{"lines":["a"],"note":5}"#), None);
    assert_eq!(screen::terminal_note(br#"{"lines":["a"]}"#), None);
    assert_eq!(screen::terminal_note(br#"{"note":"\ud800"}"#), Some(None));
    assert_eq!(classify(br#"{"is_hitl":1,"source":"x|y"}"#), "true|x|y");
    assert_eq!(classify(br#"{"source":"s"}"#), "null|s");
    assert_eq!(classify(br#"{"is_hitl":false,"source":3}"#), "");
    assert_eq!(classify(b"[]"), "");
    assert_eq!(resolved_path(br#"{"path":null}"#), "None");
    assert_eq!(resolved_path(br#"{"other":1}"#), "");
    let rows = br#"[{"sessionId":"a"},{"sessionId":"b","isV2":1,"agentName":"x"},{"sessionId":"c","isV2":true,"agentName":"y"}]"#;
    assert_eq!(session_id(rows, ""), "b");
    assert_eq!(session_id(rows, "y"), "c");
    assert_eq!(session_id(rows, "zed"), "b");
    assert_eq!(session_id(br#"[{"sessionId":"a"},5]"#, ""), "");
    assert_eq!(session_id(br#"{"sessionId":"a"}"#, ""), "");
    assert!(is_uuid("0123ABCD-0000-4000-8000-00000000beef"));
    assert!(is_uuid("x\n0123abcd-0000-4000-8000-00000000beef"));
    assert!(!is_uuid("0123abcd-0000-4000-8000-00000000bee"));
}

#[test]
fn the_form_parsers_do_what_the_shell_does() {
    let form =
        "top\r\n←  ☐ Colour  ☒ Size → ✔ Submit  →\nWhich?\n❯ 1. Red\n  2. Blue\n3) Green\nfooter";
    assert_eq!(tabbar(form), vec!["Colour".to_string(), "Size".into()]);
    assert_eq!(
        trim(form),
        "←  ☐ Colour  ☒ Size → ✔ Submit  →\nWhich?\n❯ 1. Red\n  2. Blue\n3) Green"
    );
    assert_eq!(trim("a\r\nb\n\n"), "a\nb");
    assert_eq!(tabbar("no bar"), Vec::<String>::new());
    assert_eq!(
        total("x\nquestion  2 OF 12 and Question 3 of 4", false),
        "12"
    );
    assert_eq!(total("Question 2 of", false), "");
    assert_eq!(
        option_lines(" ❯ 1. Red\n  2. Blue\n3) Green\n4.x", true),
        "   ❯ 1. Red\n   2. Blue\n   3) Green\n"
    );
    // The C locale reads `❯?` as two bytes and an optional third.
    assert_eq!(
        option_lines(" ❯ 1. Red\n  2. Blue\n3) Green", false),
        "   ❯ 1. Red\n"
    );
    assert_eq!(screen::answers("1, 2,,3,\n4", false), vec!["1", "2", "3"]);
    assert_eq!(screen::answers(",", false), Vec::<String>::new());
    assert_eq!(splitlines("a\rb\r\nc\u{2028}d\n"), vec!["a", "b", "c", "d"]);
}

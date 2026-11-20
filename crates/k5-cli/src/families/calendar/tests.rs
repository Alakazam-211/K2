use super::*;

/// `_cal_py`'s script as python3 reads it from stdin: line 1 is the one
/// after `<<'PYEOF'`.
fn script() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/calendar.sh");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let body = text
        .split_once("python3 - <<'PYEOF'\n")
        .expect("_cal_py's heredoc")
        .1;
    let end = body.find("\nPYEOF\n").expect("the heredoc's end");
    body[..end].split('\n').map(str::to_string).collect()
}

#[test]
fn the_lines_that_raise_are_the_shell_s() {
    let s = script();
    let at = |n: u32| s[n as usize - 1].trim().to_string();
    let pins: &[(u32, &str)] = &[
        (line::CALL_D, "if isinstance(d.get(\"error\"), dict):"),
        (line::WHEN_GET, "s = e.get(\"start\") or \"?\""),
        (line::WHEN_SLICE, "return \"%s (all day)\" % (s[:10])"),
        (line::JOIN_GET, "name = {\"meet\""),
        (line::LINE_GET, "if e.get(\"join\"):"),
        (
            line::LINE_JOIN,
            "extras += \" · %s\" % join_text(e.get(\"join\"))",
        ),
        (
            line::LINE_WHEN,
            "print(\"%s · %s · %s%s\" % (e.get(\"id\", \"\"), when(e)",
        ),
        (
            line::FULL_GET,
            "print(\"id       : %s\" % e.get(\"id\", \"\"))",
        ),
        (line::FULL_WHEN, "print(\"when     : %s\" % when(e))"),
        (line::FULL_IDS, "print(\"calendar : %s\" % \", \".join("),
        (
            line::FULL_JOIN,
            "print(\"join     : %s · %s\" % (join_text(j)",
        ),
        (line::FULL_ATTENDEES, "for a in e.get(\"attendees\") or []:"),
        (
            line::FULL_ATTENDEE,
            "print(\"attendee : %s%s\" % (a.get(\"email\")",
        ),
        (
            line::FULL_PARTS,
            "for p in ([] if e.get(\"attendees\") is not None",
        ),
        (
            line::FULL_PART,
            "print(\"attendee : %s%s\" % (p.get(\"email\")",
        ),
        (
            line::FULL_REDACTED,
            "elif \"description\" in (e.get(\"redacted\") or []):",
        ),
        (
            line::LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/list\"",
        ),
        (line::LIST_FOR, "for c in cals:"),
        (line::LIST_C, "r = c.get(\"rights\") or {}"),
        (
            line::LIST_R,
            "can = [k for k in (\"read\", \"write\", \"delete\") if r.get(k)]",
        ),
        (
            line::EVENTS_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/events\"",
        ),
        (line::EVENTS_FOR, "for e in evs:"),
        (line::EVENTS_FULL, "print_event_full(e)"),
        (line::EVENTS_LINE, "print_event_line(e)"),
        (
            line::CALS_POST,
            "d = call(\"POST\", \"/cli/mail/calendar/calendars\",",
        ),
        (
            line::CALS_GET,
            "d = call(\"GET\", \"/cli/mail/calendar/calendars\"",
        ),
        (line::CALS_FOR, "for c in d.get(\"calendars\") or []:"),
        (
            line::CALS_C,
            "mark = \"on \" if c.get(\"optedIn\", True) else \"off\"",
        ),
        (
            line::CALS_JOIN,
            "print(\"%s: %s\" % (k, \", \".join(d[k])))",
        ),
        (
            line::SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/show\"",
        ),
        (line::SHOW_FULL, "print_event_full(d.get(\"event\") or {})"),
        (
            line::FREE_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/freebusy\"",
        ),
        (line::FREE_FOR, "for b in busy:"),
        (line::FREE_B, "print(\"%s → %s · %s\" % (b.get(\"start\")"),
        (
            line::WAIT_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/wait\"",
        ),
        (line::WAIT_FOR, "for i in d.get(kind) or []:"),
        (
            line::WRITE_CALL,
            "d = call(\"POST\", \"/cli/mail/calendar/\" + verb, body=body)",
        ),
        (line::WRITE_LINE, "print_event_line(ev)"),
        (
            line::DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/calendar/delete\"",
        ),
    ];
    for (n, want) in pins {
        assert!(
            at(*n).starts_with(want),
            "line {n}: {:?}, not {want:?}",
            at(*n)
        );
    }
}

fn run(words: &[&str]) -> Form {
    parse(words)
}

fn hint(f: Form) -> String {
    match f {
        Form::Usage(h) => h,
        other => panic!("not a usage error: {other:?}"),
    }
}

#[test]
fn help_and_the_verbs() {
    for w in [&[][..], &[""], &["help"], &["--help"], &["list", "-h"]] {
        assert_eq!(run(w), Form::Help, "{w:?}");
    }
    assert!(matches!(run(&["event", "e1"]), Form::Run(Verb::Show, a) if a.id == "e1"));
    assert!(
        matches!(run(&["list", "a@x", "--json"]), Form::Run(Verb::List, a) if a.json && a.address == "a@x")
    );
    assert_eq!(
        hint(run(&["nope"])),
        concat!(
            "unknown command 'nope' — see ",
            cli_name!(),
            " calendar --help"
        )
    );
}

#[test]
fn the_flag_checks_are_the_shell_s() {
    assert_eq!(
        hint(run(&["event", "--from", "x"])),
        concat!(
            "'",
            cli_name!(),
            " calendar show' does not take '--from' — see ",
            cli_name!(),
            " calendar --help"
        )
    );
    // `case "$allowed" in *" $1 "*)` is a substring test.
    assert!(matches!(
        run(&["events", "--from --to", "v"]),
        Form::Run(Verb::Events, _)
    ));
    assert_eq!(
        hint(run(&["events", "--from"])),
        concat!(
            "--from requires a value — see ",
            cli_name!(),
            " calendar --help"
        )
    );
    assert_eq!(
        hint(run(&["list", "-x"])),
        concat!("unknown flag '-x' — see ", cli_name!(), " calendar --help")
    );
    assert_eq!(
        hint(run(&["list", "a", "b"])),
        concat!(
            "unexpected extra argument 'b' — see ",
            cli_name!(),
            " calendar --help"
        )
    );
    assert_eq!(
        hint(run(&["delete"])),
        concat!(
            "delete requires an event <id> (from '",
            cli_name!(),
            " calendar events')"
        )
    );
    assert_eq!(
        hint(run(&["calendars"])),
        "calendars requires the linked account's <address>"
    );
    assert_eq!(
        hint(run(&["events", "--limit", "1x"])),
        "--limit must be a number from 1 to 500"
    );
    for t in ["0", "901", "x", "-1"] {
        assert_eq!(
            hint(run(&["wait", "--timeout", t])),
            "--timeout must be 1-900 seconds (default 300); loop the call to wait longer",
            "{t}"
        );
    }
    assert!(matches!(
        run(&["wait", "--timeout", "0900"]),
        Form::Run(Verb::Wait, _)
    ));
    // Past 64 bits bash's `[` fails with its own line: the shell's.
    assert_eq!(
        run(&["wait", "--timeout", "99999999999999999999"]),
        Form::Shell
    );
    assert_eq!(
        hint(run(&["create", "--start", "a", "--end", "b"])),
        "create requires --title"
    );
    assert_eq!(
        hint(run(&["create", "--title", "t", "--start", "a"])),
        "create requires --start and --end (RFC 3339, e.g. 2026-10-08T09:00:00-07:00)"
    );
    assert_eq!(
        hint(run(&["update", "e1"])),
        "update needs at least one of --title --start --end --tz --location --notes"
    );
    assert!(
        matches!(run(&["update", "e1", "--title", ""]), Form::Run(Verb::Update, a) if a.title.as_deref() == Some(""))
    );
}

#[test]
fn link_checks_its_words_then_goes_to_the_shell() {
    assert_eq!(
        hint(run(&["link"])),
        "link requires the account's <address> (never a password or the pasted page address)"
    );
    assert_eq!(
        hint(run(&["link", "a@x"])),
        "link requires --provider google|microsoft"
    );
    assert_eq!(
        hint(run(&["link", "a@x", "--provider", "yahoo"])),
        "--provider is google or microsoft"
    );
    assert_eq!(
        hint(run(&[
            "link",
            "a@x",
            "--provider",
            "google",
            "--request",
            "--write"
        ])),
        "--request takes only <address> --provider (your human picks the rest)"
    );
    assert_eq!(
        hint(run(&["link", "a@x", "--provider", "google"])),
        "link requires --workspace <ws> (whose agents see the calendar)"
    );
    assert_eq!(
        hint(run(&[
            "link",
            "a@x",
            "--provider",
            "microsoft",
            "--workspace",
            "w",
            "--paste"
        ])),
        "--paste is for Google; Microsoft links with a device code"
    );
    assert_eq!(
        run(&["link", "a@x", "--provider", "google", "--request"]),
        Form::Shell
    );
}

#[test]
fn the_usage_line_is_mail_err_s() {
    assert_eq!(
        usage_line("a \"b\" — é"),
        r#"{"error":{"code":"usage","hint":"a \"b\" — é"}}"#
    );
}

#[test]
fn a_traceback_reads_from_stdin() {
    let t = Tb {
        frames: vec![(240, "<module>"), (154, "print_event_line")],
        exception: "AttributeError: 'str' object has no attribute 'get'".into(),
    };
    assert_eq!(
        t.text(),
        "Traceback (most recent call last):\n  File \"<stdin>\", line 240, in <module>\n  File \"<stdin>\", line 154, in print_event_line\nAttributeError: 'str' object has no attribute 'get'\n"
    );
}

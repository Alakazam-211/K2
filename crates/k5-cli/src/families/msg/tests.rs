use super::send::{self, address, is_postal, parse, Address, Msg, Parsed};
use super::send_once::{self, Soft};
use super::{verify, who};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A text of ours as the shell spells it: the brand's names as the
/// shell's default brand has them.
fn as_shell(text: &str) -> String {
    text.replace(&format!("{CLI_NAME} "), "k2 ")
        .replace(&format!("'{CLI_NAME} "), "'k2 ")
        .replace(&format!("{PRODUCT_NAME}'s"), "K2's")
        .replace(&format!("({PRODUCT_NAME} "), "(K2 ")
        .replace(&format!(": {PRODUCT_NAME} "), ": K2 ")
        .replace(&format!("note: {PRODUCT_NAME} "), "note: K2 ")
}

fn pinned(file: &str, line: &str) {
    let text = source(file);
    let want = as_shell(line);
    assert!(text.contains(&want), "{file} no longer has {want:?}");
}

#[test]
fn the_shell_s_lines() {
    let staging = "msg-whoami-done.sh";
    for line in [
        send::IDEM_USAGE,
        send::SESSION_USAGE,
        send::SUMMARY_USAGE,
        send::SECRET_USAGE,
        send::SESSION_ONE_WORD,
        send::SESSION_SECRET,
    ] {
        pinned(
            staging,
            &format!("echo \"{}\" >&2", line.replace('"', "\\\"")),
        );
    }
    pinned(
        staging,
        r#"echo "Error: unknown flag '$1' (try 'k2 msg --help')" >&2"#,
    );
    assert_eq!(
        as_shell(&send::unknown_flag("-x")),
        "Error: unknown flag '-x' (try 'k2 msg --help')\n"
    );
    pinned(staging, r#"echo "Error: workspace required" >&2"#);
    pinned(
        staging,
        r#"echo "Usage: k2 msg <workspace> \"text\" [--from <name>] [--command \"<text>\"]" >&2"#,
    );
    pinned(
        staging,
        r#"echo "       Run 'k2 msg --help' for full reference." >&2"#,
    );
    pinned(staging, r#"echo "Error: message text required" >&2"#);
    pinned(
        staging,
        r#"echo "hint: this message is $n characters with no --summary. Add --summary \"one line: what you need or what you did\" so people can scan it (K2 uses its first line otherwise)." >&2"#,
    );
    pinned(
        staging,
        r#"echo "k2 msg: '${target}' looks like federation wire form (<fp>::<workspace>::<agent>)." >&2"#,
    );
    pinned(
        staging,
        r#"echo "  For agents use the user form:  k2 msg <agent>::<host> \"text\"" >&2"#,
    );
    pinned(
        staging,
        r#"echo "  Wire form is for:              k2 fed send <fp>::<ws>::<agent> \"text\"" >&2"#,
    );
    pinned(
        staging,
        r#"response=$(_k2_idem_send cli_post_form "/cli/workspace/msg" "$params")"#,
    );
    // The woke and reopened notes and the gate's JSON line go nowhere.
    let msg = source(staging);
    assert!(msg.contains("        sys.stderr.write(\"note: reopened a past conversation as %s; your message is its first turn. Reply arrives as [from %s].\\n\" % (d.get(\"addr\"), d.get(\"addr\")))\nexcept Exception:\n    pass' 2>/dev/null"));
    assert!(msg.contains("except Exception:\n    print(2)\n' 2>/dev/null)"));
    let inbox = source("msg-inbox.sh");
    assert!(inbox.contains(&format!(
        "cat >&2 <<'EOF'\n{}EOF\n    exit 2",
        as_shell(send::BARE_INBOX)
    )));
    pinned(
        "msg-inbox-staging.sh",
        &format!("echo \"{}\" >&2\n        exit 2", send::BOTH_INBOX),
    );
    let dep = source("deprecation.sh");
    assert!(dep.contains("Error: '$old_verb' is no longer supported.\n\nUse instead:\n  $new_form\n\nRun 'k2 help-deprecated' for the full map of retired verbs.\nEOF\n    exit 1"));
    assert_eq!(
        as_shell(send::AGENT_RETIRED),
        "Error: 'k2 msg --agent <name>' is no longer supported.\n\nUse instead:\n  k2 msg <workspace> \"text\"  (workspace-implicit; pass a workspace name/path)\n\nRun 'k2 help-deprecated' for the full map of retired verbs.\n"
    );
    pinned(
        staging,
        r#"fail_deprecated "k2 msg --agent <name>" "k2 msg <workspace> \"text\"  (workspace-implicit; pass a workspace name/path)" ;;"#,
    );
}

#[test]
fn the_send_once_lines() {
    let cell = source("cell-socket.sh");
    assert!(cell.contains(&format!("echo \"{}\" >&2", send_once::KEY_USAGE)));
    assert_eq!(
        as_shell(&send_once::once_line("/cli/workspace/msg", 3)),
        "Error: K2's server didn't answer /cli/workspace/msg (3 s). It may have been delivered; check before resending (this server doesn't say it drops a repeated send, so k2 didn't try again)."
    );
    assert!(cell.contains(r#"echo "Error: K2's server didn't answer ${endpoint} ($((SECONDS - t0)) s). It may have been delivered; check before resending (this server doesn't say it drops a repeated send, so k2 didn't try again)." >&2"#));
    assert_eq!(
        as_shell(&send_once::tries_line("/e", 3, 2, "kkkkkkkk")),
        "Error: K2's server didn't answer /e (3 tries, 2 s); it may have been delivered. It is safe to run the same command again with --idempotency-key kkkkkkkk: K2 delivers one key once."
    );
    assert!(cell.contains(r#"echo "Error: K2's server didn't answer ${endpoint} (${n} tries, $((SECONDS - t0)) s); it may have been delivered. It is safe to run the same command again with --idempotency-key ${_K2_IDEM_KEY}: K2 delivers one key once." >&2"#));
    let note = send_once::replay_note(br#"{"replayed":true}"#).unwrap();
    assert!(cell.contains(&format!("echo \"{}\" >&2 ;;", as_shell(&note))));
    assert!(cell
        .contains(r#"local endpoint="$2" tries="${_K2_IDEM_TRIES:-3}" n=1 out rc t0="$SECONDS""#));
    assert!(cell.contains(r#"case "$rc" in 28|52|56) return 0 ;; esac"#));
    assert!(cell.contains(r#"[[ "$out" == *'"code":"in_progress"'* ]] && return 0"#));
}

#[test]
fn the_post_time_limit() {
    use std::ffi::OsStr;
    use std::time::Duration;
    let t = |v: &str| super::post_max_time(Some(OsStr::new(v)));
    assert_eq!(super::post_max_time(None), Some(None));
    assert_eq!(t(""), Some(None));
    assert_eq!(t("1"), Some(Some(Duration::from_secs(1))));
    assert_eq!(t("0.5"), Some(Some(Duration::from_millis(500))));
    assert_eq!(t("0"), Some(Some(Duration::from_secs(u64::from(u32::MAX)))));
    for odd in ["x", "1e2", "-1", "1.2.3", ".", " 1"] {
        assert_eq!(t(odd), None, "{odd}");
    }
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains(r#"--connect-timeout 5 --max-time "${_K2_POST_MAX_TIME:-30}" \"#));
}

#[test]
fn keys_and_tries() {
    for ok in ["abcdefgh", "A-_9a-_9", "x".repeat(64).as_str()] {
        assert!(send_once::valid(ok), "{ok}");
    }
    for bad in [
        "",
        "abcdefg",
        "abc defgh",
        "abcdefgh!",
        "x".repeat(65).as_str(),
        "ééééé",
    ] {
        assert!(!send_once::valid(bad), "{bad}");
    }
    let k = send_once::uuid4();
    let b = k.as_bytes();
    assert_eq!(k.len(), 36, "{k}");
    assert_eq!((b[8], b[13], b[18], b[23]), (b'-', b'-', b'-', b'-'), "{k}");
    assert_eq!(b[14], b'4', "{k}");
    assert!(matches!(b[19], b'8' | b'9' | b'a' | b'b'), "{k}");
    assert!(send_once::valid(&k));
    assert_ne!(send_once::uuid4(), send_once::uuid4());
    assert_eq!(send_once::tries(None), 3);
    assert_eq!(send_once::tries(Some("")), 3);
    assert_eq!(send_once::tries(Some("0")), 3);
    assert_eq!(send_once::tries(Some("x")), 3);
    assert_eq!(send_once::tries(Some("-1")), 3);
    assert_eq!(send_once::tries(Some("00")), 0);
    assert_eq!(send_once::tries(Some("5")), 5);
    let soft = |out: &[u8], rc| Soft {
        out: out.to_vec(),
        rc,
    };
    for s in [
        soft(b"", 0),
        soft(b"", 28),
        soft(b"", 52),
        soft(b"x", 56),
        soft(br#"{"error":{"code":"in_progress"}}"#, 0),
    ] {
        assert!(send_once::unanswered(&s), "{s:?}");
    }
    for s in [soft(b"{}", 0), soft(b"", 7), soft(b"", 1), soft(b"", 76)] {
        assert!(!send_once::unanswered(&s), "{s:?}");
    }
}

fn sends(words: &[&str]) -> Msg {
    match parse(words) {
        Parsed::Send(m) => m,
        other => panic!("{words:?}: {other:?}"),
    }
}

fn stops(words: &[&str]) -> (String, Exit) {
    match parse(words) {
        Parsed::Stop(t, e) => (t, e),
        other => panic!("{words:?}: {other:?}"),
    }
}

#[test]
fn the_words() {
    let m = sends(&["ws", "hello"]);
    assert_eq!(
        (m.target.as_str(), m.text.as_str(), m.wake),
        ("ws", "hello", true)
    );
    let m = sends(&[
        "ws",
        "a",
        "b",
        "--no-wake",
        "--from",
        "me",
        "--command",
        "/x",
    ]);
    assert_eq!(
        (m.text.as_str(), m.wake, m.from.as_str(), m.command.as_str()),
        ("b", false, "me", "/x")
    );
    let m = sends(&["", "ws", "t"]);
    assert_eq!((m.target.as_str(), m.text.as_str()), ("ws", "t"));
    let m = sends(&["--session", "s1", "only text"]);
    assert_eq!(
        (m.target.as_str(), m.text.as_str(), m.session.as_str()),
        ("", "only text", "s1")
    );
    let m = sends(&["--summary", "", "ws", "t", "--idempotency-key", "abcdefgh"]);
    assert_eq!((m.summary.as_str(), m.idem_key.as_str()), ("", "abcdefgh"));
    // Not hosts: local.
    for t in ["a::b", "agent@", "::x.y"] {
        assert_eq!(sends(&[t, "t"]).target, t);
    }
    assert_eq!(sends(&["a::b::c", "t"]).target, "a::b::c");

    assert_eq!(stops(&["--idempotency-key"]).1, Exit::Usage);
    assert_eq!(stops(&["--idempotency-key", ""]).1, Exit::Usage);
    assert_eq!(
        stops(&["--session"]).0,
        format!("{}\n", send::SESSION_USAGE)
    );
    assert_eq!(
        stops(&["ws", "--summary"]).0,
        format!("{}\n", send::SUMMARY_USAGE)
    );
    assert_eq!(
        stops(&["--secret", ""]).0,
        format!("{}\n", send::SECRET_USAGE)
    );
    assert_eq!(
        stops(&["ws", "--bogus", "--from"]),
        (send::unknown_flag("--bogus"), Exit::Error)
    );
    assert_eq!(stops(&["ws", "-"]).0, send::unknown_flag("-"));
    assert_eq!(
        stops(&["--agent", "x"]),
        (send::AGENT_RETIRED.to_string(), Exit::Error)
    );
    assert_eq!(stops(&[]), (send::no_target(), Exit::Error));
    assert_eq!(stops(&["--no-wake", ""]), (send::no_target(), Exit::Error));
    assert_eq!(stops(&["ws"]), (send::no_text(), Exit::Error));
    assert_eq!(stops(&["ws", ""]), (send::no_text(), Exit::Error));
    assert_eq!(
        stops(&["--session", "s", "a", "b"]).0,
        format!("{}\n", send::SESSION_ONE_WORD)
    );
    assert_eq!(stops(&["--session", "s"]), (send::no_text(), Exit::Error));
    let long = "x".repeat(601);
    let words = ["--session", "s", long.as_str(), "--secret", "N"];
    let (text, exit) = stops(&words);
    assert_eq!(exit, Exit::Usage);
    assert!(
        text.starts_with("hint: this message is 601 characters"),
        "{text}"
    );
    assert!(text.ends_with(&format!("\n{}\n", send::SESSION_SECRET)));
    assert_eq!(
        stops(&["ws", "--inbox"]),
        (send::BARE_INBOX.to_string(), Exit::Usage)
    );
    assert_eq!(
        stops(&["ws", "--inbox", "--inbox-silent", "--inbox-wake"]).0,
        format!("{}\n", send::BOTH_INBOX)
    );

    for shell in [&["--help"][..], &["ws", "t", "-h"]] {
        assert_eq!(parse(shell), Parsed::Shell, "{shell:?}");
    }
    // `--secret --stdin`: the vault's handoff now reads the value typed
    // in itself, so it is a handoff too.
    assert!(
        matches!(parse(&["ws", "t", "--secret", "--stdin"]), Parsed::Secret(_, ref n) if n == "--stdin"),
        "--secret --stdin is the handoff"
    );
    // The tray forms: the module's own words.
    for (words, mode) in [
        (&["ws", "--inbox-silent", "f"][..], "silent"),
        (&["ws", "f", "--inbox-wake"], "wake"),
    ] {
        let Parsed::Tray(f) = parse(words) else {
            panic!("{words:?} is a tray form");
        };
        assert_eq!(
            (f.target.as_str(), f.mode.as_str(), f.positional.as_slice()),
            ("ws", mode, &["f".to_string()][..])
        );
    }
    // `--secret NAME`: the handoff, with the text as its note.
    let Parsed::Secret(m, name) = parse(&["ws", "t", "--secret", "N", "--summary", "s"]) else {
        panic!("--secret is the handoff");
    };
    assert_eq!(name, "N");
    assert_eq!(
        send::handoff_words(&m, &name),
        ["N", "--to", "ws", "--note", "t", "--via", "msg"]
    );
    assert_eq!(m.summary, "s");
    // The shell's own call, word for word.
    let call = r#"_vault_handoff send "$secret_name" --to "$target" --note "$text" --via msg"#;
    assert!(source("msg-whoami-done.sh").contains(call));
    // A value flag as the last word: the shell's `_need_value` line, exit 2.
    for flag in ["--from", "--command", "--wake-timeout", "--via"] {
        assert_eq!(
            stops(&["ws", "t", flag]),
            (send::needs_value(flag), Exit::Usage)
        );
        assert_eq!(
            send::needs_value(flag),
            format!("Error: {flag} takes a value\n")
        );
    }
    let staging = source("msg-whoami-done.sh");
    for line in [
        r#"--from) explicit_from="$2"; shift 2 ;;"#,
        r#"--command) explicit_command="$2"; shift 2 ;;"#,
        r#"wake_timeout="$2"; shift 2 ;;"#,
        r#"explicit_via="$2"; shift 2 ;;"#,
    ] {
        assert!(staging.contains(line), "{line}");
    }
}

#[test]
fn postal_mail() {
    use super::postal::{self, Mail};
    let mail = |to: &str, text: &str, wake: bool, json: bool| {
        Parsed::Postal(Mail {
            to: to.into(),
            text: text.into(),
            wake,
            json,
        })
    };
    assert_eq!(
        parse(&["a::label.postal.bot", "t"]),
        mail("a::label.postal.bot", "t", true, false)
    );
    assert_eq!(
        parse(&[
            "--no-wake",
            "A_b::my-label.Postal.Bot",
            "one",
            "",
            "two",
            "--json",
            "--wake"
        ]),
        mail("A_b::my-label.Postal.Bot", "one  two", false, true)
    );
    assert_eq!(
        parse(&["a::abc.postal.bot", "", "t"]),
        mail("a::abc.postal.bot", "t", true, false)
    );
    let to = "a::abc.postal.bot";
    assert_eq!(
        parse(&[to]),
        Parsed::Stop(format!("{}\n", postal::usage(to)), Exit::Usage)
    );
    assert_eq!(
        parse(&[to, "t", "--from", "x"]),
        Parsed::Stop(format!("{}\n", postal::k2_only("--from", to)), Exit::Usage)
    );
    assert_eq!(
        parse(&[to, "t", "-q"]),
        Parsed::Stop(format!("{}\n", postal::unknown_flag("-q")), Exit::Usage)
    );
    // The first word that is no flag decides, even a flag's value.
    assert_eq!(
        parse(&["--from", to, "ws", "t"]),
        Parsed::Stop(format!("{}\n", postal::k2_only("--from", to)), Exit::Usage)
    );
    // A value flag first: msg's own loop reads the address. A flag Postal
    // mail can't honour is refused with the address-first form's line,
    // never dropped.
    let same = |flag_first: &[&str], address_first: &[&str]| {
        let got = parse(flag_first);
        assert!(
            matches!(got, Parsed::Stop(_, Exit::Usage)),
            "{flag_first:?}: {got:?}"
        );
        assert_eq!(got, parse(address_first), "{flag_first:?}");
    };
    same(&["--from", "me", to, "hi"], &[to, "hi", "--from", "me"]);
    same(&["--summary", "s", to, "hi"], &[to, "hi", "--summary", "s"]);
    same(
        &["--no-wake", "--from", "me", to, "hi"],
        &[to, "hi", "--no-wake", "--from", "me"],
    );
    same(
        &["--idempotency-key", "abcdefgh", to, "hi"],
        &[to, "hi", "--idempotency-key", "abcdefgh"],
    );
    same(&["--secret", "K", to, "hi"], &[to, "hi", "--secret", "K"]);
    same(&["--from", "me", to], &[to, "--from", "me"]);
    assert_eq!(
        parse(&["--summary", "s", to, "hi"]),
        Parsed::Stop(
            format!("{}\n", postal::k2_only("--summary", to)),
            Exit::Usage
        )
    );
    assert_eq!(
        parse(&["--secret", "K", to, "hi"]),
        Parsed::Stop(
            format!("{}\n", postal::unknown_flag("--secret")),
            Exit::Usage
        )
    );
    // The flags it can honour, then the text as it is.
    assert_eq!(
        postal::after_loop(to, &["--wake", "--no-wake"], "-x"),
        Ok(Mail {
            to: to.into(),
            text: "-x".into(),
            wake: false,
            json: false,
        })
    );
    assert_eq!(
        postal::after_loop(to, &["--wake"], ""),
        Err(postal::usage(to))
    );
    let src = source("remote-target.sh");
    for line in [
        r#"echo "k2 msg: $1 is for messages to agents on this server; Postal mail to $to takes only the text, --no-wake and --json." >&2"#,
        r#"-*) echo "k2 msg: unknown flag '$1' for Postal mail (k2 msg --help)" >&2; exit 2 ;;"#,
        r#"echo "Usage: k2 msg $to \"text\" [--no-wake] [--json]" >&2"#,
        "--summary|--wake-timeout|--session|--inbox|--inbox-wake|--inbox-silent|--signal|--command|--from|--via)",
    ] {
        assert!(src.contains(line), "{line}");
    }
    assert_eq!(
        as_shell(&postal::usage("A")),
        "Usage: k2 msg A \"text\" [--no-wake] [--json]"
    );
    let m = Mail {
        to: "a::b.postal.bot".into(),
        text: "hé".into(),
        wake: false,
        json: false,
    };
    assert_eq!(
        postal::body("/w", &m),
        r#"{"workspace": "/w", "to": "a::b.postal.bot", "text": "h\u00e9", "wake": false}"#
    );

    let r = postal::reader(
        br#"{"status":"delivered","to":"a::b.postal.bot","badge":"B","deliveryId":"m_1"}"#,
        false,
        false,
    );
    assert_eq!(
        as_shell(&r.stdout),
        "Postal mail to a::b.postal.bot: delivered\nBadge:    B\nRecord:   m_1 (k2 msg verify m_1)\nTheir server stored it.\n"
    );
    assert_eq!(r.exit, Exit::Ok);
    let r = postal::reader(
        br#"{"status":"failed","to":"x","reason":"gone"}"#,
        false,
        false,
    );
    assert_eq!(
        (r.stdout.as_str(), r.exit),
        ("Not sent to x: failed (gone)\n", Exit::Error)
    );
    let r = postal::reader(br#"{"status":"queued"}"#, false, false);
    assert_eq!(
        r.stdout,
        "Postal mail to None: queued\nQueued on this server; it goes out at the next try.\n"
    );
    let r = postal::reader(
        r#"{"asked":true,"card":"c1","waiting":true,"hint":"ßee"}"#.as_bytes(),
        false,
        false,
    );
    assert_eq!(
        (r.stdout.as_str(), r.exit),
        (
            "Asked: a card is on your person's Thread (c1), waiting already.\nSSee.\n",
            Exit::Usage
        )
    );
    let r = postal::reader(
        br#"{"error":{"code":"postal_not_friends","hint":"ask first"}}"#,
        false,
        false,
    );
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: postal_not_friends: ask first\n", Exit::Refused)
    );
    let r = postal::reader(
        br#"{"error":{"code":"too_large","hint":"h"}}"#,
        false,
        false,
    );
    assert_eq!(r.exit, Exit::Usage);
    let r = postal::reader(br#"{"status":"held"}"#, true, false);
    assert_eq!(r.stdout, "{\n  \"status\": \"held\"\n}\n");
    let r = postal::reader(br#"{"workspace":"x"}"#, false, false);
    let want = format!(
        "line {}, in <module>\nAttributeError: 'str' object has no attribute 'get'\n",
        postal::line::WORKSPACE
    );
    assert!(r.stderr.ends_with(&want), "{}", r.stderr);
}

/// In a session (`PS`), a person's step ends with where a person takes
/// it, unless the daemon's hint already says; outside a session it never
/// does. Each line number names the line of `_postal_print`'s Python.
#[test]
fn postal_mail_in_a_session() {
    use super::postal;
    use crate::families::postal::PERSON_LINE;
    let owner = br#"{"error":{"code":"owner_only","hint":"only the owner"}}"#;
    let r = postal::reader(owner, false, true);
    assert_eq!(
        (r.stderr, r.exit),
        (
            format!("Error: owner_only: only the owner\n{PERSON_LINE}\n"),
            Exit::Refused
        )
    );
    let r = postal::reader(owner, false, false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: owner_only: only the owner\n", Exit::Refused)
    );
    let people = br#"{"error":{"code":"postal_people_only","hint":"a person"}}"#;
    let r = postal::reader(people, false, true);
    let want = format!("Error: postal_people_only: a person\n{PERSON_LINE}\n");
    assert_eq!(r.stderr, want);
    for says in [
        r#"{"error":{"code":"owner_only","hint":"open the Postal tab"}}"#,
        r#"{"error":{"code":"owner_only","hint":"in Settings"}}"#,
        r#"{"error":{"code":"owner_only","hint":["Settings"]}}"#,
        r#"{"error":{"code":"postal_not_friends","hint":"ask first"}}"#,
        r#"{"error":"owner_only"}"#,
    ] {
        let r = postal::reader(says.as_bytes(), false, true);
        assert!(!r.stderr.contains(PERSON_LINE), "{says}: {}", r.stderr);
    }
    // A hint Python can't look in: the error line, then its traceback.
    let r = postal::reader(br#"{"error":{"code":"owner_only","hint":5}}"#, false, true);
    let want = format!(
        "Error: owner_only: 5\nTraceback (most recent call last):\n  File \"<string>\", line {}, in <module>\nTypeError: argument of type 'int' is not iterable\n",
        postal::line::PERSON
    );
    assert_eq!((r.stderr, r.exit), (want, Exit::Error));
    // A code that is a list: the error line raises before the check.
    let r = postal::reader(br#"{"error":{"code":["owner_only"]}}"#, false, true);
    let want = format!(
        "line {}, in <module>\nTypeError: can only concatenate list (not \"str\") to list\n",
        postal::line::ERROR
    );
    assert!(r.stderr.ends_with(&want), "{}", r.stderr);
    // An empty list: no prefix, the check passes it by, the map raises.
    let r = postal::reader(br#"{"error":{"code":[],"hint":"h"}}"#, false, true);
    let want = format!(
        "Error: h\nTraceback (most recent call last):\n  File \"<string>\", line {}, in <module>\nTypeError: unhashable type: 'list'\n",
        postal::line::EXIT
    );
    assert_eq!(r.stderr, want);
    let r = postal::reader(b"[]", false, true);
    let want = format!(
        "line {}, in <module>\nAttributeError: 'list' object has no attribute 'get'\n",
        postal::line::ASKED
    );
    assert!(r.stderr.ends_with(&want), "{}", r.stderr);
    // The lines, as the shell's Python has them.
    let src = source("postal.sh");
    let body = src
        .split_once("_postal_print() {\n")
        .expect("_postal_print")
        .1;
    let py = body.split_once("python3 -c '").expect("its python").1;
    let at = |n: u32| py.lines().nth(n as usize - 1).expect("the line").trim();
    for (n, starts) in [
        (postal::line::ERROR, r#"print("Error: %s%s""#),
        (postal::line::PERSON, r#"if os.environ.get("PS")"#),
        (postal::line::EXIT, "sys.exit(3 if code in perm"),
        (postal::line::ASKED, r#"if d.get("asked"):"#),
        (postal::line::CARD, r#"print("Asked: a card"#),
        (postal::line::HINT, r#"hint = d.get("hint") or"#),
        (postal::line::CAPITAL, "print(hint[0].upper()"),
        (postal::line::HANDLE, r#"h = d.get("handle")"#),
        (postal::line::WORKSPACE, r#"ws = (d.get("workspace")"#),
        (postal::line::STATUS, r#"st = d.get("status")"#),
        (postal::line::SENT, r#"print("%s to %s: %s%s""#),
        (postal::line::BADGE, r#"if d.get("badge"):"#),
        (postal::line::RECORD, r#"if d.get("deliveryId"):"#),
        (postal::line::QUEUED, r#"if st == "queued":"#),
    ] {
        assert!(at(n).starts_with(starts), "line {n}: {}", at(n));
    }
}

#[test]
fn addresses() {
    assert!(is_postal("a::abc.postal.bot"));
    assert!(is_postal("A-1_::a-b.POSTAL.bot"));
    for not in [
        "::abc.postal.bot",
        "-a::abc.postal.bot",
        "a::ab.postal.bot",
        "a::-bc.postal.bot",
        "a::abc-.postal.bot",
        "a::a.c.postal.bot",
        "a::abc.postal.bot.x",
        "a:abc.postal.bot",
    ] {
        assert!(!is_postal(not), "{not}");
    }
    assert!(is_postal(&format!("a::a{}a.postal.bot", "b".repeat(61))));
    assert!(!is_postal(&format!("a::a{}a.postal.bot", "b".repeat(62))));
    assert_eq!(address("ws"), Address::Local);
    assert_eq!(address("a::b::c"), Address::Wire);
    assert_eq!(address(":::"), Address::Local);
    assert_eq!(address("::::"), Address::Wire);
    assert_eq!(address("a::x.k2.dev"), Address::Remote);
    assert_eq!(address("a::x"), Address::Local);
    assert_eq!(address("a::"), Address::Local);
    assert_eq!(address("a@b@c"), Address::Remote);
    assert_eq!(address("a@"), Address::Local);
}

#[test]
fn the_form() {
    let m = Msg {
        target: "my ws".into(),
        text: "a&b=c é".into(),
        from: "me/side".into(),
        wake: false,
        summary: "s".into(),
        ..Msg::default()
    };
    assert_eq!(
        send::params(&m, "abcdefgh"),
        "workspace=my%20ws&text=a%26b%3Dc%20%C3%A9&from=me%2Fside&wake=false&summary=s&idempotency_key=abcdefgh"
    );
    let m = Msg {
        target: "w".into(),
        text: "t".into(),
        from: "f".into(),
        command: "/c".into(),
        wake: true,
        wake_timeout: "9".into(),
        via: "talk".into(),
        session: "s1".into(),
        ..Msg::default()
    };
    assert_eq!(
        send::params(&m, "k"),
        "workspace=w&text=t&from=f&command=%2Fc&wake=true&wake_timeout=9&via=talk&session=s1&idempotency_key=k"
    );
}

#[test]
fn the_summary_hint() {
    assert_eq!(send::summary_hint(&"é".repeat(600), ""), None);
    assert!(send::summary_hint(&"é".repeat(601), "").is_some());
    assert_eq!(send::summary_hint(&"x".repeat(900), "s"), None);
}

fn parsed(text: &str) -> Value {
    json::parse(text).unwrap()
}

#[test]
fn the_answer() {
    assert_eq!(send::echo(b"{}"), b"{}\n");
    assert_eq!(send::echo(b""), b"\n");
    assert_eq!(send::echo(b"-n"), b"");
    assert_eq!(send::echo(b"-neE"), b"");
    assert_eq!(send::echo(b"-e"), b"\n");
    assert_eq!(send::echo(b"-"), b"-\n");
    assert_eq!(send::echo(b"-x"), b"-x\n");
    assert_eq!(send::gate_code(&parsed(r#"{"success":true}"#)), 0);
    assert_eq!(send::gate_code(&parsed(r#"{"success":1}"#)), 2);
    assert_eq!(send::gate_code(&parsed(r#"[1]"#)), 2);
    assert_eq!(
        send::gate_code(&parsed(r#"{"success":true,"error":{"code":"gated"}}"#)),
        3
    );
    assert_eq!(send::gate_code(&parsed(r#"{"error":{"code":"nope"}}"#)), 2);
    assert_eq!(send::gate_code(&parsed(r#"{"error":"gated"}"#)), 2);
    assert_eq!(
        send::moved_note(&parsed(r#"{"movedFrom":"old","addr":"new"}"#)).as_deref(),
        Some("note: old is now new")
    );
    assert_eq!(
        send::moved_note(&parsed(r#"{"movedFrom":"o d","addr":"n"}"#)).as_deref(),
        Some("note: o is now d n")
    );
    assert_eq!(
        send::moved_note(&parsed(r#"{"movedFrom":"a","addr":"a"}"#)),
        None
    );
    assert_eq!(
        send::moved_note(&parsed(r#"{"movedFrom":"a","addr":""}"#)),
        None
    );
    assert_eq!(
        send::moved_note(&parsed(r#"{"movedFrom":5,"addr":true}"#)).as_deref(),
        Some("note: 5 is now True")
    );
    assert!(send_once::replay_note(br#"{"duplicate":true}"#).is_some());
    assert!(send_once::replay_note(br#"{"replayed": true}"#).is_none());
}

#[test]
fn the_sender_readers() {
    assert_eq!(send::handle_of(br#"{"handle":"  acme \n"}"#), "acme");
    assert_eq!(send::handle_of(br#"{"handle":5}"#), "");
    assert_eq!(send::handle_of(br#"{"handle":null}"#), "");
    assert_eq!(send::handle_of(br#"["handle"]"#), "");
    assert_eq!(send::handle_of(b"not json"), "");
    assert_eq!(send::basename_of(br#"{"path":"/a/b/proj/"}"#), "proj");
    assert_eq!(send::basename_of(br#"{"path":"proj"}"#), "proj");
    assert_eq!(send::basename_of(br#"{"path":"/"}"#), "");
    assert_eq!(send::basename_of(br#"{"path":7}"#), "");
    assert_eq!(send::basename_of(br#"{}"#), "");
}

#[test]
fn who_words_and_reader() {
    assert_eq!(who::parse(&["m_1"]), Some(Ok(("m_1".into(), false))));
    assert_eq!(
        who::parse(&["--json", "m_1"]),
        Some(Ok(("m_1".into(), true)))
    );
    assert_eq!(who::parse(&["-h"]), None);
    let usage = |tail: &str| format!("Usage: {CLI_NAME} msg who <m_…> [--json]  ({tail})");
    assert_eq!(
        who::parse(&["m", "-x"]),
        Some(Err(usage("unknown flag -x")))
    );
    assert_eq!(who::parse(&["a", "b"]), Some(Err(usage("one id only"))));
    assert_eq!(who::parse(&[]), Some(Err(who::no_id())));
    assert_eq!(who::parse(&["", "b"]), Some(Ok(("b".into(), false))));
    pinned(
        "agent-template.sh",
        r#"[ -n "$id" ] || { echo "Usage: k2 msg who <m_…> [--json]  (the id in [from … · m_…])" >&2; exit 2; }"#,
    );

    let r = who::reader(
        br#"{"id":"m_1","via":"passport","sender":{"label":"acme"},"actor":"owner","onBehalfOf":{"value":"x@y"},"linked":true,"links":[{"id":"l1","scope":{"a":1}}]}"#,
        false,
    );
    assert_eq!(
        r.stdout,
        "Delivery m_1\n  via:     passport  (the sender agent's own session)\n  sender:  acme\n  actor:   owner\n  for:     x@y (app-asserted, not verified)\n  linked:  yes l1 {\"a\": 1}\n"
    );
    assert_eq!(r.exit, Exit::Ok);
    let r = who::reader(br#"{"id":"m_1"}"#, false);
    assert_eq!(
        r.stdout,
        "Delivery m_1\n  via:     None\n  sender:  -\n  actor:   None\n  linked:  no\n"
    );
    let r = who::reader(br#"{"error":{"code":"not_found","hint":"no such"}}"#, false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: not_found: no such\n", Exit::NotFound)
    );
    let r = who::reader(br#"{"error":{"hint":"bad token"}}"#, false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: bad token\n", Exit::Refused)
    );
    let r = who::reader(br#"{"error":"plain"}"#, false);
    assert_eq!((r.stderr.as_str(), r.exit), ("Error: plain\n", Exit::Error));
    let r = who::reader(b"", false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: daemon answered: \n", Exit::Error)
    );
    let r = who::reader(b"oops \xff", false);
    assert_eq!(r.stderr, "Error: daemon answered: oops \\udcff\n");
    let r = who::reader(br#"{"a":"\u00e9"}"#, true);
    assert_eq!(r.stdout, "{\n  \"a\": \"\\u00e9\"\n}\n");
    let r = who::reader(br#"[1]"#, false);
    assert_eq!(r.exit, Exit::Error);
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    let r = who::reader(br#"{"linked":1,"links":[{"x":1}]}"#, false);
    assert!(r.stderr.ends_with("KeyError: 'id'\n"), "{}", r.stderr);
    let r = who::reader(br#"{"error":{"code":5}}"#, false);
    assert!(r
        .stderr
        .ends_with("TypeError: unsupported operand type(s) for +: 'int' and 'str'\n"));
    let r = who::reader(br#"{"error":{"code":[],"hint":"h"}}"#, false);
    assert!(
        r.stderr.ends_with("TypeError: unhashable type: 'list'\n"),
        "{}",
        r.stderr
    );
}

fn clock(secs: i64) -> Option<String> {
    Some(format!("T{secs}"))
}

struct NoStdin;

impl verify::Stdin for NoStdin {
    fn read_all(&mut self) -> Vec<u8> {
        b"from stdin\n\n".to_vec()
    }
}

#[test]
fn verify_words_hashes_and_reader() {
    let p = |w: &[&str]| verify::parse(w, &mut NoStdin);
    let ask = p(&["m_1", "--text", "hi", "--json"]).unwrap();
    assert_eq!(
        (ask.code.as_str(), ask.text.as_deref(), ask.json),
        ("m_1", Some("hi"), true)
    );
    assert_eq!(
        p(&["m_1", "--text-file", "-"]).unwrap().text.as_deref(),
        Some("from stdin")
    );
    let pre = format!("{CLI_NAME} msg verify: ");
    assert_eq!(
        p(&["--text"]).unwrap_err(),
        (format!("{pre}--text needs the text"), Exit::Usage)
    );
    assert_eq!(
        p(&["--text-file", "/no/such/file"]).unwrap_err(),
        (format!("{pre}no file /no/such/file"), Exit::Usage)
    );
    assert_eq!(
        p(&["a", "b"]).unwrap_err().0,
        format!("{pre}one code only ({CLI_NAME} msg verify --help)")
    );
    assert_eq!(
        p(&["-q"]).unwrap_err().0,
        format!("{pre}unknown flag '-q' ({CLI_NAME} msg verify --help)")
    );
    let src = source("msg-verify.sh");
    for line in [
        "k2 msg verify: --text needs the text",
        "k2 msg verify: --text-file needs a file (- for stdin)",
        "k2 msg verify: no file $2",
        "k2 msg verify: unknown flag '$1' (k2 msg verify --help)",
        "k2 msg verify: one code only (k2 msg verify --help)",
        "Usage: k2 msg verify <code> [--text-file f]  (the m_… or p_… in the message's label)",
    ] {
        assert!(src.contains(&format!("echo \"{line}\" >&2")), "{line}");
    }

    assert_eq!(
        verify::hashes("m_abc1234567", "hello\r\nworld  \n\n"),
        "26c60a61d01db5836ca70fefd44a6a016620413c8ef5f259a6c5612d4f79d3b8"
    );
    assert_eq!(
        verify::hashes("m_abc1234567", "[from acme · m_abc1234567] hi there\n> quoted\n>\nplain"),
        "e5a084407b24c09ceb857e0509d29c2ba97c52d551e0dd13a2c664fec9a7daed,7e2e0d80bdec01c68bcdc342a659e67557fe6f3a679bb2b0bb4ee36c1b7b7707,990c677c40b657cecfc0b35170ee5e20955994683bd63b0ac29ec6ba40b0491c,c4f5837531b758a3b55c35ba437caf2425654cbbd46cd049676b999da203b96d"
    );
    assert_eq!(
        verify::hashes("p_00", "  [x] p_00 ] tail"),
        "b840719bdc42cc94ef9f4e05958efd6ffbcb21dbcaa3916db56f55152310d925,0c62f876ef1dea830de9f32c2f4b46dd6d74d50d15896e09ef5a2fcd4ac7e1d7"
    );

    let r = verify::reader(
        br#"{"code":"m_1","kind":"msg","sender":{"address":"a@b","handle":"acme","fingerprint":"ff"},"via":"passport","actor":"owner","onBehalfOf":{"value":"v"},"target":{"workspaceId":"w1"},"at":12.9,"delivered":true,"badge":"b2","landed":{"x":[1]},"content":"match","contentHint":"ok"}"#,
        false,
        clock,
    );
    assert_eq!(
        r.stdout,
        "code      m_1 (msg)\nfrom      a@b  via passport  [workspace acme, key ff]\nactor     owner\nfor       v (app-asserted, not verified)\nto        w1\nwhen      T12\ntyped in  yes\nbadge     b2\nlanded    {\"x\": [1]}\ncontent   match: ok\n"
    );
    assert_eq!(r.exit, Exit::Ok);
    let r = verify::reader(
        br#"{"content":"mismatch","contentHint":null}"#,
        false,
        clock,
    );
    assert_eq!(
        r.stdout,
        "code      None (None)\nfrom      ?  via None\nto        ?\nwhen      T0\ntyped in  no (the delivery did not reach the agent)\ncontent   mismatch: None\n"
    );
    assert_eq!(r.exit, Exit::Error);
    let r = verify::reader(br#"{"content":"mismatch"}"#, true, clock);
    assert_eq!(
        (r.stdout.as_str(), r.exit),
        ("{\n  \"content\": \"mismatch\"\n}\n", Exit::Error)
    );
    let r = verify::reader(
        br#"{"error":{"code":"forbidden","hint":"no"}}"#,
        false,
        clock,
    );
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        ("Error: forbidden: no\n", Exit::Refused)
    );
    let r = verify::reader(br#"{"at":"x"}"#, false, clock);
    assert!(r
        .stderr
        .ends_with("TypeError: 'str' object cannot be interpreted as an integer\n"));
    let r = verify::reader(br#"{"sender":{"handle":5}}"#, false, clock);
    assert_eq!(r.exit, Exit::Ok, "who is the handle: no workspace part");
    let r = verify::reader(br#"{"sender":{"address":"a","handle":5}}"#, false, clock);
    assert!(
        r.stderr
            .ends_with("TypeError: can only concatenate str (not \"int\") to str\n"),
        "{}",
        r.stderr
    );
}

#[cfg(unix)]
#[test]
fn the_local_clock_writes_a_zone() {
    let t = verify::now_local(0).unwrap();
    assert!(t.len() > "1970-01-01 00:00:00 ".len(), "{t}");
    assert!(t[..4].bytes().all(|b| b.is_ascii_digit()), "{t}");
}

/// Every name a line prints is the brand's: a `K2_BRAND_DIR=brand/xy`
/// build says `xy` and `XY`, and no line says k2, k5 or K2.
#[test]
fn the_lines_name_the_brand() {
    use super::postal;
    let cli = CLI_NAME;
    let product = PRODUCT_NAME;
    let stop = |w: &[&str]| match parse(w) {
        Parsed::Stop(text, _) => text,
        other => panic!("{other:?}"),
    };
    let lines: Vec<String> = vec![
        send::BARE_INBOX.to_string(),
        send::AGENT_RETIRED.to_string(),
        send::SESSION_USAGE.to_string(),
        send::SESSION_ONE_WORD.to_string(),
        send::SESSION_SECRET.to_string(),
        send::no_target(),
        send::no_text(),
        send::unknown_flag("-x"),
        send::wire_form("a::b::c"),
        send::summary_hint(&"x".repeat(601), "").unwrap(),
        send_once::once_line("/e", 3),
        send_once::tries_line("/e", 3, 2, "kkkkkkkk"),
        send_once::replay_note(br#"{"replayed":true}"#).unwrap(),
        postal::k2_only("--from", "a::b.postal.bot"),
        postal::unknown_flag("-x"),
        postal::usage("a::b.postal.bot"),
        postal::reader(
            br#"{"status":"delivered","to":"a::b.postal.bot","badge":"B","deliveryId":"m_1"}"#,
            false,
            false,
        )
        .stdout,
        who::no_id(),
        verify::parse(&["--text"], &mut NoStdin).unwrap_err().0,
        verify::parse(&["a", "b"], &mut NoStdin).unwrap_err().0,
        stop(&["ws", "hi", "--agent", "x"]),
        stop(&["ws"]),
    ];
    for line in &lines {
        let named = line.contains(&format!("{cli} ")) || line.contains(product);
        assert!(named, "names neither {cli} nor {product}: {line:?}");
    }
    assert!(send::BARE_INBOX.contains(&format!("  {cli} msg <workspace> --inbox-wake ")));
    assert!(send::no_target().contains(&format!("Usage: {cli} msg <workspace> ")));
    assert!(send::summary_hint(&"x".repeat(601), "")
        .unwrap()
        .contains(&format!("({product} uses its first line otherwise)")));
    if cli != "k2" {
        for line in &lines {
            for name in ["k2 ", "k5 ", "'k2", "K2", "K5"] {
                assert!(!line.contains(name), "{name:?} in {line:?}");
            }
        }
    }
}

#[test]
fn remote_addresses_and_readers() {
    use super::remote::{self, Target};
    for w in [&["agent::host.example", "t"][..], &["agent@host", "t"]] {
        assert!(matches!(parse(w), Parsed::Send(_)), "{w:?}");
    }
    let t = |a: &str, h: &str, at: bool| {
        Some(Target {
            agent: a.into(),
            host: h.into(),
            legacy_at: at,
        })
    };
    assert_eq!(
        remote::split("ada::box.example"),
        t("ada", "box.example", false)
    );
    assert_eq!(remote::split("ada@box"), t("ada", "box", true));
    assert_eq!(remote::split("a@b@c"), t("a@b", "c", true));
    assert_eq!(remote::split("@box"), t("", "box", true));
    for local in ["ada", "ada::box", "::box.x", "a::b::c", "ada@"] {
        assert_eq!(remote::split(local), None, "{local}");
    }
    assert_eq!(remote::slug("  Field   Notes \n"), "field-notes");
    assert_eq!(
        remote::chat("ada/Field Notes"),
        Ok(("ada".into(), Some("field-notes".into())))
    );
    assert_eq!(remote::chat("ada/a/b"), Err(remote::bad_chat()));
    assert_eq!(remote::chat("/x"), Err(remote::bad_chat()));

    let peers = |raw: &str, host: &str| remote::peers(raw.as_bytes(), host);
    assert_eq!(peers("", "h.k2.dev").unwrap(), "ERR\tparse");
    assert_eq!(peers("[]", "h.k2.dev").unwrap(), "NOFED");
    assert_eq!(
        peers(r#"{"error":{"code":"forbidden","hint":"no"}}"#, "h").unwrap(),
        "FORBIDDEN\tno"
    );
    assert_eq!(peers(r#"{"error":"off"}"#, "h").unwrap(), "NOFED\toff");
    assert_eq!(
        peers(r#"{"error":{"code":"x"}}"#, "h").unwrap(),
        "NOFED\t{'code': 'x'}"
    );
    let list = r#"{"peers":[{"subdomain":"other","label":"L"},{"subdomain":"box","fingerprint":"fp1","trust":"Trusted"}]}"#;
    assert_eq!(peers(list, "box.k2.dev").unwrap(), "OK\tfp1\tTrusted");
    assert_eq!(peers(list, "BOX").unwrap(), "OK\tfp1\tTrusted");
    assert_eq!(
        peers(list, "nope.example").unwrap(),
        "NONE\tother.k2.dev, box.k2.dev"
    );
    assert_eq!(
        peers(
            r#"{"peers":[{"base_url":"https://lan.local:8443/x","fingerprint":"f"}]}"#,
            "lan.local:8443"
        )
        .unwrap(),
        "OK\tf\t"
    );
    let raised = peers(r#"{"peers":[5]}"#, "h.x").unwrap_err();
    assert_eq!(
        raised.exception,
        "AttributeError: 'int' object has no attribute 'get'"
    );
    let raised = peers(r#"{"peers":[{"subdomain":5}]}"#, "h.x").unwrap_err();
    assert_eq!(raised.frames, vec![(39, "<module>"), (6, "bare")]);
    assert_eq!(
        remote::read3("OK\t\ttrusted"),
        ("OK".into(), "trusted".into(), String::new())
    );
    assert_eq!(
        remote::read3("OK\ta\tb\tc\t\nnext"),
        ("OK".into(), "a".into(), "b\tc".into())
    );

    let roster = |raw: &str, agent: &str| remote::roster(raw.as_bytes(), agent);
    let r = r#"{"roster":{"agents":[{"agent":"ada","workspace_id":"w1","aliases":["Lovelace"]},{"agent":"bob","workspace_id":"w2","workspace_name":"Bob Shop"}]}}"#;
    assert_eq!(roster(r, "Ada").unwrap(), "OK\tw1\t");
    assert_eq!(roster(r, "lovelace").unwrap(), "OK\tw1\tada");
    assert_eq!(roster(r, "bob_shop").unwrap(), "OK\tw2\tbob");
    assert_eq!(roster(r, "carl").unwrap(), "NONE\tada, bob");
    assert_eq!(roster(r#"{"error":null}"#, "a").unwrap(), "ERR\tNone");
    assert_eq!(roster("5", "a").unwrap(), "ERR\t");
    assert_eq!(
        roster(r#"{"roster":{"agents":[{"agent":null}]}}"#, "x")
            .unwrap_err()
            .exception,
        "TypeError: sequence item 0: expected str instance, NoneType found"
    );
    assert_eq!(
        roster(r#"{"roster":{"agents":[{"agent":"a","aliases":7}]}}"#, "x")
            .unwrap_err()
            .exception,
        "TypeError: 'int' object is not iterable"
    );
    let two = r#"{"roster":{"agents":[{"agent":"a","workspace_id":"w1","workspace_name":"A"},{"agent":"a","workspace_id":"w2"}]}}"#;
    assert_eq!(roster(two, "a").unwrap(), "AMBIG\tA (w1); ? (w2)");

    let sent = |raw: &str| remote::sent(raw.as_bytes());
    assert_eq!(sent(r#"{"status":"sent"}"#).unwrap(), "SENT");
    assert_eq!(
        sent(r#"{"status":"queued","reason":"delivery deferred to retry: conn  refused"}"#)
            .unwrap(),
        "QUEUED\tconn refused"
    );
    assert_eq!(
        sent(r#"{"error":{"code":"gated","hint":"on"}}"#).unwrap(),
        "GATE\tgated\ton"
    );
    assert_eq!(sent(r#"{"error":"x"}"#).unwrap(), "ERROR\tx");
    assert_eq!(
        sent(r#"{"status":"declined"}"#).unwrap(),
        "DECLINED\tdeclined by peer"
    );
    assert_eq!(sent("nope").unwrap(), "RAW");
    assert_eq!(sent("{}").unwrap(), "OTHER");
    assert_eq!(
        sent(r#"{"status":"queued","reason":5}"#)
            .unwrap_err()
            .exception,
        "AttributeError: 'int' object has no attribute 'startswith'"
    );
    assert_eq!(
        remote::gate_line("forbidden", "é"),
        r#"{"error":{"code":"forbidden","hint":"é"}}"#
    );
    assert_eq!(
        remote::body("f::w::a", "hé", "/p", "k"),
        r#"{"to": "f::w::a", "text": "h\u00e9", "from_workspace": "/p", "idempotency_key": "k"}"#
    );

    // The shell's lines, pinned: as the shell's `echo "..."` spells them.
    let src = source("remote-msg.sh");
    let esc = |t: &str| {
        as_shell(t)
            .replace(&format!("{PRODUCT_NAME} retries"), "K2 retries")
            .replace('"', "\\\"")
            .replace('`', "\\`")
    };
    let mut parts = vec![
        esc(&remote::empty_agent()),
        esc(&remote::bad_chat()),
        esc(&remote::at_hint("${agent}", "${host}")),
        esc(&remote::no_fed("$host")),
    ];
    for text in [
        remote::no_peer("$host", "$fp"),
        remote::no_peer("$host", ""),
        remote::no_agent("$agent", "$host", "$rdetail"),
        remote::no_agent("$agent", "$host", ""),
        remote::queued("${shown}", "${host}", "$sdetail"),
    ] {
        parts.extend(text.lines().map(esc));
    }
    for part in parts {
        assert!(src.contains(&part), "{part}");
    }
}

#[test]
fn signal_words_as_the_signal_form_reads_them() {
    use super::signal::{parse_with, Parsed as P, Signal, USAGE};
    let sig = |target: &str, kind: &str, payload: &str, from: &str, inbox: bool| {
        P::Send(Signal {
            target: target.into(),
            kind: kind.into(),
            payload: payload.into(),
            from: from.into(),
            inbox,
        })
    };
    assert_eq!(
        parse_with(&["ws", "--signal", "status"], Ok(None)),
        sig("ws", "status", "{}", "cli", false)
    );
    assert_eq!(
        parse_with(
            &["ws", "x", "--payload", "", "--signal", "", "--inbox-wake"],
            Ok(Some("ada".into()))
        ),
        sig("ws", "msg", "{}", "ada", true)
    );
    // --from wins, even empty; the first word is the target, whatever it is.
    assert_eq!(
        parse_with(
            &["--signal", "k", "ws", "--from", ""],
            Ok(Some("ada".into()))
        ),
        sig("--signal", "msg", "{}", "", false)
    );
    assert_eq!(
        parse_with(&["", "--signal", "k"], Ok(None)),
        P::Stop(USAGE.into(), Exit::Error)
    );
    for flag in ["--signal", "--payload", "--from"] {
        assert_eq!(
            parse_with(&["ws", "--signal", "k", flag], Ok(None)),
            P::Stop(send::needs_value(flag), Exit::Usage),
            "{flag}"
        );
    }
    assert_eq!(parse_with(&["ws", "--signal", "k"], Err(())), P::Shell);
    assert_eq!(
        parse_with(&["ws", "--signal", "k", "--from", "f"], Err(())),
        sig("ws", "k", "{}", "f", false)
    );
    assert!(USAGE.starts_with(&format!("Usage: {CLI_NAME} signal <target>")));
}

#[test]
fn signal_body_as_the_shell_python_builds_it() {
    use super::signal::{body, iso_utc, Signal};
    let s = |kind: &str, payload: &str| Signal {
        target: "t\"o".into(),
        kind: kind.into(),
        payload: payload.into(),
        from: "f\u{e9}".into(),
        inbox: false,
    };
    let head = r#"{"id": "I", "from": {"scope": "agent", "workspace": "W", "name": "f\u00e9"}, "to": {"scope": "agent", "workspace": "W", "name": "t\"o"}, "kind": "#;
    let tail = r#", "priority": "normal", "delivery": "live", "at": "A"}"#;
    for (kind, payload, want) in [
        (
            "msg",
            r#"{"text": 1.50}"#,
            r#"{"kind": "msg", "data": {"text": 1.5}}"#,
        ),
        (
            "msg",
            "not json",
            r#"{"kind": "msg", "data": {"text": "not json"}}"#,
        ),
        (
            "status",
            " \n",
            r#"{"kind": "status", "data": {"text": ""}}"#,
        ),
        (
            "presence",
            "{}",
            r#"{"kind": "presence", "data": {"state": "active"}}"#,
        ),
        (
            "ping",
            "[1, NaN]",
            r#"{"kind": "custom", "data": {"kind": "ping", "payload": [1, NaN]}}"#,
        ),
        (
            "ping",
            "oops",
            r#"{"kind": "custom", "data": {"kind": "ping", "payload": {"text": "oops"}}}"#,
        ),
    ] {
        assert_eq!(
            body(&s(kind, payload), "I", "W", "A").unwrap(),
            format!("{head}{want}{tail}"),
            "{kind} {payload}"
        );
    }
    let r = body(&s("status", "[1]"), "I", "W", "A").unwrap_err();
    assert_eq!(
        r.text(),
        "Traceback (most recent call last):\n  File \"<string>\", line 15, in <module>\n  File \"<string>\", line 7, in <lambda>\nAttributeError: 'list' object has no attribute 'get'\n"
    );
    let long = "9".repeat(5000);
    let r = body(&s("ping", &long), "I", "W", "A").unwrap_err();
    assert!(
        r.exception.starts_with("ValueError: Exceeds the limit"),
        "{}",
        r.exception
    );
    assert_eq!(iso_utc(0, 7), "1970-01-01T00:00:00.000007+00:00");
    assert_eq!(
        iso_utc(1_700_000_000, 123_456),
        "2023-11-14T22:13:20.123456+00:00"
    );
}

#[test]
fn signal_database_as_the_shell_finds_it() {
    use super::signal::{db_uri, home_db, workspace_id};
    use std::path::Path;
    assert_eq!(
        db_uri(Path::new("/h/a%b?c#d/k2.db")),
        "file:/h/a%25b%3fc%23d/k2.db?mode=rw"
    );
    let dir = std::env::temp_dir().join(format!("k5-signal-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Neither: k2.db. Only the old one: it.
    assert_eq!(home_db(&dir, None), dir.join("k2.db"));
    std::fs::write(dir.join("k2so.db"), b"x").unwrap();
    assert_eq!(home_db(&dir, None), dir.join("k2so.db"));
    // Both, no sqlite3, sizes alike: the old one.
    std::fs::write(dir.join("k2.db"), b"y").unwrap();
    assert_eq!(home_db(&dir, None), dir.join("k2so.db"));
    // No database: the project is its own id, whatever is on PATH.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(workspace_id(&empty, "/w/p"), "/w/p");
    std::fs::remove_dir_all(&dir).unwrap();
}

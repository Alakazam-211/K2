use super::*;
use std::path::Path;

fn src() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/permission.sh");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The text of the `python3 -c '...'` that follows `after` in
/// `permission.sh`, split as Python numbers its lines (line 1 is the
/// empty one after `-c '`).
fn python_after(after: &str) -> Vec<String> {
    let text = src();
    let body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?}"))
        .1;
    let start = body.find("python3 -c '").expect("python3 -c '") + "python3 -c '".len();
    let py = &body[start..];
    let end = py.find("\n'").expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

/// The 1-based line of `py` holding `needle`.
fn line_of(py: &[String], needle: &str) -> u32 {
    let i = py
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in the Python"));
    u32::try_from(i + 1).unwrap()
}

#[test]
fn the_usage_is_the_shell_s() {
    let text = src();
    let from = text
        .split_once("_permission_usage() {\n    cat <<'EOF'\n")
        .expect("the usage heredoc")
        .1;
    let usage = &from[..from.find("\nEOF\n").expect("its end") + 1];
    // The shell's text, with the names the brand gives.
    let branded = usage
        .replace("k2 ", &format!("{} ", k2_client::brand::CLI_NAME))
        .replace("K2", k2_client::brand::PRODUCT_NAME);
    assert_eq!(USAGE, branded);
}

/// Every name a line prints is the brand's (a `K2_BRAND_DIR=brand/xy`
/// build says `xy`).
#[test]
fn the_lines_name_the_brand() {
    let cli = k2_client::brand::CLI_NAME;
    let stop = |w: &[&str], passport: bool| match parse(w, passport) {
        Ask::Stop { lines, .. } => lines,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        stop(&["cancel"], false),
        vec![format!("Usage: {cli} permission cancel <id>")]
    );
    assert_eq!(
        stop(&["deny"], false),
        vec![format!("Usage: {cli} permission deny <id>")]
    );
    assert!(stop(&["pending"], true)[0]
        .contains(&format!("An agent asks with {cli} permission request.")));
    assert!(stop(&["nope"], false)[0].ends_with(&format!("unknown {cli} permission command: nope")));
    assert!(USAGE.starts_with(&format!("Usage: {cli} permission <command> (Beta)\n")));
    if cli != "k2" {
        for line in USAGE
            .lines()
            .chain(stop(&["request", "x"], false).iter().map(String::as_str))
        {
            assert!(!line.contains("k2 ") && !line.contains("K2"), "{line}");
        }
    }
    let denied = print_answer(
        "request",
        409,
        br#"{"error":"recently_denied","denied":{"id":"pr_1"}}"#,
        false,
        0,
    );
    assert!(denied
        .stderr
        .contains(&format!("add facts: {cli} permission update pr_1 --why")));
}

#[test]
fn the_lines_that_raise_are_the_shell_s() {
    let py = python_after("_permission_print() {");
    assert_eq!(line::P_DENIED_BY, line_of(&py, "msg = \"Denied by"));
    assert_eq!(line::P_PEOPLE_FOR, line_of(&py, "for p in r or []"));
    assert_eq!(line::P_PEOPLE_JOIN, line_of(&py, "\", \".join(out)"));
    assert_eq!(line::P_DRY_SHOWN, line_of(&py, "print(\"Shown to: %s\""));
    assert_eq!(line::P_ASKED, line_of(&py, "print(\"Asked: "));
    assert_eq!(line::P_THREAD, line_of(&py, "Card raised on your Thread"));
    let beta = python_after("raw=\"$(cli_send_json GET /cli/settings/get)\"");
    assert_eq!(line::B_GET, line_of(&beta, "s.get(\"permissionCards\")"));
    let approvers = python_after("_permission_people_only approvers");
    assert_eq!(line::A_GET, line_of(&approvers, "names = s.get("));
    assert_eq!(line::A_JOIN, line_of(&approvers, "\", \".join(names)"));
}

#[test]
fn the_shell_s_texts_are_these() {
    let text = src();
    for line in [
        "Error [person_only]: k2 permission $1 is for people, from a terminal outside K2 (or the K2 app). An agent asks with k2 permission request.",
        "Error [vault_confirm_required]: $id needs the same confirm Settings takes. Approve it in the K2 app, on your phone, or in Settings.",
        "Error [not_found]: no request $id waits on you (k2 permission pending)",
        "Error: no answer from the K2 daemon",
        "Usage: k2 permission request <action> [key=value \u{2026}] --why \\\"<one line>\\\"",
        "Error [bad_why]: --why is required: one line, at most 200 characters, saying what you need it for",
        "Error: $1 needs one or more logins, comma-separated ($1 ada,owner)",
        "Error: ${1%%=*} needs one or more logins, comma-separated (${1%%=*} ada,owner)",
        "Error: unexpected argument '$1' (values are key=value)",
        "Usage: k2 permission update <id> --why \\\"<new facts>\\\"",
        "Usage: k2 permission cancel <id>",
        "Usage: k2 permission $sub <id>",
        "Usage: k2 permission beta on|off|status",
        "Usage: k2 permission approvers [<login>[,<login>\u{2026}]|everyone]",
        "Usage: k2 permission agent <workspace> on|off",
        "Error: unknown k2 permission command: $sub",
        "Let agents ask with permission cards: %s",
        "Cards from agents ask: %s",
        "everyone who may answer (an owner or admin)",
        "Ask again after %s, or add facts: k2 permission update %s --why \\\"\u{2026}\\\"",
        "Card raised on your Thread%s: they answer it there (desktop, phone or web); you get one [permission:%s] turn with the answer.",
        "Already asked: %s (pending). Nothing new sent.",
        "Already on: nothing to ask. Run your command again.",
    ] {
        assert!(text.contains(line), "not in permission.sh: {line}");
    }
}

fn v(text: &str) -> Value {
    json::parse(text).unwrap()
}

#[test]
fn the_words_are_read_as_the_shell_reads_them() {
    let usage = |w: &[&str]| parse(w, false);
    assert_eq!(usage(&[]), Ask::Usage);
    assert_eq!(usage(&["help"]), Ask::Usage);
    assert_eq!(usage(&["request", "x", "--help"]), Ask::Usage);
    // `--why` with no value after it.
    assert!(matches!(
        usage(&["request", "x", "--why"]),
        Ask::Stop {
            exit: Exit::Usage,
            ..
        }
    ));
    // A value flag as the last word: the shell's `_need_value` line.
    assert_eq!(
        usage(&["update", "id", "--why"]),
        Ask::Stop {
            lines: vec!["Error: --why takes a value".into()],
            exit: Exit::Usage
        }
    );
    assert_eq!(
        usage(&["list", "--area"]),
        Ask::Stop {
            lines: vec!["Error: --area takes a value".into()],
            exit: Exit::Usage
        }
    );
    // An empty first word is no action; the next one is.
    match usage(&[
        "request",
        "",
        "compute.grant",
        "b=2",
        "a=1",
        "b=3",
        "--why",
        "w",
    ]) {
        Ask::Send { payload, .. } => assert_eq!(
            payload,
            r#"{"action": "compute.grant", "params": {"b": "3", "a": "1"}, "why": "w"}"#
        ),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        parse(&["pending"], true),
        Ask::Stop {
            exit: Exit::Refused,
            ..
        }
    ));
    assert_eq!(usage(&["beta", ""]), Ask::BetaStatus);
    assert_eq!(approvers(" EveryOne "), Vec::<Text>::new());
    assert_eq!(
        approvers(" ada , ,owner"),
        vec![Text::from("ada"), Text::from("owner")]
    );
}

#[test]
fn the_printer_says_what_the_shell_s_python_says() {
    let now = 1_000_000;
    let p = |verb: &str, status: u16, body: &str| {
        print_answer(verb, status, body.as_bytes(), false, now)
    };
    let a = p(
        "request",
        200,
        &format!(
            r#"{{"id":"pr_1","status":"pending","expiresAt":{},"routedTo":[{{"login":"ada","role":"owner"}},"bo"],"addr":"k2/x","text":"Use the node","duration":"2h"}}"#,
            now + 3 * 86_400 + 60
        ),
    );
    assert_eq!(
        a.stdout,
        "Asked: pr_1 (pending; expires in 3 days). Shown to: ada (owner), bo.\n\
         Card raised on your Thread (k2/x): they answer it there (desktop, phone or web); you get one [permission:pr_1] turn with the answer.\n\
         Card: Use the node Lasts: 2h.\n"
    );
    assert_eq!(a.exit, Exit::Ok);
    let e = p(
        "cancel",
        404,
        r#"{"error":{"code":"not_found","hint":"no such id"}}"#,
    );
    assert_eq!(
        (e.stderr.as_str(), e.exit),
        ("Error [not_found]: no such id\n", Exit::NotFound)
    );
    let e = p("status", 500, "  ");
    assert_eq!(e.stderr, "Error [error]: status 500\n");
    let r = p("request", 200, r#"{"id":"x","routedTo":5}"#);
    assert_eq!(
        r.stderr,
        crate::families::traceback(
            &[(line::P_ASKED, "<module>"), (line::P_PEOPLE_FOR, "people")],
            "TypeError: 'int' object is not iterable"
        )
    );
    assert_eq!(r.exit, Exit::Ok, "the status decides the exit");
    assert_eq!(expires_in(&v("null"), now), "");
    assert_eq!(
        expires_in(&v(&format!("{}", now + 10)), now),
        "expires in 1 hour"
    );
    assert_eq!(expires_in(&v(&format!("{}", now)), now), "expired");
    assert_eq!(
        expires_in(&v("1000000000000000000000000000000000000000000"), now),
        "expires in 11574074074074074074074074074074074062 days"
    );
    assert_eq!(when(&v("\"soon\"")), "soon");
}

#[test]
fn the_pending_lookup_is_the_shell_s() {
    let raw = br#"{"requests":[{"requestId":"a","cardId":"c1","addr":"k2/x"},{"requestId":"b","proof":{"kind":"confirm"}}]}"#;
    assert_eq!(
        lookup(raw, "a", true, "why not"),
        Lookup::Body(
            r#"{"addr": "k2/x", "id": "c1", "answer": "Allow", "reason": "why not"}"#.into()
        )
    );
    assert_eq!(lookup(raw, "b", true, ""), Lookup::Confirm);
    assert_eq!(
        lookup(raw, "b", false, ""),
        Lookup::Body(r#"{"addr": null, "id": null, "answer": "Deny"}"#.into())
    );
    assert_eq!(lookup(raw, "z", true, ""), Lookup::NotFound);
    assert_eq!(
        lookup(br#"{"requests":["x"]}"#, "a", true, ""),
        Lookup::NotFound
    );
    assert_eq!(lookup(b"", "a", true, ""), Lookup::NotFound);
}

/// Help wins (`_wants_help`): `-h` or `--help` anywhere before `--`, on
/// any verb and as any flag's value, is the usage; nothing is sent. After
/// `--` it is a word like any other.
#[test]
fn help_wins_before_any_verb_reads_its_words() {
    for passport in [false, true] {
        for flag in ["-h", "--help"] {
            for line in [
                vec![flag],
                vec!["status", flag],
                vec!["status", "pr_1", flag],
                vec!["cancel", flag],
                vec!["pending", flag],
                vec!["approve", flag],
                vec!["deny", "pr_1", "--reason", flag],
                vec!["update", flag],
                vec!["update", "--why", flag],
                vec!["update", "pr_1", "--why", flag],
                vec!["list", flag],
                vec!["list", "--area", flag],
                vec!["list", "--search", flag],
                vec!["request", "--why", flag],
                vec!["request", "compute.grant", "--to", flag],
                vec!["beta", flag],
                vec!["beta", "on", flag],
                vec!["beta", "off", flag],
                vec!["beta", "status", flag],
                vec!["approvers", flag],
                vec!["approvers", "ada", flag],
                vec!["agent", flag],
                vec!["agent", "acme", "on", flag],
                vec![flag, "--", "x"],
                vec!["nope", flag],
            ] {
                assert_eq!(parse(&line, passport), Ask::Usage, "{line:?} {passport}");
            }
        }
    }
    // After `--`, the verb reads it.
    assert_eq!(
        parse(&["status", "--", "--help"], false),
        send("status", false, STATUS, "id=--".into())
    );
    assert_eq!(
        parse(&["request", "compute.grant", "--", "-h"], false),
        unknown_flag("--")
    );
}

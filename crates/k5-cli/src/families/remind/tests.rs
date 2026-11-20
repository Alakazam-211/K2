use super::*;
use crate::families::traceback;
use k2_client::brand::CLI_NAME;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // The shell's command name, as this build names it.
    text.replace("k2 ", &format!("{CLI_NAME} "))
}

/// The body of the shell function `name` in `file`.
fn function(file: &str, name: &str) -> String {
    let text = source(file);
    let start = text
        .find(&format!("\n{name}() {{\n"))
        .unwrap_or_else(|| panic!("no {name} in {file}"));
    let body = &text[start + 1..];
    body[..body.find("\n}\n").expect("its end") + 2].to_string()
}

/// The single-quoted `python3 -c '…'` in `body`, its lines as `python3
/// -c` numbers them.
fn single_quoted(body: &str) -> Vec<String> {
    let start = body.find("python3 -c '").expect("a python3 -c '") + "python3 -c '".len();
    let py = &body[start..];
    let end = py.find('\'').expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

fn no_dirs(w: &str) -> Option<String> {
    Some(w.to_string())
}

fn form(words: &[&str]) -> Form {
    parse(words, &no_dirs).expect("a form")
}

fn ask(words: &[&str]) -> Ask {
    match form(words) {
        Form::Send(a) => a,
        other => panic!("{words:?}: {other:?}"),
    }
}

fn stopped(words: &[&str]) -> Vec<String> {
    match form(words) {
        Form::Stop(lines) => lines,
        other => panic!("{words:?}: {other:?}"),
    }
}

#[test]
fn the_router_s_arms() {
    let router = source("router.sh");
    assert!(router.contains("    remind)\n        shift\n        cmd_heartbeat_remind \"$@\"\n"));
    assert!(router
        .contains("    reminders)\n        shift\n        cmd_heartbeat_remind list \"$@\"\n"));
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains("        heartbeat|remind|reminders) echo \"heartbeat\" ;;"));
}

#[test]
fn the_forms() {
    assert_eq!(form(&[]), Form::Help);
    assert_eq!(form(&["help"]), Form::Help);
    assert_eq!(form(&["list", "-h"]), Form::Help);
    // With a time, the first word is text.
    let a = ask(&["help", "--in", "5m"]);
    assert_eq!(
        (a.verb, a.params[0].as_str()),
        (Kind::Add, "text=help&in=5m")
    );
    let a = ask(&["list", "--in", "1m"]);
    assert_eq!(a.params, vec!["text=list&in=1m".to_string()]);
    let a = ask(&["snooze", "r1", "10m"]);
    assert_eq!(
        (a.verb, a.endpoint, a.params[0].as_str()),
        (Kind::Snooze, SNOOZE, "name=r1&in=10m")
    );
    let a = ask(&[
        "snooze",
        "r1",
        "--at",
        "13:00",
        "--tz",
        "Europe/Paris",
        "--json",
    ]);
    assert_eq!(a.params[0], "name=r1&at=13%3A00&tz=Europe%2FParis");
    assert!(a.json);
    let a = ask(&[
        "Call",
        "",
        "the",
        "--in",
        "45m",
        "--name",
        "n",
        "--file",
        "f.md",
        "--session",
        "own",
        "--on",
        "2026-10-30",
        "--at",
        "09:00",
    ]);
    assert_eq!(
        a.params[0],
        "text=Call%20%20the&in=45m&at=09%3A00&on=2026-10-30&name=n&file=f.md&session=own"
    );
    let a = ask(&["", "x", "--in", "1m"]);
    assert_eq!(a.params[0], "text=x&in=1m");
    let a = ask(&["--file", "f", "--at", "9", "--workspace", ""]);
    assert_eq!(a.params[0], "text=&at=9&file=f&workspace=");
    let a = ask(&["-x", "é", "--in", "1m", "--project", "/w s"]);
    assert_eq!(a.params[0], "text=-x%20%C3%A9&in=1m&workspace=%2Fw%20s");
    let a = ask(&["list", "--all", "--workspace", "w", "--json"]);
    assert_eq!((a.verb, a.post, a.endpoint), (Kind::List, false, LIST));
    assert_eq!(
        a.params,
        vec!["all=1".to_string(), "workspace=w".to_string()]
    );
    assert!(ask(&["list"]).params.is_empty());
    let a = ask(&["show", "a b", "extra", "--json"]);
    assert_eq!(
        (a.verb, a.params.clone(), a.json),
        (Kind::Show, vec!["name=a%20b".to_string()], true)
    );
    let a = ask(&["cancel", "r1", "--workspace", "w"]);
    assert_eq!(
        (a.verb, a.post, a.params[0].as_str(), a.name.as_str()),
        (Kind::Cancel, true, "name=r1&workspace=w", "r1")
    );
    // A --workspace the binary can't resolve: the shell's.
    assert_eq!(parse(&["list", "--workspace", "w"], &|_| None), None);
}

#[test]
fn the_stops() {
    let add = ADD_USAGE.to_string();
    assert_eq!(
        stopped(&["x", "--in"]),
        vec!["Error: --in needs a value".to_string(), add.clone()]
    );
    assert_eq!(
        stopped(&["x", "--bogus"]),
        vec!["Error: unknown flag: --bogus".to_string(), add.clone()]
    );
    assert_eq!(
        stopped(&["--in", "5m"]),
        vec![
            "Error: a reminder needs some text, or --file".to_string(),
            add.clone()
        ]
    );
    assert_eq!(
        stopped(&["text", "--at", ""]),
        vec![
            "Error: say when: --in 45m, --at 13:00 or --on 2026-10-30".to_string(),
            add.clone()
        ]
    );
    assert_eq!(
        stopped(&["list", "--workspace"]),
        vec!["Error: --workspace needs a value".to_string()]
    );
    assert_eq!(
        stopped(&["list", "x"]),
        vec![
            "Error: unknown argument: x".to_string(),
            LIST_USAGE.to_string()
        ]
    );
    assert_eq!(stopped(&["show"]), vec![SHOW_USAGE.to_string()]);
    assert_eq!(
        stopped(&["show", "--x"]),
        vec!["Error: unknown flag: --x".to_string()]
    );
    assert_eq!(
        stopped(&["cancel", "--json"]),
        vec![CANCEL_USAGE.to_string()]
    );
    assert_eq!(
        stopped(&["cancel", "--project"]),
        vec!["Error: --project needs a value".to_string()]
    );
    assert_eq!(stopped(&["snooze", "r1"]), vec![SNOOZE_USAGE.to_string()]);
    assert_eq!(
        stopped(&["snooze", "a", "b", "c"]),
        vec!["Error: unexpected argument: c".to_string()]
    );
    assert_eq!(
        stopped(&["snooze", "--tz"]),
        vec!["Error: --tz needs a value".to_string()]
    );
    // Each line is the shell's.
    let text = source("remind.sh");
    for l in [
        ADD_USAGE,
        LIST_USAGE,
        SHOW_USAGE,
        SNOOZE_USAGE,
        CANCEL_USAGE,
    ] {
        let quoted = l.replace('"', "\\\"");
        assert!(text.contains(&quoted), "{l}");
    }
    for l in [
        r#"echo "Error: $1 needs a value" >&2; echo "$usage" >&2; exit 2"#,
        r#"echo "Error: unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;"#,
        r#"echo "Error: a reminder needs some text, or --file" >&2; echo "$usage" >&2; exit 2"#,
        r#"echo "Error: say when: --in 45m, --at 13:00 or --on 2026-10-30" >&2; echo "$usage" >&2; exit 2"#,
        r#"[ $# -ge 2 ] || { echo "Error: $1 needs a value" >&2; exit 2; }"#,
        r#"*) echo "Error: unknown argument: $1" >&2; echo "Usage: "#,
        r#"--*) echo "Error: unknown flag: $1" >&2; exit 2 ;;"#,
        r#"else echo "Error: unexpected argument: $1" >&2; exit 2"#,
        r#"echo "Reminder $name cancelled.""#,
    ] {
        assert!(text.contains(l), "{l}");
    }
}

#[test]
fn the_python_lines() {
    let add = single_quoted(&function("remind.sh", "_remind_add"));
    assert_eq!(add[(line::ADD_NAME - 1) as usize], r#"n = d["name"]"#);
    assert!(add[(line::ADD_FIRES - 1) as usize]
        .starts_with(r#"print("Reminder %s set: %s." % (n, d["fires"]))"#));
    let list = single_quoted(&function("remind.sh", "_remind_list"));
    assert_eq!(list[(line::LIST_FOR - 1) as usize], "for r in rows:");
    assert_eq!(
        list[(line::LIST_FIRE - 1) as usize],
        r#"    when = r.get("fireAt") or "?""#
    );
    assert_eq!(
        list[(line::LIST_TEXT - 1) as usize],
        r#"    text = (r.get("text") or "").replace("\n", " ")"#
    );
    assert_eq!(
        list[(line::LIST_ROW - 1) as usize],
        r#"    print("%-22s %-10s %s  %s" % (r["name"], state, when, text))"#
    );
    let show = single_quoted(&function("remind.sh", "_remind_show"));
    assert_eq!(
        show[(line::SHOW_HEAD - 1) as usize],
        r#"print("Reminder %s (%s)" % (r["name"], r["state"]))"#
    );
    assert_eq!(show[(line::SHOW_FOR - 1) as usize], "    for f in fires:");
    assert!(show[(line::SHOW_ROW - 1) as usize].starts_with(
        r#"        print("    %s  %s  %s" % (f.get("firedAt", f.get("fired_at", "")),"#
    ));
    let snooze = single_quoted(&function("remind.sh", "_remind_snooze"));
    assert_eq!(
        snooze[(line::SNOOZE_ROW - 1) as usize],
        r#"print("Reminder %s now fires %s." % (d["name"], d["fires"]))"#
    );
    let err = single_quoted(&function("heartbeat.sh", "_hb_print_err"));
    assert_eq!(
        err[(err_line::HINT_CHECK - 1) as usize].trim(),
        r#"if code in ("owner_only", "forbidden", "gated") and "ask your human" not in hint:"#
    );
    assert!(err[(err_line::HINT_JOIN - 1) as usize]
        .trim()
        .starts_with("hint = (hint + "));
    assert_eq!(
        err[(err_line::PRINT_HINT - 1) as usize].trim(),
        r#"print("Error: " + hint, file=sys.stderr)"#
    );
}

fn piped(body: &str) -> Piped {
    Piped {
        body: body.as_bytes().to_vec(),
        exit: Exit::Ok,
    }
}

#[test]
fn the_answers() {
    let add = ask(&["x", "--in", "1m"]);
    assert_eq!(
        answer(&add, &piped(r#"{"name":"r1","fires":"in 1m (13:01)"}"#)),
        Shown::out(format!(
            "Reminder r1 set: in 1m (13:01).\n  Cancel: {CLI_NAME} heartbeat remind cancel r1   Snooze: {CLI_NAME} heartbeat remind snooze r1 <when>\n"
        ))
    );
    let json = ask(&["x", "--in", "1m", "--json"]);
    assert_eq!(
        answer(&json, &piped("{\"name\":\"r1\"}\n\n")),
        Shown::out("{\"name\":\"r1\"}\n")
    );
    let list = ask(&["list"]);
    assert_eq!(
        answer(&list, &piped("[]")),
        Shown::out(format!(
            "No reminders. Set one: {CLI_NAME} heartbeat remind \"text\" --in 45m\n"
        ))
    );
    let long = "é".repeat(80);
    assert_eq!(
        answer(
            &list,
            &piped(&format!(
                r#"[{{"name":"r1","state":"pending","fireAt":"13:01","text":"a\nb"}},{{"name":"long-name-of-a-reminder-x","text":"{long}"}}]"#
            ))
        ),
        Shown::out(format!(
            "r1                     pending    13:01  a b\nlong-name-of-a-reminder-x None       ?  {}...\n",
            "é".repeat(67)
        ))
    );
    let show = ask(&["show", "r1"]);
    assert_eq!(
        answer(
            &show,
            &piped(r#"{"name":"r1","state":"fired","fireAt":"13:01","tz":"UTC","text":"hi","createdBy":"ann","firedAt":"13:02","waitReason":"busy","waitDetail":"in a turn","fires":[{"firedAt":"13:02","decision":"sent"},{"fired_at":"x","reason":"r"}]}"#)
        ),
        Shown::out("Reminder r1 (fired)\n  fires:   13:01 (UTC)\n  text:    hi\n  set by:  ann\n  fired:   13:02\n  waiting: busy (in a turn)\n  history:\n    13:02  sent  \n    x    r\n")
    );
    assert_eq!(
        answer(&show, &piped(r#"{"name":"r1","state":"pending"}"#)),
        Shown::out("Reminder r1 (pending)\n  fires:   None\n  text:    \n  set by:  ?\n")
    );
    let snooze = ask(&["snooze", "r1", "5m"]);
    assert_eq!(
        answer(&snooze, &piped(r#"{"name":"r1","fires":"at 13:06"}"#)),
        Shown::out("Reminder r1 now fires at 13:06.\n")
    );
    let cancel = ask(&["cancel", "r 1"]);
    assert_eq!(
        answer(&cancel, &piped(r#"{"ok":true}"#)),
        Shown::out("Reminder r 1 cancelled.\n")
    );
}

#[test]
fn the_errors() {
    let list = ask(&["list"]);
    let err = |body: &str| answer(&list, &piped(body));
    assert_eq!(
        err(""),
        Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error
        )
    );
    assert_eq!(
        err("\n\n"),
        Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error
        )
    );
    assert_eq!(
        err("  "),
        Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error
        )
    );
    assert_eq!(err("<html>"), Shown::err("Error: <html>\n", Exit::Error));
    assert_eq!(
        err(&"y".repeat(300)),
        Shown::err(format!("Error: {}\n", "y".repeat(200)), Exit::Error)
    );
    assert_eq!(
        err(r#"{"error":{"code":"owner_only","hint":"Owner only."}}"#),
        Shown::err(
            "Error: Owner only. — requires owner/admin — ask your human\n",
            Exit::Refused
        )
    );
    assert_eq!(
        err(r#"{"error":{"code":"forbidden","hint":"x — ask your human"}}"#),
        Shown::err("Error: x — ask your human\n", Exit::Refused)
    );
    assert_eq!(
        err(r#"{"error":{"code":"usage","message":"bad when"}}"#),
        Shown::err("Error: bad when\n", Exit::Usage)
    );
    assert_eq!(
        err(r#"{"error":{"code":"not_found"}}"#),
        Shown::err("Error: not_found\n", Exit::Error)
    );
    assert_eq!(
        err(r#"{"error":{}}"#),
        Shown::err("Error: error\n", Exit::Error)
    );
    assert_eq!(
        err(r#"{"error":"Invalid or missing auth Token"}"#),
        Shown::err("Error: Invalid or missing auth Token\n", Exit::Refused)
    );
    assert_eq!(
        err(r#"{"error":"nope"}"#),
        Shown::err("Error: nope\n", Exit::Error)
    );
    assert_eq!(
        err(r#"{"error":null}"#),
        Shown::err("Error: None\n", Exit::Error)
    );
    assert_eq!(
        err(r#"{"ok":false,"message":"m"}"#),
        Shown::err("Error: m\n", Exit::Error)
    );
    let r = err(r#"{"error":{"code":"gated","hint":5}}"#);
    assert_eq!(
        (r.stderr, r.exit),
        (
            traceback(
                &[(err_line::HINT_CHECK, "<module>")],
                "TypeError: argument of type 'int' is not iterable"
            ),
            Exit::Error
        )
    );
    let r = err(r#"{"error":{"code":"x","hint":[1]}}"#);
    assert_eq!(
        r.stderr,
        traceback(
            &[(err_line::PRINT_HINT, "<module>")],
            "TypeError: can only concatenate str (not \"list\") to str"
        )
    );
    // The call failed: its body is empty, so the reader's line and code.
    let failed = Piped {
        body: Vec::new(),
        exit: Exit::Curl(56),
    };
    assert_eq!(
        answer(&list, &failed),
        Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error
        )
    );
}

#[test]
fn where_the_python_raises() {
    let add = ask(&["x", "--in", "1m"]);
    let r = answer(&add, &piped(r#"{"fires":"x"}"#));
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            traceback(&[(line::ADD_NAME, "<module>")], "KeyError: 'name'").as_str(),
            Exit::Error
        )
    );
    let r = answer(&add, &piped("[1]"));
    assert_eq!(
        r.stderr,
        traceback(
            &[(line::ADD_NAME, "<module>")],
            "TypeError: list indices must be integers or slices, not str"
        )
    );
    let list = ask(&["list"]);
    let r = answer(&list, &piped("5"));
    assert_eq!(
        r.stderr,
        traceback(
            &[(line::LIST_FOR, "<module>")],
            "TypeError: 'int' object is not iterable"
        )
    );
    let r = answer(&list, &piped(r#"{"a":1}"#));
    assert_eq!(
        r.stderr,
        traceback(
            &[(line::LIST_FIRE, "<module>")],
            "AttributeError: 'str' object has no attribute 'get'"
        )
    );
    let r = answer(&list, &piped(r#"[{"name":"a"},{"text":5}]"#));
    assert_eq!(
        (r.stdout.as_str(), r.stderr),
        (
            "a                      None       ?  \n",
            traceback(
                &[(line::LIST_TEXT, "<module>")],
                "AttributeError: 'int' object has no attribute 'replace'"
            )
        )
    );
    let show = ask(&["show", "x"]);
    let r = answer(&show, &piped(r#"{"name":"x"}"#));
    assert_eq!(
        r.stderr,
        traceback(&[(line::SHOW_HEAD, "<module>")], "KeyError: 'state'")
    );
    let r = answer(&show, &piped(r#"{"name":"x","state":"s","fires":["a"]}"#));
    assert_eq!(
        r.stderr,
        traceback(
            &[(line::SHOW_ROW, "<module>")],
            "AttributeError: 'str' object has no attribute 'get'"
        )
    );
}

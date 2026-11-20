use super::py::big;
use super::watch::{self, Plan, Printer};
use crate::exit::Exit;
use crate::transport::{Channel, Error, Response};
use k2_client::brand::CLI_NAME;

fn shell(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The lines of the single-quoted Python that starts on the line after
/// `start` in mail-watch.sh, numbered as Python numbers them (line 1 is
/// what follows the opening quote).
fn python_after(start: &str) -> Vec<String> {
    let text = shell("mail-watch.sh");
    let from = text.find(start).unwrap_or_else(|| panic!("no {start:?}"));
    let body = &text[from + start.len()..];
    let end = body.find("\n'").expect("the closing quote");
    body[..end].split('\n').map(str::to_string).collect()
}

fn words(line: &[&str]) -> Option<watch::Ask> {
    watch::parse(line, "/w/proj")
}

fn plan(line: &[&str]) -> Plan {
    words(line).expect("native").plan
}

fn usage(line: &[&str]) -> String {
    match plan(line) {
        Plan::Usage(l) => l,
        other => panic!("{line:?}: {other:?}"),
    }
}

fn post(line: &[&str]) -> (&'static str, Vec<String>, Printer) {
    match plan(line) {
        Plan::Post {
            path,
            words,
            printer,
        } => (path, words, printer),
        other => panic!("{line:?}: {other:?}"),
    }
}

fn get(line: &[&str]) -> (&'static str, String, Printer) {
    match plan(line) {
        Plan::Get {
            path,
            query,
            printer,
        } => (path, query, printer),
        other => panic!("{line:?}: {other:?}"),
    }
}

#[test]
fn every_usage_line_is_the_shells() {
    let text = shell("mail-watch.sh");
    for line in [
        usage(&["--workspace"]),
        usage(&["inboxes", "x"]),
        usage(&["inboxes", "--add", "a", "--wake", "x"]),
        usage(&["vip", "add"]),
        usage(&["vip", "zz"]),
        usage(&["window"]),
        usage(&["window", "9-5", "x"]),
        usage(&["window", "9"]),
        usage(&["digest"]),
        usage(&["digest", "--every"]),
        usage(&["digest", "--every", "x"]),
        usage(&["outside-window"]),
        usage(&["snippets"]),
        usage(&["pause", "x"]),
        usage(&["log", "x"]),
        usage(&["what"]),
    ] {
        // The shell's text, with its variables and the command's name as
        // the shell spells them.
        let shell_form = line
            .replace(CLI_NAME, "k2")
            .replace("--workspace needs", "$1 needs")
            .replace("vip add <", "vip $verb <")
            .replace("watch snippets on", "watch $sub on")
            .replace("unknown flag: x", "unknown flag: $1")
            .replace("'x' is not", "'$2' is not")
            .replace("subcommand: what.", "subcommand: $sub.");
        assert!(
            text.contains(&format!("\"{shell_form}\"")),
            "{shell_form:?} is not in mail-watch.sh"
        );
    }
}

#[test]
fn the_flags_come_out_wherever_they_are() {
    let a = words(&["--json", "log", "--workspace", "/o b", "--limit", "3"]).unwrap();
    assert!(a.json);
    assert_eq!(
        a.plan,
        Plan::Get {
            path: watch::LOG,
            query: "project=%2Fo%20b&limit=3".into(),
            printer: Printer::Log
        }
    );
    // An empty --workspace is the shell's $PROJECT.
    assert_eq!(get(&["--project", "", "status"]).1, "project=%2Fw%2Fproj");
    assert_eq!(
        get(&[]),
        (watch::WATCH, "project=%2Fw%2Fproj".into(), Printer::Status)
    );
    let none = watch::parse(&["log"], "").unwrap();
    assert_eq!(
        none.plan,
        Plan::Get {
            path: watch::LOG,
            query: String::new(),
            printer: Printer::Log
        }
    );
    assert_eq!(usage(&["--project"]), "Error: --project needs a workspace");
}

#[test]
fn the_posts_and_their_words() {
    assert_eq!(
        post(&["on"]),
        (
            watch::SET,
            vec!["project=/w/proj".into(), "on=true".into()],
            Printer::Status
        )
    );
    assert_eq!(post(&["off"]).1[1], "on=false");
    assert_eq!(
        post(&["inboxes", "--add", "a@x.io", "--remove", "b@x.io"]).1,
        vec![
            "project=/w/proj".to_string(),
            r#"add=[{"address": "a@x.io", "wake": "vip"}]"#.into(),
            r#"remove=["b@x.io"]"#.into(),
        ]
    );
    assert_eq!(
        post(&["inboxes", "--add", "é", "--wake", "all"]).1[1],
        r#"add=[{"address": "\u00e9", "wake": "all"}]"#
    );
    // No --add or --remove: the status, and --wake is not checked.
    assert_eq!(get(&["inboxes", "--wake", "x"]).2, Printer::Status);
    assert_eq!(
        post(&["vip", "rm", "a", "", "@b"]),
        (
            watch::VIP,
            vec!["project=/w/proj".into(), r#"remove=["a", "", "@b"]"#.into()],
            Printer::VipSet
        )
    );
    assert_eq!(get(&["vip", ""]).2, Printer::VipList);
    assert_eq!(
        post(&[
            "window",
            "09:00-17:00-x",
            "--days",
            "mon,,tue",
            "--tz",
            "A/B"
        ])
        .1[1],
        r#"window={"start": "09:00", "end": "17:00-x", "days": ["mon", "tue"], "tz": "A/B"}"#
    );
    assert_eq!(
        post(&["window", "9-5", "--days", "x", "--weekdays"]).1[1],
        r#"window={"start": "9", "end": "5", "days": ["mon", "tue", "wed", "thu", "fri"]}"#
    );
    assert_eq!(
        post(&["window", "off", "--tz", "x"]).1[1],
        r#"window={"off":true}"#
    );
    assert_eq!(
        post(&["digest", "--every", "1H30m"]).1[1],
        "digestEverySecs=5400"
    );
    assert_eq!(
        post(&["outside-window", "hold"]).1[1],
        "vipOutsideWindow=false"
    );
    assert_eq!(post(&["vip-line", "on"]).1[1], "vipSessionLine=true");
    assert_eq!(post(&["snippets", "off"]).1[1], "snippets=false");
    assert_eq!(
        post(&["pause", "--all", "--for", "2h"]),
        (
            watch::PAUSE_ALL,
            vec![
                "project=/w/proj".into(),
                "for=2h".into(),
                "resume=false".into()
            ],
            Printer::Pause
        )
    );
    // pause's words are quoted: a path with a space stays one word.
    let w = watch::parse(&["pause", "--resume"], "/a b").unwrap();
    assert_eq!(
        w.plan,
        Plan::Post {
            path: watch::PAUSE,
            words: vec!["project=/a b".into(), "resume=true".into()],
            printer: Printer::Pause
        }
    );
    assert_eq!(
        plan(&["digest", "instructions", "x"]),
        Plan::Print(watch::DIGEST_INSTRUCTIONS)
    );
}

#[test]
fn the_shell_answers_what_it_alone_can() {
    for line in [
        &["inboxes", "--add"][..],
        &["inboxes", "--remove"],
        &["inboxes", "--wake"],
        &["window", "9-5", "--days"],
        &["window", "9-5", "--tz"],
        &["pause", "--for"],
        &["log", "--limit"],
        &["log", "--limit", "1 2"],
        &["log", "--limit", "1#"],
        &["digest", "--every", "６０m"],
    ] {
        assert!(words(line).is_none(), "{line:?}");
    }
    assert!(watch::parse(&["on"], "/a*").is_none());
    // A GET never splits the path: no pattern to expand.
    assert!(watch::parse(&["status"], "/a*").is_some());
}

#[test]
fn an_unquoted_workspace_splits_at_white_space() {
    assert_eq!(watch::unquoted(""), Some(vec![]));
    assert_eq!(
        watch::unquoted(" /a  b\tc=d\n"),
        Some(vec![
            "project=".into(),
            "/a".into(),
            "b".into(),
            "c=d".into()
        ])
    );
    assert_eq!(watch::unquoted("/x[1]"), None);
}

#[test]
fn the_body_is_mw_bodys() {
    let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        watch::body(&w(&["project=12", "on=true", "for=2", "x=NaN", "y=1e2", "z= [1,{\"a\":\"é\"}] ", "for=[1"])),
        (
            r#"{"project": "12", "on": true, "for": "[1", "x": NaN, "y": 100.0, "z": [1, {"a": "\u00e9"}]}"#.into(),
            None
        )
    );
    let (b, raise) = watch::body(&w(&["project=/a", "b", "on=true"]));
    assert_eq!(b, "");
    assert_eq!(
        raise.expect("a raise").text(),
        "Traceback (most recent call last):\n  File \"<string>\", line 5, in <module>\nValueError: not enough values to unpack (expected 2, got 1)\n"
    );
    let py = python_after("    python3 -I -c '\nimport json, sys\nout = {}\n");
    assert_eq!(py[1].trim(), r#"k, v = pair.split("=", 1)"#);
    assert_eq!(watch::BODY_SPLIT_LINE, 5);
}

#[test]
fn durations_are_the_pythons() {
    assert_eq!(watch::duration("60m").as_deref(), Some("3600"));
    assert_eq!(watch::duration("1d1h1m1s").as_deref(), Some("90061"));
    assert_eq!(watch::duration("007S").as_deref(), Some("7"));
    for bad in ["60", "m", "1x", "1m ", "", "1mm"] {
        assert_eq!(watch::duration(bad), None, "{bad:?}");
    }
    let big = format!("{}d", "9".repeat(30));
    assert_eq!(
        watch::duration(&big).as_deref(),
        Some(big::mul_small(&"9".repeat(30), 86400).as_str())
    );
    assert_eq!(watch::duration(&format!("{}s", "1".repeat(4301))), None);
}

#[test]
fn big_numbers() {
    assert_eq!(big::add("999", "1"), "1000");
    assert_eq!(
        big::mul_small("123456789123456789", 86400),
        "10666666580266666569600"
    );
    assert_eq!(big::floor_div("3600", 60), "60");
    assert_eq!(big::floor_div("-1", 60), "-1");
    assert_eq!(big::floor_div("-60", 60), "-1");
    assert_eq!(big::floor_div("-61", 60), "-2");
    assert_eq!(big::floor_div("59", 60), "0");
}

fn answer(status: u16, body: &str) -> Result<Response, Error> {
    Ok(Response {
        status,
        body: body.as_bytes().to_vec(),
        channel: Channel::Tcp,
        permission: None,
        server_version: None,
    })
}

#[test]
fn the_reply_holds_curls_lines() {
    assert_eq!(watch::reply(&answer(200, "{}"), 0), b"{}\n200");
    assert_eq!(watch::reply(&answer(200, "{\0}"), 2), b"\n000\n000{}\n200");
    let gone = Err(Error::Unreachable {
        endpoint: "/x".into(),
        err: std::io::Error::other("refused"),
    });
    assert_eq!(watch::reply(&gone, 3), b"\n000\n000\n000\n000\n000");
}

fn read(reply: &str, printer: Printer, json: bool) -> (String, String, Exit) {
    let s = watch::reader(reply.as_bytes(), printer, json);
    (s.stdout, s.stderr, s.exit)
}

const STATUS: &str = r#"{"enabled":true,"inboxes":[{"address":"a@x.io","wake":"vip","needsRelink":true},{"address":"b@x.io","wake":"all","lastError":"auth"},{"address":"c@x.io"}],"vips":["boss@x.io","@x.io"],"digest":{"start":"09:00","end":"17:00","days":["mon","tue"],"tz":"A/B","everySeconds":5400,"nextFireAt":"2026-11-20T09:00:00Z"},"vipOutsideWindow":false,"vipSessionLine":true,"snippets":false,"pausedUntil":"2026-11-21","last24h":{"vipDelivered":2,"failedAuth":null}}"#;

#[test]
fn the_status_prints_as_the_shell_prints_it() {
    let (out, err, exit) = read(&format!("{STATUS}\n200"), Printer::Status, false);
    assert_eq!(err, "");
    assert_eq!(exit, Exit::Ok);
    assert_eq!(
        out,
        "Mail notices: on\n\
         Inboxes chosen: a@x.io (vip), b@x.io (all)\n\
         Can also choose: c@x.io\n\
         VIPs: boss@x.io, @x.io\n\
         Check window: 09:00-17:00 mon,tue (A/B)\n\
         Digest every 90 min; next: 2026-11-20T09:00:00Z\n\
         VIP mail outside the window: hold; session line: on; snippets: off\n\
         Paused until 2026-11-21\n\
         Stopped: a@x.io refused its login. Reconnect the inbox and it starts again.\n\
         Last poll problem for b@x.io: auth\n\
         Last 24h: 2 VIP delivered, 0 digested, None failed authentication\n"
    );
    let bare = r#"{"enabled":0,"inboxes":[],"vips":"ab","digest":{"everySeconds":"-61"},"vipOutsideWindow":1,"vipSessionLine":0,"snippets":1}"#;
    let (out, _, exit) = read(&format!("{bare}\n201"), Printer::Status, false);
    assert_eq!(exit, Exit::Ok);
    assert_eq!(
        out,
        "Mail notices: off\nInboxes chosen: none\nVIPs: a, b\nCheck window: off\n\
         Digest every -2 min; next: -\n\
         VIP mail outside the window: deliver; session line: off; snippets: on\n\
         Last 24h: 0 VIP delivered, 0 digested, 0 failed authentication\n"
    );
    let (out, _, _) = read(
        "{\"a\":\"\u{e9}\",\"b\":[1,2.5]}\n200",
        Printer::Status,
        true,
    );
    assert_eq!(
        out,
        "{\n  \"a\": \"\\u00e9\",\n  \"b\": [\n    1,\n    2.5\n  ]\n}\n"
    );
}

#[test]
fn the_status_printers_raises() {
    let tb = |frames: &[(u32, &str)], exc: &str| {
        let mut s = String::from("Traceback (most recent call last):\n");
        for (l, n) in frames {
            s.push_str(&format!("  File \"<string>\", line {l}, in {n}\n"));
        }
        s + exc + "\n"
    };
    let (out, err, exit) = read("{}\n200", Printer::Status, false);
    assert_eq!((out.as_str(), exit), ("", Exit::Error));
    assert_eq!(
        err,
        tb(&[(10, "<module>"), (4, "<module>")], "KeyError: 'enabled'")
    );
    let (out, err, _) = read(
        r#"{"enabled":1,"inboxes":[{"wake":1}]}
200"#,
        Printer::Status,
        false,
    );
    assert_eq!(out, "Mail notices: on\n");
    assert_eq!(
        err,
        tb(
            &[(10, "<module>"), (7, "<module>"), (7, "<genexpr>")],
            "KeyError: 'address'"
        )
    );
    let (_, err, _) = read(
        r#"{"enabled":1,"inboxes":[],"vips":[1]}
200"#,
        Printer::Status,
        false,
    );
    assert!(
        err.ends_with(
            "line 9, in <module>\nTypeError: sequence item 0: expected str instance, int found\n"
        ),
        "{err}"
    );
    let (_, err, _) = read("[1]\n200", Printer::Status, false);
    assert!(err.ends_with("TypeError: list indices must be integers or slices, not str\n"));
    let (_, err, _) = read(
        r#"{"enabled":1,"inboxes":[],"vips":[],"digest":{"everySeconds":1e400}}
200"#,
        Printer::Status,
        false,
    );
    assert!(
        err.ends_with(
            "line 15, in <module>\nOverflowError: cannot convert float infinity to integer\n"
        ),
        "{err}"
    );
    let (_, err, _) = read(
        r#"{"enabled":1,"inboxes":[],"vips":[],"vipOutsideWindow":1,"vipSessionLine":1,"snippets":1,"pausedUntil":5}
200"#,
        Printer::Status,
        false,
    );
    assert!(
        err.ends_with("TypeError: can only concatenate str (not \"int\") to str\n"),
        "{err}"
    );
}

#[test]
fn the_status_printer_is_the_shells_source() {
    let py = python_after("_mw_status_print='");
    let want = [
        (4, "print(\"Mail notices: \""),
        (5, "ibx = [i for i in d[\"inboxes\"]"),
        (6, "others = [i[\"address\"]"),
        (7, "print(\"Inboxes chosen: \""),
        (8, "if others: print(\"Can also choose: \""),
        (9, "print(\"VIPs: \""),
        (10, "dg = d.get(\"digest\") or {}"),
        (12, "    days = dg.get(\"days\") or []"),
        (13, "    win = "),
        (14, "    print(\"Check window: "),
        (15, "    print(\"Digest every "),
        (16, "print(\"VIP mail outside the window: \""),
        (17, "if d.get(\"pausedUntil\"): print("),
        (19, "    if i.get(\"needsRelink\"): print("),
        (20, "    elif i.get(\"lastError\"): print("),
        (21, "c = d.get(\"last24h\") or {}"),
        (22, "print(\"Last 24h: "),
    ];
    for (n, start) in want {
        assert!(
            py[n - 1].starts_with(start),
            "line {n} is {:?}, not {start:?}",
            py[n - 1]
        );
    }
    let outer = python_after("STATUS=\"$status\" PYSRC=\"$src\" BODY=\"$body\" python3 -c '");
    assert_eq!(
        outer[watch::EXEC_LINE as usize - 1].trim(),
        "exec(os.environ[\"PYSRC\"])"
    );
    assert!(outer[watch::ERR_LINE as usize - 1]
        .trim()
        .starts_with("print(\"Error: \" + (err.get(\"hint\")"));
    // From line 2: the start takes line 1's newline.
    let log = python_after("_mw_python '\n");
    assert!(log[2].starts_with("for r in d[\"log\"] or []:"));
    assert!(log[4].starts_with("if not d[\"log\"]:"));
}

#[test]
fn the_other_printers() {
    assert_eq!(
        read("{\"vips\":[]}\n200", Printer::VipList, false).0,
        "No VIPs.\n"
    );
    assert_eq!(
        read("{\"vips\":[\"a\",\"b\"]}\n200", Printer::VipList, false).0,
        "a\nb\n"
    );
    assert_eq!(
        read("{\"vips\":[\"a\",\"b\"]}\n200", Printer::VipSet, false).0,
        "VIPs: a, b\n"
    );
    assert_eq!(
        read("{\"vips\":[]}\n200", Printer::VipSet, false).0,
        "VIPs: none\n"
    );
    assert_eq!(
        read("{\"pausedUntil\":null}\n200", Printer::Pause, false).0,
        "Resumed.\n"
    );
    assert_eq!(
        read("{\"pausedUntil\":\"\"}\n200", Printer::Pause, false).0,
        "Paused until \n"
    );
    let log = r#"{"log":[{"receivedAt":"t1","class":"vip","state":"delivered","from":"a@x","inbox":"me@x"},{"receivedAt":"t2","class":"digest-long","state":"held","from":"b","inbox":"c","reason":"window"}]}"#;
    assert_eq!(
        read(&format!("{log}\n200"), Printer::Log, false).0,
        "t1  vip      delivered  a@x -> me@x\nt2  digest-long held       b -> c  (window)\n"
    );
    assert_eq!(
        read("{\"log\":null}\n200", Printer::Log, false).0,
        "Nothing yet.\n"
    );
    assert_eq!(
        read("{\"log\":[]}\n200", Printer::Log, true).0,
        "{\n  \"log\": []\n}\n"
    );
}

#[test]
fn errors_are_mw_pythons() {
    assert_eq!(
        read("\n000\n000", Printer::Status, false),
        (String::new(), "Error: \n000\n".into(), Exit::Error)
    );
    let long = "x".repeat(300);
    assert_eq!(
        read(&format!("{long}\n200"), Printer::Status, false).1,
        format!("Error: {}\n", "x".repeat(200))
    );
    assert_eq!(
        read(
            r#"{"error":{"code":"owner_only","hint":"ask"}}
400"#,
            Printer::Status,
            false
        ),
        (String::new(), "Error: ask\n".into(), Exit::Refused)
    );
    assert_eq!(
        read(
            r#"{"error":{"code":"x","message":"m"}}
403"#,
            Printer::Status,
            false
        )
        .2,
        Exit::Refused
    );
    assert_eq!(
        read(
            r#"{"error":{"code":"usage"}}
400"#,
            Printer::Status,
            false
        ),
        (String::new(), "Error: usage\n".into(), Exit::Usage)
    );
    assert_eq!(
        read(
            r#"{"ok":false,"hint":"h"}
200"#,
            Printer::Status,
            false
        ),
        (String::new(), "Error: h\n".into(), Exit::Error)
    );
    assert_eq!(read("{}\n500", Printer::Status, false).1, "Error: error\n");
    assert_eq!(
        read(
            r#"{"error":"down"}
500"#,
            Printer::Status,
            false
        )
        .1,
        "Error: down\n"
    );
    assert_eq!(
        read(
            r#"{"error":null}
500"#,
            Printer::Status,
            false
        )
        .1,
        "Error: None\n"
    );
    assert_eq!(
        read("[1, 2]\n500", Printer::Status, false).1,
        "Error: [1, 2]\n"
    );
    let (_, err, exit) = read(
        r#"{"error":{"hint":5}}
500"#,
        Printer::Status,
        false,
    );
    assert_eq!(exit, Exit::Error);
    assert_eq!(
        err,
        "Traceback (most recent call last):\n  File \"<string>\", line 15, in <module>\nTypeError: can only concatenate str (not \"int\") to str\n"
    );
}

#[test]
fn int_is_pythons() {
    use crate::out::json::Value;
    let s = |t: &str| Value::Str(t.into());
    assert_eq!(super::py::py_int(&s(" -1_000 ")).as_deref(), Ok("-1000"));
    assert_eq!(super::py::py_int(&s("+007")).as_deref(), Ok("7"));
    assert_eq!(super::py::py_int(&Value::Float(-2.9)).as_deref(), Ok("-2"));
    assert_eq!(
        super::py::py_int(&Value::Float(1e20)).as_deref(),
        Ok("100000000000000000000")
    );
    assert_eq!(super::py::py_int(&Value::Bool(true)).as_deref(), Ok("1"));
    assert_eq!(
        super::py::py_int(&s("1__0")),
        Err("ValueError: invalid literal for int() with base 10: '1__0'".into())
    );
    assert_eq!(
        super::py::py_int(&Value::Array(vec![])),
        Err("TypeError: int() argument must be a string, a bytes-like object or a real number, not 'list'".into())
    );
}

/// `_mail_py`'s script as python3 compiles it: line 1 is the one after
/// the command line that feeds the heredoc.
fn mail_py_script() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/mail.sh");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let body = text
        .split_once("python3 -c \"$_K2_PY_STDIN_BOOT\"\n")
        .expect("_mail_py's command line")
        .1;
    let end = body.find("\nPYEOF\n").expect("the heredoc's end");
    body[..end].split('\n').map(str::to_string).collect()
}

#[test]
fn the_lines_that_raise_are_the_shell_s() {
    let s = mail_py_script();
    assert_eq!(s[0], "import base64, json, os, socket, sys, time");
    let pins: &[(u32, &str)] = &[
        (super::core::line::DIE, "def die(err, exit_code=None, permission=None):"),
        (super::core::line::DIE_CODE, "code = err.get(\"code\", \"error\")"),
        (super::core::line::DIE_HINT, "hint = err.get(\"hint\", \"\")"),
        (super::core::line::DIE_IN, "if code in (\"forbidden\", \"owner_only\") and \"ask "),
        (super::core::line::DIE_CONCAT, "hint = (hint + \" — \" if hint else \"\") + \"require"),
        (super::core::line::DIE_CAP, "if err.get(k):"),
        (super::core::line::REQUEST_FLOAT, "timeout = float(env_t)"),
        (super::core::line::REQUEST_READ, "raw = e.read().decode(\"utf-8\", \"replace\")"),
        (super::core::line::CALL_REQUEST, "d = request(method, route, params, body, timeout"),
        (super::core::line::CALL_TIMEOUT, "if timeout_hint and (d.get(\"error\") or {}).get(\""),
        (super::core::line::CALL_DIE_TIMEOUT, "die({\"code\": \"timeout\", \"hint\": timeout_hint(d[\""),
        (super::core::line::CALL_ERROR, "if d.get(\"error\"):"),
        (super::core::line::CALL_DIE_ERROR, "die(d[\"error\"], permission=d.get(\"permission\"))"),
        (super::core::line::CALL_OK, "if not d.get(\"ok\"):"),
        (super::core::line::FMT_ADDR, "name = a.get(\"name\")"),
        (super::core::line::FMT_ADDR_EMAIL, "email = a.get(\"address\") or \"?\""),
        (super::core::line::FMT_ADDRS, "return \", \".join(fmt_addr(a) for a in (lst or []"),
        (super::core::line::CAP_USED, "used = (cap or {}).get(\"used\", \"?\")"),
        (super::core::line::CAP_LIMIT, "limit = (cap or {}).get(\"cap\", 0)"),
        (super::core::line::FULL_ID, "print(\"message  : %s\" % m.get(\"id\", \"\"))"),
        (super::core::line::FULL_ADDRESS, "print(\"mailbox  : %s\" % m.get(\"address\", \"?\"))"),
        (super::core::line::FULL_FROM, "print(\"from     : %s\" % fmt_addr(m.get(\"from\")))"),
        (super::core::line::FULL_TO, "print(\"to       : %s\" % (fmt_addrs(m.get(\"to\")) "),
        (super::core::line::FULL_CC_IF, "if m.get(\"cc\"):"),
        (super::core::line::FULL_CC, "print(\"cc       : %s\" % fmt_addrs(m.get(\"cc\")))"),
        (super::core::line::FULL_SUBJECT, "print(\"subject  : %s\" % (m.get(\"subject\") or \"(n"),
        (super::core::line::FULL_DATE, "print(\"date     : %s  (%s)\" % (m.get(\"date\") or "),
        (super::core::line::FULL_AUTH, "ar = m.get(\"authResults\") or {}"),
        (super::core::line::FULL_ANY, "if any(ar.get(k) for k in (\"spf\", \"dkim\", \"dmarc"),
        (super::core::line::FULL_AUTH_LINE, "print(\"auth     : %s\" % \"  \".join("),
        (super::core::line::FULL_ATTS, "atts = m.get(\"attachments\") or []"),
        (super::core::line::FULL_FOR, "for a in atts:"),
        (super::core::line::FULL_ATT, "print(\"  %s. %s (%s, %s bytes)\" % (a.get(\"index\""),
        (super::core::line::FULL_FETCH, "print(\"  fetch one: k2 mail attachments %s --get"),
        (super::core::line::FULL_TEXT, "print(m.get(\"text\") or \"\")"),
        (super::core::line::FULL_HTML_IF, "if m.get(\"html\"):"),
        (super::core::line::FULL_HTML, "print(m.get(\"html\"))"),
        (super::core::line::ROW_FIRST, "print(\"%s  %-17s %-11s to %s — %s\" % ("),
        (super::core::line::ROW_BCC_IF, "if r.get(\"bcc\"):"),
        (super::core::line::ROW_BCC, "print(\"    bcc: %s\" % \", \".join(r[\"bcc\"]))"),
        (super::core::line::ROW_POLICY_IF, "if r.get(\"policyBcc\"):"),
        (super::core::line::ROW_POLICY, "print(\"    bcc (owner policy): %s\" % \", \".join(r"),
        (super::core::line::ROW_REPLY_IF, "if r.get(\"replyTo\"):"),
        (super::core::line::ROW_REPLY, "print(\"    reply-to: %s\" % \", \".join(r[\"replyTo\""),
        (super::core::line::ROW_FWD_IF, "if r.get(\"forwardOf\"):"),
        (super::core::line::ROW_FWD, "print(\"    forwards: %s\" % forward_line(r[\"forwa"),
        (super::core::line::ROW_ATT, "att = r.get(\"attachments\") or {}"),
        (super::core::line::ROW_ATT_IF, "if att.get(\"count\"):"),
        (super::core::line::ROW_ATT_LINE, "print(\"    attachments: %d (%s)\" % (att[\"count\"]"),
        (super::core::line::ROW_NOTE_IF, "if r.get(\"note\"):"),
        (super::core::line::ROW_NOTE, "print(\"    note from your human: %s\" % r[\"note\"]"),
        (super::core::line::FWD_LINE, "return \"a message in %s from %s (unverified)%s\" "),
        (super::core::line::FWD_MAILBOX, "term_line(f.get(\"mailbox\") or \"?\"), term_line(f."),
        (super::core::line::FWD_EML, "\", whole, as an .eml file\" if f.get(\"asEml\") els"),
        (super::core::line::DETAIL_ID, "print(\"outbound : %s\" % r.get(\"id\", \"\"))"),
        (super::core::line::DETAIL_STATUS, "print(\"status   : %s — %s\" % (r.get(\"status\", \"?"),
        (super::core::line::DETAIL_FROM, "print(\"from     : %s\" % (r.get(\"from\") or \"?\"))"),
        (super::core::line::DETAIL_TO, "print(\"to       : %s\" % (\", \".join(r.get(\"to\") o"),
        (super::core::line::DETAIL_CC_IF, "if r.get(\"cc\"):"),
        (super::core::line::DETAIL_CC, "print(\"cc       : %s\" % \", \".join(r[\"cc\"]))"),
        (super::core::line::DETAIL_BCC_IF, "if r.get(\"bcc\"):"),
        (super::core::line::DETAIL_BCC, "print(\"bcc      : %s\" % \", \".join(r[\"bcc\"]))"),
        (super::core::line::DETAIL_POLICY_IF, "if r.get(\"policyBcc\"):"),
        (super::core::line::DETAIL_POLICY, "print(\"bcc (owner policy): %s\" % \", \".join(r[\"po"),
        (super::core::line::DETAIL_REPLY_IF, "if r.get(\"replyTo\"):"),
        (super::core::line::DETAIL_REPLY, "print(\"reply-to : %s\" % \", \".join(r[\"replyTo\"]))"),
        (super::core::line::DETAIL_FWD_IF, "if r.get(\"forwardOf\"):"),
        (super::core::line::DETAIL_FWD, "print(\"forwards : %s\" % forward_line(r[\"forwardO"),
        (super::core::line::DETAIL_SUBJECT, "print(\"subject  : %s\" % (r.get(\"subject\") or \"(n"),
        (super::core::line::DETAIL_ATT, "att = r.get(\"attachments\") or {}"),
        (super::core::line::DETAIL_ATT_IF, "if att.get(\"count\"):"),
        (super::core::line::DETAIL_ATT_LINE, "print(\"attach   : %d (%s)\" % (att[\"count\"], \", \""),
        (super::core::line::DETAIL_QUEUED, "print(\"queued   : %s\" % unix_ago(r.get(\"createdA"),
        (super::core::line::DETAIL_DECIDED_IF, "if r.get(\"decidedAt\"):"),
        (super::core::line::DETAIL_BY, "by = \" by %s\" % r[\"decidedBy\"] if r.get(\"decided"),
        (super::core::line::DETAIL_DECIDED, "print(\"decided  : %s%s\" % (unix_ago(r.get(\"decid"),
        (super::core::line::DETAIL_SENT_IF, "if r.get(\"sentAt\"):"),
        (super::core::line::DETAIL_SENT, "print(\"submitted: %s\" % unix_ago(r.get(\"sentAt\")"),
        (super::core::line::DETAIL_NOTE_IF, "if r.get(\"note\"):"),
        (super::core::line::DETAIL_NOTE, "print(\"note     : %s\" % r[\"note\"])"),
        (super::core::line::DOMAIN_FIRST, "print(\"domain  : %s  (%s)\" % (d.get(\"domain\", \"?"),
        (super::core::line::DOMAIN_FOR, "for r in d.get(\"records\") or []:"),
        (super::core::line::DOMAIN_NOTE, "if d.get(\"note\"):"),
        (super::core::line::INBOXES_W, "w = max(max(len(r.get(\"address\", \"\")) for r in r"),
        (super::core::line::INBOXES_ST, "st = r.get(\"yourSendState\")"),
        (super::core::line::INBOXES_ROW, "print((\"%-\" + str(w) + \"s%-9s%-7s%-10s%-7s%s\") %"),
        (super::core::line::INBOXES_PRI, "pri = r.get(\"primary\") or {}"),
        (super::core::line::INBOXES_LVL, "st = e.get(\"sendState\")"),
        (super::core::line::INBOXES_GSUM, "gsum = \", \".join(\"%s:%s\" % (g.get(\"workspace\") o"),
        (super::core::line::INBOXES_OWNER_ROW, "print((\"%-\" + str(w) + \"s%-9s%-22s%s\") % ("),
        (super::core::line::INBOXES_SORTED, "for ws in sorted(set(b[2] for b in blocked)):"),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

#[test]
fn the_boot_script_runs_the_mail_script_at_its_line_13() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let boot = text
        .split_once("_K2_PY_STDIN_BOOT='")
        .expect("the boot script")
        .1;
    let boot = &boot[..boot.find("\n'\n").expect("its end")];
    let line = boot
        .split('\n')
        .position(|l| l.starts_with("exec(compile(_b, \"<stdin>\""))
        .expect("the exec line");
    assert_eq!(line as u32 + 1, super::core::BOOT_LINE);
}

mod core_rules {
    use super::super::core::{iso, py_float};

    #[test]
    fn float_is_pythons() {
        assert_eq!(py_float(" 1.5 "), Some(1.5));
        assert_eq!(py_float("1_0"), Some(10.0));
        assert_eq!(py_float("1e3"), Some(1000.0));
        assert_eq!(py_float("-inf"), Some(f64::NEG_INFINITY));
        assert!(py_float("nan").is_some_and(f64::is_nan));
        for bad in ["", "soon", "1__0", "_1", "1_", "--1", "0x10"] {
            assert_eq!(py_float(bad), None, "{bad}");
        }
    }

    #[test]
    fn iso_dates_with_an_offset() {
        assert_eq!(iso::timestamp("1970-01-01T00:00:00+00:00"), Some(0.0));
        assert_eq!(iso::timestamp("1970-01-02T01:00:00+01:00"), Some(86400.0));
        assert_eq!(iso::timestamp("19700101T000010-0000"), Some(10.0));
        assert_eq!(iso::timestamp("2020-02-30T00:00:00+00:00"), None);
        assert_eq!(iso::timestamp("not a date"), None);
        assert_eq!(iso::timestamp("1970-01-01T00:00:00.5+00:00"), Some(0.5));
    }
}

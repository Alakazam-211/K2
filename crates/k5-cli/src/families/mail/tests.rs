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
         Stopped: a@x.io refused its sign-in. A person signs in again (Settings → Email Link → Sign in again, or k2 mail link relink a@x.io) and it starts again.\n\
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

/// `dumps(body).encode()`: a body that is UTF-8 is its bytes; a lone
/// surrogate (a byte of stdin that is not UTF-8) is Python's refusal,
/// placed by code points in the dumped text, a run of them as a range.
#[test]
fn encode_body_refuses_a_lone_surrogate_as_python_does() {
    use super::core::{encode_body, obj};
    use crate::confirm::text_of_bytes;
    fn body(text: &[u8]) -> crate::out::json::Value {
        obj(vec![
            ("address", super::py::sv("a@x")),
            ("text", text_of_bytes(text)),
        ])
    }
    assert_eq!(
        encode_body(&body("Away é now".as_bytes())),
        Ok(r#"{"address":"a@x","text":"Away é now"}"#.as_bytes().to_vec())
    );
    let refused = |at: &str| {
        Err(format!(
            "UnicodeEncodeError: 'utf-8' codec can't encode {at}: surrogates not allowed"
        ))
    };
    assert_eq!(
        encode_body(&body(b"Away \xff now")),
        refused("character '\\udcff' in position 30")
    );
    assert_eq!(
        encode_body(&body(b"Away \xff\xfe")),
        refused("characters in position 30-31")
    );
    assert_eq!(
        encode_body(&body(b"a\nb\xe2\x82 \x80")),
        refused("characters in position 29-30")
    );
    assert_eq!(
        encode_body(&body(b"\xc3\xa9\x80")),
        refused("character '\\udc80' in position 26")
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
        (
            super::core::line::DIE,
            "def die(err, exit_code=None, permission=None):",
        ),
        (
            super::core::line::DIE_CODE,
            "code = err.get(\"code\", \"error\")",
        ),
        (
            super::core::line::DIE_HINT,
            "hint = err.get(\"hint\", \"\")",
        ),
        (
            super::core::line::DIE_IN,
            "if code in (\"forbidden\", \"owner_only\") and \"ask ",
        ),
        (
            super::core::line::DIE_CONCAT,
            "hint = (hint + \" — \" if hint else \"\") + \"require",
        ),
        (super::core::line::DIE_CAP, "if err.get(k):"),
        (super::core::line::REQUEST_FLOAT, "timeout = float(env_t)"),
        (
            super::core::line::REQUEST_DATA,
            "data = dumps(body).encode() if body is not None el",
        ),
        (
            super::core::line::REQUEST_READ,
            "raw = e.read().decode(\"utf-8\", \"replace\")",
        ),
        (
            super::core::line::CALL_REQUEST,
            "d = request(method, route, params, body, timeout",
        ),
        (
            super::core::line::CALL_TIMEOUT,
            "if timeout_hint and (d.get(\"error\") or {}).get(\"",
        ),
        (
            super::core::line::CALL_DIE_TIMEOUT,
            "die({\"code\": \"timeout\", \"hint\": timeout_hint(d[\"",
        ),
        (super::core::line::CALL_ERROR, "if d.get(\"error\"):"),
        (
            super::core::line::CALL_DIE_ERROR,
            "die(d[\"error\"], permission=d.get(\"permission\"))",
        ),
        (super::core::line::CALL_OK, "if not d.get(\"ok\"):"),
        (super::core::line::FMT_ADDR, "name = a.get(\"name\")"),
        (
            super::core::line::FMT_ADDR_EMAIL,
            "email = a.get(\"address\") or \"?\"",
        ),
        (
            super::core::line::FMT_ADDRS,
            "return \", \".join(fmt_addr(a) for a in (lst or []",
        ),
        (
            super::core::line::CAP_USED,
            "used = (cap or {}).get(\"used\", \"?\")",
        ),
        (
            super::core::line::CAP_LIMIT,
            "limit = (cap or {}).get(\"cap\", 0)",
        ),
        (
            super::core::line::FULL_ID,
            "print(\"message  : %s\" % m.get(\"id\", \"\"))",
        ),
        (
            super::core::line::FULL_ADDRESS,
            "print(\"mailbox  : %s\" % m.get(\"address\", \"?\"))",
        ),
        (
            super::core::line::FULL_FROM,
            "print(\"from     : %s\" % fmt_addr(m.get(\"from\")))",
        ),
        (
            super::core::line::FULL_TO,
            "print(\"to       : %s\" % (fmt_addrs(m.get(\"to\")) ",
        ),
        (super::core::line::FULL_CC_IF, "if m.get(\"cc\"):"),
        (
            super::core::line::FULL_CC,
            "print(\"cc       : %s\" % fmt_addrs(m.get(\"cc\")))",
        ),
        (
            super::core::line::FULL_SUBJECT,
            "print(\"subject  : %s\" % (m.get(\"subject\") or \"(n",
        ),
        (
            super::core::line::FULL_DATE,
            "print(\"date     : %s  (%s)\" % (m.get(\"date\") or ",
        ),
        (
            super::core::line::FULL_AUTH,
            "ar = m.get(\"authResults\") or {}",
        ),
        (
            super::core::line::FULL_ANY,
            "if any(ar.get(k) for k in (\"spf\", \"dkim\", \"dmarc",
        ),
        (
            super::core::line::FULL_AUTH_LINE,
            "print(\"auth     : %s\" % \"  \".join(",
        ),
        (
            super::core::line::FULL_ATTS,
            "atts = m.get(\"attachments\") or []",
        ),
        (super::core::line::FULL_FOR, "for a in atts:"),
        (
            super::core::line::FULL_ATT,
            "print(\"  %s. %s (%s, %s bytes)\" % (a.get(\"index\"",
        ),
        (
            super::core::line::FULL_FETCH,
            "print(\"  fetch one: k2 mail attachments %s --get",
        ),
        (
            super::core::line::FULL_TEXT,
            "print(m.get(\"text\") or \"\")",
        ),
        (super::core::line::FULL_HTML_IF, "if m.get(\"html\"):"),
        (super::core::line::FULL_HTML, "print(m.get(\"html\"))"),
        (
            super::core::line::ROW_FIRST,
            "print(\"%s  %-17s %-11s to %s — %s\" % (",
        ),
        (super::core::line::ROW_BCC_IF, "if r.get(\"bcc\"):"),
        (
            super::core::line::ROW_BCC,
            "print(\"    bcc: %s\" % \", \".join(r[\"bcc\"]))",
        ),
        (super::core::line::ROW_POLICY_IF, "if r.get(\"policyBcc\"):"),
        (
            super::core::line::ROW_POLICY,
            "print(\"    bcc (owner policy): %s\" % \", \".join(r",
        ),
        (super::core::line::ROW_REPLY_IF, "if r.get(\"replyTo\"):"),
        (
            super::core::line::ROW_REPLY,
            "print(\"    reply-to: %s\" % \", \".join(r[\"replyTo\"",
        ),
        (super::core::line::ROW_FWD_IF, "if r.get(\"forwardOf\"):"),
        (
            super::core::line::ROW_FWD,
            "print(\"    forwards: %s\" % forward_line(r[\"forwa",
        ),
        (
            super::core::line::ROW_ATT,
            "att = r.get(\"attachments\") or {}",
        ),
        (super::core::line::ROW_ATT_IF, "if att.get(\"count\"):"),
        (
            super::core::line::ROW_ATT_LINE,
            "print(\"    attachments: %d (%s)\" % (att[\"count\"]",
        ),
        (super::core::line::ROW_NOTE_IF, "if r.get(\"note\"):"),
        (
            super::core::line::ROW_NOTE,
            "print(\"    note from your human: %s\" % r[\"note\"]",
        ),
        (
            super::core::line::FWD_LINE,
            "return \"a message in %s from %s (unverified)%s\" ",
        ),
        (
            super::core::line::FWD_MAILBOX,
            "term_line(f.get(\"mailbox\") or \"?\"), term_line(f.",
        ),
        (
            super::core::line::FWD_EML,
            "\", whole, as an .eml file\" if f.get(\"asEml\") els",
        ),
        (
            super::core::line::DETAIL_ID,
            "print(\"outbound : %s\" % r.get(\"id\", \"\"))",
        ),
        (
            super::core::line::DETAIL_STATUS,
            "print(\"status   : %s — %s\" % (r.get(\"status\", \"?",
        ),
        (
            super::core::line::DETAIL_FROM,
            "print(\"from     : %s\" % (r.get(\"from\") or \"?\"))",
        ),
        (
            super::core::line::DETAIL_TO,
            "print(\"to       : %s\" % (\", \".join(r.get(\"to\") o",
        ),
        (super::core::line::DETAIL_CC_IF, "if r.get(\"cc\"):"),
        (
            super::core::line::DETAIL_CC,
            "print(\"cc       : %s\" % \", \".join(r[\"cc\"]))",
        ),
        (super::core::line::DETAIL_BCC_IF, "if r.get(\"bcc\"):"),
        (
            super::core::line::DETAIL_BCC,
            "print(\"bcc      : %s\" % \", \".join(r[\"bcc\"]))",
        ),
        (
            super::core::line::DETAIL_POLICY_IF,
            "if r.get(\"policyBcc\"):",
        ),
        (
            super::core::line::DETAIL_POLICY,
            "print(\"bcc (owner policy): %s\" % \", \".join(r[\"po",
        ),
        (super::core::line::DETAIL_REPLY_IF, "if r.get(\"replyTo\"):"),
        (
            super::core::line::DETAIL_REPLY,
            "print(\"reply-to : %s\" % \", \".join(r[\"replyTo\"]))",
        ),
        (super::core::line::DETAIL_FWD_IF, "if r.get(\"forwardOf\"):"),
        (
            super::core::line::DETAIL_FWD,
            "print(\"forwards : %s\" % forward_line(r[\"forwardO",
        ),
        (
            super::core::line::DETAIL_SUBJECT,
            "print(\"subject  : %s\" % (r.get(\"subject\") or \"(n",
        ),
        (
            super::core::line::DETAIL_ATT,
            "att = r.get(\"attachments\") or {}",
        ),
        (super::core::line::DETAIL_ATT_IF, "if att.get(\"count\"):"),
        (
            super::core::line::DETAIL_ATT_LINE,
            "print(\"attach   : %d (%s)\" % (att[\"count\"], \", \"",
        ),
        (
            super::core::line::DETAIL_QUEUED,
            "print(\"queued   : %s\" % unix_ago(r.get(\"createdA",
        ),
        (
            super::core::line::DETAIL_DECIDED_IF,
            "if r.get(\"decidedAt\"):",
        ),
        (
            super::core::line::DETAIL_BY,
            "by = \" by %s\" % r[\"decidedBy\"] if r.get(\"decided",
        ),
        (
            super::core::line::DETAIL_DECIDED,
            "print(\"decided  : %s%s\" % (unix_ago(r.get(\"decid",
        ),
        (super::core::line::DETAIL_SENT_IF, "if r.get(\"sentAt\"):"),
        (
            super::core::line::DETAIL_SENT,
            "print(\"submitted: %s\" % unix_ago(r.get(\"sentAt\")",
        ),
        (super::core::line::DETAIL_NOTE_IF, "if r.get(\"note\"):"),
        (
            super::core::line::DETAIL_NOTE,
            "print(\"note     : %s\" % r[\"note\"])",
        ),
        (
            super::core::line::DOMAIN_FIRST,
            "print(\"domain  : %s  (%s)\" % (d.get(\"domain\", \"?",
        ),
        (
            super::core::line::DOMAIN_FOR,
            "for r in d.get(\"records\") or []:",
        ),
        (super::core::line::DOMAIN_NOTE, "if d.get(\"note\"):"),
        (
            super::core::line::INBOXES_W,
            "w = max(max(len(r.get(\"address\", \"\")) for r in r",
        ),
        (
            super::core::line::INBOXES_ST,
            "st = r.get(\"yourSendState\")",
        ),
        (
            super::core::line::INBOXES_ROW,
            "print((\"%-\" + str(w) + \"s%-9s%-7s%-10s%-7s%s\") %",
        ),
        (
            super::core::line::INBOXES_PRI,
            "pri = r.get(\"primary\") or {}",
        ),
        (super::core::line::INBOXES_LVL, "st = e.get(\"sendState\")"),
        (
            super::core::line::INBOXES_GSUM,
            "gsum = \", \".join(\"%s:%s\" % (g.get(\"workspace\") o",
        ),
        (
            super::core::line::INBOXES_OWNER_ROW,
            "print((\"%-\" + str(w) + \"s%-9s%-22s%s\") % (",
        ),
        (
            super::core::line::INBOXES_SORTED,
            "for ws in sorted(set(b[2] for b in blocked)):",
        ),
        (
            super::reads::line::DOMAINS_CALL,
            "d = call(\"GET\", \"/cli/mail/domain/list\", params=",
        ),
        (
            super::reads::line::DOMAINS_ROWS,
            "rows = d.get(\"domains\") or []",
        ),
        (
            super::reads::line::DOMAINS_PENDING,
            "pending = d.get(\"pending\") or []",
        ),
        (
            super::reads::line::DOMAINS_NAMES,
            "names = \", \".join(p if isinstance(p, str) else s",
        ),
        (super::reads::line::DOMAINS_FOR, "for r in rows:"),
        (
            super::reads::line::DOMAINS_ROW,
            "print(\"%s%s\" % (r.get(\"domain\", \"?\"), \"   (defau",
        ),
        (
            super::reads::line::MESSAGES_CALL,
            "d = call(\"GET\", \"/cli/mail/messages\", params=par",
        ),
        (
            super::reads::line::MESSAGES_MSGS,
            "msgs = d.get(\"messages\") or []",
        ),
        (
            super::reads::line::MESSAGES_NEXT,
            "next_offset = d.get(\"nextOffset\")",
        ),
        (
            super::reads::line::MESSAGES_ERRORS,
            "inbox_errors = d.get(\"inboxErrors\") or []",
        ),
        (super::reads::line::MESSAGES_FOR, "for m in msgs:"),
        (
            super::reads::line::MESSAGES_ATT,
            "if m.get(\"hasAttachment\"):",
        ),
        (super::reads::line::MESSAGES_UNREAD, "if m.get(\"unread\"):"),
        (
            super::reads::line::MESSAGES_ROW,
            "print(\"%s · %s · %s · %s%s\" % (m.get(\"id\", \"\"), ",
        ),
        (
            super::reads::line::MESSAGES_WINDOW,
            "hint = (d.get(\"searchWindow\") or {}).get(\"hint\")",
        ),
        (
            super::reads::line::MESSAGES_NOTE,
            "print(\"note:     %d inbox(es) had errors and wer",
        ),
        (
            super::reads::line::READ_CALL,
            "d = call(\"GET\", \"/cli/mail/read\", params=params)",
        ),
        (
            super::reads::line::READ_RAW,
            "sys.stdout.buffer.write(base64.b64decode(d.get(\"",
        ),
        (
            super::reads::line::READ_FULL,
            "print_full(d.get(\"message\") or {})",
        ),
        (
            super::reads::line::ATT_POST,
            "d = call(\"POST\", \"/cli/mail/attachments\", params",
        ),
        (
            super::reads::line::ATT_GET,
            "d = call(\"GET\", \"/cli/mail/attachments\", params=",
        ),
        (
            super::reads::line::ATT_WROTE,
            "print(\"wrote attachment #%s: %s (%s, %s bytes) -",
        ),
        (
            super::reads::line::ATT_ATTS,
            "atts = d.get(\"attachments\") or []",
        ),
        (
            super::reads::line::ATT_NONE,
            "print(\"No attachments on %s.\" % d.get(\"id\", \"\"))",
        ),
        (super::reads::line::ATT_FOR, "for a in atts:"),
        (
            super::reads::line::ATT_ROW,
            "print(\"  %s. %s (%s, %s bytes)\" % (a.get(\"index\"",
        ),
        (
            super::reads::line::ATT_FETCH,
            "print(\"fetch one: k2 mail attachments %s --get <",
        ),
        (
            super::reads::line::WAIT_CALL,
            "d = call(\"GET\", \"/cli/mail/wait\", params=params,",
        ),
        (
            super::reads::line::WAIT_TIMED_OUT,
            "if d.get(\"timedOut\"):",
        ),
        (super::reads::line::WAIT_DIE, "die({\"code\": \"timeout\","),
        (
            super::reads::line::WAIT_FULL,
            "print_full(d.get(\"message\") or {})",
        ),
        (
            super::reads::line::CANCEL_CALL,
            "d = call(\"POST\", \"/cli/mail/outbox/cancel\",",
        ),
        (
            super::reads::line::CANCEL_PRINT,
            "print(d.get(\"hint\") or (\"cancelled %s\" % d.get(\"",
        ),
        (
            super::reads::line::OUTBOX_CALL,
            "d = call(\"GET\", \"/cli/mail/outbox\", params=param",
        ),
        (
            super::reads::line::OUTBOX_DETAIL,
            "print_outbound_detail(d.get(\"outbound\") or {})",
        ),
        (
            super::reads::line::OUTBOX_ROWS,
            "rows = d.get(\"outbox\") or []",
        ),
        (super::reads::line::OUTBOX_FOR, "for r in rows:"),
        (super::reads::line::OUTBOX_ROW, "print_outbound_row(r)"),
        (
            super::reads::line::DRAFTS_CALL,
            "d = call(\"GET\", \"/cli/mail/drafts\", params=param",
        ),
        (
            super::reads::line::DRAFTS_ROWS,
            "rows = d.get(\"drafts\") or []",
        ),
        (super::reads::line::DRAFTS_FOR, "for r in rows:"),
        (
            super::reads::line::DRAFTS_ST,
            "st = r.get(\"status\") or \"?\"",
        ),
        (
            super::reads::line::DRAFTS_WHAT,
            "what = \"compose\" if r.get(\"kind\") == \"compose\" else",
        ),
        (
            super::reads::line::DRAFTS_LINE,
            "line = \"%s  %-9s %-34s %s\" % (r.get(\"id\"",
        ),
        (super::reads::line::DRAFTS_EDITED, "if r.get(\"editedBy\"):"),
        (
            super::reads::line::DRAFTS_EDITED_BY,
            "line += \"  · edited by %s\" % r[\"editedBy\"]",
        ),
        (
            super::reads::line::DRAFTS_STATE,
            "if st in (\"sent\", \"held\", \"discarded\") and r.get",
        ),
        (
            super::reads::line::DRAFTS_STATE_BY,
            "line += \"  · %s by %s\" % (st, r[\"stateBy\"])",
        ),
        (
            super::reads::line::DRAFTS_OUTBOUND,
            "if r.get(\"outboundId\"):",
        ),
        (
            super::reads::line::DRAFTS_OUTBOUND_ID,
            "line += \"  · outbox %s\" % r[\"outboundId\"]",
        ),
        (
            super::reads::line::CAPS_REQUEST,
            "d = request(\"GET\", \"/cli/mail/capabilities\", par",
        ),
        (
            super::reads::line::CAPS_ERR,
            "err = d.get(\"error\") if isinstance(d.get(\"error\"",
        ),
        (
            super::reads::line::CAPS_HINT,
            "if err and \"project\" in params and str(err.get(\"",
        ),
        (
            super::reads::line::CAPS_AGAIN,
            "d = request(\"GET\", \"/cli/mail/capabilities\", par",
        ),
        (
            super::reads::line::CAPS_IS_ERR,
            "if isinstance(d.get(\"error\"), dict):",
        ),
        (super::reads::line::CAPS_DIE_ERR, "die(d[\"error\"])"),
        (
            super::reads::line::CAPS_ROWS,
            "rows = [d] if env(\"M_ADDRESS\") else (d.get(\"inbo",
        ),
        (super::reads::line::CAPS_FOR, "for r in rows:"),
        (
            super::reads::line::CAPS_C,
            "c = r.get(\"capabilities\") or {}",
        ),
        (
            super::reads::line::CAPS_MAIL,
            "mail = c.get(\"mail\") or {}",
        ),
        (
            super::reads::line::CAPS_CAL,
            "cal = c.get(\"calendar\") or {}",
        ),
        (
            super::reads::line::CAPS_ON,
            "on = lambda m: \",\".join(k for k in (\"read\", \"dra",
        ),
        (super::reads::line::CAPS_LVL, "lvl = r.get(\"yourLevel\")"),
        (
            super::reads::line::CAPS_FIRST,
            "print(\"%s  [%s]%s\" % (r.get(\"address\", \"?\"), c.g",
        ),
        (
            super::reads::line::CAPS_SECOND,
            "print(\"  mail: %s   calendar: %s   free/busy: %s",
        ),
        (super::reads::line::CAPS_ACCT, "acct = c.get(\"account\")"),
        (
            super::reads::line::CAPS_ACCT_LINE,
            "print(\"  account: %s%s%s\"",
        ),
        (
            super::reads::line::CAPS_MISSING,
            "for m in c.get(\"missing\") or []:",
        ),
        (
            super::reads::line::CAPS_MISSING_LINE,
            "print(\"  - %s off (%s): %s\" % (m.get(\"cap\"), m.g",
        ),
        (
            super::reads::line::INBOXES_CALL,
            "d = call(\"GET\", \"/cli/mail/inboxes\", params={\"pr",
        ),
        (
            super::reads::line::INBOXES_ROWS,
            "rows = d.get(\"inboxes\") or []",
        ),
        (
            super::reads::line::INBOXES_OWNER,
            "owner_catalog = bool(rows) and all(r.get(\"yourLe",
        ),
        (
            super::reads::line::INBOXES_PRINT,
            "print_inboxes(rows, agent_view=not owner_catalog",
        ),
        (
            super::reads::line::FOLDERS_CALL,
            "d = call(\"GET\", \"/cli/mail/folder/list\",",
        ),
        (
            super::reads::line::FOLDERS_LIST,
            "folders = d.get(\"folders\") or []",
        ),
        (super::reads::line::FOLDERS_FOR, "for f in folders:"),
        (
            super::reads::line::FOLDER_CREATE_CALL,
            "d = call(\"POST\", \"/cli/mail/folder/create\",",
        ),
        (
            super::reads::line::FOLDER_CREATE_PRINT,
            "print(\"created folder '%s' in %s\" % (d.get(\"fold",
        ),
        (
            super::reads::line::FOLDER_RENAME_CALL,
            "d = call(\"POST\", \"/cli/mail/folder/rename\",",
        ),
        (
            super::reads::line::FOLDER_RENAME_PRINT,
            "print(\"renamed '%s' -> '%s' in %s\"",
        ),
        (
            super::writes::line::OUTCOME_TIMED_OUT,
            "if waiting and d.get(\"timedOut\"):",
        ),
        (
            super::writes::line::OUTCOME_DIE,
            "die({\"code\": \"timeout\", \"hint\": d.get(\"hint\") or",
        ),
        (super::writes::line::OUTCOME_QUEUED, "if d.get(\"queued\"):"),
        (
            super::writes::line::OUTCOME_QUEUED_PRINT,
            "print(d.get(\"hint\") or \"queued for approval (%s)",
        ),
        (super::writes::line::OUTCOME_HAS, "if \"timedOut\" in d:"),
        (
            super::writes::line::OUTCOME_DECIDED,
            "print(\"%s %s\" % (d.get(\"id\", \"\"), d.get(\"statusN",
        ),
        (super::writes::line::OUTCOME_NOTE, "if d.get(\"note\"):"),
        (
            super::writes::line::OUTCOME_NOTE_PRINT,
            "print(\"note from your human: %s\" % d[\"note\"])",
        ),
        (
            super::writes::line::OUTCOME_TRACK,
            "print(\"track with: k2 mail outbox %s\" % d.get(\"i",
        ),
        (
            super::writes::line::OUTCOME_SUBMITTED,
            "print(\"%s %s\" % (d.get(\"id\", \"\"), d.get(\"hint\") ",
        ),
        (
            super::writes::line::SEND_CALL,
            "d = call(\"POST\", \"/cli/mail/send\", body=body, ti",
        ),
        (
            super::writes::line::SEND_OUTCOME,
            "send_outcome(d, waiting, t)",
        ),
        (
            super::writes::line::REPLY_CALL,
            "d = call(\"POST\", \"/cli/mail/reply\", body=body, t",
        ),
        (
            super::writes::line::REPLY_OUTCOME,
            "send_outcome(d, waiting, t)",
        ),
        (
            super::writes::line::FORWARD_CALL,
            "d = call(\"POST\", \"/cli/mail/forward-message\", bo",
        ),
        (
            super::writes::line::FORWARD_OUTCOME,
            "send_outcome(d, waiting, t)",
        ),
        (
            super::writes::line::ADDRESS_DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/address/delete\",",
        ),
        (
            super::writes::line::ADDRESS_DELETE_PRINT,
            "print(\"retired %s — its cap slot is free immedia",
        ),
        (
            super::writes::line::DRAFT_REQUEST,
            "d = request(\"POST\", \"/cli/mail/draft\", body=body, timeout=60)",
        ),
        (
            super::writes::line::DRAFT_CODE,
            "code = ((d or {}).get(\"error\") or {}).get(\"code\")",
        ),
        (super::writes::line::DRAFT_ERROR, "if d.get(\"error\"):"),
        (
            super::writes::line::DRAFT_DIE,
            "die(d[\"error\"], permission=d.get(\"permission\"))",
        ),
        (
            super::writes::line::DRAFT_HINT,
            "hint = d.get(\"hint\") or \"draft saved to '%s' in %s\" %",
        ),
        (
            super::writes::line::DRAFT_ID,
            "if d.get(\"draftId\") and d[\"draftId\"] not in hint:",
        ),
        (
            super::writes::line::DRAFT_DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/draft/delete\", body=",
        ),
        (
            super::writes::line::DRAFT_DELETE_PRINT,
            "print(d.get(\"hint\") or \"draft %s deleted\" % env(",
        ),
        (
            super::writes::line::MOVE_CALL,
            "d = call(\"POST\", \"/cli/mail/move\",",
        ),
        (
            super::writes::line::MOVE_PRINT,
            "print(\"moved %s -> %s\" % (d.get(\"id\", \"\"), d.get",
        ),
        (
            super::writes::line::FLAG_CALL,
            "d = call(\"POST\", \"/cli/mail/flag\", body=body)",
        ),
        (
            super::writes::line::FLAG_PRINT,
            "print(\"%s: %s\" % (d.get(\"id\", \"\"), \", \".join(par",
        ),
        (
            super::writes::line::ARCHIVE_CALL,
            "d = call(\"POST\", \"/cli/mail/archive\", body={\"pro",
        ),
        (
            super::writes::line::ARCHIVE_PRINT,
            "print(\"archived %s → %s\" % (d.get(\"id\", \"\"), d.g",
        ),
        (
            super::writes::line::DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/delete\", body={\"proj",
        ),
        (
            super::writes::line::DELETE_PRINT,
            "print(\"deleted %s -> %s (moved to Trash, not era",
        ),
        (
            super::owner::line::SENDING_SET,
            "call(\"POST\", \"/cli/mail/config/set\",",
        ),
        (
            super::owner::line::SENDING_GET,
            "d = call(\"GET\", \"/cli/mail/config\", params={\"wor",
        ),
        (
            super::owner::line::SENDING_W,
            "w = d.get(\"workspace\") or {}",
        ),
        (
            super::owner::line::SENDING_A,
            "a = w.get(\"agentSend\") or {}",
        ),
        (
            super::owner::line::SENDING_JSON,
            "print(dumps({\"ok\": True, \"workspace\": ws, \"proje",
        ),
        (
            super::owner::line::SENDING_EFF,
            "eff = a.get(\"effective\") or \"?\"",
        ),
        (
            super::owner::line::SENDING_SRC,
            "src = \"set for this workspace\" if a.get(\"overrid",
        ),
        (
            super::owner::line::EXT_ADD_CALL,
            "d = call(\"POST\", \"/cli/mail/external/add\", body=",
        ),
        (
            super::owner::line::EXT_ADD_PRINT,
            "print(\"connected %s -> workspace %s\" % (d.get(\"a",
        ),
        (
            super::owner::line::EXT_ADD_DRAFTS,
            "if d.get(\"draftsFolder\"):",
        ),
        (
            super::owner::line::EXT_ADD_DRAFTS_PRINT,
            "print(\"drafts folder: %s\" % d[\"draftsFolder\"])",
        ),
        (
            super::owner::line::CAL_POST,
            "d = call(\"POST\", \"/cli/mail/link/calendars\", bod",
        ),
        (
            super::owner::line::CAL_GET,
            "d = call(\"GET\", \"/cli/mail/link/calendars\")",
        ),
        (
            super::owner::line::CAL_STATE,
            "state = d.get(\"linkedCalendars\") or (\"on\" if d.g",
        ),
        (
            super::owner::line::CAL_PRINT,
            "print(\"Calendars on linked inboxes: %s%s\" % (",
        ),
        (
            super::owner::line::OAUTH_START,
            "d = call(\"POST\", \"/cli/mail/link/oauth/start\", b",
        ),
        (super::owner::line::OAUTH_FLOW, "flow = d.get(\"flow\")"),
        (
            super::owner::line::OAUTH_LINK_ID,
            "link_id = d.get(\"linkId\")",
        ),
        (
            super::owner::line::OAUTH_DEVICE,
            "sys.stderr.write(\"To %s %s (%s): open %s and ent",
        ),
        (
            super::owner::line::OAUTH_BROWSER,
            "sys.stderr.write(\"A browser window opened — appr",
        ),
        (
            super::owner::line::OAUTH_STATUS,
            "s = request(\"GET\", \"/cli/mail/link/oauth/status\"",
        ),
        (super::owner::line::OAUTH_ERROR, "if s.get(\"error\"):"),
        (super::owner::line::OAUTH_STATE, "state = s.get(\"state\")"),
        (
            super::owner::line::OAUTH_FINAL,
            "state = final.get(\"state\")",
        ),
        (
            super::owner::line::OAUTH_RELINKED,
            "print(\"re-linked %s (%s)\" % (final.get(\"address\")",
        ),
        (
            super::owner::line::OAUTH_UPGRADED,
            "print(\"upgraded %s (%s)\" % (final.get(\"address\")",
        ),
        (
            super::owner::line::OAUTH_UPGRADED_HINT,
            "print(final.get(\"hint\") or \"its grants, levels a",
        ),
        (
            super::owner::line::OAUTH_CONNECTED,
            "print(\"connected %s -> workspace %s (%s)\"",
        ),
        (
            super::owner::line::OAUTH_CONNECTED_HINT,
            "print(final.get(\"hint\")",
        ),
        (
            super::owner::line::OAUTH_HINT,
            "hint = final.get(\"hint\") or (\"the link %s\" % sta",
        ),
        (
            super::owner::line::OAUTH_CONSENT,
            "if final.get(\"adminConsentUrl\") and final[\"admin",
        ),
        (
            super::owner::line::OAUTH_CONSENT_ADD,
            "hint += \" Admin consent: %s\" % final[\"adminConse",
        ),
        (
            super::owner::line::OAUTH_DIE,
            "die({\"code\": final.get(\"code\") or (\"denied\" if s",
        ),
        (
            super::owner::line::EXT_REMOVE_CALL,
            "d = call(\"POST\", \"/cli/mail/external/remove\", bo",
        ),
        (
            super::owner::line::EXT_REMOVE_PRINT,
            "print(\"removed %s — its stored credential was de",
        ),
        (
            super::owner::line::EXT_REMOVE_HINT,
            "if d.get(\"revokeHint\"):",
        ),
        (
            super::owner::line::EXT_REMOVE_HINT_PRINT,
            "print(d[\"revokeHint\"])",
        ),
        (
            super::owner::line::OAUTH_SHOW,
            "d = call(\"GET\", \"/cli/mail/oauth-config\")",
        ),
        (
            super::owner::line::OAUTH_SET,
            "d = call(\"POST\", \"/cli/mail/oauth-config/set\", b",
        ),
        (
            super::owner::line::OAUTH_CLEAR,
            "d = call(\"POST\", \"/cli/mail/oauth-config/clear\",",
        ),
        (
            super::owner::line::OAUTH_PROVS,
            "provs = d.get(\"providers\") or {d.get(\"provider\",",
        ),
        (super::owner::line::OAUTH_P, "p = provs.get(name)"),
        (
            super::owner::line::OAUTH_LINE,
            "line = \"%s: %s app %s%s\" % (",
        ),
        (
            super::owner::line::OAUTH_SECRET,
            "line += \"  secret %s\" % (\"set\" if p.get(\"secretS",
        ),
        (
            super::owner::line::OAUTH_TENANT,
            "if name == \"microsoft\" and p.get(\"tenant\"):",
        ),
        (
            super::owner::line::OAUTH_TENANT_LINE,
            "line += \"  tenant %s\" % p[\"tenant\"]",
        ),
        (
            super::owner::line::LINKS_CALL,
            "d = call(\"GET\", \"/cli/mail/inboxes\")",
        ),
        (
            super::owner::line::LINKS_ROWS,
            "rows = [r for r in (d.get(\"inboxes\") or []) if r",
        ),
        (
            super::owner::line::LINKS_COUNT,
            "print(dumps({\"ok\": True, \"count\": len(rows), \"in",
        ),
        (
            super::owner::line::LINKS_W,
            "w = max(max(len(r.get(\"address\", \"\")) for r in r",
        ),
        (
            super::owner::line::LINKS_PRI,
            "pri = r.get(\"primary\") or {}",
        ),
        (
            super::owner::line::LINKS_ROW,
            "print((\"%-\" + str(w) + \"s%-25s%-22s%s\") % (",
        ),
        (
            super::owner::line::LINKS_GRANTS,
            "for g in (r.get(\"grants\") or []):",
        ),
        (super::owner::line::LINKS_ST, "st = g.get(\"sendState\")"),
        (
            super::owner::line::LINKS_NOTE,
            "note = (\"   BLOCKED: agent sending is %s for thi",
        ),
        (
            super::owner::line::LINKS_GRANT,
            "print(\"    grant: %-20s %s%s\" % (",
        ),
        (
            super::owner::line::GRANT_CALL,
            "d = call(\"POST\", \"/cli/mail/access/grant\", body=",
        ),
        (
            super::owner::line::GRANT_PRINT,
            "print(d.get(\"hint\") or \"granted %s on %s to %s\"",
        ),
        (
            super::owner::line::REVOKE_CALL,
            "d = call(\"POST\", \"/cli/mail/access/revoke\",",
        ),
        (
            super::owner::line::REVOKE_PRINT,
            "print(\"revoked %s's access to %s.\" % (env(\"M_WOR",
        ),
        (
            super::owner::line::PRIMARY_CALL,
            "d = call(\"POST\", \"/cli/mail/access/set-primary\",",
        ),
        (
            super::owner::line::PRIMARY_PRINT,
            "print(d.get(\"hint\") or \"%s now manages %s\"",
        ),
        (
            super::owner::line::LEVEL_CALL,
            "d = call(\"POST\", \"/cli/mail/access/set-level\", b",
        ),
        (
            super::owner::line::LEVEL_PRINT,
            "who = \"primary\" if d.get(\"isPrimary\") else env(\"",
        ),
        (
            super::owner::line::MANAGE_CALL,
            "d = call(\"POST\", \"/cli/mail/access/set-manage\",",
        ),
        (
            super::owner::line::MANAGE_WHO,
            "who = \"primary\" if d.get(\"isPrimary\") else env(\"",
        ),
        (
            super::owner::line::MANAGE_CAPS,
            "caps = [\"manage\" if d.get(\"canManage\") else \"no-",
        ),
        (
            super::owner::line::MANAGE_PRINT,
            "print(\"set %s caps on %s: %s\" % (who, d.get(\"add",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` the forward branch raises at.
#[test]
fn the_forward_lines_are_the_shell_s() {
    use super::forward::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::DIE_CODE,
            "code = (err or {}).get(\"code\", \"error\")",
        ),
        (line::DIE_HINT, "hint = (err or {}).get(\"hint\", \"\")"),
        (line::DIE_ANY, "die(err, permission=permission)"),
        (
            line::CALL_REQUEST,
            "d = request(method, route, params, body, timeout",
        ),
        (line::CALL_ERROR, "if d.get(\"error\"):"),
        (
            line::CALL_DIE,
            "fwd_die(d[\"error\"], d.get(\"permission\"))",
        ),
        (line::CALL_OK, "if not d.get(\"ok\"):"),
        (
            line::LINE_TO,
            "f.get(\"to\") or \"?\", \"true\" if f.get(\"keep\") else",
        ),
        (
            line::LINE_KIND,
            "f.get(\"kind\") or \"?\", f.get(\"state\") or \"?\")",
        ),
        (
            line::LINE_STATE,
            "if f.get(\"state\") == \"paused\" and f.get(\"pauseRe",
        ),
        (
            line::DETAIL_STATE,
            "if f.get(\"state\") == \"paused\" and f.get(\"pauseRe",
        ),
        (
            line::DETAIL_WORDS,
            "elif f.get(\"state\") == \"paused\" and f.get(\"pause",
        ),
        (
            line::DETAIL_REASON,
            "reason = f.get(\"pauseReason\") or \"\"",
        ),
        (line::DETAIL_HELD, "if f.get(\"heldBy\"):"),
        (line::DETAIL_CANT, "if f.get(\"cantArrive\"):"),
        (line::DETAIL_NOTICE_WORDS, "if f.get(\"noticeWords\"):"),
        (
            line::DETAIL_NOTICE,
            "elif f.get(\"notice\") == \"sent\" and f.get(\"notice",
        ),
        (
            line::DETAIL_TOLD,
            "out.append(\"  told %s: emailed it a stop link %s",
        ),
        (line::DETAIL_C, "c = f.get(\"last24h\") or {}"),
        (
            line::DETAIL_ANY,
            "if f.get(\"kind\") == \"external\" or any(c.get(k) f",
        ),
        (
            line::DETAIL_COUNTS,
            "c.get(\"forwarded\", 0), c.get(\"delivered\", 0), c.",
        ),
        (line::DETAIL_SET_BY, "if f.get(\"setBy\"):"),
        (line::NOTE, "if d.get(\"note\"):"),
        (
            line::GET_CALL,
            "d = fwd_call(\"GET\", base_route, params=params)",
        ),
        (line::GET_A, "a = d.get(\"address\") or addr_arg or \"?\""),
        (line::GET_ROWS, "rows = d.get(\"forwards\")"),
        (
            line::GET_FORWARD,
            "rows = [d[\"forward\"]] if d.get(\"forward\") else [",
        ),
        (line::GET_FOR, "for f in rows:"),
        (line::GET_LINE, "print(\"%s  %s\" % (a, fwd_line(f)))"),
        (line::GET_DETAIL, "for line in fwd_detail(f):"),
        (line::GET_VOL, "vol = d.get(\"volume\") or {}"),
        (
            line::GET_VOL_PRINT,
            "vol.get(\"copiesCounted\", 0), vol.get(\"copiesPerD",
        ),
        (line::GET_EVS, "evs = d.get(\"events\") or []"),
        (line::GET_EV_FOR, "for e in evs[:5]:"),
        (line::GET_EV_DETAIL, "det = e.get(\"detail\") or {}"),
        (
            line::GET_EV_PRINT,
            "print(\"    %s  %-12s %s%s\" % (when(e.get(\"at\")),",
        ),
        (
            line::SET_CALL,
            "d = fwd_call(\"POST\", route, body=body, timeout=3",
        ),
        (line::SET_A, "a = d.get(\"address\") or addr_arg or \"?\""),
        (line::SET_FOR, "for one in d.get(\"destinations\") or [d]:"),
        (line::SET_TO, "dest = one.get(\"to\") or \"?\""),
        (
            line::SET_KEEP,
            "keep = \"keeps a copy\" if one.get(\"keep\") else \"n",
        ),
        (
            line::SET_PRINT,
            "print(\"forward %s -> %s (%s): %s\" % (a, dest, ke",
        ),
        (line::SET_MESSAGE, "if one.get(\"message\"):"),
        (
            line::SET_NOTE,
            "if one.get(\"note\") and one.get(\"note\") != d.get(",
        ),
        (line::SET_NOTE_LINE, "note_line(d)"),
        (
            line::RM_CALL,
            "d = fwd_call(\"POST\", base_route + \"/rm\", body=bo",
        ),
        (line::RM_A, "a = d.get(\"address\") or addr_arg or \"?\""),
        (
            line::RM_PRINT,
            "print(\"%s no longer forwards to %s\" % (a, \", \".j",
        ),
        (line::RM_LEFT, "left = d.get(\"remaining\") or []"),
        (
            line::RM_LEFT_PRINT,
            "print(\"still forwarding to: %s\" % (\", \".join(lef",
        ),
        (line::RM_NOTE_LINE, "note_line(d)"),
        (
            line::UNSET_CALL,
            "d = fwd_call(\"POST\", base_route + \"/unset\", body",
        ),
        (line::UNSET_A, "a = d.get(\"address\") or addr_arg or \"?\""),
        (line::UNSET_REMOVED, "if d.get(\"removed\") is False:"),
        (line::UNSET_NOTE_LINE, "note_line(d)"),
        (
            line::PAUSE_CALL,
            "d = fwd_call(\"POST\", base_route + \"/\" + op, body",
        ),
        (line::PAUSE_A, "a = d.get(\"address\") or addr_arg or \"?\""),
        (line::PAUSE_ROWS, "rows = d.get(\"forwards\") or []"),
        (
            line::PAUSE_WHICH,
            "which = \" (%s)\" % \", \".join(r.get(\"to\") or \"?\" f",
        ),
        (
            line::PAUSE_STATE,
            "print(\"forward resumed for %s%s (state %s)\" % (a",
        ),
        (line::PAUSE_SKIPPED, "for sk in d.get(\"skipped\") or []:"),
        (
            line::PAUSE_SK,
            "print(\"left alone: %s: %s\" % (sk.get(\"to\") or \"?",
        ),
        (line::PAUSE_NOTE_LINE, "note_line(d)"),
        (
            line::LIST_CALL,
            "d = fwd_call(\"GET\", \"/cli/mail/forwards\", params",
        ),
        (line::LIST_ROWS, "rows = d.get(\"forwards\") or []"),
        (line::LIST_OC, "oc = d.get(\"outcomes\") or {}"),
        (line::LIST_OC_OK, "if oc.get(\"ok\") is False:"),
        (
            line::LIST_OC_ERROR,
            "print(\"warning: delivery reports aren't reaching",
        ),
        (line::LIST_FOR, "for f in rows:"),
        (
            line::LIST_LINE,
            "print(\"%s  %s\" % (f.get(\"address\") or \"?\", fwd_l",
        ),
        (
            line::LIST_PAUSED,
            "if f.get(\"state\") == \"paused\" and f.get(\"pauseWo",
        ),
        (line::LIST_CANT, "if f.get(\"cantArrive\"):"),
        (line::LIST_NOTICE, "if f.get(\"noticeWords\"):"),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` the hostmail verbs raise at.
#[test]
fn the_hostmail_lines_are_the_shell_s() {
    use crate::families::hostmail::{mailbox, notices};
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            mailbox::line::QUOTA_GET_CALL,
            "d = call(\"GET\", \"/cli/mail/quota\", params={\"addr",
        ),
        (mailbox::line::QUOTA_GET_Q, "q = d.get(\"quotas\") or {}"),
        (
            mailbox::line::QUOTA_GET_ADDR,
            "d.get(\"address\") or env(\"M_ADDRESS\") or \"?\",",
        ),
        (
            mailbox::line::QUOTA_GET_DISK,
            "q.get(\"maxDiskQuota\") if q.get(\"maxDiskQuota\") i",
        ),
        (
            mailbox::line::QUOTA_GET_MSGS,
            "q.get(\"maxEmails\") if q.get(\"maxEmails\") is not ",
        ),
        (
            mailbox::line::QUOTA_SET_BYTES,
            "body[\"bytes\"] = int(env(\"M_QUOTA_BYTES\"))",
        ),
        (
            mailbox::line::QUOTA_SET_GB,
            "body[\"gb\"] = int(env(\"M_QUOTA_GB\"))",
        ),
        (
            mailbox::line::QUOTA_SET_MSGS_INT,
            "body[\"messages\"] = int(env(\"M_QUOTA_MESSAGES\"))",
        ),
        (
            mailbox::line::QUOTA_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/quota\", body=body)",
        ),
        (mailbox::line::QUOTA_SET_Q, "q = d.get(\"quotas\") or {}"),
        (
            mailbox::line::QUOTA_SET_ADDR,
            "d.get(\"address\") or env(\"M_ADDRESS\") or \"?\",",
        ),
        (
            mailbox::line::QUOTA_SET_DISK,
            "q.get(\"maxDiskQuota\") if q.get(\"maxDiskQuota\") i",
        ),
        (
            mailbox::line::QUOTA_SET_MSGS,
            "q.get(\"maxEmails\") if q.get(\"maxEmails\") is not ",
        ),
        (
            mailbox::line::CATCHALL_GET_CALL,
            "d = call(\"GET\", \"/cli/mail/catchall\", params={\"d",
        ),
        (
            mailbox::line::CATCHALL_GET_ADDR,
            "addr = d.get(\"catchAllAddress\")",
        ),
        (
            mailbox::line::CATCHALL_GET_DOMAIN,
            "print(\"%s  catch-all=%s\" % (d.get(\"domain\") or e",
        ),
        (
            mailbox::line::CATCHALL_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/catchall\", body=body",
        ),
        (
            mailbox::line::CATCHALL_SET_ADDR,
            "addr = d.get(\"catchAllAddress\")",
        ),
        (
            mailbox::line::CATCHALL_SET_DOMAIN,
            "print(\"%s  catch-all=%s\" % (d.get(\"domain\") or e",
        ),
        (
            mailbox::line::ALIAS_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/alias\", params={\"mail",
        ),
        (
            mailbox::line::ALIAS_LIST_ROWS,
            "rows = d.get(\"aliases\") or []",
        ),
        (
            mailbox::line::ALIAS_LIST_NONE,
            "print(\"%s  aliases=none\" % (d.get(\"mailbox\") or ",
        ),
        (
            mailbox::line::ALIAS_LIST_PRINT,
            "print(\"%s  aliases=%s\" % (d.get(\"mailbox\") or \"?",
        ),
        (
            mailbox::line::ALIAS_ADD_CALL,
            "d = call(\"POST\", \"/cli/mail/alias\", body=body)",
        ),
        (
            mailbox::line::ALIAS_ADD_PRINT,
            "print(\"alias %s -> %s\" % (d.get(\"alias\") or env(",
        ),
        (
            mailbox::line::ALIAS_REMOVE_CALL,
            "d = call(\"POST\", \"/cli/mail/alias/remove\", body=",
        ),
        (
            mailbox::line::ALIAS_REMOVE_PRINT,
            "print(\"removed alias %s from %s\" % (d.get(\"remov",
        ),
        (
            notices::line::OOO_GET_CALL,
            "d = call(\"GET\", \"/cli/mail/ooo\", params={\"addres",
        ),
        (notices::line::OOO_GET_ON, "if d.get(\"enabled\"):"),
        (
            notices::line::OOO_GET_UNTIL,
            "until = d.get(\"until\") or \"none\"",
        ),
        (
            notices::line::OOO_GET_ADDR,
            "d.get(\"address\") or env(\"M_ADDRESS\") or \"?\",",
        ),
        (
            notices::line::OOO_GET_DAYS,
            "d.get(\"days\") if d.get(\"days\") is not None else ",
        ),
        (notices::line::OOO_GET_TEXT, "if d.get(\"text\"):"),
        (
            notices::line::OOO_GET_OFF,
            "print(\"%s  ooo off\" % (d.get(\"address\") or env(\"",
        ),
        (
            notices::line::OOO_SET_DAYS,
            "body[\"days\"] = int(env(\"M_DAYS\"))",
        ),
        (
            notices::line::OOO_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/ooo\", body=body)",
        ),
        (
            notices::line::OOO_SET_ADDR,
            "d.get(\"address\") or env(\"M_ADDRESS\") or \"?\",",
        ),
        (
            notices::line::OOO_SET_DAYS_PRINT,
            "d.get(\"days\") if d.get(\"days\") is not None else ",
        ),
        (
            notices::line::OOO_SET_UNTIL,
            "d.get(\"until\") or \"none\"))",
        ),
        (
            notices::line::OOO_UNSET_CALL,
            "d = call(\"POST\", \"/cli/mail/ooo/unset\", body={\"a",
        ),
        (
            notices::line::OOO_UNSET_PRINT,
            "print(\"ooo unset for %s\" % (d.get(\"address\") or ",
        ),
        (
            notices::line::FOOTER_GET_CALL,
            "d = call(\"GET\", \"/cli/mail/footer\")",
        ),
        (
            notices::line::FOOTER_GET_ON,
            "if d.get(\"enabled\") and d.get(\"text\"):",
        ),
        (
            notices::line::FOOTER_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/footer\", body=body)",
        ),
        (
            notices::line::FOOTER_UNSET_CALL,
            "d = call(\"POST\", \"/cli/mail/footer/unset\", body=",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` server raises at.
#[test]
fn the_hostmail_server_lines_are_the_shell_s() {
    use crate::families::hostmail::server::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::ENABLE_CALL,
            "d = call(\"POST\", \"/cli/mail/server/enable\", ",
        ),
        (line::ENABLE_ALREADY, "elif d.get(\"alreadyEnabled\"):"),
        (line::ENABLE_HELPER, "if d.get(\"helper\"):"),
        (
            line::ENABLE_HINT,
            "print(d.get(\"hint\") or \"mail server: enable ",
        ),
        (
            line::IMPORT_PORT,
            "imapsync[\"port\"] = int(env(\"M_IMAPSYNC_PORT\"",
        ),
        (
            line::IMPORT_CALL,
            "d = call(\"POST\", \"/cli/mail/import\", body=bo",
        ),
        (
            line::IMPORT_PRINT,
            "print(d.get(\"hint\") or \"imported %s message(",
        ),
        (
            line::DISABLE_CALL,
            "d = call(\"POST\", \"/cli/mail/server/disable\",",
        ),
        (
            line::DISABLE_PRINT,
            "print(dumps(d) if as_json else (d.get(\"hint\"",
        ),
        (
            line::UNINSTALL_CALL,
            "d = call(\"POST\", \"/cli/mail/server/uninstall",
        ),
        (
            line::UNINSTALL_PRINT,
            "print(dumps(d) if as_json else (d.get(\"hint\"",
        ),
        (line::HB_FLOAT, "n = float(n)"),
        (
            line::HB_FORMAT,
            "return (\"%d %s\" % (n, unit)) if unit == \"B\" ",
        ),
        (
            line::ACK_CALL,
            "d = call(\"POST\", \"/cli/mail/server/upgrade\",",
        ),
        (
            line::ACK_PRINT,
            "print(dumps(d) if as_json else (d.get(\"hint\"",
        ),
        (
            line::DRY_CALL,
            "d = call(\"POST\", \"/cli/mail/server/upgrade\",",
        ),
        (line::DRY_BLOCKERS, "for b in d.get(\"blockers\") or []:"),
        (
            line::DRY_BLOCKER,
            "print(\"blocked  : [%s] %s\" % (b.get(\"code\"),",
        ),
        (
            line::DRY_HB,
            "_hb(d.get(\"dataBytes\")), _hb(d.get(\"configBy",
        ),
        (
            line::DRY_HB_FREE,
            "d.get(\"snapshotDir\") or \"?\", _hb(d.get(\"free",
        ),
        (
            line::DRY_OUTAGE,
            "out.get(\"typicalSecs\"), out.get(\"worstCaseSe",
        ),
        (
            line::DRY_ASSUMPTION,
            "print(\"           (%s)\" % out.get(\"assumptio",
        ),
        (
            line::DRY_PLAN,
            "for i, s in enumerate(d.get(\"plan\") or [], 1",
        ),
        (line::DRY_ROLLBACK, "for s in d.get(\"rollback\") or []:"),
        (line::DRY_NOTES, "for n in d.get(\"notes\") or []:"),
        (
            line::UP_CALL,
            "d = call(\"POST\", \"/cli/mail/server/upgrade\",",
        ),
        (line::UP_OUTAGE, "out.get(\"typicalSecs\", \"?\")))"),
        (line::UP_SLEEP, "time.sleep(interval)"),
        (line::UP_LOST, "die({\"code\": \"daemon_lost\","),
        (line::UP_UPGRADE, "up = s.get(\"upgrade\") or {}"),
        (line::UP_STEPS, "steps = up.get(\"steps\") or []"),
        (line::UP_STEP_FOR, "for st in steps[seen:]:"),
        (
            line::UP_STEP,
            "print(\"  %-18s %-6s %s\" % (st.get(\"step\"), \"",
        ),
        (line::UP_SEEN, "seen = len(steps)"),
        (
            line::UP_MANUAL,
            "for s in final.get(\"manualSteps\") or []:",
        ),
        (
            line::PERSON_LINK_CALL,
            "d = call(\"POST\", \"/cli/mail/address/person-l",
        ),
        (
            line::ROTATE_ADMIN_CALL,
            "d = call(\"POST\", \"/cli/mail/server/rotate-ad",
        ),
        (
            line::PW_CALL,
            "d = call(\"POST\", \"/cli/mail/address/password",
        ),
        (line::PW_EP_CALL, "_ep(\"imap    \", \"imap\", 443)"),
        (line::PW_EP_SUBMIT, "_ep(\"submit  \", \"submission\", 465)"),
        (line::PW_EP_JMAP, "_ep(\"jmap    \", \"jmap\", 443)"),
        (line::PW_EP_HOST, "host = ep.get(\"host\") or \"?\""),
        (
            line::APW_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/app-password\", pa",
        ),
        (
            line::APW_LIST_LEN,
            "print(\"%s app password(s) for %s:\" % (len(ro",
        ),
        (line::APW_LIST_FOR, "for r in rows:"),
        (
            line::APW_LIST_ROW,
            "print(\"  %s  %s  %s\" % (r.get(\"id\") or \"?\", ",
        ),
        (
            line::APW_ADD_CALL,
            "d = call(\"POST\", \"/cli/mail/app-password\", b",
        ),
        (
            line::APW_REVOKE_CALL,
            "d = call(\"POST\", \"/cli/mail/app-password/rev",
        ),
        (
            line::KEEP_CALL,
            "d = call(\"POST\", \"/cli/mail/credentials/keep",
        ),
        (
            line::PERSON_ALL_CALL,
            "d = call(\"POST\", \"/cli/mail/address/person\",",
        ),
        (
            line::PERSON_ALL_LEN,
            "print(\"marked %d mailbox(es) in %s as %s mai",
        ),
        (line::PERSON_ALL_FOR, "for a in changed:"),
        (
            line::PERSON_ALL_JOIN,
            "print(\"left as the agent's own (its send ide",
        ),
        (
            line::PERSON_CALL,
            "d = call(\"POST\", \"/cli/mail/address/person\",",
        ),
        (
            line::KEEP_ALL_CALL,
            "d = call(\"POST\", \"/cli/mail/credentials/keep",
        ),
        (
            line::KEEP_ALL_KEPT,
            "n = kept.get(\"mailboxPasswords\") if kind == ",
        ),
        (
            line::KEEP_ALL_NEWER,
            "newer = int(left.get(\"createdAfterCutoff\") o",
        ),
        (
            line::KEEP_ALL_UNDATED,
            "undated = int(left.get(\"noCreationTime\") or ",
        ),
        (
            line::KEEP_ALL_ERRORS,
            "for e in d.get(\"listErrors\") or []:",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` cert raises at.
#[test]
fn the_hostmail_cert_lines_are_the_shell_s() {
    use crate::families::hostmail::cert::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::RENEW_CALL,
            "d = call(\"POST\", \"/cli/mail/cert/renew\", bod",
        ),
        (
            line::RENEW_PRINT,
            "print(d.get(\"hint\") or \"ACME renewal queued ",
        ),
        (
            line::OWNER_ACME_CALL,
            "d = call(\"POST\", \"/cli/mail/cert/owner\", bod",
        ),
        (
            line::OWNER_ACME_PRINT,
            "print(d.get(\"hint\") or \"Stalwart ACME orders",
        ),
        (
            line::OWNER_CALL,
            "d = call(\"GET\", \"/cli/mail/cert/owner\")",
        ),
        (line::OWNER_ERROR, "if acme.get(\"error\"):"),
        (
            line::OWNER_AUTO,
            "elif acme.get(\"type\") == \"Automatic\":",
        ),
        (
            line::OWNER_NAMES,
            "print(\"stalwart : ACME Automatic, names: %s\"",
        ),
        (line::OWNER_TYPE, "elif acme.get(\"type\"):"),
        (line::OWNER_MANUAL, "elif acme.get(\"type\") == \"Manual\":"),
        (
            line::OWNER_RESTORE,
            "print(\"           Stalwart ACME is off and K",
        ),
        (line::GIB_FLOAT, "n = float(n)"),
        (line::GIB_D, "return \"%d B\" % n"),
        (line::DUR_INT, "s = int(secs)"),
        (line::DUR_HOURS, "return \"%.1f h\" % (s / 3600.0)"),
        (
            line::PB_MODE,
            "print(\"mode     : %s\" % (b.get(\"mode\") or \"?",
        ),
        (line::PB_BY, "if b.get(\"configuredBy\"):"),
        (line::PB_REASON, "if b.get(\"reason\"):"),
        (line::PB_WINDOW, "if b.get(\"window\"):"),
        (line::PB_RETENTION, "r = b.get(\"retention\") or {}"),
        (line::PB_LOCAL, "loc = r.get(\"local\") or {}"),
        (line::PB_OFFSITE, "off = r.get(\"offsite\") or {}"),
        (line::PB_CAP, "cap = loc.get(\"maxLocalGb\")"),
        (
            line::PB_KEEP,
            "loc.get(\"keepDaily\"), loc.get(\"keepWeekly\"),",
        ),
        (
            line::PB_OFF_KEEP,
            "off.get(\"keepDaily\"), off.get(\"keepWeekly\"),",
        ),
        (line::PB_OBSERVER, "o = b.get(\"observer\") or {}"),
        (
            line::PB_STATUS,
            "line = \"observer : %s\" % (o.get(\"status\") or",
        ),
        (line::PB_LAST, "if o.get(\"lastSuccess\"):"),
        (
            line::PB_DAYS,
            "line += \", %s day(s) of churn\" % (o.get(\"day",
        ),
        (line::PB_CHURN, "cp = o.get(\"churnPerDay\") or {}"),
        (
            line::PB_CHURN_DEL,
            "_gib(cp.get(\"deletedBytes\")), _gib(cp.get(\"m",
        ),
        (
            line::PB_CHURN_NEW,
            "_gib(cp.get(\"createdBytes\")), \"\" if cp.get(\"",
        ),
        (line::PB_STORE_IF, "if o.get(\"storeBytes\") is not None:"),
        (
            line::PB_STORE,
            "_gib(o.get(\"storeBytes\")), _gib(o.get(\"mutab",
        ),
        (
            line::PB_STORE_FS,
            "_gib(o.get(\"fsTotal\")), {True: \"yes\", False:",
        ),
        (line::PB_ERR, "if o.get(\"lastError\"):"),
        (line::PB_CONFIG, "if b.get(\"configError\"):"),
        (
            line::PB_NOTE,
            "print(\"note     : %s\" % (b.get(\"note\") or \"\"",
        ),
        (line::STATUS_CALL, "d = call(\"GET\", \"/cli/mail/backup\")"),
        (line::STATUS_PRINT, "_print_backup(d.get(\"backup\") or {})"),
        (
            line::PLAN_CALL,
            "d = call(\"GET\", \"/cli/mail/backup/plan\", par",
        ),
        (
            line::PLAN_STORE,
            "_gib(st.get(\"bytes\")), st.get(\"path\"), st.ge",
        ),
        (
            line::PLAN_STORE_2,
            "_gib(st.get(\"mutableBytes\")), _gib(st.get(\"c",
        ),
        (
            line::PLAN_DISK,
            "_gib(st.get(\"fsFree\")), _gib(st.get(\"fsTotal",
        ),
        (
            line::PLAN_DISK_2,
            "st.get(\"viewRoot\"), {True: \"yes\", False: \"NO",
        ),
        (
            line::PLAN_CHURN,
            "ch.get(\"label\"), _gib(ch.get(\"deletedPerDay\"",
        ),
        (line::PLAN_CHURN_2, "_gib(ch.get(\"createdPerDay\"))))"),
        (line::PLAN_LOCAL, "loc = r.get(\"local\") or {}"),
        (line::PLAN_OFFSITE, "off = r.get(\"offsite\") or {}"),
        (
            line::PLAN_KEEP,
            "loc.get(\"keepDaily\"), loc.get(\"keepWeekly\"),",
        ),
        (
            line::PLAN_CAP,
            "(\", max %s GiB\" % loc[\"maxLocalGb\"]) if loc.",
        ),
        (
            line::PLAN_OFF_KEEP,
            "off.get(\"keepDaily\"), off.get(\"keepWeekly\"),",
        ),
        (
            line::PLAN_MODES,
            "m = (d.get(\"modes\") or {}).get(name) or {}",
        ),
        (
            line::PLAN_VERDICT,
            "head = \"%-12s verdict %s\" % (name, (m.get(\"v",
        ),
        (line::PLAN_AVAILABLE, "if m.get(\"available\") is False:"),
        (
            line::PLAN_UNAVAILABLE,
            "head += \"  (not available yet: %s)\" % (m.get",
        ),
        (
            line::PLAN_LOCAL_DISK,
            "_gib(m.get(\"firstNightLocalBytes\")), _gib(m.",
        ),
        (
            line::PLAN_LOCAL_DISK_2,
            "_gib(m.get(\"peakBytes\")), _gib(m.get(\"freeAf",
        ),
        (line::PLAN_OFF, "o = m.get(\"offsite\")"),
        (line::PLAN_FU, "fu = o.get(\"firstUploadBytes\") or {}"),
        (
            line::PLAN_UPLOAD,
            "_gib(fu.get(\"low\")), _gib(fu.get(\"high\")), _",
        ),
        (
            line::PLAN_UPLOAD_2,
            "_gib(o.get(\"nightlyUploadBytes\")), _dur(o.ge",
        ),
        (
            line::PLAN_TEMP,
            "_gib(o.get(\"firstTempBytes\")), _gib(o.get(\"n",
        ),
        (
            line::PLAN_DOWN,
            "print(\"  downtime   : ~%s nightly (%s)\" % (_",
        ),
        (
            line::PLAN_PROTECTS,
            "print(\"  protects   : %s\" % (m.get(\"protects",
        ),
        (line::PLAN_NOTES, "for n in d.get(\"notes\") or []:"),
        (
            line::PLAN_HASH,
            "print(\"planHash : %s\" % d.get(\"planHash\"))",
        ),
        (line::SET_INT, "body[k] = int(env(e))"),
        (
            line::SET_CALL,
            "d = call(\"POST\", \"/cli/mail/backup/set\", bod",
        ),
        (
            line::SET_MODE,
            "print(\"backup mode: %s → %s\" % (d.get(\"from\"",
        ),
        (line::SET_PRINT, "_print_backup(d.get(\"backup\") or {})"),
        (
            line::DANE_CALL,
            "d = call(\"POST\", \"/cli/mail/outbound\", body=",
        ),
        (
            line::DANE_PRINT,
            "print(d.get(\"hint\") or \"DANE is optional aga",
        ),
        (line::OUT_CALL, "d = call(\"GET\", \"/cli/mail/outbound\")"),
        (
            line::OUT_WHO,
            "who.get(d.get(\"ipStrategySetBy\"), d.get(\"ipS",
        ),
        (line::OUT_DANE, "for t in d.get(\"dane\") or []:"),
        (
            line::OUT_DANE_ROW,
            "print(\"DANE     : %-12s %s (%s)\" % (t.get(\"n",
        ),
        (
            line::OUT_DANE_WHO,
            "who.get(t.get(\"setBy\"), t.get(\"setBy\") or \"?",
        ),
        (
            line::OUT_DNSSEC,
            "print(\"DNSSEC   : %s\" % (d.get(\"dnssec\") or ",
        ),
        (line::OUT_PENDING, "if d.get(\"pendingReload\"):"),
        (
            line::OUT_PENDING_ROW,
            "print(\"WARNING  : settings saved but not liv",
        ),
        (
            line::NAMES_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/cert/names\", time",
        ),
        (
            line::NAMES_CALL,
            "d = call(\"POST\", \"/cli/mail/cert/names\", bod",
        ),
        (line::NAMES_ROWS, "rows = d.get(\"names\") or []"),
        (line::NAMES_FOR, "for r in rows:"),
        (line::NAMES_CERT, "cert = r.get(\"cert\") or {}"),
        (
            line::NAMES_EXP,
            "exp = (\" expires %s\" % cert[\"expiresAt\"]) if",
        ),
        (
            line::NAMES_ROW,
            "r.get(\"name\") or \"?\", _yn(r.get(\"pointsHere\"",
        ),
        (
            line::NAMES_ROW_2,
            "cert.get(\"state\") or \"?\", exp, r.get(\"issuer",
        ),
        (
            line::NAMES_DNS,
            "if r.get(\"dns\") and r.get(\"pointsHere\") is n",
        ),
        (line::NAMES_REASON, "if r.get(\"reason\"):"),
        (line::NAMES_LAST, "if cert.get(\"lastError\"):"),
        (
            line::NAMES_R_EXP,
            "exp = (\" expires %s\" % r[\"expiresAt\"]) if r.",
        ),
        (
            line::NAMES_R_ROW,
            "print(\"%-34s %s%s\" % (r.get(\"name\") or \"?\", ",
        ),
        (line::NAMES_DETAIL, "if r.get(\"detail\"):"),
        (line::NAMES_RELOAD, "rel = d.get(\"reload\") or {}"),
        (
            line::NAMES_METHOD,
            "line = \"reload   : %s\" % (rel.get(\"method\") ",
        ),
        (line::NAMES_RELOAD_ERR, "if rel.get(\"error\"):"),
        (
            line::NAMES_FAILED,
            "if action != \"list\" and d.get(\"failed\"):",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` dns raises at.
#[test]
fn the_hostmail_dns_lines_are_the_shell_s() {
    use crate::families::hostmail::dns::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::DKIM_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/dkim\", params={\"d",
        ),
        (line::DKIM_SHOW_FOR, "for s in d.get(\"signatures\") or []:"),
        (line::DKIM_SHOW_ROW, "s.get(\"selector\") or \"?\","),
        (line::DKIM_SHOW_EXPECTED, "if s.get(\"expected\"):"),
        (
            line::DKIM_SHOW_NAME,
            "print(\"    TXT %s\" % (s.get(\"name\") or \"\"))",
        ),
        (
            line::DKIM_ROTATE_CALL,
            "d = call(\"POST\", \"/cli/mail/dkim\", body={\"do",
        ),
        (
            line::DKIM_ROTATE_FOR,
            "for rec in d.get(\"records\") or []:",
        ),
        (
            line::DKIM_ROTATE_ROW,
            "print(\"%s  TXT  %s\" % (rec.get(\"name\") or \"?",
        ),
        (
            line::DKIM_RETIRE_CALL,
            "d = call(\"POST\", \"/cli/mail/dkim/retire\",",
        ),
        (
            line::DMARC_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/dmarc\", params={\"",
        ),
        (
            line::DMARC_SHOW_REC,
            "print(\"  txt     : %s\" % (rec.get(\"expected\"",
        ),
        (
            line::DMARC_SHOW_MATCH,
            "print(\"  live    : %s\" % (rec.get(\"match\") o",
        ),
        (
            line::DMARC_REPORT_CALL,
            "d = call(\"POST\", \"/cli/mail/dmarc\",",
        ),
        (line::PTR_SHOW_CALL, "d = call(\"GET\", \"/cli/mail/ptr\")"),
        (
            line::PTR_SHOW_A,
            "print(\"A      : %s\" % (\", \".join(a) if a els",
        ),
        (
            line::PTR_SHOW_RELAY,
            "unresolved = (d.get(\"relayCheck\") or {}).get",
        ),
        (
            line::PTR_SHOW_UNRESOLVED,
            "print(\"relays : could not resolve %s (ptr se",
        ),
        (
            line::PTR_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/ptr/set\", body={",
        ),
        (
            line::PTR_SET_SOURCE,
            "if d.get(\"ptrSource\") == \"authoritative\":",
        ),
        (line::PTR_SET_WARNING, "if settings.get(\"warning\"):"),
        (line::PTR_SET_DC, "if dc.get(\"state\") == \"moved\":"),
        (line::PTR_SET_DC_WARNING, "elif dc.get(\"warning\"):"),
        (
            line::PROFILE_CALL,
            "d = call(\"POST\", \"/cli/mail/profile\", body={",
        ),
        (line::PROFILE_WRITE, "os.write(fd, xml.encode(\"utf-8\"))"),
        (
            line::PROFILE_INCLUDES,
            "print(\"includes : %s\" % \", \".join(d.get(\"inc",
        ),
        (line::PROFILE_NOTES, "for n in d.get(\"notes\") or []:"),
        (line::PROFILE_STDOUT, "sys.stdout.write(xml)"),
        (
            line::PROFILE_STDERR_NOTES,
            "for n in d.get(\"notes\") or []:",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` bans raises at.
#[test]
fn the_hostmail_bans_lines_are_the_shell_s() {
    use crate::families::hostmail::bans::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (line::LIST_CALL, "d = call(\"GET\", \"/cli/mail/bans\")"),
        (
            line::LIST_BITS,
            "bits = [\"%s=%s\" % (k, counts[k]) for k in so",
        ),
        (
            line::LIST_COUNT,
            "print(\"%s blocked  %s\" % (d.get(\"count\") if ",
        ),
        (
            line::LIST_AUTH,
            "auth = [b for b in bans if (b.get(\"reason\") ",
        ),
        (
            line::CLEAR_CALL,
            "d = call(\"POST\", \"/cli/mail/bans/clear\", bod",
        ),
        (
            line::CLEAR_JOIN,
            "\",\".join(d.get(\"destroyed\") or []) or \"(none",
        ),
        (line::CLEAR_WARN, "for w in (d.get(\"warnings\") or []):"),
        (
            line::MIGRATE_INT,
            "body = {\"hours\": int(hours)} if hours else {",
        ),
        (
            line::MIGRATE_CALL,
            "d = call(\"POST\", \"/cli/mail/bans/migrate\", b",
        ),
        (
            line::RESTORE_CALL,
            "d = call(\"POST\", \"/cli/mail/bans/migrate/res",
        ),
        (
            line::STATUS_CALL,
            "d = call(\"GET\", \"/cli/mail/bans/migrate\")",
        ),
        (
            line::ALLOW_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/allowlist\")",
        ),
        (line::ALLOW_LIST_FOR, "for b in rows:"),
        (line::ALLOW_LIST_ROW, "b.get(\"address\") or \"?\","),
        (
            line::ALLOW_ADD_CALL,
            "d = call(\"POST\", \"/cli/mail/allowlist/add\", ",
        ),
        (
            line::ALLOW_REMOVE_CALL,
            "d = call(\"POST\", \"/cli/mail/allowlist/remove",
        ),
        (
            line::ALLOW_REMOVE_JOIN,
            "\",\".join(d.get(\"destroyed\") or []) or \"(none",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` groups raises at.
#[test]
fn the_hostmail_groups_lines_are_the_shell_s() {
    use crate::families::hostmail::groups::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (line::DAV_GET, "d = call(\"GET\", \"/cli/mail/dav\")"),
        (
            line::DAV_POST,
            "d = call(\"POST\", \"/cli/mail/dav\", body=body,",
        ),
        (line::DAV_POLICY, "if d.get(\"policy\") == \"default\":"),
        (line::DAV_ACC, "acc = d.get(\"accounts\")"),
        (
            line::DAV_ACC_GET,
            "acc.get(\"total\", \"?\"), acc.get(\"updated\", \"?",
        ),
        (line::DAV_SKIPPED, "len(acc.get(\"skipped\") or [])))"),
        (
            line::PG_HEAD,
            "g.get(\"address\") or g.get(\"name\") or \"?\", le",
        ),
        (
            line::PG_MAIL,
            "\"\" if g.get(\"receivesMail\") else \" · inbound",
        ),
        (line::PG_FOR, "for m in g.get(\"members\") or []:"),
        (
            line::CREATE_CALL,
            "d = call(\"POST\", \"/cli/mail/group\", body=bod",
        ),
        (
            line::MEMBERS_CALL,
            "d = call(\"POST\", \"/cli/mail/group/members\", ",
        ),
        (
            line::MEMBERS_JOIN,
            "d.get(\"group\") or env(\"M_GROUP\"), \",\".join(d",
        ),
        (
            line::LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/group\", timeout=6",
        ),
        (line::LIST_FOR, "for g in gs:"),
        (line::LIST_PRINT, "print_group(g)"),
        (
            line::SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/group\", params={\"",
        ),
        (line::SHOW_PRINT, "print_group(d.get(\"group\") or {})"),
        (
            line::DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/group/delete\", b",
        ),
        (
            line::DELETE_PRINT,
            "d.get(\"group\") or env(\"M_GROUP\"), d.get(\"sha",
        ),
        (
            line::MANAGE_CALL,
            "d = call(\"POST\", \"/cli/mail/calendar/manage\"",
        ),
        (line::MANAGE_C, "c = d.get(\"calendar\") or {}"),
        (
            line::MANAGE_CREATED,
            "print(\"created calendar '%s' (%s) on %s\" % (",
        ),
        (
            line::MANAGE_RENAMED,
            "c.get(\"previousName\"), c.get(\"name\"), c.get(",
        ),
        (
            line::SHARE_CALL,
            "d = call(\"POST\", \"/cli/mail/calendar/\" + (\"s",
        ),
        (line::SHARE_C, "c = d.get(\"calendar\") or {}"),
        (
            line::SHARE_PRINT,
            "c.get(\"name\"), d.get(\"owner\"), d.get(\"with\")",
        ),
        (
            line::SHARE_REMOVED,
            "print(\"stopped sharing '%s' of %s with %s\" %",
        ),
        (
            line::SHARE_NOTHING,
            "c.get(\"name\"), d.get(\"owner\"), d.get(\"with\")",
        ),
        (
            line::SHARES_CALL,
            "d = call(\"GET\", \"/cli/mail/calendar/shares\",",
        ),
        (line::SHARES_FOR, "for c in cals:"),
        (line::SHARES_SHARES, "shares = c.get(\"shares\") or []"),
        (
            line::SHARES_ROW,
            "print(\"%s · %s%s · %s\" % (c.get(\"id\"), c.get",
        ),
        (
            line::SHARES_DEFAULT,
            "\" · default\" if c.get(\"isDefault\") else \"\",",
        ),
        (
            line::SHARES_LEN,
            "\"not shared\" if not shares else \"%d share(s)",
        ),
        (line::SHARES_EACH, "for s in shares:"),
        (
            line::SHARES_EACH_ROW,
            "print(\"  %s (%s): %s\" % (s.get(\"with\"), s.ge",
        ),
        (
            line::SHARING_POST,
            "d = call(\"POST\", \"/cli/mail/calendar/sharing",
        ),
        (
            line::SHARING_INT,
            "body={\"maxShares\": int(env(\"M_MAX_SHARES\"))}",
        ),
        (
            line::SHARING_GET,
            "d = call(\"GET\", \"/cli/mail/calendar/sharing\"",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` lists raises at.
#[test]
fn the_hostmail_lists_lines_are_the_shell_s() {
    use crate::families::hostmail::lists::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::LIST_CREATE_CALL,
            "d = call(\"POST\", \"/cli/mail/list\", body=body",
        ),
        (
            line::LIST_CREATE_JOIN,
            "\",\".join(d.get(\"members\") or [])))",
        ),
        (
            line::LIST_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/list\", params={\"a",
        ),
        (
            line::LIST_SHOW_JOIN,
            "d.get(\"address\") or env(\"M_ADDRESS\"), \",\".jo",
        ),
        (
            line::LIST_MEMBERS_CALL,
            "d = call(\"POST\", \"/cli/mail/list/members\", b",
        ),
        (
            line::LIST_MEMBERS_JOIN,
            "d.get(\"address\") or env(\"M_ADDRESS\"), \",\".jo",
        ),
        (
            line::LIST_DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/list/delete\", bo",
        ),
        (
            line::TRAIN_CALL,
            "d = call(\"POST\", \"/cli/mail/spam/train\", bod",
        ),
        (
            line::SENDER_CALL,
            "d = call(\"POST\", route, body={\"sender\": env(",
        ),
        (
            line::QUAR_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/spam/quarantine\",",
        ),
        (line::QUAR_LIST_FOR, "for m in rows:"),
        (
            line::QUAR_LIST_ROW,
            "print(\"%s  %s\" % (m.get(\"id\") or \"?\", m.get(",
        ),
        (
            line::QUAR_ID_CALL,
            "d = call(\"POST\", route, body={\"address\": env",
        ),
        (
            line::QUEUE_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/queue\")",
        ),
        (line::QUEUE_LIST_FOR, "for m in rows:"),
        (
            line::QUEUE_LIST_ROW,
            "print(\"%s  retry=%s  from=%s\" % (m.get(\"id\")",
        ),
        (
            line::QUEUE_RETRY_CALL,
            "d = call(\"POST\", \"/cli/mail/queue/retry\", bo",
        ),
        (
            line::QUEUE_DROP_CALL,
            "d = call(\"POST\", \"/cli/mail/queue/drop\", bod",
        ),
        (
            line::ACL_GRANT_CALL,
            "d = call(\"POST\", \"/cli/mail/acl\", body=body)",
        ),
        (
            line::ACL_REVOKE_CALL,
            "d = call(\"POST\", \"/cli/mail/acl/revoke\", bod",
        ),
        (
            line::ACL_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/acl\", params={\"ma",
        ),
        (
            line::AUTO_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/autoconfig\", para",
        ),
        (line::AUTO_SHOW_FOR, "for rec in d.get(\"records\") or []:"),
        (
            line::AUTO_SHOW_ROW,
            "print(\"%s  %s  %s\" % (rec.get(\"type\") or \"?\"",
        ),
        (
            line::AUTO_APPLY_CALL,
            "d = call(\"POST\", \"/cli/mail/autoconfig\", bod",
        ),
        (line::AUTO_APPLY_FOR, "for rec in d.get(\"records\") or []:"),
        (
            line::AUTO_APPLY_ROW,
            "print(\"%s  %s  %s\" % (rec.get(\"type\") or \"?\"",
        ),
        (
            line::AUDIT_CALL,
            "d = call(\"GET\", \"/cli/mail/sieve-audit\", tim",
        ),
        (
            line::AUDIT_ENOTIFY,
            "enotify = \"off\" if en.get(\"disabled\") is Tru",
        ),
        (
            line::AUDIT_SCANNED,
            "print(\"accounts scanned : %s (%s skipped: ad",
        ),
        (line::AUDIT_ACCOUNTS, "for a in d.get(\"accounts\") or []:"),
        (
            line::AUDIT_WHO,
            "who = a.get(\"address\") or (\"%s (not a K2 mai",
        ),
        (
            line::AUDIT_FLAGS,
            "if a.get(\"k2Minted\") and not a.get(\"sieveLoc",
        ),
        (
            line::AUDIT_REDIRECTS,
            "for r in a.get(\"k2RedirectsNoRow\") or []:",
        ),
        (
            line::AUDIT_REDIRECT,
            "print(\"  %-24s %-8s redirect → %s%s, no acti",
        ),
        (line::AUDIT_SCRIPTS, "for sc in a.get(\"scripts\") or []:"),
        (
            line::AUDIT_TARGETS,
            "tg = \", \".join(\"%s → %s%s\" % (t.get(\"action\"",
        ),
        (
            line::AUDIT_TARGETS_OF,
            "for t in sc.get(\"targets\") or []) or \"no for",
        ),
        (
            line::AUDIT_SCRIPT,
            "print(\"  %-24s %-8s %-24s %s\" % ('\"%s\"' % sc",
        ),
        (line::AUDIT_NONE, "if not d.get(\"accounts\"):"),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

/// The lines of `_mail_py` admin raises at.
#[test]
fn the_hostmail_admin_lines_are_the_shell_s() {
    use crate::families::hostmail::admin::line;
    let s = mail_py_script();
    let pins: &[(u32, &str)] = &[
        (
            line::CREATE_CALL,
            "d = call(\"POST\", \"/cli/mail/address/create\",",
        ),
        (
            line::CREATE_CAP,
            "print(\"%s %s   (%s)\" % (word, d.get(\"address",
        ),
        (line::CREATE_EP_HOST, "host = ep.get(\"host\") or \"?\""),
        (line::CREATE_EP_IMAP, "_ep(\"imap    \", \"imap\", 443)"),
        (
            line::CREATE_EP_SUBMIT,
            "_ep(\"submit  \", \"submission\", 465)",
        ),
        (line::CREATE_EP_JMAP, "_ep(\"jmap    \", \"jmap\", 443)"),
        (
            line::DELETE_CALL,
            "d = call(\"POST\", \"/cli/mail/address/delete\",",
        ),
        (
            line::STATUS_REQUEST,
            "d = request(\"GET\", \"/cli/mail/status\", param",
        ),
        (line::STATUS_ERROR, "if d.get(\"error\"):"),
        (
            line::STATUS_NAME0,
            "host = cert.get(\"host\") or (names[0] if name",
        ),
        (
            line::STATUS_ORDER_JOIN,
            "line += \" (orders: %s%s)\" % (\", \".join(order",
        ),
        (
            line::STATUS_ORDER_LEN,
            "\" only\" if len(order) == 1 else \"\")",
        ),
        (
            line::STATUS_PORTS,
            "ports = [p for p in (cert.get(\"ports\") or []",
        ),
        (
            line::STATUS_PORT_STATES,
            "if len({p.get(\"state\") for p in ports}) > 1:",
        ),
        (
            line::STATUS_NAMES_LEN,
            "if len(names) > 1 or (names and host and nam",
        ),
        (
            line::STATUS_SERVES,
            "print(\"           serves: %s\" % \", \".join(na",
        ),
        (
            line::STATUS_OWNER,
            "print(\"owner    : %s — %s\" % (cert[\"owner\"],",
        ),
        (
            line::STATUS_PENDING,
            "% (ob[\"pendingReload\"].get(\"error\") or \"?\"))",
        ),
        (
            line::STATUS_HERE,
            "here = [e for e in extra_names if e.get(\"poi",
        ),
        (
            line::STATUS_BAD,
            "if (e.get(\"cert\") or {}).get(\"state\") != \"is",
        ),
        (
            line::STATUS_BAD_JOIN,
            "(\"; no own cert: \" + \", \".join(bad)) if bad ",
        ),
        (
            line::DOMAIN_ADD_CALL,
            "d = call(\"POST\", \"/cli/mail/domain/add\", bod",
        ),
        (line::DOMAIN_ADD_PRINT, "print_domain(d)"),
        (line::DOMAIN_CMDS_FOR, "for c in cmds:"),
        (line::DOMAIN_CMD, "print(\"  \" + c)"),
        (
            line::DOMAIN_LIST_CALL,
            "d = call(\"GET\", \"/cli/mail/domain/list\")",
        ),
        (
            line::DOMAIN_LIST_W,
            "w = max(max(len(r.get(\"domain\", \"\")) for r i",
        ),
        (line::DOMAIN_LIST_RECS, "recs = r.get(\"records\") or {}"),
        (line::DOMAIN_LIST_REC, "if recs.get(k):"),
        (line::DOMAIN_LIST_NAG, "if r.get(\"dmarcNag\"):"),
        (
            line::DOMAIN_LIST_ROW,
            "print((\"%-\" + str(w) + \"s%-11s%-14s%s\") % (r",
        ),
        (
            line::DOMAIN_SHOW_CALL,
            "d = call(\"GET\", \"/cli/mail/domain/show\", par",
        ),
        (line::DOMAIN_SHOW_PRINT, "print_domain(d)"),
        (
            line::DOMAIN_CHECK_CALL,
            "d = call(\"POST\", \"/cli/mail/domain/check\", b",
        ),
        (line::DOMAIN_CHECK_PRINT, "print_domain(d)"),
        (
            line::DOMAIN_REMOVE_REQUEST,
            "d = request(\"POST\", \"/cli/mail/domain/remove",
        ),
        (line::DOMAIN_REMOVE_ERROR, "err = d.get(\"error\")"),
        (
            line::DOMAIN_REMOVE_HINT,
            "err = dict(err, hint=str(err.get(\"hint\", \"\")",
        ),
        (
            line::DOCTOR_CALL,
            "d = call(\"POST\", \"/cli/mail/doctor\", body=bo",
        ),
        (
            line::CONFIG_GET_CALL,
            "d = call(\"GET\", \"/cli/mail/config\")",
        ),
        (
            line::FWD_CAP_CALL,
            "d = call(\"POST\", \"/cli/mail/config/forward-n",
        ),
        (
            line::FWD_COPIES_CALL,
            "d = call(\"POST\", \"/cli/mail/config/forward-c",
        ),
        (
            line::CONFIG_SET_INT,
            "body[key] = int(v) if key in (\"relayPort\", \"",
        ),
        (
            line::CONFIG_SET_GB,
            "body[\"quotaBytes\"] = int(env(\"M_QUOTA_GB\")) ",
        ),
        (
            line::CONFIG_SET_CALL,
            "d = call(\"POST\", \"/cli/mail/config/set\", bod",
        ),
    ];
    for (n, text) in pins {
        let got = s[*n as usize - 1].trim();
        assert!(got.starts_with(text), "line {n}: {got:?} is not {text:?}");
    }
}

#[test]
fn the_boot_script_runs_the_mail_script_at_its_line_13() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh");
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

mod reads_words {
    use super::super::core::{Env, Plan};
    use super::super::reads::{b64decode, parse};
    use crate::out::json::Value;
    use k2_client::brand::CLI_NAME;

    fn hint(verb: &str, words: &[&str]) -> String {
        match parse(verb, words) {
            Plan::Usage(h) => h,
            other => panic!("{verb} {words:?}: {other:?}"),
        }
    }

    fn env(verb: &str, words: &[&str]) -> Env {
        match parse(verb, words) {
            Plan::Run(e) => e,
            other => panic!("{verb} {words:?}: {other:?}"),
        }
    }

    /// Under any brand: the usage lines name the brand's command, never
    /// another.
    #[test]
    fn usage_lines_name_the_brand_s_command() {
        for (verb, words) in [
            ("inboxes", &["x"][..]),
            ("messages", &["--nope"]),
            ("read", &[]),
            ("read", &["-x"]),
            ("attachments", &[]),
            ("wait", &["--nope"]),
            ("drafts", &["x"]),
            ("outbox", &["--x"]),
            ("outbox", &["cancel", "a", "-x"]),
            ("capabilities", &["-x"]),
            ("folder", &["move"]),
            ("folder", &["list"]),
        ] {
            let h = hint(verb, words);
            assert!(h.contains(&format!("{CLI_NAME} mail ")), "{verb}: {h}");
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{verb}: {h}");
            }
        }
    }

    #[test]
    fn folder_fills_its_words_as_bash_does() {
        let e = env("folder", &["rename", "", "a@x", "", "Old", "New"]);
        assert_eq!(
            (
                e.get("M_ADDRESS"),
                e.get("M_FOLDER_FROM"),
                e.get("M_FOLDER_TO")
            ),
            ("a@x", "Old", "New")
        );
        assert_eq!(parse("folder", &[]), Plan::Help("mail/delete"));
        assert!(crate::text::help("mail/delete").is_some());
    }

    #[test]
    fn numbers_bash_cannot_read_are_the_shell_s() {
        assert_eq!(
            parse("messages", &["--limit", "99999999999999999999"]),
            Plan::Shell
        );
        assert_eq!(
            parse("wait", &["--timeout", "99999999999999999999"]),
            Plan::Shell
        );
        assert_eq!(
            parse(
                "attachments",
                &["m", "--get", "99999999999999999999", "--out", "f"]
            ),
            Plan::Shell
        );
        assert_eq!(
            parse("outbox", &["--limit", "99999999999999999999"]),
            Plan::Shell
        );
        assert!(matches!(
            parse("messages", &["--limit", "0200"]),
            Plan::Run(_)
        ));
        assert!(matches!(
            parse("wait", &["--timeout", "0900"]),
            Plan::Run(_)
        ));
    }

    #[test]
    fn base64_as_python_decodes_it() {
        let s = |t: &str| Value::Str(t.into());
        assert_eq!(b64decode(&s("QUJD")).unwrap(), b"ABC");
        assert_eq!(b64decode(&s("QU JD\n")).unwrap(), b"ABC");
        assert_eq!(b64decode(&s("QQ==")).unwrap(), b"A");
        assert_eq!(b64decode(&s("QQ==QUJD")).unwrap(), b"A");
        assert_eq!(b64decode(&s("=QUJD")).unwrap(), b"ABC");
        assert_eq!(
            b64decode(&s("QUJDR")),
            Err("binascii.Error: Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4".into())
        );
        assert_eq!(
            b64decode(&s("QUJDRA")),
            Err("binascii.Error: Incorrect padding".into())
        );
        assert_eq!(
            b64decode(&Value::Int("1".into())),
            Err(
                "TypeError: argument should be a bytes-like object or ASCII string, not 'int'"
                    .into()
            )
        );
    }
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

mod forward_words {
    use super::super::core::Plan;
    use super::super::forward::{parse, parse_set, Family};
    use k2_client::brand::CLI_NAME;

    fn hint(words: &[&str]) -> String {
        match parse("forward", words) {
            Plan::Usage(h) => h,
            other => panic!("{words:?}: {other:?}"),
        }
    }

    /// Each usage line names the brand's command, and no other.
    #[test]
    fn usage_lines_name_the_brand_s_command() {
        let mut lines: Vec<String> = [
            &["x"][..],
            &["set"],
            &["add", "--to", "a", "--keep", "--no-keep"],
            &["rm", "--keep"],
            &["show", "--to", "a"],
            &["pause", "x"],
        ]
        .iter()
        .map(|w| hint(w))
        .collect();
        for words in [&[][..], &["--to", "a", "x", "y"]] {
            match parse_set(Family::Hostmail, "set", words, "forward_set") {
                Err(Plan::Usage(h)) => lines.push(h),
                other => panic!("{words:?}: {other:?}"),
            }
        }
        for h in lines {
            assert!(h.contains(&format!("{CLI_NAME} ")), "{h}");
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{h}");
            }
        }
    }

    /// `--to` repeats, one address a line; an empty one adds nothing, as
    /// bash's `${_fw_to:+…}` does.
    #[test]
    fn to_repeats_one_a_line() {
        let Plan::Run(e) = parse("forward", &["add", "--to", "", "--to", "a", "--to", "b"]) else {
            panic!("not run");
        };
        assert_eq!(e.get("M_TO"), "a\nb");
        assert_eq!(e.get("M_VERB"), "self_forward_add");
        assert_eq!(e.get("M_KEEP_ARG"), "");
        assert_eq!(e.get("M_RESTART"), "false");
    }
}

mod hostmail_words {
    use super::super::core::Plan;
    use crate::families::hostmail::parse;
    use k2_client::brand::CLI_NAME;

    /// Each usage line that names a command names the brand's, and no
    /// other.
    #[test]
    fn usage_lines_name_the_brand_s_command() {
        for (verb, words) in [
            ("forward", &["x"][..]),
            ("forward", &["show"]),
            ("forward", &["list", "x"]),
            ("quota", &["x"]),
            ("quota", &["get"]),
            ("quota", &["set", "a", "--tb"]),
            ("catchall", &["show"]),
            ("catchall", &["set", "d", "-x"]),
            ("alias", &["add", "m"]),
            ("alias", &["list"]),
            ("ooo", &["set", "a", "--x"]),
            ("ooo", &["show"]),
            ("footer", &["set", "w"]),
            ("footer", &["x"]),
            ("enable", &["x"]),
            ("disable", &["x"]),
            ("uninstall", &[]),
            ("upgrade", &["--x"]),
            ("rotate-admin", &["x"]),
            ("password", &["x"]),
            ("password", &["rotate"]),
            ("password", &["keep", "-x"]),
            ("app-password", &["add"]),
            ("app-password", &["revoke", "a"]),
            ("person", &["a"]),
            ("person", &["link", "a"]),
            ("import", &["a", "--x"]),
            ("cert", &["x"]),
            ("cert", &["renew", "x"]),
            ("cert", &["owner", "x"]),
            ("cert", &["names", "--name"]),
            ("outbound", &["x"]),
            ("backup", &["x"]),
            ("backup", &["status", "x"]),
            ("backup", &["plan", "--x"]),
            ("backup", &["set", "--mode", "local"]),
            ("dkim", &["x"]),
            ("dkim", &["show"]),
            ("dkim", &["retire", "-x"]),
            ("dmarc", &["report-to", "d"]),
            ("ptr", &["show", "x"]),
            ("ptr", &["set"]),
            ("profile", &["a"]),
            ("bans", &["x"]),
            ("bans", &["list", "x"]),
            ("bans", &["clear"]),
            ("bans", &["migrate", "--json"]),
            ("bans", &["migrate", "status", "-x"]),
            ("allowlist", &["x"]),
            ("allowlist", &["add"]),
            ("group", &["x"]),
            ("group", &["list", "-x"]),
            ("group", &["create"]),
            ("calendar", &["x"]),
            ("calendar", &["status", "x"]),
            ("calendar", &["create", "a", "--with"]),
            ("calendar", &["sharing", "x"]),
            ("list", &["x"]),
            ("list", &["create"]),
            ("list", &["members", "add"]),
            ("spam", &["x"]),
            ("spam", &["ham"]),
            ("spam", &["quarantine", "list"]),
            ("queue", &["list", "x"]),
            ("acl", &["grant"]),
            ("autoconfig", &["show"]),
            ("audit-sieve", &["x"]),
            ("domain", &["x"]),
            ("domain", &["list", "a"]),
            ("create", &["a", "-x"]),
            ("doctor", &["-x"]),
            ("config", &["x"]),
        ] {
            let h = match parse(verb, words) {
                Plan::Usage(h) => h,
                other => panic!("{verb} {words:?}: {other:?}"),
            };
            assert!(h.contains(&format!("{CLI_NAME} hostmail ")), "{verb}: {h}");
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{verb}: {h}");
            }
        }
        // status and delete name `mail` verbs in their lines, as bash's do.
        for (verb, words) in [("status", &["x"][..]), ("delete", &[])] {
            let h = match parse(verb, words) {
                Plan::Usage(h) => h,
                other => panic!("{verb} {words:?}: {other:?}"),
            };
            assert!(h.contains(&format!("{CLI_NAME} mail ")), "{verb}: {h}");
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{verb}: {h}");
            }
        }
    }

    /// A `--days` bash can't read is the shell's. `--text -` is the
    /// binary's: the verb reads stdin when it runs.
    #[test]
    fn the_shell_answers_what_it_alone_can() {
        for (verb, words) in [
            ("ooo", &["set", "a", "--text", "-"][..]),
            ("footer", &["set", "--text", "-"]),
        ] {
            let got = format!("{:?}", parse(verb, words));
            assert!(
                got.starts_with("Run(") && got.contains(r#"("M_TEXT_STDIN", "1")"#),
                "{verb} {words:?}: {got}"
            );
        }
        assert_eq!(
            parse(
                "ooo",
                &["set", "a", "--text", "x", "--days", "99999999999999999999"]
            ),
            Plan::Shell
        );
        assert_eq!(
            parse("config", &["--relay-port", "99999999999999999999"]),
            Plan::Shell
        );
    }
}

/// A link, relink or upgrade that runs out of time names its own verb to
/// run again, as the shell's `again` line does: relink for a relink,
/// upgrade with the features for an upgrade, add for a new link.
#[test]
fn a_link_that_runs_out_of_time_names_its_own_verb() {
    use super::owner::timeout_again;
    assert_eq!(
        timeout_again(true, true, "me@x.example", "", "gmail"),
        format!("{CLI_NAME} mail link relink me@x.example")
    );
    assert_eq!(
        timeout_again(false, true, "me@x.example", "calendar-read", "microsoft"),
        format!("{CLI_NAME} mail link upgrade me@x.example --features calendar-read")
    );
    assert_eq!(
        timeout_again(false, false, "me@x.example", "mail", "gmail"),
        format!("{CLI_NAME} mail link add --provider gmail")
    );
    let src = shell("mail.sh");
    let again = src
        .lines()
        .find(|l| {
            l.trim_start()
                .starts_with(r#"again = ("k2 mail link relink"#)
        })
        .expect("the link wait's again line");
    for part in [
        r#""k2 mail link relink %s" % env("M_ADDRESS") if relink"#,
        r#""k2 mail link upgrade %s --features %s" % (env("M_ADDRESS"), env("M_FEATURES")) if upgrade"#,
        r#"else "k2 mail link add --provider %s" % provider"#,
    ] {
        assert!(again.contains(part), "{part:?} not in {again:?}");
    }
}

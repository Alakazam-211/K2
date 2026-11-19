use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A Python text kept in a single-quoted shell variable, `name='...'`,
/// as `python3 -c "$name"` numbers it (line 1 is the empty one after the
/// opening quote).
fn single_quoted(file: &str, name: &str) -> Vec<String> {
    let text = source(file);
    let open = format!("{name}='");
    let body = text
        .split_once(open.as_str())
        .unwrap_or_else(|| panic!("no {open:?} in {file}"))
        .1;
    let end = body.find("\n'\n").expect("its end");
    body[..end].split('\n').map(str::to_string).collect()
}

fn at_line(lines: &[String], n: u32) -> String {
    lines[n as usize - 1].trim().to_string()
}

/// Each line the parse and the call follow, as `cmd_feedback_list` and
/// `cmd_feedback_show` have them.
#[test]
fn the_shell_s_lines() {
    let text = source("feedback.sh");
    for shell in [
        r#"    if _wants_help "$@"; then cmd_help_feedback_list; exit 0; fi"#,
        r#"            --all) all=true; shift ;;"#,
        r#"                [ $# -ge 2 ] || { _feedback_err "usage" "--status requires a value (waiting | needs_discussion | answered | planned | resolved | dismissed)"; exit 2; }"#,
        r#"                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets list [--all] [--status <s>] [--json]""#,
        r#"                _feedback_err "usage" "unexpected argument '$1' — usage: k2 tickets list [--all] [--status <s>] [--json]""#,
        r#"            waiting|needs_discussion|answered|planned|resolved|dismissed) ;;"#,
        r#"                _feedback_err "usage" "invalid --status '$status' — valid: waiting, needs_discussion, answered, planned, resolved, dismissed""#,
        r#"    _cli_use_passport_project"#,
        r#"        resp=$(cli_request "/cli/feedback/list" "status=$(urlencode "$status")") || resp="""#,
        r#"        resp=$(cli_request "/cli/feedback/list" "all=1") || resp="""#,
        r#"        resp=$(cli_request "/cli/feedback/list") || resp="""#,
        r#"        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)""#,
        r#"    RESP="$resp" AS_JSON="$as_json" python3 - <<'PYEOF'"#,
        r#"    if _wants_help "$@"; then cmd_help_feedback_show; exit 0; fi"#,
        r#"                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets show <id> [--html] [--json]""#,
        r#"                if [ -z "$id" ]; then id="$1"; else"#,
        r#"                    _feedback_err "usage" "unexpected extra argument '$1' — usage: k2 tickets show <id>""#,
        r#"        _feedback_err "usage" "show requires an <id> (from ask/list; a short unique prefix works)""#,
        r#"    ABASE="$BASE_URL" ATOKEN="$TOKEN" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)" python3 - <<'PYEOF'"#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "feedback.sh no longer has {shell:?}"
        );
    }
    assert!(text.contains(
        "_feedback_err() {\n    local code=\"$1\" hint=\"$2\"\n    CODE=\"$code\" HINT=\"$hint\" python3 -c 'import json,os\nprint(json.dumps({\"error\": {\"code\": os.environ[\"CODE\"], \"hint\": os.environ[\"HINT\"]}}))' >&2 \\\n"
    ));
    let router = source("router.sh");
    assert!(router.contains("            list)    shift 2; cmd_feedback_list \"$@\" ;;\n            show)    shift 2; cmd_feedback_show \"$@\" ;;\n"));
    assert!(router.contains("    feedback|tickets)\n"));
    // No tool id: an open tool.
    let policy = source("cli-tools-policy.sh");
    let ids = policy
        .split_once("_cli_tool_id_for_verb() {\n")
        .expect("_cli_tool_id_for_verb")
        .1;
    let ids = &ids[..ids.find("\n}\n").expect("its end")];
    assert!(ids.contains("        *) echo \"\" ;;"));
    for word in ["feedback", "tickets"] {
        assert!(!ids.contains(word), "{word} has a tool id now");
    }
}

#[test]
fn list_s_python() {
    let l = single_quoted("feedback.sh", "_feedback_list_py");
    for (n, text) in [
        (
            3,
            r#"raw = sys.stdin.buffer.read().decode("utf-8", "replace")"#,
        ),
        (5, "d = json.loads(raw)"),
        (line::L_ERR, r#"err = d.get("error")"#),
        (line::L_CODE, r#"code = err.get("code")"#),
        (
            16,
            r#"sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)"#,
        ),
        (line::L_JSON, "print(json.dumps(d))"),
        (line::L_FOR, r#"for i in d.get("items", []):"#),
        (22, r#"print("%-10s%-18s%2s   %-10s%s" % ("#),
        (
            line::L_ROW,
            r#"str(i.get("id", ""))[:8], i.get("status", "?"),"#,
        ),
        (
            24,
            r#"i.get("priority", "?"), i.get("kind", "?"), i.get("title", "")))"#,
        ),
    ] {
        assert_eq!(at_line(&l, n), text, "line {n}");
    }
    assert_eq!(l.len(), 24);
}

#[test]
fn show_s_python() {
    let l = single_quoted("feedback.sh", "_feedback_show_py");
    for (n, text) in [
        (line::S_DIE_CODE, r#"code = err.get("code")"#),
        (
            15,
            r#"raw = sys.stdin.buffer.read().decode("utf-8", "replace")"#,
        ),
        (
            line::S_UNPARSEABLE,
            r#"die({"code": "bad_response", "hint": "daemon returned an unparseable response: %r" % raw[:200]})"#,
        ),
        (
            line::S_UNEXPECTED,
            r#"die({"code": "bad_response", "hint": "daemon returned an unexpected response: %r" % raw[:200]})"#,
        ),
        (line::S_ERR, "die(err)"),
        (
            26,
            r#"brief = d.get("brief") if isinstance(d.get("brief"), dict) else None"#,
        ),
        (
            line::S_NO_BRIEF,
            r#"die({"code": "no_brief", "hint": "ticket %s has no HTML brief" % str(d.get("id", ""))[:8]})"#,
        ),
        (line::S_HTML_JSON, "print(json.dumps(d))"),
        (35, r#"sys.stdout.write(brief["html"])"#),
        (39, r#"d.pop("brief", None)"#),
        (line::S_JSON, "print(json.dumps(d))"),
        (line::S_AGO_INT, "s = int(time.time() - ts)"),
        (46, r#"if s < 60: return "just now""#),
        (47, r#"if s < 3600: return "%dm ago" % (s // 60)"#),
        (48, r#"if s < 172800: return "%dh ago" % (s // 3600)"#),
        (49, r#"return "%dd ago" % (s // 86400)"#),
        (52, r#"if n < 1024: return "%d B" % n"#),
        (
            line::S_SIZE_KB,
            r#"if n < 1024 * 1024: return "%.1f KB" % (n / 1024.0)"#,
        ),
        (
            line::S_SIZE_MB,
            r#"return "%.1f MB" % (n / (1024.0 * 1024.0))"#,
        ),
        (
            56,
            r#"print("feedback : %s  (%s)" % (str(d.get("id", ""))[:8], d.get("status", "?")))"#,
        ),
        (57, r#"print("title    : %s" % d.get("title", ""))"#),
        (
            58,
            r#"print("kind     : %s · priority %s" % (d.get("kind", "?"), d.get("priority", "?")))"#,
        ),
        (59, r#"asked = "%s" % (d.get("agentName") or "?")"#),
        (61, r#"asked += " · session %s" % str(d["sessionId"])[:8]"#),
        (
            line::S_ASKED,
            r#"asked += " · %s" % ago(d.get("createdAt", 0))"#,
        ),
        (64, r#"assignees = d.get("assignees") or []"#),
        (
            line::S_ASSIGNED,
            r#"print("assigned : %s" % (", ".join(assignees) if assignees else "(none)"))"#,
        ),
        (
            line::S_OPTIONS,
            r#"print("options  : %s" % " | ".join(d["options"]))"#,
        ),
        (69, r#"print("answer   : %s" % d["answer"])"#),
        (71, r#"body_lines = str(d["body"]).splitlines() or [""]"#),
        (74, r#"print(" " * 11 + extra)"#),
        (76, r#"n = d.get("briefBytes")"#),
        (78, r#"n = brief.get("bytes")"#),
        (
            line::S_BRIEF,
            r#"% (size(int(n)) if isinstance(n, (int, float)) else "?", str(d.get("id", ""))[:8]))"#,
        ),
        (81, r#"text = str((brief or {}).get("text") or "")"#),
        (83, "for line in lines[:40]:"),
        (
            86,
            r#"print("  … %d more lines (open the ticket, or use --html)" % (len(lines) - 40))"#,
        ),
        (line::S_FOR, r#"for c in d.get("comments", []):"#),
        (
            line::S_C,
            r#"lines = str(c.get("body", "")).splitlines() or [""]"#,
        ),
        (
            line::S_AT,
            r#"print("  %-9s%-10s%s" % (c.get("author", "?"), "(%s)" % ago(c.get("at", 0)), lines[0]))"#,
        ),
        (93, r#"print(" " * 21 + extra)"#),
        (
            line::S_FILES,
            r#"for line in ticket_file_lines(c.get("attachments"), d.get("projectPath"), " " * 21):"#,
        ),
    ] {
        assert_eq!(at_line(&l, n), text, "line {n}");
    }
    assert_eq!(l.len(), 96);
}

/// `_ticket_files_check_py`, as `exec` numbers it (line 1 is the empty
/// one after the opening quote).
#[test]
fn the_files_python() {
    let text = source("browser.sh");
    let body = text
        .split_once("_ticket_files_check_py='")
        .expect("_ticket_files_check_py")
        .1;
    let body = &body[..body.find("\n'\n").expect("its end")];
    let l: Vec<String> = body.split('\n').map(str::to_string).collect();
    for (n, text) in [
        (9, r#"if n < 1024: return "%d B" % n"#),
        (
            line::T_SIZE_KB,
            r#"if n < 1024 * 1024: return "%.1f KB" % (n / 1024.0)"#,
        ),
        (
            line::T_SIZE_MB,
            r#"return "%.1f MB" % (n / (1024.0 * 1024.0))"#,
        ),
        (line::T_FOR, "for a in atts or []:"),
        (16, "if not isinstance(a, dict):"),
        (18, r#"if a.get("state") == "hidden":"#),
        (
            19,
            r#"out.append("%s[file] (not shared with you)" % indent)"#,
        ),
        (21, r#"rel = str(a.get("path", ""))"#),
        (
            line::T_JOIN,
            "shown = os.path.join(root, rel) if root else rel",
        ),
        (23, r#"if a.get("line"):"#),
        (
            24,
            r#"shown += ":%s" % a["line"] + ("-%s" % a["lineEnd"] if a.get("lineEnd") else "")"#,
        ),
        (
            line::T_EXTRA,
            r#"extra = {"changed": " (changed since sent)", "gone": " (gone)"}.get(a.get("state"), "")"#,
        ),
        (
            line::T_SIZE,
            r#"out.append("%s[file] %s (%s)%s" % (indent, shown, ticket_size(int(a.get("size") or 0)), extra))"#,
        ),
    ] {
        assert_eq!(at_line(&l, n), text, "line {n}");
    }
}

fn words(list: &[&str]) -> Option<Result<Ask, String>> {
    parse(list)
}

fn usage(hint: &str) -> Option<Result<Ask, String>> {
    Some(Err(format!(
        "{{\"error\": {{\"code\": \"usage\", \"hint\": \"{hint}\"}}}}"
    )))
}

#[test]
fn the_words() {
    let list = |all, status: &str, json| {
        Some(Ok(Ask::List {
            all,
            status: status.into(),
            json,
        }))
    };
    assert_eq!(words(&["list"]), list(false, "", false));
    assert_eq!(
        words(&["list", "--all", "--json", "--status", "planned"]),
        list(true, "planned", true)
    );
    // An empty status is no status; the last one counts.
    assert_eq!(words(&["list", "--status", ""]), list(false, "", false));
    assert_eq!(
        words(&["list", "--status", "x", "--status", "waiting"]),
        list(false, "waiting", false)
    );
    assert_eq!(
        words(&["list", "--status"]),
        usage("--status requires a value (waiting | needs_discussion | answered | planned | resolved | dismissed)")
    );
    assert_eq!(
        words(&["list", "-"]),
        usage("unknown flag '-' \\u2014 usage: k2 tickets list [--all] [--status <s>] [--json]")
    );
    assert_eq!(
        words(&["list", "x", "--bogus"]),
        usage("unexpected argument 'x' \\u2014 usage: k2 tickets list [--all] [--status <s>] [--json]")
    );
    // A bad word comes first, the status after the loop.
    assert_eq!(
        words(&["list", "--status", "nope", "--bogus"]),
        usage(
            "unknown flag '--bogus' \\u2014 usage: k2 tickets list [--all] [--status <s>] [--json]"
        )
    );
    assert_eq!(
        words(&["list", "--status", "nope"]),
        usage("invalid --status 'nope' \\u2014 valid: waiting, needs_discussion, answered, planned, resolved, dismissed")
    );
    assert_eq!(
        words(&["list", "--status", "--json"]),
        usage("invalid --status '--json' \\u2014 valid: waiting, needs_discussion, answered, planned, resolved, dismissed")
    );
    let show = |id: &str, json, html| {
        Some(Ok(Ask::Show {
            id: id.into(),
            json,
            html,
        }))
    };
    assert_eq!(words(&["show", "ab12"]), show("ab12", false, false));
    assert_eq!(
        words(&["show", "--html", "ab12", "--json"]),
        show("ab12", true, true)
    );
    assert_eq!(words(&["show", "", "ab12"]), show("ab12", false, false));
    assert_eq!(
        words(&["show"]),
        usage("show requires an <id> (from ask/list; a short unique prefix works)")
    );
    assert_eq!(
        words(&["show", ""]),
        usage("show requires an <id> (from ask/list; a short unique prefix works)")
    );
    assert_eq!(
        words(&["show", "a", "b"]),
        usage("unexpected extra argument 'b' \\u2014 usage: k2 tickets show <id>")
    );
    assert_eq!(
        words(&["show", "a", "-x"]),
        usage("unknown flag '-x' \\u2014 usage: k2 tickets show <id> [--html] [--json]")
    );
    // A non-ASCII word is escaped, as json.dumps writes it.
    assert_eq!(
        words(&["show", "a", "é"]),
        usage("unexpected extra argument '\\u00e9' \\u2014 usage: k2 tickets show <id>")
    );
    for help in [&["list", "--help"][..], &["show", "x", "-h"], &["ask"], &[]] {
        assert_eq!(words(help), None, "{help:?}");
    }
}

#[test]
fn python_s_str_methods() {
    let lines = |t: &str| -> Vec<String> {
        PyStr::from(t)
            .splitlines()
            .iter()
            .map(PyStr::render)
            .collect()
    };
    // What Python 3 returned for each.
    assert_eq!(
        lines("ab\r\ncd\u{b}ef\u{1c}g\u{85}h\u{2028}i\n\n"),
        ["ab", "cd", "ef", "g", "h", "i", ""]
    );
    assert_eq!(lines("x\n"), ["x"]);
    assert_eq!(lines("\n"), [""]);
    assert!(lines("").is_empty());
    assert_eq!(
        lines("a\rb\u{c}c\u{1d}d\u{1e}e\u{2029}f"),
        ["a", "b", "c", "d", "e", "f"]
    );
    assert_eq!(PyStr::from("日本").ljust(4), "日本  ");
    assert_eq!(PyStr::from("7").rjust(2), " 7");
    assert_eq!(PyStr::from("123").rjust(2), "123");
    assert_eq!(PyStr::of(&Value::Int("5".into())).render(), "5");
    assert_eq!(PyStr::of(&Value::Null).render(), "None");
    assert_eq!(PyStr::from("abcdefghij").head(8).render(), "abcdefgh");
}

#[test]
fn python_s_int() {
    for (v, want) in [
        (s(" 12 "), "12"),
        (s("1_000"), "1000"),
        (s("+7"), "7"),
        (s("-0"), "0"),
        (s("007"), "7"),
        (Value::Bool(true), "1"),
        (Value::Float(2047.9), "2047"),
        (Value::Float(-3.5), "-3"),
        (
            Value::Int("99999999999999999999".into()),
            "99999999999999999999",
        ),
    ] {
        assert_eq!(py_int(&v).as_deref(), Ok(want), "{v:?}");
    }
    for (v, want) in [
        (s("1__0"), "ValueError: invalid literal for int() with base 10: '1__0'"),
        (s("_1"), "ValueError: invalid literal for int() with base 10: '_1'"),
        (s(" "), "ValueError: invalid literal for int() with base 10: ' '"),
        (s("1.5"), "ValueError: invalid literal for int() with base 10: '1.5'"),
        (
            Value::Float(f64::NAN),
            "ValueError: cannot convert float NaN to integer",
        ),
        (
            Value::Float(f64::INFINITY),
            "OverflowError: cannot convert float infinity to integer",
        ),
        (
            Value::Array(vec![]),
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not 'list'",
        ),
    ] {
        assert_eq!(py_int(&v), Err(want.to_string()), "{v:?}");
    }
}

/// What Python 3 printed for `size(n)`.
#[test]
fn python_s_size() {
    for (n, want) in [
        ("0", "0 B"),
        ("-5", "-5 B"),
        ("1023", "1023 B"),
        ("1024", "1.0 KB"),
        ("1280", "1.2 KB"),
        ("1792", "1.8 KB"),
        ("3584", "3.5 KB"),
        ("10291", "10.0 KB"),
        ("1048575", "1024.0 KB"),
        ("1048576", "1.0 MB"),
        ("2359296", "2.2 MB"),
        ("9007199254740993", "8589934592.0 MB"),
    ] {
        assert_eq!(size(n, 1, 2), Ok(want.to_string()), "{n}");
    }
    let huge = "9".repeat(400);
    assert_eq!(
        size(&huge, 1, 2),
        Err((
            2,
            "OverflowError: int too large to convert to float".to_string()
        ))
    );
    // Python's '%.1f' % (1e300 / (1024.0 * 1024.0)): every digit.
    let big = tenths(1e300, 20);
    assert_eq!(big.len(), 296);
    assert!(big.starts_with("9536743164062500500724413"), "{big}");
    assert!(big.ends_with("160.0"), "{big}");
}

#[test]
fn python_s_ago() {
    let now = 1_800_000_000.5;
    let at = |t: f64| ago(&Value::Float(t), now, 74);
    assert_eq!(at(now - 59.9).unwrap(), "just now");
    assert_eq!(at(now + 500.0).unwrap(), "just now");
    assert_eq!(at(now - 60.0).unwrap(), "1m ago");
    assert_eq!(at(now - 3599.0).unwrap(), "59m ago");
    assert_eq!(at(now - 3600.0).unwrap(), "1h ago");
    assert_eq!(at(now - 172_799.0).unwrap(), "47h ago");
    assert_eq!(at(now - 172_800.0).unwrap(), "2d ago");
    assert_eq!(ago(&Value::Int("0".into()), now, 74).unwrap(), "20833d ago");
    assert_eq!(ago(&Value::Bool(true), now, 74).unwrap(), "20833d ago");
    // A number far in the past: every digit of the whole days.
    assert_eq!(
        ago(&Value::Float(-1e30), 0.0, 74).unwrap(),
        format!(
            "{}d ago",
            floor_div("1000000000000000019884624838656", 86400)
        )
    );
    assert_eq!(floor_div("172800", 86400), "2");
    assert_eq!(floor_div("86399", 86400), "0");
    for (v, want) in [
        (
            s("x"),
            "TypeError: unsupported operand type(s) for -: 'float' and 'str'",
        ),
        (
            Value::Null,
            "TypeError: unsupported operand type(s) for -: 'float' and 'NoneType'",
        ),
        (
            Value::Float(f64::NAN),
            "ValueError: cannot convert float NaN to integer",
        ),
        (
            Value::Float(f64::NEG_INFINITY),
            "OverflowError: cannot convert float infinity to integer",
        ),
        (
            Value::Int("9".repeat(400)),
            "OverflowError: int too large to convert to float",
        ),
    ] {
        let e = ago(&v, now, 74).unwrap_err();
        assert_eq!(e.frames, vec![(74, "<module>"), (line::S_AGO_INT, "ago")]);
        assert_eq!(e.exception, want, "{v:?}");
    }
}

fn parsed(text: &str) -> Value {
    json::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn the_files_lines() {
    let lines = |atts: &str, root: &str| ticket_file_lines(&parsed(atts), &parsed(root), "  ");
    assert_eq!(
        lines(
            r#"[{"path":"a.png","size":2048},{"path":"/abs/b","size":"12","state":"changed","line":3,"lineEnd":9},
               {"state":"hidden"},"skip",{"path":"c","state":"gone","line":4},{"size":0}]"#,
            r#""/srv/acme""#
        )
        .unwrap(),
        [
            "  [file] /srv/acme/a.png (2.0 KB)",
            "  [file] /abs/b:3-9 (12 B) (changed since sent)",
            "  [file] (not shared with you)",
            "  [file] /srv/acme/c:4 (0 B) (gone)",
            "  [file] /srv/acme/ (0 B)",
        ]
    );
    assert_eq!(
        lines(r#"[{"path":"x"}]"#, r#""/srv/""#).unwrap(),
        ["  [file] /srv/x (0 B)"]
    );
    assert_eq!(
        lines(r#"[{"path":"x"}]"#, "null").unwrap(),
        ["  [file] x (0 B)"]
    );
    assert!(lines("null", "null").unwrap().is_empty());
    assert!(lines(r#""ab""#, "null").unwrap().is_empty());
    let e = |atts: &str, root: &str| lines(atts, root).unwrap_err().exception;
    assert_eq!(
        e(r#"[{"path":"x"}]"#, "5"),
        "TypeError: expected str, bytes or os.PathLike object, not int"
    );
    assert_eq!(e("7", "null"), "TypeError: 'int' object is not iterable");
    assert_eq!(
        e(r#"[{"state":["x"]}]"#, "null"),
        "TypeError: unhashable type: 'list'"
    );
    assert_eq!(
        e(r#"[{"size":"big"}]"#, "null"),
        "ValueError: invalid literal for int() with base 10: 'big'"
    );
    let r = lines(&format!(r#"[{{"size":{}}}]"#, "9".repeat(400)), "null").unwrap_err();
    assert_eq!(
        r.frames,
        vec![
            (line::S_FILES, "<module>"),
            (line::T_SIZE, "ticket_file_lines"),
            (line::T_SIZE_MB, "ticket_size")
        ]
    );
}

#[test]
fn list_s_reader() {
    let r = list_reader(
        br#"{"items":[{"id":"abcdefghijk","status":"waiting","priority":2,"kind":"question","title":"Ship it? \u00e9"},{}]}"#,
        false,
    );
    assert_eq!(
        r.stdout,
        "ID        STATUS             PRI  KIND      TITLE\nabcdefgh  waiting            2   question  Ship it? \u{e9}\n          ?                  ?   ?         \n"
    );
    assert_eq!((r.stderr.as_str(), r.exit), ("", Exit::Ok));
    let r = list_reader(br#"{"items":[],"x":"\u00e9"}"#, true);
    assert_eq!(r.stdout, "{\"items\": [], \"x\": \"\\u00e9\"}\n");
    let r = list_reader(br#"{"error":{"code":"not_found","hint":"no"}}"#, false);
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit),
        (
            "",
            "{\"error\": {\"code\": \"not_found\", \"hint\": \"no\"}}\n",
            Exit::NotFound
        )
    );
    let r = list_reader(br#"{"error":"bad"}"#, false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            "{\"error\": {\"code\": \"error\", \"hint\": \"bad\"}}\n",
            Exit::Error
        )
    );
    let r = list_reader(br#"{"error":{"code":"usage"}}"#, false);
    assert_eq!(r.exit, Exit::Usage);
    let r = list_reader(br#"{"error":3}"#, false);
    assert_eq!(
        r.stderr,
        "{\"error\": 3}\nTraceback (most recent call last):\n  File \"<stdin>\", line 14, in <module>\nAttributeError: 'int' object has no attribute 'get'\n"
    );
    let r = list_reader(b"nope", false);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            "{\"error\": {\"code\": \"bad_response\", \"hint\": \"daemon returned an unparseable response: 'nope'\"}}\n",
            Exit::Error
        )
    );
    let r = list_reader(br#"{"items":["x"]}"#, false);
    assert!(
        r.stderr.ends_with(
            "line 22, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"
        ),
        "{r:?}"
    );
    assert_eq!(
        r.stdout,
        "ID        STATUS             PRI  KIND      TITLE\n"
    );
}

#[test]
fn show_s_reader() {
    let now = 1_800_000_000.0;
    let body = r#"{"id":"fb_12345678zz","status":"waiting","title":"Pick one","kind":"question","priority":1,
        "agentName":"acme","sessionId":"sess-abcdefghij","createdAt":1799999700,"assignees":["ann","bob"],
        "options":["A","B"],"answer":"B","body":"Line one\nLine two","hasBrief":true,
        "brief":{"html":"<p>x</p>","text":"Brief text","bytes":1280},"projectPath":"/srv/acme",
        "comments":[{"author":"ann","at":1799996400,"body":"ok\nsecond","attachments":[{"path":"a.png","size":2048}]}]}"#;
    let r = show_reader(body, false, false, now);
    assert_eq!(
        r.stdout,
        "feedback : fb_12345  (waiting)\n\
         title    : Pick one\n\
         kind     : question \u{b7} priority 1\n\
         asked by : acme \u{b7} session sess-abc \u{b7} 5m ago\n\
         assigned : ann, bob\n\
         options  : A | B\n\
         answer   : B\n\
         body     : Line one\n\
         \x20          Line two\n\
         brief    : HTML, 1.2 KB  (k2 tickets show fb_12345 --html > brief.html)\n\
         \x20 Brief text\n\
         \n\
         thread:\n\
         \x20 ann      (1h ago)  ok\n\
         \x20                    second\n\
         \x20                    [file] /srv/acme/a.png (2.0 KB)\n"
    );
    assert_eq!((r.stderr.as_str(), r.exit), ("", Exit::Ok));
    let html = show_reader(body, false, true, now);
    assert_eq!((html.stdout.as_str(), html.exit), ("<p>x</p>", Exit::Ok));
    let json = show_reader(r#"{"id":"x","brief":{"html":"h"},"z":1}"#, true, false, now);
    assert_eq!(json.stdout, "{\"id\": \"x\", \"z\": 1}\n");
    let both = show_reader(r#"{"id":"x","brief":{"html":"h"}}"#, true, true, now);
    assert_eq!(
        both.stdout,
        "{\"id\": \"x\", \"brief\": {\"html\": \"h\"}}\n"
    );
    let none = show_reader(r#"{"id":"plain123456"}"#, false, true, now);
    assert_eq!(
        (none.stdout.as_str(), none.stderr.as_str(), none.exit),
        (
            "",
            "{\"error\": {\"code\": \"no_brief\", \"hint\": \"ticket plain123 has no HTML brief\"}}\n",
            Exit::Error
        )
    );
    let bare = show_reader("{}", false, false, now);
    assert_eq!(
        bare.stdout,
        "feedback :   (?)\ntitle    : \nkind     : ? \u{b7} priority ?\nasked by : ? \u{b7} 20833d ago\nassigned : (none)\n\nthread:\n"
    );
    let list = show_reader("[1]", false, false, now);
    assert_eq!(
        list.stderr,
        "{\"error\": {\"code\": \"bad_response\", \"hint\": \"daemon returned an unexpected response: '[1]'\"}}\n"
    );
    let err = show_reader(r#"{"error":["x"]}"#, false, false, now);
    assert_eq!(
        err.stderr,
        "{\"error\": [\"x\"]}\nTraceback (most recent call last):\n  File \"<stdin>\", line 36, in <module>\n  File \"<stdin>\", line 15, in die\nAttributeError: 'list' object has no attribute 'get'\n"
    );
    let many: Vec<String> = (0..45).map(|i| format!("t{i}")).collect();
    let long = show_reader(
        &format!(
            r#"{{"hasBrief":true,"briefBytes":5,"brief":{{"text":"{}"}}}}"#,
            many.join("\\n")
        ),
        false,
        false,
        now,
    );
    assert!(long
        .stdout
        .contains("brief    : HTML, 5 B  (k2 tickets show  --html > brief.html)\n  t0\n"));
    assert!(long
        .stdout
        .contains("\n  t39\n  \u{2026} 5 more lines (open the ticket, or use --html)\n"));
    let joined = show_reader(r#"{"assignees":"ab","options":{"k":1}}"#, false, false, now);
    assert!(
        joined.stdout.contains("assigned : a, b\noptions  : k\n"),
        "{joined:?}"
    );
    let bad = show_reader(r#"{"assignees":[1]}"#, false, false, now);
    assert!(bad.stderr.ends_with(
        "line 77, in <module>\nTypeError: sequence item 0: expected str instance, int found\n"
    ));
}

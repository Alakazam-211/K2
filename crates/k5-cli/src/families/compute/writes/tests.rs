use super::cancel::{self, Ask};
use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python of the first `python3 -c '` at or after `after` in `file`,
/// its lines as `python3 -c` numbers them (line 1 is the rest of the
/// `-c '` line, empty for these readers).
fn python_after(file: &str, after: &str) -> Vec<String> {
    let text = source(file);
    let from = text
        .find(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"));
    let start = "python3 -c '";
    let body = &text[from..];
    let body = &body[body.find(start).expect("a python3 -c '") + start.len()..];
    let end = body.find('\'').expect("the end of the Python");
    body[..end].split('\n').map(str::to_string).collect()
}

fn at(lines: &[String], n: u32) -> &str {
    lines[n as usize - 1].trim()
}

/// Each line the parse and the calls follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let lib = source("compute-lib.sh");
    let cmd = source("compute-command.sh");
    for (text, shell) in [
        (&cmd, r#"                    --force|-f) force=1; shift ;;"#),
        (&cmd, r#"                    --if-queued) ifq=1; shift ;;"#),
        (
            &cmd,
            r#"                    --all-mine) scope="session"; shift ;;"#,
        ),
        (
            &cmd,
            r#"                    --workspace-all) scope="workspace"; shift ;;"#,
        ),
        (
            &cmd,
            r#"                    --tag) ctag="${2:-}"; shift 2 ;;"#,
        ),
        (
            &cmd,
            r#"                    --state) cstate="${2:-}"; shift 2 ;;"#,
        ),
        (
            &cmd,
            r#"                    --node) cnode="${2:-}"; shift 2 ;;"#,
        ),
        (
            &cmd,
            r#"                    --workspace) cws="${2:-}"; shift 2 ;;"#,
        ),
        (
            &cmd,
            r#"                    -*) echo "$cusage (unknown flag '$1')" >&2; exit 2 ;;"#,
        ),
        (
            &cmd,
            r#"                        [ -z "$cjob" ] || { echo "$cusage (one job)" >&2; exit 2; }"#,
        ),
        (
            &cmd,
            r#"                [ -z "$cjob" ] || { echo "$cusage (a job id or --all-mine, not both)" >&2; exit 2; }"#,
        ),
        (
            &cmd,
            r#"                case "${cstate:-queued}" in queued|running|any) ;; *) echo "$cusage (--state is queued, running or any)" >&2; exit 2 ;; esac"#,
        ),
        (
            &cmd,
            r#"                    echo "[k2 compute] --state ${cstate} stops started jobs only with --force; without it they are listed as skipped" >&2"#,
        ),
        (
            &cmd,
            r#"                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_filter_j="$filt" K2_CJ_force_b="$fb")""#,
        ),
        (
            &cmd,
            r#"            [ -n "$cjob" ] || { echo "$cusage" >&2; exit 2; }"#,
        ),
        (
            &cmd,
            r#"            [ "$ifq" = "1" ] && [ "$force" = "1" ] && { echo "$cusage (--if-queued or --force, not both)" >&2; exit 2; }"#,
        ),
        (
            &cmd,
            r#"                craw="$(_compute_send POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState=queued)")""#,
        ),
        (
            &cmd,
            r#"                if [ "$_CP_STATUS" = "409" ] && printf '%s' "$_CP_BODY" | grep -Eq '"code":"(job_started|already_ended)"'; then"#,
        ),
        (
            &cmd,
            r#"                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState="$if_state")""#,
        ),
        (
            &cmd,
            r#"            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }"#,
        ),
        (
            &lib,
            r#"    [ -z "$line" ] && line="{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the K2 daemon (status ${status:-none})\"}""#,
        ),
        (&lib, r#"        400|413) exit 2 ;;"#),
        (&lib, r#"        404) exit "${_CP_404_EXIT:-3}" ;;"#),
        (&lib, r#"        *) exit 3 ;;"#),
        (
            &lib,
            r#"    print("daemon_error: " + raw[:200]); sys.exit(0)"#,
        ),
        (
            &lib,
            r#"            --connect-timeout 5 --max-time 60 -w $'\n%{http_code}' 2>/dev/null || printf '\n000'"#,
        ),
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "the shell no longer has {shell:?}"
        );
    }
    if k2_client::brand::CLI_NAME == "k2" {
        let line = format!("            local cusage=\"{}\"", cancel::USAGE);
        assert!(
            cmd.lines().any(|l| l == line),
            "$cusage is no longer {:?}",
            cancel::USAGE
        );
    }
}

/// The readers' Python, at the lines `line` names.
#[test]
fn the_readers_lines() {
    let many = python_after("compute-command.sh", "local fb=0");
    use cancel::line::many as m;
    assert_eq!(at(&many, m::LOAD), "v = json.load(sys.stdin)");
    assert_eq!(at(&many, m::C), r#"c = v.get("cancelled") or []"#);
    assert!(at(&many, m::PRINT).starts_with(r#"print("cancelled %d of %d matching job(s)%s" % (len(c), v.get("matched", 0), (": " + " ".join(x["job"] for x in c)) if c else ""))"#));
    assert_eq!(at(&many, m::FOR), r#"for s in v.get("skipped") or []:"#);
    assert!(at(&many, m::SKIPPED).starts_with(
        r#"print("  skipped %s: %s (%s)" % (s.get("job"), s.get("code"), s.get("hint")))"#
    ));

    let kept = python_after(
        "compute-command.sh",
        "grep -Eq '\"code\":\"(job_started|already_ended)\"'",
    );
    use cancel::line::kept as k;
    assert_eq!(
        at(&kept, k::LOAD),
        r#"e = (json.load(sys.stdin).get("error") or {})"#
    );
    assert_eq!(
        at(&kept, k::CODE),
        r#"if e.get("code") == "already_ended":"#
    );

    let one = python_after("compute-command.sh", "printf '%s' \"$_CP_BODY\" | K2_CP_JOB=\"$cjob\" python3 -c '\nimport json, os, sys\nv = json.load");
    use cancel::line::one as o;
    assert_eq!(at(&one, o::LOAD), "v = json.load(sys.stdin)");
    assert_eq!(at(&one, o::NODE), r#"node = v.get("node") or "?""#);
    assert_eq!(at(&one, o::INT), "s = int(s)");
    assert_eq!(
        at(&one, o::STATE),
        r#"if v.get("state") == "cancelled" and v.get("was") == "queued":"#
    );
    assert!(at(&one, o::STOPPING).contains("took(v.get(\"ranSecs\"))"));
}

fn ask(f: impl FnOnce(&mut Ask)) -> Words<Ask> {
    let mut a = Ask::default();
    f(&mut a);
    Words::Ask(a)
}

fn usage(why: &str) -> Words<Ask> {
    Words::Usage(if why.is_empty() {
        cancel::USAGE.to_string()
    } else {
        format!("{} ({why})", cancel::USAGE)
    })
}

#[test]
fn cancel_reads_the_words_as_the_loop_does() {
    let rows: Vec<(&[&str], Words<Ask>)> = vec![
        (&["j1"], ask(|a| a.job = "j1".into())),
        (
            &["--force", "j1"],
            ask(|a| {
                a.job = "j1".into();
                a.force = true
            }),
        ),
        (
            &["j1", "-f", "--json"],
            ask(|a| {
                a.job = "j1".into();
                a.force = true
            }),
        ),
        (
            &["j1", "--if-queued"],
            ask(|a| {
                a.job = "j1".into();
                a.if_queued = true
            }),
        ),
        (
            &["j1", "--if-queued", "--force"],
            usage("--if-queued or --force, not both"),
        ),
        (&["j1", "j2"], usage("one job")),
        (&["", "j2"], ask(|a| a.job = "j2".into())),
        (&[""], usage("")),
        (&[], usage("")),
        (&["j1", "--bogus"], usage("unknown flag '--bogus'")),
        (&["-"], usage("unknown flag '-'")),
        (&["j1", "--tag"], Words::ShiftFailed),
        (&["--node", "--bogus"], usage("")),
        (&["--bogus", "--tag"], usage("unknown flag '--bogus'")),
        (&["--all-mine"], ask(|a| a.scope = Some("session"))),
        (
            &[
                "--workspace-all",
                "--tag",
                "t",
                "--state",
                "any",
                "--node",
                "n",
                "--workspace",
                "w",
            ],
            ask(|a| {
                a.scope = Some("workspace");
                a.tag = "t".into();
                a.state = "any".into();
                a.node = "n".into();
                a.workspace = "w".into();
            }),
        ),
        (
            &["--all-mine", "j1"],
            usage("a job id or --all-mine, not both"),
        ),
        (
            &["--all-mine", "--state", "done"],
            usage("--state is queued, running or any"),
        ),
        (
            &["--all-mine", "--state", ""],
            ask(|a| a.scope = Some("session")),
        ),
        (&["--all-mine", "--state"], Words::ShiftFailed),
    ];
    for (words, want) in rows {
        assert_eq!(cancel::parse(words), want, "{words:?}");
    }
}

#[test]
fn the_bodies_are_the_shell_s() {
    assert_eq!(
        body_of(&[("job", "abc"), ("ifState", "queued")]),
        r#"{"job": "abc", "ifState": "queued"}"#
    );
    assert_eq!(
        body_of(&[("job", "é\"x"), ("ifState", "")]),
        r#"{"job": "é\"x"}"#
    );
    let mut a = Ask::default();
    assert_eq!(
        cancel::filter_body(&a, "session", ""),
        r#"{"filter": {"scope": "session"}, "force": false}"#
    );
    a.tag = "t".into();
    a.state = "any".into();
    a.node = "n".into();
    a.force = true;
    assert_eq!(
        cancel::filter_body(&a, "workspace", "/w x"),
        r#"{"filter": {"scope": "workspace", "tag": "t", "state": "any", "node": "n", "workspace": "/w x"}, "force": true}"#
    );
}

fn shown(s: &Shown) -> (String, String, i32) {
    (s.stdout.clone(), s.stderr.clone(), s.exit.code())
}

#[test]
fn one_job_s_lines() {
    let rows = [
        (
            r#"{"state":"cancelled","was":"queued","node":"mini"}"#,
            "abcdefghij",
            "cancelled abcdefgh (was queued on mini)\n",
        ),
        (
            r#"{"state":"cancelled","was":"queued"}"#,
            "j",
            "cancelled j (was queued on ?)\n",
        ),
        (
            r#"{"state":"cancel_requested","onNode":true,"node":"n","cancel":"sigterm","note":"x"}"#,
            "j",
            "cancel asked for j (queued on n; sigterm): x\n",
        ),
        (
            r#"{"state":"cancel_requested","onNode":1,"node":"n"}"#,
            "j",
            "cancel asked for j (queued on n; None): \n",
        ),
        (
            r#"{"state":"running","was":"running","node":"n","ranSecs":125,"cancel":"sigterm"}"#,
            "j",
            "stopping j (was running on n for 2m05s; cancel sigterm)\n",
        ),
        (
            r#"{"state":"stopping","cancel":true,"ranSecs":null}"#,
            "j",
            "stopping j (was stopping on ?; cancel True)\n",
        ),
        (
            r#"{"cancel":"x","ranSecs":-5}"#,
            "j",
            "stopping j (was None on ? for -1m55s; cancel x)\n",
        ),
        (
            r#"{"cancel":"x","ranSecs":" 61 "}"#,
            "j",
            "stopping j (was None on ? for 1m01s; cancel x)\n",
        ),
        (
            r#"{"cancel":"x","ranSecs":59.9}"#,
            "j",
            "stopping j (was None on ? for 0m59s; cancel x)\n",
        ),
        (r#"{"state":"done"}"#, "j", "done\n"),
        (r#"{"state":null}"#, "j", "None\n"),
        (r#"{}"#, "j", "ok\n"),
    ];
    for (body, job, want) in rows {
        assert_eq!(
            shown(&cancel::one_reader(body.as_bytes(), job)),
            (want.to_string(), String::new(), 0),
            "{body}"
        );
    }
    let s = cancel::one_reader(br#"[1]"#, "j");
    assert_eq!(s.exit, Exit::Error);
    assert!(
        s.stderr.ends_with(
            "line 5, in <module>\nAttributeError: 'list' object has no attribute 'get'\n"
        ),
        "{}",
        s.stderr
    );
    let s = cancel::one_reader(br#"{"cancel":"x","ranSecs":"1.5"}"#, "j");
    assert_eq!(
        s.stderr,
        "Traceback (most recent call last):\n  File \"<string>\", line 15, in <module>\n  File \"<string>\", line 8, in took\nValueError: invalid literal for int() with base 10: '1.5'\n"
    );
    let s = cancel::one_reader(b"", "j");
    assert!(s
        .stderr
        .ends_with("json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n"));
}

#[test]
fn many_jobs_lines() {
    let rows = [
        (
            r#"{"cancelled":[{"job":"a"},{"job":"b"}],"matched":3,"skipped":[{"job":"c","code":"job_started","hint":"h"}]}"#,
            "cancelled 2 of 3 matching job(s): a b\n  skipped c: job_started (h)\n",
        ),
        (r#"{}"#, "cancelled 0 of 0 matching job(s)\n"),
        (
            r#"{"cancelled":null,"matched":1.5,"skipped":[{}]}"#,
            "cancelled 0 of 1 matching job(s)\n  skipped None: None (None)\n",
        ),
    ];
    for (body, want) in rows {
        assert_eq!(
            shown(&cancel::many_reader(body.as_bytes())),
            (want.to_string(), String::new(), 0),
            "{body}"
        );
    }
    let s = cancel::many_reader(br#"{"cancelled":[{"job":1}]}"#);
    assert!(
        s.stderr.ends_with(
            "line 5, in <module>\nTypeError: sequence item 0: expected str instance, int found\n"
        ),
        "{}",
        s.stderr
    );
    let s = cancel::many_reader(br#"{"cancelled":[{"x":1}]}"#);
    assert!(
        s.stderr
            .ends_with("line 5, in <genexpr>\nKeyError: 'job'\n"),
        "{}",
        s.stderr
    );
    let s = cancel::many_reader(br#"{"cancelled":7}"#);
    assert!(
        s.stderr
            .ends_with("TypeError: object of type 'int' has no len()\n"),
        "{}",
        s.stderr
    );
    let s = cancel::many_reader(br#"{"matched":"x"}"#);
    assert!(
        s.stderr
            .ends_with("TypeError: %d format: a real number is required, not str\n"),
        "{}",
        s.stderr
    );
    let s = cancel::many_reader(br#"{"skipped":[1]}"#);
    assert_eq!(s.stdout, "cancelled 0 of 0 matching job(s)\n");
    assert!(
        s.stderr.ends_with(
            "line 7, in <module>\nAttributeError: 'int' object has no attribute 'get'\n"
        ),
        "{}",
        s.stderr
    );
}

#[test]
fn if_queued_lines() {
    assert!(cancel::names_started_or_ended(
        br#"{"error":{"code":"job_started"}}"#
    ));
    assert!(cancel::names_started_or_ended(
        br#"x"code":"already_ended"y"#
    ));
    assert!(!cancel::names_started_or_ended(
        br#"{"error":{"code": "job_started"}}"#
    ));
    assert_eq!(
        shown(&cancel::kept_reader(
            br#"{"error":{"code":"already_ended","hint":"done 2m ago"}}"#,
            "abcdefghij"
        )),
        (
            "not cancelled: abcdefgh already ended (done 2m ago)\n".into(),
            String::new(),
            0
        )
    );
    assert_eq!(
        shown(&cancel::kept_reader(br#"{"error":{"code":"job_started"}}"#, "j")),
        (format!("not cancelled: j already started; left running ({} compute cancel j --force stops it)\n", k2_client::brand::CLI_NAME), String::new(), 0)
    );
    let s = cancel::kept_reader(br#"{"error":"job_started"}"#, "j");
    assert!(
        s.stderr.ends_with(
            "line 5, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"
        ),
        "{}",
        s.stderr
    );
}

#[test]
fn fail_lines() {
    let rows: [(&str, &[u8], &str); 9] = [
        (
            "409",
            br#"{"error":{"code":"job_started","hint":"use --force"}}"#,
            r#"{"error": "job_started", "hint": "use --force"}"#,
        ),
        (
            "403",
            br#"{"error":{"code":"x"}}"#,
            r#"{"error": "x", "hint": ""}"#,
        ),
        (
            "403",
            br#"{"error":"forbidden","message":"no"}"#,
            r#"{"error": "forbidden", "hint": "no"}"#,
        ),
        (
            "403",
            br#"{"error":"forbidden"}"#,
            r#"{"error": "error", "hint": "forbidden"}"#,
        ),
        (
            "500",
            br#"{"x":1}"#,
            r#"{"error": "error", "hint": "None"}"#,
        ),
        (
            "500",
            "[1, \"é\"]".as_bytes(),
            r#"{"error": "error", "hint": "None"}"#,
        ),
        ("502", b"oops\n\n", "daemon_error: oops"),
        ("000", b"\n000", "daemon_error: \n000"),
        (
            "503",
            b"",
            concat!(
                r#"{"error":"daemon_unreachable","hint":"no answer from the "#,
                k2_client::product_name!(),
                r#" daemon (status 503)"}"#
            ),
        ),
    ];
    for (status, body, want) in rows {
        assert_eq!(fail_line(status, body), want, "{status} {body:?}");
    }
}

#[test]
fn python_s_int_and_divmod() {
    for (v, want) in [
        ("0", "0"),
        ("-0", "0"),
        (" 1_000 ", "1000"),
        ("+7", "7"),
        ("007", "7"),
    ] {
        assert_eq!(int_of(&str_value(v)).as_deref(), Ok(want), "{v:?}");
    }
    for bad in ["", "1.5", "_1", "1_", "1__0", "x", "- 1"] {
        assert!(int_of(&str_value(bad)).is_err(), "{bad:?}");
    }
    for (n, q, r) in [
        ("0", "0", 0),
        ("59", "0", 59),
        ("60", "1", 0),
        ("125", "2", 5),
        ("-5", "-1", 55),
        ("-60", "-1", 0),
        ("-61", "-2", 59),
        (
            "123456789012345678901234567890",
            "2057613150205761315020576131",
            30,
        ),
    ] {
        assert_eq!(divmod(n, 60), (q.to_string(), r), "{n}");
    }
}

#[test]
fn the_shell_s_own_settings_go_to_the_shell() {
    let v = |pairs: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    };
    assert!(!shell_settings(v(&[("HOME", "/h"), ("_CP_404_EXIT", "")])));
    assert!(shell_settings(v(&[("_CP_404_EXIT", "4")])));
    assert!(shell_settings(v(&[("K2_CJ_extra", "")])));
    assert!(shell_settings(v(&[("_K2_REFUSAL_AREA", "app")])));
    assert!(shell_settings(v(&[("_K2_NOANSWER_SOFT", "1")])));
}

#[test]
fn k2_cp_max_time_as_curl_takes_it() {
    let t = |s: &str| post_max_time(Some(OsString::from(s)));
    assert_eq!(post_max_time(None), Some(None));
    assert_eq!(t(""), Some(None));
    assert_eq!(t("30"), Some(Some(Duration::from_secs(30))));
    assert_eq!(t("0.5"), Some(Some(Duration::from_millis(500))));
    assert_eq!(t("2."), Some(Some(Duration::from_secs(2))));
    for shell in ["0", "0.0", "x", "-1", "1e3", " 5", "."] {
        assert_eq!(t(shell), None, "{shell:?}");
    }
}

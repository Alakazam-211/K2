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

mod wait_verb {
    use super::super::wait::{self, Ask};
    use super::super::Words;
    use super::{at, python_after, source};
    use crate::exit::Exit;

    #[test]
    fn the_shell_s_lines() {
        let jobs = source("compute-jobs.sh");
        for shell in [
            r#"            --failures) failures=1; shift ;;"#,
            r#"            --quiet|-q) quiet=1; shift ;;"#,
            r#"            --json) as_json=1; shift ;;"#,
            r#"            --follow|-f) follow=1; shift ;;"#,
            r#"            --max-wait) max_wait="${2:-}"; shift 2 ;;"#,
            r#"            --any) any=1; shift ;;"#,
            r#"            w=20"#,
            r#"            resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$j")&wait=$w")""#,
            r#"                [ "$fails" -eq 1 ] && echo "[k2 compute] lost the K2 server; retrying (the job keeps running on the node)" >&2"#,
            r#"                if [ "$fails" -gt 60 ]; then"#,
            r#"                sleep 10"#,
            r#"                _CP_404_EXIT=4 _compute_fail "$_CP_STATUS" "$_CP_BODY""#,
            r#"                echo "[k2 compute] $line" >&2"#,
            r#"            resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$j")&wait=1")""#,
            r#"            [ "${#others[@]}" -gt 0 ] && [ "$quiet" = "0" ] && echo "[k2 compute] still running: ${others[*]} (k2 compute wait ${others[*]} --any)" >&2"#,
            r#"            echo "{\"error\":\"still_running\",\"jobs\":\"$*\",\"hint\":\"none ended yet: k2 compute wait $* --any\"}" >&2"#,
            r#"        [ "$quiet" = "0" ] && [ -z "$last" ] && { echo "[k2 compute] waiting for the first of $# job(s) to end" >&2; last=1; }"#,
            r#"    exit "${rc:-3}""#,
        ] {
            assert!(
                jobs.lines().any(|l| l == shell),
                "the shell no longer has {shell:?}"
            );
        }
        if k2_client::brand::CLI_NAME == "k2" {
            let line = format!(r#"            -*) echo "{}" >&2; exit 2 ;;"#, wait::USAGE);
            assert!(jobs.lines().any(|l| l == line), "the usage moved");
        }
    }

    #[test]
    fn the_readers_lines() {
        let info = python_after("compute-jobs.sh", "info=\"$(python3 -c '");
        use wait::line::info as i;
        assert_eq!(at(&info, i::LOAD), "j = json.load(open(sys.argv[1]))");
        assert_eq!(at(&info, i::Q), r#"q = j.get("queue") or {}"#);
        assert!(at(&info, i::KEY).starts_with("key = "));
        assert!(at(&info, i::ELSEWHERE).starts_with("elsewhere = "));
        let end = python_after(
            "compute-jobs.sh",
            "out=\"$(K2_CP_JSON=\"$as_json\" K2_CP_N=",
        );
        use wait::line::end as e;
        assert_eq!(at(&end, e::FOR), "for j in bodies:");
        assert!(at(&end, e::TOOK).starts_with("took = "));
        assert_eq!(at(&end, e::RL), r#"if rl.get("secs"):"#);
        assert!(at(&end, e::LATE).starts_with("took += "));
        assert!(at(&end, e::EXIT).starts_with(r#"code, what = int(j["exitCode"])"#));
        assert!(at(&end, e::SIGNAL).starts_with("code, what = 128 + int("));
        assert!(at(&end, e::ST).starts_with("code, what = 3, st + "));
        assert!(at(&end, e::STILL).contains("still_running"));
        assert!(at(&end, e::RC).starts_with(r#"print("RC %d %s""#));
        let any = python_after(
            "compute-jobs.sh",
            "out=\"$(printf '%s' \"$_CP_BODY\" | K2_CP_JSON=\"$as_json\" python3 -c '",
        );
        use wait::line::any as a;
        assert_eq!(at(&any, a::LOAD), "j = json.load(sys.stdin)");
        assert_eq!(at(&any, a::ST), r#"st = j.get("state")"#);
        assert!(at(&any, a::TOOK).starts_with("took = "));
        assert!(at(&any, a::EXIT).starts_with(r#"code, what = int(j["exitCode"])"#));
        assert!(at(&any, a::SIGNAL).starts_with("code, what = 128 + int("));
        assert!(at(&any, a::ST_PLUS).starts_with("code, what = 3, st + "));
    }

    fn ask(f: impl FnOnce(&mut Ask)) -> Option<Words<Ask>> {
        let mut a = Ask::default();
        f(&mut a);
        Some(Words::Ask(a))
    }

    #[test]
    fn the_words() {
        let usage = || Some(Words::Usage(wait::USAGE.to_string()));
        let rows: Vec<(&[&str], Option<Words<Ask>>)> = vec![
            (&["a"], ask(|a| a.jobs = vec!["a".into()])),
            (
                &["a", "b", "-q", "--json"],
                ask(|a| {
                    a.jobs = vec!["a".into(), "b".into()];
                    a.quiet = true;
                    a.json = true;
                }),
            ),
            (
                &["a", "--any", "--max-wait", "1h30m"],
                ask(|a| {
                    a.jobs = vec!["a".into()];
                    a.any = true;
                    a.max_wait = Some(5400);
                }),
            ),
            (&["a", "--max-wait", ""], ask(|a| a.jobs = vec!["a".into()])),
            (&[], usage()),
            (&["--json"], usage()),
            (&["a", "--x"], usage()),
            (&["a", "--max-wait"], Some(Words::ShiftFailed)),
            (&["a", "--follow"], None),
            (&["a", "-f", "--x"], usage()),
            (&["a", "--failures"], None),
            (&["a", "--max-wait", "5x"], None),
            (&["--max-wait", "5x"], usage()),
        ];
        for (words, want) in rows {
            assert_eq!(wait::parse(words), want, "{words:?}");
        }
    }

    #[test]
    fn durations() {
        for (v, want) in [
            ("300", Some(300)),
            ("007", Some(7)),
            ("2h", Some(7200)),
            ("90m", Some(5400)),
            ("1h30m", Some(5400)),
            ("1s1s", Some(2)),
            ("", None),
            ("h", None),
            ("1h5", None),
            ("1d", None),
            ("-1", None),
        ] {
            assert_eq!(wait::secs(v), want, "{v:?}");
        }
    }

    #[test]
    fn progress_lines() {
        let rows = [
            (
                r#"{"state":"running","short":"ab12","node":"mini"}"#,
                "0\trunning|None|None\tab12 running on mini\n",
            ),
            (
                r#"{"state":"done","short":"ab12","node":"mini"}"#,
                "1\tdone|None|None\tab12 done on mini\n",
            ),
            (r#"{"short":"ab12"}"#, "0\t?|None|None\tab12 ? on None\n"),
            (
                r#"{"state":"queued","short":"s","queue":{"line":"queued 2nd","position":2,"waitReason":"busy"},"wait":{"elsewhere":"x can start it"}}"#,
                "0\tqueued|2|busy\ts queued 2nd · x can start it\n",
            ),
            (
                r#"{"state":"queued","short":"s","wait":{"words":"waits for a slot"}}"#,
                "0\tqueued|None|None\ts queued: waits for a slot\n",
            ),
            (
                r#"{"state":"queued","short":"s","node":"n","queue":{"position":1,"of":3,"reason":"cpu"}}"#,
                "0\tqueued|1|cpu\ts queued on n: position 1 of 3 (cpu)\n",
            ),
        ];
        for (body, want) in rows {
            assert_eq!(
                wait::info(body.as_bytes()).map_err(|r| r.text()),
                Ok(want.to_string()),
                "{body}"
            );
        }
        assert_eq!(
            wait::split_info("0\ta|b\tline\twith tab\n\n"),
            ("0".into(), "a|b".into(), "line\twith tab".into())
        );
        let r = wait::info(br#"{"queue":[1]}"#).unwrap_err();
        assert_eq!(
            r.exception,
            "AttributeError: 'list' object has no attribute 'get'"
        );
        let r = wait::info(br#"{"wait":{"elsewhere":1}}"#).unwrap_err();
        assert_eq!(
            r.exception,
            "TypeError: can only concatenate str (not \"int\") to str"
        );
    }

    #[test]
    fn the_end() {
        let s = wait::end_of(
            &[br#"{"state":"done","exitCode":0,"short":"a","node":"n","startedAt":100,"endedAt":225}"#.to_vec()],
            false,
        );
        assert_eq!((s.stdout.as_str(), s.exit), ("", Exit::Ok));
        let s = wait::end_of(
            &[
                br#"{"state":"done","exitCode":2,"short":"a","id":"aaaa"}"#.to_vec(),
                br#"{"state":"running","short":"b"}"#.to_vec(),
            ],
            true,
        );
        assert_eq!(s.exit, Exit::Refused, "still running wins");
        assert_eq!(
            s.stdout,
            "[{\"state\": \"done\", \"exitCode\": 2, \"short\": \"a\", \"id\": \"aaaa\"}, {\"state\": \"running\", \"short\": \"b\"}]\n"
        );
        let s = wait::end_of(&[br#"{"state":"done","signal":9}"#.to_vec()], false);
        assert_eq!(s.exit.code(), 137);
        let s = wait::end_of(&[br#"{"state":"done","exitCode":300}"#.to_vec()], false);
        assert_eq!(s.exit.code(), 44);
        let s = wait::end_of(&[br#"{"state":"failed","reason":"oom"}"#.to_vec()], false);
        assert_eq!(s.exit, Exit::Refused);
        let s = wait::end_of(&[br#"{"state":"done","exitCode":"x"}"#.to_vec()], false);
        assert_eq!(s.exit, Exit::Error);
        assert!(s
            .stderr
            .ends_with("ValueError: invalid literal for int() with base 10: 'x'\n"));
    }

    #[test]
    fn exit_codes_wrap_as_the_shell_s() {
        for (d, want) in [
            ("0", 0),
            ("3", 3),
            ("256", 0),
            ("300", 44),
            ("-1", 255),
            ("137", 137),
        ] {
            assert_eq!(wait::exit_of(d).code(), want, "{d}");
        }
    }

    #[test]
    fn the_first_to_end() {
        assert_eq!(
            wait::any_reader(br#"{"state":"running"}"#, false).map_err(|r| r.text()),
            Ok(None)
        );
        let e = wait::any_reader(br#"{"state":"done","exitCode":1,"id":"x"}"#, true)
            .unwrap()
            .unwrap();
        assert_eq!(e.rc, "1");
        assert_eq!(
            e.json.as_deref(),
            Some(r#"{"state": "done", "exitCode": 1, "id": "x"}"#)
        );
        let r = wait::any_reader(b"[]", false).unwrap_err();
        assert_eq!(
            r.exception,
            "AttributeError: 'list' object has no attribute 'get'"
        );
    }
}

mod fetch_verb {
    use super::super::fetch::{self, Ask};
    use super::super::Words;
    use super::{at, python_after};

    #[test]
    fn the_readers_lines() {
        use fetch::line::{list as l, step as s};
        let list = python_after("compute-jobs.sh", "list=\"$(printf");
        assert_eq!(at(&list, l::LOAD), "v = json.load(sys.stdin)");
        assert!(at(&list, l::HAVE).starts_with("have = {a[\"name\"]: a for a in"));
        assert!(at(&list, l::MISSING).contains("artifact_not_found"));
        assert_eq!(at(&list, l::FOR), "for n in (want or sorted(have)):");
        let step = python_after("compute-jobs.sh", "step=\"$(printf");
        assert_eq!(at(&step, s::PENDING), r#"if v.get("pending"):"#);
        assert!(at(&step, s::MB).contains("MB on the server so far"));
        assert_eq!(at(&step, s::NAME), r#"name = v["name"]"#);
        assert!(at(&step, s::MAKEDIRS).starts_with("os.makedirs("));
        assert_eq!(at(&step, s::DATA), r#"data = base64.b64decode(v["data"])"#);
        assert!(at(&step, s::PRINT).starts_with(r#"print("%d %d %s %s %s""#));
        if k2_client::brand::CLI_NAME == "k2" {
            let text = super::source("compute-jobs.sh");
            let line = format!("    local fusage=\"{}\"", fetch::USAGE);
            assert!(text.lines().any(|x| x == line), "$fusage moved");
        }
    }

    #[test]
    fn the_words() {
        let usage = || Words::Usage(fetch::USAGE.to_string());
        let a = |f: &dyn Fn(&mut Ask)| {
            let mut a = Ask::default();
            f(&mut a);
            Words::Ask(a)
        };
        let rows: Vec<(&[&str], Words<Ask>)> = vec![
            (&["j"], a(&|x| x.job = "j".into())),
            (
                &[
                    "j",
                    "a",
                    "b",
                    "-o",
                    "d",
                    "--json",
                    "--over-relay",
                    "--attempt",
                    "02",
                ],
                a(&|x| {
                    x.job = "j".into();
                    x.names = vec!["a".into(), "b".into()];
                    x.out = "d".into();
                    x.json = true;
                    x.over_relay = true;
                    x.attempt = "02".into();
                }),
            ),
            (&[], usage()),
            (&["j", "--out", ""], usage()),
            (&["j", "--out"], Words::ShiftFailed),
            (&["j", "--attempt"], Words::ShiftFailed),
            (
                &["j", "--attempt", "0"],
                Words::Usage(format!(
                    "{}  (--attempt N: an attempt number from 1)",
                    fetch::USAGE
                )),
            ),
            (&["j", "--bogus"], usage()),
        ];
        for (w, want) in rows {
            assert_eq!(fetch::parse(w), want, "{w:?}");
        }
    }

    #[test]
    fn base64_as_python_decodes_it() {
        assert_eq!(fetch::b64decode("aGVsbG8="), Ok(b"hello".to_vec()));
        assert_eq!(fetch::b64decode("aGVs\nbG8="), Ok(b"hello".to_vec()));
        assert_eq!(fetch::b64decode(""), Ok(Vec::new()));
        assert_eq!(fetch::b64decode("aGk="), Ok(b"hi".to_vec()));
        assert_eq!(fetch::b64decode("aA=="), Ok(b"h".to_vec()));
        assert_eq!(
            fetch::b64decode("aGVsbG8").unwrap_err(),
            "binascii.Error: Incorrect padding"
        );
        assert!(fetch::b64decode("a")
            .unwrap_err()
            .contains("cannot be 1 more than a multiple of 4"));
    }

    #[test]
    fn the_list() {
        let names = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let body =
            br#"{"short":"j1","artifacts":[{"name":"d/a.txt"},{"name":"b.txt"},{"name":"b.txt"}]}"#;
        assert_eq!(
            fetch::list_reader(body, &[]).map_err(|r| r.text()),
            Ok(Ok("b.txt\nd/a.txt\n".into()))
        );
        assert_eq!(
            fetch::list_reader(body, &names(&["d/a.txt", "d/a.txt"])).map_err(|r| r.text()),
            Ok(Ok("d/a.txt\nd/a.txt\n".into()))
        );
        assert_eq!(
            fetch::list_reader(body, &names(&["x"])).map_err(|r| r.text()),
            Ok(Err(
                r#"{"error": "artifact_not_found", "hint": "job j1 kept: b.txt, d/a.txt"}"#.into()
            ))
        );
        assert_eq!(
            fetch::list_reader(b"{}", &names(&["x"])).map_err(|r| r.text()),
            Ok(Err(
                r#"{"error": "artifact_not_found", "hint": "job None kept: (nothing)"}"#.into()
            ))
        );
        let r = fetch::list_reader(br#"{"artifacts":[{"x":1}]}"#, &[]).unwrap_err();
        assert_eq!(r.exception, "KeyError: 'name'");
    }

    #[test]
    fn a_folder_mkdir_can_make() {
        let d = std::env::temp_dir().join(format!("k5-fetch-creatable-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("afile");
        std::fs::write(&file, "x").unwrap();
        assert!(fetch::creatable(&d.to_string_lossy()));
        assert!(fetch::creatable(&d.join("new/deeper").to_string_lossy()));
        assert!(!fetch::creatable(&file.to_string_lossy()));
        assert!(!fetch::creatable(&file.join("under").to_string_lossy()));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

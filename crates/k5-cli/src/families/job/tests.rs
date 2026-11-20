use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/job.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python that starts on the line after `start` (which ends in
/// `python3 -c '`), its lines as `python3 -c` numbers them: line 1 is the
/// empty one after `-c '`.
fn python_after(start: &str) -> Vec<String> {
    let text = source();
    let at = text.find(start).unwrap_or_else(|| panic!("no {start:?}"));
    let body = &text[at + start.len()..];
    let end = body.find("\n'").expect("its end");
    std::iter::once(String::new())
        .chain(body[..end].split('\n').map(str::to_string))
        .collect()
}

fn words(list: &[&str]) -> Parsed {
    parse(list)
}

fn ask(list: &[&str]) -> Ask {
    match parse(list) {
        Parsed::Call(a) => a,
        other => panic!("{list:?}: {other:?}"),
    }
}

/// Each line the parse, the checks and the calls follow, as the shell
/// has them.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"        ""|help|--help|-h) _job_usage; return 0 ;;"#,
        r#"        [ "$a" = "--" ] && break"#,
        r#"        case "$a" in -h|--help) _job_usage; return 0 ;; esac"#,
        r#"            --) shift; have_cmd=1; argv=("$@"); break ;;"#,
        r#"            --json) as_json=1; shift ;;"#,
        r#"            --all) all=1; shift ;;"#,
        r#"            --every|--cron|--timeout|--cwd|--workspace|--run)"#,
        r#"                [ -n "${2:-}" ] || { echo "usage: $1 needs a value (k2 job --help)" >&2; exit 2; }"#,
        r#"            --*) echo "usage: unknown flag '$1' (k2 job --help)" >&2; exit 2 ;;"#,
        r#"            *) pos+=("$1"); shift ;;"#,
        r#"    if [ -z "$ws" ] && [ -z "${SCOPED_TOKEN:-}" ]; then"#,
        r#"        ws="$PROJECT""#,
        r#"    local name="${pos[0]:-}""#,
        r#"        list|ls|pause|resume) need_name=0 ;;"#,
        r#"        echo "usage: k2 job $sub NAME (k2 job --help)" >&2"#,
        r#"            [ "$have_cmd" = 1 ] && [ "${#argv[@]}" -gt 0 ] || { echo "usage: k2 job add NAME --every 15m -- <cmd> [args…]" >&2; exit 2; }"#,
        r#"            _job_call POST add "$body""#,
        r#"            _job_call POST edit "$body" ;;"#,
        r#"            _job_call GET list "$q""#,
        r#"            _job_call GET show "$q""#,
        r#"            _job_call GET logs "$q""#,
        r#"        run|run-now)"#,
        r#"            _job_call POST run "$body" ;;"#,
        r#"        enable|disable|kill)"#,
        r#"            _job_call POST "$sub" "$body" ;;"#,
        r#"        rm|remove|delete)"#,
        r#"            _job_call POST remove "$body" ;;"#,
        r#"        pause|resume)"#,
        r#"                echo "usage: k2 job $sub NAME | k2 job $sub --all" >&2"#,
        r#"        *) echo "usage: unknown k2 job verb '$sub' (k2 job --help)" >&2; exit 2 ;;"#,
        r#"    _sidecar_split "$(_sidecar_send "$method" "/cli/jobs/${route}" "$payload")""#,
        r#"            echo "daemon_unreachable: the K2 daemon did not answer" >&2"#,
        r#"        exit "$(_job_exit_for "$_SC_STATUS")""#,
        r#"        400|413) echo 2 ;;"#,
        r#"        401|403) echo 3 ;;"#,
        r#"        404) echo 4 ;;"#,
        r#"            if [ "$as_json" = 1 ]; then printf '%s\n' "$_SC_BODY"; else _job_render_list; fi"#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "cmd_job no longer has {shell:?}"
        );
    }
    let policy = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh"),
    )
    .unwrap();
    assert!(policy.contains("        job|jobs) echo \"jobs\" ;;\n"));
    assert!(policy.contains(
        "        *) for _a in \"${@:2}\"; do [ \"$_a\" = \"--\" ] && break; case \"$_a\" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;\n"
    ));
}

/// The lines [`line`] names are the ones that raise in the shell.
#[test]
fn the_python_lines() {
    let fail = python_after("        fi\n        printf '%s' \"$_SC_BODY\" | python3 -c '");
    assert_eq!(fail[3], "raw = sys.stdin.read()");
    assert!(fail[5].contains(r#"json.loads(raw).get("error") or {}"#));
    assert!(fail[8].contains(r#"raw[:300] or "the daemon did not answer""#));
    let l = python_after("_job_render_list() {\n    printf '%s' \"$_SC_BODY\" | python3 -c '");
    for (at, has) in [
        (line::LOAD, "v = json.load(sys.stdin)"),
        (line::L_JOBS, r#"jobs = v.get("jobs") or []"#),
        (line::L_PAUSED, r#"(v.get("pausedBy") or "?")"#),
        (line::L_FOR, "for j in jobs:"),
        (
            line::L_STATE,
            r#""running" if j.get("running") else ("on" if j.get("enabled") else "off")"#,
        ),
        (line::L_LAST, r#"last = j.get("lastRun") or {}"#),
        (line::L_LR, r#"(last.get("startedAt") or "")[5:16]"#),
        (line::L_NAME, r#"name = j.get("name", "")"#),
        (
            line::L_ALL,
            r#"len({x.get("workspace") for x in jobs}) > 1"#,
        ),
        (line::L_CMD, r#"cmd = j.get("command", "")"#),
        (line::L_LEN, "if len(cmd) > 60:"),
        (line::L_CUT, r#"cmd = cmd[:57] + "...""#),
        (
            line::L_ROW,
            r#"(name, j.get("schedule", ""), state, lr, cmd)"#,
        ),
    ] {
        assert!(
            l[at as usize].contains(has),
            "list line {at}: {:?}",
            l[at as usize]
        );
    }
    let sh = python_after("_job_render_show() {\n    printf '%s' \"$_SC_BODY\" | python3 -c '");
    for (at, has) in [
        (line::S_JOB, r#"j = json.load(sys.stdin).get("job") or {}"#),
        (
            line::S_STATE,
            r#""running (run %s)" % j["running"] if j.get("running")"#,
        ),
        (
            line::S_CHANGED,
            r#"j.get("updatedAt") != j.get("createdAt")"#,
        ),
        (line::S_GRANTS, r#"g = j.get("grants") or []"#),
        (
            line::S_SECRETS,
            r#"(" as $" + x["envName"]) if x.get("envName") else "") for x in g)"#,
        ),
        (line::S_RUNS, r#"runs = j.get("runs") or []"#),
        (line::S_FOR, "    for r in runs:"),
        (line::S_EC, r#"ec = r.get("exitCode")"#),
    ] {
        assert!(
            sh[at as usize].contains(has),
            "show line {at}: {:?}",
            sh[at as usize]
        );
    }
    let lg =
        python_after("            else\n                printf '%s' \"$_SC_BODY\" | python3 -c '");
    for (at, has) in [
        (line::LOAD, "v = json.load(sys.stdin)"),
        (line::G_RUN, r#"r = v.get("run") or {}"#),
        (line::G_EC, r#"ec = r.get("exitCode")"#),
        (line::G_WRITE, r#"sys.stdout.write(v.get("log") or "")"#),
    ] {
        assert!(
            lg[at as usize].contains(has),
            "logs line {at}: {:?}",
            lg[at as usize]
        );
    }
    let d = python_after("    fi\n    printf '%s' \"$_SC_BODY\" | python3 -c '");
    for (at, has) in [
        (line::D_LOAD, "v = json.load(sys.stdin)"),
        (
            line::D_JOB,
            r#"j = v.get("job") if isinstance(v.get("job"), dict) else None"#,
        ),
        (line::D_ADD, r#"print("added %s: %s, runs %s""#),
        (line::D_EDIT, r#"print("changed %s: %s, runs %s""#),
    ] {
        assert!(
            d[at as usize].contains(has),
            "done line {at}: {:?}",
            d[at as usize]
        );
    }
}

#[test]
fn the_help_forms() {
    for list in [
        &[][..],
        &[""],
        &["help"],
        &["--help"],
        &["-h", "x"],
        &["list", "--help"],
        &["add", "x", "-h", "--", "y"],
        &["nosuch", "--every", "--help"],
    ] {
        assert_eq!(words(list), Parsed::Help, "{list:?}");
    }
    // After `--`, a help flag is the command's.
    let a = ask(&["add", "n", "--every", "1m", "--", "ls", "--help"]);
    assert_eq!(a.argv, Some(vec!["ls".to_string(), "--help".to_string()]));
    assert!(offline_verb(&["job", "add", "--", "-h"]));
    assert!(!offline_verb(&["job", "add", "--", "-hx"]));
}

#[test]
fn the_words() {
    let a = ask(&[
        "add",
        "-x",
        "n2",
        "--json",
        "--all",
        "--every",
        "15m",
        "--cron",
        "* * *",
        "--timeout",
        "10m",
        "--cwd",
        "d",
        "--workspace",
        "w",
        "--run",
        "r1",
        "--",
        "a",
        "b",
    ]);
    assert_eq!(a.name, "-x");
    assert!(a.json && a.all && a.have_cwd);
    assert_eq!(
        (
            a.every.as_str(),
            a.cron.as_str(),
            a.timeout.as_str(),
            a.cwd.as_str()
        ),
        ("15m", "* * *", "10m", "d")
    );
    assert_eq!((a.workspace.as_str(), a.run.as_str()), ("w", "r1"));
    assert_eq!(
        words(&["add", "--every"]),
        Parsed::Early(needs_value("--every"))
    );
    assert_eq!(
        words(&["add", "--cwd", ""]),
        Parsed::Early(needs_value("--cwd"))
    );
    assert_eq!(
        words(&["list", "--jsonx"]),
        Parsed::Early(unknown_flag("--jsonx"))
    );
    assert_eq!(
        words(&["list", "--json=1"]),
        Parsed::Early(unknown_flag("--json=1"))
    );
    // The value is taken as it is, a flag too.
    assert_eq!(ask(&["show", "--run", "--json"]).run, "--json");
    assert_eq!(ask(&["add", "n", "--"]).argv, Some(Vec::new()));
}

#[test]
fn the_checks() {
    assert_eq!(check(&ask(&["show"])), Err(needs_name("show")));
    assert_eq!(check(&ask(&["nosuch"])), Err(needs_name("nosuch")));
    assert_eq!(check(&ask(&["nosuch", "x"])), Err(unknown_verb("nosuch")));
    assert_eq!(check(&ask(&["add", "x"])), Err(ADD_USAGE.to_string()));
    assert_eq!(check(&ask(&["add", "x", "--"])), Err(ADD_USAGE.to_string()));
    assert_eq!(check(&ask(&["pause"])), Err(pause_usage("pause")));
    assert_eq!(check(&ask(&["resume"])), Err(pause_usage("resume")));
    assert_eq!(check(&ask(&["pause", "--all"])), Ok(Verb::Post("pause")));
    assert_eq!(check(&ask(&["ls"])), Ok(Verb::List));
    assert_eq!(check(&ask(&["log", "x"])), Ok(Verb::Logs));
    assert_eq!(check(&ask(&["run-now", "x"])), Ok(Verb::Post("run")));
    assert_eq!(check(&ask(&["delete", "x"])), Ok(Verb::Post("remove")));
    assert_eq!(check(&ask(&["kill", "x"])), Ok(Verb::Post("kill")));
}

/// Python 3's `json.dumps` and `urllib.parse.urlencode` wrote these.
#[test]
fn the_body_and_the_query() {
    let a = ask(&[
        "add",
        "n\"x",
        "--every",
        "15m",
        "--cwd",
        "sub dir",
        "--all",
        "--",
        "sh",
        "-c",
        "echo é ☃",
    ]);
    assert_eq!(
        a.body("/w é"),
        r#"{"workspace": "/w \u00e9", "name": "n\"x", "every": "15m", "cwd": "sub dir", "all": true, "argv": ["sh", "-c", "echo \u00e9 \u2603"]}"#
    );
    let q = ask(&["list", "a b/c", "--all"]);
    assert_eq!(
        q.query("/w é&x+y"),
        "workspace=%2Fw+%C3%A9%26x%2By&name=a+b%2Fc&all=1"
    );
    assert_eq!(ask(&["list"]).query(""), "");
    assert_eq!(ask(&["list"]).body(""), "{}");
    let w = ask(&["list", "--workspace", "w"]);
    assert_eq!(w.workspace_for("/p", false), "w");
    assert_eq!(ask(&["list"]).workspace_for("/p", false), "/p");
    assert_eq!(ask(&["list"]).workspace_for("/p", true), "");
}

#[test]
fn a_refusal_s_line() {
    assert_eq!(
        fail_line(br#"{"error":{"code":"owner_only","hint":"no."}}"#),
        "owner_only: no.\n"
    );
    assert_eq!(fail_line(br#"{"error":{"code":null}}"#), "None: \n");
    assert_eq!(fail_line(br#"{"error":"boom"}"#), "boom\n");
    assert_eq!(fail_line(br#"{"error":[1,"a"]}"#), "[1, 'a']\n");
    assert_eq!(fail_line(br#"{"error":0}"#), "error: \n");
    assert_eq!(fail_line(b"[1]"), "[1]\n");
    assert_eq!(fail_line(b"oops\r\nx\r"), "oops\r\nx\r\n");
    assert_eq!(fail_line(b""), "the daemon did not answer\n");
    let long = "é".repeat(400);
    assert_eq!(fail_line(long.as_bytes()), format!("{}\n", "é".repeat(300)));
    assert_eq!(exit_for("401"), Exit::Refused);
    assert_eq!(exit_for("413"), Exit::Usage);
    assert_eq!(exit_for("404"), Exit::NotFound);
    assert_eq!(exit_for("500"), Exit::Error);
}

/// The reader's output, the command's name read back as `k2` (the
/// brand's name is in it where the shell's Python says `k2`).
fn shown(verb: Verb, sub: &str, body: &str) -> Shown {
    let mut r = render(verb, sub, body.as_bytes());
    let ours = format!("{} ", k2_client::brand::CLI_NAME);
    for text in [&mut r.stdout, &mut r.stderr] {
        *text = text
            .replace(&format!("({ours}"), "(k2 ")
            .replace(&format!("with {ours}"), "with k2 ");
    }
    r
}

/// Under any brand, the lines name the command by the brand's name.
#[test]
fn the_lines_say_the_brand() {
    let cli = k2_client::brand::CLI_NAME;
    for line in [
        ADD_USAGE.to_string(),
        needs_value("--every"),
        unknown_flag("--x"),
        needs_name("show"),
        pause_usage("pause"),
        unknown_verb("x"),
    ] {
        assert!(line.contains(&format!("{cli} job")), "{line}");
        if cli != "k2" {
            assert!(!line.contains("k2"), "{line}");
        }
    }
    let r = render(Verb::List, "list", br#"{"jobs":[]}"#);
    assert!(
        r.stdout.contains(&format!("({cli} job add")),
        "{}",
        r.stdout
    );
}

/// What the shell's Python printed for these bodies (Python 3.12).
#[test]
fn the_readers() {
    let list = r#"{"paused":true,"pausedBy":null,"jobs":[
        {"workspace":"a","name":"backup","schedule":"every 15m","enabled":true,
         "lastRun":{"outcome":"ok","startedAt":"2026-11-19T10:15:00Z"},"command":"sh -c 'tar czf /tmp/x.tgz .'"},
        {"workspace":"b","name":"n","schedule":"*/5 * * * *","running":1,"command":"x"}]}"#;
    assert_eq!(
        shown(Verb::List, "list", list).stdout,
        concat!(
            "ALL JOBS PAUSED by ? (k2 job resume --all)\n",
            "NAME                     SCHEDULE               STATE    LAST RUN                     COMMAND\n",
            "a/backup                 every 15m              on       ok 11-19T10:15               sh -c 'tar czf /tmp/x.tgz .'\n",
            "b/n                      */5 * * * *            running  -                            x\n",
        )
    );
    assert_eq!(
        shown(Verb::List, "ls", r#"{"jobs":[]}"#).stdout,
        "no jobs (k2 job add NAME --every 15m -- <cmd>)\n"
    );
    let long = format!(
        r#"{{"jobs":[{{"name":"x","command":"{}"}}]}}"#,
        "c".repeat(61)
    );
    assert!(shown(Verb::List, "list", &long)
        .stdout
        .ends_with(&format!(" {}...\n", "c".repeat(57))));
    let r = shown(
        Verb::List,
        "list",
        r#"{"jobs":[{"workspace":[1]},{"workspace":2}]}"#,
    );
    assert_eq!(r.exit, Exit::Error);
    assert!(
        r.stderr.ends_with("TypeError: unhashable type: 'list'\n"),
        "{}",
        r.stderr
    );
    let r = shown(Verb::List, "list", r#"{"jobs":[{"command":null}]}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: object of type 'NoneType' has no len()\n"));
    let r = shown(Verb::List, "list", "nope");
    assert!(r.stderr.contains("json.decoder.JSONDecodeError: "));

    let show = r#"{"job":{"name":"n","workspace":"w","schedule":"every 1h","running":"r9","command":"ls","cwd":"","timeoutSecs":600,
        "createdBy":"me","createdAt":"t1","updatedBy":"you","updatedAt":"t2",
        "grants":[{"secret":"K","delivery":"env","envName":"K_V"},{"secret":"F","delivery":"file"}],
        "runs":[{"id":"r9","outcome":"running","trigger":"schedule","exitCode":null,"startedAt":"t3"},
                {"id":"r8","outcome":"ok","trigger":"manual","exitCode":0,"startedAt":"t0"}]}}"#;
    assert_eq!(
        shown(Verb::Show, "show", show).stdout,
        concat!(
            "job:        n (workspace w)\n",
            "schedule:   every 1h\n",
            "state:      running (run r9)\n",
            "next:       -\n",
            "command:    ls\n",
            "folder:     (the workspace folder)\n",
            "timeout:    600s\n",
            "made by:    me at t1\n",
            "changed by: you at t2\n",
            "secrets:    K (env as $K_V), F (file)\n",
            "\n",
            "RUN                                    OUTCOME          TRIGGER    EXIT   STARTED\n",
            "r9                                     running          schedule   -      t3\n",
            "r8                                     ok               manual     0      t0\n",
        )
    );
    assert_eq!(
        shown(Verb::Show, "show", r#"{}"#).stdout,
        concat!(
            "job:        None (workspace None)\n",
            "schedule:   None\n",
            "state:      off\n",
            "next:       -\n",
            "command:    None\n",
            "folder:     (the workspace folder)\n",
            "timeout:    Nones\n",
            "made by:    None at None\n",
            "secrets:    none granted (k2 vault grant KEY --to job:None)\n",
        )
    );
    let r = shown(Verb::Show, "show", r#"{"job":{"grants":[{"envName":5}]}}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: can only concatenate str (not \"int\") to str\n"));
    assert!(r.stderr.contains("line 16, in <genexpr>"), "{}", r.stderr);

    assert_eq!(
        shown(Verb::Logs, "logs", r#"{"run":{"id":"r1","startedAt":"t","outcome":"ok","exitCode":3,"bytes":12},"truncated":true,"log":"a\nb"}"#).stdout,
        "# run r1  t  ok  exit 3  12 bytes  (tail)\na\nb"
    );
    let r = shown(Verb::Logs, "log", r#"{"log":5}"#);
    assert_eq!(r.stdout, "# run None  None  None  exit -  None bytes\n");
    assert!(r
        .stderr
        .ends_with("TypeError: write() argument must be str, not int\n"));

    for (sub, body, want) in [
        ("add", r#"{"job":{"name":"n","schedule":"s","command":"c","next":"t"}}"#,
         "added n: s, runs c\nnext run: t. Grant it secrets with k2 vault grant KEY --to job:n.\n"),
        ("edit", r#"{"job":{"name":"n","schedule":"s","command":"c"}}"#, "changed n: s, runs c\n"),
        ("run-now", r#"{"job":"n","run":"r"}"#, "started n, run r (k2 job logs n --run r)\n"),
        ("kill", r#"{"job":"n","run":"r"}"#, "stopping n, run r\n"),
        ("delete", r#"{"removed":"n","grantsRevoked":2}"#, "removed n (2 secret grant(s) removed)\n"),
        ("enable", r#"{"job":{"name":"n","enabled":true}}"#, "n is on\n"),
        ("pause", r#"{"paused":false}"#, "every job is running again\n"),
        ("resume", r#"{"ok":true}"#, "ok\n"),
    ] {
        assert_eq!(shown(Verb::Post("x"), sub, body).stdout, want, "{sub}");
    }
    let r = shown(Verb::Add, "add", r#"{"job":"n"}"#);
    assert!(r.stderr.contains("line 7, in <module>"));
    assert!(r
        .stderr
        .ends_with("AttributeError: 'NoneType' object has no attribute 'get'\n"));
    let r = shown(Verb::Post("run"), "run", "[1]");
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
}

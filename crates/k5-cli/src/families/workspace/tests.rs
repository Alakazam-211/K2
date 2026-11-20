use super::*;
use crate::families::Shown;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
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

fn has(body: &str, line: &str, what: &str) {
    assert!(
        body.lines().any(|l| l == line),
        "{what} no longer has {line:?}"
    );
}

/// A single-quoted `python3 -c '…'` after `after` in `file`, its lines as
/// `python3 -c` numbers them (line 1 is the text right after `-c '`).
fn single_quoted(file: &str, after: &str) -> Vec<String> {
    let text = source(file);
    let body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"))
        .1;
    let start = body.find("python3 -c '").expect("a python3 -c '") + "python3 -c '".len();
    let py = &body[start..];
    let end = py.find('\'').expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

#[test]
fn the_router_s_arms() {
    let router = source("router.sh");
    for (verb, call) in [
        ("list", "cmd_workspace_list"),
        ("launch", "cmd_workspace_launch"),
        ("profile", "cmd_workspace_profile"),
        ("update", "cmd_workspace_update"),
        ("create", "cmd_workspace_create"),
        ("open", "cmd_workspace_open"),
    ] {
        let arm = format!("            {verb})");
        let ends = format!(" shift 2; {call} \"$@\" ;;");
        assert!(
            router
                .lines()
                .any(|l| l.starts_with(&arm) && l.ends_with(&ends)),
            "the workspace case has no {verb} arm calling {call}"
        );
        assert!(VERBS.contains(&verb));
    }
    // Help anywhere in the words goes to the help, never a verb.
    assert!(router.contains("    workspace)\n"));
    assert!(router.contains("        if _wants_help \"${@:2}\"; then\n"));
}

#[test]
fn the_forms() {
    assert_eq!(
        parse(&["list", "x", "--json", "--running"]),
        Some(Form::List(list::Ask {
            running: true,
            json: true
        }))
    );
    assert_eq!(parse(&["profile"]), Some(Form::Profile(None)));
    assert_eq!(
        parse(&["launch", "--json", "x"]),
        Some(Form::Launch(Ok("x".into())))
    );
    assert_eq!(parse(&["launch"]), Some(Form::Launch(Ok(String::new()))));
    assert!(matches!(
        parse(&["update", "--field"]),
        Some(Form::Update(Err(Stop {
            exit: Exit::Usage,
            ..
        })))
    ));
    assert_eq!(parse(&["remove", "x"]), None);
    assert_eq!(parse(&[]), None);
}
#[test]
fn list_s_lines() {
    let body = function("agent-context.sh", "cmd_workspace_list");
    for line in [
        r#"            --running) running="true"; shift ;;"#,
        r#"            --json) as_json="true"; shift ;;"#,
        "            *) shift ;;",
        r#"        _ws_request cli_request "/cli/agent/list""#,
        r#"        printf '%s' "$_WS_ANSWER" | RUNNING_ONLY="$running" python3 -c '"#,
        r#"    _ws_request cli_request "/cli/projects/list""#,
        r#"    printf '%s' "$_WS_ANSWER" | RUNNING_ONLY="$running" python3 -c ""#,
        r#"        _ws_list_exit "$rc""#,
        r#"    _ws_list_exit "$rc""#,
    ] {
        has(&body, line, "cmd_workspace_list");
    }
    assert!(body.contains(&format!("cli_request \"{}\"", list::AGENTS)));
    assert!(body.contains(&format!("cli_request \"{}\"", list::PROJECTS)));
    let exit = function("agent-context.sh", "_ws_list_exit");
    for line in [
        r#"    [ "$1" -eq 0 ] && return 0"#,
        r#"    _permission_in_body "$_WS_ANSWER" && exit 3"#,
        r#"    exit "$1""#,
    ] {
        has(&exit, line, "_ws_list_exit");
    }
}

/// `_ws_request` and what it sends with: the cell socket first for any
/// route, the lines for a failed request, and the verbs that send the
/// session's passport (or nothing) in a session.
#[test]
fn ws_request_s_lines() {
    let body = function("agent-context.sh", "_ws_request");
    let p = |l: &str| l.replace(PRODUCT_NAME, "K2");
    let line = |e: Error| p(&failed_line(&e, "${endpoint}", "${BASE_URL}"));
    let broken = Error::Broken {
        endpoint: "x".into(),
        why: String::new(),
        curl: 56,
    };
    for want in [
        r#"    _WS_ANSWER="$(_K2_UDS_TRY=1 cli_send "$fn" "$endpoint" "$@")" || rc=$?"#.to_string(),
        r#"    [ "$rc" -eq 0 ] && return 0"#.to_string(),
        format!(
            r#"        7) echo "{}" >&2 ;;"#,
            line(Error::Unreachable {
                endpoint: "x".into(),
                err: std::io::Error::other("refused")
            })
        ),
        format!(
            r#"        "$_CLI_LOCAL_TRANSPORT_DENIED") echo "{}" >&2 ;;"#,
            line(Error::Denied {
                endpoint: "x".into(),
                code: LOCAL_TRANSPORT_DENIED
            })
        ),
        format!(
            r#"        *) echo "{}" >&2 ;;"#,
            line(broken).replace("curl exit 56", "curl exit ${rc}")
        ),
        "    exit 1".to_string(),
    ] {
        has(&body, &want, "_ws_request");
    }
    let cell = source("cell-socket.sh");
    assert!(cell.contains("    [ \"${_K2_UDS_TRY:-0}\" = 1 ] && return 0\n"));
    let policy = function("cli-tools-policy.sh", "_cli_verb_sends_session_passport");
    has(
        &policy,
        r#"        "workspace list"|"workspace create"|"workspace open"|"workspace launch"|"workspace profile"|"workspace update"|"workspace triage") return 0 ;;"#,
        "_cli_verb_sends_session_passport",
    );
    let apply = function("cli-tools-policy.sh", "_cli_apply_auth_for_verb");
    for want in [
        r#"        if [ -n "$hook_sock" ] && _cli_verb_sends_session_passport "$verb" "${2:-}"; then"#,
        r#"            TOKEN="${SCOPED_TOKEN:-}""#,
    ] {
        has(&apply, want, "_cli_apply_auth_for_verb");
    }
    assert!(source("cli-tools-policy.sh")
        .contains("_k2_cfg_auth() {\n    [ -n \"${1:-}\" ] || return 0\n"));
}
#[test]
fn list_s_python() {
    let py = super::super::shell_python(
        "agent-context.sh",
        "_ws_request cli_request \"/cli/projects/list\"",
    );
    let at = |n: u32| py[n as usize - 1].trim().to_string();
    for (n, text) in [
        (3, "raw = sys.stdin.read()"),
        (5, "rows = json.loads(raw)"),
        (
            7,
            r"sys.stderr.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])",
        ),
        (9, "if isinstance(rows, dict):"),
        (10, "e = rows.get('error', raw[:200])"),
        (11, "if isinstance(e, dict):"),
        (12, "e = e.get('hint') or e.get('code') or raw[:200]"),
        (13, r"sys.stderr.write('Error: %s\n' % e)"),
        (15, "if os.environ.get('RUNNING_ONLY') == 'true':"),
        (
            list::line::RUNNING,
            "rows = [r for r in rows if r.get('agentEnabled')]",
        ),
        (17, "if not rows:"),
        (18, &format!("print('{}')", list::NO_AGENTS)),
        (20, "if not rows:"),
        (21, &format!("print('{}')", list::NONE)),
        (list::line::FOR, "for r in rows:"),
        (list::line::NAME, "name = r.get('name') or '?'"),
        (25, "mode = r.get('agentMode') or 'off'"),
        (26, "path = r.get('path', '')"),
        (list::line::ROW, "print(f'  {name:<24} [{mode:<8}] {path}')"),
    ] {
        assert_eq!(at(n), text, "line {n}");
    }
    assert_eq!(py.len(), 27, "the Python grew");
    assert!(list::UNPARSEABLE.ends_with(": "));

    let j = single_quoted(
        "agent-context.sh",
        "_ws_request cli_request \"/cli/agent/list\"",
    );
    let at = |n: u32| j[n as usize - 1].trim().to_string();
    for (n, text) in [
        (3, "raw = sys.stdin.read()"),
        (5, "d = json.loads(raw)"),
        (
            7,
            r#"print(json.dumps({"error": {"code": "bad_response", "hint": "daemon returned an unparseable response: %r" % raw[:200]}}), file=sys.stderr)"#,
        ),
        (
            9,
            r#"if not isinstance(d, dict) or d.get("error") or not d.get("ok"):"#,
        ),
        (
            10,
            r#"err = d.get("error") if isinstance(d, dict) else None"#,
        ),
        (11, "if not isinstance(err, dict):"),
        (
            12,
            r#"err = {"code": "error", "hint": str(err or raw[:200])}"#,
        ),
        (13, r#"print(json.dumps({"error": err}), file=sys.stderr)"#),
        (15, r#"rows = d.get("agents") or []"#),
        (16, r#"if os.environ.get("RUNNING_ONLY") == "true":"#),
        (
            list::line::J_RUNNING,
            r#"rows = [r for r in rows if r.get("enabled")]"#,
        ),
        (18, r#"print(json.dumps({"ok": True, "workspaces": rows}))"#),
    ] {
        assert_eq!(at(n), text, "--json line {n}");
    }
    assert_eq!(j.len(), 19, "the --json Python grew");
}
#[test]
fn the_table() {
    let rows = r#"[{"name":"acme","agentMode":"agent","path":"/srv/acme ws","agentEnabled":true},
        {"name":"","agentMode":null,"path":null},{"name":true,"agentMode":1.5},
        {"name":"a-very-long-workspace-name-past-24","path":"/x"}]"#;
    assert_eq!(
        list::text(rows.as_bytes(), false).stdout,
        concat!(
            "  acme                     [agent   ] /srv/acme ws\n",
            "  ?                        [off     ] None\n",
            "  1                        [1.5     ] \n",
            "  a-very-long-workspace-name-past-24 [off     ] /x\n",
        )
    );
    let running = list::text(rows.as_bytes(), true);
    assert_eq!(
        running.stdout,
        "  acme                     [agent   ] /srv/acme ws\n"
    );
    assert_eq!(
        list::text(b"[]", false),
        Shown::out(format!("{}\n", list::NONE))
    );
    assert_eq!(
        list::text(b"[{}]", true),
        Shown::out(format!("{}\n", list::NO_AGENTS))
    );
    assert_eq!(
        list::text(b"{\"x\":1}", false),
        Shown::err("Error: {\"x\":1}\n", Exit::Error)
    );
    assert_eq!(
        list::text(b"{\"error\":{\"code\":\"no\"}}", false).stderr,
        "Error: no\n"
    );
    assert_eq!(
        list::text(b"{\"error\":{\"hint\":\"h\",\"code\":\"no\"}}", false).stderr,
        "Error: h\n"
    );
    assert_eq!(
        list::text(b"{\"error\":{\"hint\":\"\"}}", false).stderr,
        "Error: {\"error\":{\"hint\":\"\"}}\n"
    );
    assert_eq!(
        list::text(b"{\"error\":null}", false).stderr,
        "Error: None\n"
    );
    assert_eq!(
        list::text(b"", false),
        Shown::err(format!("{}''\n", list::UNPARSEABLE), Exit::Error)
    );
    let raised = list::text(br#"[{"name":"a"},{"name":[1]}]"#, false);
    assert_eq!(raised.exit, Exit::Error);
    assert_eq!(raised.stdout, "  a                        [off     ] \n");
    assert!(raised.stderr.ends_with(
        "line 27, in <module>\nTypeError: unsupported format string passed to list.__format__\n"
    ));
    let raised = list::text(b"\"ab\"", true);
    assert!(raised
        .stderr
        .ends_with("line 16, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
    let raised = list::text(b"5", false);
    assert!(raised
        .stderr
        .ends_with("line 23, in <module>\nTypeError: 'int' object is not iterable\n"));
}

#[test]
fn the_json_form() {
    let d = r#"{"ok":true,"agents":[{"name":"é","enabled":true},{"name":"b","enabled":false}]}"#;
    assert_eq!(
        list::json_form(d.as_bytes(), false).stdout,
        "{\"ok\": true, \"workspaces\": [{\"name\": \"\\u00e9\", \"enabled\": true}, {\"name\": \"b\", \"enabled\": false}]}\n"
    );
    assert_eq!(
        list::json_form(d.as_bytes(), true).stdout,
        "{\"ok\": true, \"workspaces\": [{\"name\": \"\\u00e9\", \"enabled\": true}]}\n"
    );
    assert_eq!(
        list::json_form(br#"{"ok":true,"agents":null}"#, true).stdout,
        "{\"ok\": true, \"workspaces\": []}\n"
    );
    assert_eq!(
        list::json_form(br#"{"error":"down"}"#, false),
        Shown::err(
            "{\"error\": {\"code\": \"error\", \"hint\": \"down\"}}\n",
            Exit::Error
        )
    );
    assert_eq!(
        list::json_form(br#"{"ok":false,"error":{"code":"x","hint":"y"}}"#, false).stderr,
        "{\"error\": {\"code\": \"x\", \"hint\": \"y\"}}\n"
    );
    assert_eq!(
        list::json_form(b"[1]", false).stderr,
        "{\"error\": {\"code\": \"error\", \"hint\": \"[1]\"}}\n"
    );
    assert_eq!(
        list::json_form(b"nope", false).stderr,
        "{\"error\": {\"code\": \"bad_response\", \"hint\": \"daemon returned an unparseable response: 'nope'\"}}\n"
    );
    let raised = list::json_form(br#"{"ok":1,"agents":"ab"}"#, true);
    assert!(raised
        .stderr
        .ends_with("line 17, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
}

#[test]
fn register_s_lines() {
    for verb in [register::Verb::Create, register::Verb::Open] {
        let body = function("wiki.sh", &format!("cmd_workspace_{}", verb.word()));
        let usage = verb.usage();
        for line in [
            "            --no-wiki) no_wiki=true; shift ;;".to_string(),
            "            --no-agents-md) no_agents_md=true; shift ;;".to_string(),
            "            --fanout) fanout=true; shift ;;".to_string(),
            format!(r#"            -*) echo "Unknown flag: $1" >&2; echo "{usage}" >&2; exit 1 ;;"#),
            r#"            *) if [ -z "$target_path" ]; then target_path="$1"; else echo "Unexpected extra argument: $1" >&2; exit 1; fi; shift ;;"#.to_string(),
            format!(r#"        echo "{usage}" >&2"#),
            r#"    [ "$no_wiki" = true ] && extra+=("no_wiki=1")"#.to_string(),
            r#"    [ "$no_agents_md" = true ] && extra+=("no_agents_md=1")"#.to_string(),
            r#"    [ "$fanout" = true ] && extra+=("fanout=1")"#.to_string(),
            format!(
                r#"        _ws_request cli_request_post "{}" "path=$(urlencode "$target_path")" "${{extra[@]}}""#,
                verb.endpoint()
            ),
            format!(
                r#"        _ws_request cli_request_post "{}" "path=$(urlencode "$target_path")""#,
                verb.endpoint()
            ),
            format!(
                r#"    cli_print_or_fail "$_WS_ANSWER" {}"#,
                verb.endpoint()
            ),
        ] {
            has(&body, &line, verb.word());
        }
    }
    has(
        &function("wiki.sh", "cmd_workspace_create"),
        r#"    target_path="$(cli_abs_path "$target_path")""#,
        "create",
    );
    let open = function("wiki.sh", "cmd_workspace_open");
    has(&open, r#"    if [ -d "$target_path" ]; then"#, "open");
    has(
        &open,
        r#"        target_path="$(cd "$target_path" && pwd)""#,
        "open",
    );
    let abs = function("cli-tools-policy.sh", "cli_abs_path");
    for line in [
        r#"    [ -n "$p" ] || return 0"#,
        r#"    case "$p" in /*) ;; *) p="$PWD/$p" ;; esac"#,
        r#"    while [ "$p" != "/" ] && [ "${p%/}" != "$p" ]; do p="${p%/}"; done"#,
        r#"    while [ ! -d "$head" ]; do"#,
        r#"        tail="/$(basename "$head")$tail""#,
        r#"        head="$(dirname "$head")""#,
        r#"    base="$(cd "$head" 2>/dev/null && pwd)" || base="$head""#,
        r#"    [ "$base" = "/" ] && base="""#,
        r#"    printf '%s%s\n' "${base}" "${tail}""#,
    ] {
        has(&abs, line, "cli_abs_path");
    }
}
#[test]
fn print_or_fail_s_lines() {
    let body = function("wiki.sh", "cli_print_or_fail");
    let py = single_quoted("wiki.sh", "cli_print_or_fail() {");
    let want = [
        "import json,os,sys",
        "raw = sys.stdin.read()",
        "of = os.environ.get(\"K2_ANSWER_OF\") or \"\"",
        "try:",
        "    d = json.loads(raw)",
        "except Exception:",
        "    if of and not raw.strip():",
        "        print(\"K2\\x27s server gave an empty answer to %s\" % of)",
        "    elif of:",
        "        print(\"K2\\x27s server answered %s with something that is not JSON: %r\" % (of, raw[:200]))",
        "    sys.exit(0)",
        "if not isinstance(d, dict):",
        "    if of:",
        "        print(\"K2\\x27s server answered %s with JSON that is not an object: %s\" % (of, raw[:200]))",
        "    sys.exit(0)",
        "e = d.get(\"error\")",
        "if not e:",
        "    sys.exit(0)",
        "if isinstance(e, dict):",
        "    print(e.get(\"hint\") or e.get(\"code\") or \"daemon refused\")",
        "elif e in (\"folder_trust_pending\", \"folder_trust_denied\") and d.get(\"message\"):",
        "    print(\"%s: %s\" % (e, d[\"message\"]))",
        "else:",
        "    print(e)",
    ];
    assert_eq!(py, want);
    // The texts the binary prints for them.
    let p = |v: Verdict| match v {
        Verdict::Error(e) => e.replace(PRODUCT_NAME, "K2"),
        Verdict::Print => panic!("no error"),
    };
    assert_eq!(
        p(verdict(b"", "%s")),
        "K2's server gave an empty answer to %s"
    );
    assert_eq!(
        p(verdict(b"x", "%s")),
        "K2's server answered %s with something that is not JSON: 'x'"
    );
    assert_eq!(
        p(verdict(b"[]", "%s")),
        "K2's server answered %s with JSON that is not an object: []"
    );
    for line in [
        r#"    local resp="$1" endpoint="${2:-}""#,
        r#"    local err rc=0"#,
        r#"    err=$(printf '%s' "$resp" | K2_ANSWER_OF="$endpoint" python3 -c 'import json,os,sys"#,
        r#"    print(e)' 2>/dev/null) || rc=$?"#,
        r#"    if [ "$rc" -ne 0 ] && [ -z "$err" ]; then"#,
        r#"        err="could not read K2's server answer${endpoint:+ to $endpoint}""#,
        r#"    if [ -n "$err" ]; then"#,
        r#"        echo "Error: $err" >&2"#,
        r#"        if _permission_in_body "$resp"; then"#,
        r#"            exit 3"#,
        r#"        exit 1"#,
        r#"    printf '%s\n' "$resp""#,
    ] {
        has(&body, line, "cli_print_or_fail");
    }
}
#[test]
fn the_words_of_create_and_open() {
    use register::{parse as p, Ask, Verb};
    let usage = |v: Verb| Stop {
        lines: vec![v.usage()],
        exit: Exit::Error,
    };
    assert_eq!(p(Verb::Create, &[]), Err(usage(Verb::Create)));
    assert_eq!(p(Verb::Open, &[""]), Err(usage(Verb::Open)));
    assert_eq!(
        p(Verb::Create, &["", "x", "--fanout", "--no-wiki"]),
        Ok(Ask {
            target: "x".into(),
            no_wiki: true,
            no_agents_md: false,
            fanout: true
        })
    );
    assert_eq!(
        p(Verb::Create, &["x", "y"]),
        Err(Stop {
            lines: vec!["Unexpected extra argument: y".into()],
            exit: Exit::Error
        })
    );
    assert_eq!(
        p(Verb::Open, &["x", "-"]),
        Err(Stop {
            lines: vec!["Unknown flag: -".into(), Verb::Open.usage()],
            exit: Exit::Error
        })
    );
}

fn scratch(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("k5-ws-{tag}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    // The folder as the system names it (macOS's temp folder is a link).
    std::fs::canonicalize(&dir)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[cfg(unix)]
#[test]
fn paths_as_bash_resolves_them() {
    use register::{abs_path, canonpath, path, Verb};
    let d = scratch("paths");
    std::fs::create_dir_all(format!("{d}/a/b")).unwrap();
    std::fs::write(format!("{d}/file"), "").unwrap();
    std::os::unix::fs::symlink(format!("{d}/a/b"), format!("{d}/link")).unwrap();
    // Missing parents stay as typed; the folder that exists is `cd`'s.
    assert_eq!(
        abs_path(&format!("{d}/a/x/y"), "/nowhere"),
        Some(format!("{d}/a/x/y"))
    );
    assert_eq!(
        abs_path(&format!("{d}/a/b/"), "/"),
        Some(format!("{d}/a/b"))
    );
    assert_eq!(abs_path("a/new", &d), Some(format!("{d}/a/new")));
    assert_eq!(
        abs_path("b/../x", &format!("{d}/a")),
        Some(format!("{d}/a/x"))
    );
    assert_eq!(
        abs_path("../new/x", &format!("{d}/a/b")),
        Some(format!("{d}/a/new/x"))
    );
    assert_eq!(
        abs_path("./b/.", &format!("{d}/a")),
        Some(format!("{d}/a/b"))
    );
    // A link stays a link (a logical `cd`), and `..` after it is textual.
    assert_eq!(abs_path("link/new", &d), Some(format!("{d}/link/new")));
    assert_eq!(abs_path("link/..", &d), Some(d.clone()));
    assert_eq!(
        abs_path(&format!("{d}/file/x"), "/"),
        Some(format!("{d}/file/x"))
    );
    assert_eq!(abs_path("/", "/x"), Some(String::new()));
    assert_eq!(abs_path("///", "/x"), Some(String::new()));
    assert_eq!(
        abs_path("/nonexistent-k5-root/ai", "/"),
        Some("/nonexistent-k5-root/ai".into())
    );
    assert_eq!(abs_path("", "/x"), Some(String::new()));
    // What the binary leaves to the shell.
    assert_eq!(abs_path(&format!("{d}//a"), "/"), None);
    assert_eq!(abs_path("x", "/"), None, "`$PWD/x` with PWD=/ is //x");
    assert_eq!(abs_path("a\nb", &d), None);
    // `cd file/..`: `file` is no folder, bash's check fails.
    assert_eq!(canonpath(&format!("{d}/file/..")), None);
    assert_eq!(canonpath("/.."), Some("/".into()));
    assert_eq!(canonpath(&format!("{d}/a//b/")), Some(format!("{d}/a/b")));
    // `open`: a folder as `cd` names it, anything else as typed.
    assert_eq!(
        path(Verb::Open, &format!("{d}/a/../a/b/"), "/x", false),
        Some(format!("{d}/a/b"))
    );
    assert_eq!(
        path(Verb::Open, "no such/dir", &d, false),
        Some("no such/dir".into())
    );
    assert_eq!(
        path(Verb::Open, &format!("{d}/file"), "/x", false),
        Some(format!("{d}/file"))
    );
    assert_eq!(
        path(Verb::Open, &format!("//{}", &d[1..]), "/x", false),
        None
    );
    assert_eq!(
        path(Verb::Create, &format!("{d}/a"), "/x", true),
        Some(format!("{d}/a"))
    );
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn print_or_fail_s_verdicts() {
    let v = |resp: &[u8]| match verdict(resp, "/x") {
        Verdict::Error(e) => Verdict::Error(e.replace(PRODUCT_NAME, "K2")),
        p => p,
    };
    let e = |t: &str| Verdict::Error(t.to_string());
    assert_eq!(v(b"{\"ok\":true}"), Verdict::Print);
    assert_eq!(
        v(b"not json"),
        e("K2's server answered /x with something that is not JSON: 'not json'")
    );
    assert_eq!(v(b""), e("K2's server gave an empty answer to /x"));
    assert_eq!(v(b" \t\x1c"), e("K2's server gave an empty answer to /x"));
    assert_eq!(
        v(b"[1,\n2]"),
        e("K2's server answered /x with JSON that is not an object: [1,\n2]")
    );
    let long = format!("\"{}\"", "a".repeat(300));
    assert_eq!(
        v(long.as_bytes()),
        e(&format!(
            "K2's server answered /x with JSON that is not an object: \"{}",
            "a".repeat(199)
        ))
    );
    assert_eq!(v(b"{\"error\":\"\"}"), Verdict::Print);
    assert_eq!(v(b"{\"error\":\"x\"}"), e("x"));
    assert_eq!(v(b"{\"error\":{\"hint\":\"\",\"code\":\"c\"}}"), e("c"));
    assert_eq!(v(b"{\"error\":{\"a\":1}}"), e("daemon refused"));
    assert_eq!(v(b"{\"error\":{\"hint\":[1]}}"), e("[1]"));
    assert_eq!(
        v(b"{\"error\":\"folder_trust_pending\",\"message\":\"ask\"}"),
        e("folder_trust_pending: ask")
    );
    assert_eq!(
        v(b"{\"error\":\"folder_trust_denied\",\"message\":\"\"}"),
        e("folder_trust_denied")
    );
    assert_eq!(v(b"{\"error\":\"x\",\"message\":\"m\"}"), e("x"));
    assert_eq!(v(b"{\"error\":\"\\n\\n\"}"), Verdict::Print);
    assert_eq!(v(b"{\"error\":\"a\\n\"}"), e("a"));
    assert_eq!(v(b"{\"error\":true}"), e("True"));
    // A string Python can't write out: the print raises.
    assert_eq!(
        v(b"{\"error\":\"\\ud800\"}"),
        e("could not read K2's server answer to /x")
    );
    assert_eq!(held(b"a\0b\n\n"), b"ab");
}
#[test]
fn agent_s_lines() {
    let k2 = |l: &str| l.replace(PRODUCT_NAME, "K2");
    let launch = function("agent-context.sh", "cmd_workspace_launch");
    for line in [
        r#"            --json) shift ;;"#.to_string(),
        r#"            --no-attach)"#.to_string(),
        format!(r#"                echo "{}" >&2"#, k2(agent::NO_ATTACH)),
        "                exit 2 ;;".to_string(),
        r#"                echo "Unknown flag: $1" >&2"#.to_string(),
        format!(r#"                echo "{}" >&2"#, agent::LAUNCH_USAGE),
        r#"                if [[ -n "$target" ]]; then"#.to_string(),
        r#"                    echo "Unexpected extra argument: $1" >&2"#.to_string(),
        format!(r#"                    echo "{}" >&2"#, agent::LAUNCH_USAGE),
        r#"                target="$1"; shift ;;"#.to_string(),
        r#"    [[ -n "$target" ]] || target="$PROJECT""#.to_string(),
        r#"    if [[ ! -d "$target" ]]; then"#.to_string(),
        r#"        _ws_request cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")""#.to_string(),
        r#"        resolved=$(printf '%s' "$_WS_ANSWER" | python3 -c 'import json,sys"#.to_string(),
        "try: d = json.load(sys.stdin)".to_string(),
        "except Exception: sys.exit(0)".to_string(),
        r#"p = d.get("path") if isinstance(d, dict) else None"#.to_string(),
        r#"if isinstance(p, str) and p.strip(): print(p)' 2>/dev/null) || true"#.to_string(),
        r#"        if [[ -z "$resolved" ]]; then"#.to_string(),
        r#"            cli_print_or_fail "$_WS_ANSWER" /cli/workspace/resolve >/dev/null"#.to_string(),
        format!(
            r#"            echo "{}" >&2"#,
            k2(&String::from_utf8(agent::no_folder_line("$target", b"", || false)).unwrap())
                .trim_end_matches(")\n")
                .to_string()
                + "${_WS_ANSWER:0:200})"
        ),
        "            exit 1".to_string(),
        r#"        target="$resolved""#.to_string(),
        r#"    PROJECT="$target" _ws_request cli_request_post "/cli/workspace/ensure-canonical-session""#.to_string(),
        r#"    cli_print_or_fail "$_WS_ANSWER" /cli/workspace/ensure-canonical-session"#.to_string(),
    ] {
        has(&launch, &line, "cmd_workspace_launch");
    }
    assert!(launch.contains(agent::RESOLVE) && launch.contains(agent::ENSURE));
    let profile = function("agent-context.sh", "cmd_workspace_profile");
    has(&profile, r#"    local target="${1:-$PROJECT}""#, "profile");
    has(
        &profile,
        r#"    PROJECT="$target" _ws_request cli_request "/cli/workspace/agent-display-name" "project_path=$(urlencode "$target")""#,
        "profile",
    );
    has(
        &profile,
        r#"    cli_print_or_fail "$_WS_ANSWER" /cli/workspace/agent-display-name"#,
        "profile",
    );
    assert_eq!(
        profile
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .count(),
        5,
        "cmd_workspace_profile grew"
    );
    let update = function("agent-context.sh", "cmd_workspace_update");
    for line in [
        format!(r#"    local usage="{}""#, agent::UPDATE_USAGE),
        "            --field|--value)".to_string(),
        r#"                if [[ $# -lt 2 ]]; then"#.to_string(),
        r#"                    echo "Error: $1 needs a value" >&2"#.to_string(),
        r#"                    echo "$usage" >&2"#.to_string(),
        "                    exit 2".to_string(),
        r#"                if [[ "$1" = --field ]]; then field="$2"; else value="$2"; fi"#
            .to_string(),
        r#"                echo "Error: unknown argument: $1" >&2"#.to_string(),
        r#"    if [[ -z "$field" || -z "$value" ]]; then"#.to_string(),
        r#"        echo "$usage" >&2"#.to_string(),
        "        exit 2".to_string(),
        r#"    _K2_PERMISSION_QUIET=1 _ws_request cli_request "/cli/workspace/agent-display-name""#
            .to_string(),
        r#"    agent_name=$(printf '%s' "$_WS_ANSWER" | python3 -c 'import json,sys"#.to_string(),
        r#"try: print(json.load(sys.stdin).get("display_name",""))"#.to_string(),
        r#"except: pass' 2>/dev/null)"#.to_string(),
        r#"        agent_name="$(basename "$PROJECT")""#.to_string(),
        r#"    _ws_request cli_request_post "/cli/agent/update" \"#.to_string(),
        r#"        "agent=$(urlencode "$agent_name")" \"#.to_string(),
        r#"        "field=$(urlencode "$field")" \"#.to_string(),
        r#"        "value=$(urlencode "$value")""#.to_string(),
        r#"    cli_print_or_fail "$_WS_ANSWER" /cli/agent/update"#.to_string(),
    ] {
        has(&update, &line, "cmd_workspace_update");
    }
    let columns = format!("        {})", agent::DB_COLUMNS.join("|"));
    has(&update, &columns, "cmd_workspace_update's DB columns");
    for l in agent::db_column_lines("$field") {
        has(
            &update,
            &format!(r#"            echo "{l}" >&2"#),
            "cmd_workspace_update",
        );
    }
}
#[test]
fn update_s_words() {
    use agent::{parse_update as p, Update, UPDATE_USAGE};
    let stop = |lines: &[&str]| Stop {
        lines: lines.iter().map(|l| l.to_string()).collect(),
        exit: Exit::Usage,
    };
    assert_eq!(p(&[]), Err(stop(&[UPDATE_USAGE])));
    assert_eq!(
        p(&["--field", "role", "--value", ""]),
        Err(stop(&[UPDATE_USAGE]))
    );
    assert_eq!(
        p(&["--field", "a", "--field", "role", "--value", "--lead"]),
        Ok(Update {
            field: "role".into(),
            value: "--lead".into()
        })
    );
    assert_eq!(
        p(&["--value", "v", "--field"]),
        Err(stop(&["Error: --field needs a value", UPDATE_USAGE]))
    );
    assert_eq!(
        p(&["--field", "role", "x", "--value", "v"]),
        Err(stop(&["Error: unknown argument: x", UPDATE_USAGE]))
    );
    assert_eq!(
        p(&["--field", "agent_mode", "--value", "x"]),
        Err(Stop {
            lines: agent::db_column_lines("agent_mode"),
            exit: Exit::Usage
        })
    );
}

#[test]
fn launch_s_words() {
    use agent::{parse_launch as p, LAUNCH_USAGE, NO_ATTACH};
    let stop = |lines: &[&str]| Stop {
        lines: lines.iter().map(|l| l.to_string()).collect(),
        exit: Exit::Usage,
    };
    assert_eq!(p(&[]), Ok(String::new()));
    assert_eq!(p(&["--json", "a", "--json"]), Ok("a".into()));
    assert_eq!(p(&["", "a"]), Ok("a".into()));
    assert_eq!(p(&["a", "--no-attach"]), Err(stop(&[NO_ATTACH])));
    assert_eq!(p(&["-"]), Err(stop(&["Unknown flag: -", LAUNCH_USAGE])));
    assert_eq!(
        p(&["a", "b"]),
        Err(stop(&["Unexpected extra argument: b", LAUNCH_USAGE]))
    );
}
#[test]
fn the_lookup_and_the_folder_name() {
    use agent::{basename, field, no_folder_line, resolved};
    assert_eq!(
        field(b"{\"display_name\":\"A b\\n\\n\"}", "display_name"),
        "A b"
    );
    assert_eq!(field(b"{\"path\":null}", "path"), "None");
    assert_eq!(
        field(b"{\"path\":{\"a\":[1,true]}}", "path"),
        "{'a': [1, True]}"
    );
    assert_eq!(field(b"{\"x\":1}", "path"), "");
    assert_eq!(field(b"[1]", "path"), "");
    assert_eq!(field(b"", "path"), "");
    assert_eq!(field(b"{\"path\":\"\\ud800\"}", "path"), "");
    assert_eq!(resolved(b"{\"path\":\"/srv/a b\\n\\n\"}"), "/srv/a b");
    assert_eq!(resolved(b"{\"path\":null}"), "");
    assert_eq!(resolved(b"{\"path\":\" \\t\"}"), "");
    assert_eq!(resolved(b"{\"path\":1}"), "");
    assert_eq!(resolved(b"[\"/x\"]"), "");
    assert_eq!(resolved(b"nope"), "");
    assert_eq!(resolved(b"{\"path\":\"\\u0000\"}"), "");
    assert_eq!(resolved(b"{\"path\":\"\\ud800\"}"), "");
    // `${_WS_ANSWER:0:200}`: bytes, or characters in a UTF-8 locale.
    let tail = |n: usize| format!("answered: {})\n", "é".repeat(n)).into_bytes();
    let line = no_folder_line("acme", "é".repeat(150).as_bytes(), || false);
    assert!(line.ends_with(&tail(100)));
    let line = no_folder_line("acme", "é".repeat(150).as_bytes(), || true);
    assert!(line.ends_with(&tail(150)));
    assert_eq!(basename("/srv/acme/"), Some("acme".into()));
    assert_eq!(basename("/"), Some("/".into()));
    assert_eq!(basename("rel"), None);
    assert_eq!(basename("/a\nb"), None);
}

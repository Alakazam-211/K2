use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Each line the parse and the call follow, as `cmd_connections` has them.
#[test]
fn the_shell_s_lines() {
    let text = source("usage.sh");
    for shell in [
        r#"    if [[ "${1:-}" == "--json" || "${1:-}" == "--users" || "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then"#,
        r#"        set -- list "$@""#,
        r#"    local action="${1:-list}""#,
        r#"                    --json) as_json=true; shift ;;"#,
        r#"                    --users) include_users=true; shift ;;"#,
        r#"                        echo "Error: unknown flag: $1" >&2"#,
        r#"                        echo "Error: unexpected argument: $1" >&2"#,
        r#"                        echo "Usage: k2 connections list [--json] [--users]" >&2"#,
        r#"                raw=$(cli_send cli_request "/cli/connections" "action=list" "users=1")"#,
        r#"                raw=$(cli_send cli_request "/cli/connections" "action=list")"#,
        r#"            AS_JSON="$as_json" INCLUDE_USERS="$include_users" python3 -c ""#,
        r#"" <<< "$raw""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "cmd_connections no longer has {shell:?}"
        );
    }
    let router = source("router.sh");
    assert!(router.contains(
        "    connections)\n        # 0.40.24 (B4): --help/-h on any connections subcommand prints help\n        # and exits 0 instead of being consumed as a workspace name.\n        # 0.46.17: so does `k2 connections help` (it printed the usage and\n        # exited 1).\n        if [ \"${2:-}\" = help ] || _wants_help \"${@:2}\"; then\n            cmd_help_connections\n            exit 0\n        fi\n        shift; cmd_connections \"$@\"\n"
    ));
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains(r#"        connections|reserve|release) echo "connections" ;;"#));
    assert!(policy.contains("        msg|inbox|respond|awareness|connections|"));
    let cell = source("cell-socket.sh");
    assert!(cell.contains("        /cli/connections) return 0 ;;"));
}

/// `cmd_connections`'s case has exactly the arms `run` knows (`list`,
/// `add`, `remove`, `*`), and its `*` arm prints [`UNKNOWN`] and exits 1.
#[test]
fn the_shell_s_arms() {
    let text = source("usage.sh");
    let body = &text[text
        .find("\ncmd_connections() {\n")
        .expect("cmd_connections")..];
    let body = &body[..body.find("\n}\n").expect("its end")];
    let arms: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("        "))
        .filter(|l| {
            l.strip_suffix(')').is_some_and(|w| {
                !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '*')
            })
        })
        .collect();
    assert_eq!(arms, ["list)", "add)", "remove)", "*)"]);
    let star = &body[body.find("\n        *)\n").expect("the * arm")..];
    let echoes: Vec<&str> = star
        .lines()
        .filter_map(|l| l.trim().strip_prefix("echo \""))
        .map(|l| l.strip_suffix("\" >&2").expect("an echo to stderr"))
        .collect();
    // The shell's name, whatever the brand.
    let ours: Vec<String> = UNKNOWN
        .iter()
        .map(|l| {
            l.replace(k2_client::brand::CLI_NAME, "k2")
                .replace('"', "\\\"")
        })
        .collect();
    assert_eq!(echoes, ours);
    assert!(star.contains("\n            exit 1\n            ;;\n    esac"));
}

/// The Python, its lines as `python3 -c` numbers them.
#[test]
fn the_python() {
    let lines = super::super::shell_python("usage.sh", "cmd_connections() {");
    let at = |n: u32| lines[n as usize - 1].trim().to_string();
    for (n, text) in [
        (3, "raw = sys.stdin.read()"),
        (6, "d = json.loads(raw)"),
        (
            8,
            r"sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])",
        ),
        (line::ERR, "err = d.get('error')"),
        (
            13,
            r"sys.stderr.write(json.dumps({'error': err}, ensure_ascii=False) + '\n')",
        ),
        (15, r"sys.stdout.write('Error: %s\n' % err)"),
        (17, "ws = d.get('workspace') or {}"),
        (18, "conns = d.get('connections', []) or []"),
        (line::J_WS, "'name': ws.get('name') or '',"),
        (line::J_FOR, "for c in conns:"),
        (line::J_C, "if c.get('remote'):"),
        (
            57,
            r"sys.stdout.write('Error: daemon omitted users[] (asked with users=1)\\n')",
        ),
        (
            74,
            "print(json.dumps(out, ensure_ascii=False, separators=(',', ':')))",
        ),
        (line::LABEL, "label = ws.get('name') or ws.get('path') or '?'"),
        (79, "if isinstance(tw, dict) and not include_users:"),
        (
            81,
            r"sys.stderr.write('This workspace: (could not read sessions: %s)\\n' % tw.get('error'))",
        ),
        (
            line::TW_LEN,
            "print('This workspace (%s): %d live' % (tw.get('handle') or label, len(rows)))",
        ),
        (
            line::TW_ROW,
            r"title = (r.get('title') or '').replace('\\n', ' ')",
        ),
        (87, "if len(title) > 50:"),
        (88, "title = title[:49] + '…'"),
        (91, "tail = '  * = you. ' if any(r.get('you') for r in rows) else '  '"),
        (92, "print(tail + 'Same workspace: always allowed to k2 msg. More: k2 sessions (--past for older chats)')"),
        (93, "print('')"),
        (94, "print('Connections for %s:' % label)"),
        (96, "print('  (none)')"),
        (line::FOR, "for c in conns:"),
        (line::C, "if c.get('remote'):"),
        (106, "bound = ' → %s' % handle"),
        (
            line::DISPLAY,
            "if display and display.strip().lower() != handle.strip().lower():",
        ),
        (108, "bound += ' (%s)' % display"),
        (112, "print('  - %s%s   (remote)%s%s' % (addr, bound, repair, gone))"),
        (114, "flag = '' if c.get('reachable', True) else '  [UNREACHABLE — project missing or path gone]'"),
        (115, "print('  - %s%s' % (c.get('projectName', '?'), flag))"),
        (122, "print('People on this host (humans — not workspace-agents; do not k2 msg):')"),
        (131, "(str(u.get('displayName') or '') if named else '') or '-',"),
        (141, "print('%s  %s  %s  %s' % ('USERNAME'.ljust(w_user), 'NAME'.ljust(w_name), 'ROLE'.ljust(w_role), 'DISABLED'))"),
        (145, "print('These are workspace-agents you can k2 msg. Humans on this box are not listed — k2 connections list --users (do not k2 msg those names).')"),
    ] {
        assert_eq!(at(n), text, "line {n}");
    }
    assert!(at(109).ends_with("'  [older pairing code: messages still work; re-pair recommended]' if c.get('repairRecommended') else ''"));
    assert!(at(111).ends_with("'  [GONE: that workspace is no longer on the host; handles are never reused, so this connection stays closed until your human adds it again]' if c.get('remoteWorkspaceGone') else ''"));
    assert!(at(89).starts_with("print('  %s %-16s %-9s %-7s %-8s %s' % ('*' if r.get('you') else ' ', r.get('address') or r.get('handle') or '-',"));
    assert_eq!(lines.len(), 145);
}

#[test]
fn the_words() {
    let ask = |json, users| Some(Ok(Ask { json, users }));
    assert_eq!(parse(&["list"]), ask(false, false));
    assert_eq!(parse(&["list", "--json", "--users"]), ask(true, true));
    assert_eq!(parse(&["--json"]), ask(true, false));
    assert_eq!(parse(&["--users", "--json"]), ask(true, true));
    assert_eq!(parse(&["--users", "--users"]), ask(false, true));
    let stop = |first: String| Some(Err([first, USAGE.to_string()]));
    assert_eq!(
        parse(&["list", "-x"]),
        stop("Error: unknown flag: -x".into())
    );
    assert_eq!(
        parse(&["list", "ws"]),
        stop("Error: unexpected argument: ws".into())
    );
    assert_eq!(
        parse(&["list", ""]),
        stop("Error: unexpected argument: ".into())
    );
    assert_eq!(
        parse(&["--json", "list"]),
        stop("Error: unexpected argument: list".into())
    );
    assert_eq!(parse(&[]), ask(false, false));
    assert_eq!(parse(&["", "--json"]), ask(true, false));
    assert_eq!(
        parse(&["", "x"]),
        stop("Error: unexpected argument: x".into())
    );
    for shell in [
        &["help"][..],
        &["add", "x"],
        &["list", "--help"],
        &["--json", "-h"],
        &["remove"],
    ] {
        assert_eq!(parse(shell), None, "{shell:?}");
    }
}

fn text(body: &str) -> Shown {
    reader(body.as_bytes(), Ask::default())
}

#[test]
fn errors_and_odd_answers() {
    assert_eq!(
        text(""),
        Shown {
            stdout: format!("{UNPARSEABLE}'\\n'\n"),
            stderr: String::new(),
            exit: Exit::Error
        }
    );
    assert_eq!(text("x").stdout, format!("{UNPARSEABLE}'x\\n'\n"));
    assert_eq!(
        text(r#"{"error":{"code":"owner_only","hint":"é"}}"#),
        Shown::err(
            "{\"error\": {\"code\": \"owner_only\", \"hint\": \"é\"}}\n",
            Exit::Error
        )
    );
    assert_eq!(
        text(r#"{"error":"not allowed"}"#),
        Shown {
            stdout: "Error: not allowed\n".into(),
            stderr: String::new(),
            exit: Exit::Error
        }
    );
    assert_eq!(text(r#"{"error":false}"#).stdout, "Error: False\n");
    assert_eq!(text(r#"{"error":null}"#).exit, Exit::Ok);
    let last = |body: &str, ask: Ask| {
        let r = reader(body.as_bytes(), ask);
        assert_eq!(r.exit, Exit::Error, "{body}");
        r.stderr.lines().last().unwrap().to_string()
    };
    let json = Ask {
        json: true,
        users: false,
    };
    for ask in [Ask::default(), json] {
        assert_eq!(
            last("[1]", ask),
            "AttributeError: 'list' object has no attribute 'get'"
        );
        assert_eq!(
            last(r#"{"workspace":"w"}"#, ask),
            "AttributeError: 'str' object has no attribute 'get'"
        );
        assert_eq!(
            last(r#"{"connections":5}"#, ask),
            "TypeError: 'int' object is not iterable"
        );
        assert_eq!(
            last(r#"{"connections":"ab"}"#, ask),
            "AttributeError: 'str' object has no attribute 'get'"
        );
    }
    assert!(reader(br#"{"error":[1]}"#, json).stdout == "Error: [1]\n");
}

#[test]
fn the_text_form() {
    let body = r#"{"workspace":{"name":"acme","path":"/w"},
      "thisWorkspace":{"handle":"acme","sessions":[
        {"address":"acme","role":"main","cli":"claude","state":"working","title":"T\nx","you":true},
        {"handle":"acme/h","title":"LONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONG"}]},
      "connections":[
        {"projectName":"beta"},
        {"projectName":"gone","reachable":false},
        {"remote":true,"agent":"seoca","host":"quahog.example","remoteWorkspaceId":"r1","handle":"quahog-site","displayName":"Quahog Site","repairRecommended":true},
        {"remote":true,"address":"x::y.example","remoteWorkspaceId":"r2","handle":"h","displayName":" H ","remoteWorkspaceGone":true},
        {"remote":true}]}"#;
    let r = text(body);
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(r.stderr, "");
    let want = [
        "This workspace (acme): 2 live".to_string(),
        format!("  * {:16} {:9} {:7} {:8} T x", "acme", "main", "claude", "working"),
        format!("    {:16} {:9} {:7} {:8} {}\u{2026}", "acme/h", "-", "-", "-", &"LONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONGLONG"[..49]),
        "  * = you. Same workspace: always allowed to k2 msg. More: k2 sessions (--past for older chats)".to_string(),
        String::new(),
        "Connections for acme:".to_string(),
        "  - beta".to_string(),
        "  - gone  [UNREACHABLE \u{2014} project missing or path gone]".to_string(),
        "  - seoca::quahog.example \u{2192} quahog-site (Quahog Site)   (remote)  [older pairing code: messages still work; re-pair recommended]".to_string(),
        "  - x::y.example \u{2192} h   (remote)  [GONE: that workspace is no longer on the host; handles are never reused, so this connection stays closed until your human adds it again]".to_string(),
        "  - ?::?   (remote)".to_string(),
        "These are workspace-agents you can k2 msg. Humans on this box are not listed \u{2014} k2 connections list --users (do not k2 msg those names).".to_string(),
    ];
    assert_eq!(r.stdout, want.join("\n") + "\n");
    let r = text(r#"{"thisWorkspace":{"error":"nope"}}"#);
    assert_eq!(
        r.stderr,
        "This workspace: (could not read sessions: nope)\n"
    );
    assert!(r.stdout.starts_with("\nConnections for ?:\n  (none)\n"));
    let r = text(
        r#"{"workspace":{"path":"/p"},"thisWorkspace":"x","connections":[{"projectName":null}]}"#,
    );
    assert!(r.stdout.starts_with("Connections for /p:\n  - None\n"));
    let r = text(r#"{"thisWorkspace":{"sessions":5}}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("TypeError: object of type 'int' has no len()")
    );
    let r = text(r#"{"thisWorkspace":{"sessions":[{"title":5}]}}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("AttributeError: 'int' object has no attribute 'replace'")
    );
    let r = text(
        r#"{"connections":[{"remote":1,"remoteWorkspaceId":1,"handle":"h","displayName":7}]}"#,
    );
    assert_eq!(
        r.stderr.lines().last(),
        Some("AttributeError: 'int' object has no attribute 'strip'")
    );
}

#[test]
fn the_users() {
    let users = Ask {
        json: false,
        users: true,
    };
    let body = r#"{"workspace":{"name":"w"},"thisWorkspace":{"handle":"w"},"users":[
        {"username":"ann","role":"owner","firstName":"Ann","displayName":"Ann Lee"},
        {"username":"bob","role":"member","disabled":true,"displayName":"Bob"},
        "skip",
        {}]}"#;
    let r = reader(body.as_bytes(), users);
    assert_eq!(r.exit, Exit::Ok);
    let want = [
        "Connections for w:",
        "  (none)",
        "",
        "People on this host (humans \u{2014} not workspace-agents; do not k2 msg):",
        "USERNAME  NAME     ROLE    DISABLED",
        "ann       Ann Lee  owner   no",
        "bob       -        member  yes",
        "?         -        ?       no",
    ];
    assert_eq!(r.stdout, want.join("\n") + "\n");
    let r = reader(br#"{"users":[]}"#, users);
    assert!(r.stdout.ends_with("do not k2 msg):\n  (none)\n"));
    let r = reader(br#"{"users":{}}"#, users);
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stdout.ends_with(&format!("  (none)\n{NO_USERS}\n")));
}

#[test]
fn the_json_form() {
    let ask = Ask {
        json: true,
        users: true,
    };
    let body = r#"{"workspace":{"name":"w","projectId":"p1"},"connections":[
        {"projectName":"","name":"n","id":"i","reachable":0},
        {"remote":true,"agent":"a","host":"h.example","handle":"é","remoteWorkspaceGone":1}],
        "users":[{"username":"u","firstName":null},3],"thisWorkspace":null}"#;
    let r = reader(body.as_bytes(), ask);
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(
        r.stdout,
        concat!(
            r#"{"ok":true,"workspace":{"name":"w","path":"","id":"p1"},"connections":["#,
            r#"{"name":"n","kind":"local","projectId":"i","reachable":0},"#,
            r#"{"name":"a::h.example","kind":"remote","address":"a::h.example","agent":"a","host":"h.example","remoteWorkspaceId":null,"handle":"é","displayName":null,"remoteWorkspaceGone":true}],"#,
            r#""users":[{"username":"u","role":"","disabled":false,"firstName":null,"lastName":null,"displayName":"u"}],"thisWorkspace":null}"#,
            "\n"
        )
    );
    let r = reader(
        br#"{}"#,
        Ask {
            json: true,
            users: false,
        },
    );
    assert_eq!(
        r.stdout,
        "{\"ok\":true,\"workspace\":{\"name\":\"\",\"path\":\"\",\"id\":\"\"},\"connections\":[]}\n"
    );
    let r = reader(br#"{"connections":[]}"#, ask);
    assert_eq!(r.stdout, format!("{NO_USERS}\n"));
    assert_eq!(r.exit, Exit::Error);
}

#[test]
fn what_dollar_paren_holds() {
    assert_eq!(held(b"{\0}\n\n"), b"{}");
    assert_eq!(held(b"\n"), b"");
    assert_eq!(held(b"a\nb"), b"a\nb");
}

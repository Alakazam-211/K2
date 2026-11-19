use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Each line the verbs follow, as `cmd_connections` has them.
#[test]
fn the_shell_s_lines() {
    let text = source("usage.sh");
    for verb in [Verb::Add, Verb::Remove] {
        let w = verb.word();
        for shell in [
            format!("        {w})"),
            r#"            local target="${1:-}""#.to_string(),
            format!(
                r#"            [[ -z "$target" ]] && {{ echo "{}" >&2; exit 1; }}"#,
                verb.usage()
            ),
        ] {
            assert!(
                text.lines().any(|l| l == shell),
                "cmd_connections no longer has {shell:?}"
            );
        }
        let call = format!(
            r#"            cli_request_post "/cli/connections" "action={w}&target=$(urlencode "$target")" | python3 -c ""#
        );
        assert!(
            text.lines().any(|l| l == call),
            "cmd_connections no longer has {call:?}"
        );
    }
    // `shift` after the action, so `${1:-}` is the word after the verb.
    assert!(text.contains("    local action=\"${1:-list}\"\n    shift 2>/dev/null || true\n"));
}

/// The Python, its lines as `python3 -c` numbers them; the two verbs'
/// error parts are the same.
#[test]
fn the_python() {
    let add = super::super::super::shell_python("usage.sh", "action=add&target=");
    let remove = super::super::super::shell_python("usage.sh", "action=remove&target=");
    assert_eq!(add[..33], remove[..33], "the error parts differ");
    let at = |lines: &[String], n: u32| lines[n as usize - 1].trim().to_string();
    for (n, text) in [
        (3, "raw = sys.stdin.read()"),
        (5, "d = json.loads(raw)"),
        (
            7,
            r"sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])",
        ),
        (line::ERR, "err = d.get('error')"),
        (13, "code = err.get('code') or ''"),
        (14, "hint = err.get('hint') or json.dumps(err)"),
        (16, "hint = str(err)"),
        (
            18,
            "code in ('agents_create_connections_disabled', 'gated', 'forbidden')",
        ),
        (line::ALLOW, "or 'Allow agents to create connections' in hint"),
        (
            line::NOT_ALLOWED,
            r#"or \"isn't allowed to create or remove connections\" in hint"#,
        ),
        (line::SETTINGS, "if 'Settings' not in hint:"),
        (line::JOIN, "hint = (hint + ' — ' if hint else '') + ("),
        (
            25,
            r#"\"this agent isn't allowed to create or remove connections — \""#,
        ),
        (26, r"'the owner can enable it in Settings \u2192 K2 Connect '"),
        (27, r"'(Allow agents to create connections) or Workspaces \u2192 '"),
        (28, r"'(workspace) \u2192 Allow agents to create connections'"),
        (
            30,
            r"sys.stderr.write(json.dumps({'error': {'code': code or 'agents_create_connections_disabled', 'hint': hint}}, ensure_ascii=False) + '\n')",
        ),
        (31, "sys.exit(3)"),
        (32, r"sys.stdout.write('Error: %s\n' % hint)"),
    ] {
        assert_eq!(at(&add, n), text, "line {n}");
    }
    for (n, text) in [
        (34, "if d.get('noop'):"),
        (
            35,
            "print('Already connected to %s (%s)' % (d.get('target', '?'), d.get('message', '')))",
        ),
        (37, "print('Connected to %s' % d.get('target', '?'))"),
        (40, "if d.get('bound') and d.get('handle'):"),
        (
            line::RPARTITION,
            "host = (d.get('target') or '').rpartition('::')[2]",
        ),
        (
            line::LABEL,
            "label = d.get('handle') + ('::' + host if host else '')",
        ),
        (
            line::STRIP,
            "if d.get('displayName') and d['displayName'].strip().lower() != d['handle'].strip().lower():",
        ),
        (44, "label += ' (%s)' % d['displayName']"),
        (45, "print('  reaches %s' % label)"),
        (46, "if d.get('bindWarning'):"),
        (47, r"sys.stderr.write('warning: %s\n' % d['bindWarning'])"),
        (48, "if d.get('warning'):"),
        (49, r"sys.stderr.write('warning: %s\n' % d['warning'])"),
    ] {
        assert_eq!(at(&add, n), text, "add line {n}");
    }
    assert_eq!(add.len(), 49);
    assert_eq!(at(&remove, 34), "print('Disconnected.')");
    assert_eq!(remove.len(), 34);
}

fn show(body: &str, verb: Verb) -> (String, String, i32) {
    let s = reader(body.as_bytes(), verb);
    (s.stdout, s.stderr, s.exit.code())
}

#[test]
fn what_an_add_prints() {
    let (o, e, c) = show(r#"{"ok":true,"target":"beta"}"#, Verb::Add);
    assert_eq!((o.as_str(), e.as_str(), c), ("Connected to beta\n", "", 0));
    let (o, _, _) = show(r#"{"noop":true,"target":"beta","message":"already"}"#, Verb::Add);
    assert_eq!(o, "Already connected to beta (already)\n");
    let (o, _, _) = show(r#"{"noop":1}"#, Verb::Add);
    assert_eq!(o, "Already connected to ? ()\n");
    let (o, _, _) = show(r#"{"target":null,"noop":true,"message":null}"#, Verb::Add);
    assert_eq!(o, "Already connected to None (None)\n");
    let (o, e, c) = show(
        r#"{"target":"seoca::quahog.example","bound":true,"handle":"quahog-site","displayName":"Quahog Site","bindWarning":"slow","warning":["w"]}"#,
        Verb::Add,
    );
    assert_eq!(
        o,
        "Connected to seoca::quahog.example\n  reaches quahog-site::quahog.example (Quahog Site)\n"
    );
    assert_eq!((e.as_str(), c), ("warning: slow\nwarning: ['w']\n", 0));
    let (o, _, _) = show(
        r#"{"target":"plain","bound":true,"handle":"h","displayName":" H "}"#,
        Verb::Add,
    );
    assert_eq!(o, "Connected to plain\n  reaches h::plain\n");
    let (o, _, _) = show(r#"{"target":"a::","bound":true,"handle":"h"}"#, Verb::Add);
    assert_eq!(o, "Connected to a::\n  reaches h\n");
}

#[test]
fn what_a_remove_prints() {
    let (o, e, c) = show(r#"{"ok":true,"warning":"x"}"#, Verb::Remove);
    assert_eq!((o.as_str(), e.as_str(), c), ("Disconnected.\n", "", 0));
    let (o, _, c) = show(r#"{"error":null}"#, Verb::Remove);
    assert_eq!((o.as_str(), c), ("Disconnected.\n", 0));
}

#[test]
fn the_daemon_s_errors() {
    for verb in [Verb::Add, Verb::Remove] {
        let (o, e, c) = show(r#"{"error":{"code":"not_found","hint":"no such é"}}"#, verb);
        assert_eq!((o.as_str(), e.as_str(), c), ("Error: no such é\n", "", 1));
        let (o, _, c) = show(r#"{"error":{"code":"x"}}"#, verb);
        assert_eq!((o.as_str(), c), ("Error: {\"code\": \"x\"}\n", 1));
        let (o, _, c) = show(r#"{"error":"plain"}"#, verb);
        assert_eq!((o.as_str(), c), ("Error: plain\n", 1));
        let (o, _, c) = show(r#"{"error":false}"#, verb);
        assert_eq!((o.as_str(), c), ("Error: False\n", 1));
        let (o, _, c) = show(r#"{"error":{"hint":[1,"a"]}}"#, verb);
        assert_eq!((o.as_str(), c), ("Error: [1, 'a']\n", 1));
    }
}

#[test]
fn the_gate() {
    let teach = format!(
        "{{\"error\": {{\"code\": \"gated\", \"hint\": \"no \u{2014} {TEACHING}\"}}}}\n"
    );
    let (o, e, c) = show(r#"{"error":{"code":"gated","hint":"no"}}"#, Verb::Add);
    assert_eq!((o.as_str(), e.as_str(), c), ("", teach.as_str(), 3));
    // A hint that says it, with Settings in it: as it is, the default code.
    let (o, e, c) = show(
        r#"{"error":{"hint":"Allow agents to create connections in Settings é"}}"#,
        Verb::Remove,
    );
    assert_eq!(
        (o.as_str(), e.as_str(), c),
        (
            "",
            "{\"error\": {\"code\": \"agents_create_connections_disabled\", \"hint\": \"Allow agents to create connections in Settings é\"}}\n",
            3
        )
    );
    // A text error that says it.
    let (_, e, c) = show(
        r#"{"error":"this agent isn't allowed to create or remove connections"}"#,
        Verb::Add,
    );
    assert_eq!(c, 3);
    assert!(e.contains(&format!("connections \u{2014} {TEACHING}")), "{e}");
    // An empty text error is not the gate.
    let (o, _, c) = show(r#"{"error":""}"#, Verb::Add);
    assert_eq!((o.as_str(), c), ("Error: \n", 1));
    // A code that is the gate's and an empty hint: the teaching alone.
    let (_, e, c) = show(r#"{"error":{"code":"forbidden","hint":""}}"#, Verb::Add);
    assert_eq!(c, 3);
    assert!(
        e.starts_with("{\"error\": {\"code\": \"forbidden\", \"hint\": \"{\\\"code\\\": \\\"forbidden\\\", \\\"hint\\\": \\\"\\\"} \u{2014} "),
        "{e}"
    );
}

#[test]
fn where_python_raises() {
    let tb = |body: &str, verb: Verb| {
        let s = reader(body.as_bytes(), verb);
        assert_eq!(s.exit, Exit::Error, "{body}");
        s.stderr
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        tb("[1]", Verb::Add),
        "AttributeError: 'list' object has no attribute 'get'"
    );
    assert_eq!(
        tb(r#"{"error":{"hint":5}}"#, Verb::Add),
        "TypeError: argument of type 'int' is not iterable"
    );
    assert_eq!(
        tb(r#"{"error":{"code":"gated","hint":true}}"#, Verb::Add),
        "TypeError: argument of type 'bool' is not iterable"
    );
    assert_eq!(
        tb(r#"{"error":{"code":"gated","hint":["x"]}}"#, Verb::Remove),
        "TypeError: can only concatenate list (not \"str\") to list"
    );
    assert_eq!(
        tb(r#"{"error":{"code":"gated","hint":{"a":1}}}"#, Verb::Add),
        "TypeError: unsupported operand type(s) for +: 'dict' and 'str'"
    );
    assert_eq!(
        tb(r#"{"bound":true,"handle":"h","target":5}"#, Verb::Add),
        "AttributeError: 'int' object has no attribute 'rpartition'"
    );
    assert_eq!(
        tb(r#"{"bound":true,"handle":[1],"target":"t"}"#, Verb::Add),
        "TypeError: can only concatenate list (not \"str\") to list"
    );
    assert_eq!(
        tb(r#"{"bound":true,"handle":2}"#, Verb::Add),
        "TypeError: unsupported operand type(s) for +: 'int' and 'str'"
    );
    assert_eq!(
        tb(r#"{"bound":true,"handle":"h","displayName":1}"#, Verb::Add),
        "AttributeError: 'int' object has no attribute 'strip'"
    );
    // What was printed before the raise stays.
    let s = reader(br#"{"bound":true,"handle":"h","displayName":1}"#, Verb::Add);
    assert_eq!(s.stdout, "Connected to ?\n");
    assert!(s.stderr.contains("line 43, in <module>"), "{}", s.stderr);
}

#[test]
fn an_answer_that_is_not_json() {
    for (body, want) in [
        ("", "Error: daemon returned an unparseable response: ''\n"),
        ("x", "Error: daemon returned an unparseable response: 'x'\n"),
        (
            "it's",
            "Error: daemon returned an unparseable response: \"it's\"\n",
        ),
    ] {
        let s = reader(body.as_bytes(), Verb::Add);
        assert_eq!((s.stdout.as_str(), s.exit), (want, Exit::Error), "{body:?}");
    }
    let long = "é".repeat(300);
    let s = reader(long.as_bytes(), Verb::Remove);
    assert_eq!(
        s.stdout,
        format!(
            "Error: daemon returned an unparseable response: '{}'\n",
            "é".repeat(200)
        )
    );
}

#[test]
fn the_target_and_its_usage() {
    assert_eq!(target(Verb::Add, &["beta", "x"]), Ok("beta"));
    assert_eq!(
        target(Verb::Add, &[]),
        Err("Usage: k2 connections add <workspace-name>".into())
    );
    assert_eq!(
        target(Verb::Remove, &[""]),
        Err("Usage: k2 connections remove <workspace-name>".into())
    );
    assert_eq!(Verb::of("add"), Some(Verb::Add));
    assert_eq!(Verb::of("remove"), Some(Verb::Remove));
    assert_eq!(Verb::of("create"), None);
}

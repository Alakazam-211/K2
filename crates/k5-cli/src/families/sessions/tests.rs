use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `_sessions_render`'s Python, its lines as `python3 -c` numbers them
/// (line 1 is the empty one after `-c '`).
fn render_python() -> Vec<String> {
    let text = source("sessions.sh");
    let start = "K2_SS_VERB=\"$verb\" python3 -c '\n";
    let body = &text[text.find(start).expect("the render's Python") + start.len()..];
    let end = body.find("\n'\n").expect("its end");
    std::iter::once(String::new())
        .chain(body[..end].split('\n').map(str::to_string))
        .collect()
}

/// Each line the parse and the calls follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let awareness = source("awareness.sh");
    for shell in [
        r#"        help|--help|-h) _sessions_usage; return 0 ;;"#,
        r#"        ""|-*) cmd_sessions_roster "$@"; return ;;"#,
        r#"        list|ls|past|show|open|new|stop|main|archives|live|compact|set-label)"#,
        r#"        main)      cmd_sessions_main "$@" ;;"#,
        r#"        list|ls)   cmd_sessions_roster "$@" ;;"#,
        r#"        past)      cmd_sessions_roster --past "$@" ;;"#,
        r#"        show)      cmd_sessions_show "$@" ;;"#,
        r#"        open)      cmd_sessions_open "$@" ;;"#,
        r#"        new)       cmd_sessions_new "$@" ;;"#,
        r#"        stop)      cmd_sessions_stop "$@" ;;"#,
        r#"        archives)  cmd_sessions_list "$@" ;;"#,
        r#"        spawn)     cmd_sessions_spawn "$@" ;;"#,
        r#"        live)      cmd_sessions_live "$@" ;;"#,
        r#"        compact)   cmd_sessions_compact "$@" ;;"#,
        r#"        set-label) cmd_sessions_set_label "$@" ;;"#,
        r#"            echo "usage: unknown sessions verb '$sub' (k2 sessions --help)" >&2"#,
    ] {
        assert!(
            awareness.lines().any(|l| l == shell),
            "cmd_sessions no longer has {shell:?}"
        );
    }
    let text = source("sessions.sh");
    for shell in [
        r#"            --all|-a) all=1; shift ;;"#,
        r#"            --json) as_json=1; shift ;;"#,
        r#"            --past) past=1; shift ;;"#,
        r#"            --search) search="${2:-}"; past=1; shift 2 ;;"#,
        r#"            --days) days="${2:-}"; past=1; shift 2 ;;"#,
        r#"            --cli) cli="${2:-}"; past=1; shift 2 ;;"#,
        r#"            --limit) limit="${2:-}"; past=1; shift 2 ;;"#,
        r#"            --archived) archived=1; past=1; shift ;;"#,
        r#"            --workspace) workspace="${2:-}"; shift 2 ;;"#,
        r#"            *) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;"#,
        r#"    [ -z "$workspace" ] && workspace="$PROJECT""#,
        r#"        q="workspace=$(urlencode "$workspace")""#,
        r#"        [ -n "$search" ] && q="${q}&search=$(urlencode "$search")""#,
        r#"        [ -n "$days" ] && q="${q}&days=$(urlencode "$days")""#,
        r#"        [ -n "$cli" ] && q="${q}&cli=$(urlencode "$cli")""#,
        r#"        [ -n "$limit" ] && q="${q}&limit=$(urlencode "$limit")""#,
        r#"        [ "$archived" = "1" ] && q="${q}&archived=1""#,
        r#"        raw="$(_sidecar_send GET /cli/sessions/past "$q")" || true"#,
        r#"    [ "$all" = "1" ] && q="${q}&all=1""#,
        r#"    raw="$(_sidecar_send GET /cli/sessions/roster "$q")" || true"#,
        r#"                    keep-main|keep_main|adopt-sidecar|adopt_sidecar) ;;"#,
        r#"                    *) echo "usage: --thread keep-main|adopt-sidecar (which Thread the main chat shows afterwards)" >&2; exit 2 ;;"#,
        r#"                if [ -z "$action" ]; then action="set"; shift; continue; fi"#,
        r#"                    echo "usage: k2 sessions main [--json]  |  k2 sessions main set <id> [--json]" >&2; exit 2"#,
        r#"    raw="$(_sidecar_send GET /cli/sessions/main "workspace=$(urlencode "$workspace")")" || true"#,
        r#"            --tail) tail="${2:-}"; shift 2 ;;"#,
        r#"                    echo "usage: k2 sessions show <id|address> [--tail N] [--json]" >&2; exit 2"#,
        r#"        echo "usage: k2 sessions show <id|address> [--tail N] [--json]  (ids: k2 sessions --past)" >&2"#,
        r#"    local q="workspace=$(urlencode "$workspace")&id=$(urlencode "$id")""#,
        r#"    [ -n "$tail" ] && q="${q}&tail=$(urlencode "$tail")""#,
        r#"    raw="$(_sidecar_send GET /cli/sessions/show "$q")" || true"#,
        r#"    if [ "$_SC_STATUS" != "200" ]; then"#,
        r#"        if [ "$_SC_STATUS" = "404" ] && [[ "$_SC_BODY" == *"route not found"* || "$_SC_BODY" == *"Not Found"* ]]; then"#,
        r#"            echo "daemon_too_old: this K2 daemon has no k2 sessions routes yet (update K2: k2 update --app)" >&2"#,
        r#"        _sidecar_fail "$_SC_STATUS" "$_SC_BODY""#,
        r#"        printf '%s\n' "$_SC_BODY""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "k2 sessions no longer has {shell:?}"
        );
    }
    // The shell's words in the default brand.
    assert_eq!(
        TOO_OLD.replace(k2_client::brand::PRODUCT_NAME, "K2"),
        "daemon_too_old: this K2 daemon has no k2 sessions routes yet (update K2: k2 update --app)"
    );
    let sidecar = source("sidecar.sh");
    for shell in [
        r#"    _SC_STATUS="${raw##*$'\n'}""#,
        r#"    _SC_BODY="${raw%$'\n'*}""#,
        r#"    [ -z "$_SC_STATUS" ] && _SC_STATUS="000""#,
        r#"    print("daemon_error: " + raw[:200]); sys.exit(0)"#,
        r#"    print("%s: %s" % (e.get("code", "error"), e.get("hint", "")))"#,
        r#"    print("error: %s" % e)"#,
        r#"    print("error: unexpected response")"#,
        r#"    [ -z "$line" ] && line="daemon_unreachable: no answer from the K2 daemon (status ${status:-none})""#,
        r#"    _permission_note "$body""#,
        r#"        400|413) echo 2 ;;"#,
        r#"        403) echo 3 ;;"#,
        r#"        404) echo 4 ;;"#,
        r#"            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \"#,
        r#"                --connect-timeout 5 --max-time 120 \"#,
        r#"        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then"#,
        // 0.46.17: the TCP arm's credential is the passport in a session
        // (`_k2_sock_fallback_token`); `sessions` is an id tool, so a
        // session with no passport is refused before any send.
        r#"    _k2_sock_fallback_token "$sock" "$out""#,
        r#"        _k2_cfg_auth "$_K2_TCP_TOKEN""#,
        r#"            -w $'\n%{http_code}' 2>/dev/null || true"#,
    ] {
        assert!(
            sidecar.lines().any(|l| l == shell),
            "_sidecar_send or _sidecar_fail no longer has {shell:?}"
        );
    }
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains(r#"        sessions|terminal) echo "sessions-spawn" ;;"#));
    assert!(policy.contains("|connect-account|sessions-spawn|fs|daemon-admin|"));
    assert!(policy.contains("_K2_CURL_TRIES=4"));
}

/// The lines the traceback names, as `python3 -c` numbers them.
#[test]
fn the_python_s_lines() {
    let lines = render_python();
    let at = |n: u32| lines[n as usize - 1].trim().to_string();
    for (n, text) in [
        (line::LOAD, "v = json.load(sys.stdin)"),
        (line::AGO_D, "d = max(0, int(now - t))"),
        (line::CUT_S, r#"s = (s or "").replace("\n", " ").strip()"#),
        (line::R_ROWS, r#"rows = v.get("sessions", [])"#),
        (
            line::R_WS,
            r#"ws = (v.get("workspace") or {}).get("handle") or "?""#,
        ),
        (
            line::R_LIVE,
            r#"live = [r for r in rows if r.get("state") != "asleep"]"#,
        ),
        (line::R_TITLE, r#"title = cut(r.get("title"), 60)"#),
        (
            line::R_AGO,
            r#"r.get("state") or "-", ago(r.get("lastActiveAt")), title))"#,
        ),
        (
            line::R_ASLEEP,
            r#"tail.append("%d asleep (k2 sessions --all)" % asleep)"#,
        ),
        (line::P_ROWS, r#"rows = v.get("conversations", [])"#),
        (line::P_DAYS, r#"what = "last %d days" % v.get("days", 14)"#),
        (line::P_FOR, "for r in rows:"),
        (line::P_RUN, r#"run = r.get("runner") or {}"#),
        (
            line::P_FIRST,
            r#"print("Open one: k2 sessions open %s --name <name>   ·   Read first: k2 sessions show %s" % (first["id"], first["id"]))"#,
        ),
        (
            line::P_UNSUPPORTED,
            r#"for u in v.get("unsupported") or []:"#,
        ),
        (
            line::P_NOTE,
            r#"print("note: %s: %s" % (u.get("cli"), u.get("why")), file=sys.stderr)"#,
        ),
        (line::S_C, r#"c = v.get("conversation") or {}"#),
        (line::S_WHERE, r#"where = c.get("worktree") or "root""#),
        (
            line::S_COUNT,
            r#"head += " · %d messages" % v["messageCount"]"#,
        ),
        (
            line::S_ASK,
            r#"print("Asked first: \"%s\"" % cut(v["firstAsk"], 200))"#,
        ),
        (
            line::S_LEN,
            r#"print("--- last %d messages ---" % len(msgs))"#,
        ),
        (line::S_FOR, "for m in msgs:"),
        (
            line::S_TOOLS,
            r#"print("(%d tool calls not shown)" % v["toolCalls"])"#,
        ),
        (line::S_RUN, r#"if run and run.get("address"):"#),
        (line::M_M, r#"m = v.get("main") or {}"#),
        (line::M_STATE, r#"state = m.get("state")"#),
    ] {
        assert_eq!(at(n), text, "line {n}");
    }
    assert!(at(line::R_HEAD).starts_with(r#"print("%s — %d live (cap %d sidecars; %d in use)" % (ws, len(live), sc.get("cap", 0), sc.get("live", 0)))"#));
    assert!(at(line::P_NOW).starts_with(r#"now_s = "%s as %s" % (run.get("state") or "live""#));
    assert!(at(line::P_ROW).starts_with(r#"print(fmt % (r.get("id"), r.get("cli"), ago(r.get("lastActiveAt")), cut(r.get("worktree") or "(root)", 24), cut(title, 34), now_s))"#));
    assert!(
        at(line::P_MATCH).starts_with(r#"print("             match: %s" % cut(r["match"], 110))"#)
    );
    assert!(at(line::P_FILES).contains(r#"% sc.get("files", 0))"#));
    assert!(at(line::S_HEAD).starts_with(r#"head = "%s  %s  \"%s\"   last active %s ago" % (c.get("id"), c.get("cli"), cut(c.get("customName") or c.get("title"), 60), ago(c.get("lastActiveAt")))"#));
    assert!(at(line::S_MSG).starts_with(r#"print("[%s %s]  %s" % (m.get("role"), ago(m.get("at")) if m.get("at") else "", m.get("text", "")))"#));
    assert!(at(line::M_PATH).ends_with(r#""\n  " + m["path"] if m.get("path") else ""))"#));
    // The texts this prints, each as the Python has it.
    for text in [
        r#"print(fmt % (" ", "ADDRESS", "ROLE", "CLI", "STATE", "ACTIVE", "TITLE / BRIEF"))"#,
        r#"fmt = "%s %-16s %-9s %-7s %-8s %-7s %s""#,
        r#"hint = ("Message one: k2 msg %s \"…\"" % example) if example else "Message one: k2 msg <address> \"…\"""#,
        r#"print(("* = you.  " if any(r.get("you") for r in rows) else "") + hint)"#,
        r#"print("(nobody is running here)")"#,
        r#"tail.append("past conversations: k2 sessions --past")"#,
        r#"print(" · ".join(tail))"#,
        r#"what += ", matching \"%s\"" % v["search"]"#,
        r#"what += ", %s only" % v["cli"]"#,
        r#"print("Past conversations in %s (%s):" % (ws, what))"#,
        r#"print("  (none found; try --days 90, or another --search)")"#,
        r#"fmt = "  %-10s %-7s %-5s %-24s %-34s %s""#,
        r#"print(fmt % ("ID", "CLI", "LAST", "WHERE", "TITLE", "NOW"))"#,
        r#"print("Every one listed is held already: k2 msg its address (NOW column).")"#,
        r#"head += " · %s" % where"#,
        r#"head += " · branch %s" % c["branch"]"#,
        r#"print("(%s)" % v["unsupported"])"#,
        r#"print("It is %s as %s. Message it: k2 msg %s \"…\"" % (run.get("state"), run["address"], run["address"]))"#,
        r#"print("Reopen: k2 sessions open %s   (or k2 msg --session %s \"…\")" % (c.get("id"), c.get("id")))"#,
        r#"print("%s has no main chat pinned yet (it starts on the next message)." % ws)"#,
        r#"sys.stderr.write("%s main chat %s (%s) CANNOT BE FOUND: no conversation on disk in any of its folders.\n" % (ws, m.get("conversationId"), m.get("cli")))"#,
        r#"sys.stderr.write("K2 kept it pinned and did not start or switch to another chat.\n")"#,
        r#"sys.stderr.write("Choose one: k2 sessions --past, then k2 sessions main set <id>\n")"#,
        r#"print("%s main chat %s (%s): pre-allocated, no first turn yet." % (ws, m.get("conversationId"), m.get("cli")))"#,
        r#"print("%s main chat %s (%s): a custom preset; K2 cannot check it on disk." % (ws, m.get("conversationId"), m.get("cli")))"#,
        r#"if d < 60: return "now""#,
        r#"if d < 3600: return "%dm" % (d // 60)"#,
        r#"if d < 86400: return "%dh" % (d // 3600)"#,
        r#"return "%dd" % (d // 86400)"#,
        r#"return s if len(s) <= n else s[: n - 1] + "…""#,
    ] {
        assert!(
            lines.iter().any(|l| l.trim() == text),
            "the render's Python no longer has {text:?}"
        );
    }
}

fn roster_ask(workspace: &str, all: bool, json: bool) -> Option<Result<Ask, String>> {
    Some(Ok(Ask::Roster {
        workspace: workspace.into(),
        all,
        json,
    }))
}

fn past_ask(search: &str, days: &str, archived: bool, json: bool) -> Option<Result<Ask, String>> {
    Some(Ok(Ask::Past {
        workspace: String::new(),
        search: search.into(),
        days: days.into(),
        cli: String::new(),
        limit: String::new(),
        archived,
        json,
    }))
}

#[test]
fn the_words() {
    assert_eq!(parse(&[]), roster_ask("", false, false));
    assert_eq!(parse(&["--all", "--json"]), roster_ask("", true, true));
    assert_eq!(parse(&["-a"]), roster_ask("", true, false));
    assert_eq!(
        parse(&["list", "--workspace", "w"]),
        roster_ask("w", false, false)
    );
    assert_eq!(parse(&["ls"]), roster_ask("", false, false));
    assert_eq!(parse(&["--past"]), past_ask("", "", false, false));
    assert_eq!(parse(&["past", "--json"]), past_ask("", "", false, true));
    assert_eq!(
        parse(&["--search", "x y", "--all"]),
        past_ask("x y", "", false, false)
    );
    assert_eq!(
        parse(&["--days", "", "--archived"]),
        past_ask("", "", true, false)
    );
    assert_eq!(parse(&["--days", "9"]), past_ask("", "9", false, false));
    assert_eq!(
        parse(&["--search", "--json"]),
        past_ask("--json", "", false, false)
    );
    assert_eq!(parse(&[""]), Some(Err(unknown_flag(""))));
    assert_eq!(parse(&["--bogus"]), Some(Err(unknown_flag("--bogus"))));
    assert_eq!(parse(&["list", "x"]), Some(Err(unknown_flag("x"))));
    assert_eq!(parse(&["nope"]), Some(Err(unknown_verb("nope"))));
    assert_eq!(parse(&["List"]), Some(Err(unknown_verb("List"))));
    for shell in [
        &["help"][..],
        &["--help"],
        &["-h"],
        &["list", "-h"],
        &["--search"],
        &["--workspace"],
        &["list", "--days"],
        &["open", "x"],
        &["new", "x"],
        &["stop", "x"],
        &["archives"],
        &["spawn"],
        &["live", "w"],
        &["compact", "s"],
        &["set-label", "x"],
        &["main", "set", "x"],
        &["main", "--json", "set", "x"],
        &["main", "set"],
        &["main", "--workspace"],
        &["show", "--tail"],
        &["show", "x", "--workspace"],
    ] {
        assert_eq!(parse(shell), None, "{shell:?}");
    }
    let main = |w: &str, json| {
        Some(Ok(Ask::Main {
            workspace: w.into(),
            json,
        }))
    };
    assert_eq!(parse(&["main"]), main("", false));
    assert_eq!(
        parse(&["main", "--json", "--workspace", "w"]),
        main("w", true)
    );
    assert_eq!(
        parse(&["main", "--thread", "adopt_sidecar"]),
        main("", false)
    );
    assert_eq!(parse(&["main", "--thread"]), Some(Err(THREAD_USAGE.into())));
    assert_eq!(
        parse(&["main", "--thread", "x", "set"]),
        Some(Err(THREAD_USAGE.into()))
    );
    assert_eq!(parse(&["main", "x"]), Some(Err(MAIN_USAGE.into())));
    assert_eq!(
        parse(&["main", "set", "a", "b"]),
        Some(Err(MAIN_USAGE.into()))
    );
    assert_eq!(
        parse(&["main", "set", "", "b"]),
        None,
        "an empty id is no id"
    );
    assert_eq!(parse(&["main", "set", "a", "set"]), None);
    assert_eq!(
        parse(&["main", "set", "a", "-x"]),
        Some(Err(unknown_flag("-x")))
    );
    let show = |id: &str, tail: &str, json| {
        Some(Ok(Ask::Show {
            workspace: String::new(),
            id: id.into(),
            tail: tail.into(),
            json,
        }))
    };
    assert_eq!(parse(&["show", "ab12"]), show("ab12", "", false));
    assert_eq!(
        parse(&["show", "--tail", "5", "a", "--json"]),
        show("a", "5", true)
    );
    assert_eq!(parse(&["show"]), Some(Err(SHOW_NO_ID.into())));
    assert_eq!(
        parse(&["show", "--tail", "5"]),
        Some(Err(SHOW_NO_ID.into()))
    );
    assert_eq!(parse(&["show", "a", "b"]), Some(Err(SHOW_USAGE.into())));
    assert_eq!(parse(&["show", "-x"]), Some(Err(unknown_flag("-x"))));
}

#[test]
fn the_queries() {
    let q = |words: &[&str]| {
        let ask = parse(words).unwrap().unwrap();
        let (route, q) = request(&ask, "/srv/a b");
        format!("{route}?{q}")
    };
    assert_eq!(q(&[]), "/cli/sessions/roster?workspace=%2Fsrv%2Fa%20b");
    assert_eq!(
        q(&["--all", "--workspace", "w&x"]),
        "/cli/sessions/roster?workspace=w%26x&all=1"
    );
    assert_eq!(
        q(&["--workspace", ""]),
        "/cli/sessions/roster?workspace=%2Fsrv%2Fa%20b"
    );
    assert_eq!(
        q(&["--past", "--search", "é x", "--days", "9", "--cli", "codex", "--limit", "3", "--archived"]),
        "/cli/sessions/past?workspace=%2Fsrv%2Fa%20b&search=%C3%A9%20x&days=9&cli=codex&limit=3&archived=1"
    );
    assert_eq!(
        q(&["show", "a/b", "--tail", "2"]),
        "/cli/sessions/show?workspace=%2Fsrv%2Fa%20b&id=a%2Fb&tail=2"
    );
    assert_eq!(
        q(&["main", "--workspace", "w"]),
        "/cli/sessions/main?workspace=w"
    );
}

#[test]
fn what_the_call_holds() {
    let ok = |status: u16, body: &[u8]| {
        held(Ok(Response {
            status,
            body: body.to_vec(),
            channel: Channel::Tcp,
            permission: None,
            server_version: None,
        }))
    };
    assert_eq!(ok(200, b"{\0}\n"), ("200".into(), b"{}\n".to_vec()));
    assert_eq!(ok(0, b""), ("000".into(), Vec::new()));
    let refused = held(Err(Error::Unreachable {
        endpoint: ROSTER.into(),
        err: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
    }));
    assert_eq!(refused, ("000".into(), b"\n000\n000\n000".to_vec()));
    assert_eq!(
        held(Err(Error::Empty {
            endpoint: ROSTER.into()
        })),
        ("000".into(), Vec::new())
    );
    for (status, exit) in [
        ("400", Exit::Usage),
        ("413", Exit::Usage),
        ("403", Exit::Refused),
        ("404", Exit::NotFound),
        ("000", Exit::Error),
        ("500", Exit::Error),
        ("201", Exit::Error),
    ] {
        assert_eq!(exit_for(status), exit, "{status}");
    }
}

#[test]
fn the_fail_line() {
    let f = |b: &str| fail_line("403", b.as_bytes());
    assert_eq!(f(""), unreachable("403"));
    assert_eq!(
        unreachable("000").replace(k2_client::brand::PRODUCT_NAME, "K2"),
        "daemon_unreachable: no answer from the K2 daemon (status 000)"
    );
    assert_eq!(f("x"), "daemon_error: x");
    assert_eq!(f("\n000\n000\n000"), "daemon_error: \n000\n000\n000");
    assert_eq!(f("ab\n\n"), "daemon_error: ab");
    assert_eq!(
        f(&"é".repeat(300)),
        format!("daemon_error: {}", "é".repeat(200))
    );
    assert_eq!(
        f(r#"{"error":{"code":"owner_only","hint":"no é"}}"#),
        "owner_only: no é"
    );
    assert_eq!(f(r#"{"error":{}}"#), "error: ");
    assert_eq!(f(r#"{"error":{"code":null,"hint":5}}"#), "None: 5");
    assert_eq!(f(r#"{"error":"bad"}"#), "error: bad");
    assert_eq!(f(r#"{"error":[1]}"#), "error: [1]");
    assert_eq!(f(r#"{"error":""}"#), "error: unexpected response");
    assert_eq!(f("[1]"), "error: unexpected response");
    assert_eq!(f(r#"{"error":"x\n\n"}"#), "error: x");
}

const NOW: f64 = 1_800_000_000.5;

fn rendered(ask: &[&str], body: &str) -> Shown {
    let ask = parse(ask).unwrap().unwrap();
    render(&ask, body.as_bytes(), NOW)
}

#[test]
fn ago_and_cut_are_the_shell_s() {
    let n = |v: Value| ago(Some(&v), NOW, 1);
    let int = |x: i64| Value::Int(x.to_string());
    assert_eq!(ago(None, NOW, 1).unwrap(), "-");
    assert_eq!(n(int(0)).unwrap(), "-");
    assert_eq!(n(Value::Float(NOW - 59.9)).unwrap(), "now");
    assert_eq!(n(int(1_800_000_000 + 99)).unwrap(), "now", "the future");
    assert_eq!(n(Value::Float(NOW - 60.0)).unwrap(), "1m");
    assert_eq!(n(Value::Float(NOW - 3599.0)).unwrap(), "59m");
    assert_eq!(n(Value::Float(NOW - 3600.0)).unwrap(), "1h");
    assert_eq!(n(Value::Float(NOW - 86400.0 * 3.5)).unwrap(), "3d");
    assert_eq!(n(Value::Bool(true)).unwrap(), "20833d");
    let e = n(s("x")).unwrap_err();
    assert_eq!(e.frames, vec![(1, "<module>"), (line::AGO_D, "ago")]);
    assert_eq!(
        e.exception,
        "TypeError: unsupported operand type(s) for -: 'float' and 'str'"
    );
    assert!(n(Value::Float(f64::NAN))
        .unwrap_err()
        .exception
        .starts_with("ValueError"));
    assert!(n(Value::Float(f64::NEG_INFINITY))
        .unwrap_err()
        .exception
        .starts_with("OverflowError"));
    assert!(n(Value::Int("9".repeat(400)))
        .unwrap_err()
        .exception
        .starts_with("OverflowError"));
    let c = |v: Value, k| cut(Some(&v), k, 1);
    assert_eq!(cut(None, 5, 1).unwrap(), "");
    assert_eq!(c(int(0), 5).unwrap(), "");
    assert_eq!(c(s("  a\nb  "), 5).unwrap(), "a b");
    assert_eq!(c(s("abcdef"), 5).unwrap(), "abcd\u{2026}");
    assert_eq!(c(s("ééééé"), 5).unwrap(), "ééééé");
    assert_eq!(c(s("\u{3000}x\u{a0}"), 5).unwrap(), "x");
    let e = c(int(5), 5).unwrap_err();
    assert_eq!(e.frames, vec![(1, "<module>"), (line::CUT_S, "cut")]);
    assert_eq!(
        e.exception,
        "AttributeError: 'int' object has no attribute 'replace'"
    );
}

#[test]
fn the_roster() {
    let t = NOW - 7200.0;
    let body = format!(
        r#"{{"workspace":{{"handle":"acme"}},"sidecars":{{"cap":4,"live":1}},"asleep":2,"sessions":[
        {{"address":"acme","role":"main","cli":"claude","state":"working","lastActiveAt":{t},"title":"Fix the thing\nnow","you":true}},
        {{"handle":"acme/h","role":"sidecar","state":"idle","lastActiveAt":null,"title":"{long}"}},
        {{"address":"acme/r","state":"asleep"}}]}}"#,
        long = "ü".repeat(70)
    );
    let r = rendered(&[], &body);
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(r.stderr, "");
    let want = [
        "acme \u{2014} 2 live (cap 4 sidecars; 1 in use)".to_string(),
        format!(
            "  {:16} {:9} {:7} {:8} {:7} TITLE / BRIEF",
            "ADDRESS", "ROLE", "CLI", "STATE", "ACTIVE"
        ),
        format!(
            "* {:16} {:9} {:7} {:8} {:7} Fix the thing now",
            "acme", "main", "claude", "working", "2h"
        ),
        format!(
            "  {:16} {:9} {:7} {:8} {:7} {}\u{2026}",
            "acme/h",
            "sidecar",
            "-",
            "idle",
            "-",
            "ü".repeat(59)
        ),
        format!(
            "  {:16} {:9} {:7} {:8} {:7} ",
            "acme/r", "-", "-", "asleep", "-"
        ),
        "* = you.  Message one: k2 msg acme/r \"\u{2026}\"".to_string(),
        "past conversations: k2 sessions --past".to_string(),
    ];
    assert_eq!(r.stdout, want.join("\n") + "\n");
    let r = rendered(&[], r#"{"asleep":3}"#);
    assert_eq!(
        r.stdout,
        "? \u{2014} 0 live (cap 0 sidecars; 0 in use)\n(nobody is running here)\n3 asleep (k2 sessions --all) \u{b7} past conversations: k2 sessions --past\n"
    );
    let r = rendered(
        &[],
        r#"{"sessions":[{"address":"a","you":true}],"sidecars":{"cap":2.9}}"#,
    );
    assert!(r
        .stdout
        .starts_with("? \u{2014} 1 live (cap 2 sidecars; 0 in use)\n"));
    assert!(r.stdout.ends_with("* = you.  Message one: k2 msg <address> \"\u{2026}\"\npast conversations: k2 sessions --past\n"));
    // Where Python raises.
    let raise = |body: &str| {
        let r = rendered(&[], body);
        assert_eq!(r.exit, Exit::Error, "{body}");
        r.stderr.lines().last().unwrap().to_string()
    };
    assert_eq!(
        raise("[1]"),
        "AttributeError: 'list' object has no attribute 'get'"
    );
    assert_eq!(
        raise(r#"{"sessions":null}"#),
        "TypeError: 'NoneType' object is not iterable"
    );
    assert_eq!(
        raise(r#"{"sessions":"ab"}"#),
        "AttributeError: 'str' object has no attribute 'get'"
    );
    assert_eq!(
        raise(r#"{"workspace":"w"}"#),
        "AttributeError: 'str' object has no attribute 'get'"
    );
    assert_eq!(
        raise(r#"{"sidecars":{"cap":"x"}}"#),
        "TypeError: %d format: a real number is required, not str"
    );
    assert_eq!(
        raise(r#"{"sidecars":[1]}"#),
        "AttributeError: 'list' object has no attribute 'get'"
    );
    assert_eq!(
        raise(r#"{"sessions":[{"title":5}]}"#),
        "AttributeError: 'int' object has no attribute 'replace'"
    );
    assert_eq!(
        raise(r#"{"asleep":"x"}"#),
        "TypeError: %d format: a real number is required, not str"
    );
    assert_eq!(
        raise(""),
        "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)"
    );
    let r = rendered(&[], r#"{"sessions":[{"address":"a","lastActiveAt":"x"}]}"#);
    assert_eq!(
        r.stdout.lines().count(),
        2,
        "the head and the column line stay"
    );
    assert!(r
        .stderr
        .contains("line 30, in <module>\n  File \"<string>\", line 9, in ago\n"));
}

#[test]
fn the_past() {
    let t = NOW - 86400.0 * 2.0 - 5.0;
    let body = format!(
        r#"{{"workspace":"acme","days":30,"search":"bug","cli":"codex","truncated":true,"scanned":{{"files":12}},"unsupported":[{{"cli":"pi","why":"no logs"}}],"conversations":[
        {{"id":"ab12cd","cli":"claude","lastActiveAt":{t},"worktree":"wt-1","title":"Old","customName":"Named","runner":{{"address":"acme/x"}},"match":"the bug is here"}},
        {{"id":"ef34","cli":"codex","title":null}}]}}"#
    );
    let r = rendered(&["--past"], &body);
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(r.stderr, "note: pi: no logs\n");
    let row = |c: [&str; 6]| {
        format!(
            "  {:10} {:7} {:5} {:24} {:34} {}",
            c[0], c[1], c[2], c[3], c[4], c[5]
        )
    };
    let want = [
        "Past conversations in acme (last 30 days, matching \"bug\", codex only):".to_string(),
        row(["ID", "CLI", "LAST", "WHERE", "TITLE", "NOW"]),
        row(["ab12cd", "claude", "2d", "wt-1", "Named", "live as acme/x"]),
        "             match: the bug is here".to_string(),
        row(["ef34", "codex", "-", "(root)", "", "-"]),
        "Open one: k2 sessions open ef34 --name <name>   \u{b7}   Read first: k2 sessions show ef34".to_string(),
        "(search stopped after 12 files; narrow with --days or --cli)".to_string(),
    ];
    assert_eq!(r.stdout, want.join("\n") + "\n");
    let r = rendered(&["--past"], r#"{"conversations":[]}"#);
    assert_eq!(r.stdout, "Past conversations in ? (last 14 days):\n  (none found; try --days 90, or another --search)\n");
    let r = rendered(
        &["--past"],
        r#"{"conversations":[{"id":"a","runner":{"state":"asleep"}}]}"#,
    );
    assert!(r
        .stdout
        .ends_with("Every one listed is held already: k2 msg its address (NOW column).\n"));
    assert!(r.stdout.contains("asleep as -"));
    let r = rendered(&["--past"], r#"{"conversations":[{}]}"#);
    assert!(
        r.stdout
            .ends_with("Every one listed is held already: k2 msg its address (NOW column).\n"),
        "an empty dict is falsy"
    );
    let r = rendered(&["--past"], r#"{"conversations":[{"cli":"x"}]}"#);
    assert_eq!(r.stderr.lines().last(), Some("KeyError: 'id'"));
    assert!(r.stdout.contains("None"));
    let r = rendered(&["--past"], r#"{"days":null}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("TypeError: %d format: a real number is required, not NoneType")
    );
    let r = rendered(&["--past"], r#"{"conversations":[{"runner":"x"}]}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("AttributeError: 'str' object has no attribute 'get'")
    );
    let r = rendered(&["--past"], r#"{"unsupported":[{"cli":1},2]}"#);
    assert!(r
        .stderr
        .starts_with("note: 1: None\nTraceback (most recent call last):\n"));
}

#[test]
fn the_show() {
    let t = NOW - 600.0;
    let body = format!(
        r#"{{"conversation":{{"id":"ab12","cli":"claude","title":"T é","lastActiveAt":{t},"worktree":"wt","branch":"b1","runner":{{"state":"live","address":"acme/x"}}}},"messageCount":3,"firstAsk":"do it","messages":[{{"role":"user","at":{t},"text":"hi"}},{{"role":"assistant","text":"ok"}}],"toolCalls":2}}"#
    );
    let r = rendered(&["show", "ab12"], &body);
    assert_eq!(r.exit, Exit::Ok);
    let want = [
        "ab12  claude  \"T é\"   last active 10m ago \u{b7} 3 messages \u{b7} wt \u{b7} branch b1",
        "Asked first: \"do it\"",
        "--- last 2 messages ---",
        "[user 10m]  hi",
        "[assistant ]  ok",
        "(2 tool calls not shown)",
        "It is live as acme/x. Message it: k2 msg acme/x \"\u{2026}\"",
    ];
    assert_eq!(r.stdout, want.join("\n") + "\n");
    let r = rendered(&["show", "x"], r#"{"unsupported":"no logs for pi"}"#);
    assert_eq!(
        r.stdout,
        "None  None  \"\"   last active - ago \u{b7} root\n(no logs for pi)\nReopen: k2 sessions open None   (or k2 msg --session None \"\u{2026}\")\n"
    );
    let r = rendered(&["show", "x"], r#"{"messages":5}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("TypeError: object of type 'int' has no len()")
    );
    let r = rendered(&["show", "x"], r#"{"conversation":{"runner":"r"}}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("AttributeError: 'str' object has no attribute 'get'")
    );
    let r = rendered(&["show", "x"], r#"{"messages":"ab"}"#);
    assert!(r.stdout.ends_with("--- last 2 messages ---\n"));
}

#[test]
fn the_main() {
    let r = rendered(&["main"], r#"{"workspace":"acme","main":{"state":"none"}}"#);
    assert_eq!(
        r.stdout,
        "acme has no main chat pinned yet (it starts on the next message).\n"
    );
    let r = rendered(
        &["main"],
        r#"{"workspace":"acme","main":{"state":"missing","conversationId":"c1","cli":"codex"}}"#,
    );
    assert_eq!(r.exit, Exit::Error);
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr.replace(k2_client::brand::PRODUCT_NAME, "K2"),
        "acme main chat c1 (codex) CANNOT BE FOUND: no conversation on disk in any of its folders.\nK2 kept it pinned and did not start or switch to another chat.\nChoose one: k2 sessions --past, then k2 sessions main set <id>\n"
    );
    let r = rendered(
        &["main"],
        r#"{"main":{"state":"unstarted","conversationId":"c1","cli":"codex"}}"#,
    );
    assert_eq!(
        r.stdout,
        "? main chat c1 (codex): pre-allocated, no first turn yet.\n"
    );
    let r = rendered(&["main"], r#"{"main":{"state":"custom"}}"#);
    assert_eq!(
        r.stdout,
        "? main chat None (None): a custom preset; K2 cannot check it on disk.\n"
    );
    let r = rendered(
        &["main"],
        r#"{"workspace":"w","main":{"state":"ok","conversationId":"c","cli":"claude","path":"/p/x.jsonl"}}"#,
    );
    assert_eq!(r.stdout, "w main chat c (claude)\n  /p/x.jsonl\n");
    let r = rendered(&["main"], r#"{"main":{}}"#);
    assert_eq!(r.stdout, "? main chat None (None)\n");
    let r = rendered(&["main"], r#"{"main":{"path":5}}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("TypeError: can only concatenate str (not \"int\") to str")
    );
    let r = rendered(&["main"], r#"{"main":"m"}"#);
    assert_eq!(
        r.stderr.lines().last(),
        Some("AttributeError: 'str' object has no attribute 'get'")
    );
}

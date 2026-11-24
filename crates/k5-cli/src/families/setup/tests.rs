use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/setup.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `_setup_render_json_py`'s Python as `python3 -c` numbers it: line 1
/// is what follows `-c '` on its line.
fn python() -> Vec<String> {
    let text = source();
    let body = text
        .split_once("_setup_render_json_py() {")
        .expect("the Python's function")
        .1;
    let start = body.find("python3 -c '").expect("python3 -c '") + "python3 -c '".len();
    let rest = &body[start..];
    let end = rest.find("\n'\n").expect("the end of the Python");
    rest[..end].split('\n').map(str::to_string).collect()
}

fn at(lines: &[String], n: u32) -> &str {
    lines[n as usize - 1].trim()
}

/// Each line, path and pattern the binary follows, as `cmd_setup`,
/// `_setup_send` and `_setup_render` have them.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"            -h|--help|help) _setup_usage; return 0 ;;"#,
        r#"            --json) as_json=1; shift ;;"#,
        r#"            hide|show|kessel|chief-of-staff|dismiss-offer|restart|kessel-welcome)"#,
        r#"                [ -z "$sub" ] || { _setup_usage >&2; exit 2; }"#,
        r#"            --tool|--name|--handle|--about|--focus-group)"#,
        r#"                    *) echo "k2 setup: $1 goes with k2 setup kessel or k2 setup chief-of-staff" >&2; exit 2 ;;"#,
        r#"                [ $# -ge 2 ] || { echo "k2 setup: $1 needs a value" >&2; exit 2; }"#,
        r#"                        [ "$sub" = "kessel" ] || { echo "k2 setup chief-of-staff: no --handle (its folder comes from --name)" >&2; exit 2; }"#,
        r#"                        [ "$sub" = "chief-of-staff" ] || { echo "k2 setup kessel: no --about" >&2; exit 2; }"#,
        r#"                [ -n "$row" ] || { echo "usage: k2 setup skip <step> (k2 setup lists them)" >&2; exit 2; }"#,
        r#"            *) echo "k2 setup: unknown argument: $1" >&2; _setup_usage >&2; exit 2 ;;"#,
        r#"        if [ -z "$kessel_name" ] || [ "$have_about" = "0" ] || [ -z "$kessel_tool" ]; then"#,
        r#"            echo "usage: k2 setup chief-of-staff --name N --about \"what they do\" --tool claude|codex|grok [--focus-group G] (ask your person all three first)" >&2"#,
        r#"            if [ -n "$kessel_tool" ]; then _setup_jstr "$kessel_tool"; body="${body}${sep}\"tool\": ${_SETUP_J}"; sep=", "; fi"#,
        r#"            if [ -n "$kessel_name" ]; then _setup_jstr "$kessel_name"; body="${body}${sep}\"name\": ${_SETUP_J}"; sep=", "; fi"#,
        r#"            if [ -n "$kessel_handle" ]; then _setup_jstr "$kessel_handle"; body="${body}${sep}\"handle\": ${_SETUP_J}"; sep=", "; fi"#,
        r#"            if [ -n "$kessel_about" ] || [ "$sub" = "chief-of-staff" ]; then"#,
        r#"            if [ -n "$kessel_group" ]; then _setup_jstr "$kessel_group"; body="${body}${sep}\"focusGroup\": ${_SETUP_J}"; sep=", "; fi"#,
        r#"        skip) _setup_jstr "$row"; body="{\"row\": ${_SETUP_J}}" ;;"#,
        r#"        hide|show|dismiss-offer|kessel-welcome) body='{}' ;;"#,
        r#"        "") raw="$(_setup_send GET /cli/first-run/status "$as_json")" ;;"#,
        r#"        hide|show) raw="$(_setup_send POST "/cli/first-run/$sub" "$as_json" "$body")" ;;"#,
        r#"        dismiss-offer) raw="$(_setup_send POST /cli/first-run/offer/dismiss "$as_json" "$body")" ;;"#,
        r#"        kessel) raw="$(_setup_send POST /cli/first-run/kessel "$as_json" "$body")" ;;"#,
        r#"        chief-of-staff) raw="$(_setup_send POST /cli/first-run/chief-of-staff "$as_json" "$body")" ;;"#,
        r#"        skip) raw="$(_setup_send POST /cli/first-run/skip "$as_json" "$body")" ;;"#,
        // `_setup_jstr`.
        r#"            $'\177') out="${out}${c}" ;;"#,
        r#"            [[:cntrl:]]) printf -v c '\\u%04x' "'$c"; out="${out}${c}" ;;"#,
        // `_setup_send`.
        r#"    [ "$as_json" = "1" ] || q="?format=text""#,
        r#"        _k2_cfg_add url "${BASE_URL}${path}${q}""#,
        r#"        _setup_token"#,
        r#"        _k2_cfg_auth "$_SETUP_TOK""#,
        // `_setup_token` (0.46.17): in a session the passport or nothing.
        r#"    if _envn_any HOOK_SOCK; then"#,
        r#"        _SETUP_TOK="${SCOPED_TOKEN:-}""#,
        r#"        _SETUP_TOK="${TOKEN:-}""#,
        r#"            out="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 30 -w $'\n%{http_code}' 2>/dev/null; printf x)""#,
        r#"            _k2_cfg_add data-raw "$body""#,
        r#"                -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null; printf x)""#,
        r#"        if [ "$method" = "GET" ] && [ "$code" = "503" ] && [[ "$out" == *'"state":"migrating"'* ]] && [ "$try" -lt "$_K2_CURL_TRIES" ]; then"#,
        r#"        if _cli_retry_with_healed_passport "${out%$'\n'*}"; then"#,
        // `_setup_render`.
        r#"    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ] || [ -z "$body" ]; then"#,
        r#"        echo "Error: could not reach the K2 daemon (k2 setup)" >&2"#,
        r#"    _permission_note "$body""#,
        r#"            printf '%s\n' "$body""#,
        r#"                printf '%s' "$body" | _setup_render_json_py "$sub" "$as_json""#,
        r#"            echo "Error: the daemon sent something k2 setup does not understand: ${body:0:200}" >&2"#,
        r#"    local re_obj='"error"[[:space:]]*:[[:space:]]*\{(.*)$'"#,
        r#"    local re_str='"error"[[:space:]]*:[[:space:]]*"'"#,
        r#"        _setup_jfield code "$rest" && err_code="$_SETUP_J""#,
        r#"        if _setup_jfield hint "$rest" && [ -n "$_SETUP_J" ]; then"#,
        r#"        elif _setup_jfield message "$rest" && [ -n "$_SETUP_J" ]; then"#,
        r#"    elif [[ "$body" =~ $re_str ]] && _setup_jfield error "$body"; then"#,
        r#"        hint="${body:0:200}""#,
        r#"    if [ -n "$err_code" ] && [[ "$hint" != *"$err_code"* ]]; then"#,
        r#"        echo "Error: ${err_code}: ${hint}" >&2"#,
        r#"                [ "$sub" = "restart" ] || { echo "k2 setup: --kessel goes with k2 setup restart" >&2; exit 2; }"#,
        r#"        restart) if [ "$restart_kessel" = 1 ]; then body='{"kessel": true}'; else body='{}'; fi ;;"#,
        r#"        restart) raw="$(_setup_send POST /cli/first-run/restart "$as_json" "$body")" ;;"#,
        r#"        kessel-welcome) raw="$(_setup_send POST /cli/first-run/kessel/welcome "$as_json" "$body")" ;;"#,
        r#"        usage|unknown_row|tool_not_signed_in) exit 2 ;;"#,
        r#"        unknown_focus_group|ambiguous_focus_group|no_kessel) exit 4 ;;"#,
        r#"        *person_only*|*session_only*|*forbidden*|*role_required*|*token*|*passport*) exit 3 ;;"#,
        r#"    local re='"'"$1"'"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)"'"#,
    ] {
        assert!(text.contains(shell), "cmd_setup no longer has {shell:?}");
    }
    // The shell's words in the default brand.
    assert_eq!(
        UNREACHABLE.replace(k2_client::brand::PRODUCT_NAME, "K2"),
        "Error: could not reach the K2 daemon (k2 setup)"
    );
    assert_eq!(
        MEET_KESSEL.replace(k2_client::brand::PRODUCT_NAME, "K2"),
        "Next: meet Kessel, the agent that runs K2: k2 setup kessel"
    );
    for (sub, word) in [
        (Sub::List, ""),
        (Sub::Hide, "hide"),
        (Sub::Show, "show"),
        (Sub::Kessel, "kessel"),
        (Sub::ChiefOfStaff, "chief-of-staff"),
        (Sub::DismissOffer, "dismiss-offer"),
        (Sub::Skip, "skip"),
        (Sub::Restart, "restart"),
        (Sub::KesselWelcome, "kessel-welcome"),
    ] {
        assert_eq!(sub.word(), word);
        assert_eq!(sub.post(), sub != Sub::List);
    }
    // `setup chief-of-staff` rides the locked tool.
    let policy = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh"),
    )
    .expect("cli-tools-policy.sh");
    assert!(policy.contains(
        "        setup)\n            case \"${2:-}\" in\n                chief-of-staff) echo \"agent-harness\" ;;"
    ));
}

/// The Python's texts and the lines it raises on.
#[test]
fn the_python_s_lines() {
    let py = python();
    for (n, line) in [
        (3, "raw = sys.stdin.read()"),
        (
            PY_ERROR_LINE,
            r#"print("Error: %s%s" % ((code + ": ") if code and code not in hint else "", hint), file=sys.stderr)"#,
        ),
        (PY_LOW, r#"low = (code + " " + hint).lower()"#),
        (PY_GRANTED, r#"granted = perms.get("granted") or []"#),
        (PY_LEFT, r#"left = perms.get("left") or []"#),
        (
            PY_JOIN_GRANTED,
            r#"print("Allowed from the start: " + ", ".join(granted))"#,
        ),
        (
            PY_JOIN_LEFT,
            r#"print("It asks on a permission card for: " + ", ".join("%s (%s)" % (l.get("action"), l.get("reason")) for l in left))"#,
        ),
        (
            PY_DONE,
            r#"done = sum(1 for r in rows if r.get("state") == "done")"#,
        ),
        (
            PY_DONE + 1,
            r#"print("Setup: %d of %d done" % (done, len(rows)))"#,
        ),
        (
            PY_LINE,
            r#"line = "  %s %s" % (MARK.get(r.get("state"), "[?]"), LABELS.get(rid, rid))"#,
        ),
        (PY_NOTE, r#"note = NOTES.get(r.get("note") or "")"#),
        (
            PY_KESSEL,
            r#"print("Kessel: %s at %s (k2 thread <its handle> to talk to it)" % (kessel.get("name"), kessel.get("path")))"#,
        ),
    ] {
        assert_eq!(at(&py, n), line, "line {n}");
    }
    let all = py.join("\n");
    for (id, label) in LABELS {
        assert!(all.contains(&format!("    \"{id}\": \"{label}\",")), "{id}");
    }
    for (id, note) in NOTES {
        assert!(all.contains(&format!("    \"{id}\": \"{note}\",")), "{id}");
    }
    assert!(all.contains(r#"MARK = {"done": "[x]", "todo": "[ ]", "skipped": "[-]"}"#));
    for text in [APPLE_TOOLS, FINISHED, HIDDEN, NOT_ENABLED, MEET_KESSEL] {
        let text = text.replace(k2_client::brand::PRODUCT_NAME, "K2");
        assert!(all.contains(&text), "{text:?}");
    }
    for line in [
        r#"    print("Error: the daemon sent something k2 setup does not understand: " + raw[:200], file=sys.stderr)"#,
        r#"if err is not None or not isinstance(d, dict) or ("rows" not in d and not (made and "workspaceId" in d)):"#,
        r#"        hint = err.get("hint") or err.get("message") or code"#,
        r#"        code, hint = "", str(err if err is not None else raw[:200])"#,
        r#"    if any(w in low for w in ("person_only", "session_only", "forbidden", "role_required", "token", "passport")):"#,
        r#"    what = "Kessel" if sub == "kessel" else "your Chief of Staff""#,
        r#"    if rid == "dev_tools" and r.get("opens") is None and r.get("note"):"#,
        r#"        note = "run: " + str(r.get("note"))"#,
        r#"    elif rid == "dev_tools" and r.get("opens") == "dev_tools" and not note:"#,
        r#"    if note and r.get("state") != "done":"#,
        r#"        line += " (" + note + ")""#,
    ] {
        assert!(all.contains(line), "{line:?}");
    }
}

fn usage_text() -> String {
    usage().to_string()
}

#[test]
fn the_words_as_cmd_setup_reads_them() {
    let ok = |words: &[&str]| parse(words).unwrap_or_else(|e| panic!("{words:?}: {e:?}"));
    let l = ok(&[]);
    assert_eq!((l.sub, l.json, l.body.as_str()), (Sub::List, false, ""));
    let l = ok(&["--json", "--json"]);
    assert_eq!((l.sub, l.json), (Sub::List, true));
    for (verb, sub) in [
        ("hide", Sub::Hide),
        ("show", Sub::Show),
        ("dismiss-offer", Sub::DismissOffer),
    ] {
        let l = ok(&[verb, "--json"]);
        assert_eq!((l.sub, l.json, l.body.as_str()), (sub, true, "{}"));
    }
    assert_eq!(ok(&["kessel"]).body, "{}");
    assert_eq!(
        ok(&[
            "kessel",
            "--focus-group",
            "Ops",
            "--handle",
            "kay",
            "--name",
            "Kay",
            "--tool",
            "claude"
        ])
        .body,
        r#"{"tool": "claude", "name": "Kay", "handle": "kay", "focusGroup": "Ops"}"#
    );
    assert_eq!(
        ok(&["kessel", "--name", "", "--focus-group", ""]).body,
        "{}"
    );
    assert_eq!(
        ok(&[
            "chief-of-staff",
            "--name",
            "Ada",
            "--about",
            "",
            "--tool",
            "codex"
        ])
        .body,
        r#"{"tool": "codex", "name": "Ada", "about": ""}"#
    );
    assert_eq!(ok(&["skip", "dev_tools"]).body, r#"{"row": "dev_tools"}"#);
    assert_eq!(ok(&["skip", "--json"]).body, r#"{"row": "--json"}"#);
    assert_eq!(ok(&["--json", "skip", "x"]).sub, Sub::Skip);
    let usage = |words: &[&str]| match parse(words) {
        Err(Stop::Usage(t)) => t,
        other => panic!("{words:?}: {other:?}"),
    };
    assert_eq!(usage(&["hide", "show"]), usage_text());
    assert_eq!(usage(&["skip", "x", "hide"]), usage_text());
    assert_eq!(usage(&["hide", "skip", "x"]), usage_text());
    assert_eq!(usage(&["skip"]), format!("{SKIP_USAGE}\n"));
    assert_eq!(usage(&["skip", ""]), format!("{SKIP_USAGE}\n"));
    assert_eq!(
        usage(&["--tool", "claude"]),
        "k2 setup: --tool goes with k2 setup kessel or k2 setup chief-of-staff\n"
    );
    assert_eq!(
        usage(&["skip", "x", "--focus-group", "Ops"]),
        "k2 setup: --focus-group goes with k2 setup kessel or k2 setup chief-of-staff\n"
    );
    assert_eq!(
        usage(&["kessel", "--name"]),
        "k2 setup: --name needs a value\n"
    );
    assert_eq!(usage(&["kessel", "--about", "x"]), format!("{NO_ABOUT}\n"));
    assert_eq!(
        usage(&["chief-of-staff", "--handle", "x"]),
        format!("{NO_HANDLE}\n")
    );
    assert_eq!(
        usage(&["chief-of-staff", "--name", "Ada"]),
        format!("{CHIEF_USAGE}\n")
    );
    assert_eq!(
        usage(&["chief-of-staff", "--name", "Ada", "--tool", "x"]),
        format!("{CHIEF_USAGE}\n")
    );
    assert_eq!(
        usage(&["--bogus"]),
        format!("k2 setup: unknown argument: --bogus\n{}", usage_text())
    );
    assert_eq!(
        usage(&[""]),
        format!("k2 setup: unknown argument: \n{}", usage_text())
    );
    assert_eq!(
        usage(&["k5"]),
        format!("k2 setup: unknown argument: k5\n{}", usage_text())
    );
    for help in [&["help"][..], &["--json", "-h"], &["kessel", "--help"]] {
        assert_eq!(parse(help), Err(Stop::Help), "{help:?}");
    }
    // A value is no help.
    assert_eq!(
        ok(&["kessel", "--name", "help"]).body,
        r#"{"name": "help"}"#
    );
    assert!(usage_text().starts_with("k2 setup [--json]"));
}

/// `agent-reports` hands every word after it to `_sf_settings_run setup`
/// where the loop reaches it with no verb before it (`--json` first when
/// it came before the verb); after a verb it is the usage.
#[test]
fn agent_reports_takes_the_words_after_it() {
    let reports = |w: &[&str]| -> Result<Line, Stop> {
        Err(Stop::Reports(w.iter().map(|s| s.to_string()).collect()))
    };
    for (words, want) in [
        (&["agent-reports"][..], &[][..]),
        (
            &[
                "agent-reports",
                "--agents",
                "all",
                "--attach",
                "computer,log-tail",
            ],
            &["--agents", "all", "--attach", "computer,log-tail"],
        ),
        (&["--json", "agent-reports"], &["--json"]),
        (&["agent-reports", "--json"], &["--json"]),
        (&["agent-reports", "--bogus"], &["--bogus"]),
        (&["agent-reports", "hide"], &["hide"]),
        (
            &["--json", "--json", "agent-reports", "x"],
            &["--json", "x"],
        ),
        (&["agent-reports", "-h"], &["-h"]),
    ] {
        assert_eq!(parse(words), reports(want), "{words:?}");
    }
    for words in [
        &["hide", "agent-reports"][..],
        &["kessel", "agent-reports"],
        &["skip", "x", "agent-reports"],
    ] {
        assert_eq!(parse(words), Err(Stop::Usage(usage_text())), "{words:?}");
    }
    // A mistake before it is the binary's; a value is no verb.
    assert_eq!(
        parse(&["--bogus", "agent-reports"]),
        Err(Stop::Usage(format!(
            "k2 setup: unknown argument: --bogus\n{}",
            usage_text()
        )))
    );
    assert_eq!(
        parse(&["skip", "agent-reports"]).map(|l| l.body),
        Ok(r#"{"row": "agent-reports"}"#.to_string())
    );
    assert_eq!(
        parse(&["kessel", "--name", "agent-reports"]).map(|l| l.body),
        Ok(r#"{"name": "agent-reports"}"#.to_string())
    );
    assert_eq!(parse(&["-h", "agent-reports"]), Err(Stop::Help));
    let text = source();
    assert!(text.contains(
        "            agent-reports)\n                # The agent opt-in (0.46.17): send-feedback's settings read\n                # and write, in bash and curl like the rest of k2 setup.\n                [ -z \"$sub\" ] || { _setup_usage >&2; exit 2; }\n                shift\n                if [ \"$as_json\" = 1 ]; then set -- --json \"$@\"; fi\n                _sf_settings_run setup \"$@\"\n                return ;;\n"
    ));
}

/// The words of `cmd_setup`'s loop (`cli/src/setup.sh`), as its arms
/// name them: a new one fails here until the binary parses it.
#[test]
fn each_word_of_cmd_setup_is_parsed_or_handed_on() {
    let text = source();
    let body = text.split_once("\ncmd_setup() {\n").expect("cmd_setup").1;
    let body = body
        .split_once("    while [ $# -gt 0 ]; do\n        case \"$1\" in\n")
        .expect("cmd_setup's loop")
        .1;
    let arms = &body[..body
        .find("\n        esac\n    done\n")
        .expect("the loop's end")];
    let mut words = Vec::new();
    for line in arms.lines() {
        // An arm of the loop's own case: twelve spaces, then the pattern.
        let Some(rest) = line.strip_prefix("            ") else {
            continue;
        };
        if rest.starts_with(' ') {
            continue;
        }
        let pattern = rest.split_once(')').expect("an arm's pattern").0;
        words.extend(pattern.split('|').map(str::to_string));
    }
    let want = [
        "-h",
        "--help",
        "help",
        "--json",
        "hide",
        "show",
        "kessel",
        "chief-of-staff",
        "dismiss-offer",
        "restart",
        "kessel-welcome",
        "--kessel",
        "agent-reports",
        "--tool",
        "--name",
        "--handle",
        "--about",
        "--focus-group",
        "skip",
        "*",
    ];
    assert_eq!(words, want, "cmd_setup's words moved: read them again");
    for w in words.iter().filter(|w| *w != "*") {
        let unknown = Err(Stop::Usage(format!(
            "{}\n{}",
            unknown_argument(w),
            usage_text()
        )));
        let got = parse(&[w.as_str()]);
        assert_ne!(got, unknown, "{w}: not parsed");
    }
}

/// Settings → Getting started's verbs, read as `cmd_setup` reads them.
#[test]
fn restart_and_kessel_welcome_are_parsed_as_the_shell_reads_them() {
    let line = |w: &[&str]| parse(w).unwrap_or_else(|e| panic!("{w:?}: {e:?}"));
    let r = line(&["restart"]);
    assert_eq!((r.sub, r.json, r.body.as_str()), (Sub::Restart, false, "{}"));
    assert_eq!(line(&["restart", "--kessel"]).body, r#"{"kessel": true}"#);
    assert_eq!(line(&["--json", "restart", "--kessel"]).json, true);
    let k = line(&["kessel-welcome", "--json"]);
    assert_eq!((k.sub, k.json, k.body.as_str()), (Sub::KesselWelcome, true, "{}"));
    assert_eq!(Sub::Restart.endpoint(), "/cli/first-run/restart");
    assert_eq!(Sub::KesselWelcome.endpoint(), "/cli/first-run/kessel/welcome");
    for w in [&["--kessel", "restart"][..], &["show", "--kessel"], &["kessel-welcome", "--kessel"]] {
        assert_eq!(parse(w), Err(Stop::Usage(format!("{KESSEL_GOES_WITH}\n"))), "{w:?}");
    }
    assert_eq!(parse(&["restart", "show"]), Err(Stop::Usage(usage_text().to_string())));
}

#[test]
fn values_as_setup_jstr_writes_them() {
    for (value, want) in [
        ("plain", r#""plain""#),
        ("", r#""""#),
        (r#"K "Five" \ 5"#, r#""K \"Five\" \\ 5""#),
        (
            "runs \"ops\"\tand\nbooks\r",
            r#""runs \"ops\"\tand\nbooks\r""#,
        ),
        ("a\u{1}b\u{1f}c\u{7f}d", "\"a\\u0001b\\u001fc\u{7f}d\""),
        ("\u{1b}[0m", "\"\\u001b[0m\""),
        ("Ann's caf\u{e9} \u{2192}", "\"Ann's caf\u{e9} \u{2192}\""),
        ("\u{8}\u{c}", "\"\\u0008\\u000c\""),
    ] {
        assert_eq!(jstr(value), want, "{value:?}");
    }
}

#[test]
fn json_strings_as_setup_junesc_reads_them() {
    for (s, want) in [
        (&br"plain"[..], &b"plain"[..]),
        (br#"a\"b\\c\/d"#, br#"a"b\c/d"#),
        (br"\n\t\r\b\f", b"\n\t\r\x08\x0c"),
        (br"\u0041\u00e9\u2192", "A\u{e9}\u{2192}".as_bytes()),
        (br"\ud83d\ude00", b"\xed\xa0\xbd\xed\xb8\x80"),
        (br"x\u0000y", b"xy"),
        (br"\uZZ12rest", b"rest"),
        (br"\u12", b""),
        (br"a\u", b"a"),
        (br"\q", b"q"),
    ] {
        assert_eq!(junesc(s), want, "{:?}", String::from_utf8_lossy(s));
    }
}

#[test]
fn fields_as_setup_jfield_finds_them() {
    let f = |k: &str, t: &str| jfield(k, t.as_bytes()).map(|v| String::from_utf8(v).unwrap());
    assert_eq!(f("code", r#"{"code":"x"}"#).as_deref(), Some("x"));
    assert_eq!(
        f("code", "{\"code\" \t:\n \"x y\"}").as_deref(),
        Some("x y")
    );
    assert_eq!(
        f("code", r#"{"code":5,"code":"later"}"#).as_deref(),
        Some("later")
    );
    assert_eq!(
        f("hint", r#"{"hint":"a \"q\" b"}"#).as_deref(),
        Some(r#"a "q" b"#)
    );
    assert_eq!(f("hint", r#"{"hint":""}"#).as_deref(), Some(""));
    assert_eq!(f("hint", r#"{"hint":"open"#), None);
    assert_eq!(f("hint", r#"{"xhint":"no"}"#), None);
    assert_eq!(f("hint", r#"{"hint" "no"}"#), None);
    assert_eq!(
        f("message", "{\"message\":\"two\nlines\"}").as_deref(),
        Some("two\nlines")
    );
    assert_eq!(f("hint", "{\"hint\":\"end\\"), None);
    let o = |t: &str| error_object(t.as_bytes()).map(|r| String::from_utf8(r.to_vec()).unwrap());
    assert_eq!(
        o(r#"{"error":{"code":"a"}}"#).as_deref(),
        Some(r#""code":"a"}}"#)
    );
    assert_eq!(
        o(r#"{"error":"s","x":{"error" : {"y":1}}}"#).as_deref(),
        Some(r#""y":1}}}"#)
    );
    assert_eq!(o(r#"{"error":"s"}"#), None);
    assert!(error_string(br#"{"error" : "s"}"#));
    assert!(!error_string(br#"{"error":5}"#));
}

#[test]
fn the_cut_bash_makes() {
    let ascii = "a".repeat(300);
    assert_eq!(cut_200(ascii.as_bytes(), || panic!("not asked")).len(), 200);
    let short = "\u{e9}".repeat(90);
    assert_eq!(cut_200(short.as_bytes(), || panic!("not asked")).len(), 180);
    let wide = "\u{e9}".repeat(300);
    assert_eq!(cut_200(wide.as_bytes(), || true).len(), 400);
    assert_eq!(cut_200(wide.as_bytes(), || false).len(), 200);
    let mut bad = vec![0xffu8; 150];
    bad.extend("\u{2192}".repeat(100).as_bytes());
    assert_eq!(cut_200(&bad, || true).len(), 150 + 50 * 3);
    // A cut-off character at the end counts byte by byte.
    let mut tail = "a".repeat(199).into_bytes();
    tail.extend_from_slice(&[0xe2, 0x86]);
    tail.extend_from_slice(&[b'x'; 10]);
    assert_eq!(cut_200(&tail, || true), &tail[..200]);
}

fn rendered(sub: Sub, json: bool, raw: &str) -> (String, String, Exit) {
    let r = render(sub, json, raw.as_bytes(), || false);
    (
        String::from_utf8(r.stdout).unwrap(),
        String::from_utf8(r.stderr).unwrap(),
        r.exit,
    )
}

#[test]
fn the_answers_as_setup_render_prints_them() {
    let unreachable = (String::new(), format!("{UNREACHABLE}\n"), Exit::Error);
    for raw in ["", "000", "\n000", "x\n000", "\n200", "200", "\n000\n000"] {
        assert_eq!(rendered(Sub::List, false, raw), unreachable, "{raw:?}");
    }
    let list = "Setup: 1 of 2 done\n  [x] Sign in to an AI tool\n";
    assert_eq!(
        rendered(Sub::List, false, &format!("{list}\n200")),
        (list.to_string(), String::new(), Exit::Ok)
    );
    // A refused connect tried again left its 000 above the answer.
    assert_eq!(
        rendered(Sub::List, false, &format!("\n000{list}\n200")),
        (format!("\n000{list}"), String::new(), Exit::Ok)
    );
    assert_eq!(
        rendered(Sub::Hide, true, "{\"rows\":[]}\n200"),
        ("{\"rows\":[]}\n".to_string(), String::new(), Exit::Ok)
    );
    let refusal = |body: &str| rendered(Sub::List, false, &format!("{body}\n403"));
    for (body, line, exit) in [
        (
            r#"{"ok":false,"error":{"code":"first_run_person_only","hint":"agents have none"}}"#,
            "Error: first_run_person_only: agents have none",
            Exit::Refused,
        ),
        (
            r#"{"error":{"code":"unknown_row","hint":"\"nope\" is not a row (unknown_row)"}}"#,
            r#"Error: "nope" is not a row (unknown_row)"#,
            Exit::Usage,
        ),
        (
            r#"{"error":{"code":"kessel_first_run_off","hint":"off"}}"#,
            "Error: kessel_first_run_off: off",
            Exit::Error,
        ),
        (
            r#"{"error":{"code":"unknown_focus_group","message":"none"}}"#,
            "Error: unknown_focus_group: none",
            Exit::NotFound,
        ),
        (
            r#"{"error":{"code":"ambiguous_focus_group","hint":""}}"#,
            "Error: ambiguous_focus_group",
            Exit::NotFound,
        ),
        (
            r#"{"error":{"hint":"Bad TOKEN"}}"#,
            "Error: Bad TOKEN",
            Exit::Refused,
        ),
        (
            r#"{"error":"role_required","message":"Voice only."}"#,
            "Error: role_required",
            Exit::Refused,
        ),
        (
            r#"{"error":"invalid or missing token"}"#,
            "Error: invalid or missing token",
            Exit::Refused,
        ),
        (
            r#"{"error":{"code":"store","hint":"disk full\twhile saving \\ the list"}}"#,
            "Error: store: disk full\twhile saving \\ the list",
            Exit::Error,
        ),
        (
            r#"{"state":"migrating"}"#,
            r#"Error: {"state":"migrating"}"#,
            Exit::Error,
        ),
        (r#"{"error":5}"#, r#"Error: {"error":5}"#, Exit::Error),
    ] {
        assert_eq!(
            refusal(body),
            (String::new(), format!("{line}\n"), exit),
            "{body}"
        );
    }
    assert_eq!(
        rendered(Sub::List, false, "<html>bad gateway</html>\n502"),
        (
            String::new(),
            format!("{NOT_UNDERSTOOD}<html>bad gateway</html>\n"),
            Exit::Error
        )
    );
    // 201 is not 200.
    assert_eq!(rendered(Sub::List, false, "Setup\n201").2, Exit::Error);
    let r = render(Sub::List, false, b"\n000x\n200", || false);
    assert_eq!(r.noted.as_deref(), Some(&b"\n000x"[..]));
    assert_eq!(render(Sub::List, false, b"\n000", || false).noted, None);
}

#[test]
fn what_raw_holds() {
    let answer = |status: u16, body: &[u8]| -> Result<Response, Error> {
        Ok(Response {
            status,
            body: body.to_vec(),
            channel: Channel::Tcp,
            permission: None,
            server_version: None,
        })
    };
    assert_eq!(held(&answer(200, b"a\n"), 0), b"a\n\n200");
    assert_eq!(held(&answer(200, b"a\0b"), 2), b"\n000\n000ab\n200");
    assert_eq!(held(&answer(204, b""), 0), b"\n204");
    let failed: Result<Response, Error> = Err(Error::Empty {
        endpoint: "/x".into(),
    });
    assert_eq!(held(&failed, 3), b"\n000");
}

fn old(sub: Sub, body: &str) -> (String, String, Exit) {
    let s = old_daemon(sub, body.as_bytes());
    (s.stdout, s.stderr, s.exit)
}

/// What `tests/cli/setup_dev_tools.sh` checks the shell's Python prints.
#[test]
fn an_older_daemon_s_list() {
    let rest = r#"{"id":"llm_sign_in","state":"todo","opens":"llm_sign_in"},{"id":"first_agent","state":"todo","opens":"add_workspace"}"#;
    let with = |dev: &str| {
        format!(r#"{{"enabled":true,"hidden":false,"finished":false,"rows":[{dev}{rest}]}}"#)
    };
    let (out, err, exit) = old(
        Sub::List,
        &with(r#"{"id":"dev_tools","state":"todo","opens":"dev_tools"},"#),
    );
    assert_eq!((err.as_str(), exit), ("", Exit::Ok));
    assert_eq!(
        out,
        format!("Setup: 0 of 3 done\n  [ ] Install developer tools ({APPLE_TOOLS})\n  [ ] Sign in to an AI tool\n  [ ] Make your first agent\n")
    );
    let (out, _, _) = old(
        Sub::List,
        &with(r#"{"id":"dev_tools","state":"todo","opens":null,"note":"sudo apt install git"},"#),
    );
    assert!(out.contains("  [ ] Install developer tools (run: sudo apt install git)\n"));
    let (out, _, _) = old(
        Sub::List,
        &with(r#"{"id":"dev_tools","state":"done","opens":"dev_tools"},"#),
    );
    assert!(out.starts_with("Setup: 1 of 3 done\n  [x] Install developer tools\n"));
    let (out, _, _) = old(
        Sub::List,
        &with(r#"{"id":"dev_tools","state":"todo","opens":"dev_tools","note":"installing"},"#),
    );
    assert!(
        out.contains("(The Apple installer is open. Follow it, and this step finishes by itself.)")
    );
    let (out, _, _) = old(
        Sub::Skip,
        r#"{"enabled":false,"hidden":true,"rows":[{"id":"x","state":"odd"},{"state":"skipped","id":7}],"kessel":{"name":"Kessel","path":"/k"},"offer":"meet_kessel"}"#,
    );
    assert_eq!(
        out,
        format!("Setup: 0 of 2 done\n  [?] x\n  [-] 7\n{HIDDEN}\n{NOT_ENABLED}\nKessel: Kessel at /k (k2 thread <its handle> to talk to it)\n{MEET_KESSEL}\n")
    );
}

#[test]
fn an_older_daemon_s_made_and_refusals() {
    let (out, err, exit) = old(
        Sub::Kessel,
        r#"{"workspaceId":"w","name":"Kessel","handle":"kessel","path":"/a/kessel","permissions":{"granted":["msg","read"],"left":[{"action":"x.write","reason":"why"}]}}"#,
    );
    assert_eq!((err.as_str(), exit), ("", Exit::Ok));
    assert_eq!(
        out,
        "Made Kessel: Kessel (kessel) at /a/kessel\nAllowed from the start: msg, read\nIt asks on a permission card for: x.write (why)\nKessel is starting and will say hello on its Thread: k2 thread kessel\n"
    );
    let (out, _, _) = old(Sub::ChiefOfStaff, r#"{"workspaceId":"w","name":"Ada"}"#);
    assert_eq!(
        out,
        "Made your Chief of Staff: Ada (None) at None\nTell your person on your Thread, and point them to Ada (k2 msg None).\n"
    );
    for (body, err, exit) in [
        (
            r#"{"error":{"code":"tool_not_signed_in","hint":"sign in"}}"#,
            "Error: tool_not_signed_in: sign in\n",
            Exit::Usage,
        ),
        (
            r#"{"error":{"code":"x_passport","hint":"has x_passport"}}"#,
            "Error: has x_passport\n",
            Exit::Refused,
        ),
        (r#"{"ok":true}"#, "Error: {\"ok\":true}\n", Exit::Error),
        (r#"[1]"#, "Error: [1]\n", Exit::Error),
        (r#"{"error":["a"]}"#, "Error: ['a']\n", Exit::Error),
        (
            "{bad",
            "Error: the daemon sent something k2 setup does not understand: {bad\n",
            Exit::Error,
        ),
    ] {
        assert_eq!(
            old(Sub::List, body),
            (String::new(), err.to_string(), exit),
            "{body}"
        );
    }
    // Where the Python raises: what it printed, then its traceback.
    for (body, printed, exception) in [
        (
            r#"{"error":{"code":5,"hint":"h"}}"#,
            "",
            "TypeError: 'in <string>' requires string as left operand, not int",
        ),
        (
            r#"{"error":{"code":"c","hint":["c"]}}"#,
            "Error: ['c']\n",
            "TypeError: can only concatenate str (not \"list\") to str",
        ),
        (
            r#"{"error":{"code":"c","hint":7}}"#,
            "",
            "TypeError: argument of type 'int' is not iterable",
        ),
        (
            r#"{"rows":5}"#,
            "",
            "TypeError: 'int' object is not iterable",
        ),
        (
            r#"{"rows":"ab"}"#,
            "",
            "AttributeError: 'str' object has no attribute 'get'",
        ),
        (
            r#"{"rows":[{"state":["x"]}]}"#,
            "Setup: 0 of 1 done\n",
            "TypeError: unhashable type: 'list'",
        ),
        (
            r#"{"rows":[],"kessel":[1]}"#,
            "Setup: 0 of 0 done\nThe app does not show setup lists on this server yet.\n",
            "AttributeError: 'list' object has no attribute 'get'",
        ),
    ] {
        let (out, err, exit) = old(Sub::List, body);
        let (o, e) = if printed.starts_with("Error:") {
            ("", printed)
        } else {
            (printed, "")
        };
        assert_eq!((out.as_str(), exit), (o, Exit::Error), "{body}");
        assert!(
            err.starts_with(&format!("{e}Traceback (most recent call last):\n")),
            "{body}: {err}"
        );
        assert!(err.ends_with(&format!("{exception}\n")), "{body}: {err}");
    }
    let (out, err, _) = old(
        Sub::Kessel,
        r#"{"workspaceId":"w","permissions":{"granted":[1]}}"#,
    );
    assert_eq!(out, "Made Kessel: None (None) at None\n");
    assert!(err.ends_with("TypeError: sequence item 0: expected str instance, int found\n"));
    let (_, err, _) = old(
        Sub::Kessel,
        r#"{"workspaceId":"w","permissions":{"left":["x"]}}"#,
    );
    assert!(err.contains("in <genexpr>\n"), "{err}");
}

#[test]
fn plain_hosts() {
    for h in ["127.0.0.1", "localhost", "box-1.example"] {
        assert!(plain_host(h), "{h}");
    }
    for h in ["", "::1", "[::1]", "a b", "a/b", "a\"b", "h:1"] {
        assert!(!plain_host(h), "{h}");
    }
}

#[test]
fn the_switches_pick_the_forms() {
    // Usage mistakes send nothing, so they go with the reads.
    let native = |words: &[&str]| match parse(words) {
        Ok(line) if line.sub.post() => WRITE_NATIVE,
        _ => READ_NATIVE,
    };
    assert_eq!(native(&[]), READ_NATIVE);
    assert_eq!(native(&["--bogus"]), READ_NATIVE);
    assert_eq!(native(&["skip", "x"]), WRITE_NATIVE);
}

/// `_cli_tool_id_for_args setup <words>`: the locked tool is
/// `chief-of-staff` first, or first after the leading flags.
#[test]
fn the_locked_tool_after_leading_flags() {
    for words in [
        &["chief-of-staff"][..],
        &["--json", "chief-of-staff"],
        &["--tool", "claude", "chief-of-staff"],
        &["--name", "x", "--json", "chief-of-staff", "--about", "y"],
        &["--", "chief-of-staff"],
        &["chief-of-staff", "--help"],
    ] {
        assert!(names_locked_tool(words), "{words:?}");
    }
    for words in [
        &[][..],
        &["hide"],
        &["--json"],
        &["--json", "hide", "chief-of-staff"],
        &["--help", "chief-of-staff"],
        &["--tool", "chief-of-staff"],
        &["--", "--json", "chief-of-staff"],
    ] {
        assert!(!names_locked_tool(words), "{words:?}");
    }
    let policy = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cli-tools-policy.sh"),
    )
    .unwrap();
    for line in [
        "                -h|--help) break ;;",
        "                --) shift; break ;;",
        "                        setup:--tool|setup:--name|setup:--handle|setup:--about|setup:--focus-group)",
        "                chief-of-staff) echo \"agent-harness\" ;;",
    ] {
        assert!(policy.lines().any(|l| l == line), "_cli_tool_id_for_args moved: no line {line:?}");
    }
}

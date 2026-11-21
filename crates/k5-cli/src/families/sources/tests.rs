use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/sources.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn python_after(after: &str) -> Vec<String> {
    let text = source();
    let at = text.find(after).unwrap_or_else(|| panic!("no {after:?}"));
    let body = &text[at..];
    let start = body.find("python3 -c '").expect("its python3") + "python3 -c '".len();
    let body = &body[start..];
    let end = body.find("\n'").expect("its end");
    body[..end].split('\n').map(str::to_string).collect()
}

fn w(list: &[&str]) -> Vec<String> {
    list.iter().map(|x| x.to_string()).collect()
}

/// Each line the words and the calls follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"        ""|help|--help|-h) _sources_usage; return 0 ;;"#,
        r#"        _sources_verb_help "$sub" || _sources_usage"#,
        r#"    for a in "$@"; do [ "$a" = "--json" ] && _SRC_JSON=1; done"#,
        r#"        person_only|owner_only|vault_confirm_required|confirm_required|lan_owner_only|field_person_only|secret_not_usable|forbidden)"#,
        r#"            case "$code" in person_only|vault_confirm_required|confirm_required|field_person_only) person_code=1 ;; esac"#,
        r#"        source_not_found|trigger_not_found|removed) exit 4 ;;"#,
        r#"        echo "Error: no answer from the K2 daemon (status ${status:-none})" >&2"#,
        r#"    print("Error: daemon returned: %s" % raw[:200])"#,
        r#"            --*) ;;"#,
        r#"    echo "Error: usage: k2 sources $verb <name>" >&2"#,
        r#"        *[!0-9smhd]*|"") echo "Error: $flag takes a duration like 30s, 5m, 2h, 1d" >&2; exit 2 ;;"#,
        r#"            *[smhd]) v=$(( now - $(_sources_secs "$v" "--$t") )) ;;"#,
        r#"            *[!0-9]*) echo "Error: --$t takes a duration (6h) or epoch seconds" >&2; exit 2 ;;"#,
        r#"            --from) _need_value "$@"; from="${2:-}"; shift 2 ;;"#,
        r#"            *) if [ -z "$name" ]; then name="$1"; elif [ -z "$field" ]; then field="$1"; fi; shift ;;"#,
        r#"    _sources_call GET /cli/sources/export "name=$(urlencode "$name")""#,
        r#"    _sources_print_json "$_SRC_BODY""#,
        r#"    echo "$name: paused""#,
        r#"    echo "$name: active""#,
        r#"    echo "$name: unmuted""#,
        r#"    echo "$name: muted for $for_""#,
        r#"b = {"name": os.environ["K2_SRC_NAME"], "forSecs": int(os.environ["K2_SRC_FOR"])}"#,
        r#"        if [ -n "${K5_HOOK_SOCK:-}${K2_HOOK_SOCK:-}${K2SO_HOOK_SOCK:-}" ] || [ ! -t 0 ]; then"#,
        r#"            echo "Error: k2 sources remove needs --yes here" >&2"#,
        r#"        case "$ans" in y|Y|yes) ;; *) echo "Not removed." >&2; exit 1 ;; esac"#,
        r#"    echo "Removed $name (name held for 7 days).""#,
        r#"            *) echo "Error: unknown flag '$1'" >&2; exit 2 ;;"#,
        r#"    [ -n "$workspace" ] || workspace="$(pwd)""#,
        r#"    raw="$(_compute_send POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=gardens_manage K2_CJ_value_b="$([ "$state" = on ] && echo 1 || echo 0)")" tcp)""#,
    ] {
        assert!(text.contains(shell), "sources.sh no longer has: {shell}");
    }
}

/// The renderer's lines where this says Python raises.
#[test]
fn the_renderer_s_lines() {
    let py = python_after("_sources_render() {");
    let at = |n: u32| py[n as usize - 1].trim().to_string();
    assert_eq!(at(4), "d = json.load(sys.stdin)");
    assert_eq!(at(line::AGO), "s = max(0, now - t)");
    assert_eq!(at(line::S_COUNT), "if not s.get(\"count\"):");
    assert_eq!(at(line::S_LEVEL), "if s.get(\"level\"):");
    assert!(at(line::S_SOFT).starts_with("return \"soft %s %d/%d\""));
    assert!(at(line::S_RETRY).starts_with("return \"retrying"));
    assert_eq!(at(line::LINE_C), "c = v.get(\"counts\") or {}");
    assert!(at(line::LINE_COUNTS).starts_with("counts = "));
    assert!(at(line::LINE_AGO).contains("ago(v[\"serverNow\"]"));
    assert_eq!(at(line::LINE_JOIN), "return \" \u{b7} \".join(bits)");
    assert!(at(line::CARD_HEAD).starts_with("print(\"%s %s (%s)\""));
    assert!(at(line::CARD_GAP).starts_with("print(\"  gap:"));
    assert!(at(line::CARD_ITEM).starts_with("print(\"  %s %-24s"));
    assert_eq!(at(line::LIST_ROWS), "rows = d.get(\"sources\") or []");
    assert!(at(line::LIST_ROW).starts_with("print(\"%s %-24s %s%s\""));
    assert_eq!(at(line::SHOW_SOURCE), "s = d[\"source\"]");
    assert!(at(line::SHOW_STATUS).starts_with("% (s[\"status\"]"));
    assert!(at(line::ST_LINE).starts_with("% (\"running\""));
    assert!(at(line::ST_GAP_LINE).starts_with("print(\"  last gap"));
    assert!(at(line::H_LINE).starts_with("print(\"%d\\t%s%s\\t%s"));
    assert!(at(line::H_POINT).starts_with("print(\"%d\\t%s\" % (p[\"t\"]"));
}

#[test]
fn the_help_pages() {
    assert_eq!(parse(&[]), Parsed::Help(String::new()));
    assert_eq!(parse(&["show", "x", "-h"]), Parsed::Help("show".into()));
    assert_eq!(help_page(""), crate::text::help("sources").unwrap());
    assert_eq!(help_page("ls"), crate::text::help("sources").unwrap());
    assert_eq!(
        help_page("list"),
        crate::text::help("sources/read").unwrap()
    );
    assert!(help_page("add").starts_with(&format!("{CLI} sources add --from")));
    assert!(help_page("try").ends_with(crate::text::help("sources/_flags").unwrap()));
}

#[test]
fn the_verbs_words() {
    assert_eq!(
        ask("show", &w(&["--json", "-x"]), 0),
        Ok(Ask::Get {
            route: "show",
            query: "name=-x".into(),
            kind: "show"
        })
    );
    assert_eq!(
        ask("read", &w(&["--json"]), 0),
        Err(Stop::Usage(format!(
            "Error: usage: {CLI} sources read <name>"
        )))
    );
    assert_eq!(
        ask(
            "history",
            &w(&["a", "health", "x", "--item", "k é", "--from", "100", "--to", "2m"]),
            1000
        ),
        Ok(Ask::Get {
            route: "history",
            query: "name=a&field=health&item=k%20%C3%A9&from=100&to=880".into(),
            kind: "history"
        })
    );
    assert_eq!(
        ask("history", &w(&["a", "--to"]), 0),
        Err(Stop::Shift("--to".into()))
    );
    assert_eq!(
        ask("history", &w(&["a", "f", "--from", "5x"]), 0),
        Err(Stop::Usage(
            "Error: --from takes a duration (6h) or epoch seconds".into()
        ))
    );
    for odd in ["h", "08h", "1d5h", "5xs", "-5h"] {
        assert_eq!(
            ask("history", &w(&["a", "f", "--from", odd]), 0),
            Err(Stop::Shell),
            "{odd}"
        );
    }
    assert_eq!(
        ask("mute", &w(&["a", "--for", "2h", "--reason", "r"]), 0),
        Ok(Ask::Post {
            route: "mute",
            body: r#"{"name": "a", "forSecs": 7200, "reason": "r"}"#.into(),
            done: "a: muted for 2h".into()
        })
    );
    assert_eq!(
        ask("mute", &w(&["a", "--for", "05s"]), 0).map(|a| match a {
            Ask::Post { body, .. } => body,
            other => panic!("{other:?}"),
        }),
        Ok(r#"{"name": "a", "forSecs": 5}"#.into())
    );
    assert_eq!(
        ask("mute", &w(&["a", "--for", "5x"]), 0),
        Err(Stop::Usage(
            "Error: --for takes a duration like 30s, 5m, 2h, 1d".into()
        ))
    );
    assert_eq!(ask("mute", &w(&["a", "--for", "s"]), 0), Err(Stop::Shell));
    assert_eq!(
        ask("pending", &w(&["x"]), 0),
        Err(Stop::Usage("Error: unknown flag 'x'".into()))
    );
    assert_eq!(
        ask("access", &w(&["on", "off"]), 0),
        Err(Stop::Usage(format!(
            "usage: {CLI} sources access on|off [--workspace <ws>]"
        )))
    );
    assert_eq!(
        ask("access", &w(&["on", "--workspace", ""]), 0),
        Err(Stop::Usage(
            "usage: --workspace <workspace name or path>".into()
        ))
    );
    assert_eq!(
        ask("propose", &w(&[]), 0),
        Err(Stop::Usage("Error: --name is required".into()))
    );
    assert_eq!(ask("nosuch", &w(&[]), 0), Err(Stop::Shell));
}

#[test]
fn a_refusal() {
    let f = fail(
        "403",
        br#"{"error":{"code":"person_only","hint":"ask"}}"#,
        false,
    );
    assert_eq!(f.exit, Exit::Refused);
    assert!(f
        .stderr
        .starts_with("Error: person_only: ask\nThis needs a person"));
    let f = fail(
        "403",
        br#"{"error":{"code":"forbidden","hint":"no"}}"#,
        false,
    );
    assert_eq!(f.stderr, "Error: forbidden: no\n");
    let f = fail("404", br#"{"error":{"code":"source_not_found"}}"#, true);
    assert_eq!(f.exit, Exit::NotFound);
    assert_eq!(
        f.stdout,
        "{\n  \"error\": {\n    \"code\": \"source_not_found\"\n  }\n}\n"
    );
    let f = fail("500", b"oops", false);
    assert_eq!(f.stderr, "Error: daemon returned: oops\n");
    let f = fail("000", b"", false);
    assert_eq!(
        f.stderr,
        format!("Error: no answer from the {PRODUCT_NAME} daemon (status 000)\n")
    );
}

#[test]
fn the_renderer() {
    let r = render(
        "list",
        br#"{"sources":[{"name":"web","status":"active","triggers":2,"keep":"states","view":{"display":"ok","serverNow":1000,"updatedAt":900,"counts":{"ok":3,"crit":1}}}]}"#,
    );
    assert_eq!(
        r.stdout,
        format!(
            "\u{25cf} {:<24} ok \u{b7} 3 ok, 1 crit \u{b7} updated 1 min ago \u{b7} 2 triggers \u{b7} states only\n",
            "web"
        )
    );
    let r = render(
        "history",
        br#"{"field":"count","points":[{"t":1,"v":null},{"t":2.5,"v":3}]}"#,
    );
    assert_eq!(r.stdout, "1\tgap\n2\t3\n");
    let r = render(
        "read",
        br#"{"name":"n","label":"L","display":5,"serverNow":1}"#,
    );
    assert_eq!(r.exit, Exit::Error);
    assert!(
        r.stderr
            .ends_with("TypeError: sequence item 0: expected str instance, int found\n"),
        "{}",
        r.stderr
    );
}

#[test]
fn simulate_and_refresh_words() {
    assert_eq!(
        ask("simulate", &w(&["web", "crit", "--for", "2h"]), 0),
        Ok(Ask::Simulate {
            body: r#"{"name": "web", "state": "crit", "forSecs": 7200}"#.into(),
            json: false
        })
    );
    assert_eq!(
        ask("simulate", &w(&["-x", "crit"]), 0),
        Err(Stop::Usage(format!(
            "usage: {CLI} sources simulate <name> ok|warn|crit|stale|unknown|slow|off [--for 1h]"
        )))
    );
    assert_eq!(
        ask("simulate", &w(&["web", "ok", "--for"]), 0),
        Err(Stop::Usage("usage: --for 1h".into()))
    );
    assert_eq!(
        ask("simulate", &w(&["web", "ok", "--for", "5q"]), 0),
        Err(Stop::Usage(
            "usage: bad duration '5q' (use 30s, 5m, 2h, 1d)".into()
        ))
    );
    assert_eq!(
        ask("simulate", &w(&["web", "ok", "--for", "xs"]), 0),
        Err(Stop::Shell)
    );
    assert!(ask("simulate", &w(&["web", "ok", "--for", "5é"]), 0).is_err());
    assert_eq!(
        ask("refresh", &w(&["--wait", "web"]), 0),
        Ok(Ask::Refresh {
            name: "web".into(),
            wait: true
        })
    );
    assert_eq!(
        utc_minutes(&Value::Int("3660".into())),
        Ok("01:01 UTC".into())
    );
    assert_eq!(utc_minutes(&Value::Float(-0.5)), Ok("23:59 UTC".into()));
}

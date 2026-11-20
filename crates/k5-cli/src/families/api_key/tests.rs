use super::*;
use crate::families::skin_token::tests::{k2, quoted_python, source};

/// `cmd_api_key`, as the shell has it.
fn cmd_api_key() -> String {
    let text = source("talk-and-workspace.sh");
    let start = text.find("cmd_api_key() {").expect("cmd_api_key");
    let end = start + text[start..].find("\n}\n").expect("its end");
    text[start..end].to_string()
}

/// `echo "<text>" >&2` as bash prints it.
fn echoed(line: &str) -> String {
    line.replace('\\', "\\\\")
}

#[test]
fn the_shell_s_lines() {
    let f = cmd_api_key();
    let usage = &f[f
        .find("        *)\n            echo \"Usage: k2 api-key <")
        .expect("the usage arm")..];
    let lines: Vec<&str> = usage
        .lines()
        .skip(1)
        .take_while(|l| l.trim_start().starts_with("echo "))
        .collect();
    assert_eq!(lines.len(), USAGE.len());
    for (shell, ours) in lines.iter().zip(USAGE) {
        assert_eq!(
            *shell,
            format!("            echo \"{}\" >&2", echoed(&k2(ours)))
        );
    }
    assert!(usage.contains(&format!(
        "{}\n            exit 1 ;;",
        lines[lines.len() - 1]
    )));
    for shell in [
        r#"                    --label)        label="${2:-}"; shift 2 ;;"#,
        r#"                    *)              echo "Unknown flag: $1" >&2; exit 1 ;;"#,
        r#"                [ "$1" != "--label" ] && [ "$1" != "--status" ] && other_flags=1"#,
        r#"                _cli_read_secret_stdin "Paste the LLM API key: ""#,
        r#"                | cli_post_json_stdin "/cli/api-keys/create" \"#,
        r#"            cli_request "/cli/api-keys/list" \"#,
        r#"            cli_post_json "/cli/api-keys/revoke" "$body" \"#,
        r#"            cli_post_json "/cli/api-keys/disable" "$body" \"#,
        r#"            cli_post_json "/cli/api-keys/enable" "$body" \"#,
        r#"                hire_confirm="$(_vault_confirm_token hire-key-mint "" "API key that can hire agents")" || exit 3"#,
        r#"        hire-ceiling|app-access)"#,
        r#"                    --hire)         hire=1; shift ;;"#,
        r#"                    --app-login)    h_apps="${h_apps}${2:-}"$'\n'; shift 2 ;;"#,
        r#"                    --app-login) c_apps="${c_apps}${2:-}"$'\n'; shift 2 ;;"#,
        r#"                    --none)      c_none=1; shift ;;"#,
        r#"                    *)           echo "Unknown flag: $1" >&2; exit 2 ;;"#,
        r#"            if [ -z "$id" ] || [ "${id#-}" != "$id" ]; then"#,
        r#"            c_resp="$(printf '%s' "$c_body" | cli_post_json_stdin "/cli/api-keys/hire-ceiling")""#,
        r#"            if [ "$c_code" = "vault_confirm_required" ]; then"#,
        r#"                c_tok="$(_vault_confirm_token hire-key-mint "" "Widen an API key's App access")" || exit 3"#,
        r#"                c_body="$(_with_confirm_token "$c_body" "$c_tok")""#,
        r#"                    if [ -n "$ak_body" ] && [ -n "$hire_confirm" ]; then _with_confirm_token "$ak_body" "$hire_confirm"; else printf '%s' "$ak_body"; fi; } \"#,
    ] {
        assert!(
            f.lines().any(|l| l == shell),
            "cmd_api_key no longer has {shell:?}"
        );
    }
    for l in [
        STATUS_ONLY,
        BOTH_KEYS,
        NO_KEY,
        STORE_NOW,
        CEILING_BOTH,
        CEILING_NEITHER,
        LIMITS_NEED_HIRE,
    ] {
        assert!(f.contains(l), "{l:?}");
    }
    for v in [IdVerb::Revoke, IdVerb::Disable, IdVerb::Enable] {
        assert!(f.contains(&k2(v.usage())), "{v:?}");
        let (yes, no) = v.lines();
        assert!(f.contains(&k2(yes)) && f.contains(no), "{v:?}");
    }
    assert!(f.contains(&NO_GRANT[2].replace('\'', "\\x27")));
    assert!(f.contains(&format!("echo \"{}\" >&2", k2(CEILING_USAGE))));
    for (_, key) in NUMBERS {
        assert!(f.contains(&format!("\"{key}\"")), "{key}");
    }
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains(r#"        api-key) echo "api-key" ;;"#));
    assert!(policy.contains(&format!("    echo \"{ARGV_WARNING}\" >&2\n")));
    let router = source("router.sh");
    assert!(router.contains("    api-key)\n        # P3a (sandbox) \u{2014} owner-only API-key auth-tier management for /v1/*.\n        shift; cmd_api_key \"$@\"\n"));
}

#[test]
fn the_status_python() {
    let py = quoted_python("talk-and-workspace.sh", "cmd_api_key() {", 1);
    let at = |n: u32| py[n as usize - 1].trim().to_string();
    assert_eq!(at(line::status::IN), r#"if "error" in d:"#);
    assert_eq!(
        at(line::status::INDEX),
        r#"print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)"#
    );
    assert!(at(line::status::ID).starts_with(r#"print("Status key created (id: %s)."#));
}

#[test]
fn the_words() {
    assert_eq!(parse(&["list"]), Some(Ask::List));
    assert_eq!(
        parse(&["create", "--x", "--hire"]),
        Some(Ask::Stop(vec!["Unknown flag: --x".into()], Exit::Error))
    );
    let Some(Ask::Create(c)) = parse(&["create", "--hire", "--link", "--app-login", "a:b"]) else {
        panic!()
    };
    assert!(c.hire && c.link && !c.retire && c.apps == "a:b\n");
    // Python's int() reads other digits than ASCII: the shell's.
    assert_eq!(parse(&["create", "--hire", "--max-live", "\u{663}"]), None);
    assert!(parse(&["create", "--max-live", "\u{663}"]).is_some());
    let usage = |l: &str| Some(Ask::Stop(vec![l.to_string()], Exit::Usage));
    assert_eq!(parse(&["hire-ceiling"]), usage(CEILING_USAGE));
    assert_eq!(parse(&["app-access", "-k1"]), usage(CEILING_USAGE));
    assert_eq!(parse(&["hire-ceiling", "k1"]), usage(CEILING_NEITHER));
    assert_eq!(
        parse(&["hire-ceiling", "k1", "--none", "--app-login", ""]),
        usage(CEILING_BOTH)
    );
    assert_eq!(
        parse(&["hire-ceiling", "k1", "--x"]),
        usage("Unknown flag: --x")
    );
    assert_eq!(
        parse(&["hire-ceiling", "k1", "--app-login"]),
        Some(Ask::Stop(vec![], Exit::Error))
    );
    assert_eq!(
        parse(&["app-access", "k1", "--none"]),
        Some(Ask::Ceiling(Ceiling {
            id: "k1".into(),
            apps: String::new()
        }))
    );
    assert_eq!(
        parse(&["create", "--label"]),
        Some(Ask::Stop(vec![], Exit::Error))
    );
    assert_eq!(
        parse(&["revoke"]),
        Some(Ask::Stop(vec![IdVerb::Revoke.usage().into()], Exit::Error))
    );
    assert_eq!(
        parse(&["disable", "k1", "x"]),
        Some(Ask::Id(IdVerb::Disable, "k1".into()))
    );
    let Some(Ask::Create(c)) = parse(&["create", "--status", "--label", "m"]) else {
        panic!()
    };
    assert!(c.status && !c.other_flags && c.label == "m");
    let Some(Ask::Create(c)) = parse(&["create", "--reach", "x", "--reach", ""]) else {
        panic!()
    };
    assert!(!c.hire_limits && c.other_flags);
    let Some(Ask::Create(c)) = parse(&["create", "--app-login", ""]) else {
        panic!()
    };
    assert!(c.hire_limits);
    assert!(matches!(parse(&[]), Some(Ask::Stop(l, Exit::Error)) if l.len() == USAGE.len()));
}

#[test]
fn the_create_body() {
    let c = Create {
        label: "wiki".into(),
        provider: " ".into(),
        base_url: "u ".into(),
        workspaces: " a, ,b ".into(),
        allow: ",".into(),
        ..Create::default()
    };
    assert_eq!(
        create_body(&c, "  sk-x\t").unwrap(),
        r#"{"label": "wiki", "anthropicKey": "sk-x", "baseUrl": "u ", "workspaces": ["a", "b"], "allow": []}"#
    );
    let c = Create {
        workspaces: " * ".into(),
        ..Create::default()
    };
    assert_eq!(
        create_body(&c, "").unwrap(),
        r#"{"label": "", "workspaces": "*"}"#
    );
    let limits = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(f, v)| (f.to_string(), v.to_string()))
            .collect()
    };
    let c = Create {
        hire: true,
        retire: true,
        limit_values: limits(&[
            ("--max-live", " +0_07 "),
            ("--max-per-day", "-0"),
            ("--hire-db", " read "),
            ("--link-targets", "a, ,b"),
            ("--folder", " "),
        ]),
        apps: " x :self, ,r: \n\n y\r::m\n".into(),
        ..Create::default()
    };
    assert_eq!(
        create_body(&c, "").unwrap(),
        concat!(
            r#"{"label": "", "hire": {"maxMembersPerDay": 0, "maxLiveMembers": 7, "db": "read", "#,
            r#""linkTargets": ["a", "b"], "retire": true, "apps": [{"app": "x ", "rooms": ["self", "r"]}, "#,
            r#"{"app": "y"}, {"app": "", "roles": ["m"]}]}}"#
        )
    );
    let c = Create {
        hire: true,
        ..Create::default()
    };
    assert_eq!(create_body(&c, "").unwrap(), r#"{"label": "", "hire": {}}"#);
    for bad in ["1e3", "0x1", "1__0", "_1", "1_", "+", "- 1"] {
        let c = Create {
            hire: true,
            limit_values: limits(&[("--max-per-team", "1"), ("--max-live", bad)]),
            ..Create::default()
        };
        assert_eq!(
            create_body(&c, ""),
            Err("Error: maxLiveMembers must be a number".to_string()),
            "{bad}"
        );
    }
    assert_eq!(
        ceiling_body(&Ceiling {
            id: "k\u{e9}".into(),
            apps: "a:,:\n".into()
        }),
        r#"{"id": "k\u00e9", "apps": [{"app": "a", "rooms": []}]}"#
    );
}

#[test]
fn the_hire_ceiling_python() {
    let body = quoted_python("talk-and-workspace.sh", "cmd_api_key() {", 4);
    assert_eq!(body[1], "apps=[]");
    let code = quoted_python("talk-and-workspace.sh", "cmd_api_key() {", 5);
    assert_eq!(
        code[3],
        r#"    print(e.get("code","") if isinstance(e,dict) else "")"#
    );
    let py = quoted_python("talk-and-workspace.sh", "cmd_api_key() {", 6);
    let at = |n: u32| py[n as usize - 1].trim().to_string();
    assert_eq!(
        at(line::ceiling::APPS),
        r#"apps=(d.get("hireLimits") or {}).get("apps") or []"#
    );
    assert_eq!(at(line::ceiling::FOR), "for a in apps:");
    assert_eq!(at(line::ceiling::ROLES), r#"roles=a.get("roles") or []"#);
    assert!(at(line::ceiling::PRINT)
        .starts_with(r#"print("  %s: rooms %s, %s" % (a.get("app"), ",".join("#));
}

#[test]
fn the_hire_ceiling_reader() {
    assert_eq!(
        ceiling_reader("").stderr,
        "Error: no answer from the daemon\n"
    );
    let r = ceiling_reader(r#"{"error":{"code":"c","hint":"h"}}"#);
    assert_eq!((r.stderr.as_str(), r.exit), ("Error: c: h\n", Exit::Error));
    let r = ceiling_reader(r#"{"error":"","id":"k","change":"raise"}"#);
    assert_eq!(
        r.stdout,
        "App access widened for key k.\nNo App access: agents hired with this key get no App login.\n"
    );
    let r = ceiling_reader(
        r#"{"id":"k","hireLimits":{"apps":[{"app":"a","rooms":["r","s"],"roles":["x"]},{"rooms":[]}]}}"#,
    );
    assert_eq!(
        r.stdout,
        "App access updated for key k.\n  a: rooms r,s, up to role x\n  None: rooms , no role (Thread only in those rooms)\n"
    );
    let r = ceiling_reader("[]");
    assert_eq!(r.exit, Exit::Error);
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    let r = ceiling_reader(r#"{"hireLimits":{"apps":[{"rooms":[1]}]}}"#);
    assert_eq!(r.stdout, "App access updated for key .\n");
    assert!(r
        .stderr
        .ends_with("TypeError: sequence item 0: expected str instance, int found\n"));
    assert_eq!(
        error_code(r#"{"error":{"code":"vault_confirm_required"}}"#),
        CONFIRM_REQUIRED
    );
    assert_eq!(error_code(r#"{"error":{"code":null}}"#), "None");
    assert_eq!(error_code(r#"{"error":"vault_confirm_required"}"#), "");
    assert_eq!(error_code("[]"), "");
    assert_eq!(error_code("x"), "");
}

#[test]
fn the_readers() {
    let r = list_reader("");
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            "Error: Expecting value: line 1 column 1 (char 0)\n",
            Exit::Error
        )
    );
    let r = list_reader(
        r#"{"keys":[{"id":"k1","label":"","anthropicKeySet":true,"provider":"openai","scope":"*","baseUrl":"u","capabilities":{"hostSessions":true,"hire":1},"hireLimits":{"apps":[{"app":"a","rooms":["r"],"roles":["x"]}]}}]}"#,
    );
    assert_eq!(
        r.stdout,
        "k1  [active]  -  openai-key  scope=*  base-url=u  caps=host-sessions,hire\n    App access: a rooms=r up-to-role=x\n"
    );
    let r = list_reader(r#"{"keys":[{"id":"a"},5]}"#);
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str()),
        (
            "a  [active]  -  no-llm-key  scope=  caps=none\n",
            "Error: 'int' object has no attribute 'get'\n"
        )
    );
    assert_eq!(list_reader(r#"{"error":"no"}"#).stderr, "Error: no\n");
    assert_eq!(list_reader("{}").stdout, "No API keys.\n");
    assert_eq!(
        id_reader(r#"{"success":1}"#, IdVerb::Revoke).stdout,
        "API key revoked.\n"
    );
    assert_eq!(
        id_reader("\"error\"", IdVerb::Enable).stderr,
        "Error: string indices must be integers, not 'str'\n"
    );
    let r = create_reader(
        r#"{"id":"k","key":"k2sk_x","capabilities":{"db":true}}"#,
        "",
    );
    assert_eq!(
        r.stdout,
        format!("API key created (id: k).\nCapabilities: db\n\n  k2sk_x\n\n{STORE_NOW}\n\n")
    );
    assert_eq!(r.stderr, format!("{}\n", NO_GRANT.join("\n")));
    let r = create_reader(r#"{"error":{"code":"c"}}"#, "*");
    assert_eq!((r.stderr.as_str(), r.exit), ("Error: c: \n", Exit::Error));
    assert_eq!(
        status_reader("x").stderr,
        "Error: no answer from the daemon\n"
    );
    assert_eq!(status_reader(r#"{"error":"nope"}"#).stderr, "Error: nope\n");
}

#[test]
fn the_lines_say_the_brand_s_name() {
    let name = k2_client::brand::CLI_NAME;
    let disabled = IdVerb::Disable.lines().0;
    for l in [
        USAGE[0],
        USAGE[USAGE.len() - 2],
        CEILING_USAGE,
        IdVerb::Revoke.usage(),
        disabled,
    ] {
        assert!(l.contains(&format!("{name} api-key")), "{l}");
        if name != "k2" {
            assert!(!l.contains("k2 "), "{l}");
        }
    }
}

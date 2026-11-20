use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python of a `python3 - <<'PYEOF'` heredoc in `file`, after `after`,
/// its lines as `python3 -` numbers them (line 1 is the heredoc's first).
pub fn heredoc(file: &str, after: &str) -> Vec<String> {
    let text = source(file);
    let body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"))
        .1;
    let start = body.find("<<'PYEOF'\n").expect("a heredoc") + "<<'PYEOF'\n".len();
    let py = &body[start..];
    let end = py.find("\nPYEOF\n").expect("the heredoc's end");
    py[..end].split('\n').map(str::to_string).collect()
}

/// Each line the frame and the verbs follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let text = source("domains-certs.sh");
    for shell in [
        r#"    RESP="${RESP:-}" AS_JSON="${AS_JSON:-false}" D_VERB="${D_VERB:-}" python3 - <<'PYEOF'"#,
        r#"    RESP=$(cli_send cli_request "/cli/domains") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/domains" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/domains/remove" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/domains/refresh" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/domains/names" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/domains/names/remove" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_request "/cli/certs") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/certs/issue" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/certs/renew" "$body") || RESP="""#,
        r#"    body=$(D_HOST="$hostname" python3 -c 'import json,os; print(json.dumps({"hostname": os.environ["D_HOST"]}))')"#,
        r#"    body=$(D_APEX="$apex" python3 -c 'import json,os; print(json.dumps({"apex": os.environ["D_APEX"]}))')"#,
        r#"print(json.dumps({"apex": a} if a else {}))')"#,
        r#"        remove|rm|delete|del) shift; cmd_domain_remove "$@" ;;"#,
        r#"        refresh|check) shift; cmd_domain_refresh "$@" ;;"#,
        r#"        remove|rm|delete|del) shift; cmd_domain_name_remove "$@" ;;"#,
        r#"        --help|-h|'') cmd_help_cert; exit 0 ;;"#,
        r#"        --help|-h|'') cmd_help_domain; exit 0 ;;"#,
        r#"        --help|-h|'') cmd_help_domain_name; exit 0 ;;"#,
        r#"                [ $# -ge 2 ] || { _dom_err "usage" "--role requires mail|publish|direct|other|app:<name>"; exit 2; }"#,
        r#"            _dom_err "usage" "unknown command '${1:-}' — see k2 cert --help (box inventory; not k2 hostmail cert)""#,
        r#"            _dom_err "usage" "unknown command '${1:-}' — see k2 domain --help (not k2 hostmail domain)""#,
        r#"            _dom_err "usage" "unknown name command '${1:-}' — use add | remove (see k2 domain name --help)""#,
        r#"        _dom_err "usage" "domain add requires an <apex> (example.com) — this is not k2 hostmail domain""#,
        r#"        _dom_err "usage" "cert issue requires an attached <hostname>""#,
        r#"        _dom_err "usage" "cert renew requires an attached <hostname>""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "domains-certs.sh no longer has {shell:?}"
        );
    }
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains("        domain|cert) echo \"dns\" ;;\n"));
    assert!(policy.contains("        dns) echo \"dns\" ;;\n"));
    let cell = source("cell-socket.sh");
    assert!(cell.contains(
        "        /cli/domains|/cli/domains/names|/cli/domains/names/remove|/cli/certs|/cli/certs/*) return 0 ;;"
    ));
}

/// The Python's lines the readers raise at, as `python3 -` numbers them.
#[test]
fn the_python_s_lines() {
    let lines = heredoc("domains-certs.sh", "_dom_handle_resp() {");
    let at = |n: u32| lines[n as usize - 1].trim().to_string();
    for (n, text) in [
        (line::DIE_CODE, "if code in GATED_CODES:"),
        (line::ERR, "err = d.get(\"error\")"),
        (line::DIE_ERR, "die(err)"),
        (
            line::PENDING_NS,
            "ns = [str(n) for n in (row.get(\"nameservers\") or []) if n]",
        ),
        (line::ADD_PENDING, "pending_lines(row)"),
        (
            line::REFRESH_ROWS,
            "rows = [r for r in (d.get(\"domains\") or []) if isinstance(r, dict)]",
        ),
        (line::REFRESH_PENDING, "pending_lines(row)"),
        (line::LIST_FOR, "for row in domains:"),
        (line::LIST_PENDING, "pending_lines(row)"),
        (line::LIST_NAMES, "for n in row.get(\"names\") or []:"),
        (
            line::LIST_CERT,
            "cert = (n.get(\"cert\") or {}).get(\"state\") or \"missing\"",
        ),
        (line::CERTS_FOR, "for c in certs:"),
        (
            line::CERTS_CERT,
            "cert = (c.get(\"cert\") or {}).get(\"state\") or \"missing\"",
        ),
        (38, "die({\"code\": \"daemon_unreachable\","),
    ] {
        assert_eq!(at(n), text, "line {n}");
    }
    assert_eq!(at(39), format!("\"hint\": \"{UNREACHABLE}\"}})"));
}

fn shown(stdout: &str, stderr: &str, exit: Exit) -> Shown {
    Shown {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit,
    }
}

#[test]
fn the_first_steps() {
    let unreachable =
        format!("{{\"error\":{{\"code\":\"daemon_unreachable\",\"hint\":\"{UNREACHABLE}\"}}}}\n");
    assert_eq!(
        read("", false, "list"),
        shown("", &unreachable, Exit::Error)
    );
    assert_eq!(
        read(" \u{a0}\n", true, "list"),
        shown("", &unreachable, Exit::Error)
    );
    assert_eq!(
        read("nope 'é'", false, "list"),
        shown(
            "",
            "{\"error\":{\"code\":\"bad_response\",\"hint\":\"daemon returned an unparseable response: \\\"nope 'é'\\\"\"}}\n",
            Exit::Error
        )
    );
    for (body, line, exit) in [
        (
            r#"{"error":{"code":"gated","hint":"no"}}"#,
            r#"{"error":{"code":"gated","hint":"no"}}"#,
            Exit::Refused,
        ),
        (
            r#"{"error":{"code":"invalid_role"}}"#,
            r#"{"error":{"code":"invalid_role","hint":""}}"#,
            Exit::Usage,
        ),
        (
            r#"{"error":{"hint":"x"}}"#,
            r#"{"error":{"code":"error","hint":"x"}}"#,
            Exit::Error,
        ),
        (
            r#"{"error":"plain é"}"#,
            r#"{"error":{"code":"error","hint":"plain é"}}"#,
            Exit::Error,
        ),
        (
            r#"{"error":[1,true]}"#,
            r#"{"error":{"code":"error","hint":"[1, True]"}}"#,
            Exit::Error,
        ),
        (
            r#"{"error":false}"#,
            r#"{"error":{"code":"error","hint":"False"}}"#,
            Exit::Error,
        ),
        (
            r#"{"error":{"code":null,"hint":2}}"#,
            r#"{"error":{"code":null,"hint":2}}"#,
            Exit::Error,
        ),
        (
            r#"{"ok":false,"x":1}"#,
            r#"{"error":{"code":"error","hint":"unexpected daemon response: {\"ok\":false,\"x\":1}"}}"#,
            Exit::Error,
        ),
    ] {
        assert_eq!(
            read(body, true, "list"),
            shown("", &format!("{line}\n"), exit),
            "{body}"
        );
    }
    // An unhashable code: the line, then the traceback.
    let r = read(r#"{"error":{"code":["gated"]}}"#, false, "list");
    assert_eq!(r.exit, Exit::Error);
    assert_eq!(
        r.stderr,
        "{\"error\":{\"code\":[\"gated\"],\"hint\":\"\"}}\nTraceback (most recent call last):\n  File \"<stdin>\", line 53, in <module>\n  File \"<stdin>\", line 29, in die\nTypeError: unhashable type: 'list'\n"
    );
    // Not a dict.
    let r = read("[1]", false, "list");
    assert_eq!(r.stdout, "");
    assert!(r
        .stderr
        .ends_with("line 47, in <module>\nAttributeError: 'list' object has no attribute 'get'\n"));
    // `null` is no error.
    assert_eq!(
        read(r#"{"error":null,"a":"é"}"#, true, "x"),
        shown("{\"error\":null,\"a\":\"é\"}\n", "", Exit::Ok)
    );
}

#[test]
fn list_and_certs() {
    assert_eq!(
        read(r#"{"domains":[]}"#, false, "list").stdout,
        format!("(no custom domains attached to this server)\nhint: {CLI_NAME} domain add <apex> \u{2014} attach \u{2260} nameserver transfer\n")
    );
    let body = r#"{"domains":[
        {"apex":"acme.example","dnsWrite":true,"zoneId":"z1","names":[
            {"hostname":"www.acme.example","role":"publish","cert":{"state":"valid"},
             "renewal":{"k2Issued":true,"state":"due","failures":2},
             "app":{"primary":true,"served":"elsewhere","warning":"w"}},
            "skip",
            {"hostname":"m.acme.example","cert":null,"app":{"error":"unknown_app","app":"crm"}}]},
        {"apex":"globex.example","status":"pending_ns","nameservers":["ns1.x",null,"ns2.x"],"created":1},
        {"apex":"ro.example","readOnly":true,"zoneMissing":true},
        7]}"#;
    let zone = ZONE;
    let want = format!(
        "APEX                          DNS       ZONE\n\
acme.example                  write     z1\n  www.acme.example            publish   cert valid  renew due (2 failed; next ?)  (served: live, primary)  :443 answers with another server's certificate  warning: w\n  m.acme.example              other     cert missing  (not served: no App crm here)\n\
globex.example                pending   -\n  Pending: point nameservers to ns1.x, ns2.x\n  Auto-added to your {zone} account\n  This server re-checks with {zone} by itself; to check now: {CLI_NAME} domain refresh globex.example\n\
ro.example                    readonly  -\n  Not found on {zone} for this server (removed, or moved to another account) \u{2014} {CLI_NAME} domain refresh ro.example, or remove and add it again\n  Read-only: on hold on {zone} \u{2014} records serve, writes are refused (zone_readonly)\n\
7\n"
    );
    assert_eq!(read(body, false, "list"), shown(&want, "", Exit::Ok));
    // A cert that is not a dict.
    let r = read(
        r#"{"domains":[{"apex":"a","names":[{"cert":"x"}]}]}"#,
        false,
        "list",
    );
    assert_eq!(
        r.stdout,
        "APEX                          DNS       ZONE\na                             byo       -\n"
    );
    assert!(r
        .stderr
        .ends_with("line 167, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
    // Nameservers that can't be walked.
    let r = read(
        r#"{"domains":[{"apex":"a","status":"pending_ns","nameservers":3}]}"#,
        false,
        "list",
    );
    assert!(r.stderr.ends_with("  File \"<stdin>\", line 163, in <module>\n  File \"<stdin>\", line 85, in pending_lines\nTypeError: 'int' object is not iterable\n"), "{}", r.stderr);
    // Domains that can't be walked: after the header.
    let r = read(r#"{"domains":1.5}"#, false, "list");
    assert_eq!(r.stdout, "APEX                          DNS       ZONE\n");
    assert!(r
        .stderr
        .ends_with("line 154, in <module>\nTypeError: 'float' object is not iterable\n"));

    assert_eq!(
        read(r#"{"certs":null}"#, false, "certs").stdout,
        "(no attached hostnames)\n"
    );
    assert_eq!(
        read(r#"{"certs":[{"hostname":"a.example","cert":{"state":""},"app":{"external":true}},[1],{"app":{"notServedYet":1}}]}"#, false, "certs").stdout,
        format!(
            "HOSTNAME                          ROLE      CERT\na.example                         other     missing  (served by your own web server, not by {})\n[1]\n?                                 other     missing  (not served yet: waiting_dns)\n",
            k2_client::brand::PRODUCT_NAME
        )
    );
}

#[test]
fn add_refresh_and_the_writes() {
    assert_eq!(
        read(
            r#"{"ok":true,"domain":{"apex":"a.example","dnsWrite":1}}"#,
            false,
            "add"
        )
        .stdout,
        format!(
            "attached a.example \u{2014} {}-hosted DNS (writable)\n",
            k2_client::brand::PRODUCT_NAME
        )
    );
    assert_eq!(
        read(r#"{"domain":"x"}"#, false, "add").stdout,
        "attached ? \u{2014} BYO DNS (inventory only)\n"
    );
    assert_eq!(
        read(r#"{"domains":["s",{"apex":"b","readOnly":1,"status":"held"}],"checked":false}"#, false, "refresh").stdout,
        format!(
            "b                             {p}-hosted DNS (read-only: held on {z})\n  Read-only: held on {z} \u{2014} records serve, writes are refused (zone_readonly)\nnot checked: no {p} Connect tunnel token\n",
            p = k2_client::brand::PRODUCT_NAME,
            z = ZONE
        )
    );
    assert_eq!(
        read(r#"{"domains":"ab","checked":0}"#, false, "refresh").stdout,
        "(no pending domains to check)\n"
    );
    assert_eq!(
        read(r#"{"ok":true,"removed":"é"}"#, false, "remove").stdout,
        "{\"removed\":\"é\"}\n"
    );
    assert_eq!(
        read(r#"{"ok":true}"#, false, "issue").stdout,
        "{\"ok\":true}\n"
    );
    assert_eq!(
        read(r#"{"ok":true}"#, true, "issue").stdout,
        "{\"ok\":true}\n"
    );
    assert_eq!(read(r#"{"a":1.0}"#, false, "other").stdout, "{\"a\":1.0}\n");
}

#[test]
fn the_words() {
    use crate::families::{cert, domain};
    let usage = |h: &str| Act::Usage(h.replace("k2 ", &format!("{CLI_NAME} ")));
    assert_eq!(cert::parse(&[]), Act::Help("cert"));
    assert_eq!(cert::parse(&[""]), Act::Help("cert"));
    assert_eq!(
        cert::parse(&["x"]),
        usage(
            "unknown command 'x' \u{2014} see k2 cert --help (box inventory; not k2 hostmail cert)"
        )
    );
    assert_eq!(
        cert::parse(&["list", "--json"]),
        Act::Get {
            endpoint: "/cli/certs",
            query: vec![],
            json: true,
            verb: "certs"
        }
    );
    assert_eq!(
        cert::parse(&["list", "-"]),
        usage("unknown flag '-' \u{2014} usage: k2 cert list [--json]")
    );
    assert_eq!(
        cert::parse(&["list", ""]),
        usage("unexpected argument '' \u{2014} usage: k2 cert list [--json]")
    );
    assert_eq!(
        cert::parse(&["issue", "", "a.example", "--json"]),
        Act::Post {
            endpoint: "/cli/certs/issue",
            body: r#"{"hostname": "a.example"}"#.into(),
            json: true,
            verb: "issue"
        }
    );
    assert_eq!(
        cert::parse(&["renew", "é\n"]),
        Act::Post {
            endpoint: "/cli/certs/renew",
            body: r#"{"hostname": "\u00e9\n"}"#.into(),
            json: false,
            verb: "renew"
        }
    );
    assert_eq!(
        cert::parse(&["renew"]),
        usage("cert renew requires an attached <hostname>")
    );
    assert_eq!(
        cert::parse(&["issue", "a", "b", "--x"]),
        usage("unexpected extra argument 'b'")
    );
    assert_eq!(
        domain::parse(&["add", "a", "b"]),
        usage("unexpected extra argument 'b' \u{2014} one <apex> only")
    );
    assert_eq!(
        domain::parse(&["rm", "a", "b"]),
        usage("unexpected extra argument 'b'")
    );
    assert_eq!(
        domain::parse(&["check", " "]),
        Act::Post {
            endpoint: "/cli/domains/refresh",
            body: "{}".into(),
            json: false,
            verb: "refresh"
        }
    );
    assert_eq!(
        domain::parse(&["refresh", " a "]),
        Act::Post {
            endpoint: "/cli/domains/refresh",
            body: r#"{"apex": "a"}"#.into(),
            json: false,
            verb: "refresh"
        }
    );
    assert_eq!(domain::parse(&["name"]), Act::Help("domain/name"));
    assert_eq!(
        domain::parse(&["name", "x"]),
        usage("unknown name command 'x' \u{2014} use add | remove (see k2 domain name --help)")
    );
    assert_eq!(
        domain::parse(&["name", "add", "h", "--role"]),
        usage("--role requires mail|publish|direct|other|app:<name>")
    );
    assert_eq!(
        domain::parse(&["name", "add", "--role", " app:crm ", "h", "--external"]),
        Act::Post {
            endpoint: "/cli/domains/names",
            body: r#"{"hostname": "h", "role": "app:crm", "external": true}"#.into(),
            json: false,
            verb: "name-add"
        }
    );
    assert_eq!(
        domain::parse(&["name", "add", "--role", "--json", "h"]),
        Act::Post {
            endpoint: "/cli/domains/names",
            body: r#"{"hostname": "h", "role": "--json"}"#.into(),
            json: false,
            verb: "name-add"
        }
    );
    assert_eq!(
        domain::parse(&["name", "del", "h", "--json"]),
        Act::Post {
            endpoint: "/cli/domains/names/remove",
            body: r#"{"hostname": "h"}"#.into(),
            json: true,
            verb: "name-remove"
        }
    );
}

#[test]
fn the_usage_line_is_compact_json() {
    assert_eq!(
        err_line("usage", "unknown flag '\"é\t' \u{2014} x"),
        "{\"error\":{\"code\":\"usage\",\"hint\":\"unknown flag '\\\"é\\t' \u{2014} x\"}}"
    );
}

/// Under any brand, the lines name the brand's command, never `k2` (the
/// `xy` build proves it).
#[test]
fn the_lines_name_the_brand_s_command() {
    use crate::families::{cert, domain};
    let mut shown = vec![
        read(r#"{"domains":[]}"#, false, "list").stdout,
        read(
            r#"{"domains":[{"apex":"a","status":"pending_ns","zoneMissing":1}]}"#,
            false,
            "list",
        )
        .stdout,
    ];
    for words in [
        &["x"][..],
        &["list", "-x"],
        &["list", "y"],
        &["issue", "--x"],
        &["renew", "--x"],
    ] {
        let Act::Usage(h) = cert::parse(words) else {
            panic!("{words:?}")
        };
        shown.push(h);
    }
    for words in [
        &["x"][..],
        &["add"],
        &["list", "-x"],
        &["add", "-x"],
        &["rm", "-x"],
        &["check", "-x"],
        &["name", "x"],
        &["name", "add", "-x"],
        &["name", "rm", "-x"],
    ] {
        let Act::Usage(h) = domain::parse(words) else {
            panic!("{words:?}")
        };
        shown.push(h);
    }
    for line in &shown {
        assert!(line.contains(&format!("{CLI_NAME} ")), "{line:?}");
        if CLI_NAME != "k2" {
            assert!(!line.contains("k2 ") && !line.contains("k5 "), "{line:?}");
        }
    }
}

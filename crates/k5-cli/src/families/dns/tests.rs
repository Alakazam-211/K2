use super::*;
use k2_client::brand::PRODUCT_NAME;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Each line the router and the verbs follow, as the shell has them.
#[test]
fn the_shell_s_lines() {
    let text = source("dns.sh");
    for shell in [
        r#"    RESP="${RESP:-}" AS_JSON="${AS_JSON:-false}" D_VERB="${D_VERB:-}" python3 - <<'PYEOF'"#,
        r#"    RESP=$(cli_send cli_request "/cli/dns/access") || RESP="""#,
        r#"    RESP=$(cli_send cli_request "/cli/dns/zones") || RESP="""#,
        r#"    RESP=$(cli_send cli_request "/cli/dns/records" "domain=$(urlencode "$domain")") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/dns/records/add" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/dns/records/remove" "$body") || RESP="""#,
        r#"    RESP=$(cli_send cli_post_json "/cli/dns/verify" "$body") || RESP="""#,
        r#"if t: b["ttl"]=int(t)"#,
        r#"if p: b["priority"]=int(p)"#,
        r#"            --priority|--prio)"#,
        r#"                positionals+=("$1"); shift ;;"#,
        r#"        ''|*[!0-9]*) return 1 ;;"#,
        r#"        remove|rm|delete|del) shift; cmd_dns_record_remove "$@" ;;"#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "dns.sh no longer has {shell:?}"
        );
    }
    let router = source("router.sh");
    assert!(router.contains(
        "            verify)  shift 2; cmd_dns_verify \"$@\" ;;\n            --help|-h|'') cmd_help_dns; exit 0 ;;\n            *)\n                _dns_err \"usage\" \"unknown command '${2:-}' — see k2 dns --help\"\n                exit 2 ;;\n"
    ));
    let cell = source("cell-socket.sh");
    assert!(cell.contains("        /cli/dns/*) return 0 ;;"));
}

/// The Python's lines the reader raises at, as `python3 -` numbers them.
#[test]
fn the_python_s_lines() {
    let lines = super::super::domain::dom::tests::heredoc("dns.sh", "_dns_handle_resp() {");
    let at = |n: u32| lines[n as usize - 1].trim().to_string();
    for (n, text) in [
        (
            line::DIE_PENDING,
            "if code in PENDING_NS_CODES and not hint.startswith(\"Pending:\"):",
        ),
        (
            line::DIE_GATED,
            "if code in GATED_CODES and \"Settings\" not in hint:",
        ),
        (
            line::DIE_GATED_JOIN,
            "hint = (hint + \" — \" if hint else \"\") + DNS_GATED_HINT",
        ),
        (line::ERR, "err = d.get(\"error\")"),
        (line::DIE_ERR, "die(err)"),
        (line::ACCESS_WS, "if ws.get(\"path\") or ws.get(\"id\"):"),
        (
            line::ACCESS_TYPES,
            "print(\"types      : %s\" % \", \".join(str(t) for t in types))",
        ),
        (line::ACCESS_LEN, "print(\"zones      : %d\" % len(zones))"),
        (line::ACCESS_FOR, "for z in zones:"),
        (line::LIST_FOR, "for z in zones:"),
        (
            line::LIST_NS,
            "ns = [str(n) for n in (z.get(\"nameservers\") or []) if n]",
        ),
        (line::RECORDS_FOR, "for r in recs:"),
        (line::VERIFY_FOR, "for c in details:"),
    ] {
        assert_eq!(at(n), text, "line {n}");
    }
}

fn out(raw: &str, verb: &str) -> String {
    let r = read(raw, false, verb);
    assert_eq!((r.exit, r.stderr.as_str()), (Exit::Ok, ""), "{raw}");
    r.stdout
}

#[test]
fn the_verbs_print_what_the_shell_prints() {
    assert_eq!(
        out(r#"{"allowed":null,"capability":{"allowed":false},"workspace":{"id":"w1"},"record_types":["A","MX"],"zones":[{"domain":"acme.example","id":"z1"},"raw",{"id":5}]}"#, "access"),
        format!("dns manage : DENIED\nhint       : {GATED_HINT}\nworkspace  : w1\ntypes      : A, MX\nzones      : 3\n  acme.example                  z1\n  raw\n  ?                             5\n")
    );
    assert_eq!(
        out(r#"{"zones":[{"domain":"globex.example","status":"pending","nameservers":["ns1.x"]},{"domain":"b","state":"error"},{"status":"active","id":"z"},3]}"#, "list"),
        format!("DOMAIN                        STATUS        ID\nglobex.example                pending       \n  Pending: point nameservers to ns1.x (record writes refused until active; this server re-checks by itself)\nb                             error         \n  Pending: point nameservers to the {ZONE} nameservers ({CLI_NAME} domain list shows them) (record writes refused until active; this server re-checks by itself)\n?                             active        z\n3\n")
    );
    assert_eq!(
        out(r#"{"zones":[]}"#, "list"),
        format!("(no DNS zones)\nhint: add a zone in the {PRODUCT_NAME} Connect dashboard ({PUBLIC_SITE}), then delegate NS to ns1.{ZONE} / ns2.{ZONE} \u{2014} agents cannot create zones\n")
    );
    assert_eq!(
        out(r#"{"zone":{"domain":"acme.example","records":[{"type":"MX","name":"@","ttl":0,"content":"mx.acme.example","priority":10,"id":"r1","managed_by":"k2"},{"type":"A","value":"1.2.3.4","prio":null,"priority":null,"ttl":null,"managed_by":"user"},"x"]}}"#, "records"),
        "zone: acme.example\nTYPE    NAME                      TTL       VALUE\nMX      @                         0         mx.acme.example (prio 10)  [r1]  (managed_by=k2)\nA       @                         ?         1.2.3.4\nx\n"
    );
    assert_eq!(
        out(
            r#"{"ok":true,"record":{"id":"r9","type":"TXT","name":"_x","content":"v é"},"propagation":"~60s"}"#,
            "add"
        ),
        "added TXT _x \u{2192} v é  (id r9)\npropagation: ~60s\n"
    );
    assert_eq!(out(r#"{"record_id":7}"#, "remove"), "removed record 7\n");
    assert_eq!(
        out(r#"{"ok":true,"zone":{"domain":"acme.example"},"hint":"h","checks":[{"name":"ns","ok":true},{"status":"fail","check":"soa"},{"x":1},"plain"]}"#, "verify"),
        "verify acme.example: ok\nhint: h\n  True         ns\n  fail         soa\n  ?            {\"x\":1}\n  plain\n"
    );
    assert_eq!(out(r#"{"x": 1}"#, "other"), "{\"x\": 1}\n");
    assert_eq!(read(r#"{"x": 1}"#, true, "access").stdout, "{\"x\":1}\n");
}

#[test]
fn die_as_the_shell_s() {
    let err = |body: &str, exit: Exit| {
        let r = read(body, false, "list");
        assert_eq!((r.exit, r.stdout.as_str()), (exit, ""), "{body}");
        r.stderr
    };
    let pending = pending_ns_hint();
    assert_eq!(
        err(
            r#"{"error":{"code":"zone_pending_ns","hint":"zone_pending_ns"}}"#,
            Exit::Refused
        ),
        format!("{{\"error\":{{\"code\":\"zone_pending_ns\",\"hint\":\"{pending}\"}}}}\n")
    );
    assert_eq!(
        err(
            r#"{"error":{"code":"zone_pending_ns","hint":"nope"}}"#,
            Exit::Refused
        ),
        format!(
            "{{\"error\":{{\"code\":\"zone_pending_ns\",\"hint\":\"{pending} \u{2014} nope\"}}}}\n"
        )
    );
    let gated = GATED_HINT.replace('"', "\\\"");
    assert_eq!(
        err(r#"{"error":{"code":"dns_manage_disabled","hint":"off"}}"#, Exit::Refused),
        format!("{{\"error\":{{\"code\":\"dns_manage_disabled\",\"hint\":\"off \u{2014} {gated}\"}}}}\n")
    );
    assert_eq!(
        err(r#"{"error":{"code":"forbidden"}}"#, Exit::Refused),
        format!("{{\"error\":{{\"code\":\"forbidden\",\"hint\":\"{gated}\"}}}}\n")
    );
    assert_eq!(
        err(
            r#"{"error":{"code":"gated","hint":"see Settings"}}"#,
            Exit::Refused
        ),
        "{\"error\":{\"code\":\"gated\",\"hint\":\"see Settings\"}}\n"
    );
    assert_eq!(
        err(
            r#"{"error":{"code":"zone_readonly","hint":"x"}}"#,
            Exit::Refused
        ),
        "{\"error\":{\"code\":\"zone_readonly\",\"hint\":\"x\"}}\n"
    );
    assert_eq!(
        err(
            r#"{"error":{"code":"type_not_allowed","hint":"x"}}"#,
            Exit::Usage
        ),
        "{\"error\":{\"code\":\"type_not_allowed\",\"hint\":\"x\"}}\n"
    );
    assert_eq!(
        err(r#"{"error":{"code":"invalid_domain"}}"#, Exit::Error),
        "{\"error\":{\"code\":\"invalid_domain\",\"hint\":\"\"}}\n"
    );
    // Where die raises: nothing but the traceback.
    for (body, line, exception) in [
        (
            r#"{"error":{"code":["x"]}}"#,
            line::DIE_PENDING,
            "TypeError: unhashable type: 'list'",
        ),
        (
            r#"{"error":{"code":"zone_pending_ns","hint":5}}"#,
            line::DIE_PENDING,
            "AttributeError: 'int' object has no attribute 'startswith'",
        ),
        (
            r#"{"error":{"code":"forbidden","hint":null}}"#,
            line::DIE_GATED,
            "TypeError: argument of type 'NoneType' is not iterable",
        ),
        (
            r#"{"error":{"code":"forbidden","hint":["a"]}}"#,
            line::DIE_GATED_JOIN,
            "TypeError: can only concatenate list (not \"str\") to list",
        ),
    ] {
        assert_eq!(
            err(body, Exit::Error),
            format!("Traceback (most recent call last):\n  File \"<stdin>\", line {}, in <module>\n  File \"<stdin>\", line {line}, in die\n{exception}\n", line::DIE_ERR),
            "{body}"
        );
    }
    // An empty list of hints takes the teaching text.
    assert_eq!(
        err(r#"{"error":{"code":"forbidden","hint":[]}}"#, Exit::Refused),
        format!("{{\"error\":{{\"code\":\"forbidden\",\"hint\":\"{gated}\"}}}}\n")
    );
    // Odd shapes part way.
    let r = read(r#"{"zones":5}"#, false, "access");
    assert_eq!(
        r.stdout,
        format!("dns manage : DENIED\nhint       : {GATED_HINT}\n")
    );
    assert!(r
        .stderr
        .ends_with("line 106, in <module>\nTypeError: object of type 'int' has no len()\n"));
    let r = read(r#"{"allowed":1,"workspace":"w"}"#, false, "access");
    assert_eq!(r.stdout, "dns manage : allowed\n");
    assert!(r
        .stderr
        .ends_with("line 100, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
}

#[test]
fn the_words() {
    let usage = |h: &str| Act::Usage(h.replace("k2 ", &format!("{CLI_NAME} ")));
    let post = |endpoint, body: &str, json, verb| Act::Post {
        endpoint,
        body: body.into(),
        json,
        verb,
    };
    assert_eq!(parse(&[]), Act::Help("dns"));
    assert_eq!(parse(&[""]), Act::Help("dns"));
    assert_eq!(parse(&["record"]), Act::Help("dns/record"));
    assert_eq!(
        parse(&["zones"]),
        usage("unknown command 'zones' \u{2014} see k2 dns --help")
    );
    assert_eq!(
        parse(&["record", "x"]),
        usage("unknown record command 'x' \u{2014} use add | remove (see k2 dns record --help)")
    );
    assert_eq!(
        parse(&["access", "x"]),
        usage("unexpected argument 'x' \u{2014} usage: k2 dns access [--json]")
    );
    assert_eq!(
        parse(&["records", "a b/é", "--json"]),
        Act::Get {
            endpoint: RECORDS,
            query: vec!["domain=a%20b%2F%C3%A9".into()],
            json: true,
            verb: "records"
        }
    );
    assert_eq!(
        parse(&["records"]),
        usage("records requires a <domain> (daemon resolves the zone)")
    );
    assert_eq!(
        parse(&["records", "a", "b"]),
        usage("unexpected extra argument 'b' \u{2014} one <domain> only")
    );
    assert_eq!(
        parse(&[
            "record",
            "add",
            "acme.example",
            "MX",
            "@",
            "mx.acme.example",
            "--ttl",
            "0300",
            "--prio",
            "000"
        ]),
        post(
            RECORD_ADD,
            r#"{"domain": "acme.example", "type": "MX", "name": "@", "value": "mx.acme.example", "ttl": 300, "priority": 0}"#,
            false,
            "add"
        )
    );
    assert_eq!(
        parse(&["record", "add", "", "", "", "", "--ttl", "", "--json"]),
        post(
            RECORD_ADD,
            r#"{"domain": "", "type": "", "name": "", "value": ""}"#,
            true,
            "add"
        )
    );
    assert_eq!(
        parse(&["record", "add", "a", "b", "c"]),
        usage("record add requires <domain> <type> <name> <value>")
    );
    assert_eq!(
        parse(&["record", "add", "a", "b", "c", "d", "e", "f"]),
        usage("unexpected extra argument 'e' \u{2014} quote multi-word values")
    );
    assert_eq!(
        parse(&["record", "add", "a", "b", "c", "d", "--ttl", "1h"]),
        usage("--ttl must be a non-negative integer")
    );
    assert_eq!(
        parse(&["record", "add", "a", "b", "c", "d", "--priority", "-1"]),
        usage("--priority must be a non-negative integer")
    );
    assert_eq!(
        parse(&["record", "add", "a", "--prio"]),
        usage("--priority requires a number")
    );
    assert_eq!(
        parse(&["record", "add", "a", "--ttl"]),
        usage("--ttl requires a number")
    );
    assert_eq!(parse(&["record", "add", "-5"]), usage("unknown flag '-5' \u{2014} usage: k2 dns record add <domain> <type> <name> <value> [--ttl N] [--priority N] [--json]"));
    let long = "1".repeat(4301);
    let words = ["record", "add", "a", "b", "c", "d", "--ttl", long.as_str()];
    assert_eq!(parse(&words), Act::Shell);
    assert_eq!(
        parse(&["record", "rm", "r1"]),
        post(RECORD_REMOVE, r#"{"id": "r1"}"#, false, "remove")
    );
    assert_eq!(
        parse(&["record", "del"]),
        usage("record remove requires a <record-id> (from 'k2 dns records <domain>')")
    );
    assert_eq!(
        parse(&["verify", "a", "--json"]),
        post(VERIFY, r#"{"domain": "a"}"#, true, "verify")
    );
    assert_eq!(
        parse(&["verify", "--json"]),
        usage("verify requires a <domain>")
    );
}

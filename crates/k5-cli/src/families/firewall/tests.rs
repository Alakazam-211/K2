use super::*;

fn read(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn source() -> String {
    read("firewall.sh")
}

/// The Python that starts right after `start` (which ends in
/// `python3 -c '`) and ends before `end`, indexed by the line number
/// `python3 -c` gives each: line 1 is the empty one after `-c '`.
fn python_after(start: &str, end: &str) -> Vec<String> {
    let text = source();
    let at = text.find(start).unwrap_or_else(|| panic!("no {start:?}"));
    let body = &text[at + start.len()..];
    let stop = body.find(end).unwrap_or_else(|| panic!("no {end:?}"));
    std::iter::once(String::new())
        .chain(body[..stop].split('\n').map(str::to_string))
        .collect()
}

fn ask(list: &[&str]) -> Ask {
    match parse(list) {
        Parsed::Call(a) => *a,
        other => panic!("{list:?}: {other:?}"),
    }
}

fn early(line: &str) -> Parsed {
    Parsed::Early(format!("{line}\n"))
}

/// What a text says with the brand's names read back as the shell's
/// (`k2`, `K2`), where the shell's Python names them.
fn as_shell(text: &str) -> String {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    text.replace(&format!("{cli} firewall"), "k2 firewall")
        .replace(
            &format!("{product} at your provider"),
            "K2 at your provider",
        )
        .replace(&format!("({product} Web applies"), "(K2 Web applies")
}

/// Each line the parse, the checks and the calls follow, as the shell
/// has them.
#[test]
fn the_shell_s_lines() {
    let text = source();
    for shell in [
        r#"    local sub="${1:-status}""#,
        r#"    [ $# -gt 0 ] && shift"#,
        r#"        help|--help|-h) _firewall_usage; return 0 ;;"#,
        r#"        --json) sub=status; set -- --json "$@" ;;"#,
        r#"            --json) as_json=1; shift ;;"#,
        r#"            --format) fmt="${2:-}"; shift 2 || { echo "usage: --format nft|ufw" >&2; exit 2; } ;;"#,
        r#"            --format=*) fmt="${1#--format=}"; shift ;;"#,
        r#"            --label) label="${2:-}"; shift 2 || { echo "usage: --label \"<why>\"" >&2; exit 2; } ;;"#,
        r#"            --from) froms+=("${2:-}"); shift 2 || { echo "usage: --from <cidr>" >&2; exit 2; } ;;"#,
        r#"            --conns-per-ip) conns="${2:-}"; shift 2 || { echo "usage: --conns-per-ip N" >&2; exit 2; } ;;"#,
        r#"            --writes-per-min) writes="${2:-}"; shift 2 || { echo "usage: --writes-per-min N" >&2; exit 2; } ;;"#,
        r#"            --reset) reset=1; shift ;;"#,
        r#"            --hash) hash="${2:-}"; shift 2 || { echo "usage: --hash sha256:…" >&2; exit 2; } ;;"#,
        r#"            --provider) provider="${2:-}"; shift 2 || { echo "usage: --provider <name>" >&2; exit 2; } ;;"#,
        r#"            --applied-at) applied="${2:-}"; shift 2 || { echo "usage: --applied-at <unix>" >&2; exit 2; } ;;"#,
        r#"            --managed-by) managed="${2:-}"; shift 2 || { echo "usage: --managed-by k2|owner|auto" >&2; exit 2; } ;;"#,
        r#"            -h|--help) _firewall_usage; return 0 ;;"#,
        r#"            -*) echo "usage: unknown flag '$1' (k2 firewall --help)" >&2; exit 2 ;;"#,
        r#"            *) args+=("$1"); shift ;;"#,
        r#"            if [ "$as_json" = 1 ]; then raw="$(_sidecar_send GET /cli/firewall "")"; _firewall_render 1 json "$raw""#,
        r#"            else raw="$(_sidecar_send GET /cli/firewall "format=text")"; _firewall_render 0 text "$raw"; fi"#,
        r#"            raw="$(_sidecar_send GET /cli/firewall/ports "")""#,
        r#"            case "${fmt:-nft}" in nft|nftables|ufw) ;; *) echo "usage: k2 firewall rules --format nft|ufw" >&2; exit 2 ;; esac"#,
        r#"            raw="$(_sidecar_send GET /cli/firewall/rules "format=${fmt:-nft}")""#,
        r#"            _firewall_render 0 text "$raw""#,
        r#"                any) body='{"any":true}' ;;"#,
        r#"                    [ "${#args[@]}" -ge 2 ] || { echo "usage: k2 firewall ssh allow <cidr>... (SSH is never closed; for anyone: k2 firewall ssh any)" >&2; exit 2; }"#,
        r#"                    body="$(python3 -c 'import json,sys; print(json.dumps({"allow": sys.argv[1:]}))' "${args[@]:1}")" ;;"#,
        r#"                    [[ "${args[1]:-}" =~ ^[0-9]+$ ]] || { echo "usage: k2 firewall ssh port <n>" >&2; exit 2; }"#,
        r#"                    body="{\"port\":${args[1]}}" ;;"#,
        r#"                *) echo "usage: k2 firewall ssh any | allow <cidr>... | port <n>" >&2; exit 2 ;;"#,
        r#"            raw="$(_sidecar_send POST /cli/firewall/ssh "$body")""#,
        r#"            [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 firewall $sub <port>/<tcp|udp>" >&2; exit 2; }"#,
        r#"                [ -n "$label" ] || { echo "usage: k2 firewall open <port>/<tcp|udp> --label \"<why>\" [--from <cidr>]..." >&2; exit 2; }"#,
        r#"                body="$(python3 -c 'import json,sys; print(json.dumps({"port": sys.argv[1], "label": sys.argv[2], "from": sys.argv[3:]}))' "${args[0]}" "$label" ${froms[@]+"${froms[@]}"})""#,
        r#"                body="$(python3 -c 'import json,sys; print(json.dumps({"port": sys.argv[1]}))' "${args[0]}")""#,
        r#"            raw="$(_sidecar_send POST "/cli/firewall/$sub" "$body")""#,
        r#"print(json.dumps(b))' "$(IFS=$'\x1f'; printf '%s' "${args[*]-}")" "$conns" "$writes" "$reset")" || exit 2"#,
        r#"            raw="$(_sidecar_send POST /cli/firewall/guard "$body")""#,
        r#"print(json.dumps(b))' "$hash" "$provider" "$applied" "$managed")" || exit 2"#,
        r#"            raw="$(_sidecar_send POST /cli/firewall/upstream "$body")""#,
        r#"            _firewall_render "$as_json" status "$raw""#,
        r#"            echo "usage: unknown firewall verb '$sub' (status|ports|rules|ssh|open|close|guard|upstream)" >&2"#,
        r#"    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then"#,
        r#"        echo "k2 firewall: daemon unreachable" >&2"#,
        r#"    if [ "$code" != "200" ]; then"#,
        r#"        [ "$as_json" = "1" ] && printf '%s\n' "$body""#,
        r#"        exit "$(_sidecar_exit_for "$code")""#,
        r#"    if [ "$as_json" = "1" ] || [ "$mode" = "json" ]; then"#,
        r#"        printf '%s\n' "$body""#,
        r#"    if [ "$mode" = "text" ]; then"#,
        r#"        printf '%s' "$body""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "cmd_firewall no longer has {shell:?}"
        );
    }
    let sidecar = read("sidecar.sh");
    for shell in [
        "        200) echo 0 ;;",
        "        400|413) echo 2 ;;",
        "        403) echo 3 ;;",
        "        404) echo 4 ;;",
        "        *) echo 1 ;;",
    ] {
        assert!(
            sidecar.contains("_sidecar_exit_for() {\n    case \"$1\" in\n")
                && sidecar.lines().any(|l| l == shell),
            "_sidecar_exit_for no longer has {shell:?}"
        );
    }
    let policy = read("cli-tools-policy.sh");
    assert!(policy.contains("        firewall|front) echo \"dns\" ;;\n"));
    // No prelude row lets `firewall` past the connection gate.
    assert!(!policy.contains("[ \"${1:-}\" = \"firewall\" ]"));
    assert!(policy.contains(
        "for _a in \"$@\"; do case \"$_a\" in -h|--help) _k2_offline_verb=1 ;; esac; done\n"
    ));
}

/// The lines [`line`] names are the ones that raise in the shell.
#[test]
fn the_python_lines() {
    let e = python_after(
        "K2_FW_BODY=\"$body\" K2_FW_CODE=\"$code\" python3 -c '",
        "\n'",
    );
    for (at, has) in [
        (line::E_ERROR, r#"e = d.get("error")"#),
        (
            line::E_MESSAGE,
            r#"(": " + d["message"]) if d.get("message")"#,
        ),
        (
            line::E_ASK,
            r#"print("Ask with: " + p["command"], file=sys.stderr)"#,
        ),
    ] {
        assert!(
            e[at as usize].contains(has),
            "error line {at}: {:?}",
            e[at as usize]
        );
    }
    assert!(e[7].contains(r#""k2 firewall: error: HTTP " + code + ": " + raw.strip()[:300]"#));
    let v = python_after("    K2_FW_BODY=\"$body\" python3 -c '", "\n'");
    for (at, has) in [
        (line::V_LOAD, r#"d = json.loads(os.environ["K2_FW_BODY"])"#),
        (line::V_PUBLIC, r#"if "public" in d and "version" in d:"#),
        (
            line::V_HEAD,
            r#"print("view %s (v%s)" % (d.get("hash"), d.get("version")))"#,
        ),
        (line::V_PUBLIC_FOR, r#"for r in d.get("public") or []:"#),
        (
            line::V_PUBLIC_ROW,
            r#"print("  %5s/%-3s  %-11s  from %-20s  %s" %"#,
        ),
        (
            line::V_CLOSED_FOR,
            r#"for r in d.get("knownNotPublic") or []:"#,
        ),
        (line::V_CLOSED_ROW, r#"keep closed: %s" % (r["port"]"#),
        (line::V_UP, r#"up = d.get("upstream") or {}"#),
        (line::V_APPLIED_BY, r#"up.get("managedBy") == "k2""#),
        (line::V_GUARD, r#"elif "guard" in d and "counters" in d:"#),
        (line::V_G, r#"g = d["guard"]"#),
        (line::V_G_LINE, r#"print("App guard: %s"#),
        (line::V_APPS, r#"sorted((g.get("apps") or {}).items())"#),
        (
            line::V_APP,
            r#"{True: "on", False: "off"}.get(o.get("enabled"), "box default")"#,
        ),
        (line::V_UPSTREAM, r#"elif "upstream" in d:"#),
        (line::V_UP_D, r#"up = d["upstream"]"#),
        (line::V_A, r#"a = up.get("applied") or {}"#),
        (
            line::V_MANAGED,
            r#"print("managed by: %s" % up.get("managedBy"))"#,
        ),
        (line::V_APPLIED, r#"print("applied   : %s at %s (%s)""#),
        (line::V_OPENED, "if d.get(k):"),
        (line::V_SSH, r#"if d.get("ssh"):"#),
        (line::V_SSH_LINE, r#"print("ssh: port %s from %s""#),
        (line::V_NOTE, r#"if d.get("note"):"#),
        (line::V_NOTE_LINE, r#"print("note: " + d["note"])"#),
        (line::V_HASH, r#"if d.get("hash"):"#),
    ] {
        assert!(
            v[at as usize].contains(has),
            "view line {at}: {:?}",
            v[at as usize]
        );
    }
    let g = python_after(
        "        guard)\n            body=\"$(python3 -c '",
        "' \"$(IFS=",
    );
    assert_eq!(g[line::G_INT as usize], "        b[k] = int(v)");
    assert!(g[11].contains("usage: k2 firewall guard [<app>] [on|off] [--conns-per-ip N] [--writes-per-min N] [--reset]"));
    assert!(g[15].contains(r#"print("usage: %s takes a number" % k"#));
    let u = python_after(
        "        upstream)\n            body=\"$(python3 -c '",
        "' \"$hash\"",
    );
    assert_eq!(
        u[line::U_INT as usize],
        r#"        b["appliedAt"] = int(a)"#
    );
    assert!(u[9].contains(r#"print("usage: --applied-at <unix seconds>""#));
    assert!(u[14].contains("usage: k2 firewall upstream --hash <sha256:…> --provider <name> [--applied-at <unix>] | --managed-by k2|owner|auto"));
}

#[test]
fn the_help_forms() {
    for list in [
        &["help"][..],
        &["--help"],
        &["-h", "x"],
        &["ports", "--help"],
        &["--json", "-h"],
        &["nosuch", "x", "--json", "-h"],
        &["ssh", "--label", "x", "--help"],
    ] {
        assert_eq!(parse(list), Parsed::Help, "{list:?}");
    }
    // A problem before the help flag is the answer; a value takes it.
    assert_eq!(
        parse(&["ssh", "--bogus", "--help"]),
        early(&unknown_flag("--bogus"))
    );
    assert_eq!(ask(&["ssh", "--label", "--help"]).label, "--help");
    assert_eq!(ask(&["status", "help"]).args, vec!["help".to_string()]);
    assert!(offline_verb(&["firewall", "x", "-h"]));
    assert!(!offline_verb(&["firewall", "help"]));
}

#[test]
fn the_words() {
    assert_eq!(ask(&[]).sub, "status");
    assert_eq!(ask(&[""]).sub, "status");
    let a = ask(&["--json"]);
    assert_eq!((a.sub.as_str(), a.json), ("status", true));
    let a = ask(&[
        "open",
        "8080/tcp",
        "--label",
        "web é",
        "--from",
        "10.0.0.0/8",
        "--from",
        "",
        "--json",
        "--format=",
        "--reset",
        "--conns-per-ip",
        "5",
        "--writes-per-min",
        "6",
        "--hash",
        "h",
        "--provider",
        "p",
        "--applied-at",
        "7",
        "--managed-by",
        "auto",
    ]);
    assert_eq!(a.args, vec!["8080/tcp".to_string()]);
    assert_eq!(a.froms, vec!["10.0.0.0/8".to_string(), String::new()]);
    assert!(a.json && a.reset);
    assert_eq!(
        (
            a.label.as_str(),
            a.format.as_str(),
            a.conns.as_str(),
            a.writes.as_str()
        ),
        ("web é", "", "5", "6")
    );
    assert_eq!(
        (
            a.hash.as_str(),
            a.provider.as_str(),
            a.applied.as_str(),
            a.managed.as_str()
        ),
        ("h", "p", "7", "auto")
    );
    assert_eq!(ask(&["rules", "--format=ufw"]).format, "ufw");
    assert_eq!(ask(&["rules", "--format", ""]).format, "");
    for flag in [
        "--format",
        "--label",
        "--from",
        "--conns-per-ip",
        "--writes-per-min",
        "--hash",
        "--provider",
        "--applied-at",
        "--managed-by",
    ] {
        assert_eq!(parse(&["ports", flag]), early(flag_usage(flag)), "{flag}");
    }
    assert_eq!(flag_usage("--label"), "usage: --label \"<why>\"");
    assert_eq!(parse(&["ports", "-"]), early(&unknown_flag("-")));
    assert_eq!(
        parse(&["ports", "--label=x"]),
        early(&unknown_flag("--label=x"))
    );
    // An unknown verb is found after the flags.
    assert_eq!(ask(&["--bogus"]).sub, "--bogus");
    assert_eq!(
        parse(&["nosuch", "--bogus"]),
        early(&unknown_flag("--bogus"))
    );
}

fn req(list: &[&str]) -> Result<Request, String> {
    request(&ask(list))
}

/// Python 3's `json.dumps` wrote these bodies.
#[test]
fn the_requests() {
    let r = req(&[]).unwrap();
    assert_eq!(
        (
            r.get,
            r.path.as_str(),
            r.payload.as_str(),
            r.as_json,
            r.mode
        ),
        (true, "/cli/firewall", "format=text", false, Mode::Text)
    );
    let r = req(&["status", "--json"]).unwrap();
    assert_eq!(
        (r.payload.as_str(), r.as_json, r.mode),
        ("", true, Mode::Json)
    );
    let r = req(&["ports", "x"]).unwrap();
    assert_eq!(
        (
            r.get,
            r.path.as_str(),
            r.payload.as_str(),
            r.as_json,
            r.mode
        ),
        (true, "/cli/firewall/ports", "", false, Mode::Status)
    );
    let r = req(&["rules", "--json"]).unwrap();
    assert_eq!(
        (r.path.as_str(), r.payload.as_str(), r.as_json, r.mode),
        ("/cli/firewall/rules", "format=nft", false, Mode::Text)
    );
    assert_eq!(
        req(&["rules", "--format", "nftables"]).unwrap().payload,
        "format=nftables"
    );
    assert_eq!(
        req(&["rules", "--format", "iptables"]),
        Err(format!("{RULES_USAGE}\n"))
    );
    let post = |list: &[&str]| {
        let r = req(list).unwrap();
        assert!(!r.get && r.mode == Mode::Status, "{list:?}");
        (r.path, r.payload, r.as_json)
    };
    assert_eq!(
        post(&["ssh", "any", "x", "--json"]),
        ("/cli/firewall/ssh".into(), r#"{"any":true}"#.into(), true)
    );
    assert_eq!(
        post(&["ssh", "allow", "10.0.0.0/8", "é\u{1f}"]).1,
        r#"{"allow": ["10.0.0.0/8", "\u00e9\u001f"]}"#
    );
    assert_eq!(post(&["ssh", "port", "022"]).1, r#"{"port":022}"#);
    assert_eq!(req(&["ssh", "allow"]), Err(format!("{SSH_ALLOW_USAGE}\n")));
    // `-1` is a flag to the parse: an unknown one.
    assert_eq!(parse(&["ssh", "port", "-1"]), early(&unknown_flag("-1")));
    for bad in ["", "x", "2 2", "\u{663}"] {
        assert_eq!(
            req(&["ssh", "port", bad]),
            Err(format!("{SSH_PORT_USAGE}\n")),
            "{bad:?}"
        );
    }
    assert_eq!(req(&["ssh", "port"]), Err(format!("{SSH_PORT_USAGE}\n")));
    assert_eq!(req(&["ssh"]), Err(format!("{SSH_USAGE}\n")));
    assert_eq!(req(&["ssh", "none"]), Err(format!("{SSH_USAGE}\n")));
    assert_eq!(
        post(&[
            "open",
            "8080/tcp",
            "--label",
            "web é",
            "--from",
            "1.2.3.4/32",
            "--from",
            ""
        ]),
        (
            "/cli/firewall/open".into(),
            r#"{"port": "8080/tcp", "label": "web \u00e9", "from": ["1.2.3.4/32", ""]}"#.into(),
            false
        )
    );
    assert_eq!(
        post(&["open", "1/udp", "--label", "x"]).1,
        r#"{"port": "1/udp", "label": "x", "from": []}"#
    );
    assert_eq!(
        post(&["close", "8080/tcp", "--json"]),
        (
            "/cli/firewall/close".into(),
            r#"{"port": "8080/tcp"}"#.into(),
            true
        )
    );
    assert_eq!(req(&["open", "1/tcp"]), Err(format!("{OPEN_USAGE}\n")));
    assert_eq!(
        req(&["open", "1/tcp", "--label", ""]),
        Err(format!("{OPEN_USAGE}\n"))
    );
    assert_eq!(req(&["open"]), Err(format!("{}\n", port_usage("open"))));
    assert_eq!(
        req(&["close", "a", "b"]),
        Err(format!("{}\n", port_usage("close")))
    );
    assert_eq!(
        req(&["nosuch"]),
        Err(format!("{}\n", unknown_verb("nosuch")))
    );
    assert_eq!(
        req(&["--bogus"]),
        Err(format!("{}\n", unknown_verb("--bogus")))
    );
}

/// What `guard`'s and `upstream`'s Python printed (Python 3.12).
#[test]
fn the_guard_and_upstream_bodies() {
    let body = |list: &[&str]| req(list).map(|r| r.payload);
    assert_eq!(body(&["guard"]), Ok("{}".into()));
    assert_eq!(
        body(&[
            "guard",
            "off",
            "acme é",
            "on",
            "--conns-per-ip",
            "007",
            "--reset"
        ]),
        Ok(r#"{"on": true, "app": "acme \u00e9", "connsPerIp": 7, "reset": true}"#.into())
    );
    assert_eq!(
        body(&["guard", "--conns-per-ip", "\u{663}\u{664}"]),
        Ok(r#"{"connsPerIp": 34}"#.into())
    );
    assert_eq!(
        body(&["guard", "--writes-per-min", "0", "--conns-per-ip", ""]),
        Ok(r#"{"writesPerMin": 0}"#.into())
    );
    let usage = Err(format!("{GUARD_USAGE}\n"));
    assert_eq!(body(&["guard", "acme", "x"]), usage);
    // The shell joins the words with \x1f and Python splits them again.
    assert_eq!(body(&["guard", "", ""]), usage);
    assert_eq!(body(&["guard", "a\u{1f}b"]), usage);
    assert_eq!(body(&["guard", ""]), Ok("{}".into()));
    assert_eq!(
        body(&["guard", "--conns-per-ip", "12a"]),
        Err("usage: connsPerIp takes a number\n".into())
    );
    assert_eq!(
        body(&["guard", "--writes-per-min", "-1"]),
        Err("usage: writesPerMin takes a number\n".into())
    );
    assert_eq!(
        body(&["guard", "--writes-per-min", "\u{b2}"]),
        Err(concat!(
            "Traceback (most recent call last):\n",
            "  File \"<string>\", line 16, in <module>\n",
            "ValueError: invalid literal for int() with base 10: '\u{b2}'\n"
        )
        .into())
    );
    let usage = Err(format!("{UPSTREAM_USAGE}\n"));
    assert_eq!(body(&["upstream"]), usage);
    assert_eq!(
        body(&["upstream", "--provider", "p", "--applied-at", "1"]),
        usage
    );
    assert_eq!(
        body(&["upstream", "--hash", "sha256:ab"]),
        Ok(r#"{"hash": "sha256:ab", "provider": ""}"#.into())
    );
    assert_eq!(
        body(&[
            "upstream",
            "--hash",
            "h",
            "--provider",
            "acme",
            "--applied-at",
            "1700000000",
            "--managed-by",
            "auto"
        ]),
        Ok(
            r#"{"hash": "h", "provider": "acme", "appliedAt": 1700000000, "managedBy": "auto"}"#
                .into()
        )
    );
    assert_eq!(
        body(&["upstream", "--applied-at", "zz", "--managed-by", "k2"]),
        Ok(r#"{"managedBy": "k2"}"#.into())
    );
    assert_eq!(
        body(&["upstream", "--hash", "h", "--applied-at", "17x"]),
        Err(format!("{APPLIED_AT_USAGE}\n"))
    );
    let r = body(&["upstream", "--hash", "h", "--applied-at", "\u{b9}"]).unwrap_err();
    assert!(r.contains("line 10, in <module>"), "{r}");
    assert!(r.ends_with("ValueError: invalid literal for int() with base 10: '\u{b9}'\n"));
}

#[test]
fn python_s_digits() {
    for yes in [
        "0",
        "123",
        "\u{b2}",
        "\u{663}",
        "\u{ff11}",
        "\u{2460}",
        "\u{1f100}",
        "1\u{b2}",
    ] {
        assert!(py_isdigit(yes), "{yes:?}");
    }
    for no in [
        "",
        " 1",
        "1 ",
        "+1",
        "-1",
        "1_0",
        "1.0",
        "x",
        "\u{2155}",
        "\u{10d40}",
    ] {
        assert!(!py_isdigit(no), "{no:?}");
    }
    assert_eq!(py_int("0007", 1), Ok(Value::Int("7".into())));
    assert_eq!(py_int("000", 1), Ok(Value::Int("0".into())));
    assert_eq!(py_int("\u{661}\u{6f2}3", 1), Ok(Value::Int("123".into())));
    let r = py_int(&"0".repeat(4301), 9).unwrap_err();
    assert_eq!(
        r.exception,
        "ValueError: Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit"
    );
    assert_eq!(py_int(&"1".repeat(4300), 9).map(|_| ()), Ok(()));
    // A digit int() refuses comes first; the text is cut at 200.
    let r = py_int(&format!("{}\u{b2}", "1".repeat(4301)), 9).unwrap_err();
    assert_eq!(
        r.exception,
        format!(
            "ValueError: invalid literal for int() with base 10: '{}",
            "1".repeat(199)
        )
    );
}

#[test]
fn a_refusal_s_lines() {
    let lines = |body: &str| as_shell(&error_lines("403", body.as_bytes()));
    assert_eq!(
        lines(r#"{"error":{"code":"owner_only","hint":"Only the owner."},"permission":{"command":"k2 permission request --scope x","requestable":true}}"#),
        "k2 firewall: error: owner_only: Only the owner.\nAsk with: k2 permission request --scope x\n"
    );
    assert_eq!(
        lines(r#"{"error":{"code":null}}"#),
        "k2 firewall: error: None: \n"
    );
    assert_eq!(
        lines(r#"{"error":"boom","message":"why"}"#),
        "k2 firewall: error: boom: why\n"
    );
    assert_eq!(
        lines(r#"{"error":[1,"a"],"permission":{"requestable":false}}"#),
        "k2 firewall: error: [1, 'a']\nIt can't be requested.\n"
    );
    assert_eq!(lines("{}"), "k2 firewall: error: None\n");
    assert_eq!(
        lines(r#"{"permission":{"requestable":0}}"#),
        "k2 firewall: error: None\n"
    );
    assert_eq!(
        lines("  <html>\n "),
        "k2 firewall: error: HTTP 403: <html>\n"
    );
    assert_eq!(lines(""), "k2 firewall: error: HTTP 403: \n");
    let long = format!("{}{}", "é".repeat(300), "x");
    assert_eq!(
        lines(&long),
        format!("k2 firewall: error: HTTP 403: {}\n", "é".repeat(300))
    );
    assert_eq!(
        lines("[1]"),
        concat!(
            "Traceback (most recent call last):\n",
            "  File \"<string>\", line 8, in <module>\n",
            "AttributeError: 'list' object has no attribute 'get'\n"
        )
    );
    let r = lines(r#"{"error":"x","message":5}"#);
    assert!(r.starts_with("Traceback") && r.contains("line 12,"), "{r}");
    assert!(r.ends_with("TypeError: can only concatenate str (not \"int\") to str\n"));
    let r = lines(r#"{"error":{"code":"c","hint":0},"permission":{"command":7}}"#);
    assert!(r.starts_with("k2 firewall: error: c: \nTraceback"), "{r}");
    assert!(r.contains("line 16,"), "{r}");
    assert_eq!(exit_for("400"), Exit::Usage);
    assert_eq!(exit_for("413"), Exit::Usage);
    assert_eq!(exit_for("401"), Exit::Error);
    assert_eq!(exit_for("403"), Exit::Refused);
    assert_eq!(exit_for("404"), Exit::NotFound);
    assert_eq!(exit_for("201"), Exit::Error);
}

#[test]
fn the_render() {
    let r = render(true, Mode::Status, "000", b"");
    assert_eq!((r.stdout.as_slice(), r.exit), (&b""[..], Exit::Error));
    assert_eq!(as_shell(&r.stderr), "k2 firewall: daemon unreachable\n");
    // A refusal: its lines, the body too with --json, `_sidecar_exit_for`.
    let r = render(true, Mode::Status, "404", br#"{"error":"nope"}"#);
    assert_eq!(r.stdout, b"{\"error\":\"nope\"}\n");
    assert_eq!(
        (as_shell(&r.stderr).as_str(), r.exit),
        ("k2 firewall: error: nope\n", Exit::NotFound)
    );
    let r = render(false, Mode::Text, "500", b"x");
    assert_eq!((r.stdout.as_slice(), r.exit), (&b""[..], Exit::Error));
    // Where the refusal's Python raises, `set -e` ends it: exit 1, no
    // body.
    let r = render(true, Mode::Status, "400", b"[1]");
    assert_eq!((r.stdout.as_slice(), r.exit), (&b""[..], Exit::Error));
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    let r = render(
        true,
        Mode::Status,
        "403",
        br#"{"error":"x","permission":{"command":7}}"#,
    );
    assert_eq!((r.stdout.as_slice(), r.exit), (&b""[..], Exit::Error));
    assert!(as_shell(&r.stderr).starts_with("k2 firewall: error: x\nTraceback"));
    // A 200: the body as it came, a newline after it for --json.
    let (with_newline, as_it_came): (&[u8], &[u8]) = (b"{\"a\":\xff}\n\n", b"{\"a\":\xff}\n");
    for (as_json, mode, want) in [
        (true, Mode::Status, with_newline),
        (false, Mode::Json, with_newline),
        (false, Mode::Text, as_it_came),
    ] {
        let r = render(as_json, mode, "200", b"{\"a\":\xff}\n");
        assert_eq!(
            (r.stdout.as_slice(), r.stderr.as_str(), r.exit),
            (want, "", Exit::Ok)
        );
    }
}

fn view(body: &str) -> Shown {
    let mut r = view_lines(body.as_bytes());
    r.stdout = as_shell(&r.stdout);
    r
}

/// What the shell's Python printed for these bodies (the exceptions are
/// Python 3.12's).
#[test]
fn the_view() {
    assert_eq!(
        view(r#"{"hash":"sha256:ab","version":3,"public":[{"port":22,"proto":"tcp","source":"ssh","allowFrom":["10.0.0.0/8","192.168.1.0/24"],"why":"SSH"},{"port":443,"proto":"tcp","source":"front443","why":"the :443 front é"},{"port":51820,"proto":"udp","source":"owner"}],"knownNotPublic":[{"port":4100,"proto":"tcp","source":"daemon","why":"loopback only"}],"upstream":{"managedBy":"k2"}}"#).stdout,
        concat!(
            "view sha256:ab (v3)\n",
            "     22/tcp  ssh          from 10.0.0.0/8, 192.168.1.0/24  SSH\n",
            "    443/tcp  front443     from                       the :443 front é\n",
            "  51820/udp  owner        from                       \n",
            "   4100/tcp  daemon       keep closed: loopback only\n",
            "applied by: K2 at your provider\n",
        )
    );
    assert_eq!(
        view(r#"{"hash":null,"version":1,"public":[],"upstream":{"managedBy":"owner"}}"#).stdout,
        "view None (v1)\napplied by: nobody (k2 firewall rules)\n"
    );
    assert_eq!(
        view(r#"{"public":[],"version":1,"knownNotPublic":[{"port":"x","proto":"udp","source":"a long source name"}],"upstream":{"managedBy":"k2"},"hash":"h"}"#).stdout,
        "view h (v1)\n      x/udp  a long source name  keep closed: \napplied by: K2 at your provider\n"
    );
    assert_eq!(
        view(r#"{"guard":{"enabled":true,"connsPerIp":64,"writesPerMin":120,"apps":{"zeta":{"enabled":false},"alpha":{"enabled":true,"writesPerMin":30},"mid":{},"one":{"enabled":1.0,"writesPerMin":0}}},"counters":{}}"#).stdout,
        concat!(
            "App guard: on, 64 connections per visitor, 120 writes per visitor per App a minute\n",
            "  alpha: on, 30 writes a minute\n",
            "  mid: box default\n",
            "  one: on\n",
            "  zeta: off\n",
        )
    );
    assert_eq!(
        view(r#"{"guard":{"apps":{"a":{"enabled":"yes","writesPerMin":[2]}},"connsPerIp":null},"counters":1}"#).stdout,
        "App guard: off, None connections per visitor, None writes per visitor per App a minute\n  a: box default, [2] writes a minute\n"
    );
    assert_eq!(
        view(r#"{"upstream":{"managedBy":"auto","applied":{"hash":"sha256:cd","provider":"acme","current":true}}}"#).stdout,
        "managed by: auto\napplied   : sha256:cd at acme (this list)\n"
    );
    assert_eq!(
        view(r#"{"upstream":{"managedBy":"owner","applied":{"hash":"h","provider":"p"}}}"#).stdout,
        "managed by: owner\napplied   : h at p (an older list)\n"
    );
    assert_eq!(view(r#"{"upstream":{}}"#).stdout, "managed by: None\n");
    assert_eq!(
        view(r#"{"opened":"8080/tcp","closed":"","ssh":{"port":2222,"allowFrom":[]},"note":"open it at your provider too","hash":"sha256:ef"}"#).stdout,
        concat!(
            "opened 8080/tcp\n",
            "ssh: port 2222 from anyone\n",
            "note: open it at your provider too\n",
            "view hash now sha256:ef (K2 Web applies it on its next pass, where it manages this server)\n",
        )
    );
    assert_eq!(
        view(r#"{"ssh":{"allowFrom":"ab"}}"#).stdout,
        "ssh: port None from a, b\n"
    );
    assert_eq!(
        view(r#"{"opened":[1,2],"closed":{"a":1},"hash":1.5}"#).stdout,
        "opened [1, 2]\nclosed {'a': 1}\nview hash now 1.5 (K2 Web applies it on its next pass, where it manages this server)\n"
    );
    for body in [r#"{"ok":true}"#, "{}"] {
        assert_eq!(view(body), Shown::out(""), "{body}");
    }
    for (body, stdout, at, exception) in [
        ("[]", "", 25, "AttributeError: 'list' object has no attribute 'get'"),
        (r#""public version""#, "", 5, "AttributeError: 'str' object has no attribute 'get'"),
        ("5", "", 4, "TypeError: argument of type 'int' is not iterable"),
        ("null", "", 4, "TypeError: argument of type 'NoneType' is not iterable"),
        ("true", "", 4, "TypeError: argument of type 'bool' is not iterable"),
        (r#"["public","version"]"#, "", 5, "AttributeError: 'list' object has no attribute 'get'"),
        (r#"{"public":[{"proto":"tcp"}],"version":1}"#, "view None (v1)\n", 7, "KeyError: 'port'"),
        (r#"{"public":[{"port":1,"proto":"t","source":"s","allowFrom":[1]}],"version":1}"#, "view None (v1)\n", 7, "TypeError: sequence item 0: expected str instance, int found"),
        (r#"{"public":5,"version":1}"#, "view None (v1)\n", 6, "TypeError: 'int' object is not iterable"),
        (r#"{"public":"ab","version":1}"#, "view None (v1)\n", 7, "TypeError: string indices must be integers, not 'str'"),
        (r#"{"public":[],"version":1,"upstream":[1]}"#, "view None (v1)\n", 11, "AttributeError: 'list' object has no attribute 'get'"),
        (r#"{"guard":[1],"counters":1}"#, "", 14, "AttributeError: 'list' object has no attribute 'get'"),
        (r#"{"guard":{"apps":[1]},"counters":1}"#, "App guard: off, None connections per visitor, None writes per visitor per App a minute\n", 15, "AttributeError: 'list' object has no attribute 'items'"),
        (r#"{"guard":{"apps":{"a":{"enabled":[1]}}},"counters":1}"#, "App guard: off, None connections per visitor, None writes per visitor per App a minute\n", 16, "TypeError: unhashable type: 'list'"),
        (r#"{"guard":{"apps":{"a":5}},"counters":1}"#, "App guard: off, None connections per visitor, None writes per visitor per App a minute\n", 16, "AttributeError: 'int' object has no attribute 'get'"),
        (r#"["guard","counters"]"#, "", 13, "TypeError: list indices must be integers or slices, not str"),
        (r#""upstream""#, "", 18, "TypeError: string indices must be integers, not 'str'"),
        (r#"{"upstream":null}"#, "", 19, "AttributeError: 'NoneType' object has no attribute 'get'"),
        (r#"{"upstream":{"applied":[1]}}"#, "managed by: None\n", 22, "AttributeError: 'list' object has no attribute 'get'"),
        (r#"{"note":5}"#, "", 31, "TypeError: can only concatenate str (not \"int\") to str"),
        (r#"{"ssh":{"allowFrom":5}}"#, "", 29, "TypeError: can only join an iterable"),
        (r#"{"ssh":"x"}"#, "", 29, "AttributeError: 'str' object has no attribute 'get'"),
    ] {
        let r = view(body);
        assert_eq!(
            r,
            Shown {
                stdout: stdout.into(),
                stderr: Raise::at(at, exception).text(),
                exit: Exit::Error,
            },
            "{body}"
        );
    }
    for body in ["<html>", "", " \n"] {
        let r = view(body);
        assert_eq!((r.stdout.as_str(), r.exit), ("", Exit::Error), "{body:?}");
        assert!(r.stderr.contains("line 3, in <module>"), "{body:?}");
        assert!(
            r.stderr.contains("json.decoder.JSONDecodeError: "),
            "{body:?}"
        );
    }
}

/// Under any brand, the lines name the command and the product by the
/// brand's names; the wire's `k2` stays.
#[test]
fn the_lines_say_the_brand() {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for line in [
        UNREACHABLE.to_string(),
        ERROR.to_string(),
        GUARD_USAGE.into(),
        UPSTREAM_USAGE.into(),
        RULES_USAGE.into(),
        SSH_ALLOW_USAGE.into(),
        SSH_PORT_USAGE.into(),
        SSH_USAGE.into(),
        OPEN_USAGE.into(),
        unknown_flag("--x"),
        port_usage("open"),
    ] {
        assert!(line.contains(&format!("{cli} firewall")), "{line}");
        if cli != "k2" {
            assert!(
                !line.replace("--managed-by k2|", "").contains("k2"),
                "{line}"
            );
        }
    }
    assert!(UPSTREAM_USAGE.ends_with("--managed-by k2|owner|auto"));
    let r = view_lines(br#"{"public":[],"version":1,"upstream":{"managedBy":"k2"}}"#);
    assert!(
        r.stdout.contains(&format!("{product} at your provider")),
        "{}",
        r.stdout
    );
    let r = view_lines(br#"{"public":[],"version":1}"#);
    assert!(
        r.stdout.contains(&format!("({cli} firewall rules)")),
        "{}",
        r.stdout
    );
    let r = view_lines(br#"{"hash":"h"}"#);
    assert!(
        r.stdout.contains(&format!("({product} Web applies")),
        "{}",
        r.stdout
    );
}

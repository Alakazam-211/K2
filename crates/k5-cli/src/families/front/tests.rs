use super::*;

fn read(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python in `file` that starts right after `start` (which ends in
/// `python3 -c '`) and ends before `end`, indexed by the line number
/// `python3 -c` gives each: line 1 is the empty one after `-c '`.
fn python_after(file: &str, start: &str, end: &str) -> Vec<String> {
    let text = read(file);
    let at = text.find(start).unwrap_or_else(|| panic!("no {start:?}"));
    let body = &text[at + start.len()..];
    let stop = body.find(end).unwrap_or_else(|| panic!("no {end:?}"));
    std::iter::once(String::new())
        .chain(body[..stop].split('\n').map(str::to_string))
        .collect()
}

fn call(list: &[&str]) -> Ask {
    match parse(list) {
        Parsed::Call(a) => a,
        other => panic!("{list:?}: {other:?}"),
    }
}

fn early_line(line: &str) -> Parsed {
    Parsed::Early(format!("{line}\n"))
}

/// What a text says with the brand's names read back as the shell's.
fn as_shell(text: &str) -> String {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    text.replace(&format!("{cli} front"), "k2 front")
        .replace(&format!("{cli} firewall"), "k2 firewall")
        .replace(&format!(": {product} will restart"), ": K2 will restart")
}

/// Each line the parse, the checks and the calls follow, as the shell
/// has them.
#[test]
fn the_shell_s_lines() {
    let desktop = read("desktop.sh");
    for shell in [
        r#"    local sub="${1:-status}""#,
        r#"        help|--help|-h) _front_usage; return 0 ;;"#,
        r#"        passthrough) _front_passthrough "$@"; return ;;"#,
        r#"        http) _front_http "$@"; return ;;"#,
        r#"            --mail) mail=1; shift ;;"#,
        r#"            *) echo "usage: unknown argument '$1' (k2 front --help)" >&2; exit 2 ;;"#,
        r#"            raw="$(_sidecar_send GET /cli/front/status "")" || true"#,
        r#"            local body='{"mail":false}'"#,
        r#"            [ "$mail" = 1 ] && body='{"mail":true}'"#,
        r#"            raw="$(_sidecar_send POST /cli/front/enable "$body")" || true"#,
        r#"            [ "$mail" = 1 ] && { echo "usage: --mail is for enable (disable takes everything back)" >&2; exit 2; }"#,
        r#"            raw="$(_sidecar_send POST /cli/front/disable "{}")" || true"#,
        r#"            echo "usage: unknown front verb '$sub' (status|enable|disable|http|passthrough)" >&2"#,
        r#"            on|off) mode="$1"; shift ;;"#,
        r#"            *) echo "usage: k2 front http on|off [--json]" >&2; exit 2 ;;"#,
        r#"    [ -n "$mode" ] || { echo "usage: k2 front http on|off [--json]" >&2; exit 2; }"#,
        r#"    body='{"on":false}'"#,
        r#"    [ "$mode" = on ] && body='{"on":true}'"#,
        r#"    raw="$(_sidecar_send POST /cli/front/http "$body")" || true"#,
        r#"    local verb="${1:-list}" as_json=0 to="" proxy=0 label="" host="" raw body"#,
        r#"        -*) verb=list ;;   # `k2 front passthrough --json`"#,
        r#"        *) [ $# -gt 0 ] && shift ;;"#,
        r#"            --to) to="${2:-}"; shift 2 || { echo "usage: --to 127.0.0.1:<port>" >&2; exit 2; } ;;"#,
        r#"            --to=*) to="${1#--to=}"; shift ;;"#,
        r#"            --proxy-v2) proxy=1; shift ;;"#,
        r#"            --label) label="${2:-}"; shift 2 || { echo "usage: --label <why>" >&2; exit 2; } ;;"#,
        r#"            -*) echo "usage: unknown flag '$1' (k2 front --help)" >&2; exit 2 ;;"#,
        r#"            *) host="$1"; shift ;;"#,
        r#"        list|ls) raw="$(_sidecar_send GET /cli/front/passthrough "")" ;;"#,
        r#"            { [ -n "$host" ] && [ -n "$to" ]; } || { echo "usage: k2 front passthrough add <host>|'*' --to 127.0.0.1:<port> [--proxy-v2] [--label <why>]" >&2; exit 2; }"#,
        r#"            body="$(python3 -c 'import json,sys; print(json.dumps({"host": sys.argv[1], "to": sys.argv[2], "proxyV2": sys.argv[3] == "1", "label": sys.argv[4]}))' "$host" "$to" "$proxy" "$label")""#,
        r#"            raw="$(_sidecar_send POST /cli/front/passthrough/add "$body")" ;;"#,
        r#"        rm|remove)"#,
        r#"            [ -n "$host" ] || { echo "usage: k2 front passthrough rm <host>|'*'" >&2; exit 2; }"#,
        r#"            body="$(python3 -c 'import json,sys; print(json.dumps({"host": sys.argv[1]}))' "$host")""#,
        r#"            raw="$(_sidecar_send POST /cli/front/passthrough/rm "$body")" ;;"#,
        r#"        *) echo "usage: k2 front passthrough [list|add|rm]" >&2; exit 2 ;;"#,
        r#"        echo "k2 front: daemon unreachable" >&2; exit 1"#,
        r#"    if [ "$as_json" = 1 ]; then printf '%s\n' "$out"; [ "$code" = 200 ] || exit "$(_sidecar_exit_for "$code")"; return 0; fi"#,
        r#"        _firewall_render 0 status "$raw""#,
    ] {
        assert!(
            desktop.lines().any(|l| l == shell),
            "cmd_front no longer has {shell:?}"
        );
    }
    let front = read("front.sh");
    for shell in [
        r#"    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then"#,
        r#"        echo "k2 front: daemon unreachable" >&2"#,
        r#"    if [ "$as_json" = "1" ]; then"#,
        r#"        printf '%s\n' "$body""#,
        r#"        [ "$code" = "200" ] || { [ "$code" = "403" ] && exit 3; exit 1; }"#,
    ] {
        assert!(
            front.lines().any(|l| l == shell),
            "_front_render no longer has {shell:?}"
        );
    }
    let policy = read("cli-tools-policy.sh");
    assert!(policy.contains("        firewall|front) echo \"dns\" ;;\n"));
    // No prelude row lets `front` past the connection gate.
    assert!(!policy.contains("[ \"${1:-}\" = \"front\" ]"));
}

/// The lines [`line`] names are the ones that raise in the shell.
#[test]
fn the_python_lines() {
    let r = python_after(
        "front.sh",
        "K2_FR_BODY=\"$body\" K2_FR_CODE=\"$code\" python3 -c '",
        "\n'",
    );
    assert!(r[7].contains(r#""k2 front: error: HTTP " + code + ": " + raw.strip()[:200]"#));
    assert!(r[7].contains(r#"sys.exit(3 if code == "403" else 1)"#));
    for (at, has) in [
        (
            line::R_OK,
            r#"if code != "200" or d.get("ok") is False or (isinstance(d.get("error"), str) and "serving" not in d):"#,
        ),
        (line::R_ERROR, r#"e = d.get("error")"#),
        (line::R_MESSAGE, r#"(": " + msg if msg else "")"#),
        (
            line::R_ASK,
            r#"print("Ask with: " + p["command"], file=sys.stderr)"#,
        ),
        (
            line::R_SOCKET,
            r#"str((d.get("socket") or {}).get("source"))"#,
        ),
        (
            line::R_HTTP,
            r#"print(":80        :", ("on" if ht.get("on") else "off")"#,
        ),
        (line::R_HELPER, r#"if ht.get("helper"):"#),
        (line::R_ROUTES, r#"for r in d.get("routes") or []:"#),
        (line::R_ROUTE, r#"print("route      :", r.get("host")"#),
        (
            line::R_HOLDER,
            r#"if h.get("holder") and h.get("holder") != "free":"#,
        ),
        (
            line::R_HOLDER_LINE,
            r#"print(":443       :", h.get("holder"), "-", h.get("message"))"#,
        ),
        (
            line::R_PASSTHROUGH,
            r#"", ".join(x.get("host") + " -> " + x.get("to") for x in pt)"#,
        ),
        (
            line::R_GUARD,
            r#"print("App guard  :", "refused connections", g.get("perIpRefused")"#,
        ),
        (
            line::R_CLOSED,
            r#"", ".join("%s %s" % (k, v) for k, v in cl.items() if v)"#,
        ),
        (line::R_STATE, r#"st = su.get("state") or {}"#),
        (
            line::R_UNIT,
            r#"print("socket unit:", st.get("ActiveState"), st.get("SubState") or """#,
        ),
        (line::R_JOURNAL, r#"for line in su.get("journal") or []:"#),
        (line::R_WARNINGS, r#"for w in d.get("warnings") or []:"#),
    ] {
        assert!(
            r[at as usize].contains(has),
            "render line {at}: {:?}",
            r[at as usize]
        );
    }
    assert!(r[19].contains(r#"if "serving" in d:"#));
    assert!(r[56].contains(r#"if "on" in d:"#));
    let p = python_after("desktop.sh", "K2_FP_BODY=\"$out\" python3 -c '", "'\n}");
    for (at, has) in [
        (line::P_LOAD, r#"d = json.loads(os.environ["K2_FP_BODY"])"#),
        (line::P_ROWS, r#"rows = d.get("passthrough") or []"#),
        (line::P_FOR, "for r in rows:"),
        (
            line::P_ROW,
            r#"("  (" + r["label"] + ")") if r.get("label") else """#,
        ),
        (line::P_WARNINGS, r#"for w in d.get("warnings") or []:"#),
        (line::P_WARNING, r#"print("warning: " + w)"#),
        (line::P_NOTE, r#"print("note: " + d["next"])"#),
    ] {
        assert!(
            p[at as usize].contains(has),
            "passthrough line {at}: {:?}",
            p[at as usize]
        );
    }
    assert_eq!(p.len(), 13, "{p:?}");
}

#[test]
fn the_help_forms() {
    for list in [
        &["help"][..],
        &["--help"],
        &["-h", "x"],
        &["status", "--json", "-h"],
        &["enable", "--mail", "--help"],
        &["http", "on", "-h"],
        &["passthrough", "-h"],
        &["passthrough", "add", "h", "--to", "x", "--help"],
        &["nosuch", "--help"],
    ] {
        assert_eq!(parse(list), Parsed::Help, "{list:?}");
    }
    // A problem before the help flag is the answer; a value takes it.
    assert_eq!(
        parse(&["status", "x", "-h"]),
        early_line(&unknown_argument("x"))
    );
    assert_eq!(parse(&["http", "x", "-h"]), early_line(HTTP_USAGE));
    assert_eq!(
        parse(&["passthrough", "--bogus", "-h"]),
        early_line(&unknown_flag("--bogus"))
    );
    assert!(matches!(
        call(&["passthrough", "add", "--label", "--help"]),
        Ask::Passthrough { label, .. } if label == "--help"
    ));
    assert!(offline_verb(&["front", "x", "--help"]));
    assert!(!offline_verb(&["front", "help"]));
}

#[test]
fn the_words() {
    let front = |sub: &str, json: bool, mail: bool| Ask::Front {
        sub: sub.into(),
        json,
        mail,
    };
    assert_eq!(call(&[]), front("status", false, false));
    assert_eq!(call(&[""]), front("status", false, false));
    assert_eq!(
        call(&["enable", "--mail", "--json"]),
        front("enable", true, true)
    );
    // `--json` first is a verb the shell doesn't have.
    assert_eq!(call(&["--json"]), front("--json", false, false));
    assert_eq!(
        parse(&["status", "--bogus"]),
        early_line(&unknown_argument("--bogus"))
    );
    assert_eq!(
        call(&["http", "on", "off", "--json"]),
        Ask::Http {
            mode: "off".into(),
            json: true
        }
    );
    assert_eq!(
        call(&["http"]),
        Ask::Http {
            mode: String::new(),
            json: false
        }
    );
    assert_eq!(parse(&["http", "--mail"]), early_line(HTTP_USAGE));
    let pt =
        |verb: &str, json: bool, to: &str, proxy: bool, label: &str, host: &str| Ask::Passthrough {
            verb: verb.into(),
            json,
            to: to.into(),
            proxy,
            label: label.into(),
            host: host.into(),
        };
    assert_eq!(call(&["passthrough"]), pt("list", false, "", false, "", ""));
    assert_eq!(
        call(&["passthrough", ""]),
        pt("list", false, "", false, "", "")
    );
    assert_eq!(
        call(&["passthrough", "--json"]),
        pt("list", true, "", false, "", "")
    );
    assert_eq!(
        call(&[
            "passthrough",
            "add",
            "a",
            "b é",
            "--to",
            "127.0.0.1:1",
            "--to=127.0.0.1:2",
            "--proxy-v2",
            "--label",
            "",
        ]),
        pt("add", false, "127.0.0.1:2", true, "", "b é")
    );
    assert_eq!(parse(&["passthrough", "add", "--to"]), early_line(TO_USAGE));
    assert_eq!(
        parse(&["passthrough", "add", "--label"]),
        early_line(LABEL_USAGE)
    );
    assert_eq!(
        parse(&["passthrough", "-x"]),
        early_line(&unknown_flag("-x"))
    );
    assert_eq!(
        parse(&["passthrough", "rm", "--to-x"]),
        early_line(&unknown_flag("--to-x"))
    );
}

fn req(list: &[&str]) -> Result<Request, String> {
    request(&call(list))
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
            r.json,
            r.passthrough
        ),
        (true, "/cli/front/status", "", false, false)
    );
    let r = req(&["enable", "--json"]).unwrap();
    assert_eq!(
        (r.get, r.path.as_str(), r.payload.as_str(), r.json),
        (false, "/cli/front/enable", r#"{"mail":false}"#, true)
    );
    assert_eq!(
        req(&["enable", "--mail"]).unwrap().payload,
        r#"{"mail":true}"#
    );
    let r = req(&["disable"]).unwrap();
    assert_eq!(
        (r.path.as_str(), r.payload.as_str()),
        ("/cli/front/disable", "{}")
    );
    assert_eq!(req(&["disable", "--mail"]), Err(format!("{MAIL_USAGE}\n")));
    assert_eq!(
        req(&["status", "--mail"]).unwrap().path,
        "/cli/front/status"
    );
    assert_eq!(
        req(&["nosuch", "--mail"]),
        Err(format!("{}\n", unknown_verb("nosuch")))
    );
    assert_eq!(
        req(&["--json"]),
        Err(format!("{}\n", unknown_verb("--json")))
    );
    let r = req(&["http", "on"]).unwrap();
    assert_eq!(
        (r.get, r.path.as_str(), r.payload.as_str(), r.passthrough),
        (false, "/cli/front/http", r#"{"on":true}"#, false)
    );
    assert_eq!(req(&["http", "off"]).unwrap().payload, r#"{"on":false}"#);
    assert_eq!(req(&["http", "--json"]), Err(format!("{HTTP_USAGE}\n")));
    let r = req(&["passthrough", "ls", "--json"]).unwrap();
    assert_eq!(
        (
            r.get,
            r.path.as_str(),
            r.payload.as_str(),
            r.json,
            r.passthrough
        ),
        (true, "/cli/front/passthrough", "", true, true)
    );
    let r = req(&[
        "passthrough",
        "add",
        "a.example é",
        "--to",
        "127.0.0.1:8443",
        "--proxy-v2",
        "--label",
        "mine \"x\"",
    ])
    .unwrap();
    assert_eq!(
        (r.get, r.path.as_str(), r.payload.as_str()),
        (
            false,
            "/cli/front/passthrough/add",
            r#"{"host": "a.example \u00e9", "to": "127.0.0.1:8443", "proxyV2": true, "label": "mine \"x\""}"#
        )
    );
    assert_eq!(
        req(&["passthrough", "add", "*", "--to", "127.0.0.1:1"])
            .unwrap()
            .payload,
        r#"{"host": "*", "to": "127.0.0.1:1", "proxyV2": false, "label": ""}"#
    );
    assert_eq!(
        req(&["passthrough", "add", "h"]),
        Err(format!("{ADD_USAGE}\n"))
    );
    assert_eq!(
        req(&["passthrough", "add", "--to", "x"]),
        Err(format!("{ADD_USAGE}\n"))
    );
    let r = req(&["passthrough", "remove", "a"]).unwrap();
    assert_eq!(
        (r.path.as_str(), r.payload.as_str()),
        ("/cli/front/passthrough/rm", r#"{"host": "a"}"#)
    );
    assert_eq!(req(&["passthrough", "rm"]), Err(format!("{RM_USAGE}\n")));
    assert_eq!(
        req(&["passthrough", "show"]),
        Err(format!("{PASSTHROUGH_USAGE}\n"))
    );
}

fn shown(status: &str, body: &str) -> Shown {
    let mut r = front_lines(status, body.as_bytes());
    r.stdout = as_shell(&r.stdout);
    r.stderr = as_shell(&r.stderr);
    r
}

/// What `_front_render`'s Python printed (Python 3.12's exceptions).
#[test]
fn the_front_lines() {
    assert_eq!(
        shown(
            "200",
            r#"{"enabled":true,"serving":true,"socket":{"source":"systemd"},"mail":false,"http":{"on":true,"serving":false,"next":"restart","helper":"install the :80 socket"},"cap":4096,"e2eCap":512,"nofileSoftLimit":65536,"routes":[{"host":"a.example","kind":"app","backend":"127.0.0.1:8080","proxyV2":true},{"host":"*","kind":"passthrough","backend":"127.0.0.1:9443"}],"port443":{"holder":"caddy","message":"Caddy holds it"},"passthrough":[{"host":"x","to":"y"}],"guard":{"perIpRefused":3,"guardLimited":0},"connections":{"accepted":10,"live":2,"overCap":0,"closed":{"eof":5,"reset":0,"timeout":1}},"helper":"ok","socketUnit":{"state":{"ActiveState":"active","SubState":"listening","Result":"success"},"journal":["line one","line two"]}}"#
        ),
        Shown::out(concat!(
            "front      : on \u{b7} serving (socket from systemd)\n",
            "mail       : not behind the front\n",
            ":80        : on \u{b7} not serving (restart)\n",
            "note       : install the :80 socket\n",
            "cap        : 4096  (E2E listener 512; RLIMIT_NOFILE 65536)\n",
            "route      : a.example -> app 127.0.0.1:8080 PROXY v2\n",
            "route      : * -> passthrough 127.0.0.1:9443 no PROXY\n",
            ":443       : caddy - Caddy holds it\n",
            "App guard  : refused connections 3 \u{b7} limited writes 0\n",
            "conns      : accepted 10 \u{b7} live 2 \u{b7} over cap 0\n",
            "closed     : eof 5, timeout 1\n",
            "helper     : ok\n",
            "socket unit: active listening (success)\n",
            "  systemd  : line one\n",
            "  systemd  : line two\n",
        ))
    );
    assert_eq!(
        shown("200", r#"{"enabled":false,"serving":false,"restartNeeded":true,"passthrough":[{"host":"a","to":"127.0.0.1:1"},{"host":"*","to":"127.0.0.1:2"}],"port443":{"holder":"free"},"connections":{"closed":{}},"socketUnit":{"state":{}}}"#).stdout,
        concat!(
            "front      : off \u{b7} not serving \n",
            "note       : enabled but not serving: the daemon takes the socket at its next start\n",
            "mail       : not behind the front\n",
            "cap        : None  (E2E listener None; RLIMIT_NOFILE None)\n",
            "passthrough: a -> 127.0.0.1:1, * -> 127.0.0.1:2 (routes once the front is on)\n",
            "conns      : accepted None \u{b7} live None \u{b7} over cap None\n",
            "closed     : \n",
            "socket unit: None  (None)\n",
        )
    );
    assert_eq!(
        shown("200", r#"{"on":true,"next":"the daemon restarts once"}"#).stdout,
        ":80 front  : on\nnext       : the daemon restarts once\n"
    );
    assert_eq!(shown("200", r#"{"on":0}"#).stdout, ":80 front  : off\n");
    assert_eq!(
        shown(
            "200",
            r#"{"enabled":true,"mail":true,"restarting":true,"warnings":["w1",5],"next":"wait"}"#
        )
        .stdout,
        concat!(
            "front      : on\n",
            "mail       : behind the front\n",
            "restarting : K2 will restart its daemon once to take port 443\n",
            "warning    : w1\n",
            "warning    : 5\n",
            "next       : wait\n",
        )
    );
    assert_eq!(
        shown("200", r#"{"enabled":true,"restartNeeded":1}"#).stdout,
        "front      : on\nmail       : not behind the front\nrestart    : needed to take port 443 (see warning)\n"
    );
    assert_eq!(shown("200", "{}"), Shown::out("front      : off\n"));
    assert_eq!(
        shown("200", r#"{"error":"nope","serving":false}"#).stdout,
        "front      : off \u{b7} not serving \nmail       : not behind the front\ncap        : None  (E2E listener None; RLIMIT_NOFILE None)\n"
    );
    // What a refusal says, and its exit.
    for (status, body, stderr, exit) in [
        (
            "200",
            r#"{"ok":false,"error":{"code":"busy","hint":"try later"}}"#,
            "k2 front: error: busy: try later\n",
            Exit::Error,
        ),
        (
            "200",
            r#"{"error":"nope"}"#,
            "k2 front: error: nope\n",
            Exit::Error,
        ),
        (
            "200",
            "<html>",
            "k2 front: error: HTTP 200: <html>\n",
            Exit::Error,
        ),
        (
            "403",
            r#"{"error":{"code":"owner_only","hint":"Only the owner."},"permission":{"command":"ask x","requestable":true}}"#,
            "k2 front: error: owner_only: Only the owner.\nAsk with: ask x\n",
            Exit::Refused,
        ),
        (
            "403",
            r#"{"error":null}"#,
            "k2 front: error: None\n",
            Exit::Refused,
        ),
        (
            "403",
            "  boom\r\n ",
            "k2 front: error: HTTP 403: boom\n",
            Exit::Refused,
        ),
        ("403", "", "k2 front: error: HTTP 403: \n", Exit::Refused),
        (
            "404",
            r#"{"error":"nf"}"#,
            "k2 front: error: nf\n",
            Exit::Error,
        ),
        ("400", "x", "k2 front: error: HTTP 400: x\n", Exit::Error),
    ] {
        assert_eq!(
            shown(status, body),
            Shown::err(stderr, exit),
            "{status} {body}"
        );
    }
    // No card: the feedback line closes it (0.46.18) while the daemon says
    // agents may report...
    assert_eq!(
        shown(
            "403",
            r#"{"error":{"code":"c"},"message":"m","permission":{"requestable":false,"feedback":true}}"#
        ),
        Shown::err(
            format!(
                "k2 front: error: c: m\nIt can't be requested.\n{}\n",
                crate::transport::permission::NO_CARD_FEEDBACK
            ),
            Exit::Refused
        )
    );
    // ...and is left out while it doesn't.
    assert_eq!(
        shown(
            "403",
            r#"{"error":{"code":"c"},"message":"m","permission":{"requestable":false}}"#
        ),
        Shown::err("k2 front: error: c: m\nIt can't be requested.\n".to_string(), Exit::Refused)
    );
    let long = "é".repeat(201);
    assert_eq!(
        shown("500", &long).stderr,
        format!("k2 front: error: HTTP 500: {}\n", "é".repeat(200))
    );
    let header = "front      : off \u{b7} not serving \nmail       : not behind the front\n";
    let cap = "cap        : None  (E2E listener None; RLIMIT_NOFILE None)\n";
    let tb = |r: Raise| r.text();
    for (status, body, stdout, stderr) in [
        (
            "200",
            "[]".to_string(),
            String::new(),
            tb(Raise::at(
                8,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "403",
            "[1]".to_string(),
            String::new(),
            tb(Raise::at(
                9,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "403",
            r#"{"error":"x","message":5}"#.to_string(),
            String::new(),
            tb(Raise::at(
                11,
                "TypeError: can only concatenate str (not \"int\") to str",
            )),
        ),
        (
            "403",
            r#"{"error":{"hint":7}}"#.to_string(),
            String::new(),
            tb(Raise::at(
                11,
                "TypeError: can only concatenate str (not \"int\") to str",
            )),
        ),
        (
            "403",
            r#"{"permission":{"command":5}}"#.to_string(),
            String::new(),
            format!(
                "k2 front: error: None\n{}",
                tb(Raise::at(
                    15,
                    "TypeError: can only concatenate str (not \"int\") to str"
                ))
            ),
        ),
        (
            "200",
            r#"{"serving":1,"socket":[1]}"#.to_string(),
            String::new(),
            tb(Raise::at(
                21,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"http":[1]}"#.to_string(),
            header.to_string(),
            tb(Raise::at(
                27,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"http":{"next":5}}"#.to_string(),
            header.to_string(),
            tb(Raise::at(
                27,
                "TypeError: can only concatenate str (not \"int\") to str",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"routes":[1]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                32,
                "AttributeError: 'int' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"routes":"x"}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                32,
                "AttributeError: 'str' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"port443":[1]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                34,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"passthrough":[{"to":"y"}]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::inside(
                38,
                38,
                "<genexpr>",
                "TypeError: unsupported operand type(s) for +: 'NoneType' and 'str'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"passthrough":[{"host":"x"}]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::inside(
                38,
                38,
                "<genexpr>",
                "TypeError: can only concatenate str (not \"NoneType\") to str",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"passthrough":[5]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::inside(
                38,
                38,
                "<genexpr>",
                "AttributeError: 'int' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"passthrough":[{"host":[1],"to":"y"}]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::inside(
                38,
                38,
                "<genexpr>",
                "TypeError: can only concatenate list (not \"str\") to list",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"passthrough":[{"host":5,"to":"y"}]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::inside(
                38,
                38,
                "<genexpr>",
                "TypeError: unsupported operand type(s) for +: 'int' and 'str'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"guard":[1]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                41,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"connections":{"closed":[1]}}"#.to_string(),
            format!(
                "{header}{cap}conns      : accepted None \u{b7} live None \u{b7} over cap None\n"
            ),
            tb(Raise::at(
                46,
                "AttributeError: 'list' object has no attribute 'items'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"socketUnit":[1]}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                51,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"socketUnit":{"state":[1]}}"#.to_string(),
            format!("{header}{cap}"),
            tb(Raise::at(
                52,
                "AttributeError: 'list' object has no attribute 'get'",
            )),
        ),
        (
            "200",
            r#"{"serving":0,"socketUnit":{"journal":5}}"#.to_string(),
            format!("{header}{cap}socket unit: None  (None)\n"),
            tb(Raise::at(53, "TypeError: 'int' object is not iterable")),
        ),
        (
            "200",
            r#"{"warnings":5}"#.to_string(),
            "front      : off\n".to_string(),
            tb(Raise::at(68, "TypeError: 'int' object is not iterable")),
        ),
    ] {
        assert_eq!(
            shown(status, &body),
            Shown {
                stdout,
                stderr,
                exit: Exit::Error,
            },
            "{status} {body}"
        );
    }
}

fn listed(body: &str) -> Shown {
    let mut r = passthrough_lines(body.as_bytes());
    r.stdout = as_shell(&r.stdout);
    r
}

/// What `_front_passthrough`'s Python printed.
#[test]
fn the_passthrough_lines() {
    let none = "no passthrough names (k2 front passthrough add <host> --to 127.0.0.1:<port>)\n";
    assert_eq!(
        listed(
            r#"{"passthrough":[{"host":"a.example","to":"127.0.0.1:8443","proxyV2":true,"label":"mine"},{"host":"*","to":"127.0.0.1:9443"}],"warnings":["w"],"next":"restart"}"#
        ),
        Shown::out(concat!(
            "a.example                        -> 127.0.0.1:8443  PROXY v2  (mine)\n",
            "*                                -> 127.0.0.1:9443\n",
            "warning: w\n",
            "note: restart\n",
        ))
    );
    assert_eq!(
        listed(r#"{"passthrough":[],"next":"routed now"}"#),
        Shown::out(none)
    );
    assert_eq!(listed("{}"), Shown::out(none));
    for (body, stdout, at, exception) in [
        (
            "[]",
            "",
            4,
            "AttributeError: 'list' object has no attribute 'get'",
        ),
        (
            r#"{"passthrough":5}"#,
            "",
            7,
            "TypeError: 'int' object is not iterable",
        ),
        (
            r#"{"passthrough":[{"label":5}]}"#,
            "",
            8,
            "TypeError: can only concatenate str (not \"int\") to str",
        ),
        (
            r#"{"passthrough":"ab"}"#,
            "",
            8,
            "AttributeError: 'str' object has no attribute 'get'",
        ),
        (
            r#"{"warnings":[5]}"#,
            none,
            10,
            "TypeError: can only concatenate str (not \"int\") to str",
        ),
        (
            r#"{"next":5}"#,
            none,
            12,
            "TypeError: can only concatenate str (not \"int\") to str",
        ),
    ] {
        assert_eq!(
            listed(body),
            Shown {
                stdout: stdout.into(),
                stderr: Raise::at(at, exception).text(),
                exit: Exit::Error,
            },
            "{body}"
        );
    }
    let r = listed("<html>");
    assert_eq!((r.stdout.as_str(), r.exit), ("", Exit::Error));
    assert!(r.stderr.contains("line 3, in <module>") && r.stderr.contains("JSONDecodeError"));
}

#[test]
fn the_render() {
    let request = |json: bool, passthrough: bool| Request {
        get: true,
        path: String::new(),
        payload: String::new(),
        json,
        passthrough,
    };
    let r = render(&request(true, false), "000", b"");
    assert_eq!(
        (as_shell(&r.stderr).as_str(), r.exit),
        ("k2 front: daemon unreachable\n", Exit::Error)
    );
    // --json: the body as it came, and each renderer's exit.
    for (passthrough, status, exit) in [
        (false, "200", Exit::Ok),
        (false, "403", Exit::Refused),
        (false, "404", Exit::Error),
        (false, "400", Exit::Error),
        (true, "200", Exit::Ok),
        (true, "403", Exit::Refused),
        (true, "404", Exit::NotFound),
        (true, "413", Exit::Usage),
        (true, "401", Exit::Error),
    ] {
        let r = render(&request(true, passthrough), status, b"{\"a\":\xff}");
        assert_eq!(
            (r.stdout.as_slice(), r.stderr.as_str(), r.exit),
            (&b"{\"a\":\xff}\n"[..], "", exit),
            "{passthrough} {status}"
        );
    }
    // A passthrough refusal: the firewall's lines and exits.
    let r = render(&request(false, true), "404", br#"{"error":"nope"}"#);
    assert_eq!(
        (r.stdout.as_slice(), as_shell(&r.stderr).as_str(), r.exit),
        (&b""[..], "k2 firewall: error: nope\n", Exit::NotFound)
    );
    let r = render(&request(false, false), "404", br#"{"error":"nope"}"#);
    assert_eq!(
        (as_shell(&r.stderr).as_str(), r.exit),
        ("k2 front: error: nope\n", Exit::Error)
    );
}

/// Under any brand, the lines name the command and the product by the
/// brand's names.
#[test]
fn the_lines_say_the_brand() {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for line in [
        UNREACHABLE.to_string(),
        ERROR.to_string(),
        HTTP_USAGE.into(),
        ADD_USAGE.into(),
        RM_USAGE.into(),
        PASSTHROUGH_USAGE.into(),
        NO_NAMES.into(),
        unknown_argument("x"),
        unknown_flag("-x"),
    ] {
        assert!(line.contains(&format!("{cli} front")), "{line}");
        if cli != "k2" {
            assert!(!line.contains("k2"), "{line}");
        }
    }
    let r = front_lines("200", br#"{"enabled":true,"restarting":true}"#);
    assert!(
        r.stdout.contains(&format!(": {product} will restart")),
        "{}",
        r.stdout
    );
}

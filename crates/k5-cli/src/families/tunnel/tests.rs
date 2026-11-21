use super::status::{self, line, sub_line};
use super::*;

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The Python of the first `python3 -c '` after `after` in usage.sh, as
/// `python3 -c` numbers its lines (line 1 is the empty one after the
/// quote).
fn python(after: &str) -> Vec<String> {
    let text = source("usage.sh");
    let body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in usage.sh"))
        .1;
    let py = &body[body.find("python3 -c '").expect("python3 -c") + "python3 -c '".len()..];
    let end = py.find("\n'").expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

fn at(lines: &[String], n: u32) -> &str {
    lines[n as usize - 1].trim()
}

/// Each line a reader raises at, as the shell's Python has it.
#[test]
fn the_readers_lines() {
    let start = python("cli_post \"/cli/tunnel/start\" $extra");
    assert_eq!(at(&start, 7), r#"if d.get("error"):"#);
    assert_eq!(at(&start, 10), r#"url = d.get("public_url") or """#);
    assert_eq!(at(&start, 13), r#"if d.get("repaired"):"#);
    assert!(at(&start, 14)
        .starts_with(r#"print("Tunnel was running but not serving: " + d["repaired"])"#));
    let stop = python("cli_post \"/cli/tunnel/stop\"");
    assert_eq!(at(&stop, 7), r#"if d.get("ok"):"#);
    assert_eq!(at(&stop, 10), r#"err = d.get("error") or "unknown error""#);
    let disable = python("cli_post \"/cli/tunnel/disable\"");
    assert_eq!(at(&disable, 7), r#"if d.get("error"):"#);
    let enable = python("cli_post \"/cli/tunnel/enable\"");
    assert_eq!(at(&enable, 7), r#"if d.get("error"):"#);
    assert_eq!(at(&enable, 10), r#"if d.get("running"):"#);
    assert_eq!(at(&enable, 16), r#"if d.get("autoStart") is True:"#);
    let switch = python("cli_post_json \"/cli/tunnel/switch\"");
    assert_eq!(at(&switch, 7), r#"if d.get("error") and "steps" not in d:"#);
    assert_eq!(at(&switch, 10), r#"for s in d.get("steps") or []:"#);
    assert_eq!(at(&switch, 11), r#"mark = "ok " if s.get("ok") else "ERR""#);
    assert_eq!(at(&switch, 12), r#"step = s.get("step", "")"#);
    assert_eq!(at(&switch, 13), r#"detail = s.get("detail", "")"#);
    let body = python("body=$(printf '%s' \"$token\"");
    assert_eq!(at(&body, 4), "t = sys.stdin.read()");
    let release = python("_k2_tunnel_release_print() {");
    assert_eq!(at(&release, 7), r#"if d.get("error"):"#);
    assert_eq!(
        at(&release, 10),
        r#"sub = (d.get("subdomain") or "").strip()"#
    );
    let show = python("cli_request \"/cli/tunnel/status\" |");
    assert_eq!(at(&show, 7), r#"if d.get("error"):"#);
    let set = python("cli_post_json \"/cli/settings/update\"");
    assert_eq!(at(&set, 8), r#"if "error" in d:"#);
    assert_eq!(
        at(&set, 9),
        r#"print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)"#
    );
    assert!(at(&set, 10).starts_with(
        r#"print("Boot status: %s (live now, no restart)" % (d.get("bootStatusPublic")"#
    ));
}

#[test]
fn the_status_reader_s_lines() {
    let st = python("raw=$(cli_request \"/cli/tunnel/status\")");
    for (n, text) in [
        (
            line::RUNNING,
            r#"running = "yes" if d.get("running") else "no""#,
        ),
        (
            line::LAST_ERROR,
            r#"print("  Last error: " + d["frpc_last_error"])"#,
        ),
        (
            line::CONFIG,
            r#"print("  Config:     MISSING " + (d.get("frpc_config") or ""))"#,
        ),
        (
            line::TOKEN,
            r#"print("  Token:      LOCKED — " + d["token_locked"])"#,
        ),
        (line::VI_WORDS, r#"if vi.get("words"):"#),
        (line::WORDS, r#"print("  " + vi["words"])"#),
        (line::VI_RELAY, r#"if vi.get("relayAsClient"):"#),
        (
            line::SERVER_PORT,
            r#"sa = sa + ":" + str(d["server_port"])"#,
        ),
        (line::PRIMARY, r#"sa += " (primary relay)""#),
        (
            line::FALLBACK,
            r#"sa += " (fallback relay; returns to the primary once it answers again)""#,
        ),
        (line::DETAIL, r#"print("  Failover:   " + fo["detail"])"#),
        (
            line::RELAYS,
            r#"relays = ", ".join(rl.get("relays") or []) or "(none)""#,
        ),
        (
            line::REPLACED,
            r#"src += "; replaces the old built-in " + ", ".join(rl["replaced"])"#,
        ),
        (
            line::CONFIG_FILE,
            r#"print("  Tunnel config: " + rl["config_file"])"#,
        ),
        (
            line::CHECKED,
            r#"ts = datetime.datetime.fromtimestamp(rl["checked_at"]).strftime("%Y-%m-%d %H:%M")"#,
        ),
        (
            line::ACTIVE,
            r#"print("  Live relay: *.k2.dev points at " + rl["active"] + " (per k2.dev)")"#,
        ),
        (
            line::FAILED,
            r#"print("  Relay list: last refresh failed: " + rl["last_error"])"#,
        ),
        (line::NOTICE, r#"print("  Relay list: " + rl["notice"])"#),
        (line::TURN, r#"what += " for " + vl["turnHost"]"#),
        (
            line::VIA,
            r#"what += " via " + vl["serverAddr"] + ":" + str(vl.get("serverPort") or "")"#,
        ),
        (line::ADOPTED, r#"what += " (adopted)""#),
        (line::VOICE, r#"print("  Voice lane: " + what)"#),
        (
            line::VOICE_ERROR,
            r#"print("  Voice lane error: " + vl["lastError"])"#,
        ),
        (
            line::MEDIA_GET,
            r#"if mp.get("state") and mp.get("state") != "off":"#,
        ),
        (line::LI_SUB, r#"lsub = li.get("subdomain", "")"#),
        (line::LI_PORT, r#"lport = li.get("port", "")"#),
        (line::WANT, r#"want = (d.get("subdomain") or "").strip()"#),
    ] {
        assert_eq!(at(&st, n), text, "line {n}");
    }
    assert!(at(&st, line::BY).starts_with(r#"by = {"env": "#));
    assert!(at(&st, line::WHAT).starts_with(r#"what = "legacy (pre-0.46.6"#));
    assert!(at(&st, line::VOICE_WHAT).starts_with(r#"what = {"off": "off""#));
    assert!(at(&st, line::MEDIA).starts_with(r#"print("  Media path: " + mp["state"]"#));
    let sub = python("K2_PROJECTS_JSON=\"$projects_raw\"");
    for (n, text) in [
        (sub_line::ERROR, r#"if d.get("error"):"#),
        (
            sub_line::PRIMARY,
            r#"primary = (d.get("primary") or "").strip()"#,
        ),
        (sub_line::SORTED, "for label in sorted(targets):"),
        (sub_line::INDEX, "t = targets[label]"),
        (
            sub_line::NAMES,
            r#"ws = names.get(pid, pid) if pid else "-""#,
        ),
        (
            sub_line::HW,
            r#"hw = max([len(r[0]) for r in rows] + [len("HOST")])"#,
        ),
        (
            sub_line::TW,
            r#"tw = max([len(r[1]) for r in rows] + [len("TARGET")])"#,
        ),
    ] {
        assert_eq!(at(&sub, n), text, "line {n}");
    }
}

/// The case's arms and the lines the binary prints itself, as
/// `cmd_tunnel` has them (the command's name is the brand's).
#[test]
fn the_shell_s_arms_and_lines() {
    let text = source("usage.sh");
    let body = &text[text.find("\ncmd_tunnel() {\n").expect("cmd_tunnel")..];
    let body = &body[..body.find("\n}\n").expect("its end")];
    let arms: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("        "))
        .filter(|l| {
            l.strip_suffix(')').is_some_and(|w| {
                !w.is_empty()
                    && w.chars()
                        .all(|c| c.is_ascii_lowercase() || "*|-".contains(c))
            })
        })
        .collect();
    assert_eq!(
        arms,
        [
            "start)",
            "stop)",
            "disable)",
            "enable)",
            "switch)",
            "release)",
            "forget)",
            "status)",
            "subdomains)",
            "--help|-h|help)",
            "boot-status)",
            "*)",
        ]
    );
    assert!(body.contains("    local sub=\"${1:-status}\"\n"));
    // The shell's names, whatever the brand.
    let k2 = |l: &str| {
        l.replace(k2_client::brand::CLI_NAME, "k2")
            .replace(k2_client::brand::PRODUCT_NAME, "K2")
    };
    let echo = |l: &str| format!("echo \"{}\" >&2", k2(l).replace('"', "\\\""));
    for l in UNKNOWN
        .iter()
        .chain(BOOT_USAGE.iter())
        .chain(FORGET_LINES.iter())
        .chain(RELEASE_REST.iter())
        .chain([RELEASE_HEAD, RELEASE_NO_SUB].iter())
    {
        assert!(
            body.lines().any(|b| b.trim() == echo(l)),
            "cmd_tunnel no longer prints {l:?}"
        );
    }
    assert!(body.contains(
        r#"echo "  - ${_sub}.k2.dev stops routing here; this copy of the identity is refused" >&2"#
    ));
    assert!(body.contains(&format!(
        "                echo \"{}\" >&2\n                exit 2",
        k2(SWITCH_USAGE)
    )));
    for l in [
        r#"-*) echo "k2 tunnel switch: unknown flag $1" >&2; exit 2 ;;"#,
        r#"[ -z "$token" ] || { echo "k2 tunnel switch: --token-stdin or --token, not both" >&2; exit 2; }"#,
        r#"_cli_read_secret_stdin "Tunnel token: ""#,
        r#"[ -n "$token" ] || { echo "k2 tunnel switch: --token-stdin: no token on stdin" >&2; exit 2; }"#,
        r#"_k2_tunnel_release_print "$(cli_post "/cli/tunnel/release" "confirm=1")""#,
        r#"_k2_tunnel_release_print "$(cli_post "/cli/tunnel/release" "confirm=1&mode=forget")""#,
        r#"projects_raw=$(cli_request "/cli/companion/projects" 2>/dev/null)"#,
        r#"' "$mode" || exit 1"#,
    ] {
        assert!(
            body.lines().any(|b| b.trim() == l),
            "cmd_tunnel no longer has {l:?}"
        );
    }
    assert_eq!(body.matches("_k2_refuse_if_airgap").count(), 2);
}

/// `tunnel` is an open tool: the router's session checks pass it.
#[test]
fn tunnel_is_an_open_tool() {
    assert!(!crate::tools::is_id(crate::tools::tool_id("tunnel", "")));
}

#[test]
fn the_words() {
    let p = |w: &[&str]| parse(w);
    assert_eq!(p(&[]), Some(Form::Status(false)));
    assert_eq!(p(&[""]), Some(Form::Status(false)));
    assert_eq!(p(&["status", "--json", "x"]), Some(Form::Status(true)));
    assert_eq!(p(&["status", "x", "--json"]), Some(Form::Status(false)));
    assert_eq!(p(&["subdomains", "--json"]), Some(Form::Subdomains(true)));
    assert_eq!(p(&["start"]), Some(Form::Start(None)));
    assert_eq!(p(&["start", "--subdomain", ""]), Some(Form::Start(None)));
    assert_eq!(
        p(&["start", "x", "--subdomain", "a b", "--subdomain", "acme"]),
        Some(Form::Start(Some("subdomain=acme".into())))
    );
    assert_eq!(
        p(&["start", "x", "--subdomain"]),
        Some(Form::NeedsValue("--subdomain"))
    );
    assert_eq!(
        p(&["enable", "--no-auto-start", "--subdomain", "é"]),
        Some(Form::Enable("auto_start=0&subdomain=%C3%A9".into()))
    );
    assert_eq!(
        p(&["enable", "--no-auto-start", "--auto-start"]),
        Some(Form::Enable("auto_start=1".into()))
    );
    assert_eq!(
        p(&["enable", "--subdomain"]),
        Some(Form::NeedsValue("--subdomain"))
    );
    assert_eq!(p(&["release", "x", "--confirm"]), Some(Form::Release(true)));
    assert_eq!(p(&["forget"]), Some(Form::Forget(false)));
    assert_eq!(p(&["help", "x"]), Some(Form::Help));
    assert_eq!(p(&["-h"]), None);
    assert_eq!(p(&["boot-status"]), Some(Form::Boot(Boot::Show)));
    assert_eq!(p(&["boot-status", ""]), Some(Form::Boot(Boot::Show)));
    assert_eq!(
        p(&["boot-status", "ghost", "x"]),
        Some(Form::Boot(Boot::Set("ghost")))
    );
    assert_eq!(p(&["boot-status", "Ghost"]), Some(Form::Boot(Boot::Usage)));
    assert_eq!(p(&["Status"]), Some(Form::Unknown));
    let ask = |sub: &str, token: &str, stdin, start| {
        Some(Form::Switch(Switch::Ask {
            subdomain: sub.into(),
            token: token.into(),
            stdin,
            start,
        }))
    };
    assert_eq!(p(&["switch", "a", "b"]), ask("b", "", false, None));
    assert_eq!(
        p(&["switch", "a", "--token-stdin", "--no-start"]),
        ask("a", "", true, Some(false))
    );
    assert_eq!(
        p(&["switch", "--start", "a", "--token", "t"]),
        ask("a", "t", false, Some(true))
    );
    let stop = |l: String| Some(Form::Switch(Switch::Stop(l)));
    assert_eq!(p(&["switch"]), stop(SWITCH_USAGE.into()));
    assert_eq!(
        p(&["switch", "a", "-x"]),
        stop(format!("{SWITCH_PREFIX}unknown flag -x"))
    );
    assert_eq!(
        p(&["switch", "a", "-"]),
        stop(format!("{SWITCH_PREFIX}unknown flag -"))
    );
    assert_eq!(
        p(&["switch", "a", "--token", "t", "--token-stdin"]),
        stop(format!("{SWITCH_PREFIX}{SWITCH_BOTH}"))
    );
    assert_eq!(
        p(&["switch", "a", "--token"]),
        stop("Error: --token takes a value".into())
    );
}

#[test]
fn the_switch_body() {
    assert_eq!(
        switch_body("acme", &Text::from(""), None),
        r#"{"subdomain": "acme"}"#
    );
    assert_eq!(
        switch_body("é", &Text::from("t\"k"), Some(false)),
        r#"{"subdomain": "\u00e9", "token": "t\"k", "start": false}"#
    );
    assert_eq!(
        switch_body("a", &escaped(b"x\xffy"), Some(true)),
        r#"{"subdomain": "a", "token": "x\udcffy", "start": true}"#
    );
}

#[test]
fn release_reads_tunnel_json_as_python_does() {
    let d = std::env::temp_dir().join(format!("k5-tunnel-json-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("tunnel.json");
    for (body, want) in [
        (&b"{\"subdomain\": \" acme \\n\"}"[..], Some("acme")),
        (b"{\"subdomain\": \"\"}", None),
        (b"{\"subdomain\": 5}", None),
        (b"[1]", None),
        (b"{\"subdomain\": \"\\ud800\"}", None),
        (b"{\"subdomain\": \"a\xff\"}", None),
        (b"not json", None),
    ] {
        std::fs::write(&f, body).unwrap();
        assert_eq!(tunnel_json_subdomain(&f).as_deref(), want, "{body:?}");
    }
    std::fs::remove_dir_all(&d).unwrap();
    assert_eq!(tunnel_json_subdomain(&d.join("tunnel.json")), None);
}

fn shown(stdout: &str, stderr: &str, exit: Exit) -> Shown {
    Shown {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit,
    }
}

#[test]
fn the_small_readers() {
    let empty = "Failed to parse daemon response: Expecting value: line 1 column 1 (char 0)\n";
    for reader in [
        start_reader,
        stop_reader,
        disable_reader,
        enable_reader,
        switch_reader,
        release_reader,
    ] {
        assert_eq!(reader(b""), shown("", empty, Exit::Error));
    }
    assert_eq!(
        start_reader(br#"{"public_url":"https://a.k2.dev","repaired":"frpc died"}"#),
        shown(
            "Tunnel was running but not serving: frpc died\nRe-rendered the frpc config and restarted frpc.\nTunnel started. Public URL: https://a.k2.dev\n",
            "",
            Exit::Ok
        )
    );
    assert_eq!(
        start_reader(br#"{"error":{"code":"x"}}"#),
        shown("", "Failed to start tunnel: {'code': 'x'}\n", Exit::Error)
    );
    assert_eq!(
        stop_reader(br#"{"ok":0}"#).stderr,
        "Failed to stop tunnel: unknown error\n"
    );
    let e = enable_reader(br#"{"running":true,"autoStart":false}"#);
    assert_eq!(
        e.stdout,
        format!("Tunnel enabled and running. Public URL: (subdomain assigned by the server)\nAuto-start: off (after a restart, run {} tunnel enable again).\n", k2_client::brand::CLI_NAME)
    );
    let sw = switch_reader(br#"{"ok":false,"error":"busy","rolledBack":1,"steps":[{"ok":true,"step":"bind"},{"step":null,"detail":2}]}"#);
    assert_eq!(sw.stdout, "  [ok ] bind: \n  [ERR] None: 2\n");
    assert_eq!(
        sw.stderr,
        "Switch failed: busy\nRolled back: the previous subdomain is still in place.\n"
    );
    assert_eq!(sw.exit, Exit::Error);
    let raised = switch_reader(br#"{"steps":"ab"}"#);
    assert!(raised
        .stderr
        .ends_with("AttributeError: 'str' object has no attribute 'get'\n"));
    let r = release_reader(br#"{"subdomain":" a ","localOnly":1,"queued":1,"note":"n"}"#);
    assert_eq!(
        r.stdout.lines().next(),
        Some("Forgotten on this device: a.k2.dev no longer routes here.")
    );
    assert!(release_reader(br#"{"subdomain":3}"#)
        .stderr
        .ends_with("AttributeError: 'int' object has no attribute 'strip'\n"));
    assert_eq!(
        boot_set_reader(b"<html>", "ghost").stderr,
        "Error: daemon returned an unparseable response: '<html>'\n"
    );
    assert_eq!(
        boot_set_reader(br#"{"bootStatusPublic":""}"#, "compat").stdout,
        "Boot status: compat (live now, no restart)\n"
    );
    assert!(boot_set_reader(b"5", "ghost")
        .stderr
        .ends_with("TypeError: argument of type 'int' is not iterable\n"));
    assert_eq!(boot_show_reader(b"{}").stdout, "This daemon predates boot-status views (0.46.3): every caller gets the full /boot-status.\n");
}

#[test]
fn the_status_reader() {
    let r = status::status_reader(br#"[]"#);
    assert!(r.stdout.ends_with(" Connect tunnel\n"));
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    let r = status::status_reader(
        br#"{"running":true,"frpc_healthy":false,"frpc_consecutive_failures":3,"frpc_config_present":false,"public_url":"u","server_addr":"relay","server_port":7000,"relay_primary":false,"autoStart":true}"#,
    );
    let cli = k2_client::brand::CLI_NAME;
    assert_eq!(
        r.stdout.lines().skip(1).collect::<Vec<_>>(),
        [
            "  Running:    yes, but frpc is FAILING (3 consecutive failures)",
            "  Config:     MISSING ",
            &format!("  Repair:     {cli} tunnel start"),
            "  Public URL: u",
            "  Server:     relay:7000 (fallback relay; returns to the primary once it answers again)",
            "  Auto-start: on",
        ]
    );
    let r = status::status_reader(br#"{"relay_list":{"relays":["a",1]}}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: sequence item 1: expected str instance, int found\n"));
    let r =
        status::status_reader(br#"{"relay_list":{"relays":"ab","source":"x","replaced":["c"]}}"#);
    assert!(r
        .stdout
        .contains("  Relays:     a, b [x; replaces the old built-in c]\n"));
}

#[test]
fn the_subdomains_reader() {
    let r = status::subdomains_reader(
        br#"{"primary":"acme","targets":{"b":{"target":"127.0.0.1:3000","projectId":"p1"},"a":"x","c":{"projectId":"p9"}}}"#,
        br#"[{"id":"p1","name":"Shop"},{"id":[1]},{"id":"p9","name":"late"}]"#,
    );
    let row = |h: &str, t: &str, w: &str| {
        format!(
            "  {}  {}  {w}",
            super::super::ljust(h, 13),
            super::super::ljust(t, 16)
        )
    };
    assert_eq!(
        r.stdout.lines().skip(1).collect::<Vec<_>>(),
        [
            row("HOST", "TARGET", "WORKSPACE"),
            row("acme.k2.dev", "daemon (primary)", "-"),
            row("a.acme.k2.dev", "x", "-"),
            row("b.acme.k2.dev", "127.0.0.1:3000", "Shop"),
            row("c.acme.k2.dev", "?", "p9"),
        ]
    );
    let r = status::subdomains_reader(br#"{}"#, b"");
    assert_eq!(r.exit, Exit::Ok);
    assert!(r
        .stdout
        .ends_with("   requires the tunnel running with E2E on)\n"));
    let r = status::subdomains_reader(br#"{"targets":{"a":{"target":5}}}"#, b"");
    assert!(r
        .stderr
        .ends_with("TypeError: object of type 'int' has no len()\n"));
}

#[test]
fn from_timestamp_as_python() {
    assert!(status::from_timestamp(&Value::Int("0".into())).is_ok());
    assert!(status::from_timestamp(&Value::Str(Text::from("x")))
        .unwrap_err()
        .starts_with("TypeError"));
    assert!(status::from_timestamp(&Value::Float(f64::NAN))
        .unwrap_err()
        .starts_with("ValueError"));
}

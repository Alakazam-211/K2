use super::call::{self, Cj, Split};
use super::py::Out;
use super::wallet;
use super::*;
use crate::transport::{Channel, Response};

fn answer(status: u16, body: &str) -> Response {
    Response {
        status,
        body: body.as_bytes().to_vec(),
        channel: Channel::Tcp,
        permission: None,
        server_version: None,
    }
}

/// The command's name as the shell CLI's source types it.
const SHELL_NAME: &str = "k2";

fn shell_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn what_the_shell_holds_of_compute_send() {
    assert_eq!(call::held(&Ok(answer(200, "{}\n\n")), 0), b"{}\n\n\n200");
    assert_eq!(
        call::held(&Ok(answer(404, "a\0b")), 2),
        b"\n000\n000ab\n404"
    );
    let none: Result<Response, crate::transport::Error> = Err(crate::transport::Error::TimedOut {
        endpoint: "/x".into(),
    });
    assert_eq!(call::held(&none, 0), b"\n000\n000");
    assert_eq!(call::held(&none, 3), b"\n000\n000\n000\n000\n000");
}

#[test]
fn compute_split_splits_at_the_last_newline() {
    assert_eq!(
        call::split(b"{\"a\":1}\n\n200"),
        Split {
            status: "200".into(),
            body: b"{\"a\":1}\n".to_vec()
        }
    );
    assert_eq!(
        call::split(b"\n000\n000"),
        Split {
            status: "000".into(),
            body: b"\n000".to_vec()
        }
    );
    assert_eq!(
        call::split(b"200"),
        Split {
            status: "200".into(),
            body: Vec::new()
        }
    );
    assert_eq!(call::split(b"").status, "000");
}

#[test]
fn compute_fail_lines() {
    // The daemon's error object; a registry row adds next and explain.
    let line = call::fail_line(
        "409",
        br#"{"error":{"code":"app_exists","hint":"taken"}}"#,
        false,
    );
    let mut lines = line.lines();
    let first = lines.next().unwrap();
    assert!(
        first.starts_with(r#"{"error": "app_exists", "hint": "taken", "next": ""#),
        "{first}"
    );
    assert!(
        first.ends_with(&format!(
            r#", "explain": "{} explain app_exists"}}"#,
            k2_client::brand::CLI_NAME
        )),
        "{first}"
    );
    assert!(lines.next().unwrap().starts_with("fix: Deploy to it"));
    assert_eq!(lines.next(), None);
    // --json: no fix line.
    assert_eq!(
        call::fail_line("409", br#"{"error":{"code":"app_exists"}}"#, true)
            .lines()
            .count(),
        1
    );
    // No row: the code and hint only, non-ASCII escaped.
    assert_eq!(
        call::fail_line(
            "403",
            r#"{"error":{"code":"nope","hint":"é"}}"#.as_bytes(),
            false
        ),
        r#"{"error": "nope", "hint": "\u00e9"}"#
    );
    // The route floor's shape.
    assert_eq!(
        call::fail_line("403", br#"{"error":"nope","message":"m"}"#, false),
        r#"{"error": "nope", "hint": "m"}"#
    );
    // Anything else.
    assert_eq!(
        call::fail_line("500", br#"{"error":"x"}"#, false),
        r#"{"error": "error", "hint": "x"}"#
    );
    assert_eq!(
        call::fail_line("500", br#"[1]"#, false),
        r#"{"error": "error", "hint": "None"}"#
    );
    // Not JSON, and the no-answer lines.
    assert_eq!(
        call::fail_line("000", b"\n000\n000", false),
        "daemon_error: \n000\n000"
    );
    assert_eq!(
        call::fail_line("502", b"", false),
        format!(
            r#"{{"error":"daemon_unreachable","hint":"no answer from the {} daemon (status 502)"}}"#,
            k2_client::brand::PRODUCT_NAME
        )
    );
    assert_eq!(call::fail_exit("400"), Exit::Usage);
    assert_eq!(call::fail_exit("413"), Exit::Usage);
    assert_eq!(call::fail_exit("404"), Exit::Refused);
    assert_eq!(call::fail_exit("000"), Exit::Refused);
}

#[test]
fn the_registry_is_the_shell_s() {
    let json = call::registry_json();
    assert!(
        json.starts_with(r#"{"areas":["#),
        "{}",
        &json[..40.min(json.len())]
    );
    assert!(!json.contains('\n'));
    assert!(crate::out::json::parse(json).is_ok());
}

#[test]
fn compute_json_as_python_prints_it() {
    assert_eq!(
        Cj::new()
            .s("app", "a é")
            .b("enabled", "1")
            .b("fallback", "0")
            .body(),
        r#"{"app": "a \u00e9", "enabled": true, "fallback": false}"#
    );
    assert_eq!(Cj::new().s("app", "").b("enabled", "").body(), "{}");
}

#[test]
fn wallet_words() {
    let p = |w: &[&str]| wallet::parse(w);
    let a = p(&["settings", "acme", "--on", "--fallback", "off", "--json"]).unwrap();
    assert_eq!(
        (a.a1.as_str(), a.on.as_str(), a.fallback.as_str(), a.json),
        ("acme", "1", "0", true)
    );
    assert_eq!(
        p(&["settings", "--fallback"]).unwrap_err(),
        "usage: --fallback on|off"
    );
    assert_eq!(
        p(&["settings", "--fallback", "x"]).unwrap_err(),
        "usage: --fallback on|off"
    );
    assert_eq!(p(&["people", "--zap"]).unwrap_err(), "Unknown flag: --zap");
    assert_eq!(p(&["x", "a", "b", "c", "d"]).unwrap_err(), "unexpected: d");
    let a = p(&["personal-agent", "", "a", "b"]).unwrap();
    assert_eq!((a.a1.as_str(), a.a2.as_str()), ("a", "b"));
    assert!(p(&["people", "-h", "--zap"]).unwrap().help);
    assert!(parse(&["nosuch"]).is_none());
    assert!(parse(&[]).is_none());
}

fn printed(f: fn(&[u8], &mut Out) -> Result<Exit, crate::families::Raise>, body: &str) -> Shown {
    let mut o = Out::default();
    let r = f(body.as_bytes(), &mut o);
    o.done(r)
}

#[test]
fn wallet_printers() {
    let s = printed(
        wallet::settings_lines,
        r#"{"appId":"acme","settings":{"serviceId":"svc","enabled":true,"fallback":0}}"#,
    );
    assert_eq!(
        s.stdout,
        "acme (svc): wallet on, fallback to the company plan off\n"
    );
    let s = printed(
        wallet::settings_lines,
        r#"{"settings":{"serviceId":"svc"}}"#,
    );
    assert_eq!(s.exit, Exit::Error);
    assert!(s.stderr.ends_with("KeyError: 'enabled'\n"), "{}", s.stderr);
    let s = printed(wallet::people_lines, r#"{"items":[]}"#);
    assert_eq!(s.stdout, "No wallet items.\n");
    let s = printed(
        wallet::people_lines,
        r#"{"items":[{"id":"w1","username":"ann","provider":"openai","type":"key","label":"L","state":"ok","usedBy":["a","b"],"dormantSince":5},{"id":"w2","username":"bo","provider":"p","type":"t","label":"M","state":"s"},{"usedBy":[1]}]}"#,
    );
    assert_eq!(
        s.stdout,
        "w1  ann  openai key \"L\"  ok  used by a, b  dormant\nw2  bo  p t \"M\"  s  used by no agent\n"
    );
    assert!(s
        .stderr
        .ends_with("TypeError: sequence item 0: expected str instance, int found\n"));
    let s = printed(
        wallet::personal_agent_lines,
        r#"{"cleared":false,"workspace":"w"}"#,
    );
    assert_eq!(s.stdout, "w: was not a personal agent\n");
    let s = printed(
        wallet::personal_agent_lines,
        r#"{"workspace":"w","username":"ann","app":"acme"}"#,
    );
    assert_eq!(s.stdout, "w is now ann's own agent (through acme)\n");
    let s = printed(wallet::personal_agent_lines, "3");
    assert!(s
        .stderr
        .ends_with("TypeError: argument of type 'int' is not iterable\n"));
    let s = printed(
        wallet::personal_agents_lines,
        r#"{"agents":[{"handle":"","workspace":"/w","app":"a"},{"handle":"h","username":"u","app":"b"}]}"#,
    );
    assert_eq!(s.stdout, "/w  (gone)  through a\nh  u  through b\n");
    let s = printed(wallet::personal_agents_lines, "");
    assert!(s
        .stderr
        .ends_with("json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n"));
}

/// The printers' words are the shell's.
#[test]
fn wallet_text_is_the_shell_s() {
    let src = shell_source("app-wallet.sh");
    for needle in [
        r#"print("%s: wallet %s, fallback to the company plan %s" % (name, "on" if s["enabled"] else "off", "on" if s["fallback"] else "off"))"#,
        r#"print("No wallet items."); sys.exit(0)"#,
        r#"print("%s  %s  %s %s \"%s\"  %s  used by %s%s" % (i["id"], i["username"], i["provider"], i["type"], i["label"], i["state"], used, extra))"#,
        r#"echo "Removed $a1 (deleted, not Trash; the person signs in again to use it).""#,
        r#"print("No personal agents."); sys.exit(0)"#,
        r#"print("%s  %s  through %s" % (r.get("handle") or r["workspace"], r.get("username") or "(gone)", r["app"]))"#,
    ] {
        assert!(src.contains(needle), "app-wallet.sh no longer has {needle}");
    }
    // The usage lines, with the shell's own name for the command.
    for line in [
        wallet::usage::SETTINGS,
        wallet::usage::REMOVE,
        wallet::usage::CLEAR,
        wallet::usage::PERSONAL_AGENT,
        wallet::usage::VERBS,
    ] {
        let shell = line.replace(k2_client::brand::CLI_NAME, SHELL_NAME);
        assert!(src.contains(&shell), "app-wallet.sh no longer has {shell}");
    }
    assert!(crate::text::help(wallet::USAGE_PAGE).is_some());
}

#[test]
fn list_and_deployment_words() {
    use super::reads;
    let l = reads::parse_list(&["--workspace", "w", "--box", "--json"])
        .unwrap()
        .unwrap();
    assert_eq!((l.workspace.as_str(), l.boxed, l.json), ("w", true, true));
    assert_eq!(
        reads::parse_list(&["--workspace"]).unwrap().unwrap_err(),
        Stop::silent(Exit::Error)
    );
    assert_eq!(
        reads::parse_list(&["x"]).unwrap().unwrap_err(),
        Stop::usage("Unknown argument: x")
    );
    assert!(reads::parse_list(&["--help"]).is_none());
    let d = reads::parse_deployment(&["a", "b", "c", "--json"])
        .unwrap()
        .unwrap();
    assert_eq!((d.app.as_str(), d.id.as_str(), d.json), ("a", "c", true));
    assert_eq!(
        reads::parse_deployment(&["--json"]).unwrap().unwrap_err(),
        Stop::usage(reads::usage::DEPLOYMENT)
    );
    assert_eq!(
        reads::parse_deployment(&["a", "-x"]).unwrap().unwrap_err(),
        Stop::usage("Unknown flag: -x")
    );
}

#[test]
fn list_and_deployment_printers() {
    use super::reads;
    let s = printed(reads::list_lines, r#"{"apps":[]}"#);
    assert_eq!(s.stdout, format!("{}\n", reads::NO_APPS));
    let s = printed(
        reads::list_lines,
        r#"{"apps":[{"appId":"acme","state":"live","workspaceHandle":"ws","liveSeed":"0123456789abcdef","frontPort":4100,"label":"Acme","engine":{"label":"v2","rehostDue":true,"rehost":"odd"}}],"legacy":[{"name":"old","hint":"h","labelMovedTo":"acme"}],"notices":[{"hint":"n"}]}"#,
    );
    let want = format!(
        "{:<20} {:<12} {:<14} seed 0123456789ab  127.0.0.1:4100  engine v2 [re-host due]\n{:<20} Acme\n{:<20} {:<12} {:<14} {}  (its label now serves App acme)\n{:<20} how: h\nnote: n\n",
        "acme", "live", "ws", "", "old", "", "", "", ""
    );
    assert_eq!(s.stdout, want);
    let s = printed(
        reads::box_lines,
        r#"{"memory":{"usedBytes":2097152,"freeBytes":1048576,"pool":{"poolBytes":3145728}},"apps":[{"appId":"a","state":"live","maintainer":{"kind":"agent","agent":"x","address":"y"},"memory":{"limitBytes":null},"disk":{"budgetBytes":1048576},"ports":[{"port":1,"proto":"udp"}]}]}"#,
    );
    assert_eq!(
        s.stdout,
        concat!(
            "App memory on this box: 2 MiB used of 3 MiB (1 MiB free)\n",
            "a                    live         x (y)\n",
            "                     memory -  disk 1 MiB  ports 1/udp\n"
        )
    );
    let s = printed(
        reads::deployment_lines,
        r#"{"app":"acme","state":"live","who":"ann","deployment":{"id":"d1","kind":"deploy","error":"boom","fromSeed":"0123456789abcdef"}}"#,
    );
    assert_eq!(
        s.stdout,
        "acme d1 (deploy)\n  state   live  \u{2014} boom\n  who     ann\n  commit  -\n  seed    0123456789ab \u{2192} -\n"
    );
    let s = printed(
        reads::deployment_lines,
        r#"{"app":"a","deployment":{"id":1,"kind":2}}"#,
    );
    assert!(s.stderr.ends_with("KeyError: 'state'\n"), "{}", s.stderr);
}

#[test]
fn inspect_words_and_printer() {
    use super::reads;
    let i = reads::parse_inspect(&["a", "b", "--json"])
        .unwrap()
        .unwrap();
    assert_eq!((i.app.as_str(), i.json), ("b", true));
    assert!(reads::parse_inspect(&["a", "--resources"]).is_none());
    assert_eq!(
        reads::parse_inspect(&["--resources"]).unwrap().unwrap_err(),
        Stop::usage(reads::usage::INSPECT)
    );
    let s = printed(
        reads::inspect_lines,
        r#"{"app":{"appId":"acme","state":"live","label":"Acme","frontPort":4100,"memoryLimitBytes":1073741824},"runtime":{"live":{"deployment":"d1","port":5001}},"audit":{"verified":true,"rows":3}}"#,
    );
    assert_eq!(
        s.stdout,
        concat!(
            "acme  live  (Acme)\n",
            "  workspace None\n",
            "  url       local only: http://127.0.0.1:4100\n",
            "  front     127.0.0.1:4100 (stable across deploys; your own web server proxies here)\n",
            "  memory   1024 MiB\n",
            "  live      d1 on :5001 (pid None)\n",
            "  audit     chain verifies (3 rows)\n"
        )
    );
}

/// `app` is the catalog's `skin` tool, an `id` tool: a session needs its
/// passport.
#[test]
fn app_is_the_skin_id_tool() {
    assert_eq!(crate::tools::tool_id("app", "wallet"), TOOL);
    assert!(crate::tools::is_id(TOOL));
}

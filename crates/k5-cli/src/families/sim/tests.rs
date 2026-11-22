use super::*;

/// The shell's Python after `fn_name` in `cli/src/sim.sh` (a `python3 -c '`
/// block), numbered as `python3 -c` numbers it: line 1 is the empty one
/// after the quote.
fn python_of(fn_name: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/sim.sh");
    let text = std::fs::read_to_string(&path).expect("cli/src/sim.sh");
    let body = text
        .split_once(&format!("\n{fn_name}() {{"))
        .unwrap_or_else(|| panic!("no {fn_name} in sim.sh"))
        .1;
    let start = body.find("python3 -c '").expect("a python3 -c block") + "python3 -c '".len();
    let end = body[start..].find("\n'").expect("the end of the Python");
    body[start..start + end]
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// Line `n` of the reader holds `text`.
fn at(lines: &[String], n: usize, text: &str) {
    let l = &lines[n - 1];
    assert!(l.contains(text), "line {n} is {l:?}, not {text:?}");
}

#[test]
fn the_readers_lines_are_the_shell_s() {
    let st = python_of("_sim_status");
    for (n, t) in [
        (3, "v = json.load(sys.stdin)"),
        (4, r#"lanes = v.get("lanes") or []"#),
        (5, "for l in lanes:"),
        (6, r#"setup = l.get("setup") or {}"#),
        (7, r#"word = "ready" if l.get("ready")"#),
        (8, r#"line = "%-9s %s""#),
        (9, r#"if l.get("detail"):"#),
        (12, r#"needs = l.get("needs") or []"#),
        (13, "licences = [n for n in needs"),
        (14, "accepted = setup.get"),
        (16, r#"k = n.get("kind")"#),
        (18, "title, lid ="),
        (19, "lid in title"),
        (21, r#"if not setup.get("error"):"#),
        (22, "first use downloads"),
        (24, "owner step:"),
        (27, r#"if setup.get("running"):"#),
        (29, "all %d licences"),
        (31, r#"elif setup.get("error") and not licences:"#),
        (33, r#"s = v.get("sim")"#),
        (34, r#"if not s or s.get("state")"#),
        (35, "ready = any("),
        (37, r#"if s and s.get("state") == "failed":"#),
        (38, r#"s["reason"]"#),
        (41, r#"p = s.get("profile") or {}"#),
        (42, "size ="),
        (43, r#"s["appId"]"#),
        (44, "device: sim"),
        (45, r#"h = s.get("hold") or {}"#),
        (46, r#"if h.get("kind") == "person":"#),
        (47, "is using the device"),
        (50, "agents:"),
    ] {
        at(&st, n, t);
    }
    assert_eq!(st.len(), 50, "the status reader's length");
    let dv = python_of("_sim_list_devices");
    at(&dv, 3, "v = json.load(sys.stdin)");
    at(&dv, 4, r#"ds = v.get("devices") or []"#);
    at(&dv, 7, "for d in ds:");
    at(&dv, 8, r#"d.get("id", "?")"#);
    let bt = python_of("_sim_boot");
    at(&bt, 3, "s = json.load(sys.stdin) or {}");
    at(&bt, 4, r#"state = s.get("state", "?")"#);
    at(
        &bt,
        5,
        r#"name = (s.get("profile") or {}).get("name", "?")"#,
    );
    at(&bt, 7, "Running: sim");
    at(&bt, 9, "Booting: sim");
    let lg = python_of("_sim_logs");
    at(&lg, 3, "v = json.load(sys.stdin)");
    at(&lg, 4, r#"lines = v.get("lines") or []"#);
    at(&lg, 5, "for l in lines:");
    at(&lg, 9, r#"if v.get("truncated"):"#);
}

/// Each hint and line the binary prints is in `cli/src/sim.sh`, with
/// `k2` where the binary has the command's name.
#[test]
fn the_texts_are_the_shell_s() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/sim.sh"),
    )
    .unwrap();
    let n = cli_name!();
    for t in [
        "--workspace needs a path or name",
        "--platform needs android or ios",
        "--device needs a profile (k2 sim list-devices)",
        "unknown argument for k2 sim boot: ",
        "Stopped. Its data is kept; k2 sim boot starts it again.",
        "Erased. The next k2 sim boot starts a fresh device.",
        "k2 sim erase deletes the device's apps and data; pass --yes",
        "no such file: $p (build the app in your terminal first)",
        "usage: k2 sim tap e5 | k2 sim tap --at 120,300",
        "k2 sim tap takes an element OR --at x,y, not both",
        "--at is x,y in dp, like 120,300",
        "is not an element id (use one from k2 sim tree, like e5)",
        "is not an element id (use one from k2 sim tree, like e3)",
        "usage: k2 sim type [--into e3] \\\"text\\\" | --secret <vault-name>",
        "k2 sim type takes text OR --secret <vault-name>, not both",
        "k2 sim type takes one text (quote it)",
        "k2 sim swipe up|down|left|right [--from e7] (not: $1)",
        "usage: k2 sim swipe up|down|left|right [--from e7]",
        "open-url needs a URL with a scheme (https://…, myapp://…)",
        "location is lat,lon, like 37.33,-122.03",
        "--since is a duration like 30s, 2m or 1h",
        "--lines is a positive number",
        "--timeout is a duration like 10s or 2m",
        "--timeout is at most 2m (wait again if you must)",
        "(the daemon sent no tree)",
        "(the daemon sent no path)",
        "No device profiles on this server (k2 sim status says why).",
        "(cut at the line/byte cap; narrow with --since, --app or --grep)",
        "Run `k2 sim wait`, then `k2 sim tree` before acting again.",
        "no answer from the K2 daemon",
        "held: a person is using the device",
        "Erase this workspace'\"'\"'s device? Its apps and data are deleted. [y/N] ",
        "Error: unknown k2 sim command: $sub (status, list-devices, boot, stop, erase, install, launch, terminate, tree, tap, type, swipe, key, button, rotate, open-url, location, appearance, screenshot, logs, wait)",
        "  installing now; devices can boot when it finishes. k2 sim status again to follow it",
        "; the owner of this server runs it again (Retry setup on the Mobile tab), never you",
    ] {
        assert!(text.contains(t), "sim.sh has no {t:?}");
    }
    // The binary says its own name where the shell says k2.
    let d = parse("stop", &["x".into()], &|_| true, false);
    assert_eq!(
        d,
        Parsed::Die(usage(format!("{n} sim stop takes no arguments: x")))
    );
}

fn plan(sub: &str, rest: &[&str]) -> Parsed {
    let rest: Vec<String> = rest.iter().map(|w| w.to_string()).collect();
    parse(sub, &rest, &|p| p == "app.apk", false)
}

fn act_body(sub: &str, rest: &[&str], ws: &str) -> String {
    match plan(sub, rest) {
        Parsed::Plan(Plan::Post { fields, .. }) => body(ws, &fields),
        other => panic!("{sub} {rest:?}: {other:?}"),
    }
}

#[test]
fn the_bodies() {
    assert_eq!(
        act_body("tap", &["e5"], "/p"),
        r#"{"workspace": "/p", "action": {"verb": "tap", "element": "e5"}}"#
    );
    assert_eq!(
        act_body("tap", &["--at", "0120.50,007"], ""),
        r#"{"action": {"verb": "tap", "x": 120.5, "y": 7}}"#
    );
    assert_eq!(
        act_body("location", &["-0,-1.0"], ""),
        r#"{"action": {"verb": "location", "lat": 0, "lon": -1.0}}"#
    );
    assert_eq!(
        act_body("type", &["--into", "t2:e3", "--", "-x é\n"], ""),
        r#"{"action": {"verb": "type", "into": "t2:e3", "text": "-x \u00e9\n"}}"#
    );
    assert_eq!(
        act_body("type", &["--secret", "pw"], ""),
        r#"{"action": {"verb": "type", "secret": "pw"}}"#
    );
    assert_eq!(
        act_body("boot", &["--no-tab", "--device=pixel"], "w"),
        r#"{"workspace": "w", "device": "pixel", "noTab": true}"#
    );
    assert_eq!(
        act_body("wait", &["--timeout", " 1.5 m "], ""),
        r#"{"timeoutMs": 90000}"#
    );
    assert_eq!(
        act_body("rotate", &["left"], ""),
        r#"{"action": {"verb": "rotate", "turn": "left"}}"#
    );
    assert_eq!(act_body("kill", &["com.x"], ""), r#"{"app": "com.x"}"#);
    match plan("wait", &["--timeout", "2m"]) {
        Parsed::Plan(Plan::Post { max_time, .. }) => assert_eq!(max_time, Some(150)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_queries() {
    assert_eq!(query("", &[]), "");
    assert_eq!(
        query(
            "/a b",
            &[
                ("all", "1".into()),
                ("find", "x&y".into()),
                ("boxes", "".into())
            ]
        ),
        "workspace=%2Fa%20b&all=1&find=x%26y"
    );
    match plan("logs", &["--since", "1500ms", "--lines", "007"]) {
        Parsed::Plan(Plan::Get { pairs, .. }) => {
            assert_eq!(query("", &pairs), "sinceSecs=2&maxLines=007")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn durations() {
    assert_eq!(ms("10s"), Ms::Value(10_000));
    assert_eq!(ms("30"), Ms::Value(30_000));
    assert_eq!(ms("500ms"), Ms::Value(500));
    assert_eq!(ms("0.0015s"), Ms::Value(1));
    assert_eq!(ms("\t2 h\n"), Ms::Value(7_200_000));
    for bad in ["", "s", "5.", ".5", "5 . 5", "-1s", "5d", "5ss", "1e3"] {
        assert_eq!(ms(bad), Ms::Bad, "{bad:?}");
    }
    assert_eq!(ms("１s"), Ms::Shell);
    assert_eq!(ms(&"9".repeat(400)), Ms::Shell);
    assert_eq!(ms("99999999999999999h"), Ms::Shell);
}

#[test]
fn element_ids() {
    for good in ["e5", "t12:e0", "e007"] {
        assert!(is_id(good), "{good}");
    }
    for bad in [
        "", "e", "5", "t:e5", "t1e5", "x1:e5", "e5:", "t1:e5:e6", "E5", "e5 ",
    ] {
        assert!(!is_id(bad), "{bad}");
    }
}

#[test]
fn the_stops() {
    let die_of = |sub: &str, rest: &[&str]| match plan(sub, rest) {
        Parsed::Die(d) => d.hint,
        other => panic!("{sub} {rest:?}: {other:?}"),
    };
    assert_eq!(
        die_of("tap", &["e1", "e2"]),
        format!("{} sim tap takes one element", cli_name!())
    );
    assert_eq!(
        die_of("tap", &["-x"]),
        format!("unknown flag for {} sim tap: -x", cli_name!())
    );
    assert_eq!(
        die_of("tap", &["--at", "1,2", "e1"]),
        format!(
            "{} sim tap takes an element OR --at x,y, not both",
            cli_name!()
        )
    );
    assert_eq!(
        die_of("tap", &["--at", "1,-2"]),
        "--at is x,y in dp, like 120,300"
    );
    assert_eq!(die_of("key", &["f1"]), format!("{} sim key takes enter, backspace, delete, tab, escape, arrow-up, arrow-down, arrow-left, arrow-right (not f1)", cli_name!()));
    assert_eq!(
        die_of("button", &[]),
        format!(
            "usage: {} sim button home|back|lock|volume-up|volume-down|app-switch|dev-menu",
            cli_name!()
        )
    );
    assert_eq!(die_of("swipe", &["up", "down"]), "one direction");
    assert_eq!(
        die_of("swipe", &["up", "--from", "x"]),
        "\"x\" is not an element id"
    );
    assert_eq!(
        die_of("logs", &["--lines", "0"]),
        "--lines is a positive number"
    );
    assert_eq!(
        die_of("logs", &["--since"]),
        "--since needs a duration (2m)"
    );
    assert_eq!(
        die_of("wait", &["--timeout", "121s"]),
        "--timeout is at most 2m (wait again if you must)"
    );
    assert_eq!(
        die_of("install", &["nope.apk"]),
        "no such file: nope.apk (build the app in your terminal first)"
    );
    assert_eq!(
        die_of("open-url", &["x"]),
        "open-url needs a URL with a scheme (https://…, myapp://…)"
    );
    assert_eq!(
        plan("logs", &["--lines", "99999999999999999999"]),
        Parsed::Shell
    );
    let c = common(&["a", "--json", "-w"]).unwrap_err();
    assert!(c.json && c.hint == "--workspace needs a path or name");
}

#[test]
fn the_dies() {
    let d = usage("x \"é\"");
    assert_eq!(d.exit, Exit::Usage);
    let c = common(&["--workspace=w", "e1", "--json"]).unwrap();
    assert_eq!(
        (c.json, c.workspace.as_str(), c.rest),
        (true, "w", vec!["e1".to_string()])
    );
}

#[test]
fn the_refusals() {
    let r = refusal(
        "409",
        br#"{"error":"held","code":"sim_held","message":"alice is using it"}"#,
        false,
    );
    assert_eq!(r.exit, Exit::Usage);
    assert_eq!(
        r.stderr,
        format!("Error: alice is using it\nRun `{n} sim wait`, then `{n} sim tree` before acting again.\n{{\"error\": {{\"code\": \"sim_held\", \"hint\": \"alice is using it\", \"status\": 409}}}}\n", n = cli_name!())
    );
    let r = refusal(
        "403",
        br#"{"error":{"code":"sim_switch_off","hint":"off"}}"#,
        true,
    );
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            "{\"error\": {\"code\": \"sim_switch_off\", \"hint\": \"off\", \"status\": 403}}\n",
            Exit::Refused
        )
    );
    let r = refusal("000", b"", false);
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stderr.starts_with("Error: no answer from the "));
    assert!(r.stderr.ends_with(concat!(
        "\"code\": \"daemon_unreachable\", \"hint\": \"no answer from the ",
        k2_client::product_name!(),
        " daemon\", \"status\": 0}}\n"
    )));
    let r = refusal("502", b"  <html>bad</html>\n", false);
    assert_eq!(r.stderr.lines().next(), Some("Error: <html>bad</html>"));
    assert_eq!(r.exit, Exit::Error);
    let r = refusal("400", br#"{"error":"bad thing"}"#, false);
    assert!(r
        .stderr
        .contains("\"code\": \"bad_request\", \"hint\": \"bad thing\""));
    assert_eq!(r.exit, Exit::Usage);
    let r = refusal("418", br#"{"error":null}"#, false);
    assert!(r
        .stderr
        .starts_with("Error: the daemon refused (HTTP 418)\n"));
    let r = refusal("404", br#"{"error":"nope_x","code":""}"#, false);
    assert!(r.stderr.contains("\"code\": \"nope_x\""));
}

#[test]
fn the_readers() {
    assert_eq!(
        print_field(br#"{"line":"ok tap"}"#, "line", "ok").stdout,
        "ok tap\n"
    );
    assert_eq!(print_field(br#"{"text":"a\n"}"#, "text", "x").stdout, "a\n");
    assert_eq!(print_field(b"nope", "line", "ok").stdout, "ok\n");
    assert_eq!(print_field(br#"{"line":5}"#, "line", "ok").stdout, "ok\n");
    let r = render_status(br#"{"lanes":[{"lane":"android","ready":true,"needs":[{"kind":"licence","title":"SDK","id":"sdk"},{"kind":"licence","id":"x"}],"setup":{"running":true,"licencesAccepted":1}}],"sim":{"sim":"s1","lane":"android","state":"running","profile":{"name":"P","widthPt":4,"heightPt":9},"appId":"a","hold":{"kind":"takeover"}},"agentsAllowed":true}"#);
    let n = cli_name!();
    assert_eq!(
        r.stdout,
        format!(
            "lane android   ready\n  the owner must accept the SDK [sdk] (a card; never you)\n  the owner must accept the x (a card; never you)\n  one card covers all 2 licences; the owner accepts them together\n  the owner accepted the licence; nothing left to accept\n  installing now; devices can boot when it finishes. {n} sim status again to follow it\ndevice: sim s1 · android · P · running · 4x9 pt · a\nheld: a person has taken over (until Hand back)\nagents: allowed (Allow mobile devices is on)\n"
        )
    );
    let r = render_status(br#"{"sim":{"state":"failed","reason":5}}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: can only concatenate str (not \"int\") to str\n"));
    assert!(r.stderr.contains("line 38"));
    let r = render_status(b"");
    assert!(r
        .stderr
        .ends_with("json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n"));
    let r = render_status(br#"{"lanes":[{"needs":[{"kind":"licence"}]}]}"#);
    assert!(
        r.stderr
            .ends_with("TypeError: argument of type 'NoneType' is not iterable\n"),
        "{}",
        r.stderr
    );
    let r = render_devices(br#"{"devices":[{"id":"pixel_9","name":"Pixel 9","widthPt":412,"heightPt":915,"os":"android","default":true}]}"#);
    assert_eq!(
        r.stdout,
        format!(
            "{:<28} {:<18} 412x915 pt  android  (default)\n",
            "pixel_9", "Pixel 9"
        )
    );
    let r = render_boot(br#"{"state":"booting","sim":"s1","lane":"ios"}"#);
    assert_eq!(
        r.stdout,
        format!("Booting: sim s1 · ios · ? (booting). Next: {n} sim wait, then {n} sim tree\n")
    );
    let r = render_logs(br#"{"lines":["a",1],"truncated":true}"#);
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str()),
        (
            "a\n1\n",
            "(cut at the line/byte cap; narrow with --since, --app or --grep)\n"
        )
    );
    let r = render_logs(br#"[]"#);
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
}

#[test]
fn resolve_answers() {
    assert_eq!(resolved(br#"{"path":"/w/a\n"}"#), "/w/a");
    assert_eq!(resolved(br#"{"path":"","projectPath":"/p"}"#), "/p");
    assert_eq!(resolved(br#"{"path":3}"#), "3");
    assert_eq!(resolved(b"[1]"), "");
    assert_eq!(resolved(b""), "");
    assert_eq!(resolved(b"nope"), "");
}

#[test]
fn paths() {
    assert_eq!(
        (dirname("a/b/"), basename("a/b/")),
        ("a".into(), "b".into())
    );
    assert_eq!(
        (dirname("app.apk"), basename("app.apk")),
        (".".into(), "app.apk".into())
    );
    assert_eq!((dirname("/"), basename("/")), ("/".into(), "/".into()));
    assert_eq!((dirname("/x"), basename("//x//")), ("/".into(), "x".into()));
    let tmp = std::env::temp_dir().canonicalize().unwrap();
    let t = tmp.to_string_lossy();
    assert_eq!(cd_pwd(&tmp, ".").as_deref(), Some(&*t));
    assert_eq!(
        cd_pwd(Path::new("/"), &format!("{t}//./")).as_deref(),
        Some(&*t)
    );
    assert_eq!(cd_pwd(Path::new("/"), "/").as_deref(), Some("/"));
}

#[test]
fn help_words() {
    for w in [&[][..], &[""], &["help"], &["tap", "-h"], &["x", "--help"]] {
        assert!(wants_help(w), "{w:?}");
    }
    assert!(!wants_help(&["tap", "help"]));
    // After a bare `--` the words are the text (`_wants_help`).
    assert!(!wants_help(&["type", "--", "--help"]));
    assert!(!wants_help(&["type", "--into", "e3", "--", "-h"]));
    assert!(wants_help(&["type", "-h", "--", "x"]));
}

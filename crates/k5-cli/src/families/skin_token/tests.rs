use super::*;

pub fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `n`th (from 0) single-quoted `python3 -c '…'` after `after` in
/// `file`, its lines as `python3 -c` numbers them (line 1 is the empty
/// one after the quote).
pub fn quoted_python(file: &str, after: &str, n: usize) -> Vec<String> {
    let text = source(file);
    let mut body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"))
        .1;
    let open = "python3 -c '";
    for _ in 0..n {
        body = &body[body.find(open).expect("a python3 -c") + open.len()..];
    }
    let py = &body[body.find(open).expect("a python3 -c") + open.len()..];
    let end = py.find('\'').expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

/// A line of ours as the shell's default brand spells it.
pub fn k2(text: &str) -> String {
    text.replace(k2_client::brand::CLI_NAME, "k2")
}

fn at_line(lines: &[String], n: u32) -> String {
    lines[n as usize - 1].trim().to_string()
}

#[test]
fn the_shell_s_lines() {
    let text = source("app-wallet.sh");
    for shell in [
        r#"                    --name) name="${2:-}"; shift 2 ;;"#,
        r#"                    --caps|--cap) caps="${2:-}"; shift 2 ;;"#,
        r#"                    --agent|--agents) agents+=("${2:-}"); shift 2 ;;"#,
        r#"            raw=$(cli_post_json "/cli/skin-tokens" "$body")"#,
        r#"            raw=$(cli_request "/cli/skin-tokens")"#,
        r#"            raw=$(cli_post_json "/cli/skin-tokens/rooms" "$body")"#,
        r#"            raw=$(cli_post_json "/cli/skin-tokens/revoke" "$body")"#,
        r#"            printf '%s' "$raw" | _skin_die_if_err"#,
        r#"                    *) [[ -z "$id" ]] && id="$1"; shift ;;"#,
        r#"            [[ -z "$id" ]] && { echo "Usage: k2 app token revoke <id>" >&2; exit 1; }"#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "cmd_skin_token no longer has {shell:?}"
        );
    }
    for line in USAGE {
        assert!(
            text.contains(&format!("            echo \"{}\" >&2\n", k2(line))),
            "{line:?}"
        );
    }
    for line in [CREATE_USAGE, ROOMS_USAGE, NOT_USER, USE_NAME, NO_AGENT] {
        assert!(text.contains(&k2(line)), "{line:?}");
    }
    assert!(text.contains(STORE_NOW));
    let tw = source("talk-and-workspace.sh");
    assert!(tw.contains(&format!("    echo \"{}\" >&2\n", k2(TEACH))));
    let router = source("router.sh");
    assert!(router.contains(
        "    skin-token)\n        _teach_skin_token_now_app_token\n        if _wants_help \"${@:2}\"; then\n            cmd_skin_token --help\n            exit 0\n        fi\n        shift; cmd_skin_token \"$@\"\n"
    ));
    let policy = source("cli-tools-policy.sh");
    assert!(policy.contains(r#"        app|skin|skin-token|capabilities) echo "skin" ;;"#));
}

#[test]
fn the_python() {
    let die = quoted_python("talk-and-workspace.sh", "_skin_die_if_err() {", 0);
    for (n, text) in [
        (3, "raw = sys.stdin.read().strip()"),
        (line::die::ERR, r#"err = d.get("error")"#),
        (line::die::OK, r#"if err is None and d.get("ok") is False:"#),
        (
            line::die::IN_HINT,
            r#"if code in ("owner_only", "forbidden", "gated") and "ask your human" not in hint:"#,
        ),
        (
            line::die::PLUS,
            "hint = (hint + \" \u{2014} \" if hint else \"\") + \"requires owner/admin \u{2014} ask your human\"",
        ),
        (line::die::PRINT, r#"print("Error: " + hint, file=sys.stderr)"#),
    ] {
        assert_eq!(at_line(&die, n), text, "_skin_die_if_err line {n}");
    }
    let after = "cmd_skin_token() {";
    let create = quoted_python("app-wallet.sh", after, 1);
    for (n, text) in [
        (line::LOAD, "d=json.load(sys.stdin)"),
        (
            line::create::ID,
            r#"print("Skin token created (id: %s) name %s." % (d.get("id",""), d.get("name") or d.get("username","")))"#,
        ),
        (
            line::create::CAPS,
            r#"print("Capabilities: %s" % (", ".join(caps) if caps else "(none)"))"#,
        ),
        (
            line::create::ROOMS,
            r#"print("Rooms: %s" % (", ".join(handles) if handles else "(none)"))"#,
        ),
        (line::create::TOKEN, r#"print("  %s" % d.get("token",""))"#),
    ] {
        assert_eq!(at_line(&create, n), text, "create line {n}");
    }
    let list = quoted_python("app-wallet.sh", after, 2);
    for (n, text) in [
        (line::LOAD, "d=json.load(sys.stdin)"),
        (line::list::TOKENS, r#"tokens=d.get("tokens",[])"#),
        (line::list::FOR, "for t in tokens:"),
        (
            line::list::STATE,
            r#"state="revoked" if t.get("revokedAt") else "active""#,
        ),
        (
            line::list::CAPS,
            r#"caps=",".join(t.get("caps") or []) or "none""#,
        ),
        (
            line::list::ROOMS,
            r#"rooms=",".join(handles) if handles else "(none)""#,
        ),
    ] {
        assert_eq!(at_line(&list, n), text, "list line {n}");
    }
    let rooms = quoted_python("app-wallet.sh", after, 4);
    assert_eq!(
        at_line(&rooms, line::rooms::HANDLES),
        r#"handles=d.get("roomHandles") or d.get("room_handles") or []"#
    );
    assert!(at_line(&rooms, line::rooms::PRINT).starts_with(r#"print("Skin token %s rooms: %s""#));
    let revoke = quoted_python("app-wallet.sh", after, 6);
    assert!(at_line(&revoke, line::revoke::PRINT)
        .starts_with(r#"print("Skin token revoked." if d.get("success")"#));
}

fn words(list: &[&str]) -> Result<Ask, Stop> {
    parse(list)
}

#[test]
fn the_words() {
    assert_eq!(words(&["list", "x"]), Ok(Ask::List));
    assert_eq!(
        words(&["create", "--name", "n", "--agent", "a", "--cap", "c", "--agents", "b"]),
        Ok(Ask::Create {
            name: "n".into(),
            caps: "c".into(),
            agents: vec!["a".into(), "b".into()]
        })
    );
    assert_eq!(words(&["create", "--name"]).unwrap_err(), Stop::shift("--name"));
    assert_eq!(
        words(&["create", "--agent", "a"]).unwrap_err().lines,
        vec![USE_NAME, CREATE_USAGE]
    );
    assert_eq!(
        words(&["create", "--name", "n"]).unwrap_err().lines,
        vec![NO_AGENT, CREATE_USAGE]
    );
    assert_eq!(
        words(&["create", "--x"]).unwrap_err(),
        Stop {
            lines: vec!["Unknown flag: --x".into()],
            exit: Exit::Error
        }
    );
    assert_eq!(
        words(&["rooms", "", "i1", "i2", "--agent", "a", "--clear"]),
        Ok(Ask::Rooms {
            id: "i1".into(),
            agents: vec![]
        })
    );
    assert_eq!(words(&["rooms", "--agent"]).unwrap_err(), Stop::shift("--agent"));
    assert_eq!(
        words(&["revoke", ""]).unwrap_err().lines,
        vec![REVOKE_USAGE]
    );
    assert_eq!(words(&[]).unwrap_err().exit, Exit::Error);
    assert_eq!(words(&["nope"]).unwrap_err().lines, vec![USAGE[0]]);
}

#[test]
fn the_bodies() {
    assert_eq!(
        create_body("n\u{e9}", " a, b ,, ", &["x\ry".into(), " ".into()]),
        "{\"name\": \"n\\u00e9\", \"caps\": [\"a\", \"b\"], \"rooms\": [\"x\", \"y\"]}"
    );
    assert_eq!(create_body("n", "  ", &[]), r#"{"name": "n", "rooms": []}"#);
    assert_eq!(rooms_body("i", &[]), r#"{"id": "i", "rooms": []}"#);
    assert_eq!(revoke_body("i\""), r#"{"id": "i\""}"#);
}

#[test]
fn die_if_err_as_the_shell() {
    let die = |raw: &str| die_if_err(raw.as_bytes());
    assert_eq!(die("  {\"a\":1} "), Shown::err("", Exit::Ok));
    assert_eq!(die("[1]"), Shown::err("", Exit::Ok));
    assert_eq!(
        die(" \n"),
        Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error
        )
    );
    assert_eq!(
        die(&format!("x{}", "y".repeat(300))),
        Shown::err(format!("Error: x{}\n", "y".repeat(199)), Exit::Error)
    );
    assert_eq!(
        die(r#"{"error":{"code":"gated"}}"#),
        Shown::err(
            "Error: gated \u{2014} requires owner/admin \u{2014} ask your human\n",
            Exit::Refused
        )
    );
    assert_eq!(
        die(r#"{"error":{"code":"usage","message":"m"}}"#),
        Shown::err("Error: m\n", Exit::Usage)
    );
    assert_eq!(
        die(r#"{"ok":false,"hint":"h"}"#),
        Shown::err("Error: h\n", Exit::Error)
    );
    assert_eq!(
        die(r#"{"error":"Bad Token"}"#),
        Shown::err("Error: Bad Token\n", Exit::Refused)
    );
    assert_eq!(die(r#"{"error":0}"#), Shown::err("Error: 0\n", Exit::Error));
    let r = die(r#"{"error":{"code":"x","hint":5}}"#);
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stderr.ends_with(
        "line 24, in <module>\nTypeError: can only concatenate str (not \"int\") to str\n"
    ));
}

#[test]
fn the_printers() {
    assert_eq!(
        create_reader(r#"{"id":"t1","username":"u","caps":["a","b"],"room_handles":["r"],"token":"k2skn_x"}"#).stdout,
        format!("Skin token created (id: t1) name u.\nCapabilities: a, b\nRooms: r\n\n  k2skn_x\n\n{STORE_NOW}\n")
    );
    assert_eq!(
        list_reader(r#"{"tokens":[{"id":"t","revokedAt":1,"name":"n","prefix":"k2skn_ab","caps":[]},{"id":"u"}]}"#).stdout,
        "t  [revoked]  n  k2skn_ab  caps=none  rooms=(none)\nu  [active]      caps=none  rooms=(none)\n"
    );
    assert_eq!(list_reader(r#"{"tokens":null}"#).stdout, "No App tokens.\n");
    let r = list_reader(r#"{"tokens":"ab"}"#);
    assert!(r
        .stderr
        .ends_with("line 8, in <module>\nAttributeError: 'str' object has no attribute 'get'\n"));
    assert_eq!(
        rooms_reader(r#"{"id":"t","roomHandles":["a","b"]}"#).stdout,
        "Skin token t rooms: a, b\n"
    );
    assert_eq!(
        revoke_reader(r#"{"success":true}"#).stdout,
        "Skin token revoked.\n"
    );
    let r = revoke_reader("[]");
    assert_eq!((r.stdout.as_str(), r.exit), ("", Exit::Error));
}

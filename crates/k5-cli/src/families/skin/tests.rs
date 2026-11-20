//! The words, the bodies and the printers' Python corners. The answers
//! against the shell itself are `tests/skin_parity.rs`.

use super::print::{self, py_eq};
use super::*;
use crate::families::Shown;
use crate::out::json::parse as j;

fn ask(words: &[&str]) -> Result<Ask, Stop> {
    parse(words)
}

fn body(words: &[&str]) -> (String, String) {
    match ask(words) {
        Ok(Ask::Plain(p)) | Ok(Ask::OwnerOnly(p)) => match p.req {
            Req::Post(path, body) => (path.to_string(), body),
            Req::Get(path, pairs) => (path.to_string(), pairs.join("&")),
            Req::PostPassword(path, body, _) => (path.to_string(), body),
        },
        other => panic!("{words:?}: {other:?}"),
    }
}

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/app-wallet.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_shell_hands_these_on() {
    for w in [
        &[] as &[&str],
        &["nope"],
        &["user", "chat", "bob"],
        &["user", "add", "bob", "--test"],
        &["role", "suggest"],
        &["grant", "hooks"],
        &["grant", "copy-host", "acme"],
        &["front-door", "direct", "--ui-port", "\u{663}"],
    ] {
        assert_eq!(ask(w), Ok(Ask::Shell), "{w:?}");
    }
}

#[test]
fn usage_stops() {
    let stop = |w: &[&str]| ask(w).expect_err("a stop");
    assert_eq!(stop(&["user", "x", "--role", "r"]).exit, Exit::Usage);
    assert_eq!(stop(&["user", "show", "bob"]).exit, Exit::Error);
    // `--agent` as the last word: the failed `shift 2`, nothing said.
    let s = stop(&["user", "rooms", "bob", "--agent"]);
    assert_eq!((s.exit, s.stderr.len()), (Exit::Error, 0));
    assert_eq!(
        stop(&["user", "rooms", "bob", "--apply-tokens"]).exit,
        Exit::Usage
    );
    assert_eq!(stop(&["role", "x", "--user"]).exit, Exit::Usage);
    assert_eq!(stop(&["role", "paths", "r", "h"]).exit, Exit::Usage);
    assert_eq!(
        stop(&["role", "paths", "r", "h", "x"]).stderr,
        ["Unexpected argument: x"]
    );
    assert_eq!(stop(&["role", "add", "--nope"]).exit, Exit::Usage);
    assert_eq!(stop(&["grant", "set", "x"]).stderr, ["Unknown argument: x"]);
    assert_eq!(stop(&["hydra", "on", "off"]).exit, Exit::Usage);
    assert_eq!(stop(&["front-door", "x"]).stderr, ["Unknown flag: x"]);
}

#[test]
fn the_bodies_are_what_python_prints() {
    assert_eq!(
        body(&[
            "user",
            "add",
            "bob",
            "--temp-password",
            "--app",
            " acme ",
            "--role=r"
        ]),
        (
            "/cli/skin/users".into(),
            r#"{"username": "bob", "generate": true, "app": "acme", "role": "r"}"#.into()
        )
    );
    assert_eq!(
        body(&["user", "reset", "bob", "--revoke", "--to", "a@b"]).1,
        r#"{"username": "bob", "to": "a@b", "code": false, "revoke": true}"#
    );
    assert_eq!(
        body(&["user", "sessions", "bob", "--app", "a b", "--revoke-all"]).1,
        r#"{"user": "bob", "app": "a b", "all": true}"#
    );
    assert_eq!(
        body(&["user", "show", "b&o", "--app", "a/b"]).1,
        "app=a%2Fb&user=b%26o"
    );
    assert_eq!(
        body(&[
            "role",
            "paths",
            "r",
            "h",
            "--hide",
            "a",
            "--hide",
            "b",
            "--no-download"
        ])
        .1,
        r#"{"name": "r", "handle": "h", "hideAdd": ["a", "b"], "download": []}"#
    );
    assert_eq!(
        body(&["role", "room", "r", "h", "--caps", " a, ,b "]).1,
        r#"{"name": "r", "handle": "h", "caps": ["a", "b"]}"#
    );
    assert_eq!(
        body(&[
            "grant",
            "add",
            "--kind",
            "CHAT",
            "--subject-kind",
            "p",
            "--subject",
            "s"
        ])
        .1,
        r#"{"subjectKind": "p", "subjectId": "s", "kind": "CHAT", "targetId": "sew"}"#
    );
    assert_eq!(body(&["grant", "rm", "--id", " g1 "]).1, r#"{"id": "g1"}"#);
    match ask(&["front-door", "direct", "--ui-port", "+0_7"]) {
        Ok(Ask::FrontDoorSet { body, .. }) => {
            assert_eq!(body, r#"{"mode": "direct", "apply": true, "uiPort": 7}"#)
        }
        other => panic!("{other:?}"),
    }
    match ask(&["front-door", "direct", "--ui-port", "x1"]) {
        Ok(Ask::FrontDoorSet { body, .. }) => {
            assert_eq!(body, r#"{"mode": "direct", "apply": true, "uiPort": "x1"}"#)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_routes_are_the_shells() {
    let src = source();
    for w in [
        &["user", "add", "b"][..],
        &["user", "image", "b", "--clear"],
        &["user", "invite", "b", "--app", "a"],
        &["user", "reset", "b"],
        &["user", "unlock", "b"],
        &["user", "email", "b"],
        &["user", "full-name", "b"],
        &["user", "agent", "b", "w"],
        &["user", "remove", "b"],
        &["user", "rooms", "b"],
        &["user", "role", "b", "r"],
        &["user", "unassign", "b"],
        &["role", "create", "r"],
        &["role", "room", "r", "h"],
        &["role", "paths", "r", "h", "--clear"],
        &["role", "update", "r"],
        &["role", "remove", "r"],
        &["role", "add", "--from", "t"],
        &[
            "grant",
            "add",
            "--kind",
            "chat",
            "--subject-kind",
            "p",
            "--subject",
            "s",
        ],
        &["grant", "rm", "--id", "g"],
        &["grant", "set", "--id", "g", "--scope", "s"],
        &["grant", "role", "g", "--role", "r"],
    ] {
        let path = body(w).0;
        assert!(src.contains(&format!("\"{path}\"")), "{w:?}: {path}");
    }
}

#[test]
fn python_equality() {
    assert!(py_eq(&j("1").unwrap(), &j("true").unwrap()));
    assert!(py_eq(&j("2").unwrap(), &j("2.0").unwrap()));
    assert!(!py_eq(&j("\"1\"").unwrap(), &j("1").unwrap()));
    assert!(py_eq(
        &j(r#"{"a":1,"b":2}"#).unwrap(),
        &j(r#"{"b":2,"a":1.0}"#).unwrap()
    ));
}

fn shown(s: Shown) -> (String, String, Exit) {
    (s.stdout, s.stderr, s.exit)
}

#[test]
fn invite_hours_floor_as_python() {
    let one = |ttl: &str| {
        let raw = format!(r#"{{"delivered":"sent","sentTo":"a","from":"b","ttlSecs":{ttl}}}"#);
        shown(print::read(&Reader::UserInvite, &raw)).0
    };
    let tail = |h: &str| {
        format!("Invite sent to a from b (the link works once; it expires in {h} hours)\n")
    };
    assert_eq!(one("7200"), tail("2"));
    assert_eq!(one("3599"), tail("<1"));
    assert_eq!(one("-1"), tail("-1"));
    assert_eq!(one("7200.5"), tail("2.0"));
    assert_eq!(one("true"), tail("<1"));
    assert_eq!(
        one("123456789012345678901234567890"),
        tail("34293552503429355250342935")
    );
    let (_, err, exit) = shown(print::read(
        &Reader::UserInvite,
        r#"{"delivered":"sent","ttlSecs":null}"#,
    ));
    assert_eq!(exit, Exit::Error);
    assert!(err.ends_with(
        "line 6, in <module>\nTypeError: unsupported operand type(s) for //: 'NoneType' and 'int'\n"
    ), "{err}");
}

#[test]
fn slices_and_held() {
    let (out, _, _) = shown(print::read(
        &Reader::UserShow,
        r#"{"username":"b","app":"a","imageHash":"0123456789abcdef"}"#,
    ));
    assert!(
        out.contains("  picture       yes (0123456789ab\u{2026})\n"),
        "{out}"
    );
    let (_, err, exit) = shown(print::read(
        &Reader::UserShow,
        r#"{"username":"b","app":"a","imageHash":{"x":1}}"#,
    ));
    assert_eq!(exit, Exit::Error);
    assert!(err.ends_with("KeyError: slice(None, 12, None)\n"), "{err}");
    assert_eq!(
        shown(print::held(
            r#"{"status":"held","request":{"id":"r1"},"hint":"h"}"#
        )),
        (
            "Held for the owner (request r1): \n  h\n".to_string(),
            String::new(),
            Exit::Usage
        )
    );
    assert_eq!(shown(print::held("not json")).2, Exit::Ok);
}

#[test]
fn user_list_reads_one_line_of_each() {
    let stream = concat!(
        r#"{"users":[{"username":"b","roleName":"r"},{"username":"c","appGrants":[{"role":"m","app":"x","rooms":["s","s"]}]}]}"#,
        "\n",
        r#"{"roles":[{"name":"r","roomHandles":["h1","h2"]}]}"#,
        "\n"
    );
    assert_eq!(
        shown(print::user_list(stream)).0,
        "b  \u{2014}  r  h1, h2  \u{2014}\nc  \u{2014}  m (App x)  s  \u{2014}\n"
    );
    // An answer over two lines: the first line alone is not JSON.
    let (_, err, exit) = shown(print::user_list("{\n\"users\":[]}\n{}\n"));
    assert_eq!(exit, Exit::Error);
    assert!(err.contains("line 3, in <module>"), "{err}");
}

/// The lines name the command by the brand's name (a build with
/// `K2_BRAND_DIR=brand/xy` says `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let mut lines: Vec<String> = Vec::new();
    for w in [
        &["user", "show", "bob"][..],
        &["user", "x", "--role", "r"],
        &["user", "nope"],
        &["user", "agent", "bob"],
        &["role", "x", "--user"],
        &["role", "create", "r", "--caps", "c"],
        &["role", "add", "x"],
        &["grant", "nope"],
        &["grant", "rm"],
        &["front-door", "connect", "direct"],
        &["hydra", "x"],
    ] {
        let stop = ask(w).expect_err("a stop");
        assert!(!stop.stderr.is_empty(), "{w:?}");
        lines.extend(stop.stderr);
    }
    lines.push(super::TEACH.to_string());
    let (out, _, _) = shown(print::read(
        &Reader::GrantAdd,
        r#"{"id":"g","kind":"chat"}"#,
    ));
    lines.push(out);
    let (out, _, _) = shown(print::read(&Reader::RoleAdd, r#"{"role":{"name":"r"}}"#));
    lines.push(out);
    for l in &lines {
        assert!(l.contains(&format!("{cli} ")), "{l}");
        if cli != "k2" {
            assert!(!l.contains("k2 ") && !l.contains("k5 "), "{l}");
        }
    }
}

#[test]
fn a_password_on_stdin_joins_the_body_when_sent() {
    let Ok(Ask::Plain(p)) = ask(&["user", "password", "bob", "--password-stdin"]) else {
        panic!("user password --password-stdin is native")
    };
    assert_eq!(
        p.req,
        Req::PostPassword(
            "/cli/skin/users/password",
            r#"{"username": "bob"}"#.into(),
            1
        )
    );
    assert!(p.held && p.reader == Reader::UserPassword);
    let Ok(Ask::Plain(p)) = ask(&["user", "add", "bob", "--password-stdin", "--app", " a "]) else {
        panic!("user add --password-stdin is native")
    };
    assert_eq!(
        p.req,
        Req::PostPassword(
            "/cli/skin/users",
            r#"{"username": "bob", "app": "a"}"#.into(),
            1
        )
    );
    let pw = Secret::new(b"p\xffw\"d\r".to_vec());
    assert_eq!(
        with_password(r#"{"username": "bob", "app": "a"}"#, 1, &pw).expose(),
        br#"{"username": "bob", "password": "p\udcffw\"d\r", "app": "a"}"#
    );
    assert_eq!(
        with_password(r#"{"username": "b"}"#, 1, &Secret::new(Vec::new())).expose(),
        br#"{"username": "b", "password": ""}"#
    );
}

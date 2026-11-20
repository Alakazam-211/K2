use super::phones::{self, safe};
use super::read::{self, add_body, kept, presence_secret};
use super::*;
use crate::families::tunnel::Out;
use crate::out::json;

fn shell(file: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Each line this prints is the shell's (in the default brand, `k2`).
#[test]
fn the_lines_are_the_shell_s() {
    let clone = shell("clone.sh");
    let phones = shell("users-phones.sh");
    for l in [
        ADD_USAGE,
        LIST_USAGE,
        SET_ROLE_USAGE,
        SHARE_USAGE,
        SET_PASSWORD_USAGE,
        AUDIT_USAGE,
        CAN_CLONE_USAGE,
        UNLOCK_USAGE,
        NO_PASSWORD_ON_ARGV[0],
        NO_PASSWORD_ON_ARGV[1],
        VIEWER_REMOVED,
        OWNER_CHECK,
    ] {
        assert!(clone.contains(l), "{l:?} not in clone.sh");
    }
    for l in [phones::PROMPT, phones::NEED_PASSWORD, phones::EMPTY] {
        assert!(phones.contains(l), "{l:?} not in users-phones.sh");
    }
    assert!(phones.contains(std::str::from_utf8(phones::AGAIN).unwrap()));
}

fn stop(f: Form) -> (Vec<String>, bool, Exit) {
    match f {
        Form::Stop(s) => (s.lines, s.page, s.exit),
        other => panic!("not a stop: {other:?}"),
    }
}

#[test]
fn the_words_as_the_shell_reads_them() {
    assert_eq!(parse(&[]), Form::List { json: false });
    assert_eq!(parse(&[""]), Form::List { json: false });
    assert_eq!(parse(&["list", "--json"]), Form::List { json: true });
    assert_eq!(stop(parse(&["list", "x"])).0, vec![LIST_USAGE]);
    assert_eq!(stop(parse(&["help"])), (vec![], true, Exit::Ok));
    let (l, page, exit) = stop(parse(&["nope"]));
    assert_eq!(l, vec!["Unknown 'k2 users' subcommand: nope"]);
    assert!(page);
    assert_eq!(exit, Exit::Usage);

    let Form::Add(a) = parse(&[
        "add",
        "ann",
        "--role=voice-chat",
        "--share-agent",
        "a1",
        "--share-garden=g",
        "--share-source",
        "s",
        "--no-fixed-password",
        "--force",
        "--keep-password",
        "--password-stdin",
    ]) else {
        panic!()
    };
    assert_eq!(a.role, "voice_chat");
    assert_eq!((a.agents.as_str(), a.gardens.as_str()), ("a1\n", "g\n"));
    assert_eq!((a.fixed, a.force, a.keep, a.stdin), ("0", true, true, true));
    assert_eq!(stop(parse(&["add"])).0, vec![ADD_USAGE]);
    assert_eq!(
        stop(parse(&["add", "ann", "--role"])).0,
        vec!["Error: --role needs a value"]
    );
    assert_eq!(stop(parse(&["add", "ann", "--role", "viewer"])).0, vec![VIEWER_REMOVED]);
    assert_eq!(
        stop(parse(&["add", "ann", "--share-agent", "x"])).0,
        vec!["Error: --share-agent, --share-garden and --share-source are only for --role voice_chat"]
    );
    let (l, page, _) = stop(parse(&["add", "ann", "--zap"]));
    assert_eq!((l, page), (vec!["Error: unknown flag '--zap'".to_string()], true));
    assert_eq!(stop(parse(&["add", "--password", "x"])).0.len(), 2);

    assert_eq!(
        parse(&["set-role", "ann", "voice-chat", "--fixed-password"]),
        Form::SetRole {
            user: "ann".into(),
            role: "voice_chat".into(),
            fixed: "true"
        }
    );
    assert_eq!(stop(parse(&["set-role", "ann"])).0, vec![SET_ROLE_USAGE]);
    assert!(matches!(
        parse(&["shares", "ann", "--clear"]),
        Form::Share { set: true, .. }
    ));
    assert_eq!(
        stop(parse(&["share", "ann", "--agent"])).0,
        vec!["Error: --agent needs an agent address"]
    );
    assert_eq!(
        parse(&["audit", "--tail", "5"]),
        Form::Audit {
            tail: "5".into(),
            json: false
        }
    );
    assert_eq!(
        stop(parse(&["audit", "--tail", "x"])).0,
        vec!["Error: --tail must be an integer 1..1000 (got 'x')"]
    );
    assert_eq!(stop(parse(&["audit", "x"])).0, vec!["Error: unexpected argument 'x'", AUDIT_USAGE]);
    assert_eq!(
        parse(&["set-can-clone", "ann", "yes", "extra"]),
        Form::CanClone {
            user: "ann".into(),
            on: true
        }
    );
    assert_eq!(stop(parse(&["set-can-clone", "", "on"])).0, vec![CAN_CLONE_USAGE]);
    assert_eq!(stop(parse(&["unlock"])).0, vec![UNLOCK_USAGE]);
    assert_eq!(
        parse(&["pair-phone", "--user=bo", "--invert"]),
        Form::Phone(phones::Form::Pair {
            user: "bo".into(),
            json: false,
            invert: true
        })
    );
    assert_eq!(stop(parse(&["pair-phone", "--user"])).0, vec!["Error: --user needs a login"]);
    assert_eq!(stop(parse(&["phones", "x"])).0.len(), 2);
    assert_eq!(stop(parse(&["unpair-phone", "-x"])).0, vec!["Error: unknown flag '-x'"]);
}

fn pw(t: &str) -> crate::prompt::Secret {
    crate::prompt::Secret::new(t.as_bytes().to_vec())
}

#[test]
fn add_s_body_is_its_python_s() {
    let mut a = Add {
        username: "ann".into(),
        ..Add::default()
    };
    assert_eq!(
        add_body(&a, &pw("p é")),
        r#"{"username": "ann", "password": "p é", "mustChangePassword": true}"#
    );
    a.role = "owner".into();
    a.fixed = "1";
    assert_eq!(
        add_body(&a, &pw("x")),
        r#"{"username": "ann", "password": "x", "mustChangePassword": false, "fixedPassword": true}"#
    );
    a.role = "voice_chat".into();
    a.fixed = "";
    a.agents = "a\n \nb\n".into();
    a.force = true;
    assert_eq!(
        add_body(&a, &pw("x")),
        r#"{"username": "ann", "password": "x", "role": "voice_chat", "shareAgents": ["a", "b"], "force": true}"#
    );
    assert_eq!(kept(" x \n\n"), vec![" x "]);
}

#[test]
fn the_presence_check_s_secret() {
    let ask = br#"{"error":{"code":"vault_presence_required"},"confirm":{"action":"owner-login","secretId":"s1"}}"#;
    assert_eq!(presence_secret(ask), "s1");
    assert_eq!(presence_secret(br#"{"error":{"code":"x"},"confirm":{"action":"owner-login","secretId":"s1"}}"#), "");
    assert_eq!(presence_secret(b""), "");
    assert_eq!(presence_secret(b"[1]"), "");
}

fn shown(r: Shown) -> (String, String, Exit) {
    (r.stdout, r.stderr, r.exit)
}

#[test]
fn list_reads_as_its_python() {
    let (o, e, x) = shown(read::list(br#"{"users":[{"username":"ann","role":"owner","createdAt":"2026"},{"username":"bo","firstName":"Bo","displayName":"Bo B","mustChangePassword":true,"disabled":1,"wasViewer":true},{"username":"vc","role":"voice_chat","shares":{"agents":[{"address":"a@w"},{"id":"x"}],"gardens":[]}}],"doctor":["d1"]}"#, false, false));
    assert_eq!(
        o,
        "USERNAME  NAME  ROLE        DISABLED          PASSWORD   CREATED\n\
         ann       -     owner       no                -          2026\n\
         bo        Bo B  member      yes (was viewer)  temporary  -\n\
         vc        -     voice_chat  no                -          -\n  \
         vc (Voice & Chat only) shares: agents a@w, x (gone); gardens none\n"
    );
    assert_eq!((e.as_str(), x), ("doctor: d1\n", Exit::Ok));
    let (o, _, _) = shown(read::list(br#"{"users":[{"username":"é","canClone":false}]}"#, true, false));
    assert_eq!(
        o,
        "{\"ok\":true,\"users\":[{\"username\":\"\u{e9}\",\"role\":\"member\",\"disabled\":false,\"wasViewer\":false,\"canClone\":false,\"createdAt\":null,\"firstName\":null,\"lastName\":null,\"displayName\":\"\u{e9}\",\"mustChangePassword\":false,\"shares\":{\"agents\":[],\"gardens\":[]}}]}\n"
    );
    let (_, e, x) = shown(read::list(br#"{"error":{"code":"role_required","hint":"Owner only"}}"#, false, true));
    assert!(e.starts_with("Error: owner_only: Owner/admin only; agents use `k2 connections"));
    assert_eq!(x, Exit::Refused);
    let (_, e, x) = shown(read::list(br#"{"error":{"code":5}}"#, false, false));
    assert!(e.ends_with("TypeError: unsupported operand type(s) for +: 'int' and 'str'\n"), "{e}");
    assert_eq!(x, Exit::Error);
    assert_eq!(shown(read::list(b"<x", false, false)).1, "Error: daemon returned an unparseable response: '<x'\n");
    assert_eq!(shown(read::list(b"", false, false)).0.lines().count(), 2);
}

#[test]
fn the_other_readers() {
    assert_eq!(
        shown(read::add(br#"{"success":true,"role":"voice_chat","shares":{"agents":[{"id":"x"}]},"warnings":["w"],"mustChangePassword":false}"#, "ann", false)),
        (
            "Created user ann.\nVoice & Chat only: joins voice rooms; shared agents: x; shared Gardens: none.\nThe password is kept (not temporary).\n".into(),
            "WARNING: shared anyway (--force): w\n".into(),
            Exit::Ok
        )
    );
    assert_eq!(
        shown(read::add(b"", "ann", true)).1,
        "Error: users/add failed: empty response\n"
    );
    assert_eq!(
        shown(read::set_role(br#"{"success":true,"from":"member","live":false}"#, "ann", "admin")).0,
        "ann: member -> admin (they sign in again)\n"
    );
    assert_eq!(
        shown(read::set_role(br#"{"error":"role_required"}"#, "ann", "admin")),
        (String::new(), "Error: role_required\n".into(), Exit::Refused)
    );
    assert_eq!(
        shown(read::share(br#"{"users":[{"username":"ann","role":"voice_chat","shares":{"sources":[{"name":"s","missing":1}]}}]}"#, " ANN ", false)).0,
        "ann (Voice & Chat only)\n  agents:  none\n  gardens: none\n  sources: s (gone)\n"
    );
    assert!(shown(read::share(b"[1]", "a", false)).1.ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    assert_eq!(
        shown(read::audit(br#"{"events":[{"ts":"t","event":"login","ip":"1.2.3.4"}]}"#)).0,
        "TS  EVENT  OUTCOME  USER  INGRESS  IP       CLIENT\nt   login  -        -     -        1.2.3.4  -\n"
    );
    assert_eq!(
        shown(read::can_clone(b"oops", "ann", true)),
        (String::new(), "Error: oops\n".into(), Exit::Refused)
    );
    assert_eq!(
        shown(read::unlock(br#"{"ok":true,"username":"ann","cleared":true}"#, false)).0,
        "Lock cleared for ann\n"
    );
    assert_eq!(shown(read::unlock(b" ", false)).2, Exit::Error);
}

#[test]
fn the_phone_readers() {
    let r = phones::read(br#"{"error":{"code":"no_such_device"}}"#, |_, _| Ok(()));
    assert_eq!(
        shown(r),
        (String::new(), "Error: no_such_device (no_such_device)\n".into(), Exit::Refused)
    );
    let r = phones::read(br#"{"error":{"hint":3}}"#, |_, _| Ok(()));
    assert!(r.stderr.starts_with("Error: 3\nTraceback"), "{}", r.stderr);
    let d = json::parse(r#"{"user":"ann","code":"12\u0007","qr":{"rows":["10","01"]}}"#).unwrap();
    let mut o = Out::default();
    phones::pair_lines(&d, &mut o, false, false).unwrap();
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(lines[0], "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}");
    assert_eq!(lines[1], "\u{2588}\u{2588}\u{2584}\u{2580}\u{2588}\u{2588}");
    assert_eq!(lines[4], "Pairing code for ann: 12?");
    assert!(o.stdout.ends_with("Works once, for 10 minutes (until ).\nA paired phone stays signed in while used (30 days unused or 180 days in all, then it pairs again).\n"));
    assert_eq!(safe(Some(&s("a\u{200b}b\u{e9}"))), "a?b\u{e9}");
    assert_eq!(safe(None), "");
}

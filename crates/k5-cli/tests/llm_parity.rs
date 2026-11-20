//! `k5-cli llm` against the shell CLI's `cli/k2 llm` (`families::llm`), on
//! the vault rig ([`common::vault`], for stdin and the vault socket): the
//! help and every usage line, each verb's request and layout over TCP and
//! in a K2 terminal, the refusals, the sign-in it follows, the API token
//! and the sign-in code read from stdin (no child sees them), the confirm
//! of `auto-next on`, a body Python can't read, and one large list.

#![cfg(unix)]

mod common;

use common::parity::PASSPORT;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn case(name: &'static str, args: &[&'static str], code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = "llm";
    c.code = code;
    c
}

/// The daemon answers these, in order.
fn daemon(name: &'static str, args: &[&'static str], replies: Vec<Reply>, code: i32) -> VCase {
    let mut c = case(name, args, code);
    c.daemon_requests = replies.len();
    c.daemon = replies;
    c.env = vec![("K2_LLMA_POLL_SECS", "0".to_string())];
    c
}

fn in_session(mut c: VCase) -> VCase {
    c.env.push(("K2_HOOK_SOCK", "{sock}".to_string()));
    c.env.push(("K2_HOOK_TOKEN", PASSPORT.to_string()));
    c
}

fn traceback(mut c: VCase) -> VCase {
    c.compare = VCompare::Traceback;
    c
}

const LIST: &str = r#"{"tools":[{"tool":"claude","display":"Claude","supported":true,"switchedAt":1700000000,"switchedBy":"rosa","autoNext":{"on":true,"lastLine":"moved at 99%"},"accounts":[{"id":"a1","label":"Work","active":true,"state":"signed_in","plan":"max-plan-long","usage":{"windows":[{"label":"5h","used":0.425},{"label":"week","used":"0.5"}],"error":"timeout"},"usageCheckedAt":9999999999,"pinnedTo":[{"scopeKind":"session","label":"chat one"},{"scopeKind":"workspace","scopeId":"ws9"}],"inUse":true,"detail":"all good","signInEndsSoon":true,"signInEndsAt":1700003600},{"id":"a2","label":"A label that is far too long to fit","kind":"api_key","state":null},{"id":"a3","label":"Home","usage":{"status":"no reading"}}]},{"tool":"codex","display":"Codex","supported":true,"accounts":[]},{"tool":"gemini","supported":true,"subscription":false,"accounts":[]},{"tool":"cursor","display":"Cursor","supported":false}],"airgap":true,"keychainRepairs":[{"service":"Claude Code","holds":"a1","partitions":["teamid:x","apple:"],"command":"security set-key"},{"service":"s2","partitions":[]}],"switchNote":""}"#;

const STATUS: &str = r#"{"account":{"tool":"claude","label":"Work \"one\"","active":true,"state":"signed_in","detail":"","plan":"max","email":"a@example.com","signInEndsAt":1700003600,"signInEndsSoon":true,"usage":{"windows":[{"label":"5h","used":0.335,"resetsAt":"t1"},{"label":"weekly-window","used":1}]}}}"#;

const SWITCHED: &str = r#"{"switch":{"tool":"claude","to":"a2","fromLabel":"Work","adopted":[{"created":true,"account":"x@example.com","label":"Found","foundUnder":"Work"},{"account":"y@example.com","label":"Mine"}],"needsSignIn":["Old"]},"account":{"label":"Home"},"warnings":["one warning"]}"#;

#[test]
fn help_and_usage() {
    let rig = VRig::new();
    for c in [
        case("help", &[], 0),
        case("help word", &["help"], 0),
        case("tokens alone", &["tokens"], 0),
        case("an unknown noun", &["nope"], 2),
        case("an unknown verb", &["tokens", "nope"], 2),
        case("an unknown flag", &["tokens", "list", "--nope"], 2),
        case("--tool with no value", &["tokens", "list", "--tool"], 2),
        case("--workspace empty", &["tokens", "pin", "--workspace", ""], 2),
        case("list with a word", &["tokens", "list", "x"], 2),
        case("list --tool gemini", &["tokens", "list", "--tool", "gemini"], 2),
        case("list --tool nope", &["tokens", "list", "--tool", "nope"], 2),
        case("status with one word", &["tokens", "status", "claude"], 2),
        case("status, a tool with none", &["tokens", "status", "pi", "x"], 2),
        case("usage with a word", &["tokens", "usage", "x"], 2),
        case("add with one word", &["tokens", "add", "claude"], 2),
        case("add gemini", &["tokens", "add", "gemini", "x"], 2),
        case("login with none", &["token", "login"], 2),
        case("switch with one", &["tokens", "use", "claude"], 2),
        case("next with none", &["tokens", "next"], 2),
        case("next, an empty tool", &["tokens", "next", ""], 2),
        case("rename with two", &["tokens", "rename", "claude", "a"], 2),
        case("remove with one", &["tokens", "rm", "claude"], 2),
        case("clear-default with none", &["tokens", "clear-default"], 2),
        case("refresh with one", &["tokens", "refresh", "claude"], 2),
        case("storage nope", &["tokens", "storage", "nope"], 2),
        case("storage with two", &["tokens", "storage", "files", "x"], 2),
        case("auto-next with one", &["tokens", "auto-next", "claude"], 2),
        case("auto-next maybe", &["tokens", "auto-next", "claude", "maybe"], 2),
        case("pins with a word", &["tokens", "pins", "x"], 2),
        case("for with none", &["tokens", "for"], 2),
        case("pin with no scope", &["tokens", "pin", "claude", "x"], 2),
        case("unpin with no scope", &["tokens", "unpin", "claude"], 2),
        case(
            "pin with both scopes",
            &["tokens", "pin", "--workspace", "w", "--session", "s", "claude", "x"],
            2,
        ),
        case("pin with one word", &["tokens", "pin", "--workspace", "w", "claude"], 2),
        case("pin cursor", &["tokens", "pin", "--workspace", "w", "cursor", "x"], 2),
        case("unpin nope", &["tokens", "unpin", "--session", "s", "nope"], 2),
        case("add-key with three", &["tokens", "add-key", "claude", "x", "sk-1"], 2),
        case("add-key with one", &["tokens", "add-key", "claude"], 2),
        case("add-key pi", &["tokens", "add-key", "pi", "x"], 2),
    ] {
        rig.check(&c);
    }
    // A line with `-h` or `--help` is the front's (a help page, or the
    // shell's): the binary hands it on.
    for (name, args) in [
        ("accounts --help", &["accounts", "--help"][..]),
        ("a verb's help", &["tokens", "list", "--json", "-h"]),
    ] {
        let mut c = case(name, args, 0);
        c.handed = true;
        rig.check(&c);
    }
    let mut c = case("add-key, an empty stdin", &["tokens", "add-key", "claude", "x"], 2);
    c.stdin = b"\n".to_vec();
    rig.check(&c);
}

/// Help never acts (K2's rule): `-h` or `--help` on the line of a verb
/// that changes something prints the help and sends nothing. The binary
/// hands these lines on (the front's rule for a family's help).
#[test]
fn help_never_acts() {
    let rig = VRig::new();
    for args in [
        &["tokens", "add", "claude", "x", "--help"][..],
        &["tokens", "login", "claude", "x", "-h"],
        &["tokens", "switch", "claude", "x", "--help"],
        &["tokens", "next", "claude", "-h"],
        &["tokens", "rename", "claude", "a", "b", "--help"],
        &["tokens", "remove", "claude", "a", "-h"],
        &["tokens", "clear-default", "claude", "--help"],
        &["tokens", "refresh", "-h"],
        &["tokens", "storage", "vault", "--help"],
        &["tokens", "auto-next", "claude", "on", "-h"],
        &["tokens", "pin", "--workspace", "w", "claude", "x", "--help"],
        &["tokens", "unpin", "--session", "s", "claude", "-h"],
        &["tokens", "add-key", "claude", "x", "--help"],
        &["tokens", "for", "ws", "--refresh", "-h"],
        &["accounts", "add", "--workspace", "--help"],
    ] {
        let mut c = case("help", args, 0);
        c.stdin = b"sk-never-read\n".to_vec();
        c.handed = true;
        rig.check(&c);
    }
}

#[test]
fn reads() {
    let rig = VRig::new();
    let mut big = String::from(r#"{"tools":[{"tool":"claude","supported":true,"accounts":["#);
    for i in 0..3000 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!(
            r#"{{"id":"acct-{i:05}","label":"token number {i}","state":"signed_in","plan":"max","usage":{{"windows":[{{"label":"5h","used":0.{i:03}}}]}}}}"#
        ));
    }
    big.push_str("]}]}");
    assert!(big.len() > 256 * 1024, "the large answer is large");
    let big: &'static str = Box::leak(big.into_boxed_str());
    for c in [
        // covers: llm tokens list
        daemon("list", &["tokens", "list"], vec![ok(LIST)], 0),
        daemon("ls --tool claude", &["tokens", "ls", "--tool", "claude"], vec![ok(LIST)], 0),
        daemon("list, empty", &["accounts", "list"], vec![ok("{}")], 0),
        daemon("list, large", &["tokens", "list"], vec![ok(big)], 0),
        // covers: llm tokens list --json
        daemon("list --json", &["tokens", "list", "--json"], vec![ok(LIST)], 0),
        daemon("list --json, large", &["tokens", "list", "--json"], vec![ok(big)], 0),
        in_session(daemon("list in a K2 terminal", &["tokens", "list"], vec![ok(LIST)], 0)),
        daemon(
            "list refused",
            &["tokens", "list"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"not you"}}"#.into(),
            )],
            3,
        ),
        daemon("list, no answer", &["tokens", "list"], vec![Reply::Close], 1),
        daemon(
            "list, not JSON refused",
            &["tokens", "list"],
            vec![Reply::Answer(500, "boom".into())],
            1,
        ),
        traceback(daemon("list, not JSON", &["tokens", "list"], vec![ok("nope")], 1)),
        traceback(daemon(
            "list, a used that is no number",
            &["tokens", "list"],
            vec![ok(
                r#"{"tools":[{"tool":"claude","supported":true,"accounts":[{"usage":{"windows":[{"used":"lots"}]}}]}]}"#,
            )],
            1,
        )),
        // covers: llm tokens status
        daemon("status", &["tokens", "status", "claude", "Work one"], vec![ok(STATUS)], 0),
        daemon(
            "status, not soon",
            &["tokens", "status", "grok", "a/b?"],
            vec![ok(r#"{"account":{"tool":"grok","signInEndsAt":1700003600}}"#)],
            0,
        ),
        daemon("status, empty", &["tokens", "status", "codex", "x"], vec![ok("{}")], 0),
        // covers: llm tokens status --json
        daemon(
            "status --json",
            &["tokens", "status", "--json", "claude", "a1"],
            vec![ok(STATUS)],
            0,
        ),
        daemon(
            "status not found",
            &["tokens", "status", "claude", "zz"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such token"}}"#.into(),
            )],
            4,
        ),
        // covers: llm tokens usage
        daemon(
            "usage",
            &["tokens", "usage"],
            vec![ok(
                r#"{"rows":[{"active":true,"tool":"claude","label":"Work","usage":{"windows":[{"label":"5h","used":0.5}]}},{"tool":"codex","usage":{"status":"waiting"}},{"tool":"grok"}]}"#,
            )],
            0,
        ),
        daemon("usage, none", &["tokens", "usage"], vec![ok(r#"{"rows":[]}"#)], 0),
        // covers: llm tokens usage --json
        daemon("usage --json", &["tokens", "usage", "--json"], vec![ok("{}")], 0),
        // covers: llm tokens pins
        daemon(
            "pins",
            &["tokens", "pins"],
            vec![
                ok(
                    r#"{"pins":[{"tool":"claude","scopeKind":"session","label":"a chat with a long name over the cut","accountId":"a1"},{"tool":"grok","scopeKind":"workspace","scopeId":"ws9","accountId":"gone"}]}"#,
                ),
                ok(LIST),
            ],
            0,
        ),
        daemon(
            "pins, none",
            &["tokens", "pins"],
            vec![ok(r#"{"pins":[]}"#), ok("{}")],
            0,
        ),
        // covers: llm tokens pins --json
        daemon("pins --json", &["tokens", "pins", "--json"], vec![ok(r#"{"pins":[]}"#)], 0),
        // covers: llm tokens for
        daemon(
            "for",
            &["tokens", "for", "ws/2"],
            vec![ok(
                r#"{"address":"ws/2","tool":"claude","label":"Work","source":"pinned","pinScope":"chat","live":false,"unavailable":"signed out","maxPercent":42.7,"windows":[{"label":"5h","usedPercent":42.6},{"label":"week","used":0.996},{"resetPassed":true}],"updatedAt":"t9","nextStart":{"source":"default"},"refreshed":false,"reason":"rate_limited","retryAfterSecs":30}"#,
            )],
            0,
        ),
        daemon(
            "for, no tool",
            &["tokens", "for", "ws"],
            vec![ok(r#"{"tool":null}"#)],
            0,
        ),
        daemon(
            "for, an API token",
            &["tokens", "for", "ws"],
            vec![ok(
                r#"{"tool":"gemini","kind":"api_key","source":"default","live":true,"refreshed":false}"#,
            )],
            0,
        ),
        daemon(
            "for --refresh",
            &["tokens", "for", "ws", "--refresh"],
            vec![ok(r#"{"tool":"claude","source":"resumed","maxPercent":3,"live":true}"#)],
            0,
        ),
        // covers: llm tokens for --json
        daemon("for --json", &["tokens", "for", "ws", "--json"], vec![ok("{}")], 0),
        daemon(
            "storage",
            &["tokens", "storage"],
            vec![ok(
                r#"{"storage":"vault","source":"env","vaultOpen":false,"vaultError":"","banner":"b","sync":{"sealed":2,"unchanged":1.9,"disabled":true},"export":{"restored":3},"items":[{"tool":"claude","label":"Work","file":true,"vault":"sealed","inStep":true},{"tool":"grok","vault":"-","inStep":null,"removed":true},{"tool":"codex","inStep":false}]}"#,
            )],
            0,
        ),
        traceback(daemon(
            "storage, an odd step",
            &["tokens", "storage"],
            vec![ok(r#"{"items":[{"inStep":"maybe"}]}"#)],
            1,
        )),
        // covers: llm tokens auto-next
        daemon(
            "auto-next",
            &["tokens", "auto-next"],
            vec![ok(
                r#"{"tools":[{"tool":"claude","on":true,"by":"login:rosa","byCanAct":false,"lastLine":"moved"},{"tool":"codex","on":true,"by":"owner-token","byCanAct":true},{"tool":"grok"}],"note":"a note"}"#,
            )],
            0,
        ),
        // covers: llm tokens auto-next --json
        daemon("auto-next --json", &["tokens", "auto-rotate", "--json"], vec![ok("{}")], 0),
    ] {
        rig.check(&c);
    }
}

#[test]
fn writes() {
    let rig = VRig::new();
    let list = || ok(LIST);
    for c in [
        // covers: llm tokens switch
        daemon(
            "switch",
            &["tokens", "switch", "claude", "home"],
            vec![list(), ok(SWITCHED)],
            0,
        ),
        daemon(
            "switch, already",
            &["tokens", "use", "claude", "a1"],
            vec![list(), ok(r#"{"switch":{"alreadyActive":true,"to":"a1"}}"#)],
            0,
        ),
        daemon(
            "switch, saved",
            &["tokens", "switch", "claude", "work", "--json"],
            vec![list(), ok(r#"{"switch":{"appliedSaved":true}}"#)],
            0,
        ),
        daemon(
            "switch, no such token",
            &["tokens", "switch", "codex", "work"],
            vec![list()],
            4,
        ),
        daemon(
            "switch, the list refused",
            &["tokens", "switch", "claude", "work"],
            vec![Reply::Answer(401, r#"{"error":"Invalid or missing auth token"}"#.into())],
            3,
        ),
        // covers: llm tokens next
        daemon(
            "next",
            &["tokens", "next", "claude"],
            vec![ok(r#"{"switch":{"tool":"claude","to":"a3","profileSwapped":false}}"#)],
            0,
        ),
        daemon(
            "next refused",
            &["tokens", "next", "grok"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"admin_only","hint":"ask"},"permission":{"command":"k2 permission request --scope llm.switch --why \"...\"","requestable":true}}"#.into(),
            )],
            3,
        ),
        in_session(daemon(
            "next in a K2 terminal: refused",
            &["tokens", "next", "claude"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"read_only","hint":"passports read only"}}"#.into(),
            )],
            3,
        )),
        daemon(
            "rename",
            &["tokens", "rename", "claude", "Work", "Job"],
            vec![list(), ok("{}")],
            0,
        ),
        daemon(
            "remove",
            &["tokens", "remove", "claude", "a3", "--clear-default"],
            vec![list(), ok("{}")],
            0,
        ),
        daemon(
            "clear-default",
            &["tokens", "clear-default", "claude"],
            vec![ok(
                r#"{"cleared":{"cleared":true,"nowDefault":true,"clearedLabel":"Work","nowDefaultLabel":"Home"}}"#,
            )],
            0,
        ),
        daemon(
            "clear-default, none",
            &["tokens", "clear-default", "grok"],
            vec![ok("{}")],
            0,
        ),
        daemon(
            "refresh all",
            &["tokens", "refresh", "--usage"],
            vec![ok(r#"{"accounts":[{"tool":"claude","label":"Work","active":true,"state":"ok"},{}]}"#)],
            0,
        ),
        daemon(
            "refresh one",
            &["tokens", "refresh", "claude", "work"],
            vec![list(), ok(r#"{"accounts":[]}"#)],
            0,
        ),
        daemon(
            "storage set",
            &["tokens", "storage", "files"],
            vec![ok(r#"{"storage":"files","source":"setting","vaultOpen":true}"#)],
            0,
        ),
        // covers: llm tokens pin
        daemon(
            "pin",
            &["tokens", "pin", "--session", "s1", "gemini", "work"],
            vec![
                ok(r#"{"tools":[{"tool":"gemini","accounts":[{"id":"g1","label":"WORK"}]}]}"#),
                ok(r#"{"pin":{"scopeKind":"session","scopeId":"s1","tool":"gemini"},"note":"restart the chat"}"#),
            ],
            0,
        ),
        daemon(
            "pin --json",
            &["tokens", "pin", "--workspace", "w", "claude", "a1", "--json"],
            vec![list(), ok(r#"{"pin":{}}"#)],
            0,
        ),
        // covers: llm tokens unpin
        daemon(
            "unpin",
            &["tokens", "unpin", "--workspace", "w", "claude"],
            vec![ok(r#"{"unpinned":true}"#)],
            0,
        ),
        daemon(
            "unpin, none",
            &["tokens", "unpin", "--session", "s", "codex"],
            vec![ok(r#"{"unpinned":false}"#)],
            0,
        ),
        traceback(daemon(
            "unpin, not JSON",
            &["tokens", "unpin", "--session", "s", "codex"],
            vec![ok("nope")],
            0,
        )),
        daemon(
            "auto-next off",
            &["tokens", "auto-next", "claude", "off"],
            vec![ok(r#"{"tools":[]}"#)],
            0,
        ),
    ] {
        rig.check(&c);
    }
}

const KEY: &str = "sk-llm-secret-4242";
const CODE: &str = "PASTED-CODE-3131";
const PASSWORD: &str = "pw-llm-5151";
const TOKEN: &str = "ctok-llm-6161";

#[test]
fn secrets_from_stdin() {
    let rig = VRig::new();
    // covers: llm tokens add-key
    let mut add_key = daemon(
        "add-key",
        &["tokens", "add-key", "claude", "Work key"],
        vec![ok(r#"{"added":true}"#), ok(r#"{"offer":"meet_kessel"}"#)],
        0,
    );
    add_key.stdin = format!("  {KEY}  \n").into_bytes();
    add_key.secrets = vec![KEY];
    let mut add_key_json = daemon(
        "add-key --json, no newline",
        &["tokens", "add-key", "gemini", "k", "--json"],
        vec![ok(r#"{"added":true}"#)],
        0,
    );
    add_key_json.stdin = KEY.as_bytes().to_vec();
    add_key_json.secrets = vec![KEY];
    let mut refused = daemon(
        "add-key refused",
        &["tokens", "add-key", "grok", "k"],
        vec![Reply::Answer(
            400,
            r#"{"error":{"code":"bad_key","hint":"not a key"}}"#.into(),
        )],
        2,
    );
    refused.stdin = format!("{KEY}\n").into_bytes();
    refused.secrets = vec![KEY];
    // covers: llm tokens add
    let mut add = daemon(
        "add: the sign-in, followed",
        &["tokens", "add", "claude", "Work"],
        vec![
            ok(r#"{"login":{"loginId":"L 1/x"}}"#),
            ok(r#"{"login":{"state":"waiting","url":"https://example.com/a","code":"AB-CD","banner":"Opening the sign-in"}}"#),
            ok(r#"{"login":{"state":"waiting","url":"https://example.com/a","code":"AB-CD","banner":"Opening the sign-in"}}"#),
            ok(r#"{"login":{"state":"waiting_for_code"}}"#),
            ok("{}"),
            ok(r#"{"login":{"state":"signed_in","tool":"claude","label":"Work","offerMakeActive":true}}"#),
            ok(r#"{"rows":[]}"#),
        ],
        0,
    );
    add.stdin = format!("{CODE}\n").into_bytes();
    add.secrets = vec![CODE];
    let mut no_code = daemon(
        "add: no code pasted",
        &["tokens", "add", "codex", "Home", "--device"],
        vec![
            ok(r#"{"login":{"loginId":"L2"}}"#),
            ok(r#"{"login":{"state":"waiting_for_code"}}"#),
            ok("{}"),
        ],
        1,
    );
    no_code.stdin = b"half a line".to_vec();
    let mut cancel_refused = daemon(
        "add: no code, the cancel refused",
        &["tokens", "add", "codex", "Home"],
        vec![
            ok(r#"{"login":{"loginId":"L2"}}"#),
            ok(r#"{"login":{"state":"waiting_for_code"}}"#),
            Reply::Answer(404, r#"{"error":"gone"}"#.into()),
        ],
        4,
    );
    cancel_refused.stdin = Vec::new();
    // covers: llm tokens auto-next
    let mut on = daemon(
        "auto-next on: the confirm",
        &["tokens", "auto-next", "claude", "on"],
        vec![ok(r#"{"tools":[{"tool":"claude","on":true,"by":"owner-token","byCanAct":true}]}"#)],
        0,
    );
    on.vault = Some(vec![
        ok(r#"{"methods":["vault-password"]}"#),
        ok(&format!(r#"{{"token":"{TOKEN}"}}"#)),
    ]);
    on.vault_requests = 2;
    on.stdin = format!("{PASSWORD}\n").into_bytes();
    on.secrets = vec![PASSWORD, TOKEN];
    let mut on_refused = case(
        "auto-next on: the confirm refused",
        &["tokens", "auto-next", "grok", "on"],
        3,
    );
    on_refused.vault = Some(vec![Reply::Answer(
        403,
        r#"{"error":{"code":"owner_only","hint":"a person"}}"#.into(),
    )]);
    on_refused.vault_requests = 1;
    for c in [
        add_key,
        add_key_json,
        refused,
        add,
        no_code,
        cancel_refused,
        on,
        on_refused,
        daemon(
            "add --json",
            &["tokens", "add", "grok", "x", "--json"],
            vec![ok(r#"{"login":{"loginId":"L3"}}"#)],
            0,
        ),
        daemon(
            "add: no login started",
            &["tokens", "add", "grok", "x"],
            vec![ok("{}")],
            1,
        ),
        daemon(
            "add: a start that is not JSON",
            &["tokens", "add", "grok", "x"],
            vec![ok("nope")],
            1,
        ),
        daemon(
            "add: the sign-in failed",
            &["tokens", "add", "grok", "x"],
            vec![
                ok(r#"{"login":{"loginId":"L4"}}"#),
                ok(r#"{"login":{"state":"failed","error":"denied\tby you"}}"#),
            ],
            1,
        ),
        daemon(
            "add: it ended",
            &["tokens", "add", "grok", "x"],
            vec![
                ok(r#"{"login":{"loginId":"L5"}}"#),
                ok(r#"{"login":{"done":true}}"#),
            ],
            1,
        ),
        daemon(
            "login: found, signed in",
            &["tokens", "login", "claude", "work"],
            vec![
                ok(LIST),
                ok(r#"{"login":{"loginId":"L6"}}"#),
                ok(r#"{"login":{"state":"signed_in","tool":"claude","label":"Work"}}"#),
                Reply::Answer(503, "{}".into()),
            ],
            0,
        ),
    ] {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = VRig::new();
    let cli = k2_client::brand::CLI_NAME;
    for (args, err) in [
        (&["nope"][..], format!("usage: unknown llm verb 'nope' ({cli} llm tokens --help)\n")),
        (&["tokens", "list", "--x"][..], format!("usage: unknown flag '--x' ({cli} llm tokens --help)\n")),
        (&["tokens", "usage", "x"][..], format!("usage: {cli} llm tokens usage [--json]\n")),
        (&["tokens", "next", "nope"][..], format!("usage: unknown tool 'nope' (claude, codex or grok) — {cli} llm tokens --help\n")),
        (&["tokens", "pin", "--session", "s", ""][..], format!("usage: {cli} llm tokens pin --workspace <ws>|--session <key> <tool> <label>\n")),
    ] {
        let c = case("brand", args, 2);
        let ran = rig.run(false, false, &c);
        assert_eq!(ran.out.stderr, err, "{args:?}");
        assert_eq!(ran.out.code, Some(2), "{args:?}");
    }
    let c = daemon(
        "brand: list",
        &["tokens", "list"],
        vec![ok(r#"{"tools":[{"tool":"codex","supported":true,"accounts":[]}]}"#)],
        0,
    );
    let ran = rig.run(false, false, &c);
    assert!(
        ran.out
            .stdout
            .contains(&format!("(add a subscription: {cli} llm tokens add codex <label>)")),
        "{ran:#?}"
    );
    if cli != "k2" {
        assert!(!ran.out.stdout.contains("k2 ") && !ran.out.stdout.contains("k5 "));
    }
}

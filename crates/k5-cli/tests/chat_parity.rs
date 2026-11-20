//! `k5-cli chat` against the shell CLI's `cli/k2 chat`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - Every verb's lines and `--json`: spaces, channels, messages, read,
//!   wait (and its exit 2), whoami, unread, mark-read, follow, prefs,
//!   send, post, reply, dm, edit, delete, outbox, access, channel, space,
//!   role, ban, unban, timeout. The message text from stdin (`-`).
//! - The daemon's refusals (`_chat_fail`: 400 is exit 2, 401 and 403 are
//!   3, the rest 1, with `details.key`; not JSON; no body), a permission
//!   card's closing line, Python's raise part way, an empty 200.
//! - The words: `_chat_usage_err`'s lines, `chat help …` (with no port
//!   too: no gate there), `--parts`.
//! - A passport over TCP, the cell socket, a POST with no answer there,
//!   the id tool's lines.
//! - One large list (above 256 KiB): messages.
//! - Help never acts.
//! - What the binary hands to the shell.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "chat";
const SPACES: &str = r#"{"spaces":[{"space":"acme","level":"send","approval":"none","name":"Acme é"},{"space":"b"}]}"#;
const MESSAGES: &str = r#"{"addr":"acme/general","messages":[{"id":"m1","createdAt":1700000000,"author":{"username":"bot","kind":"agent"},"body":"hi","mine":true,"replyCount":2,"edited":1},{"id":"m2","createdAt":"x","author":{"username":"ann"},"body":"yo é","onBehalfOf":"bob","onBehalfReview":{"state":"ok","decidedBy":"bob"},"canReply":false,"reason":"mention","addr":"acme/general","replyTo":"m1","mentionsMe":true},{"id":"m3","deleted":true,"onBehalfOfName":"Bob"}],"hasMore":true,"nextBefore":"m0"}"#;
const POSTED: &str = r#"{"status":"posted","id":"m5","addr":"acme/general"}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn one(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

#[test]
fn chat_reads_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    let v = vec![
        // covers: chat spaces
        one("spaces", &["spaces"], SPACES),
        one("spaces --json", &["spaces", "--json"], SPACES),
        one("spaces: none", &["spaces"], r#"{"spaces":[]}"#),
        // covers: chat
        one(
            "channels",
            &["channels", "acme"],
            r#"{"channels":[{"addr":"acme/general","kind":"text","private":true,"archived":1,"canSend":false},{"addr":"acme/x","kind":"voice","canSend":0}]}"#,
        ),
        // covers: chat messages
        one(
            "messages",
            &["messages", "acme/general", "--limit", "5"],
            MESSAGES,
        ),
        one(
            "messages --in, --before",
            &["messages", "--in", "acme/a b", "--before", "m9"],
            r#"{"addr":"acme/a b","messages":[]}"#,
        ),
        // covers: chat read
        one(
            "read",
            &["read", "m2"],
            r#"{"message":{"id":"m2","author":{"username":"ann"},"body":"x"}}"#,
        ),
        one("read: no message", &["read", "m2"], r#"{}"#),
        one(
            "whoami",
            &["whoami"],
            r#"{"workspace":"w","username":"bot","memberId":"u1","assistantOf":"ann","spaces":[{"space":"s","level":"send","approval":"none"}],"queued":0,"chatManage":true}"#,
        ),
        // covers: chat unread
        one(
            "unread --all",
            &["unread", "--all"],
            r#"{"channels":[{"addr":"s/c","unread":true,"mentions":1},{"addr":"s/d","mentions":true,"muted":1},{"addr":"s/e","mentions":3}]}"#,
        ),
        one("unread: none", &["unread"], r#"{"channels":[]}"#),
        // covers: chat outbox
        one(
            "outbox",
            &["outbox", "--limit", "2"],
            r#"{"items":[{"id":"out_1","status":"queued","action":"send","addr":"s/c","onBehalfOf":"ann","agent":"bot","decidedBy":"bob","decidedKind":"person","note":"n","delivery":"sent","messageId":"m1","preview":"p é"}]}"#,
        ),
        one("outbox <id>: none", &["outbox", "out_1"], r#"{"items":[]}"#),
        // covers: chat access
        one(
            "access: mine",
            &["access"],
            r#"{"workspace":"w","access":[],"chatManage":true}"#,
        ),
        one(
            "access list",
            &["access", "list", "--space", "s", "--workspace", "w"],
            r#"{"access":[{"workspace":"w","space":"s","level":"read","approval":"required"},{"space":"t"}]}"#,
        ),
        // covers: chat prefs
        one(
            "prefs",
            &["prefs"],
            r#"{"prefs":{"quietHours":"22:00-07:00","tz":"UTC","batch":"5m"},"quietNow":true,"hub":{"running":true,"delivered":3,"lastOutcome":"ok","lastError":"e"},"follows":[{"addr":"s/c","batch":"1m"},{"addr":"s/d"}],"followBatchesCleared":2}"#,
        ),
        // covers: chat space
        one(
            "space members list",
            &["space", "members", "list", "acme"],
            r#"{"members":[{"username":"ann","kind":"person","id":"u1"},{"username":"bot","kind":"agent","id":"u2"}]}"#,
        ),
        traceback(case(
            "not JSON",
            &["spaces"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an empty 200",
            &["whoami"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a message that is no dict, part way",
            &["messages", "s/c"],
            &[],
            vec![ok(
                r#"{"messages":[{"id":"m","author":{"username":"a"},"mine":1,"body":"b"},"x"]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a batch that is no string",
            &["prefs"],
            &[],
            vec![ok(r#"{"prefs":{"tz":1}}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: chat channel
        traceback(case(
            "the generic lines: not a dict",
            &["channel", "set", "s/c", "--private", "off"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn chat_wait_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "wait");
    let v = vec![
        // covers: chat wait
        one(
            "wait --in: a message",
            &[
                "wait",
                "--in",
                "s/c",
                "--mention",
                "--after",
                "m1",
                "--timeout",
                "1m",
            ],
            r#"{"messages":[{"id":"m2","author":{"username":"ann"},"body":"hi"}],"nextAfter":"m2"}"#,
        ),
        one(
            "wait --outbox: the item",
            &["wait", "--outbox", "out_1"],
            r#"{"item":{"id":"out_1","status":"sent","action":"send","addr":"s/c"}}"#,
        ),
        case(
            "wait: timed out, exit 2",
            &["wait", "--mention"],
            &[],
            vec![ok(r#"{"timedOut":true,"messages":[]}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "wait --json: timed out, exit 2 and no line",
            &["wait", "--json"],
            &[],
            vec![ok(r#"{"timedOut":true}"#)],
            owner_and_port,
            2,
            1,
        ),
        traceback(case(
            "wait --json: not a dict, exit 2",
            &["wait", "--json", "--outbox"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            2,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn chat_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let v = vec![
        // covers: chat send
        one(
            "send",
            &[
                "send",
                "acme/general",
                "hi \"there\"\n",
                "--key",
                "k1",
                "--no-wake",
                "--kind",
                "note",
                "--respond-to",
                "m1",
            ],
            POSTED,
        ),
        one(
            "send --on-behalf-of",
            &["send", "acme/general", "x", "--on-behalf-of", "@ann"],
            r#"{"id":"m5","onBehalf":{"onBehalfOf":"ann","state":"pending"}}"#,
        ),
        one(
            "post: queued",
            &["post", "acme/general", "x"],
            r#"{"status":"queued","id":"out_7"}"#,
        ),
        // covers: chat reply
        one(
            "reply --broadcast",
            &["reply", "m1", "x", "--broadcast"],
            POSTED,
        ),
        // covers: chat dm
        one("dm", &["dm", "@ann", "hello"], r#"{"id":"m6"}"#),
        // covers: chat edit
        one(
            "edit",
            &["edit", "m1", "new"],
            r#"{"status":"edited","id":"m1"}"#,
        ),
        one(
            "delete",
            &["delete", "m1", "--key", "k"],
            r#"{"status":"deleted","id":"m1"}"#,
        ),
        one(
            "mark-read --all",
            &["mark-read", "--all"],
            r#"{"marked":["s/c","s/d"]}"#,
        ),
        one("ack <addr>", &["ack", "s/c"], r#"{"marked":[]}"#),
        // covers: chat follow
        one(
            "follow --batch",
            &["follow", "s/c", "--batch", "5m"],
            r#"{"addr":"s/c","following":true,"batch":"5m"}"#,
        ),
        one("unfollow", &["unfollow", "m1"], r#"{"thread":"m1"}"#),
        one(
            "prefs set",
            &["prefs", "set", "--quiet-hours", "off", "--tz", "box"],
            r#"{"prefs":{}}"#,
        ),
        one(
            "outbox cancel",
            &["outbox", "cancel", "out_1"],
            r#"{"ok":true,"id":"out_1","status":"cancelled","n":0,"l":[],"d":{"a":[1,"é"]}}"#,
        ),
        one(
            "channel create",
            &["channel", "create", "acme/a/b", "--private", "--key", "k"],
            r#"{"ok":true,"channel":"acme/a/b"}"#,
        ),
        one(
            "channel set",
            &["channel", "set", "acme/a", "--private", "off"],
            r#"{"ok":true}"#,
        ),
        one(
            "channel perms",
            &[
                "channel",
                "perms",
                "acme/a",
                "--role",
                "everyone",
                "--allow",
                " view, ,send",
                "--deny",
                "send",
            ],
            r#"{"ok":true,"to":"acme/a"}"#,
        ),
        one(
            "space members add",
            &["space", "members", "add", "acme", "@ann"],
            r#"{"ok":true}"#,
        ),
        one(
            "space create --app",
            &[
                "space", "create", "acme", "--name", "Acme Co", "--app", "app1",
            ],
            r#"{"ok":true,"space":"acme"}"#,
        ),
        // covers: chat role
        one(
            "role add",
            &["role", "add", "acme", "mods", "@ann"],
            r#"{"ok":true}"#,
        ),
        one(
            "role create",
            &["role", "create", "acme", "mods"],
            r#"{"ok":true}"#,
        ),
        one(
            "ban --reason",
            &["ban", "acme", "@eve", "--reason", "spam \u{e9}"],
            r#"{"ok":true}"#,
        ),
        one("unban", &["unban", "acme", "eve"], r#"{"ok":true}"#),
        one(
            "mute --for",
            &["mute", "acme", "eve", "--for", "2h"],
            r#"{"ok":true}"#,
        ),
        one(
            "access grant",
            &[
                "access",
                "grant",
                "acme",
                "w",
                "--level",
                "post",
                "--approval",
                "none",
            ],
            r#"{"ok":true,"workspace":"w"}"#,
        ),
        one(
            "access set",
            &["access", "set", "acme", "w", "--approval", "required"],
            r#"{"ok":true}"#,
        ),
        one(
            "access revoke",
            &["access", "revoke", "acme", "w"],
            r#"{"ok":true}"#,
        ),
        stdin(
            "a line \u{e9}\nand more\n\n".as_bytes(),
            one(
                "send -: the text from stdin",
                &["send", "acme/general", "-"],
                POSTED,
            ),
        ),
        stdin(
            b"bytes \xff\xe2\x82 not UTF-8\n",
            one(
                "edit -: bytes that are not UTF-8",
                &["edit", "m1", "-"],
                POSTED,
            ),
        ),
        stdin(
            b"",
            one("send -: nothing on stdin", &["send", "s/c", "-"], POSTED),
        ),
        case(
            "a refusal with its key: exit 3",
            &["send", "s/c", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"no_send","hint":"not here é","details":{"key":"k1"}}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "401: exit 3",
            &["spaces"],
            &[],
            vec![Reply::Answer(401, r#"{"error":{"code":"auth"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a bad request: exit 2",
            &["dm", "ann", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"bad","message":"too long"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not found, not JSON: exit 1",
            &["delete", "m1"],
            &[],
            vec![Reply::Answer(404, "not json\r\nat all\n".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 500 with no body: exit 1",
            &["whoami"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a permission card",
            &["ban", "acme", "eve"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a reset: status 000, exit 1",
            &["spaces"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn chat_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let bad = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], owner_and_port, 2, 0)
    };
    // The family's own forms (`chat help …`, an empty or unknown verb) are
    // the shell's: chat's row is the shell's but for its verbs.
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "chat help x: its help",
            &["help", "x"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "chat help x with no port: its help",
            &["help", "x"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        )),
        shell(case(
            "chat help x, the manage piece off",
            &["help", "x"],
            &[("_CHAT_HELP_MANAGE", "0")],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "an empty verb with no port: no gate",
            &[""],
            &[],
            vec![],
            owner_only,
            2,
            0,
        )),
        case(
            "spaces with no port: the gate",
            &["spaces"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "--parts: needs KM1, exit 1",
            &["send", "s/c", "x", "--parts", "a"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        shell(bad("an unknown verb", &["nope"])),
        bad("an unknown flag, as it is", &["spaces", "--x\"y\u{e9}"]),
        bad("--as", &["send", "s/c", "x", "--as", "ann"]),
        bad(
            "a value flag with no value",
            &["messages", "s/c", "--limit"],
        ),
        bad("a value flag with an empty value", &["outbox", "--key", ""]),
        bad("--on-behalf-of on read", &["read", "m1", "--behalf", "ann"]),
        bad("--broadcast on send", &["send", "s/c", "x", "--broadcast"]),
        bad("messages: no address", &["messages"]),
        bad("read: no id", &["read"]),
        bad("wait --after with no --in", &["wait", "--after", "m1"]),
        bad("mark-read: both", &["mark-read", "s/c", "--all"]),
        bad("mark-read: neither", &["mark-read"]),
        bad("follow: nothing", &["follow"]),
        bad("prefs: flags with no set", &["prefs", "--tz", "box"]),
        bad("prefs set: nothing", &["prefs", "set"]),
        bad("prefs: another word", &["deliveries", "x"]),
        bad("send: no text", &["send", "s/c"]),
        bad("reply: no text", &["reply", "m1"]),
        bad("dm: nothing", &["dm"]),
        bad("edit: no text", &["edit", "m1"]),
        bad("delete: no id", &["delete"]),
        bad("outbox cancel: no id", &["outbox", "cancel"]),
        bad("access: a bad level", &["access", "list", "--level", "x"]),
        bad("access: a bad approval", &["access", "--approval", "x"]),
        bad("access grant: no level", &["access", "grant", "s", "w"]),
        bad("access set: nothing to set", &["access", "set", "s", "w"]),
        bad("access revoke: one word", &["access", "revoke", "s"]),
        bad("access: another word", &["access", "x"]),
        bad("channel create: no slash", &["channel", "create", "acme"]),
        bad("channel set: no --private", &["channel", "set", "s/c"]),
        bad(
            "channel perms: no allow or deny",
            &["channel", "perms", "s/c", "--role", "r"],
        ),
        bad("channel: another word", &["channel"]),
        bad(
            "space members list: no space",
            &["space", "members", "list"],
        ),
        bad(
            "space members add: no member",
            &["space", "members", "add", "s"],
        ),
        bad("space members: another word", &["space", "members"]),
        bad("space create: no slug", &["space", "create"]),
        bad(
            "space create: --app and --owner",
            &["space", "create", "s", "--app", "a", "--owner", "b"],
        ),
        bad("space: another word", &["space"]),
        bad("role create: no role", &["role", "create", "s"]),
        bad("role add: no member", &["role", "add", "s", "r"]),
        bad("role: another word", &["role", "x"]),
        bad("ban: no user", &["ban", "s"]),
        bad("timeout: no --for", &["timeout", "s", "u"]),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["spaces"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn chat_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            with_passport(&[]),
            one("a host session: TCP", &["spaces"], SPACES),
        ),
        with(
            cell(),
            case(
                "the cell socket",
                &["whoami"],
                &[],
                vec![ok(r#"{"workspace":"w"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served: TCP",
                &["send", "s/c", "x"],
                &[],
                vec![not_served(), ok(POSTED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "a GET with no answer there: TCP",
                &["spaces"],
                &[],
                vec![Reply::Close, ok(SPACES)],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(),
            case(
                "a POST with no answer there: not sent again",
                &["send", "s/c", "x"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn chat_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    for c in [
        Case {
            compare: Compare::Shell,
            ..case(
                "K2_CP_MAX_TIME set: the shell reads it",
                &["send", "s/c", "x"],
                &[("K2_CP_MAX_TIME", "30")],
                vec![ok(POSTED)],
                owner_and_port,
                0,
                1,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "a K2_CJ_ variable set: the shell's",
                &["send", "s/c", "x"],
                &[("K2_CJ_extra", "1")],
                vec![ok(POSTED)],
                owner_and_port,
                0,
                1,
            )
        },
    ] {
        rig.check(&c);
    }
}

/// The large list: above 256 KiB, thousands of rows. The answer reaches
/// the shell's Python on stdin, so bash prints it too.
#[test]
fn chat_messages_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| format!(r#"{{"id":"m{i}","createdAt":{},"author":{{"username":"user{}"}},"body":"message number {i} in the channel"}}"#, 1_700_000_000 + i * 61, i % 50))
        .collect();
    let body = format!(
        r#"{{"addr":"acme/general","messages":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "a large list",
        &["messages", "acme/general"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn chat_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens: status 000, exit 1",
        &["spaces"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell. Where bash acts anyway (a value flag takes
/// `-h`/`--help` as its value, so the loop never sees it: a bash bug,
/// cli/src/chat.sh:295-335), the rows copy bash and say so.
#[test]
fn chat_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str,
                 args: &'static [&'static str],
                 replies: Vec<Reply>,
                 requests: usize| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], replies, owner_and_port, 0, requests)
    };
    for c in [
        shell("send --help", &["send", "s/c", "hi", "--help"], vec![], 0),
        shell("post -h first", &["post", "-h", "s/c", "hi"], vec![], 0),
        shell("reply -h", &["reply", "m1", "x", "-h"], vec![], 0),
        shell("dm --help", &["dm", "ann", "x", "--help"], vec![], 0),
        shell("edit -h", &["edit", "m1", "x", "-h"], vec![], 0),
        shell("delete --help", &["delete", "m1", "--help"], vec![], 0),
        shell("mark-read -h", &["mark-read", "--all", "-h"], vec![], 0),
        shell("follow --help", &["follow", "s/c", "--help"], vec![], 0),
        shell("unfollow -h", &["unfollow", "s/c", "-h"], vec![], 0),
        shell(
            "prefs set -h",
            &["prefs", "set", "--tz", "box", "-h"],
            vec![],
            0,
        ),
        shell(
            "outbox cancel --help",
            &["outbox", "cancel", "out_1", "--help"],
            vec![],
            0,
        ),
        shell(
            "channel create -h",
            &["channel", "create", "s/c", "-h"],
            vec![],
            0,
        ),
        shell(
            "channel perms --help",
            &[
                "channel", "perms", "s/c", "--role", "r", "--allow", "view", "--help",
            ],
            vec![],
            0,
        ),
        shell(
            "space create -h",
            &["space", "create", "s", "-h"],
            vec![],
            0,
        ),
        shell(
            "space members add --help",
            &["space", "members", "add", "s", "u", "--help"],
            vec![],
            0,
        ),
        shell(
            "role add -h",
            &["role", "add", "s", "r", "u", "-h"],
            vec![],
            0,
        ),
        shell("ban --help", &["ban", "s", "u", "--help"], vec![], 0),
        shell("unban -h", &["unban", "s", "u", "-h"], vec![], 0),
        shell(
            "timeout --help",
            &["timeout", "s", "u", "--for", "2h", "--help"],
            vec![],
            0,
        ),
        shell(
            "access grant -h",
            &["access", "grant", "s", "w", "--level", "read", "-h"],
            vec![],
            0,
        ),
        shell(
            "access revoke --help",
            &["access", "revoke", "s", "w", "--help"],
            vec![],
            0,
        ),
        // The bash bug, copied: a value flag eats the help word and the
        // verb runs.
        shell(
            "BASH BUG: send --key -h sends",
            &["send", "s/c", "hi", "--key", "-h"],
            vec![ok(POSTED)],
            1,
        ),
        shell(
            "BASH BUG: ban --reason --help bans",
            &["ban", "s", "u", "--reason", "--help"],
            vec![ok(r#"{"ok":true}"#)],
            1,
        ),
        shell(
            "BASH BUG: mute --for -h mutes",
            &["mute", "s", "u", "--for", "-h"],
            vec![ok(r#"{"ok":true}"#)],
            1,
        ),
    ] {
        rig.check(&c);
    }
}

//! `k5-cli mail` reads against the shell CLI's `cli/k2 mail`, on the
//! parity rig ([`common::parity`]): the same requests (urllib's, from
//! `_mail_py`), stdout, stderr and exit code.
//!
//! - Every verb, plain and `--json`: inboxes (and `addresses`, `list`;
//!   the agent view, a blocked send, the owner's catalog with grants),
//!   domains, messages (filters, the next page, the search window, inbox
//!   errors), read (`--html`, `--raw`), attachments (the list and
//!   `--get`), wait (a match, and its timed-out answer, exit 2), drafts,
//!   outbox (the list, one, `cancel`), capabilities (one inbox, all, the
//!   owner view asked again outside a workspace), folder (list, create,
//!   rename, and no sub-command: the help page).
//! - Each `cmd_mail_*`'s usage lines (exit 2, nothing sent).
//! - The daemon's errors: a permission card (exit 3), owner only and
//!   forbidden (the teaching hint), a missing capability (`cap`,
//!   `reason`, `fix`), usage and too large (exit 2), a string, `ok`
//!   missing, an HTTP error that is not JSON, a 2xx that is not JSON,
//!   empty or `null`; answers Python raises on (a list, a number to
//!   walk); the 503 "migrating" answer asked again; no answer in time
//!   (`K5_MAIL_HTTP_TIMEOUT`, and `attachments --get`'s own hint).
//! - No answer: nothing listening (four tries), closed with nothing and a
//!   reset (a GET once more, a POST never), not HTTP.
//! - The owner from disk, a passport over TCP, the `mail` id tool's
//!   refusal in a session with no passport, the gate.
//! - Help never acts: the verbs that change something with `-h` or
//!   `--help` anywhere print their help and send nothing.
//! - One answer above 256 KiB: thousands of messages.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "mail";

const AGENT_INBOXES: &str = r#"{"ok":true,"inboxes":[
 {"address":"bot@acme.example","source":"hosted","yourLevel":"send","yourSendState":"blocked_approval","maxLevel":"send","status":"active"},
 {"address":"me@globex.example","source":"linked","yourLevel":"read","yourSendState":"no_send_level","maxLevel":"draft","status":"ok é"},
 {"address":"x@acme.example","yourLevel":"weird","yourSendState":null}]}"#;

const AGENT_OFF: &str = r#"{"ok":true,"inboxes":[
 {"address":"bot@acme.example","source":"hosted","yourLevel":"send","yourSendState":"blocked_off","maxLevel":"send","status":"active"}]}"#;

const OWNER_INBOXES: &str = r#"{"ok":true,"inboxes":[
 {"address":"bot@acme.example","source":"hosted","primary":{"workspace":"alpha","level":"send","sendState":"on"},
  "grants":[{"workspace":"beta","level":"send","sendState":"blocked_off"},{"projectId":"p-9","level":"read"}]},
 {"address":"longer-address@globex.example","source":"linked","primary":{"projectId":"p-1","level":"draft","sendState":"blocked_approval"},"grants":[]}]}"#;

const DOMAINS: &str = r#"{"ok":true,"domains":[{"domain":"acme.example","isDefault":true},{"domain":"globex.example"}],"pending":["new.example"]}"#;

const MESSAGES: &str = r#"{"ok":true,"messages":[
 {"id":"m1","from":{"name":"Ann","address":"ann@acme.example"},"subject":"Hello é","date":"2020-01-02T03:04:05Z","hasAttachment":true,"unread":true},
 {"id":"m2","from":{"address":"bob@globex.example"},"subject":"","date":"2021-06-01"},
 {"id":"m3","from":null,"date":"not a date"}],
 "nextOffset":25,"searchWindow":{"hint":"searched the last 90 days"},"inboxErrors":[{"address":"x"},{"address":"y"}]}"#;

const MESSAGE: &str = r#"{"ok":true,"message":{"id":"m1","address":"bot@acme.example",
 "from":{"name":"Ann","address":"ann@acme.example"},"to":[{"address":"bot@acme.example"},{"name":"B","address":"b@x.example"}],
 "cc":[{"address":"c@x.example"}],"subject":"Hi","date":"2020-01-02T03:04:05+02:00",
 "authResults":{"spf":"pass","dkim":null,"dmarc":"fail"},
 "attachments":[{"index":1,"filename":"a.pdf","mime":"application/pdf","size":1234},{"index":2}],
 "text":"--- BEGIN EXTERNAL EMAIL ---\nbody é\n--- END EXTERNAL EMAIL ---","html":"<p>x</p>"}}"#;

const OUTBOX: &str = r#"{"ok":true,"outbox":[
 {"id":"out_1","status":"queued","createdAt":1600000000,"to":["a@x.example","b@x.example"],"subject":"S",
  "bcc":["c@x.example"],"policyBcc":["audit@acme.example"],"replyTo":["r@x.example"],
  "forwardOf":{"mailbox":"bot@acme.example","sender":"evil\u001b[2J@x.example","asEml":true},
  "attachments":{"count":2,"names":["a.txt","b.txt"]},"note":"not now"},
 {"id":"out_2","to":[],"createdAt":"bad"}]}"#;

const OUTBOUND: &str = r#"{"ok":true,"outbound":{"id":"out_1","status":"approved","statusNote":"sent on","from":"bot@acme.example",
 "to":["a@x.example"],"cc":["c@x.example"],"bcc":["d@x.example"],"subject":"S","attachments":{"count":1,"names":["a.txt"]},
 "createdAt":1600000000,"decidedAt":1600000100,"decidedBy":"owner","sentAt":1600000200,"note":"ok"}}"#;

const DRAFTS: &str = r#"{"ok":true,"drafts":[
 {"id":"d1","status":"sent","inbox":"me@globex.example","replyOf":"m1","editedBy":"Ann","stateBy":"Ann","outboundId":"out_9"},
 {"id":"d2","inbox":"me@globex.example","replyOf":"m2","stateBy":"Bob"},
 {"id":"d3","status":"held","inbox":null,"replyOf":"m3"},
 {"id":"d4","status":"waiting","inbox":"me@globex.example","kind":"compose","replyOf":null},
 {"id":"d5","inbox":"me@globex.example","kind":"reply","replyOf":"m5"},
 {"id":"d6","kind":["compose"]}]}"#;

/// The agent view of linked inboxes that need a person or are paused.
const AGENT_RELINK: &str = r#"{"ok":true,"inboxes":[
 {"address":"a@gmail.example","source":"linked","yourLevel":"draft","yourSendState":"no_send_level","maxLevel":"draft","status":"connected","needsRelink":true,"relinkReason":"credential_refused"},
 {"address":"b@gmail.example","source":"linked","yourLevel":"read","maxLevel":"draft","status":"connected","needsRelink":true,"relinkReason":"mail_access_not_granted"},
 {"address":"c@gmail.example","source":"linked","yourLevel":"read","maxLevel":"draft","status":"connected","relinkReason":"throttled"},
 {"address":"d@acme.example","source":"hosted","yourLevel":"send","yourSendState":"on","maxLevel":"send","status":"active","needsRelink":false}]}"#;

const CAPS: &str = r#"{"ok":true,"inboxes":[
 {"address":"me@globex.example","yourLevel":"draft","capabilities":{"source":"linked",
  "mail":{"read":true,"draft":true,"send":false},"calendar":{"read":true,"freebusy":"busy-only"},
  "account":{"kind":"microsoft","tenant":"t1","checked":false},
  "missing":[{"cap":"send","reason":"no smtp","fix":"link smtp"}]}},
 {"address":"bot@acme.example","capabilities":{"source":"hosted","mail":{"read":true,"send":true},"account":{"checked":true}}}]}"#;

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

#[test]
fn mail_reads_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        usage_case("inboxes, an extra word", &["inboxes", "x"]),
        usage_case("list, a flag", &["list", "--all"]),
        usage_case("addresses, an extra word", &["addresses", "--json", "y"]),
        usage_case("domains, an extra word", &["domains", "z"]),
        usage_case("messages, an unknown flag", &["messages", "--nope"]),
        usage_case("messages, two addresses", &["messages", "a@x", "b@x"]),
        usage_case("messages, a limit of 0", &["messages", "--limit", "0"]),
        usage_case("messages, a limit of 201", &["messages", "--limit", "201"]),
        usage_case(
            "messages, a limit not a number",
            &["messages", "--limit", "5x"],
        ),
        usage_case(
            "messages, a value flag at the end",
            &["messages", "--query"],
        ),
        usage_case(
            "messages, an offset not a number",
            &["messages", "--offset", "-1"],
        ),
        usage_case(
            "messages, junk and a folder",
            &["messages", "--junk", "--folder", "X"],
        ),
        usage_case("read, no id", &["read"]),
        usage_case("read, html and raw", &["read", "m1", "--html", "--raw"]),
        usage_case("read, an unknown flag", &["read", "m1", "-x"]),
        usage_case("read, two ids", &["read", "m1", "m2"]),
        usage_case("attachments, no id", &["attachments", "--json"]),
        usage_case(
            "attachments, get with no out",
            &["attachments", "m1", "--get", "1"],
        ),
        usage_case(
            "attachments, out with no get",
            &["attachments", "m1", "--out", "f"],
        ),
        usage_case(
            "attachments, get 0",
            &["attachments", "m1", "--get", "0", "--out", "f"],
        ),
        usage_case(
            "attachments, a value flag at the end",
            &["attachments", "m1", "--out"],
        ),
        usage_case("wait, a timeout of 901", &["wait", "--timeout", "901"]),
        usage_case("wait, a word", &["wait", "now"]),
        usage_case("wait, a value flag at the end", &["wait", "--to"]),
        usage_case("wait, an unknown flag", &["wait", "--nope"]),
        usage_case("drafts, a word", &["drafts", "x"]),
        usage_case("drafts, a limit at the end", &["drafts", "--limit"]),
        usage_case("outbox, a limit of 101", &["outbox", "--limit", "101"]),
        usage_case("outbox, two ids", &["outbox", "a", "b"]),
        usage_case("outbox cancel, no id", &["outbox", "cancel"]),
        usage_case("outbox cancel, two ids", &["outbox", "cancel", "a", "b"]),
        usage_case("outbox cancel, a flag", &["outbox", "cancel", "a", "--now"]),
        usage_case("capabilities, two addresses", &["capabilities", "a", "b"]),
        usage_case("folder, an unknown sub-command", &["folder", "move"]),
        usage_case("folder list, no address", &["folder", "list"]),
        usage_case("folder create, no name", &["folder", "create", "a@x"]),
        usage_case("folder rename, no to", &["folder", "rename", "a@x", "Old"]),
        usage_case(
            "folder rename, four words",
            &["folder", "rename", "a", "b", "c", "d"],
        ),
        usage_case("folder, a flag", &["folder", "list", "a@x", "--all"]),
        case(
            "folder alone: the help",
            &["folder"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_reads_print_as_the_shell_does() {
    let rig = Rig::new(VERB, "print");
    let many = MESSAGES;
    let v = vec![
        // covers: mail inboxes
        ok_case("inboxes, the agent view", &["inboxes"], AGENT_INBOXES),
        ok_case("list, sending off", &["list"], AGENT_OFF),
        ok_case(
            "addresses, the owner's catalog",
            &["addresses"],
            OWNER_INBOXES,
        ),
        ok_case("inboxes, none", &["inboxes"], r#"{"ok":true,"inboxes":[]}"#),
        ok_case("inboxes, the re-link words", &["inboxes"], AGENT_RELINK),
        // covers: mail inboxes --json
        ok_case("inboxes --json", &["inboxes", "--json"], AGENT_INBOXES),
        // covers: mail domains
        ok_case("domains", &["domains"], DOMAINS),
        ok_case(
            "domains, only pending",
            &["domains"],
            r#"{"ok":true,"domains":[],"pending":["a.example",{"domain":"b.example"},{"x":1}]}"#,
        ),
        ok_case("domains, none", &["domains"], r#"{"ok":true}"#),
        // covers: mail domains --json
        ok_case("domains --json", &["domains", "--json"], DOMAINS),
        // covers: mail messages
        ok_case("messages", &["messages"], many),
        ok_case(
            "messages, every filter",
            &[
                "messages",
                "a@x.example",
                "--unread",
                "--limit",
                "25",
                "--offset",
                "5",
                "--since",
                "7d",
                "--before",
                "2026-01-01",
                "--folder",
                "Sent Items",
                "--query",
                "a b&c",
                "--from",
                "ann",
            ],
            many,
        ),
        // covers: mail messages --json
        ok_case("messages, junk", &["messages", "--junk", "--json"], many),
        ok_case(
            "messages, none with a filter",
            &["messages", "--from", "nobody"],
            r#"{"ok":true,"messages":[],"searchWindow":{"hint":"only 30 days"}}"#,
        ),
        ok_case(
            "messages, none",
            &["messages"],
            r#"{"ok":true,"messages":[]}"#,
        ),
        // covers: mail read
        ok_case("read", &["read", "m1"], MESSAGE),
        ok_case("read --html", &["read", "m1", "--html"], MESSAGE),
        // covers: mail read --json
        ok_case("read --json", &["read", "m1", "--json"], MESSAGE),
        ok_case(
            "read --raw",
            &["read", "m1", "--raw"],
            r#"{"ok":true,"rawBase64":"RnJvbTogYUB4DQoNCmJvZHkg/w=="}"#,
        ),
        compare(
            Compare::Traceback,
            case(
                "read --raw, bad padding",
                &["read", "m1", "--raw"],
                &[],
                vec![ok(r#"{"ok":true,"rawBase64":"QUJD\nRA"}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        ok_case("read, no message", &["read", "m1"], r#"{"ok":true}"#),
        // covers: mail attachments
        ok_case(
            "attachments",
            &["attachments", "m1"],
            r#"{"ok":true,"id":"m1","attachments":[{"index":1,"filename":"a.pdf","mime":"application/pdf","size":10},{}]}"#,
        ),
        // covers: mail attachments --json
        ok_case(
            "attachments --json",
            &["attachments", "m1", "--json"],
            r#"{"ok":true,"id":"m1","attachments":[]}"#,
        ),
        // covers: mail wait
        // covers: mail wait --json
        ok_case("wait --json", &["wait", "--json"], MESSAGE),
        ok_case(
            "attachments, none",
            &["attachments", "m1"],
            r#"{"ok":true,"id":"m1","attachments":[]}"#,
        ),
        ok_case(
            "attachments --get",
            &["attachments", "m1", "--get", "2", "--out", "dl/"],
            r#"{"ok":true,"index":2,"filename":"b.txt","mime":"text/plain","bytes":3,"wrote":"dl/b.txt"}"#,
        ),
        ok_case(
            "wait",
            &[
                "wait",
                "--to",
                "bot@acme.example",
                "--from",
                "ann",
                "--subject",
                "Hi",
                "--timeout",
                "5",
            ],
            MESSAGE,
        ),
        case(
            "wait, timed out",
            &["wait", "--timeout", "1"],
            &[],
            vec![ok(r#"{"ok":true,"timedOut":true}"#)],
            owner_and_port,
            2,
            1,
        ),
        ok_case("drafts", &["drafts", "--limit", "3"], DRAFTS),
        ok_case("drafts, none", &["drafts"], r#"{"ok":true,"drafts":[]}"#),
        ok_case("drafts --json", &["drafts", "--json"], DRAFTS),
        // covers: mail outbox
        ok_case("outbox", &["outbox", "--limit", "20"], OUTBOX),
        ok_case("outbox, one", &["outbox", "out_1"], OUTBOUND),
        // covers: mail outbox --json
        ok_case(
            "outbox --id",
            &["outbox", "--id", "out_1", "--json"],
            OUTBOUND,
        ),
        ok_case("outbox, empty", &["outbox"], r#"{"ok":true,"outbox":[]}"#),
        ok_case(
            "outbox cancel",
            &["outbox", "cancel", "out_1"],
            r#"{"ok":true,"id":"out_1"}"#,
        ),
        ok_case(
            "outbox cancel, the hint",
            &["outbox", "cancel", "out_1", "--json"],
            r#"{"ok":true,"hint":"cancelled it"}"#,
        ),
        // covers: mail capabilities
        ok_case("capabilities", &["capabilities"], CAPS),
        ok_case(
            "capabilities of one",
            &["capabilities", "bot@acme.example"],
            r#"{"ok":true,"address":"bot@acme.example","yourLevel":"send","capabilities":{"source":"hosted","mail":{"read":true}}}"#,
        ),
        ok_case(
            "capabilities, none",
            &["capabilities"],
            r#"{"ok":true,"inboxes":[]}"#,
        ),
        case(
            "capabilities, the owner view asked again",
            &["capabilities"],
            &[],
            vec![
                answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no agent/workspace named proj"}}"#,
                ),
                ok(CAPS),
            ],
            owner_and_port,
            0,
            2,
        ),
        // covers: mail capabilities --json
        case(
            "capabilities, an error",
            &["capabilities", "--json"],
            &[],
            vec![answer(403, r#"{"error":{"code":"owner_only","hint":""}}"#)],
            owner_and_port,
            3,
            1,
        ),
        // covers: mail folder
        ok_case(
            "folder list",
            &["folder", "list", "a@x.example"],
            r#"{"ok":true,"folders":["INBOX","Sent",3]}"#,
        ),
        ok_case(
            "folder list, none",
            &["folder", "list", "a@x.example"],
            r#"{"ok":true,"folders":[]}"#,
        ),
        ok_case(
            "folder create",
            &["folder", "create", "a@x.example", "Work"],
            r#"{"ok":true,"address":"a@x.example"}"#,
        ),
        // covers: mail folder --json
        ok_case(
            "folder rename",
            &["folder", "rename", "a@x.example", "Work", "Jobs", "--json"],
            r#"{"ok":true,"from":"Work","to":"Jobs","address":"a@x.example"}"#,
        ),
        ok_case(
            "folder rename, printed",
            &["folder", "rename", "", "a@x.example", "Work", "Jobs"],
            r#"{"ok":true,"from":"Work","to":"Jobs","address":"a@x.example"}"#,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn mail_reads_read_the_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answers");
    let traceback = |c: Case| compare(Compare::Traceback, c);
    let v = vec![
        case(
            "owner only, with a permission card",
            &["inboxes"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "forbidden, the teaching hint",
            &["messages"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"not yours"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a missing capability",
            &["read", "m1"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"capability_missing","hint":"h","cap":"mail.read","reason":"calendar only","fix":"relink","x":1},"permission":{"requestable":false,"hint":"Ask the owner."}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the daemon's usage error",
            &["messages", "--since", "x"],
            &[],
            vec![answer(
                400,
                r#"{"error":{"code":"usage","hint":"bad --since"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "too large",
            &["attachments", "m1", "--get", "1", "--out", "f"],
            &[],
            vec![answer(
                413,
                r#"{"error":{"code":"attachment_too_large","hint":"over 100 MB"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "inbox_throttled, final",
            &["attachments", "m1"],
            &[],
            vec![answer(
                503,
                r#"{"ok":false,"error":{"code":"inbox_throttled","hint":"the provider is limiting this account","retry_after_s":300}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "inbox_busy, final",
            &["messages", "me@globex.example"],
            &[],
            vec![answer(
                429,
                r#"{"ok":false,"error":{"code":"inbox_busy","hint":"4 sessions open","retry_after_s":5.5}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a failed fetch says whether to retry",
            &["attachments", "m1"],
            &[],
            vec![answer(
                502,
                r#"{"ok":false,"error":{"code":"engine","hint":"FETCH failed","retry":"after_relink","retry_after_s":null}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error string",
            &["drafts"],
            &[],
            vec![ok(r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not ok",
            &["outbox"],
            &[],
            vec![ok(r#"{"ok":false,"why":"é"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an HTTP error that is not JSON",
            &["domains"],
            &[],
            vec![answer(500, "oops")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 2xx that is not JSON",
            &["inboxes"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 2xx",
            &["wait"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "null",
            &["read", "m1"],
            &[],
            vec![ok("null")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a list",
            &["inboxes"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "messages as a number",
            &["messages"],
            &[],
            vec![ok(r#"{"ok":true,"messages":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an address that is a number",
            &["inboxes"],
            &[],
            vec![ok(
                r#"{"ok":true,"inboxes":[{"address":7,"yourLevel":"read"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an error that is a list",
            &["drafts"],
            &[],
            vec![ok(r#"{"error":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "migrating, then the answer",
            &["inboxes"],
            &[],
            vec![
                answer(503, r#"{"ok":false,"state":"migrating"}"#),
                ok(AGENT_INBOXES),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "no answer in time",
            &["messages"],
            &[("K5_MAIL_HTTP_TIMEOUT", "1")],
            vec![Reply::Hang(Duration::from_secs(3))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "attachments --get, no answer in time",
            &["attachments", "m1", "--get", "1", "--out", "f"],
            &[("K2_MAIL_HTTP_TIMEOUT", "1.5")],
            vec![Reply::Hang(Duration::from_secs(4))],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a timeout setting that is not a number",
            &["inboxes"],
            &[("K5_MAIL_HTTP_TIMEOUT", "soon")],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        // No answer.
        case(
            "nothing listening",
            &["inboxes"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "closed, a GET tries once more",
            &["messages"],
            &[],
            vec![Reply::Close, Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "closed, then the answer",
            &["drafts"],
            &[],
            vec![Reply::Close, ok(DRAFTS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "closed, a POST never again",
            &["folder", "create", "a@x.example", "W"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["read", "m1"],
            &[],
            vec![Reply::Reset, Reply::Reset],
            owner_and_port,
            1,
            2,
        ),
        case(
            "not HTTP",
            &["outbox"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        // Who it is.
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["inboxes"],
                &[],
                vec![ok(AGENT_INBOXES)],
                nothing,
                0,
                1,
            )
        },
        case(
            "the id tool in a session with no passport",
            &["messages"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "no port: the gate",
            &["inboxes"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent (the front hands a help flag to the shell).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    const LINES: &[&[&str]] = &[
        &["attachments", "m1", "--get", "1", "--out", "f", "-h"],
        &["attachments", "--help", "m1", "--get", "1", "--out", "f"],
        &["outbox", "cancel", "out_1", "--help"],
        &["outbox", "-h", "cancel", "out_1"],
        &["folder", "create", "a@x.example", "W", "-h"],
        &["folder", "rename", "--help", "a@x.example", "W", "J"],
        &["folder", "-h"],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}

/// The one large answer: 3,000 messages (above 256 KiB). `_mail_py` reads
/// it from urllib itself, so both print every row.
#[test]
fn a_large_list() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"id":"m{n}","from":{{"name":"Sender {n}","address":"sender{n}@example.com"}},"subject":"Message number {n} about the quarterly plan","date":"2020-03-{:02}T10:00:00Z","unread":{}}}"#,
                1 + n % 28,
                n % 2 == 0
            )
        })
        .collect();
    let body = format!(
        r#"{{"ok":true,"messages":[{}],"nextOffset":3000}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    let c = case(
        "a large list",
        &["messages", "--limit", "200"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    );
    rig.check(&c);
}

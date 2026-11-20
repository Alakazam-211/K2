//! `k5-cli organization` against the shell CLI's `cli/k2 organization`,
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code for every verb.
//!
//! - The verbs: `bulletin` (alone, `--peek`, `read`, `show`, `set` with
//!   words, with `--file` and with both), `status`, `list`, `show`,
//!   `mine`, `night`, `chart`, `reach` (sent and held), `updates`, `ask`,
//!   `update`, `create`, `rename`, `purpose`, `delete`, `add` (and no
//!   change), `move`, `remove`, `head`, `link`, `unlink`; `--json`; many
//!   orgs (an answer above 256 KiB).
//! - The words: each flag with a value and with `=`, `--`, a flag with no
//!   value, an unknown flag, each verb's missing words, a bad link kind,
//!   the position verbs, an unknown verb (`--json` first among them).
//! - The answers: a refusal with a permission card (the closing line), a
//!   401, a 404 with a message, a 5xx, not JSON, empty, not an object, the
//!   bulletin's mark (the line after a command that worked), Python's
//!   raise part way.
//! - The owner from disk, a passport over TCP, a live cell socket (a GET
//!   and a POST; not served goes on to TCP), the id tool's checks, a
//!   re-minted and a healed passport; the gates.
//! - No answer (an empty reply, a reset, not HTTP, nothing listening):
//!   the line, exit 1.
//! - What the binary hands to the shell: `organization` alone, help, a
//!   port that is no port, a file it can't read.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "organization";

const ORG: &str = r#"{"name":"Ops","head":{"handle":"ann"},"purpose":"Keep it up","members":[{"handle":"ann","isHead":true,"role":"lead"},{"handle":"bob","role":""}],"bulletin":{"lines":["one","two é"]},"lines":[{"kind":"below","a":{"name":"Ops"},"b":{"name":"Dev"}},{"kind":"beside","a":{"name":"Ops"},"b":{"name":"QA"}}]}"#;

fn list() -> String {
    format!(r#"{{"orgs":[{ORG},{{"name":"Dev","head":null,"members":[]}}]}}"#)
}

fn org() -> String {
    format!(r#"{{"org":{ORG}}}"#)
}

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"name":"org-{i}-{}","head":{{"handle":"h{i}"}},"members":[{{"handle":"a{i}","isHead":true}},{{"handle":"b{i}","role":"r"}}],"bulletin":{{"lines":["line {i}"]}}}}"#,
                "é".repeat(i % 30)
            )
        })
        .collect();
    format!(r#"{{"orgs":[{}]}}"#, rows.join(","))
}

const NIGHT: &str = r#"{"tonight":true,"night":"2026-11-18","zone":"UTC","runAt":"01:00","spreadEndsAt":"03:00","lastCallAt":"04:00","dayStartsAt":"06:00",
  "governor":{"slots":2,"holding":"low memory","machine":{"cores":8,"availableBytes":5368709120,"load1":1.75}},
  "counts":{"queued":1,"running":0,"done":3,"skipped":1,"failed":2},
  "orgs":[{"due":"01:00","org":{"name":"A"},"head":{"handle":"h"},"state":"done","wrote":true},
          {"due":"01:10","org":{"name":"B"},"head":{"handle":"i"},"state":"skipped"}]}"#;

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn with_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("lines.txt"), "first\nsecond é\n\n").unwrap();
}

fn with_unreadable_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("bytes.txt"), b"a\xffb").unwrap();
}

#[test]
fn organization_reads_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    let v = vec![
        // covers: organization list
        case(
            "list",
            &["list"],
            &[],
            vec![reply(list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, none",
            &["list"],
            &[],
            vec![ok(r#"{"orgs":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: organization list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![reply(list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show",
            &["show", "Ops", "x"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, an escaped name",
            &["show", "a&b é"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, in none",
            &["mine"],
            &[],
            vec![ok(r#"{"org":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, the next step",
            &["mine"],
            &[],
            vec![ok(r#"{"next":"Ask your head."}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, the head",
            &["mine"],
            &[],
            vec![reply(format!(r#"{{"org":{ORG},"isHead":true}}"#))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, connected",
            &["mine"],
            &[],
            vec![reply(format!(
                r#"{{"org":{ORG},"head":{{"handle":"ann"}},"connectedToHead":true}}"#
            ))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, not connected",
            &["mine"],
            &[],
            vec![reply(format!(
                r#"{{"org":{ORG},"head":{{"handle":"ann"}}}}"#
            ))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "mine, no head",
            &["mine"],
            &[],
            vec![ok(r#"{"org":{"name":"Dev"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "night",
            &["night"],
            &[],
            vec![ok(NIGHT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "night, odd numbers",
            &["night"],
            &[],
            vec![ok(
                r#"{"governor":{"machine":{"availableBytes":true,"load1":0.25,"cores":null}},"counts":{"failed":0}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chart: always JSON",
            &["chart"],
            &[],
            vec![ok(r#"{"nodes":[{"id":"é"}],"edges":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status, unread",
            &["status"],
            &[],
            vec![ok(r#"{"day":"2026-11-18","unread":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status, read",
            &["status"],
            &[],
            vec![ok(r#"{"day":"2026-11-18"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: organization bulletin
        case(
            "bulletin",
            &["bulletin"],
            &[],
            vec![ok(r#"{"text":"Today: ship it.\n"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bulletin --peek",
            &["bulletin", "--peek"],
            &[],
            vec![ok(r#"{"text":"x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: organization bulletin --json
        case(
            "bulletin --json",
            &["bulletin", "--json"],
            &[],
            vec![ok(r#"{"text":"x","n":1.0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bulletin show",
            &["bulletin", "show", "Ops"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bulletin show --org",
            &["bulletin", "show", "--org=Dev"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "updates",
            &["updates"],
            &[],
            vec![ok(r#"{"updates":[{"from":{"handle":"a"},"body":"b"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "updates --org --since, none",
            &["updates", "--org", "O p", "--since", "17"],
            &[],
            vec![ok(r#"{"updates":[],"org":{"name":"O p"}}"#)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn organization_writes_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let v = vec![
        case(
            "bulletin read",
            &["bulletin", "read"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bulletin set",
            &["bulletin", "set", "one", "two é", "--org", "Ops"],
            &[],
            vec![ok(r#"{"org":{"name":"Ops"},"lines":["one","two é"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "bulletin set --file",
            &["bulletin", "set", "--file", "lines.txt", "third"],
            &[],
            vec![ok(r#"{"org":{"name":"Ops"},"lines":["x"]}"#)],
            with_file,
            0,
            1,
        ),
        // covers: organization reach
        case(
            "reach",
            &["reach", "Ops", "--why", "need a hand"],
            &[],
            vec![ok(r#"{"text":"Sent to ann.","status":"sent"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "reach, held",
            &["reach", "Ops", "--why=x"],
            &[],
            vec![ok(r#"{"text":"A card is up.","status":"card"}"#)],
            owner_and_port,
            2,
            1,
        ),
        // covers: organization reach --json
        case(
            "reach --json, held",
            &["reach", "Ops", "--why", "x", "--json"],
            &[],
            vec![ok(r#"{"status":"ticket"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "ask",
            &["ask", "how", "goes?", "--member", "bob"],
            &[],
            vec![ok(
                r#"{"asked":["bob"],"queued":["cy"],"spacing":"every 5 min","notDelivered":[{"member":"dee","why":"asleep"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ask, nobody",
            &["ask", "q"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "update, delivered",
            &["update", "shipped", "--org", "Ops"],
            &[],
            vec![ok(r#"{"deliveredNow":true,"head":{"handle":"ann"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "update, stored",
            &["update", "shipped"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: organization create
        case(
            "create",
            &["create", "Big", "Team", "--purpose", "Do it"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rename",
            &["rename", "Ops", "New", "Ops"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "purpose",
            &["purpose", "Ops", "a", "b"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "purpose --purpose",
            &["purpose", "Ops", "a", "--purpose", "p"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "delete",
            &["delete", "Ops"],
            &[],
            vec![ok(r#"{"deleted":{"name":"Ops"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add",
            &["add", "Ops", "bob"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add, no change",
            &["add", "Ops", "bob"],
            &[],
            vec![reply(format!(r#"{{"changed":false,"org":{ORG}}}"#))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "move",
            &["move", "bob", "Dev"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove",
            &["remove", "Ops", "bob"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "head",
            &["head", "Ops", "none"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link above",
            &["link", "Ops", "above", "Dev"],
            &[],
            vec![ok(
                r#"{"line":{"kind":"below","a":{"name":"Ops"},"b":{"name":"Dev"}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link below",
            &["link", "Ops", "below", "Dev"],
            &[],
            vec![ok(r#"{"line":{"kind":"under"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link beside",
            &["link", "Ops", "beside", "QA"],
            &[],
            vec![ok(
                r#"{"line":{"kind":"beside","a":{"name":"Ops"},"b":{"name":"QA"}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unlink",
            &["unlink", "Ops", "Dev"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: organization create --json
        case(
            "create --json",
            &["create", "X", "--json"],
            &[],
            vec![reply(org())],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn organization_stops_as_the_shell_does() {
    let rig = Rig::new(VERB, "stops");
    let stop = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], owner_and_port, 2, 0)
    };
    let v = vec![
        stop("--file with no path", &["bulletin", "set", "--file"]),
        stop("--why with no line", &["reach", "Ops", "--why"]),
        stop("--purpose with no text", &["create", "--purpose"]),
        stop("--member with no agent", &["ask", "q", "--member"]),
        stop("--org with no org", &["list", "--org"]),
        stop("--since with no time", &["updates", "--since"]),
        stop("--since= is unknown", &["updates", "--since=5"]),
        stop("an unknown flag", &["list", "--bogus"]),
        stop("an unknown flag before a bad verb", &["nope", "--x"]),
        stop("bulletin show, no org", &["bulletin", "show"]),
        stop("bulletin set, no lines", &["bulletin", "set"]),
        stop(
            "bulletin set, no file",
            &["bulletin", "set", "--file", "none.txt", "x"],
        ),
        stop("bulletin, a bad word", &["bulletin", "write"]),
        stop("show, no org", &["show"]),
        stop("reach, no org", &["reach"]),
        stop("reach, no why", &["reach", "Ops"]),
        stop("ask, no question", &["ask"]),
        stop("update, no news", &["update", "--org", "x"]),
        stop("create, no name", &["create"]),
        stop("rename, one word", &["rename", "Ops"]),
        stop("purpose, nothing", &["purpose"]),
        stop("delete, two words", &["delete", "a", "b"]),
        stop("add, one word", &["add", "Ops"]),
        stop("move, three words", &["move", "a", "b", "c"]),
        stop("remove, none", &["remove"]),
        stop("head, one", &["head", "Ops"]),
        stop("link, two", &["link", "a", "b"]),
        stop("link, a bad kind", &["link", "a", "over", "b"]),
        stop("unlink, one", &["unlink", "a"]),
        stop("position", &["position", "x"]),
        stop("arrange", &["arrange"]),
        stop("an unknown verb", &["nope"]),
        stop("--json first is the verb", &["--json", "list"]),
        stop("-x first is the verb", &["-x"]),
        stop("-- first is the verb", &["--", "link"]),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn organization_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answers");
    let traceback = |c: Case| Case {
        compare: Compare::Traceback,
        ..c
    };
    let v = vec![
        case(
            "a refusal with a card",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 401",
            &["delete", "Ops"],
            &[],
            vec![Reply::Answer(401, r#"{"error":{"message":"who?"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 404 with a message",
            &["show", "Nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"not_found","message":"No org Nope."}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a 500",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"oops":1}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx with no body",
            &["add", "a", "b"],
            &[],
            vec![Reply::Answer(503, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("<html>nope</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long",
            &["list"],
            &[],
            vec![reply("y".repeat(500))],
            owner_and_port,
            1,
            1,
        ),
        case("empty", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        case(
            "not an object, refused",
            &["list"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "the bulletin's mark: the line",
            &["list"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin's mark, --json",
            &["list", "--json"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "Python raises part way",
            &["list"],
            &[],
            vec![ok(r#"{"orgs":[{"name":"A","members":[{"handle":1}]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "Python raises on a text that is no string",
            &["bulletin"],
            &[],
            vec![ok(r#"{"text":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "night with a governor that is a list",
            &["night"],
            &[],
            vec![ok(r#"{"governor":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "many orgs",
            &["list"],
            &[],
            vec![reply(many(2500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many orgs, --json",
            &["list", "--json"],
            &[],
            vec![reply(many(2500))],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn organization_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "the passport over TCP",
                &["list"],
                &[],
                vec![reply(list())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport",
                &["mine"],
                &[],
                vec![ok("{}")],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["list"],
                &[],
                vec![heal(&rig), reply(list())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["list"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(),
            case(
                "the cell socket, a GET",
                &["list"],
                &[],
                vec![reply(list())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket, a POST",
                &["add", "Ops", "bob"],
                &[],
                vec![reply(org())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served: TCP",
                &["create", "X"],
                &[],
                vec![not_served(), reply(org())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "a card on the cell socket",
                &["list"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "an empty answer there is the answer",
                &["list"],
                &[],
                vec![ok("")],
                port_only,
                1,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn organization_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let port = dead_port();
    let v = vec![
        case(
            "an empty reply",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["add", "a", "b"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["list"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["list", "--bogus"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["list"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
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

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn organization_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "organization alone",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case("help", &["help"], &[], vec![], owner_and_port, 0, 0)),
        shell(case(
            "list -h",
            &["list", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            1,
            0,
        )),
        shell(case(
            "a file with bytes that are not UTF-8",
            &["bulletin", "set", "--file", "bytes.txt"],
            &[],
            vec![ok(r#"{"org":{"name":"O"}}"#)],
            with_unreadable_file,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

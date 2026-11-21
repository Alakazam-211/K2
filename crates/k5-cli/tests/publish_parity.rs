//! `k5-cli publish` against the shell CLI's `cli/k2 publish`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - `ps` (`list`, `ls`; text, `--long` and `--json`), `logs`: the table and
//!   its lines (legacy environments, moved labels, reasons, warnings, dev
//!   servers), none, odd and broken answers, Python's raise part way.
//! - `run`, `start`, `stop`, `rm`, `frame`: what each prints (on stdout and
//!   stderr), held for the owner, the daemon's refusals and their exits,
//!   every usage line.
//! - `visibility` (the read, each mode, the PIN on stdin over TCP,
//!   `needs_yes` with no terminal), `open --no-browser`, `sessions` and
//!   `--revoke-all`.
//! - `subdomain claim` and `unclaim`.
//! - `publish` alone, `help`, an unknown verb (the other verbs: tests/publish_cp_parity.rs).
//! - The owner from disk, the cell socket, no answer, the gates.
//! - 0.46.17: `run --assign` (one login, a comma list, `--assign=`, the
//!   value stripped, an empty `--assign=` left out of the body), and its
//!   usage line (no value, an empty one; `--json` seen before it).
//! - Help never acts; one large list (above 256 KiB).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "publish";

const LIST: &str = r#"{"services":[{"name":"web","status":"running","port":3000,"url":"https://web.acme.k2.dev","visibility":{"mode":"private"},"framePolicy":"self","frameAncestors":"https://a.example https://b.example","bindWarning":"listens on 0.0.0.0"},{"name":"api","status":"starting","port":3047,"startingLeftMs":252000,"target":"localhost:3047","cmd":"(app)","envMode":"legacy","envRemovedIn":"0.47.0","framePolicy":"x","frameAncestors":"none"},{"name":"old","desired":"stopped","legacyLabel":true,"labelMovedTo":"crm","reason":"exit 1: boom","visibility":{"mode":"public","enforced":false},"reservedLabel":true,"frameWarning":"frames off"},{"name":"dev1","status":"running","port":"","visibility":{"mode":"public","dev":true,"reason":"asked"},"dev":{"lane":"box","caps":"2 CPUs","idleSecs":1800,"warning":"hot"},"envDeprecation":true,"framePolicy":"p","frameAncestors":""},"skip",{"name":"bare"}]}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

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

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

/// The owner from disk and the stub's port, `answers`, `code`, the
/// number of requests.
fn on(
    name: &'static str,
    args: &'static [&'static str],
    answers: Vec<Reply>,
    code: i32,
    n: usize,
) -> Case {
    case(name, args, &[], answers, owner_and_port, code, n)
}

fn site_dir(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("site")).unwrap();
}

fn check_all(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

#[test]
fn ps_and_logs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    check_all(
        &rig,
        vec![
            // covers: publish ps
            on("ps", &["ps"], vec![ok(LIST)], 0, 1),
            // covers: publish ps --json
            on("ps --json", &["ps", "--json"], vec![ok(LIST)], 0, 1),
            on("ps --long", &["ps", "--long"], vec![ok(LIST)], 0, 1),
            on("list -l", &["list", "-l"], vec![ok(LIST)], 0, 1),
            on("ls", &["ls"], vec![ok(r#"{"services":[]}"#)], 0, 1),
            on(
                "services not a list",
                &["ps"],
                vec![ok(r#"{"services":{"a":1}}"#)],
                0,
                1,
            ),
            on(
                "one legacy service, the end now",
                &["ps"],
                vec![ok(
                    r#"{"services":[{"name":"a","envMode":"legacy","envRemovedIn":"0.46.17","cutoverNow":true,"legacyLabel":1,"legacyHint":"move it"}]}"#,
                )],
                0,
                1,
            ),
            on(
                "not JSON",
                &["ps"],
                vec![ok("Upgrade to Pro: https://k2.dev")],
                1,
                1,
            ),
            on("an empty answer", &["ps"], vec![ok("")], 1, 1),
            on("a JSON list", &["ps"], vec![ok("[1,2]")], 1, 1),
            on(
                "a JSON list, --json",
                &["ps", "--json"],
                vec![ok("[1,\"\u{e9}\"]")],
                0,
                1,
            ),
            on(
                "a refusal",
                &["ps"],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
                )],
                1,
                1,
            ),
            on(
                "a refusal, --json",
                &["ps", "--json"],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","message":"mé"}}"#.into(),
                )],
                1,
                1,
            ),
            on(
                "a word error with a hint",
                &["ps"],
                vec![ok(r#"{"error":"not_pro","hint":"Error: needs Pro\n"}"#)],
                1,
                1,
            ),
            on(
                "an error with a code",
                &["ps"],
                vec![ok(
                    r#"{"error":"label taken","code":"label_owned_by_other_workspace","hint":"ask"}"#,
                )],
                1,
                1,
            ),
            on(
                "pro_required with a message",
                &["ps", "--json"],
                vec![ok(r#"{"error":"pro_required","message":"Pro only"}"#)],
                1,
                1,
            ),
            on("error 0", &["ps"], vec![ok(r#"{"error":0}"#)], 1, 1),
            on(
                "error false",
                &["ps"],
                vec![ok(r#"{"error":false,"services":[]}"#)],
                0,
                1,
            ),
            on(
                "success false",
                &["ps"],
                vec![ok(r#"{"success":false}"#)],
                1,
                1,
            ),
            traceback(on(
                "a url that is no string",
                &["ps"],
                vec![ok(r#"{"services":[{"name":"a"},{"name":"b","url":5}]}"#)],
                1,
                1,
            )),
            traceback(on(
                "an idle that is no number",
                &["ps", "--long"],
                vec![ok(
                    r#"{"services":[{"name":"a","dev":{"idleSecs":"soon"}}]}"#,
                )],
                1,
                1,
            )),
            on("ps: an extra word", &["ps", "x"], vec![], 2, 0),
            on(
                "ps: a flag, --json first",
                &["ps", "--json", "--nope"],
                vec![],
                2,
                0,
            ),
            // covers: publish logs
            on(
                "logs: text",
                &["logs", "web"],
                vec![ok("line 1\nline 2\n")],
                0,
                1,
            ),
            on(
                "logs --tail: a log",
                &["logs", "web", "--tail"],
                vec![ok(r#"{"log":"a\nb"}"#)],
                0,
                1,
            ),
            on(
                "logs: lines",
                &["logs", "w e"],
                vec![ok(r#"{"lines":["a",1,null]}"#)],
                0,
                1,
            ),
            on(
                "logs: a refusal",
                &["logs", "web"],
                vec![ok(
                    r#"{"error":{"code":"not_found","hint":"no such service"}}"#,
                )],
                1,
                1,
            ),
            on("logs: no name", &["logs"], vec![], 2, 0),
            on("logs: a flag", &["logs", "web", "--follow"], vec![], 2, 0),
            waited(on("ps: no answer", &["ps"], vec![Reply::Close], 1, 1)),
            case(
                "HOOK_SOCK set and no passport: the require-passport lines",
                &["ps"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![],
                owner_and_port,
                3,
                0,
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "the cell socket: ps",
                    &["ps"],
                    &[],
                    vec![ok(LIST)],
                    port_only,
                    0,
                    1,
                ),
            ),
        ],
    );
}

#[test]
fn run_start_stop_rm_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let started = r#"{"service":{"name":"web","status":"running","port":3000,"url":"https://web.acme.k2.dev","hostnamePending":true,"visibility":{"mode":"private"}},"visibilityNote":"new sites are private"}"#;
    let held = r#"{"status":"held","request":{"id":"req_1","summary":"run web"},"reasons":["a new --cmd service","disk"],"reused":true,"askedAgo":"3m","card":{"where":"your Thread"},"hint":"The owner decides.","permission":{"command":"k2 permission request --scope publish.run --why \"...\"","requestable":true}}"#;
    let noisy = r#"{"service":{"name":"web","status":"starting","port":3000,"startingLeftMs":61000,"dev":{"caps":"2 CPUs","warning":"hot reload"},"envMode":"legacy","visibility":{"mode":"pin"}},"devWarning":"dev servers show source","diskWarning":{"hint":"92% full"},"diskOverride":{"applied":true,"via":"flag"},"crossover":{"wouldHold":true,"hint":"ask next time"},"wall":{"path":"open","app":"web","deployment":"d1","created":true}}"#;
    check_all(
        &rig,
        vec![
            // covers: publish run
            on(
                "run",
                &["run", "web", "--cmd", "npm start", "--port", "3000"],
                vec![ok(started)],
                0,
                1,
            ),
            // covers: publish run --json
            on(
                "run --json: held",
                &["run", "web", "--cmd=x", "--port=3000", "--json"],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run: held",
                &[
                    "run",
                    "web",
                    "--cmd",
                    "x",
                    "--port",
                    "3000",
                    "--transfer",
                    "--no-tunnel",
                ],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run: the stderr lines",
                &[
                    "run",
                    "web",
                    "--cmd",
                    "x",
                    "--port",
                    "3000",
                    "--dev",
                    "--idle",
                    "30m",
                    "--mem=4G",
                    "--cpus",
                    "2",
                    "--disk-override",
                    " full ",
                    "--probe-timeout",
                    "1m30s",
                ],
                vec![ok(noisy)],
                0,
                1,
            ),
            on(
                "run: E2E off, a legacy wall",
                &[
                    "run", "web", "--cmd", "x", "--port", "00080", "--cwd", "sub/../x",
                ],
                vec![ok(
                    r#"{"service":{"name":"web","port":80,"visibility":{"mode":"public","enforced":false},"envDeprecation":1,"envRemovedIn":"0.47","error":"crashed"},"wall":{"path":"legacy"}}"#,
                )],
                0,
                1,
            ),
            case(
                "run --skin",
                &["run", "site", "--skin", "site", "--port", "8080"],
                &[],
                vec![ok(
                    r#"{"service":{"name":"site","status":"running","port":8080}}"#,
                )],
                site_dir,
                0,
                1,
            ),
            on(
                "run --skin=: no root",
                &["run", "s", "--skin=", "--port", "1", "--cwd", "~"],
                vec![],
                2,
                0,
            ),
            on(
                "run: a refusal, disk_low",
                &["run", "web", "--cmd", "x", "--port", "1"],
                vec![ok(r#"{"error":{"code":"disk_low","hint":"Disk is full"}}"#)],
                3,
                1,
            ),
            on(
                "run: skin_retired, --json",
                &["run", "web", "--skin", "--port", "1", "--json"],
                vec![ok(
                    r#"{"error":{"code":"skin_retired","hint":"use an App"}}"#,
                )],
                3,
                1,
            ),
            on("run: no port", &["run", "web", "--cmd", "x"], vec![], 2, 0),
            on("run: no cmd", &["run", "web", "--port", "1"], vec![], 2, 0),
            on(
                "run: port 0",
                &["run", "web", "--cmd", "x", "--port", "0"],
                vec![],
                2,
                0,
            ),
            on(
                "run: port 65536",
                &["run", "web", "--cmd", "x", "--port", "65536", "--json"],
                vec![],
                2,
                0,
            ),
            on(
                "run: port abc",
                &["run", "web", "--cmd", "x", "--port", "3k"],
                vec![],
                2,
                0,
            ),
            on(
                "run: a bad probe timeout",
                &[
                    "run",
                    "web",
                    "--cmd",
                    "x",
                    "--port",
                    "1",
                    "--probe-timeout",
                    "5 x",
                ],
                vec![],
                2,
                0,
            ),
            on(
                "run: a long probe timeout",
                &[
                    "run",
                    "web",
                    "--cmd",
                    "x",
                    "--port",
                    "1",
                    "--probe-timeout",
                    "16M",
                ],
                vec![],
                2,
                0,
            ),
            on(
                "run: --cmd and --skin",
                &["run", "web", "--cmd", "x", "--skin"],
                vec![],
                2,
                0,
            ),
            on(
                "run: --dev --skin",
                &["run", "web", "--dev", "--skin", "--port", "1"],
                vec![],
                2,
                0,
            ),
            on(
                "run: --dev --no-tunnel",
                &["run", "web", "--dev", "--no-tunnel", "--json"],
                vec![],
                2,
                0,
            ),
            on(
                "run: --idle without --dev",
                &["run", "web", "--idle", "5m"],
                vec![],
                2,
                0,
            ),
            on(
                "run: --cwd and --skin",
                &["run", "web", "--skin", "--cwd", "/x"],
                vec![],
                2,
                0,
            ),
            on(
                "run: an absolute skin root",
                &["run", "web", "--skin", "/srv", "--port", "1"],
                vec![],
                2,
                0,
            ),
            on(
                "run: a skin root with ..",
                &["run", "web", "--skin", "a/../b", "--port", "1"],
                vec![],
                2,
                0,
            ),
            on(
                "run: a skin root not there",
                &["run", "web", "--skin", "nope", "--port", "1"],
                vec![],
                2,
                0,
            ),
            on(
                "run: an unknown flag",
                &["run", "web", "--fast"],
                vec![],
                2,
                0,
            ),
            on(
                "run: a flag with no value",
                &["run", "web", "--cmd"],
                vec![],
                2,
                0,
            ),
            on("run: two names", &["run", "a", "b"], vec![], 2, 0),
            // covers: publish start
            on("start", &["start", "web"], vec![ok(noisy)], 0, 1),
            // covers: publish start --json
            on(
                "start --json",
                &["start", "--json", "web"],
                vec![ok(started)],
                0,
                1,
            ),
            // covers: publish stop
            on(
                "stop: legacy env",
                &["stop", "web"],
                vec![ok(
                    r#"{"service":{"name":"web","status":"stopped","envMode":"legacy","envRemovedIn":"0.47.0","cutoverNow":true}}"#,
                )],
                0,
                1,
            ),
            // covers: publish stop --json
            on(
                "stop --json",
                &["stop", "web", "--json"],
                vec![ok(r#"{"ok":true}"#)],
                0,
                1,
            ),
            on("stop: no name", &["stop"], vec![], 2, 0),
            on("stop: a flag", &["stop", "web", "--now"], vec![], 2, 0),
            // covers: publish rm
            on(
                "rm",
                &["rm", "web", "--keep-hostname"],
                vec![ok(r#"{"service":{"name":"web","envDeprecation":true}}"#)],
                0,
                1,
            ),
            on(
                "rm --json: a refusal",
                &["rm", "web", "--json"],
                vec![ok(r#"{"error":"nope"}"#)],
                1,
                1,
            ),
            on("rm: no name", &["rm"], vec![], 2, 0),
            on("rm: a flag", &["rm", "web", "--hostname"], vec![], 2, 0),
            waited(on(
                "run: no answer",
                &["run", "web", "--cmd", "x", "--port", "1"],
                vec![Reply::Close],
                1,
                1,
            )),
            waited(with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "the cell socket: stop with no answer, not sent again",
                    &["stop", "web"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    1,
                    1,
                ),
            )),
        ],
    );
}

#[test]
fn frame_open_sessions_visibility_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "site");
    let vis = r#"{"label":"web","mode":"public","dev":true,"url":"https://web.acme.k2.dev","reason":"a demo","warnings":["one","two"]}"#;
    let sessions = r#"{"label":"web","sessions":[{"kind":"owner","user":"ana","createdAt":1700000000,"expiresAt":"1700086400"},{"kind":null,"createdAt":"soon"}]}"#;
    check_all(
        &rig,
        vec![
            // covers: publish frame
            on(
                "frame --allow",
                &[
                    "frame",
                    "web",
                    "--allow",
                    "https://a.example",
                    "--allow=https://b.example",
                ],
                vec![ok(
                    r#"{"service":{"name":"web","framePolicy":"'self' https://a.example","frameAncestors":"https://a.example https://b.example"},"restarted":true}"#,
                )],
                0,
                1,
            ),
            // covers: publish frame --json
            on(
                "frame --none --json",
                &["frame", "web", "--none", "--json"],
                vec![ok(r#"{"service":{"name":"web"}}"#)],
                0,
                1,
            ),
            on(
                "frame --reset",
                &["frame", "web", "--reset"],
                vec![ok(r#"{"name":"web","framePolicy":"'self'"}"#)],
                0,
                1,
            ),
            on(
                "frame: a bad origin",
                &["frame", "web", "--allow", "x"],
                vec![ok(
                    r#"{"error":{"code":"bad_frame_origin","hint":"not an origin"}}"#,
                )],
                2,
                1,
            ),
            on(
                "frame: owner only",
                &["frame", "web", "--none"],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"owner"}}"#.into(),
                )],
                3,
                1,
            ),
            on(
                "frame: --none --reset",
                &["frame", "web", "--none", "--reset"],
                vec![],
                2,
                0,
            ),
            on(
                "frame: --allow after --none",
                &["frame", "web", "--none", "--allow", "x"],
                vec![],
                2,
                0,
            ),
            on("frame: no mode", &["frame", "web"], vec![], 2, 0),
            on(
                "frame: --allow with no value",
                &["frame", "web", "--allow"],
                vec![],
                2,
                0,
            ),
            // covers: publish visibility
            on(
                "visibility: the read",
                &["visibility", "web"],
                vec![ok(vis)],
                0,
                1,
            ),
            // covers: publish visibility --json
            on(
                "visibility --json: the read",
                &["visibility", "w b", "--json"],
                vec![ok(vis)],
                0,
                1,
            ),
            on(
                "visibility: private",
                &["visibility", "web", "private", "--reason", "done", "--yes"],
                vec![ok(r#"{"label":"web","mode":"private"}"#)],
                0,
                1,
            ),
            stdin(
                b"secret-pin-1\n",
                on(
                    "visibility: pin, the PIN on stdin over TCP",
                    &["visibility", "web", "pin", "--pin-days", "7"],
                    vec![
                        ok(r#"{"check":true}"#),
                        ok(r#"{"label":"web","mode":"pin"}"#),
                    ],
                    0,
                    2,
                ),
            ),
            stdin(
                b"",
                on(
                    "visibility: pin, no PIN",
                    &["visibility", "web", "pin"],
                    vec![ok(r#"{"check":true}"#)],
                    2,
                    1,
                ),
            ),
            on(
                "visibility: public needs yes, no terminal",
                &["visibility", "web", "public"],
                vec![ok(
                    r#"{"error":"moving it changes how K2 carries it","code":"needs_yes"}"#,
                )],
                2,
                1,
            ),
            on(
                "visibility: public dev, no --yes",
                &["visibility", "web", "public", "--dev-is-public"],
                vec![],
                2,
                0,
            ),
            on(
                "visibility: public dev --yes",
                &[
                    "visibility",
                    "web",
                    "public",
                    "--dev-is-public",
                    "-y",
                    "--reason=demo",
                ],
                vec![ok(r#"{"label":"web","mode":"public"}"#)],
                0,
                1,
            ),
            on(
                "visibility: held",
                &["visibility", "web", "public"],
                vec![ok(r#"{"status":"held","request":"req_9"}"#)],
                2,
                1,
            ),
            on(
                "visibility: refused, a card",
                &["visibility", "web", "private", "--json"],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                3,
                1,
            ),
            on(
                "visibility: --pin on argv",
                &["visibility", "web", "--pin", "1234"],
                vec![],
                2,
                0,
            ),
            on(
                "visibility: a bad mode",
                &["visibility", "web", "secret"],
                vec![],
                2,
                0,
            ),
            on(
                "visibility: --pin-days 300",
                &["visibility", "web", "pin", "--pin-days", "300"],
                vec![],
                2,
                0,
            ),
            on(
                "visibility: no name",
                &["visibility", "--json"],
                vec![],
                2,
                0,
            ),
            on(
                "visibility: not JSON",
                &["visibility", "web"],
                vec![ok("oops")],
                1,
                1,
            ),
            // covers: publish open
            on(
                "open --no-browser",
                &["open", "web", "--no-browser", "--path", "/docs"],
                vec![ok(
                    r#"{"url":"https://web.acme.k2.dev/?t=1","oneTime":true,"expiresInSecs":30}"#,
                )],
                0,
                1,
            ),
            // covers: publish open --json
            on(
                "open --json",
                &["open", "web", "--json"],
                vec![ok(r#"{"url":"https://web.acme.k2.dev/?t=1"}"#)],
                0,
                1,
            ),
            on(
                "open: person only",
                &["open", "web"],
                vec![ok(
                    r#"{"error":{"code":"person_only","hint":"agents can't"}}"#,
                )],
                3,
                1,
            ),
            on("open: no name", &["open"], vec![], 2, 0),
            // covers: publish sessions
            on("sessions", &["sessions", "web"], vec![ok(sessions)], 0, 1),
            // covers: publish sessions --json
            on(
                "sessions --json",
                &["sessions", "web", "--json"],
                vec![ok(sessions)],
                0,
                1,
            ),
            on(
                "sessions: none",
                &["sessions", "web"],
                vec![ok(r#"{"label":"web","sessions":[]}"#)],
                0,
                1,
            ),
            on(
                "sessions --revoke-all",
                &["sessions", "web", "--revoke-all"],
                vec![ok(r#"{"label":"web","revoked":1}"#)],
                0,
                1,
            ),
            on(
                "sessions --revoke-all: two",
                &["sessions", "web", "--revoke-all"],
                vec![ok(r#"{"label":"web","revoked":2}"#)],
                0,
                1,
            ),
            traceback(on(
                "sessions: a row that is no dict",
                &["sessions", "web"],
                vec![ok(r#"{"sessions":["x"]}"#)],
                1,
                1,
            )),
            on(
                "sessions: a flag",
                &["sessions", "web", "--all"],
                vec![],
                2,
                0,
            ),
        ],
    );
}

#[test]
fn claim_help_and_the_words_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "words");
    check_all(
        &rig,
        vec![
            // covers: publish subdomain claim
            on("claim", &["subdomain", "claim", "acme"], vec![ok(r#"{"success":true,"projectId":"p1"}"#)], 0, 1),
            on("claim --transfer", &["subdomain", "claim", "--transfer", "acme", "extra"], vec![ok(r#"{"success":true,"transferred":true}"#)], 0, 1),
            on("unclaim", &["subdomain", "unclaim", "acme"], vec![ok(r#"{"success":true,"removed":true}"#)], 0, 1),
            on("unclaim: nothing there", &["subdomain", "unclaim", "acme"], vec![ok(r#"{"success":true}"#)], 0, 1),
            on("claim: refused", &["subdomain", "claim", "acme"], vec![Reply::Answer(403, r#"{"error":"owned by another workspace","hint":"k2 publish subdomain claim acme --transfer"}"#.into())], 1, 1),
            on("claim: not JSON", &["subdomain", "claim", "acme"], vec![ok("x")], 1, 1),
            on("claim: no label", &["subdomain", "claim"], vec![], 1, 0),
            on("unclaim: no label", &["subdomain", "unclaim"], vec![], 1, 0),
            on("unclaim --transfer", &["subdomain", "unclaim", "a", "--transfer"], vec![], 2, 0),
            on("claim: a flag", &["subdomain", "claim", "a", "--force"], vec![], 2, 0),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case("the cell socket: claim", &["subdomain", "claim", "acme"], &[], vec![ok(r#"{"success":true,"projectId":"p1"}"#)], port_only, 0, 1),
            ),
            waited(on("claim: no answer", &["subdomain", "claim", "acme"], vec![Reply::Close], 1, 1)),
            // covers: publish
            on("publish alone", &[], vec![], 0, 0),
            on("publish help", &["help"], vec![], 0, 0),
            // covers: publish --json
            on("publish --json: an unknown verb", &["--json"], vec![], 1, 0),
            on("an unknown verb", &["deploy"], vec![], 1, 0),
            // 0.46.17 (8803c691a0): help anywhere is the help, nothing sent.
            shell(on("subdomain claim -h: the help", &["subdomain", "claim", "-h"], vec![], 0, 0)),
        ],
    );
}

/// Every verb that changes something, with `-h` or `--help` on the line:
/// the help, and no request.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let mut v = Vec::new();
    let lines: [&'static [&'static str]; 15] = [
        &["run", "web", "--cmd", "x", "--port", "1", "-h"],
        &["run", "--help"],
        &["start", "web", "--help"],
        &["stop", "-h", "web"],
        &["rm", "web", "--help"],
        &["frame", "web", "--none", "-h"],
        &["visibility", "web", "public", "--yes", "--help"],
        &["open", "web", "-h"],
        &["sessions", "web", "--revoke-all", "--help"],
        &["subdomain", "claim", "--help"],
        &["subdomain", "claim", "acme", "-h"],
        &["subdomain", "unclaim", "acme", "--help"],
        &["subdomain", "claim", "--transfer", "acme", "-h"],
        &["-h"],
        &["--help"],
    ];
    for args in lines {
        v.push(shell(on("help", args, vec![], 0, 0)));
    }
    check_all(&rig, v);
}

/// 0.46.17: who a held run's permission card goes to, in the body.
#[test]
fn run_assign_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "assign");
    let held = r#"{"ok":true,"status":"held","request":{"id":"w_run1","summary":"run web"},"card":{"where":"a permission card w_run1 is on a's Thread: sent to ada"},"hint":"The owner decides."}"#;
    check_all(
        &rig,
        vec![
            on(
                "run --assign: held",
                &[
                    "run",
                    "web",
                    "--cmd",
                    "python3 -m http.server 8999",
                    "--port",
                    "8999",
                    "--no-tunnel",
                    "--assign",
                    "ada",
                ],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run --assign= a comma list, --dev",
                &[
                    "run",
                    "web",
                    "--assign=ada,owner",
                    "--dev",
                    "--cmd",
                    "x",
                    "--port",
                    "1",
                    "--mem",
                    "2G",
                ],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run --assign: the value stripped",
                &[
                    "run", "web", "--cmd", "x", "--port", "1", "--assign", " ada é ",
                ],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run --assign= empty: no assign",
                &["run", "web", "--cmd", "x", "--port", "1", "--assign="],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run --assign: blank, no assign",
                &["run", "web", "--cmd", "x", "--port", "1", "--assign", "  "],
                vec![ok(held)],
                2,
                1,
            ),
            on(
                "run: --assign given last",
                &["run", "web", "--cmd", "x", "--port", "1", "--assign"],
                vec![],
                2,
                0,
            ),
            on(
                "run --json: --assign empty",
                &["run", "web", "--json", "--assign", "", "--cmd", "x"],
                vec![],
                2,
                0,
            ),
            shell(on(
                "help never acts: run --assign ada --help",
                &[
                    "run", "web", "--cmd", "x", "--port", "1", "--assign", "ada", "--help",
                ],
                vec![],
                0,
                0,
            )),
        ],
    );
}

/// The binary's lines name the brand's command and product.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    type Row = (&'static [&'static str], Vec<Reply>, i32, String, String);
    let rows: Vec<Row> = vec![
        (&["deploy"], vec![], 1, String::new(), format!("Unknown '{cli} publish' subcommand: deploy\n")),
        (&["stop"], vec![], 2, String::new(), format!("Error: usage: {cli} publish stop <name>\n")),
        (&["subdomain", "claim"], vec![], 1, String::new(), format!("Usage: {cli} publish subdomain claim <label>\n       {cli} publish subdomain claim <label> --transfer   (owner/admin; audited)\n")),
        (&["run", "w", "--dev", "--no-tunnel"], vec![], 2, String::new(), format!("Error: --dev puts a dev server behind {product}'s check at its address, so it can't use --no-tunnel. To run one for yourself only, run the command in a terminal\n")),
        (
            &["ps"],
            vec![ok(r#"{"services":[{"name":"a","reason":"boom","legacyLabel":true}]}"#)],
            0,
            format!("NAME  STATUS  PORT  VISIBILITY  FRAME       URL/TARGET\na     -       -     -           own server  -  [legacy (--cmd)]\na is not serving (down): boom. More: {cli} publish logs a\nlegacy (--cmd): a runs unwalled and keeps running; move it to an App when ready ({cli} study app-move).\n"),
            String::new(),
        ),
    ];
    for (args, answers, code, out, err) in rows {
        let c = on("brand", args, answers, code, 0);
        let (ran, _) = rig.binary(&c, false);
        if args.first() == Some(&"deploy") {
            assert!(ran.stderr.starts_with(&err), "{args:?}: {}", ran.stderr);
        } else {
            assert_eq!(
                (ran.stdout.as_str(), ran.stderr.as_str()),
                (out.as_str(), err.as_str()),
                "{args:?}"
            );
        }
        assert_eq!(ran.code, Some(code), "{args:?}");
    }
}

/// The large list: above 256 KiB, thousands of rows.
#[test]
fn ps_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| format!(r#"{{"name":"svc-{i:05}","status":"running","port":{},"url":"https://svc-{i:05}.acme.k2.dev"}}"#, 10000 + i))
        .collect();
    let body = format!(r#"{{"services":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let mut want = format!(
        "{:<9}  {:<7}  {:<5}  {:<10}  {:<10}  URL/TARGET\n",
        "NAME", "STATUS", "PORT", "VISIBILITY", "FRAME"
    );
    for i in 0..3000 {
        want.push_str(&format!(
            "{:<9}  {:<7}  {:<5}  {:<10}  {:<10}  https://svc-{i:05}.acme.k2.dev\n",
            format!("svc-{i:05}"),
            "running",
            10000 + i,
            "-",
            "own server"
        ));
    }
    let c = on("a large list", &["ps"], vec![ok(&body)], 0, 1);
    let (out, log) = rig.binary(&c, false);
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""), "the binary");
    assert!(
        out.stdout == want,
        "the binary's stdout differs ({} bytes, want {})",
        out.stdout.len(),
        want.len()
    );
    assert_eq!(log.len(), 1, "the binary's requests");
    rig.prepare(Side::Bash, &c);
    let bash = rig.run(true, &c);
    assert_eq!(
        rig.read(Side::Bash, false, c.name).len(),
        1,
        "bash's requests"
    );
    if bash.code == Some(0) {
        assert!(
            bash.stdout == want,
            "bash's stdout differs from the binary's"
        );
    } else {
        assert_eq!(bash.code, Some(126), "bash: {}", bash.stderr);
        assert!(
            bash.stderr.contains("Argument list too long"),
            "bash: {}",
            bash.stderr
        );
    }
}

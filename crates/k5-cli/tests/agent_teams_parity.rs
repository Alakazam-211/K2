//! `k5-cli agent ...` against the shell CLI's `cli/k2 agent ...` on the
//! parity rig ([`common::parity`]) for the router's last `agent` forms:
//! `team`, `instances` and `hire-root` (`cli/src/agent-hire.sh`), the
//! family alone, `template` alone, `update`, `complete`, `profile`, and a
//! word the router has no arm for (a retired verb's line, or `Unknown
//! agent subcommand`).
//!
//! - Every usage line, and a flag that takes a value as the last word
//!   (`_need_value`'s line, exit 2, nothing sent).
//! - `_hire_print`'s views, `--json`, each refusal's exit (3, 4, 2, 1), an
//!   answer that is not JSON, a reader that raises (compared by its last
//!   line), no answer (the line, then SIGPIPE), and one list above
//!   256 KiB.
//! - `team` and `instances` are the `id` tool `agent-teams`: a session
//!   with no passport is refused (exit 3), one with a passport sends it,
//!   on both sides. `hire-root` and `complete` are open verbs: a session
//!   with a passport is the named difference `owner-token-with-passport`.
//!   `update` and `profile` send the session's passport, on both sides.
//! - Help never acts: `-h` or `--help` anywhere prints a page and sends
//!   nothing. A captured form is the front's page; help with other words
//!   is the shell's, which prints the same page.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "agent";

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

fn leak(args: &[&str]) -> &'static [&'static str] {
    let owned: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(owned.into_boxed_slice())
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// A request on the left of a pipe into the shell's reader, with no
/// answer: the line, then SIGPIPE (141), compared as
/// [`Compare::Waited`] on every bash.
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

const LINKS: &str = r#"{"ok":true,"workspace":{"handle":"ops","id":"w1"},"team":{"name":"Acme crew","instance":"hi_1","members":[{"handle":"lead-a","lead":true,"role":"lead"},{"handle":"dev-b","role":"dev"},{"handle":"qa-c"}]},"servedBy":[{"id":"l1","kind":"serves","teamName":"Globex","teamLeadHandle":"gx-lead","scope":{"rooms":["a"]}},{"id":"l2","kind":"helper_of","teamLeadHandle":"solo"}],"serves":[{"id":"l3","kind":"serves","consumerHandle":"front","scope":null},{"id":"l4","kind":"helper_of"}]}"#;

const INSTANCES: &str = r#"{"ok":true,"instances":[{"id":"hi_1","status":"live","teamName":"Acme crew","members":[{"handle":"lead-a"},{"handle":"dev-b"}],"apiKeyId":"key_0123456789","onBehalfOf":"ana@acme.test"},{"id":"hi_2","status":"retired","members":[],"actor":"owner"},{"id":"hi_3","status":"live","apiKeyId":"","actor":null}]}"#;

const INSTANCE: &str = r#"{"ok":true,"instance":{"id":"hi_1","status":"retire_blocked","teamName":"Acme crew","reason":"a member is busy","members":[{"handle":"lead-a","lead":true,"role":"lead","status":"live","path":"/srv/h/lead-a"},{"handle":"dev-b","role":"dev","status":"busy"}]},"links":[{"id":"l1","teamLeadHandle":"lead-a","consumerHandle":"front"}],"events":[{"at":"2026-11-20T10:00:00Z","action":"hire","result":"ok"},{"at":"2026-11-20T11:00:00Z","action":"retire","result":"blocked","reason":"dev-b is busy"}]}"#;

const HIRED: &str = r#"{"ok":true,"changed":true,"instance":"hi_9","members":[{"handle":"lead-a","lead":true,"path":"/srv/h/lead-a"},{"handle":"dev-b","path":"/srv/h/dev-b"}],"links":[{"id":"l7"}]}"#;

const PLAN: &str = r#"{"ok":true,"dryRun":true,"plan":{"members":[{"handle":"lead-a","lead":true,"path":"/srv/h/lead-a"}],"note":"A dry run: nothing written."}}"#;

const RETIRED: &str = r#"{"ok":true,"instance":"hi_1","status":"retired","members":[{"handle":"lead-a","status":"retired"},{"handle":"dev-b","status":"kept","reason":"has open work"}],"hint":"Archive folders stay under the hire root."}"#;

fn recipe(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.proj.join("recipe.json"),
        "{\"team\": \"Acme crew\", \"members\": [{\"handle\": \"lead-a\", \"lead\": true}], \"key\": \"old\", \"n\": 1E5}\n\n",
    )
    .unwrap();
    std::fs::create_dir_all(r.proj.join("adir")).unwrap();
}

#[test]
fn the_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    // native: agent team link
    // native: agent team unlink
    // native: agent team served
    // native: agent instances show
    // native: agent instances retire
    // native: agent instances hire
    // native: agent hire-root set
    // native: agent update
    // native: agent complete
    for (args, code) in [
        (&["team", "link"][..], 2),
        (&["team", "link", "t"], 2),
        (&["team", "link", "t", "w", "x"], 2),
        (&["team", "link", "--x"], 2),
        (&["team", "link", "t", "w", "--scope", "{bad"], 2),
        (&["team", "link", "t", "--kind"], 2),
        (&["team", "link", "t", "w", "--scope"], 2),
        (&["team", "unlink"], 2),
        (&["team", "unlink", ""], 2),
        (&["team", "--workspace"], 2),
        (&["team", "served", "--workspace"], 2),
        (&["team", "served", "x"], 2),
        (&["team", ""], 2),
        (&["team", "nope"], 2),
        (&["teams", "nope", "x"], 2),
        (&["instances", "show"], 2),
        (&["instances", "retire"], 2),
        (&["instances", "retire", "--x"], 2),
        (&["instances", "hire"], 2),
        (&["instances", "hire", "--x"], 2),
        (&["instances", "hire", "--key"], 2),
        (&["instances", "hire", "r.json", "--key"], 2),
        (&["instances", "hire", "no-such.json"], 2),
        (&["instances", "x"], 2),
        (&["instances", "--json", "x"], 2),
        (&["instance", "--key"], 2),
        (&["instances", "--json", "--key"], 2),
        (&["hire-root", "x"], 2),
        (&["hire-root", "set"], 2),
        (&["update"], 2),
        (&["update", "--name"], 2),
        (&["update", "qa", "--field", "role"], 2),
        (&["update", "qa", "--value", "x", "--field"], 2),
        (&["complete"], 1),
        (&["complete", "--agent", "qa"], 1),
        (&["complete", "--agent", "", "--file", "f.md"], 1),
        (&["complete", "--file", "f.md", "--agent"], 2),
        (&["complete", "--agent"], 2),
    ] {
        v.push(case(
            "a usage line, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            code,
            0,
        ));
    }
    // native: agent create
    // native: agent delete
    // native: agent remove
    for (args, code) in [
        (&["nope"][..], 2),
        (&["--json"], 2),
        (&["create"], 1),
        (&["delete", "qa"], 1),
        (&["remove"], 1),
    ] {
        v.push(case(
            "a word the router has no arm for",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            code,
            0,
        ));
    }
    // native: agent
    // native: agent template
    // native: agent templates
    for args in [
        &[][..],
        &[""],
        &["", "x"],
        &["template"],
        &["templates"],
        &["template", ""],
    ] {
        v.push(case(
            "a help page",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    v.push(case(
        "template alone needs no daemon",
        &["template"],
        &[],
        vec![],
        nothing,
        0,
        0,
    ));
    for args in [&[][..], &["team"], &["hire-root"], &["nope"], &["complete"]] {
        v.push(case(
            "no daemon: the gate",
            leak(args),
            &[],
            vec![],
            nothing,
            1,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let mut v = Vec::new();
    // The front's captured pages: what the shell printed (the hire verbs'
    // help, or the agent page for update, complete and profile), exit 0,
    // nothing sent. The first four were bash bugs where help acted (team
    // unlink and hire-root set sent, instances show and profile asked);
    // the fixed shell prints the page.
    // native: agent team unlink --help
    // native: agent instances show --help
    // native: agent hire-root set --help
    // native: agent profile --help
    for args in [
        &["team", "unlink", "--help"][..],
        &["team", "unlink", "-h"],
        &["instances", "show", "--help"],
        &["instances", "show", "-h"],
        &["hire-root", "set", "--help"],
        &["hire-root", "set", "-h"],
        &["profile", "--help"],
        &["profile", "-h"],
        &["team", "link", "--help"],
        &["team", "served", "-h"],
        &["instances", "retire", "--help"],
        &["instances", "hire", "-h"],
        &["hire-root", "--help"],
        &["update", "--help"],
        &["complete", "-h"],
        &["team", "help"],
        &["instances", "help"],
    ] {
        v.push(case(
            "a page, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    // Help with other words: not a captured form, so the front hands the
    // line to the shell, which prints the page and sends nothing. Before
    // the shell's fix, the last five sent their request (or asked).
    for args in [
        &["team", "link", "t", "w", "--help"][..],
        &["instances", "retire", "i1", "--help"],
        &["team", "unlink", "l1", "--help"],
        &["hire-root", "set", "/srv/x", "--help"],
        &["update", "qa", "--field", "f", "--value", "v", "--help"],
        &["complete", "--agent", "qa", "--file", "f.md", "-h"],
        &["profile", "/srv/ws", "--help"],
    ] {
        v.push(shell(case(
            "help with words: the shell's page, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )));
    }
    check(&rig, v);
}

#[test]
fn team_reads() {
    let rig = Rig::new(VERB, "team");
    let v = vec![
        // native: agent team
        case(
            "the team, its links",
            &["team"],
            &[],
            vec![ok(LINKS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["team", "--json"],
            &[],
            vec![ok(LINKS)],
            owner_and_port,
            0,
            1,
        ),
        // native: agent teams
        case(
            "another workspace",
            &["teams", "--workspace", "acme ops&x=1"],
            &[],
            vec![ok(LINKS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "served",
            &["team", "served", "--workspace", "ops"],
            &[],
            vec![ok(LINKS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nobody, a single hire",
            &["team"],
            &[],
            vec![ok(
                r#"{"ok":true,"workspace":{"id":"w1"},"team":{"instance":"hi_2"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no workspace in the answer",
            &["team", "served"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refused: exit 3",
            &["team"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"an agent reads its own links"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a token problem: exit 3",
            &["team"],
            &[],
            vec![Reply::Answer(401, r#"{"error":"Invalid TOKEN"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal's permission card line",
            &["team"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not found: exit 4",
            &["team", "--workspace", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no workspace nope"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "usage: exit 2",
            &["team"],
            &[],
            vec![ok(r#"{"error":{"code":"usage","hint":"say which"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "any other error: exit 1",
            &["team"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["team"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a link that is not a dict",
            &["team"],
            &[],
            vec![ok(r#"{"workspace":{"id":"w"},"servedBy":["x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a link with no id",
            &["team", "served"],
            &[],
            vec![ok(r#"{"serves":[{"kind":"serves"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an error code that is a list",
            &["team"],
            &[],
            vec![ok(r#"{"error":{"code":["x"],"hint":"h"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a hint that is not text",
            &["team"],
            &[],
            vec![ok(r#"{"error":{"code":"x","hint":7}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an answer that is a list",
            &["team"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        in_pipe(case(
            "no answer: the line, SIGPIPE",
            &["team"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a session with no passport: refused",
            &["team"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "a session with a passport: it sends it (the probe first)",
                &["team"],
                &[],
                vec![ok(r#"{"ok":true}"#), ok(LINKS)],
                owner_and_port,
                0,
                2,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a passport outside a session",
                &["instances"],
                &[],
                vec![ok(INSTANCES)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    check(&rig, v);
}

#[test]
fn instance_reads() {
    let rig = Rig::new(VERB, "inst");
    let v = vec![
        // native: agent instances
        case(
            "the hires",
            &["instances"],
            &[],
            vec![ok(INSTANCES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "none yet",
            &["instances"],
            &[],
            vec![ok(r#"{"ok":true,"instances":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // native: agent instance
        case(
            "by key, --json",
            &["instance", "--key", "k 1", "--json"],
            &[],
            vec![ok(INSTANCES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty key is no key",
            &["instances", "--key", ""],
            &[],
            vec![ok(INSTANCES)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "a member with no handle",
            &["instances"],
            &[],
            vec![ok(
                r#"{"instances":[{"id":"i","status":"s","members":[{}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a key id that is a number",
            &["instances"],
            &[],
            vec![ok(
                r#"{"instances":[{"id":"i","status":"s","apiKeyId":7}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a row with no status",
            &["instances"],
            &[],
            vec![ok(r#"{"instances":[{"id":"i"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "show",
            &["instances", "show", "hi_1"],
            &[],
            vec![ok(INSTANCE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show --json",
            &["instance", "show", "hi_1", "--json"],
            &[],
            vec![ok(INSTANCE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, a bare instance",
            &["instances", "show", "x"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, not found: exit 4",
            &["instances", "show", "nope", "--json"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no hire nope"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        traceback(case(
            "show, an event reason that is not text",
            &["instances", "show", "x"],
            &[],
            vec![ok(
                r#"{"instance":{"id":"x"},"events":[{"at":"t","action":"a","result":"r","reason":5}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // native: agent hire-root
        case(
            "the hire root, the default",
            &["hire-root"],
            &[],
            vec![ok(r#"{"ok":true,"root":"/srv/hires","default":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the hire root --json",
            &["hire-root", "--json", "x"],
            &[],
            vec![ok(r#"{"ok":true,"root":"/srv/hires","default":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty word is the read",
            &["hire-root", ""],
            &[],
            vec![ok(r#"{"ok":true,"root":"/srv/x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "hire-root in a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["hire-root"],
                    &[],
                    vec![ok(r#"{"ok":true,"root":"/srv/x"}"#)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
    ];
    check(&rig, v);
}

/// One list above 256 KiB: the shell's reader reads it on stdin.
#[test]
fn a_list_above_256_kib() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|n| {
            format!(
                r#"{{"id":"hi_{n:05}","status":"live","teamName":"Acme crew {n}","members":[{{"handle":"lead-{n}"}},{{"handle":"dev-{n}"}}],"apiKeyId":"key_{n:012}","onBehalfOf":"person-{n}@acme.test"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"ok":true,"instances":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "thousands of hires",
        &["instances"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn writes() {
    let rig = Rig::new(VERB, "write");
    let v = vec![
        case(
            "link",
            &["team", "link", "hi_1", "front"],
            &[],
            vec![ok(r#"{"ok":true,"changed":true,"link":{"id":"l9"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link, every flag",
            &[
                "team",
                "link",
                "--kind",
                "helper_of",
                "hi_1",
                "--mesh",
                "front",
                "--scope",
                r#"{"rooms":["a"],"n":1E2}"#,
                "--json",
            ],
            &[],
            vec![ok(r#"{"ok":true,"changed":true,"link":{"id":"l9"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link, already",
            &["team", "link", "hi_1", "front", "--kind", ""],
            &[],
            vec![ok(r#"{"ok":true,"changed":false,"link":{"id":"l9"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "link, refused",
            &["team", "link", "hi_1", "front"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"the owner links teams"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        in_pipe(case(
            "link, no answer: the line, SIGPIPE",
            &["team", "link", "hi_1", "front"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        case(
            "unlink",
            &["team", "unlink", "l9", "x"],
            &[],
            vec![ok(r#"{"ok":true,"changed":true,"link":"l9"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unlink, already gone",
            &["team", "unlink", "l9"],
            &[],
            vec![ok(r#"{"ok":true,"changed":false,"link":"l9"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "retire --force",
            &["instances", "retire", "hi_1", "--force"],
            &[],
            vec![ok(RETIRED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "retire, blocked: exit 3",
            &["instances", "retire", "hi_1"],
            &[],
            vec![ok(
                r#"{"ok":true,"instance":"hi_1","status":"retire_blocked","members":[{"handle":"dev-b","status":"busy","reason":"open work"}]}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "retire --json",
            &["instance", "retire", "--json", "hi_1"],
            &[],
            vec![ok(RETIRED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "hire from a file, a key",
            &["instances", "hire", "recipe.json", "--key", "k1"],
            &[],
            vec![ok(HIRED)],
            recipe,
            0,
            1,
        ),
        case(
            "hire, already (same key)",
            &["instances", "hire", "recipe.json", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"changed":false,"instance":"hi_9"}"#)],
            recipe,
            0,
            1,
        ),
        Case {
            stdin: Some(b"{\"team\": \"From stdin\", \"x\": [1, 2.50]}\n"),
            ..case(
                "hire from stdin, a dry run",
                &["instances", "hire", "-", "--dry-run"],
                &[],
                vec![ok(PLAN)],
                owner_and_port,
                0,
                1,
            )
        },
        Case {
            stdin: Some(b"not json"),
            ..case(
                "hire, a recipe that is not JSON",
                &["instances", "hire", "-"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
        Case {
            stdin: Some(b"[1]"),
            ..traceback(case(
                "hire, a recipe that is a list, with a key",
                &["instances", "hire", "-", "--key", "k"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ))
        },
        case(
            "hire, a folder",
            &["instances", "hire", "adir"],
            &[],
            vec![],
            recipe,
            1,
            0,
        ),
        case(
            "hire, no answer: the reader's line",
            &["instances", "hire", "recipe.json"],
            &[],
            vec![Reply::Close],
            recipe,
            1,
            1,
        ),
        case(
            "hire-root set",
            &["hire-root", "set", "/srv/new root", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"root":"/srv/new root"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "hire-root set, refused",
            &["hire-root", "set", "/x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"the owner sets the hire root"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
    ];
    check(&rig, v);
}

#[test]
fn update_complete_profile() {
    let rig = Rig::new(VERB, "older");
    let v = vec![
        case(
            "update",
            &[
                "update",
                "qa",
                "--field",
                "Work Sources",
                "--value",
                "a & b",
            ],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "update, --name wins the first word",
            &[
                "update", "--name", "qa", "x", "--field", "role", "--value", "v",
            ],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "update, no success in the answer",
            &["update", "qa", "--field", "role", "--value", "v"],
            &[],
            vec![ok(r#"{"ok":true,"note":"nothing to do"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "update, an error",
            &["update", "qa", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"no agent qa"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "update, a permission card: exit 3",
            &["update", "qa", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "update, no answer",
            &["update", "qa", "--field", "role", "--value", "v"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "update in a session: the passport, the socket first",
                &["update", "qa", "--field", "role", "--value", "v"],
                &[],
                vec![ok(r#"{"success":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "complete",
            &["complete", "--file", "done.md", "x", "--agent", "qa bot"],
            &[],
            vec![ok("{\"ok\":true,\"moved\":\"done.md\"}\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "complete, another agent: not_this_agent",
            &["complete", "--agent", "other", "--file", "f.md"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"not_this_agent","agent":"qa"}}"#.into(),
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "complete, no answer",
            &["complete", "--agent", "qa", "--file", "f.md"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "complete in a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["complete", "--agent", "qa", "--file", "f.md"],
                    &[],
                    vec![ok(r#"{"ok":true}"#)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        // native: agent profile
        case(
            "profile",
            &["profile"],
            &[],
            vec![ok(r#"{"display_name":"QA"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "profile, a target",
            &["profile", "/srv/other ws", "z"],
            &[],
            vec![ok(r#"{"display_name":"other"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "profile, an error",
            &["profile", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"no such workspace"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
    ];
    check(&rig, v);
}

/// The lines name the command and the product by the brand (in a build
/// with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or
/// `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for (args, want, code, replies) in [
        (
            &["team", "unlink"][..],
            format!("Usage: {cli} agent team unlink <link-id>\n"),
            2,
            vec![],
        ),
        (
            &["instances", "nope"],
            format!("Unknown: {cli} agent instances nope (see {cli} agent instances --help)\n"),
            2,
            vec![],
        ),
        (
            &["hire-root", "x"],
            format!("Usage: {cli} agent hire-root [set <dir>]  \u{2014} the folder API keys hire agents into (owner sets it)\n"),
            2,
            vec![],
        ),
        (
            &["complete"],
            format!("Usage: {cli} agent complete --agent <name> --file <filename>\n"),
            1,
            vec![],
        ),
        (
            &["delete"],
            format!("Error: 'agent delete' is retired. Use instead: {cli} agent retire <name>\n"),
            1,
            vec![],
        ),
        (
            &["update", "qa", "--field", "role", "--value", "v"],
            format!("Error: {product} did not say it updated role for agent qa: {{}}\n"),
            1,
            vec![ok("{}")],
        ),
    ] {
        let sent = replies.len();
        let c = case("brand", args, &[], replies, owner_and_port, code, sent);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert_eq!(log.len(), sent, "{args:?}");
    }
}

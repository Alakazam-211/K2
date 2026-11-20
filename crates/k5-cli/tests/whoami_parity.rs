//! `k5-cli whoami` against the shell CLI's `cli/k2 whoami`, on the parity
//! rig ([`common::parity`]): for every case both get the same environment,
//! folder and answers, each against its own stub, and must send the same
//! requests and print the same stdout and stderr with the same exit code.
//! Then the binary runs once more with no `python3` on `PATH`: the same
//! requests and the same output as with one.
//!
//! One case differs on purpose, by PRD 13.9 #4, and is checked as such:
//! in a session with a cell socket and the owner token on disk, bash's
//! open tools send the owner token over TCP (COMPAT-58); the binary sends
//! the passport. The requests are otherwise the same.
//!
//! Where the shell's Python fails with a traceback (an answer that is JSON
//! but no object, a `siblings.live` that is no number), its text depends
//! on the Python version, so those cases compare the exit code, stdout and
//! the traceback's exception name.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::path::Path;
use std::process::Command;

const VERB: &str = "whoami";
const FULL: &str = r#"{"workspace":"sales","role":"sidecar","address":"sales/reviewer","primary":"sales","session":"s1","path":"/w/sales","siblings":{"live":2,"others":[]}}"#;

fn cases(rig: &Rig) -> Vec<Case> {
    let with = with_passport;
    let permission_ask = r#"{"error":{"code":"owner_only","hint":"Only the owner can do that."},"permission":{"command":"k2 permission request --scope whoami.read --why \"...\"","requestable":true}}"#;
    let permission_none = r#"{"error":"nope","hint":"ask a person","permission":{"requestable":false,"hint":"Only a person can change this."}}"#;
    let mut v = vec![
        // covers: whoami
        case(
            "owner from disk, the plain form",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        // covers: whoami --json
        case(
            "owner from disk, --json",
            &["--json"],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "owner from disk, in a session by its markers",
            &[],
            &[("K2_CELL", "canonical")],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the session's names, under each prefix",
            &["--json"],
            &[
                ("K5_SESSION_ID", "s 5/é"),
                ("K2_SESSION_ID", "s2"),
                ("K5_CELL", ""),
                ("K2_CELL", "sidecar"),
                ("K2SO_SIDECAR_NAME", "rev&1"),
                ("K2_PRIMARY", "acme"),
            ],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "words that are not flags are ignored",
            &["extra", "--json", "help"],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an unknown flag",
            &["--bogus"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "a lone dash is a flag",
            &["--json", "-"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "no port: the connection gate",
            &[],
            &[],
            vec![],
            |r| r.owner_on_disk(),
            1,
            0,
        ),
        case(
            "no token: the connection gate",
            &[],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
        case(
            "an empty port setting is no port",
            &[],
            &[("K5_PORT", "")],
            vec![],
            |r| r.owner_on_disk(),
            1,
            0,
        ),
        case(
            "a port that is no port",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            |r| r.owner_on_disk(),
            2,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "the guard, under the K5 name, comes before the flags",
            &["--bogus"],
            &[("K5_NO_OWNER_FALLBACK", "1")],
            vec![],
            |r| r.owner_on_disk(),
            3,
            0,
        ),
        Case {
            env: with(&[("K2_NO_OWNER_FALLBACK", "1")]),
            ..case(
                "the guard lets a passport through",
                &[],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a passport over TCP",
                &[],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "a passport and a live cell socket: still TCP",
                &["--json"],
                &[],
                vec![ok(FULL)],
                port_only,
                0,
                1,
            )
        },
        Case {
            env: with(&[("K2_HOOK_SOCK", "{sock}")]),
            compare: Compare::OwnerTokenWithPassport,
            ..case(
                "a passport, a cell socket and the owner token on disk (PRD 13.9 #4)",
                &[],
                &[],
                vec![ok(FULL)],
                owner_and_port,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a re-minted passport is used from the start",
                &["--json"],
                &[],
                vec![ok(FULL)],
                healed_on_disk,
                0,
                1,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a refused passport heals once",
                &["--json"],
                &[],
                vec![heal(rig), ok(FULL)],
                port_only,
                0,
                2,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a dead passport: the probe, then the restart line",
                &["--json"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            )
        },
        Case {
            env: with(&[]),
            ..case(
                "a refused passport the probe finds good: no line",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into()), ok(FULL)],
                port_only,
                1,
                2,
            )
        },
        case(
            "a refusal's closing line, the plain form",
            &[],
            &[],
            vec![Reply::Answer(403, permission_ask.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal's closing line, --json",
            &["--json"],
            &[],
            vec![Reply::Answer(403, permission_ask.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal that can't be requested",
            &[],
            &[],
            vec![Reply::Answer(403, permission_none.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no identity here",
            &[],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"cannot determine identity","hint":"run inside a workspace session"}"#
                    .into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with no hint or code",
            &[],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"msg":"é \"q\" 'x'","n":[1.5,null,true]}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json without a role",
            &["--json"],
            &[],
            vec![ok(r#"{"workspace":"w"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json drops the answer's trailing newlines",
            &["--json"],
            &[],
            vec![ok("{\"role\":\"r\"}\n\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an answer that is not JSON",
            &[],
            &[],
            vec![ok("<html>oops</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer with 200",
            &[],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Waited,
            ..case(
                "a 5xx with an empty body: no answer",
                &[],
                &[],
                vec![Reply::Answer(502, String::new())],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "migrating: asked again",
            &[],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(FULL),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "Python's str() of each value",
            &[],
            &[],
            vec![ok(
                r#"{"workspace":null,"role":1.0,"address":["a",{"b":false}],"primary":1e16,"session":-0,"siblings":{"live":2.9}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &[],
            &[],
            vec![ok(
                r#"{"role":"r","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no bulletin line when it failed",
            &["--json"],
            &[],
            vec![ok(
                r#"{"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer that is no object",
                &[],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "a siblings count that is no number",
                &[],
                &[],
                vec![ok(r#"{"role":"r","siblings":{"live":"2"}}"#)],
                owner_and_port,
                1,
                1,
            )
        },
    ];
    // PWD names the workspace through a link: both keep the link's path.
    let link = link_to_proj(rig);
    v.push(Case {
        env: vec![("PWD", link.to_string_lossy().into_owned())],
        cwd: Some(link.clone()),
        ..case(
            "PWD through a link",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )
    });
    // PROJECT_PATH wins over the folder.
    v.push(case(
        "PROJECT_PATH",
        &[],
        &[("K2_PROJECT_PATH", "/w/proj é&")],
        vec![ok(FULL)],
        owner_and_port,
        0,
        1,
    ));
    v
}

#[test]
fn whoami_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    for c in cases(&rig) {
        rig.check(&c);
    }
}

/// Nothing accepted the connection: exit 7, nothing printed, after the
/// tries a second apart, TMPDIR or not (a refused connect is never the
/// machine's).
#[test]
fn whoami_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    for no_tmpdir in [false, true] {
        let c = Case {
            env: vec![("K2_PORT", dead_port())],
            no_tmpdir,
            ..case(
                "nothing listens",
                &[],
                &[],
                vec![],
                |r| r.owner_on_disk(),
                7,
                0,
            )
        };
        rig.check(&c);
    }
}

/// The other transport failures end with curl's code, as `set -e` passes
/// it on, and print nothing: a reset is 56; with TMPDIR unusable a failure
/// is this machine's, `_cli_curl`'s 75, a port that is no port too.
#[test]
fn whoami_with_a_broken_exchange() {
    let rig = Rig::new(VERB, "broken");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "a reset (56)",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        case(
            "a reset, --json (56)",
            &["--json"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            56,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable: a reset is 76",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            76,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable: a port that is no port is refused first",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            |r| r.owner_on_disk(),
            2,
            0,
        )),
        no_tmp(case(
            "TMPDIR unusable: an answer is an answer",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// git: no line while `.k2so/` files are still in git's index (the
/// 0.40.4 rename heads-up is gone from both), and a worktree's main
/// folder when no folder up from here is a workspace.
#[test]
fn whoami_and_git() {
    let rig = Rig::new(VERB, "git");
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("HOME", &rig.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    // No rename line: a .k2so file in the index, the folder gone, .k2 there.
    git(&rig.proj, &["init", "-q"]);
    std::fs::create_dir_all(rig.proj.join(".k2so")).unwrap();
    std::fs::write(rig.proj.join(".k2so/note.md"), "x").unwrap();
    git(&rig.proj, &["add", ".k2so/note.md"]);
    std::fs::remove_dir_all(rig.proj.join(".k2so")).unwrap();
    let mut c = case(
        "no rename line",
        &[],
        &[],
        vec![ok(FULL)],
        owner_and_port,
        0,
        1,
    );
    rig.check(&c);
    let (probe, _) = rig.binary(&c, false);
    assert!(
        probe.stderr.is_empty(),
        "the binary printed on stderr: {probe:?}"
    );

    // A worktree outside the workspace: the main folder is the project.
    let main = rig.dir.join("main");
    std::fs::create_dir_all(main.join(".k2")).unwrap();
    git(&main, &["init", "-q"]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    );
    let wt = rig.dir.join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    c = Case {
        cwd: Some(wt.clone()),
        ..case(
            "a worktree finds its main folder",
            &[],
            &[],
            vec![ok(FULL)],
            owner_and_port,
            0,
            1,
        )
    };
    rig.check(&c);
    let (_, log) = rig.binary(&c, false);
    let main_q =
        k5_cli::transport::encode::quote(&std::fs::canonicalize(&main).unwrap().to_string_lossy());
    // A normalised request: its channel, then its request line, the
    // query's escapes already in upper case.
    let line = log[0]
        .lines()
        .nth(1)
        .unwrap_or_else(|| panic!("no request line in {:?}", log[0]));
    assert!(
        line.contains(&format!("project={main_q}&")),
        "not the main folder: {line}"
    );
}

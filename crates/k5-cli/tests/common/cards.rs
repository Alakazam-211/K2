//! The permission cards probe's rows, for a `k2 study` page that asks it
//! (`_permission_probe`, `cli/src/permission.sh`): each way the answer
//! can go, on the cell socket and over TCP, and each way it can fail to
//! come. A study page's parity test runs them on a [`Rig`] for its own
//! arguments: `cli/k2` and the binary must send the same request (or
//! none) and print the same page with the same exit.
//!
//! With no `python3` the binary's probe reads the daemon's answer itself
//! (K2's ruling), so two rows answer otherwise there, on purpose: a
//! `PYTHON*` setting and a locale that is not UTF-8 (no Python reads the
//! answer, so neither counts). Each reads the daemon's "on" as on
//! ([`check_without_python3`]).
//!
//! The daily tier of the help asks the feedback probe first
//! (`_sf_agents_off_here`, [`k5_cli::feedback_optin`]): its rows
//! ([`daily_rows`]) are these, each after a settings read that shows
//! send-feedback.

#![cfg(unix)]

use super::parity::{
    case, dead_port, heal, healed_on_disk, ok, owner_and_port, with_passport, Case, Compare,
    NoPython3, Rig, GARBAGE, PORT, REFUSED,
};
use super::stub::Reply;
use std::time::Duration;

pub const ON: &str = r#"{"enabled":true}"#;
pub const OFF: &str = r#"{"enabled":false}"#;
/// The feedback settings' answer in [`daily_rows`]: agents may report, so
/// send-feedback shows.
pub const AGENTS_ON: &str = r#"{"enabled":true,"agents":true,"canSend":true,"senderKind":"agent"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// No `daemon.port` from an earlier row (the rig's own reset leaves it).
fn fresh(r: &Rig) {
    let _ = std::fs::remove_file(r.k2home.join("daemon.port"));
}

fn fresh_owner(r: &Rig) {
    fresh(r);
    owner_and_port(r);
}

fn fresh_healed(r: &Rig) {
    fresh(r);
    healed_on_disk(r);
}

/// The daemon moved to the stub's port: `daemon.port` says so.
fn moved(r: &Rig) {
    std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub().port)).unwrap();
}

/// The row with a passport and neither a socket nor a port.
pub const NO_SOCKET_NO_PORT: &str = "no socket and no port: nothing asked, off";

/// The rows for a help page (the daily tier: `k2`, `k2 help`,
/// `k2 --help`), which exits 0 either way: [`daily_rows`], but the shell's
/// connection gate wants a port for help, so with none it stops at the
/// gate (`Error: Cannot connect to K2.`, exit 1) before anything is asked.
pub fn help_rows(rig: &Rig, args: &'static [&'static str]) -> Vec<Case> {
    let mut all = daily_rows(rig, args, 0, 0);
    let gate = all
        .iter_mut()
        .find(|c| c.name == NO_SOCKET_NO_PORT)
        .expect("the row with no port");
    gate.name = "no socket and no port: the connection gate, exit 1";
    gate.code = 1;
    all
}

/// The rows for `args`, whose page exits `on_code` with cards on and
/// `off_code` with them off.
pub fn rows(rig: &Rig, args: &'static [&'static str], on_code: i32, off_code: i32) -> Vec<Case> {
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let tcp = || with_passport(&[("K2_HOOK_SOCK", "{nosock}")]);
    let row = |name, replies, code, requests| case(name, args, &[], replies, fresh, code, requests);
    vec![
        with(
            cell(),
            row("on, on the cell socket", vec![ok(ON)], on_code, 1),
        ),
        with(
            cell(),
            row("off, on the cell socket", vec![ok(OFF)], off_code, 1),
        ),
        with(
            tcp(),
            row(
                "on, over TCP (the socket is gone)",
                vec![ok(ON)],
                on_code,
                1,
            ),
        ),
        with(tcp(), row("off, over TCP", vec![ok(OFF)], off_code, 1)),
        with(
            with_passport(&[]),
            row("on, over TCP (no socket set)", vec![ok(ON)], on_code, 1),
        ),
        with(
            cell(),
            row(
                "no answer in time (curl 28): off",
                vec![Reply::Hang(Duration::from_secs(3))],
                off_code,
                1,
            ),
        ),
        with(
            tcp(),
            row(
                "no answer in time over TCP: off",
                vec![Reply::Hang(Duration::from_secs(3))],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "closed with nothing (52): off",
                vec![Reply::Close],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row("a reset (56): off", vec![Reply::Reset], off_code, 1),
        ),
        with(
            tcp(),
            row("a reset over TCP: off", vec![Reply::Reset], off_code, 1),
        ),
        with(
            cell(),
            row(
                "an empty 200: off",
                vec![Reply::Answer(200, String::new())],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "not HTTP (000): off",
                vec![Reply::Raw(GARBAGE.to_vec())],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "a 500 that says enabled: off",
                vec![Reply::Answer(500, ON.to_string())],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "a refused passport: off, and never asked again",
                vec![heal(rig)],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "403: off",
                vec![Reply::Answer(403, REFUSED.to_string())],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row(
                "enabled is the string \"true\": off",
                vec![ok(r#"{"enabled":"true"}"#)],
                off_code,
                1,
            ),
        ),
        with(
            cell(),
            row("not JSON: off", vec![ok("enabled: true")], off_code, 1),
        ),
        with(
            cell(),
            row(
                "a key twice: the last one counts",
                vec![ok(r#"{"enabled":false,"enabled":true}"#)],
                on_code,
                1,
            ),
        ),
        with(
            cell(),
            Case {
                no_tmpdir: true,
                ..row(
                    "TMPDIR not there (the shell's 75 elsewhere): still on",
                    vec![ok(ON)],
                    on_code,
                    1,
                )
            },
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            row(
                "nothing listens on the socket: the tries, then off",
                vec![],
                off_code,
                0,
            ),
        ),
        with(
            vec![
                ("K2_PORT", dead_port()),
                ("K2_HOOK_TOKEN", super::parity::PASSPORT.to_string()),
                ("K2_HOOK_SOCK", "{nosock}".to_string()),
            ],
            row(
                "nothing on the port: the tries, then off",
                vec![],
                off_code,
                0,
            ),
        ),
        with(
            vec![
                ("K2_HOOK_TOKEN", super::parity::PASSPORT.to_string()),
                ("K2_HOOK_SOCK", "{nosock}".to_string()),
            ],
            row(NO_SOCKET_NO_PORT, vec![], off_code, 0),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            Case {
                setup: fresh_healed,
                ..row(
                    "a re-minted passport is the one sent",
                    vec![ok(ON)],
                    on_code,
                    1,
                )
            },
        ),
        with(
            vec![
                ("K2_HOOK_SOCK", "{sock}".to_string()),
                ("K2_PORT", PORT.to_string()),
            ],
            Case {
                setup: fresh_owner,
                ..row("no passport: nothing asked, off", vec![], off_code, 0)
            },
        ),
        with(
            cell()
                .into_iter()
                .chain([("PYTHONPATH", "/nonexistent".to_string())])
                .collect(),
            Case {
                compare: Compare::Shell,
                no_python3: NoPython3::Own,
                ..row("a PYTHON setting: the shell's", vec![ok(ON)], on_code, 1)
            },
        ),
        with(
            cell()
                .into_iter()
                .chain([("LANG", "de_DE.ISO8859-1".to_string())])
                .collect(),
            Case {
                compare: Compare::Shell,
                no_python3: NoPython3::Own,
                ..row(
                    "a locale that is not UTF-8: the shell's",
                    vec![ok(ON)],
                    on_code,
                    1,
                )
            },
        ),
        with(
            tcp()
                .into_iter()
                .chain([("K2_HOST", "localhost".to_string())])
                .collect(),
            Case {
                compare: Compare::Shell,
                ..row(
                    "a host that is a name, over TCP: the shell's",
                    vec![ok(ON)],
                    on_code,
                    1,
                )
            },
        ),
        // The probe has one try (`_K2_CURL_TRIES=1`): a refused connect is
        // not tried again at the port the daemon moved to, so nothing is
        // asked there: off.
        with(
            vec![
                ("K2_PORT", dead_port()),
                ("K2_HOOK_TOKEN", super::parity::PASSPORT.to_string()),
            ],
            Case {
                setup: moved,
                ..row(
                    "the daemon moved its port: one try, not asked there, off",
                    vec![],
                    off_code,
                    0,
                )
            },
        ),
    ]
}

/// [`rows`] for a page that asks the feedback probe first, as the daily
/// tier does: each row's answers after one that shows send-feedback
/// ([`AGENTS_ON`]), and one more request where the row's reaches the
/// daemon (the two probes ask the same way, each on its own connection).
pub fn daily_rows(
    rig: &Rig,
    args: &'static [&'static str],
    on_code: i32,
    off_code: i32,
) -> Vec<Case> {
    rows(rig, args, on_code, off_code)
        .into_iter()
        .map(|c| {
            let replies = if c.requests == 0 {
                c.replies.clone()
            } else {
                std::iter::once(ok(AGENTS_ON))
                    .chain(c.replies.iter().cloned())
                    .collect()
            };
            Case {
                replies,
                requests: c.requests + usize::from(c.requests > 0),
                ..c
            }
        })
        .collect()
}

/// The rows that answer otherwise with no `python3` ([`NoPython3::Own`]):
/// the binary sends what the row says and prints the page as it does for
/// "on, on the cell socket" with `python3`, the daemon's "on" read as on.
pub fn check_without_python3(
    rig: &Rig,
    args: &'static [&'static str],
    on_code: i32,
    off_code: i32,
) {
    own_without_python3(rig, args, on_code, rows(rig, args, on_code, off_code));
}

/// [`check_without_python3`] for [`daily_rows`].
pub fn check_daily_without_python3(
    rig: &Rig,
    args: &'static [&'static str],
    on_code: i32,
    off_code: i32,
) {
    own_without_python3(rig, args, on_code, daily_rows(rig, args, on_code, off_code));
}

fn own_without_python3(rig: &Rig, args: &'static [&'static str], on_code: i32, all: Vec<Case>) {
    let on = all
        .iter()
        .find(|c| c.name == "on, on the cell socket")
        .expect("the row with cards on");
    let (want, _) = rig.binary(on, false);
    assert_eq!(want.code, Some(on_code), "{args:?}: {want:?}");
    let own: Vec<&Case> = all
        .iter()
        .filter(|c| c.no_python3 == NoPython3::Own)
        .collect();
    assert_eq!(own.len(), 2, "{args:?}: the rows that differ");
    for c in own {
        let (got, log) = rig.binary(c, true);
        let name = c.name;
        println!("== {name} {args:?} (no python3)\n-- rust: {got:?}");
        assert_eq!(rig.stub().left(), 0, "{name}: answers left unused");
        assert_eq!(log.len(), c.requests, "{name}: the requests {log:?}");
        assert_eq!(
            (got.code, got.stdout.as_str(), got.stderr.as_str()),
            (want.code, want.stdout.as_str(), want.stderr.as_str()),
            "{name} {args:?}: with no python3, the page with cards on"
        );
    }
}

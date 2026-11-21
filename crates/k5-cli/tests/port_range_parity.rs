//! A port setting or saved port outside 1 to 65535, on the parity rig
//! ([`common::parity`]): `k5-cli` stops every command as the shell's
//! prelude does (`_k2_port_ok`, `_k2_port_refuse` in `cli/src/prelude.sh`):
//! the same line on stderr, exit 2, nothing on stdout and no request.
//! Each row runs on both sides (the rig's check), then the binary's line
//! is checked word for word.
//!
//! - The setting (`K2_PORT`, `K5_PORT` over it, `K2SO_PORT`): above
//!   65535, 0, a leading zero, a sign, not a number.
//! - The saved `heartbeat.port` with no setting; a setting that is a port
//!   wins over a saved one that isn't.
//! - Every verb whose parity rows send a port that is no port (`41o0`,
//!   `1x`), `--version`, `help` and a verb the binary hands to the shell
//!   (`preset list`).

#![cfg(unix)]

mod common;

use common::parity::*;

const WHO: &str = r#"{"workspace":"sales","role":"sidecar","address":"sales/reviewer","primary":"sales","session":"s1","path":"/w/sales","siblings":{"live":2,"others":[]}}"#;

fn bad_port_on_disk(r: &Rig) {
    r.owner_on_disk();
    std::fs::write(r.k2home.join("heartbeat.port"), "70000\n").unwrap();
}

fn port_zero_on_disk(r: &Rig) {
    r.owner_on_disk();
    std::fs::write(r.k2home.join("heartbeat.port"), "0\n").unwrap();
}

/// The line `_k2_port_refuse` prints.
fn line(what: &str, value: &str) -> String {
    format!("Error: {what} must be a number from 1 to 65535 (got: {value})\n")
}

/// The row on both sides, then the binary's run alone: exit 2, `want`
/// on stderr, stdout empty, no request.
fn refused(rig: &Rig, c: &Case, want: &str) {
    rig.check(c);
    rig.prepare(Side::Rust, c);
    let out = rig.run(false, c);
    let sent = rig.read(Side::Rust, true, c.name);
    assert_eq!(
        (
            out.code,
            out.stdout.as_str(),
            out.stderr.as_str(),
            sent.len()
        ),
        (Some(2), "", want, 0),
        "{}: the refusal",
        c.name
    );
}

/// The setting's and the saved port's rows for a verb with `args`.
fn rows(rig: &Rig, args: &'static [&'static str]) {
    for (name, env, value) in [
        ("K2_PORT above 65535", "K2_PORT", "70000"),
        ("K2_PORT 0", "K2_PORT", "0"),
        ("K2_PORT with a leading zero", "K2_PORT", "080"),
        ("K2_PORT with a sign", "K2_PORT", "+80"),
        ("K2_PORT not a number", "K2_PORT", "41o0"),
        ("K2SO_PORT above 65535", "K2SO_PORT", "99999"),
    ] {
        let c = case(name, args, &[(env, value)], vec![], owner_only, 2, 0);
        refused(rig, &c, &line(env, value));
    }
    let c = case(
        "K5_PORT over a K2_PORT that is a port",
        args,
        &[("K5_PORT", "131072"), ("K2_PORT", "4100")],
        vec![],
        owner_only,
        2,
        0,
    );
    refused(rig, &c, &line("K5_PORT", "131072"));
    let saved = format!("the port in {}/heartbeat.port", rig.k2home.display());
    let c = case(
        "heartbeat.port above 65535",
        args,
        &[],
        vec![],
        bad_port_on_disk,
        2,
        0,
    );
    refused(rig, &c, &line(&saved, "70000"));
    let c = case(
        "heartbeat.port 0",
        args,
        &[],
        vec![],
        port_zero_on_disk,
        2,
        0,
    );
    refused(rig, &c, &line(&saved, "0"));
}

#[test]
fn whoami_stops_on_a_port_that_isn_t_one() {
    let rig = Rig::new("whoami", "port");
    rows(&rig, &[]);
    // The setting wins: a saved port that isn't one is not read.
    rig.check(&case(
        "a setting that is a port over a saved one that isn't",
        &["--json"],
        &[("K2_PORT", PORT)],
        vec![ok(WHO)],
        bad_port_on_disk,
        0,
        1,
    ));
}

#[test]
fn version_help_and_a_shell_verb_stop_on_a_port_that_isn_t_one() {
    for (verb, args) in [
        ("--version", &[][..]),
        ("help", &[][..]),
        ("preset", &["list"][..]),
    ] {
        let rig = Rig::new(verb, "port");
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rows(&rig, args);
    }
}

/// Each verb whose own parity rows send `41o0` or `1x`: the line, word for
/// word, before anything else.
#[test]
fn the_native_families_stop_on_a_port_that_isn_t_one() {
    for (verb, args, value) in [
        ("activity", &[][..], "41o0"),
        ("connections", &["list"][..], "41o0"),
        ("connections", &["add", "beta"][..], "41o0"),
        ("connect", &["status"][..], "41o0"),
        ("inbox", &[][..], "41o0"),
        ("workspace", &["list"][..], "41o0"),
        ("workspace", &["create", "x"][..], "41o0"),
        ("feedback", &["list"][..], "41o0"),
        ("respond", &["x"][..], "41o0"),
        ("done", &[][..], "41o0"),
        ("setup", &[][..], "41o0"),
        ("sessions", &[][..], "41o0"),
        ("read", &["acme"][..], "41o0"),
        ("update", &["--cli"][..], "1x"),
        ("update", &["--app"][..], "1x"),
        ("api-key", &["list"][..], "41o0"),
        ("api", &["on"][..], "41o0"),
        ("cert", &["list"][..], "41o0"),
        ("compute", &["cancel", "j1"][..], "41o0"),
        ("compute", &["jobs"][..], "41o0"),
        ("focus-group", &["list"][..], "41o0"),
        ("glossary", &["x"][..], "41o0"),
        ("organization", &["list"][..], "41o0"),
        ("permission", &["status"][..], "41o0"),
        ("remind", &["list"][..], "41o0"),
        ("skin-token", &["list"][..], "41o0"),
        ("skin", &["role", "list"][..], "41o0"),
        ("wiki", &["status"][..], "41o0"),
    ] {
        let rig = Rig::new(verb, "port");
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        let c = case(
            "a port that is no port",
            args,
            &[("K2_PORT", value)],
            vec![],
            owner_only,
            2,
            0,
        );
        refused(&rig, &c, &line("K2_PORT", value));
    }
}

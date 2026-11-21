//! The edge shapes of the native bar (`tests/data/native_bar_shapes.tsv`,
//! class E) that the families answer natively, against the shell CLI's,
//! on the parity rig ([`common::parity`]).
//!
//! - A value flag as the last word: the shell's `_need_value` line
//!   (`Error: <flag> takes a value`, exit 2) on both sides, the binary's
//!   from [`k5_cli::out::need_value`], nothing sent by either.

#![cfg(unix)]

mod common;

use common::parity::*;

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The row for `args` (after the rig's verb), whose last word is `flag`:
/// bash's `_need_value` line, then the binary's, with and without
/// `python3` on `PATH`.
fn needs_value(rig: &Rig, args: &'static [&'static str], flag: &str) {
    let name = leak(format!("{} (the last word)", args.join(" ")));
    let c = case(name, args, &[], vec![], owner_and_port, 2, 0);
    rig.prepare(Side::Bash, &c);
    let b = rig.run(true, &c);
    let sent = rig.read(Side::Bash, false, name);
    let want = format!("Error: {flag} takes a value\n");
    assert_eq!((b.code, b.signal), (Some(2), None), "{name}: bash: {b:?}");
    assert_eq!(b.stdout, "", "{name}: bash's stdout");
    assert_eq!(b.stderr, want, "{name}: bash's stderr");
    assert!(sent.is_empty(), "{name}: bash sent {} request(s)", sent.len());
    for without_python3 in [false, true] {
        let (r, sent) = rig.binary(&c, without_python3);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert_eq!(r.stdout, "", "{name}");
        assert_eq!(r.stderr, want, "{name} (no python3: {without_python3})");
    }
}

#[test]
fn commit_m_last() {
    let rig = Rig::new("commit", "d6");
    needs_value(&rig, &["-m"], "-m");
    needs_value(&rig, &["--merge", "--message"], "--message");
}

#[test]
fn terminal_value_flag_last() {
    let rig = Rig::new("terminal", "d6");
    for (args, flag) in [
        (&["spawn", "--title"][..], "--title"),
        (&["spawn", "--wait", "--command"], "--command"),
        (&["spawn", "--command", "ls", "--agent"], "--agent"),
        (&["spawn", "--cwd"], "--cwd"),
        (&["read", "t1", "--lines"], "--lines"),
    ] {
        needs_value(&rig, args, flag);
    }
}

#[test]
fn tunnel_value_flag_last() {
    let rig = Rig::new("tunnel", "d6");
    for (args, flag) in [
        (&["start", "--subdomain"][..], "--subdomain"),
        (&["enable", "--no-auto-start", "--subdomain"], "--subdomain"),
        (&["switch", "acme", "--token"], "--token"),
    ] {
        needs_value(&rig, args, flag);
    }
}

#[test]
fn read_value_flag_last() {
    let rig = Rig::new("read", "d6");
    for (args, flag) in [
        (&["w1", "--lines"][..], "--lines"),
        (&["w1", "--agent"], "--agent"),
        (&["w1@box.example", "--session"], "--session"),
        (&["w1", "--token"], "--token"),
    ] {
        needs_value(&rig, args, flag);
    }
}

#[test]
fn inbox_value_flag_last() {
    let rig = Rig::new("inbox", "d6");
    for (args, flag) in [
        (&["list", "--folder"][..], "--folder"),
        (&["--folder"], "--folder"),
        (&["projects", "--folder"], "--folder"),
        (&["compose", "--title"], "--title"),
        (&["compose", "--title", "t", "--body"], "--body"),
        (&["compose", "--priority"], "--priority"),
        (&["compose", "--source"], "--source"),
        (&["compose", "--from"], "--from"),
        (&["compose", "--summary"], "--summary"),
    ] {
        needs_value(&rig, args, flag);
    }
}

/// A session with no passport stops `inbox` (an `id` tool) before its
/// words are read, as the shell's router does.
#[test]
fn inbox_value_flag_last_in_a_session_without_a_passport() {
    let rig = Rig::new("inbox", "d6session");
    rig.check(&Case {
        env: vec![("K2_HOOK_SOCK", "{sock}".to_string())],
        ..case(
            "list --folder, a session with no passport",
            &["list", "--folder"],
            &[],
            vec![],
            owner_and_port,
            3,
            0,
        )
    });
}

#[test]
fn root_helper_from_last_is_bash_s_silent_exit_1() {
    let rig = Rig::new("root-helper", "shift");
    for (name, args) in [
        ("update --from (the last word)", &["update", "--from"][..]),
        ("update --json --from", &["update", "--json", "--from"]),
        ("status --from", &["status", "--from"]),
    ] {
        rig.check(&case(name, args, &[], vec![], owner_and_port, 1, 0));
    }
    // An unknown word before it stops first.
    rig.check(&case(
        "update --x --from",
        &["update", "--x", "--from"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    ));
    // No port and no token: the gate speaks first.
    rig.check(&case(
        "update --from, no daemon",
        &["update", "--from"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
}

#[test]
fn study_permissions_without_a_passport() {
    let rig = Rig::new("study", "noperm-edge");
    for (name, args) in [
        ("permissions, no passport", &["permissions"][..]),
        ("permissions --json, no passport", &["permissions", "--json"]),
        ("--json permissions, no passport", &["--json", "permissions"]),
    ] {
        rig.check(&case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
}

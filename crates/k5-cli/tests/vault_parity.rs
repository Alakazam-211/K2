//! `k5-cli vault` against the shell CLI's `cli/k2 vault`, on the vault
//! rig ([`common::vault`]): what `cmd_vault` answers before any verb
//! (an unknown verb, the gates), and the rig's own checks. The verbs'
//! own cases are in `vault_verbs_parity.rs`, `vault_handoff_parity.rs`
//! and `vault_tools_parity.rs`.

#![cfg(unix)]

mod common;

use common::parity::OWNER;
use common::vault::{traceback_shape, VCase, VRig};

#[test]
fn an_unknown_verb_says_so() {
    let rig = VRig::new();
    for (name, args) in [
        ("an unknown verb", &["nope"][..]),
        ("a word for run", &["exec", "x"][..]),
        ("env", &["env"][..]),
        ("inject", &["inject", "--json"][..]),
        ("a flag first", &["--json"][..]),
    ] {
        let mut c = VCase::new(name, args);
        c.code = 2;
        rig.check(&c);
    }
}

#[test]
fn the_gates_come_first() {
    let rig = VRig::new();
    // No port and no token: the connection gate.
    let mut c = VCase::new("no port, no token", &["nope"]);
    c.owner_on_disk = false;
    c.code = 1;
    rig.check(&c);
    // A session with no passport: the id tool's refusal.
    let mut c = VCase::new("a session without a passport", &["nope"]);
    c.env = vec![("K2_HOOK_SOCK", "{sock}".to_string())];
    c.code = 3;
    rig.check(&c);
}

/// The rig finds a secret a child saw.
#[test]
fn the_rig_sees_a_secret_on_a_child_s_argv() {
    let rig = VRig::new();
    let mut c = VCase::new("the spy", &["nope"]);
    c.secrets = vec![OWNER];
    // The binary's run leaves the spy log; write one as a child would.
    std::fs::write(&rig.spy_log, format!("argv: curl -H {OWNER}\n")).unwrap();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.no_secret_to_a_child(&c)
    }));
    assert!(caught.is_err(), "the rig missed a secret on a child's argv");
}

#[test]
fn a_traceback_compares_by_its_shape() {
    let a = "x\nTraceback (most recent call last):\n  File \"<string>\", line 3, in <module>\nKeyError: 'a'\ny\n";
    let b = "x\nTraceback (most recent call last):\n  File \"<string>\", line 9, in <module>\n  File \"/usr/lib/json\", line 1\nKeyError: 'b'\ny\n";
    assert_eq!(traceback_shape(a), traceback_shape(b));
    assert_eq!(traceback_shape(a), "x\n<traceback>\nKeyError\ny\n");
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on the line of
/// a verb that changes something prints help and sends nothing, on both
/// sides (the binary hands these lines to the shell).
///
/// A bash bug this row set leaves out, reported: `k2 vault fetch URL
/// --secret A --json --help` reads `--help` as `--json`'s body and sends
/// the fetch (`cli/src/vault.sh`, `_vault_fetch`'s `--json` and `-d`
/// arms); `-H`, `-o`, `-X` and `--dst` take it as their value too.
#[test]
fn help_never_acts() {
    let rig = VRig::new();
    for args in [
        &["set", "API", "--help"][..],
        &["set", "-h", "API", "--for", "a.com"],
        &["rotate", "API", "-h"],
        &["hosts", "API", "--add", "a", "--help"],
        &["sign", "API", "ovh", "-h"],
        &["delivery", "API", "env", "--help"],
        &["agent-limit", "API", "--burst", "5", "-h"],
        &["grant", "API", "--to", "ws:a", "--help"],
        &["revoke", "--grant", "g1", "-h"],
        &["open", "--help"],
        &["disable", "API", "-h"],
        &["enable", "API", "--help"],
        &["destroy", "API", "--yes", "--help"],
        &["get", "API", "-h"],
        &["file", "API", "--help"],
        &["env-file", "A", "-h"],
        &["proxy-url", "API", "--help"],
        &["sync", "-h"],
        &["placement", "add", "API", "--where", "h:/x", "--help"],
        &["placement", "rm", "p1", "-h"],
        &["send", "API", "--to", "bob", "--help"],
        &["incoming", "accept", "d1", "-h"],
        &["unsend", "d1", "--help"],
        &["ask", "bob", "API", "-h"],
        &["accept-from", "add", "bob", "--workspace", "w", "--help"],
        &[
            "send-allow",
            "remove",
            "A",
            "--to",
            "b",
            "--workspace",
            "w",
            "-h",
        ],
        &["reveal", "API", "--help"],
        &["recovery", "rotate", "-h"],
        &["confirm-password", "set", "--help"],
        &["kek", "rotate", "-h"],
        &["rekey", "--help"],
        &["rollback", "API", "--to", "1", "-h"],
        &["backup", "/tmp", "--help"],
        &["restore", "--from", "/tmp", "-h"],
        &["carry", "export", "--help"],
        &["import", ".env", "-h"],
        &["service", "web", "clean", "--help"],
        &["systemd-cred", "API", "--unit", "u", "-h"],
        &["fetch", "-h"],
        &["run", "--help"],
        &["send", "API", "--to", "bob", "--stdin", "-h"],
        &[
            "send-allow",
            "add",
            "A",
            "--to",
            "b",
            "--workspace",
            "w",
            "--help",
        ],
        &["recovery", "show-once", "--help"],
        &["backup", "key", "age1x", "-h"],
        &["carry", "import", "f.age", "--apply", "--help"],
        &["systemd-cred", "API", "--replace", "--help"],
    ] {
        let mut c = VCase::new("help", args);
        c.handed = true;
        rig.check(&c);
    }
}

/// Help never acts, even where a value flag would take the
/// word: `_vault_fetch` checks every word first (`_wants_help`), so
/// `--json --help` prints the fetch help, exit 0, and sends NO request
/// (it used to send the fetch with `--help` as its body). The binary
/// hands the line to the shell; both sides must send nothing.
#[test]
fn fetch_json_then_help_sends_nothing() {
    let rig = VRig::new();
    for args in [
        &[
            "fetch",
            "https://api.example.com/x",
            "--secret",
            "API",
            "--json",
            "--help",
        ][..],
        &[
            "fetch",
            "https://api.example.com/x",
            "--secret",
            "API",
            "-d",
            "-h",
        ],
        &[
            "fetch",
            "https://api.example.com/x",
            "-H",
            "--help",
            "--secret",
            "API",
        ],
    ] {
        let mut c = VCase::new("fetch: help where a value goes, nothing sent", args);
        c.handed = true;
        rig.check(&c);
        let bash = rig.run(true, false, &c);
        assert_eq!(
            (bash.out.code, bash.daemon.len(), bash.vault.len()),
            (Some(0), 0, 0),
            "{bash:#?}"
        );
        assert!(!bash.out.stdout.is_empty(), "no help printed: {bash:#?}");
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = VRig::new();
    let cli = k2_client::brand::CLI_NAME;
    for (args, want) in [
        (&["nope"][..], format!("usage: unknown vault verb 'nope' ({cli} vault --help)\n")),
        (&["show"][..], format!("usage: {cli} vault show NAME [--scope <s>]\n")),
        (&["fetch", "-s", "A"][..], format!("usage: {cli} vault fetch --secret NAME URL\n")),
        (&["send", "A"][..], format!("usage: {cli} vault send NAME --to <agent>|<agent>::<host> [--note …]\n")),
        (&["placements", "--scope", "server"][..], format!("usage: --scope picks which secret NAME is: {cli} vault placements NAME --scope <s>\n")),
        (&["run", "--secret", "A"][..], format!("usage: {cli} vault run [--secret NAME[=ENV_NAME]]... [--env-file .env.k2] [--scope workspace|server|session] [--no-mask] -- cmd [args…]\nPut -- before the command: {cli} vault run --secret API_KEY -- node app.js\n")),
    ] {
        let mut c = VCase::new("brand", args);
        c.code = 2;
        let ran = rig.run(false, false, &c);
        assert_eq!(ran.out.stderr, want, "{args:?}");
        assert_eq!(ran.out.code, Some(2), "{args:?}");
        if cli != "k2" {
            assert!(!ran.out.stderr.contains("k2 ") && !ran.out.stderr.contains("k5 "));
        }
    }
}

/// `vault` alone, `vault help` and an empty verb: the top page, nothing
/// sent to the daemon or the vault socket, and no connection gate (the
/// shell skips it for these), but the no-owner refusal. The `-h` lines
/// beside them are the front's (a page, or the shell): they send nothing
/// either, past the same gates.
#[test]
fn vault_alone_sends_nothing() {
    let rig = VRig::new();
    let page = |name: &'static str, args: &[&'static str], code: i32| {
        let mut c = VCase::new(name, args);
        c.code = code;
        c
    };
    let handed = |mut c: VCase| {
        c.handed = true;
        c
    };
    // covers: vault
    let mut alone_no_owner = page("vault alone, no owner and no port", &[], 0);
    alone_no_owner.owner_on_disk = false;
    let mut alone_no_fallback = page("vault alone under the no-owner rule", &[], 3);
    alone_no_fallback.env = vec![("K2_NO_OWNER_FALLBACK", "1".to_string())];
    let mut help_no_fallback = page("help under the no-owner rule", &["help"], 3);
    help_no_fallback.env = vec![("K2_NO_OWNER_FALLBACK", "1".to_string())];
    let mut alone_no_passport = page("vault alone in a session with no passport", &[], 3);
    alone_no_passport.env = vec![("K2_HOOK_SOCK", "{sock}".to_string())];
    let mut h_no_fallback = page("-h under the no-owner rule", &["list", "--json", "-h"], 0);
    h_no_fallback.env = vec![("K2_NO_OWNER_FALLBACK", "1".to_string())];
    let mut h_no_owner = page("-h, no owner and no port", &["set", "API", "-h"], 0);
    h_no_owner.owner_on_disk = false;
    for c in [
        page("vault alone", &[], 0),
        alone_no_owner,
        alone_no_fallback,
        alone_no_passport,
        page("help", &["help"], 0),
        page("help, more words", &["help", "set", "--json"], 0),
        page("an empty verb", &[""], 0),
        help_no_fallback,
        handed(page("--help, more words", &["--help", "list"], 0)),
        handed(h_no_fallback),
        handed(h_no_owner),
        handed(page("an unknown verb with --help", &["exec", "--help"], 2)),
    ] {
        rig.check(&c);
    }
}

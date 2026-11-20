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

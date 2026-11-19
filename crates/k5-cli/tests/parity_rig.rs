//! The parity rig ([`common::parity`]) fails a case whose requests cross:
//! one side's CLI talking to the other side's stub. Each side has its own
//! stub (its own port, socket and log), so a request can only land in
//! the log of the side that sent it; one found anywhere else fails the
//! case, naming it as crossed.

#![cfg(unix)]

mod common;

use common::parity::*;

const UPDATED: &str = r#"{"ok":true,"agent":"Acme"}"#;

/// A case with two requests, from `workspace_agent_parity.rs`.
fn update() -> Case {
    case(
        "the agent's name from the daemon",
        &["update", "--field", "role", "--value", "v"],
        &[],
        vec![ok(r#"{"display_name":"Acme Lead"}"#), ok(UPDATED)],
        owner_and_port,
        0,
        2,
    )
}

#[test]
fn the_case_passes_when_nothing_crosses() {
    Rig::new("workspace", "nocross").check(&update());
}

#[test]
#[should_panic(expected = "crossed")]
fn bash_talking_to_the_binary_s_stub_fails_the_case() {
    let rig = Rig::new("workspace", "crossb");
    rig.cross(Side::Bash, Side::Rust);
    rig.check(&update());
}

#[test]
#[should_panic(expected = "crossed")]
fn the_binary_talking_to_bash_s_stub_fails_the_case() {
    let rig = Rig::new("workspace", "crossr");
    rig.cross(Side::Rust, Side::Bash);
    rig.check(&update());
}

//! The admin verbs' words and readers, against what the shell's
//! `cmd_compute` and its Python do with them.

use super::super::writes::Words;
use super::{cache, moves, node, retier, switch};
use crate::exit::Exit;
use k2_client::cli_name;

fn ask<T: std::fmt::Debug>(w: Words<T>) -> T {
    match w {
        Words::Ask(a) => a,
        other => panic!("expected an ask, got {other:?}"),
    }
}

#[test]
fn revoke_words() {
    let a = ask(switch::parse("revoke", &["--workspace", "acme", "mini"]));
    assert_eq!(
        a,
        switch::Ask::Revoke {
            node: "mini".into(),
            workspace: "acme".into()
        }
    );
    // `--json` is a node name to the shell.
    let a = ask(switch::parse(
        "revoke",
        &["mini", "--workspace", "acme", "--json"],
    ));
    assert_eq!(
        a,
        switch::Ask::Revoke {
            node: "--json".into(),
            workspace: "acme".into()
        }
    );
    assert_eq!(
        switch::parse("revoke", &["mini"]),
        Words::Usage(switch::REVOKE_USAGE.into())
    );
    assert_eq!(
        switch::parse("revoke", &["mini", "--workspace"]),
        Words::ShiftFailed
    );
}

#[test]
fn toggle_words() {
    let a = ask(switch::parse("access", &["on", "--all-machines"]));
    assert_eq!(
        a,
        switch::Ask::Toggle {
            toggle: switch::Toggle::Compute,
            on: true,
            workspace: String::new()
        }
    );
    // Only `access` takes --all-machines.
    assert_eq!(
        switch::parse("operate", &["on", "--all-machines"]),
        Words::Usage(switch::OPERATE_USAGE.into())
    );
    // `status` is no action of `access`: the usage, after the flags.
    assert_eq!(
        switch::parse("access", &[]),
        Words::Usage(switch::ACCESS_USAGE.into())
    );
    assert_eq!(
        switch::parse("manage", &["off", "--workspace"]),
        Words::ShiftFailed
    );
    assert_eq!(
        switch::parse("manage", &["maybe", "--workspace"]),
        Words::ShiftFailed
    );
    assert_eq!(
        switch::parse("manage", &["--workspace", "acme"]),
        Words::Usage(switch::MANAGE_USAGE.into())
    );
}

#[test]
fn retier_words() {
    let a = ask(retier::parse(&["abc", "--standard"]));
    assert_eq!(a.tier, "normal");
    let a = ask(retier::parse(&["--tier", "standard", "abc", "--json"]));
    assert_eq!((a.tier.as_str(), a.json), ("normal", true));
    assert_eq!(
        retier::parse(&["abc", "--priority"]),
        Words::Usage(format!(
            "usage: --priority needs a one-line reason: {} compute retier abc --priority --why \"release gate for 0.46.6\"",
            cli_name!()
        ))
    );
    assert_eq!(
        retier::parse(&["abc"]),
        Words::Usage(format!("{}   (--priority needs --why)", retier::USAGE))
    );
    assert_eq!(
        retier::parse(&["--batch"]),
        Words::Usage(retier::USAGE.to_string())
    );
    assert_eq!(retier::parse(&["abc", "--why"]), Words::ShiftFailed);
}

#[test]
fn readers() {
    let s = switch::revoke_reader(
        br#"{"removed":true,"node":"mini","workspace":"acme","note":"done."}"#,
    );
    assert_eq!(s.stdout, "revoked mini for acme. done.\n");
    let s = switch::revoke_reader(br#"{"removed":0,"note":"x"}"#);
    assert_eq!(s.stdout, "no grant to revoke. x\n");
    let s = switch::revoke_reader(br#"{"removed":false,"note":3}"#);
    assert_eq!(s.exit, Exit::Error);
    assert!(s
        .stderr
        .ends_with("TypeError: can only concatenate str (not \"int\") to str\n"));

    let s = switch::placement_reader(br#"{"computePlacement":"v1"}"#);
    assert_eq!(
        s.stdout,
        "Compute placement: v1 on this server (the 0.46.5 order: warm slot first, then free CPU)\n"
    );
    let s = switch::placement_reader(br#"{"computePlacement":["v1"]}"#);
    assert!(s.stderr.ends_with("TypeError: unhashable type: 'list'\n"));

    let s = switch::access_reader(
        br#"{"agentsCanUseCompute":true,"workspace":"acme","granted":["a"],"kept":["b"]}"#,
    );
    assert_eq!(s.stdout, "Allow compute nodes: on for acme: a, b\n");
    let s = switch::access_reader(
        br#"{"agentsCanUseCompute":false,"workspace":"acme","grantsRemoved":2}"#,
    );
    assert_eq!(
        s.stdout,
        "Allow compute nodes: off for acme (2 grants removed)\n"
    );
    let s = switch::access_reader(br#"{"agentsCanUseCompute":true,"workspace":"acme"}"#);
    assert_eq!(
        s.stdout,
        "Allow compute nodes: on for acme: no machines switched on yet (ones added later are granted)\n"
    );

    let s = retier::reader(
        br#"{"job":"abc","from":"normal","tier":"priority","asked":"ci","node":"mini","note":"moved up."}"#,
    );
    assert_eq!(
        s.stdout,
        "job abc: Standard → Priority on mini (asked CI: this server's Priority doesn't count there)\n  moved up.\n"
    );
}

fn usage<T: std::fmt::Debug>(w: Words<T>) -> String {
    match w {
        Words::Usage(line) => line,
        other => panic!("expected a usage line, got {other:?}"),
    }
}

/// Under any brand (a `K2_BRAND_DIR=brand/xy` build too) the lines name
/// the command by the brand's name, never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let mut lines: Vec<String> = [
        switch::REVOKE_USAGE,
        switch::ACCESS_USAGE,
        switch::OPERATE_USAGE,
        switch::MANAGE_USAGE,
        retier::USAGE,
        moves::USAGE,
        cache::USAGE,
        cache::CLEAR_USAGE,
        cache::LINEAGE_LIST_USAGE,
        node::USAGE,
        node::ADD_USAGE,
        node::CONFIRM_USAGE,
        node::REMOVE_USAGE,
        node::RETIRE_USAGE,
        node::RENAME_USAGE,
        node::PINNED_USAGE,
        node::ROUTE_USAGE,
        node::ALERTS_USAGE,
    ]
    .map(String::from)
    .to_vec();
    lines.push(usage(retier::parse(&["abc", "--priority"])));
    match node::parse(&["drain"]) {
        Some(w) => lines.push(usage(w)),
        None => panic!("node drain is native"),
    }
    let enrolled = node::reader(
        &node::Reader::Enroll,
        br#"{"name":"n","enroll":"c","install":{"linux":"l","macos":"m","servers":"s"},"confirm":"c"}"#,
    );
    assert!(enrolled.stdout.contains(&format!("({cli} vault set NAME")));
    assert!(enrolled
        .stdout
        .contains(&format!("Scripts: {cli} compute node add n --json")));
    if cli != "k2" {
        assert!(!enrolled.stdout.contains("k2 ") && !enrolled.stdout.contains("k5 "));
    }
    let moved = moves::reader(br#"{"warnings":["w"],"matched":0,"moved":[]}"#);
    lines.push(moved.stderr.clone());
    let listed = cache::reader(br#"{"nodes":[]}"#, false, 0);
    lines.push(listed.stderr.clone());
    for line in &lines {
        assert!(line.contains(&format!("{cli} compute")), "{line:?}");
        if cli != "k2" {
            assert!(!line.contains("k2 ") && !line.contains("k5 "), "{line:?}");
            assert!(!line.contains("[k2 ") && !line.contains("[k5 "), "{line:?}");
        }
    }
}

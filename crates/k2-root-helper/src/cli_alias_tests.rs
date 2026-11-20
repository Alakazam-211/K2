//! `helper cli-refresh` and the brand's retired aliases in
//! `/usr/local/bin`, over the temp root and fakes of `tests.rs`. The real
//! verb code runs with a hand-made brand table (`CliNames`), so these hold
//! for a helper built as any brand; this crate builds with the default one.

use super::*;
use k2_root_proto::verbs::Verb;

use crate::core::{cli_refresh_with, CliNames, CLI_CONTENT};

/// A helper built as `xy`: no aliases.
const XY: CliNames = CliNames { name: "xy", aliases: &[] };
/// A helper built as `k5`, retiring `k2` with a notice, `k2so` silent.
const K5: CliNames = CliNames {
    name: "k5",
    aliases: &[("k2", AliasMode::Notice), ("k2so", AliasMode::Silent)],
};
/// The same, with `k2` in mode tombstone.
const K5_TOMBSTONE: CliNames = CliNames {
    name: "k5",
    aliases: &[("k2", AliasMode::Tombstone), ("k2so", AliasMode::Silent)],
};

fn bin(t: &T, name: &str) -> PathBuf {
    t.p(&format!("{CLI_DIR}/{name}"))
}

/// A box with a stale root-installed CLI under `own`'s name.
fn box_for(own: &str) -> T {
    let t = box_root();
    let p = bin(&t, own);
    std::fs::write(&p, STALE_CLI).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    t
}

fn daemon() -> crate::caller::Caller {
    crate::caller::Caller { pid: DAEMON_PID, uid: Some(K2_UID), user: Some("k2".into()), is_daemon: true, why_not: None }
}

fn refresh(t: &T, names: &CliNames) -> serde_json::Value {
    cli_refresh_with(&t.ctx, &daemon(), names).unwrap()
}

fn link(t: &T, name: &str, target: &str) {
    symlink(target, bin(t, name)).unwrap();
}

fn target(t: &T, name: &str) -> String {
    let p = bin(t, name);
    std::fs::read_link(&p).unwrap_or_else(|e| panic!("{} must be a link: {e}", p.display())).to_string_lossy().into_owned()
}

/// An `xy` helper has no aliases: links and files under other names, even
/// one that points at `xy` itself, are not its to tend.
#[test]
fn an_xy_helper_tends_no_alias_and_leaves_other_brands_files() {
    let t = box_for("xy");
    link(&t, "k2", "xy");
    link(&t, "k2so", "/opt/k2/cli/k2so");
    std::fs::write(bin(&t, "k5"), STALE_CLI).unwrap();
    let v = refresh(&t, &XY);
    assert_eq!(v["result"], "updated", "{v}");
    assert!(v.get("aliases").is_none(), "{v}");
    assert_eq!(std::fs::read_to_string(bin(&t, "xy")).unwrap(), CLI_CONTENT);
    assert_eq!(target(&t, "k2"), "xy");
    assert_eq!(target(&t, "k2so"), "/opt/k2/cli/k2so");
    assert_eq!(std::fs::read_to_string(bin(&t, "k5")).unwrap(), STALE_CLI);
}

/// A `k5` helper re-points a stale `k2` link (notice) at its own path,
/// once; a silent alias and another brand's link keep their targets.
#[test]
fn a_k5_helper_repoints_a_stale_notice_alias_link_once() {
    let t = box_for("k5");
    link(&t, "k2", "/opt/old/cli/k5");
    link(&t, "k2so", "/opt/old/cli/k5");
    link(&t, "xy", "/opt/old/cli/k5");
    let v = refresh(&t, &K5);
    assert_eq!(v["result"], "updated", "{v}");
    assert_eq!(
        v["aliases"],
        serde_json::json!([{"name": "k2", "result": "repointed", "from": "/opt/old/cli/k5"}])
    );
    assert_eq!(target(&t, "k2"), "/usr/local/bin/k5");
    assert_eq!(target(&t, "k2so"), "/opt/old/cli/k5", "a silent alias is left alone");
    assert_eq!(target(&t, "xy"), "/opt/old/cli/k5", "another brand's name is left alone");

    // Second pass: nothing to do.
    let v = refresh(&t, &K5);
    assert_eq!(v["result"], "unchanged", "{v}");
    assert_eq!(v["aliases"], serde_json::json!([{"name": "k2", "result": "current"}]));
    assert_eq!(target(&t, "k2"), "/usr/local/bin/k5");

    // A link to the command by its bare name, in the same folder, is current too.
    std::fs::remove_file(bin(&t, "k2")).unwrap();
    link(&t, "k2", "k5");
    assert_eq!(refresh(&t, &K5)["aliases"], serde_json::json!([{"name": "k2", "result": "current"}]));
    assert_eq!(target(&t, "k2"), "k5");
}

/// A `k5` helper removes a `k2` link (tombstone) to its command; the
/// command itself and a silent alias stay.
#[test]
fn a_k5_helper_removes_a_tombstone_alias_link() {
    for old in ["/usr/local/bin/k5", "k5", "/opt/old/cli/k5"] {
        let t = box_for("k5");
        link(&t, "k2", old);
        link(&t, "k2so", "/usr/local/bin/k5");
        let v = refresh(&t, &K5_TOMBSTONE);
        assert_eq!(v["result"], "updated", "{v}");
        assert_eq!(v["aliases"], serde_json::json!([{"name": "k2", "result": "removed", "from": old}]), "{old}");
        assert!(std::fs::symlink_metadata(bin(&t, "k2")).is_err(), "{old}: the link is gone");
        assert_eq!(std::fs::read_to_string(bin(&t, "k5")).unwrap(), CLI_CONTENT, "{old}");
        assert_eq!(target(&t, "k2so"), "/usr/local/bin/k5", "{old}: a silent alias is left alone");
        // Nothing at the name: nothing to say.
        assert!(refresh(&t, &K5_TOMBSTONE).get("aliases").is_none());
    }
}

/// At a retired alias's name, only a link to this brand's command is
/// tended: a regular file (an old CLI), a folder, or a link to another
/// brand's command keep what they are, in either mode.
#[test]
fn a_retired_name_that_is_not_a_link_to_the_command_is_left_alone() {
    for names in [K5, K5_TOMBSTONE] {
        let t = box_for("k5");
        std::fs::write(bin(&t, "k2"), STALE_CLI).unwrap();
        let v = refresh(&t, &names);
        assert_eq!(v["aliases"], serde_json::json!([{"name": "k2", "result": "skipped", "reason": "not a link"}]));
        assert_eq!(std::fs::read_to_string(bin(&t, "k2")).unwrap(), STALE_CLI, "{names:?}");

        std::fs::remove_file(bin(&t, "k2")).unwrap();
        std::fs::create_dir(bin(&t, "k2")).unwrap();
        let v = refresh(&t, &names);
        assert_eq!(v["aliases"], serde_json::json!([{"name": "k2", "result": "skipped", "reason": "not a link"}]));
        assert!(bin(&t, "k2").is_dir(), "{names:?}");

        std::fs::remove_dir(bin(&t, "k2")).unwrap();
        for other in ["/opt/k2/cli/k2", "/usr/local/bin/xy", "k5.old"] {
            link(&t, "k2", other);
            let v = refresh(&t, &names);
            assert_eq!(
                v["aliases"],
                serde_json::json!([{"name": "k2", "result": "skipped", "reason": "a link to another file"}]),
                "{names:?} {other}"
            );
            assert_eq!(target(&t, "k2"), other, "{names:?}");
            std::fs::remove_file(bin(&t, "k2")).unwrap();
        }
    }
}

/// When the command's own file is not the helper's to refresh (absent, a
/// link, not the K2 CLI), no alias is tended either.
#[test]
fn aliases_are_left_alone_when_the_command_is_not_ours() {
    for names in [K5, K5_TOMBSTONE] {
        let t = box_root();
        link(&t, "k2", "/usr/local/bin/k5");
        let v = refresh(&t, &names);
        assert_eq!((v["result"].as_str(), v["reason"].as_str()), (Some("skipped"), Some("absent")), "{v}");
        assert!(v.get("aliases").is_none(), "{v}");
        assert_eq!(target(&t, "k2"), "/usr/local/bin/k5");

        std::fs::write(bin(&t, "k5"), "#!/bin/bash\necho some other k5\n").unwrap();
        std::fs::set_permissions(bin(&t, "k5"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let v = refresh(&t, &names);
        assert_eq!(v["reason"], "not the K2 CLI", "{v}");
        assert!(v.get("aliases").is_none(), "{v}");
        assert_eq!(target(&t, "k2"), "/usr/local/bin/k5");
    }
}

/// What root did to a retired alias is in the audit line.
#[test]
fn the_audit_line_says_what_was_done_to_each_alias() {
    let t = box_for("k5");
    link(&t, "k2", "/opt/old/cli/k5");
    let v = refresh(&t, &K5_TOMBSTONE);
    // The verb runs with the compiled names, so the line is written here
    // from this answer by the function `run` calls.
    crate::audit(&t.ctx, Some(&Verb::HelperCliRefresh), Some(&daemon()), exit::OK, Some(&v));
    let line: serde_json::Value = serde_json::from_str(&t.audit.0.borrow().last().unwrap().clone()).unwrap();
    assert_eq!(line["aliases"], serde_json::json!([{"name": "k2", "result": "removed", "from": "/opt/old/cli/k5"}]));
    assert_eq!(line["cli"], "updated");
}

/// The default brand's one alias (`k2so`) is silent: through the real
/// verb, nothing at an alias's name is touched and the answer has no
/// `aliases`. No brand is named here but the compiled one.
#[test]
fn the_compiled_brand_tends_no_alias_today() {
    let compiled = CliNames::compiled();
    assert_eq!(compiled.name, CLI_FILE);
    assert!(
        compiled.aliases.iter().all(|(_, mode)| *mode == AliasMode::Silent),
        "the default brand retires no name: {:?}",
        compiled.aliases
    );
    let t = stale_box();
    for (alias, _) in compiled.aliases {
        link(&t, alias, CLI_PATH);
    }
    let v = cli_json(&t.call(&["helper", "cli-refresh"]));
    assert_eq!(v["result"], "updated", "{v}");
    assert!(v.get("aliases").is_none(), "{v}");
    for (alias, _) in compiled.aliases {
        assert_eq!(target(&t, alias), CLI_PATH);
    }
}

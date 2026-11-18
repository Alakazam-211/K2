//! K2's own release config, as committed (prd-compute-build-attestations-v1
//! S1): `.k2/signer.toml` (the policy a person approves at the signer),
//! `.k2/ci.toml` (what `k2 ci release` builds) and the old path's sources
//! of truth (`scripts/k2sign/policy`, `scripts/lib/compute-release.sh`,
//! `scripts/ci/cross-pins.sh`, the public keys in the source tree) must say
//! the same thing, or the client path would build what its signer refuses.

use std::collections::BTreeSet;
use std::path::PathBuf;

use k2_node_proto::ci::Isolation;
use k2_node_proto::ci_config::CiConfig;
use k2_node_proto::signer_policy::{Changes, ProvenanceRule, SignerPolicy};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The committed policy with its three placeholders filled.
fn policy() -> SignerPolicy {
    let text = read(".k2/signer.toml").replace("@CONTROLLER_FP@", &"c".repeat(64)).replace("@OWNER_FP@", &"0".repeat(64)).replace("@WORKSPACE_ID@", "ws-release");
    k2_signer::policy::parse(&text, "k2").unwrap_or_else(|e| panic!(".k2/signer.toml: {e}"))
}

/// One `key = value` line of the k2sign policy.
fn k2sign(key: &str) -> String {
    read("scripts/k2sign/policy")
        .lines()
        .find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string()))
        .unwrap_or_else(|| panic!("scripts/k2sign/policy has no {key}"))
}

/// A `NAME="value"` line of a shell file.
fn shell_value(rel: &str, name: &str) -> String {
    read(rel)
        .lines()
        .find_map(|l| l.trim().strip_prefix(&format!("{name}=\"")).and_then(|r| r.strip_suffix('"')).map(str::to_string))
        .unwrap_or_else(|| panic!("{rel} has no {name}=\"…\""))
}

/// The `public_key_b64` of the key with this role in a keys source file.
fn source_key(rel: &str, role: &str) -> String {
    let text = read(rel);
    let at = text.find(&format!("role: \"{role}\"")).unwrap_or_else(|| panic!("{rel}: no {role} key"));
    let rest = &text[at..];
    let k = rest.find("public_key_b64: \"").unwrap() + "public_key_b64: \"".len();
    rest[k..k + rest[k..].find('"').unwrap()].to_string()
}

#[test]
fn the_policy_parses_and_asks_a_person_for_new_machines() {
    let p = policy();
    assert_eq!(p.roster_changes, Changes::Ask, "BA-32: K2's signer accepts new node keys only through its person");
    assert_eq!(p.provenance, ProvenanceRule::Prefer, "S4: statements checked where nodes sign them; required once every release node can");
    assert_eq!(p.source.repo.as_deref(), Some(k2sign("repo").as_str()));
    assert_eq!(p.receipt_max_hours.to_string(), k2sign("compute_receipt_max_hours"));
    for b in ["linux-x86_64", "linux-aarch64"] {
        let r = &p.build[b];
        assert!(r.rust_pin, "{b}: the job's rustc must be the pin");
        assert!(r.isolation.iter().all(|i| matches!(i, Isolation::JobUid | Isolation::Exclusive)), "{b}");
    }
}

#[test]
fn ci_toml_builds_what_the_policy_accepts() {
    let p = policy();
    let ci = CiConfig::parse(&read(".k2/ci.toml")).unwrap();
    let rel = ci.release.expect("[release]");
    assert_eq!(rel.project, "k2");
    let a: BTreeSet<&String> = rel.builds.keys().collect();
    let b: BTreeSet<&String> = p.build.keys().collect();
    assert_eq!(a, b, "the same builds in .k2/ci.toml and .k2/signer.toml");
    for (name, c) in &rel.builds {
        let r = &p.build[name];
        assert_eq!(c.argv, r.argv, "{name} argv");
        assert_eq!(c.needs, r.needs, "{name} needs");
        assert!(!c.needs.iter().any(|n| n.starts_with("rust@")), "{name}: a rust@ need drops rust-toolchain.toml's targets");
        assert_eq!(c.artifacts, r.artifacts, "{name} artifacts");
        assert_eq!(c.env, r.env, "{name} env");
        let mut want: Vec<String> = r.secrets.iter().chain(&r.secrets_optional).cloned().collect();
        let mut got = c.secrets.clone();
        want.sort();
        got.sort();
        assert_eq!(got, want, "{name}: the jobs get every secret the policy names, the optional Microsoft id too (parity with the Mac build)");
        assert_eq!(c.verify_label.is_some(), !r.dual.is_empty(), "{name}: a dual build in both or neither");
        assert_eq!(c.exclusive, !r.secrets.is_empty(), "{name}: a build that holds secrets runs alone on its node");
        let ci_dims: Vec<_> = c.independence().into_iter().filter(|d| *d != k2_node_proto::ci::Dimension::Node).collect();
        if !r.dual.is_empty() {
            assert_eq!(ci_dims, r.independent, "{name}: placement and the signer use the same independence");
        }
    }
    let labels: Vec<(&str, &str, Option<&str>)> = rel.builds.iter().map(|(n, b)| (n.as_str(), b.label.as_str(), b.verify_label.as_deref())).collect();
    assert_eq!(labels, vec![("linux-aarch64", "L2", Some("V2")), ("linux-app", "L3", None), ("linux-x86_64", "L1", Some("V1"))]);
}

#[test]
fn the_policy_ports_k2signs_rules() {
    let p = policy();
    for (b, sfx) in [("linux-x86_64", ""), ("linux-aarch64", "_aarch64")] {
        let r = &p.build[b];
        let argv: Vec<String> = k2sign(&format!("compute_argv{sfx}")).split_whitespace().map(String::from).collect();
        assert_eq!(r.argv, argv, "{b}: k2sign's command");
        let dual: Vec<String> = k2sign(&format!("compute_dual_names{sfx}")).split_whitespace().map(|n| format!("dist/{n}")).collect();
        assert_eq!(r.dual, dual, "{b}: k2sign's dual names");
        assert_eq!(r.secrets.join(" "), k2sign("compute_secret_env"), "{b}");
        assert_eq!(r.secrets_optional.join(" "), k2sign("compute_secret_env_optional"), "{b}");
        assert_eq!(r.artifacts.join(" "), k2sign("compute_artifacts"), "{b}");
    }
    // The needs: k2sign's aarch64 needs (no rust@) for both binaries builds.
    let needs: Vec<String> = k2sign("compute_needs_aarch64").split_whitespace().map(String::from).collect();
    assert_eq!(p.build["linux-x86_64"].needs, needs);
    assert_eq!(p.build["linux-aarch64"].needs, needs);
    // The app build's needs: the old path's, less its rust@.
    let lib = read("scripts/lib/compute-release.sh");
    let line = lib.lines().find(|l| l.trim_start().starts_with("local app_needs=\"")).expect("compute-release.sh app_needs");
    let old: Vec<&str> = line.split('"').nth(1).unwrap().split(',').filter(|n| *n != "$rust").collect();
    assert_eq!(p.build["linux-app"].needs, old);
    // BA-10: the cross toolchain hashes.
    let rep = p.build["linux-aarch64"].reported.as_ref().expect("[build.linux-aarch64.reported]");
    assert_eq!(rep.file, "dist/build-info.json");
    let zig = shell_value("scripts/ci/cross-pins.sh", "K2_CROSS_ZIG_SHA256_X86_64");
    let zb = shell_value("scripts/ci/cross-pins.sh", "K2_CROSS_ZIGBUILD_CRATE_SHA256");
    assert_eq!(rep.equals["/cross/zig_tarball_sha256"], zig);
    assert_eq!(rep.equals["/cross/cargo_zigbuild_crate_sha256"], zb);
    assert_eq!(zig, k2sign("compute_aarch64_zig_sha256"));
    assert_eq!(zb, k2sign("compute_aarch64_zigbuild_sha256"));
    assert!(p.build["linux-x86_64"].reported.is_none());
}

#[test]
fn assets_and_keys_match_the_source_tree() {
    let p = policy();
    let legacy = read("crates/k2-release-keys/src/lib.rs");
    let l = legacy.find("LEGACY_UPDATER_KEY_B64: &str = \"").unwrap() + "LEGACY_UPDATER_KEY_B64: &str = \"".len();
    assert_eq!(p.keys["u1"].public, legacy[l..l + legacy[l..].find('"').unwrap()]);
    assert_eq!(p.keys["u2"].public, source_key("crates/k2-release-keys/src/lib.rs", "primary"));
    assert_eq!(p.keys["root-helper"].public, source_key("crates/k2-root-proto/src/keys.rs", "primary"));
    assert!(p.keys["u1"].held && p.keys["u2"].held, "the update keys are the signer's");
    assert!(!p.keys["root-helper"].held, "BA-11: the signer only gives verdicts for the root-helper key");
    // The daemons and k5-cli, signed with the update key: exactly k2sign's compute_assets.
    let signed: BTreeSet<String> = p.assets.iter().filter(|(_, a)| a.key == "u2").map(|(n, _)| n.clone()).collect();
    let compute: BTreeSet<String> = k2sign("compute_assets").split_whitespace().map(String::from).collect();
    assert_eq!(signed, compute);
    let want: BTreeSet<String> =
        ["k2-daemon-linux-x86_64", "k2-daemon-linux-aarch64", "k5-cli-linux-x86_64", "k5-cli-linux-aarch64"].map(String::from).into();
    assert_eq!(signed, want, "the update key signs the Linux daemons and k5-cli, nothing else from compute");
    // U2 signs k5-cli, U1 (the bridge for installs older than 0.46.6) never does.
    let u1: BTreeSet<String> = k2sign("assets.u1").split_whitespace().map(String::from).collect();
    let u2: BTreeSet<String> = k2sign("assets.u2").split_whitespace().map(String::from).collect();
    for k5 in ["k5-cli-linux-x86_64", "k5-cli-linux-aarch64"] {
        assert!(u2.contains(k5) && !u1.contains(k5), "{k5}: U2 only");
    }
    // Every other dual-built file gets a verdict for the root-helper key.
    for (b, r) in [("linux-x86_64", &p.build["linux-x86_64"]), ("linux-aarch64", &p.build["linux-aarch64"])] {
        for d in &r.dual {
            let name = d.strip_prefix("dist/").unwrap();
            let a = p.assets.get(name).unwrap_or_else(|| panic!("no asset for {d}"));
            assert_eq!((a.build.as_str(), a.artifact.as_str()), (b, d.as_str()));
            let update_key = name.starts_with("k2-daemon-") || name.starts_with("k5-cli-");
            assert_eq!(a.key, if update_key { "u2" } else { "root-helper" }, "{name}");
        }
    }
}

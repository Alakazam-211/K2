//! The frozen formats' golden vectors (tests/release/fixtures/ci-formats-v1.json,
//! ci-formats-v1-signer-policy.toml): the complete evidence bundle v2 there
//! passes every k2-signer rule against the matching policy, both as a
//! rehearsal and as a release bound by a (fake) forge, and fails the rules it
//! should when one thing about the signer's own state differs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_node_proto::admin::ApprovalKey;
use k2_node_proto::evidence::EvidenceBundle;
use k2_signer::check::{check, Input, Target};
use k2_signer::forge::{resolve_github, FakeForge, ReleaseSource};
use k2_signer::trust::{AcceptedKey, ControllerPin, OwnerPin, PinHow, RosterMark, Trust};
use serde_json::Value;

const NOW: i64 = 1_800_001_500;

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/release/fixtures").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

struct Golden {
    bundle: EvidenceBundle,
    policy: k2_node_proto::signer_policy::SignerPolicy,
    root: Value,
}

fn golden() -> Golden {
    let root: Value = serde_json::from_str(&fixture("ci-formats-v1.json")).unwrap();
    let bundle_text = serde_json::to_vec(&root["evidence_bundle"]).unwrap();
    let bundle = EvidenceBundle::parse(&bundle_text).expect("the golden bundle parses");
    let policy = k2_signer::policy::parse(&fixture("ci-formats-v1-signer-policy.toml"), "acme-desktop").expect("the golden policy passes the signer's rules");
    Golden { bundle, policy, root }
}

fn trust(g: &Golden, accept: &[&str]) -> Trust {
    let keys = &g.root["keys"];
    let mut t = Trust::default();
    let ctl = keys["controller"]["fp"].as_str().unwrap().to_string();
    t.controllers.insert(ctl, ControllerPin { label: "client.example".into(), spki: Some(keys["controller"]["spki_b64"].as_str().unwrap().into()), how: PinHow::Sas, at: 0 });
    let roster_fp = keys["roster"]["fp"].as_str().unwrap().to_string();
    let roster_key = ApprovalKey::parse(&format!("ed25519:{}", keys["roster"]["public"].as_str().unwrap().trim_start_matches("ed25519:"))).unwrap();
    assert_eq!(roster_key.fingerprint(), roster_fp);
    let mut accepted = BTreeMap::new();
    for k in accept {
        accepted.insert(keys[*k]["fp"].as_str().unwrap().to_string(), AcceptedKey { name: k.to_string(), seq: 2, at: 0 });
    }
    t.owners.insert(roster_fp, OwnerPin { key: roster_key.encode(), label: "owner".into(), at: 0, accepted });
    t.check().unwrap();
    t
}

fn release_source(g: &Golden) -> ReleaseSource {
    ReleaseSource { tag_commit: g.bundle.source.commit.clone(), tree: g.bundle.source.tree.clone(), base: g.bundle.source.commit.clone(), rust_pin: Some("1.99.1".into()) }
}

#[test]
fn the_golden_bundle_rehearses_and_releases() {
    let g = golden();
    let t = trust(&g, &["node_a_epoch1", "node_b"]);
    let marks = BTreeMap::new();
    let s = check(&Input { policy: &g.policy, trust: &t, marks: &marks, bundle: &g.bundle, now: NOW, target: Target::Rehearse }).expect("rehearse");
    assert_eq!(s.rust_pin.as_deref(), Some("1.99.1"));
    assert_eq!(s.jobs["L1"].node, "node-a");
    assert_eq!(s.jobs["V1"].node, "node-b");
    let roster_fp = g.root["keys"]["roster"]["fp"].as_str().unwrap();
    assert_eq!(s.new_marks[roster_fp].seq, 2);
    // S9: the policy pins a baked-in secret's hash, which receipt v2 can't show yet.
    assert!(s.notes.iter().any(|n| n.contains("secret_sha256") && n.contains("not checked")), "{:?}", s.notes);

    let src = release_source(&g);
    let s2 = check(&Input {
        policy: &g.policy,
        trust: &t,
        marks: &marks,
        bundle: &g.bundle,
        now: NOW,
        target: Target::Release { version: "1.2.3", base: &src.base, opened: Some(NOW - 3600), source: Some(&src) },
    })
    .expect("release");
    assert_eq!(s2.tag_commit.as_deref(), Some(g.bundle.source.commit.as_str()));

    // Through the forge: tag v1.2.3 → the built commit, its tree, its pin.
    let mut f = FakeForge::default();
    f.tags.insert("v1.2.3".into(), g.bundle.source.commit.clone());
    f.commits.insert(g.bundle.source.commit.clone(), (g.bundle.source.tree.clone(), vec!["6".repeat(40)]));
    f.files.insert(("rust-toolchain.toml".into(), g.bundle.source.commit.clone()), b"[toolchain]\nchannel = \"1.99.1\"\n".to_vec());
    let rs = resolve_github(&f, &g.policy, "1.2.3", &g.bundle.source.commit, None, true).unwrap();
    assert_eq!(rs, src);
    // A client project's tag must be the approved commit itself.
    let e = resolve_github(&f, &g.policy, "1.2.3", &"6".repeat(40), None, true).unwrap_err();
    assert!(e.message().contains("not the approved commit"), "{e}");
}

#[test]
fn the_golden_bundle_fails_when_the_signer_state_differs() {
    let g = golden();
    let marks = BTreeMap::new();
    let run = |t: &Trust, marks: &BTreeMap<String, RosterMark>, now: i64| {
        check(&Input { policy: &g.policy, trust: t, marks, bundle: &g.bundle, now, target: Target::Rehearse }).map(|_| ()).unwrap_err().message().to_string()
    };
    // Node B's key was never accepted by a person (roster_changes = ask).
    let e = run(&trust(&g, &["node_a_epoch1"]), &marks, NOW);
    assert!(e.contains("roster_change_needs_person") && e.contains("V1"), "{e}");
    // The controller is not paired here.
    let mut t = trust(&g, &["node_a_epoch1", "node_b"]);
    t.controllers.clear();
    assert!(run(&t, &marks, NOW).contains("not paired"));
    // The owner's key is not pinned.
    let mut t = trust(&g, &["node_a_epoch1", "node_b"]);
    t.owners.clear();
    assert!(run(&t, &marks, NOW).contains("not pinned"));
    let t = trust(&g, &["node_a_epoch1", "node_b"]);
    // A higher roster seq was registered before: a replay.
    let roster_fp = g.root["keys"]["roster"]["fp"].as_str().unwrap().to_string();
    let mut m = BTreeMap::new();
    m.insert(roster_fp.clone(), RosterMark { seq: 3, digest: format!("sha256:{}", "1".repeat(64)), issued_at: 0, seen_at: 0, auto_accepted: BTreeMap::new() });
    assert!(run(&t, &m, NOW).contains("node_roster_replayed"));
    // Seq 2 registered with another digest: a fork.
    m.get_mut(&roster_fp).unwrap().seq = 2;
    assert!(run(&t, &m, NOW).contains("a second roster for one seq"));
    // Seq 1 registered with another digest than the roster's prev_digest: the chain breaks.
    m.get_mut(&roster_fp).unwrap().seq = 1;
    assert!(run(&t, &m, NOW).contains("does not chain"));
    // The roster is older than roster_max_age_hours (and the receipts too old).
    let e = run(&t, &marks, 1_800_000_600 + 169 * 3600);
    assert!(e.contains("node_roster_stale"), "{e}");
    // The receipts are older than receipt_max_hours.
    let e = run(&t, &marks, 1_800_001_320 + 25 * 3600);
    assert!(e.contains("an old receipt"), "{e}");
}

//! The rule set end to end: a pinned owner key, a roster, node keys,
//! self-statements, statements and files. Each test changes one thing from
//! a valid set and expects that one rule to fail (or the time rule to stay
//! unproven).

mod common;

use common::*;
use kessel_attest::load;
use kessel_attest::trust::TrustDir;
use kessel_attest::verify::{verify, FileArg, Input, Outcome, Policy, Report, Verdict};
use kessel_attest::roster::Signed;
use serde_json::{json, Value};

fn input(w: &World, files: Vec<FileArg>) -> Input {
    Input {
        statements: load::envelopes(&w.provenance("L1")).unwrap(),
        twin: None,
        files,
        trust: Some(TrustDir::open(&w.trust()).unwrap()),
        rosters: Vec::new(),
        policy: Policy { owner_fp: w.owner.fp.clone(), ..Policy::default() },
        now: t(NOW),
    }
}

fn bin(w: &World) -> FileArg {
    load::file_arg(&w.file("dl/acme-linux-x86_64", BIN_A_BYTES)).unwrap()
}

fn failed(r: &Report) -> (&'static str, String) {
    assert_eq!(r.verdict, Verdict::NotVerified, "expected a failure: {:#?}", r.checks);
    let c = r.failed().expect("a failed check");
    (c.rule, c.detail.clone())
}

fn set_entry(w: &mut World, fp: &str, f: impl Fn(&mut Value)) {
    let e = w.entries.iter_mut().find(|e| e["node_fp"] == json!(fp)).expect("entry");
    f(e);
}

#[test]
fn a_valid_set_is_verified_with_every_rule_run_in_order() {
    let w = World::new("valid");
    w.write(t(AFTER_BUILD));
    let r = verify(&input(&w, vec![bin(&w)]));
    assert_eq!(r.verdict, Verdict::Verified, "{:#?}", r.checks);
    let rules: Vec<&str> = r.checks.iter().map(|c| c.rule).collect();
    assert_eq!(rules, ["roster", "node key", "signature", "owner", "key status", "self-statement", "file"]);
    assert!(r.checks.iter().all(|c| c.outcome == Outcome::Pass));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let s = &r.statements[0];
    assert_eq!((s.node.as_str(), s.host.as_str(), s.key_status, s.isolation), ("test-node-01", "host-a", "active", "exclusive"));
    assert_eq!(r.roster.as_ref().map(|(o, seq, _)| (o.as_str(), *seq)), Some(("acme-owner.example.test", 1)));
}

#[test]
fn both_subjects_and_policy_checks_pass_when_they_hold() {
    let w = World::new("policy-ok");
    w.write(t(AFTER_BUILD));
    let sum = load::file_arg(&w.file("dl/acme-linux-x86_64.sha256", SUM_A_BYTES)).unwrap();
    let mut i = input(&w, vec![bin(&w), sum]);
    let commit = w.statement["predicate"]["buildDefinition"]["externalParameters"]["source"]["digest"]["gitCommit"].as_str().unwrap().to_string();
    i.policy.commit = Some(commit);
    i.policy.repo = Some("https://git.example.test/acme/app".into());
    i.policy.isolation = vec![kessel_attest::statement::Isolation::Exclusive, kessel_attest::statement::Isolation::JobUid];
    let r = verify(&i);
    assert_eq!(r.verdict, Verdict::Verified, "{:#?}", r.checks);
    let rules: Vec<&str> = r.checks.iter().map(|c| c.rule).collect();
    assert_eq!(rules, ["roster", "node key", "signature", "owner", "key status", "self-statement", "commit", "repo", "isolation", "file", "file"]);
}

#[test]
fn policy_mismatches_fail_by_name() {
    let w = World::new("policy-bad");
    w.write(t(AFTER_BUILD));
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.commit = Some("0".repeat(40));
    assert_eq!(failed(&verify(&i)).0, "commit");
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.repo = Some("git.example.test/acme/other".into());
    assert_eq!(failed(&verify(&i)).0, "repo");
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.isolation = vec![kessel_attest::statement::Isolation::JobUid];
    let (rule, detail) = failed(&verify(&i));
    assert_eq!(rule, "isolation");
    assert!(detail.contains("exclusive"), "{detail}");
}

/// K2's repo moved from Alakazam-211/K2 to AlakazamLabs/K5: a statement
/// built under either name passes `--repo` with either name, and a third
/// name fails the repo rule.
#[test]
fn a_renamed_repo_passes_under_both_names_and_no_other() {
    for (built, n) in [("git+https://github.com/Alakazam-211/K2.git", "old"), ("git+https://github.com/AlakazamLabs/K5.git", "new")] {
        let mut w = World::new(&format!("renamed-{n}"));
        w.statement["predicate"]["buildDefinition"]["externalParameters"]["source"]["uri"] = json!(built);
        w.write(t(AFTER_BUILD));
        for want in ["https://github.com/Alakazam-211/K2", "https://github.com/AlakazamLabs/K5"] {
            let mut i = input(&w, vec![bin(&w)]);
            i.policy.repo = Some(want.into());
            let r = verify(&i);
            assert_eq!(r.verdict, Verdict::Verified, "built from {built}, --repo {want}: {:#?}", r.checks);
            assert!(r.checks.iter().any(|c| c.rule == "repo" && c.outcome == Outcome::Pass), "{:#?}", r.checks);
        }
        for third in ["https://github.com/AlakazamLabs/K2", "https://github.com/Alakazam-211/K5", "https://github.com/someone/K5"] {
            let mut i = input(&w, vec![bin(&w)]);
            i.policy.repo = Some(third.into());
            let (rule, detail) = failed(&verify(&i));
            assert_eq!(rule, "repo", "built from {built}, --repo {third}");
            assert!(detail.contains(built), "{detail}");
        }
    }
}

#[test]
fn a_file_that_is_not_a_subject_fails() {
    let w = World::new("file");
    w.write(t(AFTER_BUILD));
    let f = load::file_arg(&w.file("dl/acme-linux-x86_64", b"acme-linux-x86_65")).unwrap();
    let (rule, detail) = failed(&verify(&input(&w, vec![f])));
    assert_eq!(rule, "file");
    assert!(detail.contains("dist/acme-linux-x86_64"), "names what the statement has: {detail}");
}

#[test]
fn a_tampered_payload_fails_the_signature() {
    let w = World::new("tamper");
    w.write(t(AFTER_BUILD));
    let mut env: Value = serde_json::from_str(&std::fs::read_to_string(w.provenance("L1")).unwrap()).unwrap();
    // Still a well-formed, canonical statement: only the signature can tell.
    let payload = String::from_utf8(kessel_attest::util::unb64(env["payload"].as_str().unwrap()).unwrap()).unwrap();
    let evil = payload.replacen("scripts/ci/build.sh", "scripts/ci/other.sh", 1);
    assert_ne!(evil, payload);
    env["payload"] = json!(kessel_attest::util::b64(evil.as_bytes()));
    write_json(&w.provenance("L1"), &env);
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "signature");
    assert!(detail.contains("does not verify"), "{detail}");
}

#[test]
fn a_statement_signed_in_another_form_fails_even_with_a_good_signature() {
    let w = World::new("noncanon");
    w.write(t(AFTER_BUILD));
    let pretty = serde_json::to_vec_pretty(&w.statement).unwrap();
    write_json(&w.provenance("L1"), &w.node.envelope_bytes(&pretty));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "signature");
    assert!(detail.contains("canonical"), "{detail}");
}

#[test]
fn a_key_outside_the_roster_is_refused_by_name() {
    let mut w = World::new("not-in-roster");
    let gone = w.node.fp.clone();
    w.entries.retain(|e| e["node_fp"] != json!(gone));
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "node key");
    assert!(detail.contains(&gone) && detail.contains("acme-owner.example.test"), "names the key and the owner: {detail}");
}

#[test]
fn another_owner_key_pinned_refuses_the_roster() {
    let w = World::new("pin");
    w.write(t(AFTER_BUILD));
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.owner_fp = Owner::new(8).fp;
    let (rule, detail) = failed(&verify(&i));
    assert_eq!(rule, "roster");
    assert!(detail.contains("no roster for owner key"), "{detail}");
    // The right file name, but signed by another key.
    let other = Owner::new(8);
    let forged = other.roster(1, t(AFTER_BUILD), None, w.entries.clone());
    write_json(&w.trust().join("rosters").join(format!("{}.json", w.owner.fp)), &forged);
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "roster");
    assert!(detail.contains("not the pinned owner key"), "{detail}");
}

#[test]
fn a_roster_with_a_broken_signature_is_refused() {
    let w = World::new("roster-sig");
    let mut roster = w.owner.roster(1, t(AFTER_BUILD), None, w.entries.clone());
    roster["body"] = json!(roster["body"].as_str().unwrap().replace("host-a", "host-z"));
    w.write_with_roster(&roster);
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "roster");
    assert!(detail.contains("does not verify"), "{detail}");
}

#[test]
fn the_owner_server_form_of_builder_id_is_refused() {
    let mut w = World::new("owner-server");
    w.statement["predicate"]["runDetails"]["builder"]["id"] = json!(format!("https://k2.dev/compute/builder/v1/owner-server/{}", "e".repeat(64)));
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "owner");
    assert!(detail.contains("owner-server"), "{detail}");
}

#[test]
fn a_statement_naming_another_owner_or_epoch_is_refused() {
    let mut w = World::new("other-owner");
    w.statement["predicate"]["runDetails"]["builder"]["id"] = json!(Owner::new(9).builder_id());
    w.write(t(AFTER_BUILD));
    assert_eq!(failed(&verify(&input(&w, vec![bin(&w)]))).0, "owner");

    let mut w = World::new("epoch");
    for d in w.statement["predicate"]["runDetails"]["builder"]["builderDependencies"].as_array_mut().unwrap() {
        if d["name"] == "k2-node-key" {
            d["annotations"]["keyEpoch"] = json!(2);
        }
    }
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "owner");
    assert!(detail.contains("epoch"), "{detail}");
}

// ── key status and time (the time rules for a standalone verifier) ──

fn retire(w: &mut World, valid_to: &str) {
    let fp = w.node.fp.clone();
    set_entry(w, &fp, |e| {
        e["status"] = json!("retired");
        e["valid_to"] = json!(t(valid_to));
    });
}

#[test]
fn a_retired_key_is_verified_but_time_not_proven() {
    let mut w = World::new("retired");
    retire(&mut w, "2027-01-15T12:00:00Z");
    w.write(t(AFTER_BUILD));
    let r = verify(&input(&w, vec![bin(&w)]));
    assert_eq!(r.verdict, Verdict::TimeNotProven, "{:#?}", r.checks);
    assert_eq!(r.verdict.exit_code(), 3);
    let time = r.checks.iter().find(|c| c.rule == "time").expect("a time check");
    assert_eq!(time.outcome, Outcome::Unproven);
    assert!(time.detail.contains("--existed-before"), "{}", time.detail);
    assert!(!r.statements[0].time_proven);
}

#[test]
fn a_time_vouched_for_before_retirement_settles_the_time() {
    let mut w = World::new("retired-proven");
    retire(&mut w, "2027-01-15T12:00:00Z");
    w.write(t(AFTER_BUILD));
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.existed_before = Some(t("2027-01-15T09:00:00Z"));
    let r = verify(&i);
    assert_eq!(r.verdict, Verdict::Verified, "{:#?}", r.checks);
    assert_eq!(r.checks.iter().find(|c| c.rule == "time").map(|c| c.outcome), Some(Outcome::Pass));
    // A time after the retirement says nothing about when it was signed.
    i.policy.existed_before = Some(t("2027-01-15T12:00:01Z"));
    assert_eq!(verify(&i).verdict, Verdict::TimeNotProven);
}

#[test]
fn a_key_retired_before_the_job_started_is_refused() {
    let mut w = World::new("retired-before");
    retire(&mut w, "2027-01-15T08:00:00Z");
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "key status");
    assert!(detail.contains("retired"), "{detail}");
}

#[test]
fn a_revoked_key_is_refused_whatever_the_time() {
    let mut w = World::new("revoked");
    let fp = w.node.fp.clone();
    set_entry(&mut w, &fp, |e| {
        e["status"] = json!("revoked");
        e["revoked_reason"] = json!("compromised");
    });
    w.write(t(AFTER_BUILD));
    let mut i = input(&w, vec![bin(&w)]);
    i.policy.existed_before = Some(t("2027-01-15T09:00:00Z"));
    let (rule, detail) = failed(&verify(&i));
    assert_eq!(rule, "key status");
    assert!(detail.contains("revoked (compromised)"), "{detail}");
}

#[test]
fn a_job_before_the_key_was_valid_or_an_unvouched_key_is_refused() {
    let mut w = World::new("valid-from");
    let fp = w.node.fp.clone();
    set_entry(&mut w, &fp, |e| e["valid_from"] = json!(t("2027-01-15T09:00:00Z")));
    w.write(t(AFTER_BUILD));
    assert_eq!(failed(&verify(&input(&w, vec![bin(&w)]))).0, "key status");

    let mut w = World::new("taint-basis");
    set_entry(&mut w, &fp, |e| e["taint_basis"] = Value::Null);
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "key status");
    assert!(detail.contains("vouches for nothing"), "{detail}");
}

#[test]
fn a_roster_older_than_the_build_or_a_month_warns_without_changing_the_verdict() {
    let w = World::new("old-roster");
    w.write(t("2027-01-10T00:00:00Z"));
    let r = verify(&input(&w, vec![bin(&w)]));
    assert_eq!(r.verdict, Verdict::Verified, "{:#?}", r.checks);
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("older than the build") && r.warnings[0].contains("--roster"), "{}", r.warnings[0]);
    let mut i = input(&w, vec![bin(&w)]);
    i.now = t("2027-03-01T00:00:00Z");
    let r = verify(&i);
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings[1].contains("over 30 days ago"), "{}", r.warnings[1]);
}

#[test]
fn a_newer_roster_given_with_roster_wins_and_shows_a_later_retirement() {
    let mut w = World::new("newer");
    w.write(t(AFTER_BUILD));
    let first = w.owner.roster(1, t(AFTER_BUILD), None, w.entries.clone());
    retire(&mut w, "2027-01-15T12:00:00Z");
    let second = w.owner.roster(2, t("2027-01-16T12:00:00Z"), Some(&body_digest(&first)), w.entries.clone());
    let mut i = input(&w, vec![bin(&w)]);
    i.rosters.push(("current.json".into(), serde_json::from_value::<Signed>(second.clone()).unwrap()));
    let r = verify(&i);
    assert_eq!(r.roster.as_ref().map(|(_, seq, _)| *seq), Some(2));
    assert_eq!(r.verdict, Verdict::TimeNotProven, "{:#?}", r.checks);

    // A second roster can't claim seq 2 without chaining to seq 1.
    let unchained = w.owner.roster(2, t("2027-01-16T12:00:00Z"), Some(&format!("sha256:{}", "0".repeat(64))), w.entries.clone());
    let mut i = input(&w, vec![bin(&w)]);
    i.rosters.push(("current.json".into(), serde_json::from_value::<Signed>(unchained).unwrap()));
    let (rule, detail) = failed(&verify(&i));
    assert_eq!(rule, "roster");
    assert!(detail.contains("does not follow"), "{detail}");

    // An older --roster than trust/'s: the newest is used, with a warning.
    w.write_with_roster(&second);
    let mut i = input(&w, vec![bin(&w)]);
    i.rosters.push(("old.json".into(), serde_json::from_value::<Signed>(first).unwrap()));
    let r = verify(&i);
    assert_eq!(r.roster.as_ref().map(|(_, seq, _)| *seq), Some(2));
    assert!(r.warnings.iter().any(|x| x.contains("old.json is roster seq 1")), "{:?}", r.warnings);
}

#[test]
fn two_rosters_with_one_seq_are_refused() {
    let mut w = World::new("fork");
    w.write(t(AFTER_BUILD));
    let fp = w.node.fp.clone();
    set_entry(&mut w, &fp, |e| e["host"] = json!("host-c"));
    let fork = w.owner.roster(1, t(AFTER_BUILD), None, w.entries.clone());
    let mut i = input(&w, vec![bin(&w)]);
    i.rosters.push(("fork.json".into(), serde_json::from_value::<Signed>(fork).unwrap()));
    let (rule, detail) = failed(&verify(&i));
    assert_eq!(rule, "roster");
    assert!(detail.contains("two different rosters with seq 1"), "{detail}");
}

// ── the node's self-statement and trust/ files ──

#[test]
fn a_self_statement_that_admits_a_weak_wall_is_refused() {
    let cases: Vec<EditCase> = vec![
        ("dev", Box::new(|v: &mut Value| v["dev"] = json!(true)), "--dev"),
        ("readable", Box::new(|v: &mut Value| v["key_exposure"]["job_readable"] = json!(true)), "read its key"),
        ("tainted", Box::new(|v: &mut Value| v["taint"] = json!("tainted")), "tainted"),
        ("unknown", Box::new(|v: &mut Value| v["taint"] = json!("unknown")), "predates"),
        ("late", Box::new(|v: &mut Value| v["issued_at"] = json!(t("2027-01-15T09:00:00Z"))), "after the job started"),
    ];
    for (tag, edit, want) in cases {
        let mut w = World::new(tag);
        w.self_st = w.node.self_statement(edit);
        let fp = w.node.fp.clone();
        let d = body_digest(&w.self_st);
        set_entry(&mut w, &fp, |e| e["statement_digest"] = json!(d));
        w.statement = statement_for(&w.owner, &w.node, &w.self_st);
        w.write(t(AFTER_BUILD));
        let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
        assert_eq!(rule, "self-statement", "{tag}");
        assert!(detail.contains(want), "{tag}: {detail}");
    }
}

#[test]
fn an_unknown_taint_passes_when_the_owner_vouched() {
    let mut w = World::new("attested");
    w.self_st = w.node.self_statement(|v| v["taint"] = json!("unknown"));
    let fp = w.node.fp.clone();
    let d = body_digest(&w.self_st);
    set_entry(&mut w, &fp, |e| {
        e["statement_digest"] = json!(d);
        e["taint_basis"] = json!("owner_attested");
        e["attestation_digest"] = json!(format!("sha256:{}", "a".repeat(64)));
    });
    w.statement = statement_for(&w.owner, &w.node, &w.self_st);
    w.write(t(AFTER_BUILD));
    assert_eq!(verify(&input(&w, vec![bin(&w)])).verdict, Verdict::Verified);
}

#[test]
fn a_missing_or_swapped_trust_file_is_refused() {
    let w = World::new("missing-self");
    w.write(t(AFTER_BUILD));
    let hex = &body_digest(&w.self_st)["sha256:".len()..];
    std::fs::remove_file(w.trust().join("statements").join(format!("{hex}.json"))).unwrap();
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "self-statement");
    assert!(detail.contains("lacks"), "{detail}");

    // Another node's self-statement under this one's name: the hash is wrong.
    let w = World::new("swapped-self");
    w.write(t(AFTER_BUILD));
    let hex = &body_digest(&w.self_st)["sha256:".len()..];
    write_json(&w.trust().join("statements").join(format!("{hex}.json")), &w.other_self_st);
    assert_eq!(failed(&verify(&input(&w, vec![bin(&w)]))).0, "self-statement");

    // A node PEM that isn't the roster's key.
    let w = World::new("swapped-pem");
    w.write(t(AFTER_BUILD));
    write(&w.trust().join("nodes").join(format!("{}.pem", w.node.fp)), &w.other.pem());
    let (rule, detail) = failed(&verify(&input(&w, vec![bin(&w)])));
    assert_eq!(rule, "node key");
    assert!(detail.contains("fingerprint"), "{detail}");
}

// ── twins ──

fn twin_input(w: &World) -> Input {
    let mut i = input(w, vec![bin(w)]);
    i.twin = load::envelopes(&w.provenance("V1")).unwrap().pop();
    i
}

#[test]
fn a_twin_on_another_machine_with_the_same_bytes_passes() {
    let w = World::new("twin");
    w.write(t(AFTER_BUILD));
    let r = verify(&twin_input(&w));
    assert_eq!(r.verdict, Verdict::Verified, "{:#?}", r.checks);
    let last = r.checks.last().unwrap();
    assert_eq!((last.rule, last.outcome), ("twin", Outcome::Pass));
    assert!(last.detail.contains("host-b") && last.detail.contains("host-a"), "{}", last.detail);
    assert_eq!(r.statements.len(), 2);
}

#[test]
fn a_twin_on_the_same_machine_or_with_other_bytes_fails() {
    let mut w = World::new("twin-host");
    let fp = w.other.fp.clone();
    set_entry(&mut w, &fp, |e| e["host"] = json!("host-a"));
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&twin_input(&w)));
    assert_eq!(rule, "twin");
    assert!(detail.contains("one machine"), "{detail}");

    let mut w = World::new("twin-bytes");
    w.other_statement["subject"][0]["digest"]["sha256"] = json!("f".repeat(64));
    w.write(t(AFTER_BUILD));
    let (rule, detail) = failed(&verify(&twin_input(&w)));
    assert_eq!(rule, "twin");
    assert!(detail.contains("dist/acme-linux-x86_64"), "{detail}");
}

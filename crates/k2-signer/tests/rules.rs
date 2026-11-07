//! Every evidence rule, one refusal at a time: the S3b acceptance cases
//! (tests/release/k2sign-s3b.sh, and its review fixes H1, H2, M2, M4) ported
//! onto receipt v2 and owner node rosters, plus the v2-only rules (rosters,
//! seats, taint, independence, pairing pins). Each test starts from a bundle
//! that passes and bends one thing.

mod common;

use std::collections::BTreeMap;

use common::*;
use k2_node_proto::ci::{Dimension, Isolation, KeyStatus, RevokeReason, Taint, TaintBasis, Wall};
use k2_node_proto::evidence::{EvidenceBundle, Role};
use k2_node_proto::frames::{ExitInfo, JobState};
use k2_node_proto::signer_policy::{Changes, SignerPolicy};
use k2_signer::check::{check, Input, Summary, Target};
use k2_signer::forge::ReleaseSource;
use k2_signer::trust::{RosterMark, Trust};
use serde_json::json;

struct Run {
    policy: SignerPolicy,
    trust: Trust,
    marks: BTreeMap<String, RosterMark>,
    now: i64,
    source: Option<ReleaseSource>,
    rehearse: bool,
    opened: Option<i64>,
    /// Node names a person accepted, with the attributes the bundle's roster
    /// gives them (off: `trust` as it is).
    accept: Vec<&'static str>,
    accept_from_bundle: bool,
}

impl Run {
    fn new(w: &World) -> Self {
        Self { policy: w.policy(), trust: w.trust(&["node-a", "node-b", "node-c"]), marks: BTreeMap::new(), now: NOW, source: Some(w.source()), rehearse: false, opened: Some(NOW - 3600), accept: vec!["node-a", "node-b", "node-c"], accept_from_bundle: true }
    }
    fn go(&self, b: &EvidenceBundle) -> Result<Summary, String> {
        let base = self.source.as_ref().map(|s| s.base.clone()).unwrap_or_default();
        let target = if self.rehearse {
            Target::Rehearse
        } else {
            Target::Release { version: VERSION, base: &base, opened: self.opened, source: self.source.as_ref() }
        };
        let mut trust = self.trust.clone();
        if self.accept_from_bundle {
            accept_from(&mut trust, b, &self.accept);
        }
        check(&Input { policy: &self.policy, trust: &trust, marks: &self.marks, bundle: b, now: self.now, target }).map_err(|e| e.message().to_string())
    }
}

#[track_caller]
fn refuses(w: &World, r: &Run, s: Spec, text: &str) {
    let b = w.build(s);
    match r.go(&b) {
        Ok(_) => panic!("accepted, want a refusal containing {text:?}"),
        Err(e) => assert!(e.contains(text), "refusal {e:?} does not contain {text:?}"),
    }
}

#[track_caller]
fn passes(w: &World, r: &Run, s: Spec) -> Summary {
    let b = w.build(s);
    r.go(&b).unwrap_or_else(|e| panic!("refused: {e}"))
}

fn entry_mut<'a>(s: &'a mut Spec, fp: &str) -> &'a mut k2_node_proto::node_roster::RosterEntry {
    s.roster.entries.iter_mut().find(|e| e.node_fp == fp).unwrap()
}

fn l1(s: &mut Spec) -> &mut common::Rc {
    s.receipts.get_mut("L1").unwrap()
}
fn v1(s: &mut Spec) -> &mut common::Rc {
    s.receipts.get_mut("V1").unwrap()
}

#[test]
fn a_good_bundle_passes_as_release_and_rehearsal() {
    let w = World::new();
    let mut r = Run::new(&w);
    let s = passes(&w, &r, w.spec());
    assert_eq!(s.jobs.len(), 2);
    assert_eq!(s.jobs["L1"].isolation, Isolation::JobUid);
    assert_eq!(s.new_marks[&w.owner_fp].seq, 1);
    r.rehearse = true;
    passes(&w, &r, w.spec());
    // The preview path: the tag is base+bump with the same tree; the built
    // commit's object names the tree and the approved base as its parent.
    r.rehearse = false;
    r.source = Some(ReleaseSource { tag_commit: "7".repeat(40), tree: w.tree.clone(), base: w.base.clone(), rust_pin: Some("1.99.1".into()) });
    passes(&w, &r, w.spec());
}

#[test]
fn controller_rules() {
    let w = World::new();
    let r = Run::new(&w);
    // A bundle from (and receipts sent by) another controller.
    let mut s = w.spec();
    s.controller_fp = w.other_controller.fingerprint();
    refuses(&w, &r, s, "is not one project acme-desktop's policy lets build it");
    // In the policy but never paired with this signer.
    let mut r2 = Run::new(&w);
    r2.trust.controllers.clear();
    refuses(&w, &r2, w.spec(), "is not paired with this signer");
    // A receipt naming another controller.
    let mut s = w.spec();
    l1(&mut s).receipt.controller_fp = w.other_controller.fingerprint();
    refuses(&w, &r, s, "L1: the job was sent by controller");
    // A seat for another client.
    let mut s = w.spec();
    s.seats[0].client_fp = w.other_controller.fingerprint();
    refuses(&w, &r, s, "is another client's");
}

#[test]
fn signature_and_key_rules() {
    let w = World::new();
    let r = Run::new(&w);
    // S3b-1: a body edited after signing.
    let mut s = w.spec();
    s.tamper = Some("L1".into());
    refuses(&w, &r, s, "L1: the receipt's signature does not verify with node node-a's key");
    // S3b-2: a node in no trusted roster.
    let mut s = w.spec();
    let mut rc = w.receipt("rogue", &"c".repeat(32), Isolation::Exclusive);
    rc.role = Role::Verify;
    s.receipts.insert("V1".into(), rc);
    refuses(&w, &r, s, "V1: the receipt is from node");
    // S3b-2: the wrong key claiming node-a's fingerprint.
    let mut s = w.spec();
    l1(&mut s).signer = "rogue".into();
    refuses(&w, &r, s, "does not verify with node node-a's key");
    // §4.1.6: the bundle carries the client server's pairing pins, and it
    // never paired node-a's key.
    // (The person pinned this owner as the client's own server, so M7's rule below doesn't fire first.)
    let mut r_own = Run::new(&w);
    r_own.trust.owners.get_mut(&w.owner_fp).unwrap().own_controllers = vec![w.controller.fingerprint()];
    let mut b = w.build(w.spec());
    b.pairing_pins.remove(&w.fp("node-a"));
    assert!(r_own.go(&b).unwrap_err().contains("L1: the client server never paired node node-a's key"));
    // A pin whose key isn't its fingerprint is a shape error.
    b.pairing_pins.insert(w.fp("node-a"), w.spki("node-b"));
    assert!(b.check().is_err());
    // M7: no pins for nodes another owner runs: refused for a client project …
    let mut s = w.spec();
    s.pairing_pins = false;
    refuses(&w, &r, s, "L1: node node-a belongs to owner.example, and the evidence carries no pairing pin");
    // … even when the other owner's roster claims to be the client (M7 bypass:
    // owner_server_fp is the owner's own word) …
    let mut s = w.spec();
    s.pairing_pins = false;
    s.roster.owner_server_fp = w.controller.fingerprint();
    refuses(&w, &r, s, "carries no pairing pin");
    // … fine for nodes the person pinned as the client's own (trust-owner --own-controller) …
    let mut r_own = Run::new(&w);
    r_own.trust.owners.get_mut(&w.owner_fp).unwrap().own_controllers = vec![w.controller.fingerprint()];
    let mut s = w.spec();
    s.pairing_pins = false;
    passes(&w, &r_own, s);
    // … and when the person made the pins optional for the project.
    let mut r2 = Run::new(&w);
    r2.trust.pairing_pins.insert("acme-desktop".into(), false);
    let mut s = w.spec();
    s.pairing_pins = false;
    passes(&w, &r2, s);
    assert!(Trust::default().pairing_pins_required("acme-desktop") && !Trust::default().pairing_pins_required("k2"));
    // A roster signed by an owner the policy doesn't trust.
    let mut s = w.spec();
    s.roster_signer_is_owner2 = true;
    refuses(&w, &r, s, "which project acme-desktop does not trust");
    // The owner is in the policy but not pinned at the signer.
    let mut r2 = Run::new(&w);
    r2.trust.owners.clear();
    refuses(&w, &r2, w.spec(), "is not pinned at this signer");
}

#[test]
fn roster_change_needs_a_person_unless_auto() {
    let w = World::new();
    let mut r = Run::new(&w);
    r.accept = vec!["node-a"];
    refuses(&w, &r, w.spec(), "roster_change_needs_person: V1: node node-b's key");
    r.policy.roster_changes = Changes::Auto;
    let s = passes(&w, &r, w.spec());
    assert!(s.new_marks[&w.owner_fp].auto_accepted["acme-desktop"].contains_key(&w.fp("node-b")));
    assert!(s.notes.iter().any(|n| n.contains("roster_changes = auto")));
}

/// H1: one project's `auto` never accepts a key for another project that says `ask`.
#[test]
fn auto_acceptance_is_per_project() {
    let w = World::new();
    let mut a = Run::new(&w);
    a.accept = vec!["node-a"];
    a.policy.roster_changes = Changes::Auto;
    let marks = passes(&w, &a, w.spec()).new_marks;
    // Another project on the same owner's nodes, which says ask.
    let mut b = Run::new(&w);
    b.accept = vec!["node-a"];
    b.marks = marks.clone();
    b.policy = k2_signer::policy::parse(&w.policy_text(TEST_PUB).replace("project = \"acme-desktop\"", "project = \"other-app\""), "other-app").unwrap();
    let mut s = w.spec();
    s.project = "other-app".into();
    refuses(&w, &b, s, "roster_change_needs_person: V1: node node-b's key");
    // The same project switched from auto to ask: what auto accepted no longer counts …
    let mut c = Run::new(&w);
    c.accept = vec!["node-a"];
    c.marks = marks.clone();
    refuses(&w, &c, w.spec(), "roster_change_needs_person: V1: node node-b's key");
    // … and the next registration under ask drops it from the mark.
    c.accept = vec!["node-a", "node-b", "node-c"];
    let out = passes(&w, &c, w.spec());
    assert!(!out.new_marks[&w.owner_fp].auto_accepted.contains_key("acme-desktop"), "{:?}", out.new_marks);
}

/// M4: a person's acceptance holds for the attributes they saw.
#[test]
fn acceptance_is_tied_to_the_roster_attributes() {
    let w = World::new();
    let mut r = Run::new(&w);
    r.accept_from_bundle = false; // the person accepted the default roster (host-b for node-b)
    passes(&w, &r, w.spec());
    for (what, f) in [
        ("host", Box::new(|e: &mut k2_node_proto::node_roster::RosterEntry| e.host = "host-z".into()) as Box<dyn Fn(&mut k2_node_proto::node_roster::RosterEntry)>),
        ("provider", Box::new(|e: &mut k2_node_proto::node_roster::RosterEntry| e.provider = Some("other-cloud".into()))),
        ("site", Box::new(|e: &mut k2_node_proto::node_roster::RosterEntry| e.site = Some("site-9".into()))),
        ("admin reach", Box::new(|e: &mut k2_node_proto::node_roster::RosterEntry| e.admin_reach = vec!["contractor-ssh".into()])),
        ("taint basis", Box::new(|e: &mut k2_node_proto::node_roster::RosterEntry| e.taint_basis = Some(TaintBasis::Rekeyed))),
    ] {
        let mut s = w.spec();
        f(entry_mut(&mut s, &w.fp("node-b")));
        let e = r.go(&w.build(s)).map(|_| ()).unwrap_err();
        assert!(e.contains("V1: node node-b") && e.contains("since a person accepted it"), "{what}: {e}");
    }
}

#[test]
fn roster_freshness_and_chain() {
    let w = World::new();
    let mut r = Run::new(&w);
    // Stale.
    let mut s = w.spec();
    s.roster.issued_at = NOW - 169 * 3600;
    refuses(&w, &r, s, "node_roster_stale");
    // From the future.
    let mut s = w.spec();
    s.roster.issued_at = NOW + 3600;
    refuses(&w, &r, s, "issued in the future");
    // Below the registered high-water mark.
    r.marks.insert(w.owner_fp.clone(), mark(2, &format!("sha256:{}", "1".repeat(64))));
    refuses(&w, &r, w.spec(), "node_roster_replayed");
    // Seq 2 that doesn't chain to the registered seq 1.
    let b1 = w.build(w.spec());
    let d1 = b1.rosters[0].digest();
    r.marks.insert(w.owner_fp.clone(), mark(1, &d1));
    let mut s = w.spec();
    s.roster.seq = 2;
    s.roster.prev_digest = Some(format!("sha256:{}", "2".repeat(64)));
    s.seats[0].roster_seq = 2;
    l1(&mut s).receipt.run.as_mut().unwrap().roster_seq_seen = Some(2);
    v1(&mut s).receipt.run.as_mut().unwrap().roster_seq_seen = Some(2);
    refuses(&w, &r, s, "does not chain to the registered seq 1");
    // …and one that does chain passes; its mark rises to 2.
    let mut s = w.spec();
    s.roster.seq = 2;
    s.roster.prev_digest = Some(d1.clone());
    let out = passes(&w, &r, s);
    assert_eq!(out.new_marks[&w.owner_fp].seq, 2);
    // The same seq as registered, another digest: a fork.
    r.marks.insert(w.owner_fp.clone(), mark(1, &format!("sha256:{}", "3".repeat(64))));
    refuses(&w, &r, w.spec(), "a second roster for one seq");
    // A node that had seen a newer roster than the bundle carries.
    let r = Run::new(&w);
    let mut s = w.spec();
    l1(&mut s).receipt.run.as_mut().unwrap().roster_seq_seen = Some(5);
    refuses(&w, &r, s, "node node-a had seen");
    // Two rosters in the bundle that don't chain.
    let mut s = w.spec();
    s.prev_rosters.push(s.roster.clone());
    s.roster.seq = 2;
    s.roster.prev_digest = Some(format!("sha256:{}", "4".repeat(64)));
    refuses(&w, &r, s, "does not chain to seq 1");
}

#[test]
fn key_status_and_taint() {
    let w = World::new();
    let r = Run::new(&w);
    // Revoked (compromised): every receipt void.
    let mut s = w.spec();
    let e = entry_mut(&mut s, &w.fp("node-b"));
    e.status = KeyStatus::Revoked;
    e.revoked_reason = Some(RevokeReason::Compromised);
    refuses(&w, &r, s, "is revoked");
    // Retired before the job started.
    let mut s = w.spec();
    let e = entry_mut(&mut s, &w.fp("node-a"));
    e.status = KeyStatus::Retired;
    e.valid_to = Some(NOW - 7200);
    refuses(&w, &r, s, "was retired");
    // The owner vouches for nothing.
    let mut s = w.spec();
    entry_mut(&mut s, &w.fp("node-a")).taint_basis = None;
    refuses(&w, &r, s, "node_key_tainted: L1");
    // A basis the policy doesn't accept.
    let mut r2 = Run::new(&w);
    r2.policy.taint_basis = vec![TaintBasis::Rekeyed];
    refuses(&w, &r2, w.spec(), "which project acme-desktop does not accept");
    // A tainted key.
    let mut s = w.spec();
    s.statements.get_mut("node-a").unwrap().taint = Taint::Tainted;
    refuses(&w, &r, s, "is tainted");
    // An unknown taint without an owner attestation.
    let mut s = w.spec();
    s.statements.get_mut("node-b").unwrap().taint = Taint::Unknown;
    refuses(&w, &r, s, "predates the taint flag");
    // …with the owner's attestation (V12) it passes.
    let mut s = w.spec();
    s.statements.get_mut("node-b").unwrap().taint = Taint::Unknown;
    s.attest.push(w.attestation("node-b"));
    passes(&w, &r, s);
    // An attestation the bundle lacks.
    let mut s = w.spec();
    s.statements.get_mut("node-b").unwrap().taint = Taint::Unknown;
    s.attest.push(w.attestation("node-b"));
    let mut b = w.build(s);
    b.attestations.clear();
    assert!(r.go(&b).unwrap_err().contains("lacks owner.example's attestation"));
    // "Re-keyed" for an enrolment key.
    let mut s = w.spec();
    entry_mut(&mut s, &w.fp("node-a")).taint_basis = Some(TaintBasis::Rekeyed);
    refuses(&w, &r, s, "it is the enrolment key");
    // A --dev node; a node whose jobs can read its key.
    let mut s = w.spec();
    s.statements.get_mut("node-a").unwrap().dev = true;
    refuses(&w, &r, s, "is a --dev node");
    let mut s = w.spec();
    s.statements.get_mut("node-a").unwrap().key_exposure.job_readable = true;
    refuses(&w, &r, s, "jobs can read its key");
    // The statement the receipt names is missing.
    let mut s = w.spec();
    l1(&mut s).fix_statement_digest = false;
    refuses(&w, &r, s, "lacks node node-a's statement");
    // The statement and the receipt disagree on the k2-node build.
    let mut s = w.spec();
    l1(&mut s).receipt.run.as_mut().unwrap().k2node_version = "0.46.9".into();
    refuses(&w, &r, s, "another k2-node build");
    // A statement issued after the job started.
    let mut s = w.spec();
    s.statements.get_mut("node-a").unwrap().issued_at = NOW - 60;
    refuses(&w, &r, s, "was issued after the job started");
    // Secret builds need ptrace_scope ≥ 1 (V24).
    let mut s = w.spec();
    s.statements.get_mut("node-a").unwrap().ptrace_scope = Some(0);
    refuses(&w, &r, s, "ptrace_scope");
}

#[test]
fn seats_and_lanes() {
    let w = World::new();
    let r = Run::new(&w);
    let mut s = w.spec();
    s.seats.clear();
    refuses(&w, &r, s, "L1: node node-a is not on an active seat");
    let mut s = w.spec();
    s.seats[0].nodes = vec!["node-a".into(), "node-c".into()];
    refuses(&w, &r, s, "V1: node node-b is not on an active seat");
    let mut s = w.spec();
    s.seats[0].state = k2_node_proto::ci::SeatState::Suspended;
    refuses(&w, &r, s, "not on an active seat");
    let mut s = w.spec();
    l1(&mut s).receipt.run.as_mut().unwrap().lane = Some("other-lane".into());
    refuses(&w, &r, s, "not the seat's lane ci-standard");
}

#[test]
fn source_rules() {
    let w = World::new();
    let r = Run::new(&w);
    // S3b-3: the wrong tree.
    let mut s = w.spec();
    l1(&mut s).receipt.src.as_mut().unwrap().tree = "9".repeat(40);
    refuses(&w, &r, s, "L1: the job built commit");
    // The forge's tree is not the bundle's.
    let mut r2 = Run::new(&w);
    r2.source.as_mut().unwrap().tree = "9".repeat(40);
    refuses(&w, &r2, w.spec(), "the evidence built tree");
    // A preview commit whose parent is not the approved base.
    let mut r3 = Run::new(&w);
    r3.source = Some(ReleaseSource { tag_commit: "7".repeat(40), tree: w.tree.clone(), base: "1".repeat(40), rust_pin: Some("1.99.1".into()) });
    refuses(&w, &r3, w.spec(), "not the approved base");
    // …with no commit object to bind it.
    let mut r3 = Run::new(&w);
    r3.source = Some(ReleaseSource { tag_commit: "7".repeat(40), tree: w.tree.clone(), base: w.base.clone(), rust_pin: Some("1.99.1".into()) });
    let mut s = w.spec();
    s.commit_object = false;
    refuses(&w, &r3, s, "no commit object to bind it");
    // An edited commit object no longer hashes to the commit (bundle shape).
    let mut b = w.build(w.spec());
    b.source.commit_object = Some(k2_node_proto::crypto::b64(b"tree 9999999999999999999999999999999999999999\n\nx\n"));
    assert!(r.go(&b).unwrap_err().contains("does not hash to source.commit"));
    // --dirty.
    let mut s = w.spec();
    l1(&mut s).receipt.src.as_mut().unwrap().dirty_sha256 = Some("d".repeat(64));
    refuses(&w, &r, s, "ran uncommitted changes");
    // A version the evidence is not for.
    let mut s = w.spec();
    s.version = Some("1.2.4".into());
    refuses(&w, &r, s, "not 1.2.3");
}

#[test]
fn what_ran_rules() {
    let w = World::new();
    let r = Run::new(&w);
    let mut s = w.spec();
    l1(&mut s).receipt.argv_sha256 = "0".repeat(64);
    refuses(&w, &r, s, "did not run build linux-x86_64's command");
    let mut s = w.spec();
    l1(&mut s).receipt.exit = Some(ExitInfo { code: Some(1), signal: None });
    refuses(&w, &r, s, "did not end done/exit 0");
    let mut s = w.spec();
    l1(&mut s).receipt.state = JobState::Failed;
    refuses(&w, &r, s, "did not end done/exit 0");
    let mut s = w.spec();
    l1(&mut s).receipt.env_names.push("K2_JOB_CACHE".into());
    refuses(&w, &r, s, "ran with a --cache mount");
    let mut s = w.spec();
    l1(&mut s).receipt.env_names.push("RUSTFLAGS".into());
    refuses(&w, &r, s, "env the build doesn't set: RUSTFLAGS");
    let mut s = w.spec();
    l1(&mut s).receipt.env_names.push("BUILD_OAUTH".into());
    refuses(&w, &r, s, "env the build doesn't set: BUILD_OAUTH");
    // H2: warm / unknown warmth.
    let mut s = w.spec();
    l1(&mut s).receipt.src.as_mut().unwrap().warmth = Some("warm".into());
    refuses(&w, &r, s, "built in a shared target dir");
    let mut s = w.spec();
    l1(&mut s).receipt.src.as_mut().unwrap().warmth = None;
    refuses(&w, &r, s, "built in a shared target dir (warmth None)");
    // The rustc that ran is not the pin.
    let mut s = w.spec();
    l1(&mut s).receipt.tools_used.insert("rustc".into(), "1.98.0".into());
    refuses(&w, &r, s, "L1: the job's rustc was 1.98.0, the commit pins 1.99.1");
    // A workspace the policy doesn't name (V30).
    let mut s = w.spec();
    l1(&mut s).receipt.workspace_id = "ws-other".into();
    refuses(&w, &r, s, "workspace ws-other may not build");
    // A v1 receipt is k2sign's.
    let mut s = w.spec();
    l1(&mut s).receipt.v = 1;
    l1(&mut s).receipt.run = None;
    refuses(&w, &r, s, "k2-signer checks receipt v2");
    // A build the policy doesn't have.
    let mut s = w.spec();
    l1(&mut s).build = "other-build".into();
    v1(&mut s).build = "other-build".into();
    refuses(&w, &r, s, "build other-build is not in the policy");
}

/// L2: an acceptance covers the status and validity the person saw; a wider
/// one asks again, a narrower one doesn't, and a key once revoked never
/// comes back.
#[test]
fn widening_asks_again_and_revoked_keys_stay_out() {
    let w = World::new();
    let base = |entry: k2_node_proto::node_roster::RosterEntry| {
        let mut r = Run::new(&w);
        r.accept_from_bundle = false;
        let fp = entry.node_fp.clone();
        r.trust.owners.get_mut(&w.owner_fp).unwrap().accepted.insert(fp, k2_signer::trust::AcceptedKey::of(&entry, 1, NOW - 86_400));
        r
    };
    // Accepted while retired; now active again (an un-retire).
    let mut retired = w.entry("node-b", "host-b");
    retired.status = KeyStatus::Retired;
    retired.valid_to = Some(NOW - 100);
    refuses(&w, &base(retired), w.spec(), "V1: node node-b");
    // Accepted from valid_from T; now an earlier valid_from.
    let r = base(w.entry("node-b", "host-b"));
    let mut s = w.spec();
    entry_mut(&mut s, &w.fp("node-b")).valid_from = NOW - 300_000;
    refuses(&w, &r, s, "status or validity widened");
    // Narrowing (retired after the job started) needs no one.
    let mut s = w.spec();
    let e = entry_mut(&mut s, &w.fp("node-b"));
    e.status = KeyStatus::Retired;
    e.valid_to = Some(NOW - 60);
    passes(&w, &r, s);
    // A key a registered roster showed revoked never comes back …
    let mut r2 = Run::new(&w);
    let mut m = mark(1, &w.build(w.spec()).rosters[0].digest());
    m.revoked.insert(w.fp("node-b"));
    r2.marks.insert(w.owner_fp.clone(), m);
    refuses(&w, &r2, w.spec(), "was revoked in an earlier roster");
    // … nor one the person saw revoked at roster-accept.
    let mut r3 = Run::new(&w);
    r3.trust.owners.get_mut(&w.owner_fp).unwrap().revoked.insert(w.fp("node-b"));
    refuses(&w, &r3, w.spec(), "was revoked in an earlier roster");
    // A registration remembers revocations in the mark.
    let mut s = w.spec();
    let e = entry_mut(&mut s, &w.fp("node-c"));
    e.status = KeyStatus::Revoked;
    e.revoked_reason = Some(RevokeReason::Compromised);
    let out = passes(&w, &Run::new(&w), s);
    assert!(out.new_marks[&w.owner_fp].revoked.contains(&w.fp("node-c")));
}

/// M1: the controller picks env values; only the build facts may carry one.
#[test]
fn env_values() {
    let w = World::new();
    let mut r = Run::new(&w);
    let out = passes(&w, &r, w.spec());
    assert_eq!(out.jobs["L1"].env["BUILD_COMMIT"], w.commit);
    assert_eq!(out.jobs["L1"].env["BUILD_TREE"], w.tree);
    r.policy.build.get_mut("linux-x86_64").unwrap().env.push("BUILD_FLAVOR".into());
    let with_flavor = |v: &str| {
        let mut s = w.spec();
        for rc in s.receipts.values_mut() {
            rc.plan["env"]["BUILD_FLAVOR"] = json!(v);
            rc.receipt.env_names.push("BUILD_FLAVOR".into());
            rc.receipt.env_names.sort();
        }
        s
    };
    refuses(&w, &r, with_flavor("--features=evil"), "the plan sets BUILD_FLAVOR=\"--features=evil\": only BUILD_COMMIT and BUILD_TREE carry values");
    let out = passes(&w, &r, with_flavor(""));
    assert_eq!(out.jobs["V1"].env["BUILD_FLAVOR"], "");
}

/// L1: node-set K2_* names are an explicit list.
#[test]
fn node_env_names_are_an_explicit_list() {
    let w = World::new();
    let r = Run::new(&w);
    let mut s = w.spec();
    l1(&mut s).receipt.env_names.push("K2_JOB_CPUS".into());
    passes(&w, &r, s);
    let mut s = w.spec();
    l1(&mut s).receipt.env_names.push("K2_EXTRA_SWITCH".into());
    refuses(&w, &r, s, "env the build doesn't set: K2_EXTRA_SWITCH");
}

/// L3: a seat statement expires with the roster's max age.
#[test]
fn old_seats_expire() {
    let w = World::new();
    let r = Run::new(&w);
    let mut s = w.spec();
    s.seats[0].issued_at = NOW - 169 * 3600;
    refuses(&w, &r, s, "seat seat-1 from owner.example was issued");
}

#[test]
fn isolation_rules() {
    let w = World::new();
    let r = Run::new(&w);
    // M4: a secret job that shared its tenant uid.
    let mut s = w.spec();
    let run = l1(&mut s).receipt.run.as_mut().unwrap();
    run.isolation = Isolation::TenantUid;
    run.uid_index = None;
    refuses(&w, &r, s, "L1: the job ran with isolation TenantUid");
    // …or behind the Seatbelt wall the build doesn't accept.
    let mut s = w.spec();
    s.statements.get_mut("node-b").unwrap().wall = Wall::None;
    v1(&mut s).receipt.run.as_mut().unwrap().wall = Wall::None;
    refuses(&w, &r, s, "V1: the job ran behind wall None");
}

#[test]
fn time_rules() {
    let w = World::new();
    let mut r = Run::new(&w);
    // S3b-7: started before this version's first approval.
    r.opened = Some(NOW - 600);
    refuses(&w, &r, w.spec(), "L1: an old receipt: the job started");
    // Ended over receipt_max_hours ago.
    let r = Run::new(&w);
    let mut s = w.spec();
    l1(&mut s).receipt.started_at = Some(NOW - 26 * 3600);
    l1(&mut s).receipt.ended_at = NOW - 25 * 3600;
    refuses(&w, &r, s, "over 24 hours ago");
    // Ends in the future.
    let mut s = w.spec();
    l1(&mut s).receipt.ended_at = NOW + 3600;
    refuses(&w, &r, s, "ends in the future");
}

#[test]
fn plan_rules() {
    let w = World::new();
    let r = Run::new(&w);
    let with_plan = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut s = w.spec();
        f(&mut l1(&mut s).plan);
        s
    };
    // H1: a plan that doesn't hash to the receipt's plan_digest.
    let mut s = w.spec();
    l1(&mut s).fix_plan_digest = false;
    l1(&mut s).receipt.plan_digest = format!("sha256:{}", "a".repeat(64));
    refuses(&w, &r, s, "does not hash to the receipt's plan_digest");
    // H1 exploit: a --with repo laid into the build.
    refuses(&w, &r, with_plan(&|p| p["with"] = json!([{"path": "crates/k2-daemon", "project_key": "e".repeat(64), "remote_url": null, "commit": "1".repeat(40)}])), "ships --with repos");
    refuses(&w, &r, with_plan(&|p| p["src_path"] = json!("k2")), "has a --with layout (src_path \"k2\")");
    refuses(&w, &r, with_plan(&|p| p["cwd"] = json!("crates")), "runs in another folder (cwd \"crates\")");
    refuses(&w, &r, with_plan(&|p| p["cache"] = json!("target")), "mounts a --cache (\"target\")");
    refuses(&w, &r, with_plan(&|p| p["git_access"] = json!(["github.com/acme/secret"])), "reads other repos with a git token");
    refuses(&w, &r, with_plan(&|p| p["packages"] = json!(["evil-tools"])), "installs packages beyond the build's needs: evil-tools");
    refuses(&w, &r, with_plan(&|p| p["needs"] = json!(["rust@stable"])), "the plan needs rust@stable, the build needs rust@1.99.1");
    refuses(&w, &r, with_plan(&|p| p["needs"] = json!(["rust@1.99.1", "cmake@3"])), "the build needs rust@1.99.1");
    refuses(&w, &r, with_plan(&|p| p["env"]["BUILD_API_KEY"] = json!("value")), "the plan sets env the build doesn't: BUILD_API_KEY");
    refuses(&w, &r, with_plan(&|p| p["env"]["BUILD_COMMIT"] = json!("1".repeat(40))), "BUILD_COMMIT/BUILD_TREE are not the commit and tree it ran");
    refuses(&w, &r, with_plan(&|p| p["artifacts"] = json!(["dist/*", "target/*"])), "the plan collects");
    refuses(&w, &r, with_plan(&|p| p["future_switch"] = json!(true)), "fields the signer doesn't know");
    refuses(&w, &r, with_plan(&|p| p["argv"] = json!(["bash", "-c", "evil"])), "the plan's command is not the build's command");
    refuses(&w, &r, with_plan(&|p| p["receipt_v"] = json!(null)), "fields the signer doesn't know");
    refuses(&w, &r, with_plan(&|p| { p.as_object_mut().unwrap().remove("receipt_v"); }), "did not ask for receipt v2");
    refuses(&w, &r, with_plan(&|p| { p.as_object_mut().unwrap().remove("exclusive"); }), "is not a job plan");
    refuses(&w, &r, with_plan(&|p| p["job_id"] = json!("f".repeat(32))), "the plan is for job");
    refuses(&w, &r, with_plan(&|p| p["src"]["commit"] = json!("1".repeat(40))), "the plan's source is not the commit the receipt ran");
    // H2: the secrets.
    let mut s = w.spec();
    l1(&mut s).plan.as_object_mut().unwrap().remove("secret_env");
    l1(&mut s).receipt.secret_env = vec![];
    refuses(&w, &r, s, "did not get its vault secrets (secret_env lacks BUILD_API_KEY)");
    let mut s = w.spec();
    l1(&mut s).plan["secret_env"] = json!(["BUILD_API_KEY", "BUILD_SIGNING_KEY"]);
    l1(&mut s).receipt.secret_env = vec!["BUILD_API_KEY".into(), "BUILD_SIGNING_KEY".into()];
    refuses(&w, &r, s, "vault secrets the build doesn't use: BUILD_SIGNING_KEY");
    let mut s = w.spec();
    l1(&mut s).receipt.secret_env = vec!["BUILD_API_KEY".into(), "BUILD_X".into()];
    refuses(&w, &r, s, "the plan's vault secrets are not the receipt's");
}

#[test]
fn dual_build_rules() {
    let w = World::new();
    let r = Run::new(&w);
    // S3b-5: no verify receipt.
    let mut s = w.spec();
    s.receipts.remove("V1");
    refuses(&w, &r, s, "no verify receipt for build linux-x86_64");
    // V1 did not build the dual file.
    let mut s = w.spec();
    v1(&mut s).receipt.artifacts.clear();
    refuses(&w, &r, s, "V1 match missing");
    // S3b-6: V1 built other bytes.
    let mut s = w.spec();
    v1(&mut s).receipt.artifacts[0].sha256 = "1".repeat(64);
    refuses(&w, &r, s, "V1 mismatch: dist/acme differs between L1 on node-a");
    // V1 on L1's node; V1 the same job.
    let mut s = w.spec();
    let mut rc = w.receipt("node-a", &"c".repeat(32), Isolation::Exclusive);
    rc.role = Role::Verify;
    s.receipts.insert("V1".into(), rc);
    refuses(&w, &r, s, "V1 ran on the same node as L1 (node-a)");
    let mut s = w.spec();
    v1(&mut s).receipt.job_id = "a".repeat(32);
    v1(&mut s).plan["job_id"] = json!("a".repeat(32));
    refuses(&w, &r, s, "V1 is the same job as L1");
    // V1 got other secrets.
    let mut r2 = Run::new(&w);
    r2.policy.build.get_mut("linux-x86_64").unwrap().secrets_optional = vec!["BUILD_MS_ID".into()];
    let mut s = w.spec();
    v1(&mut s).receipt.secret_env = vec!["BUILD_API_KEY".into(), "BUILD_MS_ID".into()];
    v1(&mut s).plan["secret_env"] = json!(["BUILD_API_KEY", "BUILD_MS_ID"]);
    refuses(&w, &r2, s, "V1 got other vault secrets than L1");
    // Independence: V1 on node-c, the same host as node-a.
    let on_c = || {
        let mut s = w.spec();
        let mut rc = w.receipt("node-c", &"c".repeat(32), Isolation::Exclusive);
        rc.role = Role::Verify;
        s.receipts.insert("V1".into(), rc);
        s
    };
    refuses(&w, &r, on_c(), "no_independent_nodes: V1 is not independent of L1: both ran on host host-a");
    // …fine when the policy only asks for another node.
    let mut r3 = Run::new(&w);
    r3.policy.build.get_mut("linux-x86_64").unwrap().independent = vec![Dimension::Node];
    passes(&w, &r3, on_c());
    // Site and admin reach the roster doesn't separate.
    let mut r4 = Run::new(&w);
    r4.policy.build.get_mut("linux-x86_64").unwrap().independent = vec![Dimension::Host, Dimension::Site];
    refuses(&w, &r4, w.spec(), "both at site site-1");
    r4.policy.build.get_mut("linux-x86_64").unwrap().independent = vec![Dimension::AdminReach];
    refuses(&w, &r4, w.spec(), "fleet-ssh holds root on both");
    r4.policy.build.get_mut("linux-x86_64").unwrap().independent = vec![Dimension::Owner];
    refuses(&w, &r4, w.spec(), "one owner's word for both");
}

#[test]
fn signer_rules_on_policies() {
    let w = World::new();
    // A signed asset that no verify build reproduces.
    let text = w.policy_text(TEST_PUB).replace("dual = [\"dist/acme\"]\nindependent = [\"host\"]\n", "");
    let e = k2_signer::policy::parse(&text, "acme-desktop").unwrap_err();
    assert!(e.message().contains("not in build linux-x86_64's dual list"), "{e}");
    // inner_binary waits for C8.
    let text = w.policy_text(TEST_PUB).replace("check = \"artifact\"", "check = \"inner_binary\"\ninner = \"usr/bin/acme\"");
    assert!(k2_signer::policy::parse(&text, "acme-desktop").unwrap_err().message().contains("inner_binary"));
    // The file name must be the project.
    assert!(k2_signer::policy::parse(&w.policy_text(TEST_PUB), "other").is_err());
    // L4: the repo and tag prefix that reach GitHub API paths are a strict charset.
    for bad in ["acme/desktop?ref=x", "acme/../x", "acme/desk top", "acme/%2e%2e"] {
        let text = w.policy_text(TEST_PUB).replace("repo = \"acme/desktop\"", &format!("repo = \"{bad}\""));
        assert!(k2_signer::policy::parse(&text, "acme-desktop").is_err(), "{bad}");
    }
    for bad in ["v/../", "v?", "release v"] {
        let text = w.policy_text(TEST_PUB).replace("tag_prefix = \"v\"", &format!("tag_prefix = \"{bad}\""));
        let e = k2_signer::policy::parse(&text, "acme-desktop").unwrap_err();
        assert!(e.message().contains("tag_prefix"), "{bad}: {e}");
    }
    let text = w.policy_text(TEST_PUB).replace("tag_prefix = \"v\"", "tag_prefix = \"release-\"");
    k2_signer::policy::parse(&text, "acme-desktop").unwrap();
    // A key that is not a minisign key.
    let text = w.policy_text("not-a-key");
    assert!(k2_signer::policy::parse(&text, "acme-desktop").unwrap_err().message().contains("keys.updater.public"));
}

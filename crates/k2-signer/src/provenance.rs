//! The signer's build-statement rules (prd-compute-build-attestations-v1
//! §3.3, §7.2, slice S4): each receipt's statement, from an evidence bundle
//! v3, against the receipt the signer has already checked, the pinned
//! owner's roster entry for its key and the project's policy.
//!
//! [`crate::check::check`] runs these after every receipt rule has passed,
//! so the receipt, its key and its roster entry are already trusted here.
//! What a statement adds is checked in this order, and each check is
//! reported in plain words ([`StatementReport`]):
//!
//! 1. **signature**: the DSSE envelope verifies with the receipt's node key
//!    (the one in the pinned owner's roster), and only with it.
//! 2. **receipt**: the statement names this receipt and agrees with it
//!    ([`k2_node_proto::provenance::Dsse::open_for_receipt`]): the same
//!    files and sha256s, commit and tree, command, plan, ids and times.
//! 3. **owner**: `builder.id` names the pinned roster key of the owner whose
//!    roster lists the node, never the `owner-server` form
//!    ([`k2_node_proto::provenance::Statement::check_builder_owner`]).
//! 4. **key active**: the node key was active in that roster for the whole
//!    job the statement records (valid from before it started, not
//!    revoked, not retired before it ended, when the statement was signed).
//! 5. **time**: the statement's times are the node's clock, so the signer
//!    anchors them (BA-31): the job started after this version's first
//!    approval and did not end in the future.
//! 6. **k2-node** (`min_k2node`): the node build it names is at least the
//!    policy's.
//! 7. **network** (`[build.<name>].network`): the network it ran under is
//!    one the build accepts.
//!
//! `provenance = "off"` reads no statement (today's rule). `"prefer"`
//! checks every statement that is there and refuses one that fails; a
//! receipt without one passes with a note. `"required"` refuses a receipt
//! without one, and a v2 bundle (which can't carry any).

use k2_node_proto::ci::KeyStatus;
use k2_node_proto::evidence::{EvidenceBundle, EvidenceReceipt, EVIDENCE_V2};
use k2_node_proto::frames::Receipt;
use k2_node_proto::node_roster::RosterEntry;
use k2_node_proto::provenance::parse_rfc3339;
use k2_node_proto::signer_policy::{k2node_at_least, BuildRule, ProvenanceRule, SignerPolicy};
use serde::Serialize;

use crate::check::SKEW_SECS;
use crate::ctx::hhmm;
use crate::{refused, Fail, R};

/// Refusal code: `provenance = "required"` and a bundle v2.
pub const PROVENANCE_V2_BUNDLE: &str = "provenance_v2_bundle";
/// Refusal code: a receipt without the statement the policy needs.
pub const PROVENANCE_MISSING: &str = "provenance_missing";
/// Refusal code: the statement's signature, or its agreement with the
/// receipt (PRD §14 `provenance_mismatch`).
pub const PROVENANCE_MISMATCH: &str = "provenance_mismatch";
/// Refusal code: `builder.id` is not the pinned owner's roster key.
pub const PROVENANCE_OWNER: &str = "provenance_owner";
/// Refusal code: the node key was not active for the job the statement
/// records, or its times are outside the approval window.
pub const PROVENANCE_KEY_INACTIVE: &str = "provenance_key_inactive";
/// Refusal code: the statement's network is not one the build accepts.
pub const PROVENANCE_NETWORK: &str = "provenance_network";
/// Refusal code: the job ran on a k2-node older than `min_k2node`.
pub const K2NODE_TOO_OLD: &str = "k2node_too_old";

/// What happened to one receipt's statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementStatus {
    /// `provenance = "off"`: not read.
    NotRead,
    /// `provenance = "prefer"` and the bundle has none for this receipt.
    Absent,
    /// Every check passed.
    Checked,
}

/// One check, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatementCheck {
    pub check: String,
    pub ok: bool,
    pub detail: String,
}

/// The statement checks of one receipt, for the verdict and the summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatementReport {
    pub status: StatementStatus,
    pub checks: Vec<StatementCheck>,
}

impl StatementReport {
    fn of(status: StatementStatus) -> Self {
        Self { status, checks: Vec::new() }
    }

    /// One line: `signature ok; receipt ok; …` (or why nothing was checked).
    pub fn line(&self) -> String {
        match self.status {
            StatementStatus::NotRead => "build statement not read (provenance = off)".into(),
            StatementStatus::Absent => "no build statement (provenance = prefer: the receipt was checked as before)".into(),
            StatementStatus::Checked => format!(
                "build statement checked: {}",
                self.checks.iter().map(|c| format!("{} {}", c.check, if c.ok { "passed" } else { "failed" })).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

/// The bundle-wide rule, before any receipt: under `required` a v2 bundle
/// is refused by name; under `off` the statements a v3 bundle carries are
/// said to be unread.
pub fn bundle_gate(p: &SignerPolicy, b: &EvidenceBundle, notes: &mut Vec<String>) -> R<()> {
    match p.provenance {
        ProvenanceRule::Required if b.v == EVIDENCE_V2 => refused(format!(
            "{PROVENANCE_V2_BUNDLE}: the evidence is a v2 bundle, which carries no build statements, and project {} requires one per receipt \
             (provenance = \"required\"): write it again with k2 ci evidence on a K2 server that writes bundle v3",
            p.project
        )),
        ProvenanceRule::Off => {
            let n = b.provenance.as_ref().map_or(0, |m| m.len());
            if n > 0 {
                notes.push(format!("provenance = off: the bundle's {n} build statement(s) were not read"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// One receipt, after the signer's receipt rules passed for it.
pub struct Job<'a> {
    pub label: &'a str,
    pub build: &'a str,
    pub er: &'a EvidenceReceipt,
    /// The verified receipt.
    pub rc: &'a Receipt,
    pub rule: &'a BuildRule,
    /// Its key's entry in the pinned owner's newest roster.
    pub entry: &'a RosterEntry,
    /// That owner's pinned roster key fingerprint, its id and the roster's seq.
    pub owner_fp: &'a str,
    pub owner_id: &'a str,
    pub roster_seq: u64,
    /// The version's first approval (a release); `None` in a rehearsal.
    pub opened: Option<i64>,
    pub now: i64,
}

struct Checks<'a> {
    label: &'a str,
    done: Vec<StatementCheck>,
}

impl Checks<'_> {
    fn pass(&mut self, check: &str, detail: String) {
        self.done.push(StatementCheck { check: check.into(), ok: true, detail });
    }

    /// The refusal for a failed check: its code, the receipt, the check and
    /// why, then the checks that passed before it.
    fn fail<T>(&self, code: &str, check: &str, detail: &str) -> R<T> {
        let passed = if self.done.is_empty() {
            String::new()
        } else {
            format!(" (passed: {})", self.done.iter().map(|c| c.check.as_str()).collect::<Vec<_>>().join(", "))
        };
        refused(format!("{code}: {}: build statement: {check} failed: {}{passed}", self.label, detail.trim_start_matches("provenance: ")))
    }
}

fn fp16(s: &str) -> &str {
    &s[..s.len().min(16)]
}

/// The statement rules for one receipt (and `min_k2node`, which holds for
/// every receipt whatever `provenance` says).
pub fn check_job(p: &SignerPolicy, b: &EvidenceBundle, j: &Job, notes: &mut Vec<String>) -> R<StatementReport> {
    let label = j.label;
    let name = &j.entry.name;
    let run = j.rc.run.as_ref().ok_or_else(|| Fail::Internal("a checked receipt without run facts".into()))?;
    if let Some(min) = &p.min_k2node {
        match k2node_at_least(&run.k2node_version, min) {
            Some(true) => {}
            Some(false) => {
                return refused(format!(
                    "{K2NODE_TOO_OLD}: {label}: node {name} ran k2-node {}, and project {} accepts {min} or later (min_k2node)",
                    run.k2node_version, p.project
                ))
            }
            None => {
                return refused(format!("{K2NODE_TOO_OLD}: {label}: node {name}'s k2-node version {:?} doesn't read as one (min_k2node {min})", run.k2node_version))
            }
        }
    }
    if p.provenance.is_off() {
        return Ok(StatementReport::of(StatementStatus::NotRead));
    }
    let Some(env) = b.provenance.as_ref().and_then(|m| m.get(label)) else {
        let why = if b.v == EVIDENCE_V2 { "the bundle is v2, which carries no build statements" } else { "the bundle has no build statement for it" };
        if p.provenance == ProvenanceRule::Required {
            return refused(format!(
                "{PROVENANCE_MISSING}: {label}: {why}, and project {} requires one per receipt (provenance = \"required\"): \
                 the job runs on a node with k2-node 0.46.13 or later (feature provenance-v1), and k2 ci evidence carries its statement",
                p.project
            ));
        }
        if !j.rule.network.is_empty() {
            return refused(format!(
                "{PROVENANCE_MISSING}: {label}: {why}, and build {} accepts only some networks (network = [{}]), which only a build statement records",
                j.build,
                j.rule.network.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        notes.push(format!("{label}: {why} (node {name}; provenance = prefer): its receipt was checked as before"));
        return Ok(StatementReport::of(StatementStatus::Absent));
    };

    let mut c = Checks { label, done: Vec::new() };
    let der = j.entry.spki_der().map_err(Fail::Refused)?;
    let fp = &j.entry.node_fp;

    // 1. Signature.
    if let Err(e) = env.open(&der) {
        return c.fail(PROVENANCE_MISMATCH, "signature", &format!("{e} (node {name}'s key {}…, the key that signed its receipt)", fp16(fp)));
    }
    c.pass("signature", format!("signed by node {name}'s key {}…, the key {}'s roster lists and that signed the receipt", fp16(fp), j.owner_id));

    // 2. Agreement with the receipt.
    let st = match env.open_for_receipt(&der, &j.er.signed()) {
        Ok(st) => st,
        Err(e) => return c.fail(PROVENANCE_MISMATCH, "receipt", &e),
    };
    c.pass(
        "receipt",
        format!(
            "it names this receipt and agrees with it: {} file(s) and their sha256, the commit and tree, the command, the plan and the times",
            st.subject.len()
        ),
    );

    // 3. Owner.
    if let Err(e) = st.check_builder_owner(j.owner_fp) {
        return c.fail(PROVENANCE_OWNER, "owner", &e);
    }
    c.pass("owner", format!("builder.id names {}'s roster key {}…, which this signer pins", j.owner_id, fp16(j.owner_fp)));

    // 4. The key, active for the whole job the statement records.
    let md = &st.predicate.run_details.metadata;
    let (Some(started), Some(finished)) = (parse_rfc3339(&md.started_on), parse_rfc3339(&md.finished_on)) else {
        return c.fail(PROVENANCE_KEY_INACTIVE, "key active", "its times don't read as RFC 3339");
    };
    let e = j.entry;
    let roster = format!("{}'s roster seq {}", j.owner_id, j.roster_seq);
    match e.status {
        KeyStatus::Revoked => {
            return c.fail(PROVENANCE_KEY_INACTIVE, "key active", &format!("node {name}'s key is revoked in {roster}"));
        }
        KeyStatus::Retired => {
            let to = e.valid_to.unwrap_or(i64::MIN);
            if finished > to {
                return c.fail(
                    PROVENANCE_KEY_INACTIVE,
                    "key active",
                    &format!(
                        "node {name}'s key was retired {} in {roster}, before the job ended ({}) and the statement was signed",
                        if to == i64::MIN { "with no end time".to_string() } else { hhmm(to) },
                        hhmm(finished)
                    ),
                );
            }
        }
        KeyStatus::Active => {}
    }
    if started < e.valid_from {
        return c.fail(PROVENANCE_KEY_INACTIVE, "key active", &format!("the job started ({}) before node {name}'s key was valid in {roster} ({})", hhmm(started), hhmm(e.valid_from)));
    }
    c.pass(
        "key active",
        format!(
            "node {name}'s key is {} in {roster} for the whole job ({} to {})",
            match e.status {
                KeyStatus::Retired => "retired, but still valid",
                _ => "active",
            },
            hhmm(started),
            hhmm(finished)
        ),
    );

    // 5. Time, anchored to the approval (BA-31).
    if finished < started {
        return c.fail(PROVENANCE_KEY_INACTIVE, "time", "the job ends before it starts");
    }
    if finished > j.now + SKEW_SECS {
        return c.fail(PROVENANCE_KEY_INACTIVE, "time", &format!("the job ends in the future ({})", hhmm(finished)));
    }
    match j.opened {
        Some(o) if started < o - SKEW_SECS => {
            return c.fail(
                PROVENANCE_KEY_INACTIVE,
                "time",
                &format!("the job started {}, before this version's first approval ({}): the node's clock is not trusted alone", hhmm(started), hhmm(o)),
            );
        }
        Some(o) => c.pass("time", format!("the job started after this version's first approval ({}) and ended in the past", hhmm(o))),
        None => c.pass("time", "ended in the past (a rehearsal has no approval to anchor its start)".into()),
    }

    // 6. k2-node.
    if let Some(min) = &p.min_k2node {
        let have = st.predicate.run_details.builder.version.get("k2-node").map(String::as_str).unwrap_or_default();
        if k2node_at_least(have, min) != Some(true) {
            return c.fail(K2NODE_TOO_OLD, "k2-node", &format!("it names k2-node {have:?}, and project {} accepts {min} or later", p.project));
        }
        c.pass("k2-node", format!("k2-node {have}, at least {min}"));
    }

    // 7. Network.
    if !j.rule.network.is_empty() {
        let net = st.predicate.build_definition.internal_parameters.network;
        let want = j.rule.network.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ");
        if !j.rule.network.contains(&net) {
            return c.fail(PROVENANCE_NETWORK, "network", &format!("the job ran with network {}, and build {} accepts {want}", net.as_str(), j.build));
        }
        c.pass("network", format!("the job ran with network {}, which build {} accepts ({want})", net.as_str(), j.build));
    }

    Ok(StatementReport { status: StatementStatus::Checked, checks: c.done })
}

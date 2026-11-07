//! Every evidence rule, as one pure function ([`check`]): an evidence bundle
//! v2 (`k2_node_proto::evidence`) against a project's policy, the person's
//! pins and the signer's roster marks. Nothing in the bundle is trusted
//! because it is in the bundle: each receipt, statement, roster, seat and
//! attestation carries its own signature, and the keys come from the pins.
//!
//! This is the port of k2sign's S3b `compute_check` / `check_plan` (and the
//! S3b review fixes H1, H2, M2, M4, L3) onto receipt v2 and owner node
//! rosters (PRD §4.1, §4.5, §4.6; vs-live V12, V13, V28, V30, S1, S6):
//!
//! 1. **Controller.** The bundle's controller is in the policy's
//!    `controllers` **and** paired with this signer; every receipt names it
//!    and every seat is this client's.
//! 2. **Node rosters.** Each roster verifies with a pinned owner key the
//!    policy lists; it is fresh (`roster_max_age_hours`), never below the
//!    highest `seq` registered (`node_roster_replayed`), never a second
//!    roster for a registered `seq`, and chains by `prev_digest` where the
//!    bundle carries the chain (V13).
//! 3. **Node keys.** A receipt's key is in exactly one trusted roster, equals
//!    the client server's pairing pin when the bundle has one (§4.1.6), was
//!    accepted by a person (`roster_changes = ask`, S1) or auto, is not
//!    revoked, not used after `valid_to`, carries a taint basis the policy
//!    accepts, and its self-statement is not `dev`, not tainted, not
//!    job-readable, behind an accepted wall.
//! 4. **Seats.** The node is on an active seat of this client, under the
//!    seat's lane.
//! 5. **Receipts.** v2 only; done/exit 0; the policy's command, commit and
//!    tree, clean, private warmth; only the policy's env; the isolation the
//!    policy accepts (`job_uid` or `exclusive` for secret builds, with
//!    `ptrace_scope ≥ 1`); fresh; the pinned rustc.
//! 6. **Plans.** The plan hashes to the receipt's `plan_digest` and holds
//!    nothing that changes the build (no `--with`, cwd, cache, git access;
//!    the policy's needs, artifacts, env and vault secrets exactly).
//! 7. **Dual builds.** Every dual build has a verify receipt on an
//!    independent node (policy dimensions; `node` implied) whose `dual`
//!    files match byte for byte.

use std::collections::{BTreeMap, BTreeSet};

use k2_node_proto::canonical;
use k2_node_proto::ci::{codes, Dimension, Isolation, KeyStatus, Signed, Taint, TaintBasis, Wall};
use k2_node_proto::evidence::{EvidenceBundle, EvidenceReceipt, Role};
use k2_node_proto::frames::{argv_sha256, JobState, Plan, Receipt, RECEIPT_V2};
use k2_node_proto::node_roster::{NodeRoster, OwnerAttestation, RosterEntry, SeatStatement};
use k2_node_proto::signer_policy::{BuildRule, Changes, SignerPolicy};
use k2_node_proto::statement::{NodeStatement, RotationStatement};
use serde::Serialize;

use crate::ctx::{hhmm, iso};
use crate::forge::ReleaseSource;
use crate::trust::{AcceptedKey, RosterMark, Trust};
use crate::{refused, Fail, R};

/// Clock skew allowed for "ends in the future" and the session window.
pub const SKEW_SECS: i64 = 600;
/// A roster issued this far in the future is refused.
pub const ROSTER_FUTURE_SECS: i64 = 600;
/// The `--cache` mount's env name.
pub const JOB_CACHE_ENV: &str = "K2_JOB_CACHE";
/// Node-set names a receipt may list beyond `NODE_SET_VARS` (L1: an
/// explicit list, never "any K2_*").
pub const NODE_JOB_VARS: &[&str] =
    &[k2_node_proto::env::ENV_JOB_CPUS, k2_node_proto::env::ENV_JOB_PORT_BASE, k2_node_proto::env::ENV_JOB_PORT_COUNT];
/// Refusal code: a node key no person (or `auto`) has accepted yet.
pub const ROSTER_CHANGE_NEEDS_PERSON: &str = "roster_change_needs_person";

/// What the version is bound to.
#[derive(Debug, Clone)]
pub enum Target<'a> {
    /// `rehearse`: no tag yet. The bundle's own commit and tree, and its
    /// build receipts' rustc, stand in; nothing else is relaxed.
    Rehearse,
    /// A release under an approved session.
    Release {
        version: &'a str,
        /// The approved base commit (the session's).
        base: &'a str,
        /// The session's first approval (receipts must start after it).
        opened: Option<i64>,
        /// What the forge said (`github_tag`); `None` for `approval_commit`.
        source: Option<&'a ReleaseSource>,
    },
}

pub struct Input<'a> {
    pub policy: &'a SignerPolicy,
    pub trust: &'a Trust,
    pub marks: &'a BTreeMap<String, RosterMark>,
    pub bundle: &'a EvidenceBundle,
    pub now: i64,
    pub target: Target<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RosterUse {
    pub owner_id: String,
    pub seq: u64,
    pub digest: String,
    pub issued_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobSummary {
    pub build: String,
    pub role: Role,
    pub job: String,
    pub node: String,
    pub node_fp: String,
    pub owner_fp: String,
    pub host: String,
    /// sha256 hex of the receipt body (the replay registry's key).
    pub receipt: String,
    pub isolation: Isolation,
    pub wall: Wall,
    pub started: String,
    pub ended: String,
    pub statement_digest: String,
    /// Every `--env` value the plan set (M1: shown, not only named).
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub project: String,
    pub version: Option<String>,
    pub run_id: String,
    pub controller_fp: String,
    pub commit: String,
    pub tree: String,
    pub tag_commit: Option<String>,
    pub base: Option<String>,
    pub rust_pin: Option<String>,
    pub rosters: BTreeMap<String, RosterUse>,
    pub jobs: BTreeMap<String, JobSummary>,
    pub notes: Vec<String>,
    /// The roster marks to save when this evidence registers.
    #[serde(skip)]
    pub new_marks: BTreeMap<String, RosterMark>,
}

impl Summary {
    /// The build receipt of `build`.
    pub fn build_job(&self, build: &str) -> Option<(&String, &JobSummary)> {
        self.jobs.iter().find(|(_, j)| j.build == build && j.role == Role::Build)
    }
}

fn short(s: &str) -> &str {
    &s[..s.len().min(12)]
}

fn fp16(s: &str) -> &str {
    &s[..s.len().min(16)]
}

struct OwnerRosters {
    newest: NodeRoster,
    newest_digest: String,
}

/// Check a bundle; the first failed rule is the refusal.
pub fn check(inp: &Input) -> R<Summary> {
    let p = inp.policy;
    let b = inp.bundle;
    b.check().map_err(Fail::Refused)?;
    if b.project != p.project {
        return refused(format!("the evidence is for project {}, not {}", b.project, p.project));
    }
    let mut notes = Vec::new();
    let (version, base, opened, source) = match &inp.target {
        Target::Rehearse => (None, None, None, None),
        Target::Release { version, base, opened, source } => {
            if b.version.as_deref() != Some(*version) {
                return refused(format!("the evidence is for version {:?}, not {version}", b.version));
            }
            (Some(*version), Some(*base), *opened, *source)
        }
    };

    // 1. Controller: the policy's and paired here.
    let ctl = &b.controller_fp;
    if !p.controllers.contains(ctl) {
        return refused(format!("controller {}… is not one project {}'s policy lets build it", fp16(ctl), p.project));
    }
    if !inp.trust.controllers.contains_key(ctl) {
        return refused(format!(
            "controller {}… is not paired with this signer (sudo k2-signer-admin pair-begin, then k2 ci signer add on that server)",
            fp16(ctl)
        ));
    }

    // 2. Node rosters.
    let mut rosters: BTreeMap<String, OwnerRosters> = BTreeMap::new();
    let mut new_marks: BTreeMap<String, RosterMark> = BTreeMap::new();
    let mut by_owner: BTreeMap<String, Vec<(NodeRoster, String)>> = BTreeMap::new();
    for s in &b.rosters {
        let peek: NodeRoster = s.peek(k2_node_proto::ci::DOMAIN_NODE_ROSTER).map_err(Fail::Refused)?;
        let fp = peek.key_fp().map_err(Fail::Refused)?;
        if !p.owners.contains(&fp) {
            return refused(format!("a node roster from owner {} (key {}…), which project {} does not trust", peek.owner_id, fp16(&fp), p.project));
        }
        let Some(key) = inp.trust.owner_key(&fp) else {
            return refused(format!(
                "owner {}'s roster key {}… is not pinned at this signer (sudo k2-signer-admin trust-owner {fp} <key>)",
                peek.owner_id,
                fp16(&fp)
            ));
        };
        let r = NodeRoster::open(s, &key).map_err(|e| Fail::Refused(format!("node roster from {}: {e}", peek.owner_id)))?;
        by_owner.entry(fp).or_default().push((r, s.digest()));
    }
    for (fp, mut list) in by_owner {
        list.sort_by_key(|(r, _)| r.seq);
        for w in list.windows(2) {
            let ((a, ad), (bb, _)) = (&w[0], &w[1]);
            if a.seq == bb.seq {
                return refused(format!("{}: two node rosters with seq {} from {}", codes::ROSTER_REPLAYED, a.seq, a.owner_id));
            }
            if bb.seq == a.seq + 1 && bb.prev_digest.as_deref() != Some(ad.as_str()) {
                return refused(format!(
                    "{}: {}'s roster seq {} does not chain to seq {} (prev_digest)",
                    codes::ROSTER_REPLAYED,
                    a.owner_id,
                    bb.seq,
                    a.seq
                ));
            }
        }
        let (newest, newest_digest) = list.last().cloned().ok_or_else(|| Fail::Internal("empty roster list".into()))?;
        let age = inp.now - newest.issued_at;
        if age > i64::from(p.roster_max_age_hours) * 3600 {
            return refused(format!(
                "{}: {}'s node roster (seq {}) was issued {}, over {} hours ago",
                codes::ROSTER_STALE,
                newest.owner_id,
                newest.seq,
                hhmm(newest.issued_at),
                p.roster_max_age_hours
            ));
        }
        if newest.issued_at > inp.now + ROSTER_FUTURE_SECS {
            return refused(format!("{}'s node roster is issued in the future ({})", newest.owner_id, hhmm(newest.issued_at)));
        }
        let mark = inp.marks.get(&fp);
        if let Some(m) = mark {
            if newest.seq < m.seq {
                return refused(format!(
                    "{}: {}'s node roster seq {} is older than seq {} this signer already registered",
                    codes::ROSTER_REPLAYED,
                    newest.owner_id,
                    newest.seq,
                    m.seq
                ));
            }
            for (r, d) in &list {
                if r.seq == m.seq && *d != m.digest {
                    return refused(format!(
                        "{}: {}'s node roster seq {} is not the one this signer registered (a second roster for one seq)",
                        codes::ROSTER_REPLAYED,
                        r.owner_id,
                        r.seq
                    ));
                }
                if r.seq == m.seq + 1 && r.prev_digest.as_deref() != Some(m.digest.as_str()) {
                    return refused(format!(
                        "{}: {}'s node roster seq {} does not chain to the registered seq {} (prev_digest)",
                        codes::ROSTER_REPLAYED,
                        r.owner_id,
                        r.seq,
                        m.seq
                    ));
                }
            }
            let linked = list.iter().any(|(r, _)| r.seq == m.seq || r.seq == m.seq + 1);
            if !linked {
                notes.push(format!(
                    "{}'s node roster went from seq {} to {}: the rosters between are not in the bundle, so the chain was not checked",
                    newest.owner_id, m.seq, newest.seq
                ));
            }
        }
        // Auto acceptances are per project (H1): another project's `auto`
        // never stands in for this one's person, and a project that now
        // says `ask` drops what it accepted under `auto`.
        let mut auto = mark.map(|m| m.auto_accepted.clone()).unwrap_or_default();
        if p.roster_changes == Changes::Ask {
            auto.remove(&p.project);
        }
        // L2: remember every key this owner ever showed revoked.
        let mut revoked = mark.map(|m| m.revoked.clone()).unwrap_or_default();
        for (r, _) in &list {
            revoked.extend(r.entries.iter().filter(|e| e.status == KeyStatus::Revoked).map(|e| e.node_fp.clone()));
        }
        new_marks.insert(
            fp.clone(),
            RosterMark { seq: newest.seq, digest: newest_digest.clone(), issued_at: newest.issued_at, seen_at: inp.now, auto_accepted: auto, revoked },
        );
        rosters.insert(fp, OwnerRosters { newest, newest_digest });
    }

    // Statements (self-certifying), attestations and seats (owner-signed).
    let mut statements: BTreeMap<String, NodeStatement> = BTreeMap::new();
    for s in &b.statements {
        let st = NodeStatement::open(s).map_err(|e| Fail::Refused(format!("a node statement: {e}")))?;
        statements.insert(s.digest(), st);
    }
    let mut attestations: BTreeMap<String, (String, OwnerAttestation)> = BTreeMap::new();
    for s in &b.attestations {
        let (fp, a) = open_owner_doc::<OwnerAttestation>(inp, s, k2_node_proto::ci::DOMAIN_NODE_ATTEST, |a: &OwnerAttestation| a.roster_key.clone())?;
        a.check().map_err(Fail::Refused)?;
        attestations.insert(s.digest(), (fp, a));
    }
    let mut seats: BTreeMap<String, Vec<SeatStatement>> = BTreeMap::new();
    for s in &b.seats {
        let (fp, st) = open_owner_doc::<SeatStatement>(inp, s, k2_node_proto::ci::DOMAIN_NODE_SEAT, |x: &SeatStatement| x.roster_key.clone())?;
        st.check().map_err(Fail::Refused)?;
        if st.client_fp != *ctl {
            return refused(format!("seat {} from {} is another client's (client {}…)", st.seat_id, st.owner_id, fp16(&st.client_fp)));
        }
        seats.entry(fp).or_default().push(st);
    }
    // Rotations: informational (a rotation from an accepted key vouches for a re-key).
    let mut rotated_from: BTreeMap<String, String> = BTreeMap::new();
    for s in &b.rotations {
        let peek: RotationStatement = s.peek(k2_node_proto::ci::DOMAIN_NODE_ROTATION).map_err(Fail::Refused)?;
        let old = rosters.values().find_map(|o| o.newest.entry(&peek.old_fp).cloned());
        if let Some(old) = old {
            let der = old.spki_der().map_err(Fail::Refused)?;
            let r = RotationStatement::open(s, &der).map_err(|e| Fail::Refused(format!("a rotation statement: {e}")))?;
            rotated_from.insert(r.new_fp, r.old_fp);
        }
    }

    // The rust pin: the forge's at the tag, else the build receipts' rustc.
    let needs_pin = |r: &BuildRule| r.needs.iter().any(|n| n.contains("{rust_pin}"));
    let mut rust_pin: Option<String> = source.and_then(|s| s.rust_pin.clone());
    if rust_pin.is_none() && p.build.values().any(needs_pin) {
        for (label, er) in &b.receipts {
            if er.role != Role::Build || !p.build.get(&er.build).is_some_and(needs_pin) {
                continue;
            }
            let rc: Receipt = serde_json::from_str(&er.body).map_err(|e| Fail::Refused(format!("{label}: the receipt body: {e}")))?;
            let rustc = rc.tools_used.get("rustc").cloned().unwrap_or_default();
            if forge_channel_ok(&rustc) {
                match &rust_pin {
                    None => rust_pin = Some(rustc),
                    Some(p0) if *p0 == rustc => {}
                    Some(p0) => return refused(format!("{label}: the build ran rustc {rustc}, another build ran {p0}")),
                }
            } else {
                return refused(format!("{label}: the receipt names no rustc it ran"));
            }
        }
    }

    // 3–6. Each receipt.
    let mut jobs: BTreeMap<String, JobSummary> = BTreeMap::new();
    let mut parsed: BTreeMap<String, (Receipt, RosterEntry, String)> = BTreeMap::new();
    let (commit, tree) = (&b.source.commit, &b.source.tree);
    if let Some(src) = source {
        if *tree != src.tree {
            return refused(format!("the evidence built tree {}, but v{}'s tree is {}", short(tree), version.unwrap_or("?"), short(&src.tree)));
        }
        if *commit != src.tag_commit {
            let Some(obj) = &b.source.commit_object else {
                return refused(format!(
                    "the jobs built commit {}, not the tag's {}, and the bundle has no commit object to bind it",
                    short(commit),
                    short(&src.tag_commit)
                ));
            };
            let raw = k2_node_proto::crypto::unb64(obj).map_err(Fail::Refused)?;
            let (c_tree, c_parents) = parse_commit_object(&raw)?;
            if c_tree != src.tree {
                return refused(format!("commit {} has tree {}, not the tag's {}", short(commit), short(&c_tree), short(&src.tree)));
            }
            if c_parents != [src.base.clone()] {
                return refused(format!(
                    "commit {}'s parent is {}, not the approved base {}",
                    short(commit),
                    c_parents.iter().map(|x| short(x)).collect::<Vec<_>>().join(","),
                    short(&src.base)
                ));
            }
        }
    } else if let Some(base) = base {
        // approval_commit: the approval names the commit; its object names the tree.
        if commit != base {
            return refused(format!("the jobs built commit {}, not the approved commit {}", short(commit), short(base)));
        }
        let Some(obj) = &b.source.commit_object else {
            return refused("approval_commit: the bundle needs the commit object (it binds the tree to the approved commit)");
        };
        let raw = k2_node_proto::crypto::unb64(obj).map_err(Fail::Refused)?;
        let (c_tree, _) = parse_commit_object(&raw)?;
        if c_tree != *tree {
            return refused(format!("the approved commit's tree is {}, the jobs built {}", short(&c_tree), short(tree)));
        }
    }
    let oldest = inp.now - i64::from(p.receipt_max_hours) * 3600;
    for (label, er) in &b.receipts {
        let rule = p.build.get(&er.build).ok_or_else(|| Fail::Refused(format!("{label}: build {} is not in the policy", er.build)))?;
        let peek: Receipt = serde_json::from_str(&er.body).map_err(|e| Fail::Refused(format!("{label}: the receipt body: {e}")))?;
        let fp = peek.node_fp.clone();
        let mut found: Vec<(&String, &OwnerRosters, &RosterEntry)> = Vec::new();
        for (ofp, o) in &rosters {
            if let Some(e) = o.newest.entry(&fp) {
                found.push((ofp, o, e));
            }
        }
        let (owner_fp, owned, entry) = match found.as_slice() {
            [one] => *one,
            [] => return refused(format!("{label}: the receipt is from node {}…, which is in no trusted node roster", fp16(&fp))),
            _ => return refused(format!("{label}: node {}… is in two owners' rosters", fp16(&fp))),
        };
        let name = entry.name.clone();
        // §4.1.6 cross-check: when the bundle carries the client server's
        // pairing pins, the key must be one it paired, and the same key. For a
        // node another owner runs, the pins are required unless the person
        // made them optional for this project (M7: default required for
        // client projects, optional for `k2`).
        // Who runs the node is the person's pin (trust-owner --own-controller),
        // never the owner's own `owner_server_fp`, which a foreign owner could
        // set to the client's fingerprint (M7).
        let foreign = !inp.trust.owners.get(owner_fp).is_some_and(|o| o.own_controllers.contains(ctl));
        if foreign && inp.trust.pairing_pins_required(&p.project) && !b.pairing_pins.contains_key(&fp) {
            return refused(format!(
                "{label}: node {name} belongs to {}, and the evidence carries no pairing pin for its key: the client server's own \
                 SAS-checked pin is required (k2-signer-admin pairing-pins {} optional to drop it)",
                owned.newest.owner_id, p.project
            ));
        }
        if !b.pairing_pins.is_empty() {
            let Some(pin) = b.pairing_pins.get(&fp) else {
                return refused(format!("{label}: the client server never paired node {name}'s key {}… (its pairing pins don't list it)", fp16(&fp)));
            };
            let pin_der = k2_node_proto::crypto::unb64(pin).map_err(Fail::Refused)?;
            if pin_der != entry.spki_der().map_err(Fail::Refused)? {
                return refused(format!("{label}: the client server pinned another key for node {name} at pairing than {}'s roster lists", owned.newest.owner_id));
            }
        }
        let der = entry.spki_der().map_err(Fail::Refused)?;
        let Some(rc) = er.signed().verify(&der) else {
            return refused(format!("{label}: the receipt's signature does not verify with node {name}'s key (owner {}'s roster)", owned.newest.owner_id));
        };
        if canonical::to_string(&rc).map_err(Fail::Internal)? != er.body {
            return refused(format!("{label}: the receipt body is not its canonical form (unknown or reordered fields)"));
        }
        rc.check_version().map_err(|e| Fail::Refused(format!("{label}: {e}")))?;
        if rc.v != RECEIPT_V2 {
            return refused(format!(
                "{label}: receipt v{}: k2-signer checks receipt v2 (a v1 receipt is k2sign's S3b path)",
                rc.v
            ));
        }
        let run = rc.run.clone().ok_or_else(|| Fail::Internal("v2 without run".into()))?;
        if rc.controller_fp != *ctl {
            return refused(format!("{label}: the job was sent by controller {}…, not the bundle's {}…", fp16(&rc.controller_fp), fp16(ctl)));
        }
        if !p.workspaces.contains(&rc.workspace_id) {
            return refused(format!("{label}: workspace {} may not build project {}", rc.workspace_id, p.project));
        }
        // A person accepted this key with these roster attributes (S1, M4),
        // or this project says auto (H1: never another project's auto).
        // L2: a key ever shown revoked never comes back.
        let ever_revoked = inp.trust.owners.get(owner_fp).is_some_and(|o| o.revoked.contains(&fp))
            || inp.marks.get(owner_fp).is_some_and(|m| m.revoked.contains(&fp));
        if ever_revoked {
            return refused(format!("{label}: node {name}'s key {}… was revoked in an earlier roster of {}: a revoked key never comes back", fp16(&fp), owned.newest.owner_id));
        }
        let person = inp.trust.owners.get(owner_fp).and_then(|o| o.accepted.get(&fp));
        let person_ok = person.is_some_and(|a| a.covers(entry));
        let auto_ok = p.roster_changes == Changes::Auto
            && new_marks.get(owner_fp).and_then(|m| m.auto_accepted.get(&p.project)).and_then(|m| m.get(&fp)).is_some_and(|a| a.covers(entry));
        if !person_ok && !auto_ok {
            match p.roster_changes {
                Changes::Ask => {
                    let why = if person.is_some() {
                        "its roster attributes (host, provider, site, admin reach, taint basis…) changed, or its status or validity widened, since a person accepted it"
                    } else {
                        "it is new to this signer"
                    };
                    return refused(format!(
                        "{ROSTER_CHANGE_NEEDS_PERSON}: {label}: node {name}'s key {}… (owner {}, roster seq {}): {why}: \
                         a person checks and accepts it (sudo k2-signer-admin roster-accept {owner_fp} < the evidence bundle)",
                        fp16(&fp),
                        owned.newest.owner_id,
                        owned.newest.seq
                    ));
                }
                Changes::Auto => {
                    let how = match rotated_from.get(&fp) {
                        Some(old) => format!("a re-key of {}…", fp16(old)),
                        None => "a new key".to_string(),
                    };
                    notes.push(format!("roster_changes = auto: accepted node {name}'s key {}… ({how}) from {}'s roster seq {}", fp16(&fp), owned.newest.owner_id, owned.newest.seq));
                    if let Some(m) = new_marks.get_mut(owner_fp) {
                        m.auto_accepted
                            .entry(p.project.clone())
                            .or_default()
                            .insert(fp.clone(), AcceptedKey::of(entry, owned.newest.seq, inp.now));
                    }
                }
            }
        }
        let started = rc.started_at.ok_or_else(|| Fail::Refused(format!("{label}: the receipt has no start time")))?;
        let ended = rc.ended_at;
        match entry.status {
            KeyStatus::Active => {}
            KeyStatus::Revoked => {
                return refused(format!(
                    "{label}: node {name}'s key {}… is revoked ({:?}) in {}'s roster: every receipt from it is void",
                    fp16(&fp),
                    entry.revoked_reason,
                    owned.newest.owner_id
                ))
            }
            KeyStatus::Retired => {
                let to = entry.valid_to.unwrap_or(i64::MIN);
                if started > to {
                    return refused(format!("{label}: node {name}'s key was retired {}, before the job started", hhmm(to)));
                }
            }
        }
        if started < entry.valid_from {
            return refused(format!("{label}: the job started before node {name}'s key was valid"));
        }
        let Some(basis) = entry.taint_basis else {
            return refused(format!("{}: {label}: {}'s roster vouches for nothing about node {name}'s key (taint_basis null)", codes::NODE_KEY_TAINTED, owned.newest.owner_id));
        };
        if !p.taint_basis.contains(&basis) {
            return refused(format!("{label}: node {name}'s key is untainted by {basis:?}, which project {} does not accept", p.project));
        }
        let st = statements
            .get(&run.statement_digest)
            .ok_or_else(|| Fail::Refused(format!("{label}: the bundle lacks node {name}'s statement the receipt names")))?;
        if st.node_fp != fp || st.key_epoch != entry.key_epoch || run.key_epoch != entry.key_epoch {
            return refused(format!("{label}: node {name}'s statement, roster entry and receipt disagree on the key or its epoch"));
        }
        if st.dev {
            return refused(format!("{label}: node {name} is a --dev node"));
        }
        if st.key_exposure.job_readable {
            return refused(format!("{label}: node {name}'s jobs can read its key (key_exposure.job_readable)"));
        }
        if st.wall != run.wall {
            return refused(format!("{label}: node {name}'s statement says wall {:?}, the receipt {:?}", st.wall, run.wall));
        }
        if run.wall == Wall::None || !rule.walls.contains(&run.wall) {
            return refused(format!("{label}: the job ran behind wall {:?}, which build {} does not accept", run.wall, er.build));
        }
        match (st.taint, basis) {
            (Taint::Tainted, _) => return refused(format!("{}: {label}: node {name}'s key is tainted (a job ran without the wall)", codes::NODE_KEY_TAINTED)),
            (Taint::Unknown, TaintBasis::OwnerAttested) => {}
            (Taint::Unknown, _) => return refused(format!("{}: {label}: node {name}'s key predates the taint flag and was not attested", codes::NODE_KEY_TAINTED)),
            (Taint::Clean, TaintBasis::Rekeyed) if st.key_epoch == 0 => {
                return refused(format!("{label}: node {name}'s key is said to be re-keyed, but it is the enrolment key (epoch 0)"))
            }
            (Taint::Clean, _) => {}
        }
        if st.issued_at > started || st.key_born_at > started {
            return refused(format!("{label}: node {name}'s statement was issued after the job started"));
        }
        if st.k2node_version != run.k2node_version || st.k2node_commit != run.k2node_commit {
            return refused(format!("{label}: node {name}'s statement and the receipt name another k2-node build"));
        }
        if entry.statement_digest != run.statement_digest {
            if let Some(approved) = statements.get(&entry.statement_digest) {
                if approved.taint == Taint::Tainted {
                    return refused(format!("{}: {label}: node {name}'s approved statement says tainted", codes::NODE_KEY_TAINTED));
                }
            }
            notes.push(format!("{label}: node {name} ran under a newer statement than the one in its owner's roster"));
        }
        if basis == TaintBasis::OwnerAttested {
            let ad = entry.attestation_digest.as_deref().unwrap_or_default();
            let (afp, a) = attestations
                .get(ad)
                .ok_or_else(|| Fail::Refused(format!("{label}: the bundle lacks {}'s attestation for node {name}", owned.newest.owner_id)))?;
            if afp != owner_fp || a.node_fp != fp || a.key_epoch != entry.key_epoch || a.statement_digest != entry.statement_digest {
                return refused(format!("{label}: the owner attestation for node {name} is for another key or statement"));
            }
        }
        // Seat (§4.3): this client's, active, naming this node, under its lane.
        let seat = seats.get(owner_fp).and_then(|v| {
            v.iter().find(|s| s.state == k2_node_proto::ci::SeatState::Active && s.nodes.contains(&name) && s.roster_seq <= owned.newest.seq)
        });
        let Some(seat) = seat else {
            return refused(format!("{label}: node {name} is not on an active seat of this client from {}", owned.newest.owner_id));
        };
        // Seat v1 has no expiry field: a seat statement lives as long as a
        // roster may (roster_max_age_hours), so a suspended or ended seat
        // can't be replayed by keeping an old `active` one (L3).
        if inp.now - seat.issued_at > i64::from(p.roster_max_age_hours) * 3600 {
            return refused(format!(
                "{label}: seat {} from {} was issued {}, over {} hours ago (roster_max_age_hours): the owner reissues it",
                seat.seat_id,
                owned.newest.owner_id,
                hhmm(seat.issued_at),
                p.roster_max_age_hours
            ));
        }
        if run.lane.as_deref() != Some(seat.lane.as_str()) {
            return refused(format!("{label}: the job ran under lane {:?}, not the seat's lane {}", run.lane, seat.lane));
        }
        if let Some(seen) = run.roster_seq_seen {
            if seen > owned.newest.seq {
                return refused(format!(
                    "{}: {label}: node {name} had seen {}'s roster seq {seen}, the bundle carries seq {}",
                    codes::ROSTER_REPLAYED,
                    owned.newest.owner_id,
                    owned.newest.seq
                ));
            }
        }
        // Isolation (S3b M4, PRD §4.3): secret builds own their uid or the node.
        if !rule.isolation.contains(&run.isolation) {
            return refused(format!("{label}: the job ran with isolation {:?}, build {} accepts {:?}", run.isolation, er.build, rule.isolation));
        }
        let secret = !rule.secrets.is_empty() || !rule.secrets_optional.is_empty();
        if secret {
            if !matches!(run.isolation, Isolation::JobUid | Isolation::Exclusive) {
                return refused(format!("{label}: a secret build that did not run job_uid or exclusive"));
            }
            if run.wall != Wall::JobUser || st.ptrace_scope.unwrap_or(0) < 1 {
                return refused(format!("{label}: a secret build needs the job_user wall and ptrace_scope ≥ 1 (node {name})"));
            }
        }
        if rc.state != JobState::Done || rc.exit.as_ref().map(|e| (e.code, e.signal)) != Some((Some(0), None)) {
            return refused(format!("{label}: the job did not end done/exit 0 ({:?}, {:?})", rc.state, rc.exit));
        }
        if rc.argv_sha256 != argv_sha256(&rule.argv) {
            return refused(format!("{label}: the job did not run build {}'s command ({})", er.build, rule.argv.join(" ")));
        }
        let src = rc.src.as_ref().ok_or_else(|| Fail::Refused(format!("{label}: the receipt names no source commit and tree")))?;
        if src.commit != *commit || src.tree != *tree {
            return refused(format!("{label}: the job built commit {} tree {}, not the evidence's {} {}", short(&src.commit), short(&src.tree), short(commit), short(tree)));
        }
        if src.dirty_sha256.is_some() {
            return refused(format!("{label}: the job ran uncommitted changes (--dirty)"));
        }
        if src.warmth.as_deref() != Some("private") {
            return refused(format!(
                "{label}: the job built in a shared target dir (warmth {:?}): a release build uses only its own target, CARGO_HOME and sccache",
                src.warmth
            ));
        }
        if rc.env_names.iter().any(|n| n == JOB_CACHE_ENV) {
            return refused(format!("{label}: the job ran with a --cache mount"));
        }
        let extra: Vec<&String> = rc
            .env_names
            .iter()
            .filter(|n| {
                !rule.env.contains(n)
                    && !k2_node_proto::env::NODE_SET_VARS.contains(&n.as_str())
                    && !k2_node_proto::env::SCCACHE_SERVER_VARS.contains(&n.as_str())
                    && !NODE_JOB_VARS.contains(&n.as_str())
            })
            .collect();
        if !extra.is_empty() {
            return refused(format!(
                "{label}: the job ran with env the build doesn't set: {}",
                extra.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        if ended < started {
            return refused(format!("{label}: the receipt ends before it starts"));
        }
        if ended < oldest {
            return refused(format!("{label}: an old receipt: the job ended {}, over {} hours ago", hhmm(ended), p.receipt_max_hours));
        }
        if let Some(o) = opened {
            if started < o - SKEW_SECS {
                return refused(format!("{label}: an old receipt: the job started {}, before this version's first approval ({})", hhmm(started), hhmm(o)));
            }
        }
        if ended > inp.now + SKEW_SECS {
            return refused(format!("{label}: the receipt ends in the future ({})", hhmm(ended)));
        }
        if needs_pin(rule) {
            let pin = rust_pin.as_deref().unwrap_or_default();
            let ran = rc.tools_used.get("rustc").map(String::as_str).unwrap_or_default();
            if ran != pin {
                return refused(format!("{label}: the job's rustc was {ran}, the commit pins {pin}"));
            }
        }
        let env = check_plan(label, er, &rc, rule, rust_pin.as_deref())?;
        let rid = crate::ctx::sha256_hex(er.body.as_bytes());
        jobs.insert(
            label.clone(),
            JobSummary {
                build: er.build.clone(),
                role: er.role,
                job: rc.job_id.clone(),
                node: name.clone(),
                node_fp: fp.clone(),
                owner_fp: owner_fp.clone(),
                host: entry.host.clone(),
                receipt: rid,
                isolation: run.isolation,
                wall: run.wall,
                started: iso(started),
                ended: iso(ended),
                statement_digest: run.statement_digest.clone(),
                env,
            },
        );
        parsed.insert(label.clone(), (rc, entry.clone(), owner_fp.clone()));
    }

    // 7. Dual builds and independence.
    let mut builds: BTreeSet<&String> = BTreeSet::new();
    for er in b.receipts.values() {
        builds.insert(&er.build);
    }
    for build in builds {
        let rule = &p.build[build];
        let (bl, (brc, bentry, bowner)) = parsed
            .iter()
            .find(|(l, _)| b.receipts[*l].build == *build && b.receipts[*l].role == Role::Build)
            .ok_or_else(|| Fail::Refused(format!("build {build} has no build receipt")))?;
        let verifies: Vec<(&String, &(Receipt, RosterEntry, String))> =
            parsed.iter().filter(|(l, _)| b.receipts[*l].build == *build && b.receipts[*l].role == Role::Verify).collect();
        if rule.dual.is_empty() {
            continue;
        }
        if verifies.is_empty() {
            return refused(format!("no verify receipt for build {build}: the dual build on an independent node is required"));
        }
        let barts = artifacts_of(bl, brc)?;
        for (vl, (vrc, ventry, vowner)) in verifies {
            if vrc.node_fp == brc.node_fp {
                return refused(format!("{vl} ran on the same node as {bl} ({}): the dual build needs a second node", bentry.name));
            }
            if vrc.job_id == brc.job_id {
                return refused(format!("{vl} is the same job as {bl}"));
            }
            let mut a = brc.secret_env.clone();
            let mut v = vrc.secret_env.clone();
            a.sort();
            v.sort();
            if a != v {
                return refused(format!("{vl} got other vault secrets than {bl}: the two builds are not the same build"));
            }
            for dim in &rule.independent {
                independent(*dim, bl, vl, (bentry, bowner), (ventry, vowner))?;
            }
            let varts = artifacts_of(vl, vrc)?;
            for n in &rule.dual {
                let Some(x) = barts.get(n) else {
                    return refused(format!("{vl} match: {bl}'s receipt lists no {n}"));
                };
                let Some(y) = varts.get(n) else {
                    return refused(format!("{vl} match missing: {vl}'s receipt lists no {n} (the dual build must reproduce it)"));
                };
                if x != y {
                    return refused(format!(
                        "{vl} mismatch: {n} differs between {bl} on {} ({}…) and {vl} on {} ({}…)",
                        bentry.name,
                        short(&x.0),
                        ventry.name,
                        short(&y.0)
                    ));
                }
            }
        }
    }

    for (name, rule) in &p.build {
        if !rule.secret_sha256.is_empty() && b.receipts.values().any(|er| er.build == *name) {
            notes.push(format!(
                "build {name}: secret_sha256 is set, but receipt v2 records no hash of what a build baked in, so it is not checked yet (S9)"
            ));
        }
    }
    let rosters_out = rosters
        .iter()
        .map(|(fp, o)| {
            (
                fp.clone(),
                RosterUse { owner_id: o.newest.owner_id.clone(), seq: o.newest.seq, digest: o.newest_digest.clone(), issued_at: iso(o.newest.issued_at) },
            )
        })
        .collect();
    Ok(Summary {
        project: p.project.clone(),
        version: b.version.clone(),
        run_id: b.run_id.clone(),
        controller_fp: ctl.clone(),
        commit: commit.clone(),
        tree: tree.clone(),
        tag_commit: source.map(|s| s.tag_commit.clone()),
        base: base.map(str::to_string),
        rust_pin,
        rosters: rosters_out,
        jobs,
        notes,
        new_marks,
    })
}

fn forge_channel_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.as_bytes()[0].is_ascii_alphanumeric() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// Open an owner-signed document with the pinned key its `roster_key` names
/// (the owner must be pinned and in the policy). Returns the owner fp.
fn open_owner_doc<T>(inp: &Input, s: &Signed, domain: &str, key_of: impl Fn(&T) -> String) -> R<(String, T)>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let peek: T = s.peek(domain).map_err(Fail::Refused)?;
    let k = k2_node_proto::admin::ApprovalKey::parse(&key_of(&peek)).ok_or_else(|| Fail::Refused(format!("{domain}: a bad roster_key")))?;
    let fp = k.fingerprint();
    if !inp.policy.owners.contains(&fp) {
        return refused(format!("{domain}: from owner key {}…, which the policy does not trust", fp16(&fp)));
    }
    let pinned = inp.trust.owner_key(&fp).ok_or_else(|| Fail::Refused(format!("{domain}: owner key {}… is not pinned here", fp16(&fp))))?;
    let doc: T = s.open_owner(&pinned, domain).map_err(Fail::Refused)?;
    if key_of(&doc) != pinned.encode() {
        return refused(format!("{domain}: roster_key is not the verifying key"));
    }
    Ok((fp, doc))
}

/// `(tree, parents)` of a git commit object's bytes (k2sign's
/// `parse_commit_object`; the bundle check already hashed it to the commit).
pub fn parse_commit_object(raw: &[u8]) -> R<(String, Vec<String>)> {
    let head_end = raw.windows(2).position(|w| w == b"\n\n").unwrap_or(raw.len());
    let header = std::str::from_utf8(&raw[..head_end]).map_err(|_| Fail::Refused("the commit object's header is not text".into()))?;
    let lines: Vec<&str> = header.split('\n').collect();
    let is_hex40 = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let tree = lines.first().and_then(|l| l.strip_prefix("tree ")).filter(|t| is_hex40(t));
    let Some(tree) = tree else {
        return refused("the commit object does not start with its tree");
    };
    if lines.iter().filter(|l| l.starts_with("tree ")).count() != 1 {
        return refused("the commit object names more than one tree");
    }
    let mut parents = Vec::new();
    for l in &lines[1..] {
        if let Some(p) = l.strip_prefix("parent ") {
            if !is_hex40(p) {
                return refused("the commit object has a bad parent line");
            }
            parents.push(p.to_string());
        }
    }
    Ok((tree.to_string(), parents))
}

/// name → (sha256, bytes) of a receipt's artifacts (each once).
fn artifacts_of(label: &str, rc: &Receipt) -> R<BTreeMap<String, (String, u64)>> {
    let mut out = BTreeMap::new();
    for a in &rc.artifacts {
        if !k2_node_proto::ci::is_fp(&a.sha256) {
            return refused(format!("{label}: the receipt's {} has no sha256", a.name));
        }
        if out.insert(a.name.clone(), (a.sha256.clone(), a.bytes)).is_some() {
            return refused(format!("{label}: the receipt lists {} twice", a.name));
        }
    }
    Ok(out)
}

/// The receipt's artifact `name`, if listed: `(sha256, bytes)`.
pub fn receipt_artifact(label: &str, er: &EvidenceReceipt, name: &str) -> R<Option<(String, u64)>> {
    let rc: Receipt = serde_json::from_str(&er.body).map_err(|e| Fail::Refused(format!("{label}: {e}")))?;
    Ok(artifacts_of(label, &rc)?.get(name).cloned())
}

fn independent(dim: Dimension, bl: &str, vl: &str, b: (&RosterEntry, &String), v: (&RosterEntry, &String)) -> R<()> {
    let (be, bo) = b;
    let (ve, vo) = v;
    let fail = |what: &str| refused(format!("{}: {vl} is not independent of {bl}: {what}", codes::NO_INDEPENDENT_NODES));
    match dim {
        Dimension::Node => {
            if be.node_fp == ve.node_fp {
                return fail("the same node");
            }
        }
        Dimension::Host => {
            if be.host == ve.host {
                return fail(&format!("both ran on host {} (the owner's roster)", be.host));
            }
        }
        Dimension::Site => match (&be.site, &ve.site) {
            (Some(x), Some(y)) if x != y => {}
            (Some(x), Some(_)) => return fail(&format!("both at site {x}")),
            _ => return fail("the roster does not say a node's site"),
        },
        Dimension::Provider => match (&be.provider, &ve.provider) {
            (Some(x), Some(y)) if x != y => {}
            (Some(x), Some(_)) => return fail(&format!("both at provider {x}")),
            _ => return fail("the roster does not say a node's provider"),
        },
        Dimension::AdminReach => {
            if be.admin_reach.is_empty() || ve.admin_reach.is_empty() {
                return fail("the roster does not say who holds root on a node");
            }
            if let Some(x) = be.admin_reach.iter().find(|a| ve.admin_reach.contains(a)) {
                return fail(&format!("{x} holds root on both"));
            }
        }
        Dimension::Owner => {
            if bo == vo {
                return fail("one owner's word for both");
            }
        }
    }
    Ok(())
}

/// The job's plan (S3b review H1): what it is (its digest), and that nothing
/// in it changes the build beyond the policy's own inputs.
fn check_plan(label: &str, er: &EvidenceReceipt, rc: &Receipt, rule: &BuildRule, pin: Option<&str>) -> R<BTreeMap<String, String>> {
    let plan: Plan = serde_json::from_value(er.plan.clone()).map_err(|e| Fail::Refused(format!("{label}: the plan is not a job plan: {e}")))?;
    if canonical::to_string(&plan).map_err(Fail::Internal)? != canonical::to_string(&er.plan).map_err(Fail::Internal)? {
        return refused(format!("{label}: the plan has fields the signer doesn't know (or is not the controller's form)"));
    }
    if er.plan_digest().map_err(Fail::Internal)? != rc.plan_digest {
        return refused(format!("{label}: the plan does not hash to the receipt's plan_digest"));
    }
    if plan.job_id != rc.job_id || plan.workspace_id != rc.workspace_id {
        return refused(format!("{label}: the plan is for job {} in {}, the receipt for {} in {}", plan.job_id, plan.workspace_id, rc.job_id, rc.workspace_id));
    }
    if plan.argv != rule.argv {
        return refused(format!("{label}: the plan's command is not the build's command"));
    }
    if !plan.with.is_empty() {
        return refused(format!(
            "{label}: the plan ships --with repos ({}): they would lay other code into the build",
            plan.with.iter().map(|w| w.path.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(sp) = &plan.src_path {
        return refused(format!("{label}: the plan has a --with layout (src_path {sp:?})"));
    }
    if let Some(c) = &plan.cwd {
        return refused(format!("{label}: the plan runs in another folder (cwd {c:?})"));
    }
    if let Some(c) = &plan.cache {
        return refused(format!("{label}: the plan mounts a --cache ({c:?})"));
    }
    if !plan.git_access.is_empty() {
        return refused(format!("{label}: the plan reads other repos with a git token ({})", plan.git_access.join(", ")));
    }
    if plan.receipt_v != Some(RECEIPT_V2) {
        return refused(format!("{label}: the plan did not ask for receipt v2"));
    }
    let src_commit = rc.src.as_ref().map(|s| s.commit.as_str()).unwrap_or_default();
    let src_tree = rc.src.as_ref().map(|s| s.tree.as_str()).unwrap_or_default();
    match &plan.src {
        Some(s) if s.commit == src_commit && s.dirty.is_none() => {}
        _ => return refused(format!("{label}: the plan's source is not the commit the receipt ran, clean")),
    }
    let keys: BTreeSet<&String> = plan.env.keys().collect();
    let want: BTreeSet<&String> = rule.env.iter().collect();
    if keys != want {
        let extra: Vec<&&String> = keys.difference(&want).collect();
        if !extra.is_empty() {
            return refused(format!(
                "{label}: the plan sets env the build doesn't: {}",
                extra.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        return refused(format!("{label}: the plan lacks env the build sets: {}", want.difference(&keys).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    for (k, v) in [("BUILD_COMMIT", src_commit), ("BUILD_TREE", src_tree)] {
        if let Some(got) = plan.env.get(k) {
            if got != v {
                return refused(format!("{label}: the plan's BUILD_COMMIT/BUILD_TREE are not the commit and tree it ran"));
            }
        }
    }
    // M1: the controller picks env values; the signer can vouch only for the
    // build facts. Signer policy v1 lists env names, not values, so any other
    // name must be empty.
    for (k, v) in &plan.env {
        if k != "BUILD_COMMIT" && k != "BUILD_TREE" && !v.is_empty() {
            return refused(format!(
                "{label}: the plan sets {k}={:?}: only BUILD_COMMIT and BUILD_TREE carry values (the policy can't pin one; signer policy v1 lists names only)",
                v.chars().take(80).collect::<String>()
            ));
        }
    }
    let want_needs: Vec<String> = rule.needs.iter().map(|n| n.replace("{rust_pin}", pin.unwrap_or("{rust_pin}"))).collect();
    let mut a = plan.needs.clone();
    let mut w = want_needs.clone();
    a.sort();
    w.sort();
    let uniq: BTreeSet<&String> = plan.needs.iter().collect();
    if a != w || uniq.len() != plan.needs.len() {
        return refused(format!("{label}: the plan needs {}, the build needs {}", plan.needs.join(" "), want_needs.join(" ")));
    }
    if !plan.packages.is_empty() {
        let mut want_pkgs: Vec<String> = Vec::new();
        for t in &want_needs {
            if let Some(pr) = t.strip_prefix("profile:") {
                let prof = k2_node_proto::profiles::resolve(pr).map_err(Fail::Refused)?;
                want_pkgs.extend(prof.all_sys());
            } else if let Some(s) = t.strip_prefix("sys:") {
                want_pkgs.push(s.to_string());
            }
        }
        want_pkgs.sort();
        want_pkgs.dedup();
        if plan.packages != want_pkgs {
            return refused(format!("{label}: the plan installs packages beyond the build's needs: {}", plan.packages.join(" ")));
        }
    }
    if plan.artifacts != rule.artifacts {
        return refused(format!("{label}: the plan collects {:?}, not {:?}", plan.artifacts, rule.artifacts));
    }
    let sec: BTreeSet<&String> = plan.secret_env.iter().collect();
    if plan.secret_env != rc.secret_env || sec.len() != plan.secret_env.len() {
        return refused(format!("{label}: the plan's vault secrets are not the receipt's"));
    }
    let missing: Vec<&String> = rule.secrets.iter().filter(|s| !sec.contains(s)).collect();
    if !missing.is_empty() {
        return refused(format!(
            "{label}: the job did not get its vault secrets (secret_env lacks {})",
            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let unused: Vec<&&String> = sec.iter().filter(|s| !rule.secrets.contains(s) && !rule.secrets_optional.contains(s)).collect();
    if !unused.is_empty() {
        return refused(format!(
            "{label}: the job got vault secrets the build doesn't use: {}",
            unused.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(plan.env.clone())
}

//! Lanes, seats and the node roster on the node (protocol 9, C1/C3a; PRD
//! §4.1, §4.3; vs-live V11, V20–V23).
//!
//! **Lanes.** A lane is the node owner's standing policy for one class of
//! client work. The owner approves a lane template once on its server
//! (`k2 compute lanes set`); a seat (one client's set of the owner's nodes)
//! names one lane. The node holds what applies to it in a root file,
//! [`LANES_FILE`] (`/etc/k2-node-lanes.toml`, root:root 0644, like
//! `k2-node-package-policy.toml`), so `servers.toml` keeps its closed schema
//! and rollbacks stay safe (vs-live B2). The owner server writes the whole
//! file with one privileged verb, [`VERB_LANES_WRITE`] (V22): the lanes in
//! use on that node and one line per client server seated there. A node
//! without the [`crate::ci::FEATURE_LANES`] feature refuses the verb by name,
//! so a seat is offered only on nodes that have it.
//!
//! The file is the owner's word, not a wall: what a lane allows (secrets for
//! a `trust team` tenant, CI tier, caps) is enforced where each rule lives
//! (C3b lane secrets, the queue). Unknown keys are refused, enums are
//! `snake_case` like the wire, the file carries `version = 1`.
//!
//! **Node roster on the node** (V11, the node is the courier). The owner
//! server hands its node its latest signed [`crate::node_roster::NodeRoster`]
//! and the seat statements naming that node with [`VERB_NODE_ROSTER_SET`]
//! (operate: the documents verify themselves). The node pins the roster key
//! the first time (trust on first use under the owner server's signed
//! command); a later set under another key is refused unless the command is
//! the owner's approved (privileged) one. The node keeps only a rising
//! `seq`. It relays the roster, with each tenant's **own** seat statements
//! only, to every protocol-9 server it serves ([`NodeRosterDoc`] in the
//! `node_roster` frame), once per session and on every change.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin::ApprovalKey;
use crate::ci::{self, Isolation, Signed};
use crate::node_roster::{NodeRoster, SeatStatement};
use crate::pkgpolicy;

/// The root file on the node.
pub const LANES_FILE: &str = "/etc/k2-node-lanes.toml";
/// Its `version`.
pub const LANES_FILE_VERSION: u32 = 1;
/// Privileged, `full`: write [`LANES_FILE`] whole. Args: `{"file": LanesFile}`.
pub const VERB_LANES_WRITE: &str = "lanes.write";
/// Operate, `policy`, answered by the node: hold and relay the owner's node
/// roster. Args: [`NodeRosterDoc`].
pub const VERB_NODE_ROSTER_SET: &str = "node_roster.set";
/// Lanes in one file.
pub const MAX_LANES: usize = 16;
/// Seat lines in one file (one per client server; the node's slot cap).
pub const MAX_SEAT_LINES: usize = crate::share::MAX_SLOTS as usize;
/// Seat statements a roster set may carry for one node.
pub const MAX_SET_SEATS: usize = 32;
/// The lane template K2 ships (`ci-standard`, PRD §4.3).
pub const LANE_CI_STANDARD: &str = "ci-standard";

/// Refusal codes (stable).
pub mod codes {
    /// The lanes file or the verb's arguments break a rule.
    pub const BAD_LANES: &str = "bad_lanes";
    /// A roster set under another key than the one this node pinned.
    pub const ROSTER_KEY_CHANGED: &str = "node_roster_key_changed";
    /// A roster set older than the one the node holds.
    pub const ROSTER_OLDER: &str = crate::ci::codes::ROSTER_REPLAYED;
    /// The roster names another owner server than this node's.
    pub const ROSTER_WRONG_OWNER: &str = "node_roster_wrong_owner";
}

/// One lane's policy (the owner's template). Every key always present in
/// the file, except `other_tenants_max_job_secs` (absent = off, V23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneTemplate {
    /// The seat's grants may carry `--ci` (`allow_ci`) and its share `gate`.
    pub tier_ci: bool,
    /// Secret env for this `trust team` tenant (C3b, V24).
    pub secrets: bool,
    /// What a secret job must run under (`job_uid`, `exclusive`), sorted.
    pub isolation: Vec<Isolation>,
    /// Budget for `--exclusive` jobs per client per node (0–1440 min).
    pub exclusive_minutes_day: u32,
    /// Soft quota for CI-tier work per client (hours, 0–6144).
    pub ci_cpu_hours_day: u32,
    /// CI jobs running at once per client per node (1–64).
    pub max_ci_running: u32,
    /// Per job (60 s – 7 days).
    pub max_job_secs: u32,
    /// Package policy level for this tenant.
    pub packages: pkgpolicy::Level,
    /// Profiles at `profiles` or above (`tauri-linux@1`).
    pub profiles: Vec<String>,
    /// The tenant's jobs may reach the network.
    pub network: bool,
    /// A cap on everyone else's non-CI jobs on lane nodes, so a CI job never
    /// waits longer. **Off unless the owner sets it** (V23: K2 Cores', K2
    /// Engine's and Sew's long jobs share these nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_tenants_max_job_secs: Option<u32>,
}

impl LaneTemplate {
    /// `ci-standard` (PRD §4.3, with V23's default: no cap on others).
    pub fn ci_standard() -> Self {
        LaneTemplate {
            tier_ci: true,
            secrets: true,
            isolation: vec![Isolation::JobUid, Isolation::Exclusive],
            exclusive_minutes_day: 120,
            ci_cpu_hours_day: 24,
            max_ci_running: 2,
            max_job_secs: 7200,
            packages: pkgpolicy::Level::Profiles,
            profiles: ["rust-gui-ci", "tauri-linux"]
                .iter()
                .filter_map(|n| crate::profiles::latest(n))
                .map(|p| p.id())
                .collect(),
            network: true,
            other_tenants_max_job_secs: None,
        }
    }

    /// A shipped template by name.
    pub fn named(name: &str) -> Option<Self> {
        (name == LANE_CI_STANDARD).then(Self::ci_standard)
    }

    pub fn check(&self) -> Result<(), String> {
        ci::check_sorted_unique("lane.isolation", &self.isolation)?;
        if let Some(bad) = self.isolation.iter().find(|i| !matches!(i, Isolation::JobUid | Isolation::Exclusive)) {
            return Err(format!("lane.isolation: {bad:?} never isolates a secret job (job_uid or exclusive)"));
        }
        if self.secrets && self.isolation.is_empty() {
            return Err("lane.isolation: a lane with secrets names job_uid and/or exclusive".into());
        }
        if self.exclusive_minutes_day > 1440 {
            return Err("lane.exclusive_minutes_day is 0-1440".into());
        }
        if self.ci_cpu_hours_day > 6144 {
            return Err("lane.ci_cpu_hours_day is 0-6144".into());
        }
        if !(1..=64).contains(&self.max_ci_running) {
            return Err("lane.max_ci_running is 1-64".into());
        }
        if !(60..=7 * 86_400).contains(&self.max_job_secs) {
            return Err("lane.max_job_secs is 60 s to 7 days".into());
        }
        if let Some(o) = self.other_tenants_max_job_secs {
            if !(300..=7 * 86_400).contains(&o) {
                return Err("lane.other_tenants_max_job_secs is 5 min to 7 days (or absent: off)".into());
            }
        }
        ci::check_sorted_unique("lane.profiles", &self.profiles)?;
        pkgpolicy::ServerPolicy { level: self.packages, profiles: self.profiles.clone(), max_install_mb: pkgpolicy::DEFAULT_MAX_INSTALL_MB }
            .check()
            .map_err(|e| format!("lane.profiles: {e}"))?;
        Ok(())
    }

    /// Does `to` allow anything this doesn't? (A lane change that widens is
    /// the owner's, in person; narrowing applies at once.)
    pub fn widened_by(&self, to: &LaneTemplate) -> bool {
        (to.tier_ci && !self.tier_ci)
            || (to.secrets && !self.secrets)
            || to.isolation.iter().any(|i| !self.isolation.contains(i))
            || to.exclusive_minutes_day > self.exclusive_minutes_day
            || to.ci_cpu_hours_day > self.ci_cpu_hours_day
            || to.max_ci_running > self.max_ci_running
            || to.max_job_secs > self.max_job_secs
            || to.packages > self.packages
            || to.profiles.iter().any(|p| !self.profiles.contains(p))
            || (to.network && !self.network)
            // A cap on others is a widening for this lane's clients and a
            // narrowing for everyone else: always the owner's.
            || to.other_tenants_max_job_secs != self.other_tenants_max_job_secs
    }

    /// One line for people.
    pub fn summary(&self) -> String {
        let iso: Vec<&str> = self
            .isolation
            .iter()
            .map(|i| match i {
                Isolation::JobUid => "job_uid",
                Isolation::Exclusive => "exclusive",
                Isolation::TenantUid => "tenant_uid",
                Isolation::SharedUid => "shared_uid",
            })
            .collect();
        format!(
            "CI tier {}, secrets {}{}, {} CI jobs at once, {} h per job, packages {}, network {}, exclusive {} min/day, others capped {}",
            if self.tier_ci { "on" } else { "off" },
            if self.secrets { "on" } else { "off" },
            if self.secrets { format!(" ({})", iso.join(" or ")) } else { String::new() },
            self.max_ci_running,
            self.max_job_secs / 3600,
            self.packages.as_str(),
            if self.network { "on" } else { "off" },
            self.exclusive_minutes_day,
            self.other_tenants_max_job_secs.map(|s| format!("at {} min", s / 60)).unwrap_or_else(|| "off".into()),
        )
    }
}

/// One client server seated on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSeat {
    /// The seat's id (the owner's [`SeatStatement::seat_id`]).
    pub seat: String,
    /// The client server's full tunnel-key fingerprint (64 hex).
    pub server_fp: String,
    pub lane: String,
    /// The client's label (its host), shown only to the node's owner.
    pub label: String,
}

/// `/etc/k2-node-lanes.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanesFile {
    pub version: u32,
    #[serde(default)]
    pub lanes: BTreeMap<String, LaneTemplate>,
    /// Sorted by `server_fp`, one line per client server.
    #[serde(default)]
    pub seats: Vec<LaneSeat>,
}

impl Default for LanesFile {
    fn default() -> Self {
        LanesFile { version: LANES_FILE_VERSION, lanes: BTreeMap::new(), seats: Vec::new() }
    }
}

impl LanesFile {
    pub fn check(&self) -> Result<(), String> {
        if self.version != LANES_FILE_VERSION {
            return Err(format!("lanes file version {} (this build knows {LANES_FILE_VERSION})", self.version));
        }
        if self.lanes.len() > MAX_LANES {
            return Err(format!("lanes file: over {MAX_LANES} lanes"));
        }
        for (name, t) in &self.lanes {
            ci::check_name("lanes file lane", name)?;
            t.check().map_err(|e| format!("lane {name}: {e}"))?;
        }
        if self.seats.len() > MAX_SEAT_LINES {
            return Err(format!("lanes file: over {MAX_SEAT_LINES} seats"));
        }
        let fps: Vec<&str> = self.seats.iter().map(|s| s.server_fp.as_str()).collect();
        ci::check_sorted_unique("lanes file seats (by server_fp)", &fps)?;
        for s in &self.seats {
            ci::check_name("lanes file seat", &s.seat)?;
            ci::check_fp("lanes file seat server_fp", &s.server_fp)?;
            if !self.lanes.contains_key(&s.lane) {
                return Err(format!("lanes file: seat {} names lane {:?}, which the file doesn't define", s.seat, s.lane));
            }
            ci::check_text("lanes file seat label", &s.label, 253)?;
        }
        Ok(())
    }

    /// The lane a server's jobs run under on this node, if it holds a seat
    /// here. `server_fp` is the full fingerprint (or the 16+ hex prefix
    /// `servers.toml` keeps).
    pub fn lane_for(&self, server_fp: &str) -> Option<(&LaneSeat, &LaneTemplate)> {
        if server_fp.len() < 16 {
            return None;
        }
        let s = self.seats.iter().find(|s| s.server_fp.starts_with(server_fp))?;
        Some((s, self.lanes.get(&s.lane)?))
    }

    pub fn parse(text: &str) -> Result<LanesFile, String> {
        let f: LanesFile = toml::from_str(text).map_err(|e| format!("lanes file: {e}"))?;
        f.check()?;
        Ok(f)
    }

    pub fn render(&self) -> Result<String, String> {
        self.check()?;
        let body = toml::to_string(self).map_err(|e| format!("lanes file: {e}"))?;
        Ok(format!(
            "# CI lanes and seats on this node (root's file). Written by the node's\n\
             # owner server (k2 compute seats …): one line per client server seated\n\
             # here and the lane its jobs run under. Hand edits are replaced at the\n\
             # owner's next seat change.\n\n{body}"
        ))
    }
}

/// `lanes.write` arguments: the whole file.
pub fn check_write_args(args: &serde_json::Value) -> Result<LanesFile, String> {
    let o = args.as_object().ok_or("lanes.write: arguments are {\"file\": {…}}")?;
    if o.len() != 1 || !o.contains_key("file") {
        return Err("lanes.write: arguments are {\"file\": {…}} only".into());
    }
    let f: LanesFile = serde_json::from_value(o["file"].clone()).map_err(|e| format!("lanes.write: {e}"))?;
    f.check()?;
    Ok(f)
}

// ── the node roster on the node ─────────────────────────────────────

/// The owner's roster and the seats that name a node: the
/// [`VERB_NODE_ROSTER_SET`] arguments (all seats for that node), and the
/// `node_roster` frame's body (only the receiving server's own seats).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRosterDoc {
    pub roster: Signed,
    #[serde(default)]
    pub seats: Vec<Signed>,
}

/// A [`NodeRosterDoc`] whose signatures all verified with its roster's key.
#[derive(Debug, Clone)]
pub struct OpenedRoster {
    pub key: ApprovalKey,
    pub signed: Signed,
    pub roster: NodeRoster,
    /// `(signed, opened)`.
    pub seats: Vec<(Signed, SeatStatement)>,
}

impl NodeRosterDoc {
    /// Verify the roster with the key it names (`pinned`: the key this
    /// side already holds; `None` = trust on first use), then every seat
    /// with the same key and owner, issued against this roster or an older
    /// one.
    pub fn open(&self, pinned: Option<&ApprovalKey>) -> Result<OpenedRoster, String> {
        if self.seats.len() > MAX_SET_SEATS {
            return Err(format!("node roster: over {MAX_SET_SEATS} seats"));
        }
        let peek: NodeRoster = self.roster.peek(ci::DOMAIN_NODE_ROSTER)?;
        let named = ApprovalKey::parse(&peek.roster_key).ok_or("node roster: roster_key is not an ed25519 key")?;
        if let Some(p) = pinned {
            if p.encode() != named.encode() {
                return Err(format!("{}: the roster is signed by another key than the pinned one", codes::ROSTER_KEY_CHANGED));
            }
        }
        let roster = NodeRoster::open(&self.roster, &named)?;
        let mut seats = Vec::new();
        for s in &self.seats {
            let st = SeatStatement::open(s, &named)?;
            if st.owner_id != roster.owner_id {
                return Err("node roster: a seat names another owner".into());
            }
            if st.roster_seq > roster.seq {
                return Err("node roster: a seat was issued against a newer roster than this one".into());
            }
            seats.push((s.clone(), st));
        }
        Ok(OpenedRoster { key: named, signed: self.roster.clone(), roster, seats })
    }
}

impl OpenedRoster {
    /// This node's entry name (by its key), if the roster lists it active.
    pub fn name_of(&self, node_fp: &str) -> Option<&str> {
        self.roster.entry(node_fp).filter(|e| e.status == ci::KeyStatus::Active).map(|e| e.name.as_str())
    }

    /// What one server may see: the roster and its own seats only (a client
    /// never sees another client's identity).
    pub fn for_server(&self, server_fp: &str) -> NodeRosterDoc {
        let seats = self
            .seats
            .iter()
            .filter(|(_, st)| server_fp.len() >= 16 && st.client_fp.starts_with(server_fp))
            .map(|(s, _)| s.clone())
            .collect();
        NodeRosterDoc { roster: self.signed.clone(), seats }
    }

    /// The whole document back (what the node stores).
    pub fn doc(&self) -> NodeRosterDoc {
        NodeRosterDoc { roster: self.signed.clone(), seats: self.seats.iter().map(|(s, _)| s.clone()).collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> LanesFile {
        let mut f = LanesFile::default();
        f.lanes.insert(LANE_CI_STANDARD.into(), LaneTemplate::ci_standard());
        f.seats.push(LaneSeat { seat: "seat-a".into(), server_fp: "a".repeat(64), lane: LANE_CI_STANDARD.into(), label: "client-a.k2.dev".into() });
        f.seats.push(LaneSeat { seat: "seat-b".into(), server_fp: "b".repeat(64), lane: LANE_CI_STANDARD.into(), label: "client-b.k2.dev".into() });
        f
    }

    #[test]
    fn ci_standard_is_valid_and_caps_nobody_else() {
        let t = LaneTemplate::ci_standard();
        t.check().unwrap();
        assert_eq!(t.other_tenants_max_job_secs, None, "V23: off unless the owner sets it");
        assert!(t.secrets && t.tier_ci && t.network);
        assert_eq!(t.isolation, vec![Isolation::JobUid, Isolation::Exclusive]);
        assert!(LaneTemplate::named("ci-standard").is_some() && LaneTemplate::named("nope").is_none());
    }

    #[test]
    fn template_rules() {
        let t = LaneTemplate::ci_standard();
        assert!(LaneTemplate { isolation: vec![Isolation::TenantUid], ..t.clone() }.check().is_err());
        assert!(LaneTemplate { isolation: vec![], ..t.clone() }.check().is_err(), "secrets need an isolation");
        LaneTemplate { isolation: vec![], secrets: false, ..t.clone() }.check().unwrap();
        assert!(LaneTemplate { isolation: vec![Isolation::Exclusive, Isolation::JobUid], ..t.clone() }.check().is_err(), "sorted");
        assert!(LaneTemplate { max_ci_running: 0, ..t.clone() }.check().is_err());
        assert!(LaneTemplate { max_job_secs: 10, ..t.clone() }.check().is_err());
        assert!(LaneTemplate { other_tenants_max_job_secs: Some(10), ..t.clone() }.check().is_err());
        LaneTemplate { other_tenants_max_job_secs: Some(1800), ..t.clone() }.check().unwrap();
        assert!(LaneTemplate { profiles: vec!["nope@1".into()], ..t.clone() }.check().is_err());
    }

    #[test]
    fn widening_is_any_more_and_any_cap_change() {
        let t = LaneTemplate::ci_standard();
        let narrower = LaneTemplate { secrets: false, isolation: vec![], max_ci_running: 1, network: false, ..t.clone() };
        assert!(!t.widened_by(&narrower));
        assert!(narrower.widened_by(&t));
        assert!(t.widened_by(&LaneTemplate { other_tenants_max_job_secs: Some(1800), ..t.clone() }));
        assert!(!t.widened_by(&t));
    }

    #[test]
    fn file_round_trips_through_toml_and_refuses_unknown_keys() {
        let f = file();
        let text = f.render().unwrap();
        assert!(text.contains("version = 1"));
        assert!(text.contains("isolation = [\"job_uid\", \"exclusive\"]"), "{text}");
        assert!(!text.contains("other_tenants_max_job_secs"), "absent = off");
        assert_eq!(LanesFile::parse(&text).unwrap(), f);
        assert!(LanesFile::parse(&format!("{text}\nsurprise = 1\n")).is_err());
        assert!(LanesFile::parse(&text.replace("version = 1", "version = 2")).is_err());
        let mut lane = text.replace("network = true", "network = true\nmystery = 3");
        assert!(LanesFile::parse(&lane).is_err());
        lane = text.replace("lane = \"ci-standard\"", "lane = \"nope\"");
        assert!(LanesFile::parse(&lane).unwrap_err().contains("doesn't define"));
    }

    #[test]
    fn file_rules_and_lookup() {
        let f = file();
        assert_eq!(f.lane_for(&"a".repeat(16)).unwrap().0.seat, "seat-a");
        assert_eq!(f.lane_for(&"b".repeat(64)).unwrap().0.seat, "seat-b");
        assert!(f.lane_for(&"c".repeat(64)).is_none());
        assert!(f.lane_for("aaaa").is_none(), "a short prefix never matches");
        let mut unsorted = f.clone();
        unsorted.seats.reverse();
        assert!(unsorted.check().is_err());
        let mut short = f.clone();
        short.seats[0].server_fp = "a".repeat(16);
        assert!(short.check().is_err(), "the file names full fingerprints");
    }

    #[test]
    fn write_args() {
        let f = file();
        let v = serde_json::json!({"file": f});
        assert_eq!(check_write_args(&v).unwrap(), f);
        assert!(check_write_args(&serde_json::json!({"file": f, "x": 1})).is_err());
        assert!(check_write_args(&serde_json::json!({})).is_err());
        let mut bad = serde_json::to_value(&f).unwrap();
        bad["lanes"]["ci-standard"]["max_ci_running"] = 0.into();
        assert!(check_write_args(&serde_json::json!({"file": bad})).is_err());
    }

    fn roster_doc(o: &crate::admin::ApprovalSigner, node: &crate::crypto::SigningKey, clients: &[&str]) -> NodeRosterDoc {
        use crate::ci::{KeyStatus, SeatState, TaintBasis};
        use crate::node_roster::RosterEntry;
        let r = NodeRoster {
            v: 1,
            owner_id: "owner.k2.dev".into(),
            owner_server_fp: "e".repeat(64),
            roster_key: o.public().encode(),
            seq: 3,
            issued_at: 1_700_000_000,
            prev_digest: Some(format!("sha256:{}", "2".repeat(64))),
            entries: vec![RosterEntry {
                node_fp: node.fingerprint(),
                node_spki: crate::crypto::b64(node.spki_der()),
                name: "n1".into(),
                key_epoch: 0,
                statement_digest: format!("sha256:{}", "1".repeat(64)),
                status: KeyStatus::Active,
                revoked_reason: None,
                valid_from: 1,
                valid_to: None,
                taint_basis: Some(TaintBasis::SelfClean),
                attestation_digest: None,
                host: "h1".into(),
                provider: None,
                site: None,
                admin_reach: vec![],
                pod: None,
                point: None,
            }],
        };
        let seats = clients
            .iter()
            .map(|c| {
                SeatStatement {
                    v: 1,
                    owner_id: "owner.k2.dev".into(),
                    roster_key: o.public().encode(),
                    seat_id: format!("seat-{}", &c[..1]),
                    client_fp: c.to_string(),
                    client_label: "c".into(),
                    lane: LANE_CI_STANDARD.into(),
                    nodes: vec!["n1".into(), "n2".into()],
                    roster_seq: 3,
                    state: SeatState::Active,
                    issued_at: 1_700_000_000,
                    pod: None,
                    line: None,
                }
                .sign(o)
                .unwrap()
            })
            .collect();
        NodeRosterDoc { roster: r.sign(o).unwrap(), seats }
    }

    #[test]
    fn roster_doc_opens_pins_and_filters_per_server() {
        let o = crate::admin::ApprovalSigner::from_seed(&[4u8; 32]).unwrap();
        let node = crate::crypto::SigningKey::generate().unwrap();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let doc = roster_doc(&o, &node, &[&a, &b]);
        let opened = doc.open(None).unwrap();
        assert_eq!(opened.name_of(&node.fingerprint()), Some("n1"));
        assert_eq!(opened.doc(), doc);
        // Each server gets its own seat only.
        let for_a = opened.for_server(&a[..16]);
        assert_eq!(for_a.seats.len(), 1);
        assert!(for_a.seats[0].body.contains(&a) && !for_a.seats[0].body.contains(&b));
        assert!(opened.for_server(&"c".repeat(64)).seats.is_empty());
        assert!(opened.for_server("").seats.is_empty());
        // Pinned: the same key opens; another key's roster is refused.
        doc.open(Some(&o.public())).unwrap();
        let other = crate::admin::ApprovalSigner::from_seed(&[5u8; 32]).unwrap();
        assert!(roster_doc(&other, &node, &[]).open(Some(&o.public())).unwrap_err().contains(codes::ROSTER_KEY_CHANGED));
        // A seat signed by another key, or for a newer roster: refused.
        let mut mixed = doc.clone();
        mixed.seats.push(roster_doc(&other, &node, &[&a]).seats.remove(0));
        assert!(mixed.open(None).is_err());
        let mut tampered = doc.clone();
        tampered.roster.body = tampered.roster.body.replace("\"h1\"", "\"h2\"");
        assert!(tampered.open(None).is_err());
    }
}

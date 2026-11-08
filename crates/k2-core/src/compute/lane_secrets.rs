//! Lane secrets, the controller's rules (0.46.10, prd-ci-on-compute-for-
//! clients-v1 §4.4 and its vs-live amendments V24–V26; slice C3b).
//!
//! A build secret may be granted to a **lane** instead of one node:
//! `k2 vault grant NAME --to compute:<ws>/lane:<owner>/<lane>`, stored as
//! the vault consumer `compute:<ws id>/@lane-<seat id>` (V26). It reaches a
//! job only on a node of that seat, and only when every rule below holds
//! **on this server** before anything is sealed (V25); the node checks its
//! own half again (V24, `k2-node` `lane_refusal`):
//!
//! 1. the seat is active and names the node;
//! 2. the owner's lane allows secrets for this client (`secrets = true`);
//! 3. the node is Linux, behind the job wall, offering `secret-env` and
//!    `lane-secrets` to this server (k2-node with protocol 9);
//! 4. the job runs with one of the lane's isolations: `job_uid` (the node
//!    offers `job-uid`) or `exclusive` (the plan says so);
//! 5. the node's key is active, named by the seat and untainted in the
//!    owner's node roster, and not revoked;
//! 6. a person here agreed to "send vault secrets to nodes owned by
//!    <owner>; that owner's root can read them", for that owner's pinned
//!    roster key, else `secret_owner_not_trusted`.
//!
//! **One consent record.** Rule 6 reads the pinned owner
//! (`compute_owner_trust.secrets_consent`, migration 0274,
//! `crate::compute::trust`): the seat request card a person approves (with
//! a fresh confirm bound to that request) pins the owner's roster key and
//! gives the consent in the same step; `k2 compute owners consent <owner>
//! on|off` gives it later (a person, a fresh confirm bound to
//! `consent:<roster key fp>`) or takes it back at once. A new roster key
//! is a new pin, so it needs a new consent.
//!
//! Seats, lanes and rosters are C3a/C1's tables; the controller reads them
//! through [`SeatBook`]. The daemon installs [`TrustSeats`] (this server's
//! own tables) at boot; until then [`NoSeats`] answers "no seat", so every
//! lane grant is refused and nothing changes for per-node grants.
//! [`StaticSeats`] is the test stub.

use std::sync::{Arc, RwLock};

use rusqlite::{params, Connection};
use serde::Serialize;

use super::proto::ci::{self, Isolation, KeyStatus};
use super::proto::frames::{Offer, FEATURE_JOB_WALL, PROTOCOL_CI};
use super::proto::lanes::LaneTemplate;
use super::proto::secret::{codes as scodes, FEATURE_SECRET_ENV};
use super::store::NodeRow;
use super::trust;

/// A refusal: `(code, message for a person)`.
pub type Refused = (&'static str, String);

// ── the seat book ────────────────────────────────────────────────────

/// One of this server's seats on another owner's nodes: the owner-signed
/// seat statement plus the lane template the owner sent with the offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatView {
    /// `SeatStatement.seat_id` (a frozen name).
    pub seat_id: String,
    /// The node owner's server host (`NodeRoster.owner_id`, e.g.
    /// `rosson.k2.dev`): what consent is given for.
    pub owner_id: String,
    /// The fingerprint of the owner's roster key a person pinned here
    /// (`compute_owner_trust.key_fp`): consent is bound to it.
    pub owner_key_fp: String,
    /// The lane name (`SeatStatement.lane`).
    pub lane: String,
    /// `SeatStatement.state == active`.
    pub active: bool,
    /// The lane template's `secrets`.
    pub lane_secrets: bool,
    /// The lane template's `isolation` for secret jobs (`job_uid`,
    /// `exclusive`; sorted).
    pub isolation: Vec<Isolation>,
    /// This server's `compute_nodes.id` of every node that enrolled through
    /// the seat.
    pub node_ids: Vec<String>,
}

/// A node's key as the owner's node roster has it (C1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStanding {
    /// Active, named by the seat, with a taint basis.
    Clean,
    /// Active, but the owner vouches for nothing on it (no taint basis).
    Tainted,
    /// The owner's roster revoked it.
    Revoked,
    /// Not on the roster as an active key under a name the seat lists, or
    /// no verified roster held.
    Unlisted,
}

/// What the controller asks about seats. Every method is a lookup on this
/// server's own store; none may block on the network.
pub trait SeatBook: Send + Sync {
    /// The seat with this id.
    fn seat(&self, seat_id: &str) -> Option<SeatView>;
    /// This server's seat on `owner_id`'s lane `lane` (the CLI form
    /// `compute:<ws>/lane:<owner>/<lane>`).
    fn seat_named(&self, owner_id: &str, lane: &str) -> Option<SeatView>;
    /// Every seat (any state) that names the node `node_id`.
    fn seats_with_node(&self, node_id: &str) -> Vec<SeatView>;
    /// The node's key in the seat owner's verified node roster.
    fn node_standing(&self, seat: &SeatView, node: &NodeRow) -> NodeStanding;
}

/// No seats at all: every lane grant is refused, every node is "on no
/// seat".
pub struct NoSeats;

impl SeatBook for NoSeats {
    fn seat(&self, _: &str) -> Option<SeatView> {
        None
    }
    fn seat_named(&self, _: &str, _: &str) -> Option<SeatView> {
        None
    }
    fn seats_with_node(&self, _: &str) -> Vec<SeatView> {
        Vec::new()
    }
    fn node_standing(&self, _: &SeatView, _: &NodeRow) -> NodeStanding {
        NodeStanding::Unlisted
    }
}

/// A fixed book: the test stub. Standing per node id; a node not listed is
/// `Unlisted`.
#[derive(Debug, Clone, Default)]
pub struct StaticSeats {
    pub seats: Vec<SeatView>,
    pub standing: std::collections::BTreeMap<String, NodeStanding>,
}

impl SeatBook for StaticSeats {
    fn seat(&self, seat_id: &str) -> Option<SeatView> {
        self.seats.iter().find(|s| s.seat_id == seat_id).cloned()
    }
    fn seat_named(&self, owner_id: &str, lane: &str) -> Option<SeatView> {
        self.seats.iter().find(|s| s.owner_id == owner_id && s.lane == lane).cloned()
    }
    fn seats_with_node(&self, node_id: &str) -> Vec<SeatView> {
        self.seats.iter().filter(|s| s.node_ids.iter().any(|n| n == node_id)).cloned().collect()
    }
    fn node_standing(&self, _: &SeatView, node: &NodeRow) -> NodeStanding {
        self.standing.get(&node.id).copied().unwrap_or(NodeStanding::Unlisted)
    }
}

/// This server's own seats, lanes and rosters (`crate::compute::trust`,
/// migrations 0274/0275): the book the daemon installs at boot. Each call
/// takes the shared DB lock for its own lookup only, so a caller must not
/// hold it.
pub struct TrustSeats;

impl SeatBook for TrustSeats {
    fn seat(&self, seat_id: &str) -> Option<SeatView> {
        let db = crate::db::shared();
        let conn = db.lock();
        seat_view(&conn, seat_id)
    }
    fn seat_named(&self, owner_id: &str, lane: &str) -> Option<SeatView> {
        let db = crate::db::shared();
        let conn = db.lock();
        seat_named_in(&conn, owner_id, lane)
    }
    fn seats_with_node(&self, node_id: &str) -> Vec<SeatView> {
        let db = crate::db::shared();
        let conn = db.lock();
        seats_with_node_in(&conn, node_id)
    }
    fn node_standing(&self, seat: &SeatView, node: &NodeRow) -> NodeStanding {
        let db = crate::db::shared();
        let conn = db.lock();
        node_standing_in(&conn, seat, node)
    }
}

static BOOK: RwLock<Option<Arc<dyn SeatBook>>> = RwLock::new(None);

/// The installed seat book ([`NoSeats`] until one is set).
///
/// While CI on compute is off ([`super::ci_enabled`], 0.46.10's default)
/// this is always [`NoSeats`], whatever is installed: no lane grant
/// resolves, nothing is sealed under a lane's rules.
pub fn seat_book() -> Arc<dyn SeatBook> {
    if !super::ci_enabled() {
        return Arc::new(NoSeats);
    }
    BOOK.read().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| Arc::new(NoSeats))
}

/// Install a book (`None`: back to [`NoSeats`]). The daemon installs
/// [`TrustSeats`] at boot; tests install a [`StaticSeats`].
pub fn set_seat_book(book: Option<Arc<dyn SeatBook>>) {
    *BOOK.write().unwrap_or_else(|p| p.into_inner()) = book;
}

// ── the seat book over this server's tables ──────────────────────────

/// This server's nodes that enrolled through `seat_id` (V20 ties each one
/// to its seat: `compute_nodes.seat_id`).
fn seat_node_ids(conn: &Connection, seat_id: &str) -> Vec<String> {
    let Ok(mut st) = conn.prepare("SELECT id FROM compute_nodes WHERE seat_id = ?1 ORDER BY id") else { return Vec::new() };
    st.query_map(params![seat_id], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// A client seat row as the gate sees it. A seat with no lane template
/// (none sent, or one this build can't read) carries no secrets.
pub fn view_of(conn: &Connection, s: &trust::SeatRow) -> SeatView {
    let t: Option<LaneTemplate> = trust::seat_lane(conn, &s.side, &s.id).filter(|t| t.check().is_ok());
    SeatView {
        seat_id: s.id.clone(),
        owner_id: s.owner_id.clone(),
        owner_key_fp: s.owner_key_fp.clone().unwrap_or_default(),
        lane: s.lane.clone(),
        active: s.state == "active",
        lane_secrets: t.as_ref().is_some_and(|t| t.secrets),
        isolation: t.map(|t| t.isolation).unwrap_or_default(),
        node_ids: seat_node_ids(conn, &s.id),
    }
}

/// [`SeatBook::seat`] on `conn`: this server's (client) seat `seat_id`.
pub fn seat_view(conn: &Connection, seat_id: &str) -> Option<SeatView> {
    trust::seat(conn, "client", seat_id).map(|s| view_of(conn, &s))
}

/// [`SeatBook::seat_named`] on `conn` (an active seat first, if several).
pub fn seat_named_in(conn: &Connection, owner_id: &str, lane: &str) -> Option<SeatView> {
    let mut hits: Vec<trust::SeatRow> = trust::seats(conn, Some("client")).into_iter().filter(|s| s.owner_id == owner_id && s.lane == lane).collect();
    hits.sort_by_key(|s| (s.state != "active", s.id.clone()));
    hits.first().map(|s| view_of(conn, s))
}

/// [`SeatBook::seats_with_node`] on `conn`.
pub fn seats_with_node_in(conn: &Connection, node_id: &str) -> Vec<SeatView> {
    let (_, _, seat) = trust::node_key_facts(conn, node_id);
    seat.and_then(|id| seat_view(conn, &id)).into_iter().collect()
}

/// [`SeatBook::node_standing`] on `conn`: the node's key in the newest
/// roster this server holds under the seat's pinned owner key.
pub fn node_standing_in(conn: &Connection, seat: &SeatView, node: &NodeRow) -> NodeStanding {
    if trust::key_revoked(conn, &node.fingerprint).is_some() {
        return NodeStanding::Revoked;
    }
    if seat.owner_key_fp.is_empty() {
        return NodeStanding::Unlisted;
    }
    let Some(r) = trust::latest_roster(conn, &seat.owner_key_fp) else { return NodeStanding::Unlisted };
    let Some(seat_row) = trust::seat(conn, "client", &seat.seat_id) else { return NodeStanding::Unlisted };
    match r.roster.entry(&node.fingerprint).filter(|e| e.status == KeyStatus::Active && seat_row.nodes.contains(&e.name)) {
        None => NodeStanding::Unlisted,
        Some(e) if e.taint_basis.is_none() => NodeStanding::Tainted,
        Some(_) => NodeStanding::Clean,
    }
}

// ── consent (the pinned owner, `compute_owner_trust`) ────────────────

/// What the person agrees to, word for word (the seat request card and
/// `k2 compute owners consent` say it).
pub fn consent_text(owner_id: &str) -> String {
    format!(
        "Send this server's vault secrets to compute nodes owned by {owner_id}, for jobs on a lane of theirs that allows secrets. \
         That owner's root (the machine's admin) can read them; other tenants can't."
    )
}

/// A live consent: a person here agreed to send secrets to the nodes of
/// the owner whose roster key is `owner_key_fp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Consent {
    pub owner_id: String,
    pub owner_key_fp: String,
    pub consented_by: Option<String>,
    pub consented_at: Option<i64>,
}

/// The live consent covering `seat`'s owner: the pinned owner key the seat
/// names, with `secrets_consent` on.
pub fn consent_for(conn: &Connection, seat: &SeatView) -> Option<Consent> {
    if seat.owner_key_fp.is_empty() {
        return None;
    }
    let t = trust::owner_trust(conn, &seat.owner_key_fp).filter(|t| t.secrets_consent)?;
    Some(Consent { owner_id: t.owner_id, owner_key_fp: t.key_fp, consented_by: t.consent_by, consented_at: t.consent_at })
}

// ── the gate (V25) ───────────────────────────────────────────────────

/// Is this node's machine Linux, as far as this server knows (its row and
/// its live offer must both say so; a missing offer label counts as no)?
pub fn node_is_linux(node: &NodeRow, offer: Option<&Offer>) -> bool {
    let row = node.os.as_deref().or_else(|| node.labels.get("os").map(String::as_str)) == Some("linux");
    let live = offer.is_none_or(|o| o.labels.get("os").map(String::as_str) == Some("linux"));
    row && live
}

fn has(offer: &Offer, f: &str) -> bool {
    offer.features.iter().any(|x| x == f)
}

/// The isolation a secret job of this plan gets on this node: `exclusive`
/// when the plan says so, else `job_uid` when the node offers per-job
/// uids, else none good enough for a lane.
pub fn job_isolation(offer: &Offer, exclusive: bool) -> Option<Isolation> {
    if exclusive {
        Some(Isolation::Exclusive)
    } else if has(offer, ci::FEATURE_JOB_UID) {
        Some(Isolation::JobUid)
    } else {
        None
    }
}

/// Does the lane's `isolation` accept what the job gets? The job's must be
/// one the lane names (V24), where `exclusive` (the whole node) also meets
/// a lane naming only `job_uid`. A shared uid never does. The same rule as
/// the node's (`k2-node` `lane_accepts`).
pub fn lane_accepts(lane: &[Isolation], job: Isolation) -> bool {
    match job {
        Isolation::JobUid => lane.contains(&Isolation::JobUid),
        Isolation::Exclusive => lane.contains(&Isolation::Exclusive) || lane.contains(&Isolation::JobUid),
        Isolation::TenantUid | Isolation::SharedUid => false,
    }
}

/// Everything the gate looks at for one (node, seat, job).
pub struct GateInput<'a> {
    pub node: &'a NodeRow,
    /// The node's live offer to this server (`None`: not connected).
    pub offer: Option<&'a Offer>,
    pub seat: &'a SeatView,
    pub standing: NodeStanding,
    pub consent: Option<&'a Consent>,
    /// The plan's `exclusive`.
    pub exclusive: bool,
}

/// The command a person runs to agree (named in refusals).
pub fn consent_command(owner_id: &str) -> String {
    format!("k2 compute owners consent {owner_id} on")
}

/// V25: may this server seal a secret to `node` for a job on `seat`?
/// `Ok(())` only when every rule in the module docs holds. The one
/// controller check: submit, `--any` placement and every release call it
/// (through `k2-daemon` `compute_secrets::secret_gate`).
pub fn lane_gate(i: &GateInput<'_>) -> Result<(), Refused> {
    let (node, seat) = (i.node, i.seat);
    let who = format!("'{}'", node.name);
    if !seat.active {
        return Err((scodes::UNTRUSTED_SERVER, format!("your seat {} on {}'s lane {} isn't active, so {who} gets no secrets", seat.seat_id, seat.owner_id, seat.lane)));
    }
    if !seat.node_ids.iter().any(|n| n == &node.id) {
        return Err((scodes::UNTRUSTED_SERVER, format!("{who} isn't on your seat {} ({}'s lane {})", seat.seat_id, seat.owner_id, seat.lane)));
    }
    if !seat.lane_secrets {
        return Err((scodes::UNTRUSTED_SERVER, format!("{}'s lane {} doesn't allow secrets (ask them to turn secrets on for the lane)", seat.owner_id, seat.lane)));
    }
    if !node_is_linux(node, i.offer) {
        return Err((scodes::LINUX_ONLY, format!("{who} isn't a Linux node: secrets go only to Linux nodes of a lane (one uid per Mac node)")));
    }
    let Some(offer) = i.offer else {
        return Err((scodes::FEATURE_MISSING, format!("{who} isn't connected now; secrets are sealed only to a node whose live offer is checked")));
    };
    if offer.protocol < PROTOCOL_CI || !has(offer, ci::FEATURE_LANE_SECRETS) || !has(offer, FEATURE_SECRET_ENV) {
        return Err((
            scodes::NEEDS_UPDATE,
            format!("{who} doesn't offer lane secrets to this server: it needs k2-node with protocol 9, the lane on, and the job wall (k2 compute info {} shows its features)", node.name),
        ));
    }
    if !has(offer, FEATURE_JOB_WALL) {
        return Err((scodes::NEEDS_WALL, format!("{who} runs without the job wall; lane secrets need it")));
    }
    match job_isolation(offer, i.exclusive) {
        Some(iso) if lane_accepts(&seat.isolation, iso) => {}
        got => {
            let want = if seat.isolation.contains(&Isolation::JobUid) { "a per-job uid (the node's job-uid) or --exclusive" } else { "--exclusive" };
            return Err((
                scodes::NEEDS_ISOLATION,
                format!(
                    "{}'s lane {} needs {want} for secret jobs; on {who} this job would get {}",
                    seat.owner_id,
                    seat.lane,
                    got.map(isolation_label).unwrap_or("its tenant's shared uid")
                ),
            ));
        }
    }
    match i.standing {
        NodeStanding::Clean => {}
        NodeStanding::Tainted => {
            return Err((ci::codes::NODE_KEY_TAINTED, format!("{who}'s key is tainted in {}'s node roster; it gets secrets again after a re-key", seat.owner_id)));
        }
        NodeStanding::Revoked => {
            return Err((ci::codes::NODE_KEY_TAINTED, format!("{}'s node roster revoked {who}'s key; it never gets secrets again", seat.owner_id)));
        }
        NodeStanding::Unlisted => {
            return Err((scodes::UNTRUSTED_SERVER, format!("{who}'s key isn't active under your seat in {}'s node roster this server holds", seat.owner_id)));
        }
    }
    match i.consent {
        Some(c) if c.owner_key_fp == seat.owner_key_fp && !seat.owner_key_fp.is_empty() => Ok(()),
        _ => Err((
            ci::codes::SECRET_OWNER_NOT_TRUSTED,
            format!(
                "no person here agreed to send vault secrets to nodes owned by {} (their root can read them). Only a person can agree, in a terminal outside K2: {}",
                seat.owner_id,
                consent_command(&seat.owner_id)
            ),
        )),
    }
}

fn isolation_label(i: Isolation) -> &'static str {
    match i {
        Isolation::JobUid => "job_uid",
        Isolation::Exclusive => "exclusive",
        Isolation::TenantUid => "tenant_uid",
        Isolation::SharedUid => "shared_uid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::frames::{Availability, Caps, Control, Free};

    fn node(id: &str, os: &str) -> NodeRow {
        let conn = crate::db::isolated_test_connection();
        let labels = std::collections::BTreeMap::from([("os".to_string(), os.to_string())]);
        crate::compute::store::insert_pending_node(&conn, id, id, &format!("{id:0>64}"), "pem", &labels, "owner", "123456", 1).unwrap();
        let mut n = crate::compute::store::node_by_id(&conn, id).unwrap();
        n.role = "tenant".into();
        n.owner_label = Some("owner.k2.dev".into());
        n
    }

    fn offer(features: &[&str], protocol: u32, os: &str) -> Offer {
        Offer {
            share: None,
            revision: 1,
            control: Control::Active,
            control_by: None,
            control_at: None,
            availability: Availability { ok: true, reasons: vec![] },
            caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
            free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
            slots: vec![],
            running: 0,
            parallel_max: 4,
            refusal: None,
            labels: [("os".to_string(), os.to_string())].into(),
            tools: Default::default(),
            protocol,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0.46.10".into(),
            features: features.iter().map(|f| f.to_string()).collect(),
            host: None,
            pkg: None,
        }
    }

    fn seat(nodes: &[&str]) -> SeatView {
        SeatView {
            seat_id: "seat-1".into(),
            owner_id: "owner.k2.dev".into(),
            owner_key_fp: "e".repeat(64),
            lane: "ci-standard".into(),
            active: true,
            lane_secrets: true,
            isolation: vec![Isolation::JobUid, Isolation::Exclusive],
            node_ids: nodes.iter().map(|n| n.to_string()).collect(),
        }
    }

    const FULL: &[&str] = &[FEATURE_SECRET_ENV, ci::FEATURE_LANE_SECRETS, ci::FEATURE_JOB_UID, FEATURE_JOB_WALL];

    fn consent() -> Consent {
        Consent { owner_id: "owner.k2.dev".into(), owner_key_fp: "e".repeat(64), consented_by: Some("owner".into()), consented_at: Some(1) }
    }

    fn gate(n: &NodeRow, o: Option<&Offer>, s: &SeatView, st: NodeStanding, c: Option<&Consent>, excl: bool) -> Result<(), &'static str> {
        lane_gate(&GateInput { node: n, offer: o, seat: s, standing: st, consent: c, exclusive: excl }).map_err(|e| e.0)
    }

    #[test]
    fn every_rule_must_hold_before_a_seal() {
        let n = node("n1", "linux");
        let s = seat(&["n1"]);
        let o = offer(FULL, 9, "linux");
        let c = consent();
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Clean, Some(&c), false), Ok(()), "every rule holds");

        // Consent, for the pinned owner key.
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Clean, None, false), Err(ci::codes::SECRET_OWNER_NOT_TRUSTED));
        let mut moved = consent();
        moved.owner_key_fp = "f".repeat(64);
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Clean, Some(&moved), false), Err(ci::codes::SECRET_OWNER_NOT_TRUSTED), "another roster key needs its own consent");
        // Linux only (row and live offer).
        let mac = node("n1", "macos");
        assert_eq!(gate(&mac, Some(&offer(FULL, 9, "macos")), &s, NodeStanding::Clean, Some(&c), false), Err(scodes::LINUX_ONLY));
        assert_eq!(gate(&n, Some(&offer(FULL, 9, "macos")), &s, NodeStanding::Clean, Some(&c), false), Err(scodes::LINUX_ONLY));
        // Isolation: no per-job uid and not exclusive.
        let no_uid = offer(&[FEATURE_SECRET_ENV, ci::FEATURE_LANE_SECRETS, FEATURE_JOB_WALL], 9, "linux");
        assert_eq!(gate(&n, Some(&no_uid), &s, NodeStanding::Clean, Some(&c), false), Err(scodes::NEEDS_ISOLATION));
        assert_eq!(gate(&n, Some(&no_uid), &s, NodeStanding::Clean, Some(&c), true), Ok(()), "exclusive is enough");
        let mut excl_lane = seat(&["n1"]);
        excl_lane.isolation = vec![Isolation::Exclusive];
        assert_eq!(gate(&n, Some(&o), &excl_lane, NodeStanding::Clean, Some(&c), false), Err(scodes::NEEDS_ISOLATION), "an exclusive lane takes no job_uid");
        assert_eq!(gate(&n, Some(&o), &excl_lane, NodeStanding::Clean, Some(&c), true), Ok(()));
        let mut uid_lane = seat(&["n1"]);
        uid_lane.isolation = vec![Isolation::JobUid];
        assert_eq!(gate(&n, Some(&no_uid), &uid_lane, NodeStanding::Clean, Some(&c), true), Ok(()), "exclusive meets a job_uid-only lane");
        let mut weak = seat(&["n1"]);
        weak.isolation = vec![Isolation::TenantUid];
        assert_eq!(gate(&n, Some(&o), &weak, NodeStanding::Clean, Some(&c), true), Err(scodes::NEEDS_ISOLATION), "a lane naming a shared uid never carries secrets");
        let mut none = seat(&["n1"]);
        none.isolation = vec![];
        assert_eq!(gate(&n, Some(&o), &none, NodeStanding::Clean, Some(&c), true), Err(scodes::NEEDS_ISOLATION));
        // The job wall.
        let unwalled = offer(&[FEATURE_SECRET_ENV, ci::FEATURE_LANE_SECRETS, ci::FEATURE_JOB_UID], 9, "linux");
        assert_eq!(gate(&n, Some(&unwalled), &s, NodeStanding::Clean, Some(&c), false), Err(scodes::NEEDS_WALL));
        // Protocol 8 / no lane-secrets: an older node never gets one.
        let p8 = offer(&[FEATURE_SECRET_ENV, FEATURE_JOB_WALL], 8, "linux");
        assert_eq!(gate(&n, Some(&p8), &s, NodeStanding::Clean, Some(&c), true), Err(scodes::NEEDS_UPDATE));
        assert_eq!(gate(&n, Some(&offer(&[ci::FEATURE_LANE_SECRETS, ci::FEATURE_JOB_UID, FEATURE_JOB_WALL], 9, "linux")), &s, NodeStanding::Clean, Some(&c), false), Err(scodes::NEEDS_UPDATE), "no secret-env");
        // Offline.
        assert_eq!(gate(&n, None, &s, NodeStanding::Clean, Some(&c), false), Err(scodes::FEATURE_MISSING));
        // The roster.
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Tainted, Some(&c), false), Err(ci::codes::NODE_KEY_TAINTED));
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Revoked, Some(&c), false), Err(ci::codes::NODE_KEY_TAINTED));
        assert_eq!(gate(&n, Some(&o), &s, NodeStanding::Unlisted, Some(&c), false), Err(scodes::UNTRUSTED_SERVER));
        // The seat and the lane.
        let mut off = seat(&["n1"]);
        off.active = false;
        assert_eq!(gate(&n, Some(&o), &off, NodeStanding::Clean, Some(&c), false), Err(scodes::UNTRUSTED_SERVER));
        assert_eq!(gate(&n, Some(&o), &seat(&["n2"]), NodeStanding::Clean, Some(&c), false), Err(scodes::UNTRUSTED_SERVER), "not on the seat");
        let mut closed = seat(&["n1"]);
        closed.lane_secrets = false;
        assert_eq!(gate(&n, Some(&o), &closed, NodeStanding::Clean, Some(&c), false), Err(scodes::UNTRUSTED_SERVER));
        // The refusal names the person's command.
        let e = lane_gate(&GateInput { node: &n, offer: Some(&o), seat: &s, standing: NodeStanding::Clean, consent: None, exclusive: false }).unwrap_err();
        assert!(e.1.contains("k2 compute owners consent owner.k2.dev on"), "{}", e.1);
    }

    #[test]
    fn the_static_book_and_no_seats() {
        let n = node("n1", "linux");
        let s = seat(&["n1"]);
        let book = StaticSeats { seats: vec![s.clone()], standing: [("n1".to_string(), NodeStanding::Clean)].into() };
        assert_eq!(book.seat("seat-1"), Some(s.clone()));
        assert_eq!(book.seat_named("owner.k2.dev", "ci-standard"), Some(s.clone()));
        assert_eq!(book.seat_named("owner.k2.dev", "other"), None);
        assert_eq!(book.seats_with_node("n1"), vec![s.clone()]);
        assert!(book.seats_with_node("n2").is_empty());
        assert_eq!(book.node_standing(&s, &n), NodeStanding::Clean);
        assert_eq!(NoSeats.seats_with_node("n1"), vec![]);
        assert_eq!(NoSeats.node_standing(&s, &n), NodeStanding::Unlisted);
        assert!(consent_text("owner.k2.dev").contains("root"), "the card says the owner's root can read them");
    }
}

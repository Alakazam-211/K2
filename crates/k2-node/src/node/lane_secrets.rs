//! Lane secrets on the node (0.46.10, prd-ci-on-compute-for-clients-v1
//! §4.4, vs-live V24; slice C3b). The node's half of "may this server's
//! job get secret env here":
//!
//! `own || (lane.secrets && job isolation ∈ lane.isolation && wall == job_user && ptrace_scope ≥ 1)`
//!
//! for a server speaking protocol 9 (the `lane-secrets` feature is offered
//! only to such a session), on Linux, behind the per-tenant wall. The
//! server (controller) checks its own half before it seals anything (V25:
//! the seat, the owner's roster, its owner's consent); this half is what
//! the node owner's root files say.
//!
//! The lanes themselves (`/etc/k2-node-lanes.toml`, its Privileged verb
//! `lanes.write`, the lane template) are slice C3a's
//! (`k2_node_proto::lanes`). This module reads them through [`LaneSource`]
//! (`NodeOptions::lanes`). A node opened without one reads root's file
//! ([`FileLanes`], cached by its size and mtime); a `--dev` node has none,
//! so no server is on a lane there and today's rule (`trust own` only)
//! holds exactly.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use k2_node_proto::ci::{self, Isolation};
use k2_node_proto::frames::Plan;
use k2_node_proto::lanes::LanesFile;
use k2_node_proto::queue::UidShare;
use k2_node_proto::secret::codes;
use k2_node_proto::share::{Slot, Trust};

use super::{Node, Tenant};
use crate::wall::Wall;

/// What the node's lanes say for one tenant server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneRule {
    /// The lane's name (a frozen name; receipt v2's `run.lane`).
    pub lane: String,
    /// The lane template's `secrets`.
    pub secrets: bool,
    /// The lane template's `isolation` for secret jobs (sorted, from
    /// `job_uid` and `exclusive`).
    pub isolation: Vec<Isolation>,
}

/// The tenant server a lane lookup is for.
#[derive(Debug, Clone, Copy)]
pub struct LaneServer<'a> {
    pub slot: Slot,
    /// Its `servers.toml` line's `fp_prefix` (empty when it has none).
    pub fp_prefix: &'a str,
    /// The full fingerprint it attached with (its pin), when known.
    pub controller_fp: Option<&'a str>,
}

/// The node's lanes. Must answer from memory (no I/O on the session path
/// beyond a cheap cached read).
pub trait LaneSource: Send + Sync {
    /// The lane `server` is on here, if any.
    fn lane_for(&self, server: &LaneServer<'_>) -> Option<LaneRule>;
}

/// No lanes: today's node.
pub struct NoLanes;

impl LaneSource for NoLanes {
    fn lane_for(&self, _: &LaneServer<'_>) -> Option<LaneRule> {
        None
    }
}

/// Fixed lanes by tenant slot: the test stub.
#[derive(Debug, Clone, Default)]
pub struct StaticLanes(pub BTreeMap<Slot, LaneRule>);

impl LaneSource for StaticLanes {
    fn lane_for(&self, server: &LaneServer<'_>) -> Option<LaneRule> {
        self.0.get(&server.slot).cloned()
    }
}

/// Root's lanes file (`/etc/k2-node-lanes.toml`, C3a): `[lanes.<name>]`
/// templates and one `[[seats]]` line per client server, matched here by
/// the server's **full pinned fingerprint** only (a `servers.toml` prefix
/// never names a lane: a lane carries secrets). The file is root's 0644;
/// the node reads it, never writes it. Parsed again only when its size or
/// mtime changes; a missing, damaged or unknown-version file is no lanes.
pub struct FileLanes {
    path: PathBuf,
    cache: Mutex<Option<(FileStamp, Option<LanesFile>)>>,
}

type FileStamp = (u64, Option<std::time::SystemTime>);

impl FileLanes {
    pub fn new(path: PathBuf) -> Self {
        FileLanes { path, cache: Mutex::new(None) }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The file as it is now (cached by size and mtime).
    pub fn file(&self) -> Option<LanesFile> {
        let meta = std::fs::metadata(&self.path).ok()?;
        let stamp = (meta.len(), meta.modified().ok());
        let mut g = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((st, f)) = g.as_ref() {
            if *st == stamp {
                return f.clone();
            }
        }
        let f = std::fs::read_to_string(&self.path).ok().and_then(|t| LanesFile::parse(&t).ok());
        *g = Some((stamp, f.clone()));
        f
    }
}

/// The rule for `server_fp` in a lanes file: its `[[seats]]` line, by the
/// full fingerprint, and that line's lane.
pub fn rule_in(f: &LanesFile, server_fp: &str) -> Option<LaneRule> {
    if !ci::is_fp(server_fp) {
        return None;
    }
    let seat = f.seats.iter().find(|s| k2_node_proto::crypto::ct_eq(s.server_fp.as_bytes(), server_fp.as_bytes()))?;
    let t = f.lanes.get(&seat.lane)?;
    Some(LaneRule { lane: seat.lane.clone(), secrets: t.secrets, isolation: t.isolation.clone() })
}

impl LaneSource for FileLanes {
    fn lane_for(&self, server: &LaneServer<'_>) -> Option<LaneRule> {
        rule_in(&self.file()?, server.controller_fp?)
    }
}

/// Does the lane's `isolation` accept the job's? The job's must be one the
/// lane names (V24: `plan.isolation ∈ lane.isolation`), where `exclusive`
/// (the whole node) also meets a lane that names only `job_uid`. A shared
/// uid never does.
pub fn lane_accepts(lane: &[Isolation], job: Isolation) -> bool {
    match job {
        Isolation::JobUid => lane.contains(&Isolation::JobUid),
        Isolation::Exclusive => lane.contains(&Isolation::Exclusive) || lane.contains(&Isolation::JobUid),
        Isolation::TenantUid | Isolation::SharedUid => false,
    }
}

/// V24, the lane half, for a server that isn't `trust own` (no plan yet).
/// `None`: its lane here lets it have secret env. The node's own facts:
/// does the server speak protocol 9, its lane (root's file), the job wall,
/// Yama `ptrace_scope`.
pub fn lane_refusal(speaks_ci: bool, lane: Option<&LaneRule>, job_wall: bool, ptrace_scope: Option<u8>) -> Option<(&'static str, &'static str)> {
    if !speaks_ci {
        return Some((codes::UNTRUSTED_SERVER, "secret env goes only to servers this node trusts as its own (trust own), or under a lane to a server speaking protocol 9"));
    }
    let Some(lane) = lane else {
        return Some((codes::UNTRUSTED_SERVER, "secret env goes only to servers this node trusts as its own (trust own), or on a lane that allows secrets"));
    };
    if !lane.secrets {
        return Some((codes::UNTRUSTED_SERVER, "this server's lane here doesn't allow secrets"));
    }
    if !job_wall {
        return Some((codes::NEEDS_WALL, "lane secrets need the job wall (sudo k2-node setup)"));
    }
    if !ptrace_scope.is_some_and(|p| p >= 1) {
        return Some((codes::NEEDS_WALL, "lane secrets need Yama ptrace_scope 1 or more (kernel.yama.ptrace_scope)"));
    }
    if !lane.isolation.iter().any(|i| matches!(i, Isolation::JobUid | Isolation::Exclusive)) {
        return Some((codes::NEEDS_ISOLATION, "this server's lane names no isolation good enough for secrets (job_uid or exclusive)"));
    }
    None
}

impl Node {
    /// The lane `t`'s seat runs under here (root's lanes file, by `t`'s
    /// pinned key), with a valid name: receipt v2's `run.lane`. The lane
    /// secret rules below never apply to a `trust own` server.
    pub fn lane_of(&self, t: &Tenant) -> Option<LaneRule> {
        let src = self.opts.lanes.as_ref()?;
        let prefix = t.share().map(|s| s.entry.fp_prefix.clone()).unwrap_or_default();
        let pin = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone());
        let r = src.lane_for(&LaneServer { slot: t.slot, fp_prefix: &prefix, controller_fp: pin.as_deref() })?;
        ci::is_name(&r.lane).then_some(r)
    }

    /// Yama `ptrace_scope` as this node sees it (tests set it).
    pub fn ptrace_scope_now(&self) -> Option<u8> {
        match self.opts.ptrace_scope {
            Some(p) => p,
            None => crate::statement::ptrace_scope(),
        }
    }

    /// Does `t` trust this node as its own (`servers.toml`)?
    fn trusts_own(t: &Tenant) -> bool {
        t.is_owner() || t.share().is_some_and(|s| s.entry.trust == Trust::Own)
    }

    /// V24, the lane half for a tenant that isn't `trust own` (no plan
    /// yet: the offer and the first assign check). `None`: its lane lets
    /// it have secret env here.
    pub(super) fn lane_secret_refusal(&self, t: &Tenant) -> Option<(&'static str, &'static str)> {
        let speaks = t.speaks_ci();
        let lane = if speaks { self.lane_of(t) } else { None };
        lane_refusal(speaks, lane.as_ref(), matches!(self.wall, Wall::JobUser { .. }), self.ptrace_scope_now())
    }

    /// Is `lane-secrets` offered to `t`? A protocol-9 server that isn't
    /// the owner, whose lane here lets it have secret env (whatever its
    /// trust: a server on a lane gets lane rules).
    pub fn offers_lane_secrets(&self, t: &Tenant) -> bool {
        !t.is_owner() && t.speaks_ci() && self.lane_secret_refusal(t).is_none() && self.secret_env_refusal(t).is_none()
    }

    /// V24 with the plan (assign, and again right before `secret_need`):
    /// a secret job of a server that isn't `trust own` runs only with its
    /// lane's isolation, `exclusive` (the plan) or its own uid (`uid`).
    pub fn secret_plan_refusal(&self, t: &Tenant, plan: &Plan, uid: UidShare) -> Option<(&'static str, String)> {
        if plan.secret_env.is_empty() || Self::trusts_own(t) {
            return None;
        }
        if let Some((c, d)) = self.lane_secret_refusal(t) {
            return Some((c, d.to_string()));
        }
        let lane = self.lane_of(t)?;
        let got = if plan.exclusive {
            Isolation::Exclusive
        } else if uid == UidShare::Own {
            Isolation::JobUid
        } else {
            return Some((codes::NEEDS_ISOLATION, format!("lane {} runs secret jobs only with their own uid (this node has no per-job uid pool now) or --exclusive", lane.lane)));
        };
        (!lane_accepts(&lane.isolation, got)).then(|| (codes::NEEDS_ISOLATION, format!("lane {} needs --exclusive for secret jobs", lane.lane)))
    }
}

/// The lanes file reader and the isolation rule (any OS).
#[cfg(test)]
mod file_tests {
    use super::*;
    use k2_node_proto::lanes::{LaneSeat, LaneTemplate, LANE_CI_STANDARD};

    #[test]
    fn root_file_names_a_lane_by_the_full_pin_only() {
        let d = crate::util::temp_dir("lanes-file");
        let path = d.join("k2-node-lanes.toml");
        let src = FileLanes::new(path.clone());
        let fp = "a".repeat(64);
        fn who(pin: Option<&str>) -> LaneServer<'_> {
            LaneServer { slot: 2, fp_prefix: "aaaaaaaaaaaaaaaa", controller_fp: pin }
        }
        assert_eq!(src.lane_for(&who(Some(&fp))), None, "no file: no lane");
        let mut f = LanesFile::default();
        f.lanes.insert(LANE_CI_STANDARD.into(), LaneTemplate::ci_standard());
        f.seats.push(LaneSeat { seat: "seat-1".into(), server_fp: fp.clone(), lane: LANE_CI_STANDARD.into(), label: "client.k2.dev".into() });
        std::fs::write(&path, f.render().unwrap()).unwrap();
        let pin = fp.as_str();
        let r = src.lane_for(&who(Some(pin))).expect("the seated server's lane");
        assert_eq!((r.lane.as_str(), r.secrets, r.isolation.clone()), (LANE_CI_STANDARD, true, vec![Isolation::JobUid, Isolation::Exclusive]));
        assert_eq!(src.lane_for(&who(None)), None, "no pin: the prefix alone never names a lane");
        assert_eq!(src.lane_for(&who(Some("aaaaaaaaaaaaaaaa"))), None, "a prefix is not a fingerprint");
        let other = "b".repeat(64);
        assert_eq!(src.lane_for(&who(Some(&other))), None, "another server");
        // Secrets off in the owner's lane: the line still names it (receipt
        // v2), with secrets off.
        f.lanes.get_mut(LANE_CI_STANDARD).unwrap().secrets = false;
        f.lanes.get_mut(LANE_CI_STANDARD).unwrap().isolation = vec![];
        std::fs::write(&path, format!("{}\n# changed\n", f.render().unwrap())).unwrap();
        assert!(!src.lane_for(&who(Some(pin))).unwrap().secrets, "a changed file is read again");
        // A damaged file is no lanes.
        std::fs::write(&path, "version = 2\n").unwrap();
        assert_eq!(src.lane_for(&who(Some(pin))), None);
    }

    #[test]
    fn isolation_is_one_the_lane_names() {
        use Isolation::*;
        assert!(lane_accepts(&[JobUid, Exclusive], JobUid));
        assert!(lane_accepts(&[JobUid, Exclusive], Exclusive));
        assert!(lane_accepts(&[JobUid], Exclusive), "the whole node meets a job_uid lane");
        assert!(!lane_accepts(&[Exclusive], JobUid), "an exclusive lane takes no per-job uid");
        assert!(!lane_accepts(&[JobUid, Exclusive], TenantUid));
        assert!(!lane_accepts(&[], Exclusive));
        let lane = LaneRule { lane: "l".into(), secrets: true, isolation: vec![JobUid] };
        assert_eq!(lane_refusal(true, Some(&lane), true, Some(1)), None);
        assert_eq!(lane_refusal(false, Some(&lane), true, Some(1)).map(|r| r.0), Some(codes::UNTRUSTED_SERVER), "protocol 8: never");
        assert_eq!(lane_refusal(true, None, true, Some(1)).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        assert_eq!(lane_refusal(true, Some(&lane), false, Some(1)).map(|r| r.0), Some(codes::NEEDS_WALL));
        assert_eq!(lane_refusal(true, Some(&lane), true, Some(0)).map(|r| r.0), Some(codes::NEEDS_WALL));
    }
}

/// V24: who gets secret env under a lane, and who is refused.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use k2_node_proto::ci::{Isolation, FEATURE_LANE_SECRETS};
    use k2_node_proto::frames::{Frame, JobState};
    use k2_node_proto::queue::UidShare;
    use k2_node_proto::secret::{codes, FEATURE_SECRET_ENV};
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};

    use super::{LaneRule, StaticLanes};
    use crate::node::tenant::ShareInfo;
    use crate::node::{Node, NodeOptions, Tenant};
    use crate::wall::Wall;

    fn lane(secrets: bool, isolation: Isolation) -> LaneRule {
        LaneRule { lane: "ci-standard".into(), secrets, isolation: vec![isolation] }
    }

    /// A node with the owner and three tenants: slot 1 `trust own` (no
    /// lane), slot 2 `trust team` on `lane2`, slot 3 `trust team` with no
    /// lane. Every tenant speaks `protocol`, with the tenant wall on.
    async fn node(lane2: Option<LaneRule>, job_uid: bool, ptrace: Option<u8>, protocol: u32) -> (Arc<Node>, Vec<Arc<Tenant>>) {
        let d = crate::util::temp_dir("lane-secrets");
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for (slot, trust) in [(1u8, Trust::Own), (2, Trust::Team), (3, Trust::Team)] {
            let mut e = f.servers[0].clone();
            e.slot = slot;
            e.label = format!("s{slot}");
            e.fp_prefix = format!("{:016x}", slot);
            e.trust = trust;
            e.set_limits(&ShareLimits::new_server(trust, false));
            f.servers.push(e);
        }
        let entries = f.servers.clone();
        std::fs::write(layout.config.join(k2_node_proto::share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
        let mut o = NodeOptions::new(layout.clone());
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.wall = Some(Wall::JobUser { user: "k2job".into() });
        o.exec_stdin = Some(true);
        o.exec_job_uid = Some(job_uid);
        o.ptrace_scope = Some(ptrace);
        o.lanes = lane2.map(|l| Arc::new(StaticLanes([(2u8, l)].into())) as Arc<dyn super::LaneSource>);
        o.cancel_grace = Duration::from_millis(100);
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
        let n = Node::open(o).await.unwrap();
        let mut ts = Vec::new();
        for slot in [1u8, 2, 3] {
            layout.tenant(slot).ensure_state().unwrap();
            let t = n.add_tenant(slot).await.unwrap();
            *t.share.lock().unwrap() = Some(ShareInfo { entry: entries[slot as usize].clone(), servers: 4, shared: true, owner_label: "o".into() });
            t.protocol.store(protocol, Ordering::SeqCst);
            t.tenant_wall.store(true, Ordering::SeqCst);
            ts.push(t);
        }
        (n, ts)
    }

    fn feats(n: &Node, t: &Tenant) -> (bool, bool) {
        let f = n.session_features(t);
        (f.iter().any(|x| x == FEATURE_SECRET_ENV), f.iter().any(|x| x == FEATURE_LANE_SECRETS))
    }

    fn secret_plan(t: &Tenant, exclusive: bool) -> k2_node_proto::frames::Plan {
        let mut p = crate::ledger::tests::assign("sj", 1).plan;
        p.node_id = t.node_id();
        p.secret_env = vec!["K2T_SECRET".into()];
        p.exclusive = exclusive;
        p
    }

    #[tokio::test]
    async fn a_team_server_on_a_lane_that_allows_secrets_gets_them_with_isolation() {
        let (n, ts) = node(Some(lane(true, Isolation::JobUid)), true, Some(1), 9).await;
        let (own, on_lane, no_lane) = (&ts[0], &ts[1], &ts[2]);
        // trust own: today's rule, no lane feature.
        assert_eq!(feats(&n, own), (true, false));
        // trust team on a lane with secrets: both features.
        assert_eq!(n.secret_env_refusal(on_lane), None);
        assert_eq!(feats(&n, on_lane), (true, true));
        // trust team, no lane: refused as today.
        assert_eq!(n.secret_env_refusal(no_lane).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        assert_eq!(feats(&n, no_lane), (false, false));
        // Isolation, with the plan: its own uid, or exclusive.
        let p = secret_plan(on_lane, false);
        assert_eq!(n.uid_share(on_lane, &p), UidShare::Own);
        assert_eq!(n.secret_plan_refusal(on_lane, &p, UidShare::Own), None);
        assert_eq!(n.secret_plan_refusal(on_lane, &p, UidShare::SlotAlone).map(|r| r.0), Some(codes::NEEDS_ISOLATION));
        assert_eq!(n.secret_plan_refusal(on_lane, &secret_plan(on_lane, true), UidShare::Slot), None, "exclusive is enough");
        // The trust-own server needs neither (today's rule).
        assert_eq!(n.secret_plan_refusal(own, &secret_plan(own, false), UidShare::Slot), None);
        // Receipt v2 names the lane for the lane's server only.
        assert_eq!(n.lane_of(on_lane).map(|l| l.lane), Some("ci-standard".into()));
        assert_eq!(n.lane_of(own), None);
        assert_eq!(n.lane_of(&n.owner()), None);
    }

    #[tokio::test]
    async fn refused_without_the_lane_rules() {
        // The lane doesn't allow secrets.
        let (n, ts) = node(Some(lane(false, Isolation::JobUid)), true, Some(1), 9).await;
        assert_eq!(n.secret_env_refusal(&ts[1]).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        assert_eq!(feats(&n, &ts[1]), (false, false));
        // ptrace_scope 0 (or unknown): no.
        let (n, ts) = node(Some(lane(true, Isolation::JobUid)), true, Some(0), 9).await;
        assert_eq!(n.secret_env_refusal(&ts[1]).map(|r| r.0), Some(codes::NEEDS_WALL));
        let (n, ts) = node(Some(lane(true, Isolation::JobUid)), true, None, 9).await;
        assert_eq!(n.secret_env_refusal(&ts[1]).map(|r| r.0), Some(codes::NEEDS_WALL));
        // An exclusive lane takes no per-job uid.
        let (n, ts) = node(Some(lane(true, Isolation::Exclusive)), true, Some(2), 9).await;
        let p = secret_plan(&ts[1], false);
        assert_eq!(n.secret_plan_refusal(&ts[1], &p, UidShare::Own).map(|r| r.0), Some(codes::NEEDS_ISOLATION));
        assert_eq!(n.secret_plan_refusal(&ts[1], &secret_plan(&ts[1], true), UidShare::Slot), None);
        // A lane naming a shared uid never carries secrets.
        let (n, ts) = node(Some(lane(true, Isolation::TenantUid)), true, Some(1), 9).await;
        assert_eq!(n.secret_env_refusal(&ts[1]).map(|r| r.0), Some(codes::NEEDS_ISOLATION));
        // No per-job uid pool and not exclusive: refused at assign, before
        // any ledger row.
        let (n, ts) = node(Some(lane(true, Isolation::JobUid)), false, Some(1), 9).await;
        let t = &ts[1];
        assert_eq!(n.secret_env_refusal(t), None, "the lane itself is fine");
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        t.hub.attach(tx);
        let mut a = crate::ledger::tests::assign("sj", 1);
        a.plan = secret_plan(t, false);
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(t, a), None);
        let Some(Frame::State(s)) = rx.recv().await else { panic!("no answer") };
        assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(codes::NEEDS_ISOLATION)));
        assert!(t.ledger.lock().unwrap().get("sj", 1).unwrap().is_none(), "no ledger row");
    }

    /// Mixed versions: a protocol-8 server on a lane that allows secrets
    /// is still refused (it can't know the lane rules or check them), and
    /// sees neither feature; its trust-own neighbour keeps today's rule.
    #[tokio::test]
    async fn a_protocol_8_server_never_gets_lane_secrets() {
        let (n, ts) = node(Some(lane(true, Isolation::JobUid)), true, Some(1), 8).await;
        assert_eq!(n.secret_env_refusal(&ts[1]).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        assert_eq!(feats(&n, &ts[1]), (false, false));
        assert_eq!(feats(&n, &ts[0]), (true, false), "a protocol-8 trust-own server: today's secret env");
        assert!(!n.session_features(&ts[0]).iter().any(|f| k2_node_proto::ci::FEATURES.contains(&f.as_str())), "no protocol-9 feature to a protocol-8 server");
    }
}

//! The offer one session's server sees (§10.1 of the nodes PRD; per
//! session since node sharing, prd-compute-shared-queue-v1 V53).
//!
//! Solo mode with a 0.46.2 server: exactly today's offer (the solo goldens
//! hold it). Per session:
//! - `slots`: only this server's project keys (its own cache root);
//! - `features`: the build's and the wall's; `servers` to a server that
//!   speaks protocol 3; never `admin`, `self-update` or `health` to a
//!   server that isn't the node's owner (V52);
//! - `labels`: this server's own pin labels (never the owner's) plus the
//!   machine facts;
//! - `control_by` (a person's name) only to the owner;
//! - `share`: this server's [`ShareView`] when it speaks protocol 3 and the
//!   node is set up for sharing (`servers.toml`), with `priority` (does its
//!   Priority count here) when it speaks protocol 5;
//! - `tiers` (protocol 5) in `features` to a server that speaks 5;
//! - protocol 7: the host labels (`distro`, `distro_version`, `pkg`,
//!   `libc`, `gpu`), `host` and `pkg` (this server's package view), and
//!   their features, only to a server that speaks 7;
//! - revision and dedupe per session; `share.expires_in_secs` and the
//!   quotes don't count as a change (they ride the 30 s offer).

use std::sync::atomic::Ordering;

use k2_node_proto::frames::{Availability, Caps, Free, Offer, PROTOCOL};
use k2_node_proto::refusal::{self, NodeView};
use k2_node_proto::share::{ShareView, FEATURE_QUEUE, FEATURE_SERVERS, FEATURE_SERVERS_OWNER, FEATURE_TENANT_WALL};
use k2_node_proto::tiers::FEATURE_TIERS;

use super::{Node, Tenant, NODE_VERSION};

/// Features only the owner server may see (V52).
const OWNER_ONLY_FEATURES: &[&str] = &[
    k2_node_proto::admin::FEATURE_ADMIN,
    k2_node_proto::admin::FEATURE_SELF_UPDATE,
    k2_node_proto::admin::FEATURE_HEALTH,
    k2_node_proto::admin::FEATURE_WALL_RESTART,
];

impl Node {
    /// `t`'s offer right now (its revision bumps only when content changed).
    pub fn offer(&self, t: &Tenant) -> Offer {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let disk_free = self.disk_free_bytes();
        let mut free_mem = cap_mem.saturating_sub(used_mem);
        if let Some(avail) = crate::sysinfo::mem_available() {
            free_mem = free_mem.min(avail);
        }
        let tools = self.tools.lock().unwrap().clone();
        let control = self.effective_control(&l);
        let view = NodeView {
            control,
            unavailable: &l.unavailable,
            foreign_lock: l.locks.holding.as_deref(),
            disk_free_bytes: disk_free,
            disk_floor_bytes: l.policy.disk_floor_bytes,
            workspaces_allow: &l.policy.workspaces,
            tools: &tools,
            running,
            parallel_max: par,
            exclusive_running: excl,
            free_cpu_millis: cap_cpu.saturating_sub(used_cpu),
            free_mem_bytes: free_mem,
            network_none: l.policy.network == "none",
            network_none_enforced: self.wall.network_none(),
            free_gpus: self.free_gpus_for(t),
        };
        let node_refusal = refusal::node_refusal(&view);
        let pin = t.pin.lock().unwrap().clone();
        let mut labels = pin.as_ref().map(|p| p.labels.clone()).unwrap_or_default();
        labels.insert("os".into(), crate::sysinfo::os_label().into());
        labels.insert("arch".into(), crate::sysinfo::arch_label().into());
        if let Some(p) = &pin {
            labels.insert("name".into(), p.name.clone());
        }
        if crate::sysinfo::is_vm() {
            labels.insert("vm".into(), "true".into());
        }
        let mut host = t.speaks_host().then(|| self.host.lock().unwrap().clone()).filter(|h| !h.distro.is_empty());
        if let Some(h) = host.as_mut() {
            // GPU jobs (0.46.11): which GPUs this server may use here.
            self.mark_gpu_jobs(t, &mut h.gpus);
            // Cache generations: only to a server that gets the feature.
            h.cache_fs = self.gens_for(t).map(|g| g.fs.clone());
            // Host facts win over any owner label of the same name.
            labels.extend(h.labels());
            if h.gpus.iter().any(|g| g.jobs) {
                labels.insert(k2_node_proto::host::GPU_JOBS_LABEL.into(), "yes".into());
            }
        }
        let ledger_id = t.ledger_id();
        let mut o = Offer {
            share: self.share_view(t),
            revision: 0,
            control,
            control_by: if t.is_owner() { l.control.by.clone() } else { None },
            control_at: l.control.at,
            availability: Availability { ok: l.unavailable.is_empty(), reasons: l.unavailable.clone() },
            caps: Caps { cpu_millis: cap_cpu, mem_bytes: cap_mem, disk_budget_bytes: l.policy.disk_budget_bytes, hard: self.caps_hard(), gpus: self.gpus_for(t) },
            free: Free { cpu_millis: cap_cpu.saturating_sub(used_cpu), mem_bytes: free_mem, disk_free_bytes: disk_free, gpus: self.free_gpus_for(t) },
            slots: self.slots.lock().unwrap().list(&t.paths.cache()),
            running,
            parallel_max: par,
            refusal: node_refusal,
            labels,
            tools,
            protocol: PROTOCOL,
            boot_id: self.opts.clock.boot_id(),
            ledger_id,
            node_version: NODE_VERSION.to_string(),
            features: self.session_features(t),
            host,
            pkg: if t.speaks_host() { self.pkg_status(t) } else { None },
            // Sim sessions (protocol 11): only to a server whose slot the
            // owner turned on, on a Mac (prd-ios-on-mac-node-0467-v1 §3.8).
            sim: self.sim_offer(t, free_mem),
        };
        let mut last = t.last_offer.lock().unwrap();
        let same = last.as_ref().is_some_and(|p| {
            let mut q = p.clone();
            q.revision = 0;
            // Free disk moves constantly; only a 1 GiB change is news.
            q.free.disk_free_bytes = o.free.disk_free_bytes;
            // The end-date countdown and quotes ride the periodic offer.
            if let (Some(qs), Some(os)) = (q.share.as_mut(), o.share.as_ref()) {
                qs.expires_in_secs = os.expires_in_secs;
                qs.skew_secs = os.skew_secs;
                qs.quotes = os.quotes.clone();
                // Moves all the time, like free disk: only 1 GiB is news.
                qs.disk_used_bytes = os.disk_used_bytes;
            }
            let share_disk_moved = match (p.share.as_ref().and_then(|s| s.disk_used_bytes), o.share.as_ref().and_then(|s| s.disk_used_bytes)) {
                (Some(a), Some(b)) => a.abs_diff(b) >= (1 << 30),
                (a, b) => a.is_some() != b.is_some(),
            };
            q == o && !share_disk_moved && p.free.disk_free_bytes.abs_diff(o.free.disk_free_bytes) < (1 << 30)
        });
        if same {
            o.revision = last.as_ref().map(|p| p.revision).unwrap_or(0);
            o.free.disk_free_bytes = last.as_ref().map(|p| p.free.disk_free_bytes).unwrap_or(0);
        } else {
            o.revision = t.offer_rev.fetch_add(1, Ordering::SeqCst) + 1;
            *last = Some(o.clone());
        }
        o
    }

    /// What `t`'s session is told this node understands (§10.1, V52).
    pub fn session_features(&self, t: &Tenant) -> Vec<String> {
        let mut f = self.features();
        // iPhones on this Mac for this server (protocol 11, §3.8).
        if self.sim_feature(t) {
            f.push(k2_node_proto::sim::FEATURE_SIM_IOS.to_string());
        }
        if !t.is_owner() {
            f.retain(|x| !OWNER_ONLY_FEATURES.contains(&x.as_str()));
        }
        // Protocol 3: this node takes more than one server and keeps the
        // node queue (`queue_get`, the withdraw/if_queued cancels, `Plan`'s
        // queue fields). Without `queue` a server never sends into the
        // node's queue and falls back to today's placement.
        for feat in [FEATURE_SERVERS, FEATURE_QUEUE] {
            if t.speaks_shared() && !f.iter().any(|x| x == feat) {
                f.push(feat.to_string());
            }
        }
        // `servers-owner`: the owner may move the node to another of its
        // own servers here (prd-macos-tenant-compute-v1 §6.4). Owner and
        // protocol 3 only.
        if t.is_owner() && t.speaks_shared() && !f.iter().any(|x| x == FEATURE_SERVERS_OWNER) {
            f.push(FEATURE_SERVERS_OWNER.to_string());
        }
        // `tenant-wall` only when the per-tenant wall is really on (lane
        // S3's probe), and only to protocol 3.
        if t.speaks_shared() && t.tenant_wall.load(Ordering::SeqCst) && !f.iter().any(|x| x == FEATURE_TENANT_WALL) {
            f.push(FEATURE_TENANT_WALL.to_string());
        }
        // Secret-env (SE16): only when every condition holds for this
        // session (protocol 4, Linux wall, trust own, tenant wall for a
        // tenant, an executor that relays stdin). Not owner-only.
        if self.secret_env_refusal(t).is_none() {
            f.push(k2_node_proto::secret::FEATURE_SECRET_ENV.to_string());
        }
        // Git access (protocol 8, GH#78): `Plan.git_access` and the git
        // frames, to a server that speaks 8; to one that isn't the owner
        // only while the node's policy allows it (`tenant_git_access`).
        if self.git_access_refusal(t).is_none() && !f.iter().any(|x| x == k2_node_proto::gitcred::FEATURE_GIT_ACCESS) {
            f.push(k2_node_proto::gitcred::FEATURE_GIT_ACCESS.to_string());
        }
        // Job tiers (protocol 5): `Plan.tier`, the tier ledger column, the
        // tier-first queue, `servers.limits priority=`. Only to a server
        // that speaks protocol 5; an older one keeps sending `class`.
        if t.speaks_tiers() && !f.iter().any(|x| x == FEATURE_TIERS) {
            f.push(FEATURE_TIERS.to_string());
        }
        // Job caches (protocol 6): `Plan.cache` and `cache_get`. Only to a
        // server that speaks protocol 6 (an older one would never send
        // either, and a 0.46.5 server's goldens stay as they were).
        if t.speaks_cache() && !f.iter().any(|x| x == k2_node_proto::cache::FEATURE_JOB_CACHE) {
            f.push(k2_node_proto::cache::FEATURE_JOB_CACHE.to_string());
        }
        // `--with` sibling repos (protocol 6, same release).
        if t.speaks_cache() && !f.iter().any(|x| x == k2_node_proto::frames::FEATURE_MULTI_REPO) {
            f.push(k2_node_proto::frames::FEATURE_MULTI_REPO.to_string());
        }
        // Job tiers v2 (protocol 6, same release): Batch, the submitter
        // fields, alternating lines, submitter turns. Only to a server that
        // speaks protocol 6 (a 0.46.5 parser rejects `batch`).
        if t.speaks_tiers2() && !f.iter().any(|x| x == k2_node_proto::tiers::FEATURE_TIERS2) {
            f.push(k2_node_proto::tiers::FEATURE_TIERS2.to_string());
        }
        // Self-serve environments (protocol 7): host facts, the name map,
        // profiles and complete Rust toolchains; the package status and
        // requests where this node can install packages.
        if t.speaks_host() {
            let mut add: Vec<&str> = super::HOST_FEATURES.to_vec();
            if self.pkg_backend().is_some() {
                add.extend([k2_node_proto::pkgpolicy::FEATURE_PKG_STATUS, k2_node_proto::pkgpolicy::FEATURE_PKG_REQUESTS]);
            }
            for feat in add {
                if !f.iter().any(|x| x == feat) {
                    f.push(feat.to_string());
                }
            }
        }
        // Protocol 9 (C0): the signed self-statement (`node_statement`)
        // and receipt v2, only to a server that speaks 9, and only while
        // the node holds a signed statement (receipt v2 names it).
        if t.speaks_ci() && self.statement_signed().is_some() {
            for feat in [k2_node_proto::ci::FEATURE_NODE_STATEMENT, k2_node_proto::ci::FEATURE_RECEIPT_V2] {
                if !f.iter().any(|x| x == feat) {
                    f.push(feat.to_string());
                }
            }
        }
        // C1/C3a: the node relays its owner's node roster, and takes the
        // lanes file verb (root's executor), protocol 9 only. A `--dev`
        // node has no executor, so no lanes.
        // C3b (V27): this server may delete a secret job's kept artifacts
        // once it fetched them (`artifact_delete`).
        if t.speaks_ci() {
            let mut add = vec![k2_node_proto::ci::FEATURE_NODE_ROSTER, k2_node_proto::ci::FEATURE_ARTIFACT_DELETE];
            if !self.opts.dev {
                add.push(k2_node_proto::ci::FEATURE_LANES);
            }
            for feat in add {
                if !f.iter().any(|x| x == feat) {
                    f.push(feat.to_string());
                }
            }
        }
        // C2: secret jobs run as a per-job uid here (protocol 9 only).
        if t.speaks_ci() && self.job_uid_ready() && !f.iter().any(|x| x == k2_node_proto::ci::FEATURE_JOB_UID) {
            f.push(k2_node_proto::ci::FEATURE_JOB_UID.to_string());
        }
        // C3b (V24): this server's secret env here comes under a lane's
        // rules (protocol 9 only, never the owner): the server then checks
        // its seat, the roster and its consent before sealing (V25).
        if self.offers_lane_secrets(t) && !f.iter().any(|x| x == k2_node_proto::ci::FEATURE_LANE_SECRETS) {
            f.push(k2_node_proto::ci::FEATURE_LANE_SECRETS.to_string());
        }
        // Cache generations (B1): a protocol-10 server, a node with the
        // owner's switch on, its wall, and a closed, probed gens root.
        if self.gens_for(t).is_some() && !f.iter().any(|x| x == k2_node_proto::gens::FEATURE_CACHE_GENS) {
            f.push(k2_node_proto::gens::FEATURE_CACHE_GENS.to_string());
        }
        // GPU jobs (0.46.11): only to a server the machine's owner named in
        // `gpu.toml` (the owner's always), behind the Linux job wall, whose
        // self-test passed; protocol 7 (it reads `host.gpus`).
        if self.gpus_for(t) > 0 && !f.iter().any(|x| x == k2_node_proto::gpu::FEATURE_GPU) {
            f.push(k2_node_proto::gpu::FEATURE_GPU.to_string());
        }
        // GPU jobs S6: `--gpu-mem` here only beside `gpu`, and only when
        // root reads a job's own GPU memory (an older executor can't, so
        // the budget would go unwatched).
        if self.offers_gpu_mem(t) && !f.iter().any(|x| x == k2_node_proto::gpu::FEATURE_GPU_MEM) {
            f.push(k2_node_proto::gpu::FEATURE_GPU_MEM.to_string());
        }
        // prd-compute-blacksmith-class-v1 R2 (protocol 10): `--cpus N+M`
        // (`Plan.cpus_burst`), while the owner's `cpu_burst` is on.
        if self.offers_cpu_burst(t) && !f.iter().any(|x| x == k2_node_proto::gens::FEATURE_CPU_BURST) {
            f.push(k2_node_proto::gens::FEATURE_CPU_BURST.to_string());
        }
        // Build statements (protocol 10, prd-compute-build-attestations-v1
        // S2): `Plan.provenance` and `ReceiptFrame.provenance`.
        if self.offers_provenance(t) && !f.iter().any(|x| x == k2_node_proto::provenance::FEATURE_PROVENANCE_V1) {
            f.push(k2_node_proto::provenance::FEATURE_PROVENANCE_V1.to_string());
        }
        // 0.46.14 (protocol 10): a job's own `--network none`, and dummy
        // interfaces in its own namespace (Linux job wall only).
        if t.speaks_gens()
            && self.offers_job_network()
            && !f
                .iter()
                .any(|x| x == k2_node_proto::jobnet::FEATURE_JOB_NETWORK)
        {
            f.push(k2_node_proto::jobnet::FEATURE_JOB_NETWORK.to_string());
        }
        if t.speaks_gens()
            && self.offers_netns_dummy()
            && !f
                .iter()
                .any(|x| x == k2_node_proto::jobnet::FEATURE_NETNS_DUMMY)
        {
            f.push(k2_node_proto::jobnet::FEATURE_NETNS_DUMMY.to_string());
        }
        // 0.46.18: user namespaces are walled in every job here, and the
        // owner's `--userns` jobs run without that wall.
        if t.speaks_gens() && self.offers_job_userns() && !f.iter().any(|x| x == k2_node_proto::userns::FEATURE_JOB_USERNS) {
            f.push(k2_node_proto::userns::FEATURE_JOB_USERNS.to_string());
        }
        // O2 (protocol 10): `Outputs.stats`, the job's 5 s series.
        if t.speaks_gens() && !f.iter().any(|x| x == k2_node_proto::gens::FEATURE_JOB_STATS) {
            f.push(k2_node_proto::gens::FEATURE_JOB_STATS.to_string());
        }
        // Compute easy mode (protocol 6, same release): only to a server
        // that speaks it, the only one the node does that work for.
        if t.speaks_easy() {
            for feat in super::EASY_FEATURES {
                if !f.iter().any(|x| x == feat) {
                    f.push(feat.to_string());
                }
            }
        }
        f
    }

    /// `Offer.share` for `t` (V53): only to a protocol-3 server, only on a
    /// node set up for sharing, and only `t`'s own share and counts.
    pub fn share_view(&self, t: &Tenant) -> Option<ShareView> {
        if !t.speaks_shared() {
            return None;
        }
        let info = t.share()?;
        let now = self.now();
        let state = t.status_at(now)?;
        // The owner gets its view whenever `servers.toml` exists (`share()`
        // is set only then), shared or not: a new owner on a node left with
        // no other server must still learn it's the owner (A2, MT34). A
        // solo node without the file sends none, as in 0.46.2.
        let running = self.running_by_tenant().get(&t.slot).map(|v| v.len() as u32).unwrap_or(0);
        // Its own statement: the ledger guard must be gone before `quotes`
        // (the queue's snapshot locks every ledger).
        let queued = t.ledger.lock().unwrap().queued().map(|q| q.len() as u32).unwrap_or(0);
        let quotes = self.fairq.quotes(self, t);
        Some(ShareView {
            role: t.role(),
            slot: t.slot,
            shared: info.shared,
            servers: info.servers,
            owner_label: (!t.is_owner()).then(|| info.owner_label.clone()),
            state,
            limits: info.entry.limits(),
            running,
            queued,
            // Today's CPU isn't exposed by the queue store yet (S2 follow-up).
            cpu_secs_today: 0,
            expires_in_secs: info.entry.expires_at.map(|e| (e - now).max(0)),
            skew_secs: t.link.lock().unwrap().skew_secs,
            // Start-now / ETA for this server's last four sizes (V53).
            quotes,
            // Does this server's Priority count here (`queue.toml`, T7)?
            // Protocol 5 only.
            priority: t.speaks_tiers().then(|| self.fairq.priority(t.slot)),
            // What the 60 s pass last measured (the owner has no share).
            disk_used_bytes: if t.is_owner() { None } else { self.tenant_disk_used(t.slot) },
        })
    }
}

/// SE16/SE17: who is offered `secret-env`, and who is refused at assign.
/// The node runs with the Linux job-user wall set (no executor needed for
/// the offer) and an executor that relays stdin.
#[cfg(all(test, target_os = "linux"))]
mod secret_env_tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use k2_node_proto::frames::{Frame, JobState};
    use k2_node_proto::secret::{codes, FEATURE_SECRET_ENV};
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};

    use crate::node::tenant::ShareInfo;
    use crate::node::{Node, NodeOptions};
    use crate::wall::Wall;

    fn has(n: &Node, t: &crate::node::Tenant) -> bool {
        n.session_features(t).iter().any(|f| f == FEATURE_SECRET_ENV)
    }

    #[tokio::test]
    async fn only_trusted_walled_protocol_4_sessions_get_secret_env() {
        let d = crate::util::temp_dir("secret-offer");
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for (slot, trust) in [(1u8, Trust::Own), (2, Trust::Team)] {
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
        o.cancel_grace = Duration::from_millis(100);
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
        let n = Node::open(o).await.unwrap();
        let owner = n.owner();
        // The owner: only with a protocol-4 session.
        owner.protocol.store(3, Ordering::SeqCst);
        assert!(!has(&n, &owner), "a protocol-3 server never sees it");
        owner.protocol.store(4, Ordering::SeqCst);
        assert!(has(&n, &owner));
        // An executor that can't relay stdin: no.
        n.exec_stdin.store(false, Ordering::SeqCst);
        assert!(!has(&n, &owner));
        assert_eq!(n.secret_env_refusal(&owner).map(|r| r.0), Some(codes::NEEDS_UPDATE));
        n.exec_stdin.store(true, Ordering::SeqCst);
        // Tenants: `own` with the tenant wall only; `team` never.
        for slot in [1u8, 2] {
            layout.tenant(slot).ensure_state().unwrap();
        }
        let t_own = n.add_tenant(1).await.unwrap();
        let t_team = n.add_tenant(2).await.unwrap();
        for (t, e) in [(&t_own, &entries[1]), (&t_team, &entries[2])] {
            *t.share.lock().unwrap() = Some(ShareInfo { entry: e.clone(), servers: 3, shared: true, owner_label: "o".into() });
            t.protocol.store(4, Ordering::SeqCst);
        }
        assert!(!has(&n, &t_own), "no tenant wall yet");
        assert_eq!(n.secret_env_refusal(&t_own).map(|r| r.0), Some(codes::NEEDS_WALL));
        t_own.tenant_wall.store(true, Ordering::SeqCst);
        t_team.tenant_wall.store(true, Ordering::SeqCst);
        assert!(has(&n, &t_own), "a trust-own server with the tenant wall");
        assert!(!has(&n, &t_team), "a team server never sees it");
        assert_eq!(n.secret_env_refusal(&t_team).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        // At assign: a team server's secret plan fails before any row.
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        t_team.hub.attach(tx);
        let mut a = crate::ledger::tests::assign("sj", 1);
        a.plan.node_id = t_team.node_id();
        a.plan.secret_env = vec!["K2T_SECRET".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&t_team, a), None);
        let Some(Frame::State(s)) = rx.recv().await else { panic!("no answer") };
        assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(codes::UNTRUSTED_SERVER)));
        assert!(t_team.ledger.lock().unwrap().get("sj", 1).unwrap().is_none(), "no ledger row");
        // The offer itself (owner) lists the feature beside the others.
        let off = n.offer(&owner);
        assert!(off.features.iter().any(|f| f == FEATURE_SECRET_ENV), "{:?}", off.features);
    }

    /// R2 (prd-compute-blacksmith-class-v1 §4.4.4): `cpu-burst` only to a
    /// protocol-10 server, and only while the owner's `cpu_burst` is on.
    #[tokio::test]
    async fn cpu_burst_is_offered_to_protocol_10_while_the_owner_allows_it() {
        let d = crate::util::temp_dir("offer-burst");
        let mut o = NodeOptions::new(crate::paths::Layout::new(d.join("home"), d.join("cfg")));
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        let n = Node::open(o).await.unwrap();
        let owner = n.owner();
        let has = |n: &Node, t: &crate::node::tenant::Tenant| n.session_features(t).iter().any(|f| f == k2_node_proto::gens::FEATURE_CPU_BURST);
        let stats = |n: &Node, t: &crate::node::tenant::Tenant| n.session_features(t).iter().any(|f| f == k2_node_proto::gens::FEATURE_JOB_STATS);
        owner.protocol.store(k2_node_proto::frames::PROTOCOL_CI, Ordering::SeqCst);
        assert!(!has(&n, &owner), "a protocol-9 server never sees it");
        assert!(!stats(&n, &owner), "nor job-stats (O2)");
        owner.protocol.store(k2_node_proto::frames::PROTOCOL_GENS, Ordering::SeqCst);
        assert!(has(&n, &owner) && stats(&n, &owner));
        n.local.lock().unwrap().policy.cpu_burst = false;
        assert!(!has(&n, &owner), "the owner turned idle cores off");
        assert!(stats(&n, &owner), "job stats don't depend on it");
    }
}

/// 0.46.14 (`--network none`, `--netns-dummy`): `job-network` and
/// `netns-dummy` go only to a protocol-10 server, only from a node whose
/// wall runs the job at that network (Linux: the job wall with an executor
/// that says `netns`); anywhere else a plan that asks is refused at assign
/// (no ledger row), never run with network.
#[cfg(all(test, target_os = "linux"))]
mod job_network_tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use k2_node_proto::frames::{Frame, JobState, PROTOCOL_CI, PROTOCOL_GENS};
    use k2_node_proto::jobnet::{
        codes, Dummy, JobNet, JobNetMode, FEATURE_JOB_NETWORK, FEATURE_NETNS_DUMMY,
    };

    use crate::node::{Node, NodeOptions, Tenant};
    use crate::wall::Wall;

    async fn node(tag: &str, wall: Option<Wall>, exec_netns: bool) -> std::sync::Arc<Node> {
        let d = crate::util::temp_dir(tag);
        let mut o = NodeOptions::new(crate::paths::Layout::new(d.join("home"), d.join("cfg")));
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        match wall {
            Some(w) => {
                o.wall = Some(w);
                o.exec_stdin = Some(true);
                o.exec_netns = Some(exec_netns);
                // Hermetic: never the machine's own root executor.
                o.admin_socket = d.join("no-executor.sock");
            }
            None => o.dev = true,
        }
        o.cancel_grace = Duration::from_millis(100);
        o.facts = Some(crate::config::MachineFacts {
            cores: 8,
            mem_bytes: 32 << 30,
            disk_total_bytes: 1 << 40,
            has_battery: false,
            is_macos: false,
        });
        Node::open(o).await.unwrap()
    }

    fn feats(n: &Node, t: &Tenant) -> (bool, bool) {
        let f = n.session_features(t);
        (
            f.iter().any(|x| x == FEATURE_JOB_NETWORK),
            f.iter().any(|x| x == FEATURE_NETNS_DUMMY),
        )
    }

    fn sew() -> JobNet {
        JobNet {
            mode: JobNetMode::None,
            dummies: vec![Dummy {
                name: "sewlane0".into(),
                addr: "10.254.254.1/32".into(),
            }],
        }
    }

    /// The node's answer to an assign of a plan with `net`, and whether it
    /// kept a ledger row.
    async fn assign_answer(
        n: &std::sync::Arc<Node>,
        t: &std::sync::Arc<Tenant>,
        job: &str,
        net: JobNet,
    ) -> (JobState, Option<String>, bool) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        t.hub.attach(tx);
        let mut a = crate::ledger::tests::assign(job, 1);
        a.plan.node_id = t.node_id();
        a.plan.network = Some(net);
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(t, a), None, "never taken");
        let s = loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("no answer")
                .expect("hub closed")
            {
                Frame::State(s) if s.job_id == job => break s,
                _ => continue,
            }
        };
        let row = t.ledger.lock().unwrap().get(job, 1).unwrap().is_some();
        (s.state, s.reason, row)
    }

    #[tokio::test]
    async fn offered_to_protocol_10_behind_a_wall_that_runs_it() {
        let n = node(
            "jobnet-offer",
            Some(Wall::JobUser {
                user: "k2job".into(),
            }),
            true,
        )
        .await;
        let owner = n.owner();
        owner.protocol.store(PROTOCOL_CI, Ordering::SeqCst);
        assert_eq!(
            feats(&n, &owner),
            (false, false),
            "a protocol-9 server never sees them"
        );
        owner.protocol.store(PROTOCOL_GENS, Ordering::SeqCst);
        assert_eq!(
            feats(&n, &owner),
            (true, true),
            "Linux job wall + an executor that says netns"
        );
        assert_eq!(
            n.job_network_refusal(&owner, &crate::ledger::tests::assign("x", 1).plan),
            None,
            "no ask, nothing to refuse"
        );
        let mut p = crate::ledger::tests::assign("x", 1).plan;
        p.network = Some(JobNet::none());
        assert_eq!(n.job_network_refusal(&owner, &p), None);
        p.network = Some(sew());
        assert_eq!(n.job_network_refusal(&owner, &p), None);
        // A plan whose dummy breaks the rules (a public address) is refused
        // even here, by the node itself.
        p.network = Some(JobNet {
            mode: JobNetMode::None,
            dummies: vec![Dummy {
                name: "x".into(),
                addr: "8.8.8.8/32".into(),
            }],
        });
        assert_eq!(
            n.job_network_refusal(&owner, &p).map(|r| r.0),
            Some(codes::JOB_NETWORK_INVALID)
        );
        // The offer itself carries them.
        let off = n.offer(&owner);
        assert!(
            off.features.iter().any(|f| f == FEATURE_JOB_NETWORK)
                && off.features.iter().any(|f| f == FEATURE_NETNS_DUMMY),
            "{:?}",
            off.features
        );
    }

    /// 0.46.18 `--userns`: offered only with the Linux job wall and an
    /// executor that says `userns`; refused for a tenant whatever the node
    /// offers (the node owner's own server's alone); no ask, no refusal.
    #[tokio::test]
    async fn userns_is_the_owners_alone_and_needs_a_new_executor() {
        use k2_node_proto::userns::{codes as ucodes, FEATURE_JOB_USERNS};
        let n = node("userns-offer", Some(Wall::JobUser { user: "k2job".into() }), true).await;
        let owner = n.owner();
        owner.protocol.store(PROTOCOL_GENS, Ordering::SeqCst);
        let offered = |n: &Node, t: &Tenant| n.session_features(t).iter().any(|x| x == FEATURE_JOB_USERNS);
        let mut p = crate::ledger::tests::assign("u", 1).plan;
        assert_eq!(n.job_userns_refusal(&owner, &p), None, "no ask, nothing to refuse");
        p.userns = true;
        // An older executor (no `userns` in Ready).
        assert!(!offered(&n, &owner));
        assert_eq!(n.job_userns_refusal(&owner, &p).map(|r| r.0), Some(ucodes::USERNS_NEEDS_UPDATE));
        n.exec_userns.store(true, Ordering::SeqCst);
        let tenant = n.add_tenant(2).await.unwrap();
        tenant.protocol.store(PROTOCOL_GENS, Ordering::SeqCst);
        let r = n.job_userns_refusal(&tenant, &p).unwrap();
        assert_eq!(r.0, ucodes::USERNS_OWNER_ONLY, "{}", r.1);
        assert!(r.1.contains("tenant"), "a named reason: {}", r.1);
        if cfg!(target_os = "linux") {
            assert!(offered(&n, &owner) && offered(&n, &tenant), "every server learns the node walls user namespaces");
            assert_eq!(n.job_userns_refusal(&owner, &p), None);
        } else {
            assert!(!offered(&n, &owner), "a Mac has no user namespaces to wall");
            assert_eq!(n.job_userns_refusal(&owner, &p).map(|r| r.0), Some(ucodes::USERNS_NEEDS_UPDATE));
        }
    }

    /// An older root executor (no `netns` in Ready): neither is offered, and
    /// a plan that asks fails at assign with no ledger row.
    #[tokio::test]
    async fn an_older_executor_offers_neither_and_refuses_the_plan() {
        let n = node(
            "jobnet-oldexec",
            Some(Wall::JobUser {
                user: "k2job".into(),
            }),
            false,
        )
        .await;
        let owner = n.owner();
        owner.protocol.store(PROTOCOL_GENS, Ordering::SeqCst);
        assert_eq!(feats(&n, &owner), (false, false));
        let (state, reason, row) = assign_answer(&n, &owner, "oldexec", JobNet::none()).await;
        assert_eq!(
            (state, reason.as_deref(), row),
            (
                JobState::Failed,
                Some(codes::JOB_NETWORK_UNAVAILABLE),
                false
            )
        );
        let (state, reason, row) = assign_answer(&n, &owner, "oldexec2", sew()).await;
        assert_eq!(
            (state, reason.as_deref(), row),
            (
                JobState::Failed,
                Some(codes::JOB_NETWORK_UNAVAILABLE),
                false
            )
        );
        // The executor updated (a later Ready says netns): offered again.
        n.exec_netns.store(true, Ordering::SeqCst);
        assert_eq!(feats(&n, &owner), (true, true));
    }

    /// A node without a job wall (`--dev`): it can't keep a job off the
    /// network, so it never offers either and refuses every plan that asks.
    #[tokio::test]
    async fn a_node_without_a_wall_never_runs_a_network_none_job() {
        let n = node("jobnet-nowall", None, false).await;
        let owner = n.owner();
        owner.protocol.store(PROTOCOL_GENS, Ordering::SeqCst);
        assert_eq!(feats(&n, &owner), (false, false));
        let (state, reason, row) = assign_answer(&n, &owner, "nowall", JobNet::none()).await;
        assert_eq!(
            (state, reason.as_deref(), row),
            (
                JobState::Failed,
                Some(codes::JOB_NETWORK_UNAVAILABLE),
                false
            )
        );
    }

    /// A protocol-9 session can't ask: refused (it never saw the feature).
    #[tokio::test]
    async fn a_protocol_9_session_cant_ask() {
        let n = node(
            "jobnet-p9",
            Some(Wall::JobUser {
                user: "k2job".into(),
            }),
            true,
        )
        .await;
        let owner = n.owner();
        owner.protocol.store(PROTOCOL_CI, Ordering::SeqCst);
        let (state, reason, row) = assign_answer(&n, &owner, "p9", JobNet::none()).await;
        assert_eq!(
            (state, reason.as_deref(), row),
            (
                JobState::Failed,
                Some(codes::JOB_NETWORK_UNAVAILABLE),
                false
            )
        );
    }
}

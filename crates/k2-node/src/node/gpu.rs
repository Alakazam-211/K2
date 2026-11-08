//! GPU jobs, node side (0.46.11, prd-compute-gpu-jobs-v1 S1).
//!
//! The node learns which GPUs jobs may use, and by whom, from the root
//! executor's `Ready.gpus` (asked at start and on every wall probe, so an
//! owner's `sudo k2-node admin gpu on|off` reaches the offers within a
//! minute). It never reads `gpu.toml` itself and never names a device:
//! root picks and leases one at each start.
//!
//! Per session:
//! - the `gpu` feature, `Caps.gpus`, `Free.gpus`, `host.gpus[].jobs` and
//!   the `gpu_jobs=yes` label only for a server the owner named (the
//!   owner's server always, once a device is on), behind the Linux job
//!   wall, to a server that reads host facts (protocol 7);
//! - a GPU job holds one GPU while it runs (`RunningJob.gpus`, the queue's
//!   `gpus`): another waits `gpu_busy`, never shares it;
//! - a GPU plan for a server that may no longer run GPU jobs here fails
//!   `gpu_unavailable` before any ledger row.

use k2_node_proto::frames::Plan;

use super::{Node, Tenant};
use crate::admin::gpu::ReadyGpu;
use crate::wall::Wall;

impl Node {
    /// GPUs can be given to jobs here at all: Linux, the job-user wall.
    fn gpu_capable(&self) -> bool {
        cfg!(target_os = "linux") && matches!(self.wall, Wall::JobUser { .. })
    }

    /// `t`'s server's key prefix (a tenant's `servers.toml` line).
    fn server_fp16(t: &Tenant) -> Option<String> {
        t.share.lock().unwrap().as_ref().and_then(|s| crate::admin::gpu::fp16(&s.entry.fp_prefix))
    }

    /// The GPUs `t`'s server may use here.
    pub(crate) fn ready_gpus_for(&self, t: &Tenant) -> Vec<ReadyGpu> {
        if !self.gpu_capable() || !t.speaks_host() {
            return Vec::new();
        }
        let fp = Self::server_fp16(t);
        self.exec_gpus.lock().unwrap().iter().filter(|g| g.allows(t.slot, fp.as_deref())).cloned().collect()
    }

    /// How many GPUs `t`'s server may use here (`Caps.gpus`).
    pub(crate) fn gpus_for(&self, t: &Tenant) -> u32 {
        self.ready_gpus_for(t).len() as u32
    }

    /// Every GPU jobs may use here, any server (the queue's `Caps.gpus`).
    pub(crate) fn gpus_on(&self) -> u32 {
        if !self.gpu_capable() {
            return 0;
        }
        self.exec_gpus.lock().unwrap().len() as u32
    }

    /// GPUs running jobs hold (all servers).
    pub(crate) fn used_gpus(&self) -> u32 {
        self.jobs.lock().unwrap().values().map(|j| j.gpus).sum()
    }

    /// Of `t`'s GPUs, how many no job holds (`Free.gpus`). Counted
    /// node-wide: root's lease still decides the device at each start.
    pub(crate) fn free_gpus_for(&self, t: &Tenant) -> u32 {
        self.gpus_for(t).min(self.gpus_on().saturating_sub(self.used_gpus()))
    }

    /// Set `jobs` on each GPU fact `t`'s server may use here.
    pub(crate) fn mark_gpu_jobs(&self, t: &Tenant, facts: &mut [k2_node_proto::gpu::GpuFact]) {
        let mine = self.ready_gpus_for(t);
        for f in facts.iter_mut() {
            f.jobs = mine.iter().any(|g| g.pci == f.pci);
        }
    }

    /// Why `plan`'s GPU ask can't run here for `t` now, or `None` (no ask,
    /// or a GPU of that vendor is open to it).
    pub(crate) fn gpu_plan_refusal(&self, t: &Tenant, plan: &Plan) -> Option<String> {
        let ask = plan.gpu?;
        if let Err(e) = ask.check() {
            return Some(e);
        }
        if self.ready_gpus_for(t).iter().any(|g| ask.vendor.matches(&g.vendor)) {
            return None;
        }
        Some(format!(
            "this node runs no{} GPU jobs for this server now (its owner turns them on at the machine: sudo k2-node admin gpu on)",
            if ask.vendor == k2_node_proto::gpu::GpuVendor::Any { String::new() } else { format!(" {}", ask.vendor.as_str()) }
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use k2_node_proto::frames::{Frame, JobState};
    use k2_node_proto::gpu::{codes, GpuAsk, GpuFact, GpuVendor, FEATURE_GPU};
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};

    use crate::admin::gpu::ReadyGpu;
    use crate::node::tenant::ShareInfo;
    use crate::node::{Node, NodeOptions, RunningJob, Tenant};
    use crate::wall::Wall;

    const PCI: &str = "0000:c4:00.0";

    /// A walled node with slots 1 (named in gpu.toml) and 2 (not), its
    /// executor saying `gpus` (empty = the owner never turned GPU jobs on).
    async fn rig(name: &str, gpus: Vec<ReadyGpu>) -> (Arc<Node>, Arc<Tenant>, Arc<Tenant>, Arc<Tenant>) {
        let d = crate::util::temp_dir(name);
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        // Hermetic: no host lock file, window or disk floor decides here,
        // and a walled Linux node counts the host's own live k2job units as
        // lingering (a compute node running these tests has some): room
        // for them, so only the GPU count decides.
        std::fs::write(layout.policy(), "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = 64\n").unwrap();
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for slot in [1u8, 2] {
            let mut e = f.servers[0].clone();
            e.slot = slot;
            e.label = format!("s{slot}");
            e.fp_prefix = format!("{:016x}", slot);
            e.trust = Trust::Team;
            e.set_limits(&ShareLimits::new_server(Trust::Team, false));
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
        o.exec_gpus = Some(gpus);
        // Hermetic: never the machine's own root executor (a compute node
        // running these tests has one, with live units of its own).
        o.admin_socket = d.join("no-executor.sock");
        o.disk_free = Some(Arc::new(|| 1u64 << 40));
        o.cancel_grace = Duration::from_millis(100);
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
        let n = Node::open(o).await.unwrap();
        let owner = n.owner();
        owner.protocol.store(9, Ordering::SeqCst);
        for slot in [1u8, 2] {
            layout.tenant(slot).ensure_state().unwrap();
        }
        let t1 = n.add_tenant(1).await.unwrap();
        let t2 = n.add_tenant(2).await.unwrap();
        for (t, e) in [(&t1, &entries[1]), (&t2, &entries[2])] {
            *t.share.lock().unwrap() = Some(ShareInfo { entry: e.clone(), servers: 3, shared: true, owner_label: "o".into() });
            t.protocol.store(9, Ordering::SeqCst);
        }
        *n.host.lock().unwrap() = k2_node_proto::host::HostFacts {
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: "arch".into(),
            distro_version: "rolling".into(),
            gpu: Some("amd".into()),
            gpus: vec![GpuFact { vendor: "amd".into(), pci: PCI.into(), render: "renderD128".into(), ..Default::default() }],
            ..Default::default()
        };
        (n, owner, t1, t2)
    }

    fn ready(servers: &[&str]) -> Vec<ReadyGpu> {
        vec![ReadyGpu { pci: PCI.into(), vendor: "amd".into(), render: "renderD128".into(), servers: servers.iter().map(|s| s.to_string()).collect() }]
    }

    fn has(n: &Node, t: &Tenant) -> bool {
        n.session_features(t).iter().any(|f| f == FEATURE_GPU)
    }

    fn gpu_assign(n: &Node, t: &Tenant, job: &str) -> k2_node_proto::frames::Assign {
        let _ = n;
        let mut a = crate::ledger::tests::assign(job, 1);
        a.plan.node_id = t.node_id();
        a.plan.gpu = Some(GpuAsk::one(GpuVendor::Any));
        a.fence.plan_digest = a.plan.digest().unwrap();
        a
    }

    #[tokio::test]
    async fn session_features_offer_gpu_only_to_named_servers() {
        let (n, owner, t1, t2) = rig("gpu-offer", ready(&["owner", "0000000000000001"])).await;
        assert!(has(&n, &owner), "the owner's server always, once a device is on");
        assert!(has(&n, &t1), "a server the owner named");
        assert!(!has(&n, &t2), "a server the owner didn't name");
        let o1 = n.offer(&t1);
        assert_eq!((o1.caps.gpus, o1.free.gpus), (1, 1));
        let h1 = o1.host.as_ref().unwrap();
        assert!(h1.gpus[0].jobs);
        assert_eq!(o1.labels.get("gpu_jobs").map(String::as_str), Some("yes"));
        let o2 = n.offer(&t2);
        assert_eq!((o2.caps.gpus, o2.free.gpus), (0, 0));
        assert!(!o2.host.as_ref().unwrap().gpus[0].jobs, "the GPU is listed, not open to it");
        assert!(!o2.labels.contains_key("gpu_jobs"));
        // A server below protocol 7 never sees it.
        t1.protocol.store(6, Ordering::SeqCst);
        assert!(!has(&n, &t1));
    }

    /// An owner who never ran `gpu on`: no feature, no GPU counts, no
    /// `gpu_jobs` label, and the offer's caps and free have no GPU keys.
    #[tokio::test]
    async fn an_owner_who_never_opts_in_sees_no_change() {
        let (n, owner, t1, _) = rig("gpu-off", Vec::new()).await;
        for t in [&owner, &t1] {
            assert!(!has(&n, t));
            let o = n.offer(t);
            assert_eq!((o.caps.gpus, o.free.gpus), (0, 0));
            assert!(!o.labels.contains_key("gpu_jobs"));
            let caps = serde_json::to_string(&o.caps).unwrap();
            let free = serde_json::to_string(&o.free).unwrap();
            assert!(!caps.contains("gpus") && !free.contains("gpus"), "{caps} {free}");
            assert!(!o.host.as_ref().unwrap().gpus[0].jobs);
        }
        assert_eq!(n.queue_caps().gpus, 0);
    }

    #[tokio::test]
    async fn used_counts_gpus_and_refuses_gpu_busy() {
        let (n, owner, _, _) = rig("gpu-busy", ready(&["owner"])).await;
        let a = gpu_assign(&n, &owner, "g1");
        assert_eq!(n.assign_refusal(&owner, &a), None, "a free GPU: it may start");
        assert_eq!(n.queue_caps().gpus, 1);
        n.jobs.lock().unwrap().insert(
            (0, "other".into()),
            RunningJob {
                generation: 1,
                cpu_millis: Some(100),
                mem_bytes: Some(1 << 20),
                exclusive: false,
                stop: tokio::sync::watch::channel(None).0,
                seq: 0,
                gate: false,
                workspace: "w".into(),
                secret: false,
                uid: Default::default(),
                port_block: None,
                adoptable: false,
                rust: Vec::new(),
                gpus: 1,
            },
        );
        assert_eq!(n.used_gpus(), 1);
        assert_eq!(n.free_gpus_for(&owner), 0);
        let diag = || {
            let l = n.local.lock().unwrap().clone();
            let keys: Vec<_> = n.jobs.lock().unwrap().keys().cloned().collect();
            format!("caps {:?}, policy max_parallel {}, jobs {keys:?}", n.caps(&owner, &l), l.policy.max_parallel)
        };
        assert_eq!(n.assign_refusal(&owner, &a).map(|r| r.code).as_deref(), Some(codes::GPU_BUSY), "{}", diag());
        let mut plain = crate::ledger::tests::assign("p1", 1);
        plain.plan.node_id = owner.node_id();
        plain.fence.plan_digest = plain.plan.digest().unwrap();
        assert_eq!(n.assign_refusal(&owner, &plain), None, "a job without --gpu starts beside it");
        assert_eq!(n.offer(&owner).free.gpus, 0);
    }

    #[tokio::test]
    async fn assign_refuses_gpu_unavailable_after_off() {
        let (n, owner, _, t2) = rig("gpu-after-off", ready(&["owner"])).await;
        // The owner ran `gpu off`; the next probe told the node.
        n.exec_gpus.lock().unwrap().clear();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        owner.hub.attach(tx);
        assert_eq!(n.on_assign(&owner, gpu_assign(&n, &owner, "g1")), None);
        let Some(Frame::State(s)) = rx.recv().await else { panic!("no answer") };
        assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(codes::GPU_UNAVAILABLE)));
        assert!(owner.ledger.lock().unwrap().get("g1", 1).unwrap().is_none(), "no ledger row");
        // A server never named fails the same way, even while it's on.
        *n.exec_gpus.lock().unwrap() = ready(&["owner"]);
        let (tx2, mut rx2) = tokio::sync::mpsc::channel(16);
        t2.hub.attach(tx2);
        assert_eq!(n.on_assign(&t2, gpu_assign(&n, &t2, "g2")), None);
        let Some(Frame::State(s)) = rx2.recv().await else { panic!("no answer") };
        assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(codes::GPU_UNAVAILABLE)));
    }
}

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

    /// GPU jobs S6: root reads a job's own GPU memory here (`gpu_usage`).
    pub(crate) fn gpu_usage_ready(&self) -> bool {
        self.gpu_capable() && self.exec_gpu_usage.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// S6: `t`'s server may send `--gpu-mem` here (`gpu-mem` beside `gpu`).
    pub(crate) fn offers_gpu_mem(&self, t: &Tenant) -> bool {
        self.gpus_for(t) > 0 && self.gpu_usage_ready()
    }

    /// S6: would `ask` get a unified GPU (an APU or iGPU) here? Any GPU of
    /// that vendor open to jobs here that the facts call unified (root
    /// picks the device; this never undercounts).
    fn gpu_unified(&self, ask: &k2_node_proto::gpu::GpuAsk) -> bool {
        let open: Vec<String> = self.exec_gpus.lock().unwrap().iter().map(|g| g.pci.clone()).collect();
        self.host.lock().unwrap().gpus.iter().any(|g| g.unified && ask.vendor.matches(&g.vendor) && open.contains(&g.pci))
    }

    /// S6: the system RAM `plan`'s GPU memory takes here beside its
    /// `--mem` (its `--gpu-mem`, default 2G, on a unified GPU; else 0).
    pub(crate) fn gpu_mem_counted(&self, plan: &Plan) -> u64 {
        plan.gpu.as_ref().map_or(0, |a| k2_node_proto::gpu::counted_bytes(Some(a), plan.gpu_mem, self.gpu_unified(a)))
    }

    /// S6: `plan`'s memory as this node counts it (its `--mem` plus
    /// [`Node::gpu_mem_counted`]): what it holds while it runs, and what
    /// must fit before it starts.
    pub(crate) fn plan_mem(&self, plan: &Plan) -> Option<u64> {
        k2_node_proto::gpu::with_gpu_mem(plan.limits.mem_bytes, self.gpu_mem_counted(plan))
    }

    /// S6: the GPU memory budget the node watches `plan`'s job against
    /// (`None`: none; a discrete GPU without `--gpu-mem`).
    pub(crate) fn gpu_mem_cap(&self, plan: &Plan) -> Option<u64> {
        plan.gpu.as_ref().and_then(|a| k2_node_proto::gpu::cap_bytes(Some(a), plan.gpu_mem, self.gpu_unified(a)))
    }

    /// S6: one reading of what `unit`'s own processes hold on the GPU
    /// (root's `gpu_usage`), or `None` (no such op here, or it failed).
    pub(crate) async fn gpu_mem_sample(&self, unit: &str) -> Option<k2_node_proto::gpu::GpuMemUsage> {
        if !self.gpu_usage_ready() {
            return None;
        }
        match self.job_op(crate::admin::jobs::JobOp::GpuUsage { unit: unit.to_string() }, vec![], std::time::Duration::from_secs(10)).await {
            Ok(crate::admin::jobs::JobAnswer::GpuUsage(u)) => Some(u),
            Ok(other) => {
                crate::nlog!("gpu_usage {unit}: unexpected answer {other:?}");
                None
            }
            Err(e) => {
                crate::nlog!("gpu_usage {unit}: {e}");
                None
            }
        }
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
        let Some(ask) = plan.gpu else {
            return plan.gpu_mem.map(|_| "a GPU memory budget (--gpu-mem) without --gpu".to_string());
        };
        if let Err(e) = ask.check() {
            return Some(e);
        }
        // S6: a budget this node can't watch is never taken (the server
        // sends one only where `gpu-mem` is offered; the executor may
        // have been rolled back since).
        if let Some(b) = plan.gpu_mem {
            if let Err(e) = k2_node_proto::gpu::check_gpu_mem(b) {
                return Some(e);
            }
            if !self.gpu_usage_ready() {
                return Some("this node can't watch a job's GPU memory (--gpu-mem): its root executor is older than k2-node 0.46.11 (update k2-node there)".into());
            }
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

    /// GPU jobs S6: `gpu-mem` only beside `gpu`, and only when root reads
    /// a job's own GPU memory (`Ready.gpu_usage`); a budget an executor
    /// can't watch is refused at assign, never run unwatched.
    #[tokio::test]
    async fn gpu_mem_is_offered_only_where_root_reads_gpu_memory() {
        use k2_node_proto::gpu::FEATURE_GPU_MEM;
        let (n, owner, t1, t2) = rig("gpu-mem-offer", ready(&["owner", "0000000000000001"])).await;
        let mem = |t: &Tenant| n.session_features(t).iter().any(|f| f == FEATURE_GPU_MEM);
        assert!(!mem(&owner) && !mem(&t1), "an older executor (no gpu_usage): no gpu-mem");
        let mut a = gpu_assign(&n, &owner, "m1");
        a.plan.gpu_mem = Some(4 << 30);
        let why = n.gpu_plan_refusal(&owner, &a.plan).expect("refused");
        assert!(why.contains("--gpu-mem") && why.contains("0.46.11"), "{why}");
        n.exec_gpu_usage.store(true, Ordering::SeqCst);
        assert!(mem(&owner) && mem(&t1), "with gpu");
        assert!(!mem(&t2), "never without gpu");
        assert_eq!(n.gpu_plan_refusal(&owner, &a.plan), None);
        let mut plain = crate::ledger::tests::assign("p", 1);
        plain.plan.gpu_mem = Some(4 << 30);
        assert!(n.gpu_plan_refusal(&owner, &plain.plan).is_some(), "--gpu-mem without --gpu");
        a.plan.gpu_mem = Some(1024);
        assert!(n.gpu_plan_refusal(&owner, &a.plan).is_some(), "out of range");
    }

    /// GPU jobs S6: on a unified GPU a GPU job's memory is its `--mem` plus
    /// its `--gpu-mem` (default 2G): it must fit before it starts and is
    /// held while it runs. On a discrete GPU only `--mem` counts, and only
    /// an explicit `--gpu-mem` is watched.
    #[tokio::test]
    async fn a_unified_gpu_job_counts_its_gpu_memory_beside_its_mem() {
        let (n, owner, _, _) = rig("gpu-mem-count", ready(&["owner"])).await;
        let caps_mem = n.queue_caps().mem_bytes;
        let mut a = gpu_assign(&n, &owner, "u1");
        a.plan.limits.mem_bytes = Some(caps_mem - (1 << 30));
        a.fence.plan_digest = a.plan.digest().unwrap();
        // Discrete (the rig's facts): --mem alone; not watched.
        assert_eq!((n.gpu_mem_counted(&a.plan), n.gpu_mem_cap(&a.plan)), (0, None));
        assert_eq!(n.assign_refusal(&owner, &a), None, "caps - 1G fits");
        n.host.lock().unwrap().gpus[0].unified = true;
        assert_eq!(n.gpu_mem_counted(&a.plan), 2 << 30, "the default --gpu-mem on a unified GPU");
        assert_eq!(n.gpu_mem_cap(&a.plan), Some(2 << 30));
        assert_eq!(n.plan_mem(&a.plan), Some(caps_mem + (1 << 30)));
        let r = n.assign_refusal(&owner, &a).expect("caps - 1G + 2G of GPU memory doesn't fit");
        assert_eq!(r.code, k2_node_proto::refusal::LOCAL_RESOURCE_LIMIT, "{r:?}");
        a.plan.gpu_mem = Some(512 << 20);
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.assign_refusal(&owner, &a), None, "a smaller --gpu-mem fits");
        // A running GPU job holds its GPU memory: the offer's free memory
        // shows it.
        let free_before = n.offer(&owner).free.mem_bytes;
        a.plan.limits.mem_bytes = Some(1 << 30);
        a.plan.gpu_mem = None;
        n.jobs.lock().unwrap().insert(
            (0, "held".into()),
            RunningJob {
                generation: 1,
                cpu_millis: Some(100),
                mem_bytes: n.plan_mem(&a.plan),
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
        assert_eq!(free_before - n.offer(&owner).free.mem_bytes, 3 << 30, "1G --mem + 2G GPU memory held");
        // A plan without --gpu never counts GPU memory, whatever it says.
        let mut plain = crate::ledger::tests::assign("p", 1);
        plain.plan.gpu_mem = Some(8 << 30);
        assert_eq!((n.gpu_mem_counted(&plain.plan), n.gpu_mem_cap(&plain.plan)), (0, None));
        // Discrete with an explicit budget: watched, not counted.
        n.host.lock().unwrap().gpus[0].unified = false;
        a.plan.gpu_mem = Some(1 << 30);
        assert_eq!((n.gpu_mem_counted(&a.plan), n.gpu_mem_cap(&a.plan)), (0, Some(1 << 30)));
    }

    /// GPU jobs S6: the node asks root for a unit's GPU memory only when
    /// `Ready` said `gpu_usage`, and reads the answer off the wire.
    #[tokio::test]
    async fn gpu_mem_sample_asks_root_only_when_it_can() {
        use crate::admin::exec::{LocalRequest, LocalResponse};
        use crate::admin::jobs::{JobAnswer, JobOp};
        let (n, _, _, _) = rig("gpu-mem-sample", ready(&["owner"])).await;
        assert_eq!(n.gpu_mem_sample("k2job-x-g1.service").await, None, "no gpu_usage: never asked (the rig's socket doesn't exist)");
        let d = crate::util::temp_dir("gpu-mem-sock");
        let sock = crate::admin::server::short_sock(&d);
        let _ = std::fs::remove_file(&sock);
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        // Every other op (the node's own open asks root for its key's
        // taint mark) is refused, and on.
        std::thread::spawn(move || loop {
            let Ok((mut s, _)) = l.accept() else { break };
            let Ok(h) = crate::admin::server::read_frame(&mut s, 1 << 20) else { continue };
            let ans = match serde_json::from_slice::<LocalRequest>(&h) {
                Ok(LocalRequest::Job { job: JobOp::GpuUsage { unit } }) => {
                    let _ = tx.send(unit);
                    JobAnswer::GpuUsage(k2_node_proto::gpu::GpuMemUsage { bytes: 5 << 30, gtt_bytes: 5 << 30, clients: 2, pids: 3, ..Default::default() })
                }
                _ => JobAnswer::Refused(k2_node_proto::admin::AdminRefusal::new("not_configured", "the test executor answers gpu_usage only")),
            };
            let _ = crate::admin::server::write_frame(&mut s, &serde_json::to_vec(&LocalResponse::Job(ans)).unwrap());
        });
        // A node whose executor reads GPU memory, on that socket.
        let mut o = NodeOptions::new(crate::paths::Layout::new(d.join("home"), d.join("cfg")));
        o.layout.ensure().unwrap();
        std::fs::create_dir_all(&o.layout.config).unwrap();
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.wall = Some(Wall::JobUser { user: "k2job".into() });
        o.exec_stdin = Some(true);
        o.exec_gpus = Some(ready(&["owner"]));
        o.exec_gpu_usage = Some(true);
        o.admin_socket = sock.clone();
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
        let m = Node::open(o).await.unwrap();
        let u = m.gpu_mem_sample("k2job-x-g1.service").await.expect("an answer");
        assert_eq!((u.bytes, u.clients, u.pids), (5 << 30, 2, 3));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "k2job-x-g1.service");
        let _ = std::fs::remove_file(&sock);
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

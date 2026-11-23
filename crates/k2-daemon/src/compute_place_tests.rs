//! `compute_place`'s unit tests, in their own file so compute_place.rs stays under the
//! ratchet's 4000 lines (moved unchanged).

use super::*;

/// The offer features the controller reads are the strings k2-node
/// writes (Lane A and Lane C each name them; one must not drift).
#[test]
fn offer_feature_names_match_the_node_protocol() {
    assert_eq!(FEATURE_RUST_COMPONENTS, k2_node_proto::needs::FEATURE_RUST_COMPONENTS);
    assert_eq!(FEATURE_TOOLCHAIN_BUDGET, k2_node_proto::needs::FEATURE_TOOLCHAIN_BUDGET);
}

// ── auto-placement fixtures (prd-compute-auto-placement-v1 §10) ──

const GB: u64 = 1 << 30;
const ME: &str = "ws-me";

fn node_row(id: &str) -> NodeRow {
    NodeRow {
        id: id.into(),
        name: id.into(),
        fingerprint: String::new(),
        public_key_pem: String::new(),
        os: None,
        arch: None,
        labels: Default::default(),
        state: "active".into(),
        enrolled_via: "code".into(),
        enrolled_by: None,
        enrolled_at: 1,
        sas: None,
        confirmed_by: None,
        confirmed_at: None,
        revoked_by: None,
        revoked_at: None,
        controller_pause: None,
        controller_pause_by: None,
        controller_pause_at: None,
        last_seen: Some(100),
        last_route: None,
        protocol: Some(6),
        node_version: None,
        routes: vec![],
        last_offer: None,
        role: "owner".into(),
        shared: false,
        owner_label: None,
        share: None,
        share_at: None,
    }
}

fn grant_row(node: &str, max_job_secs: i64, max_parallel: i64) -> GrantRow {
    GrantRow {
        node_id: node.into(),
        workspace_id: ME.into(),
        scopes: String::new(),
        max_job_secs,
        max_disk_gb: 60,
        max_parallel,
        max_queued: 10,
        warm_at: None,
        expires_at: None,
        granted_by: "owner".into(),
        granted_at: 1,
        allow_ci: false,
        submitter_max_running: 0,
        allow_gpu: false,
        allow_userns: false,
        allow_no_addr_filter: false,
    }
}

/// A solo node: `free_gb` of `caps_gb` memory free, 8 CPUs, `running`
/// of `parallel` slots in use (the registry agrees, another workspace's).
pub(super) fn cand(id: &str, free_gb: u64, caps_gb: u64, running: u32, parallel: u32) -> Cand {
    let offer = Offer {
        share: None,
        revision: 1,
        control: frames::Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 8000, mem_bytes: caps_gb * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: frames::Free { cpu_millis: 8000 - u64::from(running) * 1000, mem_bytes: free_gb * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running,
        parallel_max: parallel,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.47".to_string())].into(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.6".into(),
        features: vec![], host: None, pkg: None, sim: None,
    };
    let mut load = scheduler::Load::default();
    if running > 0 {
        load.running_by_workspace.insert("ws-other-secret".into(), running);
    }
    Cand { node: node_row(id), online: true, offer, load, queued: 0, grant: Some(grant_row(id, 7200, 2)), pending: Pending::default(), recent: 0, holds_cache: false, est_wait: None, held_wait: 0, gen_heads: Vec::new(), ws_id: "ws-test".into() }
}

/// Make `c` a node shared with another server (this server a tenant):
/// `quote` = (starts now, ETA) for a 2 GB ask, or none.
pub(super) fn shared(mut c: Cand, quote: Option<(bool, Option<u64>)>) -> Cand {
    c.offer.features.push(pshare::FEATURE_QUEUE.to_string());
    c.offer.share = Some(ShareView {
        role: pshare::Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some("other.k2.dev".into()),
        state: pshare::ServerStatus::Active,
        limits: pshare::ShareLimits::new_server(pshare::Trust::Own, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        disk_used_bytes: None,
        quotes: quote
            .map(|(now, eta)| vec![pshare::Quote { cpu_millis: 0, mem_bytes: 2 * GB, starts_now: now, eta_secs: eta, eta_p90_secs: None, blocked_by: None }])
            .unwrap_or_default(),
        priority: None,
    });
    c
}

fn warm(mut c: Cand, sha: &str) -> Cand {
    c.offer.slots.push(frames::SlotInfo { project_key: "pk".into(), slot: 0, busy: false, last_sha: Some(sha.into()) });
    c
}

/// B2 (§4.1.6, BK-17): a node whose trusted cache head is this commit
/// is as warm as a slot that built it, and outranks a cold node; a
/// head of another project, or one the workspace's repo can't place
/// (here: no such workspace), counts as cold.
#[test]
fn a_trusted_head_at_the_commit_is_warm() {
    let c1 = "c".repeat(40);
    let mut held = cand("held", 32, 32, 0, 2);
    held.gen_heads = vec![("pk".into(), c1.clone())];
    let cold = cand("cold", 32, 32, 0, 2);
    assert_eq!(warmth(&held, Some("pk"), Some(&c1)), 2);
    assert_eq!(warmth(&cold, Some("pk"), Some(&c1)), 0);
    assert_eq!(warmth(&held, Some("other"), Some(&c1)), 0, "another project's head");
    assert_eq!(gen_warmth(&held, Some("pk"), Some(&"d".repeat(40))), 0, "a head the repo can't place is cold");
    let w = want(1);
    let cands = vec![cold, held];
    let (c, _) = choose_by(V2, &cands, &w, &[], Some("pk"), Some(&c1)).expect("a pick");
    assert_eq!(c.node.id, "held", "the warm head wins a tie");
}

pub(super) fn want(mem_gb: u64) -> Want {
    Want {
        ws_id: ME.into(),
        admin: false,
        auto: true,
        filter: AnyFilter::default(),
        needs: vec![],
        artifacts: false,
        exclusive: false,
        needs_git: false,
        cpu_millis: None,
        mem_bytes: Some(mem_gb * GB),
        mem_defaulted: false,
        timeout_secs: None,
        stall: false,
        secret_ids: vec![],
        tier: JobTier::Normal,
        cache: None,
        multi_repo: false,
        receipt_v2: false,
        gpu: None,
        gpu_mem: None,
        network: None,
        userns: false,
        no_addr_filter: false,
    }
}

/// A node offering GPU jobs to this server: one AMD GPU open to it.
fn gpu_cand(id: &str) -> Cand {
    let mut c = cand(id, 30, 32, 0, 2);
    c.offer.features.push(k2_core::compute::proto::gpu::FEATURE_GPU.into());
    c.offer.caps.gpus = 1;
    c.offer.free.gpus = 1;
    c.offer.host = Some(k2_node_proto::host::HostFacts {
        os: "linux".into(),
        arch: "x86_64".into(),
        distro: "arch".into(),
        gpus: vec![k2_core::compute::proto::gpu::GpuFact { vendor: "amd".into(), pci: "0000:c4:00.0".into(), jobs: true, ..Default::default() }],
        ..Default::default()
    });
    c
}

fn gpu_want(admin: bool, vendor: k2_core::compute::proto::gpu::GpuVendor) -> Want {
    Want { admin, gpu: Some(k2_core::compute::proto::gpu::GpuAsk::one(vendor)), ..want(2) }
}

/// GPU jobs (0.46.11): `--gpu` skips a node without the `gpu` feature
/// for this server, one whose GPU isn't open to it, one of another
/// vendor, and (for an agent) one whose grant lacks `--gpu`; a job
/// without `--gpu` sees every node as before.
#[test]
fn gpu_want_skips_nodes_without_the_feature() {
    use k2_core::compute::proto::gpu::GpuVendor;
    let plain = cand("plain", 30, 32, 0, 2);
    let mut g = gpu_cand("gpu");
    let agent = gpu_want(false, GpuVendor::Any);
    assert_eq!(gpu_skip(&agent, plain.grant.as_ref(), &plain.offer), Some(skip::NO_GPU));
    assert_eq!(gpu_skip(&agent, g.grant.as_ref(), &g.offer), Some(skip::GPU_NEEDS_GRANT), "the grant lacks --gpu");
    g.grant.as_mut().unwrap().allow_gpu = true;
    assert_eq!(gpu_skip(&agent, g.grant.as_ref(), &g.offer), None);
    assert_eq!(gpu_skip(&gpu_want(false, GpuVendor::Intel), g.grant.as_ref(), &g.offer), Some(skip::NO_GPU), "another vendor");
    // The GPU listed, but not open to this server (jobs: false).
    let mut closed = gpu_cand("closed");
    closed.grant.as_mut().unwrap().allow_gpu = true;
    closed.offer.host.as_mut().unwrap().gpus[0].jobs = false;
    assert_eq!(gpu_skip(&agent, closed.grant.as_ref(), &closed.offer), Some(skip::NO_GPU));
    // The owner or an Admin needs no --gpu grant (like --ci).
    let mut owner_g = gpu_cand("o");
    owner_g.grant = None;
    assert_eq!(gpu_skip(&gpu_want(true, GpuVendor::Amd), None, &owner_g.offer), None);
    // No --gpu: nothing changes.
    assert_eq!(gpu_skip(&want(4), plain.grant.as_ref(), &plain.offer), None);
    assert!(required_features(&agent).contains(&k2_core::compute::proto::gpu::FEATURE_GPU));
    assert!(!required_features(&want(4)).contains(&k2_core::compute::proto::gpu::FEATURE_GPU));
}

/// GPU jobs: with no node left, `no_gpu_node` (404, exit 3) names both
/// steps; a GPU node whose grant lacks `--gpu` is `gpu_needs_grant`
/// (403, the card) naming that node.
#[test]
fn no_gpu_node_when_no_visible_node_has_a_gpu() {
    use k2_core::compute::proto::gpu::{codes, GpuVendor};
    let w = gpu_want(false, GpuVendor::Any);
    let mut r = CandReport { visible: 2, ..Default::default() };
    r.skipped.insert(skip::NO_GPU, 2);
    let (code, status, hint, node) = gpu_refusal(&w, &r, "builder").expect("refused");
    assert_eq!((code, status, node), (codes::NO_GPU_NODE, 404, None));
    assert!(hint.contains("sudo k2-node admin gpu on") && hint.contains("k2 compute grant <node> --workspace builder --gpu"), "{hint}");
    r.skipped.insert(skip::GPU_NEEDS_GRANT, 1);
    r.gpu_needs_grant = vec!["k2n-gpu".into()];
    let (code, status, hint, node) = gpu_refusal(&w, &r, "builder").expect("refused");
    assert_eq!((code, status, node.as_deref()), (codes::GPU_NEEDS_GRANT, 403, Some("k2n-gpu")));
    assert!(hint.contains("k2 compute grant k2n-gpu --workspace builder --gpu"), "{hint}");
    // Nodes left, or no GPU ask: no GPU refusal.
    r.cands.push(gpu_cand("gpu"));
    assert_eq!(gpu_refusal(&w, &r, "builder"), None);
    assert_eq!(gpu_refusal(&want(4), &CandReport::default(), "builder"), None);
}

/// A10: on a shared node the quote says "now", but a GPU job also
/// needs a free GPU in the offer.
#[test]
fn a_shared_gpu_node_with_its_gpu_held_is_not_now() {
    use k2_core::compute::proto::gpu::GpuVendor;
    let w = gpu_want(true, GpuVendor::Any);
    let mut c = shared(gpu_cand("s"), Some((true, None)));
    assert!(c.starts_now(&w));
    c.offer.free.gpus = 0;
    assert!(!c.starts_now(&w), "its GPU is held");
    assert!(c.starts_now(&Want { gpu: None, ..w }), "a job without --gpu still starts");
}

/// GPU jobs S6: on a unified GPU (an APU) a GPU job's `--gpu-mem`
/// (default 2G) counts against the node's free memory beside its
/// `--mem`, so placement never overpacks it; on a discrete GPU nothing
/// is added. A node whose caps can't hold both is too small.
#[test]
fn unified_gpu_memory_counts_against_the_nodes_memory() {
    use k2_core::compute::proto::gpu::GpuVendor;
    let mut c = gpu_cand("apu");
    c.offer.free.mem_bytes = 5 * GB;
    let w = Want { mem_bytes: Some(4 * GB), ..gpu_want(true, GpuVendor::Any) };
    assert_eq!(w.mem_on(&c.offer), Some(4 * GB), "a discrete GPU: --mem alone");
    assert!(c.starts_now(&w), "4G fits in 5G free");
    c.offer.host.as_mut().unwrap().gpus[0].unified = true;
    assert_eq!(w.mem_on(&c.offer), Some(6 * GB), "--mem 4G + the default --gpu-mem 2G");
    assert!(!c.starts_now(&w), "4G + 2G of GPU memory doesn't fit in 5G free");
    let small = Want { gpu_mem: Some(512 << 20), ..w.clone() };
    assert_eq!(small.mem_on(&c.offer), Some(4 * GB + (512 << 20)));
    assert!(c.starts_now(&small), "a smaller --gpu-mem fits");
    assert!(c.starts_now(&Want { gpu: None, gpu_mem: None, ..w.clone() }), "no --gpu: --mem alone");
    // Over the node's caps with its GPU memory: never fits there.
    c.offer.caps.mem_bytes = 5 * GB;
    assert!(scheduler::never_fits(&c.offer, &w.probe_on(&c.offer)).is_some());
    assert!(scheduler::never_fits(&c.offer, &small.probe_on(&c.offer)).is_none());
    // A queued plan counts the same way (the solo scheduler, pending asks).
    let mut p: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j", "workspace_id": ME, "workspace_label": "w", "requested_by": "owner", "node_id": "apu", "argv": ["true"], "env": {},
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": 4 * GB, "disk_bytes": GB, "log_cap_bytes": 1024},
        "src": null, "exclusive": false, "created_at": 1
    }))
    .expect("a plan");
    assert_eq!(plan_mem_on(&p, Some(&c.offer)), Some(4 * GB));
    p.gpu = w.gpu;
    assert_eq!(plan_mem_on(&p, Some(&c.offer)), Some(6 * GB));
    p.gpu_mem = Some(GB);
    assert_eq!(plan_mem_on(&p, Some(&c.offer)), Some(5 * GB));
    assert_eq!(plan_mem_on(&p, None), Some(4 * GB), "no offer: nothing to say it's unified");
}

/// GPU jobs S6: an explicit `--gpu-mem` goes only to a node offering
/// `gpu-mem` (k2-node 0.46.11 with a new executor); an older GPU node
/// is skipped with its own code, and with none left the refusal says
/// to update k2-node or drop `--gpu-mem` (409, exit 3). Without
/// `--gpu-mem` the older node still takes the GPU job.
#[test]
fn gpu_mem_needs_a_node_that_watches_it() {
    use k2_core::compute::proto::gpu::{codes, GpuVendor, FEATURE_GPU_MEM};
    let mut c = gpu_cand("old");
    c.grant.as_mut().unwrap().allow_gpu = true;
    let plain = gpu_want(false, GpuVendor::Any);
    let budget = Want { gpu_mem: Some(4 * GB), ..plain.clone() };
    assert_eq!(gpu_skip(&plain, c.grant.as_ref(), &c.offer), None, "an older GPU node takes --gpu as before");
    assert_eq!(gpu_skip(&budget, c.grant.as_ref(), &c.offer), Some(skip::GPU_MEM_NEEDS_UPDATE));
    assert!(required_features(&budget).contains(&FEATURE_GPU_MEM));
    assert!(!required_features(&plain).contains(&FEATURE_GPU_MEM));
    let mut r = CandReport { visible: 1, ..Default::default() };
    r.skipped.insert(skip::GPU_MEM_NEEDS_UPDATE, 1);
    let (code, status, hint, _) = gpu_refusal(&budget, &r, "builder").expect("refused");
    assert_eq!((code, status), (codes::GPU_MEM_NEEDS_UPDATE, 409));
    assert!(hint.contains("0.46.11") && hint.contains("without --gpu-mem"), "{hint}");
    assert!(skip_words(skip::GPU_MEM_NEEDS_UPDATE, 1, &budget).contains("too old for --gpu-mem"));
    c.offer.features.push(FEATURE_GPU_MEM.into());
    assert_eq!(gpu_skip(&budget, c.grant.as_ref(), &c.offer), None);
}

fn pick(policy: compute::PlacementPolicy, cands: &[Cand], w: &Want) -> (String, Tier) {
    let (c, t) = choose_by(policy, cands, w, &[], Some("pk"), Some("c1")).expect("a pick");
    (c.node.id.clone(), t)
}

/// The whole order a ranking gives (pick, drop it, pick again).
fn order(policy: compute::PlacementPolicy, cands: &[Cand], w: &Want) -> Vec<String> {
    let mut left = cands.to_vec();
    let mut out = Vec::new();
    while let Some((c, _)) = choose_by(policy, &left, w, &[], Some("pk"), Some("c1")) {
        let id = c.node.id.clone();
        out.push(id.clone());
        left.retain(|x| x.node.id != id);
    }
    out
}

use compute::PlacementPolicy::{V1, V2};

/// D2 (§10 unit 1): a warm slot is worth one load band. A warm, lightly
/// loaded node ties with a cold idle one and warmth wins; a warm,
/// heavily loaded node loses to a cold idle one.
#[test]
fn a_warm_slot_is_worth_one_band() {
    let w = want(2);
    let warm_light = warm(cand("warm-light", 28, 32, 1, 4), "c1");
    let warm_heavy = warm(cand("warm-heavy", 4, 32, 1, 2), "c1");
    let cold_idle = cand("cold-idle", 32, 32, 0, 4);
    assert_eq!((warm_light.band(&w), warm_heavy.band(&w), cold_idle.band(&w)), (1, 2, 0));
    assert_eq!(pick(V2, &[cold_idle.clone(), warm_light.clone()], &w), ("warm-light".into(), Tier::Now), "warm + light ties idle; warmth wins");
    assert_eq!(pick(V2, &[warm_heavy.clone(), cold_idle.clone()], &w), ("cold-idle".into(), Tier::Now), "warm + heavy loses to idle");
    // v1 (the 0.46.5 order): any warm slot beats any load difference.
    assert_eq!(pick(V1, &[warm_heavy, cold_idle], &w).0, "warm-heavy");
}

/// §10 unit 2 (and §4.4): two equal idle nodes; once one took a job
/// (a recent placement, or this workspace runs one there), the next
/// goes to the other.
#[test]
fn a_burst_spreads_over_equal_nodes() {
    let w = want(2);
    let (mut a, b) = (cand("a", 32, 32, 0, 4), cand("b", 32, 32, 0, 4));
    assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "a", "the name breaks a full tie");
    a.recent = 1;
    a.pending = Pending { mem_bytes: 2 * GB, cpu_millis: 0, jobs: 1, holds: 0 };
    assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "b", "a just took one: the burst spreads");
    // Same band, no pending: this controller's recent placements and
    // this workspace's own running jobs still spread it.
    let (mut a, b) = (cand("a", 20, 32, 1, 4), cand("b", 20, 32, 1, 4));
    a.recent = 1;
    assert_eq!(pick(V2, &[a.clone(), b.clone()], &w).0, "b", "recent placements spread");
    a.recent = 0;
    a.load.running_by_workspace.insert(ME.into(), 1);
    assert_eq!(pick(V2, &[a, b], &w).0, "b", "one builder's jobs spread");
}

/// §10 unit 3 (CP-A6): a shared node whose node-wide free memory is
/// 2 GB is not Now for a 4 GB job, even with no quote; it is for 1 GB.
#[test]
fn a_shared_node_checks_node_wide_free_memory() {
    let c = shared(cand("mini", 2, 8, 1, 2), None);
    assert!(!c.starts_now(&want(4)), "2 GB free node-wide: not Now for --mem 4G");
    assert!(c.starts_now(&want(1)), "1 GB fits");
    assert_eq!(c.tier(&want(4)), Some(Tier::Busy), "it still queues there");
    // A quote that says "starts now" for this size doesn't override
    // node-wide free memory (1 GB free, a 2 GB job).
    let q = shared(cand("mini", 1, 8, 1, 2), Some((true, None)));
    assert!(!q.starts_now(&want(2)));
    assert!(shared(cand("mini", 4, 8, 1, 2), Some((true, None))).starts_now(&want(2)), "with room, the quote stands");
}

/// A mini with little free memory is skipped for a big --mem job: the
/// one with room starts it now.
#[test]
fn a_big_mem_job_skips_the_node_without_free_memory() {
    let w = want(24);
    let small = cand("mini", 6, 16, 1, 2);
    let big = cand("ryzen", 100, 110, 3, 8);
    assert!(!small.starts_now(&w) && big.starts_now(&w));
    assert_eq!(pick(V2, &[small, big], &w), ("ryzen".into(), Tier::Now));
}

/// §10 unit 4 (CP-A7): pending asks. Two 24 GB jobs in the same
/// second: the second doesn't see the first one's memory as free; a
/// newer offer revision, or 60 s, drops the entry.
#[test]
fn pending_asks_cover_a_burst_until_the_next_offer() {
    let node = "pending-test-node";
    let now = 1_000_000;
    assert_eq!(pending_for(node, Some(7), now), Pending::default());
    note_pending(node, "job-1", Some(24 * GB), Some(4000), 7, now);
    let p = pending_for(node, Some(7), now);
    assert_eq!(p, Pending { mem_bytes: 24 * GB, cpu_millis: 4000, jobs: 1, holds: 0 });
    let mut c = cand(node, 40, 64, 0, 4);
    assert!(c.starts_now(&want(24)), "before: 40 GB free");
    c.pending = p;
    assert_eq!(c.free_now(), (16 * GB, 4000));
    assert!(!c.starts_now(&want(24)), "the first job's 24 GB is not free any more");
    note_pending(node, "job-1", Some(24 * GB), Some(4000), 7, now);
    assert_eq!(pending_for(node, Some(7), now).jobs, 1, "the same job counts once");
    assert_eq!(pending_for(node, Some(8), now), Pending::default(), "a newer offer shows it: dropped");
    note_pending(node, "job-2", Some(GB), None, 9, now);
    assert_eq!(pending_for(node, None, now + PENDING_SECS - 1).jobs, 1);
    assert_eq!(pending_for(node, None, now + PENDING_SECS).jobs, 0, "dropped after 60 s");
    // Recent placements: 30 s.
    note_placed(node, now);
    note_placed(node, now + 10);
    assert_eq!(recent_for(node, now + 20), 2);
    assert_eq!(recent_for(node, now + RECENT_SECS + 5), 1);
}

/// §10 unit 5 (F10, CP-A4): --timeout 4h skips a 2 h grant and keeps a
/// 6 h one; no --timeout fits every grant (it gets the grant's max).
#[test]
fn a_timeout_over_the_grant_skips_that_node() {
    let mut w = want(2);
    let (two, six) = (grant_row("a", 2 * 3600, 1), grant_row("b", 6 * 3600, 1));
    assert!(timeout_fits(&w, Some(&two)) && timeout_fits(&w, Some(&six)));
    w.timeout_secs = Some(4 * 3600);
    assert!(!timeout_fits(&w, Some(&two)), "4h > 2h grant");
    assert!(timeout_fits(&w, Some(&six)), "4h ≤ 6h grant");
    assert!(!timeout_fits(&w, None), "no grant: the 2h default");
}

/// §10 unit 6 (F11, CP-A5): a share's per-job memory of 8 GB rules the
/// node out for --mem 12G (not for a defaulted --mem, which submit
/// clamps); its 2 h per job rules out --timeout 3h; a solo node has no
/// share limits.
#[test]
fn a_shares_limits_skip_that_node() {
    let c = shared(cand("mini", 30, 32, 0, 2), None);
    let g = grant_row("mini", 2 * 3600, 1);
    assert!(share_fits(&want(8), Some(&g), &c.offer));
    assert!(!share_fits(&want(12), Some(&g), &c.offer), "over the share's 8 GB per job");
    let mut d = want(12);
    d.mem_defaulted = true;
    assert!(share_fits(&d, Some(&g), &c.offer), "a defaulted --mem is clamped at submit");
    let mut t = want(2);
    t.timeout_secs = Some(3 * 3600);
    assert!(!share_fits(&t, Some(&g), &c.offer), "over the share's 2h per job");
    let mut x = want(2);
    x.exclusive = true;
    assert!(!share_fits(&x, Some(&g), &c.offer), "the share doesn't allow exclusive");
    assert!(share_fits(&want(12), Some(&g), &cand("solo", 30, 32, 0, 2).offer), "solo: no share limits");
}

/// §10 unit 8: an exclusive job is Now only on an idle node (solo or
/// shared).
#[test]
fn an_exclusive_job_starts_only_on_an_idle_node() {
    let mut w = want(2);
    w.exclusive = true;
    assert!(cand("idle", 32, 32, 0, 4).starts_now(&w));
    assert!(!cand("busy", 30, 32, 1, 4).starts_now(&w));
    assert!(!shared(cand("sbusy", 30, 32, 1, 4), None).starts_now(&w));
    assert_eq!(pick(V2, &[cand("busy", 30, 32, 1, 4), cand("idle", 32, 32, 0, 4)], &w).0, "idle");
}

/// §10 unit 9: busy everywhere, a quoted ETA beats an unknown one and a
/// lower ETA a higher one.
#[test]
fn busy_everywhere_the_shortest_quoted_wait_wins() {
    let w = want(2);
    let solo_full = cand("a-solo-full", 30, 32, 2, 2);
    let slow = shared(cand("b-slow", 30, 32, 2, 2), Some((false, Some(900))));
    let fast = shared(cand("c-fast", 30, 32, 2, 2), Some((false, Some(120))));
    let all = [solo_full, slow, fast];
    for c in &all {
        assert_eq!(c.tier(&w), Some(Tier::Busy), "{}", c.node.id);
    }
    assert_eq!(order(V2, &all, &w), vec!["c-fast", "b-slow", "a-solo-full"]);
    assert_eq!(pick(V1, &all, &w).0, "c-fast", "v1 also takes the lowest quote");
}

/// 0.46.8 (K2 Cores, 17789afd): a node that can't start the job now
/// never outranks one that can, however warm (this commit), however
/// short its quoted or estimated wait. Warmth only orders within a tier.
#[test]
fn a_node_that_starts_now_always_outranks_a_warm_busy_one() {
    let w = want(2);
    // Warm for this commit, quoted 30 s, but no free slot or CPU now.
    let mut warm_busy = warm(shared(cand("a-warm-busy", 30, 32, 2, 2), Some((false, Some(30)))), "c1");
    warm_busy.offer.free.cpu_millis = 0;
    let warm_solo_busy = warm(cand("b-warm-solo-busy", 30, 32, 2, 2), "c1");
    let cold_free = cand("c-cold-free", 30, 32, 0, 3);
    for (c, t) in [(&warm_busy, Tier::Busy), (&warm_solo_busy, Tier::Busy), (&cold_free, Tier::Now)] {
        assert_eq!(c.tier(&w), Some(t), "{}", c.node.id);
    }
    assert!(warmth_v2(&warm_busy, &w, Some("pk"), Some("c1")) >= 2, "the busy ones really are warm");
    let all = [warm_busy, warm_solo_busy, cold_free];
    for policy in [V1, V2] {
        assert_eq!(order(policy, &all, &w)[0], "c-cold-free", "{policy:?}");
    }
}

/// 0.46.8: a burst while every node is busy spreads across shared nodes:
/// this server's jobs still held here for a node count on top of its
/// quote (the node's quote doesn't know them yet).
#[test]
fn a_burst_spreads_when_held_jobs_lengthen_the_lowest_quote() {
    let w = want(2);
    let mut low = shared(cand("a-low-quote", 30, 32, 2, 2), Some((false, Some(60))));
    let high = shared(cand("b-higher-quote", 30, 32, 2, 2), Some((false, Some(240))));
    assert_eq!(order(V2, &[low.clone(), high.clone()], &w), vec!["a-low-quote", "b-higher-quote"], "nothing held: the lowest quote");
    // One unsized job held for a-low-quote: 900 s of work on 2 slots.
    let waits = SoloWaits { held: [("a-low-quote".to_string(), vec![None])].into(), ..Default::default() };
    low.held_wait = waits.held_eta("a-low-quote", low.offer.parallel_max);
    assert_eq!(low.held_wait, 450);
    assert_eq!(low.wait_eta(&w), Some((510, "quote")));
    assert_eq!(order(V2, &[low.clone(), high.clone()], &w), vec!["b-higher-quote", "a-low-quote"], "the next job goes elsewhere");
    // A known estimate counts as itself.
    let waits = SoloWaits { held: [("a-low-quote".to_string(), vec![Some(100)])].into(), ..Default::default() };
    assert_eq!(waits.held_eta("a-low-quote", 2), 50);
    assert_eq!(waits.held_eta("nowhere", 2), 0);
}

/// §10 unit 10: the v1 switch keeps the 0.46.5 order exactly (a golden
/// over a mixed set), and v2 differs only where §4 says.
#[test]
fn the_v1_switch_keeps_the_old_order() {
    let w = want(2);
    let mut offline = cand("f-offline", 32, 32, 0, 4);
    offline.online = false;
    let set = [
        warm(cand("a-warm-heavy", 4, 32, 1, 2), "c1"),
        cand("b-cold-idle", 32, 32, 0, 4),
        cand("c-cold-light", 24, 32, 1, 4),
        shared(cand("d-busy-quoted", 30, 32, 2, 2), Some((false, Some(300)))),
        cand("e-busy-solo", 30, 32, 2, 2),
        offline,
    ];
    assert_eq!(
        order(V1, &set, &w),
        vec!["a-warm-heavy", "b-cold-idle", "c-cold-light", "d-busy-quoted", "e-busy-solo", "f-offline"],
        "v1 golden: warm first, then less busy, quoted waits, offline last"
    );
    assert_eq!(
        order(V2, &set, &w),
        vec!["b-cold-idle", "a-warm-heavy", "c-cold-light", "d-busy-quoted", "e-busy-solo", "f-offline"],
        "v2: the idle node before the warm heavy one; warm wins the band tie with a light one"
    );
    for p in [V1, V2] {
        assert_eq!(compute::PlacementPolicy::from_wire(p.as_wire()), Some(p));
    }
    assert_eq!(compute::PlacementPolicy::from_wire("v3"), None);
}

/// `sys:` packages: a node that installs them ranks first, all else
/// equal; one that can't stays a candidate.
#[test]
fn sys_needs_prefer_a_node_that_installs_packages() {
    let mut w = want(2);
    w.needs = vec!["sys:pkg-config".into()];
    let mac = cand("a-mac", 32, 32, 0, 4);
    let mut linux = cand("b-linux", 32, 32, 0, 4);
    linux.offer.features.push(needs::FEATURE_SYSTEM_PACKAGES.to_string());
    assert_eq!(pick(V2, &[mac.clone(), linux], &w).0, "b-linux");
    assert_eq!(pick(V2, &[mac], &w), ("a-mac".into(), Tier::Now), "still a candidate alone");
}

/// §7: the record and its words. Codes per case; the line names only
/// node names, node-wide numbers and counts — never another
/// workspace's id (S2/S5).
#[test]
fn the_placement_record_says_where_and_why() {
    let w = want(24);
    let cands = [warm(cand("node-a", 46, 110, 1, 8), "c1"), cand("node-b", 10, 16, 1, 2)];
    let skipped = BTreeMap::from([(skip::OFFLINE, 1u32), (skip::TOO_SMALL, 1u32)]);
    let ex = Explain { policy: V2, cands: &cands, skipped: &skipped, project_key: Some("pk"), commit: Some("c1") };
    let (c, t) = choose_by(V2, &cands, &w, &[], Some("pk"), Some("c1")).unwrap();
    let rec = placement_record(c, t, &w, &ex, 42);
    assert_eq!((rec.mode.as_str(), rec.policy.as_str(), rec.node.as_str(), rec.code.as_str()), ("auto", "v2", "node-a", "starts_now_warm"));
    assert_eq!(rec.why, "starts now · warm slot (this commit) · 46/110 GB, 7/8 CPUs free");
    assert_eq!(rec.fits, "2 nodes fit, 2 skipped (1 offline, 1 too small for --mem 24G)");
    assert_eq!((rec.starts_now, rec.considered, rec.at), (Some(true), 2, 42));
    let text = serde_json::to_string(&rec).unwrap();
    assert!(!text.contains("ws-other-secret") && !text.contains(ME), "no workspace ids in the record: {text}");
    // Idle, least loaded, quoted wait, offline only.
    let idle = [cand("idle", 32, 32, 0, 4)];
    let ex = Explain { policy: V1, cands: &idle, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&idle[0], Tier::Now, &want(2), &ex, 1);
    assert_eq!((rec.code.as_str(), rec.policy.as_str(), rec.fits.as_str()), ("starts_now_idle", "v1", "1 node fits"));
    let q = [shared(cand("q", 30, 32, 2, 2), Some((false, Some(330))))];
    let ex = Explain { policy: V2, cands: &q, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&q[0], Tier::Busy, &want(2), &ex, 1);
    assert_eq!((rec.code.as_str(), rec.eta_secs, rec.starts_now), ("queued_quoted_eta", Some(330), Some(false)));
    assert_eq!(rec.why, "no node can start it now (it asks 2G): q (full: 2 of 2 jobs run); shortest wait ~6 min (quoted) · it moves if another frees up first");
    let rec = placement_record(&q[0], Tier::Offline, &want(2), &ex, 1);
    assert_eq!(rec.code, "queued_offline_only");
    // A pinned job's record.
    let p = pinned_record("node-b", 5);
    assert_eq!((p.mode.as_str(), p.code.as_str(), p.starts_now), ("pinned", "pinned", None));
    assert_eq!(mem_words(3 * GB / 2), "1.5G");
}

/// `--os`/`--arch` spellings: every alias matches what nodes report
/// (Macs `macos`/`arm64`, Linux `linux`/`x86_64` or `aarch64`); other
/// labels match exactly; a missing label never matches.
#[test]
fn os_and_arch_aliases_match_what_nodes_report() {
    for want in ["arm64", "aarch64", "ARM64", "AArch64"] {
        assert!(label_matches("arch", want, Some("arm64")), "{want} vs a Mac's arm64");
        assert!(label_matches("arch", want, Some("aarch64")), "{want} vs Linux ARM's aarch64");
        assert!(!label_matches("arch", want, Some("x86_64")), "{want} vs x86_64");
    }
    for want in ["x86_64", "amd64", "x64", "x86-64", "AMD64"] {
        assert!(label_matches("arch", want, Some("x86_64")), "{want}");
        assert!(!label_matches("arch", want, Some("arm64")), "{want}");
    }
    for want in ["macos", "darwin", "mac", "osx", "Darwin"] {
        assert!(label_matches("os", want, Some("macos")), "{want}");
        assert!(!label_matches("os", want, Some("linux")), "{want}");
    }
    assert!(label_matches("os", "Linux", Some("linux")) && !label_matches("os", "linux", Some("macos")));
    assert!(!label_matches("arch", "arm64", None), "no label: no match");
    assert!(label_matches("gpu", "nvidia", Some("nvidia")) && !label_matches("gpu", "NVIDIA", Some("nvidia")), "other labels are exact");
    assert!(!label_matches("gpu", "darwin", Some("macos")), "aliases only for os and arch");
}

fn with_tools(mut c: Cand, tools: &[(&str, &str)]) -> Cand {
    for (k, v) in tools {
        c.offer.tools.insert(k.to_string(), v.to_string());
    }
    c.offer.features.push(needs::FEATURE_NEEDS.to_string());
    c
}

const LINUX_RUST: &[(&str, &str)] = &[
    ("rust-toolchains", "1.99.1-x86_64-unknown-linux-gnu,stable-x86_64-unknown-linux-gnu"),
    ("cargo-nextest", "0.9.100"),
    ("mise:cmake", "3.30.2,3.31.1"),
    ("mise:node", "22.23.3"),
];

/// A protocol 7 node: host facts and this server's package view.
fn with_pkg(mut c: Cand, distro: &str, version: &str, mgr: &str, level: k2_node_proto::pkgpolicy::Level, approved: &[&str], installed: &[&str]) -> Cand {
    c.offer.host = Some(k2_node_proto::host::HostFacts {
        os: "linux".into(),
        arch: "x86_64".into(),
        distro: distro.into(),
        distro_like: if distro == "ubuntu" { vec!["debian".into()] } else { vec![] },
        distro_version: version.into(),
        distro_name: distro.into(),
        pkg: Some(mgr.into()),
        ..Default::default()
    });
    c.offer.pkg = Some(k2_node_proto::pkgpolicy::PkgStatus {
        manager: mgr.into(),
        level,
        profiles: if level == k2_node_proto::pkgpolicy::Level::Profiles { vec!["tauri-linux@1".into()] } else { vec![] },
        max_install_mb: 2048,
        approved: approved.iter().map(|s| s.to_string()).collect(),
        installed: installed.iter().map(|s| s.to_string()).collect(),
        index_age_secs: None,
        open_requests: 0,
    });
    c
}

/// prd-compute-self-serve-environments §5.3: the five states.
#[test]
fn package_states_follow_the_nodes_policy_and_map() {
    use k2_node_proto::pkgpolicy::Level;
    let toks = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ub = with_pkg(cand("ub", 32, 32, 0, 4), "ubuntu", "24.04", "apt", Level::List, &["libssl-dev"], &["zlib1g-dev"]);
    let got: Vec<(String, &str, Option<String>)> =
        need_states(&ub.offer, &toks(&["sys:zlib1g-dev", "sys:libssl-dev", "sys:libgtk-3-dev", "sys:openssh-server", "sys:pacman:protobuf"]))
            .into_iter()
            .map(|s| (s.need, s.state.as_str(), s.native))
            .collect();
    assert_eq!(
        got,
        vec![
            ("sys:zlib1g-dev".into(), "ready", Some("zlib1g-dev".into())),
            ("sys:libssl-dev".into(), "install", Some("libssl-dev".into())),
            ("sys:libgtk-3-dev".into(), "ask", Some("libgtk-3-dev".into())),
            ("sys:openssh-server".into(), "denied", Some("openssh-server".into())),
            ("sys:pacman:protobuf".into(), "no_mapping", None),
        ]
    );
    // Arch: the Debian name maps; one with no row is not satisfiable.
    let arch = with_pkg(cand("ar", 32, 32, 0, 4), "arch", "rolling", "pacman", Level::Official, &[], &[]);
    let s = need_states(&arch.offer, &toks(&["sys:libssl-dev", "sys:libfoo-dev"]));
    assert_eq!((s[0].state, s[0].native.as_deref()), (NeedState::Install, Some("openssl")));
    assert_eq!(s[1].state, NeedState::NoMapping);
    // Profiles level: the profile's names install, others ask.
    let pr = with_pkg(cand("pr", 32, 32, 0, 4), "ubuntu", "24.04", "apt", Level::Profiles, &[], &[]);
    let s = need_states(&pr.offer, &expand_profiles(&toks(&["profile:tauri-linux"])));
    assert!(s.iter().filter(|x| x.need.starts_with("sys:")).all(|x| x.state == NeedState::Install), "{s:?}");
    assert_eq!(need_states(&pr.offer, &toks(&["sys:libpq-dev"]))[0].state, NeedState::Ask);
    // An older node (no package status) installs as before.
    let old = cand("old", 32, 32, 0, 4);
    assert_eq!(need_states(&old.offer, &toks(&["sys:libgtk-3-dev"]))[0].state, NeedState::Install);
}

/// §5.3: auto-placement skips nodes that can't give a name, and nodes
/// that must ask while another can take the job; all asking: kept.
#[test]
fn placement_skips_package_gaps_with_reasons() {
    use k2_node_proto::pkgpolicy::Level;
    let mut w = want(1);
    w.auto = true;
    w.needs = vec!["sys:libgtk-3-dev".into()];
    let free = with_pkg(cand("free", 32, 32, 0, 4), "ubuntu", "24.04", "apt", Level::Official, &[], &[]);
    let asks = with_pkg(cand("asks", 32, 32, 0, 4), "ubuntu", "24.04", "apt", Level::List, &[], &[]);
    let mut mac = cand("mac", 32, 32, 0, 4);
    mac.offer.labels.insert("os".into(), "macos".into());
    let mut nomap = with_pkg(cand("nomap", 32, 32, 0, 4), "arch", "rolling", "pacman", Level::Official, &[], &[]);
    nomap.offer.pkg.as_mut().unwrap().approved.clear();
    let mut out = CandReport { cands: vec![free.clone(), asks.clone(), mac, nomap], ..Default::default() };
    w.needs = vec!["sys:libgtk-3-dev".into(), "sys:libfoo-dev".into()];
    skip_package_gaps(&w, &mut out);
    assert!(out.cands.iter().all(|c| c.node.id != "nomap" && c.node.id != "mac"));
    assert_eq!(out.skipped.get(skip::UNSUPPORTED_ON_OS), Some(&1));
    // libfoo-dev has no Arch row: nomap is skipped as no_package_mapping.
    assert_eq!(out.skipped.get(skip::NO_PACKAGE_MAPPING), Some(&1));
    assert_eq!(out.cands.iter().map(|c| c.node.id.as_str()).collect::<Vec<_>>(), vec!["free"], "the asking node waits while a free one fits");
    assert_eq!(out.skipped.get(skip::PACKAGE_ASK), Some(&1));
    // Every node must ask: they stay (submit files the request).
    let mut only = CandReport { cands: vec![asks], ..Default::default() };
    skip_package_gaps(&w, &mut only);
    assert_eq!(only.cands.len(), 1);
    assert!(skip_words(skip::PACKAGE_ASK, 1, &w).contains("owner's approval"));
}

/// §4.4, §8: `--distro`, `+minimal`, profiles and native names need a
/// protocol 7 node.
#[test]
fn protocol_seven_needs_ask_for_protocol_seven_nodes() {
    let n = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(wants_pkg_map(&n(&["profile:tauri-linux"])) && wants_pkg_map(&n(&["sys:pacman:protobuf"])));
    assert!(!wants_pkg_map(&n(&["sys:libssl-dev", "rust@stable"])));
    assert!(wants_minimal(&n(&["rust@stable+minimal"])) && !wants_components(&n(&["rust@stable+minimal"])));
    let mut w = want(1);
    w.needs = n(&["profile:build-base"]);
    assert!(required_features(&w).contains(&k2_node_proto::pkgmap::FEATURE_PKG_MAP));
    assert_eq!(check_needs(&n(&["rust@stable+minimal", "profile:tauri-linux@1"])).unwrap(), n(&["rust@stable+minimal", "profile:tauri-linux@1"]));
    let p = plan_packages(&n(&["profile:build-base", "sys:zlib1g-dev"]));
    assert!(p.contains(&"build-essential".to_string()) && p.contains(&"zlib1g-dev".to_string()));
    let e = expand_profiles(&n(&["profile:rust-gui-ci", "rust@1.99"]));
    assert!(e.contains(&"rust@1.99".to_string()) && !e.iter().any(|t| t.starts_with("rust@stable")), "an explicit need wins: {e:?}");
    assert!(e.contains(&"cargo-nextest".to_string()) && e.contains(&"sys:xvfb".to_string()));
    // A bare rust need expects clippy and rustfmt only from nodes that
    // install them by default.
    let mut c = with_tools(cand("n", 32, 32, 0, 4), LINUX_RUST);
    assert_eq!(need_states(&c.offer, &n(&["rust@stable"]))[0].state, NeedState::Ready, "an older node: unchanged");
    c.offer.features.push(FEATURE_RUST_DEFAULT_COMPONENTS.into());
    assert_eq!(need_states(&c.offer, &n(&["rust@stable"]))[0].state, NeedState::Install, "clippy and rustfmt still to add");
    assert_eq!(need_states(&c.offer, &n(&["rust@stable+minimal"]))[0].state, NeedState::Ready);
    assert_eq!(FEATURE_RUST_DEFAULT_COMPONENTS, k2_node_proto::needs::FEATURE_RUST_DEFAULT_COMPONENTS);
}

/// CE-1: each need's state from the offer's tools map.
#[test]
fn need_states_read_the_offers_tools() {
    let c = with_tools(cand("n", 32, 32, 0, 4), LINUX_RUST);
    let toks: Vec<String> =
        ["rust@stable", "rust@1.99.1", "rust@1.99", "rust@nightly", "cargo-nextest", "cmake@3", "cmake@3.31", "cmake@3.3", "node@lts", "python@3.12", "sys:pkg-config"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let got: Vec<(String, &str, Option<String>)> = need_states(&c.offer, &toks).into_iter().map(|s| (s.need, s.state.as_str(), s.have)).collect();
    let want = |n: &str, st: &'static str, h: Option<&str>| (n.to_string(), st, h.map(str::to_string));
    assert_eq!(
        got,
        vec![
            want("rust@stable", "ready", Some("stable-x86_64-unknown-linux-gnu")),
            want("rust@1.99.1", "ready", Some("1.99.1-x86_64-unknown-linux-gnu")),
            want("rust@1.99", "install", None),
            want("rust@nightly", "install", None),
            want("cargo-nextest", "ready", Some("0.9.100")),
            want("cmake@3", "ready", Some("3.31.1")),
            want("cmake@3.31", "ready", Some("3.31.1")),
            want("cmake@3.3", "install", None),
            want("node@lts", "ready", Some("22.23.3")),
            want("python@3.12", "install", None),
            want("sys:pkg-config", "install", None),
        ]
    );
    assert!(!toolchain_is("nightly-2026-09-01-x86_64-unknown-linux-gnu", "nightly"), "a dated nightly isn't the nightly channel");
    assert!(toolchain_is("nightly-2026-09-01-x86_64-unknown-linux-gnu", "nightly-2026-09-01"));
    // Components: ready only when the node lists each for that toolchain.
    let comps = vec!["rust@stable+clippy+llvm-tools".to_string()];
    assert_eq!(need_states(&c.offer, &comps)[0].state, NeedState::Install, "no rust-components report (older node)");
    let mut d = c.clone();
    d.offer.tools.insert("rust-components:stable-x86_64-unknown-linux-gnu".into(), "cargo,clippy,llvm-tools-preview,rustc".into());
    assert_eq!(need_states(&d.offer, &comps)[0].state, NeedState::Ready);
    // macOS: a sys: need is unsupported; an older node with no tools
    // map installs everything.
    let mut mac = c.clone();
    mac.offer.labels.insert("os".into(), "macos".into());
    assert_eq!(need_states(&mac.offer, &toks[10..])[0].state, NeedState::UnsupportedOnOs);
    let bare = cand("old", 32, 32, 0, 4);
    assert!(need_states(&bare.offer, &toks).iter().all(|s| s.state == NeedState::Install));
    // Stable JSON keys (CE-2 acceptance 3).
    let j = need_states(&c.offer, &["cmake@4".to_string()])[0].json();
    assert_eq!(j, serde_json::json!({"need": "cmake@4", "state": "install", "have": null}));
    assert_eq!(tools_words(&need_states(&c.offer, &["rust@stable".to_string(), "cmake@4".to_string(), "python@3".to_string()])).unwrap(), "installs cmake@4, python@3 first (~minutes)");
    assert_eq!(tools_words(&need_states(&c.offer, &["rust@stable".to_string()])).unwrap(), "tools ready");
    assert_eq!(tools_words(&[]), None);
}

/// CE-1 acceptance 1–3: tools ready is worth a warm slot's band; a
/// heavy ready node still loses to an idle one that must install; a
/// `sys:` need ranks a Mac last.
#[test]
fn placement_prefers_a_node_with_the_tools_ready() {
    let mut w = want(2);
    w.needs = vec!["rust@stable".into(), "cargo-nextest".into()];
    let bare = with_tools(cand("a-bare", 32, 32, 0, 4), &[]);
    let ready = with_tools(cand("b-ready", 32, 32, 0, 4), LINUX_RUST);
    assert_eq!(pick(V2, &[bare.clone(), ready.clone()], &w).0, "b-ready", "both idle: tools ready wins");
    let cands = [bare.clone(), ready.clone()];
    let ex = Explain { policy: V2, cands: &cands, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&ready, Tier::Now, &w, &ex, 1);
    assert!(rec.why.starts_with("starts now · idle · tools ready · "), "{}", rec.why);
    let rec = placement_record(&bare, Tier::Now, &w, &ex, 1);
    assert!(rec.why.contains("installs rust@stable, cargo-nextest first (~minutes)"), "{}", rec.why);
    // One-band rule: ready but heavy loses to idle that must install.
    let heavy = with_tools(cand("a-heavy-ready", 4, 32, 3, 4), LINUX_RUST);
    let idle = with_tools(cand("b-idle-bare", 32, 32, 0, 4), &[]);
    assert_eq!(heavy.band(&w), 2);
    assert_eq!(pick(V2, &[heavy, idle], &w).0, "b-idle-bare");
    // sys: on a Mac ranks it last, never filters it.
    let mut s = want(2);
    s.needs = vec!["sys:pkg-config".into()];
    let mut mac = with_tools(cand("a-mac", 32, 32, 0, 4), &[]);
    mac.offer.labels.insert("os".into(), "macos".into());
    let linux = with_tools(cand("b-linux", 32, 32, 0, 4), &[]);
    assert_eq!(pick(V2, &[mac.clone(), linux], &s).0, "b-linux");
    assert_eq!(pick(V2, &[mac], &s).0, "a-mac", "alone it is still a candidate");
    // v1 ignores tools: its order is unchanged (the list order breaks the tie).
    assert_eq!(pick(V1, &[bare, ready], &w).0, "a-bare");
}

/// CE-6: the node that holds this workspace's `--cache` ranks first
/// among nodes that start now (warmth 3), even over an idle one; the
/// record says so; v1 ignores it.
#[test]
fn the_node_that_holds_the_cache_ranks_first() {
    let mut w = want(2);
    w.cache = Some("fz".into());
    let idle = cand("a-idle", 32, 32, 0, 4);
    let mut held = cand("b-held", 20, 32, 2, 4);
    held.holds_cache = true;
    assert_eq!(pick(V2, &[idle.clone(), held.clone()], &w), ("b-held".into(), Tier::Now));
    assert_eq!(warmth_v2(&held, &w, None, None), 3);
    let cands = [idle.clone(), held.clone()];
    let ex = Explain { policy: V2, cands: &cands, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&held, Tier::Now, &w, &ex, 1);
    assert_eq!(rec.code, "starts_now_cache");
    assert!(rec.why.starts_with("starts now · holds cache fz · "), "{}", rec.why);
    assert_eq!(pick(V1, &[idle.clone(), held.clone()], &w).0, "a-idle", "v1: the 0.46.5 order");
    // Without --cache, holding one means nothing.
    assert_eq!(pick(V2, &[idle, held], &want(2)).0, "a-idle");
    // The sweeper keeps a job queued for its cache for 15 min.
    let mut j = JobRow::default();
    j.ctl.placement = Some(store::Placement { code: QUEUED_FOR_CACHE.into(), ..Default::default() });
    assert!(held_for_cache(&j, 1000, 1000 + CACHE_QUEUE_SECS as i64 - 1));
    assert!(!held_for_cache(&j, 1000, 1000 + CACHE_QUEUE_SECS as i64));
}

/// CE-15: the hints follow --cpus (rounded up), else the node's default
/// reservation (12 CPUs, 3 slots → 4); a caller's own value wins; a
/// move recomputes them for the new node.
#[test]
fn cpu_hints_follow_the_jobs_cpus() {
    let mut node = cand("n", 32, 32, 0, 3).offer;
    node.caps.cpu_millis = 12_000;
    assert_eq!(job_cpus(Some(3500), Some(&node)), Some(4), "--cpus 3.5 → 4");
    assert_eq!(job_cpus(None, Some(&node)), Some(4), "12 CPUs / 3 slots");
    assert_eq!(job_cpus(None, None), None);
    let mut env = BTreeMap::from([("CARGO_BUILD_JOBS".to_string(), "2".to_string())]);
    let set = set_cpu_hints(&mut env, &[], &[], Some(4));
    assert_eq!(set, vec!["RUST_TEST_THREADS", "NEXTEST_TEST_THREADS", "OMP_NUM_THREADS"]);
    assert_eq!((env["CARGO_BUILD_JOBS"].as_str(), env["OMP_NUM_THREADS"].as_str()), ("2", "4"), "--env CARGO_BUILD_JOBS=2 wins");
    assert!(set_cpu_hints(&mut BTreeMap::new(), &[], &["OMP_NUM_THREADS".to_string()], Some(2)).iter().all(|n| n != "OMP_NUM_THREADS"), "a secret's name is left alone");
    // A move to a 16-CPU node with 2 slots: 8 each.
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {"CARGO_BUILD_JOBS": "4", "RUST_TEST_THREADS": "4", "NEXTEST_TEST_THREADS": "4", "OMP_NUM_THREADS": "4", "MINE": "x"}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1
    }))
    .expect("a plan");
    let ctl = JobCtl { cpu_hints: CPU_HINT_VARS.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    let mut big = cand("big", 64, 64, 0, 2).offer;
    big.caps.cpu_millis = 16_000;
    let moved = plan_for_node(&plan, &ctl, JobTier::Normal, &node_row("big"), None, &big);
    assert_eq!((moved.env["CARGO_BUILD_JOBS"].as_str(), moved.env["OMP_NUM_THREADS"].as_str(), moved.env["MINE"].as_str()), ("8", "8", "x"));
}

/// P2: the solo-node wait estimate, and busy nodes ranked by it.
#[test]
fn a_solo_nodes_wait_is_estimated() {
    assert_eq!(solo_eta(&[Some(600), None], &[Some(120), Some(300)], 2), (600 + UNKNOWN_EST_SECS) / 2 + 120);
    assert_eq!(solo_eta(&[], &[None], 4), UNKNOWN_EST_SECS, "an unknown running job: 15 min");
    assert_eq!(solo_eta(&[], &[], 4), 0);
    let w = want(2);
    let mut solo = cand("a-solo", 30, 32, 2, 2);
    solo.est_wait = Some(200);
    let quoted = shared(cand("b-quoted", 30, 32, 2, 2), Some((false, Some(900))));
    let mut unknown = cand("c-unknown", 30, 32, 2, 2);
    unknown.est_wait = None;
    assert_eq!(order(V2, &[unknown.clone(), quoted.clone(), solo.clone()], &w), vec!["a-solo", "b-quoted", "c-unknown"]);
    assert_eq!(order(V1, &[unknown.clone(), quoted.clone(), solo.clone()], &w)[0], "b-quoted", "v1 knows only quotes");
    let all = [solo.clone(), quoted, unknown];
    let ex = Explain { policy: V2, cands: &all, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&solo, Tier::Busy, &w, &ex, 1);
    assert_eq!((rec.code.as_str(), rec.eta_secs), ("queued_est_wait", Some(200)));
    assert!(rec.why.contains("~200s (estimate)") || rec.why.contains("~4 min (estimate)"), "{}", rec.why);
    // --plan's ranking: top three, stable keys.
    let ranked = rank(V2, &all, &w, &[], None, None);
    let r = ranking_json(&ranked, &w, None, None);
    assert_eq!(r.as_array().unwrap().len(), 3);
    assert_eq!(r[0]["node"], "a-solo");
    assert_eq!((r[0]["state"].as_str(), r[0]["etaSecs"].as_u64(), r[0]["etaBasis"].as_str()), (Some("busy"), Some(200), Some("estimate")));
    assert_eq!((r[1]["etaBasis"].as_str(), r[2]["etaBasis"].as_str()), (Some("quote"), Some("unknown")));
    for k in ["node", "state", "band", "warmth", "etaSecs", "etaBasis", "needs"] {
        assert!(r[2].get(k).is_some(), "{k} in {}", r[2]);
    }
}

/// CE-16: `rust@<channel>+component` splits on `+` (a comma would split
/// the list), checks each component, and needs a node offering
/// `rust-components`; `+` in any other token stays as it is.
#[test]
fn rust_components_ride_on_the_rust_need() {
    assert_eq!(check_needs(&["rust@stable+clippy+rustfmt", "cmake@3"]).unwrap(), vec!["rust@stable+clippy+rustfmt", "cmake@3"]);
    assert_eq!(check_needs(&["rust+llvm-tools-preview+clippy+clippy"]).unwrap(), vec!["rust@stable+clippy+llvm-tools"], "alias, default channel, dedupe, sorted as k2-node writes it");
    assert_eq!(check_needs(&["rust@nightly"]).unwrap(), vec!["rust@nightly"]);
    let e = check_needs(&["rust@stable+cargo-fmt"]).unwrap_err();
    assert_eq!(e.code, needs::NEED_INVALID);
    assert!(e.message.contains("clippy, rustfmt, rust-src, llvm-tools, rust-analyzer, miri"), "{}", e.message);
    assert_eq!(check_needs(&["rust@1.99+"]).unwrap_err().code, needs::NEED_INVALID, "an empty component");
    assert_eq!(check_needs(&["rust@stable+clippy", "rust@nightly"]).unwrap_err().code, needs::NEED_INVALID, "rust twice");
    assert_eq!(split_rust_components("sys:g++").unwrap(), ("sys:g++".to_string(), vec![]), "+ in a package name");
    assert_eq!(need_components("rust@stable+miri"), vec!["miri"]);
    let mut w = want(2);
    w.needs = vec!["rust@stable".into()];
    assert!(!required_features(&w).contains(&FEATURE_RUST_COMPONENTS));
    w.needs = vec!["rust@stable+clippy".into()];
    assert!(required_features(&w).contains(&FEATURE_RUST_COMPONENTS), "only nodes that install components");
}

/// `+target:<triple>` rides on the Rust need beside components
/// (repeatable, sorted, unique, checked), needs a node offering
/// `rust-targets`, is ready only when the node lists each target for
/// that toolchain, and is `unsupported` where Rust is a system package
/// with no rustup (the tools line says so).
#[test]
fn rust_targets_ride_on_the_rust_need() {
    assert_eq!(
        check_needs(&["rust@1.95.0+target:wasm32-unknown-unknown", "cmake@3"]).unwrap(),
        vec!["rust@1.95.0+target:wasm32-unknown-unknown", "cmake@3"]
    );
    assert_eq!(
        check_needs(&["rust@stable+target:wasm32-wasip1+clippy+target:aarch64-unknown-linux-gnu+target:wasm32-wasip1"]).unwrap(),
        vec!["rust@stable+clippy+target:aarch64-unknown-linux-gnu+target:wasm32-wasip1"],
        "components first, then targets, each sorted and unique, as k2-node writes it"
    );
    let e = check_needs(&["rust@stable+target:wasm32"]).unwrap_err();
    assert_eq!(e.code, needs::NEED_INVALID);
    assert!(e.message.contains("isn't a Rust target triple") && e.message.contains("rust@1.95.0+target:wasm32-unknown-unknown"), "{}", e.message);
    assert_eq!(check_needs(&["rust@stable+target:"]).unwrap_err().code, needs::NEED_INVALID);
    let nine: String = (0..9).map(|i| format!("+target:t{i}-unknown-none")).collect();
    assert!(check_needs(&[format!("rust@stable{nine}")]).unwrap_err().message.contains("at most 8 targets"));
    assert_eq!(need_targets("rust@stable+clippy+target:wasm32-unknown-unknown"), vec!["wasm32-unknown-unknown"]);
    assert_eq!(need_components("rust@stable+clippy+target:wasm32-unknown-unknown"), vec!["clippy"]);
    assert!(need_targets("rust@stable+clippy").is_empty() && need_targets("sys:g++").is_empty());
    // Features: targets alone need rust-targets, not rust-components.
    let mut w = want(2);
    w.needs = vec!["rust@1.95.0+target:wasm32-unknown-unknown".into()];
    let f = required_features(&w);
    assert!(f.contains(&needs::FEATURE_RUST_TARGETS) && !f.contains(&FEATURE_RUST_COMPONENTS), "{f:?}");
    w.needs = vec!["rust@1.95.0+clippy".into()];
    assert!(!required_features(&w).contains(&needs::FEATURE_RUST_TARGETS));
    // States.
    let c = with_tools(cand("n", 32, 32, 0, 4), &[("rust-toolchains", "1.95.0-x86_64-unknown-linux-gnu"), ("rust-installer", "rustup")]);
    let tok = vec!["rust@1.95.0+target:wasm32-unknown-unknown".to_string()];
    assert_eq!(need_states(&c.offer, &tok)[0].state, NeedState::Install, "no rust-targets report yet");
    let mut d = c.clone();
    d.offer.tools.insert("rust-targets:1.95.0-x86_64-unknown-linux-gnu".into(), "wasm32-unknown-unknown,x86_64-unknown-linux-gnu".into());
    let st = &need_states(&d.offer, &tok)[0];
    assert_eq!((st.state, st.have.as_deref()), (NeedState::Ready, Some("1.95.0-x86_64-unknown-linux-gnu")));
    assert_eq!(need_states(&d.offer, &["rust@1.95.0+target:wasm32-wasip1".to_string()])[0].state, NeedState::Install);
    let mut sys = with_tools(cand("arch", 32, 32, 0, 4), &[("rust-installer", "system")]);
    assert!(system_rust(&sys.offer) && !system_rust(&d.offer));
    let s = &need_states(&sys.offer, &tok)[0];
    assert_eq!((s.state, s.state.as_str()), (NeedState::Unsupported, "unsupported"));
    assert_eq!(need_states(&sys.offer, &["rust@1.95.0".to_string()])[0].state, NeedState::Install, "a plain Rust need is still fine there");
    assert_eq!(
        tools_words(&need_states(&sys.offer, &tok)).unwrap(),
        "rust@1.95.0+target:wasm32-unknown-unknown: this node's Rust is a system package with no rustup (no Rust targets here)"
    );
    sys.offer.tools.remove("rust-installer");
    assert_eq!(need_states(&sys.offer, &tok)[0].state, NeedState::Install, "no report (older node): not ruled out by this");
    assert_eq!(skip_words(skip::TOOLS_UNSUPPORTED, 1, &w), "1 that can't provide a --needs (system Rust: no Rust targets)");
}

#[test]
fn awake_since_restarts_after_a_long_gap() {
    note_tick(1_000);
    note_tick(1_005);
    assert_eq!(awake_since(1_005), 1_000, "a normal tick keeps the start");
    note_tick(5_000);
    assert_eq!(awake_since(5_000), 5_000, "a gap over a minute = slept");
}

/// R2 (§4.4.4, BK-13): `--cpus 4+8` reaches a node offering `cpu-burst`
/// as `cpus_burst` (the guarantee stays `limits.cpu_millis`); a node
/// without it gets today's plan and digest; a move back to a newer node
/// gets it again.
#[test]
fn the_burst_follows_the_nodes_features() {
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": 4000, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1
    }))
    .expect("a plan");
    let mut job = JobRow { id: "j1".into(), tier: "normal".into(), ..Default::default() };
    job.ctl.cpus_burst = Some(8000);
    let new_node = vec![k2_core::compute::proto::gens::FEATURE_CPU_BURST.to_string()];
    let burst = fit_plan(&plan, &job, &new_node, None).expect("a cpu-burst node gets the burst");
    assert_eq!((burst.cpus_burst, burst.limits.cpu_millis), (Some(8000), Some(4000)));
    assert_ne!(burst.digest().unwrap(), plan.digest().unwrap());
    assert_eq!(fit_plan(&burst, &job, &new_node, None), None, "already fits");
    let back = fit_plan(&burst, &job, &[], None).expect("an older node loses it");
    assert_eq!((back.cpus_burst, back.digest().unwrap()), (None, plan.digest().unwrap()), "today's digest, byte for byte");
    assert_eq!(fit_plan(&plan, &job, &[], None), None, "no feature: as it is");
    job.ctl.cpus_burst = None;
    assert_eq!(fit_plan(&plan, &job, &new_node, None), None, "no burst asked: nothing to send");
}

/// Job tiers T11, T17 (mixed versions): a tiered plan sent to a node
/// without `tiers` (an older k2-node, or one rolled back to 0.46.4)
/// loses its tier and gets the untiered digest back, so that node takes
/// it; a node offering `tiers` gets the job's own tier; a plan that
/// already fits is left alone.
#[test]
fn the_tier_follows_the_nodes_features() {
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1, "class": "interactive"
    }))
    .expect("a plan");
    let job = JobRow { id: "j1".into(), tier: "priority".into(), ..Default::default() };
    let new_node = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
    let old_node = vec![pshare::FEATURE_QUEUE.to_string()];
    assert_eq!(fit_plan(&plan, &job, &old_node, None), None, "no tier, an old node: as it is");
    let tiered = fit_plan(&plan, &job, &new_node, None).expect("a node with tiers gets the tier");
    assert_eq!(tiered.tier, Some(JobTier::Priority));
    assert_ne!(tiered.digest().unwrap(), plan.digest().unwrap());
    assert_eq!(fit_plan(&tiered, &job, &new_node, None), None, "already fits");
    let back = fit_plan(&tiered, &job, &old_node, None).expect("a rolled-back node loses the tier");
    assert_eq!((back.tier, back.digest().unwrap()), (None, plan.digest().unwrap()), "the untiered digest, byte for byte");
    assert!(offers_tiers(&new_node) && !offers_tiers(&old_node));
    // A v0.46.2 node (no `queue`, no `stall`): every field it doesn't
    // know goes, so its digest is the one it computes (the tiers
    // rollback drill: `plan_mismatch` before this).
    let mut full = tiered.clone();
    full.ws_max_running = Some(2);
    full.stall_secs = Some(600);
    let bare = fit_plan(&full, &job, &[], Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
    assert_eq!((bare.tier, bare.class, bare.ws_max_running, bare.stall_secs), (None, None, None, None));
    let mut want = plan.clone();
    want.class = None;
    assert_eq!(bare.digest().unwrap(), want.digest().unwrap(), "what a v0.46.2 node computes");
    assert_eq!(fit_plan(&bare, &job, &[], Some(GrantCaps { ws: 3, submitter: 0 })), None);
}

/// prd-compute-job-tiers-v2 L5 (T23, 10-27): the workspace cap in a
/// plan follows the grant at every send. A plan stored with cap 1 goes
/// out with the grant's current 3 to a node with a queue; a node
/// without one never gets the field; the same cap again is no change.
#[test]
fn the_workspace_cap_is_refreshed_from_the_grant_before_every_send() {
    let mut plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1, "class": "interactive"
    }))
    .expect("a plan");
    plan.ws_max_running = Some(1);
    let job = JobRow { id: "j1".into(), tier: "normal".into(), ..Default::default() };
    let queue_node = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
    let fresh = fit_plan(&plan, &job, &queue_node, Some(GrantCaps { ws: 3, submitter: 0 })).expect("the stale cap is rewritten");
    assert_eq!(fresh.ws_max_running, Some(3));
    assert_ne!(fresh.digest().unwrap(), plan.digest().unwrap(), "a new digest: the row is rewritten before the send");
    assert_eq!(fit_plan(&fresh, &job, &queue_node, Some(GrantCaps { ws: 3, submitter: 0 })), None, "already current");
    let lowered = fit_plan(&fresh, &job, &queue_node, Some(GrantCaps { ws: 1, submitter: 0 })).expect("a lowered grant reaches it too");
    assert_eq!(lowered.ws_max_running, Some(1));
    let solo = fit_plan(&plan, &job, &[FEATURE_TIERS.to_string()], Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
    assert_eq!(solo.ws_max_running, None, "a node without a queue never gets the field");
}

/// prd-compute-job-tiers-v2 T27/T30: Batch and the submitter fields go
/// only to a `tiers2` node; a `tiers`-only (0.46.5) node gets Normal
/// and no submitter, so its parser and digest are unchanged; a 0.46.4
/// node orders it by class `batch`.
#[test]
fn batch_and_the_submitter_ride_only_to_tiers2_nodes() {
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1, "class": "batch"
    }))
    .expect("a plan");
    let job = JobRow { id: "j1".into(), tier: "batch".into(), class: "batch".into(), submitter: Some("k2/abcd1234/builder-a".into()), ..Default::default() };
    let caps = Some(GrantCaps { ws: 3, submitter: 2 });
    let v2 = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string(), k2_core::compute::proto::tiers::FEATURE_TIERS2.to_string()];
    let p = fit_plan(&plan, &job, &v2, caps).expect("fitted");
    assert_eq!((p.tier, p.submitter.as_deref(), p.submitter_max_running), (Some(JobTier::Batch), Some("k2/abcd1234/builder-a"), Some(2)));
    let v1 = vec![FEATURE_TIERS.to_string(), pshare::FEATURE_QUEUE.to_string()];
    let p = fit_plan(&plan, &job, &v1, caps).expect("fitted");
    assert_eq!((p.tier, p.submitter.as_deref(), p.submitter_max_running), (Some(JobTier::Normal), None, None));
    let wire = serde_json::to_value(&p).unwrap();
    assert!(wire.get("submitter").is_none() && wire.get("submitter_max_running").is_none(), "{wire}");
    assert_eq!(wire["tier"], "normal", "a 0.46.5 parser never sees `batch`");
    let p = fit_plan(&plan, &job, &[pshare::FEATURE_QUEUE.to_string()], caps).unwrap_or(plan.clone());
    assert_eq!((p.tier, p.class), (None, Some(pshare::JobClass::Batch)), "0.46.4: class batch");
    // No cap on the grant: no field.
    let p = fit_plan(&plan, &job, &v2, Some(GrantCaps { ws: 3, submitter: 0 })).expect("fitted");
    assert_eq!(p.submitter_max_running, None);
}

// ── the `--any` mover (a waiting job moves when a node can start it) ──

fn mover_pick(cands: &[Cand], w: &Want, pinned: bool) -> Option<String> {
    move_target(pinned, compute::PlacementPolicy::V2, cands, w, &[], None, None).map(|c| c.node.id.clone())
}

/// A job waiting on a full node (the 20 GB one asked here can't start
/// on the busy node) moves to the node with a free slot and the memory.
#[test]
fn a_waiting_auto_job_moves_to_a_node_with_a_free_fitting_slot() {
    let cands = vec![cand("busy", 4, 32, 2, 2), cand("free", 30, 32, 0, 2)];
    assert_eq!(mover_pick(&cands, &want(20), false).as_deref(), Some("free"));
}

/// A free slot without the memory is not a place to move to: slots
/// alone don't say a 28 GB job fits (the 52 GB nodes with one job on).
#[test]
fn a_free_slot_without_the_memory_is_not_a_move() {
    let cands = vec![cand("one-running", 24, 52, 1, 3), cand("small", 23, 23, 0, 1)];
    assert_eq!(mover_pick(&cands, &want(28), false), None);
}

#[test]
fn a_pinned_job_never_moves() {
    let cands = vec![cand("free", 30, 32, 0, 2)];
    assert_eq!(mover_pick(&cands, &want(20), true), None);
}

#[test]
fn the_mover_never_picks_an_avoided_node() {
    let cands = vec![cand("busy", 4, 32, 2, 2), cand("free", 30, 32, 0, 2), cand("free-2", 30, 32, 0, 2)];
    let mut w = want(20);
    w.filter.avoid = vec!["free".into()];
    assert_eq!(mover_pick(&cands, &w, false).as_deref(), Some("free-2"));
    w.filter.avoid.push("free-2".into());
    assert_eq!(mover_pick(&cands, &w, false), None, "only avoided nodes are free");
}

#[test]
fn the_mover_keeps_to_an_only_list() {
    let cands = vec![cand("free", 30, 32, 0, 2), cand("listed", 30, 32, 1, 2)];
    let mut w = want(20);
    w.filter.only = vec!["listed".into()];
    assert_eq!(mover_pick(&cands, &w, false).as_deref(), Some("listed"));
}

/// A node whose k2-node can't give the job's `--needs` (here: Rust
/// components, which need the `rust-components` feature) is never a
/// target, however free it is.
#[test]
fn the_mover_never_picks_a_node_that_cant_give_the_needs() {
    let mut w = want(20);
    w.needs = vec!["rust@stable+clippy".into()];
    let lacking = cand("lacking", 30, 32, 0, 2);
    assert_eq!(mover_pick(std::slice::from_ref(&lacking), &w, false), None);
    let mut able = cand("able", 30, 32, 1, 2);
    able.offer.features.push(needs::FEATURE_NEEDS.to_string());
    able.offer.features.push(FEATURE_RUST_COMPONENTS.to_string());
    assert_eq!(mover_pick(&[lacking, able], &w, false).as_deref(), Some("able"));
}

/// The move line names both nodes and why.
#[test]
fn a_move_says_where_from_and_why() {
    assert_eq!(moved_why("node-c", "node-d", Tier::Now, "starts now · idle · 56/56 GB"), "moved from node-c: node-d can start it now · idle · 56/56 GB");
    assert_eq!(moved_why("a", "b", Tier::Busy, "no node free now"), "moved from a: no node free now");
}

/// A mover's hold on the target counts as a sent job until the node
/// answers: a newer offer from the target doesn't drop it (only the
/// answer or 60 s do), so a new submit can't take the place first.
#[test]
fn a_withdraw_hold_keeps_the_targets_place_until_dropped() {
    let node = "hold-test-node";
    let now = 2_000_000;
    note_hold(node, "hold-job-1", Some(20 * GB), Some(4000), now);
    let p = pending_for(node, Some(99), now + 5);
    assert_eq!(p, Pending { mem_bytes: 20 * GB, cpu_millis: 4000, jobs: 1, holds: 1 }, "a newer offer keeps the hold");
    let mut c = cand(node, 30, 32, 0, 1);
    c.pending = p;
    assert!(!c.starts_now(&want(8)), "the one slot is held");
    assert_eq!(c.tier(&want(8)), Some(Tier::Busy));
    let other_busy = cand("hold-other-busy", 30, 32, 1, 1);
    for policy in [compute::PlacementPolicy::V1, compute::PlacementPolicy::V2] {
        let both = [c.clone(), other_busy.clone()];
        let (first, _) = choose_by(policy, &both, &want(8), &[], None, None).expect("a pick");
        assert_eq!(first.node.id, "hold-other-busy", "{policy:?}: a job that can't start now waits elsewhere, not behind the moving one");
    }
    let mut roomy = cand("hold-roomy", 30, 32, 0, 2);
    roomy.pending = p;
    assert_eq!(roomy.tier(&want(8)), Some(Tier::Now), "a hold takes one place, not the node");
    // In flight: kept. Not in flight any more: dropped.
    keep_holds(&HashSet::from(["hold-job-1".to_string()]));
    assert_eq!(pending_for(node, Some(99), now + 6).jobs, 1);
    keep_holds(&HashSet::new());
    assert_eq!(pending_for(node, Some(99), now + 6).jobs, 0);
    // drop_hold ends it at once and leaves a sent job's ask alone.
    note_pending(node, "sent-job", Some(GB), None, 7, now);
    note_hold(node, "hold-job-2", Some(GB), None, now);
    drop_hold("hold-job-2");
    assert_eq!(pending_for(node, Some(7), now).jobs, 1);
    assert_eq!(pending_for(node, None, now + PENDING_SECS).jobs, 0);
}

/// On a shared node a sent job (or a hold) takes one of this server's
/// running slots until the next offer, like on a solo node.
#[test]
fn a_pending_job_takes_a_shared_nodes_running_slot() {
    let mut c = shared(cand("shared-pend", 30, 32, 0, 2), None);
    let max = c.shared_view().expect("shared").limits.max_running;
    c.offer.share.as_mut().expect("share").running = max - 1;
    assert!(c.starts_now(&want(2)), "one slot left");
    c.pending = Pending { mem_bytes: GB, cpu_millis: 0, jobs: 1, holds: 0 };
    assert!(!c.starts_now(&want(2)), "the pending job has it");
}

/// The JSON a waiting job shows its own workspace: pinned or auto, and
/// after a bounce the count and the next try.
#[test]
fn a_waiting_job_says_whether_and_when_it_may_move() {
    let now = 10_000;
    let mut j = JobRow { state: "queued".into(), ..Default::default() };
    let pinned = move_state_json(&j, now).expect("waiting");
    assert_eq!(pinned["auto"], false);
    j.ctl.any = Some(AnyFilter::default());
    let fresh = move_state_json(&j, now).expect("waiting");
    assert_eq!((fresh["auto"].as_bool(), fresh["bounces"].as_i64(), fresh["nextTryInSecs"].as_i64()), (Some(true), Some(0), Some(0)));
    j.withdraws = 2;
    j.withdrawn_at = Some(now - 20);
    let bounced = move_state_json(&j, now).expect("waiting");
    assert_eq!((bounced["bounces"].as_i64(), bounced["nextTryInSecs"].as_i64()), (Some(2), Some(100)));
    assert!(bounced["note"].as_str().expect("note").contains("next try in 100s"), "{bounced}");
    j.state = "running".into();
    assert!(move_state_json(&j, now).is_none(), "a started job has no mover state");
}

/// 11-19: a job was placed "starts now" on a shared node its owner was
/// draining for an update, and sat there over 3 hours. A node its
/// owner drains or pauses never starts a job now, whatever its quote
/// says; it is no target for placement or the mover (an offline node
/// last seen so is no fallback either); a busy node wins over it, and
/// the why-not line names the drain.
#[test]
fn a_draining_or_paused_node_never_starts_now_or_gets_an_auto_job() {
    use compute::PlacementPolicy::{V1, V2};
    let w = want(2);
    for control in [frames::Control::Draining, frames::Control::Paused] {
        let name = format!("{control:?}").to_lowercase();
        // Shared, idle, its quote says "starts now" (the field case).
        let mut sh = shared(cand("held-shared", 30, 32, 0, 2), Some((true, None)));
        assert!(sh.starts_now(&w), "{name}: active, the same node starts it now");
        sh.offer.control = control;
        assert!(!sh.starts_now(&w), "{name}: a shared node's quote never overrides its owner's {name}");
        assert_eq!(sh.tier(&w), None, "{name}: shared");
        let (code, words) = why_not::reason(&sh, &w).expect("a reason");
        assert_eq!(code, why_not::UNAVAILABLE, "{name}: {words}");
        assert!(words.contains(&format!("the node is {name}")), "{words}");
        // Solo, idle.
        let mut solo = cand("held-solo", 30, 32, 0, 2);
        solo.offer.control = control;
        assert!(!solo.starts_now(&w), "{name}: solo");
        assert_eq!(solo.tier(&w), None, "{name}: solo");
        // Offline, last seen held: no fallback.
        let mut off = cand("held-off", 30, 32, 0, 2);
        off.online = false;
        off.offer.control = control;
        assert_eq!(off.tier(&w), None, "{name}: offline, last seen {name}");
        // A full node (2 of 2 running) is only busy: it wins.
        let busy = cand("busy", 30, 32, 2, 2);
        assert_eq!(busy.tier(&w), Some(Tier::Busy));
        let cands = vec![sh.clone(), solo.clone(), off.clone(), busy];
        for policy in [V1, V2] {
            let ranked = rank(policy, &cands, &w, &[], None, None);
            let names: Vec<(&str, Tier)> = ranked.iter().map(|(c, t)| (c.node.id.as_str(), *t)).collect();
            assert_eq!(names, vec![("busy", Tier::Busy)], "{name} {policy:?}: only the busy node is a target");
        }
        // The mover never takes a job there, hard reason or not.
        let held = [sh, solo, off];
        assert!(move_target(false, V2, &held, &w, &[], None, None).is_none(), "{name}");
        assert!(leave_target(true, V2, &held, &w, &[], None, None).is_none(), "{name}");
    }
}

/// A node's control in words and codes: the wait a pinned job shows
/// (with who drains it), the mover's hard reason.
#[test]
fn a_held_node_says_why_and_who() {
    let mut o = cand("n", 30, 32, 0, 2).offer;
    assert!(control_wait(&o).is_none());
    assert_eq!(held_code(o.control), None);
    o.control = frames::Control::Draining;
    o.control_by = Some("k2-web-test".into());
    let r = control_wait(&o).expect("a wait");
    assert_eq!(r.code, refusal::LOCAL_DRAINING);
    assert_eq!(r.message, "the machine's owner is draining this node (by k2-web-test)");
    assert_eq!(held_code(o.control), Some("node_draining"));
    assert_eq!(held_code(frames::Control::Paused), Some("node_paused"));
    assert_eq!(held_code(frames::Control::Stopped), Some("node_stopped"));
    o.control_by = None;
    assert_eq!(control_wait(&o).expect("a wait").message, "the machine's owner is draining this node");
}

/// The stall guard's default by tier: Priority and CI with no
/// `--stall-timeout` get the longer 2 h (not none), Standard and Batch keep
/// 30 min. An ask wins, `off` is off, a stall not shorter than the job's
/// `--timeout` is none (the wall clock ends it first), and a node without
/// `stall` gets none. A move re-plans with the row's tier.
#[test]
fn priority_and_ci_get_a_longer_default_stall() {
    let f = vec![FEATURE_STALL.to_string()];
    let none = JobCtl::default();
    let day = 86_400;
    assert_eq!(stall_for(&none, JobTier::Priority, &f, day), Some(7200));
    assert_eq!(stall_for(&none, JobTier::Ci, &f, day), Some(7200));
    assert_eq!(stall_for(&none, JobTier::Normal, &f, day), Some(1800));
    assert_eq!(stall_for(&none, JobTier::Batch, &f, day), Some(1800));
    let asked = JobCtl { stall_secs: Some(600), ..Default::default() };
    assert_eq!(stall_for(&asked, JobTier::Priority, &f, day), Some(600), "an ask wins");
    let off = JobCtl { stall_secs: Some(0), ..Default::default() };
    assert_eq!(stall_for(&off, JobTier::Ci, &f, day), None, "off is off");
    assert_eq!(stall_for(&none, JobTier::Priority, &f, 7200), None, "not shorter than its --timeout");
    assert_eq!(stall_for(&none, JobTier::Priority, &[], day), None, "a node without `stall`");
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j1", "node_id": "n1", "workspace_id": "w1", "workspace_label": "k2", "requested_by": "owner",
        "argv": ["true"], "env": {}, "cwd": null,
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": null, "disk_bytes": 1, "log_cap_bytes": 1},
        "src": null, "exclusive": false, "created_at": 1
    }))
    .expect("a plan");
    let ctl = JobCtl { timeout_secs: Some(4 * 3600), ..Default::default() };
    let mut o = cand("big", 64, 64, 0, 2).offer;
    o.features.push(FEATURE_STALL.to_string());
    let g = grant_row("big", 6 * 3600, 2);
    assert_eq!(plan_for_node(&plan, &ctl, JobTier::Priority, &node_row("big"), Some(&g), &o).stall_secs, Some(7200));
    assert_eq!(plan_for_node(&plan, &ctl, JobTier::Normal, &node_row("big"), Some(&g), &o).stall_secs, Some(1800));
}

/// 0.46.17: the placement line and `--plan`'s tools line show the node's
/// python3, curl and bash after the needs' state; a node that reports
/// none of them (an older k2-node) adds nothing.
#[test]
fn the_tools_line_shows_the_nodes_python_curl_and_bash() {
    let mut c = cand("node-4", 30, 32, 0, 2);
    assert_eq!(host_tools_words(&c.offer), None, "an older node reports none");
    c.offer.tools.insert("python3".into(), "3.13.1".into());
    c.offer.tools.insert("curl".into(), "8.12.1".into());
    c.offer.tools.insert("bash".into(), "5.2.37".into());
    assert_eq!(host_tools_words(&c.offer).as_deref(), Some("python3 3.13.1 · curl 8.12.1 · bash 5.2.37"));
    assert_eq!(tools_line(&[], Some(&c.offer)).as_deref(), Some("python3 3.13.1 · curl 8.12.1 · bash 5.2.37"));
    let ready = vec![NeedStatus { need: "node@22".into(), state: NeedState::Ready, have: Some("22.1.0".into()), native: None }];
    assert_eq!(tools_line(&ready, Some(&c.offer)).as_deref(), Some("tools ready · python3 3.13.1 · curl 8.12.1 · bash 5.2.37"));
    assert_eq!(tools_line(&ready, None).as_deref(), Some("tools ready"));
    assert_eq!(tools_line(&[], None), None);
    // The placement record's why carries them.
    let w = want(2);
    let rec = placement_record(&c, Tier::Now, &w, &Explain { policy: V2, cands: std::slice::from_ref(&c), skipped: &BTreeMap::new(), project_key: None, commit: None }, 1);
    assert!(rec.why.contains("python3 3.13.1 · curl 8.12.1 · bash 5.2.37"), "{}", rec.why);
}

/// 0.46.17: a Batch job's placement line says why it waits (an idle node,
/// how many jobs run there now) before the rest; an idle node, or any
/// other tier, says nothing of it.
#[test]
fn a_batch_placement_says_it_waits_for_an_idle_node() {
    let busy = [cand("b1", 30, 32, 1, 4)];
    let mut w = want(2);
    w.tier = JobTier::Batch;
    let ex = Explain { policy: V2, cands: &busy, skipped: &BTreeMap::new(), project_key: None, commit: None };
    let rec = placement_record(&busy[0], Tier::Now, &w, &ex, 1);
    assert!(rec.why.starts_with("batch: waits for an idle node (1 other job running) · "), "{}", rec.why);
    let standard = placement_record(&busy[0], Tier::Now, &want(2), &ex, 1);
    assert!(!standard.why.contains("batch"), "{}", standard.why);
    let idle = [cand("b2", 30, 32, 0, 4)];
    let ex = Explain { policy: V2, cands: &idle, skipped: &BTreeMap::new(), project_key: None, commit: None };
    assert!(!placement_record(&idle[0], Tier::Now, &w, &ex, 1).why.contains("waits for an idle node"), "an idle node starts it");
}

/// `--userns` (0.46.18): a job that asks goes only to a node this server
/// owns (never one where it is a tenant, nor one with another server's
/// owner label), that offers `job-userns`, where the workspace's grant
/// says `--userns`. The owner or an Admin gets no bypass (the grant row is
/// the node owner's yes). A job without the ask sees every node as before.
#[test]
fn userns_places_only_on_owned_granted_nodes_that_offer_it() {
    use crate::compute_job_network::ask_skip;
    use crate::compute_userns_ask::{auto_refusal, named_refusal, owns, userns_skip};
    use k2_core::compute::proto::userns::{codes, FEATURE_JOB_USERNS};
    let w = Want { userns: true, ..want(2) };
    let plain = want(2);
    let mut c = cand("own", 30, 32, 0, 2);
    assert!(owns(&c.node), "role owner, no owner label");
    assert_eq!(ask_skip(&w, c.grant.as_ref(), &c.offer, owns(&c.node)), Some(skip::USERNS_NEEDS_UPDATE), "no job-userns yet");
    c.offer.features.push(FEATURE_JOB_USERNS.into());
    assert_eq!(ask_skip(&w, c.grant.as_ref(), &c.offer, true), Some(skip::USERNS_NOT_GRANTED), "the grant lacks --userns");
    let admin = Want { admin: true, ..w.clone() };
    assert_eq!(ask_skip(&admin, c.grant.as_ref(), &c.offer, true), Some(skip::USERNS_NOT_GRANTED), "no owner/Admin bypass");
    assert_eq!(ask_skip(&admin, None, &c.offer, true), Some(skip::USERNS_NOT_GRANTED), "no grant at all: never");
    c.grant.as_mut().unwrap().allow_userns = true;
    assert_eq!(ask_skip(&w, c.grant.as_ref(), &c.offer, true), None);
    // A node where this server is a tenant: never, whatever its grant says.
    let mut t = c.clone();
    t.node.role = "tenant".into();
    t.node.owner_label = Some("other-server".into());
    assert!(!owns(&t.node));
    assert_eq!(ask_skip(&w, t.grant.as_ref(), &t.offer, owns(&t.node)), Some(skip::USERNS_OWNER_ONLY));
    let mut labelled = c.clone();
    labelled.node.owner_label = Some("other-server".into());
    assert!(!owns(&labelled.node), "another server's owner label: not ours");
    // No ask: nothing changes, anywhere.
    assert_eq!(ask_skip(&plain, t.grant.as_ref(), &t.offer, false), None);
    assert_eq!(ask_skip(&plain, None, &cand("p", 30, 32, 0, 2).offer, true), None);
    assert!(required_features(&w).contains(&FEATURE_JOB_USERNS));
    assert!(!required_features(&plain).contains(&FEATURE_JOB_USERNS));
    // A move rebuilds the ask from the stored plan.
    assert!(Want::of_job(&JobRow::default(), &userns_plan(true)).userns, "a move keeps the ask");
    assert!(!Want::of_job(&JobRow::default(), &userns_plan(false)).userns);
    // A node never seen online can't say: refused.
    assert_eq!(userns_skip(&w, true, c.grant.as_ref(), None), Some(skip::USERNS_NEEDS_UPDATE));
    // A named node: the named refusals (exit 3), each naming the node.
    let e = named_refusal(skip::USERNS_OWNER_ONLY, "k2n7", "builder", false);
    assert_eq!((e.code, e.status), (codes::USERNS_OWNER_ONLY, 403));
    assert!(e.hint.contains("--userns is only for the node owner's own server: this server is a tenant of k2n7, so its jobs can't make user namespaces there"), "{}", e.hint);
    let e = named_refusal(skip::USERNS_NOT_GRANTED, "k2n7", "builder", false);
    assert_eq!((e.code, e.status), (codes::USERNS_NOT_GRANTED, 403));
    assert!(e.hint.contains("k2 compute grant k2n7 --workspace builder --userns"), "{}", e.hint);
    let e = named_refusal(skip::USERNS_NEEDS_UPDATE, "k2n7", "builder", false);
    assert_eq!((e.code, e.status), (codes::USERNS_NEEDS_UPDATE, 409));
    assert!(e.hint.contains("'k2n7' can't") && e.hint.contains("0.46.18"), "{}", e.hint);
    // Auto-placement with none left: the grant first (the fix an owner can
    // make), then the update, then a tenant node named.
    let mut r = CandReport { visible: 3, ..Default::default() };
    r.skipped.insert(skip::USERNS_OWNER_ONLY, 1);
    r.userns_owner_only = vec!["shared-node".into()];
    let (code, status, hint, node) = auto_refusal(&w, &r, "builder").expect("refused");
    assert_eq!((code, status, node.as_deref()), (codes::USERNS_OWNER_ONLY, 403, Some("shared-node")));
    assert!(hint.contains("tenant of shared-node"), "{hint}");
    r.skipped.insert(skip::USERNS_NEEDS_UPDATE, 1);
    let (code, status, _, node) = auto_refusal(&w, &r, "builder").expect("refused");
    assert_eq!((code, status, node), (codes::USERNS_NEEDS_UPDATE, 409, None));
    r.skipped.insert(skip::USERNS_NOT_GRANTED, 1);
    r.userns_not_granted = vec!["mine".into()];
    let (code, status, hint, node) = auto_refusal(&w, &r, "builder").expect("refused");
    assert_eq!((code, status, node.as_deref()), (codes::USERNS_NOT_GRANTED, 403, Some("mine")));
    assert!(hint.contains("k2 compute grant mine --workspace builder --userns"), "{hint}");
    assert_eq!(auto_refusal(&plain, &r, "builder"), None, "no ask: the usual no_node_fits");
    assert_eq!(crate::compute_job_network::auto_refusal(&w, &r, "builder").map(|x| x.0), Some(codes::USERNS_NOT_GRANTED), "the run route's refusal");
    r.cands.push(c.clone());
    assert_eq!(auto_refusal(&w, &r, "builder"), None, "a node is left");
    assert!(skip_words(skip::USERNS_OWNER_ONLY, 1, &w).contains("tenant"));
    assert!(skip_words(skip::USERNS_NOT_GRANTED, 1, &w).contains("--userns"));
    assert!(skip_words(skip::USERNS_NEEDS_UPDATE, 1, &w).contains("too old for --userns"));
}

/// `--no-addr-filter` (0.46.18): only with `--userns` and `--network none`
/// (no dummies), else `no_addr_filter_invalid` (exit 2); placed only on a
/// node this server owns offering `no-addr-filter` too, with a grant that
/// has `allow_no_addr_filter` beside `allow_userns`. A tenant node never.
#[test]
fn no_addr_filter_needs_userns_network_none_its_feature_and_grant() {
    use crate::compute_job_network::ask_skip;
    use crate::compute_userns_ask::{from_body, named_refusal, owns};
    use k2_core::compute::proto::jobnet::{Dummy, JobNet};
    use k2_core::compute::proto::userns::{codes, FEATURE_JOB_USERNS, FEATURE_NO_ADDR_FILTER};
    let none = JobNet::none();
    let body = |x: serde_json::Value| from_body(&x, None);
    assert_eq!(from_body(&serde_json::json!({}), None).unwrap(), (false, false));
    assert_eq!(from_body(&serde_json::json!({"userns": true, "noAddrFilter": true}), Some(&none)).unwrap(), (true, true));
    let dummies = JobNet { dummies: vec![Dummy { name: "d0".into(), addr: "10.254.254.1/32".into() }], ..JobNet::none() };
    for (why, r) in [
        ("alone", body(serde_json::json!({"noAddrFilter": true}))),
        ("without --network none", body(serde_json::json!({"userns": true, "noAddrFilter": true}))),
        ("without --userns", from_body(&serde_json::json!({"noAddrFilter": true}), Some(&none))),
        ("with --netns-dummy", from_body(&serde_json::json!({"userns": true, "noAddrFilter": true}), Some(&dummies))),
    ] {
        let e = r.expect_err(why);
        assert_eq!((e.code, e.status), (codes::NO_ADDR_FILTER_INVALID, 400), "{why}");
        assert!(e.hint.contains("only with --userns and --network none"), "{why}: {}", e.hint);
    }
    assert_eq!(body(serde_json::json!({"noAddrFilter": "yes"})).expect_err("a bool").code, k2_core::compute::codes::BAD_REQUEST);
    let w = Want { userns: true, no_addr_filter: true, network: Some(none.clone()), ..want(2) };
    let mut c = cand("own", 30, 32, 0, 2);
    c.offer.features.push(FEATURE_JOB_USERNS.into());
    c.offer.features.push(k2_core::compute::proto::jobnet::FEATURE_JOB_NETWORK.into());
    c.grant.as_mut().unwrap().allow_userns = true;
    assert_eq!(ask_skip(&Want { no_addr_filter: false, ..w.clone() }, c.grant.as_ref(), &c.offer, true), None, "--userns alone runs there");
    assert_eq!(crate::compute_userns_ask::userns_skip(&w, true, c.grant.as_ref(), Some(&c.offer.features)), Some(skip::USERNS_NEEDS_UPDATE), "no no-addr-filter feature");
    c.offer.features.push(FEATURE_NO_ADDR_FILTER.into());
    assert_eq!(crate::compute_userns_ask::userns_skip(&w, true, c.grant.as_ref(), Some(&c.offer.features)), Some(skip::USERNS_NOT_GRANTED), "the grant lacks it");
    c.grant.as_mut().unwrap().allow_no_addr_filter = true;
    assert_eq!(crate::compute_userns_ask::userns_skip(&w, true, c.grant.as_ref(), Some(&c.offer.features)), None);
    let mut t = c.clone();
    t.node.role = "tenant".into();
    assert_eq!(crate::compute_userns_ask::userns_skip(&w, owns(&t.node), t.grant.as_ref(), Some(&t.offer.features)), Some(skip::USERNS_OWNER_ONLY), "a tenant node never, granted or not");
    let admin = Want { admin: true, ..w.clone() };
    assert_eq!(crate::compute_userns_ask::userns_skip(&admin, true, None, Some(&c.offer.features)), Some(skip::USERNS_NOT_GRANTED), "no owner/Admin bypass");
    assert!(required_features(&w).contains(&FEATURE_NO_ADDR_FILTER));
    assert!(!required_features(&Want { userns: true, ..want(2) }).contains(&FEATURE_NO_ADDR_FILTER));
    let mut p = userns_plan(true);
    p.no_addr_filter = true;
    assert!(Want::of_job(&JobRow::default(), &p).no_addr_filter, "a move keeps it");
    let e = named_refusal(skip::USERNS_NOT_GRANTED, "k2n7", "builder", true);
    assert!(e.hint.contains("k2 compute grant k2n7 --workspace builder --userns --no-addr-filter"), "{}", e.hint);
    let e = named_refusal(skip::USERNS_NEEDS_UPDATE, "k2n7", "builder", true);
    assert!(e.hint.starts_with("--no-addr-filter: 'k2n7' can't"), "{}", e.hint);
    assert!(skip_words(skip::USERNS_NEEDS_UPDATE, 1, &w).contains("--no-addr-filter"));
}

fn userns_plan(userns: bool) -> Plan {
    let mut p: Plan = serde_json::from_value(serde_json::json!({
        "job_id": "j", "workspace_id": ME, "workspace_label": "w", "requested_by": "owner", "node_id": "n", "argv": ["true"], "env": {},
        "limits": {"max_secs": 60, "cpu_millis": null, "mem_bytes": GB, "disk_bytes": GB, "log_cap_bytes": 1024},
        "src": null, "exclusive": false, "created_at": 1
    }))
    .expect("a plan");
    p.userns = userns;
    p
}

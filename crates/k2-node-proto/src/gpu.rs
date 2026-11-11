//! GPU jobs (0.46.11, prd-compute-gpu-jobs-v1 S1, GH#79): one render node
//! of a node's GPU, to one job at a time, only where the machine's owner
//! turned GPU jobs on for that server (`sudo k2-node admin gpu on`).
//!
//! - The ask: [`GpuAsk`] (`k2 compute run --gpu any|amd|intel`), carried
//!   as `Plan.gpu` and `JobStart.gpu`. Absent everywhere unless asked, so
//!   every other plan, digest and unit is unchanged.
//! - The facts: [`GpuFact`] in `HostFacts.gpus`, read from sysfs (never a
//!   shell). `jobs` says whether *this* server may run GPU jobs there.
//! - The offer feature [`FEATURE_GPU`] goes only to a server the owner
//!   named. A server never sends `Plan.gpu` to a node without it.
//! - A GPU is countable and exclusive (`Caps.gpus`, `Free.gpus`, the
//!   queue's `gpus`): two jobs never share one, and a job that would has
//!   to wait ([`codes::GPU_BUSY`]).
//!
//! - APU memory (S6, [`FEATURE_GPU_MEM`]): on a unified device (an APU or
//!   iGPU, whose GPU buffers come out of system RAM and aren't charged to
//!   the unit's `MemoryMax`), a GPU job's GPU memory budget
//!   (`--gpu-mem`, default [`DEFAULT_GPU_MEM_UNIFIED`]) counts against the
//!   node's free memory beside its `--mem` ([`counted_bytes`]), and the
//!   node stops a job whose own GPU memory (root reads the DRM fdinfo of
//!   the unit's processes) goes over it by more than
//!   [`GPU_MEM_SLACK_PCT`] ([`codes::GPU_MEM_CAP`]).
//!
//! What v1 never gives a job: a display (`/dev/dri/card*`), ROCm compute
//! (`/dev/kfd`, a later switch), another job's GPU, or an NVIDIA device
//! (refused until one passes on real hardware).

use serde::{Deserialize, Serialize};

/// Offer feature: this node runs GPU jobs for this server (its owner said
/// so in `gpu.toml`, and the root executor's self-test passed).
pub const FEATURE_GPU: &str = "gpu";

/// Every v1 job gets one GPU.
pub const MAX_GPUS_PER_JOB: u32 = 1;

/// Offer feature (S6): this node counts a GPU job's GPU memory on a
/// unified device and watches it (its root executor reads the job's DRM
/// fdinfo). Offered beside [`FEATURE_GPU`] only when the executor says
/// `gpu_usage`. A server sends `Plan.gpu_mem` (an explicit `--gpu-mem`)
/// only to a node offering it: an older node never sees the field.
pub const FEATURE_GPU_MEM: &str = "gpu-mem";

/// `--gpu-mem` when not given, on a unified device (PRD §3.7): 2 GiB. A
/// discrete GPU's own VRAM isn't system RAM: 0 there (nothing counted,
/// nothing watched unless asked).
pub const DEFAULT_GPU_MEM_UNIFIED: u64 = 2 << 30;
/// `--gpu-mem` is at least this (a smaller budget can't hold a context).
pub const MIN_GPU_MEM: u64 = 64 << 20;
/// ... and at most this (a sanity bound, not a node's size).
pub const MAX_GPU_MEM: u64 = 1 << 40;
/// The node stops a job whose GPU memory goes over its budget by more
/// than this share (in percent).
pub const GPU_MEM_SLACK_PCT: u64 = 10;

/// The env names root sets in a GPU job's unit (never the node, never the
/// job's own `--env`): which device it has.
pub const ENV_VENDOR: &str = "K2_GPU_VENDOR";
pub const ENV_PCI: &str = "K2_GPU_PCI";
pub const ENV_RENDER: &str = "K2_GPU_RENDER";
pub const ENV_KEYS: &[&str] = &[ENV_VENDOR, ENV_PCI, ENV_RENDER];
/// GPU jobs S3: a GPU job's own runtime dir (Wayland sockets), set by root
/// to [`RUNTIME_DIR`] in the job's folder and made 0700 by the job's uid.
pub const ENV_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";
pub const RUNTIME_DIR: &str = "run";

/// Codes every side says the same way.
pub mod codes {
    /// No node this workspace may see runs GPU jobs for this server (or
    /// none has a GPU of the asked vendor). Exit 3.
    pub const NO_GPU_NODE: &str = "no_gpu_node";
    /// The node's GPUs are all held by other jobs: the job waits.
    pub const GPU_BUSY: &str = "gpu_busy";
    /// The node no longer runs GPU jobs for this server (its owner turned
    /// them off, or the executor can't give a device): the job fails there.
    pub const GPU_UNAVAILABLE: &str = "gpu_unavailable";
    /// A node runs GPU jobs, but this workspace's grant there doesn't say
    /// `--gpu` (owner or Admin only; an agent asks with a card). Exit 3.
    pub const GPU_NEEDS_GRANT: &str = "gpu_needs_grant";
    /// S6: the job's own GPU memory went over its `--gpu-mem` (+10%): the
    /// node stopped it (`failed`), with the numbers, like `mem_cap`.
    pub const GPU_MEM_CAP: &str = "gpu_mem_cap";
    /// S6: `--gpu-mem`, but the GPU nodes for this server run a k2-node
    /// (or root executor) older than 0.46.11, which can't watch it. Exit 3.
    pub const GPU_MEM_NEEDS_UPDATE: &str = "gpu_mem_needs_update";
}

/// Which GPU a job asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    #[default]
    Any,
    Amd,
    Intel,
    Nvidia,
}

impl GpuVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            GpuVendor::Any => "any",
            GpuVendor::Amd => "amd",
            GpuVendor::Intel => "intel",
            GpuVendor::Nvidia => "nvidia",
        }
    }

    /// `any` | `amd` | `intel` | `nvidia` (lower case only).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "any" => Some(GpuVendor::Any),
            "amd" => Some(GpuVendor::Amd),
            "intel" => Some(GpuVendor::Intel),
            "nvidia" => Some(GpuVendor::Nvidia),
            _ => None,
        }
    }

    /// Does a device of `vendor` (`amd`, `intel`, `nvidia`) answer this ask?
    pub fn matches(self, vendor: &str) -> bool {
        match self {
            GpuVendor::Any => !vendor.is_empty(),
            v => v.as_str() == vendor,
        }
    }
}

/// `--gpu [any|amd|intel]`: one GPU of that vendor, to this job alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuAsk {
    pub vendor: GpuVendor,
    pub count: u32,
}

impl GpuAsk {
    pub fn one(vendor: GpuVendor) -> Self {
        GpuAsk { vendor, count: 1 }
    }

    /// v1: exactly one GPU per job.
    pub fn check(&self) -> Result<(), String> {
        if self.count != MAX_GPUS_PER_JOB {
            return Err(format!("a job gets exactly {MAX_GPUS_PER_JOB} GPU (count {})", self.count));
        }
        Ok(())
    }
}

/// One GPU of a node, as its facts say (`HostFacts.gpus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuFact {
    /// `amd` | `intel` | `nvidia`.
    pub vendor: String,
    /// PCI address (`0000:c4:00.0`).
    pub pci: String,
    /// The kernel driver (`amdgpu`, `i915`, `xe`, `nvidia`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// `vendor:device` (`1002:1586`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ids: String,
    /// The model from `pci.ids` when the node has it, else the ids.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The render node (`renderD128`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub render: String,
    /// `mem_info_vram_total` (amdgpu).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_bytes: Option<u64>,
    /// `mem_info_gtt_total` (amdgpu).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gtt_bytes: Option<u64>,
    /// An APU or iGPU sharing system memory (a heuristic, see the node's
    /// sysfs reader).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unified: bool,
    /// This session's server may run GPU jobs on it here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub jobs: bool,
}

impl GpuFact {
    /// `AMD Radeon Test APU (amdgpu, 96G unified)` / `Intel UHD 770 (i915)`.
    pub fn line(&self) -> String {
        let name = if self.model.is_empty() { format!("{} {}", self.vendor, self.ids).trim().to_string() } else { self.model.clone() };
        let mut inner: Vec<String> = Vec::new();
        if !self.driver.is_empty() {
            inner.push(self.driver.clone());
        }
        let mem = if self.unified { self.gtt_bytes.or(self.vram_bytes) } else { self.vram_bytes };
        if let Some(b) = mem.filter(|b| *b > 0) {
            inner.push(format!("{}G{}", (b + (1 << 29)) >> 30, if self.unified { " unified" } else { "" }));
        } else if self.unified {
            inner.push("unified".into());
        }
        if inner.is_empty() {
            name
        } else {
            format!("{name} ({})", inner.join(", "))
        }
    }
}

/// Does any of `gpus` answer `ask` for this server (`jobs`)?
pub fn any_for(gpus: &[GpuFact], ask: &GpuAsk) -> bool {
    gpus.iter().any(|g| g.jobs && ask.vendor.matches(&g.vendor))
}

/// Is the GPU `ask` would get here unified (an APU or iGPU)? Among the
/// GPUs open to this server (`jobs`) of the asked vendor, any unified one
/// counts (root picks the device; this never undercounts).
pub fn unified_for(gpus: &[GpuFact], ask: &GpuAsk) -> bool {
    gpus.iter().any(|g| g.jobs && g.unified && ask.vendor.matches(&g.vendor))
}

/// S6: the system RAM a GPU job's GPU memory takes on a node, counted
/// against the node's free memory beside its `--mem`: its `--gpu-mem`
/// (default [`DEFAULT_GPU_MEM_UNIFIED`]) on a unified device, else 0 (a
/// discrete GPU's VRAM is its own). No `--gpu`: 0.
pub fn counted_bytes(ask: Option<&GpuAsk>, gpu_mem: Option<u64>, unified: bool) -> u64 {
    match ask {
        Some(_) if unified => gpu_mem.unwrap_or(DEFAULT_GPU_MEM_UNIFIED),
        _ => 0,
    }
}

/// S6: the GPU memory budget the node watches a job against (`None`:
/// none): an explicit `--gpu-mem` on any GPU, else the default on a
/// unified device. No `--gpu`: none.
pub fn cap_bytes(ask: Option<&GpuAsk>, gpu_mem: Option<u64>, unified: bool) -> Option<u64> {
    ask?;
    gpu_mem.or(unified.then_some(DEFAULT_GPU_MEM_UNIFIED))
}

/// `--mem` with what [`counted_bytes`] adds (`None` stays `None`: the
/// node's even share, as before).
pub fn with_gpu_mem(mem: Option<u64>, extra: u64) -> Option<u64> {
    mem.map(|m| m.saturating_add(extra))
}

/// Is `used` over the budget `cap` by more than [`GPU_MEM_SLACK_PCT`]?
pub fn over_cap(cap: u64, used: u64) -> bool {
    used > cap.saturating_add(cap.saturating_mul(GPU_MEM_SLACK_PCT) / 100)
}

/// A `--gpu-mem` value a server accepts ([`MIN_GPU_MEM`]..=[`MAX_GPU_MEM`]).
pub fn check_gpu_mem(b: u64) -> Result<(), String> {
    if !(MIN_GPU_MEM..=MAX_GPU_MEM).contains(&b) {
        return Err(format!("--gpu-mem is between {}M and {}G", MIN_GPU_MEM >> 20, MAX_GPU_MEM >> 30));
    }
    Ok(())
}

/// The log line and detail of a [`codes::GPU_MEM_CAP`] stop.
pub fn gpu_mem_cap_detail(cap: u64, used: u64) -> String {
    format!(
        "the job's GPU memory reached {} (its --gpu-mem is {}; the node stops a job {}% over it). On an APU, GPU buffers are system RAM outside --mem: raise --gpu-mem, or allocate less",
        gib1(used),
        gib1(cap),
        GPU_MEM_SLACK_PCT
    )
}

/// `2.5G` (one decimal).
pub fn gib1(b: u64) -> String {
    let tenths = (b.saturating_mul(10) + (1 << 29)) >> 30;
    format!("{}.{}G", tenths / 10, tenths % 10)
}

/// GPU memory a job's own processes hold, as root read it from the DRM
/// fdinfo of its unit's processes (`gpu_usage`). Each DRM client (one open
/// of the device) is counted once, however many fds or processes share
/// it; `bytes` is every memory region it reports resident.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuMemUsage {
    pub bytes: u64,
    #[serde(default)]
    pub vram_bytes: u64,
    #[serde(default)]
    pub gtt_bytes: u64,
    /// DRM clients counted.
    #[serde(default)]
    pub clients: u32,
    /// Processes read.
    #[serde(default)]
    pub pids: u32,
}

/// The hint every side gives for [`codes::NO_GPU_NODE`]: both steps a
/// person takes (the node's owner at the machine, this server's owner for
/// the grant).
pub fn no_gpu_node_hint(ws: &str) -> String {
    format!(
        "no node you can use runs GPU jobs for this server. Two people turn it on: the node's owner, at that machine: \
         sudo k2-node admin gpu on --server <this server>; then this server's owner: k2 compute grant <node> --workspace {ws} --gpu \
         (k2 compute nodes --json shows host.gpus; jobs: true is a GPU you may use)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ask_is_lower_case_json_and_one_gpu() {
        let a = GpuAsk::one(GpuVendor::Any);
        assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"vendor":"any","count":1}"#);
        let b: GpuAsk = serde_json::from_str(r#"{"vendor":"amd","count":1}"#).unwrap();
        assert_eq!(b.vendor, GpuVendor::Amd);
        assert!(b.check().is_ok());
        assert!(GpuAsk { vendor: GpuVendor::Any, count: 2 }.check().is_err());
        assert!(serde_json::from_str::<GpuAsk>(r#"{"vendor":"AMD","count":1}"#).is_err());
        assert_eq!(GpuVendor::parse("intel"), Some(GpuVendor::Intel));
        assert_eq!(GpuVendor::parse("radeon"), None);
    }

    #[test]
    fn vendors_match_their_own_devices_and_any_matches_all() {
        assert!(GpuVendor::Any.matches("amd") && GpuVendor::Any.matches("intel"));
        assert!(!GpuVendor::Any.matches(""));
        assert!(GpuVendor::Amd.matches("amd") && !GpuVendor::Amd.matches("intel"));
    }

    #[test]
    fn only_a_gpu_open_to_this_server_answers() {
        let mut g = GpuFact { vendor: "amd".into(), pci: "0000:c4:00.0".into(), ..Default::default() };
        let ask = GpuAsk::one(GpuVendor::Any);
        assert!(!any_for(std::slice::from_ref(&g), &ask), "a GPU without jobs: no");
        g.jobs = true;
        assert!(any_for(std::slice::from_ref(&g), &ask));
        assert!(!any_for(std::slice::from_ref(&g), &GpuAsk::one(GpuVendor::Intel)), "another vendor: no");
    }

    #[test]
    fn a_fact_without_gpu_jobs_serializes_without_the_flag() {
        let g = GpuFact { vendor: "amd".into(), pci: "0000:c4:00.0".into(), render: "renderD128".into(), ..Default::default() };
        let j = serde_json::to_string(&g).unwrap();
        assert_eq!(j, r#"{"vendor":"amd","pci":"0000:c4:00.0","render":"renderD128"}"#);
    }

    #[test]
    fn the_line_names_model_driver_and_memory() {
        let g = GpuFact {
            vendor: "amd".into(),
            driver: "amdgpu".into(),
            model: "AMD Radeon Test APU".into(),
            gtt_bytes: Some(96 << 30),
            vram_bytes: Some(512 << 20),
            unified: true,
            ..Default::default()
        };
        assert_eq!(g.line(), "AMD Radeon Test APU (amdgpu, 96G unified)");
        let i = GpuFact { vendor: "intel".into(), ids: "8086:a780".into(), driver: "i915".into(), ..Default::default() };
        assert_eq!(i.line(), "intel 8086:a780 (i915)");
    }

    /// S6: a GPU job on a unified device counts its `--gpu-mem` (default
    /// 2G) against the node's memory and is watched against it; on a
    /// discrete one nothing is counted, and only an explicit `--gpu-mem`
    /// is watched. No `--gpu`: nothing, whatever `gpu_mem` says.
    #[test]
    fn unified_devices_count_gpu_memory_and_discrete_ones_dont() {
        let ask = GpuAsk::one(GpuVendor::Any);
        assert_eq!(counted_bytes(Some(&ask), None, true), DEFAULT_GPU_MEM_UNIFIED);
        assert_eq!(DEFAULT_GPU_MEM_UNIFIED, 2 << 30);
        assert_eq!(counted_bytes(Some(&ask), Some(6 << 30), true), 6 << 30);
        assert_eq!(counted_bytes(Some(&ask), Some(6 << 30), false), 0, "a discrete GPU's VRAM isn't system RAM");
        assert_eq!(counted_bytes(None, Some(6 << 30), true), 0, "no --gpu, nothing counted");
        assert_eq!(cap_bytes(Some(&ask), None, true), Some(DEFAULT_GPU_MEM_UNIFIED));
        assert_eq!(cap_bytes(Some(&ask), None, false), None, "discrete, not asked: not watched");
        assert_eq!(cap_bytes(Some(&ask), Some(1 << 30), false), Some(1 << 30), "asked: watched anywhere");
        assert_eq!(cap_bytes(None, Some(1 << 30), true), None);
        assert_eq!(with_gpu_mem(Some(8 << 30), 2 << 30), Some(10 << 30));
        assert_eq!(with_gpu_mem(None, 2 << 30), None);
    }

    #[test]
    fn unified_for_looks_only_at_gpus_open_to_this_server() {
        let mut g = GpuFact { vendor: "amd".into(), pci: "0000:c4:00.0".into(), unified: true, ..Default::default() };
        let ask = GpuAsk::one(GpuVendor::Any);
        assert!(!unified_for(std::slice::from_ref(&g), &ask), "not open to it");
        g.jobs = true;
        assert!(unified_for(std::slice::from_ref(&g), &ask));
        assert!(!unified_for(std::slice::from_ref(&g), &GpuAsk::one(GpuVendor::Intel)));
        g.unified = false;
        assert!(!unified_for(std::slice::from_ref(&g), &ask));
    }

    /// The stop is strictly over the budget plus 10%; the detail names both.
    #[test]
    fn over_cap_allows_ten_percent() {
        let cap = 2u64 << 30;
        assert!(!over_cap(cap, cap));
        assert!(!over_cap(cap, cap + cap / 10));
        assert!(over_cap(cap, cap + cap / 10 + 1));
        assert!(!over_cap(0, 0));
        assert!(over_cap(0, 1));
        let d = gpu_mem_cap_detail(cap, 3 << 30);
        assert!(d.contains("3.0G") && d.contains("--gpu-mem is 2.0G"), "{d}");
        assert!(check_gpu_mem(2 << 30).is_ok());
        assert!(check_gpu_mem(1 << 20).is_err());
        assert!(check_gpu_mem(2 << 40).is_err());
        let u = GpuMemUsage { bytes: 5, ..Default::default() };
        let back: GpuMemUsage = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back, u);
        assert_eq!(serde_json::from_str::<GpuMemUsage>(r#"{"bytes":7}"#).unwrap().bytes, 7);
    }

    #[test]
    fn the_no_gpu_node_hint_names_both_steps() {
        let h = no_gpu_node_hint("builder");
        assert!(h.contains("sudo k2-node admin gpu on"), "{h}");
        assert!(h.contains("k2 compute grant <node> --workspace builder --gpu"), "{h}");
    }
}

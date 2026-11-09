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
//! What v1 never gives a job: a display (`/dev/dri/card*`), ROCm compute
//! (`/dev/kfd`, a later switch), another job's GPU, or an NVIDIA device
//! (refused until one passes on real hardware).

use serde::{Deserialize, Serialize};

/// Offer feature: this node runs GPU jobs for this server (its owner said
/// so in `gpu.toml`, and the root executor's self-test passed).
pub const FEATURE_GPU: &str = "gpu";

/// Every v1 job gets one GPU.
pub const MAX_GPUS_PER_JOB: u32 = 1;

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

    #[test]
    fn the_no_gpu_node_hint_names_both_steps() {
        let h = no_gpu_node_hint("builder");
        assert!(h.contains("sudo k2-node admin gpu on"), "{h}");
        assert!(h.contains("k2 compute grant <node> --workspace builder --gpu"), "{h}");
    }
}

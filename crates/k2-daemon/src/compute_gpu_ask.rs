//! A compute job's GPU ask (0.46.11, `k2 compute run --gpu`, `--gpu-mem`)
//! on the controller side, out of the run route and placement: the ask
//! from a run request ([`gpu_from`], [`gpu_mem_from`]), why a node can't
//! take it ([`gpu_skip`]), and the refusals for a named node
//! ([`gpu_node_refusal`]) and for auto-placement with no node left
//! ([`gpu_refusal`]). `compute_place` re-exports the placement half.

use k2_core::compute::codes;
use k2_core::compute::proto::frames::Offer;
use k2_core::compute::store::GrantRow;

use crate::compute_place::{skip, CandReport, Want};
use crate::compute_routes::{no, Refusal, R};

/// `gpu` from a run request (`--gpu [any|amd|intel]`): `{"vendor": "any",
/// "count": 1}`. NVIDIA is refused until tested on real hardware.
pub(crate) fn gpu_from(v: &serde_json::Value) -> R<Option<k2_core::compute::proto::gpu::GpuAsk>> {
    use k2_core::compute::proto::gpu::{GpuAsk, GpuVendor};
    let g = match v.get("gpu") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(g) => g,
    };
    let bad = || {
        no(codes::BAD_REQUEST, 400, "--gpu is any, amd or intel (one GPU per job): gpu = {\"vendor\": \"any\", \"count\": 1}")
    };
    let vendor = g
        .get("vendor")
        .and_then(|x| x.as_str())
        .and_then(GpuVendor::parse)
        .ok_or_else(bad)?;
    if vendor == GpuVendor::Nvidia {
        return Err(no(codes::BAD_REQUEST, 400, "--gpu nvidia: NVIDIA GPU jobs aren't supported yet (not tested on real hardware); use --gpu any, amd or intel"));
    }
    let count = match g.get("count") {
        None | Some(serde_json::Value::Null) => 1,
        Some(c) => c
            .as_u64()
            .filter(|c| *c <= u64::from(u32::MAX))
            .ok_or_else(bad)? as u32,
    };
    let ask = GpuAsk { vendor, count };
    ask.check()
        .map_err(|e| no(codes::BAD_REQUEST, 400, format!("--gpu: {e}")))?;
    Ok(Some(ask))
}

/// `gpuMemBytes` from a run request (`--gpu-mem 4G`): only with `--gpu`.
pub(crate) fn gpu_mem_from(v: &serde_json::Value, gpu: bool) -> R<Option<u64>> {
    let b = match v.get("gpuMemBytes") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(x) => x.as_u64().ok_or_else(|| {
            no(
                codes::BAD_REQUEST,
                400,
                "--gpu-mem is a size: --gpu-mem 4G (gpuMemBytes, bytes)",
            )
        })?,
    };
    if !gpu {
        return Err(no(codes::BAD_REQUEST, 400, "--gpu-mem needs --gpu: it is the GPU memory budget of a GPU job (k2 compute run --gpu --gpu-mem 4G --mem 8G -- <command>)"));
    }
    k2_core::compute::proto::gpu::check_gpu_mem(b).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    Ok(Some(b))
}

/// A named node that can't take a `--gpu` job (`gpu_skip`'s code): exit 3.
pub(crate) fn gpu_node_refusal(code: &str, node: &str, ws: &str) -> Refusal {
    use k2_core::compute::proto::gpu::codes as g;
    if code == skip::GPU_MEM_NEEDS_UPDATE {
        return no(
            g::GPU_MEM_NEEDS_UPDATE,
            409,
            gpu_mem_needs_update_hint(Some(node)),
        )
        .with("node", node.to_string());
    }
    if code == skip::GPU_NEEDS_GRANT {
        return no(
            g::GPU_NEEDS_GRANT,
            403,
            format!("--gpu: '{node}' runs GPU jobs for this server, but workspace {ws}'s grant there doesn't allow them. The owner or an Admin: k2 compute grant {node} --workspace {ws} --gpu"),
        )
        .with("node", node.to_string());
    }
    no(
        g::NO_GPU_NODE,
        404,
        format!(
            "--gpu: '{node}' runs no GPU jobs for this server (or none of that vendor). Its owner turns them on at the machine: sudo k2-node admin gpu on --server <this server>; then: k2 compute grant {node} --workspace {ws} --gpu. Or leave the node off and K2 picks one that has a GPU for you"
        ),
    )
}

/// GPU jobs (0.46.11): why a node can't take `w`'s GPU ask, or `None`.
/// [`skip::NO_GPU`]: it runs no GPU jobs for this server (no `gpu` feature
/// in this session, or no GPU of that vendor open to it in `host.gpus`).
/// [`skip::GPU_NEEDS_GRANT`]: it does, but this workspace's grant there
/// doesn't say `--gpu` and the caller isn't the owner or an Admin.
pub fn gpu_skip(w: &Want, grant: Option<&GrantRow>, offer: &Offer) -> Option<&'static str> {
    use k2_core::compute::proto::gpu;
    let ask = w.gpu.as_ref()?;
    let has = offer.features.iter().any(|f| f == gpu::FEATURE_GPU)
        && offer
            .host
            .as_ref()
            .is_some_and(|h| gpu::any_for(&h.gpus, ask));
    if !has {
        return Some(skip::NO_GPU);
    }
    if !(w.admin || grant.is_some_and(|g| g.allow_gpu)) {
        return Some(skip::GPU_NEEDS_GRANT);
    }
    // S6: an explicit `--gpu-mem` only where the node watches it.
    if w.gpu_mem.is_some() && !offer.features.iter().any(|f| f == gpu::FEATURE_GPU_MEM) {
        return Some(skip::GPU_MEM_NEEDS_UPDATE);
    }
    None
}

/// GPU jobs (0.46.11): the refusal when no node is left for a GPU ask.
/// `gpu_needs_grant` (403, an agent's card) when a node has a GPU for this
/// server but the grant lacks `--gpu`; `no_gpu_node` (404) when no node
/// this workspace may see runs GPU jobs for it. Both exit 3. `None`: no
/// GPU ask, or nodes are left.
pub fn gpu_refusal(
    w: &Want,
    report: &CandReport,
    ws_handle: &str,
) -> Option<(&'static str, u16, String, Option<String>)> {
    use k2_core::compute::proto::gpu::{codes, no_gpu_node_hint};
    w.gpu.as_ref()?;
    if !report.cands.is_empty() {
        return None;
    }
    // S6: GPU nodes for this server, but none takes `--gpu-mem` yet.
    if report
        .skipped
        .get(skip::GPU_MEM_NEEDS_UPDATE)
        .copied()
        .unwrap_or(0)
        > 0
        && report.gpu_needs_grant.is_empty()
    {
        return Some((
            codes::GPU_MEM_NEEDS_UPDATE,
            409,
            gpu_mem_needs_update_hint(None),
            None,
        ));
    }
    if let Some(node) = report.gpu_needs_grant.first() {
        return Some((
            codes::GPU_NEEDS_GRANT,
            403,
            format!(
                "--gpu: {node} runs GPU jobs for this server, but workspace {ws_handle}'s grant there doesn't allow them. The owner or an Admin: k2 compute grant {node} --workspace {ws_handle} --gpu"
            ),
            Some(node.clone()),
        ));
    }
    if report.skipped.get(skip::NO_GPU).copied().unwrap_or(0) > 0 || report.visible == 0 {
        return Some((codes::NO_GPU_NODE, 404, no_gpu_node_hint(ws_handle), None));
    }
    None
}

/// The hint for [`codes::GPU_MEM_NEEDS_UPDATE`] (a named node, or every
/// GPU node this workspace may use).
///
/// [`codes::GPU_MEM_NEEDS_UPDATE`]: k2_core::compute::proto::gpu::codes::GPU_MEM_NEEDS_UPDATE
pub fn gpu_mem_needs_update_hint(node: Option<&str>) -> String {
    let which = node
        .map(|n| format!("'{n}' runs GPU jobs for this server, but its k2-node"))
        .unwrap_or_else(|| "the nodes that run GPU jobs for this server run a k2-node that".into());
    format!(
        "--gpu-mem: {which} is older than 0.46.11 and can't watch a job's GPU memory. Its owner updates k2-node there; or run without --gpu-mem (an older node doesn't count or watch GPU memory at all: on an APU, leave headroom in --mem)"
    )
}

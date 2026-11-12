//! Cache generations on the controller (prd-compute-blacksmith-class-v1
//! B2): the plan's cache fields for a job, a plain job's scratch lineage,
//! and `k2 compute cache --lineage`. Kept out of `compute_routes` (held in
//! the size ratchet).

use std::collections::HashMap;
use std::path::Path;

use k2_core::compute::{self as compute, codes, store};

use crate::compute_routes::{need_credential, no, passport_gate, repo_pick, target_ws, Who, R};

/// `GET /cli/compute/cache/lineages?workspace=&node=&lineage=` (B2,
/// §4.1.9): the workspace's cache generations on each connected node, as
/// the nodes' `lineage_status` said: per lineage its head (seq, commit,
/// age), size and forks running. Only this workspace's project (its key
/// is a hash of the workspace and its remote, so no other workspace's
/// lineage can match). `lineage`: `trusted`, `scratch` or `release`.
pub(crate) fn cache_lineages(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    passport_gate(who)?;
    let p = |k: &str| params.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let ws = target_ws(who, p("workspace"))?;
    let kind = p("lineage");
    if let Some(k) = kind {
        if !matches!(k, "trusted" | "scratch" | "release") {
            return Err(no(codes::BAD_REQUEST, 400, "--lineage is trusted, scratch or release"));
        }
    }
    let key = repo_pick(&ws, &serde_json::json!({})).ok().map(|pick| pick.project_key(&ws));
    let now = compute::now();
    let db = k2_core::db::shared();
    let mut nodes = Vec::new();
    for (node_id, l) in crate::compute_ws::all_lineages() {
        let name = store::node_by_id(&db.lock(), &node_id).map(|n| n.name).unwrap_or_else(|| node_id.clone());
        if p("node").is_some_and(|n| n != name && n != node_id) {
            continue;
        }
        let rows: Vec<serde_json::Value> = l
            .lineages
            .iter()
            .filter(|x| key.as_deref() == Some(x.key.as_str()))
            .filter(|x| kind.is_none_or(|k| x.lineage == k || x.lineage.starts_with(&format!("{k}/"))))
            .map(|x| {
                serde_json::json!({
                    "lineage": x.lineage, "headSeq": x.head_seq, "headCommit": x.head_commit,
                    "bytes": x.bytes, "lastUsed": x.last_used, "ageSecs": (now - x.last_used).max(0), "forks": x.forks,
                })
            })
            .collect();
        if !rows.is_empty() {
            nodes.push(serde_json::json!({"node": name, "lineages": rows, "truncated": l.truncated}));
        }
    }
    Ok(serde_json::json!({"workspace": ws.handle, "nodes": nodes}))
}

/// B2: a plain job's scratch lineage (§4.1.2): `wt:<checkout>` for
/// `--dirty` (its changes are its own), else the branch it builds, else
/// `wt:<checkout>`. Never a trusted lineage: that is the CI engine's.
pub(crate) fn scratch_ref_for(root: &Path, rel: &str, dirty: bool, sha: Option<&str>) -> Option<String> {
    use k2_core::compute::proto::gens::{valid_ref, WORKTREE_REF_PREFIX};
    let wt = || {
        let name: String = rel.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("workspace").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).take(64).collect();
        let r = format!("{WORKTREE_REF_PREFIX}{}", name.trim_start_matches(['-', '.']));
        valid_ref(&r).then_some(r)
    };
    if dirty {
        return wt();
    }
    let named = sha.filter(|x| !(x.len() >= 7 && x.bytes().all(|b| b.is_ascii_hexdigit())) && *x != "HEAD" && valid_ref(x) && !x.starts_with(WORKTREE_REF_PREFIX));
    if let Some(b) = named {
        return Some(b.to_string());
    }
    let head = k2_core::git_safe::git(root).args(["symbolic-ref", "--quiet", "--short", "HEAD"]).output().ok().filter(|o| o.status.success());
    match head.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) {
        Some(b) if sha.is_none_or(|x| x == "HEAD") && valid_ref(&b) && !b.starts_with(WORKTREE_REF_PREFIX) => Some(b),
        _ => wt(),
    }
}

/// B2: the plan's cache fields for one job (see the call site).
pub(crate) fn gens_fields(
    has_src: bool,
    release: bool,
    secret: bool,
    ci: Option<(k2_core::compute::proto::gens::CacheCommit, Option<String>)>,
    scratch: Option<String>,
) -> (Option<k2_core::compute::proto::gens::CacheCommit>, Option<String>) {
    use k2_core::compute::proto::gens::CacheCommit;
    match () {
        _ if !has_src || release => (None, None),
        _ if secret => (Some(CacheCommit::None), None),
        _ if ci.is_some() => ci.map(|(c, r)| (Some(c), r)).unwrap_or((None, None)),
        _ => match scratch {
            Some(r) => (Some(CacheCommit::Scratch), Some(r)),
            None => (None, None),
        },
    }
}

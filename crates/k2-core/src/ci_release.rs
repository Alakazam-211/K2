//! `k2 ci release` (0.46.11, prd-ci-on-compute-for-clients-v1 §4.6, §4.7;
//! vs-live V31, V32; slices C5 and C6b): the pure rules and the store. The
//! daemon (`k2_daemon::ci_release`) gathers the facts, submits the jobs and
//! assembles the evidence; everything that decides is here, so it is tested
//! without nodes.
//!
//! **Placement of a release set (C5).** A release is submitted as **one
//! unit** on the client's seat on the owner `[release] owner` names, lane
//! `[release] lane`:
//! - only seat nodes whose key is **active and vouched for** (a taint
//!   basis) in the owner's newest node roster this server holds count, and
//!   only those that can take the build now (the normal job path's checks:
//!   grant, `--ci`, receipt v2, tools, size; the daemon asks it per node);
//! - a dual build's build and verify jobs (`L1`, `V1`) go on two of them
//!   that differ in every dimension the build names (`independent`,
//!   default `host`; `node` always), decided from the **owner's roster**
//!   entries (host, site, provider, admin reach), never from node labels.
//!   The rule is the signer's own (`k2-signer` `check::independent`), so a
//!   set this places is one the signer accepts on that count. Among the
//!   pairs that qualify, the two that start soonest;
//! - a single build (`L3`) goes on a seat node the set doesn't use yet,
//!   else queues on the least used one (behind `L1` on a two-node seat);
//! - no pair that qualifies: `no_independent_nodes` (exit 3), naming the
//!   dimension; fewer usable seat nodes than a dual build needs (or none
//!   for a single one): `seat_degraded` (exit 3), naming why each is out.
//!
//! **A release run** is a `ci_runs` row of kind `release` (migration 0287):
//! the same reaper and verdict as a check run, no commit statuses, one
//! `ci_checks` row per job with its label, build, role and node.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::compute::proto::ci::{codes as ccodes, Dimension};
use crate::compute::proto::evidence::Role;
use crate::compute::proto::node_roster::RosterEntry;

/// Refusal codes of `/cli/ci/release` and `/cli/ci/evidence` (beside
/// `ci::codes` and the frozen `no_independent_nodes` / `seat_degraded`).
pub mod codes {
    /// The commit's `.k2/ci.toml` has no `[release]`.
    pub const NO_RELEASE: &str = "ci_no_release";
    /// No client seat on `[release] owner` / `lane` here.
    pub const NO_SEAT: &str = "ci_no_seat";
    /// The run is a check run, not a release.
    pub const NOT_A_RELEASE: &str = "ci_not_a_release";
    /// `k2 ci evidence` before every job of the set ended done/exit 0.
    pub const NOT_DONE: &str = "ci_release_not_done";
    /// A version K2 can't name a release by.
    pub const BAD_VERSION: &str = "ci_version_invalid";
    /// A job's receipt is missing, unsigned, not v2 or failed this server's
    /// check.
    pub const RECEIPT: &str = "ci_receipt_not_usable";
    /// `--build` names a build `[release]` doesn't have.
    pub const NO_BUILD: &str = "ci_build_not_found";
    pub use crate::compute::proto::ci::codes::{NO_INDEPENDENT_NODES, SEAT_DEGRADED};
}

/// The kinds of `ci_runs.kind`.
pub mod kind {
    pub const CHECKS: &str = "checks";
    pub const RELEASE: &str = "release";
}

// ── placement (C5) ───────────────────────────────────────────────────

/// One node of the client's seat, as the daemon read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatNode {
    /// This server's `compute_nodes.id`.
    pub node_id: String,
    /// The node's roster name (the seat statement names it).
    pub name: String,
    /// Its key's entry in the owner's newest roster held here.
    pub entry: Option<RosterEntry>,
    /// Why it takes no release build at all (revoked, unlisted, no taint
    /// basis, removed); `None` = it may.
    pub out: Option<String>,
}

/// A build of the set, from `[release.builds.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub build: String,
    pub label: String,
    /// Set = a dual build: this label goes on an independent node.
    pub verify_label: Option<String>,
    /// The dimensions the verify node must differ in (`node` first).
    pub independence: Vec<Dimension>,
}

/// Can `node` take `build` now, and how soon would it start (the normal
/// job path's own placement, asked per node by the daemon)?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub starts_now: bool,
    /// The node's quoted wait when it would queue.
    pub eta_secs: u64,
}

/// One job of the placed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placed {
    pub build: String,
    pub label: String,
    pub role: Role,
    pub node_id: String,
    pub node: String,
    /// One line: why this node.
    pub why: String,
}

/// A refusal: `(code, message for a person)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused { code, message: message.into() }
}

/// Does `b` (the verify node) differ from `a` (the build node) in `dim`, on
/// one owner's roster? The signer's rule (`k2-signer` `check::independent`)
/// for entries of the same owner: `owner` never holds on one seat.
pub fn independent_of(dim: Dimension, a: &RosterEntry, b: &RosterEntry) -> Result<(), String> {
    match dim {
        Dimension::Node => {
            if a.node_fp == b.node_fp || a.name == b.name {
                return Err("the same node".into());
            }
        }
        Dimension::Host => {
            if a.host == b.host {
                return Err(format!("{} and {} are both on host {} (the owner's roster)", a.name, b.name, a.host));
            }
        }
        Dimension::Site => match (&a.site, &b.site) {
            (Some(x), Some(y)) if x != y => {}
            (Some(x), Some(_)) => return Err(format!("{} and {} are both at site {x}", a.name, b.name)),
            _ => return Err(format!("the owner's roster doesn't say the site of {}", if a.site.is_none() { &a.name } else { &b.name })),
        },
        Dimension::Provider => match (&a.provider, &b.provider) {
            (Some(x), Some(y)) if x != y => {}
            (Some(x), Some(_)) => return Err(format!("{} and {} are both at provider {x}", a.name, b.name)),
            _ => return Err(format!("the owner's roster doesn't say the provider of {}", if a.provider.is_none() { &a.name } else { &b.name })),
        },
        Dimension::AdminReach => {
            if a.admin_reach.is_empty() || b.admin_reach.is_empty() {
                return Err(format!("the owner's roster doesn't say who holds root on {}", if a.admin_reach.is_empty() { &a.name } else { &b.name }));
            }
            if let Some(x) = a.admin_reach.iter().find(|r| b.admin_reach.contains(r)) {
                return Err(format!("{x} holds root on both {} and {}", a.name, b.name));
            }
        }
        Dimension::Owner => {
            return Err("every node of one seat is one owner's: owner independence needs a verify node under a second owner (not offered yet)".into());
        }
    }
    Ok(())
}

fn dim_name(d: Dimension) -> &'static str {
    match d {
        Dimension::Node => "node",
        Dimension::Host => "host",
        Dimension::Site => "site",
        Dimension::Provider => "provider",
        Dimension::AdminReach => "admin_reach",
        Dimension::Owner => "owner",
    }
}

/// Place a release set on a seat. `fit(build, node_id)` is the normal job
/// path's answer for that build on that node (`Err` = why not). Dual builds
/// first (by build name), then single ones.
pub fn place<F>(nodes: &[SeatNode], asks: &[Ask], fit: F) -> Result<Vec<Placed>, Refused>
where
    F: Fn(&str, &str) -> Result<Fit, String>,
{
    if asks.is_empty() {
        return Err(refused(codes::NO_RELEASE, "the release names no build"));
    }
    let usable: Vec<(&SeatNode, &RosterEntry)> = nodes.iter().filter(|n| n.out.is_none()).filter_map(|n| n.entry.as_ref().map(|e| (n, e))).collect();
    let outs = || -> String {
        let v: Vec<String> = nodes.iter().filter_map(|n| n.out.as_ref().map(|o| format!("{}: {o}", n.name))).collect();
        if v.is_empty() {
            String::new()
        } else {
            format!(" ({})", v.join("; "))
        }
    };
    if usable.is_empty() {
        return Err(refused(codes::SEAT_DEGRADED, format!("no node of the seat can take release builds now{}", outs())));
    }
    let mut used: BTreeMap<String, u32> = BTreeMap::new();
    let mut out: Vec<Placed> = Vec::new();
    let mut order: Vec<&Ask> = asks.iter().collect();
    order.sort_by_key(|a| (a.verify_label.is_none(), a.build.clone()));
    for ask in order {
        // The nodes that can take this build, with how soon.
        let mut cands: Vec<(&SeatNode, &RosterEntry, Fit)> = Vec::new();
        let mut why_not: Vec<String> = Vec::new();
        for (n, e) in &usable {
            match fit(&ask.build, &n.node_id) {
                Ok(f) => cands.push((n, e, f)),
                Err(w) => why_not.push(format!("{}: {w}", n.name)),
            }
        }
        let skipped = |extra: &[String]| -> String {
            let mut all: Vec<String> = nodes.iter().filter_map(|n| n.out.as_ref().map(|o| format!("{}: {o}", n.name))).collect();
            all.extend(extra.iter().cloned());
            if all.is_empty() {
                String::new()
            } else {
                format!(" ({})", all.join("; "))
            }
        };
        let load = |id: &str, used: &BTreeMap<String, u32>| used.get(id).copied().unwrap_or(0);
        let rank = |f: &Fit| (u8::from(!f.starts_now), f.eta_secs);
        match &ask.verify_label {
            Some(vlabel) => {
                if cands.len() < 2 {
                    return Err(refused(
                        codes::SEAT_DEGRADED,
                        format!("build {} is a dual build and needs two seat nodes that can take it; {} can{}", ask.build, cands.len(), skipped(&why_not)),
                    ));
                }
                // Every ordered pair that qualifies; the best by load in this
                // set, then who starts soonest, then names (stable).
                let mut best: Option<((u32, u8, u64, String, String), usize, usize)> = None;
                let mut fails: BTreeMap<Dimension, String> = BTreeMap::new();
                for (i, (bn, be, bf)) in cands.iter().enumerate() {
                    for (k, (vn, ve, vf)) in cands.iter().enumerate() {
                        if i == k {
                            continue;
                        }
                        let bad = ask.independence.iter().find_map(|d| independent_of(*d, be, ve).err().map(|w| (*d, w)));
                        if let Some((d, w)) = bad {
                            fails.entry(d).or_insert(w);
                            continue;
                        }
                        let (rb, rv) = (rank(bf), rank(vf));
                        let key = (load(&bn.node_id, &used) + load(&vn.node_id, &used), rb.0.max(rv.0), rb.1.saturating_add(rv.1), bn.name.clone(), vn.name.clone());
                        // The build job takes the sooner of the two.
                        let (i, k) = if rv < rb { (k, i) } else { (i, k) };
                        if best.as_ref().is_none_or(|(bk, _, _)| key < *bk) {
                            best = Some((key, i, k));
                        }
                    }
                }
                let Some((_, i, k)) = best else {
                    let dims: Vec<&str> = ask.independence.iter().map(|d| dim_name(*d)).collect();
                    let first = fails.values().next().cloned().unwrap_or_else(|| "no pair qualifies".into());
                    let named: Vec<&str> = fails.keys().map(|d| dim_name(*d)).collect();
                    return Err(refused(
                        codes::NO_INDEPENDENT_NODES,
                        format!(
                            "build {} needs {} and {} on two seat nodes that differ in {}; no two of the seat's nodes that can take it do (fails on {}: {first})",
                            ask.build,
                            ask.label,
                            vlabel,
                            dims.join(", "),
                            named.join(", ")
                        ),
                    ));
                };
                let (bn, _, bf) = &cands[i];
                let (vn, _, vf) = &cands[k];
                let dims: Vec<&str> = ask.independence.iter().map(|d| dim_name(*d)).collect();
                out.push(Placed {
                    build: ask.build.clone(),
                    label: ask.label.clone(),
                    role: Role::Build,
                    node_id: bn.node_id.clone(),
                    node: bn.name.clone(),
                    why: format!("{}; independent of {} ({})", start_words(bf), vn.name, dims.join(", ")),
                });
                out.push(Placed {
                    build: ask.build.clone(),
                    label: vlabel.clone(),
                    role: Role::Verify,
                    node_id: vn.node_id.clone(),
                    node: vn.name.clone(),
                    why: format!("{}; the dual build of {} on {}", start_words(vf), ask.label, bn.name),
                });
                *used.entry(bn.node_id.clone()).or_default() += 1;
                *used.entry(vn.node_id.clone()).or_default() += 1;
            }
            None => {
                let Some((n, _, f)) = cands.iter().min_by_key(|(n, _, f)| (load(&n.node_id, &used), rank(f), n.name.clone())) else {
                    return Err(refused(codes::SEAT_DEGRADED, format!("no seat node can take build {}{}", ask.build, skipped(&why_not))));
                };
                let busy = load(&n.node_id, &used);
                let why = if busy > 0 { format!("{}; every seat node that can take it already has a job of this set, so it queues behind it", start_words(f)) } else { start_words(f) };
                out.push(Placed { build: ask.build.clone(), label: ask.label.clone(), role: Role::Build, node_id: n.node_id.clone(), node: n.name.clone(), why });
                *used.entry(n.node_id.clone()).or_default() += 1;
            }
        }
    }
    Ok(out)
}

fn start_words(f: &Fit) -> String {
    if f.starts_now {
        "starts now".into()
    } else if f.eta_secs > 0 {
        format!("queues (about {} min)", f.eta_secs.div_ceil(60))
    } else {
        "queues".into()
    }
}

/// Why a seat node takes no release build, from its roster entry (`None`:
/// it may). The roster must list the key `active` under a name the seat
/// names, with a taint basis.
pub fn entry_problem(entry: Option<&RosterEntry>, seat_nodes: &[String], revoked: bool) -> Option<String> {
    if revoked {
        return Some("its key is revoked in the owner's roster".into());
    }
    let Some(e) = entry else { return Some("its key isn't in the owner's newest node roster held here".into()) };
    if e.status != crate::compute::proto::ci::KeyStatus::Active {
        return Some(format!("its key is {:?} in the owner's roster", e.status).to_lowercase());
    }
    if !seat_nodes.contains(&e.name) {
        return Some(format!("the seat statement doesn't name {}", e.name));
    }
    if e.taint_basis.is_none() {
        return Some(format!("the owner vouches for nothing on {}'s key ({})", e.name, ccodes::NODE_KEY_TAINTED));
    }
    None
}

// ── the dual-build convenience check ─────────────────────────────────

/// What the controller saw of a dual build's files (a convenience: the
/// signer's policy decides which must match and checks it itself).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DualCompare {
    pub matched: Vec<String>,
    pub differ: Vec<String>,
    /// In one job's files only.
    pub only_one: Vec<String>,
}

/// Compare two jobs' artifacts by name (`name → sha256`).
pub fn dual_compare(build: &BTreeMap<String, String>, verify: &BTreeMap<String, String>) -> DualCompare {
    let mut d = DualCompare::default();
    for (n, h) in build {
        match verify.get(n) {
            Some(v) if v == h => d.matched.push(n.clone()),
            Some(_) => d.differ.push(n.clone()),
            None => d.only_one.push(n.clone()),
        }
    }
    for n in verify.keys().filter(|n| !build.contains_key(*n)) {
        d.only_one.push(n.clone());
    }
    d
}

/// A release version K2 names a run by: `x.y.z` with an optional
/// `-pre`/`+build` suffix of `[0-9A-Za-z.-]`, at most 64 bytes.
pub fn valid_version(v: &str) -> bool {
    let (core, rest) = match v.find(['-', '+']) {
        Some(i) => (&v[..i], &v[i..]),
        None => (v, ""),
    };
    let parts: Vec<&str> = core.split('.').collect();
    v.len() <= 64
        && parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
        && rest.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        && rest != "-"
        && rest != "+"
}

// ── the store ────────────────────────────────────────────────────────

/// A release run's own facts (`ci_runs` of kind `release`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFacts {
    pub version: Option<String>,
    pub project: Option<String>,
    pub tree: Option<String>,
    pub seat_id: Option<String>,
}

/// A release job's facts (`ci_checks` of a release run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFacts {
    pub label: String,
    pub build: String,
    /// `build` | `verify`.
    pub role: String,
    pub node_id: Option<String>,
}

/// Mark a stored run as a release (right after `ci::insert_run`).
pub fn mark_release(conn: &Connection, run_id: &str, version: &str, project: &str, tree: &str, seat_id: &str) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE ci_runs SET kind = 'release', version = ?2, project = ?3, tree = ?4, seat_id = ?5 WHERE id = ?1",
            params![run_id, version, project, tree, seat_id],
        )
        .map_err(|e| format!("ci release: {e}"))?;
    if n != 1 {
        return Err(format!("ci release: no run {run_id}"));
    }
    Ok(())
}

/// Store a release job's row (its name is its label).
pub fn insert_job(conn: &Connection, run_id: &str, p: &Placed, state: &str, description: &str) -> Result<(), String> {
    crate::ci::insert_check(conn, run_id, &p.label, state, description)?;
    let role = match p.role {
        Role::Build => "build",
        Role::Verify => "verify",
    };
    conn.execute(
        "UPDATE ci_checks SET label = ?3, build = ?4, role = ?5, node_id = ?6 WHERE run_id = ?1 AND name = ?2",
        params![run_id, p.label, p.label, p.build, role, p.node_id],
    )
    .map_err(|e| format!("ci release job: {e}"))?;
    Ok(())
}

/// The run's kind and release facts (`None`: no such run).
pub fn release_facts(conn: &Connection, run_id: &str) -> Option<(String, ReleaseFacts)> {
    conn.query_row("SELECT kind, version, project, tree, seat_id FROM ci_runs WHERE id = ?1", params![run_id], |r| {
        Ok((r.get::<_, String>(0)?, ReleaseFacts { version: r.get(1)?, project: r.get(2)?, tree: r.get(3)?, seat_id: r.get(4)? }))
    })
    .ok()
}

/// A release run's jobs' facts, by check name (= label).
pub fn job_facts(conn: &Connection, run_id: &str) -> BTreeMap<String, JobFacts> {
    let Ok(mut st) = conn.prepare("SELECT name, label, build, role, node_id FROM ci_checks WHERE run_id = ?1 AND label IS NOT NULL") else {
        return BTreeMap::new();
    };
    st.query_map(params![run_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            JobFacts { label: r.get(1)?, build: r.get::<_, Option<String>>(2)?.unwrap_or_default(), role: r.get::<_, Option<String>>(3)?.unwrap_or_default(), node_id: r.get(4)? },
        ))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Record which release run built an App release's files (`k2 app updates
/// push --evidence`). A record for people, never a trust input.
pub fn record_app_evidence(conn: &Connection, release_id: &str, run_id: &str, project: Option<&str>, sha: &str, summary: &serde_json::Value, by: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_update_evidence (release_id, run_id, project, sha, summary_json, recorded_by, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(release_id) DO UPDATE SET run_id = ?2, project = ?3, sha = ?4, summary_json = ?5, recorded_by = ?6, recorded_at = ?7",
        params![release_id, run_id, project, sha, summary.to_string(), by, at],
    )
    .map_err(|e| format!("app update evidence: {e}"))?;
    Ok(())
}

/// The evidence record of an App release, if one was given at push.
pub fn app_evidence(conn: &Connection, release_id: &str) -> Option<serde_json::Value> {
    conn.query_row("SELECT run_id, project, sha, summary_json, recorded_by, recorded_at FROM app_update_evidence WHERE release_id = ?1", params![release_id], |r| {
        let summary: String = r.get(3)?;
        Ok(serde_json::json!({
            "run": r.get::<_, String>(0)?,
            "project": r.get::<_, Option<String>>(1)?,
            "sha": r.get::<_, String>(2)?,
            "summary": serde_json::from_str::<serde_json::Value>(&summary).unwrap_or_default(),
            "by": r.get::<_, String>(4)?,
            "at": r.get::<_, i64>(5)?,
        }))
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::ci::{KeyStatus, TaintBasis};

    fn entry(name: &str, host: &str) -> RosterEntry {
        RosterEntry {
            node_fp: format!("{:0>64}", name.len().to_string() + name),
            node_spki: String::new(),
            name: name.into(),
            key_epoch: 0,
            statement_digest: String::new(),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: 0,
            valid_to: None,
            taint_basis: Some(TaintBasis::OwnerAttested),
            attestation_digest: None,
            host: host.into(),
            provider: None,
            site: None,
            admin_reach: Vec::new(),
            pod: None,
            point: None,
        }
    }

    fn node(name: &str, host: &str) -> SeatNode {
        SeatNode { node_id: format!("id-{name}"), name: name.into(), entry: Some(entry(name, host)), out: None }
    }

    fn dual() -> Ask {
        Ask { build: "linux-x86_64".into(), label: "L1".into(), verify_label: Some("V1".into()), independence: vec![Dimension::Node, Dimension::Host] }
    }

    fn single() -> Ask {
        Ask { build: "linux-app".into(), label: "L3".into(), verify_label: None, independence: vec![Dimension::Node] }
    }

    fn now(_: &str, _: &str) -> Result<Fit, String> {
        Ok(Fit { starts_now: true, eta_secs: 0 })
    }

    fn at<'a>(p: &'a [Placed], label: &str) -> &'a Placed {
        p.iter().find(|x| x.label == label).unwrap_or_else(|| panic!("no {label} in {p:?}"))
    }

    #[test]
    fn three_nodes_on_three_hosts_get_one_job_each() {
        let nodes = [node("k2n7", "h7"), node("k2n8", "h8"), node("k2n9", "h9")];
        let p = place(&nodes, &[single(), dual()], now).unwrap();
        assert_eq!(p.len(), 3);
        let (l1, v1, l3) = (at(&p, "L1"), at(&p, "V1"), at(&p, "L3"));
        assert_eq!((l1.role, v1.role, l3.role), (Role::Build, Role::Verify, Role::Build));
        assert_ne!(l1.node_id, v1.node_id);
        assert!(l3.node_id != l1.node_id && l3.node_id != v1.node_id, "L3 on the third node: {p:?}");
        assert!(v1.why.contains("dual build of L1"), "{}", v1.why);
        assert!(l1.why.contains("host"), "{}", l1.why);
    }

    #[test]
    fn two_nodes_queue_l3_behind_one_of_the_pair() {
        let nodes = [node("a", "h1"), node("b", "h2")];
        let p = place(&nodes, &[dual(), single()], now).unwrap();
        let l3 = at(&p, "L3");
        assert!(l3.why.contains("queues behind"), "{}", l3.why);
    }

    #[test]
    fn the_soonest_independent_pair_wins() {
        // a and b share a host; c is elsewhere but busy. The pair must
        // include c, and the build job takes the sooner of its pair.
        let nodes = [node("a", "h1"), node("b", "h1"), node("c", "h2")];
        let fit = |_: &str, id: &str| -> Result<Fit, String> { Ok(if id == "id-c" { Fit { starts_now: false, eta_secs: 600 } } else { Fit { starts_now: true, eta_secs: 0 } }) };
        let p = place(&nodes, &[dual()], fit).unwrap();
        let (l1, v1) = (at(&p, "L1"), at(&p, "V1"));
        assert!(l1.node == "a" || l1.node == "b", "L1 starts now: {p:?}");
        assert_eq!(v1.node, "c");
        assert!(v1.why.contains("queues (about 10 min)"), "{}", v1.why);
    }

    #[test]
    fn refusals_name_the_dimension_and_the_reasons() {
        // Same host: no independent pair.
        let e = place(&[node("a", "h1"), node("b", "h1")], &[dual()], now).unwrap_err();
        assert_eq!(e.code, codes::NO_INDEPENDENT_NODES);
        assert!(e.message.contains("fails on host") && e.message.contains("both on host h1"), "{}", e.message);
        // Site asked, unknown in the roster.
        let mut site = dual();
        site.independence.push(Dimension::Site);
        let e = place(&[node("a", "h1"), node("b", "h2")], &[site], now).unwrap_err();
        assert_eq!(e.code, codes::NO_INDEPENDENT_NODES);
        assert!(e.message.contains("doesn't say the site"), "{}", e.message);
        // Owner: never on one seat.
        let mut owner = dual();
        owner.independence.push(Dimension::Owner);
        let e = place(&[node("a", "h1"), node("b", "h2")], &[owner], now).unwrap_err();
        assert!(e.message.contains("second owner"), "{}", e.message);
        // One node out (tainted), one can't fit: degraded, both reasons named.
        let mut t = node("b", "h2");
        t.out = Some("the owner vouches for nothing".into());
        let fit = |_: &str, id: &str| -> Result<Fit, String> { if id == "id-c" { Err("offline".into()) } else { Ok(Fit { starts_now: true, eta_secs: 0 }) } };
        let e = place(&[node("a", "h1"), t, node("c", "h3")], &[dual()], fit).unwrap_err();
        assert_eq!(e.code, codes::SEAT_DEGRADED);
        assert!(e.message.contains("1 can") && e.message.contains("b: the owner vouches") && e.message.contains("c: offline"), "{}", e.message);
        // Nothing usable at all.
        let mut gone = node("a", "h1");
        gone.out = Some("revoked".into());
        assert_eq!(place(&[gone], &[single()], now).unwrap_err().code, codes::SEAT_DEGRADED);
        assert_eq!(place(&[node("a", "h1")], &[], now).unwrap_err().code, codes::NO_RELEASE);
    }

    #[test]
    fn independence_matches_the_signers_rule() {
        let mut a = entry("a", "h1");
        let mut b = entry("b", "h2");
        assert!(independent_of(Dimension::Node, &a, &b).is_ok());
        assert!(independent_of(Dimension::Node, &a, &a).is_err());
        assert!(independent_of(Dimension::Host, &a, &b).is_ok());
        a.provider = Some("ovh".into());
        b.provider = Some("ovh".into());
        assert!(independent_of(Dimension::Provider, &a, &b).unwrap_err().contains("provider ovh"));
        b.provider = Some("hetzner".into());
        assert!(independent_of(Dimension::Provider, &a, &b).is_ok());
        assert!(independent_of(Dimension::AdminReach, &a, &b).unwrap_err().contains("who holds root"));
        a.admin_reach = vec!["fleet-key".into()];
        b.admin_reach = vec!["fleet-key".into(), "other".into()];
        assert!(independent_of(Dimension::AdminReach, &a, &b).unwrap_err().contains("fleet-key holds root on both"));
        b.admin_reach = vec!["other".into()];
        assert!(independent_of(Dimension::AdminReach, &a, &b).is_ok());
    }

    #[test]
    fn entry_problems() {
        let seat = vec!["a".to_string()];
        let e = entry("a", "h1");
        assert_eq!(entry_problem(Some(&e), &seat, false), None);
        assert!(entry_problem(Some(&e), &seat, true).unwrap().contains("revoked"));
        assert!(entry_problem(None, &seat, false).unwrap().contains("isn't in the owner's newest"));
        assert!(entry_problem(Some(&e), &["b".into()], false).unwrap().contains("doesn't name a"));
        let mut t = e.clone();
        t.taint_basis = None;
        assert!(entry_problem(Some(&t), &seat, false).unwrap().contains("node_key_tainted"));
        let mut r = e;
        r.status = KeyStatus::Retired;
        assert!(entry_problem(Some(&r), &seat, false).unwrap().contains("retired"));
    }

    #[test]
    fn dual_compare_and_versions() {
        let m = |p: &[(&str, &str)]| p.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<BTreeMap<_, _>>();
        let d = dual_compare(&m(&[("app", "1"), ("app.deb", "2"), ("x", "3")]), &m(&[("app", "1"), ("app.deb", "9"), ("y", "4")]));
        assert_eq!(d.matched, vec!["app"]);
        assert_eq!(d.differ, vec!["app.deb"]);
        assert_eq!(d.only_one, vec!["x", "y"]);
        for ok in ["0.1.0", "1.2.3-rc.1", "10.20.30+build.5", "0.46.11"] {
            assert!(valid_version(ok), "{ok}");
        }
        for bad in ["", "1.2", "1.2.3.4", "v1.2.3", "1.2.3-", "1.2.x", "1.2.3 ", "1..3", "1.2.3-a/b"] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn the_store_round_trips() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../drizzle_sql/0273_ci.sql")).unwrap();
        c.execute_batch("CREATE TABLE app_update_releases (id TEXT PRIMARY KEY);").unwrap();
        for stmt in include_str!("../drizzle_sql/0287_ci_release.sql").split("--> statement-breakpoint") {
            c.execute_batch(stmt).unwrap();
        }
        let sha = "a".repeat(40);
        crate::ci::insert_run(
            &c,
            &crate::ci::NewRun { id: "r1", workspace_id: "ws", sha: &sha, git_ref: None, trigger: "manual", state: "running", detail: None, created_by: "owner", tier: "ci", statuses: None, at: 1 },
        )
        .unwrap();
        mark_release(&c, "r1", "1.2.3", "acme", &"b".repeat(40), "seat-1").unwrap();
        assert!(mark_release(&c, "nope", "1.2.3", "acme", "t", "s").is_err());
        let p = Placed { build: "linux-x86_64".into(), label: "L1".into(), role: Role::Build, node_id: "n7".into(), node: "k2n7".into(), why: "starts now".into() };
        insert_job(&c, "r1", &p, "pending", "submitting").unwrap();
        let (k, f) = release_facts(&c, "r1").unwrap();
        assert_eq!((k.as_str(), f.version.as_deref(), f.seat_id.as_deref()), (kind::RELEASE, Some("1.2.3"), Some("seat-1")));
        let jobs = job_facts(&c, "r1");
        assert_eq!(jobs["L1"], JobFacts { label: "L1".into(), build: "linux-x86_64".into(), role: "build".into(), node_id: Some("n7".into()) });
        assert_eq!(crate::ci::checks_of(&c, "r1")[0].name, "L1");
        c.execute("INSERT INTO app_update_releases (id) VALUES ('rel-1')", []).unwrap();
        record_app_evidence(&c, "rel-1", "r1", Some("acme"), &sha, &serde_json::json!({"dualMatched": ["app"]}), "owner", 5).unwrap();
        let ev = app_evidence(&c, "rel-1").unwrap();
        assert_eq!((ev["run"].as_str(), ev["summary"]["dualMatched"][0].as_str()), (Some("r1"), Some("app")));
        assert!(app_evidence(&c, "rel-2").is_none());
    }
}

//! A compute job's own network ask (0.46.14, `k2 compute run --network
//! none`, `--netns-dummy <name>=<addr>/<prefix>`; the rules are
//! [`k2_core::compute::proto::jobnet`]), on the controller side:
//!
//! - [`from_body`]: the ask from a run request, checked (exit 2 on
//!   anything not valid; a job can only narrow its network).
//! - [`ask_skip`]: why a node can't take the job's GPU or network ask,
//!   the one check both placement and a named node use. A node that
//!   doesn't offer what the network ask needs (`job-network`,
//!   `netns-dummy`) is skipped `job_network` ([`skip_words`]).
//! - [`named_refusal`] and [`auto_refusal`]: the run route's refusals for
//!   a named node and for auto-placement with no node left. The GPU ask's
//!   own refusals come first (unchanged), then the network's:
//!   `job_network_needs_update` (409, exit 3). Never a job queued where it
//!   would run with more network.

use k2_core::compute::proto::frames::Offer;
use k2_core::compute::proto::jobnet::{self, codes as jn, JobNet, JobNetMode};
use k2_core::compute::store::GrantRow;

use crate::compute_gpu_ask::{gpu_node_refusal, gpu_refusal, gpu_skip};
use crate::compute_place::{skip, CandReport, Want};
use crate::compute_routes::{no, Refusal, R};

/// The ask from a run request: `network: "none"` and `netnsDummies:
/// ["sewlane0=10.254.254.1/32"]` (repeatable, implies `none`). `on` and
/// `no-lan` are refused, never widened to.
pub(crate) fn from_body(v: &serde_json::Value) -> R<Option<JobNet>> {
    let mode = match v.get("network") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(m)) => match JobNetMode::parse(m) {
            Some(x) => Some(x),
            None if m == "on" || m == "no-lan" || m == "full" => {
                return Err(no(
                    jn::JOB_NETWORK_INVALID,
                    400,
                    format!("--network {m}: a job can only narrow its network, so the one value is --network none (no network, loopback only). Its node's owner decides how much network jobs get there (k2 compute node servers limits <server> --network)"),
                ))
            }
            None => return Err(no(jn::JOB_NETWORK_INVALID, 400, format!("--network {m:?}: the one value is none (no network, loopback only)"))),
        },
        Some(_) => return Err(no(jn::JOB_NETWORK_INVALID, 400, "network is the string \"none\" (--network none)")),
    };
    let dummies = match v.get("netnsDummies") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => {
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                let s = x.as_str().ok_or_else(|| no(jn::JOB_NETWORK_INVALID, 400, "netnsDummies is a list of \"<name>=<address>/<prefix>\" strings (--netns-dummy sewlane0=10.254.254.1/32)"))?;
                out.push(jobnet::parse_dummy(s).map_err(|e| {
                    no(jn::JOB_NETWORK_INVALID, 400, format!("--netns-dummy: {e}"))
                })?);
            }
            out
        }
        Some(_) => {
            return Err(no(
                jn::JOB_NETWORK_INVALID,
                400,
                "netnsDummies is a list of \"<name>=<address>/<prefix>\" strings",
            ))
        }
    };
    if mode.is_none() && dummies.is_empty() {
        return Ok(None);
    }
    // `--netns-dummy` implies `--network none` (its own namespace).
    let n = JobNet {
        mode: mode.unwrap_or(JobNetMode::None),
        dummies,
    };
    jobnet::check(&n)
        .map_err(|e| no(jn::JOB_NETWORK_INVALID, 400, format!("--netns-dummy: {e}")))?;
    Ok(Some(n))
}

/// A node offering `features` can't run `w`'s ask (it lacks a feature the
/// ask needs). No ask: never.
pub fn lacks(w: &Want, features: &[String]) -> bool {
    jobnet::missing_feature(w.network.as_ref(), features).is_some()
}

/// Why a node (behind `offer`) can't take `w`'s asks, or `None`: the GPU
/// ask's code ([`gpu_skip`]) first, then [`skip::JOB_NETWORK`].
pub fn ask_skip(w: &Want, grant: Option<&GrantRow>, offer: &Offer) -> Option<&'static str> {
    gpu_skip(w, grant, offer).or_else(|| lacks(w, &offer.features).then_some(skip::JOB_NETWORK))
}

/// The placement note's words for nodes skipped `job_network`.
pub fn skip_words(w: &Want) -> String {
    let dummies = w.network.as_ref().is_some_and(|n| !n.dummies.is_empty());
    format!(
        "that can't run a job at --network none{} yet (k2-node older than 0.46.14{})",
        if dummies { " with --netns-dummy" } else { "" },
        if dummies { ", or a Mac" } else { "" }
    )
}

/// The hint for `job_network_needs_update` (a named node, or every node
/// this workspace may use).
pub fn needs_update_hint(n: &JobNet, node: Option<&str>) -> String {
    let which = node
        .map(|x| format!("'{x}' can't"))
        .unwrap_or_else(|| "no node this workspace may use can".into());
    if n.dummies.is_empty() {
        format!(
            "--network none: {which} run a job with no network yet: that needs k2-node 0.46.14 there and its job wall (Linux: sudo k2-node setup; macOS: Seatbelt). Its owner updates k2-node. K2 never runs the job with more network instead"
        )
    } else {
        format!(
            "--netns-dummy: {which} make a job's own network namespace with dummy interfaces yet: that needs a Linux node with k2-node 0.46.14 and its job wall (Macs can't). Its owner updates k2-node. K2 never runs the job with more network instead"
        )
    }
}

/// A named (or `--same-node-as`) node that can't take `w`: the GPU ask's
/// refusal first, then the network ask's. A node never seen online can't
/// say what it offers, so a network ask is refused there too.
pub(crate) fn named_refusal(
    w: &Want,
    grant: Option<&GrantRow>,
    offer: Option<&Offer>,
    node: &str,
    ws: &str,
) -> Option<Refusal> {
    let code = match offer {
        Some(o) => ask_skip(w, grant, o)?,
        None if w.gpu.is_some() => skip::NO_GPU,
        None if w.network.is_some() => skip::JOB_NETWORK,
        None => return None,
    };
    match (code, w.network.as_ref()) {
        (skip::JOB_NETWORK, Some(n)) => Some(
            no(
                jn::JOB_NETWORK_NEEDS_UPDATE,
                409,
                needs_update_hint(n, Some(node)),
            )
            .with("node", node.to_string()),
        ),
        _ => Some(gpu_node_refusal(code, node, ws)),
    }
}

/// Auto-placement with no node left: the GPU ask's refusal first
/// ([`gpu_refusal`]), then `job_network_needs_update` when some
/// node was left out only for the network ask. `(code, status, hint,
/// node)`.
pub(crate) fn auto_refusal(
    w: &Want,
    report: &CandReport,
    ws_handle: &str,
) -> Option<(&'static str, u16, String, Option<String>)> {
    if let Some(r) = gpu_refusal(w, report, ws_handle) {
        return Some(r);
    }
    let n = w.network.as_ref()?;
    if !report.cands.is_empty() || report.skipped.get(skip::JOB_NETWORK).copied().unwrap_or(0) == 0
    {
        return None;
    }
    Some((
        jn::JOB_NETWORK_NEEDS_UPDATE,
        409,
        needs_update_hint(n, None),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::compute::proto::jobnet::{Dummy, FEATURE_JOB_NETWORK, FEATURE_NETNS_DUMMY};

    fn want(network: Option<JobNet>) -> Want {
        Want {
            ws_id: "ws".into(),
            admin: false,
            auto: true,
            filter: Default::default(),
            needs: vec![],
            artifacts: false,
            exclusive: false,
            needs_git: false,
            cpu_millis: None,
            mem_bytes: Some(2 << 30),
            mem_defaulted: false,
            timeout_secs: None,
            stall: false,
            secret_ids: vec![],
            tier: k2_core::compute::proto::tiers::Tier::Normal,
            cache: None,
            multi_repo: false,
            receipt_v2: false,
            gpu: None,
            gpu_mem: None,
            network,
        }
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

    /// `network: "none"` and `netnsDummies` from a run request; a job can
    /// only narrow its network, dummies imply none, and every name and
    /// address breaking the rules is exit 2 (`job_network_invalid`).
    #[test]
    fn network_asks_are_checked_and_only_ever_narrow() {
        assert_eq!(
            from_body(&serde_json::json!({})).unwrap(),
            None,
            "no ask: the node's own setting"
        );
        assert_eq!(
            from_body(&serde_json::json!({"network": null, "netnsDummies": []})).unwrap(),
            None
        );
        assert_eq!(
            from_body(&serde_json::json!({"network": "none"})).unwrap(),
            Some(JobNet::none())
        );
        let d = from_body(
            &serde_json::json!({"netnsDummies": ["sewlane0=10.254.254.1/32", "v6=fd00:0::1/128"]}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            d,
            JobNet {
                mode: JobNetMode::None,
                dummies: vec![
                    Dummy {
                        name: "sewlane0".into(),
                        addr: "10.254.254.1/32".into()
                    },
                    Dummy {
                        name: "v6".into(),
                        addr: "fd00::1/128".into()
                    }
                ]
            },
            "dummies imply none; addresses canonical"
        );
        for (body, says) in [
            (serde_json::json!({"network": "on"}), "only narrow"),
            (serde_json::json!({"network": "no-lan"}), "only narrow"),
            (
                serde_json::json!({"network": "bogus"}),
                "the one value is none",
            ),
            (serde_json::json!({"network": 1}), "the string"),
            (
                serde_json::json!({"netnsDummies": "sewlane0=10.0.0.1/32"}),
                "a list",
            ),
            (serde_json::json!({"netnsDummies": [5]}), "a list"),
            (
                serde_json::json!({"netnsDummies": ["sewlane0=8.8.8.8/32"]}),
                "never a public address",
            ),
            (
                serde_json::json!({"netnsDummies": ["lo=10.0.0.1/32"]}),
                "loopback",
            ),
            (
                serde_json::json!({"netnsDummies": ["much-too-long-name=10.0.0.1/32"]}),
                "1-15",
            ),
            (
                serde_json::json!({"netnsDummies": ["sewlane0"]}),
                "<name>=<address>/<prefix>",
            ),
            (
                serde_json::json!({"netnsDummies": ["a=10.0.0.1/32", "a=10.0.0.2/32"]}),
                "named twice",
            ),
            (
                serde_json::json!({"netnsDummies": ["a=10.0.0.1/32", "b=10.0.0.2/32", "c=10.0.0.3/32", "d=10.0.0.4/32", "e=10.0.0.5/32"]}),
                "at most 4",
            ),
        ] {
            let e = from_body(&body).expect_err(says);
            assert_eq!((e.code, e.status), (jn::JOB_NETWORK_INVALID, 400), "{body}");
            assert!(e.hint.contains(says), "{body}: {}", e.hint);
        }
    }

    /// `--network none` needs `job-network` on the node, `--netns-dummy`
    /// `netns-dummy` too (a Mac never offers it); placement requires them
    /// (so a mover never picks such a node either), and with no node left
    /// the job is refused `job_network_needs_update` (409). A job without
    /// the ask sees every node as before.
    #[test]
    fn network_asks_place_only_on_nodes_that_offer_them() {
        let plain: Vec<String> = vec!["artifacts".into()];
        let mac = vec![FEATURE_JOB_NETWORK.to_string()];
        let linux = vec![
            FEATURE_JOB_NETWORK.to_string(),
            FEATURE_NETNS_DUMMY.to_string(),
        ];
        let none = want(Some(JobNet::none()));
        let dummies = want(Some(sew()));
        assert!(!lacks(&want(None), &plain), "no ask: every node");
        assert!(lacks(&none, &plain) && !lacks(&none, &mac));
        assert!(lacks(&dummies, &mac), "a Mac has no dummy interfaces");
        assert!(!lacks(&dummies, &linux));
        let req = crate::compute_place::required_features(&none);
        assert!(req.contains(&FEATURE_JOB_NETWORK) && !req.contains(&FEATURE_NETNS_DUMMY));
        assert!(crate::compute_place::required_features(&dummies).contains(&FEATURE_NETNS_DUMMY));
        assert!(
            !crate::compute_place::required_features(&want(None)).contains(&FEATURE_JOB_NETWORK)
        );
        // No node left, some only for the ask: refused with the update hint.
        let mut r = CandReport {
            visible: 2,
            ..Default::default()
        };
        r.skipped.insert(skip::JOB_NETWORK, 2);
        let (code, status, hint, node) = auto_refusal(&dummies, &r, "ws").expect("refused");
        assert_eq!(
            (code, status, node),
            (jn::JOB_NETWORK_NEEDS_UPDATE, 409, None)
        );
        assert!(
            hint.contains("--netns-dummy")
                && hint.contains("0.46.14")
                && hint.contains("never runs the job with more network"),
            "{hint}"
        );
        let (_, _, hint, _) = auto_refusal(&none, &r, "ws").expect("refused");
        assert!(
            hint.starts_with(
                "--network none: no node this workspace may use can run a job with no network yet"
            ),
            "{hint}"
        );
        assert_eq!(
            auto_refusal(&want(None), &r, "ws"),
            None,
            "no ask: the usual no_node_fits"
        );
        let mut other = CandReport {
            visible: 2,
            ..Default::default()
        };
        other.skipped.insert(skip::OFFLINE, 2);
        assert_eq!(
            auto_refusal(&none, &other, "ws"),
            None,
            "left out for another reason"
        );
        assert!(
            crate::compute_place::skip_words(skip::JOB_NETWORK, 2, &dummies)
                .contains("--network none with --netns-dummy")
        );
        assert!(needs_update_hint(&JobNet::none(), Some("k2n7")).contains("'k2n7' can't run"));
        // A named node never seen online: refused (it can't say).
        let e = named_refusal(&none, None, None, "k2n7", "ws").expect("refused");
        assert_eq!((e.code, e.status), (jn::JOB_NETWORK_NEEDS_UPDATE, 409));
        assert!(
            named_refusal(&want(None), None, None, "k2n7", "ws").is_none(),
            "no ask, no GPU: nothing to refuse"
        );
    }
}

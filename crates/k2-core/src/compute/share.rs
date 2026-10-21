//! Shared nodes, server side (prd-compute-shared-queue-v1 §6, §7, §9).
//!
//! Pure rules the daemon's placement, sweeper, routes and CLI agree on.
//! The node is the truth for its queue and its servers; this server only
//! mirrors what the node's offer says about its own share
//! (`Offer.share`, protocol 3) and, on the owner server, the node's
//! server list (`servers.list`).
//!
//! **Solo nodes take none of these paths.** A node is "shared" for this
//! server only when its offer carries a share with `shared = true` and the
//! `queue` feature: then jobs go into the node's queue at once, `--any`
//! reads the node's quotes, and a queued job is moved only after the node
//! confirms a withdraw (V46). Anything else is today's code path.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::proto::frames::Offer;
use super::proto::share::{self, JobClass, Quote, Role, ServerStatus, ShareLimits, ShareView, Trust};

/// A job withdrawn once can't be withdrawn again for this long (V71).
pub const WITHDRAW_AGAIN_SECS: i64 = 300;
/// …and at most this many times in all (V71).
pub const MAX_WITHDRAWS: i64 = 2;
/// Shards over this many are `batch` (§7 rule 3).
pub const BATCH_SHARDS_OVER: u32 = 4;
/// Clock-skew note threshold (§14).
pub const SKEW_NOTE_SECS: i64 = 120;
/// The expiry note goes out this long before a share ends (§4.3).
pub const EXPIRY_NOTE_SECS: i64 = 24 * 3600;

/// Markers the server keeps in a node-queued job's `reason` while it waits
/// for the node's answer to an "only if not started" cancel (V57). The job
/// stays `assigned` + `node_queued` until the node answers.
pub mod pending {
    /// The `--any` mover asked the node to give the job back (§6 step 4).
    pub const WITHDRAW: &str = "withdraw_requested";
    /// `--queue-timeout` ran out on a node-queued job: withdraw, then fail.
    pub const TIMEOUT: &str = "withdraw_timeout";
    /// `k2 compute cancel` without `--force` on a node-queued job.
    pub const CANCEL: &str = "cancel_if_queued";

    pub fn any(reason: Option<&str>) -> bool {
        matches!(reason, Some(WITHDRAW) | Some(TIMEOUT) | Some(CANCEL))
    }
}

/// This session's share on the node, when the node sent one.
pub fn view(offer: &Offer) -> Option<&ShareView> {
    offer.share.as_ref()
}

/// The node keeps a queue for this server (`queue` feature).
pub fn queue_on(offer: &Offer) -> bool {
    offer.features.iter().any(|f| f == share::FEATURE_QUEUE)
}

/// The node is shared right now: jobs go into its queue at once, and moves
/// need a confirmed withdraw. Solo nodes (no share, or `shared = false`)
/// keep today's path.
pub fn is_shared(offer: &Offer) -> bool {
    queue_on(offer) && view(offer).is_some_and(|v| v.shared)
}

/// Another server owns this node: this server never sends it node admin,
/// updates or health reads (V52).
pub fn is_tenant(offer: &Offer) -> bool {
    view(offer).is_some_and(|v| v.role == Role::Tenant)
}

/// The class the server sets from facts (§7 rule 3): `--gate` (where the
/// share allows it) is `gate`; `--detach`, a split run over
/// [`BATCH_SHARDS_OVER`] shards and soaks are `batch`; an attached run or
/// `--wait` is `interactive`.
pub fn class_for(gate: bool, detach: bool, shards: Option<u32>, soak: bool) -> JobClass {
    if gate {
        JobClass::Gate
    } else if detach || soak || shards.is_some_and(|n| n > BATCH_SHARDS_OVER) {
        JobClass::Batch
    } else {
        JobClass::Interactive
    }
}

/// What one job asks of a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareAsk {
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub max_secs: u64,
    pub exclusive: bool,
    pub class: JobClass,
}

/// The submit-time check against this server's mirrored share (§7
/// "Refused outright"; the node checks again at assign). `Err((code,
/// message))`. A class above the share's `top_class` (not `gate`) is
/// lowered, never refused: see [`clamp_class`].
pub fn share_refusal(l: &ShareLimits, a: &ShareAsk) -> Option<(&'static str, String)> {
    if a.class == JobClass::Gate && !l.allows_class(JobClass::Gate) {
        return Some((share::codes::GATE_NOT_ALLOWED, "this server's share on the node doesn't allow gate jobs".into()));
    }
    if a.exclusive && !l.exclusive {
        return Some((share::codes::EXCLUSIVE_NOT_ALLOWED, "this server's share on the node doesn't allow exclusive jobs".into()));
    }
    if a.max_secs > l.max_job_secs {
        return Some((
            share::codes::EXCEEDS_SHARE_CAPS,
            format!("--timeout {} is over this server's share of {} per job", human_secs(a.max_secs), human_secs(l.max_job_secs)),
        ));
    }
    if let Some(c) = a.cpu_millis.filter(|c| *c > u64::from(l.max_cpus) * 1000) {
        return Some((
            share::codes::EXCEEDS_SHARE_CAPS,
            format!("--cpus {} is over this server's share of {} CPUs per job", c as f64 / 1000.0, l.max_cpus),
        ));
    }
    if let Some(m) = a.mem_bytes.filter(|m| *m > u64::from(l.max_mem_gb) << 30) {
        return Some((
            share::codes::EXCEEDS_SHARE_CAPS,
            format!("--mem {:.1}G is over this server's share of {} GB per job", m as f64 / (1u64 << 30) as f64, l.max_mem_gb),
        ));
    }
    None
}

/// A class the share doesn't reach is lowered to its top class (a batch
/// share runs interactive asks as batch). `gate` is refused instead
/// ([`share_refusal`]).
pub fn clamp_class(l: &ShareLimits, class: JobClass) -> JobClass {
    if class == JobClass::Gate || l.allows_class(class) {
        class
    } else {
        l.top_class.min(JobClass::Interactive)
    }
}

/// The node's quote for this exact ask (`None` = no quote yet: the node
/// quotes a tenant's last four distinct asks). A missing size is 0.
pub fn quote_for(v: &ShareView, cpu_millis: Option<u64>, mem_bytes: Option<u64>) -> Option<&Quote> {
    let (c, m) = (cpu_millis.unwrap_or(0), mem_bytes.unwrap_or(0));
    v.quotes.iter().find(|q| q.cpu_millis == c && q.mem_bytes == m)
}

/// Can a job of this size start now on a shared node? The quote when the
/// node sent one; else this server's own count against its share (a
/// running slot free, nothing of ours queued there).
pub fn starts_now(v: &ShareView, cpu_millis: Option<u64>, mem_bytes: Option<u64>) -> bool {
    if v.state != ServerStatus::Active {
        return false;
    }
    match quote_for(v, cpu_millis, mem_bytes) {
        Some(q) => q.starts_now,
        None => v.running < v.limits.max_running && v.queued == 0,
    }
}

/// The ETA (seconds) a shared node quotes for this ask; `None` = unknown
/// (no quote, or blocked).
pub fn eta_for(v: &ShareView, cpu_millis: Option<u64>, mem_bytes: Option<u64>) -> Option<u64> {
    let q = quote_for(v, cpu_millis, mem_bytes)?;
    if q.starts_now {
        Some(0)
    } else {
        q.eta_secs
    }
}

/// Room this server has in the node's queue for more jobs: its share's
/// running + queued limits minus what it already has there (§6 step 4:
/// a full tenant queue keeps jobs waiting on the server, never fails them).
pub fn room(v: &ShareView, on_node: u32) -> u32 {
    (v.limits.max_running + v.limits.max_queued).saturating_sub(on_node)
}

/// May the mover withdraw this job (V71): not again within
/// [`WITHDRAW_AGAIN_SECS`], at most [`MAX_WITHDRAWS`] in all.
pub fn may_withdraw(withdraws: i64, withdrawn_at: Option<i64>, now: i64) -> bool {
    withdraws < MAX_WITHDRAWS && withdrawn_at.is_none_or(|t| now - t >= WITHDRAW_AGAIN_SECS)
}

/// 7200 → "2h", 5400 → "90m", 45 → "45s".
pub fn human_secs(s: u64) -> String {
    if s >= 3600 && s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// "~6m", "~1h05m" (ETAs are durations, never clock times).
pub fn human_eta(secs: u64) -> String {
    if secs < 60 {
        "<1m".into()
    } else if secs < 3600 {
        format!("~{}m", secs.div_ceil(60))
    } else {
        format!("~{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// `running 1/1 · queued 2/10 · today 3.1 of 8 CPU-h · max 2h/job · 4 CPUs · 8 GB` (§4.2).
pub fn share_line(v: &ShareView) -> String {
    let l = &v.limits;
    format!(
        "running {}/{} · queued {}/{} · today {:.1} of {} CPU-h · max {}/job · {} CPUs · {} GB",
        v.running,
        l.max_running,
        v.queued,
        l.max_queued,
        v.cpu_secs_today as f64 / 3600.0,
        l.cpu_hours_day,
        human_secs(l.max_job_secs),
        l.max_cpus,
        l.max_mem_gb
    )
}

/// What a blocked-by code means, for `get` and `queue` (§4.2).
pub fn blocked_words(code: &str) -> String {
    match code {
        share::blocked::FOREIGN_LOCK => "the node is busy with work outside K2 (a lock file)".into(),
        share::blocked::PAUSED => "the node is paused".into(),
        share::blocked::SHARE_PAUSED => "the node's owner paused this server's share".into(),
        share::blocked::DISK_LOW => "the node is low on disk".into(),
        share::blocked::SERVER_OFFLINE => "a server is offline".into(),
        share::blocked::OVER_QUOTA => "this server is over its CPU-hours for today; it runs after the others".into(),
        share::blocked::INSTALLING_TOOLS => "the node is installing a toolchain".into(),
        share::codes::TENANT_DISK_FULL => "this server's disk budget on the node is full".into(),
        share::codes::TENANT_WALL_MISSING => "the node's wall for this server is missing (its owner is told)".into(),
        share::codes::FAIR_SHARE_WAIT => "fair share: another server's jobs run first".into(),
        share::codes::TENANT_QUEUE_FULL => "this server's queue on the node is full".into(),
        other => other.replace('_', " "),
    }
}

// ── the owner's mirror of a node's servers (0216) ────────────────────

/// One server on a node this server owns (`compute_node_servers`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRow {
    pub slot: u8,
    pub label: String,
    /// The full fingerprint once enrolled, else the code's 16-hex prefix.
    pub fp: Option<String>,
    pub state: String,
    pub trust: String,
    pub limits: ShareLimits,
    /// Today's usage and counts as the node reports them (running, queued,
    /// cpu_secs_today, …), plus the pending SAS and a refusal code.
    pub usage: Option<Value>,
    pub expires_at: Option<i64>,
    pub added_at: Option<i64>,
    pub added_by: Option<String>,
}

fn state_ok(s: &str) -> bool {
    matches!(s, "pending" | "active" | "paused" | "expired" | "revoked" | "refused")
}

/// Parse a `servers.list` answer (`{servers: [...]}`). Each entry is a
/// `servers.toml` line as the node lists it: `slot`, `label`, `state` (a
/// [`ServerStatus`]), `trust`, `fp` or `fp_prefix`, the share's limits
/// (a `limits` object, or flattened like `servers.toml`), `expires_at`,
/// `added_at`, `added_by`, and the runtime facts (`running`, `queued`,
/// `cpu_secs_today`, `sas`, `refusal`) kept as `usage`. Entries that
/// don't parse are an error, never skipped (a broken list isn't mirrored).
pub fn parse_servers_list(result: &Value) -> Result<Vec<ServerRow>, String> {
    let list = result.get("servers").and_then(Value::as_array).ok_or("servers.list: no servers list")?;
    let mut out = Vec::new();
    for e in list {
        let slot = e.get("slot").and_then(Value::as_u64).filter(|s| *s < u64::from(share::MAX_SLOTS)).ok_or("servers.list: a server without a slot 0–7")?
            as u8;
        let label = e.get("label").and_then(Value::as_str).filter(|l| share::valid_label(l)).ok_or("servers.list: a server without a plain label")?;
        let state = e.get("state").and_then(Value::as_str).filter(|s| state_ok(s)).ok_or_else(|| format!("servers.list: {label} has no known state"))?;
        let trust = e.get("trust").and_then(Value::as_str).and_then(Trust::parse).ok_or_else(|| format!("servers.list: {label} has no trust"))?;
        let limits: ShareLimits = match e.get("limits") {
            Some(l) => serde_json::from_value(l.clone()),
            None => serde_json::from_value(e.clone()),
        }
        .map_err(|err| format!("servers.list: {label}'s limits: {err}"))?;
        let fp = e.get("fp").or_else(|| e.get("fp_prefix")).and_then(Value::as_str).map(str::to_string);
        let mut usage = serde_json::Map::new();
        for k in ["running", "queued", "cpu_secs_today", "wall_secs_today", "jobs_today", "sas", "refusal", "last_seen", "pending_until", "uid"] {
            if let Some(v) = e.get(k).filter(|v| !v.is_null()) {
                usage.insert(k.to_string(), v.clone());
            }
        }
        if let Some(Value::Object(u)) = e.get("usage") {
            for (k, v) in u {
                usage.insert(k.clone(), v.clone());
            }
        }
        out.push(ServerRow {
            slot,
            label: label.to_string(),
            fp,
            state: state.to_string(),
            trust: trust.as_str().to_string(),
            limits,
            usage: (!usage.is_empty()).then_some(Value::Object(usage)),
            expires_at: e.get("expires_at").and_then(Value::as_i64),
            added_at: e.get("added_at").and_then(Value::as_i64),
            added_by: e.get("added_by").and_then(Value::as_str).map(str::to_string),
        });
    }
    Ok(out)
}

/// `CODE.FP` (`ABCDE-FGHJK.0123456789abcdef`, §4.1) → `(code_binding,
/// fp_prefix)`. The code itself never leaves this function: only its
/// binding goes to the node, and it is treated as a credential (V79).
pub fn split_add_code(code_fp: &str) -> Result<(String, String), String> {
    let e = super::proto::pairing::EnrollString::parse(code_fp)
        .ok_or("the code is CODE.FP (ABCDE-FGHJK.0123456789abcdef), as `k2 compute node add` printed it on the new server")?;
    if !share::valid_fp_prefix(&e.controller_fp_prefix) {
        return Err("the part after the dot is the new server's 16-hex key prefix".into());
    }
    Ok((super::proto::pairing::code_binding(&e.code), e.controller_fp_prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn limits() -> ShareLimits {
        ShareLimits::new_server(Trust::Team, false)
    }

    fn view(quotes: Vec<Quote>, running: u32, queued: u32) -> ShareView {
        ShareView {
            role: Role::Tenant,
            slot: 1,
            shared: true,
            servers: 2,
            owner_label: Some("rosson.k2.dev".into()),
            state: ServerStatus::Active,
            limits: limits(),
            running,
            queued,
            cpu_secs_today: 3 * 3600 + 360,
            expires_in_secs: Some(86_400),
            quotes,
        }
    }

    #[test]
    fn class_comes_from_facts() {
        assert_eq!(class_for(false, false, None, false), JobClass::Interactive);
        assert_eq!(class_for(false, true, None, false), JobClass::Batch, "--detach is batch");
        assert_eq!(class_for(false, false, Some(4), false), JobClass::Interactive, "4 shards stay interactive");
        assert_eq!(class_for(false, false, Some(5), false), JobClass::Batch, "over 4 shards is batch");
        assert_eq!(class_for(false, false, None, true), JobClass::Batch, "soaks are batch");
        assert_eq!(class_for(true, true, None, false), JobClass::Gate, "--gate wins");
    }

    #[test]
    fn share_caps_refuse_at_submit() {
        let l = limits();
        let ok = ShareAsk { cpu_millis: Some(4000), mem_bytes: Some(8 << 30), max_secs: 7200, exclusive: false, class: JobClass::Interactive };
        assert_eq!(share_refusal(&l, &ok), None);
        let r = |a: ShareAsk| share_refusal(&l, &a).map(|(c, _)| c);
        assert_eq!(r(ShareAsk { max_secs: 7201, ..ok }), Some("exceeds_share_caps"));
        assert_eq!(r(ShareAsk { cpu_millis: Some(4001), ..ok }), Some("exceeds_share_caps"));
        assert_eq!(r(ShareAsk { mem_bytes: Some((8 << 30) + 1), ..ok }), Some("exceeds_share_caps"));
        assert_eq!(r(ShareAsk { exclusive: true, ..ok }), Some("exclusive_not_allowed"));
        assert_eq!(r(ShareAsk { class: JobClass::Gate, ..ok }), Some("gate_not_allowed"));
        let owner = ShareLimits::owner(2, 8, 16, 100, share::Network::On);
        assert_eq!(share_refusal(&owner, &ShareAsk { class: JobClass::Gate, exclusive: true, ..ok }), None);
        let batch_only = ShareLimits { top_class: JobClass::Batch, ..l.clone() };
        assert_eq!(clamp_class(&batch_only, JobClass::Interactive), JobClass::Batch);
        assert_eq!(clamp_class(&l, JobClass::Batch), JobClass::Batch);
    }

    #[test]
    fn quotes_say_starts_now_and_eta() {
        let q = |c, m, now, eta| Quote { cpu_millis: c, mem_bytes: m, starts_now: now, eta_secs: eta, eta_p90_secs: None, blocked_by: None };
        let v = view(vec![q(4000, 0, false, Some(360)), q(0, 0, true, None)], 1, 0);
        assert!(starts_now(&v, None, None), "the (0, 0) quote");
        assert!(!starts_now(&v, Some(4000), None));
        assert_eq!(eta_for(&v, Some(4000), None), Some(360));
        assert_eq!(eta_for(&v, None, None), Some(0));
        // No quote for this ask: our own count against the share.
        assert!(!starts_now(&v, Some(2000), None), "running 1/1");
        let free = view(vec![], 0, 0);
        assert!(starts_now(&free, Some(2000), None));
        assert_eq!(eta_for(&free, Some(2000), None), None);
        let mut paused = free.clone();
        paused.state = ServerStatus::Paused;
        assert!(!starts_now(&paused, None, None));
        assert_eq!(room(&v, 3), 8, "1 running + 10 queued − 3 on the node");
        assert_eq!(room(&v, 20), 0);
    }

    #[test]
    fn withdraws_are_limited() {
        assert!(may_withdraw(0, None, 1000));
        assert!(!may_withdraw(1, Some(900), 1000), "again within 5 min");
        assert!(may_withdraw(1, Some(700), 1000));
        assert!(!may_withdraw(2, Some(0), 10_000), "at most twice in all");
    }

    #[test]
    fn share_line_reads_like_the_prd() {
        assert_eq!(
            share_line(&view(vec![], 1, 2)),
            "running 1/1 · queued 2/10 · today 3.1 of 8 CPU-h · max 2h/job · 4 CPUs · 8 GB"
        );
        assert_eq!(human_eta(360), "~6m");
        assert_eq!(human_eta(3900), "~1h05m");
    }

    #[test]
    fn servers_list_parses_flat_and_nested_limits_and_fails_loudly() {
        let l = serde_json::to_value(limits()).unwrap();
        let mut flat = l.as_object().unwrap().clone();
        flat.insert("slot".into(), json!(1));
        flat.insert("label".into(), json!("baden.k2.dev"));
        flat.insert("state".into(), json!("pending"));
        flat.insert("trust".into(), json!("team"));
        flat.insert("fp_prefix".into(), json!("0123456789abcdef"));
        flat.insert("sas".into(), json!("482913"));
        flat.insert("expires_at".into(), json!(1_761_600_000));
        let nested = json!({"slot": 0, "label": "rosson.k2.dev", "state": "active", "trust": "own", "fp": "ab".repeat(32),
            "limits": ShareLimits::owner(2, 8, 16, 100, share::Network::On), "running": 1, "queued": 0});
        let rows = parse_servers_list(&json!({"servers": [nested, Value::Object(flat)]})).expect("parses");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].slot, rows[0].trust.as_str(), rows[0].limits.weight), (0, "own", 1));
        assert_eq!(rows[1].fp.as_deref(), Some("0123456789abcdef"));
        assert_eq!(rows[1].usage.as_ref().unwrap()["sas"], "482913");
        assert_eq!(rows[1].expires_at, Some(1_761_600_000));
        assert!(parse_servers_list(&json!({"servers": [{"slot": 9, "label": "x", "state": "active", "trust": "own"}]})).is_err());
        assert!(parse_servers_list(&json!({"servers": [{"slot": 1, "label": "x", "state": "weird", "trust": "own"}]})).is_err());
        assert!(parse_servers_list(&json!({})).is_err());
    }

    #[test]
    fn add_codes_split_into_binding_and_prefix() {
        let (b, fp) = split_add_code("ABCDE-FGHJK.0123456789ABCDEF").expect("splits");
        assert_eq!(fp, "0123456789abcdef");
        assert_eq!(b, super::super::proto::pairing::code_binding("ABCDEFGHJK"), "the binding of the normalized code");
        assert!(!b.contains("ABCDE"), "only the binding travels");
        assert!(split_add_code("ABCDE-FGHJK").is_err());
        assert!(split_add_code("ABCDE-FGHJK.0123").is_err());
    }
}

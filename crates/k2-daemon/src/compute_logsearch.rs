//! `k2 compute logs --search` (prd-compute-blacksmith-class-v1 §4.5.3, slice
//! O1): one string across this workspace's kept job logs (the daemon's
//! journals, 14 days), newest job first, within budgets (500 jobs, 2 GB read,
//! 10 s), saying when a budget cut it short. No index in v1. Matches are
//! program output: the CLI frames them as data. Behind the CI preview with
//! the rest of the slice.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use k2_core::compute::logs;
use rusqlite::params;
use serde_json::{json, Value};

use crate::compute_routes::{ci_gate, need_credential, no, passport_gate, target_ws, Who, R};
use k2_core::cli_name;

/// `GET /cli/compute/logs/search`.
pub const PATH: &str = "/cli/compute/logs/search";

pub const MAX_JOBS: usize = 500;
pub const MAX_BYTES: u64 = 2 << 30;
pub const MAX_TIME: Duration = Duration::from_secs(10);
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;
/// Logs are swept after 14 days (`compute_ws`), so that is the most `--since`
/// can reach.
pub const MAX_SINCE_SECS: i64 = 14 * 86_400;
pub const MIN_NEEDLE: usize = 2;
pub const MAX_NEEDLE: usize = 200;

const BAD: &str = k2_core::compute::codes::BAD_REQUEST;

/// `7d`, `12h`, `30m`, or seconds.
fn since_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'd' => (&s[..s.len() - 1], 86_400),
        'h' => (&s[..s.len() - 1], 3600),
        'm' => (&s[..s.len() - 1], 60),
        's' => (&s[..s.len() - 1], 1),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    num.parse::<i64>().ok().filter(|n| *n > 0).map(|n| n * mul)
}

/// The SQL for `--state`: `failed` = it didn't pass (ran and exited
/// non-zero, or failed, timed out, was interrupted); `passed` = exit 0;
/// else a job state as `k2 compute jobs --state` takes it.
fn state_sql(s: &str) -> Option<&'static str> {
    Some(match s {
        "failed" => "((state = 'done' AND COALESCE(exit_code, 1) != 0) OR state IN ('failed','timeout','interrupted','unknown'))",
        "passed" => "(state = 'done' AND exit_code = 0)",
        "done" => "state = 'done'",
        "cancelled" => "state = 'cancelled'",
        "timeout" => "state = 'timeout'",
        "interrupted" => "state = 'interrupted'",
        "running" => "state IN ('assigned','preparing','running','finishing')",
        _ => return None,
    })
}

/// `GET /cli/compute/logs/search?search=&workspace=&check=&branch=&state=&since=&limit=&fold=1`.
pub(crate) fn handle(who: &Who, params_in: &HashMap<String, String>) -> R<Value> {
    need_credential(who)?;
    passport_gate(who)?;
    ci_gate(concat!(cli_name!(), " compute logs --search"))?;
    let ws = target_ws(who, params_in.get("workspace").map(String::as_str))?;
    let get = |k: &str| params_in.get(k).map(|s| s.as_str()).filter(|s| !s.trim().is_empty());
    let needle = get("search").ok_or_else(|| no(BAD, 400, "--search 'TEXT': the text to find (2-200 characters)"))?;
    if !(MIN_NEEDLE..=MAX_NEEDLE).contains(&needle.chars().count()) || needle.contains('\n') {
        return Err(no(BAD, 400, "--search: 2-200 characters on one line"));
    }
    let fold = get("fold").is_some_and(|f| f == "1" || f == "true");
    let check = get("check");
    if let Some(c) = check {
        if !k2_core::compute::proto::ci::is_name(c) {
            return Err(no(BAD, 400, format!("--check {c:?}: a check name ([a-z0-9._-])")));
        }
    }
    let branch = get("branch");
    if branch.is_some_and(|b| b.len() > 200 || b.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err(no(BAD, 400, "--branch: a branch name"));
    }
    let state = match get("state") {
        None => None,
        Some(s) => Some(state_sql(s).ok_or_else(|| no(BAD, 400, "--state: failed, passed, done, cancelled, timeout, interrupted or running"))?),
    };
    let since = match get("since") {
        None => MAX_SINCE_SECS,
        Some(s) => since_secs(s).filter(|s| *s <= MAX_SINCE_SECS).ok_or_else(|| no(BAD, 400, "--since: like 7d, 12h or 30m (at most 14d: logs are kept 14 days)"))?,
    };
    let limit = match get("limit") {
        None => DEFAULT_LIMIT,
        Some(l) => l.parse::<usize>().ok().filter(|l| (1..=MAX_LIMIT).contains(l)).ok_or_else(|| no(BAD, 400, format!("--limit: 1-{MAX_LIMIT}")))?,
    };
    let now = k2_core::compute::now();
    // Newest first; one more than the budget, to know it cut.
    let ci_only = check.is_some() || branch.is_some();
    let sql = format!(
        "SELECT id, generation, COALESCE(ended_at, started_at, created_at), json_extract(plan_json, '$.k2ctl.tag') FROM compute_jobs \
         WHERE workspace_id = ?1 AND created_at >= ?2 {} {} \
         ORDER BY created_at DESC, rowid DESC LIMIT ?5",
        state.map(|s| format!("AND {s}")).unwrap_or_default(),
        if ci_only {
            "AND id IN (SELECT c.job_id FROM ci_checks c JOIN ci_runs r ON r.id = c.run_id WHERE r.workspace_id = ?1 \
             AND (?3 IS NULL OR c.name = ?3) AND (?4 IS NULL OR r.ref = ?4) AND c.job_id IS NOT NULL)"
        } else {
            "AND (?3 IS NULL OR 1) AND (?4 IS NULL OR 1)"
        }
    );
    let jobs: Vec<(String, i64, i64, Option<String>)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.prepare(&sql)
            .and_then(|mut st| {
                st.query_map(params![ws.id, now - since, check, branch, (MAX_JOBS + 1) as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .map_err(|e| no("internal", 500, format!("log search: {e}")))?
    };
    let t0 = Instant::now();
    let mut matches: Vec<Value> = Vec::new();
    let (mut scanned, mut bytes) = (0usize, 0u64);
    let mut cut: Option<&str> = None;
    for (id, generation, at, tag) in &jobs {
        if scanned >= MAX_JOBS {
            cut = Some("jobs");
            break;
        }
        if bytes >= MAX_BYTES {
            cut = Some("bytes");
            break;
        }
        if t0.elapsed() >= MAX_TIME {
            cut = Some("time");
            break;
        }
        let left = limit - matches.len();
        let s = logs::search(id, *generation, needle, fold, left, MAX_BYTES - bytes);
        scanned += 1;
        bytes += s.bytes;
        for h in s.hits {
            matches.push(json!({
                "job": id,
                "short": k2_core::compute::message::short(id),
                "at": at,
                "check": tag.as_deref().and_then(|t| t.strip_prefix("ci:")),
                "stream": logs::stream_name(h.stream),
                "line": h.line,
            }));
        }
        if matches.len() >= limit {
            cut = Some("limit");
            break;
        }
        if s.cut {
            cut = Some("bytes");
            break;
        }
    }
    Ok(json!({
        "workspace": ws.handle,
        "search": needle,
        "fold": fold,
        "sinceSecs": since,
        "scanned": {"jobs": scanned, "bytes": bytes, "ms": t0.elapsed().as_millis() as u64},
        "cut": cut,
        "matches": matches,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_and_state_words() {
        assert_eq!((since_secs("7d"), since_secs("12h"), since_secs("30m"), since_secs("90")), (Some(604_800), Some(43_200), Some(1800), Some(90)));
        assert_eq!((since_secs("soon"), since_secs("0d"), since_secs("")), (None, None, None));
        assert!(state_sql("failed").unwrap().contains("exit_code"));
        assert!(state_sql("bogus").is_none());
    }
}

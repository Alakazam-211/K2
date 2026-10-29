//! The completion message (§5.4, CN6): facts only, never log text, so a
//! log can't carry a prompt injection into the agent's session.

/// `"6m12s"`-style duration.
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// First 8 characters of a job id (what the CLI prints).
pub fn short(job_id: &str) -> &str {
    &job_id[..job_id.len().min(8)]
}

/// More facts for a completion line (prd-compute-easy-mode-0467). Only
/// controller-made text goes in `detail` (a queue timeout's wait).
#[derive(Debug, Clone, Copy, Default)]
pub struct Extra<'a> {
    /// What a queue timeout waited on (CE-12): the controller's own words.
    pub detail: Option<&'a str>,
    /// CE-5: this server heard of the end this long after the node's own
    /// end time (it was asleep, or the node was cut off).
    pub late_secs: Option<i64>,
    /// CE-4: the Rust version the job ran, when Rust was a need.
    pub rustc: Option<&'a str>,
}

/// Keep only plain characters of a controller- or node-made fact.
fn plain(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// One line for the requesting session when a job ends. `tests` adds the
/// counts (numbers only: test names are program output and stay out).
#[allow(clippy::too_many_arguments)]
pub fn completion_text(
    job_id: &str,
    node: &str,
    state: &str,
    exit_code: Option<i64>,
    signal: Option<i64>,
    reason: Option<&str>,
    secs: Option<i64>,
    tests: Option<&super::proto::results::TestSummary>,
    extra: &Extra<'_>,
) -> String {
    let id = short(job_id);
    // CE-12: a queue timeout never started: what it waited on, and the
    // next step (it has no log to look at).
    if state == "failed" && reason == Some("queue_timeout") {
        let node = node.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>();
        let waited = extra.detail.map(|d| format!(": {}", plain(d, 300))).unwrap_or_default();
        return format!(
            "[compute] job {id} on {node}: failed (queue_timeout){waited}. Next: k2 compute retry {id}, or submit again with a longer --queue-timeout (e.g. 4h) · k2 compute get {id} --json"
        );
    }
    let after = secs.map(|s| format!(" after {}", duration(s))).unwrap_or_default();
    let what = match state {
        "done" => match (exit_code, signal) {
            (Some(c), _) => format!("exit {c}{after}"),
            (None, Some(sig)) => format!("killed by signal {sig}{after}"),
            _ => format!("finished{after}"),
        },
        "failed" => format!("failed ({}){after}", reason.unwrap_or("infrastructure")),
        "timeout" => format!("timed out{after}"),
        "cancelled" => match reason {
            // A shared node ended it (0.46.3): say why, plainly.
            Some("share_expired") => format!("cancelled (share_expired: this server's share on the node ended){after}"),
            Some("share_revoked") => format!("cancelled (share_revoked: the node's owner removed this server){after}"),
            r => format!("cancelled ({}){after}", r.unwrap_or("cancelled")),
        },
        "interrupted" => format!("interrupted ({}){after}", reason.unwrap_or("interrupted")),
        "unknown" => "lost (the node has no record of it; safe to retry)".to_string(),
        other => other.to_string(),
    };
    // Only ids, names and codes we generated or validated go in.
    let node = node.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>();
    let counts = tests
        .map(|t| {
            let mut s = format!("; tests {} passed, {} failed", t.passed, t.failed + t.errors);
            if t.skipped > 0 {
                s.push_str(&format!(", {} skipped", t.skipped));
            }
            s
        })
        .unwrap_or_default();
    let look = match tests {
        Some(t) if !t.passed_all() => format!("k2 compute logs {id} --failures"),
        _ => format!("k2 compute logs {id} --tail 50"),
    };
    let rust = extra.rustc.map(|r| format!(" ({})", plain(r, 60))).unwrap_or_default();
    let late = extra.late_secs.map(|l| format!(" (this server heard of it {} late)", duration(l))).unwrap_or_default();
    format!("[compute] job {id} on {node}: {what}{rust}{counts}{late}. {look} · k2 compute get {id} --json")
}

/// One line when a split run (`--shards`) ends: the combined result.
#[allow(clippy::too_many_arguments)]
pub fn shards_completion_text(
    job_id: &str,
    shards: usize,
    state: &str,
    exit_code: Option<i64>,
    signal: Option<i64>,
    reason: Option<&str>,
    secs: Option<i64>,
    tests: Option<&super::proto::results::TestSummary>,
) -> String {
    let one = completion_text(job_id, "x", state, exit_code, signal, reason, secs, tests, &Extra::default());
    // Same facts, "on <node>" becomes "(<n> shards)".
    one.replacen(" on x:", &format!(" ({shards} shards):"), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(duration(5), "5s");
        assert_eq!(duration(372), "6m12s");
        assert_eq!(duration(7260), "2h01m");
        assert_eq!(
            completion_text("7f3a0000aaaa", "z13flow", "done", Some(1), None, None, Some(372), None, &Extra::default()),
            "[compute] job 7f3a0000 on z13flow: exit 1 after 6m12s. k2 compute logs 7f3a0000 --tail 50 · k2 compute get 7f3a0000 --json"
        );
        assert!(completion_text("j", "n", "failed", None, None, Some("sync_failed"), None, None, &Extra::default()).contains("failed (sync_failed)"));
        assert!(completion_text("j", "n; rm -rf", "unknown", None, None, None, None, None, &Extra::default()).contains("on nrm-rf:"));
        // A shared node ended it (0.46.3).
        assert!(completion_text("j", "n", "cancelled", None, None, Some("share_expired"), None, None, &Extra::default()).contains("share on the node ended"));
        assert!(completion_text("j", "n", "cancelled", None, None, Some("share_revoked"), None, None, &Extra::default()).contains("removed this server"));
        assert!(completion_text("j", "n", "cancelled", None, None, None, None, None, &Extra::default()).contains("cancelled (cancelled)"));
        assert_eq!(
            shards_completion_text("7f3a0000aaaa", 4, "done", Some(0), None, None, Some(61), None),
            "[compute] job 7f3a0000 (4 shards): exit 0 after 1m01s. k2 compute logs 7f3a0000 --tail 50 · k2 compute get 7f3a0000 --json"
        );
        let t = super::super::proto::results::TestSummary { total: 415, passed: 410, failed: 2, errors: 1, skipped: 2, ..Default::default() };
        assert_eq!(
            completion_text("7f3a0000aaaa", "mini-1", "done", Some(100), None, None, Some(61), Some(&t), &Extra::default()),
            "[compute] job 7f3a0000 on mini-1: exit 100 after 1m01s; tests 410 passed, 3 failed, 2 skipped. k2 compute logs 7f3a0000 --failures · k2 compute get 7f3a0000 --json"
        );
    }

    #[test]
    fn queue_timeout_says_what_it_waited_on_and_the_next_step() {
        // CE-12: the wait and the retry line, never "logs" (it never ran).
        let x = Extra { detail: Some("not started within 30m00s (parallel_full)"), ..Default::default() };
        assert_eq!(
            completion_text("ab12cd34ffff", "k2n10-vin", "failed", None, None, Some("queue_timeout"), None, None, &x),
            "[compute] job ab12cd34 on k2n10-vin: failed (queue_timeout): not started within 30m00s (parallel_full). Next: k2 compute retry ab12cd34, or submit again with a longer --queue-timeout (e.g. 4h) · k2 compute get ab12cd34 --json"
        );
        let bare = completion_text("ab12cd34ffff", "n", "failed", None, None, Some("queue_timeout"), None, None, &Extra::default());
        assert!(bare.contains("failed (queue_timeout). Next: k2 compute retry ab12cd34"), "{bare}");
    }

    #[test]
    fn late_end_and_rust_version() {
        // CE-5 / CE-4: the node's duration, the late note, the Rust version.
        let x = Extra { late_secs: Some(10_560), rustc: Some("rustc 1.99.1"), ..Default::default() };
        assert_eq!(
            completion_text("7f3a0000aaaa", "k2n7-vin", "done", Some(0), None, None, Some(372), None, &x),
            "[compute] job 7f3a0000 on k2n7-vin: exit 0 after 6m12s (rustc 1.99.1) (this server heard of it 2h56m late). k2 compute logs 7f3a0000 --tail 50 · k2 compute get 7f3a0000 --json"
        );
    }
}

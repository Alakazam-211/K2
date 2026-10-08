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

/// One line for the requesting session when a job ends.
pub fn completion_text(
    job_id: &str,
    node: &str,
    state: &str,
    exit_code: Option<i64>,
    signal: Option<i64>,
    reason: Option<&str>,
    secs: Option<i64>,
) -> String {
    let id = short(job_id);
    let after = secs.map(|s| format!(" after {}", duration(s))).unwrap_or_default();
    let what = match state {
        "done" => match (exit_code, signal) {
            (Some(c), _) => format!("exit {c}{after}"),
            (None, Some(sig)) => format!("killed by signal {sig}{after}"),
            _ => format!("finished{after}"),
        },
        "failed" => format!("failed ({}){after}", reason.unwrap_or("infrastructure")),
        "timeout" => format!("timed out{after}"),
        "cancelled" => format!("cancelled ({}){after}", reason.unwrap_or("cancelled")),
        "interrupted" => format!("interrupted ({}){after}", reason.unwrap_or("interrupted")),
        "unknown" => "lost (the node has no record of it; safe to retry)".to_string(),
        other => other.to_string(),
    };
    // Only ids, names and codes we generated or validated go in.
    let node = node.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>();
    format!("[compute] job {id} on {node}: {what}. k2 compute logs {id} --tail 50")
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
            completion_text("7f3a0000aaaa", "z13flow", "done", Some(1), None, None, Some(372)),
            "[compute] job 7f3a0000 on z13flow: exit 1 after 6m12s. k2 compute logs 7f3a0000 --tail 50"
        );
        assert!(completion_text("j", "n", "failed", None, None, Some("sync_failed"), None).contains("failed (sync_failed)"));
        assert!(completion_text("j", "n; rm -rf", "unknown", None, None, None, None).contains("on nrm-rf:"));
    }
}

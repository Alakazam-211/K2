//! The stall guard's end text (CE-10): which rule ended a job
//! `timeout/stalled`, and how to set the limit. The node writes the job's
//! detail with [`detail`]; the server reads the rule back with [`rule_of`]
//! and says it in its own words ([`StallRule::words`]), so its messages
//! never repeat node-made text. No wire change: `reason` stays `stalled`,
//! and an older node's detail ("no output for 1800s") has no rule.
//!
//! Every detail starts "no output for {N}s", as before.

/// What fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallRule {
    /// Silent and under 1% of one CPU for the whole window (the CPU rule).
    NoOutputNoCpu,
    /// Silent for the window on a node that couldn't read the job's CPU:
    /// silence alone ended it.
    CpuUnreadable,
    /// Silent for the window, for a server whose rule counts output only.
    NoOutput,
}

/// How to change the limit, the same words everywhere.
pub const HOW_TO_SET: &str = "set it with --stall-timeout (e.g. --stall-timeout 2h, or off for a quiet job), or stall_timeout in .k2/compute.toml";

const CPU_MARK: &str = " and under 1% of one CPU as long";
const UNREADABLE_MARK: &str = "; this node couldn't read the job's CPU";
const OUTPUT_MARK: &str = " (the stall rule here counts output only)";

/// The job's detail when the stall guard ends it. `cpu_permille`: the
/// job's CPU over the last interval, in tenths of a percent of one CPU
/// (only said for [`StallRule::NoOutputNoCpu`]).
pub fn detail(rule: StallRule, stall_secs: u64, cpu_permille: u64) -> String {
    let what = match rule {
        StallRule::NoOutputNoCpu => format!("{CPU_MARK} (last {}.{}% of one CPU): no output, no CPU", cpu_permille / 10, cpu_permille % 10),
        StallRule::CpuUnreadable => format!("{UNREADABLE_MARK}, so silence alone ended it: no output, CPU unreadable"),
        StallRule::NoOutput => format!("{OUTPUT_MARK}: no output"),
    };
    format!("no output for {stall_secs}s{what}. The stall guard ended it; {HOW_TO_SET}")
}

/// The rule a node's detail names; `None` for an older node's detail.
pub fn rule_of(detail: &str) -> Option<StallRule> {
    let rest = detail.strip_prefix("no output for ")?;
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).strip_prefix('s')?;
    if rest.starts_with(CPU_MARK) {
        Some(StallRule::NoOutputNoCpu)
    } else if rest.starts_with(UNREADABLE_MARK) {
        Some(StallRule::CpuUnreadable)
    } else if rest.starts_with(OUTPUT_MARK) {
        Some(StallRule::NoOutput)
    } else {
        None
    }
}

impl StallRule {
    /// The server's own words for the rule.
    pub fn words(self) -> &'static str {
        match self {
            StallRule::NoOutputNoCpu => "no output and no CPU for its whole --stall-timeout",
            StallRule::CpuUnreadable => "no output for its whole --stall-timeout, and the node couldn't read its CPU (silence alone ends a job there)",
            StallRule::NoOutput => "no output for its whole --stall-timeout (an output-only stall rule)",
        }
    }
}

/// The server's words for a stalled job's end, from the node's detail
/// (an older node's detail names no rule).
pub fn words_for(detail: Option<&str>) -> &'static str {
    detail.and_then(rule_of).map_or("no output for its whole --stall-timeout", StallRule::words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_detail_names_its_rule_and_how_to_set_the_limit() {
        let cpu = detail(StallRule::NoOutputNoCpu, 1800, 3);
        assert_eq!(
            cpu,
            "no output for 1800s and under 1% of one CPU as long (last 0.3% of one CPU): no output, no CPU. The stall guard ended it; set it with --stall-timeout (e.g. --stall-timeout 2h, or off for a quiet job), or stall_timeout in .k2/compute.toml"
        );
        let blind = detail(StallRule::CpuUnreadable, 60, 0);
        assert_eq!(
            blind,
            "no output for 60s; this node couldn't read the job's CPU, so silence alone ended it: no output, CPU unreadable. The stall guard ended it; set it with --stall-timeout (e.g. --stall-timeout 2h, or off for a quiet job), or stall_timeout in .k2/compute.toml"
        );
        let out = detail(StallRule::NoOutput, 1, 0);
        assert!(out.starts_with("no output for 1s (the stall rule here counts output only): no output."), "{out}");
        for (d, r) in [(cpu, StallRule::NoOutputNoCpu), (blind, StallRule::CpuUnreadable), (out, StallRule::NoOutput)] {
            assert!(d.starts_with("no output for ") && d.contains("--stall-timeout"), "{d}");
            assert_eq!(rule_of(&d), Some(r), "{d}");
        }
    }

    #[test]
    fn an_older_nodes_detail_names_no_rule() {
        assert_eq!(rule_of("no output for 1800s"), None);
        assert_eq!(rule_of("killed"), None);
        assert_eq!(words_for(Some("no output for 1800s")), "no output for its whole --stall-timeout");
        assert_eq!(words_for(None), "no output for its whole --stall-timeout");
        assert_eq!(words_for(Some(&detail(StallRule::NoOutputNoCpu, 5, 0))), StallRule::NoOutputNoCpu.words());
    }
}

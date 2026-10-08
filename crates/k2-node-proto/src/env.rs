//! The job environment rules both ends agree on (§9.2, CN25).
//!
//! A job's env starts EMPTY on the node. The node sets the names in
//! [`NODE_SET_VARS`]; the agent may add `--env NAME=value` pairs, which the
//! controller checks with [`env_name_refusal`] before a job is queued and
//! the node checks again before spawning.

/// Copied from `k2_core::test_isolation::PROD_REACH_VARS` (k2-node must
/// not link k2-core). A k2-core test pins the two lists equal.
pub const PROD_REACH_VARS: &[&str] = &[
    "K2_HOOK_SOCK",
    "K2SO_HOOK_SOCK",
    "K2_HOOK_TOKEN",
    "K2SO_HOOK_TOKEN",
    "K2_PANE_ID",
    "K2SO_PANE_ID",
    "K2_TAB_ID",
    "K2SO_TAB_ID",
    "K2_PORT",
    "K2SO_PORT",
    "K2_SESSION_ID",
    "K2_CELL",
    "K2_API_CELL",
    "K2SO_API_CELL",
    "K2_REMOTE_TOKEN",
    "K2_CONNECT_BASE",
    "K2_HOME",
];

/// Names the node always sets itself. An agent can't override them (the
/// warm slot, the job home and the scrub depend on them).
pub const NODE_SET_VARS: &[&str] = &[
    "HOME",
    "PATH",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "SCCACHE_DIR",
    "TMPDIR",
    "LANG",
    "CI",
    "K2_COMPUTE_JOB",
    "K2_SECRETS_DIR",
    "USER",
    "LOGNAME",
    "SHELL",
];

/// Longest `--env` value.
pub const MAX_ENV_VALUE: usize = 8 * 1024;
/// Most `--env` pairs per job.
pub const MAX_ENV_PAIRS: usize = 64;

/// Why an agent-supplied env name is refused, or `None` when it's fine.
pub fn env_name_refusal(name: &str) -> Option<&'static str> {
    if name.is_empty()
        || name.len() > 128
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return Some("env_name_invalid");
    }
    if PROD_REACH_VARS.contains(&name) {
        return Some("env_reaches_daemon");
    }
    if name.starts_with("K2_") || name.starts_with("K2SO_") {
        return Some("env_k2_reserved");
    }
    if NODE_SET_VARS.contains(&name) {
        return Some("env_node_reserved");
    }
    None
}

/// Check every pair; the first problem as `(code, name)`.
pub fn check_env_pairs<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), (String, String)> {
    let mut n = 0usize;
    for (k, v) in pairs {
        n += 1;
        if n > MAX_ENV_PAIRS {
            return Err(("env_too_many".to_string(), k.to_string()));
        }
        if let Some(code) = env_name_refusal(k) {
            return Err((code.to_string(), k.to_string()));
        }
        if v.len() > MAX_ENV_VALUE || v.contains('\0') {
            return Err(("env_value_invalid".to_string(), k.to_string()));
        }
    }
    Ok(())
}

/// Longest PATH entry a job gets (same rule as k2-core `path_env.rs`).
pub const MAX_PATH_ENTRY_LEN: usize = 900;

/// Keep PATH entries that are non-empty, at most [`MAX_PATH_ENTRY_LEN`]
/// bytes and free of control characters, in order.
pub fn clean_path(path: &str) -> String {
    path.split(':')
        .filter(|e| !e.is_empty() && e.len() <= MAX_PATH_ENTRY_LEN && !e.chars().any(char::is_control))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prod_reach_var_is_refused() {
        for v in PROD_REACH_VARS {
            assert_eq!(env_name_refusal(v), Some("env_reaches_daemon"), "{v}");
        }
        assert_eq!(PROD_REACH_VARS.len(), 17);
    }

    #[test]
    fn k2_and_node_names_are_refused_ordinary_names_pass() {
        assert_eq!(env_name_refusal("K2_ANYTHING"), Some("env_k2_reserved"));
        assert_eq!(env_name_refusal("K2SO_X"), Some("env_k2_reserved"));
        assert_eq!(env_name_refusal("CARGO_TARGET_DIR"), Some("env_node_reserved"));
        assert_eq!(env_name_refusal("HOME"), Some("env_node_reserved"));
        assert_eq!(env_name_refusal("RUST_LOG"), None);
        assert_eq!(env_name_refusal("NEXTEST_PROFILE"), None);
        assert_eq!(env_name_refusal("1BAD"), Some("env_name_invalid"));
        assert_eq!(env_name_refusal("A-B"), Some("env_name_invalid"));
        assert_eq!(env_name_refusal(""), Some("env_name_invalid"));
    }

    #[test]
    fn check_pairs_reports_first_problem() {
        assert!(check_env_pairs([("RUST_LOG", "debug")]).is_ok());
        assert_eq!(
            check_env_pairs([("OK", "1"), ("K2_HOOK_TOKEN", "x")]).unwrap_err(),
            ("env_reaches_daemon".to_string(), "K2_HOOK_TOKEN".to_string())
        );
        assert_eq!(check_env_pairs([("A", "x\0y")]).unwrap_err().0, "env_value_invalid");
    }

    #[test]
    fn clean_path_drops_long_and_control_entries() {
        let long = "x".repeat(901);
        let p = format!("/usr/bin::{long}:/bin:/a\nb:/opt/x");
        assert_eq!(clean_path(&p), "/usr/bin:/bin:/opt/x");
    }
}

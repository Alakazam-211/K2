//! What a job needs installed on the node (compute polish v1).
//!
//! A need is one token, checked the same way on both ends:
//!
//! - `node@22`, `python@3.12`, `go@1.23`, `bun@1.2`, `java@21`, … — a
//!   user-space toolchain, installed by `k2-node` into its own home with
//!   mise (no root). `tool` alone means `tool@latest`.
//! - `rust@stable`, `rust@1.99`, `rust@nightly-2026-09-01` — Rust through
//!   rustup in the node home (mise delegates Rust to rustup anyway).
//! - `cargo-nextest` (or `nextest`) — the prebuilt nextest binary.
//! - `sys:<package>` — a system package (`libssl-dev`, `pkg-config`).
//!   Needs root: the node installs it only when the machine's owner put
//!   the name on the node's approved list (`/etc/k2-node/packages.toml`).
//!
//! Names and versions are a small charset so a need can never become a
//! shell word, a flag or a path.

use serde::{Deserialize, Serialize};

/// Most needs per job.
pub const MAX_NEEDS: usize = 32;

/// Offer feature: the node understands `Plan::needs`.
pub const FEATURE_NEEDS: &str = "needs";
/// Offer feature: the node installs approved system packages itself.
pub const FEATURE_SYSTEM_PACKAGES: &str = "system-packages";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Need {
    /// A mise-managed tool.
    Tool { tool: String, version: String },
    /// Rust through rustup; `channel` is `stable`, `1.99`, `nightly-…`.
    Rust { channel: String },
    /// The prebuilt `cargo-nextest`.
    Nextest,
    /// A distro package; root, owner-approved.
    System { package: String },
}

impl Need {
    /// The canonical token (`node@22`, `rust@stable`, `sys:pkg-config`).
    pub fn token(&self) -> String {
        match self {
            Need::Tool { tool, version } => format!("{tool}@{version}"),
            Need::Rust { channel } => format!("rust@{channel}"),
            Need::Nextest => "cargo-nextest".into(),
            Need::System { package } => format!("sys:{package}"),
        }
    }

    /// The tool a need is about, for "explicit beats repo file" merging.
    pub fn key(&self) -> String {
        match self {
            Need::Tool { tool, .. } => tool.clone(),
            Need::Rust { .. } => "rust".into(),
            Need::Nextest => "cargo-nextest".into(),
            Need::System { package } => format!("sys:{package}"),
        }
    }
}

/// `a-z 0-9` then `a-z 0-9 . _ + -`, at most 64: a package name on every
/// distro we support, and never an option (`-x`) or a path.
pub fn valid_package(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && p.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '-'))
}

/// mise tool names: `node`, `python`, `aqua:cli/cli`, `npm:prettier`.
fn valid_tool(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && t.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | ':' | '/'))
        && !t.contains("..")
        && !t.ends_with('/')
        && !t.ends_with(':')
}

/// Versions: `22`, `3.12.1`, `lts`, `latest`, `1.99.0`, `nightly-2026-09-01`.
fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 48
        && v.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// asdf / `.tool-versions` aliases → mise names.
pub fn canonical_tool(t: &str) -> &str {
    match t {
        "nodejs" => "node",
        "golang" => "go",
        "rustlang" => "rust",
        "python3" => "python",
        other => other,
    }
}

/// Parse one need token. `Err` says why, in words an agent can act on.
pub fn parse(token: &str) -> Result<Need, String> {
    let t = token.trim();
    if t.is_empty() {
        return Err("empty need".into());
    }
    if let Some(p) = t.strip_prefix("sys:") {
        return if valid_package(p) {
            Ok(Need::System { package: p.to_string() })
        } else {
            Err(format!("{t:?}: a system package is sys:<name> with a-z 0-9 . _ + - (e.g. sys:libssl-dev)"))
        };
    }
    if matches!(t, "cargo-nextest" | "nextest" | "cargo-nextest@latest" | "nextest@latest") {
        return Ok(Need::Nextest);
    }
    let (tool, version) = match t.split_once('@') {
        Some((a, b)) => (a, b),
        None => (t, "latest"),
    };
    let tool = canonical_tool(tool);
    if !valid_tool(tool) {
        return Err(format!("{t:?}: a tool is <name>@<version> (e.g. node@22, python@3.12, rust@stable)"));
    }
    let version = version.trim_start_matches('v');
    if !valid_version(version) {
        return Err(format!("{t:?}: versions use a-z A-Z 0-9 . _ + - (e.g. 22, 3.12.1, lts, stable)"));
    }
    if tool == "rust" || tool == "rustup" {
        let channel = if version == "latest" { "stable" } else { version };
        return Ok(Need::Rust { channel: channel.to_string() });
    }
    Ok(Need::Tool { tool: tool.to_string(), version: version.to_string() })
}

/// Parse a list (the CLI's comma list arrives split already). Refuses
/// duplicates of the same tool and more than [`MAX_NEEDS`].
pub fn parse_all<S: AsRef<str>>(tokens: &[S]) -> Result<Vec<Need>, String> {
    if tokens.len() > MAX_NEEDS {
        return Err(format!("at most {MAX_NEEDS} needs per job"));
    }
    let mut out: Vec<Need> = Vec::new();
    for t in tokens {
        let n = parse(t.as_ref())?;
        if out.iter().any(|o| o.key() == n.key()) {
            return Err(format!("{} is listed twice", n.key()));
        }
        out.push(n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_parse_and_round_trip() {
        assert_eq!(parse("node@22").unwrap(), Need::Tool { tool: "node".into(), version: "22".into() });
        assert_eq!(parse("nodejs@v20.1.0").unwrap().token(), "node@20.1.0");
        assert_eq!(parse("python").unwrap().token(), "python@latest");
        assert_eq!(parse("golang@1.23").unwrap().token(), "go@1.23");
        assert_eq!(parse("rust").unwrap(), Need::Rust { channel: "stable".into() });
        assert_eq!(parse("rust@1.99").unwrap().token(), "rust@1.99");
        assert_eq!(parse("rust@nightly-2026-09-01").unwrap().token(), "rust@nightly-2026-09-01");
        assert_eq!(parse("nextest").unwrap(), Need::Nextest);
        assert_eq!(parse("sys:libssl-dev").unwrap(), Need::System { package: "libssl-dev".into() });
        assert_eq!(parse("aqua:cli/cli@2").unwrap().token(), "aqua:cli/cli@2");
    }

    #[test]
    fn hostile_tokens_are_refused() {
        for bad in [
            "", "sys:", "sys:-y", "sys:../x", "sys:a b", "sys:A", "node@", "node@;rm", "node@$(x)", "-x@1", "../a@1",
            "node@22 && x", "a/../b@1", "sys:libssl-dev;reboot", "node@--force",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn lists_refuse_duplicates_and_floods() {
        assert!(parse_all(&["node@22", "node@20"]).is_err());
        assert!(parse_all(&["rust@stable", "rustup@1.99"]).is_err());
        let many: Vec<String> = (0..33).map(|i| format!("sys:p{i}")).collect();
        assert!(parse_all(&many).is_err());
        assert_eq!(parse_all(&["node@22", "python@3.12", "sys:pkg-config"]).unwrap().len(), 3);
    }
}

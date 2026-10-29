//! What a job needs installed on the node (compute polish v1).
//!
//! A need is one token, checked the same way on both ends:
//!
//! - `node@22`, `python@3.12`, `go@1.23`, `bun@1.2`, `java@21`, … — a
//!   user-space toolchain, installed by `k2-node` into its own home with
//!   mise (no root). `tool` alone means `tool@latest`.
//! - `rust@stable`, `rust@1.99`, `rust@nightly-2026-09-01` — Rust through
//!   rustup in the node home (mise delegates Rust to rustup anyway).
//!   Components ride on the same token, joined with `+` (a comma would
//!   split the needs list): `rust@stable+clippy+rustfmt`,
//!   `rust@nightly+llvm-tools` ([`RUST_COMPONENTS`]; nodes offering
//!   [`FEATURE_RUST_COMPONENTS`] only).
//! - `cargo-nextest` (or `nextest`) — the prebuilt nextest binary.
//! - `sys:<package>` — a system package (`libssl-dev`, `pkg-config`).
//!   Needs root: the node installs it only when the machine's owner put
//!   the name on the node's approved list (`/etc/k2-node/packages.toml`).
//!
//! Names and versions are a small charset so a need can never become a
//! shell word, a flag or a path.
//!
//! **Supported tools.** A plain tool name must be one the node knows how
//! to install and check ([`SUPPORTED_TOOLS`]: node, python, go, java,
//! bun, deno, zig, cmake, ninja, protoc, uv, pnpm); anything else is
//! refused with [`TOOL_UNSUPPORTED`] before the job starts, never dropped.
//! The long tail is still reachable by naming a mise backend explicitly
//! (`aqua:owner/repo@1`, `npm:prettier@3`, …; [`ALLOWED_BACKENDS`]).
//! asdf/vfox plugins (third-party shell scripts) are never used.

use serde::{Deserialize, Serialize};

/// Most needs per job.
pub const MAX_NEEDS: usize = 32;

/// Refusal code: a well-formed need the node can't provide.
pub const TOOL_UNSUPPORTED: &str = "tool_unsupported";
/// Refusal code: a malformed need.
pub const NEED_INVALID: &str = "need_invalid";

/// Plain tool names the node installs with mise, and the commands each
/// must put on the job's PATH (checked after the install, so a tool that
/// "installed" but provides nothing fails the job instead of a later
/// `command not found`). Non-core tools resolve through mise's registry
/// to their aqua backend (prebuilt release assets, checksum-checked).
pub const SUPPORTED_TOOLS: &[(&str, &[&str])] = &[
    ("node", &["node"]),
    ("python", &["python3"]),
    ("go", &["go"]),
    ("java", &["java"]),
    ("bun", &["bun"]),
    ("deno", &["deno"]),
    ("zig", &["zig"]),
    ("cmake", &["cmake"]),
    ("ninja", &["ninja"]),
    ("protoc", &["protoc"]),
    ("uv", &["uv"]),
    ("pnpm", &["pnpm"]),
];

/// mise backends a need may name explicitly (`aqua:cli/cli@2`). Never
/// `asdf:`/`vfox:` (plugins are shell scripts from third-party repos).
pub const ALLOWED_BACKENDS: &[&str] = &["aqua", "ubi", "github", "npm", "pipx", "cargo", "go"];

/// The commands a supported plain tool must provide (empty for a
/// backend-qualified tool, which is only checked for a bin dir).
pub fn expected_bins(tool: &str) -> &'static [&'static str] {
    SUPPORTED_TOOLS.iter().find(|(t, _)| *t == tool).map(|(_, b)| *b).unwrap_or(&[])
}

/// What `--needs` takes, for help text and refusals.
pub fn supported_list() -> String {
    let mut v: Vec<String> = SUPPORTED_TOOLS.iter().map(|(t, _)| format!("{t}@<v>")).collect();
    v.extend(["rust@<channel>[+<component>…]", "cargo-nextest", "sys:<package>"].map(String::from));
    v.join(", ")
}

/// Can the node provide this need? Rust, nextest and system packages
/// parse to their own kinds; a tool must be supported or name an allowed
/// backend.
pub fn check_supported(n: &Need) -> Result<(), String> {
    let Need::Tool { tool, version } = n else { return Ok(()) };
    if let Some((backend, rest)) = tool.split_once(':') {
        if ALLOWED_BACKENDS.contains(&backend) && !rest.is_empty() {
            return Ok(());
        }
        return Err(format!(
            "{tool}@{version}: the {backend}: backend isn't allowed (use one of {}, e.g. aqua:owner/repo@1)",
            ALLOWED_BACKENDS.iter().map(|b| format!("{b}:")).collect::<Vec<_>>().join(" ")
        ));
    }
    if SUPPORTED_TOOLS.iter().any(|(t, _)| t == tool) {
        return Ok(());
    }
    Err(format!(
        "{tool}@{version}: k2-node doesn't install {tool:?}. Supported: {}. Or name a mise backend (aqua:owner/repo@1), or ask the machine's owner to install it (Linux: --needs sys:<package> once approved)",
        supported_list()
    ))
}

/// A refused need list: a stable code ([`NEED_INVALID`] or
/// [`TOOL_UNSUPPORTED`]) and words an agent can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for NeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<NeedError> for String {
    fn from(e: NeedError) -> String {
        e.to_string()
    }
}

/// Offer feature: the node understands `Plan::needs`.
pub const FEATURE_NEEDS: &str = "needs";
/// Offer feature: the node installs approved system packages itself.
pub const FEATURE_SYSTEM_PACKAGES: &str = "system-packages";
/// Offer feature (0.46.7, CE-16): the node installs Rust components named
/// on a `rust@<channel>+<component>…` need, puts the LLVM tools on the
/// job's PATH, and lists each toolchain's components in `offer.tools`
/// (`rust-components:<toolchain>`). An older node reads `stable+clippy`
/// as a channel name and fails the install, so the controller places such
/// a need only on a node offering this.
pub const FEATURE_RUST_COMPONENTS: &str = "rust-components";

/// Offer feature (0.46.7, CE-18): the node keeps its installed tool
/// versions (mise installs, Rust toolchains) under a disk cap, the policy
/// key `toolchain_budget_gb` (default 15% of the node's disk budget, at
/// most 60 GB): over it after an install, it removes the least recently
/// used versions no running or queued job needs. An older node has no cap.
pub const FEATURE_TOOLCHAIN_BUDGET: &str = "toolchain-budget";

/// Rust components a need may name (`rust@stable+clippy`). `llvm-tools`
/// is rustup's `llvm-tools-preview` on older toolchains (rustup renames
/// it); with it the job's PATH gains the toolchain's LLVM bin dir.
pub const RUST_COMPONENTS: &[&str] = &["clippy", "rustfmt", "rust-src", "llvm-tools", "rust-analyzer", "miri"];

/// Most components on one Rust need.
pub const MAX_RUST_COMPONENTS: usize = 8;

/// `llvm-tools-preview` → `llvm-tools`, `clippy-preview` → `clippy`: the
/// name a need uses for a component rustup may list under an older name.
pub fn canonical_component(c: &str) -> &str {
    match c {
        "llvm-tools-preview" => "llvm-tools",
        "clippy-preview" => "clippy",
        "rustfmt-preview" => "rustfmt",
        "rust-analyzer-preview" => "rust-analyzer",
        "miri-preview" => "miri",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Need {
    /// A mise-managed tool.
    Tool { tool: String, version: String },
    /// Rust through rustup; `channel` is `stable`, `1.99`, `nightly-…`.
    /// `components` (sorted, unique, [`RUST_COMPONENTS`]) from the `+`
    /// form; empty (and absent on the wire) otherwise.
    Rust {
        channel: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        components: Vec<String>,
    },
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
            Need::Rust { channel, components } if components.is_empty() => format!("rust@{channel}"),
            Need::Rust { channel, components } => format!("rust@{channel}+{}", components.join("+")),
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
        return parse_rust(t, version);
    }
    Ok(Need::Tool { tool: tool.to_string(), version: version.to_string() })
}

/// `stable`, `1.99+clippy+rustfmt`, `nightly-2026-09-01+llvm-tools`.
fn parse_rust(t: &str, version: &str) -> Result<Need, String> {
    let mut parts = version.split('+');
    let channel = parts.next().unwrap_or("");
    if channel.is_empty() {
        return Err(format!("{t:?}: a Rust need is rust@<channel>[+<component>…] (e.g. rust@stable+clippy+rustfmt)"));
    }
    let channel = if channel == "latest" { "stable" } else { channel };
    let mut components: Vec<String> = Vec::new();
    for c in parts {
        let c = canonical_component(c);
        if !RUST_COMPONENTS.contains(&c) {
            return Err(format!(
                "{t:?}: {c:?} isn't a Rust component K2 installs (one of {}; join them with +, e.g. rust@stable+clippy+rustfmt)",
                RUST_COMPONENTS.join(", ")
            ));
        }
        if !components.iter().any(|x| x == c) {
            components.push(c.to_string());
        }
    }
    if components.len() > MAX_RUST_COMPONENTS {
        return Err(format!("{t:?}: at most {MAX_RUST_COMPONENTS} components"));
    }
    components.sort();
    Ok(Need::Rust { channel: channel.to_string(), components })
}

/// Parse an explicit list (the CLI's comma list arrives split already):
/// every token must parse **and** be supported ([`check_supported`]).
/// Refuses duplicates of the same tool and more than [`MAX_NEEDS`].
/// Nothing is ever dropped: one bad token refuses the whole list.
pub fn parse_all<S: AsRef<str>>(tokens: &[S]) -> Result<Vec<Need>, NeedError> {
    let invalid = |message: String| NeedError { code: NEED_INVALID, message };
    if tokens.len() > MAX_NEEDS {
        return Err(invalid(format!("at most {MAX_NEEDS} needs per job")));
    }
    let mut out: Vec<Need> = Vec::new();
    for t in tokens {
        let n = parse(t.as_ref()).map_err(invalid)?;
        check_supported(&n).map_err(|message| NeedError { code: TOOL_UNSUPPORTED, message })?;
        if out.iter().any(|o| o.key() == n.key()) {
            return Err(invalid(format!("{} is listed twice", n.key())));
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
        assert_eq!(parse("rust").unwrap(), Need::Rust { channel: "stable".into(), components: vec![] });
        assert_eq!(parse("rust@1.99").unwrap().token(), "rust@1.99");
        assert_eq!(parse("rust@nightly-2026-09-01").unwrap().token(), "rust@nightly-2026-09-01");
        assert_eq!(parse("nextest").unwrap(), Need::Nextest);
        assert_eq!(parse("sys:libssl-dev").unwrap(), Need::System { package: "libssl-dev".into() });
        assert_eq!(parse("aqua:cli/cli@2").unwrap().token(), "aqua:cli/cli@2");
    }

    /// CE-16 / CE-A10: components ride on the token with `+` (needs are
    /// comma-split), sorted and unique; an unknown one is `need_invalid`
    /// before anything queues; the plain token (and its wire form) is the
    /// same as before, so older plans and digests don't change.
    #[test]
    fn rust_components_parse() {
        let n = parse("rust@stable+rustfmt+clippy").unwrap();
        assert_eq!(n, Need::Rust { channel: "stable".into(), components: vec!["clippy".into(), "rustfmt".into()] });
        assert_eq!(n.token(), "rust@stable+clippy+rustfmt");
        assert_eq!(n.key(), "rust");
        assert_eq!(parse("rust@nightly-2026-09-01+llvm-tools-preview").unwrap().token(), "rust@nightly-2026-09-01+llvm-tools");
        assert_eq!(parse("rust@1.99+clippy+clippy").unwrap().token(), "rust@1.99+clippy");
        assert_eq!(parse("rust@latest+miri").unwrap().token(), "rust@stable+miri");
        let e = parse_all(&["rust@stable+bogus"]).unwrap_err();
        assert_eq!(e.code, NEED_INVALID);
        assert!(e.message.contains("isn't a Rust component") && e.message.contains("clippy"), "{e}");
        for bad in ["rust@+clippy", "rust@stable+", "rust@stable++clippy", "rust@stable+Clippy"] {
            assert!(parse(bad).is_err(), "{bad:?} must be refused");
        }
        // Wire form: no `components` key without components.
        let plain = serde_json::to_string(&parse("rust@stable").unwrap()).unwrap();
        assert_eq!(plain, r#"{"kind":"rust","channel":"stable"}"#);
        let old: Need = serde_json::from_str(r#"{"kind":"rust","channel":"1.99"}"#).unwrap();
        assert_eq!(old, Need::Rust { channel: "1.99".into(), components: vec![] });
        assert_eq!(canonical_component("llvm-tools-preview"), "llvm-tools");
        assert!(supported_list().contains("rust@<channel>[+<component>…]"));
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
    fn unsupported_tools_are_refused_not_dropped() {
        let ok = parse_all(&["cmake@3", "ninja", "protoc@28", "aqua:cli/cli@2", "npm:prettier@3", "node@22"]).unwrap();
        assert_eq!(ok[0], Need::Tool { tool: "cmake".into(), version: "3".into() });
        assert_eq!(expected_bins("cmake"), &["cmake"]);
        assert_eq!(expected_bins("python"), &["python3"]);
        assert!(expected_bins("aqua:cli/cli").is_empty());
        for (bad, why) in [
            ("make@4", "doesn't install \"make\""),
            ("terraform@1", "Supported: node@<v>"),
            ("asdf:asdf-community/asdf-cmake@3", "asdf: backend isn't allowed"),
            ("vfox:x/y@1", "vfox: backend isn't allowed"),
        ] {
            let e = parse_all(&["node@22", bad]).unwrap_err();
            assert_eq!(e.code, TOOL_UNSUPPORTED, "{bad}");
            assert!(e.message.contains(why), "{bad}: {}", e.message);
            assert!(e.to_string().starts_with("tool_unsupported: "), "{e}");
        }
        assert!(supported_list().contains("cmake@<v>"));
        // Malformed stays need_invalid; rust/nextest/sys are their own kinds.
        assert_eq!(parse_all(&["node@;x"]).unwrap_err().code, NEED_INVALID);
        assert_eq!(parse_all(&["rust@stable", "cargo-nextest", "sys:cmake"]).unwrap().len(), 3);
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

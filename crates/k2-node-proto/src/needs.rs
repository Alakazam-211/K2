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
//!   [`FEATURE_RUST_COMPONENTS`] only). Compile targets ride the same way
//!   as `+target:<triple>`, repeatable and mixed with components:
//!   `rust@1.95.0+target:wasm32-unknown-unknown`,
//!   `rust@stable+clippy+target:wasm32-wasip1+target:aarch64-unknown-linux-gnu`
//!   (nodes offering [`FEATURE_RUST_TARGETS`] only; rustup adds them).
//! - `cargo-nextest` (or `nextest`) — the prebuilt nextest binary.
//! - `sys:<package>` — a system package by its Debian name (`libssl-dev`,
//!   `pkg-config`), mapped per distro ([`crate::pkgmap`]); or a native
//!   name for one package manager (`sys:pacman:protobuf`, nodes offering
//!   `pkg-map` only). Needs root: the node installs it when the machine's
//!   owner put the name on the node's approved list
//!   (`/etc/k2-node-packages.toml`) or the owner's package policy for the
//!   job's server covers it ([`crate::pkgpolicy`]).
//! - `profile:<name>[@<version>]` — a K2-shipped bundle of system names and
//!   tools ([`crate::profiles`]); one need, nodes offering `pkg-map` only.
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

/// Tools k2-node installs itself from a pinned, sha256-checked download
/// (not mise): the Android NDK and cargo-ndk, for Rust cross-builds to
/// Android (Sew's arm64 phone builds, 2026-11-15). Linux x86_64 nodes
/// only: the NDK ships a linux-x86_64 host and no linux-aarch64, and on
/// macOS it is a .dmg.
pub const ANDROID_NDK: &str = "android-ndk";
pub const CARGO_NDK: &str = "cargo-ndk";
pub const NATIVE_TOOLS: &[&str] = &[ANDROID_NDK, CARGO_NDK];

/// The (os, arch) the native tools run on (the node's `os` / `arch`).
pub const NATIVE_TOOLS_HOST: (&str, &str) = ("linux", "x86_64");

/// The API level the NDK env defaults to (`aarch64-linux-android28-clang`,
/// `CARGO_NDK_PLATFORM=28`): Sew's minSdk (AAudio and the OS AEC/NS).
/// A job overrides it with its own env (`--env`), which comes after.
pub const ANDROID_DEFAULT_API: u32 = 28;

/// A pinned NDK release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NdkPin {
    /// `27`: what `android-ndk@27` names.
    pub major: &'static str,
    /// `r27d`: Google's release name (the folder in the zip).
    pub release: &'static str,
    /// `27.3.13750724`: `Pkg.Revision` in its `source.properties`.
    pub revision: &'static str,
    pub url: &'static str,
    /// Checked on the node before anything is unpacked.
    pub sha256: &'static str,
    /// Google's published SHA-1 (checked once when pinned, with the sha256).
    pub sha1: &'static str,
    pub bytes: u64,
}

/// r27d, the r27 LTS. Pinned 2026-11-15 by K2 Hardware: downloaded once,
/// sha256 computed, SHA-1 22105e41… matches developer.android.com/ndk/downloads
/// in its zh-cn, es and it pages (663956036 bytes). The en page's r27d block
/// then showed 713550176 bytes / 26b746e5…, and the ru, de, he and pt pages
/// 783549481 / 87e2bb7e…: those values belong to newer (beta) packages
/// mixed into the page, not to this file.
pub const NDK_PINS: &[NdkPin] = &[NdkPin {
    major: "27",
    release: "r27d",
    revision: "27.3.13750724",
    url: "https://dl.google.com/android/repository/android-ndk-r27d-linux.zip",
    sha256: "601246087a682d1944e1e16dd85bc6e49560fe8b6d61255be2829178c8ed15d9",
    sha1: "22105e410cf29afcf163760cc95522b9fb981121",
    bytes: 663_956_036,
}];

/// The pin an `android-ndk@<version>` names: `27`, `r27`, `r27d`, `27.3`,
/// `27.3.13750724`, `lts` or `latest` (the newest pin).
pub fn ndk_pin(version: &str) -> Option<&'static NdkPin> {
    let v = version.trim();
    if matches!(v, "lts" | "latest") {
        return NDK_PINS.last();
    }
    // `r27` / `r27d` → `27` / `27d`; `27.3.13750724` stays.
    let bare = v.strip_prefix('r').unwrap_or(v);
    NDK_PINS.iter().find(|p| {
        let release = p.release.strip_prefix('r').unwrap_or(p.release);
        bare == p.major || bare == release || v == p.revision || p.revision.strip_prefix(v).is_some_and(|r| r.starts_with('.'))
    })
}

/// The pinned cargo-ndk (prebuilt, musl): its version, asset and sha256.
pub const CARGO_NDK_VERSION: &str = "4.1.2";
pub const CARGO_NDK_URL: &str = "https://github.com/bbqsrc/cargo-ndk/releases/download/v4.1.2/cargo-ndk-x86_64-unknown-linux-musl-v4.1.2.tgz";
pub const CARGO_NDK_SHA256: &str = "dc148bebbfcd7d5355e9858530d5a69c20ce69cf3268c54b9f63c60cb4c9a966";

/// Does `cargo-ndk@<version>` name the pin (`4`, `4.1`, `4.1.2`, `latest`)?
pub fn cargo_ndk_matches(version: &str) -> bool {
    matches!(version, "latest" | "lts") || version == CARGO_NDK_VERSION || CARGO_NDK_VERSION.strip_prefix(version).is_some_and(|r| r.starts_with('.'))
}

/// A native tool's version check, in words an agent can act on.
pub fn check_native(tool: &str, version: &str) -> Result<(), String> {
    match tool {
        ANDROID_NDK if ndk_pin(version).is_none() => Err(format!(
            "{tool}@{version}: k2-node has the Android NDK {} pinned (android-ndk@{}); ask K2 for another release",
            NDK_PINS.iter().map(|p| format!("{} ({})", p.release, p.revision)).collect::<Vec<_>>().join(", "),
            NDK_PINS.last().map(|p| p.major).unwrap_or("27")
        )),
        CARGO_NDK if !cargo_ndk_matches(version) => Err(format!("{tool}@{version}: k2-node has cargo-ndk {CARGO_NDK_VERSION} pinned (cargo-ndk@4)")),
        _ => Ok(()),
    }
}

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
    v.extend(["android-ndk@27 (Linux x86_64)", "cargo-ndk@4 (Linux x86_64)"].map(String::from));
    v.extend(["rust@<channel>[+<component>…][+target:<triple>…]", "cargo-nextest", "sys:<package>", "sys:<apt|pacman|dnf|zypper>:<package>", "profile:<name>"].map(String::from));
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
    if NATIVE_TOOLS.contains(&tool.as_str()) {
        return check_native(tool, version);
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

/// Offer feature (protocol 7, prd-compute-self-serve-environments §8): a
/// bare `rust@<channel>` brings clippy and rustfmt ([`DEFAULT_RUST_COMPONENTS`]),
/// and `rust@<channel>+minimal` opts out. The controller expects the two
/// components only from nodes offering this; an older node is unchanged.
pub const FEATURE_RUST_DEFAULT_COMPONENTS: &str = "rust-default-components";

/// What a bare `rust@<channel>` installs on a node offering
/// [`FEATURE_RUST_DEFAULT_COMPONENTS`].
pub const DEFAULT_RUST_COMPONENTS: &[&str] = &["clippy", "rustfmt"];

/// `rust@stable+minimal`: no default components (a size-sensitive node).
pub const RUST_MINIMAL: &str = "minimal";

/// Offer feature (k2-node 0.46.13): the node adds the Rust compile targets
/// a `rust@<channel>+target:<triple>` need names (`rustup target add`),
/// and lists each toolchain's installed targets in `offer.tools`
/// (`rust-targets:<toolchain>`). Version skew: an older node reads
/// `target:wasm32-unknown-unknown` as a component name and refuses the
/// need (`need_invalid`) at the job's start, so the controller places a
/// need with targets only on a node offering this, and refuses a job
/// pinned to an older one (`node_upgrade_required`).
pub const FEATURE_RUST_TARGETS: &str = "rust-targets";

/// Refusal code: a Rust target was asked of a node whose Rust is a system
/// package with no rustup (the offer's [`TOOLS_RUST_INSTALLER`] is
/// `system`): nothing there can add a target. Placement goes elsewhere; a
/// job pinned there is refused with this code.
pub const RUST_TARGET_UNSUPPORTED: &str = "rust_target_unsupported";

/// The offer `tools` key saying how the node gets Rust: `rustup` (its own
/// rustup in the node home) or `system` (a distro `rustc` on the job PATH
/// and no rustup: no targets can be added). Absent = neither yet (the node
/// installs rustup on the first Rust need).
pub const TOOLS_RUST_INSTALLER: &str = "rust-installer";

/// Most compile targets on one Rust need.
pub const MAX_RUST_TARGETS: usize = 8;

/// The prefix a target takes on a Rust need (`+target:wasm32-unknown-unknown`).
pub const TARGET_PREFIX: &str = "target:";

/// A Rust target triple: 3–64 of `a-z 0-9 _ - .`, starting with a letter
/// or digit, with at least one `-` (`wasm32-unknown-unknown`,
/// `thumbv8m.main-none-eabi`, `aarch64-apple-ios-sim`). Never an option,
/// a path or a shell word.
pub fn valid_target(t: &str) -> bool {
    (3..=64).contains(&t.len())
        && t.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && t.contains('-')
        && !t.contains("..")
        && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

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
    /// form; empty (and absent on the wire) otherwise. `targets` (sorted,
    /// unique, [`valid_target`]) from `+target:<triple>`; empty (and
    /// absent on the wire) otherwise, so a need without targets is the
    /// same bytes as before targets existed.
    Rust {
        channel: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        components: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<String>,
    },
    /// The prebuilt `cargo-nextest`.
    Nextest,
    /// A distro package; root, owner-approved. `package` is a canonical
    /// (Debian) name, or `<manager>:<name>` for a native one.
    System { package: String },
    /// A K2-shipped profile (`profile:tauri-linux@1`); `version` absent =
    /// the newest the server knows.
    Profile {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
    },
}

impl Need {
    /// The canonical token (`node@22`, `rust@stable`, `sys:pkg-config`).
    pub fn token(&self) -> String {
        match self {
            Need::Tool { tool, version } => format!("{tool}@{version}"),
            Need::Rust { channel, components, targets } => {
                let mut t = format!("rust@{channel}");
                for c in components {
                    t.push('+');
                    t.push_str(c);
                }
                for x in targets {
                    t.push('+');
                    t.push_str(TARGET_PREFIX);
                    t.push_str(x);
                }
                t
            }
            Need::Nextest => "cargo-nextest".into(),
            Need::System { package } => format!("sys:{package}"),
            Need::Profile { name, version: None } => format!("profile:{name}"),
            Need::Profile { name, version: Some(v) } => format!("profile:{name}@{v}"),
        }
    }

    /// Does a bare Rust need get the default components (not `+minimal`)?
    pub fn rust_wants_defaults(&self) -> bool {
        matches!(self, Need::Rust { components, .. } if !components.iter().any(|c| c == RUST_MINIMAL))
    }

    /// The tool a need is about, for "explicit beats repo file" merging.
    pub fn key(&self) -> String {
        match self {
            Need::Tool { tool, .. } => tool.clone(),
            Need::Rust { .. } => "rust".into(),
            Need::Nextest => "cargo-nextest".into(),
            Need::System { package } => format!("sys:{package}"),
            Need::Profile { name, .. } => format!("profile:{name}"),
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
        "ndk" | "android_ndk" => ANDROID_NDK,
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
        return match crate::pkgmap::SysName::parse(p) {
            Some(n) => Ok(Need::System { package: n.as_str() }),
            None => Err(format!(
                "{t:?}: a system package is sys:<name> with a-z 0-9 . _ + - (e.g. sys:libssl-dev), or a native name sys:<apt|pacman|dnf|zypper>:<name>"
            )),
        };
    }
    if let Some(p) = t.strip_prefix("profile:") {
        let (name, version) = crate::profiles::parse_ref(p)?;
        return Ok(Need::Profile { name, version });
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
    // Rust checks its own parts (`+target:<triple>` has a `:`).
    if tool == "rust" || tool == "rustup" {
        return parse_rust(t, version);
    }
    if !valid_version(version) {
        return Err(format!("{t:?}: versions use a-z A-Z 0-9 . _ + - (e.g. 22, 3.12.1, lts, stable)"));
    }
    Ok(Need::Tool { tool: tool.to_string(), version: version.to_string() })
}

/// `stable`, `1.99+clippy+rustfmt`, `nightly-2026-09-01+llvm-tools`,
/// `1.95.0+target:wasm32-unknown-unknown`.
fn parse_rust(t: &str, version: &str) -> Result<Need, String> {
    let mut parts = version.split('+');
    let channel = parts.next().unwrap_or("");
    if channel.is_empty() || !valid_version(channel) {
        return Err(format!(
            "{t:?}: a Rust need is rust@<channel>[+<component>…][+target:<triple>…] (e.g. rust@stable+clippy, rust@1.95.0+target:wasm32-unknown-unknown)"
        ));
    }
    let channel = if channel == "latest" { "stable" } else { channel };
    let mut components: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for c in parts {
        if let Some(x) = c.strip_prefix(TARGET_PREFIX) {
            if !valid_target(x) {
                return Err(format!(
                    "{t:?}: {x:?} isn't a Rust target triple (a-z 0-9 _ - ., e.g. +target:wasm32-unknown-unknown; see rustup target list)"
                ));
            }
            if !targets.iter().any(|y| y == x) {
                targets.push(x.to_string());
            }
            continue;
        }
        let c = canonical_component(c);
        if !RUST_COMPONENTS.contains(&c) && c != RUST_MINIMAL {
            return Err(format!(
                "{t:?}: {c:?} isn't a Rust component K2 installs (one of {}, or minimal for none; join them with +, e.g. rust@stable+clippy+rustfmt; a compile target is +target:<triple>)",
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
    if targets.len() > MAX_RUST_TARGETS {
        return Err(format!("{t:?}: at most {MAX_RUST_TARGETS} targets"));
    }
    components.sort();
    targets.sort();
    Ok(Need::Rust { channel: channel.to_string(), components, targets })
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
        if let Need::Profile { name, version } = &n {
            if crate::profiles::get(name, *version).is_none() {
                let message = crate::profiles::resolve(&n.token()["profile:".len()..]).err().unwrap_or_default();
                return Err(NeedError { code: crate::profiles::PROFILE_UNKNOWN, message });
            }
        }
        if out.iter().any(|o| o.key() == n.key()) {
            return Err(invalid(format!("{} is listed twice", n.key())));
        }
        out.push(n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn android_ndk_and_cargo_ndk_are_native_tools_with_pinned_versions() {
        use super::*;
        assert_eq!(parse("ndk@27").unwrap(), Need::Tool { tool: ANDROID_NDK.into(), version: "27".into() }, "ndk is an alias");
        for v in ["27", "r27", "r27d", "27d", "27.3", "27.3.13750724", "lts", "latest"] {
            assert_eq!(ndk_pin(v).map(|p| p.release), Some("r27d"), "{v}");
            check_supported(&parse(&format!("android-ndk@{v}")).unwrap()).unwrap_or_else(|e| panic!("{v}: {e}"));
        }
        for v in ["26", "r26c", "28", "2", "27.4", "r27c"] {
            assert!(ndk_pin(v).is_none(), "{v}");
            let e = check_supported(&parse(&format!("android-ndk@{v}")).unwrap()).unwrap_err();
            assert!(e.contains("r27d (27.3.13750724) pinned"), "{e}");
        }
        for v in ["4", "4.1", "4.1.2", "latest"] {
            check_supported(&parse(&format!("cargo-ndk@{v}")).unwrap()).unwrap_or_else(|e| panic!("{v}: {e}"));
        }
        assert!(check_supported(&parse("cargo-ndk@3").unwrap()).unwrap_err().contains("cargo-ndk 4.1.2 pinned"));
        assert!(supported_list().contains("android-ndk@27") && supported_list().contains("cargo-ndk@4"));
        // The pin's own facts stay consistent (a bump changes them together).
        let p = NDK_PINS.last().unwrap();
        assert!(p.url.ends_with(&format!("android-ndk-{}-linux.zip", p.release)));
        assert_eq!((p.sha256.len(), p.sha1.len()), (64, 40));
        assert!(p.revision.starts_with(&format!("{}.", p.major)));
        assert!(CARGO_NDK_URL.contains(&format!("v{CARGO_NDK_VERSION}")) && CARGO_NDK_SHA256.len() == 64);
    }

    use super::*;

    #[test]
    fn tokens_parse_and_round_trip() {
        assert_eq!(parse("node@22").unwrap(), Need::Tool { tool: "node".into(), version: "22".into() });
        assert_eq!(parse("nodejs@v20.1.0").unwrap().token(), "node@20.1.0");
        assert_eq!(parse("python").unwrap().token(), "python@latest");
        assert_eq!(parse("golang@1.23").unwrap().token(), "go@1.23");
        assert_eq!(parse("rust").unwrap(), Need::Rust { channel: "stable".into(), components: vec![], targets: vec![] });
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
        assert_eq!(n, Need::Rust { channel: "stable".into(), components: vec!["clippy".into(), "rustfmt".into()], targets: vec![] });
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
        assert_eq!(old, Need::Rust { channel: "1.99".into(), components: vec![], targets: vec![] });
        assert_eq!(canonical_component("llvm-tools-preview"), "llvm-tools");
        assert!(supported_list().contains("rust@<channel>[+<component>…][+target:<triple>…]"));
    }

    /// Compile targets ride on the Rust need as `+target:<triple>`,
    /// repeatable and mixed with components, sorted and unique, at most
    /// [`MAX_RUST_TARGETS`]; a need without targets keeps its old token
    /// and wire bytes.
    #[test]
    fn rust_targets_parse() {
        let n = parse("rust@1.95.0+target:wasm32-unknown-unknown").unwrap();
        assert_eq!(n, Need::Rust { channel: "1.95.0".into(), components: vec![], targets: vec!["wasm32-unknown-unknown".into()] });
        assert_eq!(n.token(), "rust@1.95.0+target:wasm32-unknown-unknown");
        assert_eq!(n.key(), "rust");
        let m = parse("rust@stable+target:wasm32-wasip1+clippy+target:aarch64-unknown-linux-gnu+target:wasm32-wasip1+rustfmt").unwrap();
        assert_eq!(
            m,
            Need::Rust {
                channel: "stable".into(),
                components: vec!["clippy".into(), "rustfmt".into()],
                targets: vec!["aarch64-unknown-linux-gnu".into(), "wasm32-wasip1".into()],
            }
        );
        // Components first, then targets, each sorted: one canonical token,
        // which parses back to the same need.
        assert_eq!(m.token(), "rust@stable+clippy+rustfmt+target:aarch64-unknown-linux-gnu+target:wasm32-wasip1");
        assert_eq!(parse(&m.token()).unwrap(), m);
        assert_eq!(parse("rust@nightly+target:thumbv8m.main-none-eabi").unwrap().token(), "rust@nightly+target:thumbv8m.main-none-eabi");
        assert_eq!(parse_all(&["rust@1.95.0+target:wasm32-unknown-unknown", "cmake@3"]).unwrap().len(), 2);
        // A phone build's four targets on a pinned toolchain: one need.
        let phone = parse("rust@1.95.0+target:aarch64-apple-ios+target:aarch64-apple-ios-sim+target:aarch64-linux-android+target:x86_64-linux-android").unwrap();
        assert_eq!(
            phone,
            Need::Rust {
                channel: "1.95.0".into(),
                components: vec![],
                targets: vec!["aarch64-apple-ios".into(), "aarch64-apple-ios-sim".into(), "aarch64-linux-android".into(), "x86_64-linux-android".into()],
            }
        );
        assert_eq!(parse(&phone.token()).unwrap(), phone);
        for bad in [
            "rust@stable+target:",
            "rust@stable+target:-x",
            "rust@stable+target:Wasm32-unknown-unknown",
            "rust@stable+target:wasm32",
            "rust@stable+target:../x-y",
            "rust@stable+target:a/b-c",
            "rust@stable+target:a b-c",
            "rust@stable+target:wasm32-unknown-unknown;x",
            "rust@stable+targets:wasm32-unknown-unknown",
            "rust@+target:wasm32-unknown-unknown",
            "rust@sta:ble",
            "node@22+target:wasm32-unknown-unknown",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be refused");
        }
        let e = parse_all(&["rust@stable+target:wasm32"]).unwrap_err();
        assert_eq!(e.code, NEED_INVALID);
        assert!(e.message.contains("isn't a Rust target triple") && e.message.contains("+target:wasm32-unknown-unknown"), "{e}");
        let nine: String = (0..9).map(|i| format!("+target:t{i}-unknown-none")).collect();
        assert!(parse(&format!("rust@stable{nine}")).unwrap_err().contains("at most 8 targets"));
        let eight: String = (0..8).map(|i| format!("+target:t{i}-unknown-none")).collect();
        assert!(parse(&format!("rust@stable{eight}")).is_ok());
        // Wire: `targets` only when there are some; an older plan reads.
        assert_eq!(serde_json::to_string(&parse("rust@stable+clippy").unwrap()).unwrap(), r#"{"kind":"rust","channel":"stable","components":["clippy"]}"#);
        assert_eq!(serde_json::to_string(&n).unwrap(), r#"{"kind":"rust","channel":"1.95.0","targets":["wasm32-unknown-unknown"]}"#);
        assert!(valid_target("wasm32-unknown-unknown") && valid_target("x86_64-pc-windows-msvc") && !valid_target("x"));
        assert!(supported_list().contains("+target:<triple>"));
    }

    /// prd-compute-self-serve-environments §5.1, §7.1, §8.
    #[test]
    fn native_profile_and_minimal_tokens_parse() {
        assert_eq!(parse("sys:pacman:protobuf").unwrap(), Need::System { package: "pacman:protobuf".into() });
        assert_eq!(parse("sys:pacman:protobuf").unwrap().token(), "sys:pacman:protobuf");
        assert!(parse("sys:brew:protobuf").is_err(), "brew is no manager here");
        assert!(parse("sys:apt:-y").is_err());
        let p = parse("profile:tauri-linux@1").unwrap();
        assert_eq!(p, Need::Profile { name: "tauri-linux".into(), version: Some(1) });
        assert_eq!(p.token(), "profile:tauri-linux@1");
        assert_eq!(parse("profile:tauri-linux").unwrap().key(), "profile:tauri-linux");
        let e = parse_all(&["profile:nope"]).unwrap_err();
        assert_eq!(e.code, crate::profiles::PROFILE_UNKNOWN, "{e}");
        assert_eq!(parse_all(&["profile:tauri-linux@99"]).unwrap_err().code, crate::profiles::PROFILE_UNKNOWN);
        let m = parse("rust@stable+minimal").unwrap();
        assert_eq!(m.token(), "rust@stable+minimal");
        assert!(!m.rust_wants_defaults());
        assert!(parse("rust@stable").unwrap().rust_wants_defaults());
        assert!(parse("rust@stable+clippy").unwrap().rust_wants_defaults());
        assert!(supported_list().contains("profile:<name>"));
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

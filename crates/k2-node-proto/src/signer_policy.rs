//! The signer's per-project policy file (C4; PRD §4.5, vs-live V12, V13,
//! V30, S1, S6, S9). Formats only: `docs/ci-formats-v1.md` §8.
//!
//! One root-owned TOML file per project
//! (`/usr/local/etc/k2-signer/projects/<project>.toml`). The signer never
//! reads it from the repo at sign time; `k2 signer policy propose` shows a
//! diff and a person approves it. Unknown keys are refused; enum values are
//! snake_case, like the wire.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ci::{self, Dimension, Isolation, TaintBasis, Wall};
use crate::share::Network;

/// The policy file's `version`.
pub const SIGNER_POLICY_VERSION: u32 = 1;

/// A project's signing policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignerPolicy {
    /// [`SIGNER_POLICY_VERSION`].
    pub version: u32,
    /// `k2`, `acme-desktop`.
    pub project: String,
    /// Paired controller fingerprints allowed to build it (§4.2).
    pub controllers: Vec<String>,
    /// Pinned node-roster key fingerprints (SHA-256 of the raw Ed25519 key)
    /// whose rosters it trusts (§4.1).
    pub owners: Vec<String>,
    /// Workspace ids whose jobs may build it (V30; receipts sign
    /// `workspace_id`).
    pub workspaces: Vec<String>,
    /// A receipt older than this (ended) is refused.
    pub receipt_max_hours: u32,
    /// V13: a roster issued longer ago than this is refused (default 168).
    pub roster_max_age_hours: u32,
    /// S1/V30: a roster that adds or re-keys a node: `ask` the person at the
    /// signer (default, 0.46.10) or accept it (`auto`, with a note).
    pub roster_changes: Changes,
    /// 0.46.11: an owner-signed seat move.
    pub seat_moves: Changes,
    /// V12: the taint bases this project accepts for a node key.
    pub taint_basis: Vec<TaintBasis>,
    /// prd-compute-build-attestations-v1 §7.2, slice S4: whether each
    /// receipt needs the build statement its node signed beside it. `off`
    /// (the default, never written: today's rule, statements are not
    /// read), `prefer` (a statement that is there must pass every check; a
    /// receipt without one passes with a note) or `required` (every receipt
    /// has one that passes; a v2 bundle is refused). An older signer
    /// refuses a policy that sets it (unknown key).
    #[serde(default, skip_serializing_if = "ProvenanceRule::is_off")]
    pub provenance: ProvenanceRule,
    /// The oldest k2-node build a release job may have run on
    /// (`0.46.13`): checked against each receipt's `run.k2node_version`,
    /// which its statement repeats. Optional; absent = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_k2node: Option<String>,
    pub source: SourceRule,
    /// Builds by name (`linux-x86_64`).
    pub build: BTreeMap<String, BuildRule>,
    /// Signable files by asset name.
    pub assets: BTreeMap<String, AssetRule>,
    /// Signing keys by slot name (`updater`): the public half; the secret
    /// half stays in the role user's key dir.
    pub keys: BTreeMap<String, KeyRule>,
}

/// `provenance = "off" | "prefer" | "required"` (slice S4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceRule {
    /// Statements are not read (today's rule).
    #[default]
    Off,
    /// A statement that is there is checked and must pass; a receipt with
    /// none passes, with a note.
    Prefer,
    /// Every receipt has a statement that passes; a v2 bundle is refused.
    Required,
}

impl ProvenanceRule {
    pub fn is_off(&self) -> bool {
        *self == ProvenanceRule::Off
    }
}

/// A k2-node version (`0.46.13`, or `0.46.13-rc1`): `(major, minor, patch,
/// release)` where `release` is false for a pre-release, which sorts below
/// the release of the same number. `None` for anything else.
pub fn k2node_version(s: &str) -> Option<(u64, u64, u64, bool)> {
    let (num, pre) = match s.split_once('-') {
        Some((n, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') => (n, true),
        Some(_) => return None,
        None => (s, false),
    };
    let parts: Vec<&str> = num.split('.').collect();
    let [a, b, c] = parts.as_slice() else { return None };
    let n = |x: &str| if !x.is_empty() && x.len() <= 9 && x.bytes().all(|d| d.is_ascii_digit()) { x.parse::<u64>().ok() } else { None };
    Some((n(a)?, n(b)?, n(c)?, !pre))
}

/// `have` is at least `min` (both [`k2node_version`]s). `None` when either
/// doesn't read as one.
pub fn k2node_at_least(have: &str, min: &str) -> Option<bool> {
    Some(k2node_version(have)? >= k2node_version(min)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Changes {
    Ask,
    Auto,
}

/// How a version binds to a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRule {
    pub kind: SourceKind,
    /// `owner/repo` for `github_tag` (read with the signer's own read-only
    /// token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The tag is `<tag_prefix><version>` (default `v`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// The forge's tag names the commit (K2: `v<version>` on GitHub).
    GithubTag,
    /// The person's approval names the commit; no forge.
    ApprovalCommit,
}

/// One build the signer accepts receipts for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRule {
    /// The exact command (receipts sign its hash).
    pub argv: Vec<String>,
    /// `--needs` tokens; `{rust_pin}` is the commit's rust-toolchain channel.
    pub needs: Vec<String>,
    /// The plan's artifact globs, exactly.
    pub artifacts: Vec<String>,
    /// `--env` names the plan may set (values are the build's facts).
    #[serde(default)]
    pub env: Vec<String>,
    /// Vault env names the job must get (empty = not a secret job).
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Vault env names it may also get.
    #[serde(default)]
    pub secrets_optional: Vec<String>,
    /// Accepted isolation (receipt v2 `run.isolation`).
    pub isolation: Vec<Isolation>,
    /// Accepted walls (default `["job_user"]`).
    #[serde(default = "default_walls")]
    pub walls: Vec<Wall>,
    /// Artifact names a verify build must reproduce byte for byte; empty =
    /// no dual build.
    #[serde(default)]
    pub dual: Vec<String>,
    /// What build and verify nodes must differ in (`node` is implied).
    #[serde(default)]
    pub independent: Vec<Dimension>,
    /// Slice S4: the networks a job of this build may have run under, as
    /// its build statement records them (`none`, `no-lan`, `on`). Only a
    /// statement says it, so it needs `provenance = "prefer"` or
    /// `"required"`, and a receipt without a statement is refused.
    /// Optional; absent = any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<Network>,
    /// S9: SHA-256 hex of values baked into the binary, by secret name
    /// (catches a swapped vault value).
    #[serde(default)]
    pub secret_sha256: BTreeMap<String, String>,
    /// prd-compute-blacksmith-class-v1 §4.2.3: may a build start from a
    /// cache? `private` (the default: today's rule, every build empty) or
    /// `signed-chain` (a `warmth: gen` receipt v3 passes only when its
    /// cache chain walks back through this signer's own stored evidence).
    /// Absent when `private`, so every older policy keeps its bytes; an
    /// older signer refuses a policy that sets it (unknown key).
    #[serde(default, skip_serializing_if = "WarmPolicy::is_private")]
    pub warm: WarmPolicy,
    /// With `signed-chain` only: the deepest chain since a cold build
    /// (default [`WARM_MAX_CHAIN_DEFAULT`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_max_chain: Option<u32>,
    /// The job's rustc (`tools_used.rustc`) must be the commit's pinned
    /// channel even when `needs` names no `rust@{rust_pin}` (a build that
    /// takes its toolchain from the commit's `rust-toolchain.toml`).
    /// Optional; absent = only a `{rust_pin}` need pins it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rust_pin: bool,
    /// A file the job wrote whose facts must hold (BA-10: the hashes of a
    /// cross toolchain the build fetched itself). Job-reported: the signer
    /// trusts it only because its bytes are an artifact in the node-signed
    /// receipt, and checks only the values named here. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<ReportedRule>,
}

/// `[build.<name>.reported]`: one JSON file among the build's artifacts and
/// the exact string values it must hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedRule {
    /// The artifact's name in the receipt (`dist/build-info.json`).
    pub file: String,
    /// JSON pointer (RFC 6901, `/cross/zig_tarball_sha256`) → the exact
    /// string it must be.
    pub equals: BTreeMap<String, String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_true(b: &bool) -> bool {
    *b
}

fn default_true() -> bool {
    true
}

/// A build's cache rule at the signer (§4.2.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarmPolicy {
    /// Only `warmth: private` receipts (today's rule).
    #[default]
    #[serde(rename = "private")]
    Private,
    /// `warmth: gen` too, when the chain is proven from stored evidence.
    #[serde(rename = "signed-chain")]
    SignedChain,
}

impl WarmPolicy {
    pub fn is_private(&self) -> bool {
        *self == WarmPolicy::Private
    }
}

/// `warm_max_chain` when unset.
pub const WARM_MAX_CHAIN_DEFAULT: u32 = 10;
/// The largest `warm_max_chain` a policy may set.
pub const WARM_MAX_CHAIN_MAX: u32 = 50;

impl BuildRule {
    /// The chain depth this build accepts (`warm = "signed-chain"`).
    pub fn warm_max_chain(&self) -> u32 {
        self.warm_max_chain.unwrap_or(WARM_MAX_CHAIN_DEFAULT)
    }
}

fn default_walls() -> Vec<Wall> {
    vec![Wall::JobUser]
}

/// One signable file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRule {
    /// A `[keys.<slot>]` name.
    pub key: String,
    /// A `[build.<name>]` name.
    pub build: String,
    /// The artifact in the build receipt (`dist/acme.AppImage`).
    pub artifact: String,
    pub check: AssetCheck,
    /// For `inner_binary`: the path inside the bundle (Q11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetCheck {
    /// The file is the build receipt's artifact, byte for byte.
    Artifact,
    /// Q11: the bundle's inner binary matched the verify build (bundles that
    /// don't reproduce yet; `.tar.gz` and `.deb` first).
    InnerBinary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyRule {
    pub kind: KeyKind,
    /// The public key (minisign: the base64 line starting `RW`).
    pub public: String,
    /// `false`: the signer never holds or uses this key. Its assets get a
    /// `verdict` only (every evidence check, no signature), for a script
    /// that signs with a key the signer doesn't hold (K2's root-helper key,
    /// BA-11). Optional; absent = the signer holds it.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub held: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    /// minisign / Tauri updater signatures.
    Minisign,
}

impl SignerPolicy {
    pub fn parse(text: &str) -> Result<Self, String> {
        let p: SignerPolicy = toml::from_str(text).map_err(|e| format!("signer policy: {e}"))?;
        p.check()?;
        Ok(p)
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| format!("signer policy: {e}"))
    }

    pub fn check(&self) -> Result<(), String> {
        if self.version != SIGNER_POLICY_VERSION {
            return Err(format!("signer policy version {} (this build knows {SIGNER_POLICY_VERSION})", self.version));
        }
        ci::check_name("project", &self.project)?;
        nonempty_fps("controllers", &self.controllers)?;
        nonempty_fps("owners", &self.owners)?;
        if self.workspaces.is_empty() {
            return Err("workspaces: name the workspace ids that build this project".into());
        }
        for w in &self.workspaces {
            ci::check_text("workspaces", w, 128)?;
        }
        if !(1..=168).contains(&self.receipt_max_hours) {
            return Err("receipt_max_hours: 1-168".into());
        }
        if !(1..=720).contains(&self.roster_max_age_hours) {
            return Err("roster_max_age_hours: 1-720".into());
        }
        if self.taint_basis.is_empty() {
            return Err("taint_basis: at least one".into());
        }
        let mut tb = self.taint_basis.clone();
        tb.sort();
        tb.dedup();
        if tb.len() != self.taint_basis.len() {
            return Err("taint_basis: listed twice".into());
        }
        match (self.source.kind, &self.source.repo) {
            (SourceKind::GithubTag, Some(r)) if r.split('/').count() == 2 && !r.contains("..") => {}
            (SourceKind::GithubTag, _) => return Err("source.repo: owner/repo for github_tag".into()),
            (SourceKind::ApprovalCommit, None) => {}
            (SourceKind::ApprovalCommit, Some(_)) => return Err("source.repo: only for github_tag".into()),
        }
        if self.build.is_empty() || self.assets.is_empty() || self.keys.is_empty() {
            return Err("build, assets and keys: at least one each".into());
        }
        for (name, b) in &self.build {
            ci::check_name("build", name)?;
            b.check(name)?;
        }
        if let Some(v) = &self.min_k2node {
            if k2node_version(v).is_none() {
                return Err(format!("min_k2node: {v:?} is not a k2-node version (0.46.13)"));
            }
        }
        if self.provenance.is_off() {
            if let Some((name, _)) = self.build.iter().find(|(_, b)| !b.network.is_empty()) {
                return Err(format!(
                    "build.{name}.network: only a build statement records the network, so it needs provenance = \"prefer\" or \"required\""
                ));
            }
        }
        for (name, k) in &self.keys {
            ci::check_name("keys", name)?;
            ci::check_text(&format!("keys.{name}.public"), &k.public, 512)?;
            if k.public.trim().is_empty() {
                return Err(format!("keys.{name}.public is empty"));
            }
        }
        for (name, a) in &self.assets {
            ci::check_text("assets", name, 128)?;
            if name.is_empty() || name.contains('/') || name.starts_with('.') {
                return Err(format!("assets: {name:?} is not a file name"));
            }
            if !self.keys.contains_key(&a.key) {
                return Err(format!("assets.{name}.key: no [keys.{}]", a.key));
            }
            if !self.build.contains_key(&a.build) {
                return Err(format!("assets.{name}.build: no [build.{}]", a.build));
            }
            ci::check_text(&format!("assets.{name}.artifact"), &a.artifact, 512)?;
            match (a.check, &a.inner) {
                (AssetCheck::Artifact, None) => {}
                (AssetCheck::InnerBinary, Some(i)) if !i.is_empty() && !i.starts_with('/') && !i.contains("..") => {
                    if self.build[&a.build].dual.is_empty() {
                        return Err(format!("assets.{name}: inner_binary needs a dual build"));
                    }
                }
                _ => return Err(format!("assets.{name}: inner is a relative path exactly for check = inner_binary")),
            }
        }
        Ok(())
    }
}

impl BuildRule {
    fn check(&self, name: &str) -> Result<(), String> {
        if self.argv.is_empty() {
            return Err(format!("build.{name}.argv is empty"));
        }
        if self.artifacts.is_empty() {
            return Err(format!("build.{name}.artifacts is empty"));
        }
        if self.isolation.is_empty() || self.walls.is_empty() {
            return Err(format!("build.{name}: isolation and walls need at least one value"));
        }
        for e in self.env.iter().chain(&self.secrets).chain(&self.secrets_optional) {
            if let Some(code) = crate::env::env_name_refusal(e) {
                return Err(format!("build.{name}: {e:?} is not a job env name ({code})"));
            }
        }
        if self.secrets.iter().any(|s| self.secrets_optional.contains(s)) {
            return Err(format!("build.{name}: a secret is both required and optional"));
        }
        let secret = !self.secrets.is_empty() || !self.secrets_optional.is_empty();
        if secret && self.isolation.iter().any(|i| !matches!(i, Isolation::JobUid | Isolation::Exclusive)) {
            return Err(format!("build.{name}: a secret job accepts only job_uid or exclusive isolation"));
        }
        if secret && self.walls != [Wall::JobUser] {
            return Err(format!("build.{name}: a secret job runs only behind the job_user wall"));
        }
        if self.walls.contains(&Wall::None) {
            return Err(format!("build.{name}: wall none is never accepted"));
        }
        if self.walls.contains(&Wall::Seatbelt) && !self.dual.is_empty() && !self.independent.contains(&Dimension::Owner) {
            // S6: a one-uid Mac node can be changed in flight by a co-tenant.
            return Err(format!("build.{name}: seatbelt builds need independent = [\"owner\"] (S6)"));
        }
        if !self.dual.is_empty() && self.independent.is_empty() {
            return Err(format!("build.{name}: a dual build names its independent dimensions"));
        }
        if self.dual.is_empty() && !self.independent.is_empty() {
            return Err(format!("build.{name}: independent without a dual build"));
        }
        let mut nets = self.network.clone();
        nets.sort();
        nets.dedup();
        if nets.len() != self.network.len() {
            return Err(format!("build.{name}.network: a network listed twice"));
        }
        for (k, v) in &self.secret_sha256 {
            if !self.secrets.contains(k) && !self.secrets_optional.contains(k) {
                return Err(format!("build.{name}.secret_sha256: {k} is not one of its secrets"));
            }
            ci::check_fp(&format!("build.{name}.secret_sha256.{k}"), v)?;
        }
        match (self.warm, self.warm_max_chain) {
            (WarmPolicy::Private, Some(_)) => return Err(format!("build.{name}.warm_max_chain: only with warm = \"signed-chain\"")),
            (WarmPolicy::SignedChain, Some(n)) if !(1..=WARM_MAX_CHAIN_MAX).contains(&n) => {
                return Err(format!("build.{name}.warm_max_chain: 1-{WARM_MAX_CHAIN_MAX}"));
            }
            _ => {}
        }
        if let Some(r) = &self.reported {
            ci::check_text(&format!("build.{name}.reported.file"), &r.file, 512)?;
            if r.file.is_empty() || r.file.starts_with('/') || r.file.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                return Err(format!("build.{name}.reported.file: a relative artifact path ({:?})", r.file));
            }
            if r.equals.is_empty() {
                return Err(format!("build.{name}.reported.equals: at least one value"));
            }
            for (ptr, v) in &r.equals {
                ci::check_text(&format!("build.{name}.reported.equals"), ptr, 256)?;
                if !ptr.starts_with('/') {
                    return Err(format!("build.{name}.reported.equals: {ptr:?} is not a JSON pointer (/a/b)"));
                }
                ci::check_text(&format!("build.{name}.reported.equals.{ptr}"), v, 512)?;
            }
        }
        Ok(())
    }
}

fn nonempty_fps(what: &str, v: &[String]) -> Result<(), String> {
    if v.is_empty() {
        return Err(format!("{what}: at least one fingerprint"));
    }
    for f in v {
        ci::check_fp(what, f)?;
    }
    ci::check_sorted_unique(what, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"
version = 1
project = "acme-desktop"
controllers = ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
owners = ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
workspaces = ["ws-1"]
receipt_max_hours = 24
roster_max_age_hours = 168
roster_changes = "ask"
seat_moves = "ask"
taint_basis = ["rekeyed", "self_clean"]

[source]
kind = "approval_commit"

[build.linux-x86_64]
argv = ["bash", "scripts/ci/build.sh"]
needs = ["rust@{rust_pin}"]
artifacts = ["dist/*"]
isolation = ["job_uid", "exclusive"]

[assets."acme.AppImage"]
key = "updater"
build = "linux-x86_64"
artifact = "dist/acme.AppImage"
check = "artifact"

[keys.updater]
kind = "minisign"
public = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
"#;

    #[test]
    fn minimal_policy_parses_with_defaults_and_round_trips() {
        let p = SignerPolicy::parse(MIN).unwrap();
        assert_eq!(p.build["linux-x86_64"].walls, vec![Wall::JobUser]);
        assert!(p.build["linux-x86_64"].dual.is_empty());
        assert_eq!(SignerPolicy::parse(&p.to_toml().unwrap()).unwrap(), p);
    }

    #[test]
    fn unknown_keys_and_bad_rules_are_refused() {
        assert!(SignerPolicy::parse(&MIN.replace("version = 1", "version = 1\nsurprise = true")).is_err());
        assert!(SignerPolicy::parse(&MIN.replace("version = 1", "version = 2")).is_err());
        assert!(SignerPolicy::parse(&MIN.replace("workspaces = [\"ws-1\"]", "workspaces = []")).is_err());
        assert!(SignerPolicy::parse(&MIN.replace("kind = \"approval_commit\"", "kind = \"github_tag\"")).is_err(), "github_tag needs a repo");
        assert!(SignerPolicy::parse(&MIN.replace("roster_changes = \"ask\"", "roster_changes = \"maybe\"")).is_err());
        let secret = MIN.replace("isolation = [\"job_uid\", \"exclusive\"]", "isolation = [\"tenant_uid\"]\nsecrets = [\"BUILD_API_KEY\"]");
        assert!(SignerPolicy::parse(&secret).unwrap_err().contains("job_uid or exclusive"));
        let dual = MIN.replace("artifacts = [\"dist/*\"]", "artifacts = [\"dist/*\"]\ndual = [\"dist/acme\"]");
        assert!(SignerPolicy::parse(&dual).unwrap_err().contains("independent"));
        SignerPolicy::parse(&dual.replace("dual = [", "independent = [\"host\"]\ndual = [")).unwrap();
        let k2env = MIN.replace("isolation = [", "env = [\"K2_X\"]\nisolation = [");
        assert!(SignerPolicy::parse(&k2env).is_err());
        let inner = MIN.replace("check = \"artifact\"", "check = \"inner_binary\"\ninner = \"usr/bin/acme\"");
        assert!(SignerPolicy::parse(&inner).unwrap_err().contains("dual"));
    }

    /// Slice S4: `provenance`, `min_k2node` and `[build.*].network` are
    /// optional; absent, the file's bytes are as before.
    #[test]
    fn provenance_keys_default_off_and_keep_old_bytes() {
        let p = SignerPolicy::parse(MIN).unwrap();
        assert_eq!(p.provenance, ProvenanceRule::Off);
        assert!(p.min_k2node.is_none() && p.build["linux-x86_64"].network.is_empty());
        let text = p.to_toml().unwrap();
        assert!(!text.contains("provenance") && !text.contains("min_k2node") && !text.contains("network"), "{text}");

        let with = MIN
            .replace("taint_basis = [", "provenance = \"required\"\nmin_k2node = \"0.46.13\"\ntaint_basis = [")
            .replace("isolation = [\"job_uid\", \"exclusive\"]", "isolation = [\"job_uid\", \"exclusive\"]\nnetwork = [\"none\", \"no-lan\"]");
        let p = SignerPolicy::parse(&with).unwrap();
        assert_eq!(p.provenance, ProvenanceRule::Required);
        assert_eq!(p.min_k2node.as_deref(), Some("0.46.13"));
        assert_eq!(p.build["linux-x86_64"].network, vec![Network::None, Network::NoLan]);
        assert_eq!(SignerPolicy::parse(&p.to_toml().unwrap()).unwrap(), p);
        assert_eq!(SignerPolicy::parse(&with.replace("\"required\"", "\"prefer\"")).unwrap().provenance, ProvenanceRule::Prefer);
        let off = SignerPolicy::parse(&with.replace("\"required\"", "\"off\"").replace("network = [\"none\", \"no-lan\"]\n", "")).unwrap();
        assert_eq!(off.provenance, ProvenanceRule::Off);

        let bad = |from: &str, to: &str| SignerPolicy::parse(&with.replace(from, to)).unwrap_err();
        assert!(bad("\"required\"", "\"optional\"").contains("provenance"), "{}", bad("\"required\"", "\"optional\""));
        assert!(bad("\"required\"", "\"off\"").contains("needs provenance"), "network is only in a statement");
        assert!(bad("\"0.46.13\"", "\"0.46\"").contains("min_k2node"));
        assert!(bad("\"0.46.13\"", "\"latest\"").contains("min_k2node"));
        assert!(bad("[\"none\", \"no-lan\"]", "[\"none\", \"none\"]").contains("twice"));
        assert!(bad("[\"none\", \"no-lan\"]", "[\"no_lan\"]").contains("network"));
    }

    #[test]
    fn k2node_versions_order_with_pre_releases_below() {
        assert_eq!(k2node_at_least("0.46.13", "0.46.13"), Some(true));
        assert_eq!(k2node_at_least("0.46.14", "0.46.13"), Some(true));
        assert_eq!(k2node_at_least("0.47.0", "0.46.13"), Some(true));
        assert_eq!(k2node_at_least("0.46.12", "0.46.13"), Some(false));
        assert_eq!(k2node_at_least("0.46.9", "0.46.13"), Some(false), "numbers, not text");
        assert_eq!(k2node_at_least("0.46.13-rc1", "0.46.13"), Some(false), "a pre-release is below its release");
        assert_eq!(k2node_at_least("0.46.13-rc1", "0.46.12"), Some(true));
        assert_eq!(k2node_at_least("bad", "0.46.13"), None);
        for bad in ["", "0.46", "0.46.13.1", "v0.46.13", "0.46.x", "0.46.13-", "0.46.13-a b"] {
            assert_eq!(k2node_version(bad), None, "{bad:?}");
        }
    }

    /// prd-compute-blacksmith-class-v1 §4.2.3: `warm` defaults to private
    /// and stays out of the file; `signed-chain` and its depth parse.
    #[test]
    fn warm_policy_defaults_private_and_keeps_old_bytes() {
        let p = SignerPolicy::parse(MIN).unwrap();
        let b = &p.build["linux-x86_64"];
        assert_eq!(b.warm, WarmPolicy::Private);
        assert_eq!(b.warm_max_chain(), WARM_MAX_CHAIN_DEFAULT);
        let text = p.to_toml().unwrap();
        assert!(!text.contains("warm"), "{text}");
        let warm = MIN.replace("isolation = [\"job_uid\", \"exclusive\"]", "isolation = [\"job_uid\", \"exclusive\"]\nwarm = \"signed-chain\"\nwarm_max_chain = 4");
        let w = SignerPolicy::parse(&warm).unwrap();
        assert_eq!(w.build["linux-x86_64"].warm, WarmPolicy::SignedChain);
        assert_eq!(w.build["linux-x86_64"].warm_max_chain(), 4);
        assert_eq!(SignerPolicy::parse(&w.to_toml().unwrap()).unwrap(), w);
        assert!(SignerPolicy::parse(&warm.replace("signed-chain", "signed_chain")).is_err());
        assert!(SignerPolicy::parse(&warm.replace("signed-chain", "shared")).is_err());
        assert!(SignerPolicy::parse(&warm.replace("warm_max_chain = 4", "warm_max_chain = 0")).unwrap_err().contains("1-50"));
        let lone = MIN.replace("isolation = [\"job_uid\", \"exclusive\"]", "isolation = [\"job_uid\", \"exclusive\"]\nwarm_max_chain = 4");
        assert!(SignerPolicy::parse(&lone).unwrap_err().contains("only with warm"));
    }

    /// The optional keys (absent = the old rule, so older files parse the same
    /// and serialise without them).
    const OPTIONAL: &str = "isolation = [\"job_uid\", \"exclusive\"]\nrust_pin = true\n\n\
        [build.linux-x86_64.reported]\nfile = \"dist/build-info.json\"\n\
        equals = { \"/cross/zig_tarball_sha256\" = \"aa\", \"/cross/cargo_zigbuild_crate_sha256\" = \"bb\" }\n";

    #[test]
    fn optional_keys_parse_and_absent_ones_keep_the_old_shape() {
        let plain = SignerPolicy::parse(MIN).unwrap();
        let b = &plain.build["linux-x86_64"];
        assert!(!b.rust_pin && b.reported.is_none());
        assert!(plain.keys["updater"].held);
        let text = plain.to_toml().unwrap();
        assert!(!text.contains("rust_pin") && !text.contains("reported") && !text.contains("held"), "{text}");

        let with = MIN.replace("isolation = [\"job_uid\", \"exclusive\"]\n", OPTIONAL)
            .replace("[keys.updater]", "[keys.root-helper]\nkind = \"minisign\"\npublic = \"RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\"\nheld = false\n\n[keys.updater]");
        let p = SignerPolicy::parse(&with).unwrap();
        let b = &p.build["linux-x86_64"];
        assert!(b.rust_pin);
        let r = b.reported.as_ref().unwrap();
        assert_eq!(r.file, "dist/build-info.json");
        assert_eq!(r.equals["/cross/zig_tarball_sha256"], "aa");
        assert!(!p.keys["root-helper"].held && p.keys["updater"].held);
        assert_eq!(SignerPolicy::parse(&p.to_toml().unwrap()).unwrap(), p);
    }

    #[test]
    fn bad_reported_rules_are_refused() {
        let with = MIN.replace("isolation = [\"job_uid\", \"exclusive\"]\n", OPTIONAL);
        let bad = |from: &str, to: &str| SignerPolicy::parse(&with.replace(from, to)).unwrap_err();
        assert!(bad("file = \"dist/build-info.json\"", "file = \"../build-info.json\"").contains("reported.file"));
        assert!(bad("file = \"dist/build-info.json\"", "file = \"/etc/passwd\"").contains("reported.file"));
        assert!(bad("\"/cross/zig_tarball_sha256\"", "\"cross/zig\"").contains("JSON pointer"));
        let none = with.replace(
            "equals = { \"/cross/zig_tarball_sha256\" = \"aa\", \"/cross/cargo_zigbuild_crate_sha256\" = \"bb\" }",
            "equals = {}",
        );
        assert!(SignerPolicy::parse(&none).unwrap_err().contains("at least one"));
        assert!(SignerPolicy::parse(&with.replace("file = \"dist/build-info.json\"\n", "file = \"dist/build-info.json\"\nsurprise = 1\n")).is_err());
    }
}

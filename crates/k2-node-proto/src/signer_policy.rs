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
    pub source: SourceRule,
    /// Builds by name (`linux-x86_64`).
    pub build: BTreeMap<String, BuildRule>,
    /// Signable files by asset name.
    pub assets: BTreeMap<String, AssetRule>,
    /// Signing keys by slot name (`updater`): the public half; the secret
    /// half stays in the role user's key dir.
    pub keys: BTreeMap<String, KeyRule>,
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
    /// S9: SHA-256 hex of values baked into the binary, by secret name
    /// (catches a swapped vault value).
    #[serde(default)]
    pub secret_sha256: BTreeMap<String, String>,
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
        for (k, v) in &self.secret_sha256 {
            if !self.secrets.contains(k) && !self.secrets_optional.contains(k) {
                return Err(format!("build.{name}.secret_sha256: {k} is not one of its secrets"));
            }
            ci::check_fp(&format!("build.{name}.secret_sha256.{k}"), v)?;
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
}

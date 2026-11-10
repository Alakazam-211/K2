//! The `k2 ci` config file, `.k2/ci.toml` (C6; PRD §4.7, vs-live V32).
//! Formats only: `docs/ci-formats-v1.md` §9.
//!
//! One file per repo (V32 left `.k2/compute.toml` sections vs `.k2/ci.toml`
//! to the implementer: it is `.k2/ci.toml`, the file `k2 ci init` writes).
//! The daemon reads it at the commit it builds. It is a convenience for the
//! controller, never trusted by the signer: the signer checks receipts
//! against its own policy ([`crate::signer_policy`]), so a `[release]`
//! build here must match the policy's `[build.<name>]` or signing refuses.
//! Unknown keys are refused; enum values are snake_case.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ci;

/// The file's `version`.
pub const CI_CONFIG_VERSION: u32 = 1;
/// Where it lives in a repo.
pub const CI_CONFIG_PATH: &str = ".k2/ci.toml";
/// The statuses token's job env name (prd-ci-release stage-1 vs-live A12:
/// `K2_` is a denied env prefix, so not `K2_CI_STATUSES_TOKEN`).
pub const DEFAULT_STATUSES_ENV: &str = "CI_STATUSES_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiConfig {
    /// [`CI_CONFIG_VERSION`].
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<WatchConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statuses: Option<StatusesConfig>,
    /// Check jobs by name (`unit`, `clippy`); each tip runs all of them.
    #[serde(default)]
    pub checks: BTreeMap<String, CheckJob>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseConfig>,
}

/// `k2 ci watch`: the daemon polls the forge and runs each new tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchConfig {
    /// The git remote (`origin`).
    pub remote: String,
    /// Branch names or `prefix/*` globs.
    pub branches: Vec<String>,
    /// Poll interval, 30–3600 s (default 60).
    #[serde(default = "default_poll")]
    pub poll_secs: u32,
}

fn default_poll() -> u32 {
    60
}

/// `k2 ci statuses`: commit statuses posted with a vault token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusesConfig {
    /// The vault secret name (`K2_CI_STATUSES_TOKEN`).
    pub token_secret: String,
    /// The env name the status job gets it as (default
    /// [`DEFAULT_STATUSES_ENV`]).
    #[serde(default = "default_statuses_env")]
    pub env_name: String,
    /// The status context prefix (default `k2-ci`): `<context>/<check>`.
    #[serde(default = "default_context")]
    pub context: String,
}

fn default_statuses_env() -> String {
    DEFAULT_STATUSES_ENV.to_string()
}

fn default_context() -> String {
    "k2-ci".to_string()
}

/// One check job (`--tier ci`, `--supersede`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckJob {
    pub argv: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    /// `linux` | `macos`; absent = any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_gb: Option<u32>,
}

/// `k2 ci release`: the builds of one release set, placed together on the
/// seat (C5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// The signer project (`k2`): the bundle's `project`.
    pub project: String,
    /// The node owner whose seat runs it (`rosson.k2.dev`).
    pub owner: String,
    /// The lane on that seat (`ci-standard`).
    pub lane: String,
    /// By signer-policy build name.
    pub builds: BTreeMap<String, ReleaseBuild>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseBuild {
    pub argv: Vec<String>,
    pub needs: Vec<String>,
    pub artifacts: Vec<String>,
    /// `--env` names; the controller sets `BUILD_COMMIT` / `BUILD_TREE`.
    #[serde(default)]
    pub env: Vec<String>,
    /// Vault env names (lane grants, `compute:<ws>/lane:<owner>/<lane>`).
    #[serde(default)]
    pub secrets: Vec<String>,
    /// The build's receipt label (`L1`, `L3`).
    pub label: String,
    /// The verify build's label (`V1`); set = dual build on an independent
    /// node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_label: Option<String>,
    /// How the verify build's node must differ from the build's (C5, PRD
    /// §4.6): dimensions of the owner's node roster. Empty = the default,
    /// [`DEFAULT_INDEPENDENT`]; only with `verify_label`. The controller
    /// places by it; the signer checks its own policy's list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub independent: Vec<ci::Dimension>,
    /// The job's limits, as `[checks.<name>]`'s (C6b). The signer doesn't
    /// read them: they change where and how long it runs, not what it built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_gb: Option<u32>,
}

/// A dual build's independence when `independent` is unset (PRD §4.6:
/// `independent = ["host"]`; `node` is always implied).
pub const DEFAULT_INDEPENDENT: &[ci::Dimension] = &[ci::Dimension::Host];

impl ReleaseBuild {
    /// The dimensions the verify build must differ in (`node` first, always).
    pub fn independence(&self) -> Vec<ci::Dimension> {
        let mut out = vec![ci::Dimension::Node];
        let asked: &[ci::Dimension] = if self.independent.is_empty() { DEFAULT_INDEPENDENT } else { &self.independent };
        for d in asked {
            if !out.contains(d) {
                out.push(*d);
            }
        }
        out
    }
}

impl CiConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        let c: CiConfig = toml::from_str(text).map_err(|e| format!("{CI_CONFIG_PATH}: {e}"))?;
        c.check()?;
        Ok(c)
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| format!("{CI_CONFIG_PATH}: {e}"))
    }

    pub fn check(&self) -> Result<(), String> {
        if self.version != CI_CONFIG_VERSION {
            return Err(format!("{CI_CONFIG_PATH}: version {} (this build knows {CI_CONFIG_VERSION})", self.version));
        }
        if let Some(w) = &self.watch {
            ci::check_text("watch.remote", &w.remote, 64)?;
            if w.remote.is_empty() || w.branches.is_empty() {
                return Err("watch: a remote and at least one branch".into());
            }
            for b in &w.branches {
                let base = b.strip_suffix("/*").unwrap_or(b);
                if base.is_empty() || base.contains('*') || base.contains("..") || base.starts_with('-') || base.contains(char::is_whitespace) {
                    return Err(format!("watch.branches: {b:?} is not a branch or prefix/*"));
                }
            }
            if !(30..=3600).contains(&w.poll_secs) {
                return Err("watch.poll_secs: 30-3600".into());
            }
        }
        if let Some(s) = &self.statuses {
            if crate::env::env_name_refusal(&s.token_secret) == Some("env_name_invalid") {
                return Err("statuses.token_secret: not a vault name".into());
            }
            if let Some(code) = crate::env::env_name_refusal(&s.env_name) {
                return Err(format!("statuses.env_name: {code}"));
            }
            ci::check_name("statuses.context", &s.context)?;
        }
        for (name, c) in &self.checks {
            ci::check_name("checks", name)?;
            if c.argv.is_empty() {
                return Err(format!("checks.{name}.argv is empty"));
            }
            if let Some(os) = &c.os {
                if os != "linux" && os != "macos" {
                    return Err(format!("checks.{name}.os: linux or macos"));
                }
            }
        }
        if let Some(r) = &self.release {
            ci::check_name("release.project", &r.project)?;
            ci::check_name("release.owner", &r.owner)?;
            ci::check_name("release.lane", &r.lane)?;
            if r.builds.is_empty() {
                return Err("release.builds: at least one".into());
            }
            let mut labels = Vec::new();
            for (name, b) in &r.builds {
                ci::check_name("release.builds", name)?;
                if b.argv.is_empty() || b.artifacts.is_empty() {
                    return Err(format!("release.builds.{name}: argv and artifacts"));
                }
                for e in b.env.iter().chain(&b.secrets) {
                    if let Some(code) = crate::env::env_name_refusal(e) {
                        return Err(format!("release.builds.{name}: {e:?} ({code})"));
                    }
                }
                if !b.independent.is_empty() && b.verify_label.is_none() {
                    return Err(format!("release.builds.{name}: independent goes with verify_label (a dual build)"));
                }
                let mut seen = Vec::new();
                for d in &b.independent {
                    if seen.contains(d) {
                        return Err(format!("release.builds.{name}: independent lists {d:?} twice"));
                    }
                    seen.push(*d);
                }
                labels.push(b.label.as_str());
                labels.extend(b.verify_label.as_deref());
            }
            for l in &labels {
                if !crate::evidence::is_label(l) {
                    return Err(format!("release.builds: {l:?} is not a label (L1, V1, L3)"));
                }
            }
            let n = labels.len();
            labels.sort();
            labels.dedup();
            if labels.len() != n {
                return Err("release.builds: a label is used twice".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let c = CiConfig::parse(
            "version = 1\n[watch]\nremote = \"origin\"\nbranches = [\"main\", \"integ/*\"]\n[statuses]\ntoken_secret = \"K2_CI_STATUSES_TOKEN\"\n[checks.unit]\nargv = [\"bash\", \"scripts/ci/checks.sh\", \"unit\"]\n",
        )
        .unwrap();
        assert_eq!(c.watch.as_ref().unwrap().poll_secs, 60);
        assert_eq!(c.statuses.as_ref().unwrap().env_name, "CI_STATUSES_TOKEN");
        assert_eq!(c.statuses.as_ref().unwrap().context, "k2-ci");
        assert_eq!(CiConfig::parse(&c.to_toml().unwrap()).unwrap(), c);
        CiConfig::parse("version = 1\n").unwrap();
    }

    #[test]
    fn refusals() {
        assert!(CiConfig::parse("version = 2\n").is_err());
        assert!(CiConfig::parse("version = 1\nextra = 1\n").is_err());
        assert!(CiConfig::parse("version = 1\n[statuses]\ntoken_secret = \"T\"\nenv_name = \"K2_CI_STATUSES_TOKEN\"\n").is_err());
        assert!(CiConfig::parse("version = 1\n[watch]\nremote = \"origin\"\nbranches = [\"*\"]\n").is_err());
        let rel = "version = 1\n[release]\nproject = \"k2\"\nowner = \"rosson.k2.dev\"\nlane = \"ci-standard\"\n[release.builds.a]\nargv = [\"x\"]\nneeds = []\nartifacts = [\"dist/*\"]\nlabel = \"L1\"\nverify_label = \"V1\"\n";
        CiConfig::parse(rel).unwrap();
        assert!(CiConfig::parse(&rel.replace("\"V1\"", "\"L1\"")).is_err(), "label twice");
        assert!(CiConfig::parse(&rel.replace("\"V1\"", "\"v1\"")).is_err());
        // C5: independence, default host; node always first.
        let c = CiConfig::parse(rel).unwrap();
        assert_eq!(c.release.as_ref().unwrap().builds["a"].independence(), vec![ci::Dimension::Node, ci::Dimension::Host]);
        let site = format!("{rel}independent = [\"site\", \"host\"]\n");
        let c = CiConfig::parse(&site).unwrap();
        assert_eq!(c.release.as_ref().unwrap().builds["a"].independence(), vec![ci::Dimension::Node, ci::Dimension::Site, ci::Dimension::Host]);
        assert_eq!(CiConfig::parse(&c.to_toml().unwrap()).unwrap(), c);
        assert!(CiConfig::parse(&format!("{rel}independent = [\"host\", \"host\"]\n")).is_err());
        assert!(CiConfig::parse(&format!("{rel}independent = [\"rack\"]\n")).is_err());
        let single = rel.replace("verify_label = \"V1\"\n", "independent = [\"host\"]\n");
        assert!(CiConfig::parse(&single).is_err(), "independent without a verify build");
    }
}

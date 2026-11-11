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
//!
//! **Version 2** (prd-compute-blacksmith-class-v1 §6, BK-9) adds
//! [`V2_KEYS`]; they are allowed only in a `version = 2` file. A version 1
//! file parses exactly as before, and a new key in one is refused by name.
//! There is no key for trusted branches: those are server state, never
//! the repo's word (§4.1.5), so a PR can't name its own branch trusted.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ci;

/// The `version` a new file gets (`k2 ci init`): version 1 files parse on
/// every daemon since 0.46.10.
pub const CI_CONFIG_VERSION: u32 = 1;
/// Version 2: the new keys ([`V2_KEYS`]) are allowed only here. An older
/// daemon refuses a version 2 file loudly (its strict parse names the first
/// unknown key, or says "version 2 (this build knows 1)").
pub const CI_CONFIG_VERSION_2: u32 = 2;
/// The newest version this build knows.
pub const CI_CONFIG_VERSION_MAX: u32 = CI_CONFIG_VERSION_2;
/// Where it lives in a repo.
pub const CI_CONFIG_PATH: &str = ".k2/ci.toml";
/// The statuses token's job env name (prd-ci-release stage-1 vs-live A12:
/// `K2_` is a denied env prefix, so not `K2_CI_STATUSES_TOKEN`).
pub const DEFAULT_STATUSES_ENV: &str = "CI_STATUSES_TOKEN";
/// The trusted branch when `[watch]` names none without a glob (§4.1.5).
pub const DEFAULT_TRUSTED_BRANCH: &str = "main";
/// A `[[quarantine]]` entry's `until` is at most this many days ahead.
pub const QUARANTINE_MAX_DAYS: i64 = 30;
/// The keys version 2 adds (for the docs and the refusal).
pub const V2_KEYS: &[&str] = &[
    "setup",
    "quarantine",
    "checks.*.mem_auto",
    "checks.*.cpus_burst",
    "checks.*.setup",
    "checks.*.tests_failed_exit",
    "statuses.pr_comment",
    "statuses.pr_comment_token_secret",
    "release.builds.*.warm",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiConfig {
    /// [`CI_CONFIG_VERSION`] or [`CI_CONFIG_VERSION_2`].
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
    /// Version 2: setup steps by name (§4.7), run once per node per key and
    /// reused by every later job that names them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub setup: BTreeMap<String, SetupConfig>,
    /// Version 2: tests that don't fail their check until a date (§4.3.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quarantine: Vec<Quarantine>,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Version 2 (§4.3.5): keep one summary comment on the PR of a watched
    /// branch (off unless `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_comment: Option<bool>,
    /// Version 2: the vault secret holding a token with "Pull requests:
    /// write" (never the statuses token, which stays statuses-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_comment_token_secret: Option<String>,
}

fn default_statuses_env() -> String {
    DEFAULT_STATUSES_ENV.to_string()
}

fn default_context() -> String {
    "k2-ci".to_string()
}

/// One check job (`--tier ci`, `--supersede`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Version 2 (§4.4.3): size `--mem` from this check's history;
    /// `mem_gb` is the fallback when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_auto: Option<bool>,
    /// Version 2 (§4.4.4): use up to this many cores when they are idle
    /// (above `cpus`, the guarantee).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus_burst: Option<u32>,
    /// Version 2 (§4.7): `[setup.*]` names to run or reuse first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<String>,
    /// Version 2 (§4.3.4): the runner's "tests failed" exit code (nextest:
    /// 100). Without it a quarantined failure can't turn a non-zero exit
    /// green, except nextest's 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_failed_exit: Option<i32>,
}

/// Version 2: one `[setup.<name>]` step (§4.7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConfig {
    pub argv: Vec<String>,
    /// Files whose contents key the snapshot (repo-relative).
    pub inputs: Vec<String>,
    /// What to keep: repo-relative, or `$CARGO_HOME/…`.
    pub paths: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

/// Version 2: one `[[quarantine]]` entry (§4.3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quarantine {
    /// A `[checks.<name>]`.
    pub check: String,
    /// `classname::name` as `k2 ci tests` shows it.
    pub test: String,
    pub why: String,
    /// `YYYY-MM-DD`, at most [`QUARANTINE_MAX_DAYS`] ahead; after it the
    /// test counts again.
    pub until: String,
}

/// A quarantine entry on a given day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineState {
    Active,
    /// `until` has passed: the test counts again (with a note).
    Expired,
    /// `until` is more than [`QUARANTINE_MAX_DAYS`] ahead: refused
    /// (`ci_quarantine_invalid`).
    TooFar,
}

impl Quarantine {
    /// The entry's state on `today` (`YYYY-MM-DD`). The day `until` names
    /// is still quarantined.
    pub fn state(&self, today: &str) -> Result<QuarantineState, String> {
        let until = civil_days(&self.until).ok_or_else(|| format!("quarantine.until: {:?} is not YYYY-MM-DD", self.until))?;
        let now = civil_days(today).ok_or_else(|| format!("today: {today:?} is not YYYY-MM-DD"))?;
        Ok(if until < now {
            QuarantineState::Expired
        } else if until - now > QUARANTINE_MAX_DAYS {
            QuarantineState::TooFar
        } else {
            QuarantineState::Active
        })
    }
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date (proleptic Gregorian), or
/// `None` when it isn't a real date.
pub fn civil_days(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &s[r];
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok()).flatten()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m) || d < 1 || d > dim[(m - 1) as usize] {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Version 2 (§4.2.2, BK-7): keep this build's release cache on its
    /// node between releases. Only one of the two opt-ins: the node owner's
    /// lane must say `release_cache = true` too, and only the signer's
    /// `warm = "signed-chain"` accepts a warm build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<bool>,
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

    /// The first version-2 key this file uses (for the version 1 refusal).
    pub fn first_v2_key(&self) -> Option<String> {
        if !self.setup.is_empty() {
            return Some("setup".into());
        }
        if !self.quarantine.is_empty() {
            return Some("quarantine".into());
        }
        for (name, c) in &self.checks {
            for (set, key) in [
                (c.mem_auto.is_some(), "mem_auto"),
                (c.cpus_burst.is_some(), "cpus_burst"),
                (!c.setup.is_empty(), "setup"),
                (c.tests_failed_exit.is_some(), "tests_failed_exit"),
            ] {
                if set {
                    return Some(format!("checks.{name}.{key}"));
                }
            }
        }
        if let Some(s) = &self.statuses {
            if s.pr_comment.is_some() {
                return Some("statuses.pr_comment".into());
            }
            if s.pr_comment_token_secret.is_some() {
                return Some("statuses.pr_comment_token_secret".into());
            }
        }
        if let Some(r) = &self.release {
            if let Some((name, _)) = r.builds.iter().find(|(_, b)| b.warm.is_some()) {
                return Some(format!("release.builds.{name}.warm"));
            }
        }
        None
    }

    /// The trusted branch when the server has no list of its own (§4.1.5,
    /// BK-9): the first `[watch]` branch without a glob, else
    /// [`DEFAULT_TRUSTED_BRANCH`]. Never more than one: widening is a
    /// person's step on the server.
    pub fn default_trusted_branch(&self) -> String {
        self.watch
            .as_ref()
            .and_then(|w| w.branches.iter().find(|b| !b.contains('*')).cloned())
            .unwrap_or_else(|| DEFAULT_TRUSTED_BRANCH.to_string())
    }

    pub fn check(&self) -> Result<(), String> {
        if !(CI_CONFIG_VERSION..=CI_CONFIG_VERSION_MAX).contains(&self.version) {
            return Err(format!("{CI_CONFIG_PATH}: version {} (this build knows 1 and 2)", self.version));
        }
        if self.version < CI_CONFIG_VERSION_2 {
            if let Some(k) = self.first_v2_key() {
                return Err(format!("{CI_CONFIG_PATH}: {k} needs version = 2 (this file says version = {})", self.version));
            }
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
            match (s.pr_comment, &s.pr_comment_token_secret) {
                (Some(true), None) => return Err("statuses.pr_comment: needs pr_comment_token_secret (a token with Pull requests: write)".into()),
                (Some(true), Some(t)) => {
                    if crate::env::env_name_refusal(t) == Some("env_name_invalid") {
                        return Err("statuses.pr_comment_token_secret: not a vault name".into());
                    }
                    if *t == s.token_secret {
                        return Err("statuses.pr_comment_token_secret: its own token, not the statuses token (that one stays statuses-only)".into());
                    }
                    if self.watch.is_none() {
                        return Err("statuses.pr_comment: covers watched branches only; add [watch]".into());
                    }
                }
                (_, Some(_)) => return Err("statuses.pr_comment_token_secret: only with pr_comment = true".into()),
                (_, None) => {}
            }
        }
        for (name, s) in &self.setup {
            crate::gens::check_setup_name(name)?;
            if s.argv.is_empty() {
                return Err(format!("setup.{name}.argv is empty"));
            }
            if s.inputs.is_empty() {
                return Err(format!("setup.{name}.inputs: name the files that key it (a lockfile)"));
            }
            for p in &s.inputs {
                if p.starts_with("$CARGO_HOME") {
                    return Err(format!("setup.{name}.inputs: {p:?} is not a repo file"));
                }
                crate::gens::check_setup_path(name, p).map_err(|e| e.replace(".paths:", ".inputs:"))?;
            }
            if s.paths.is_empty() {
                return Err(format!("setup.{name}.paths: name what to keep"));
            }
            for p in &s.paths {
                crate::gens::check_setup_path(name, p)?;
            }
        }
        if self.setup.len() > crate::gens::MAX_SETUP_STEPS * 4 {
            return Err(format!("setup: at most {} steps", crate::gens::MAX_SETUP_STEPS * 4));
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
            if let Some(b) = c.cpus_burst {
                match c.cpus {
                    Some(g) if b > g => {}
                    Some(_) => return Err(format!("checks.{name}.cpus_burst: above cpus (the guarantee)")),
                    None => return Err(format!("checks.{name}.cpus_burst: needs cpus (the guarantee)")),
                }
            }
            if let Some(e) = c.tests_failed_exit {
                if !(1..=255).contains(&e) {
                    return Err(format!("checks.{name}.tests_failed_exit: 1-255"));
                }
            }
            if c.setup.len() > crate::gens::MAX_SETUP_STEPS {
                return Err(format!("checks.{name}.setup: at most {}", crate::gens::MAX_SETUP_STEPS));
            }
            for s in &c.setup {
                if !self.setup.contains_key(s) {
                    return Err(format!(
                        "{}: checks.{name}.setup names {s:?}, which this file has no [setup.{s}] for",
                        crate::gens::codes::SETUP_NOT_FOUND
                    ));
                }
            }
        }
        for (i, q) in self.quarantine.iter().enumerate() {
            let bad = |why: String| Err(format!("{}: quarantine[{i}]: {why}", crate::gens::codes::CI_QUARANTINE_INVALID));
            if !self.checks.contains_key(&q.check) {
                return bad(format!("check {:?} is not a [checks.*]", q.check));
            }
            if q.test.trim().is_empty() || ci::check_text("test", &q.test, 500).is_err() {
                return bad("test: 1-500 bytes, no control characters".into());
            }
            if q.why.trim().is_empty() || ci::check_text("why", &q.why, 300).is_err() {
                return bad("why: say why (1-300 bytes)".into());
            }
            if civil_days(&q.until).is_none() {
                return bad(format!("until: {:?} is not YYYY-MM-DD", q.until));
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
        // A version 1 file writes back without any version 2 key.
        let text = c.to_toml().unwrap();
        for k in ["mem_auto", "cpus_burst", "setup", "quarantine", "tests_failed_exit", "pr_comment", "warm"] {
            assert!(!text.contains(k), "{k}: {text}");
        }
    }

    #[test]
    fn refusals() {
        assert!(CiConfig::parse("version = 3\n").unwrap_err().contains("knows 1 and 2"));
        assert!(CiConfig::parse("version = 0\n").is_err());
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

    const V2: &str = r#"version = 2

[watch]
remote = "origin"
branches = ["integ/*", "main", "pr/*"]

[statuses]
token_secret = "K2_CI_STATUSES_TOKEN"
pr_comment = true
pr_comment_token_secret = "K2_CI_PR_TOKEN"

[setup.node]
argv = ["bun", "install", "--frozen-lockfile"]
inputs = ["bun.lock", "package.json"]
paths = ["node_modules"]
needs = ["node@22", "bun"]

[setup.cargo]
argv = ["cargo", "fetch", "--locked"]
inputs = ["Cargo.lock"]
paths = ["$CARGO_HOME/registry", "$CARGO_HOME/git"]

[checks.test]
argv = ["cargo", "nextest", "run"]
artifacts = ["target/nextest/**/junit.xml"]
cpus = 4
cpus_burst = 8
mem_gb = 8
mem_auto = true
setup = ["cargo"]
tests_failed_exit = 100

[checks.vitest]
argv = ["npx", "vitest", "run"]
setup = ["node"]

[[quarantine]]
check = "test"
test = "sync::tests::reconnect_after_blip"
why = "flaky on loaded nodes"
until = "2026-12-01"

[release]
project = "k2"
owner = "rosson.k2.dev"
lane = "ci-standard"

[release.builds.linux-x86_64]
argv = ["x"]
needs = []
artifacts = ["dist/*"]
label = "L1"
verify_label = "V1"
warm = true
"#;

    /// BK-9: every new key parses under version 2 and round-trips.
    #[test]
    fn version_2_keys_parse_and_round_trip() {
        let c = CiConfig::parse(V2).unwrap();
        assert_eq!(c.version, CI_CONFIG_VERSION_2);
        assert_eq!(c.checks["test"].cpus_burst, Some(8));
        assert_eq!(c.checks["test"].mem_auto, Some(true));
        assert_eq!(c.checks["vitest"].setup, vec!["node".to_string()]);
        assert_eq!(c.setup["cargo"].paths, vec!["$CARGO_HOME/registry".to_string(), "$CARGO_HOME/git".to_string()]);
        assert_eq!(c.quarantine.len(), 1);
        assert_eq!(c.statuses.as_ref().unwrap().pr_comment, Some(true));
        assert_eq!(c.release.as_ref().unwrap().builds["linux-x86_64"].warm, Some(true));
        assert_eq!(CiConfig::parse(&c.to_toml().unwrap()).unwrap(), c);
        // §4.1.5: the first non-glob [watch] branch, else main.
        assert_eq!(c.default_trusted_branch(), "main");
        assert_eq!(CiConfig::parse("version = 2\n").unwrap().default_trusted_branch(), "main");
        let w = CiConfig::parse("version = 1\n[watch]\nremote = \"origin\"\nbranches = [\"rel/*\", \"trunk\"]\n").unwrap();
        assert_eq!(w.default_trusted_branch(), "trunk");
    }

    /// BK-9: a new key in a version 1 file is refused by name; one that
    /// doesn't exist (a trusted-branch list) is refused in any version.
    #[test]
    fn version_1_refuses_each_new_key_by_name() {
        let v1 = V2.replacen("version = 2", "version = 1", 1);
        let e = CiConfig::parse(&v1).unwrap_err();
        assert!(e.contains("needs version = 2"), "{e}");
        for (snippet, key) in [
            ("[checks.t]\nargv = [\"x\"]\nmem_auto = true\n", "checks.t.mem_auto"),
            ("[checks.t]\nargv = [\"x\"]\ncpus = 2\ncpus_burst = 4\n", "checks.t.cpus_burst"),
            ("[checks.t]\nargv = [\"x\"]\ntests_failed_exit = 100\n", "checks.t.tests_failed_exit"),
            ("[setup.n]\nargv = [\"x\"]\ninputs = [\"a\"]\npaths = [\"b\"]\n", "setup"),
            ("[statuses]\ntoken_secret = \"K2_CI_STATUSES_TOKEN\"\npr_comment = false\n", "statuses.pr_comment"),
        ] {
            let e = CiConfig::parse(&format!("version = 1\n{snippet}")).unwrap_err();
            assert!(e.contains(key) && e.contains("version = 2"), "{key}: {e}");
            CiConfig::parse(&format!("version = 2\n{snippet}")).unwrap();
        }
        // There is no key that names a trusted branch (server state only,
        // §4.1.5): a PR's ci.toml can't add one.
        for v in [1, 2] {
            let e = CiConfig::parse(&format!("version = {v}\ntrusted_branches = [\"my-pr\"]\n")).unwrap_err();
            assert!(e.contains("unknown field"), "{e}");
            assert!(CiConfig::parse(&format!("version = {v}\n[watch]\nremote = \"origin\"\nbranches = [\"main\"]\ntrusted = true\n")).is_err());
        }
        // A bare `*` stays refused under version 2 (BK-9).
        assert!(CiConfig::parse("version = 2\n[watch]\nremote = \"origin\"\nbranches = [\"*\"]\n").is_err());
    }

    #[test]
    fn version_2_rules() {
        let bad = |from: &str, to: &str| CiConfig::parse(&V2.replacen(from, to, 1)).unwrap_err();
        assert!(bad("setup = [\"node\"]", "setup = [\"nope\"]").contains("setup_not_found"));
        assert!(bad("cpus_burst = 8", "cpus_burst = 4").contains("above cpus"));
        assert!(bad("cpus = 4\n", "").contains("needs cpus"));
        assert!(bad("tests_failed_exit = 100", "tests_failed_exit = 0").contains("1-255"));
        assert!(bad("check = \"test\"", "check = \"nope\"").contains("ci_quarantine_invalid"));
        assert!(bad("until = \"2026-12-01\"", "until = \"2026-02-30\"").contains("ci_quarantine_invalid"));
        assert!(bad("why = \"flaky on loaded nodes\"", "why = \" \"").contains("ci_quarantine_invalid"));
        assert!(bad("paths = [\"node_modules\"]", "paths = [\"../x\"]").contains("repo-relative"));
        assert!(bad("inputs = [\"Cargo.lock\"]", "inputs = [\"$CARGO_HOME/x\"]").contains("not a repo file"));
        assert!(bad("inputs = [\"Cargo.lock\"]", "inputs = []").contains("inputs"));
        assert!(bad("pr_comment_token_secret = \"K2_CI_PR_TOKEN\"", "pr_comment_token_secret = \"K2_CI_STATUSES_TOKEN\"").contains("statuses-only"));
        assert!(bad("pr_comment_token_secret = \"K2_CI_PR_TOKEN\"\n", "").contains("needs pr_comment_token_secret"));
        assert!(bad("pr_comment = true", "pr_comment = false").contains("only with pr_comment = true"));
    }

    #[test]
    fn quarantine_dates() {
        let q = Quarantine { check: "t".into(), test: "a::b".into(), why: "w".into(), until: "2026-12-01".into() };
        assert_eq!(q.state("2026-11-11").unwrap(), QuarantineState::Active);
        assert_eq!(q.state("2026-12-01").unwrap(), QuarantineState::Active, "the until day still counts");
        assert_eq!(q.state("2026-12-02").unwrap(), QuarantineState::Expired);
        assert_eq!(q.state("2026-11-01").unwrap(), QuarantineState::Active, "30 days ahead");
        assert_eq!(q.state("2026-10-31").unwrap(), QuarantineState::TooFar, "31 days ahead");
        assert!(q.state("11/11/2026").is_err());
        assert_eq!(civil_days("1970-01-01"), Some(0));
        assert_eq!(civil_days("2000-03-01"), Some(11_017));
        assert_eq!(civil_days("2024-02-29").map(|d| d - civil_days("2024-02-28").unwrap()), Some(1));
        assert_eq!(civil_days("2023-02-29"), None);
        assert_eq!(civil_days("2026-13-01"), None);
        assert_eq!(civil_days("2026-1-01"), None);
    }
}

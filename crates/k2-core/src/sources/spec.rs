//! Source definitions (contract §3.2). `validate()` is B1's; C0 ships the
//! types, the canonical JSON and the hash approvals bind to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Json = serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    HttpJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Viewers {
    Member,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentAccess {
    Values,
    Shape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Keep {
    Values,
    States,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnTargetError {
    Crit,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    Proposed,
    Active,
    Paused,
    Removed,
}

/// `sources.pause_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    PausedByPerson,
    SecretMissing,
    SecretDisabled,
    VaultUnavailable,
}

/// `sources.simulate_state` (`POST /cli/sources/simulate`; `off` clears it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SimulateState {
    Ok,
    Warn,
    Crit,
    Stale,
    Unknown,
    Slow,
}

/// Where a source fetches. Exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FetchTargets {
    Url(String),
    /// 1..=50.
    Targets(Vec<Target>),
    /// `origin` is pinned at propose/approve time.
    UrlSecret { name: String, origin: String },
}

/// One fan-out target. `key`: 1..=200 chars, unique in the source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub key: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extract {
    /// name -> JSON Pointer.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub items: Option<ItemsSpec>,
    #[serde(default)]
    pub aggregates: BTreeMap<String, Aggregate>,
    /// Field/aggregate names, <= 8.
    #[serde(default)]
    pub trend: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemsSpec {
    /// None with fan-out targets (each target body is one item).
    pub pointer: Option<String>,
    /// Pointer relative to the item; None = index or object key.
    pub item_key: Option<String>,
    #[serde(default)]
    pub item_fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Aggregate {
    Count,
    CountWhere { field: String, cond: Cond },
    Sum { field: String },
    Min { field: String },
    Max { field: String },
    /// Number of distinct values; the values list is a field.
    Distinct { field: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// An item field, or a source field/aggregate (never both).
    pub field: String,
    pub crit: Option<Cond>,
    pub warn: Option<Cond>,
    pub ok: Option<Cond>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cond {
    Eq(Json),
    Ne(Json),
    In(Vec<Json>),
    Lt(f64),
    Lte(f64),
    Gt(f64),
    Gte(f64),
    Exists(bool),
}

fn kind_default() -> SourceKind {
    SourceKind::HttpJson
}
fn timeout_default() -> u32 {
    super::DEFAULT_TIMEOUT_SECS
}
fn max_bytes_default() -> u32 {
    super::DEFAULT_MAX_BYTES
}
fn fail_after_default() -> u8 {
    3
}
fn ote_default() -> OnTargetError {
    OnTargetError::Crit
}
fn keep_default() -> Keep {
    Keep::Values
}

/// The full definition. `spec_json` in SQL is `{fetch, headers, extract,
/// rules}`; the rest are columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSpec {
    pub name: String,
    pub label: String,
    #[serde(default = "kind_default")]
    pub kind: SourceKind,
    pub preset: Option<String>,
    pub fetch: FetchTargets,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub secret: Option<String>,
    pub every_secs: u32,
    /// 10; fan-out 5.
    #[serde(default = "timeout_default")]
    pub timeout_secs: u32,
    /// 1 MiB.
    #[serde(default = "max_bytes_default")]
    pub max_bytes: u32,
    #[serde(default)]
    pub extract: Extract,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// 3.
    #[serde(default = "fail_after_default")]
    pub fail_after: u8,
    #[serde(default = "ote_default")]
    pub on_target_error: OnTargetError,
    pub link: Option<String>,
    /// None = default by secret (values / shape).
    pub agent_access: Option<AgentAccess>,
    /// None = default by secret (member / admin).
    pub viewers: Option<Viewers>,
    #[serde(default = "keep_default")]
    pub keep: Keep,
}

/// Sorted keys at every level, so the bytes don't depend on serde_json's
/// map feature flags.
fn canonical(v: &Json, out: &mut String) {
    match v {
        Json::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Json::String((*k).clone()).to_string());
                out.push(':');
                canonical(&m[k.as_str()], out);
            }
            out.push('}');
        }
        Json::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

impl SourceSpec {
    /// All checks in PRD §5.1 + GV50 + limits. `caller_is_owner` gates
    /// LAN/http [GV54].
    ///
    /// C0 skeleton: refuses every spec with `not_built` until B1 lands the
    /// checks, so nothing unchecked is ever stored.
    pub fn validate(&self, caller_is_owner: bool) -> Result<Validated, SpecError> {
        let _ = caller_is_owner;
        Err(SpecError {
            code: "not_built",
            field: String::new(),
            hint: "source checks are not built yet".into(),
        })
    }

    /// Canonical JSON (sorted keys, defaults filled) of everything except
    /// name/label.
    pub fn canonical_json(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Json::Null);
        if let Json::Object(m) = &mut v {
            m.remove("name");
            m.remove("label");
        }
        let mut out = String::new();
        canonical(&v, &mut out);
        out
    }

    /// sha256 hex of `canonical_json()`. Approvals bind to it.
    pub fn hash(&self) -> String {
        let digest = Sha256::digest(self.canonical_json().as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// What `validate()` returns: the filled spec plus derived facts.
#[derive(Debug, Clone)]
pub struct Validated {
    /// Defaults filled (agent_access, viewers, timeouts).
    pub spec: SourceSpec,
    /// Sorted unique `scheme://host:port`.
    pub origins: Vec<String>,
    pub lan: bool,
    /// Path token (GV50); the root key hint is added later by the daemon.
    pub warnings: Vec<SpecWarning>,
}

/// `code`: `bad_name`, `bad_label`, `bad_url`, `https_only`, `userinfo`,
/// `secret_in_query`, `secret_in_header`, `forbidden_header`,
/// `lan_owner_only`, `too_many_targets`, `duplicate_target_key`,
/// `mixed_origins_with_secret`, `bad_pointer`, `ambiguous_field`,
/// `unknown_field`, `too_many_trends`, `every_too_short`, `bad_timeout`,
/// `bad_max_bytes`, `bad_fail_after`, `bad_link`, `bad_preset`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecError {
    pub code: &'static str,
    pub field: String,
    pub hint: String,
}

/// `code`: `token_in_path`, `root_key` (daemon-added).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecWarning {
    pub code: &'static str,
    pub field: String,
    pub hint: String,
}

/// Who made or changed something. Stored as `made_by`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Maker {
    Owner,
    Login(String),
    Agent { project_id: String },
}

impl Maker {
    /// `owner` | `login:<user>` | `agent:<projectId>`.
    pub fn stamp(&self) -> String {
        match self {
            Maker::Owner => "owner".into(),
            Maker::Login(u) => format!("login:{u}"),
            Maker::Agent { project_id } => format!("agent:{project_id}"),
        }
    }

    /// `made_by_project`: set iff the maker is an agent.
    pub fn project(&self) -> Option<&str> {
        match self {
            Maker::Agent { project_id } => Some(project_id),
            _ => None,
        }
    }

    pub fn parse(stamp: &str) -> Option<Maker> {
        if stamp == "owner" {
            return Some(Maker::Owner);
        }
        if let Some(u) = stamp.strip_prefix("login:") {
            return (!u.is_empty()).then(|| Maker::Login(u.to_string()));
        }
        stamp
            .strip_prefix("agent:")
            .filter(|p| !p.is_empty())
            .map(|p| Maker::Agent { project_id: p.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SourceSpec {
        serde_json::from_value(serde_json::json!({
            "name": "deploys",
            "label": "Vercel deploys",
            "preset": "vercel-deployments",
            "fetch": {"url": "https://api.vercel.com/v6/deployments?limit=5"},
            "secret": "VERCEL_RO",
            "everySecs": 180,
            "extract": {"items": {"pointer": "/deployments", "itemKey": "/uid", "itemFields": {"state": "/state"}}},
            "rules": [{"field": "state", "crit": {"eq": "ERROR"}, "warn": {"in": ["BUILDING"]}, "ok": {"eq": "READY"}}],
            "link": null, "agentAccess": "shape", "viewers": "admin"
        }))
        .expect("the export shape parses")
    }

    #[test]
    fn export_shape_round_trips_with_defaults_filled() {
        let s = sample();
        assert_eq!(s.timeout_secs, 10);
        assert_eq!(s.max_bytes, 1 << 20);
        assert_eq!(s.fail_after, 3);
        assert_eq!(s.keep, Keep::Values);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["fetch"], serde_json::json!({"url": "https://api.vercel.com/v6/deployments?limit=5"}));
        assert_eq!(v["kind"], "http-json");
        assert_eq!(v["onTargetError"], "crit");
        let secret: FetchTargets =
            serde_json::from_value(serde_json::json!({"urlSecret": {"name": "GATUS", "origin": "https://s.example:443"}})).unwrap();
        assert_eq!(secret, FetchTargets::UrlSecret { name: "GATUS".into(), origin: "https://s.example:443".into() });
    }

    #[test]
    fn hash_ignores_name_and_label_and_tracks_everything_else() {
        let a = sample();
        let mut b = a.clone();
        b.name = "other".into();
        b.label = "Other".into();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.hash().len(), 64);
        let mut c = a.clone();
        c.every_secs = 60;
        assert_ne!(a.hash(), c.hash());
        assert!(!a.canonical_json().contains("\"name\""));
    }

    #[test]
    fn maker_stamps_round_trip() {
        for m in [Maker::Owner, Maker::Login("alice".into()), Maker::Agent { project_id: "p1".into() }] {
            assert_eq!(Maker::parse(&m.stamp()), Some(m.clone()));
        }
        assert_eq!(Maker::parse("agent:"), None);
        assert_eq!(Maker::parse("shell:p1"), None);
    }
}

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
#[serde(deny_unknown_fields)]
pub struct Target {
    pub key: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// Field, item-field and aggregate names: `[A-Za-z0-9_]{1,40}`.
fn check_field_name(n: &str, field: &str) -> Result<(), SpecError> {
    if n.is_empty() || n.len() > 40 || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(SpecError {
            code: "unknown_field",
            field: field.into(),
            hint: format!("'{n}' isn't a field name (letters, digits and _, up to 40)"),
        });
    }
    Ok(())
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
        use super::lint;
        let e = |code: &'static str, field: &str, hint: &str| SpecError { code, field: field.into(), hint: hint.into() };
        if !super::valid_name(&self.name) {
            return Err(e("bad_name", "name", "a source name is 1-40 characters: a-z, 0-9 and -"));
        }
        let label = self.label.trim();
        if label.is_empty() || label.chars().count() > 80 || label.chars().any(char::is_control) {
            return Err(e("bad_label", "label", "a label is 1-80 characters"));
        }
        if let Some(p) = &self.preset {
            if p.len() > 40 || !super::presets::is_known(p) {
                return Err(e("bad_preset", "preset", "unknown preset (k2 study data-sources lists them)"));
            }
        }
        let mut warnings = Vec::new();
        let mut origins: Vec<String> = Vec::new();
        let mut lan = false;
        let mut note = |p: &lint::ParsedUrl, field: &str, warnings: &mut Vec<SpecWarning>| {
            lan |= p.ip.is_some();
            if !origins.contains(&p.origin) {
                origins.push(p.origin.clone());
            }
            if let Some(w) = lint::path_token_warning(&p.url, field) {
                warnings.push(w);
            }
        };
        match &self.fetch {
            FetchTargets::Url(u) => {
                let p = lint::origin_of(u, "fetch.url", caller_is_owner)?;
                note(&p, "fetch.url", &mut warnings);
            }
            FetchTargets::Targets(ts) => {
                if ts.is_empty() || ts.len() > super::MAX_TARGETS_PER_SOURCE {
                    return Err(e("too_many_targets", "fetch.targets", "a source has 1-50 targets"));
                }
                let mut keys = std::collections::BTreeSet::new();
                for (i, t) in ts.iter().enumerate() {
                    let kf = format!("fetch.targets[{i}].key");
                    if t.key.is_empty() || t.key.chars().count() > 200 || t.key.chars().any(char::is_control) {
                        return Err(e("bad_url", &kf, "a target key is 1-200 characters"));
                    }
                    if !keys.insert(t.key.as_str()) {
                        return Err(e("duplicate_target_key", &kf, "target keys are unique within a source"));
                    }
                    let uf = format!("fetch.targets[{i}].url");
                    let p = lint::origin_of(&t.url, &uf, caller_is_owner)?;
                    note(&p, &uf, &mut warnings);
                }
            }
            FetchTargets::UrlSecret { name, origin } => {
                if name.is_empty() || name.len() > 128 {
                    return Err(e("bad_url", "fetch.urlSecret.name", "name the vault secret that holds the URL"));
                }
                let p = lint::origin_of(origin, "fetch.urlSecret.origin", caller_is_owner)?;
                if p.url.path() != "/" || p.url.query().is_some() {
                    return Err(e("bad_url", "fetch.urlSecret.origin", "the origin is just scheme://host[:port]"));
                }
                lan |= p.ip.is_some();
                origins.push(p.origin);
            }
        }
        origins.sort();
        let has_secret = self.secret.is_some() || matches!(self.fetch, FetchTargets::UrlSecret { .. });
        if let Some(s) = &self.secret {
            if s.is_empty() || s.len() > 128 {
                return Err(e("bad_url", "secret", "a secret name is 1-128 characters"));
            }
        }
        if has_secret && origins.len() > 1 {
            return Err(e(
                "mixed_origins_with_secret",
                "fetch",
                "a source with a secret fetches from one site only; split it into one source per site",
            ));
        }
        for (i, (n, v)) in self.headers.iter().enumerate() {
            lint::check_header(n, v, &format!("headers[{i}]"))?;
        }
        if self.headers.len() > 20 {
            return Err(e("forbidden_header", "headers", "at most 20 headers"));
        }
        if self.every_secs < super::MIN_EVERY_SECS || self.every_secs > 86_400 {
            return Err(e("every_too_short", "everySecs", "check every 30 s to 1 day"));
        }
        if self.timeout_secs < 1 || self.timeout_secs > super::MAX_TIMEOUT_SECS {
            return Err(e("bad_timeout", "timeoutSecs", "a timeout is 1-30 s"));
        }
        if self.max_bytes < 1024 || self.max_bytes > super::MAX_MAX_BYTES {
            return Err(e("bad_max_bytes", "maxBytes", "maxBytes is 1 KiB to 5 MiB"));
        }
        if !(1..=10).contains(&self.fail_after) {
            return Err(e("bad_fail_after", "failAfter", "failAfter is 1-10"));
        }
        if let Some(l) = &self.link {
            lint::check_link(l)?;
        }

        // Extraction and rules.
        let fanout = matches!(self.fetch, FetchTargets::Targets(_));
        let ex = &self.extract;
        if fanout && !ex.fields.is_empty() {
            return Err(e("unknown_field", "extract.fields", "fan-out sources name item fields (extract.items.itemFields), not fields"));
        }
        for (n, p) in &ex.fields {
            check_field_name(n, "extract.fields")?;
            lint::check_pointer(p, &format!("extract.fields.{n}"))?;
        }
        let mut item_names: Vec<&str> = Vec::new();
        if let Some(items) = &ex.items {
            match (&items.pointer, fanout) {
                (Some(_), true) => {
                    return Err(e("bad_pointer", "extract.items.pointer", "with targets, each target's answer is one item; leave items.pointer out"))
                }
                (None, false) => {
                    return Err(e("bad_pointer", "extract.items.pointer", "name where the item list is (a JSON Pointer, \"\" for the whole answer)"))
                }
                (Some(p), false) => lint::check_pointer(p, "extract.items.pointer")?,
                (None, true) => {}
            }
            if let Some(k) = &items.item_key {
                if fanout {
                    return Err(e("bad_pointer", "extract.items.itemKey", "with targets, the item key is the target key"));
                }
                lint::check_pointer(k, "extract.items.itemKey")?;
            }
            for (n, p) in &items.item_fields {
                check_field_name(n, "extract.items.itemFields")?;
                lint::check_pointer(p, &format!("extract.items.itemFields.{n}"))?;
                item_names.push(n);
            }
        }
        let mut source_names: Vec<&str> = ex.fields.keys().map(String::as_str).collect();
        for (n, agg) in &ex.aggregates {
            check_field_name(n, "extract.aggregates")?;
            if ex.fields.contains_key(n) {
                return Err(e("ambiguous_field", &format!("extract.aggregates.{n}"), "a field and an aggregate share this name"));
            }
            let f = match agg {
                Aggregate::Count => None,
                Aggregate::CountWhere { field, .. }
                | Aggregate::Sum { field }
                | Aggregate::Min { field }
                | Aggregate::Max { field }
                | Aggregate::Distinct { field } => Some(field),
            };
            if let Some(f) = f {
                if !item_names.contains(&f.as_str()) {
                    return Err(e("unknown_field", &format!("extract.aggregates.{n}"), "aggregates run over item fields; name one from itemFields"));
                }
            }
            source_names.push(n);
        }
        for n in &item_names {
            if source_names.contains(n) {
                return Err(e("ambiguous_field", &format!("extract.items.itemFields.{n}"), "an item field and a source field share this name"));
            }
        }
        if ex.trend.len() > super::MAX_TREND_SERIES {
            return Err(e("too_many_trends", "extract.trend", "at most 8 trend series"));
        }
        for t in &ex.trend {
            if !source_names.contains(&t.as_str()) {
                return Err(e("unknown_field", "extract.trend", "trend names a field or an aggregate"));
            }
        }
        for (i, r) in self.rules.iter().enumerate() {
            if !source_names.contains(&r.field.as_str()) && !item_names.contains(&r.field.as_str()) {
                return Err(e("unknown_field", &format!("rules[{i}].field"), "a rule names a field, an item field or an aggregate"));
            }
            if r.crit.is_none() && r.warn.is_none() && r.ok.is_none() {
                return Err(e("unknown_field", &format!("rules[{i}]"), "a rule has at least one of crit, warn, ok"));
            }
        }
        if self.rules.len() > 32 {
            return Err(e("unknown_field", "rules", "at most 32 rules"));
        }

        let mut spec = self.clone();
        spec.label = label.to_string();
        if spec.agent_access.is_none() {
            spec.agent_access = Some(if has_secret { AgentAccess::Shape } else { AgentAccess::Values });
        }
        if spec.viewers.is_none() {
            spec.viewers = Some(if has_secret { Viewers::Admin } else { Viewers::Member });
        }
        if fanout && spec.timeout_secs == super::DEFAULT_TIMEOUT_SECS {
            spec.timeout_secs = super::FANOUT_TIMEOUT_SECS;
        }
        Ok(Validated { spec, origins, lan, warnings })
    }

    /// Fan-out (`targets`) rather than one URL.
    pub fn is_fanout(&self) -> bool {
        matches!(self.fetch, FetchTargets::Targets(_))
    }

    /// The vault secret this source uses: its header secret or its URL
    /// secret.
    pub fn has_secret(&self) -> bool {
        self.secret.is_some() || matches!(self.fetch, FetchTargets::UrlSecret { .. })
    }

    pub fn url_secret_name(&self) -> Option<&str> {
        match &self.fetch {
            FetchTargets::UrlSecret { name, .. } => Some(name),
            _ => None,
        }
    }

    /// The `spec_json` column: `{fetch, headers, extract, rules}`.
    pub fn stored_json(&self) -> String {
        let v = serde_json::json!({
            "fetch": self.fetch,
            "headers": self.headers,
            "extract": self.extract,
            "rules": self.rules,
        });
        let mut out = String::new();
        canonical(&v, &mut out);
        out
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

    fn parse(v: serde_json::Value) -> SourceSpec {
        serde_json::from_value(v).expect("parses")
    }

    fn fleet(n: usize) -> serde_json::Value {
        let targets: Vec<_> = (0..n).map(|i| serde_json::json!({"key": format!("b{i}"), "url": format!("https://b{i}.example.com/boot-status")})).collect();
        serde_json::json!({
            "name": "fleet", "label": "Fleet", "fetch": {"targets": targets}, "everySecs": 60,
            "extract": {"items": {"itemFields": {"version": "/version", "phase": "/phase"}},
                        "aggregates": {"versions": {"op": "distinct", "field": "version"}}, "trend": ["versions"]},
            "rules": [{"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}}]
        })
    }

    #[test]
    fn validate_fills_defaults_and_derives_origins() {
        let v = parse(fleet(3)).validate(false).expect("valid fleet");
        assert_eq!(v.origins.len(), 3);
        assert!(!v.lan);
        assert_eq!(v.spec.timeout_secs, 5, "fan-out default timeout");
        assert_eq!((v.spec.agent_access, v.spec.viewers), (Some(AgentAccess::Values), Some(Viewers::Member)));
        let v = sample().validate(false).expect("valid deploys");
        assert_eq!(v.origins, vec!["https://api.vercel.com:443".to_string()]);
        assert_eq!((v.spec.agent_access, v.spec.viewers), (Some(AgentAccess::Shape), Some(Viewers::Admin)));
        let mut s = sample();
        s.agent_access = None;
        s.viewers = None;
        let v = s.validate(false).unwrap();
        assert_eq!((v.spec.agent_access, v.spec.viewers), (Some(AgentAccess::Shape), Some(Viewers::Admin)), "secret → shape/admin");
    }

    #[test]
    fn validate_refuses_with_the_right_codes() {
        let code = |v: serde_json::Value, owner: bool| parse(v).validate(owner).unwrap_err().code;
        let mut f = fleet(3);
        f["name"] = "Fleet".into();
        assert_eq!(code(f, false), "bad_name");
        let mut f = fleet(3);
        f["label"] = "".into();
        assert_eq!(code(f, false), "bad_label");
        assert_eq!(code(fleet(51), false), "too_many_targets");
        let mut f = fleet(2);
        f["fetch"]["targets"][1]["key"] = "b0".into();
        assert_eq!(code(f, false), "duplicate_target_key");
        let mut f = fleet(20);
        f["fetch"]["targets"][19]["url"] = "http://10.0.0.9/".into();
        assert_eq!(code(f, false), "lan_owner_only", "one LAN target refuses the whole spec");
        let mut f = fleet(2);
        f["secret"] = "K".into();
        assert_eq!(code(f, false), "mixed_origins_with_secret");
        let mut f = fleet(2);
        f["everySecs"] = 10.into();
        assert_eq!(code(f, false), "every_too_short");
        let mut f = fleet(2);
        f["timeoutSecs"] = 31.into();
        assert_eq!(code(f, false), "bad_timeout");
        let mut f = fleet(2);
        f["maxBytes"] = 6_000_000.into();
        assert_eq!(code(f, false), "bad_max_bytes");
        let mut f = fleet(2);
        f["failAfter"] = 0.into();
        assert_eq!(code(f, false), "bad_fail_after");
        let mut f = fleet(2);
        f["link"] = "https://k2.dev/?t=1".into();
        assert_eq!(code(f, false), "bad_link");
        let mut f = fleet(2);
        f["extract"]["trend"] = serde_json::json!(["nope"]);
        assert_eq!(code(f, false), "unknown_field");
        let mut f = fleet(2);
        f["extract"]["trend"] = serde_json::json!(["a","b","c","d","e","f","g","h","i"]);
        assert_eq!(code(f, false), "too_many_trends");
        let mut f = fleet(2);
        f["extract"]["aggregates"]["version"] = serde_json::json!({"op": "count"});
        assert_eq!(code(f, false), "ambiguous_field");
        let mut f = fleet(2);
        f["extract"]["items"]["itemFields"]["phase"] = "phase".into();
        assert_eq!(code(f, false), "bad_pointer");
        let mut f = fleet(2);
        f["headers"] = serde_json::json!([["Authorization", "Bearer x"]]);
        assert_eq!(code(f, false), "secret_in_header");
        let mut f = fleet(2);
        f["headers"] = serde_json::json!([["Host", "x"]]);
        assert_eq!(code(f, false), "forbidden_header");
        let mut f = fleet(2);
        f["preset"] = "nope".into();
        assert_eq!(code(f, false), "bad_preset");
        let one = |u: &str| serde_json::json!({"name": "x", "label": "X", "fetch": {"url": u}, "everySecs": 60});
        assert_eq!(code(one("http://example.com/"), true), "https_only");
        assert_eq!(code(one("https://u:p@example.com/"), true), "userinfo");
        assert_eq!(code(one("https://example.com/?api_key=1"), true), "secret_in_query");
        assert_eq!(code(one("http://169.254.169.254/latest/meta-data/"), false), "lan_owner_only");
        let v = parse(one("http://192.168.1.5:8080/status")).validate(true).expect("owner LAN");
        assert!(v.lan);
        assert_eq!(v.origins, vec!["http://192.168.1.5:8080".to_string()]);
        let w = parse(one("https://hooks.example.com/T000/abcdefghijklmnopqrstuvwx1234")).validate(false).unwrap();
        assert_eq!(w.warnings[0].code, "token_in_path");
        let us = serde_json::json!({"name": "g", "label": "G", "fetch": {"urlSecret": {"name": "GATUS", "origin": "https://status.example.com"}}, "everySecs": 60});
        let v = parse(us).validate(false).unwrap();
        assert_eq!(v.origins, vec!["https://status.example.com:443".to_string()]);
        assert_eq!(v.spec.viewers, Some(Viewers::Admin));
        let unknown: Result<SourceSpec, _> = serde_json::from_value(serde_json::json!({"name": "x", "label": "X", "fetch": {"url": "https://a.example"}, "everySecs": 60, "extra": 1}));
        assert!(unknown.is_err(), "unknown keys are refused");
    }
}

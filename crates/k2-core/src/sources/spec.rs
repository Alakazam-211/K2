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
    /// More item lists next to `items` (0.46.1), by name. Single-URL
    /// sources only; each needs a `pointer`. Their items carry `list` and
    /// the key `<list>/<itemKey>`; item field names are unique across
    /// `items` and every list, so a rule's field says which list it rates.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lists: BTreeMap<String, ItemsSpec>,
    /// name -> JSON Pointer to an array or object; the kept value is its
    /// length (null when the pointer isn't an array or object). 0.46.1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, String>,
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
    /// Items in `items` (or in the named list).
    Count {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        list: Option<String>,
    },
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
    /// Checks in a row that aren't ok before this rule's warn goes hard
    /// (1-20, default 1: at once). 0.46.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_after: Option<u8>,
    /// Checks in a row that aren't ok, the last one crit, before this
    /// rule's crit goes hard (1-20, default `warnAfter`). Until then a crit
    /// reading shows as warn. 0.46.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit_after: Option<u8>,
}

impl Rule {
    pub fn warn_after(&self) -> u32 {
        u32::from(self.warn_after.unwrap_or(1).max(1))
    }
    pub fn crit_after(&self) -> u32 {
        self.crit_after.map_or(self.warn_after(), |n| u32::from(n.max(1)))
    }
}

/// A condition on one value. `eq`/`ne` and the number comparisons also take
/// `{"field": "<name>"}`: another field of the same item, else a source
/// field (0.46.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cond {
    Eq(Json),
    Ne(Json),
    In(Vec<Json>),
    Lt(Operand),
    Lte(Operand),
    Gt(Operand),
    Gte(Operand),
    Exists(bool),
    /// The value is a time (epoch seconds, epoch milliseconds, or an
    /// RFC 3339 string) more than this many seconds ago. 0.46.1.
    AgeGt(f64),
    /// The value is a time less than this many seconds ago. 0.46.1.
    AgeLt(f64),
    /// The value is a version (`0.45.7`, `v1.2`, `0.46.0-rc1`) older than
    /// the target. 0.46.1.
    VersionBehind(VersionRef),
    /// The value is a version at or past the target. 0.46.1.
    VersionAtLeast(VersionRef),
}

/// A number, or `{"field": "<name>"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Operand {
    Num(f64),
    Field(FieldRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRef {
    pub field: String,
}

/// `"0.45.7"`, or `{"field": "target_version"}` / `{"value": "0.45.7"}`
/// with at most one of `major`/`minor`/`patch`: behind by at least that
/// many at that level (any older major or minor counts for `patch`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VersionRef {
    Value(String),
    Spec(VersionSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<u32>,
}

/// A field reference inside `eq`/`ne`: exactly `{"field": "<name>"}`.
pub fn json_field_ref(v: &Json) -> Option<&str> {
    let m = v.as_object()?;
    if m.len() != 1 {
        return None;
    }
    m.get("field")?.as_str()
}

impl Cond {
    /// Field names this condition reads besides its own value.
    pub fn refs(&self) -> Vec<&str> {
        match self {
            Cond::Eq(j) | Cond::Ne(j) => json_field_ref(j).into_iter().collect(),
            Cond::Lt(o) | Cond::Lte(o) | Cond::Gt(o) | Cond::Gte(o) => match o {
                Operand::Field(f) => vec![f.field.as_str()],
                Operand::Num(_) => vec![],
            },
            Cond::VersionBehind(VersionRef::Spec(s)) | Cond::VersionAtLeast(VersionRef::Spec(s)) => {
                s.field.as_deref().into_iter().collect()
            }
            _ => vec![],
        }
    }
}

/// What a source expects back (0.46.1). `status`: the HTTP statuses that
/// count as an answer (200-299, 400-599; up to 10). With `expect`, any
/// listed status is healthy and the body may be anything: JSON is still
/// read when it parses, anything else reads as null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expect {
    pub status: Vec<u16>,
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
    /// Status-only checks (0.46.1); see [`Expect`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
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

/// Shape checks on one condition (references are checked by the caller).
fn check_cond(c: &Cond, field: &str) -> Result<(), SpecError> {
    let e = |hint: &str| SpecError { code: "bad_rule", field: field.into(), hint: hint.into() };
    match c {
        Cond::AgeGt(n) | Cond::AgeLt(n) if !n.is_finite() || *n < 0.0 => Err(e("an age is a number of seconds, 0 or more")),
        Cond::VersionBehind(v) | Cond::VersionAtLeast(v) => {
            let (value, levels) = match v {
                VersionRef::Value(s) => (Some(s.as_str()), 0),
                VersionRef::Spec(s) => {
                    if s.field.is_some() == s.value.is_some() {
                        return Err(e("name exactly one of field or value"));
                    }
                    if let Some(f) = &s.field {
                        check_field_name(f, field)?;
                    }
                    (s.value.as_deref(), [s.major, s.minor, s.patch].iter().filter(|x| x.is_some()).count())
                }
            };
            if levels > 1 {
                return Err(e("use at most one of major, minor, patch"));
            }
            if matches!(c, Cond::VersionAtLeast(_)) && levels > 0 {
                return Err(e("versionAtLeast takes no major/minor/patch distance"));
            }
            if let Some(v) = value {
                if super::rules::parse_version(v).is_none() {
                    return Err(e("that isn't a version like 0.45.7"));
                }
            }
            Ok(())
        }
        _ => {
            for f in c.refs() {
                check_field_name(f, field)?;
            }
            Ok(())
        }
    }
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
        // Item field -> its list (None = `items`).
        let mut item_list: BTreeMap<&str, Option<&str>> = item_names.iter().map(|n| (*n, None)).collect();
        if !ex.lists.is_empty() && fanout {
            return Err(e("bad_pointer", "extract.lists", "lists read one answer; a source with targets has one item per target"));
        }
        if ex.lists.len() > 8 {
            return Err(e("unknown_field", "extract.lists", "at most 8 lists"));
        }
        for (ln, l) in &ex.lists {
            check_field_name(ln, "extract.lists")?;
            let base = format!("extract.lists.{ln}");
            match &l.pointer {
                Some(p) => lint::check_pointer(p, &format!("{base}.pointer"))?,
                None => return Err(e("bad_pointer", &format!("{base}.pointer"), "name where this list is (a JSON Pointer)")),
            }
            if let Some(k) = &l.item_key {
                lint::check_pointer(k, &format!("{base}.itemKey"))?;
            }
            for (n, p) in &l.item_fields {
                check_field_name(n, &format!("{base}.itemFields"))?;
                lint::check_pointer(p, &format!("{base}.itemFields.{n}"))?;
                if item_list.contains_key(n.as_str()) {
                    return Err(e(
                        "ambiguous_field",
                        &format!("{base}.itemFields.{n}"),
                        "item field names are unique across items and every list",
                    ));
                }
                item_list.insert(n, Some(ln));
                item_names.push(n);
            }
        }
        let mut source_names: Vec<&str> = ex.fields.keys().map(String::as_str).collect();
        for (n, p) in &ex.counts {
            check_field_name(n, "extract.counts")?;
            lint::check_pointer(p, &format!("extract.counts.{n}"))?;
            if source_names.contains(&n.as_str()) {
                return Err(e("ambiguous_field", &format!("extract.counts.{n}"), "a field and a count share this name"));
            }
            source_names.push(n);
        }
        for (n, agg) in &ex.aggregates {
            check_field_name(n, "extract.aggregates")?;
            if source_names.contains(&n.as_str()) {
                return Err(e("ambiguous_field", &format!("extract.aggregates.{n}"), "a field and an aggregate share this name"));
            }
            if let Aggregate::Count { list: Some(l) } = agg {
                if !ex.lists.contains_key(l) {
                    return Err(e("unknown_field", &format!("extract.aggregates.{n}.list"), "count names a list from extract.lists"));
                }
            }
            if let Aggregate::CountWhere { field, cond } = agg {
                let own = item_list.get(field.as_str()).copied().flatten();
                for r in cond.refs() {
                    let same_list = item_list.get(r).is_some_and(|l| *l == own);
                    if !same_list && !ex.fields.contains_key(r) && !ex.counts.contains_key(r) {
                        return Err(e(
                            "unknown_field",
                            &format!("extract.aggregates.{n}.cond"),
                            "a field reference names an item field of the same list, a field or a count",
                        ));
                    }
                }
                check_cond(cond, &format!("extract.aggregates.{n}.cond"))?;
            }
            let f = match agg {
                Aggregate::Count { .. } => None,
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
            let own = item_list.get(r.field.as_str()).copied();
            for (which, c) in [("crit", &r.crit), ("warn", &r.warn), ("ok", &r.ok)] {
                let Some(c) = c else { continue };
                let at = format!("rules[{i}].{which}");
                check_cond(c, &at)?;
                for f in c.refs() {
                    let ok = source_names.contains(&f) || (own.is_some() && item_list.get(f).copied() == own);
                    if !ok {
                        return Err(e(
                            "unknown_field",
                            &at,
                            "a field reference names a field, a count, an aggregate, or an item field of the same list",
                        ));
                    }
                }
            }
            for (which, n) in [("warnAfter", r.warn_after), ("critAfter", r.crit_after)] {
                if n.is_some_and(|n| !(1..=20).contains(&n)) {
                    return Err(e("bad_fail_after", &format!("rules[{i}].{which}"), "warnAfter and critAfter are 1-20 checks"));
                }
            }
            if r.crit_after.is_some_and(|c| c < r.warn_after.unwrap_or(1)) {
                return Err(e("bad_fail_after", &format!("rules[{i}].critAfter"), "critAfter is at least warnAfter"));
            }
        }
        if let Some(x) = &self.expect {
            if x.status.is_empty() || x.status.len() > 10 {
                return Err(e("bad_expect", "expect.status", "list 1-10 HTTP statuses that count as an answer"));
            }
            if let Some(bad) = x.status.iter().find(|s| !matches!(s, 200..=299 | 400..=599)) {
                return Err(e("bad_expect", "expect.status", &format!("{bad} can't be expected: use 200-299 or 400-599")));
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

    /// The `spec_json` column: `{fetch, headers, extract, rules}` plus
    /// `expect` when set.
    pub fn stored_json(&self) -> String {
        let mut v = serde_json::json!({
            "fetch": self.fetch,
            "headers": self.headers,
            "extract": self.extract,
            "rules": self.rules,
        });
        if let Some(e) = &self.expect {
            v["expect"] = serde_json::json!(e);
        }
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
/// `bad_max_bytes`, `bad_fail_after`, `bad_link`, `bad_preset`, `bad_rule`,
/// `bad_expect`.
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
    fn legacy_specs_keep_their_hash_and_stored_json() {
        // 0.46.1 keys only appear when used, so approved sources don't need
        // approving again.
        let s = sample();
        for k in ["expect", "lists", "counts", "warnAfter", "critAfter", "\"list\""] {
            assert!(!s.canonical_json().contains(k), "{k} in a legacy canonical spec");
            assert!(!s.stored_json().contains(k), "{k} in a legacy stored spec");
        }
        let v = parse(fleet(2));
        assert!(v.canonical_json().contains("\"aggregates\":{\"versions\":{\"field\":\"version\",\"op\":\"distinct\"}}"));
        let count = parse(serde_json::json!({"name": "x", "label": "X", "fetch": {"url": "https://a.example/"}, "everySecs": 60,
            "extract": {"items": {"pointer": "", "itemKey": null}, "aggregates": {"n": {"op": "count"}}}}));
        assert!(count.canonical_json().contains("\"n\":{\"op\":\"count\"}"), "count stays {{op: count}}");
        let mut e = sample();
        e.expect = Some(Expect { status: vec![200, 401] });
        assert!(e.stored_json().contains("\"expect\":{\"status\":[200,401]}"));
        assert_ne!(e.hash(), s.hash(), "expect is part of what an approval binds to");
    }

    #[test]
    fn new_rule_shapes_validate_and_refuse_with_codes() {
        let base = || serde_json::json!({
            "name": "ops", "label": "Ops", "fetch": {"url": "https://k2.dev/api/ops/fleet"}, "everySecs": 60,
            "extract": {
                "fields": {"target_version": "/target_version", "ts": "/ts"},
                "counts": {"past_due_n": "/past_due"},
                "items": {"pointer": "/boxes", "itemKey": "/name", "itemFields": {"version": "/version"}},
                "lists": {"certs": {"pointer": "/certs", "itemKey": "/host", "itemFields": {"cert_days": "/days"}}},
                "aggregates": {"certs_n": {"op": "count", "list": "certs"},
                               "behind": {"op": "countWhere", "field": "version", "cond": {"versionBehind": {"field": "target_version"}}}}
            },
            "rules": [
                {"field": "ts", "warn": {"ageGt": 300}, "crit": {"ageGt": 900}},
                {"field": "version", "warn": {"versionBehind": {"field": "target_version"}},
                 "crit": {"versionBehind": {"field": "target_version", "minor": 1}}, "ok": {"versionAtLeast": {"field": "target_version"}},
                 "warnAfter": 1, "critAfter": 3},
                {"field": "cert_days", "crit": {"lt": 7}, "warn": {"lt": {"field": "past_due_n"}}},
                {"field": "past_due_n", "warn": {"gt": 0}}
            ],
            "expect": {"status": [200, 401]}
        });
        let v = parse(base()).validate(false).expect("valid 0.46.1 spec");
        assert_eq!(v.spec.expect, Some(Expect { status: vec![200, 401] }));
        let code = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut b = base();
            f(&mut b);
            parse(b).validate(false).unwrap_err()
        };
        assert_eq!(code(&|b| b["rules"][1]["critAfter"] = 21.into()).code, "bad_fail_after");
        assert_eq!(code(&|b| b["rules"][1]["warnAfter"] = 0.into()).code, "bad_fail_after");
        let e = code(&|b| { b["rules"][1]["warnAfter"] = 4.into(); b["rules"][1]["critAfter"] = 2.into(); });
        assert_eq!((e.code, e.field.as_str()), ("bad_fail_after", "rules[1].critAfter"));
        assert_eq!(code(&|b| b["rules"][0]["warn"] = serde_json::json!({"ageGt": -1})).code, "bad_rule");
        assert_eq!(code(&|b| b["rules"][1]["warn"] = serde_json::json!({"versionBehind": "latest"})).code, "bad_rule");
        assert_eq!(code(&|b| b["rules"][1]["warn"] = serde_json::json!({"versionBehind": {"field": "target_version", "minor": 1, "patch": 2}})).code, "bad_rule");
        assert_eq!(code(&|b| b["rules"][1]["warn"] = serde_json::json!({"versionBehind": {"field": "a", "value": "1.0"}})).code, "bad_rule");
        assert_eq!(code(&|b| b["rules"][1]["ok"] = serde_json::json!({"versionAtLeast": {"field": "target_version", "minor": 1}})).code, "bad_rule");
        let e = code(&|b| b["rules"][1]["warn"] = serde_json::json!({"versionBehind": {"field": "nope"}}));
        assert_eq!((e.code, e.field.as_str()), ("unknown_field", "rules[1].warn"));
        // A list's rule can't reference another list's item field.
        assert_eq!(code(&|b| b["rules"][2]["warn"] = serde_json::json!({"lt": {"field": "version"}})).code, "unknown_field");
        // A source rule can't reference an item field.
        assert_eq!(code(&|b| b["rules"][3]["warn"] = serde_json::json!({"gt": {"field": "cert_days"}})).code, "unknown_field");
        assert_eq!(code(&|b| b["extract"]["lists"]["certs"]["itemFields"]["version"] = "/v".into()).code, "ambiguous_field");
        assert_eq!(code(&|b| b["extract"]["lists"]["certs"]["pointer"] = serde_json::Value::Null).code, "bad_pointer");
        assert_eq!(code(&|b| b["extract"]["counts"]["ts"] = "/x".into()).code, "ambiguous_field");
        assert_eq!(code(&|b| b["extract"]["aggregates"]["certs_n"]["list"] = "nope".into()).code, "unknown_field");
        assert_eq!(code(&|b| b["expect"]["status"] = serde_json::json!([302])).code, "bad_expect");
        assert_eq!(code(&|b| b["expect"]["status"] = serde_json::json!([])).code, "bad_expect");
        let mut f = fleet(2);
        f["extract"]["lists"] = serde_json::json!({"x": {"pointer": "/x", "itemFields": {"y": "/y"}}});
        assert_eq!(parse(f).validate(false).unwrap_err().code, "bad_pointer", "lists need a single URL");
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

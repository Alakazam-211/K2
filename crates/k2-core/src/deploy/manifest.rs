//! The App manifest: `manifest.json` at the Seed root (vision v2 §5.3,
//! schema 1, `kind: "app"`). Strict JSON: unknown keys are errors with
//! line and column. `net` and `secrets` are reserved for a later schema
//! and refused by name.
//!
//! Stage 1 runs `host.mode = "cmd"` (a server started from `host.start`
//! argv) on the process engine. `static` and `next` parse and validate;
//! `static` Apps keep `k2 publish run --skin` until the runtime hosts them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as J;

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA: u32 = 1;

/// Keys a later schema owns (vision v2 §5.3): refused by name in schema 1.
pub const RESERVED_KEYS: &[&str] = &["net", "secrets"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// The ceiling (catalog cap names exposed to `app`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reasons: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Requires>,
    pub host: Host,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Guest role templates: role → slot → caps.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// The App's data revision (§7.1). A rollback to a Seed with a lower
    /// revision is refused unless the newer Seed declares `dataDown`.
    #[serde(default)]
    pub data_revision: u32,
    /// This Seed knows how to step its data back to the previous revision.
    #[serde(default)]
    pub data_down: bool,
    /// Top-level-relative paths `k2 app build` leaves out of the Seed
    /// (`.git` and `.k2` are always left out).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// The build argv. It runs inside the wall (stage 3, S16); stage 1
    /// records it and snapshots the folder as the agent built it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<String>,
    /// Declared non-secret variables. Secrets come from the vault.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Outbound hosts the App asks for (S15). A request only: a deploy
    /// files the new ones as a held widening and the owner approves them
    /// (§5.6). A guarded App reaches nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress: Option<EgressRequest>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressRequest {
    /// `host[:port]` (443 by default) or `*.<suffix>[:port]`.
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requires {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Host {
    /// `static` | `cmd` | `next`.
    pub mode: String,
    /// `cmd`/`next`: the server argv, run in the Seed with `$PORT` set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<Health>,
    /// Drain for spliced connections when this Seed is replaced (§7.2;
    /// default 30 s, games longer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drain_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Health {
    /// `GET <path>` must answer 2xx (default `/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The whole gate, overriding the 120 s default (max 900).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Litmus L4: probe as a logged-in test guest (at deploy time and
    /// periodically). The hook is in stage 1; test guests arrive in stage
    /// 2 (S25), so a deploy that asks for it fails `test_guest_unavailable`
    /// until then.
    #[serde(default)]
    pub as_test_guest: bool,
}

impl Manifest {
    pub fn health_path(&self) -> String {
        self.host
            .health
            .as_ref()
            .and_then(|h| h.path.clone())
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "/".to_string())
    }

    pub fn wants_test_guest_probe(&self) -> bool {
        self.host.health.as_ref().is_some_and(|h| h.as_test_guest)
    }
}

/// A parse failure with its place in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

/// Parse `manifest.json` text. Reserved keys get a teaching message; every
/// other unknown key is a serde error with line and column.
pub fn parse(text: &str) -> Result<Manifest, ParseError> {
    let v: J = serde_json::from_str(text).map_err(|e| ParseError {
        message: e.to_string(),
        line: e.line(),
        column: e.column(),
    })?;
    if let Some(obj) = v.as_object() {
        for k in RESERVED_KEYS {
            if obj.contains_key(*k) {
                let (line, column) = locate_key(text, k);
                return Err(ParseError {
                    message: format!(
                        "`{k}` is reserved for a later manifest schema and refused in schema 1 \
                         (secret values never go in a manifest; use the vault)"
                    ),
                    line,
                    column,
                });
            }
        }
    }
    serde_json::from_str::<Manifest>(text).map_err(|e| ParseError {
        message: e.to_string(),
        line: e.line(),
        column: e.column(),
    })
}

fn locate_key(text: &str, key: &str) -> (usize, usize) {
    let needle = format!("\"{key}\"");
    for (i, l) in text.lines().enumerate() {
        if let Some(c) = l.find(&needle) {
            return (i + 1, c + 1);
        }
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const OK: &str = r#"{
  "schema": 1, "kind": "app", "id": "client-status", "name": "Client status", "version": "1.0.0",
  "caps": ["thread:read"],
  "host": { "mode": "cmd", "start": ["node", "server.js"], "health": { "path": "/healthz" } }
}"#;

    #[test]
    fn parses_a_minimal_cmd_app() {
        let m = parse(OK).expect("parse");
        assert_eq!(m.id, "client-status");
        assert_eq!(m.host.start, vec!["node", "server.js"]);
        assert_eq!(m.health_path(), "/healthz");
        assert!(!m.wants_test_guest_probe());
        assert_eq!(m.data_revision, 0);
    }

    #[test]
    fn unknown_keys_fail_with_a_place() {
        let bad = OK.replace("\"caps\"", "\"capz\"");
        let e = parse(&bad).unwrap_err();
        assert!(e.message.contains("capz"), "{e:?}");
        assert!(e.line >= 2, "{e:?}");
    }

    #[test]
    fn egress_hosts_are_an_optional_request() {
        assert_eq!(parse(OK).unwrap().egress, None);
        let m = parse(&OK.replace("\"caps\"", "\"egress\": {\"hosts\": [\"api.stripe.com\", \"*.s3.amazonaws.com:443\"]}, \"caps\"")).unwrap();
        assert_eq!(m.egress.unwrap().hosts, vec!["api.stripe.com", "*.s3.amazonaws.com:443"]);
        let e = parse(&OK.replace("\"caps\"", "\"egress\": {\"mode\": \"broad\"}, \"caps\"")).unwrap_err();
        assert!(e.message.contains("mode"), "a Seed can't pick broad: {e:?}");
    }

    #[test]
    fn reserved_keys_are_refused_by_name() {
        let bad = OK.replace("\"caps\"", "\"secrets\": [], \"caps\"");
        let e = parse(&bad).unwrap_err();
        assert!(e.message.contains("`secrets` is reserved"), "{e:?}");
        assert_eq!(e.line, 3);
    }
}

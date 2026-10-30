//! The App manifest: `manifest.json` at the Seed root (vision v2 §5.3,
//! schema 1, `kind: "app"`). Strict JSON: unknown keys are errors with
//! line and column. `net` and `secrets` are reserved for a later schema
//! and refused by name.
//!
//! Stage 1 runs `host.mode = "cmd"` (a server started from `host.start`
//! argv) on the process engine. `static` and `next` parse and validate;
//! `static` Apps wait until the runtime hosts them (`--skin` makes no new sites).

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
    /// Services on this box's loopback the App asks to reach (0.46.0,
    /// `deploy::box_service`): each one is a held request until the owner
    /// approves it; then the guarded engine forwards the container's
    /// `127.0.0.1:<appPort>` to host `127.0.0.1:<hostPort>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub box_services: Vec<BoxServiceRequest>,
}

/// One manifest `boxServices` entry. A known name (`sew-api`,
/// `sew-signal`) may leave `hostPort` out: K2 reads it from Sew's config.
/// `appPort` (the port inside the wall) defaults to the host port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoxServiceRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_port: Option<u16>,
    /// `tcp` (the only one; HTTP and websockets ride it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    /// The Sew chat server this box must run (0.46.3), a version range in
    /// `requires.k2`'s form (`>=0.3.0`, `>=0.3.0 <0.5.0`). Install and
    /// deploy check it against `k2 hostchat status`; `k2 app validate`
    /// checks only the form (it can run off the box).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sew: Option<String>,
    /// Voice (Sew's media engine, `k2 hostchat media`) must be on (0.46.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voice: bool,
    /// The Postgres majors the App works with (0.46.4,
    /// prd-app-pg-choice-dualrun-v1 PA4): a major range in `requires.k2`'s
    /// form with whole majors (`"17"`, `">=17"`, `">=16 <18"`). Install,
    /// deploy and re-host check it against the major of the App's database
    /// (the one its `DATABASE_URL` names); `k2 db upgrade` refuses a target
    /// outside it. `k2 app validate` checks only the form. **Needs K2
    /// 0.46.4**: older daemons refuse a manifest carrying it (pair it with
    /// `requires.k2: ">=0.46.4"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres: Option<String>,
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
    /// 0.46.3: K2 rewrites every HTTP/1.1 request from the K2 Connect
    /// tunnel. Client copies of the forwarding headers are dropped, and
    /// `X-K2-Client-IP` / `X-Forwarded-For` / `X-Real-IP` are set when K2
    /// knows the client. `false` is the escape hatch for an App that speaks
    /// something other than HTTP/1.1 on its port (raw TCP, h2c, a custom
    /// protocol): its connections are spliced raw, as before 0.46.3, with
    /// no client IP. Absent = `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<bool>,
    /// 0.46.4 (prd-app-runtime-update-continuity-v1 R31, R39): where the
    /// App keeps its sign-in sessions. `memory` = a new instance signs
    /// everyone out, so a K2-update re-host waits for the owner. **Needs K2
    /// 0.46.4**: older daemons refuse a manifest carrying it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Sessions>,
    /// 0.46.4 (R14, R39): may two instances run at once (`allow`, the
    /// default) or must the old one stop first (`handover`)? The owner's
    /// per-App policy wins. Needs K2 0.46.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<Overlap>,
}

/// `host.sessions` (0.46.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sessions {
    /// In the process: lost by any new instance.
    Memory,
    /// Sealed into `/data` (or a cookie): they carry over.
    Sealed,
    /// The App has no sign-in.
    None,
}

/// `host.overlap` (0.46.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Overlap {
    Allow,
    Handover,
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
    /// The port accepting a connection is the whole check (0.46.1, K5):
    /// what `k2 publish run --cmd` always checked, for an old service
    /// moved onto the open path. `path` is ignored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tcp: bool,
    /// 0.46.4 (R32, R39): a readiness path probed once after the health
    /// gate and logged; never a gate. Needs K2 0.46.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_path: Option<String>,
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

    /// K2's client-IP rewrite applies to this App (`host.clientIp`, default
    /// on).
    pub fn wants_client_ip(&self) -> bool {
        self.host.client_ip.unwrap_or(true)
    }

    /// The health check is a TCP accept (K5's folder manifests).
    pub fn tcp_health(&self) -> bool {
        self.host.health.as_ref().is_some_and(|h| h.tcp)
    }

    /// `host.health.readyPath` (0.46.4), when set.
    pub fn ready_path(&self) -> Option<String> {
        self.host.health.as_ref().and_then(|h| h.ready_path.clone()).filter(|p| p.starts_with('/'))
    }

    /// The manifest keys a daemon before 0.46.4 refuses (R39): K2 warns
    /// that a K2 rollback can't run this Seed.
    pub fn keys_needing_0464(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.host.sessions.is_some() {
            v.push("host.sessions");
        }
        if self.host.overlap.is_some() {
            v.push("host.overlap");
        }
        if self.host.health.as_ref().is_some_and(|h| h.ready_path.is_some()) {
            v.push("host.health.readyPath");
        }
        v
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
    fn client_ip_defaults_on_and_can_be_turned_off() {
        assert!(parse(OK).unwrap().wants_client_ip(), "absent = on");
        let off = OK.replace("\"mode\": \"cmd\",", "\"mode\": \"cmd\", \"clientIp\": false,");
        let m = parse(&off).expect("clientIp parses");
        assert!(!m.wants_client_ip());
        let on = OK.replace("\"mode\": \"cmd\",", "\"mode\": \"cmd\", \"clientIp\": true,");
        assert!(parse(&on).unwrap().wants_client_ip());
        assert!(parse(&OK.replace("\"mode\": \"cmd\",", "\"mode\": \"cmd\", \"clientIp\": \"no\",")).is_err());
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
    fn box_services_are_an_optional_request() {
        assert!(parse(OK).unwrap().box_services.is_empty());
        let m = parse(&OK.replace(
            "\"caps\"",
            "\"boxServices\": [{\"name\": \"sew-api\"}, {\"name\": \"sew-signal\", \"hostPort\": 7880, \"appPort\": 7880, \"proto\": \"tcp\", \"reason\": \"voice\"}], \"caps\"",
        ))
        .unwrap();
        assert_eq!(m.box_services.len(), 2);
        assert_eq!(m.box_services[0], BoxServiceRequest { name: "sew-api".into(), host_port: None, app_port: None, proto: None, reason: None });
        assert_eq!(m.box_services[1].host_port, Some(7880));
        let e = parse(&OK.replace("\"caps\"", "\"boxServices\": [{\"name\": \"x\", \"host\": \"10.0.0.1\"}], \"caps\"")).unwrap_err();
        assert!(e.message.contains("host"), "only a loopback port, never a host: {e:?}");
    }

    /// 0.46.3: `requires.sew` (a range) and `requires.voice` (a bool);
    /// other keys under `requires` are still errors.
    #[test]
    fn requires_sew_and_voice_parse() {
        assert_eq!(parse(OK).unwrap().requires, None);
        let m = parse(&OK.replace("\"caps\"", "\"requires\": {\"sew\": \">=0.3.0\", \"voice\": true}, \"caps\"")).unwrap();
        let r = m.requires.expect("requires");
        assert_eq!(r.sew.as_deref(), Some(">=0.3.0"));
        assert!(r.voice);
        assert_eq!(r.k2, None);
        let m = parse(&OK.replace("\"caps\"", "\"requires\": {\"sew\": \">=0.2.0\"}, \"caps\"")).unwrap();
        assert!(!m.requires.as_ref().unwrap().voice, "voice defaults to off");
        // Round trip: a false voice isn't written back.
        let back = serde_json::to_value(m.requires.unwrap()).unwrap();
        assert_eq!(back, serde_json::json!({"sew": ">=0.2.0"}));
        let e = parse(&OK.replace("\"caps\"", "\"requires\": {\"voice\": \"yes\"}, \"caps\"")).unwrap_err();
        assert!(e.message.contains("bool"), "voice is a bool: {e:?}");
        let e = parse(&OK.replace("\"caps\"", "\"requires\": {\"sewVersion\": \">=0.3.0\"}, \"caps\"")).unwrap_err();
        assert!(e.message.contains("sewVersion"), "{e:?}");
    }

    /// 0.46.4 (R39): `host.sessions`, `host.overlap`, `host.health.readyPath`
    /// parse, refuse unknown values, and are named as needing 0.46.4.
    #[test]
    fn the_0464_host_keys_parse_and_are_named() {
        assert!(parse(OK).unwrap().keys_needing_0464().is_empty());
        let m = parse(&OK.replace(
            "\"health\": { \"path\": \"/healthz\" }",
            "\"sessions\": \"memory\", \"overlap\": \"handover\", \"health\": { \"path\": \"/healthz\", \"readyPath\": \"/readyz\" }",
        ))
        .unwrap();
        assert_eq!((m.host.sessions, m.host.overlap), (Some(Sessions::Memory), Some(Overlap::Handover)));
        assert_eq!(m.ready_path().as_deref(), Some("/readyz"));
        assert_eq!(m.keys_needing_0464(), vec!["host.sessions", "host.overlap", "host.health.readyPath"]);
        assert!(parse(&OK.replace("\"mode\": \"cmd\",", "\"mode\": \"cmd\", \"sessions\": \"disk\",")).is_err());
        assert!(parse(&OK.replace("\"mode\": \"cmd\",", "\"mode\": \"cmd\", \"overlap\": \"never\",")).is_err());
    }

    #[test]
    fn reserved_keys_are_refused_by_name() {
        let bad = OK.replace("\"caps\"", "\"secrets\": [], \"caps\"");
        let e = parse(&bad).unwrap_err();
        assert!(e.message.contains("`secrets` is reserved"), "{e:?}");
        assert_eq!(e.line, 3);
    }
}

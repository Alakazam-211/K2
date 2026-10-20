//! Install inputs as JSON (SW5): `sew install --config-json <FILE|->` and
//! `sew media install --config-json <FILE|->` take their URLs, hosts and paths as one JSON
//! object (from a file, or stdin with `-`) instead of argv. K2's root helper allows argv tokens
//! of at most 64 characters from `[A-Za-z0-9-_.@=]`, which a URL or a path cannot be.
//!
//! The keys are the value flags' names (`--k2-jwks-url` → `k2_jwks_url`; repeatable flags are
//! plural arrays: `--mgmt-user` → `mgmt_users`, `--stun-server` → `stun_servers`,
//! `--turn-plain` → `turn_plain`). Mode flags (`--no-start`, `--reconfigure`, `--restart`,
//! `--now`, `--print-units`) stay on argv. Unknown keys and wrong types are refused, every
//! value is checked here ([`Validate`]), and a key given both in the JSON and as its flag is a
//! usage error (exit 2). The existing flags keep working alone. The JSON never carries key
//! material: `master_key_from` is an absolute path (not `-`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};

/// Largest `--config-json` input, bytes.
pub const CONFIG_JSON_MAX_BYTES: usize = 65_536;
/// Longest string value, bytes.
pub const INPUT_MAX_BYTES: usize = 2_048;
/// Most entries in an array value.
pub const INPUT_LIST_MAX: usize = 16;

/// `sew install --config-json` (SW5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallInputs {
    /// `--provider`: `standalone` or `k2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// `--k2-jwks-url`: K2's JWKS URL (with provider `k2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2_jwks_url: Option<String>,
    /// `--profile`: `dedicated` or `shared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// `--pg-major`: the Postgres major, 16–99.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pg_major: Option<u32>,
    /// `--master-key-from`: an absolute path to the master key file (never `-`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_key_from: Option<String>,
    /// `--cred-mode`: `encrypted` or `plain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cred_mode: Option<String>,
    /// `--mgmt-user` (repeatable): users to add to `sew-mgmt`, 0–16 login names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mgmt_users: Vec<String>,
    /// `[mgmt] allow_uids` (SW7): integrator uids allowed on the mgmt socket besides the
    /// `sew-mgmt` group; `sew install` writes them to `sew.toml` (absent: the file keeps what
    /// it has; `[]` removes the key). 0–16 distinct uids, never 0. JSON only, no flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mgmt_allow_uids: Option<Vec<u32>>,
}

/// Most uids in `mgmt_allow_uids` (the server's `[mgmt] allow_uids` cap).
pub const MGMT_ALLOW_UIDS_MAX: usize = 16;

/// `sew media install --config-json` (SW5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaInstallInputs {
    /// `--tarball`: an absolute path to the pinned sew-media tarball.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tarball: Option<String>,
    /// `--public-url`: the signalling URL clients use (`wss://voice.<host>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// `--node-ip`: public IPv4[,IPv6] the engine advertises, or `stun`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    /// `--stun-server` (repeatable): STUN `host:port` for node IP `stun`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stun_servers: Vec<String>,
    /// `--allow-private-node-ip`: allow loopback and private node IPs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_private_node_ip: Option<bool>,
    /// `--udp-port`: the UDP media port, 1–65535.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_port: Option<u16>,
    /// `--tcp-port`: the ICE-TCP port, 1–65535.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_port: Option<u16>,
    /// `--bind-v4`: the IPv4 bind address (`""` turns IPv4 off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v4: Option<String>,
    /// `--bind-v6`: the IPv6 bind address (`""` turns IPv6 off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v6: Option<String>,
    /// `--turn-tls`: `direct`, `external`, `passthrough`, `plain` or `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_tls: Option<String>,
    /// `--turn-domain`: the TURN DNS name (`turn.<host>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_domain: Option<String>,
    /// `--turn-cert`: an absolute path to the TURN TLS certificate chain (PEM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_cert: Option<String>,
    /// `--turn-key`: an absolute path to the TURN TLS private key (PEM, root-only file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_key: Option<String>,
    /// `--turn-advertise-port`: the advertised TURN TLS port (0 = the listener's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_advertise_port: Option<u16>,
    /// `--turn-proxy-protocol`: `v1` or `v2` (with TURN TLS `external`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_proxy_protocol: Option<String>,
    /// `--turn-plain`: plain TURN listeners, each `udp` or `tcp`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_plain: Vec<String>,
    /// `--profile`: `dedicated` or `shared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// `--cred-mode`: `encrypted` or `plain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cred_mode: Option<String>,
    /// `--sysctl`: `apply` or `skip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl: Option<String>,
}

fn text(field: &str, v: &str, allow_empty: bool) -> Result<(), ValidationError> {
    if v.is_empty() && !allow_empty {
        return Err(ValidationError::invalid(field, "must not be empty"));
    }
    if v.len() > INPUT_MAX_BYTES {
        return Err(ValidationError::invalid(
            field,
            format!("at most {INPUT_MAX_BYTES} bytes"),
        ));
    }
    if v.chars().any(char::is_control) {
        return Err(ValidationError::invalid(
            field,
            "control characters are not allowed",
        ));
    }
    Ok(())
}

fn opt_text(field: &str, v: &Option<String>) -> Result<(), ValidationError> {
    v.as_deref().map_or(Ok(()), |s| text(field, s, false))
}

fn one_of(field: &str, v: &Option<String>, allowed: &[&str]) -> Result<(), ValidationError> {
    match v.as_deref() {
        Some(s) if !allowed.contains(&s) => Err(ValidationError::invalid(
            field,
            format!("must be one of {}", allowed.join(", ")),
        )),
        _ => Ok(()),
    }
}

fn abs_path(field: &str, v: &Option<String>) -> Result<(), ValidationError> {
    match v.as_deref() {
        None => Ok(()),
        Some(s) => {
            text(field, s, false)?;
            if s.starts_with('/') {
                Ok(())
            } else {
                Err(ValidationError::invalid(field, "must be an absolute path"))
            }
        }
    }
}

fn url(field: &str, v: &Option<String>, schemes: &[&str]) -> Result<(), ValidationError> {
    match v.as_deref() {
        None => Ok(()),
        Some(s) => {
            text(field, s, false)?;
            if schemes.iter().any(|p| s.starts_with(p)) && !s.contains(char::is_whitespace) {
                Ok(())
            } else {
                Err(ValidationError::invalid(
                    field,
                    format!("must be a URL starting with {}", schemes.join(" or ")),
                ))
            }
        }
    }
}

fn list(
    field: &str,
    v: &[String],
    each: impl Fn(&str) -> bool,
    what: &str,
) -> Result<(), ValidationError> {
    if v.len() > INPUT_LIST_MAX {
        return Err(ValidationError::invalid(
            field,
            format!("at most {INPUT_LIST_MAX} entries"),
        ));
    }
    for (i, s) in v.iter().enumerate() {
        let f = format!("{field}.{i}");
        text(&f, s, false)?;
        if !each(s) {
            return Err(ValidationError::invalid(f, what));
        }
    }
    let mut seen: Vec<&String> = Vec::new();
    for s in v {
        if seen.contains(&s) {
            return Err(ValidationError::invalid(
                field,
                format!("{s:?} is listed twice"),
            ));
        }
        seen.push(s);
    }
    Ok(())
}

fn login_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

impl Validate for InstallInputs {
    fn validate(&self) -> Result<(), ValidationError> {
        one_of("provider", &self.provider, &["standalone", "k2"])?;
        url("k2_jwks_url", &self.k2_jwks_url, &["http://", "https://"])?;
        one_of("profile", &self.profile, &["dedicated", "shared"])?;
        if let Some(m) = self.pg_major
            && !(16..=99).contains(&m)
        {
            return Err(ValidationError::invalid("pg_major", "must be 16 to 99"));
        }
        abs_path("master_key_from", &self.master_key_from)?;
        one_of("cred_mode", &self.cred_mode, &["encrypted", "plain"])?;
        list(
            "mgmt_users",
            &self.mgmt_users,
            login_name,
            "must be a login name ([a-z_][a-z0-9_-]{0,31})",
        )?;
        if let Some(uids) = &self.mgmt_allow_uids {
            if uids.len() > MGMT_ALLOW_UIDS_MAX {
                return Err(ValidationError::invalid(
                    "mgmt_allow_uids",
                    format!("at most {MGMT_ALLOW_UIDS_MAX} uids"),
                ));
            }
            for (i, u) in uids.iter().enumerate() {
                if *u == 0 {
                    return Err(ValidationError::invalid(
                        format!("mgmt_allow_uids.{i}"),
                        "root (uid 0) is always allowed; do not list it",
                    ));
                }
                if uids[..i].contains(u) {
                    return Err(ValidationError::invalid(
                        format!("mgmt_allow_uids.{i}"),
                        "listed twice",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Validate for MediaInstallInputs {
    fn validate(&self) -> Result<(), ValidationError> {
        abs_path("tarball", &self.tarball)?;
        url("public_url", &self.public_url, &["wss://", "ws://"])?;
        opt_text("node_ip", &self.node_ip)?;
        list(
            "stun_servers",
            &self.stun_servers,
            |s| {
                s.rsplit_once(':')
                    .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok_and(|p| p > 0))
            },
            "must be host:port",
        )?;
        for (field, v) in [("udp_port", self.udp_port), ("tcp_port", self.tcp_port)] {
            if v == Some(0) {
                return Err(ValidationError::invalid(field, "must be 1 to 65535"));
            }
        }
        for (field, v) in [("bind_v4", &self.bind_v4), ("bind_v6", &self.bind_v6)] {
            if let Some(s) = v {
                text(field, s, true)?;
            }
        }
        one_of(
            "turn_tls",
            &self.turn_tls,
            &["direct", "external", "passthrough", "plain", "off"],
        )?;
        opt_text("turn_domain", &self.turn_domain)?;
        abs_path("turn_cert", &self.turn_cert)?;
        abs_path("turn_key", &self.turn_key)?;
        one_of(
            "turn_proxy_protocol",
            &self.turn_proxy_protocol,
            &["v1", "v2"],
        )?;
        list(
            "turn_plain",
            &self.turn_plain,
            |s| s == "udp" || s == "tcp",
            "must be udp or tcp",
        )?;
        one_of("profile", &self.profile, &["dedicated", "shared"])?;
        one_of("cred_mode", &self.cred_mode, &["encrypted", "plain"])?;
        one_of("sysctl", &self.sysctl, &["apply", "skip"])
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn install(v: serde_json::Value) -> Result<InstallInputs, String> {
        let i: InstallInputs = serde_json::from_value(v).map_err(|e| e.to_string())?;
        i.validate().map_err(|e| e.field)?;
        Ok(i)
    }

    fn media(v: serde_json::Value) -> Result<MediaInstallInputs, String> {
        let i: MediaInstallInputs = serde_json::from_value(v).map_err(|e| e.to_string())?;
        i.validate().map_err(|e| e.field)?;
        Ok(i)
    }

    #[test]
    fn install_inputs_rules() {
        let ok = install(json!({
            "provider": "k2",
            "k2_jwks_url": "http://127.0.0.1:7300/v1/chat/jwks",
            "profile": "shared",
            "pg_major": 16,
            "master_key_from": "/run/k2/sew-master.key",
            "cred_mode": "encrypted",
            "mgmt_users": ["k2", "_k2-daemon"],
            "mgmt_allow_uids": [988, 1001]
        }));
        assert_eq!(
            ok.map(|i| (i.mgmt_users.len(), i.mgmt_allow_uids)),
            Ok((2, Some(vec![988, 1001])))
        );
        assert_eq!(
            install(json!({"mgmt_allow_uids": []})).map(|i| i.mgmt_allow_uids),
            Ok(Some(vec![]))
        );
        assert!(install(json!({"mgmt_allow_uids": [-1]})).is_err());
        assert!(install(json!({"mgmt_allow_uids": "988"})).is_err());
        assert_eq!(install(json!({})), Ok(InstallInputs::default()));
        let unknown = install(json!({"provider": "k2", "jwks": "x"})).unwrap_err();
        assert!(unknown.contains("unknown field `jwks`"), "{unknown}");
        assert!(install(json!({"pg_major": "16"})).is_err());
        assert!(install(json!({"mgmt_users": "k2"})).is_err());
        for (v, field) in [
            (json!({"provider": "oidc"}), "provider"),
            (json!({"k2_jwks_url": "ftp://x/j"}), "k2_jwks_url"),
            (json!({"k2_jwks_url": "http://x/\nj"}), "k2_jwks_url"),
            (
                json!({"k2_jwks_url": format!("https://{}", "a".repeat(2_048))}),
                "k2_jwks_url",
            ),
            (json!({"profile": "huge"}), "profile"),
            (json!({"pg_major": 15}), "pg_major"),
            (json!({"master_key_from": "-"}), "master_key_from"),
            (json!({"master_key_from": "key.bin"}), "master_key_from"),
            (json!({"cred_mode": "clear"}), "cred_mode"),
            (json!({"mgmt_users": ["Root"]}), "mgmt_users.0"),
            (json!({"mgmt_users": ["k2", "k2"]}), "mgmt_users"),
            (json!({"mgmt_allow_uids": [0]}), "mgmt_allow_uids.0"),
            (json!({"mgmt_allow_uids": [988, 988]}), "mgmt_allow_uids.1"),
            (
                json!({"mgmt_allow_uids": (1..=17).collect::<Vec<u32>>()}),
                "mgmt_allow_uids",
            ),
        ] {
            assert_eq!(install(v.clone()).err().as_deref(), Some(field), "{v}");
        }
    }

    #[test]
    fn media_install_inputs_rules() {
        let ok = media(json!({
            "tarball": "/var/lib/k2/staging/sew-media.tar.zst",
            "public_url": "wss://voice.box.k2.dev",
            "node_ip": "135.148.122.178",
            "stun_servers": ["stun.example.net:3478"],
            "allow_private_node_ip": false,
            "udp_port": 7882,
            "tcp_port": 7881,
            "bind_v4": "0.0.0.0",
            "bind_v6": "",
            "turn_tls": "direct",
            "turn_domain": "turn.box.k2.dev",
            "turn_cert": "/var/lib/k2/turn/fullchain.pem",
            "turn_key": "/var/lib/k2/turn/key.pem",
            "turn_advertise_port": 5349,
            "turn_proxy_protocol": "v2",
            "turn_plain": ["udp", "tcp"],
            "profile": "shared",
            "cred_mode": "encrypted",
            "sysctl": "apply"
        }));
        assert!(ok.is_ok(), "{ok:?}");
        // TURN-PT: every tier K2's helper may send, `passthrough` included.
        for tier in ["direct", "external", "passthrough", "plain", "off"] {
            let v = media(json!({"turn_tls": tier}));
            assert_eq!(v.map(|m| m.turn_tls), Ok(Some(tier.to_owned())), "{tier}");
        }
        assert_eq!(media(json!({})), Ok(MediaInstallInputs::default()));
        assert!(media(json!({"publicurl": "wss://x"})).is_err());
        assert!(media(json!({"udp_port": 70000})).is_err());
        assert!(media(json!({"udp_port": -1})).is_err());
        for (v, field) in [
            (json!({"tarball": "sew-media.tar"}), "tarball"),
            (json!({"public_url": "https://voice.x"}), "public_url"),
            (json!({"stun_servers": ["stun.x"]}), "stun_servers.0"),
            (json!({"udp_port": 0}), "udp_port"),
            (json!({"turn_tls": "on"}), "turn_tls"),
            (json!({"turn_key": "key.pem"}), "turn_key"),
            (json!({"turn_proxy_protocol": "v3"}), "turn_proxy_protocol"),
            (json!({"turn_plain": ["sctp"]}), "turn_plain.0"),
            (json!({"sysctl": "yes"}), "sysctl"),
            (json!({"node_ip": ""}), "node_ip"),
        ] {
            assert_eq!(media(v.clone()).err().as_deref(), Some(field), "{v}");
        }
    }
}

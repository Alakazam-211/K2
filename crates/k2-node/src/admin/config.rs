//! `admin.toml` (prd-k2-node-admin-v1 §6): the machine owner's
//! remote-admin consent, the pinned controller key and the owner approval
//! keys. Root-owned 0644 in the config dir; written by `k2-node setup` /
//! `k2-node admin level|pin` (root, at the machine) and by the executor
//! for the two remote changes it allows (lowering the level, rotating
//! approval keys). Never by the node user.
//!
//! A file that doesn't parse is fail-closed: every command is refused
//! `not_configured` until a person fixes it.

use std::path::{Path, PathBuf};

use k2_node_proto::admin::{ApprovalKey, Level};
use k2_node_proto::crypto;
use serde::{Deserialize, Serialize};

pub const ADMIN_FILE: &str = "admin.toml";
/// The controller's persisted narrowing (`policy.narrow`), root-owned.
pub const NARROW_FILE: &str = "narrow.toml";

/// Root-only state: nonce store, audit chain, update state, node fp pin.
pub fn default_state_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/var/db/k2-node-admin")
    } else {
        PathBuf::from("/var/lib/k2-node-admin")
    }
}

/// The executor's Unix socket (mode 0660, group = the node user's group).
pub fn default_socket() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/var/run/k2-node-admin.sock")
    } else {
        PathBuf::from("/run/k2-node-admin.sock")
    }
}

pub fn default_node_user() -> &'static str {
    if cfg!(target_os = "macos") {
        "_k2node"
    } else {
        "k2node"
    }
}

/// The file as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminFile {
    /// `off` | `policy` | `full`.
    pub remote_admin: String,
    /// Root's own copy of the pinned controller key: SHA-256 of the SPKI
    /// DER and the SPKI DER itself (base64). The node's `controller.json`
    /// is writable by the node user (and so by jobs) and is never trusted
    /// here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_fp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_key: Option<String>,
    /// `ed25519:<b64>` owner approval public keys (§7.4).
    #[serde(default)]
    pub approval_keys: Vec<String>,
    /// `follow` (take newer signed builds the controller offers) | `off`.
    #[serde(default = "follow")]
    pub auto_update: String,
    #[serde(default)]
    pub alerts_local: bool,
    /// The node user, the only non-root peer the socket accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_user: Option<String>,
}

fn follow() -> String {
    "follow".into()
}

impl Default for AdminFile {
    fn default() -> Self {
        Self {
            remote_admin: "off".into(),
            controller_fp: None,
            controller_key: None,
            approval_keys: vec![],
            auto_update: follow(),
            alerts_local: true,
            node_user: None,
        }
    }
}

/// The checked, typed view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminConfig {
    pub level: Level,
    pub controller: Option<Controller>,
    pub approval_keys: Vec<ApprovalKey>,
    pub auto_update: bool,
    pub node_user: String,
    pub file: AdminFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub fp: String,
    pub spki_der: Vec<u8>,
}

impl AdminConfig {
    pub fn from_file(f: AdminFile) -> Result<Self, String> {
        let level = Level::parse(&f.remote_admin).ok_or_else(|| format!("remote_admin {:?} is not off|policy|full", f.remote_admin))?;
        let controller = match (&f.controller_fp, &f.controller_key) {
            (None, None) => None,
            (Some(fp), Some(key)) => {
                let der = crypto::unb64(key).map_err(|e| format!("controller_key: {e}"))?;
                crypto::p256_point_from_spki(&der)?;
                if crypto::fingerprint_of_spki_der(&der) != *fp {
                    return Err("controller_key does not match controller_fp".into());
                }
                Some(Controller { fp: fp.clone(), spki_der: der })
            }
            _ => return Err("controller_fp and controller_key go together".into()),
        };
        let mut approval_keys = Vec::new();
        for k in &f.approval_keys {
            approval_keys.push(ApprovalKey::parse(k).ok_or_else(|| format!("approval key {k:?} is not ed25519:<base64 32 bytes>"))?);
        }
        if approval_keys.len() > 4 {
            return Err("at most 4 approval keys".into());
        }
        let auto_update = match f.auto_update.as_str() {
            "follow" => true,
            "off" => false,
            o => return Err(format!("auto_update {o:?} is not follow|off")),
        };
        let node_user = f.node_user.clone().unwrap_or_else(|| default_node_user().to_string());
        Ok(Self { level, controller, approval_keys, auto_update, node_user, file: f })
    }
}

/// `Ok(None)` when the file is missing (= remote admin off, never set up).
pub fn read(path: &Path) -> Result<Option<AdminConfig>, String> {
    // Root trusts it (the pinned owner, the remote level, the node user):
    // read as root, only root's own copy counts (review A M5).
    let text = match super::jobs::read_root_trusted(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let f: AdminFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    AdminConfig::from_file(f).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write (root, 0644). Validates first: a file that wouldn't load is
/// never written.
pub fn write(path: &Path, f: &AdminFile) -> Result<(), String> {
    AdminConfig::from_file(f.clone())?;
    let body = toml::to_string(f).map_err(|e| format!("encode admin.toml: {e}"))?;
    let text = format!(
        "# K2 node remote admin (prd-k2-node-admin-v1 §6). The machine owner's consent.\n\
         # Raise the level only at this machine: sudo k2-node admin level off|policy|full\n{body}"
    );
    crate::util::atomic_write(path, text.as_bytes(), 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node_proto::admin::ApprovalSigner;
    use k2_node_proto::crypto::SigningKey;

    #[test]
    fn missing_is_none_and_round_trip() {
        let d = crate::util::temp_dir("admincfg");
        let p = d.join(ADMIN_FILE);
        assert!(read(&p).unwrap().is_none());
        let k = SigningKey::generate().unwrap();
        let a = ApprovalSigner::generate().unwrap();
        let f = AdminFile {
            remote_admin: "full".into(),
            controller_fp: Some(k.fingerprint()),
            controller_key: Some(crypto::b64(k.spki_der())),
            approval_keys: vec![a.public().encode()],
            ..Default::default()
        };
        write(&p, &f).unwrap();
        let c = read(&p).unwrap().unwrap();
        assert_eq!(c.level, Level::Full);
        assert_eq!(c.controller.unwrap().fp, k.fingerprint());
        assert_eq!(c.approval_keys, vec![a.public()]);
        assert!(c.auto_update);
    }

    #[test]
    fn bad_files_fail_closed() {
        let d = crate::util::temp_dir("admincfg-bad");
        let p = d.join(ADMIN_FILE);
        for bad in [
            "remote_admin = \"root\"\n",
            "remote_admin = \"full\"\nextra = 1\n",
            "remote_admin = \"full\"\ncontroller_fp = \"aa\"\n",
            "remote_admin = \"full\"\napproval_keys = [\"rsa:x\"]\n",
            "remote_admin = \"full\"\nauto_update = \"yes\"\n",
        ] {
            std::fs::write(&p, bad).unwrap();
            assert!(read(&p).is_err(), "{bad}");
        }
        // A key that doesn't match its fingerprint.
        let k = SigningKey::generate().unwrap();
        let f = AdminFile {
            remote_admin: "full".into(),
            controller_fp: Some("0".repeat(64)),
            controller_key: Some(crypto::b64(k.spki_der())),
            ..Default::default()
        };
        assert!(write(&p, &f).is_err(), "never written");
    }
}

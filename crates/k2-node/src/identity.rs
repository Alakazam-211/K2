//! The node key and the controller pin (§7.1).

use std::collections::BTreeMap;
use std::path::Path;

use k2_node_proto::crypto::{self, SigningKey};
use serde::{Deserialize, Serialize};

/// Load `state/node-key.pem`, or make one (0600) when it's missing.
pub fn load_or_create_key(path: &Path) -> Result<SigningKey, String> {
    match std::fs::read_to_string(path) {
        Ok(pem) => SigningKey::from_pkcs8_pem(&pem).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SigningKey::generate()?;
            crate::util::atomic_write(path, key.to_pkcs8_pem()?.as_bytes(), 0o600)?;
            Ok(key)
        }
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// `state/controller.json`: who this node serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub controller_fp: String,
    pub controller_public_key_pem: String,
    /// Base URLs in dial order (`https://rosson.k2.dev`, a LAN door, a tailnet URL).
    pub routes: Vec<String>,
    pub node_id: String,
    pub name: String,
    pub enrolled_at: i64,
    /// Display form, kept for status while pending.
    pub sas: String,
    pub revoked: bool,
    #[serde(default)]
    pub revoked_reason: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Set once the controller accepted an attach (the SAS was confirmed).
    #[serde(default)]
    pub confirmed: bool,
}

impl Pin {
    pub fn controller_spki(&self) -> Result<Vec<u8>, String> {
        let der = crypto::pem_decode("PUBLIC KEY", &self.controller_public_key_pem)?;
        crypto::p256_point_from_spki(&der)?;
        if crypto::fingerprint_of_spki_der(&der) != self.controller_fp {
            return Err("controller.json: key and fingerprint disagree".into());
        }
        Ok(der)
    }

    /// The controller's label for status (host of the first route).
    pub fn controller_label(&self) -> String {
        self.routes
            .first()
            .map(|r| r.split("://").nth(1).unwrap_or(r).trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
}

pub fn read_pin(path: &Path) -> Result<Option<Pin>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn write_pin(path: &Path, pin: &Pin) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(pin).map_err(|e| format!("encode pin: {e}"))?;
    crate::util::atomic_write(path, &body, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_created_once_0600_and_reloads() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("key");
        let p = d.join("state/node-key.pem");
        let a = load_or_create_key(&p).unwrap();
        let b = load_or_create_key(&p).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn pin_checks_key_matches_fingerprint() {
        let k = SigningKey::generate().unwrap();
        let mut pin = Pin {
            controller_fp: k.fingerprint(),
            controller_public_key_pem: k.spki_pem(),
            routes: vec!["https://rosson.k2.dev".into()],
            node_id: "n".into(),
            name: "mini-1".into(),
            enrolled_at: 1,
            sas: "123 456".into(),
            revoked: false,
            revoked_reason: None,
            labels: BTreeMap::new(),
            confirmed: false,
        };
        assert!(pin.controller_spki().is_ok());
        assert_eq!(pin.controller_label(), "rosson.k2.dev");
        pin.controller_fp = "0".repeat(64);
        assert!(pin.controller_spki().is_err());
    }
}

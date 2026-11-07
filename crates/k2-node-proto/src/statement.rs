//! Node self-statements and key rotation (protocol 9, C0/C1; PRD §4.1,
//! vs-live V8–V10, V3). Formats only: `docs/ci-formats-v1.md` §2–§3.
//!
//! - [`NodeStatement`]: what the node says about itself, signed by its own
//!   key under [`crate::ci::DOMAIN_NODE_STATEMENT`]. The owner's roster
//!   entry names its digest; receipt v2 names it in `run.statement_digest`.
//! - [`RotationStatement`]: the **old** key hands over to the new one, under
//!   [`crate::ci::DOMAIN_NODE_ROTATION`]. The new key then signs a statement
//!   with `prev_fp` = the old key and `key_epoch` + 1.

use serde::{Deserialize, Serialize};

use crate::ci::{self, KeyStore, RotationReason, Signed, Taint, Wall};

/// Version of [`NodeStatement`].
pub const NODE_STATEMENT_V: u32 = 1;
/// Version of [`RotationStatement`].
pub const ROTATION_V: u32 = 1;

/// The node's self-statement (signed by the node key). Every key always
/// present (`null` when unknown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeStatement {
    /// [`NODE_STATEMENT_V`].
    pub v: u32,
    /// SHA-256 of `node_spki` (the key that signs this statement).
    pub node_fp: String,
    /// The node's P-256 SubjectPublicKeyInfo, base64 DER.
    pub node_spki: String,
    /// 0 for the key made at enrolment, +1 per rotation.
    pub key_epoch: u32,
    /// When this key was made (unix seconds).
    pub key_born_at: i64,
    /// The key this one replaced; `null` exactly when `key_epoch` is 0.
    pub prev_fp: Option<String>,
    /// When this statement was signed. A node signs a new statement when a
    /// fact below changes (version, wall, taint); the newest one counts.
    pub issued_at: i64,
    pub wall: Wall,
    /// A `--dev` node (no wall, a person's own test node).
    pub dev: bool,
    /// V10: any job ever started under `Wall::None` with this key.
    pub taint: Taint,
    pub key_exposure: KeyExposure,
    pub k2node_version: String,
    /// V8: the build commit from the `K2_NODE_BUILD … commit=` marker (40
    /// hex). A candidate build reports the previous version, so the version
    /// alone can lie. `null` for a build without the marker.
    pub k2node_commit: Option<String>,
    /// The protocol the node speaks.
    pub protocol: u32,
    /// `linux` | `macos`.
    pub os: String,
    /// `x86_64` | `aarch64` | `arm64`.
    pub arch: String,
    /// `/etc/os-release` `ID`, or `macos`.
    pub distro: Option<String>,
    pub distro_version: Option<String>,
    /// `uname -r`.
    pub kernel: Option<String>,
    /// V9: `/proc/sys/kernel/yama/ptrace_scope` (0–3); `null` on macOS or
    /// when Yama is absent.
    pub ptrace_scope: Option<u8>,
    /// SHA-256 hex of `"k2-node-host-v1" ‖ 0x00 ‖ machine id`
    /// (`/etc/machine-id`, the macOS IOPlatformUUID): stable per machine,
    /// not the raw id. Self-reported; the owner's roster `host` is what the
    /// signer's `independent = ["host"]` uses.
    pub host_id: Option<String>,
}

/// Who and what can read the node key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyExposure {
    pub store: KeyStore,
    /// The OS user that owns the key (`k2node`).
    pub owner: String,
    /// File mode bits (decimal: 384 = 0o600); `null` for the keychain.
    pub mode: Option<u32>,
    /// Jobs run as the key's own uid (no wall, `--dev`): a job could read
    /// the key. Always `false` under the job wall.
    pub job_readable: bool,
}

/// The salt of [`NodeStatement::host_id`].
pub const HOST_ID_DOMAIN: &str = "k2-node-host-v1";

/// [`NodeStatement::host_id`] for a machine id.
pub fn host_id(machine_id: &str) -> String {
    let mut b = HOST_ID_DOMAIN.as_bytes().to_vec();
    b.push(0);
    b.extend_from_slice(machine_id.trim().as_bytes());
    crate::crypto::sha256_hex(&b)
}

impl NodeStatement {
    /// Shape rules (no signature): versions, hex, epoch ↔ `prev_fp`, the key
    /// matches its fingerprint.
    pub fn check(&self) -> Result<(), String> {
        if self.v != NODE_STATEMENT_V {
            return Err(format!("node statement v{} (this build knows v{NODE_STATEMENT_V})", self.v));
        }
        ci::check_fp("statement.node_fp", &self.node_fp)?;
        ci::check_spki("statement.node_spki", &self.node_spki, &self.node_fp)?;
        match (&self.prev_fp, self.key_epoch) {
            (None, 0) => {}
            (Some(p), e) if e > 0 => {
                ci::check_fp("statement.prev_fp", p)?;
                if p == &self.node_fp {
                    return Err("statement.prev_fp is the key itself".into());
                }
            }
            _ => return Err("statement: prev_fp is set exactly when key_epoch > 0".into()),
        }
        if self.issued_at < self.key_born_at {
            return Err("statement: issued before the key was born".into());
        }
        ci::check_version_text("statement.k2node_version", &self.k2node_version)?;
        if let Some(c) = &self.k2node_commit {
            ci::check_commit("statement.k2node_commit", c)?;
        }
        ci::check_name("statement.os", &self.os)?;
        ci::check_name("statement.arch", &self.arch)?;
        for (w, v) in [("distro", &self.distro), ("distro_version", &self.distro_version), ("kernel", &self.kernel)] {
            if let Some(v) = v {
                ci::check_text(w, v, 128)?;
            }
        }
        if self.ptrace_scope.is_some_and(|p| p > 3) {
            return Err("statement.ptrace_scope: 0-3".into());
        }
        if let Some(h) = &self.host_id {
            ci::check_fp("statement.host_id", h)?;
        }
        if (self.os == "linux" && self.wall == Wall::Seatbelt) || (self.os == "macos" && self.wall == Wall::JobUser) {
            return Err(format!("statement: wall {:?} on {}", self.wall, self.os));
        }
        ci::check_text("statement.key_exposure.owner", &self.key_exposure.owner, 64)?;
        match (self.key_exposure.store, self.key_exposure.mode) {
            (KeyStore::File, Some(m)) if m <= 0o7777 => {}
            (KeyStore::Keychain, None) => {}
            _ => return Err("statement.key_exposure: a file key has a mode, a keychain key none".into()),
        }
        Ok(())
    }

    /// The P-256 SPKI DER this statement names (checked against `node_fp`).
    pub fn spki_der(&self) -> Result<Vec<u8>, String> {
        ci::check_spki("statement.node_spki", &self.node_spki, &self.node_fp)
    }

    pub fn sign(&self, key: &crate::crypto::SigningKey) -> Result<Signed, String> {
        self.check()?;
        if key.fingerprint() != self.node_fp {
            return Err("statement: signed by a key that is not node_fp".into());
        }
        Signed::sign_node(key, ci::DOMAIN_NODE_STATEMENT, self)
    }

    /// Open a self-statement: the key it names must verify it, and the shape
    /// rules hold. Callers then compare `node_fp` with a pin or roster entry.
    pub fn open(s: &Signed) -> Result<Self, String> {
        let peeked: NodeStatement = s.peek(ci::DOMAIN_NODE_STATEMENT)?;
        let der = peeked.spki_der()?;
        let st: NodeStatement = s.open_node(&der, ci::DOMAIN_NODE_STATEMENT)?;
        st.check()?;
        Ok(st)
    }
}

/// A key rotation: the old key names the new one (signed by the **old**
/// key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationStatement {
    /// [`ROTATION_V`].
    pub v: u32,
    pub old_fp: String,
    pub new_fp: String,
    /// The new key's SPKI, base64 DER.
    pub new_spki: String,
    /// The old key's epoch + 1.
    pub new_epoch: u32,
    pub at: i64,
    pub reason: RotationReason,
}

impl RotationStatement {
    pub fn check(&self) -> Result<(), String> {
        if self.v != ROTATION_V {
            return Err(format!("rotation v{} (this build knows v{ROTATION_V})", self.v));
        }
        ci::check_fp("rotation.old_fp", &self.old_fp)?;
        ci::check_fp("rotation.new_fp", &self.new_fp)?;
        if self.old_fp == self.new_fp {
            return Err("rotation: the new key is the old key".into());
        }
        ci::check_spki("rotation.new_spki", &self.new_spki, &self.new_fp)?;
        if self.new_epoch == 0 {
            return Err("rotation: new_epoch is at least 1".into());
        }
        Ok(())
    }

    pub fn sign(&self, old_key: &crate::crypto::SigningKey) -> Result<Signed, String> {
        self.check()?;
        if old_key.fingerprint() != self.old_fp {
            return Err("rotation: signed by a key that is not old_fp".into());
        }
        Signed::sign_node(old_key, ci::DOMAIN_NODE_ROTATION, self)
    }

    /// Open with the old key's SPKI (from the pin or roster the verifier
    /// already trusts).
    pub fn open(s: &Signed, old_spki_der: &[u8]) -> Result<Self, String> {
        let r: RotationStatement = s.open_node(old_spki_der, ci::DOMAIN_NODE_ROTATION)?;
        r.check()?;
        if crate::crypto::fingerprint_of_spki_der(old_spki_der) != r.old_fp {
            return Err("rotation: old_fp is not the verifying key".into());
        }
        Ok(r)
    }

    /// The new key's statement continues this rotation.
    pub fn continued_by(&self, st: &NodeStatement) -> Result<(), String> {
        if st.node_fp != self.new_fp || st.prev_fp.as_deref() != Some(self.old_fp.as_str()) || st.key_epoch != self.new_epoch {
            return Err("rotation: the new key's statement does not continue it (fp, prev_fp, epoch)".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{b64, SigningKey};

    fn statement(k: &SigningKey) -> NodeStatement {
        NodeStatement {
            v: 1,
            node_fp: k.fingerprint(),
            node_spki: b64(k.spki_der()),
            key_epoch: 0,
            key_born_at: 1_700_000_000,
            prev_fp: None,
            issued_at: 1_700_000_100,
            wall: Wall::JobUser,
            dev: false,
            taint: Taint::Clean,
            key_exposure: KeyExposure { store: KeyStore::File, owner: "k2node".into(), mode: Some(0o600), job_readable: false },
            k2node_version: "0.46.10".into(),
            k2node_commit: Some("c".repeat(40)),
            protocol: 9,
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: Some("ubuntu".into()),
            distro_version: Some("24.04".into()),
            kernel: Some("6.8.0-45-generic".into()),
            ptrace_scope: Some(1),
            host_id: Some(host_id("0123456789abcdef0123456789abcdef")),
        }
    }

    #[test]
    fn statement_round_trips_and_every_key_is_present() {
        let k = SigningKey::generate().unwrap();
        let st = statement(&k);
        let s = st.sign(&k).unwrap();
        assert_eq!(NodeStatement::open(&s).unwrap(), st);
        let mut nulls = st.clone();
        (nulls.k2node_commit, nulls.distro, nulls.ptrace_scope, nulls.host_id) = (None, None, None, None);
        let body = nulls.sign(&k).unwrap().body;
        for key in ["\"k2node_commit\":null", "\"distro\":null", "\"ptrace_scope\":null", "\"host_id\":null", "\"prev_fp\":null"] {
            assert!(body.contains(key), "{key} in {body}");
        }
        // Signed by another key that claims this one: refused.
        let other = SigningKey::generate().unwrap();
        assert!(st.sign(&other).is_err());
        let forged = Signed::sign_node(&other, ci::DOMAIN_NODE_STATEMENT, &st).unwrap();
        assert!(NodeStatement::open(&forged).is_err());
        // Tampered fact: refused.
        let t = Signed { body: s.body.replace("\"dev\":false", "\"dev\":true"), sig: s.sig.clone() };
        assert!(NodeStatement::open(&t).is_err());
    }

    #[test]
    fn statement_shape_rules() {
        let k = SigningKey::generate().unwrap();
        let st = statement(&k);
        st.check().unwrap();
        assert!(NodeStatement { v: 2, ..st.clone() }.check().is_err());
        assert!(NodeStatement { key_epoch: 1, ..st.clone() }.check().is_err(), "epoch 1 without prev_fp");
        assert!(NodeStatement { prev_fp: Some("a".repeat(64)), ..st.clone() }.check().is_err(), "prev_fp at epoch 0");
        NodeStatement { prev_fp: Some("a".repeat(64)), key_epoch: 1, ..st.clone() }.check().unwrap();
        assert!(NodeStatement { node_fp: "a".repeat(64), ..st.clone() }.check().is_err(), "fp not the key's");
        assert!(NodeStatement { wall: Wall::Seatbelt, ..st.clone() }.check().is_err());
        assert!(NodeStatement { ptrace_scope: Some(4), ..st.clone() }.check().is_err());
        assert!(NodeStatement { issued_at: 1, ..st.clone() }.check().is_err());
        let kc = KeyExposure { store: KeyStore::Keychain, owner: "_k2node".into(), mode: Some(0o600), job_readable: false };
        assert!(NodeStatement { key_exposure: kc, ..st }.check().is_err());
    }

    #[test]
    fn rotation_hands_over() {
        let old = SigningKey::generate().unwrap();
        let new = SigningKey::generate().unwrap();
        let r = RotationStatement {
            v: 1,
            old_fp: old.fingerprint(),
            new_fp: new.fingerprint(),
            new_spki: b64(new.spki_der()),
            new_epoch: 1,
            at: 1_700_000_500,
            reason: RotationReason::Wall,
        };
        let s = r.sign(&old).unwrap();
        assert_eq!(RotationStatement::open(&s, old.spki_der()).unwrap(), r);
        assert!(RotationStatement::open(&s, new.spki_der()).is_err(), "the new key can't vouch for itself");
        assert!(r.sign(&new).is_err());
        let mut st = statement(&new);
        st.key_epoch = 1;
        st.prev_fp = Some(old.fingerprint());
        r.continued_by(&st).unwrap();
        st.key_epoch = 2;
        assert!(r.continued_by(&st).is_err());
        assert!(RotationStatement { new_epoch: 0, ..r.clone() }.check().is_err());
        assert!(RotationStatement { new_fp: old.fingerprint(), ..r }.check().is_err());
    }

    #[test]
    fn host_id_is_salted_and_trimmed() {
        assert_eq!(host_id("abc\n"), host_id("abc"));
        assert_ne!(host_id("abc"), crate::crypto::sha256_hex(b"abc"));
        assert_eq!(host_id("abc").len(), 64);
    }
}

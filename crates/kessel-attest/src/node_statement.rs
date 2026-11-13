//! The node's self-statement: what a build machine says about itself (its
//! wall, whether a job could read its key, whether it ever ran a job without
//! the wall), signed by its own node key. Self-reported: it catches
//! mistakes, not a hostile root. The roster and the build statement name it
//! by digest.

use serde::{Deserialize, Serialize};

use crate::dsse::NodeKey;
use crate::roster::Signed;
use crate::statement::Wall;
use crate::util::{is_fp, signed_bytes, unb64};

/// The domain a node signs its self-statement under.
pub const DOMAIN_NODE_STATEMENT: &str = "k2-node-statement-v1";
/// The version this crate reads.
pub const NODE_STATEMENT_V: u32 = 1;

/// The node's taint flag for its current key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Taint {
    /// No job ever started without the wall with this key.
    Clean,
    /// One did.
    Tainted,
    /// The key predates the flag.
    Unknown,
}

/// Where the node key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum KeyStore {
    File,
    Keychain,
}

/// Who and what can read the node key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct KeyExposure {
    pub store: KeyStore,
    pub owner: String,
    pub mode: Option<u32>,
    /// A job could read the key.
    pub job_readable: bool,
}

/// The self-statement. Every key always present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct NodeStatement {
    pub v: u32,
    pub node_fp: String,
    pub node_spki: String,
    pub key_epoch: u32,
    pub key_born_at: i64,
    pub prev_fp: Option<String>,
    pub issued_at: i64,
    pub wall: Wall,
    /// A person's own test node, with no wall.
    pub dev: bool,
    pub taint: Taint,
    pub key_exposure: KeyExposure,
    pub k2node_version: String,
    pub k2node_commit: Option<String>,
    pub protocol: u32,
    pub os: String,
    pub arch: String,
    pub distro: Option<String>,
    pub distro_version: Option<String>,
    pub kernel: Option<String>,
    pub ptrace_scope: Option<u8>,
    pub host_id: Option<String>,
}

impl NodeStatement {
    /// Verify `s` with `key` (the roster's key for this node), then parse.
    /// The statement must name that key.
    pub fn open(s: &Signed, key: &NodeKey) -> Result<Self, String> {
        if !key.verify(&signed_bytes(DOMAIN_NODE_STATEMENT, &[s.body.as_bytes()]), &s.sig) {
            return Err("self-statement: the signature does not verify with the node key".into());
        }
        let st: NodeStatement = crate::canonical::parse_exact("self-statement", s.body.as_bytes())?;
        if st.v != NODE_STATEMENT_V {
            return Err(format!("self-statement v{} (this verifier reads v{NODE_STATEMENT_V})", st.v));
        }
        if st.node_fp != key.fingerprint() {
            return Err("self-statement: it names another node key".into());
        }
        let der = unb64(&st.node_spki).map_err(|e| format!("self-statement: node_spki: {e}"))?;
        if der != key.spki_der() {
            return Err("self-statement: node_spki is not the node key".into());
        }
        match (&st.prev_fp, st.key_epoch) {
            (None, 0) => {}
            (Some(p), e) if e > 0 && is_fp(p) && p != &st.node_fp => {}
            _ => return Err("self-statement: prev_fp is set exactly when key_epoch > 0".into()),
        }
        if st.issued_at < st.key_born_at {
            return Err("self-statement: issued before the key was born".into());
        }
        Ok(st)
    }
}

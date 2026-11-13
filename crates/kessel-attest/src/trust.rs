//! The `trust/` folder a K2 server writes beside its statements:
//!
//! | Path | What |
//! |---|---|
//! | `nodes/<keyid>.pem` | each node's public key (for stock DSSE tools); `<keyid>` is the key's fingerprint, the envelope's `keyid` |
//! | `statements/<sha256>.json` | the nodes' signed self-statements, `{body, sig}` |
//! | `owners.json` | `[{id, rosterKeyFp, rosterSeq}]` |
//! | `rosters/<rosterKeyFp>.json` | each owner's roster as the server held it, `{body, sig}` |
//!
//! Nothing in it is trusted for being there: rosters verify against the
//! pinned owner key, self-statements against the roster's node key, and a
//! node PEM must be the roster's key.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::dsse::NodeKey;
use crate::roster::Signed;
use crate::util::{is_fp, sha256_hex};

/// One row of `owners.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct OwnerRow {
    pub id: String,
    pub roster_key_fp: String,
    #[serde(default)]
    pub roster_seq: Option<u64>,
}

/// A `trust/` folder on disk.
#[derive(Debug, Clone)]
pub struct TrustDir {
    root: PathBuf,
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl TrustDir {
    /// A folder that must exist.
    pub fn open(root: &Path) -> Result<Self, String> {
        if !root.is_dir() {
            return Err(format!("{}: not a folder", root.display()));
        }
        Ok(Self { root: root.to_path_buf() })
    }

    /// The folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The owners the server listed (informational: the pin decides).
    pub fn owners(&self) -> Result<Vec<OwnerRow>, String> {
        let p = self.root.join("owners.json");
        match read(&p)? {
            Some(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
            None => Ok(Vec::new()),
        }
    }

    /// The roster file for an owner key, if the folder has one.
    pub fn roster(&self, owner_fp: &str) -> Result<Option<Signed>, String> {
        if !is_fp(owner_fp) {
            return Err("an owner key fingerprint is 64 lowercase hex".into());
        }
        let p = self.root.join("rosters").join(format!("{owner_fp}.json"));
        read(&p)?.map(|t| Signed::from_json(&p.display().to_string(), &t)).transpose()
    }

    /// The node key PEM for a fingerprint, if the folder has one; refused
    /// when it isn't the key its file name says.
    pub fn node_key(&self, fp: &str) -> Result<Option<NodeKey>, String> {
        if !is_fp(fp) {
            return Ok(None);
        }
        let p = self.root.join("nodes").join(format!("{fp}.pem"));
        let Some(pem) = read(&p)? else {
            return Ok(None);
        };
        let key = NodeKey::from_pem(&pem).map_err(|e| format!("{}: {e}", p.display()))?;
        if key.fingerprint() != fp {
            return Err(format!("{}: the key in it has fingerprint {}", p.display(), key.fingerprint()));
        }
        Ok(Some(key))
    }

    /// A self-statement by its digest (`sha256:<hex>`), if the folder has
    /// it; refused when its body isn't that digest.
    pub fn self_statement(&self, digest: &str) -> Result<Option<Signed>, String> {
        let Some(hex) = digest.strip_prefix("sha256:").filter(|h| is_fp(h)) else {
            return Ok(None);
        };
        let p = self.root.join("statements").join(format!("{hex}.json"));
        let Some(text) = read(&p)? else {
            return Ok(None);
        };
        let s = Signed::from_json(&p.display().to_string(), &text)?;
        if sha256_hex(s.body.as_bytes()) != hex {
            return Err(format!("{}: its body does not hash to its name", p.display()));
        }
        Ok(Some(s))
    }
}

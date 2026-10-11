//! The executor's root-owned state (prd-k2-node-admin-v1 §7.3, §12.1):
//! the single-use nonce store and the hash-chained audit log. Both live
//! in the state dir (`/var/lib/k2-node-admin`, `/var/db/k2-node-admin`;
//! root 0700), which jobs can neither read nor write.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use k2_node_proto::admin::CLOCK_SKEW_SECS;
use k2_node_proto::crypto;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most remembered nonces (a full store refuses: never forgets early).
pub const MAX_NONCES: usize = 100_000;

/// `nonces.json`: nonce → exp. Entries are dropped once `exp + skew` has
/// passed (the command can't verify any more after that).
pub struct NonceStore {
    path: PathBuf,
    seen: BTreeMap<String, i64>,
}

impl NonceStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let seen = match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        Ok(Self { path: path.to_path_buf(), seen })
    }

    pub fn seen(&self, nonce: &str) -> bool {
        self.seen.contains_key(nonce)
    }

    /// Record a nonce durably (fsync + rename) before the command runs.
    pub fn insert(&mut self, nonce: &str, exp: i64, now: i64) -> Result<(), String> {
        self.seen.retain(|_, e| *e + CLOCK_SKEW_SECS >= now);
        if self.seen.len() >= MAX_NONCES {
            return Err("nonce store is full".into());
        }
        self.seen.insert(nonce.to_string(), exp);
        let b = serde_json::to_vec(&self.seen).map_err(|e| e.to_string())?;
        crate::util::atomic_write(&self.path, &b, 0o600)
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// One audit line. `hash = sha256(prev ‖ 0 ‖ canonical(line without hash))`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLine {
    pub seq: u64,
    pub at: i64,
    pub prev: String,
    /// `exec` (written before running) | `result` | `refused` | `update` |
    /// `keys` | `level` | `local`.
    pub event: String,
    #[serde(default)]
    pub cmd_id: Option<String>,
    #[serde(default)]
    pub verb: Option<String>,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub proposed_by: Option<String>,
    #[serde(default)]
    pub approved_by: Option<String>,
    #[serde(default)]
    pub approval_key: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub detail: Value,
    pub hash: String,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn line_hash(l: &AuditLine) -> Result<String, String> {
    let mut v = serde_json::to_value(l).map_err(|e| e.to_string())?;
    if let Value::Object(m) = &mut v {
        m.remove("hash");
    }
    let body = k2_node_proto::canonical::to_vec(&v)?;
    let mut buf = l.prev.as_bytes().to_vec();
    buf.push(0);
    buf.extend_from_slice(&body);
    Ok(crypto::sha256_hex(&buf))
}

/// Fields a caller fills; seq/at/prev/hash come from the log.
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    pub event: String,
    pub cmd_id: Option<String>,
    pub verb: Option<String>,
    pub class: Option<String>,
    pub digest: Option<String>,
    pub proposed_by: Option<String>,
    pub approved_by: Option<String>,
    pub approval_key: Option<String>,
    pub code: Option<String>,
    pub detail: Value,
}

pub struct Audit {
    path: PathBuf,
}

impl Audit {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    /// The last line (seq, hash), or (0, genesis).
    fn tail(&self) -> Result<(u64, String), String> {
        let f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, GENESIS.into())),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        let mut last: Option<AuditLine> = None;
        for line in std::io::BufReader::new(f).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            last = Some(serde_json::from_str(&line).map_err(|e| format!("audit line unreadable: {e}"))?);
        }
        Ok(last.map(|l| (l.seq, l.hash)).unwrap_or((0, GENESIS.into())))
    }

    /// Append one line (fsync). Returns its seq.
    pub fn append(&self, e: AuditEntry, at: i64) -> Result<u64, String> {
        let (seq, prev) = self.tail()?;
        let mut l = AuditLine {
            seq: seq + 1,
            at,
            prev,
            event: e.event,
            cmd_id: e.cmd_id,
            verb: e.verb,
            class: e.class,
            digest: e.digest,
            proposed_by: e.proposed_by,
            approved_by: e.approved_by,
            approval_key: e.approval_key,
            code: e.code,
            detail: e.detail,
            hash: String::new(),
        };
        l.hash = line_hash(&l)?;
        let mut text = serde_json::to_string(&l).map_err(|e| e.to_string())?;
        text.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("open {}: {e}", self.path.display()))?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        f.write_all(text.as_bytes()).map_err(|e| format!("write audit: {e}"))?;
        f.sync_all().map_err(|e| format!("sync audit: {e}"))?;
        Ok(l.seq)
    }

    /// Every line, after checking the whole chain. `Err` names the first
    /// broken line.
    pub fn verify(&self) -> Result<Vec<AuditLine>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        let mut prev = GENESIS.to_string();
        let mut out = Vec::new();
        for (i, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let l: AuditLine = serde_json::from_str(line).map_err(|e| format!("line {}: unreadable: {e}", i + 1))?;
            if l.seq != i as u64 + 1 || l.prev != prev || line_hash(&l)? != l.hash {
                return Err(format!("line {}: chain broken", i + 1));
            }
            prev = l.hash.clone();
            out.push(l);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_are_single_use_survive_restart_and_expire() {
        let d = crate::util::temp_dir("nonces");
        let p = d.join("nonces.json");
        let mut s = NonceStore::open(&p).unwrap();
        assert!(!s.seen("a"));
        s.insert("a", 1000, 900).unwrap();
        assert!(s.seen("a"));
        let s2 = NonceStore::open(&p).unwrap();
        assert!(s2.seen("a"), "survives a restart");
        let mut s3 = NonceStore::open(&p).unwrap();
        s3.insert("b", 5000, 1000 + CLOCK_SKEW_SECS + 1).unwrap();
        assert!(!s3.seen("a"), "pruned only after exp + skew");
        assert!(s3.seen("b"));
    }

    #[test]
    fn audit_chain_verifies_and_tamper_is_found() {
        let d = crate::util::temp_dir("audit");
        let a = Audit::new(&d.join("audit.log"));
        assert!(a.verify().unwrap().is_empty());
        for i in 0..3 {
            let seq = a
                .append(AuditEntry { event: "exec".into(), cmd_id: Some(format!("c{i}")), detail: serde_json::json!({"n": i}), ..Default::default() }, 100 + i)
                .unwrap();
            assert_eq!(seq, i as u64 + 1);
        }
        let lines = a.verify().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].prev, lines[0].hash);
        let text = std::fs::read_to_string(d.join("audit.log")).unwrap();
        std::fs::write(d.join("audit.log"), text.replace("\"c1\"", "\"cX\"")).unwrap();
        assert_eq!(a.verify().unwrap_err(), "line 2: chain broken");
        // Dropping a line breaks it too.
        let kept: Vec<&str> = text.lines().enumerate().filter(|(i, _)| *i != 0).map(|(_, l)| l).collect();
        std::fs::write(d.join("audit.log"), kept.join("\n")).unwrap();
        assert!(a.verify().is_err());
    }
}

//! `/etc/k2-node/queue.toml`: the node owner's queue decisions
//! (prd-compute-job-tiers-v1 §3.3, vs-live T8), in their own file because
//! `policy.toml` and `servers.toml` reject unknown keys: a new key there
//! would make a node rolled back to 0.46.4 fail to read its own config.
//! Older builds never read this file. This build reads it leniently
//! (unknown keys are ignored), so a newer build's keys never break it.
//!
//! ```toml
//! [servers.<fp_prefix>]   # the first 16 hex of the server's key
//! priority = true         # its Priority counts here
//! ```
//!
//! Defaults: the owner server's Priority counts, a guest's doesn't (it
//! runs as Normal; the server tells its submitter, T7). Written by root's
//! executor (`servers.limits … priority=…`, a widening needs the owner's
//! approval) or the machine owner; **never by the node**, which only reads
//! it (the servers loop, every pass).
//!
//! 0.46.6 adds the Batch window keys here (cut from 0.46.5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_node_proto::share::{ServerEntry, OWNER_SLOT};
use serde::{Deserialize, Serialize};

pub const QUEUE_FILE: &str = "queue.toml";

/// The whole file. Absent = every default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueFile {
    /// Per server, keyed by the first 16 hex of its key fingerprint (a
    /// slot is reused by other servers, a key prefix isn't).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, ServerQueue>,
}

/// One server's queue settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerQueue {
    /// Its Priority counts on this node. Absent = the default (owner yes,
    /// guests no).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<bool>,
}

/// A line's key: the first 16 hex of its fingerprint (the owner's line
/// carries the full 64).
pub fn key_of(e: &ServerEntry) -> String {
    e.fp_prefix.chars().take(16).collect()
}

fn valid_key(k: &str) -> bool {
    k.len() == 16 && k.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl QueueFile {
    /// Does `e`'s Priority count here?
    pub fn priority(&self, e: &ServerEntry) -> bool {
        self.servers.get(&key_of(e)).and_then(|s| s.priority).unwrap_or(e.slot == OWNER_SLOT)
    }

    /// Set `e`'s Priority word (a value equal to the default is still
    /// written down: the owner said it).
    pub fn set_priority(&mut self, e: &ServerEntry, on: bool) {
        self.servers.entry(key_of(e)).or_default().priority = Some(on);
    }

    /// Keys are 16 lowercase hex.
    pub fn validate(&self) -> Result<(), String> {
        match self.servers.keys().find(|k| !valid_key(k)) {
            Some(k) => Err(format!("queue.toml: [servers.{k}] isn't a 16-hex key prefix")),
            None => Ok(()),
        }
    }
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(QUEUE_FILE)
}

/// Read and check the file; absent = defaults. Never blocks on a planted
/// FIFO (the same reader as `servers.toml`).
pub fn read(config_dir: &Path) -> Result<QueueFile, String> {
    let p = path(config_dir);
    let text = match crate::util::read_regular(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(QueueFile::default()),
        Err(e) => return Err(format!("read {}: {e}", p.display())),
    };
    // The parse error quotes the file: logged here, never returned.
    let f: QueueFile = toml::from_str(&text).map_err(|e| {
        crate::nlog!("{}: {e}", p.display());
        format!("{} doesn't parse (see the k2-node log)", p.display())
    })?;
    f.validate()?;
    Ok(f)
}

/// The file's text as written (a header, then the settings).
pub fn render(f: &QueueFile) -> Result<String, String> {
    f.validate()?;
    let body = toml::to_string(f).map_err(|e| format!("encode queue.toml: {e}"))?;
    Ok(format!(
        "# K2 node queue settings (prd-compute-job-tiers-v1): the node owner's decisions only.\n\
         # Change them with `k2 compute node servers <node> limits <server> priority=true|false` from the owner server.\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};

    fn file() -> ServersFile {
        let mut f = ServersFile::new(&"a".repeat(64), "owner", ShareLimits::owner(2, 8, 16, 100, Network::On), 3, 1, "local:t");
        let mut g = f.servers[0].clone();
        g.slot = 1;
        g.label = "guest".into();
        g.fp_prefix = "0123456789abcdef".into();
        g.trust = Trust::Team;
        f.servers.push(g);
        f
    }

    #[test]
    fn defaults_owner_yes_guests_no_and_the_owners_word_wins() {
        let f = file();
        let (owner, guest) = (&f.servers[0], &f.servers[1]);
        let mut q = QueueFile::default();
        assert!(q.priority(owner));
        assert!(!q.priority(guest));
        q.set_priority(guest, true);
        q.set_priority(owner, false);
        assert!(q.priority(guest) && !q.priority(owner));
        assert_eq!(key_of(owner), "a".repeat(16), "the owner's full fingerprint keys by its first 16 hex");
        let text = render(&q).unwrap();
        assert!(text.contains("[servers.0123456789abcdef]") && text.contains("priority = true"), "{text}");
    }

    #[test]
    fn reads_absent_as_defaults_ignores_unknown_keys_and_refuses_bad_ones() {
        let d = crate::util::temp_dir("queue-file");
        assert_eq!(read(&d).unwrap(), QueueFile::default(), "no file = defaults");
        // A newer build's keys (0.46.6's Batch window) never break this one.
        std::fs::write(path(&d), "batch_when_idle = true\n[[batch_window]]\ndays = \"daily\"\nfrom = \"22:00\"\nto = \"07:00\"\n[servers.0123456789abcdef]\npriority = true\nlater = 1\n").unwrap();
        let q = read(&d).unwrap();
        assert!(q.priority(&file().servers[1]));
        std::fs::write(path(&d), "[servers.not-hex]\npriority = true\n").unwrap();
        assert!(read(&d).is_err(), "a key that isn't a 16-hex prefix is refused, never guessed");
        std::fs::write(path(&d), "[servers\n").unwrap();
        assert!(read(&d).is_err());
    }
}

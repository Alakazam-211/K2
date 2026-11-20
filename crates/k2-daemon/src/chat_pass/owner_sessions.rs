//! K2's own list of the owner chat sessions it opened.
//!
//! A redeemed owner handoff code ([`super::handoff`]) mints a `k2:owner`
//! pass with sid `own_<32 hex>`, and Sew turns that pass into an owner
//! session. K2 can't mint a second pass for that sid (the code is single
//! use), so the session lives until Sew's own expiry unless K2 ends it.
//! This record lets a person see those sessions and end them
//! (`k2 hostchat owner-sessions`, `/cli/sew/owner-sessions*`).
//!
//! **Where:** `~/.k2/sew/owner-sessions.json` (0600, written atomically),
//! beside K2's other small Sew records. No secret is in it: the sid is a
//! hash prefix of a spent code, and the minter is an actor stamp.
//!
//! **Size:** entries whose code was redeemed 30 days ago or more are
//! dropped on every write ([`KEEP_SECS`]) and hidden on every read.
//!
//! **Not complete by design:** a failed write is logged and the pass still
//! goes out. Ending every owner session (`SewSessionRevoke::ExternalId`
//! `k2:owner`) does not need this list, so an unlisted session can still
//! be ended.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Entries this old (redeemed this many seconds ago or more) are dropped.
pub const KEEP_SECS: i64 = 30 * 24 * 3600;

const FORMAT: u32 = 1;

/// How an end was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// This sid alone (`SewSessionRevoke::K2Sid`).
    Sid,
    /// Every owner session (`SewSessionRevoke::ExternalId("k2:owner")`).
    All,
}

/// An end K2 queued for one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct End {
    pub queued_at: i64,
    pub via: Via,
    /// Who asked (`owner` or `login:<username>`).
    pub by: String,
}

/// One owner session K2 opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `own_<first 32 hex of SHA-256(code)>`.
    pub sid: String,
    /// Who confirmed the code (actor stamp).
    pub minted_by: String,
    pub minted_at: i64,
    pub redeemed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<End>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileBody {
    format: u32,
    #[serde(default)]
    sessions: Vec<Entry>,
}

static LOCK: Mutex<()> = Mutex::new(());

/// `~/.k2/sew/owner-sessions.json`.
pub fn path() -> PathBuf {
    k2_core::paths::k2_home().join("sew").join("owner-sessions.json")
}

fn load() -> Result<Vec<Entry>, String> {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(raw) if raw.trim().is_empty() => Ok(Vec::new()),
        Ok(raw) => {
            let f: FileBody = serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", p.display()))?;
            if f.format != FORMAT {
                return Err(format!("{} has format {}, expected {FORMAT}", p.display(), f.format));
            }
            Ok(f.sessions)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

fn fresh(e: &Entry, now: i64) -> bool {
    now - e.redeemed_at < KEEP_SECS
}

/// Prunes, then writes. Every write goes through here.
fn save(mut sessions: Vec<Entry>, now: i64) -> Result<(), String> {
    sessions.retain(|e| fresh(e, now));
    let p = path();
    let dir = p.parent().ok_or("owner sessions path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let body = serde_json::to_vec_pretty(&FileBody { format: FORMAT, sessions }).map_err(|e| format!("serialize owner sessions: {e}"))?;
    k2_core::fs_atomic::atomic_write_private(&p, &body).map_err(|e| format!("write {}: {e}", p.display()))
}

fn locked<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    f()
}

/// Record a redeemed handoff code's session at `now` (its redeem time).
pub fn record_redeemed(sid: &str, minted_by: &str, minted_at: i64, now: i64) -> Result<(), String> {
    locked(|| {
        let mut s = load()?;
        s.retain(|e| e.sid != sid);
        s.push(Entry { sid: sid.to_string(), minted_by: minted_by.to_string(), minted_at, redeemed_at: now, end: None });
        save(s, now)
    })
}

/// The recorded sessions at `now`, newest redeem first.
pub fn list(now: i64) -> Result<Vec<Entry>, String> {
    let mut s = locked(load)?;
    s.retain(|e| fresh(e, now));
    s.sort_by(|a, b| b.redeemed_at.cmp(&a.redeemed_at).then_with(|| a.sid.cmp(&b.sid)));
    Ok(s)
}

/// One recorded session at `now`.
pub fn get(sid: &str, now: i64) -> Result<Option<Entry>, String> {
    Ok(list(now)?.into_iter().find(|e| e.sid == sid))
}

/// Mark an end queued. `Some(sid)`: that session (an earlier end is
/// replaced). `None` (`--all`): every session with no end yet. Returns the
/// sids marked.
pub fn mark_end(sid: Option<&str>, by: &str, now: i64) -> Result<Vec<String>, String> {
    locked(|| {
        let mut s = load()?;
        let mut marked = Vec::new();
        for e in s.iter_mut().filter(|e| fresh(e, now)) {
            let hit = match sid {
                Some(want) => e.sid == want,
                None => e.end.is_none(),
            };
            if hit {
                let via = if sid.is_some() { Via::Sid } else { Via::All };
                e.end = Some(End { queued_at: now, via, by: by.to_string() });
                marked.push(e.sid.clone());
            }
        }
        save(s, now)?;
        Ok(marked)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempHome;

    const DAY: i64 = 24 * 3600;
    const T0: i64 = 1_800_000_000;

    #[test]
    fn a_record_holds_no_secret_and_is_private() {
        let _home = TempHome::new();
        record_redeemed("own_0123456789abcdef0123456789abcdef", "owner", T0 - 10, T0).expect("record");
        let raw = std::fs::read_to_string(path()).expect("written");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(v["format"], 1);
        assert_eq!(
            v["sessions"][0],
            serde_json::json!({"sid": "own_0123456789abcdef0123456789abcdef", "mintedBy": "owner", "mintedAt": T0 - 10, "redeemedAt": T0})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path()).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn every_write_drops_entries_30_days_old() {
        let _home = TempHome::new();
        record_redeemed("own_old", "owner", T0 - 31 * DAY, T0 - 31 * DAY).expect("old");
        record_redeemed("own_edge", "owner", T0 - 30 * DAY, T0 - 30 * DAY).expect("edge");
        record_redeemed("own_young", "owner", T0 - 29 * DAY, T0 - 29 * DAY).expect("young");
        // The read hides what is due to go even before a write.
        let sids: Vec<String> = list(T0).expect("list").into_iter().map(|e| e.sid).collect();
        assert_eq!(sids, vec!["own_young".to_string()]);
        // The file still has them until the next write ...
        assert_eq!(load().expect("load").len(), 3);
        // ... which drops them.
        record_redeemed("own_new", "login:ada", T0 - 5, T0).expect("new");
        let on_disk: Vec<String> = load().expect("load").into_iter().map(|e| e.sid).collect();
        assert_eq!(on_disk, vec!["own_young".to_string(), "own_new".to_string()]);
        let listed: Vec<String> = list(T0).expect("list").into_iter().map(|e| e.sid).collect();
        assert_eq!(listed, vec!["own_new".to_string(), "own_young".to_string()], "newest first");
    }

    #[test]
    fn ends_mark_one_sid_or_every_unended_one() {
        let _home = TempHome::new();
        for (i, sid) in ["own_a", "own_b", "own_c"].iter().enumerate() {
            record_redeemed(sid, "owner", T0 + i as i64, T0 + i as i64).expect("record");
        }
        assert_eq!(mark_end(Some("own_b"), "owner", T0 + 10).expect("one"), vec!["own_b".to_string()]);
        assert_eq!(mark_end(Some("own_zzz"), "owner", T0 + 10).expect("none"), Vec::<String>::new());
        let mut all = mark_end(None, "login:ada", T0 + 20).expect("all");
        all.sort();
        assert_eq!(all, vec!["own_a".to_string(), "own_c".to_string()], "--all leaves an earlier end alone");
        let b = get("own_b", T0 + 20).expect("get").expect("own_b");
        assert_eq!(b.end, Some(End { queued_at: T0 + 10, via: Via::Sid, by: "owner".into() }));
        let a = get("own_a", T0 + 20).expect("get").expect("own_a");
        assert_eq!(a.end, Some(End { queued_at: T0 + 20, via: Via::All, by: "login:ada".into() }));
    }

    #[test]
    fn a_damaged_file_is_an_error_never_an_empty_list() {
        let _home = TempHome::new();
        std::fs::create_dir_all(path().parent().unwrap()).unwrap();
        std::fs::write(path(), "{not json").unwrap();
        assert!(list(T0).is_err());
        assert!(record_redeemed("own_a", "owner", T0, T0).is_err(), "a write never replaces a file it can't read");
        std::fs::write(path(), r#"{"format":2,"sessions":[]}"#).unwrap();
        assert!(list(T0).unwrap_err().contains("format 2"));
    }
}

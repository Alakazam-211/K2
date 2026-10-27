//! Chunked transactions (contract §3.5, §4.5; PRD §5.3.1, BG4). C0 → B2.
//!
//! `begin → parts (≤ 64 ops, ≤ 60 KiB each) → commit`, ≤ 1,000 ops and
//! ≤ 1 MiB per commit. Parts are staged here in daemon memory, invisible to
//! readers, bound to their caller and one space, and dropped after
//! `TX_IDLE_SECS` with no part. Grants are checked per part by the route
//! and again at commit. Pure: the clock is passed in.
//!
//! **Ids.** A `txId` is 32 hex chars from the OS RNG. Its first 8 are this
//! table's epoch (random per daemon start), so an id from before a restart
//! answers `tx_expired` (contract §4.5: a restart drops every open
//! transaction) while a made-up or foreign id answers `bad_tx`. Ids that
//! timed out are remembered for [`EXPIRED_MEMORY_SECS`] so their caller
//! hears `tx_expired`, not `bad_tx`.
//!
//! **Failing parts drop the transaction** (PRD §5.3.1): a part over the
//! per-part caps, a commit total over 1,000 ops / 1 MiB, or a key that was
//! already staged (a commit writes each key once, at one rev). Staging-cap
//! refusals (`busy`) keep the transaction.

use std::collections::{HashMap, HashSet};

use super::backend::Op;
use super::{
    keys, Access, DataError, Space, MAX_BATCH_BYTES, MAX_BATCH_OPS, MAX_TX_BYTES, MAX_TX_OPS, MAX_TX_PER_AGENT,
    MAX_TX_PER_PLACEMENT, MAX_TX_STAGED_BYTES, MAX_VALUE_BYTES, TX_IDLE_SECS,
};

/// How long a timed-out id still answers `tx_expired` (then `bad_tx`).
pub const EXPIRED_MEMORY_SECS: i64 = 600;

/// At most this many timed-out ids are remembered.
const EXPIRED_MEMORY_MAX: usize = 1024;

/// Who may use a transaction id. Anyone else presenting it gets `bad_tx`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TxOwner {
    /// A widget call: the placement and the live hash it ran under.
    Placement { garden: String, placement: String, hash: String },
    /// An agent (or K2 shell tab), by workspace.
    Agent { project_id: String },
    /// The person (the owner token outside K2, `k2 store batch`).
    Owner,
}

impl TxOwner {
    /// The bucket the open-transaction cap counts in: a placement (any
    /// hash), an agent, or the person.
    fn bucket(&self) -> (u8, String, String) {
        match self {
            TxOwner::Placement { garden, placement, .. } => (0, garden.clone(), placement.clone()),
            TxOwner::Agent { project_id } => (1, project_id.clone(), String::new()),
            TxOwner::Owner => (2, String::new(), String::new()),
        }
    }

    fn cap(&self) -> usize {
        match self {
            TxOwner::Placement { .. } => MAX_TX_PER_PLACEMENT,
            TxOwner::Agent { .. } | TxOwner::Owner => MAX_TX_PER_AGENT,
        }
    }
}

/// One open transaction's running totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxStatus {
    pub ops: usize,
    pub bytes: usize,
    pub expires_at: i64,
}

struct Staged {
    owner: TxOwner,
    space: Space,
    ops: Vec<Op>,
    keys: HashSet<String>,
    bytes: usize,
    last_at: i64,
}

impl Staged {
    fn expires_at(&self) -> i64 {
        self.last_at + TX_IDLE_SECS
    }

    fn idle(&self, now: i64) -> bool {
        now > self.expires_at()
    }
}

/// Every open transaction on this daemon.
pub struct TxTable {
    epoch: String,
    open: HashMap<String, Staged>,
    /// Timed-out ids: id → (owner, when it expired).
    expired: HashMap<String, (TxOwner, i64)>,
}

impl Default for TxTable {
    fn default() -> Self {
        TxTable::new()
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    // The OS RNG never fails on the platforms K2 runs on; if it ever did,
    // an all-zero id is still bound to its owner (and to this epoch).
    if let Err(e) = getrandom::getrandom(&mut buf) {
        crate::log_debug!("[garden_data/tx] OS RNG failed: {e}");
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl TxTable {
    pub fn new() -> TxTable {
        TxTable::with_epoch(random_hex(4))
    }

    /// A table with a fixed epoch (8 hex chars), for tests.
    pub fn with_epoch(epoch: String) -> TxTable {
        assert_eq!(epoch.len(), 8, "a tx epoch is 8 hex chars");
        TxTable { epoch, open: HashMap::new(), expired: HashMap::new() }
    }

    /// Open transactions (tests, diagnostics).
    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    fn staged_bytes(&self) -> usize {
        self.open.values().map(|s| s.bytes).sum()
    }

    fn remember_expired(&mut self, id: String, owner: TxOwner, now: i64) {
        self.expired.retain(|_, (_, at)| now - *at <= EXPIRED_MEMORY_SECS);
        if self.expired.len() >= EXPIRED_MEMORY_MAX {
            if let Some(oldest) = self.expired.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) {
                self.expired.remove(&oldest);
            }
        }
        self.expired.insert(id, (owner, now));
    }

    /// Why `tx_id` isn't open for `owner`: `tx_expired` for its own timed-out
    /// id or an id from before a restart, else `bad_tx`.
    fn missing(&self, tx_id: &str, owner: &TxOwner, now: i64) -> DataError {
        if let Some((o, at)) = self.expired.get(tx_id) {
            if o == owner && now - *at <= EXPIRED_MEMORY_SECS {
                return DataError::TxExpired;
            }
            return DataError::BadTx;
        }
        if valid_id(tx_id) && !tx_id.starts_with(&self.epoch) {
            return DataError::TxExpired;
        }
        DataError::BadTx
    }

    /// The open transaction `tx_id` of `owner`, after the idle check (an
    /// idle one is dropped and answers `tx_expired`).
    fn live(&mut self, tx_id: &str, owner: &TxOwner, now: i64) -> Result<&mut Staged, DataError> {
        match self.open.get(tx_id) {
            None => return Err(self.missing(tx_id, owner, now)),
            Some(s) if &s.owner != owner => return Err(DataError::BadTx),
            Some(s) if s.idle(now) => {
                let s = self.open.remove(tx_id).expect("present");
                self.remember_expired(tx_id.to_string(), s.owner, now);
                return Err(DataError::TxExpired);
            }
            Some(_) => {}
        }
        Ok(self.open.get_mut(tx_id).expect("present"))
    }

    /// Open one. Answers `(txId, expiresAt)`; `txId` is 32 hex chars
    /// (this table's 32-bit epoch, then 96 random bits). Caps:
    /// `MAX_TX_PER_PLACEMENT`, `MAX_TX_PER_AGENT` (→ `busy`).
    pub fn begin(&mut self, owner: TxOwner, space: Space, now: i64) -> Result<(String, i64), DataError> {
        self.expire(now);
        let bucket = owner.bucket();
        let open = self.open.values().filter(|s| s.owner.bucket() == bucket).count();
        if open >= owner.cap() {
            return Err(DataError::Busy);
        }
        let mut id = format!("{}{}", self.epoch, random_hex(12));
        while self.open.contains_key(&id) || self.expired.contains_key(&id) {
            id = format!("{}{}", self.epoch, random_hex(12));
        }
        let staged = Staged { owner, space, ops: Vec::new(), keys: HashSet::new(), bytes: 0, last_at: now };
        let expires_at = staged.expires_at();
        self.open.insert(id.clone(), staged);
        Ok((id, expires_at))
    }

    /// The space a transaction writes (the route checks grants against it
    /// before [`TxTable::add`]). Same refusals as `add`, without staging.
    pub fn space_of(&mut self, tx_id: &str, owner: &TxOwner, now: i64) -> Result<Space, DataError> {
        Ok(self.live(tx_id, owner, now)?.space.clone())
    }

    /// Stage one part (the route already checked keys, sizes and grants).
    pub fn add(&mut self, tx_id: &str, owner: &TxOwner, ops: Vec<Op>, now: i64) -> Result<TxStatus, DataError> {
        let staged_elsewhere = self.staged_bytes();
        let s = self.live(tx_id, owner, now)?;
        let part_bytes: usize = ops.iter().map(Op::value_bytes).fold(0usize, usize::saturating_add);
        let failure = if ops.is_empty() {
            Some(DataError::BadRequest("a transaction part has at least one op".into()))
        } else if ops.len() > MAX_BATCH_OPS || part_bytes > MAX_BATCH_BYTES {
            Some(DataError::TooLarge(format!(
                "a transaction part is at most {MAX_BATCH_OPS} ops and {} KiB of values",
                MAX_BATCH_BYTES / 1024
            )))
        } else if s.ops.len() + ops.len() > MAX_TX_OPS || s.bytes.saturating_add(part_bytes) > MAX_TX_BYTES {
            Some(DataError::TooLarge(format!(
                "a transaction is at most {MAX_TX_OPS} ops and {} MiB of values",
                MAX_TX_BYTES / (1024 * 1024)
            )))
        } else {
            let mut seen = HashSet::new();
            ops.iter()
                .map(Op::key)
                .find(|k| s.keys.contains(*k) || !seen.insert(*k))
                .map(|k| DataError::BadRequest(format!("'{k}' is in this transaction twice; a commit writes each key once")))
        };
        if let Some(e) = failure {
            self.open.remove(tx_id);
            return Err(e);
        }
        if staged_elsewhere.saturating_add(part_bytes) > MAX_TX_STAGED_BYTES {
            return Err(DataError::Busy);
        }
        let s = self.open.get_mut(tx_id).expect("live");
        for op in &ops {
            s.keys.insert(op.key().to_string());
        }
        s.ops.extend(ops);
        s.bytes += part_bytes;
        s.last_at = now;
        Ok(TxStatus { ops: s.ops.len(), bytes: s.bytes, expires_at: s.expires_at() })
    }

    /// Remove and hand back the staged ops for commit. The transaction is
    /// gone whether or not the commit then succeeds.
    pub fn take(&mut self, tx_id: &str, owner: &TxOwner, now: i64) -> Result<(Space, Vec<Op>), DataError> {
        self.live(tx_id, owner, now)?;
        let s = self.open.remove(tx_id).expect("live");
        if s.ops.is_empty() {
            return Err(DataError::BadRequest("the transaction has no ops to commit".into()));
        }
        Ok((s.space, s.ops))
    }

    /// Drop one. True when it existed and belonged to `owner`.
    pub fn abort(&mut self, tx_id: &str, owner: &TxOwner) -> bool {
        match self.open.get(tx_id) {
            Some(s) if &s.owner == owner => {
                self.open.remove(tx_id);
                true
            }
            _ => false,
        }
    }

    /// Drop everything idle past `TX_IDLE_SECS`. Answers how many.
    pub fn expire(&mut self, now: i64) -> usize {
        let idle: Vec<String> = self.open.iter().filter(|(_, s)| s.idle(now)).map(|(k, _)| k.clone()).collect();
        for id in &idle {
            if let Some(s) = self.open.remove(id) {
                self.remember_expired(id.clone(), s.owner, now);
            }
        }
        idle.len()
    }
}

/// Check one batch or transaction part before it is staged or written:
/// 1..=64 ops, ≤ 60 KiB of values, every key's syntax, every value ≤
/// 60 KiB, and every key writable under `access` (the caller's grants, or
/// a widget maker's rights). Pure.
pub fn check_part(ops: &[Op], access: &Access) -> Result<(), DataError> {
    if ops.is_empty() {
        return Err(DataError::BadRequest("a batch or transaction part has at least one op".into()));
    }
    if ops.len() > MAX_BATCH_OPS {
        return Err(DataError::TooLarge(format!("a batch or transaction part is at most {MAX_BATCH_OPS} ops")));
    }
    let mut total = 0usize;
    for op in ops {
        keys::validate_key(op.key())?;
        let bytes = op.value_bytes();
        if bytes > MAX_VALUE_BYTES {
            return Err(DataError::ValueTooLarge { bytes });
        }
        total = total.saturating_add(bytes);
    }
    if total > MAX_BATCH_BYTES {
        return Err(DataError::TooLarge(format!(
            "a batch or transaction part is at most {} KiB of values",
            MAX_BATCH_BYTES / 1024
        )));
    }
    check_writable(ops, access)
}

/// Every op's key is writable under `access` (`not_granted` names the
/// first that isn't). A commit runs this again with the rights read now.
pub fn check_writable(ops: &[Op], access: &Access) -> Result<(), DataError> {
    match ops.iter().find(|op| !access.can_write(op.key())) {
        Some(op) => Err(DataError::NotGranted { key: Some(op.key().to_string()) }),
        None => Ok(()),
    }
}

/// Values' bytes in a set of ops (the commit byte budget).
pub fn ops_bytes(ops: &[Op]) -> usize {
    ops.iter().map(Op::value_bytes).fold(0usize, usize::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_part_is_checked_for_size_keys_values_and_grants() {
        use crate::garden_data::{EffectiveGrant, GrantFrom, Level};
        let full = Access::Full;
        assert!(check_part(&puts(0, 64), &full).is_ok());
        assert_eq!(check_part(&[], &full).unwrap_err().code(), "bad_request");
        assert_eq!(check_part(&puts(0, 65), &full).unwrap_err().code(), "too_large");
        let bad_key = vec![Op::Delete { key: "/x".into(), if_rev: None }];
        assert_eq!(check_part(&bad_key, &full).unwrap_err().code(), "bad_key");
        let big = vec![Op::Put { key: "k".into(), value: json!("x".repeat(MAX_VALUE_BYTES)), if_rev: None }];
        assert_eq!(check_part(&big, &full).unwrap_err().code(), "value_too_large");
        let two = vec![
            Op::Put { key: "a".into(), value: json!("x".repeat(40 * 1024)), if_rev: None },
            Op::Put { key: "b".into(), value: json!("x".repeat(40 * 1024)), if_rev: None },
        ];
        assert_eq!(check_part(&two, &full).unwrap_err().code(), "too_large");
        let grants = Access::Grants(vec![EffectiveGrant {
            prefix: "project/7/".into(),
            level: Level::Write,
            from: GrantFrom::Agent,
            admin: false,
            grant_id: 1,
        }]);
        let mixed = vec![
            Op::Delete { key: "project/7/a".into(), if_rev: None },
            Op::Delete { key: "project/8/a".into(), if_rev: None },
        ];
        assert_eq!(check_part(&mixed, &grants), Err(DataError::NotGranted { key: Some("project/8/a".into()) }));
        assert_eq!(check_part(&mixed[..1], &Access::None), Err(DataError::NotGranted { key: Some("project/7/a".into()) }));
        assert!(check_part(&mixed[..1], &grants).is_ok());
        assert_eq!(ops_bytes(&two), 2 * (40 * 1024 + 2));
    }

    /// 0.46.5: patch ops stage like puts (the partial counts toward the
    /// part's bytes) and commit with the rest, one rev or nothing.
    #[test]
    fn patch_ops_stage_and_commit_all_or_nothing() {
        use crate::garden_data::backend::StoreBackend;
        use crate::garden_data::sqlite::SqliteStore;
        use crate::garden_data::{Actor, Cursor};
        let dir = crate::test_env::unique_temp_path("gd-tx-patch").join("stores");
        let store = SqliteStore::with_clock(&dir, std::sync::Arc::new(|| 1_760_000_000));
        let s = space();
        let r1 = store.put(&s, "obj/1", &json!({"t": "a", "x": 1}), None, &Actor::Owner).expect("put");
        let patch = |k: &str, v: serde_json::Value, if_rev: Option<i64>| Op::Patch { key: k.into(), value: v, if_rev };
        assert!(check_part(&[patch("obj/1", json!({"x": 2}), Some(r1))], &Access::Full).is_ok());
        let big = vec![patch("k", json!({"v": "x".repeat(MAX_VALUE_BYTES)}), None)];
        assert_eq!(check_part(&big, &Access::Full).unwrap_err().code(), "value_too_large", "a partial obeys the value cap");
        assert_eq!(ops_bytes(&[patch("k", json!({"a": 1}), None)]), r#"{"a":1}"#.len());

        // Stage across two parts, then commit: one rev for every key.
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), s.clone(), 0).expect("begin");
        t.add(&id, &agent("w-1"), puts(10, 3), 1).expect("part 1");
        t.add(&id, &agent("w-1"), vec![patch("obj/1", json!({"x": 2, "t": null}), Some(r1))], 2).expect("part 2");
        let (_, ops) = t.take(&id, &agent("w-1"), 3).expect("take");
        let rev = store.apply(&s, &ops, &Actor::Agent { project_id: "w-1".into() }).expect("commit");
        assert_eq!(rev, r1 + 1);
        let e = store.get(&s, "obj/1").expect("get").expect("entry");
        assert_eq!((e.rev, e.value), (rev, json!({"x": 2})));
        let feed = store.changes(&s, &Cursor { since: r1, after: None }, "", None).expect("changes");
        assert_eq!(feed.items.len(), 4, "one change-log row per key");
        assert!(feed.items.iter().all(|c| c.rev == rev && c.by == "agent:w-1"));

        // A stale patch in the last part: the commit writes nothing at all.
        let (id, _) = t.begin(agent("w-1"), s.clone(), 4).expect("begin");
        t.add(&id, &agent("w-1"), puts(20, 2), 5).expect("part 1");
        t.add(&id, &agent("w-1"), vec![patch("obj/1", json!({"x": 3}), Some(r1))], 6).expect("part 2");
        let (_, ops) = t.take(&id, &agent("w-1"), 7).expect("take");
        assert_eq!(
            store.apply(&s, &ops, &Actor::Owner),
            Err(DataError::Conflict { key: "obj/1".into(), rev, op: Some(2) })
        );
        assert_eq!(store.info(&s).expect("info").expect("space").rev, rev, "no new rev");
        assert_eq!(store.get(&s, "obj/20").expect("get"), None, "the first part rolled back too");
        if let Some(root) = dir.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    fn agent(p: &str) -> TxOwner {
        TxOwner::Agent { project_id: p.into() }
    }

    fn place(p: &str) -> TxOwner {
        TxOwner::Placement { garden: "g-1".into(), placement: p.into(), hash: "h".repeat(64) }
    }

    fn space() -> Space {
        Space::data("board").expect("space")
    }

    fn puts(from: usize, n: usize) -> Vec<Op> {
        (from..from + n).map(|i| Op::Put { key: format!("obj/{i}"), value: json!({"i": i}), if_rev: None }).collect()
    }

    fn table() -> TxTable {
        TxTable::with_epoch("0badcafe".into())
    }

    #[test]
    fn a_thousand_ops_stage_in_parts_and_come_back_in_order() {
        let mut t = table();
        let (id, exp) = t.begin(agent("w-1"), space(), 100).expect("begin");
        assert_eq!(id.len(), 32);
        assert!(id.starts_with("0badcafe"));
        assert_eq!(exp, 100 + TX_IDLE_SECS);
        let mut last = None;
        for part in 0..16 {
            let n = if part == 15 { 1000 - 15 * 64 } else { 64 };
            last = Some(t.add(&id, &agent("w-1"), puts(part * 64, n), 101 + part as i64).expect("add"));
        }
        let st = last.expect("status");
        assert_eq!(st.ops, 1000);
        assert_eq!(st.expires_at, 116 + TX_IDLE_SECS, "each part pushes the idle timer");
        let (sp, ops) = t.take(&id, &agent("w-1"), 120).expect("take");
        assert_eq!(sp, space());
        assert_eq!(ops.len(), 1000);
        assert_eq!(ops[0].key(), "obj/0");
        assert_eq!(ops[999].key(), "obj/999");
        assert!(t.is_empty(), "take removes it");
        assert_eq!(t.take(&id, &agent("w-1"), 121), Err(DataError::BadTx), "a finished id is gone");
    }

    #[test]
    fn over_a_thousand_ops_drops_the_transaction() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        for part in 0..15 {
            t.add(&id, &agent("w-1"), puts(part * 64, 64), 1).expect("add");
        }
        assert_eq!(t.add(&id, &agent("w-1"), puts(960, 41), 2).map(|_| ()).unwrap_err().code(), "too_large");
        assert_eq!(t.add(&id, &agent("w-1"), puts(2000, 1), 3), Err(DataError::BadTx), "dropped");
    }

    #[test]
    fn a_part_over_its_caps_drops_the_transaction() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        assert_eq!(t.add(&id, &agent("w-1"), puts(0, 65), 1).unwrap_err().code(), "too_large");
        assert!(t.is_empty());
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        let big = vec![Op::Put { key: "big".into(), value: json!("x".repeat(MAX_BATCH_BYTES)), if_rev: None }];
        assert_eq!(t.add(&id, &agent("w-1"), big, 1).unwrap_err().code(), "too_large");
        assert!(t.is_empty());
    }

    #[test]
    fn a_megabyte_is_the_most_one_transaction_holds() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        let part = |n: usize| vec![Op::Put { key: format!("k/{n}"), value: json!("y".repeat(50 * 1024)), if_rev: None }];
        let mut n = 0;
        while n < 20 {
            t.add(&id, &agent("w-1"), part(n), 1).expect("under 1 MiB");
            n += 1;
        }
        assert_eq!(t.add(&id, &agent("w-1"), part(n), 1).unwrap_err().code(), "too_large");
        assert!(t.is_empty());
    }

    #[test]
    fn a_key_twice_drops_the_transaction() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        t.add(&id, &agent("w-1"), puts(0, 2), 1).expect("add");
        let again = vec![Op::Delete { key: "obj/1".into(), if_rev: None }];
        assert_eq!(t.add(&id, &agent("w-1"), again, 2).unwrap_err().code(), "bad_request");
        assert!(t.is_empty());
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        let twice = vec![Op::Delete { key: "a".into(), if_rev: None }, Op::Delete { key: "a".into(), if_rev: None }];
        assert_eq!(t.add(&id, &agent("w-1"), twice, 2).unwrap_err().code(), "bad_request");
    }

    #[test]
    fn foreign_and_made_up_ids_are_bad_tx() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        assert_eq!(t.add(&id, &agent("w-2"), puts(0, 1), 1), Err(DataError::BadTx));
        assert_eq!(t.add(&id, &place("p"), puts(0, 1), 1), Err(DataError::BadTx));
        assert_eq!(t.take(&id, &agent("w-2"), 1), Err(DataError::BadTx));
        assert!(!t.abort(&id, &agent("w-2")));
        assert_eq!(t.len(), 1, "a foreign caller never drops it");
        assert_eq!(t.add("0badcafe000000000000000000000000", &agent("w-1"), puts(0, 1), 1), Err(DataError::BadTx));
        assert_eq!(t.add("not-an-id", &agent("w-1"), puts(0, 1), 1), Err(DataError::BadTx));
        // A placement's id is bound to the hash it began under too.
        let (pid, _) = t.begin(place("p"), space(), 0).expect("begin");
        let other_hash = TxOwner::Placement { garden: "g-1".into(), placement: "p".into(), hash: "i".repeat(64) };
        assert_eq!(t.add(&pid, &other_hash, puts(0, 1), 1), Err(DataError::BadTx));
    }

    #[test]
    fn an_idle_transaction_expires_and_leaves_nothing() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        t.add(&id, &agent("w-1"), puts(0, 3), 10).expect("add");
        assert_eq!(t.add(&id, &agent("w-1"), puts(3, 1), 10 + TX_IDLE_SECS + 1), Err(DataError::TxExpired));
        assert!(t.is_empty());
        assert_eq!(t.take(&id, &agent("w-1"), 50), Err(DataError::TxExpired), "remembered as expired");
        assert_eq!(t.take(&id, &agent("w-2"), 50), Err(DataError::BadTx), "never tells another caller");
        // The sweeper expires idle ones too.
        let (a, _) = t.begin(agent("w-1"), space(), 100).expect("begin");
        let (_b, _) = t.begin(agent("w-1"), space(), 120).expect("begin");
        assert_eq!(t.expire(100 + TX_IDLE_SECS + 1), 1);
        assert_eq!(t.take(&a, &agent("w-1"), 140), Err(DataError::TxExpired));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn an_id_from_before_a_restart_is_expired() {
        let mut before = TxTable::with_epoch("11111111".into());
        let (id, _) = before.begin(agent("w-1"), space(), 0).expect("begin");
        let mut after = TxTable::with_epoch("22222222".into());
        assert_eq!(after.add(&id, &agent("w-1"), puts(0, 1), 1), Err(DataError::TxExpired));
    }

    #[test]
    fn open_transactions_are_capped_per_caller() {
        let mut t = table();
        for _ in 0..MAX_TX_PER_AGENT {
            t.begin(agent("w-1"), space(), 0).expect("begin");
        }
        assert_eq!(t.begin(agent("w-1"), space(), 0), Err(DataError::Busy));
        t.begin(agent("w-2"), space(), 0).expect("another agent has its own");
        for _ in 0..MAX_TX_PER_PLACEMENT {
            t.begin(place("p"), space(), 0).expect("begin");
        }
        let other_hash = TxOwner::Placement { garden: "g-1".into(), placement: "p".into(), hash: "i".repeat(64) };
        assert_eq!(t.begin(other_hash, space(), 0), Err(DataError::Busy), "counted per placement, any hash");
        t.begin(place("q"), space(), 0).expect("another placement");
        // Idle ones free their slot.
        t.begin(agent("w-1"), space(), TX_IDLE_SECS + 1).expect("expired ones don't count");
    }

    #[test]
    fn staged_bytes_in_all_are_capped_without_dropping() {
        let mut t = table();
        let big = |k: &str| vec![Op::Put { key: k.into(), value: json!("z".repeat(59 * 1024)), if_rev: None }];
        let mut ids = Vec::new();
        for a in 0..20 {
            for _ in 0..MAX_TX_PER_AGENT {
                ids.push((t.begin(agent(&format!("w-{a}")), space(), 0).expect("begin").0, a));
            }
        }
        let mut busy = None;
        'fill: for (id, a) in &ids {
            for n in 0..17 {
                match t.add(id, &agent(&format!("w-{a}")), big(&format!("k/{n}")), 1) {
                    Ok(_) => {}
                    Err(e) => {
                        busy = Some((id.clone(), *a, e));
                        break 'fill;
                    }
                }
            }
        }
        let (id, a, e) = busy.expect("16 MiB is reached");
        assert_eq!(e, DataError::Busy);
        assert!(t.space_of(&id, &agent(&format!("w-{a}")), 2).is_ok(), "busy keeps the transaction");
    }

    #[test]
    fn abort_and_empty_commits() {
        let mut t = table();
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        assert!(t.abort(&id, &agent("w-1")));
        assert!(!t.abort(&id, &agent("w-1")), "already gone");
        let (id, _) = t.begin(agent("w-1"), space(), 0).expect("begin");
        assert_eq!(t.take(&id, &agent("w-1"), 1).unwrap_err().code(), "bad_request");
        assert!(t.is_empty());
    }
}

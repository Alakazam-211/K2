//! Garden data audit (contract §10; PRD §6.4, GL43). Owner: B1.
//!
//! Data reads and writes are **counted**: one `zen.data.counts` line per
//! placement or agent per minute with activity. Grants, revokes,
//! delegations, space delete/move/import/export, refusals, pauses and
//! signing are logged one by one. Bodies, keys and values are never logged.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// One counted data call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    Read,
    Write { ops: usize, bytes: usize },
    Delete,
    Conflict,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub reads: u64,
    pub writes: u64,
    pub deletes: u64,
    pub ops: u64,
    pub bytes: u64,
    pub conflicts: u64,
}

type Key = (String, String, Option<String>);

fn counts() -> &'static Mutex<BTreeMap<Key, Counts>> {
    static C: OnceLock<Mutex<BTreeMap<Key, Counts>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Count one call for `actor` (audit label) on `space`. `made_by` is the
/// widget's maker for widget calls.
pub fn count(space: &str, actor: &str, made_by: Option<&str>, what: Counted) {
    let mut m = counts().lock().unwrap_or_else(|e| e.into_inner());
    let c = m.entry((space.to_string(), actor.to_string(), made_by.map(str::to_string))).or_default();
    match what {
        Counted::Read => c.reads += 1,
        Counted::Write { ops, bytes } => {
            c.writes += 1;
            c.ops += ops as u64;
            c.bytes += bytes as u64;
        }
        Counted::Delete => c.deletes += 1,
        Counted::Conflict => c.conflicts += 1,
    }
}

fn record(event: &str, actor: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, outcome, "local", "-", "cli"));
}

/// One line now (`zen.data.grant`, `zen.data.revoke`, refusals, …).
/// `detail` names ids, prefixes and codes, never values.
pub fn line(event: &str, actor: &str, detail: &str) {
    record(event, actor, detail.to_string());
}

/// Take the counts gathered since the last flush (and clear them).
pub fn take() -> Vec<(Key, Counts)> {
    std::mem::take(&mut *counts().lock().unwrap_or_else(|e| e.into_inner())).into_iter().collect()
}

/// Write the per-minute count lines (the ticker calls this).
pub fn flush() {
    for ((space, actor, made_by), c) in take() {
        let made = made_by.map(|m| format!(" madeBy={m}")).unwrap_or_default();
        record(
            "zen.data.counts",
            &actor,
            format!(
                "space={space}{made} reads={} writes={} deletes={} ops={} bytes={} conflicts={}",
                c.reads, c.writes, c.deletes, c.ops, c.bytes, c.conflicts
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_gather_per_space_actor_and_maker() {
        let space = format!("data:audit-{}", uuid::Uuid::new_v4().simple());
        count(&space, "agent:keeper", None, Counted::Read);
        count(&space, "agent:keeper", None, Counted::Write { ops: 3, bytes: 90 });
        count(&space, "agent:keeper", None, Counted::Delete);
        count(&space, "widget", Some("agent:builder"), Counted::Conflict);
        let mine: Vec<_> = take().into_iter().filter(|((s, _, _), _)| *s == space).collect();
        assert_eq!(mine.len(), 2);
        let keeper = &mine.iter().find(|((_, a, _), _)| a == "agent:keeper").unwrap().1;
        assert_eq!(
            *keeper,
            Counts { reads: 1, writes: 1, deletes: 1, ops: 3, bytes: 90, conflicts: 0 }
        );
    }
}

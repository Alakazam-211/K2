//! The parent walk over a process table: one process's facts, the lookup
//! trait, and the bounded walk up its parents.
//!
//! The daemon's hook owner check uses it; k2-core's `agent_hooks::owner`
//! re-exports all three at their old paths. The live table is
//! [`crate::proc_ancestry::LiveProcessTable`].

/// One process, as the owner check sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    /// Opaque, monotonic per boot: tells a reused pid apart.
    pub start_time: u64,
}

/// Process lookup. `None` when the pid is not alive.
pub trait ProcessTable {
    fn info(&self, pid: i32) -> Option<ProcInfo>;
}

/// `pid`'s chain upward, starting with `pid` itself, at most `max`
/// entries. Stops at a missing parent, pid ≤ 1, or a self-parent loop.
pub fn ancestry(table: &dyn ProcessTable, start: ProcInfo, max: usize) -> Vec<ProcInfo> {
    let mut chain = vec![start];
    let mut cur = start;
    while chain.len() < max && cur.ppid > 1 && cur.ppid != cur.pid {
        let Some(parent) = table.info(cur.ppid) else { break };
        if chain.iter().any(|p| p.pid == parent.pid) {
            break;
        }
        chain.push(parent);
        cur = parent;
    }
    chain
}

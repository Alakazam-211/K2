//! `cell_uid_pool` — P4-H6 per-session worker-uid allocator.
//!
//! ## Why this exists (the multi-tenant fs-isolation primitive)
//!
//! Through P4-H5 EVERY microVM cell's VMM dropped to the SAME shared `k2cell`
//! uid. That made two cells share a host identity: cell A's guest-written files
//! were owned by the same uid as cell B's, so a compromised cell A could read /
//! write cell B's workspace, and the H5 nft egress rule (`skuid k2cell`) was one
//! shared policy for all cells. H6 replaces the single shared uid with a DISTINCT
//! per-session uid drawn from this pool: each cell's VMM drops to its OWN uid, so
//! guest-written files are host-owned by a per-session uid (true cross-tenant fs
//! isolation, enforced by ownership/0700) and the egress rule becomes per-tenant
//! (per-session nft table, torn down per cell).
//!
//! ## The model (a daemon-owned RAW-UID-RANGE allocator — NO box provisioning)
//!
//! A free-list over a RESERVED high uid range (default `60000..60128`,
//! configurable via `K2_SANDBOX_UID_BASE` / `K2_SANDBOX_UID_COUNT`). We do NOT
//! create `/etc/passwd` entries: the worker runs as root pre-pivot and does
//! `setgroups([kvm_gid])` → `setgid(raw_uid)` → `setuid(raw_uid)` — `/dev/kvm`
//! access is via the kvm SUPPLEMENTARY group (independent of the uid), `chown`s
//! to a raw uid succeed as root, and `verify_drop`'s `setuid(0) → EPERM` holds
//! for ANY non-zero uid. So a bare numeric uid with no passwd row is a complete
//! drop identity.
//!
//! ## Fail-closed
//!
//! - The range floor is [`UID_FLOOR`]: a base at/below it (which would collide
//!   with root / system users) yields an EMPTY pool → [`alloc`] always `None` →
//!   the spawn door REFUSES to boot (it never falls back to a shared/colliding
//!   uid).
//! - [`alloc`] returns `None` when every uid is in use (a second concurrency
//!   bound on top of H4's cap) → the spawn door 503s rather than booting two
//!   cells under one uid.
//! - [`free`] is idempotent + range-checked (a stray/double free can't corrupt
//!   the bitmap or free an out-of-range uid).
//! - uid 0 can NEVER be produced (the floor is far above it) — defence in depth
//!   on top of the worker's own uid-0 refusal.

use std::sync::{LazyLock, Mutex};

/// Minimum allocatable uid. The reserved range MUST sit well above real users:
/// root is 0, system/service accounts are conventionally `< 1000`, and login
/// users start at `1000`. We refuse any base at or below this so a misconfigured
/// range can never hand out a uid that collides with a real account (which would
/// make the "drop" land on someone with host access). 60000 (the default base)
/// is far above it; this is the hard fail-closed floor regardless of config.
pub const UID_FLOOR: u32 = 30000;

/// Default base of the reserved range (overridable via `K2_SANDBOX_UID_BASE`).
const DEFAULT_UID_BASE: u32 = 60000;
/// Default count of the reserved range (overridable via `K2_SANDBOX_UID_COUNT`).
const DEFAULT_UID_COUNT: u32 = 128;

/// A pure free-list over a contiguous uid range. Unit-testable with no globals.
#[derive(Debug)]
pub struct UidPool {
    base: u32,
    /// `in_use[i]` ⇒ uid `base + i` is allocated. Length = the effective count
    /// (0 when the configured range is invalid → fail-closed empty).
    in_use: Vec<bool>,
}

impl UidPool {
    /// Build a pool over `[base, base + count)`, fail-closed to EMPTY when the
    /// range is invalid: base at/below [`UID_FLOOR`], zero count, or a range
    /// that would overflow `u32`. An empty pool's [`alloc`](Self::alloc) is
    /// always `None`, so a bad config can never hand out a colliding uid.
    pub fn new(base: u32, count: u32) -> Self {
        // Fail-closed: a base at/below the floor (root/system users) ⇒ empty.
        if base <= UID_FLOOR || count == 0 {
            return Self {
                base,
                in_use: Vec::new(),
            };
        }
        // Clamp the count so `base + count` never overflows u32.
        let headroom = u32::MAX - base;
        let effective = count.min(headroom);
        Self {
            base,
            in_use: vec![false; effective as usize],
        }
    }

    /// Allocate the lowest free uid, or `None` when the pool is exhausted (or
    /// empty). The returned uid is guaranteed `> UID_FLOOR` and `!= 0` by
    /// construction.
    pub fn alloc(&mut self) -> Option<u32> {
        let idx = self.in_use.iter().position(|used| !used)?;
        self.in_use[idx] = true;
        Some(self.base + idx as u32)
    }

    /// Return a uid to the pool. Idempotent + range-checked: a uid outside the
    /// range, or one already free, is a silent no-op (a stray/double free can
    /// never underflow or corrupt state).
    pub fn free(&mut self, uid: u32) {
        if uid < self.base {
            return;
        }
        let idx = (uid - self.base) as usize;
        if idx < self.in_use.len() {
            self.in_use[idx] = false;
        }
    }

    /// Count of currently-allocated uids (asserted by unit tests only).
    #[cfg(test)]
    pub fn in_use_count(&self) -> usize {
        self.in_use.iter().filter(|u| **u).count()
    }

    /// Total capacity of the range (0 when fail-closed empty).
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.in_use.len()
    }
}

/// Read a `u32` env override, falling back to `default` when absent/garbage.
fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

/// The process-global pool. Range is resolved ONCE from the env at first use
/// (`K2_SANDBOX_UID_BASE` / `K2_SANDBOX_UID_COUNT`), else the defaults.
static POOL: LazyLock<Mutex<UidPool>> = LazyLock::new(|| {
    let base = env_u32("K2_SANDBOX_UID_BASE", DEFAULT_UID_BASE);
    let count = env_u32("K2_SANDBOX_UID_COUNT", DEFAULT_UID_COUNT);
    Mutex::new(UidPool::new(base, count))
});

/// Allocate a per-session worker uid, or `None` when the pool is exhausted /
/// empty. Caller MUST fail-closed (refuse to boot) on `None`, and MUST pair a
/// success with exactly one [`free`] in the authoritative teardown observer.
#[allow(dead_code)] // reached only from the linux+microvm spawn door
pub fn alloc() -> Option<u32> {
    let mut pool = POOL.lock().unwrap_or_else(|p| p.into_inner());
    pool.alloc()
}

/// Return a per-session uid to the pool. Idempotent + range-checked (see
/// [`UidPool::free`]); safe to call from the authoritative teardown arm even if
/// the uid was never allocated.
#[allow(dead_code)] // reached only from the linux+microvm teardown observer
pub fn free(uid: u32) {
    let mut pool = POOL.lock().unwrap_or_else(|p| p.into_inner());
    pool.free(uid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_hands_out_distinct_uids_in_range() {
        let mut pool = UidPool::new(60000, 4);
        let a = pool.alloc().expect("first alloc");
        let b = pool.alloc().expect("second alloc");
        let c = pool.alloc().expect("third alloc");
        let d = pool.alloc().expect("fourth alloc");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(c, d);
        for uid in [a, b, c, d] {
            assert!((60000..60004).contains(&uid), "uid {uid} out of range");
            assert!(uid > UID_FLOOR, "uid {uid} must be above the floor");
        }
        // Exhausted → None (second concurrency bound; never a colliding uid).
        assert_eq!(pool.alloc(), None, "exhausted pool must return None");
        assert_eq!(pool.in_use_count(), 4);
    }

    #[test]
    fn free_returns_a_uid_for_reuse() {
        let mut pool = UidPool::new(60000, 2);
        let a = pool.alloc().expect("a");
        let _b = pool.alloc().expect("b");
        assert_eq!(pool.alloc(), None, "exhausted");
        pool.free(a);
        // The freed uid is reused on the next alloc (range stays bounded).
        assert_eq!(pool.alloc(), Some(a), "freed uid must be reusable");
    }

    #[test]
    fn free_is_idempotent_and_range_checked() {
        let mut pool = UidPool::new(60000, 2);
        let a = pool.alloc().expect("a");
        pool.free(a);
        pool.free(a); // double free → no-op, no underflow
        // Out-of-range frees are silent no-ops (can't corrupt the bitmap).
        pool.free(0);
        pool.free(59999);
        pool.free(70000);
        assert_eq!(pool.in_use_count(), 0);
        // Still fully allocatable after the stray frees.
        assert!(pool.alloc().is_some());
        assert!(pool.alloc().is_some());
        assert_eq!(pool.alloc(), None);
    }

    #[test]
    fn base_at_or_below_floor_is_fail_closed_empty() {
        // A base at/below the floor would risk a real-user collision → EMPTY
        // pool → alloc always None → the spawn door refuses to boot.
        for base in [0u32, 1, 1000, UID_FLOOR] {
            let mut pool = UidPool::new(base, 16);
            assert_eq!(pool.capacity(), 0, "base {base} must yield an empty pool");
            assert_eq!(pool.alloc(), None, "base {base} must never allocate");
        }
    }

    #[test]
    fn zero_count_is_empty() {
        let mut pool = UidPool::new(60000, 0);
        assert_eq!(pool.capacity(), 0);
        assert_eq!(pool.alloc(), None);
    }

    #[test]
    fn count_is_clamped_against_u32_overflow() {
        // base + count would overflow u32 → clamp to the available headroom,
        // never panic, never wrap to a tiny/zero range silently.
        let base = u32::MAX - 3;
        let pool = UidPool::new(base, 100);
        assert_eq!(pool.capacity(), 3, "count clamped to u32::MAX - base");
    }

    #[test]
    fn allocated_uid_never_zero() {
        let mut pool = UidPool::new(60000, 8);
        while let Some(uid) = pool.alloc() {
            assert_ne!(uid, 0, "the pool must never produce uid 0");
        }
    }
}

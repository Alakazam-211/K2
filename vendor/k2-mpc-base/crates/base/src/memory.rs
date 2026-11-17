//! Handing freed memory back to the OS (the native engine's cut queue). After a
//! cut adopts the compacted state, the old state's memory is freed inside the process, but an
//! allocator keeps freed pages it can't return from the top of its heap: the process's resident
//! memory stays at its peak. [`release_freed`] asks the allocator to return every free page it
//! can, so the memory a cut freed leaves the process. The cut queue calls it in its idle slot,
//! after a cut; it is cheap when there is nothing to return.
//!
//! - **glibc (Linux):** `malloc_trim(0)`, which returns the free pages of every arena (not only
//!   the top of the heap) to the OS.
//! - **Other platforms:** nothing yet. macOS's and Windows' allocators return memory on their
//!   own schedule; an explicit call is added only where a measurement shows it helps.

/// What [`release_freed`] did. (Each platform builds one variant; the cut queue reads it.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Release {
    /// The allocator was asked to return its free pages; `returned`: whether it said it
    /// returned any.
    Trimmed { returned: bool },
    /// This platform's allocator has no call the kit makes.
    NotOnThisPlatform,
}

/// Ask the allocator to return its free pages to the OS (module docs).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn release_freed() -> Release {
    // the plant: nothing is released
    if crate::beta_plant::planted("release-skipped") {
        return Release::Trimmed { returned: false };
    }
    trim()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
fn trim() -> Release {
    // SAFETY: `malloc_trim` takes a pad size and touches only the allocator's own state; glibc
    // makes it safe to call from any thread at any time (it takes each arena's lock).
    let r = unsafe { libc::malloc_trim(0) };
    Release::Trimmed { returned: r == 1 }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim() -> Release {
    Release::NotOnThisPlatform
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process's resident memory, in bytes (`VmRSS` of `/proc/self/status`).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    fn rss() -> u64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = s.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        let kb: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        kb * 1024
    }

    /// Freed memory held in the middle of the heap (small blocks, every sixteenth kept so the
    /// heap can't shrink from its top) stays resident until [`release_freed`] returns it: the
    /// process's resident memory drops by most of what was freed. The control: the blocks were
    /// resident first.
    /// `MPC_BETA_PLANT=release-skipped` fails at the drop's assert.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn freed_memory_leaves_the_process() {
        const BLOCK: usize = 2 << 10;
        const BLOCKS: usize = 128 << 10; // 256 MiB in all
        let before = rss();
        let mut blocks: Vec<Vec<u8>> = (0..BLOCKS).map(|i| vec![(i % 251) as u8; BLOCK]).collect();
        let grown = rss();
        assert!(
            grown >= before + (128 << 20),
            "control: the blocks are resident: {before} -> {grown}"
        );
        let kept: Vec<Vec<u8>> = blocks
            .drain(..)
            .enumerate()
            .filter(|(i, _)| i % 16 == 0)
            .map(|(_, b)| b)
            .collect();
        let freed = rss();
        let r = release_freed();
        let after = rss();
        assert!(
            matches!(r, Release::Trimmed { .. }),
            "row: the allocator was asked: {r:?}"
        );
        assert!(
            after + (64 << 20) <= freed,
            "row: the freed memory left the process: {freed} -> {after} ({r:?})"
        );
        println!(
            "MEMORY-RELEASE before={before} grown={grown} freed={freed} after={after} kept_blocks={}",
            kept.len()
        );
        drop(kept);
    }

    /// Elsewhere the call does nothing and says so.
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn release_is_a_no_op_elsewhere() {
        assert_eq!(release_freed(), Release::NotOnThisPlatform);
    }
}

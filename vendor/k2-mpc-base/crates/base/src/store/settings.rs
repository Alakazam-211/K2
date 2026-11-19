//! What a host chooses when it opens a store ([`StoreSettings`]): today the size of the store
//! library's in-memory page cache.
//!
//! **The cache.** The store library keeps the pages it reads and writes in memory, up to a
//! size, and frees them only by evicting at that size: it is heap memory of the process, not
//! a mapping of the file. A store holds every doc of its host, so there is one cache per
//! opened store (per host process), shared by all docs. Left to the library's own default it
//! may grow to 1 GiB as docs are read. The kit sets it: [`StoreSettings::DEFAULT`], 64 MiB, or
//! the host's choice from [`StoreSettings::CACHE_LEAST`] to [`StoreSettings::CACHE_MOST`].
//!
//! Every database the kit opens for a store is built here ([`redb_builder`]): the store
//! itself ([`super::DocStore::open_with`], [`super::DocStore::open_with_retry_with`]) and the
//! fresh file a rewrite puts in its place ([`super::DocStore::rewrite_file`], with the store's
//! own settings).

/// A store's settings, given when it is opened ([`super::DocStore::open_with`]). Made only by
/// [`StoreSettings::new`] (checked) or [`StoreSettings::DEFAULT`], so every value is in range.
/// Listed by `capabilities` as `store_cache_bytes` (the session config's
/// [`crate::session::Config::store`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreSettings {
    cache_bytes: u64,
    commit: CommitDurability,
}

/// When a store's commits reach the disk ([`StoreSettings::with_commit_durability`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CommitDurability {
    /// Every commit waits for the disk: a change is durable when its call returns (the
    /// default).
    #[default]
    Immediate,
    /// Grouped commits: a change is applied, visible and ordered when its call returns, and
    /// durable with its group, at most `max_changes` changes or `max_ms` (the host's clock,
    /// at the next commit or tick) later, and at once before anything outside the store
    /// depends on it. An abrupt stop or a power loss keeps a prefix of the commits, never a torn one.
    /// Made by [`CommitDurability::grouped`] (checked).
    Grouped {
        /// The most changes a group holds.
        max_changes: u32,
        /// The longest a group waits, ms.
        max_ms: u64,
    },
}

impl CommitDurability {
    /// The most changes a group may hold.
    pub const GROUP_CHANGES_MOST: u32 = 1024;
    /// The longest a group may wait, ms.
    pub const GROUP_MS_MOST: u64 = 10_000;

    /// Grouped commits of at most `max_changes` changes (1 to [`Self::GROUP_CHANGES_MOST`])
    /// and at most `max_ms` ms (1 to [`Self::GROUP_MS_MOST`]), else refused with the ranges.
    pub fn grouped(max_changes: u32, max_ms: u64) -> Result<Self, StoreSettingsError> {
        if !(1..=Self::GROUP_CHANGES_MOST).contains(&max_changes)
            || !(1..=Self::GROUP_MS_MOST).contains(&max_ms)
        {
            return Err(StoreSettingsError::GroupOutOfRange {
                max_changes,
                max_ms,
                changes_most: Self::GROUP_CHANGES_MOST,
                ms_most: Self::GROUP_MS_MOST,
            });
        }
        Ok(CommitDurability::Grouped {
            max_changes,
            max_ms,
        })
    }
}

/// Why [`StoreSettings::new`] refused a store's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StoreSettingsError {
    /// The cache size is outside [`StoreSettings::CACHE_LEAST`]..=[`StoreSettings::CACHE_MOST`].
    #[error("the store's cache ({bytes} bytes) must be from {least} to {most} bytes")]
    CacheOutOfRange { bytes: u64, least: u64, most: u64 },
    /// A group's bounds are outside 1..=`changes_most` changes or 1..=`ms_most` ms.
    #[error(
        "a commit group ({max_changes} changes, {max_ms} ms) must hold 1 to {changes_most} \
         changes and wait 1 to {ms_most} ms"
    )]
    GroupOutOfRange {
        max_changes: u32,
        max_ms: u64,
        changes_most: u32,
        ms_most: u64,
    },
}

impl StoreSettings {
    /// The smallest cache: 16 MiB. Below it the edit path slows (a long-lived doc's pages are
    /// evicted between two of its edits). The cache fills only with what is
    /// read, so a small store costs no more at this size than at any other.
    pub const CACHE_LEAST: u64 = 16 << 20;
    /// The largest cache: 1 GiB, the store library's own default.
    pub const CACHE_MOST: u64 = 1 << 30;
    /// The default: a 64 MiB cache. It holds the pages one large doc reads to load; a cache
    /// larger than that helps only when it holds every page the host reads and the system has
    /// dropped the file from its own page cache.
    pub const DEFAULT: StoreSettings = StoreSettings {
        cache_bytes: 64 << 20,
        commit: CommitDurability::Immediate,
    };

    /// A store's settings, checked: a cache of `cache_bytes`, from [`Self::CACHE_LEAST`] to
    /// [`Self::CACHE_MOST`] inclusive, else refused with the range.
    pub fn new(cache_bytes: u64) -> Result<Self, StoreSettingsError> {
        if !(Self::CACHE_LEAST..=Self::CACHE_MOST).contains(&cache_bytes) {
            return Err(StoreSettingsError::CacheOutOfRange {
                bytes: cache_bytes,
                least: Self::CACHE_LEAST,
                most: Self::CACHE_MOST,
            });
        }
        Ok(StoreSettings {
            cache_bytes,
            commit: CommitDurability::Immediate,
        })
    }

    /// The store's cache size, in bytes.
    pub const fn cache_bytes(&self) -> u64 {
        self.cache_bytes
    }

    /// These settings with commits made durable as `commit` says.
    pub const fn with_commit_durability(self, commit: CommitDurability) -> Self {
        StoreSettings { commit, ..self }
    }

    /// When the store's commits reach the disk ([`CommitDurability`]).
    pub const fn commit_durability(&self) -> CommitDurability {
        self.commit
    }
}

impl Default for StoreSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The builder every database of a store is opened with: the cache at `s`'s size.
pub(super) fn redb_builder(s: &StoreSettings) -> redb::Builder {
    let mut b = redb::Builder::new();
    if !cache_ignored() {
        b.set_cache_size(usize::try_from(s.cache_bytes).unwrap_or(usize::MAX));
    }
    b
}

/// Test builds only: the plant `store-cache-ignored` (`MPC_STORE_PLANT`): the settings are
/// not applied, so the store library's default cache (1 GiB) is used, as before.
#[cfg(test)]
fn cache_ignored() -> bool {
    std::env::var("MPC_STORE_PLANT").as_deref() == Ok("store-cache-ignored")
}

/// Outside test builds: nothing is planted.
#[cfg(not(test))]
fn cache_ignored() -> bool {
    false
}

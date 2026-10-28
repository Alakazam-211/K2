//! Session and pool settings. Defaults follow the Markdown PRD (§6, §9.4, §12).

/// Settings for a [`super::SessionManager`] and its sessions. Start from
/// [`Config::default`] and change fields.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    // --- presence ---
    /// A participant with edit rights is an editor while their last edit or caret move is
    /// this recent. Default 30 s.
    pub editor_window_ms: u64,
    /// A participant without a hold ([`super::Stay::UntilIdle`]) leaves after this long with
    /// no call. Default 60 s.
    pub idle_leave_ms: u64,
    /// A participant not visible for this long is shown away. Default 60 s.
    pub away_after_ms: u64,

    // --- write --base ---
    /// Read-base cache: at most this many versions... Default 64.
    pub read_bases_max: usize,
    /// ...each kept at most this long... Default 30 min.
    pub read_base_ttl_ms: u64,
    /// ...and at most this many bytes of text in all. Default 16 MiB.
    pub read_bases_max_bytes: usize,

    // --- undo ---
    /// Undo (and redo) batches kept per participant per doc. Default 100 (spec U6).
    pub undo_depth: usize,
    /// Bytes of retained region text per participant per doc, undo and redo stacks
    /// together; that participant's oldest batches go first. Default 4 MiB (spec U6).
    pub undo_max_bytes: usize,

    // --- versions and history (K-HIST-4, -7, -22, -23) ---
    /// Cut a version after this much quiet following edits. Default 2 min.
    pub idle_version_ms: u64,
    /// Cut a version before an agent edit changing more than this many lines... Default 20.
    pub agent_version_lines: usize,
    /// ...or more than this percent of the doc's lines. Default 10.
    pub agent_version_percent: usize,
    /// Retention: keep every automatic version this long... Default 30 days.
    pub version_keep_all_ms: u64,
    /// ...then one per hour up to this age, then one per day. Default 90 days.
    pub version_hourly_until_ms: u64,
    /// At most this many versions per doc (the oldest unnamed go first). Default 500.
    pub version_max_count: usize,
    /// At most this many bytes of version content per doc. Default 64 MiB.
    pub version_max_bytes: u64,
    /// The revert window: records of changes newer than this... Default 30 days.
    pub revert_keep_ms: u64,
    /// ...or among the last this many changes, whichever reaches further, are kept (and
    /// every pending on-behalf change's). Default 10,000.
    pub revert_keep_changes: u64,
    /// At most this many bytes of revert records per doc (oldest non-pending dropped
    /// first). Default 64 MiB.
    pub revert_max_bytes: u64,
    /// Pending on-behalf records per doc; past either cap new on-behalf edits are held
    /// (`approvals_backlog`). Default 1,000...
    pub pending_max_records: u64,
    /// ...or 16 MiB.
    pub pending_max_bytes: u64,
    /// How often [`super::SessionManager::tick`] runs a loaded doc's retention (version
    /// thinning, the revert window). Default 1 min.
    pub history_tick_ms: u64,

    // --- limits ---
    /// Edit calls (`edit`, `replace_text`, `write_base`, `undo`, `redo`) per second per
    /// participant per doc, sustained. Default 10.
    pub edit_rate_per_s: u64,
    /// Burst allowance for edit calls. Default 10.
    pub edit_burst: u64,
    /// Events queued per subscriber before it is cut off. Default 1024.
    pub subscriber_queue: usize,

    // --- store ---
    /// Compact when the log since the snapshot reaches this many entries... Default 1,000.
    pub compact_entries: u64,
    /// ...or this many bytes. Default 2 MiB.
    pub compact_bytes: u64,
    /// On unload, compact when the log has at least this many entries. Default 100.
    pub unload_compact_entries: u64,

    // --- pool ---
    /// Memory budget for loaded sessions; idle sessions unload, least recently used first,
    /// while the estimate is over it. Default 256 MiB.
    pub memory_budget_bytes: u64,
    /// A session's memory estimate is its encoded state size times this. Default 20
    /// (measured heap is 3-20x the v1 state).
    pub memory_factor: u64,
    /// A session with no participants unloads after this long unused. Default 10 min.
    pub idle_unload_ms: u64,

    // --- disk sync (sessions opened with SessionManager::open_file) ---
    /// Write the file back once the doc has been quiet this long... Default 1 s.
    pub write_quiet_ms: u64,
    /// ...or this long after the first unwritten change, during continuous typing.
    /// Default 5 s.
    pub write_max_delay_ms: u64,
    /// While anyone is present, stat the file (size, mtime, inode) this often, as a backup
    /// for the host's watcher. Default 3 s.
    pub disk_poll_ms: u64,
    /// Files larger than this are declined without being read (`too_large`); the pack's
    /// own limit applies below it. Default 64 MiB.
    pub disk_read_max_bytes: u64,
    /// Echo suppression: a hash of one of the kit's own writes is kept this long (in memory
    /// only, keyed per doc; K-DISK-4, K-PURGE-13). A file holding an older write of ours
    /// is then an outside edit. Default 60 s.
    pub own_write_ttl_ms: u64,

    // --- tier 0 (blob docs) ---
    /// A lease lasts this long unless the caller asks otherwise; renew to keep it.
    /// Default 5 min (K-T0-2).
    pub lease_default_ms: u64,
    /// The longest lease anyone can ask for. Default 1 h.
    pub lease_max_ms: u64,
    /// A write by someone without a lease, when nobody holds one, takes a lease for the
    /// length of the write (upload included), released when it commits; this bounds it if
    /// the writer dies mid-upload. Default 2 min.
    pub lease_auto_ms: u64,
    /// Tier 0.5: a pack's `merge3_bytes` is only tried when base, ours and theirs are each
    /// at most this big (they are read into memory). Default 32 MiB.
    pub blob_merge_max_bytes: u64,
}

const DAY: u64 = 24 * 60 * 60_000;

impl Default for Config {
    fn default() -> Self {
        Self {
            editor_window_ms: 30_000,
            idle_leave_ms: 60_000,
            away_after_ms: 60_000,
            read_bases_max: 64,
            read_base_ttl_ms: 30 * 60_000,
            read_bases_max_bytes: 16 << 20,
            undo_depth: 100,
            undo_max_bytes: 4 << 20,
            idle_version_ms: 2 * 60_000,
            agent_version_lines: 20,
            agent_version_percent: 10,
            version_keep_all_ms: 30 * DAY,
            version_hourly_until_ms: 90 * DAY,
            version_max_count: 500,
            version_max_bytes: 64 << 20,
            revert_keep_ms: 30 * DAY,
            revert_keep_changes: 10_000,
            revert_max_bytes: 64 << 20,
            pending_max_records: 1_000,
            pending_max_bytes: 16 << 20,
            history_tick_ms: 60_000,
            edit_rate_per_s: 10,
            edit_burst: 10,
            subscriber_queue: 1024,
            compact_entries: 1_000,
            compact_bytes: 2 << 20,
            unload_compact_entries: 100,
            memory_budget_bytes: 256 << 20,
            memory_factor: 20,
            idle_unload_ms: 10 * 60_000,
            write_quiet_ms: 1_000,
            write_max_delay_ms: 5_000,
            disk_poll_ms: 3_000,
            disk_read_max_bytes: 64 << 20,
            own_write_ttl_ms: 60_000,
            lease_default_ms: 5 * 60_000,
            lease_max_ms: 60 * 60_000,
            lease_auto_ms: 2 * 60_000,
            blob_merge_max_bytes: 32 << 20,
        }
    }
}

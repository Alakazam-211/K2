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
    /// Undo (and redo) batches kept per participant. Default 100.
    pub undo_depth: usize,
    /// Bytes of undo/redo text kept per session across participants; the oldest batches go
    /// first. Default 32 MiB.
    pub undo_max_bytes: usize,

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
}

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
            undo_max_bytes: 32 << 20,
            edit_rate_per_s: 10,
            edit_burst: 10,
            subscriber_queue: 1024,
            compact_entries: 1_000,
            compact_bytes: 2 << 20,
            unload_compact_entries: 100,
            memory_budget_bytes: 256 << 20,
            memory_factor: 20,
            idle_unload_ms: 10 * 60_000,
        }
    }
}

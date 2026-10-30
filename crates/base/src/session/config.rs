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

    // --- agent presence (amendment 13): claims, notes, activity; memory only ---
    /// A claim lasts this long unless the caller asks otherwise. Default 30 s.
    pub claim_default_ms: u64,
    /// The longest claim anyone can ask for (longer asks are clamped). Default 10 min.
    pub claim_max_ms: u64,
    /// A read verb's `--for-edit` claim lasts this long unless `--ttl` says otherwise.
    /// Default 30 s.
    pub for_edit_ms: u64,
    /// Live claims per doc; past it a new claim is refused (`BadClaim`). Default 256.
    pub claims_max: usize,
    /// Live claims per participant per doc. Default 16.
    pub claims_per_participant: usize,
    /// Waiters per claim. Default 64.
    pub claim_waiters_max: usize,
    /// When a claim with waiters ends, its range is reserved this long for the first
    /// waiter: other agents' claims and `Respect` edits there are refused meanwhile.
    /// Default 2 s; 0 turns hand-off reservations off.
    pub claim_handoff_ms: u64,
    /// An edit this recent is a heads-up note (`edited`) and named in a `write --base`
    /// conflict. Default 60 s.
    pub note_edit_window_ms: u64,
    /// The same note (reader, other, kind, lines) is not repeated within this. Default 60 s.
    pub note_repeat_ms: u64,
    /// At most one batch of notes per reader per doc this often. Default 5 s.
    pub note_interval_ms: u64,
    /// At most this many notes in a batch; the rest are counted (`+N more`). Default 5.
    pub note_batch_max: usize,
    /// Activity rows kept per doc (a ring; dropped on unload). Default 256.
    pub activity_max: usize,

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
    /// Person undo grouping (spec U3 (a), K4): an edit with an
    /// [`super::EditOptions::group`] token joins its connection's open group only if the
    /// group's last edit is less than this old on the host clock. 0: groups never merge.
    /// Default 1 s.
    pub undo_group_idle_ms: u64,

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
    /// Roster pushes ([`super::Session::subscribe_roster`]) are coalesced over this
    /// interval: at most one delivery per interval per doc, a caret moved many times in it
    /// is one event. The host drives it ([`super::SessionManager::flush_rosters`] or `tick`
    /// at least this often). Default 250 ms.
    pub roster_interval_ms: u64,
    /// Roster events queued per subscriber (and recorded between two flushes). A
    /// subscriber that falls this far behind gets a fresh snapshot instead of the deltas it
    /// missed. Default 256.
    pub roster_queue: usize,
    /// Participants (distinct pids) a doc admits with presence (K11). A join past it is a
    /// **watcher**: read-only whatever its rights, no caret, view or claims, shown in the
    /// roster only as a `watching` count. A slot that frees is not filled by itself: the host
    /// promotes the earliest watcher ([`super::Session::promote_watcher`]). Default 32.
    pub participants_max: usize,

    // --- store ---
    /// Compact when the log since the snapshot reaches this many entries... Default 1,000.
    pub compact_entries: u64,
    /// ...or this many bytes. Default 2 MiB.
    pub compact_bytes: u64,
    /// On unload, compact when the log has at least this many entries. Default 100.
    pub unload_compact_entries: u64,

    // --- engine ---
    /// The edit path's runtime check (v0.9.3): one edit in this many is also made through a
    /// throwaway replica and the two updates compared byte for byte, and the engine's cached
    /// texts compared with a fresh read. A mismatch is never served (the replica's update is
    /// applied), is reported ([`super::AuditEvent::EditPathMismatch`],
    /// [`super::AuditEvent::TextCacheMismatch`]), and the doc uses the replica path (or reads
    /// its texts afresh) until it is reloaded. A sampled edit costs O(doc), as every edit did
    /// before v0.9.3. The same rate samples outside saves: one in this many also computes its
    /// change at element identity in full and compares it with the merge's hunks
    /// ([`super::AuditEvent::IdentityMismatch`]). 0: never. Default 1,000
    /// ([`crate::doc::DEFAULT_EDIT_CHECK_EVERY`]). Past [`Config::edit_check_scale_units`] of
    /// history the edit path's interval grows with the doc (adaptive, v0.9.7).
    pub edit_check_every: u32,
    /// The edit path's runtime check on a large doc (v0.9.7): past this much history (UTF-16
    /// units, the sum of the doc's state vector's clocks: every unit ever inserted, deleted
    /// or not), the interval between sampled edits grows in proportion to it, `every x
    /// history / this` edits and never fewer than [`Config::edit_check_every`]. A sampled
    /// edit's full encode and throwaway replica grow with the history, so a fixed rate's cost
    /// per edit does too; this keeps the check's amortised cost per edit bounded. A doc below
    /// it is checked one edit in [`Config::edit_check_every`]. The doc's first sample after a
    /// load is placed from its history alone (`every` edits in below this; past it at a
    /// phase spread over the interval), so the long-run rate does not depend on how often
    /// the doc is reloaded. Outside saves keep the fixed rate. 0: a fixed rate. Default
    /// 50,000 ([`crate::doc::DEFAULT_EDIT_CHECK_SCALE_UNITS`]): about the history at version
    /// 10,000 in the 32-participant soak, so past it the check's amortised cost per edit
    /// stays about what it is there.
    pub edit_check_scale_units: u64,
    /// Blame's runtime check (v0.9.7): blame reads the provenance map, which every change
    /// keeps up to date as it applies; one blame in this many also rebuilds the map from the
    /// engine and the store (the cold path: O(doc)) and compares the two. The rebuild runs
    /// **off the session's lock**, on a copy of the doc taken with the blame (an encode under
    /// the lock), so participants keep editing meanwhile; the sampled blame's caller waits
    /// for it. A mismatch is never served: that blame is computed again from a sound map; the
    /// kept map is replaced by the rebuild if nothing changed since, else dropped (rebuilt on
    /// next need); and it is reported ([`super::AuditEvent::ProvenanceMismatch`]). A map whose
    /// length isn't the text's is rebuilt (under the lock) and reported on every blame,
    /// sampled or not (O(roots x chunks)). 0: never sampled. Default 100
    /// ([`DEFAULT_BLAME_CHECK_EVERY`]). From [`Config::blame_check_large_at`] changes on, the
    /// rate is [`Config::blame_check_every_large`] instead (adaptive, v0.9.7).
    pub blame_check_every: u32,
    /// Blame's runtime check on a doc with a long history (v0.9.7): from this many changes
    /// (the doc's version) on, one blame in [`Config::blame_check_every_large`] is checked, not
    /// one in [`Config::blame_check_every`]: a sampled blame's rebuild and the state it
    /// copies under the lock grow with the history. Default 20,000
    /// ([`DEFAULT_BLAME_CHECK_LARGE_AT`]).
    pub blame_check_large_at: u64,
    /// The check's rate from [`Config::blame_check_large_at`] changes on. 0: never sampled
    /// there. Default 1,000 ([`DEFAULT_BLAME_CHECK_EVERY_LARGE`]).
    pub blame_check_every_large: u32,

    /// The engine's position index check (v0.9.7): the index finds a text position from a
    /// recent one instead of walking the text from its start, and one marker lookup in this
    /// many also walks and compares. A mismatch is never served (the walk's position is
    /// used, the text's markers dropped) and is reported
    /// ([`super::AuditEvent::PositionIndexMismatch`]). A sampled lookup costs one walk, as
    /// every lookup did before v0.9.7. 0: never. Default 1,000
    /// ([`crate::doc::DEFAULT_POSITION_CHECK_EVERY`]).
    pub position_check_every: u32,

    /// Reuse an agent's client ID across its connections (v0.9.7): an agent's join takes a
    /// client ID it used before in this doc, when no present connection holds it, instead of
    /// minting one. Every client ID stays in the doc's state for good, and the engine's
    /// per-transaction work grows with their number, so a long-lived doc many agent
    /// sessions join kept getting slower. Only agents ([`super::ParticipantKind::Agent`])
    /// that edit through the session alone; never a participant with
    /// [`super::Participant::own_replica`], whose stale replica could write as an old ID.
    /// Default true.
    pub reuse_agent_clients: bool,

    /// The file write-back's sampled check (v0.9.7): the disk replica is kept in step by
    /// applying the changes since the last write-back; every sync compares its state vector
    /// with the live doc's (O(clients)), and one in this many also its deletions and texts
    /// (O(the doc); not the encodings, which may differ for equal content). A mismatch is
    /// reported ([`super::AuditEvent::DiskReplicaMismatch`]) and the replica reloaded from
    /// the doc. 0: never (the state vectors are still compared). Default 100.
    pub writeback_check_every: u32,

    // --- live edits (E-LIVE, v0.9.7) ---
    /// The live window ([`super::Session::events_since`], [`super::Session::edit_at_base`]):
    /// at most this many changes after a base. An older base is `MPC-E112 base_expired`
    /// (the client reloads). Default 1,000.
    pub live_window_events: u64,
    /// ... and at most this many bytes of their events (inserted text plus 64 bytes per
    /// event). Default 4 MiB.
    pub live_window_bytes: u64,

    // --- pool ---
    /// Memory budget for loaded sessions; idle sessions unload, least recently used first,
    /// while the estimate is over it. Default 256 MiB.
    pub memory_budget_bytes: u64,
    /// A session's memory estimate is its encoded state size times this. Default 20
    /// (measured heap is 3-20x the v1 state).
    pub memory_factor: u64,
    /// A session with no participants unloads after this long unused. Default 10 min.
    pub idle_unload_ms: u64,
    /// An outside save that removes or replaces text written through the kit tells its author
    /// when the text was written since the last outside save, or within this long. 0: only
    /// since the last outside save. Default 24 h.
    pub outside_note_window_ms: u64,
    /// How long a waiting note keeps the text it carries, loaded or not. Past it the note says
    /// the text is no longer held; the fact stays for the revert window. Default 24 h.
    pub outside_note_text_keep_ms: u64,

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
    /// Conflict copies older than this are deleted on `tick` (K-A11Y-5), unless their
    /// version is named or a person edited the copy (its bytes no longer match the version
    /// it holds). Default 30 days.
    pub conflict_copy_keep_ms: u64,
}

impl Config {
    /// Every setting in effect, by field name, as numbers (milliseconds, bytes, counts as the
    /// fields say): what `capabilities` prints. Lists every field: a new one doesn't compile
    /// until it is added here.
    pub fn limits(&self) -> Vec<(&'static str, u64)> {
        let Config {
            editor_window_ms,
            idle_leave_ms,
            away_after_ms,
            claim_default_ms,
            claim_max_ms,
            for_edit_ms,
            claims_max,
            claims_per_participant,
            claim_waiters_max,
            claim_handoff_ms,
            note_edit_window_ms,
            note_repeat_ms,
            note_interval_ms,
            note_batch_max,
            activity_max,
            read_bases_max,
            read_base_ttl_ms,
            read_bases_max_bytes,
            undo_depth,
            undo_max_bytes,
            undo_group_idle_ms,
            idle_version_ms,
            agent_version_lines,
            agent_version_percent,
            version_keep_all_ms,
            version_hourly_until_ms,
            version_max_count,
            version_max_bytes,
            revert_keep_ms,
            revert_keep_changes,
            revert_max_bytes,
            pending_max_records,
            pending_max_bytes,
            history_tick_ms,
            edit_rate_per_s,
            edit_burst,
            subscriber_queue,
            roster_interval_ms,
            roster_queue,
            participants_max,
            compact_entries,
            compact_bytes,
            unload_compact_entries,
            edit_check_every,
            edit_check_scale_units,
            position_check_every,
            writeback_check_every,
            reuse_agent_clients,
            blame_check_every,
            blame_check_large_at,
            blame_check_every_large,
            live_window_events,
            live_window_bytes,
            memory_budget_bytes,
            memory_factor,
            idle_unload_ms,
            outside_note_window_ms,
            outside_note_text_keep_ms,
            write_quiet_ms,
            write_max_delay_ms,
            disk_poll_ms,
            disk_read_max_bytes,
            own_write_ttl_ms,
            lease_default_ms,
            lease_max_ms,
            lease_auto_ms,
            blob_merge_max_bytes,
            conflict_copy_keep_ms,
        } = self;
        vec![
            ("editor_window_ms", *editor_window_ms),
            ("idle_leave_ms", *idle_leave_ms),
            ("away_after_ms", *away_after_ms),
            ("claim_default_ms", *claim_default_ms),
            ("claim_max_ms", *claim_max_ms),
            ("for_edit_ms", *for_edit_ms),
            ("claims_max", *claims_max as u64),
            ("claims_per_participant", *claims_per_participant as u64),
            ("claim_waiters_max", *claim_waiters_max as u64),
            ("claim_handoff_ms", *claim_handoff_ms),
            ("note_edit_window_ms", *note_edit_window_ms),
            ("note_repeat_ms", *note_repeat_ms),
            ("note_interval_ms", *note_interval_ms),
            ("note_batch_max", *note_batch_max as u64),
            ("activity_max", *activity_max as u64),
            ("read_bases_max", *read_bases_max as u64),
            ("read_base_ttl_ms", *read_base_ttl_ms),
            ("read_bases_max_bytes", *read_bases_max_bytes as u64),
            ("undo_depth", *undo_depth as u64),
            ("undo_max_bytes", *undo_max_bytes as u64),
            ("undo_group_idle_ms", *undo_group_idle_ms),
            ("idle_version_ms", *idle_version_ms),
            ("agent_version_lines", *agent_version_lines as u64),
            ("agent_version_percent", *agent_version_percent as u64),
            ("version_keep_all_ms", *version_keep_all_ms),
            ("version_hourly_until_ms", *version_hourly_until_ms),
            ("version_max_count", *version_max_count as u64),
            ("version_max_bytes", *version_max_bytes),
            ("revert_keep_ms", *revert_keep_ms),
            ("revert_keep_changes", *revert_keep_changes),
            ("revert_max_bytes", *revert_max_bytes),
            ("pending_max_records", *pending_max_records),
            ("pending_max_bytes", *pending_max_bytes),
            ("history_tick_ms", *history_tick_ms),
            ("edit_rate_per_s", *edit_rate_per_s),
            ("edit_burst", *edit_burst),
            ("subscriber_queue", *subscriber_queue as u64),
            ("roster_interval_ms", *roster_interval_ms),
            ("roster_queue", *roster_queue as u64),
            ("participants_max", *participants_max as u64),
            ("compact_entries", *compact_entries),
            ("compact_bytes", *compact_bytes),
            ("unload_compact_entries", *unload_compact_entries),
            ("edit_check_every", u64::from(*edit_check_every)),
            ("edit_check_scale_units", *edit_check_scale_units),
            ("position_check_every", u64::from(*position_check_every)),
            ("writeback_check_every", u64::from(*writeback_check_every)),
            ("reuse_agent_clients", u64::from(*reuse_agent_clients)),
            ("blame_check_every", u64::from(*blame_check_every)),
            ("blame_check_large_at", *blame_check_large_at),
            (
                "blame_check_every_large",
                u64::from(*blame_check_every_large),
            ),
            ("live_window_events", *live_window_events),
            ("live_window_bytes", *live_window_bytes),
            ("memory_budget_bytes", *memory_budget_bytes),
            ("memory_factor", *memory_factor),
            ("idle_unload_ms", *idle_unload_ms),
            ("outside_note_window_ms", *outside_note_window_ms),
            ("outside_note_text_keep_ms", *outside_note_text_keep_ms),
            ("write_quiet_ms", *write_quiet_ms),
            ("write_max_delay_ms", *write_max_delay_ms),
            ("disk_poll_ms", *disk_poll_ms),
            ("disk_read_max_bytes", *disk_read_max_bytes),
            ("own_write_ttl_ms", *own_write_ttl_ms),
            ("lease_default_ms", *lease_default_ms),
            ("lease_max_ms", *lease_max_ms),
            ("lease_auto_ms", *lease_auto_ms),
            ("blob_merge_max_bytes", *blob_merge_max_bytes),
            ("conflict_copy_keep_ms", *conflict_copy_keep_ms),
        ]
    }
}

const DAY: u64 = 24 * 60 * 60_000;

/// The default of [`Config::blame_check_every`]: one blame in 100 is checked against a
/// rebuild of the provenance map.
pub const DEFAULT_BLAME_CHECK_EVERY: u32 = 100;

/// The default of [`Config::blame_check_large_at`]: a doc counts as long from 20,000 changes.
pub const DEFAULT_BLAME_CHECK_LARGE_AT: u64 = 20_000;

/// The default of [`Config::blame_check_every_large`]: one blame in 1,000 on a long doc.
pub const DEFAULT_BLAME_CHECK_EVERY_LARGE: u32 = 1_000;

impl Default for Config {
    fn default() -> Self {
        Self {
            editor_window_ms: 30_000,
            idle_leave_ms: 60_000,
            away_after_ms: 60_000,
            claim_default_ms: 30_000,
            claim_max_ms: 10 * 60_000,
            for_edit_ms: 30_000,
            claims_max: 256,
            claims_per_participant: 16,
            claim_waiters_max: 64,
            claim_handoff_ms: 2_000,
            note_edit_window_ms: 60_000,
            note_repeat_ms: 60_000,
            note_interval_ms: 5_000,
            note_batch_max: 5,
            activity_max: 256,
            read_bases_max: 64,
            read_base_ttl_ms: 30 * 60_000,
            read_bases_max_bytes: 16 << 20,
            undo_depth: 100,
            undo_max_bytes: 4 << 20,
            undo_group_idle_ms: 1_000,
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
            roster_interval_ms: 250,
            roster_queue: 256,
            participants_max: 32,
            compact_entries: 1_000,
            compact_bytes: 2 << 20,
            unload_compact_entries: 100,
            edit_check_every: crate::doc::DEFAULT_EDIT_CHECK_EVERY,
            edit_check_scale_units: crate::doc::DEFAULT_EDIT_CHECK_SCALE_UNITS,
            position_check_every: crate::doc::DEFAULT_POSITION_CHECK_EVERY,
            reuse_agent_clients: true,
            writeback_check_every: 100,
            blame_check_every: DEFAULT_BLAME_CHECK_EVERY,
            blame_check_large_at: DEFAULT_BLAME_CHECK_LARGE_AT,
            blame_check_every_large: DEFAULT_BLAME_CHECK_EVERY_LARGE,
            live_window_events: 1_000,
            live_window_bytes: 4 << 20,
            memory_budget_bytes: 256 << 20,
            memory_factor: 20,
            idle_unload_ms: 10 * 60_000,
            outside_note_window_ms: 24 * 60 * 60_000,
            outside_note_text_keep_ms: 24 * 60 * 60_000,
            write_quiet_ms: 1_000,
            write_max_delay_ms: 5_000,
            disk_poll_ms: 3_000,
            disk_read_max_bytes: 64 << 20,
            own_write_ttl_ms: 60_000,
            lease_default_ms: 5 * 60_000,
            lease_max_ms: 60 * 60_000,
            lease_auto_ms: 2 * 60_000,
            blob_merge_max_bytes: 32 << 20,
            conflict_copy_keep_ms: 30 * DAY,
        }
    }
}

//! Session and pool settings, with their defaults.

use std::collections::BTreeMap;

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
    /// Person undo grouping (spec U3 (a)): an edit with an
    /// [`super::EditOptions::group`] token joins its connection's open group only if the
    /// group's last edit is less than this old on the host clock. 0: groups never merge.
    /// Default 1 s.
    pub undo_group_idle_ms: u64,

    // --- versions and history ---
    /// Cut a version after this much quiet following edits. Default 2 min.
    pub idle_version_ms: u64,
    /// Cut a version before an agent edit changing more than this many lines... Default 20.
    pub agent_version_lines: usize,
    /// ...or more than this percent of the doc's lines. Default 10.
    pub agent_version_percent: usize,
    /// For a structured doc: cut a version before an agent change touching more than this
    /// many list elements (rows; a sort counts the rows it moved)... Default 20.
    pub agent_version_elems: usize,
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
    /// Participants (distinct pids) a doc admits with presence. A join past it is a
    /// **watcher**: read-only whatever its rights, no caret, view or claims, shown in the
    /// roster only as a `watching` count. A slot that frees is not filled by itself: the host
    /// promotes the earliest watcher ([`super::Session::promote_watcher`]). Default 32.
    pub participants_max: usize,

    // --- store ---
    /// The log since the snapshot is compacted from this many entries on, once its bytes are
    /// at least a quarter of the snapshot's (v0.9.8; before, at this many entries whatever
    /// the state's size, so a compaction, which writes the whole state, came every this many
    /// changes however large the state had grown). Whatever the sizes, the log is compacted at
    /// 8 times this many entries, and at [`Config::compact_bytes`]. For a doc whose snapshot is
    /// small next to its log (or with no snapshot yet) it is exactly the old threshold.
    /// Default 1,000.
    pub compact_entries: u64,
    /// ...and always at this many bytes of log. Default 2 MiB.
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
    /// minting one, after the doc's session unloads and loads again too (a host's close, an
    /// idle unload, a restart). Every client ID stays in the doc's state for good, and the
    /// engine's per-transaction work grows with their number, so a long-lived doc many agent
    /// sessions join kept getting slower. Only agents ([`super::ParticipantKind::Agent`])
    /// that edit through the session alone; never a participant with
    /// [`super::Participant::own_replica`], whose stale replica could write as an old ID.
    /// The store records what each ID was minted for with its owner (its `client_kinds`
    /// table, after v0.10.2; for a participant that can write), so an own replica's ID is
    /// never reused after a reload either;
    /// an ID of unknown kind (one a kit before that recorded) is never reused: the agent
    /// draws one fresh ID, reused from then on. Default true.
    pub reuse_agent_clients: bool,

    /// Reuse a person's writer per device (v0.14.0; `docs/SESSIONS.md`, "Person-writer reuse
    /// (v0.14.0)"): a join with a device key ([`super::JoinOptions::device`]) by a person or an App
    /// guest takes a free writer of that person on that device, or takes its own writer back on a
    /// reconnect ([`super::JoinOptions::reclaim`]), instead of minting one per connection. A writer
    /// is free only after its connection was released ([`super::Session::release`]: the client
    /// holds nothing unsent and is done with it) or, for a connection without client state
    /// ([`super::JoinOptions::device_stateless`]), after any end; every other end parks it for its
    /// own lease. Every op is made by the session itself, so a reused writer's line continues from
    /// the doc's own state. Without a device key, or off: a fresh writer per connection, as before,
    /// so a host that sends no device key sees nothing of it. Default true.
    pub reuse_person_writers: bool,

    /// How long a parked person writer (one whose connection ended without a release) stays
    /// reserved for its own lease before another connection of the same person and device
    /// may take it, in ms. 0: never (a parked writer goes back only to its own reclaim). A
    /// connection whose reclaim comes after that is told the version its lease ended at
    /// ([`super::Joined::prev_ended`]), and never gets that writer back: its client counts no
    /// change under it after that version as its own. Default a day (86,400,000): an early
    /// let-go costs one more writer, and a writer never let go stays in the doc for good.
    pub person_writer_park_ms: u64,

    /// The most writers a doc holds (`docs/SESSIONS.md`, "The writers cap"). 0: no cap. A
    /// writer is a client ID the doc's state holds or may come to hold: every ID with an owner
    /// row, every person writer a connection that could write has held, and every present
    /// connection that could write. Every writer stays in the doc for good. At the cap, a
    /// connection that could write takes a writer it may reuse (an agent's own, a person's
    /// free writer of that device, then that person's free writer of another device, then
    /// that person's oldest parked writer, let go early), or else joins read-only
    /// ([`super::Joined::writers_full`]): its edits are refused `MPC-E908`. The outside
    /// participant, the import and a tier-0 promotion's authors are never refused, and count.
    /// [`super::AuditEvent::WritersNearCap`] tells the host once per load when the doc holds
    /// 80% of the cap, or of [`WRITERS_NOTICE_MAX`] when there is none. Default 0.
    pub writers_max: usize,

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
    /// only, keyed per doc). A file holding an older write of ours
    /// is then an outside edit. Default 60 s.
    pub own_write_ttl_ms: u64,
    /// After a host notes that an app is reading the file as it is now
    /// (`note_opened(conn, OpenedAt::FileNow)`), the kit holds its write-backs to that file
    /// this long, so the app reads the version the kit pinned; the hold ends early at the
    /// first outside save. Edits stay live meanwhile. 0: no hold. Default 10 s.
    pub opened_hold_ms: u64,

    // --- tier 0 (blob docs) ---
    /// A lease lasts this long unless the caller asks otherwise; renew to keep it.
    /// Default 5 min.
    pub lease_default_ms: u64,
    /// The longest lease anyone can ask for. Default 1 h.
    pub lease_max_ms: u64,
    /// A write by someone without a lease, when nobody holds one, takes a lease for the
    /// length of the write (upload included), released when it commits; this bounds it if
    /// the writer dies mid-upload. Default 2 min.
    pub lease_auto_ms: u64,
    /// Tier 0: how long a save ticket ([`super::BlobSession::expect_save`]) stays open for
    /// the host's write and its `record_saved`; past it the ticket lapses and the bytes, if
    /// they landed, come in as an outside save. Default 30 s.
    pub save_ticket_ms: u64,
    /// Tier 0.5: a pack's `merge3_bytes` is only tried when base, ours and theirs are each
    /// at most this big (they are read into memory). Default 32 MiB.
    pub blob_merge_max_bytes: u64,
    /// The structure index: a version over this many bytes gets no job (`NotIndexed {
    /// TooLarge }`; not stored, so raising the limit works at once). Default 2 GiB.
    pub blob_index_max_bytes: u64,
    /// The structure index: a file with more nodes is recorded as not indexed. Default 100,000.
    pub blob_index_max_nodes: u32,
    /// The structure index: a row over this many bytes is recorded as not indexed (the
    /// index has its own bound, whatever the file's size). Default 64 MiB.
    pub blob_index_max_stored_bytes: u64,
    /// The tier-0 verb path: a verb that reads its version whole (`VerbInput::Whole`) is
    /// refused on a version over this many bytes (`MPC-E728`), before it runs. A streaming
    /// verb isn't bounded by it. Default 256 MiB.
    pub blob_verb_max_bytes: u64,
    /// The tier-0 verb path: the most byte ranges one verb's plan may replace (`MPC-E725`
    /// past it). Default 1,000,000.
    pub blob_verb_max_spans: u64,
    /// The tier-0 verb path: the most new bytes one verb's plan may hold, over all its ranges
    /// (`MPC-E725` past it). Default 256 MiB.
    pub blob_verb_max_insert_bytes: u64,
    /// Annotations: the longest comment body, in bytes (`MPC-E907` past it). Default 16 KiB.
    pub annot_body_max_bytes: u64,
    /// Annotations: the most comment threads in one doc. Default 10,000.
    pub annot_threads_max: u64,
    /// Annotations: the most comments in one thread. Default 1,000.
    pub annot_comments_max: u64,
    /// Annotations: the most history rows one thread keeps. Past it, the ops that only
    /// toggle or move a thread (resolve, reopen, re-anchor, a proposal) are refused
    /// (`MPC-E907`); replies, deletes and purges always fit. Default 4,096.
    pub annot_thread_log_max: u32,
    /// Conflict copies older than this are deleted on `tick`, unless their
    /// version is named or a person edited the copy (its bytes no longer match the version
    /// it holds). Default 30 days.
    pub conflict_copy_keep_ms: u64,

    // --- grids (tables on the interim engine): knobs, as measured ---
    /// A table holds at most this many cells (present, live or hidden). A change that would
    /// grow past it is refused ([`super::SessionError::AtLimit`], exit 2); existing content
    /// is never dropped. Default 25,000.
    pub grid_max_cells: usize,
    /// At most this many columns (live or deleted). Default 1,024.
    pub grid_max_cols: usize,
    /// At most this many rows (live or deleted, blank ones included: a blank row holds no
    /// cell, so the cell limit doesn't bound them). A change that would grow past it is
    /// refused ([`super::SessionError::AtLimit`], `rows`, exit 2), and a file with more opens
    /// as text (`too_large`). Default 100,000.
    pub grid_max_rows: usize,
    /// The longest cell value or column name, in bytes (contract §1.8). A change that writes
    /// a longer one is refused ([`super::SessionError::AtLimit`], `value bytes`, exit 2);
    /// values already in the doc are never touched. Raise or lower it; the table ops of an
    /// edit closure's view follow it. Default 64 KiB ([`crate::table::DEFAULT_MAX_VALUE_LEN`]).
    pub grid_max_value_len: usize,

    // --- keyed docs that aren't tables (a deck, a board): the live size limit ---
    /// A keyed doc that isn't a table holds at most this many elements and fields: every
    /// element of its list roots (live or deleted, until dropped) plus every entry of its map
    /// roots. A file with more doesn't open live: it is declined `live_too_large`
    /// (`MPC-E415`) and stays at tier 0 (or 0.5), opened the ordinary way. A change that would
    /// grow the doc past it is refused ([`super::SessionError::AtLimit`], `elements and
    /// fields`, `MPC-E822`, exit 2); nothing in the doc is ever dropped. Default 25,000: the
    /// CSV grid's measured cap ([`Config::grid_max_cells`]), borrowed until a deck-shaped
    /// measurement sets it.
    ///
    /// The host-wide limit: it applies to a file type the host set no limit for
    /// ([`Config::keyed_max_entries_by_pack`]) unless the type's pack recommends a lower one
    /// ([`super::Pack::keyed_entries_default`]); a pack never raises it.
    /// [`Config::keyed_limit`] says which applies.
    pub keyed_max_entries: usize,
    /// The host's own [`Config::keyed_max_entries`] for a file type, by its pack's name (its
    /// schema's `name`, the one stamped on each stored doc): decks, workbooks and timelines
    /// hold very different numbers of elements and fields. A file type named here takes this
    /// limit, whatever its pack recommends and whatever the host-wide limit is, above it too:
    /// it is the only way a type gets more than the host-wide limit. Any other type takes the
    /// lower of its pack's recommended default and the host-wide limit. Read at every open and
    /// every change, so a change here applies to a doc's next change. Default: empty.
    pub keyed_max_entries_by_pack: BTreeMap<String, usize>,

    // --- the native engine (a Beta) ---
    /// Whether a new doc may be made on the native engine (a Beta): a host asks for it when it
    /// creates the doc ([`super::OpenOptions::engine`]), the only time a doc's engine is
    /// chosen. Off, such a create is refused (`MPC-E167` `native_beta_off`, exit 2) and nothing
    /// is made. Docs already on the native engine open, edit and save whatever this says.
    /// Default false.
    pub native_beta: bool,
    /// When a native doc's records are cut (feature `native`): after a commit, once the
    /// changes since its last cut reach `min(ceiling, max(floor, n / divisor))` of its `n`
    /// changes in total ([`CutCadence::every`]). Default [`CutCadence::BETA`]:
    /// `min(200, max(100, n / 30))`, the threshold measured for the Beta. A host sets its own
    /// through [`CutCadence::new`], which refuses a cadence outside its bounds; a doc takes the
    /// setting at its next open.
    pub native_cut_cadence: CutCadence,
    /// When the host-wide cut queue runs a native doc's cut (feature `native`): only after
    /// `idle_ms` with no native commit (a forced cut need not wait for it), at most
    /// `duty_pct`% of the time (after a cut that took `d` ms, none for `d * (100 / duty_pct -
    /// 1)` ms, forced ones too), and a cut over `slow_ms` is reported slow. Default
    /// [`CutQueueSettings::BETA`]: idle 250 ms, duty 10%, slow 250 ms. A host sets its own
    /// through [`CutQueueSettings::new`], which refuses values outside its bounds. The queue is
    /// per [`super::SessionManager`]: a host runs one manager per process, so one cut runs at a
    /// time on the host. Listed as `native_cut_idle_ms`, `native_cut_duty_pct` and
    /// `native_cut_slow_ms`.
    pub native_cut_queue: CutQueueSettings,
}

/// When the host-wide cut queue runs a native doc's cut ([`Config::native_cut_queue`]). Made
/// only by [`CutQueueSettings::new`] (checked) or [`CutQueueSettings::BETA`], so every value is
/// in bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutQueueSettings {
    idle_ms: u64,
    duty_pct: u64,
    slow_ms: u64,
}

/// Why [`CutQueueSettings::new`] refused the queue's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CutQueueError {
    /// The idle window is 0 (a cut would run between two edits) or past
    /// [`CutQueueSettings::IDLE_MOST_MS`] (a busy doc would wait for its forced cut).
    #[error("the cut queue's idle window ({idle_ms} ms) must be from 1 to {most} ms")]
    IdleOutOfRange { idle_ms: u64, most: u64 },
    /// The duty is outside [`CutQueueSettings::DUTY_LEAST`]..=[`CutQueueSettings::DUTY_MOST`].
    #[error("the cut queue's duty ({duty_pct}%) must be from {least}% to {most}%")]
    DutyOutOfRange {
        duty_pct: u64,
        least: u64,
        most: u64,
    },
    /// A slow-cut threshold of 0 would report every cut.
    #[error("the cut queue's slow-cut threshold must be at least 1 ms")]
    SlowZero,
}

impl CutQueueSettings {
    /// The Beta's settings, the default: idle 250 ms (the native engine's idle window), duty
    /// 10% (a 200 ms cut is followed by 1.8 s with no cut), slow 250 ms.
    pub const BETA: CutQueueSettings = CutQueueSettings {
        idle_ms: 250,
        duty_pct: 10,
        slow_ms: 250,
    };

    /// The longest idle window: one minute.
    pub const IDLE_MOST_MS: u64 = 60_000;
    /// The least duty, in percent.
    pub const DUTY_LEAST: u64 = 1;
    /// The most duty, in percent: cuts never take more than half the time.
    pub const DUTY_MOST: u64 = 50;

    /// The queue's settings, checked: `1 <= idle_ms <= IDLE_MOST_MS`, `DUTY_LEAST <= duty_pct
    /// <= DUTY_MOST` and `slow_ms >= 1`, else refused with the reason.
    pub fn new(idle_ms: u64, duty_pct: u64, slow_ms: u64) -> Result<Self, CutQueueError> {
        if idle_ms == 0 || idle_ms > Self::IDLE_MOST_MS {
            return Err(CutQueueError::IdleOutOfRange {
                idle_ms,
                most: Self::IDLE_MOST_MS,
            });
        }
        if !(Self::DUTY_LEAST..=Self::DUTY_MOST).contains(&duty_pct) {
            return Err(CutQueueError::DutyOutOfRange {
                duty_pct,
                least: Self::DUTY_LEAST,
                most: Self::DUTY_MOST,
            });
        }
        if slow_ms == 0 {
            return Err(CutQueueError::SlowZero);
        }
        Ok(CutQueueSettings {
            idle_ms,
            duty_pct,
            slow_ms,
        })
    }

    /// No native commit for this long (ms) and the host is idle: a cut that isn't forced runs
    /// only then.
    pub const fn idle_ms(&self) -> u64 {
        self.idle_ms
    }

    /// The most of the time cuts may take, in percent.
    pub const fn duty_pct(&self) -> u64 {
        self.duty_pct
    }

    /// A cut that took longer than this (ms) is reported slow.
    pub const fn slow_ms(&self) -> u64 {
        self.slow_ms
    }

    /// After a cut that took `d` ms, how long (ms) no cut may start: `d * (100 / duty_pct -
    /// 1)`, rounded up.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn duty_gap_ms(&self, d: u64) -> u64 {
        d.saturating_mul(100 - self.duty_pct)
            .div_ceil(self.duty_pct)
    }
}

impl Default for CutQueueSettings {
    fn default() -> Self {
        Self::BETA
    }
}

/// When a native doc's records are cut ([`Config::native_cut_cadence`]): a live cut once the
/// changes since the last cut reach [`CutCadence::every`] of the doc's changes in total. Made
/// only by [`CutCadence::new`] (checked) or [`CutCadence::BETA`], so every value is in bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutCadence {
    floor: u64,
    ceiling: u64,
    divisor: u64,
}

/// Why [`CutCadence::new`] refused a cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CadenceError {
    /// A floor of 0 would cut after every change.
    #[error("the cut cadence's floor must be at least 1")]
    FloorZero,
    /// The floor is above the ceiling.
    #[error("the cut cadence's floor ({floor}) is above its ceiling ({ceiling})")]
    FloorOverCeiling { floor: u64, ceiling: u64 },
    /// The ceiling is past [`CutCadence::CEILING_MOST`].
    #[error("the cut cadence's ceiling ({ceiling}) is past the most it may be ({most})")]
    CeilingTooHigh { ceiling: u64, most: u64 },
    /// A divisor of 0.
    #[error("the cut cadence's divisor must be at least 1")]
    DivisorZero,
}

impl CutCadence {
    /// The Beta's cadence, the default: `min(200, max(100, n / 30))`, the largest threshold
    /// whose measured load stays at the Beta's bar.
    pub const BETA: CutCadence = CutCadence {
        floor: 100,
        ceiling: 200,
        divisor: 30,
    };

    /// The most a ceiling may be: a native doc's uncut tail stays a bounded part of its load.
    pub const CEILING_MOST: u64 = 10_000;

    /// A cadence of `min(ceiling, max(floor, n / divisor))`, checked: `1 <= floor <= ceiling
    /// <= CEILING_MOST` and `divisor >= 1`, else refused with the reason.
    pub fn new(floor: u64, ceiling: u64, divisor: u64) -> Result<Self, CadenceError> {
        // the plant: any cadence taken unchecked
        if crate::beta_plant::planted("cadence-unchecked") {
            return Ok(CutCadence {
                floor,
                ceiling,
                divisor,
            });
        }
        if floor == 0 {
            return Err(CadenceError::FloorZero);
        }
        if floor > ceiling {
            return Err(CadenceError::FloorOverCeiling { floor, ceiling });
        }
        if ceiling > Self::CEILING_MOST {
            return Err(CadenceError::CeilingTooHigh {
                ceiling,
                most: Self::CEILING_MOST,
            });
        }
        if divisor == 0 {
            return Err(CadenceError::DivisorZero);
        }
        Ok(CutCadence {
            floor,
            ceiling,
            divisor,
        })
    }

    /// The changes since the last cut at which a doc of `n` changes in total is cut:
    /// `min(ceiling, max(floor, n / divisor))`.
    pub const fn every(&self, n: u64) -> u64 {
        let v = n / self.divisor;
        let v = if v < self.floor { self.floor } else { v };
        if v > self.ceiling {
            self.ceiling
        } else {
            v
        }
    }

    /// The most changes since the last cut that the after-edit check cuts: past it the doc's
    /// store is a large uncut one, cut at the idle tick or the unload. Twice the ceiling, so a
    /// host that checks once per commit of a batch never meets it.
    pub const fn live_cut_most(&self) -> u64 {
        2 * self.ceiling
    }

    /// The floor: a live cut never comes before this many changes since the last.
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The ceiling: a live cut always comes by this many changes since the last.
    pub const fn ceiling(&self) -> u64 {
        self.ceiling
    }

    /// Between them, one change in this many of the doc's total.
    pub const fn divisor(&self) -> u64 {
        self.divisor
    }
}

impl Default for CutCadence {
    fn default() -> Self {
        Self::BETA
    }
}

/// Which setting a keyed doc's live size limit came from ([`Config::keyed_limit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitSource {
    /// The host's limit for this file type ([`Config::keyed_max_entries_by_pack`]).
    HostPerPack,
    /// The file type's own recommended default ([`super::Pack::keyed_entries_default`]): the
    /// host set none for the type, and the default is below the host-wide limit.
    PackDefault,
    /// The host-wide limit ([`Config::keyed_max_entries`]): the host set none for the type,
    /// and its pack recommends none or one at or above it (capped here).
    HostWide,
}

impl LimitSource {
    /// Every source, in the order [`Config::keyed_limit`] tries them.
    pub const ALL: [LimitSource; 3] = [
        LimitSource::HostPerPack,
        LimitSource::PackDefault,
        LimitSource::HostWide,
    ];

    /// Its name in an error's details (`limit_source`): `host_per_pack`, `pack_default` or
    /// `host_wide`.
    pub fn as_str(&self) -> &'static str {
        match self {
            LimitSource::HostPerPack => "host_per_pack",
            LimitSource::PackDefault => "pack_default",
            LimitSource::HostWide => "host_wide",
        }
    }

    /// In words, as a decline's message says it.
    pub fn words(&self) -> &'static str {
        match self {
            LimitSource::HostPerPack => "the host's limit for this file type",
            LimitSource::PackDefault => "this file type's own default",
            LimitSource::HostWide => "the host's limit for every file type",
        }
    }
}

/// A keyed doc's live size limit (its elements and fields) and where it came from: what
/// [`Config::keyed_limit`] resolves for a file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct KeyedLimit {
    pub limit: usize,
    pub source: LimitSource,
}

impl KeyedLimit {
    pub fn new(limit: usize, source: LimitSource) -> Self {
        Self { limit, source }
    }

    /// How a decline names this limit, inside its parentheses: `25000 elements and fields;
    /// the host's limit for every file type`. [`KeyedLimit::from_decline`] reads it back.
    pub(crate) fn phrase(self) -> String {
        format!(
            "{} elements and fields; {}",
            self.limit,
            self.source.words()
        )
    }

    /// The limit a `live_too_large` decline's message names ([`KeyedLimit::phrase`], after
    /// "the live limit ("), read back for its details; `None` for any other wording.
    pub(crate) fn from_decline(message: &str) -> Option<KeyedLimit> {
        let (_, rest) = message.split_once("the live limit (")?;
        let (n, rest) = rest.split_once(" elements and fields; ")?;
        let (words, _) = rest.split_once(')')?;
        let limit = n.parse().ok()?;
        let source = LimitSource::ALL.into_iter().find(|s| s.words() == words)?;
        Some(KeyedLimit { limit, source })
    }
}

impl Config {
    /// The live size limit of a keyed doc of `pack`'s type that isn't a table, and where it came
    /// from. The host owns it:
    /// 1. the host's limit for this type, [`Config::keyed_max_entries_by_pack`] under the
    ///    pack's name, wins whatever it is ([`LimitSource::HostPerPack`]);
    /// 2. else the pack's recommended default ([`super::Pack::keyed_entries_default`]) when
    ///    it is below the host-wide limit: a pack may lower the limit, never raise it
    ///    ([`LimitSource::PackDefault`]);
    /// 3. else the host-wide [`Config::keyed_max_entries`] ([`LimitSource::HostWide`]), a
    ///    pack's default at or above it included.
    ///
    /// A type that needs more than the host-wide limit gets it only from the host's limit for
    /// that type. The kit applies the result at import, at every change and at every outside
    /// save; a host can call it to show a type's limit.
    pub fn keyed_limit(&self, pack: &dyn super::Pack) -> KeyedLimit {
        if let Some(&limit) = self.keyed_max_entries_by_pack.get(pack.schema().name) {
            return KeyedLimit::new(limit, LimitSource::HostPerPack);
        }
        match pack.keyed_entries_default() {
            Some(limit) if limit < self.keyed_max_entries => {
                KeyedLimit::new(limit, LimitSource::PackDefault)
            }
            _ => KeyedLimit::new(self.keyed_max_entries, LimitSource::HostWide),
        }
    }

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
            agent_version_elems,
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
            reuse_person_writers,
            person_writer_park_ms,
            writers_max,
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
            opened_hold_ms,
            lease_default_ms,
            lease_max_ms,
            lease_auto_ms,
            save_ticket_ms,
            blob_merge_max_bytes,
            blob_index_max_bytes,
            blob_index_max_nodes,
            blob_index_max_stored_bytes,
            blob_verb_max_bytes,
            blob_verb_max_spans,
            blob_verb_max_insert_bytes,
            annot_body_max_bytes,
            annot_threads_max,
            annot_comments_max,
            annot_thread_log_max,
            conflict_copy_keep_ms,
            grid_max_cells,
            grid_max_cols,
            grid_max_rows,
            grid_max_value_len,
            keyed_max_entries,
            // per file type, not one number: a refusal names the limit that applied
            // (`limit`, `limit_source`), and a host shows a type's with `keyed_limit`
            keyed_max_entries_by_pack: _,
            native_beta,
            native_cut_cadence,
            native_cut_queue,
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
            ("agent_version_elems", *agent_version_elems as u64),
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
            ("reuse_person_writers", u64::from(*reuse_person_writers)),
            ("person_writer_park_ms", *person_writer_park_ms),
            ("writers_max", *writers_max as u64),
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
            ("opened_hold_ms", *opened_hold_ms),
            ("lease_default_ms", *lease_default_ms),
            ("lease_max_ms", *lease_max_ms),
            ("lease_auto_ms", *lease_auto_ms),
            ("save_ticket_ms", *save_ticket_ms),
            ("blob_merge_max_bytes", *blob_merge_max_bytes),
            ("blob_index_max_bytes", *blob_index_max_bytes),
            ("blob_index_max_nodes", u64::from(*blob_index_max_nodes)),
            ("blob_index_max_stored_bytes", *blob_index_max_stored_bytes),
            ("blob_verb_max_bytes", *blob_verb_max_bytes),
            ("blob_verb_max_spans", *blob_verb_max_spans),
            ("blob_verb_max_insert_bytes", *blob_verb_max_insert_bytes),
            ("annot_body_max_bytes", *annot_body_max_bytes),
            ("annot_threads_max", *annot_threads_max),
            ("annot_comments_max", *annot_comments_max),
            ("annot_thread_log_max", u64::from(*annot_thread_log_max)),
            ("conflict_copy_keep_ms", *conflict_copy_keep_ms),
            ("grid_max_cells", *grid_max_cells as u64),
            ("grid_max_cols", *grid_max_cols as u64),
            ("grid_max_rows", *grid_max_rows as u64),
            ("grid_max_value_len", *grid_max_value_len as u64),
            ("keyed_max_entries", *keyed_max_entries as u64),
            ("native_beta", u64::from(*native_beta)),
            ("native_cut_floor", native_cut_cadence.floor()),
            ("native_cut_ceiling", native_cut_cadence.ceiling()),
            ("native_cut_divisor", native_cut_cadence.divisor()),
            ("native_cut_idle_ms", native_cut_queue.idle_ms()),
            ("native_cut_duty_pct", native_cut_queue.duty_pct()),
            ("native_cut_slow_ms", native_cut_queue.slow_ms()),
        ]
    }
}

const DAY: u64 = 24 * 60 * 60_000;

/// The count [`Config::writers_max`]'s near-cap notice is computed against when there is no
/// cap: [`super::AuditEvent::WritersNearCap`] comes at 80% of it (6,554 writers).
pub const WRITERS_NOTICE_MAX: usize = 8_192;

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
            agent_version_elems: 20,
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
            reuse_person_writers: true,
            person_writer_park_ms: 86_400_000,
            writers_max: 0,
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
            opened_hold_ms: 10_000,
            lease_default_ms: 5 * 60_000,
            lease_max_ms: 60 * 60_000,
            lease_auto_ms: 2 * 60_000,
            save_ticket_ms: 30_000,
            blob_merge_max_bytes: 32 << 20,
            blob_index_max_bytes: 2 << 30,
            blob_index_max_nodes: 100_000,
            blob_index_max_stored_bytes: 64 << 20,
            blob_verb_max_bytes: 256 << 20,
            blob_verb_max_spans: 1_000_000,
            blob_verb_max_insert_bytes: 256 << 20,
            annot_body_max_bytes: 16 << 10,
            annot_threads_max: 10_000,
            annot_comments_max: 1_000,
            annot_thread_log_max: 4_096,
            conflict_copy_keep_ms: 30 * DAY,
            grid_max_cells: 25_000,
            grid_max_cols: 1_024,
            grid_max_rows: 100_000,
            grid_max_value_len: crate::table::DEFAULT_MAX_VALUE_LEN,
            // the CSV grid's measured cap (`grid_max_cells`) until a deck-shaped
            // measurement (the pptx plan's M1) sets this one
            keyed_max_entries: 25_000,
            keyed_max_entries_by_pack: BTreeMap::new(),
            native_beta: false,
            native_cut_cadence: CutCadence::BETA,
            native_cut_queue: CutQueueSettings::BETA,
        }
    }
}

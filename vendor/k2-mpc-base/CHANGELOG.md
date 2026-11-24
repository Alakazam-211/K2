# Changelog

## v0.24.0

A host release on v0.23.0. The crate version is `0.24.0`; the client's is `0.24.0`, with no API
change. The live wire stays version 3. No store needs a migration: `STORE_FORMAT` stays 6,
`STORE_FORMAT_BASE` 5 and `STORE_FORMAT_READ` 8. Pairs with the pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.23.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### Breaking

- **A person's edit goes ahead of agents at a doc's lock and at the store's writer turn** (a
  behaviour change). A waiting call passed over by 128 later agent calls is let in next, and a
  person's call (or the manager's tick) is let in ahead of agents as soon as it waits. Agents'
  edits a second are as before; no API, setting or store format changes.
- **`Config::undo_max_bytes` counts the bytes a person's undo and redo stacks hold** (a
  behaviour change). It counted about a third of them, so the same budget holds fewer batches
  (about 30% fewer in the measured doc). The default stays 4 MiB per person; a host that sets
  it may want a larger number.
- **The tick returns freed memory to the OS after it wrote a doc back** (a behaviour change, on
  by default): at most once per `Config::release_after_write_ms` (new; default 30,000; 0
  never), through the host's `Config::release` when it gave one, else glibc's `malloc_trim(0)`.
  `TickReport::memory_released_after_write` (new) reports it.
- **The store refuses a log entry, or an engine stamp, of the other engine** (new id
  `MPC-E527` `entry_engine_refused`, exit 6; `StoreError::EntryEngineRefused` and
  `StoreError::EngineStampRefused`). No kit path writes one; a host that writes a store's log
  or metadata itself maps the new variants.
- **Index row format 3 for a pack that declares its containers' own records**
  (`INDEX_ROW_FORMAT` is 3; the pptx pack does). Its versions are indexed again by the next
  index job, under the kit's offset format (`OWN_FORMAT_OFFSET`). A previous release doesn't
  read those rows: it indexes such a version again under its own format, and minted keys
  re-mint after such a round trip, as after any format bump. A pack that declares none writes
  format 2 rows, byte for byte as before.
- **A retired person writer's row is kept, marked retired** (a bit in the row's state byte),
  after a grouped store's recovery or a native store's copy or restore. A previous release
  reads such a row as unknown and never lets that writer again.
- **`capabilities` lists more:** `release_after_write_ms`, and the error rows `MPC-E527`,
  `MPCE-E1008`, `MPCE-E1010` and `MPCE-E1013`. A reader that compares it whole (a saved golden)
  sees the new lines.
- **Native docs only (feature `native`, off by default):** every native document a store holds
  runs in the engine's single-writer mode; `Doc::try_state_vector` and
  `Doc::state_vector_parts` refuse such a document (`MPC-E166`), and the deprecated
  `Doc::state_vector` returns the engine's wire tag alone for it; a copied or restored native
  store's agents and host-device persons get fresh writers.

### What changes for K2

**With no code change:**

- A person's keystroke and the tick wait behind at most the agent edit in progress, however
  many agents write.
- The undo budget counts what it holds (above).
- Less memory per open text doc on Yrs: the session's text is the only copy, the disk replica
  holds the file's texts, and holders share a text until it changes. The tick returns freed
  memory after write-backs, every 30 s at most.
- No store migration, wire still 3. The native engine stays off.

**To wire:**

- **Commit durability changed live:** `SessionManager::set_commit_durability(CommitDurability)`
  makes every change before it durable, then applies the new setting with no restart. Map its
  refusals: `SessionError::Released`, and `MPC-E526` `store_recovering` (exit 1, retry after the
  next tick). `SessionManager::config_now()` gives `capabilities` as the store runs.
- **Error ids to map:** `MPC-E527` (exit 6), from a store write that holds the other engine's
  entry or stamp.
- **The memory release:** a host built with another allocator passes its call as
  `Config::release` (`HostRelease::new(f)`); the kit calls it after cuts and, now, after
  write-backs. Set `Config::release_after_write_ms` to change how often (0 turns it off).
- **Client 0.24.0:** the version only.

### Overview

A person's edit never waits behind an agent flood; the store checks that every log entry fits
its doc's engine; commit durability can change live; a container's own record, so sibling edits
don't conflict on revert or `--base`; less memory per text doc (one copy of its text, shared
holders, a replica of texts only, an undo budget at its real size, freed memory returned after
write-backs); the native engine's documents keep one writer's stable point, and its cut holds
less memory (off in this tag).

### A person's edit never waits behind an agent flood

- **Fixed:** a doc's session lock and the store's writer turn (one write at a time across every
  doc in the store) went to whoever took them first once free, so with the edit rate limit
  lifted, agents writing back to back could pass over a person's keystroke (and the manager's
  tick) for seconds, on the same doc or from other docs in the same store. Both locks now keep
  that cheap path but bound it: a waiting call passed over by 128 later agent calls is handed
  the lock next, and a person's call (or the tick) is handed it ahead of agents as soon as it
  waits, so it waits behind at most the agent edit in progress. While a person waits for a
  doc's lock, the agent holding it writes to the store ahead of other agents too. Nothing else
  changes: no API, store format or default; the rate limit is as it was. Behaviour change: the
  order waiting calls are let in at these two locks (whoever won the race, now bounded, with
  people and the tick first).
- Rows: `session/fair_lock_tests.rs`, timing-free (a test-only count of the later agent calls
  let in ahead of each waiting call; 31 agents flat out and the tick every 20 ms; every person
  edit and every tick passed over at most once, every agent call at most 128 times):
  `overtakes_a_person_behind_31_agents_at_most_once_each` (the agents on the person's doc),
  `overtakes_a_person_on_one_doc_behind_31_agents_on_another` and
  `overtakes_a_person_behind_31_agents_on_31_other_docs` (the agents on other docs in the same
  store); and four typist rows (a person at 8 keystrokes a second, the tick every 250 ms, the
  agents flat out or at the default rate limit, on one doc or 31), printed with the agents'
  edits a second and the typist's and the tick's waits, under a loose 2 s. Liveness under
  random load: `fair_tests.rs` `the_locks_stay_live_and_bounded_under_random_load` (random
  lanes, hold times, participants coming and going, 1 or 4 docs over the store turn, the
  tick; a progress watchdog; the bounds) and
  `a_holder_unwinding_poisons_the_lock_as_std_does_and_leaves_the_queue_clear`;
  `session/fair_lock_tests.rs` `sessions_stay_live_and_bounded_as_people_and_agents_come_and_go`
  and `an_edit_that_unwinds_leaves_the_session_usable_and_its_lock_clear`. Plants
  `session-lock-barging`, `store-gate-barging` (each lock as before) and `fair-lane-skipped` (a
  person's call in the agents' lane), each caught alone, in the plants part.

### The store refuses a log entry, or an engine stamp, its doc's log doesn't fit

- **Fixed:** `DocStore::append` put entries in a doc's log without checking they were of the
  doc's engine, so a Yrs entry could be logged in a native document's log (or a native entry in
  a Yrs document's), which its next load then refused as damage. Every store write that logs an
  entry (an append, a commit, an import's create, a promotion of a blob doc) now checks each
  entry first and refuses one of another engine, typed, before anything is written:
  `StoreError::EntryEngineRefused` (new id `MPC-E527` `entry_engine_refused`, exit 6; details
  `doc`, `entry` naming the entry's engine, `stored`, `stored_format`), the log and the document
  as they were.
- **Fixed:** the same mix through a document's metadata: `DocStore::put_meta` (and a commit's or
  a write-back's metadata) could change a document's engine stamp between the native engine and
  Yrs while its log held the other engine's entries, leaving a store that refuses its own
  document at load. Such a stamp change, on a document that holds log rows or a snapshot, is now
  refused, typed, with nothing written: `StoreError::EngineStampRefused` (`MPC-E527`; details
  `doc`, `entry` naming the engine of its rows, `asked`, `asked_format`). A stamp change within
  one engine (another library version or format, an unstamped document) and one on a document
  with no rows are unchanged; such a document's load decides, as before.
- Without the native engine, a write to a native document's log is refused as its load is,
  `StoreError::NativeEngineMissing` (`MPC-E169`). No kit path writes such an entry or stamp;
  honest writes are unchanged. No store format or default changes.
- Rows: `store/tests.rs` `an_entry_of_another_engine_is_refused_at_every_log_write` (a native
  entry appended to a Yrs document, a Yrs entry to a native one, both ways at an import's
  create, a promotion; honest appends and creates beside them) and
  `an_engine_restamp_under_a_log_is_refused` (`put_meta`, a disk commit and a write-back's
  synced state; a log, a snapshot alone, a native log; a stamp within one engine and a
  document with no rows beside them). The rows that build a mixed log or restamp a document for
  the load's own checks now write past the store (`put_meta_unchecked`, test builds and feature
  `test-support` only), as only damage or another build leaves one. Plants
  `append-engine-check-skipped` and `restamp-engine-check-skipped` (each check skipped, as
  before), each caught alone in the plants part.

### Commit durability changed live, with no restart (host API)

- **`SessionManager::set_commit_durability(CommitDurability)`** (new; and
  `DocStore::set_commit_durability`): a host's "every change on disk" or "grouped saves"
  setting takes effect at once. Every change before the call is made durable first, as by
  `SessionManager::flush()`. After it, at `Immediate` every change is durable when its call
  returns; grouped, changes group under the new bounds from the next change. A change in
  flight on another thread commits wholly under the setting it began with. Refused
  `SessionError::Released` on a released manager, and `MPC-E526` `store_recovering` (exit 1:
  retry after the next tick) while a commit that failed at the disk waits for the store's
  reopen; a refused switch leaves the setting as it was.
- **`SessionManager::config_now()`** (new): the manager's config with the store's settings as
  they are now, so `capabilities` from it lists `store_commit_grouped`,
  `store_commit_max_changes` and `store_commit_max_ms` as the store runs.
  `SessionManager::config()` still reports them as the manager was made. `DocStore::settings()`
  now gives the commit durability in effect, and `DocStore::commit_durability()` (new) that
  alone.
- **Stored:** a switch to grouped sets the grouped open mark in its own commit, so an abrupt
  stop after it is a recovery, as after a grouped open; a switch to `Immediate` removes it, as a
  clean close does.
- Rows: `session/grouped_commit_tests.rs`
  `a_switch_to_immediate_makes_the_group_durable_at_once`,
  `a_switch_to_grouped_groups_the_next_changes`,
  `a_switch_racing_edits_loses_and_tears_nothing` (three threads editing while the durability
  is switched over and over, then a power loss with the editors running),
  `a_switch_is_refused_released_or_rolling_back`. Plants, each caught alone:
  `durability-switch-skips-flush`, `durability-switch-mid-group`.

### A container's own record: no false conflict between siblings (index row format 3)

A tier-0 revert, and a call with `--base`, refused when an edit to one child of a container met
a later edit to another child: a pack whose container record holds its children's (a slide's
whole XML) showed the container changed both times. Reverting one shape's edit conflicted with
any later edit to another shape of the same slide.

- **`DraftNode::own`** (new): the node's own record's digest, its record with each child's
  span replaced by a stand-in naming the child. A pack sets it on a node whose record holds its
  children's. Stored keyed as **`IndexNode::own_hash`**.
- **`NodeChange::by_children`** (new): the node's change is its record alone, its own hash is
  equal in both versions, and a child of it changed in the same diff. The overlap check
  `--base` and a revert share leaves out a node when both sides' changes to it are so; its
  children are compared themselves. A change of the container's own (its children's order, an
  added or removed child, an attribute, a field the index doesn't show) still conflicts
  (`MPC-E723`, `MPC-E731`).
- **Known limit:** a node named after a child (a slide after its title) is `renamed` when that
  child's text changes, so a title edit still conflicts with a later edit to another shape of
  the slide.
- **Stored: index row format 3** (`INDEX_ROW_FORMAT` is 3): format 2 plus each node's own hash.
  A row with no own hash is written as format 2 (or 1), byte for byte as before. The index wire
  goes to version 3 for a draft with one.
- **`IndexSpec::with_own_records()`** (new): a pack declares own digests; a draft with any from
  a spec that doesn't is refused (`MPC-E714`), nothing recorded. **The kit picks the stored
  format:** a declared spec's rows and minted keys go under `format + OWN_FORMAT_OFFSET` (new,
  2^24), read in order: that format, the pack's `format`, its older format. A pack needn't bump
  its own format for own digests.
- **Older kits:** a previous release never asks for the offset format, so it finds such a
  version pending and indexes it under its own, whatever the pack's format; it never decodes a
  row of format 3. Back here that version reads with its own hashes absent until indexed
  again, and **minted keys re-mint after such a round trip**, as after any format bump. Rows of
  format 2 it wrote read here unchanged. `tools/n1/n1.sh layout-own` / `node-own` checks both
  ways (`n1_own_producer`, `n1_own_after`, `tools/n1/consumer-own`).
- **The pptx pack declares own records** (slides and group shapes carry their own record's
  digest); its index format stays 2. Rows of format 2 read with the own hashes absent and are
  indexed again by the next index job, under the kit's offset format.
- Rows: `tests/tier0_revert.rs` `revert_container_children_alone`,
  `revert_container_own_change_refused`; `tests/tier0_base.rs` `base_container_children_alone`,
  `base_container_own_change_refused`; `tests/blob_index_v2.rs`
  `own_digests_are_stored_and_an_older_row_reads_without_them`,
  `own_records_live_out_of_an_older_kits_reach`; the row and wire codecs' and the stored format's
  unit rows. Plants: `container-always-counted` (the count before), `container-never-counted`,
  `own-offset-skipped` (`MPC_INDEX_MUTANT`: the pack's own format used).

### The undo history at the size it holds (memory)

The undo budget now counts the bytes each kept batch really holds. Before, it counted about a
third of them: at 126,690 changes the kit counted 445,490 B for 1,351,681 B held. Each kept
batch is also trimmed to what it holds; a one-region batch had kept four region slots.

- **`Config::undo_max_bytes` now bounds the bytes held** by a person's undo and redo stacks:
  - its slots and its regions' slots;
  - capacities, not lengths;
  - the roots' lists.
  The default is unchanged (4 MiB per person). **A host that sets `undo_max_bytes` should
  know the same number now holds fewer batches**. In the measured doc (126,690 changes, 900
  kept batches) a batch counts 706 B against 495 B before, so the same budget holds about 30%
  fewer, while each batch holds half the memory it did. A person whose edits are large keeps
  fewer undo steps under the same budget; the oldest go first, as before.
- Nothing stored changes.
- Rows: `session::undo_budget_tests`. `tests/history.rs` `undo_bounds_are_per_pid`'s budget
  goes from 600 to 2,000 B, since a batch there now counts about 1 KB. Plant:
  `undo-batch-unshrunk`.

### Freed memory returned after write-backs (memory)

A Yrs doc makes no cut, so what its write-backs and outside saves free was never returned to
the OS. At 126,690 changes that left about 28 MiB of the process's resident memory with the
allocator. The tick now releases after it wrote a doc back.

- **`Config::release_after_write_ms`** (new; default 30,000; 0 never): after a tick that wrote
  a doc back, the release runs at most once in this many ms. It is the host's
  (`Config::release`) when the host gave one, else the kit's own (glibc's `malloc_trim(0)`).
  It is skipped when the tick's cut already released.
- **`TickReport::memory_released_after_write`** (new): what that release did, `None` when it
  didn't run.
- `capabilities` lists `release_after_write_ms`.
- Nothing stored changes; merges and outcomes are as before.

### A text doc's texts shared by their holders until they change (memory)

A text doc's session, its disk replica and its read bases each held their own copy of the
same text: every write-back and every read base copied the live text. They now share one copy
until the live text changes. An edit gives the live text a new copy and leaves the other
holders' as they were, so the replica and each read base keep exactly the text they took.

- After a write-back with no edit since, the doc holds its text once where it held it twice. A
  read base taken with no edit since adds no copy, and neither does an outside save the doc then
  holds (the replica takes the live text).
- Under steady edits the live text gets a new copy per edit, as it did.
- Nothing stored changes; merges, outcomes and files are as before.
- Rows: `session::writeback_diff_tests` (one allocation until an edit; an edit after sharing
  leaves the replica's and the read base's text as they were, and a `write --base` at the shared
  base merges from it; outside saves after sharing against a decoded replica) and
  `session::grouped_commit_tests` (a rollback reload after sharing).
- Plant: `shared-text-written-in-place`.

### A text doc's text is held once on Yrs (memory)

On Yrs the session held each text root's text, and the engine adapter held a second copy, its
text cache. A text doc's session now owns the only copy. Edits are planned on the session's
text; the engine keeps none, and its reads walk the doc. That saves one text per Yrs text doc,
0.77 MB at 126,690 changes.

- **The canary checks the session's text.** On a sampled edit it is compared with a fresh read.
  On a mismatch:
  - it is reported (`AuditEvent::TextCacheMismatch`);
  - the edit is refused, typed (`DocError::Apply`), with nothing applied;
  - the session takes the doc's texts back, so a retry lands where the doc's text puts it.
- Structured docs keep the engine's cache. The native engine is unchanged.
- **The write-back passes the session's texts to the disk replica**, so it never walks the doc.
- Nothing stored changes; merges, outcomes and files are as before.
- Rows:
  - `session::writeback_diff_tests` compares the session-owned texts with the engine's cache in
    lockstep, and checks the engine keeps no text;
  - `session::engine_check_tests` `a_wrong_session_text_is_reported_and_never_used`;
  - the cache row runs on the reference switch.
- Plant: `session-text-stale`.

### A text doc's disk replica holds its texts only (memory)

On Yrs, a text doc with a file kept a second decoded copy of the doc as its disk replica:
about the size of the doc itself, growing with its history. The replica now holds the file's
texts, as it already did on the native engine. `docs/SESSIONS.md`, "Disk sync".

- **Nothing stored changes:** the synced state is still the engine's encoding, with the same
  rows and bytes. Older and newer kits read each other's stores as before.
- **The one step that needs the replica's elements decodes on demand.** That step is an
  outside save the doc doesn't hold, stored as a client of its own. It decodes the stored
  synced state, makes the save on it and drops it. The sync check's element comparison and a
  purge's re-encode decode it the same way.
- **The decode is checked.** The stored state must be the one the session synced and hold the
  replica's texts. Otherwise the save is refused with `DocError::Load`: nothing is merged or
  stored, and the doc keeps its text.
- **What a host sees:** the same merges, files and stored states; less memory per open text
  doc. A structured doc's replica is unchanged (a decoded doc).
- Rows: `session::writeback_diff_tests` compares the text-only replica with the decoded one in
  lockstep (files, live docs, stored states, merge bases, the sync check) and steps through the
  save the doc doesn't hold. It also checks what the replica holds, and the refusals.
  `tests/disk.rs` keeps the restart repair row on the decoded replica, beside the text-only
  one. Plant: `yrs-replica-state-from-texts` (the plants part).

### The previous release's rewrite of a store holding verb records (checks only)

- **The N−1 verbs check** (`tools/n1/n1.sh layout-verbs`, then `node-verbs`) takes a previous
  release that knows `blob_verbs` (v0.22.0 and later): that release rewrites the store, and this
  build holds the live doc's record byte for byte to what it wrote, after that rewrite and after
  its own. Against v0.21.0 the rewrite is still refused, naming the table (`NewerData`).
- **Row** `store::verbs::tests::a_rewrite_keeps_every_blob_verbs_row_byte_for_byte`: a verb's and
  a batch's records (version 1) and a revert's (version 2) come through a rewrite byte for byte.
  Plant `MPC_STORE_PLANT=rewrite-drops-blob-verbs-row` (the rewrite leaves the table's first row
  out), caught at that row's assert.
- No store, wire or API change.

### The tier-0 tests build for Windows, two test fixes, and the cut's guard (tests and tools)

- **`tests/tier0.rs` and `tests/tier0_verbs.rs` build for Windows.** The rows that need a
  read-only folder or a symlink run on Unix only (on Windows a folder's read-only attribute
  doesn't stop a file being made in it, and a symlink needs a privilege); the file-identity
  helpers read the inode on Unix, and the bytes, size and modification time elsewhere. Linux and
  macOS run every row as before.
- **`head_moved_mid_plan` counts its own plans:** the index test pack's plan counter is per
  thread, so rows running beside it in one test binary leave its count alone.
- **The public cut's guard reads the engine words it refuses from outside the tree**
  (`tools/check-public-cut.sh`), as `tools/check-private.sh` reads its patterns:
  `MPC_ENGINE_PROSE_WORDS`, `MPC_ENGINE_PROSE_WORDS_FILE` or
  `~/.config/k2-mpc/engine-prose-words.txt`. With none, as in a public cut's copy, that part of
  the labels rule is off and the run says so on a `check-public-cut (note):` line;
  `MPC_ENGINE_PROSE_REQUIRED=1` makes none an error. The gate's plants take their words from
  the same list.
- No product code changed.

### Every native Beta plant runs in the plants part (tests)

Thirty of the native Beta's test-only plants (`MPC_BETA_PLANT`) were documented beside their
rows but never run by the gate. Each now runs alone in the plants part (`tools/tests/full.sh`,
`PART=plants`) after a green control of its suite, and must fail exactly the rows its docs name,
at their own assertions: twelve without the native engine (the cut cadence's settings and
rules, the cut queue, a native refusal's exit, the memory release) and eighteen with it (the
Beta switch, the engine's limits, the host's cadence, writer rows, the text replica, the sync
refusals, the text cap and its code, the store salt, the cuts at a tick, an unload and a
close, a copy on the doc's engine, the engine chosen at create, and the refused demotion and
promotion). Five break other rows' setup too (a doc that can't load, nothing queued), so they
run over their named rows alone. No product code changed.

### Native documents keep one writer's stable point (feature `native` only)

- **Every native document a store holds, or a session opens, now runs in the native engine's
  single-writer mode.** Its stable point follows its latest change: each change is stored with
  its stable record, in the change's own transaction, so a commit (immediate or grouped) never
  leaves a change on disk without it. A native document built in memory from another document's
  state (`Doc::load_on`, `Doc::new_on`, the copy blame's sampled check makes) stays outside the
  mode: no store holds it.
- **The open writes a lifted stable point.** When a store's latest stable record is behind its
  last change (a store written by an earlier kit, or a commit cut short), the open moves the
  stable point to the last change and writes that record before the document is handed out.
  Grouped, the write is durable with its group; if it is lost, the next open does it again, to
  the same change. If the store changed between the open's read and that write, the open fails
  with nothing written; open it again.
- **A native document's cut** leaves the stable point where it is (it is already at the last
  change), and keeps the document's memory at what a fresh open holds after each cut.
- **Sync calls on a native document a store holds are refused:** `Doc::try_state_vector` and
  `Doc::state_vector_parts` now return `MPC-E166` (`native_one_replica`), as `Doc::encode_diff`
  already did. The deprecated `Doc::state_vector`, which can't fail, returns the native engine's
  wire tag alone for such a document: never a vector a peer could take as what the document
  has (every receiver refuses it), and it always returns. Use the typed calls.
- **Three engine codes join the kit's table**, each with its next step: `MPCE-E1008`
  (`single_replica_mode`, a sync call on a one-copy document), `MPCE-E1010`
  (`checkpoint_over_limit`) and `MPCE-E1013` (`stable_not_at_head`, a cut whose stable point
  isn't at the last change).
- **The memory figure moves:** with the mode, a native document's live memory grows with the
  changes since its last cut and returns to a fresh open's at each cut.

### Edit hunks end alike on both engines (feature `native` only, a correctness fix)

- **No change to what an edit's hunks come to:** the kit checks them before either engine, so a
  start after its end, hunks out of order or overlapping, two insertions at one offset, an empty
  insertion and an offset off a character boundary or past the text are refused with the same
  `MPC-E151` (its problem and hunk index) on Yrs and on the native engine, as before.
- **The native engine's own hunk refusals name the hunk:** were hunks ever to reach the native
  engine unchecked, its refusal now comes back as the kit's check names it (order, range or
  empty, and the hunk's index), not always as a range problem at hunk 0. No kit entry passes it
  unchecked hunks today.

### A native document's cut deletes its old rows a page at a time (feature `native` only)

- **Fixed:** a native document's cut deleted its old generation's records, and the log rows it
  trims, with the store library's range-retain, which copies a page for every row it deletes
  (redb 2.6), even a page the cut's own transaction has already copied. The copies then sat in
  the store's page cache: about 1,400 page buffers per cut on a 3,000-change document with
  eight agents, and the process held about 4 MiB more than it needed between cuts. The cut now
  reads the keys and removes them one by one in the same transaction, so each page is copied
  once. The same rows go, in the same one transaction: the store's rows after a cut, and after
  a cut stopped part-way, are byte for byte what they were. No store format, setting or API
  changes.

### The native engine at a newer development version, and records lent to it (feature `native` only)

- **The vendored engine crates** (feature `native` only, not for hosts; the native engine stays
  off, and a public cut has none of them) move to a newer development version of the native
  engine. Public API additions only: a single-writer mode a host may turn on (off by default;
  the kit turns it on for the documents a store holds, above); a typed refusal when a
  document's store changed under it; typed limits on a checkpoint's size; and key material
  wiped from memory when it is dropped. A load does less work and holds less memory; what a
  loaded document reads, edits and undoes is unchanged.
- **A new dependency, feature `native` only:** `zeroize` 1.9.2 (Apache-2.0 OR MIT, no default
  features), in `Cargo.lock`. A public cut has no native engine, so its lock has no `zeroize`.
- **The reference model** (`mpce-model`, with `mpce-order` and `mpce-reg`) stays at its vendored
  version: the newer engine doesn't change those three crates, and the vendored copies differ
  from them only by their public-text edits.
- **A store v0.23.0 wrote** still opens on this engine in the single-writer mode, takes an edit
  and a cut, and reopens with its text, as the kit's previous-release row asserts.
- **No copy of a record the kit holds in memory.** The kit's record sources give the native
  engine the records they hold in memory lent rather than copied: every record of a document no
  store holds, and a stored document's pending records and pending purge's rewrites, through a
  load's and a cut's reads too. A record only the store holds is still read from the store as a
  copy (a read from the store lasts only as long as its own lookup). The same bytes either way.

### A copied or restored native store's writers are fresh (feature `native`)

`docs/SESSIONS.md`, "Person-writer reuse". A follow-up to v0.23.0's writer reuse.

- **A restored or copied native store's agents and host-device persons get fresh writer
  ids.** An open that rotates the store's writer incarnation (a copy, a move, a restore written
  as a new file, an unclean close, a missing incarnation) retires, in the same transaction,
  every writer a join could reuse: each agent's (`Config::reuse_agent_clients`) and each
  person's on a host's device, as a grouped store's recovery already did. Before, those
  writers were reused on a restored older store and wrote again seqs the original had held
  with other content. Cost: one fresh writer per agent and per device after each copy,
  restore or unclean close; names don't change (blame and the doc's owners name the same
  participant). A person's retired writer keeps its row, marked retired (the row's state byte
  gains a retired bit; a kit before it reads such a row as unknown, never reused), so a late
  reclaim of its last lease is still told when that lease ended (`Joined::prev_ended`); a
  grouped store's recovery now keeps it too. With the native engine built in, a host-device
  person whose process ended takes a fresh writer at its next join, even after the park
  expiry (`tests/person_writers.rs` R4, restated). N-1 (`tools/n1/n1.sh node-retired`, and
  `node-retired-prev` the other way): the previous release opens a store holding a retired row,
  never lets that writer again (a free one it lets, the control), and this build reads the
  store back; this build reads the previous release's rows field for field.

## v0.23.0

A host release on v0.22.0. The crate version is `0.23.0`; the client's is `0.23.0`. The live
wire stays version 3. No store needs a migration: `STORE_FORMAT` stays 6, `STORE_FORMAT_BASE` 5
and `STORE_FORMAT_READ` 8. Pairs with the pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.22.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### Breaking

- **A verb on a blob doc plans with no lease** (a behaviour change). The automatic lease is
  held only around a tier-0 verb's commit, so agents' verbs on one doc run at once where they
  refused each other with `MPC-E701`; a verb whose head moved plans again, up to
  `BASE_REPLANS_MAX` times. A lease someone took themselves, and a whole-file `write` in
  progress, still refuse with `MPC-E701`.
- **The store's cache is 64 MiB, where it was the store library's 1 GiB** (a behaviour
  change). `DocStore::open` and `open_with_retry` use `StoreSettings::DEFAULT`; a host with very
  many docs whose system cache is often cold may want more (`DocStore::open_with`,
  `StoreSettings::new`, 16 MiB to 1 GiB).
- **The blob tier's registry lists `undo`, `redo` and `revert`** (`Registry::for_tier(Tier::Blob)`,
  `Registry::for_blob_pack`). A host that offers a blob doc's verbs from the registry runs these
  three through `BlobSession::run_revert`; `BlobSession::run_tool` doesn't run them (a usage
  error). A pack with a tier-0 parity list lists Undo and Revert live at its next kit update.
- **`CutCadence::new` takes two more values,** `bytes_from_changes` and `bytes_divisor`
  (`CutCadence::BETA` holds the defaults); it refuses a byte rule from 0 changes and a divisor
  of 0. Native docs only; the native engine stays off.
- **Native docs only (feature `native`, off by default):** the memory release also runs after a
  clean unload's cut and a host's close's cut; a host's close cuts a native doc with changes
  since its last cut; a native update that sends a change the doc already holds with other
  content is `MPC-E183` `native_equivocation`, where it was `MPC-E161`; a person's join with no
  device key on a native doc takes the store as its device (`Config::reuse_person_writers`).
- **`LeaveReason::RolledBack`, new, and `LeaveReason` is `#[non_exhaustive]`:** a host that
  matches it whole adds a wildcard arm. A connection leaves with it (wire `rolled_back`) only
  with grouped commits turned on, after a commit failed at the disk and the doc reloaded at the
  store's durable state; it joins again under a fresh writer.
- **A grouped store that recovered lost versions sets the required feature `FEATURE_SEQ_FLOOR`:**
  kits v0.19.0 to v0.22.0 then refuse it at open (`MPC-E502` `store_newer`). A store that never
  ran grouped, or closed cleanly, is unchanged.
- **`capabilities` lists more:** `blob_batch_max_calls`, `store_cache_bytes`,
  `store_commit_grouped`, `store_commit_max_changes`, `store_commit_max_ms`, `host_release`,
  `native_cut_bytes_from_changes` and `native_cut_bytes_divisor`, and the blob tier on `undo`,
  `redo` and `revert`. A reader that compares it whole (a saved golden) sees the new lines.

### What changes for K2

**With no code change:**

- Agents' verbs on one blob doc plan at the same time (above).
- The store's cache is 64 MiB.
- Commits are as before: `CommitDurability::Immediate`, the default, puts every change on disk
  before its call returns.
- The native engine stays off; its settings are as in v0.22.0 plus the byte rule's two.
- No store migration, wire still 3. A store holding a tier-0 revert's `blob_verbs` row (record
  version 2) opens in v0.22.0, which reads it as a version with no verb.

**To wire:**

- **Undo, redo and revert on a blob doc:** `BlobSession::run_revert(conn, what, args, opts)`,
  with `RevertWhat::Undo`, `RevertWhat::Redo` or `RevertWhat::Version(K)`. `args` take
  `--if-version V`, `--dry-run`, `--expect DIGEST` and `--json`; `--base` and reverting version
  1 are usage errors. Undo and redo take one step of the caller's own; revert needs the
  version's author, the person it was made for, or an owner (`MPC-E105`). The result is a verb's
  (`ToolOutput::version_made()`), with `changed.word` `reverted`, `undone` or `redone` and
  `data` `{reverts: K}`; nothing to undo or redo exits 1 with a note and writes nothing. New
  refusals: `MPC-E730` `revert_unsupported` (exit 2; the next step is `restore --version
  <K-1>`) and `MPC-E731` `revert_conflict` (exit 1, `details.conflicts` naming who changed
  what); also `MPC-E719` across another program's save, `MPC-E726`, `MPC-E727`, `MPC-E728`,
  `MPC-E301` and `MPC-E110`.
- **A batch of verbs on a blob doc:** `BlobSession::run_batch(conn, args, calls, opts)`, one
  `BatchCall::new(argv)` per call (a verb of `BlobPack::verbs` with its arguments, as `run_tool`
  takes them), as one version. `args` take `--if-version V` or `--base V`, `--dry-run`,
  `--expect DIGEST` and `--json`; a call carrying its own guard is a usage error naming it. `$N`
  and `$N.k` name a node call N created. At most `Config::blob_batch_max_calls` calls (default
  32, `MPC-E728`). The result is a verb's, with `changed.word` `batch` and `batch`:
  `{calls, failed_at}`. `batch` is a host call, not a registry row (a pack verb can't be named
  `batch`). No new error ids.
- **A hold binds an agent's whole-file write** when the host asks: pass
  `BlobWriteOptions::policy(ClaimPolicy::Respect)` on an agent's `write_with` and
  `write_bytes_with`, and use `BlobSession::expect_save_with`. Map `MPC-E306`
  `hold_unindexable` (exit 5, `details.holders`). With no policy (`Ignore`) nothing changes.
- **Grouped commits, opt-in:** `StoreSettings::with_commit_durability(CommitDurability::grouped(max_changes, max_ms))`
  (1 to 1,024 changes, 1 to 10,000 ms) at `DocStore::open_with`: one wait for the disk per group.
  Then `EditResult::durable`, `SessionManager::durable_through()` and `SessionManager::flush()`
  say what is on disk; map `MPC-E526` `store_recovering` (exit 1, retry) and the new
  `AuditEvent::StoreRolledBack` and `AuditEvent::RolledBack`; a connection that leaves with
  `LeaveReason::RolledBack` joins again and reads the doc again.
- **Error ids to map:** `MPC-E306`, `MPC-E526`, `MPC-E730`, `MPC-E731`; `MPC-E183` for native
  docs.
- **The memory release (native docs only):** `Config::release` takes a host's own release
  (`HostRelease::new`), for an allocator glibc's trim doesn't reach; unset, the kit trims glibc's
  heap as before.
- **Client 0.23.0:** the leave reason `rolled_back`, which `canRetry` retries; no other API
  change.

### Overview

Undo, redo and revert on a blob doc, through its file type's merge; a batch of verbs as one
change; agents' verbs on one doc plan at the same time; holds that bind an agent's whole-file
write; a smaller store cache by default; grouped commits, one wait for the disk per group
(opt-in); the native engine's cuts run through sessions, with a host's own memory release (off
in this tag).

### Revert, undo and redo on a blob doc

`docs/SESSIONS.md`, "Revert, undo and redo on a blob doc".

- **`BlobSession::run_revert(conn, what, args, opts)`** and **`session::RevertWhat`** (new):
  `Version(K)` takes version K's change away from head as a new version; `Undo` and `Redo`
  act on the caller's own versions.
  - The file type's own merge does it (`merge3_known` with base K, ours head and theirs K-1;
    the new `SaveBase::Revert`).
  - First the kit checks, node by node, what K changed against what changed since.
  - The output then goes through a verb's checks: the index, nodes outside K's (`MPC-E726`),
    claims under `Respect` (`MPC-E301`), the dry run, and one commit.
- **Who:** a revert needs the version's author, the person it was made for, or an owner
  (`MPC-E105`). Undo and redo only ever take the caller's own versions.
- **New ids:**
  - `MPC-E730 revert_unsupported` (exit 2): no merge, or the merge can't take these bytes; the
    next step is `restore`;
  - `MPC-E731 revert_conflict` (exit 1): changed since, with `details.conflicts` naming who.
- **Across another program's save** the pack doesn't prove, a revert is refused with
  `MPC-E719`, its `details.base` K-1.
- **The undo stack** is derived from `blob_verbs`, with no new table: a verb's, batch's,
  revert's or redo's version can be undone, and a new change clears redo. A revert's row is
  record version 2 (`BlobVersion::revert`: `store::Revert`, `RevertStep`;
  `BlobLogEntry::reverts`), which an older kit reads as a version with no verb.
- **Rows:** `tests/tier0_revert.rs`:
  - `revert_one_version`, `revert_conflict_names_who`, `revert_needs_author_or_owner`;
  - `undo_own_only`, `undo_redo_roundtrip`, `redo_cleared_by_new_edit`, `undo_of_batch`;
  - `revert_unproven_refused`, `revert_dry_run_matches_real`, `revert_held_refused`;
  - `revert_unsupported`, `revert_stray_refused`, `revert_flags`.
- **Plants**, each caught alone: `revert-takes-whole-version`, `revert-overwrites`,
  `revert-ignores-author`, `undo-latest-any`, `redo-not-cleared` and `revert-skips-claims`.

### Undo, redo and revert are tier-0 verbs (pack maintainers: a change to your parity list)

The person actions **Undo** and **Revert** are on tier 0 now, so the agent-parity
rule covers them there. The registry rows `undo`, `redo` and `revert` gain the blob tier
(`Registry::for_tier(Tier::Blob)`, `Registry::for_blob_pack`), and the blob parity harness
runs them through `BlobSession::run_revert`. The kit's tier-0 cases (`kit_blob_cases`) are:

- `undo` after the actor's own write;
- `redo` with nothing undone;
- `revert` of the actor's own version.

On a file type with no index or no merge, these show that the person path and the agent path
refuse alike.

- **Every pack with a tier-0 parity list must change it at its next kit bump.** Drop `Undo` and
  `Revert` from its "text docs only" arm, and list them live:
  `ParityRow::live(a, &["undo", "redo"])` and `ParityRow::live(a, &["revert"])`. Until it does,
  the list check fails (`MissingRow` for both actions).
- Usage on tier 0:
  - `undo` and `redo` take one step of the caller's own;
  - `revert <version>` takes a version and no `--partial`.
- Rows:
  - `tests/parity_blob.rs` and `tests/blob_read.rs` list the two actions live;
  - the registry's blob-tier list includes `undo`, `redo` and `revert`.

### A batch of verbs on a blob doc

`docs/SESSIONS.md`, "A batch of verbs". An agent's calls of a file type's edit verbs, made as one
change.

- **`BlobSession::run_batch(conn, args, calls, opts)`** and **`session::BatchCall`** (new). Each
  call plans on what the calls before it staged (head plus one composed list of ranges, read
  with the file type's index after every call); `$N` and `$N.k` name a node an earlier call
  created. One version, one `blob_verbs` row (the verb `batch` and its calls' names), one index
  recorded from the last stage. All or nothing: any refusal applies nothing.
- **Guards are the batch's** (`--if-version`, `--base`, `--dry-run`, `--expect`); a call carrying
  its own, a read verb or a name that is no verb is a usage error naming the call. `--base V`
  folds the whole batch at `V` too and is checked per node over every call. Each `merge: Lease`
  call needs the caller's own lease (`MPC-E718`); under `ClaimPolicy::Respect`, each call's
  reach is checked against holds (`MPC-E301`). The batch plans with no lease; the automatic
  lease covers only its commit.
- **`Config::blob_batch_max_calls`** (new; default 32, at most `session::BATCH_CALLS_CEILING`,
  1,000; listed in `capabilities`): a batch over it is refused before any call runs
  (`MPC-E728`); a setting out of range refuses every batch, naming it.
- **The result** is a single edit verb's, with `changed.word` `batch`, plus `batch`:
  `{calls: [{verb, word, created}], failed_at}`, `failed_at` the 1-based call a refusal names.
  No new error ids.
- Rows: `tests/tier0_batch.rs` (`batch_one_version`, `batch_all_or_nothing`,
  `batch_sees_staged`, `batch_exact`, `batch_strayed_union`, `batch_guards_whole`,
  `batch_base_per_node`, `batch_claims_union`, `batch_lease_rules`,
  `batch_dry_run_matches_real`, `batch_caps`, `batch_cap_setting_checked`,
  `batch_seeds_index`); plants `batch-commits-each`, `batch-plans-on-head`,
  `batch-compose-wrong`, `batch-base-last-call-only`, `batch-claims-first-call` and
  `batch-lease-whole-run`, each caught alone.

### A verb on a blob doc plans with no lease (a behaviour change for hosts)

- **A verb plans with no lease:** the automatic lease covers only a tier-0 verb's commit
  (taken and released under the session lock), so agents' verbs on one doc run at once
  instead of refusing each other with `MPC-E701`; a moved head is planned again (a `--base`
  call per node, up to `BASE_REPLANS_MAX` times, `pub const`). A lease someone took themselves,
  and a whole-file `write` in progress, still refuse with `MPC-E701`; a dry run takes none.
  Rows `swarm_on_one_doc` and `person_lease_refuses_agent_verb`; plant
  `lease-held-whole-verb`.

### A hold binds an agent's whole-file write (a behaviour change for hosts)

`docs/SESSIONS.md`, "Claims on nodes". Until now a hold on a node (`claim --node`) refused an
agent's file-type verb but not its whole-file write: an agent could write over a node someone
held.

- **`BlobWriteOptions::policy(ClaimPolicy)`** (new; default `Ignore`) and
  **`BlobSession::expect_save_with`** (new). Under `ClaimPolicy::Respect`, while someone else
  holds a node, an agent's `write_with`, `write_bytes_with` and `expect_save_with` are read with
  the file type's index and refused (`MPC-E301`, naming the holder, nothing stored) when they
  change a node inside a hold's reach, as the agent's verbs are. A host passing `Respect` on
  writes gets this; one passing nothing (`Ignore`) sees no change.
- **`MPC-E306` `hold_unindexable`** (new, exit 5): while holds are present, an agent's write the
  index can't read within its limits is refused, with `details.holders` and a next step (use the
  file type's verbs, or ask the holders to release). Without holds it lands as before.
- A hold taken while an agent's write uploads refuses that write (naming the holder); the same
  call again is checked. A checked write seeds its version's index, so no index job is out for
  it. A person's write, a write under `Ignore`, a write with nobody else holding a node and a
  file type with no index are unchanged, with no index read. An agent's `restore` isn't checked.
- Rows: `tests/tier0_holds_write.rs` (`whole_write_respects_holds`, `whole_write_hold_reach`,
  `whole_write_person_never_refused`, `whole_write_ignore_unchanged`,
  `whole_write_no_holds_no_pass`, `whole_write_unindexable_with_holds`,
  `whole_write_hold_during_upload`, `whole_write_seeds_index`); plants `whole-write-skips-holds`
  and `hold-during-upload-missed`, each caught alone.

### The store's cache (a behaviour change for hosts)

- **The store's cache drops from the store library's default, 1 GiB, to 64 MiB.** The store
  library keeps the pages it reads and writes in the process's heap, up to a size; a store
  holds every doc of its host, so this is one cache per process, filled as pages are read.
  Left at 1 GiB it grew with the pages read: on a store of 1,000 Markdown docs (310 MiB),
  reading every doc left about 375 MiB of heap at 1 GiB against about 180 MiB at 64 MiB. With
  the system's page cache warm, no read, edit or load was faster above 16 MiB. Cold (the store
  file dropped from the system's page cache), a larger cache helps only once it holds every
  page read: on that store, loading every doc again took 1.4 s at 1 GiB against 3.0 s at
  64 MiB. **A host with very many docs whose system cache is often cold may want a larger
  value.**
- **`StoreSettings`, new:** the store's settings, made by `StoreSettings::new(cache_bytes)`
  (16 MiB to 1 GiB; outside it `StoreSettingsError::CacheOutOfRange { bytes, least, most }`) or
  `StoreSettings::DEFAULT` (64 MiB). Given at `DocStore::open_with(path, settings)` and
  `DocStore::open_with_retry_with(path, wait, settings)`; `open` and `open_with_retry` use the
  default, so a host gets 64 MiB with no code change. `DocStore::settings()` says what a store
  runs with. A rewrite (a purge's) opens its fresh file with the store's own settings.
- **`Config::store`, a new field** (`Config` is `#[non_exhaustive]`; start from
  `Config::default()`): listed by `capabilities` as `store_cache_bytes`. `SessionManager::new`
  sets it from its store's settings, so a manager's config reports the store as it runs.
- **The open with a wait keeps the store it finds free.** Before, it found the store free, let
  it go and opened it again, so another opener could take it in between and the open was
  refused `MPC-E505` although its wait had not run out. Each try is now the open itself. Row
  `an_open_with_a_wait_keeps_the_store_it_found_free`; plant `retry-probe-then-open`.
- Rows `the_cache_size_is_applied`, `a_rewrite_keeps_the_cache_size`,
  `an_open_with_a_wait_applies_the_cache_size`, `a_cache_size_out_of_range_is_refused` and
  `capabilities_list_the_store_cache_size`; plant `store-cache-ignored`.

### Grouped commits: one wait for the disk per group (opt-in; the default unchanged)

- **New, off by default:** `StoreSettings::with_commit_durability(CommitDurability)`. At
  `CommitDurability::Immediate` (the default) nothing changes: every change is on disk when its
  call returns. `CommitDurability::grouped(max_changes, max_ms)` (1 to 1,024 changes, 1 to
  10,000 ms; outside them `StoreSettingsError::GroupOutOfRange`) makes one commit wait for the
  disk per group; `capabilities` lists
  `store_commit_grouped`, `store_commit_max_changes` and `store_commit_max_ms`.
- **What grouped promises** (docs/SESSIONS.md, "Grouped commits"): a change is acknowledged
  when applied, visible and ordered, and durable with its group (`EditResult::durable`,
  `SessionManager::durable_through()`, `SessionManager::flush()`). Barriers make the group
  durable first: the file write-back (the file is never ahead of the store), a blob doc's
  write-back, a native cut, a delete, a purge, a demotion or promotion, a rewrite, a close, an
  unload and a release. After an abrupt stop or a power loss the store reopens as a prefix of
  its commits, losing at most the last group.
- **No version given out twice.** A per-doc seq floor, a recovery at the open after a grouped
  opener's unclean close (lost versions recorded, reusable writers retired), and a lost
  version refused as a base, `MPC-E112` `base_expired`. The floor covers the kit's own
  versions (a doc's log positions, and a blob doc's versions). A later update of the native
  engine adds its part: after a rollback, the native engine refuses state from before the
  reload.
- **A commit failing at the disk, grouped:** reads and writes of the store's docs are refused
  `MPC-E526` `store_recovering` (new; exit 1, retry) until the session manager's next tick or
  flush reopens the store at its durable state and reloads every live doc
  (`AuditEvent::StoreRolledBack`, `AuditEvent::RolledBack` per doc, both new).
- **A behaviour change for hosts and clients:** `LeaveReason::RolledBack` (new; wire
  `rolled_back`, which the JS client's `canRetry` retries): after such a reload every
  connection is closed and joins again under a fresh writer.
- **Breaking:** `LeaveReason` gains `RolledBack` and is now non-exhaustive; add a wildcard
  arm.
- **Store format:** grouped stores keep their seq floor, the open mark and lost ranges in
  `meta` rows, which older kits ignore. A recovery that recorded lost versions sets the new
  required feature `FEATURE_SEQ_FLOOR`: kits v0.19.0 to v0.22.0 then refuse the store at
  open, `MPC-E502` `store_newer` (v0.14.0 to v0.18.0 refuse format 8 already). A grouped store
  closed cleanly stays readable by them.

### Timers: a change's store commit

- **`timers::Path::CommitStore`, `commit.store` (new path; `Path` is `#[non_exhaustive]`)**: a
  session's store transaction for one change, its rows and the commit, with the wait for the
  disk when that commit makes them durable. One per change, beside `commit` (the whole commit
  with its bookkeeping) and `store.txn_commit` (the transaction's commit alone).
- Row `commit_cost_grouped_against_immediate` (`crates/base/tests/commit_cost.rs`; printed,
  not a gate, run by name in release): 2,000 one-character edits by a person and 2,000 agent
  verbs, on each engine, at `Immediate` and grouped (32 changes, 100 ms); the commit's p50/p95
  from `commit.store`, the rest of each call's p50/p95, edits per second and wall time.

### The host's memory release after a native cut (feature `native`)

- **Pass a release function to return freed memory with your allocator; the default trims
  glibc's heap.** `Config::release: Option<HostRelease>` (new; default `None`), with
  `HostRelease::new(f)`, `f` a plain `fn() -> MemoryRelease`. When it is set the kit calls it
  in place of its own release (glibc's `malloc_trim(0)` on Linux, nothing elsewhere). A host
  built with mimalloc, whose free pages the trim doesn't reach, passes mimalloc's collect. The
  kit never links an allocator itself. `capabilities` lists `host_release` (0 or 1).
- **The release also runs after a clean unload's cut and a host's close's cut** (a behaviour
  change for hosts), once the session is out of the pool: unloading is when a doc's memory is
  freed. The kit's own release gains those calls too.
- **`MemoryRelease::ByHost { returned: Option<bool> }`** (new variant; the enum is
  `#[non_exhaustive]`) for a host's release to report, and **`NativeCut::released`** (new
  field; `NativeCut` is `#[non_exhaustive]`): what the release after a made cut did.
  `TickReport::memory_released` is still the queued cut's.
- Rows `a_host_release_runs_once_per_queued_cut`, `a_host_release_runs_after_an_unloads_cut`,
  `a_host_release_runs_after_a_hosts_close_cut`,
  `the_kits_own_release_runs_after_both_cuts_by_default` and
  `capabilities_list_the_host_release`; plants `host-release-skipped` and
  `unload-release-skipped`.

### Native documents are cut: the cut queue runs in the tick (feature `native`)

`docs/SESSIONS.md`, "The native engine (a Beta)". Native docs only, not for hosts until the
native engine is; no store format change; no change for Yrs documents.

- **The cuts run.** Until now nothing cut a native document after its first records, so its
  stored records grew with every change and an open replayed all of them. Now, after each
  commit of a native document's records (a change, an outside save, a purge), its session asks
  whether a cut is due (the cadence's `CutCadence::due`; it cuts nothing) and, if it is, queues
  the document with its manager. `SessionManager::tick` runs the queue after everything else
  and before the idle unloads: one cut per tick at most, idle unless forced, within the duty.
  An open then reads the last checkpoint and the records written since.
- **A clean unload cuts.** A native document with changes since its last cut is cut on its
  way out of memory (an idle unload, or one to keep the memory budget), counted against the
  queue's duty.
- **Memory back to the OS.** Right after a queued cut the kit asks the allocator to return the
  memory it freed (glibc's `malloc_trim`; nothing yet elsewhere).
- **New in `TickReport`:** `native_cuts` (`NativeCut { key, ms, tail, why, outcome, slow }`,
  with the new non-exhaustive `NativeCutWhy::{Cadence, Unload, Forced}` and
  `NativeCutOutcome::{Done, Refused}`), `native_cuts_deferred` and `memory_released` (the new
  non-exhaustive `MemoryRelease::{Trimmed { returned }, NotOnThisPlatform}`). A cut that fails
  is in `TickReport::errors`. These are a native document's cuts; `versions_cut` stays the
  history's versions.
- **New `AuditEvent::NativeCutSlow { key, ms, tail }`** for a cut over the queue's
  `native_cut_slow_ms`.

### The native engine's cut cadence above 30,000 changes: the byte rule

`docs/SESSIONS.md`, "The native engine (a Beta)". Native docs only (feature `native`, not for
hosts until the native engine is); no change for Yrs docs.

- **The rule.** Up to 30,000 changes in total a native document's cut is due as before, once the
  changes since its last cut reach `T(n) = min(200, max(100, n / 30))`. Above 30,000 it is due
  once they reach `T(n)` **and** the bytes of the records written since the last cut reach a
  fortieth of the checkpoint's bytes, never before `T(n)`. A document is forced (cut while
  editing goes on) at twice the rule that governs: `2·T(n)` changes, and above 30,000 a
  twentieth of the checkpoint's bytes too. A large uncut document (its cut waits for idle) is
  one past twice the ceiling in changes; above 30,000, one past twice the forced threshold in
  both figures (more than `4·T(n)` changes and a tail of more than a tenth of the checkpoint's
  bytes), so a document under continuous edits is forced before it is ever left for idle.
- **New `CutCadence::due(n, tail_changes, tail_bytes, checkpoint_bytes) -> Option<CutDue>`**,
  the whole rule, with the new `CutDue` (`Cadence`, `Forced`, `LargeUncut`; non-exhaustive).
  The cut queue takes its reason from it: forced runs while editing goes on, the others wait
  for idle.
- **Breaking: `CutCadence::new` takes two more values,** `bytes_from_changes`
  (default 30,000) and `bytes_divisor` (default 40), with the accessors of the same names;
  `CutCadence::BETA` holds the defaults. It refuses a byte rule from 0 changes
  (`CadenceError::BytesFromZero`) and a byte divisor of 0 (`CadenceError::BytesDivisorZero`).
  `capabilities` lists them as `native_cut_bytes_from_changes` and `native_cut_bytes_divisor`.

### A host's close cuts a native document, as a clean unload does (feature `native`)

- `SessionManager::close` and `close_where` cut a native document with changes since its last
  cut before its session closes, as a clean unload does: counted against the cut queue's duty,
  reported in the next tick's `TickReport::native_cuts` (`NativeCutWhy::Unload`: the document
  left memory). Before, a host's close left those changes in the tail until the document was
  edited and cut again. Row `a_hosts_close_cuts_a_native_doc` (both engines, through `close`
  and `close_where`); plant (`MPC_BETA_PLANT`) `close-skips-cut`, caught at the close's cut.

### A native document's person keeps one writer across sessions (feature `native`)

`docs/SESSIONS.md`, "Person-writer reuse". No store format change; Yrs documents unchanged.

- **The store is the device.** A person's (or App guest's) join with no device key on a native
  document takes the store as its device (`Config::reuse_person_writers`, on by default): one
  writer across sessions on this store, a stateless join (`Session::join_shared`) at once, any
  other after a clean `Session::release` or once a parked writer's `person_writer_park_ms` is
  over. A host that sends a device key keeps its own. Before this, every session of a native
  document drew a fresh writer, which the document kept for good.
- **A copied or restored store rotates.** The store keeps an incarnation (six `meta` rows a kit
  without the native engine copies and never reads); an open draws a new one, so each person
  takes one fresh writer on that store, when the last opener didn't close it cleanly, when the
  store file isn't the recorded one (a copy, a move, a restore written as a new file), or when
  the incarnation is missing. A clean close records it; the kit's own rewrite records its new
  file (a rewrite stopped before that rotates at the next open). **For hosts:** restore a
  native store as a new file, never in place over the live file.
- **New `MPC-E183` `native_equivocation`** (exit 2, `DocError::NativeEquivocation`): a native
  update that sends a change the document already holds with other content: the native engine
  refused it. It was reported as `MPC-E161` `engine_unlisted`.


## v0.22.0

A host release on v0.21.0. The crate version is `0.22.0`; the client's is `0.22.0`. The live
wire stays version 3. No store needs a migration: `STORE_FORMAT` stays 6, `STORE_FORMAT_BASE` 5
and `STORE_FORMAT_READ` 8. Pairs with the markdown, text, board, csv, psd, pptx, timeline and fab
pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.21.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off. The
reference model's vendored copy is in it, at one engine commit (`vendor/README.md`).

### Breaking

- **`ReadResult::render_failed: Option<SessionError>`, a new field.** `ReadResult` is
  `#[non_exhaustive]`; a host that destructures it whole adds `..`. When the pack can't render
  the doc as its file, `Session::read` gives the doc's content with `bytes` the file as last
  synced and the pack's error here, where it failed before.
- **A pack's structured read verb gives its result with a warning** (`ToolOutput::warnings`,
  naming the error's id) when the last rendering at the doc's version failed, where it returned
  that failure before.
- **`MPC-E823` `pack_refused` covers every refused live change**, not only a pack verb's text
  refusal: `SessionError::PackRefused`, from a person's edit, a host's, an agent's verb,
  `write --base`, undo, redo, revert and restore, on `@change`, the sync connection and the
  host's calls, with `details.code` (the pack's own) and `details.reason`.
- **`DocStore::open` refuses a store another opener holds with `StoreError::Locked`**
  (`MPC-E505` `store_locked`, exit 6), where the store library's refusal came through as
  `StoreError::Db`.
- **A blob pack's edit verb needs its word** (`BlobVerb::with_word`): the registry refuses an
  edit verb without one.
- **Results say more** (`mpc.result/1`): every result ends with `dry_run`, after `view`, the
  change's place; `changed` gains `bytes`. The schema's name is unchanged and every field is
  added; a reader that compares a result whole (a saved golden) sees the new fields.

### What changes for K2

- **A file the pack can't write stays readable.** Show `ReadResult::bytes` as the file as last
  synced, with `render_failed`. Map `SessionError::PackRefused` (`MPC-E823`) wherever a change
  can be refused, and the new `SessionError::BaseUnrendered` (`MPC-E824` `base_unrendered`, exit
  2): a whole-file write from a version whose whole read gave the synced bytes. A tick reports a
  render failure once per version.
- **A pack may check every live change** (`Pack::checks_edits`, `Pack::check_edit`,
  `PackRefusal`); a pack that doesn't opt in is unchanged.
- **Verbs on a blob doc (tier 0):** `BlobSession::run_tool` runs a file type's own verbs;
  `Registry::for_blob_pack` lists them. **`ToolOutput::version_made()`** gives the version a call
  made: a host tells its clients about a tier-0 doc's new version from it. `AgentWrites::VerbsOnly`
  refuses an agent's whole-file write (`MPC-E715`) and runs its verbs. `--base V` works per node
  (`MPC-E719`, `MPC-E723`, `MPC-E724`). New refusals: `MPC-E718` `needs_lease`, `MPC-E722`
  `keys_reissued`, `MPC-E725` to `MPC-E729`; a blob pack's own (`BlobPack::errors`).
- **A store where a verb on a blob doc made a version holds `blob_verbs` rows:** v0.21.0 opens,
  reads and writes it, and refuses to rewrite it (`MPC-E502`), as for `change_verbs`.
- **Where a change is:** `view` in every edit result; `EditOptions::view`, `WriteOptions::view`
  and `UndoRequest::view` ask for it on a host's own calls (off by default). **An agent's view
  address follows each verb it runs**, to its change; a person's never moves.
- **Structure reads across an outside save:** `BlobPack::save_pairing` (default `Unknown`) and
  `BlobPack::judge_across_save` say what a save keeps; `IndexDiff::saves`,
  `NodeChange::identity` and `NodeBlame::outside_save` report it.
- **Windows:** live and blob docs open (random keys from the OS generator), and folder syncs are
  skipped there.
- **One `SessionManager` per process**, so the native engine's cut queue runs one cut at a time
  host-wide. The native engine stays off (`Config::native_beta`, default off); this tag's tree
  doesn't build it.
- **`libc`** is a dependency on glibc targets only (MIT OR Apache-2.0, already in the lock); a
  pack's lock gains it at its next kit update.
- **Notice, carried:** the result shape for a grid's `cell get --json` and the tier-0 index
  verbs' `--json`, which v0.21.0's notice announced for this release, is not in this release; it
  is still planned. Those verbs print their own JSON, as in v0.21.0.
- **Client 0.22.0:** no API change; `WIRE.md` says an agent's view address follows its verbs.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.21.0 in every mode,
  including a store holding `blob_verbs` rows.

### Overview

A file the pack can't write stays readable, and a pack may refuse a live change before it
applies; a file type's own verbs on a blob doc, with `--base` per node and verbs-only agents;
where each change is, in every result; structure reads across another program's save; a typed
refusal for a store another process holds; Windows fixes; the native engine's Beta settings
(off in this tag).

### A pack's check of every live change, and a read that survives a failed render

`docs/SESSIONS.md`, "A file the pack can't write". Before this release, a person's direct edit
the pack's write-back can't write (for a splice: a driven value, a list insert, a field it
doesn't write) was taken; the write-back then failed, and from then on every whole read, every
structured read verb, every preview and export of the doc failed, and every tick reported the
failure again. No format or wire change.

- **Breaking: `ReadResult::render_failed: Option<SessionError>` (new field).** `ReadResult` was
  already `#[non_exhaustive]`, so no host builds one; a host that destructures it whole adds
  `..`. When the pack can't render the doc as its file now, `Session::read` gives the doc's
  content and text with `bytes` the file **as last synced** (the carrier; empty for a pack that
  keeps none) and the pack's error here, instead of failing. A host shows those bytes as the
  file as last synced, with the error.
- **Breaking (a behaviour change): a pack's structured read verb gives its result with a
  warning** (one `note:` line in `ToolOutput::warnings`, naming the error's id) when the last
  rendering at the doc's version failed. Before, it returned that failure in place of its
  result (the old "a structured read at that version returns that error" in SESSIONS.md, now
  rewritten). The rows that asserted the old behaviour, changed openly:
  `tests/live_carrier.rs` `a_read_reports_a_render_failure_without_encoding` (Ok with the
  warning, still no encode) and `tests/live_paired.rs`
  `row_a_pairing_that_does_not_fit_is_refused_typed` (the structured read is Ok with a warning
  naming `MPC-E521`; a whole read gives the synced bytes and the refusal, typed). Two rows of
  `tests/live_paired.rs` whose read after a re-keyed save passed only because nothing failed
  now say so: their `row list` and whole read refuse a warning or `render_failed`.
- **Unchanged: what reads the rendered file still fails** when it can't be rendered, as
  before: the kit's text read verbs, a table's read verbs and line addresses, `file_now`,
  `file_at`, an export, a flush.
- **The tick reports a render failure once per version.** A write-back that can't render the
  doc is reported in the first tick's `TickReport::errors`; later ticks at that version don't
  try again while the failure stands. A new version or a new carrier tries again; a disk error is
  still retried every tick.
- **New `Pack::checks_edits()` (default `false`) and `Pack::check_edit(view, edits) ->
  Result<(), PackRefusal>` (default: accept),** with the new `PackRefusal { code, message }`.
  A pack that opts in is asked of every live change before it applies, whoever makes it: a
  person's edit, a host's, an agent's verb, `write --base`, the hunk channel, undo, redo,
  revert, restore; step by step for a change of several steps. Not asked: outside saves, the
  import, and a purge's removal (erasure always goes through). Every other pack is unchanged.
- **Errors.** A refused change is `SessionError::PackRefused`, **`MPC-E823` `pack_refused`**
  (exit 2), the entry a pack's text-form verb refusal already used, now also listed for every
  change (`@change`, the sync connection, the host): `details.code` is the pack's own code,
  `details.reason` why; nothing applied, the version unmoved. New **`MPC-E824`
  `base_unrendered`** (exit 2), `SessionError::BaseUnrendered { base }`: a whole-file write
  (`write --base`, a table's `write --base` or `--seen`) from a version whose whole read was
  given the synced bytes. The session keeps those versions in memory, at most 32 ranges (past
  that the two oldest merge, which only refuses more); a reload forgets them.
- **Tests:** `tests/pack_check_edit.rs` (a person's edit, an agent's verb and computed edit, an
  undo, an accepted change, a pack that doesn't opt in, a text doc, a purge's removal) and
  `tests/render_net.rs` (the whole read, the structured read, three ticks giving one report, the
  base guard on a table and a text doc, no pairing made and the doc still dirty). Plants
  (`KeyedMutant`), each caught by one row by name: `EditCheckDropped` (the commit doesn't ask
  the pack) and `ReadReplaysRenderFailure` (the old replay).

### Verbs on a blob doc (additive)

- **A file type's own verbs on a tier-0 doc** (`BlobPack::verbs`, `BlobVerb`): a verb returns a
  plan, the byte ranges to replace (`BlobPlan`, `Span`); the kit applies it, copying every other
  byte from its verified reader, checks the output with the pack's own index (a node the verb
  didn't declare refuses), and records one version with the verb. `BlobSession::run_tool` runs
  them; `Registry::for_blob_pack` lists and checks them (`capabilities` gives each one's `merge`
  and `input`). `docs/SESSIONS.md`, "Verbs on a blob doc".
- **Merge or lease:** an edit verb says whether the pack's merge covers it (`VerbMerge::Pack`) or
  it runs only under the caller's own lease (`VerbMerge::Lease`, the note `lease_only`).
- **Guards:** `--if-version`, the lease, agent writes, and, under `ClaimPolicy::Respect`, an
  agent's change to a node someone else claims (`MPC-E301`). Head moving during the plan: one
  more plan, then `MPC-E110`.
- **`--base V` per node:** a call that read an older version lands on head unless a node it
  changes (at `V` or at head: the verb runs at both) was changed since in a way that overlaps
  (`MPC-E723 base_moved`, with `conflicts`); text is a field of its own, attributes field by
  field, and keys a verb creates never conflict. Another program's save in between that the
  pack doesn't prove keeps keys refuses `MPC-E719 base_across_unproven_save`; keys the kit
  minted refuse `MPC-E724 base_weak_key` (use `--if-version`). `docs/SESSIONS.md`, "`--base` per
  node".
- **`AgentWrites::VerbsOnly`:** a file type whose agents change it only through its verbs: an
  agent's whole-file `write` and `expect_save` refuse `MPC-E715`, its verbs run.
  `AgentWrites::Refused` and `Allowed` are unchanged (`Refused` refuses verbs too).
- **`BlobVerb::word`** (`with_word`): an edit verb's past-tense word for results, required
  (lowercase, not empty: the registry refuses an edit verb without one), listed in
  `capabilities` and carried in the verb's result as `changed.word`. A pack's edit verbs need a
  word at their next kit update.
- **A hold's reach per node kind** (`BlobPack::hold_reach`, `HoldReach::Node`, `Subtree`,
  `Members(attr)`): under `ClaimPolicy::Respect` an agent's verb changing a node inside the reach
  of someone else's hold is refused (`MPC-E301`), the reach read at the version the verb runs
  on.
- **The dry run** (`--dry-run`) on these verbs, by the one contract for every tier, and
  `--expect DIGEST` on the real run.
- **New refusals:** `MPC-E718 needs_lease`, `E725 bad_plan`, `E726 strayed`, `E727
  output_unreadable`, `E728 verb_too_large`, `E729 not_as_previewed`; a blob pack's own refusals
  (`BlobPack::errors`).
- **The result** (`mpc.result/1`): a new field `dry_run` at the end (after `view`), on every
  result, and a tier-0 verb's `view` (its changed nodes, their containers, the last one's place);
  `changed` gains `bytes` (null on a live doc; a tier-0 verb's sizes, ranges and digest), and a tier-0
  verb's `changed` comes from the index diff. The saved result goldens changed only by these
  fields.
- **Records:** `blob_verbs`, one more additive table on the terms of `change_verbs` (an older kit
  refuses a rewrite of a store holding it, `MPC-E502`); `BlobVersion::verb` and `calls`,
  `BlobLogEntry::verb` and `calls`, `NodeBlame::verb` (and `verb` in the index blame's JSON).
- **A verb's version is indexed at its commit**, from the output its check read, exactly as the
  host's index job would record it (row, minted keys, limits, `IndexReady`), so the next verb
  doesn't wait for a job.
- **`ToolOutput::version_made()`**: the version a call made, if any (a live edit's, or a tier-0
  verb's): a host tells its clients about a tier-0 doc's new version from it, as from a write's
  `BlobWriteResult`.
- **`BlobStatus::merge`**: the merge bound against the current version (`MergeBound`), and the
  note `merge_over_bound` on every verb result over it.
- **Limits:** `blob_verb_max_bytes` (256 MiB), `blob_verb_max_spans` (1,000,000),
  `blob_verb_max_insert_bytes` (256 MiB).

### Structure reads across an outside save (additive)

- **`BlobPack::save_pairing(&SaveFacts) -> SavePairing`** (`Proven`, `Unknown`, `Reissues`;
  default `Unknown`): what a file type knows of whether another program's save keeps its node
  keys. Asked of each outside save a structure read crosses. `docs/SESSIONS.md`, "Across an
  outside save".
- **`BlobPack::judge_across_save`** and `anchor::judge_across_save_by_default`: a key across an
  outside save is judged the node just before the save against the node in it; the default is the
  pack's own judge, then renamed and either content changed or moved past its diagonal (only when
  both bounds are known) reads as a doubt.
- **`index_diff`**: `IndexDiff::saves` (`SaveCrossing`: version, pairing, `keys_kept`), and
  `NodeChange::identity` (`Identity::Doubt`, `Identity::Unchecked`); in the JSON, `saves` in the
  tail and `identity` on a row only when there is one. A save whose pack says `Reissues` refuses:
  new **`MPC-E722 keys_reissued`**, exit 1.
- **Anchors** read across an outside save are judged the same way (`suspect`, the existing
  doubts); a `Reissues` save leaves them `suspect`. The state an index job records (and its
  `AnchorState` event) is judged the same way, minting a version's keys as a read does when
  none were recorded.
- **`index_blame`**: `NodeBlame::outside_save` (`ends_on: "outside_save"` in the JSON).

### Where a change is: the view in every edit result (additive)

- **`view` in a call's result** (`mpc.result/1`, after `warnings`), and `EditResult::view`
  (`session::ViewAt`, `ViewUnit`): where an edit's change is, by the addresses a person's view
  and a still use, as of the version it made: the primary view address, the containers and
  units it touched (each unit live or deleted), a grid verb's cells, a text change's lines,
  the verb's playhead. Null when the call changed nothing. `docs/SESSIONS.md`, "A call's
  result as data", has the rules: the last container touched wins; with a 3D slot the address
  is the last unit; a deleted container's is the next one, else the previous one; a text doc's
  is the innermost named span holding the change, else `L<first>`; a verb may name its own
  (`StructOutput::viewing`).
- **`Pack::text_units`** (default none, `session::TextUnit`): a text pack's named spans (a
  Markdown heading's section), each with its address and lines, for a text change's view.
- **An agent's verb moves its view address** to its change, as it moves the agent's focus and
  selection; a person's never moves. Every verb through `run_tool`, a text doc's too.
- **A host asks for the view** of its own calls with `EditOptions::view`, `WriteOptions::view`
  and `UndoRequest::view` (default off: `EditResult::view` is `None`).
- **Known limits** (each held by `known_limits_of_the_view`, which fails when one stops
  holding): a revert and a restore give no view (`HistoryOptions` has no view option yet); a
  grid host write (`write_base_grid`, `write_live_grid`) gives none; a grid-slot change's cells
  come only from a verb's report (`StructOutput::writing`), so cells written with no report give
  `cells: null`.
- Test-only: the plants `view-dropped`, `view-first-container`, `view-deleted-live`,
  `view-moves-person`, `view-section-outer` and `view-address-unchecked`
  (`differential::ErgoMutant`), in the plants part (`tests/view_address.rs`).

### One opener per store, refused typed (a fix)

`docs/SESSIONS.md`, "Lifecycle", item 7. No format change.

- **`DocStore::open` refuses a store another opener holds with `StoreError::Locked`**
  (`MPC-E505` `store_locked`, exit 6), before anything is read or written. Before this, the
  store library's refusal (it holds an exclusive OS lock on the store file for the store's
  life) came through untyped, as `StoreError::Db`. `DocStore::open_with_retry` still waits for
  the holder, and gives the same refusal past its deadline.
- **`MPC-E505`'s wording:** "Another process has this document open", with a next step to
  close it there or open with a wait (it named a daemon hand-off only).
- **An open leaves another opener's rewrite in flight alone:** the temp file an interrupted
  rewrite left is removed only once the opener holds the store.
- **For hosts:** match `StoreError::Locked` on any open, not only after a wait.
- **Tests:** `store/lock_tests.rs`: two opens in one process; a second process (the test binary
  run again) holding the store refuses this one's open, and once it is killed the store opens
  here with no lock file left behind; the wait takes the store when the holder exits and refuses
  past its deadline. They also guard that the store library still locks its file. Plant
  `MPC_STORE_PLANT=store-lock-untyped`, caught at the second open's assert.

### Windows: random keys from the OS generator; folder fsync is unix-only (from K2)

No format change; unix behaviour is unchanged.

- **Random keys on Windows:** doc hash keys, blob space keys and the fence's nonce come from the
  system generator (`RtlGenRandom`, advapi32) there, which has no `/dev/urandom`. Before this,
  every live or blob doc open on Windows failed with `MPC-E509` / `MPC-E707` ("no randomness
  source"). Unix still reads `/dev/urandom`.
- **Folder fsync is unix-only:** Windows refuses to open a folder as a file ("Access is
  denied"), and NTFS journals a folder's entries, so the folder sync after placing a blob, a
  chunked blob's folders and segments, a compacted segment, a store rewrite and a doc's
  write-back is skipped there. Before this, each of those failed on Windows.
- **Tests:** `hash.rs`: the OS generator fills buffers of several sizes whole (up to one above
  the most a single unix read returns) and two calls differ; on unix the folder sync opens the
  folder (a missing one is refused); on Windows it is a no-op (that row runs on Windows only).

### The native engine as a Beta in sessions (feature `native`, off by default)

`docs/SESSIONS.md`, "The native engine (a Beta)". No store format change.

- **A host chooses a doc's engine when it makes the doc, and only then:**
  `SessionManager::open_with` and `open_file_with` take `OpenOptions { engine }`
  (`DocEngine::Standard`, the default, or `DocEngine::Native`). A stored doc opens on the engine
  its stamp names, whatever the options say. `open` and `open_file` are unchanged.
- **The Beta switch, `Config::native_beta`** (default off; listed by `capabilities`). With it off,
  a create on the native engine is refused before anything is read or made: `MPC-E167`
  `native_beta_off`, exit 2, `SessionError::NativeBetaOff`. Docs already on the native engine
  open, edit and save whatever it says.
- **Keys:** `SessionManager::with_native_keys` takes the host's `NativeKeysFor`; it is asked when a
  native doc is made or loaded, by the doc's `NativeDocId` beside its `DocKey`:
  `NativeKeysFor::keys(&self, doc: &DocKey, id: &NativeDocId)`. A `NativeDocId` is the store's
  salt (16 random bytes, minted once per store, in two `meta` rows that kits without the native
  engine copy through and never read) and the doc's id there (reserved before a create): stable
  for the doc's life, a rename included, and never another doc's, in one store or in two. A host
  derives the doc's keys from it (`NativeDocId::bytes`), never from the path. Without keys for a
  doc, it is refused (`MPCE-E566`) and nothing is written.
- **A session holds a native doc:** an import makes it with its first engine records in the
  same transaction as the doc; a load reads it by the engine's own record load; every change
  (edits, live edits at a base, `write --base`, undo, redo, revert, restore, outside saves) is
  logged as a native log row naming the engine's change, its records written in the change's own
  transaction, person-writer rows included. People and agents join one replica, the host's, each
  its own writer, as on Yrs: presence, claims, undo per participant, blame and change events
  work as they do there.
- **A native doc's disk replica is text only:** the file's texts at the sync point, stored in the
  synced state in a kit-neutral form, never engine bytes; a write-back makes no engine encode. An
  outside save is merged from those texts, as on Yrs.
- **The live-text cap:** a native doc's live text (its text roots together) stays within the
  native engine's limit on the text one change may carry (4 MiB at its default limits; the kit
  never changes them). An edit that would grow it past is refused with its own code,
  `SessionError::NativeTextCap` (`MPC-E909` `native_text_cap`, exit 2, as the standard engine's
  size limits), nothing applied and nothing dropped; an outside save that would is declined
  (`too_large`), the doc as it was.
- **One replica:** a native doc's `Doc::encode_diff`, `check_client_update` and
  `apply_client_update` (feature `untrusted-wire`) refuse its own sync bytes, `MPC-E166`
  `native_one_replica`, and Yjs bytes, `MPC-E168` `native_no_yjs` (both exit 2, nothing
  applied): `DocError::NativeOneReplica`, `DocError::NativeNoYjs`.
- **Purge:** an owner's purge of a native doc, whatever its scope, purges the kit's tables as on
  Yrs and has the native engine erase every deleted element of the doc's text everywhere its
  records hold it, its live text kept (a Yrs doc keeps no deleted content). The engine's purge is
  one change, logged as a change of no text by the owner; its records, the rewrites of earlier
  records, the log rows and the kit's purge are written in one store transaction. Another live
  instance of the doc refuses it, `MPC-E523` `purge_doc_open`.
- **No move to or from tier 0:** demoting a native doc, or promoting at its key, is refused
  `SessionError::NativeNoTier0` (`MPC-E721` `native_no_tier0`, exit 4, as the standard engine's
  refusals of those calls), nothing changed.
- **The cut cadence, a host setting:** `Config::native_cut_cadence`, a `CutCadence` (listed by
  `capabilities` as `native_cut_floor`, `native_cut_ceiling`, `native_cut_divisor`): a live cut
  once the changes since the last cut reach `min(ceiling, max(floor, n / divisor))`. The
  default, `CutCadence::BETA`, is `min(200, max(100, n / 30))`; `CutCadence::new` refuses a
  floor of 0, a floor over the ceiling, a ceiling past 10,000 and a divisor of 0
  (`CadenceError`). A doc takes the setting at its next open.
- **Exit codes:** every native refusal exits as the standard engine's refusal in the same
  situation, or in the class it was ruled into where there is none (a row holds the table).
- **Not yet in the Beta:** a native doc's log isn't compacted (its compaction is the engine's
  cut, which waits for the engine's single-replica mode), and conversion between engines isn't
  there.
- **For hosts:** match `SessionError::NativeBetaOff`, `SessionError::NativeTextCap`,
  `SessionError::NativeNoTier0` and the two `DocError` variants (non-exhaustive enums,
  additive); `Config` has two more fields (non-exhaustive, additive); implement
  `NativeKeysFor::keys` with its `NativeDocId` argument.
- **Tests:** `session/native_beta_tests.rs`: the switch and the choice, a person and an agent on
  one replica, person-writer rows on the native commit, the verbs ending alike on both engines,
  blame's sampled check, outside saves through the text replica, the live-text cap and the
  default limits, sync bytes refused, keys by the store's salt and the doc's id, a purge, tier 0
  refused, the cut cadence; each row runs on Yrs and on the native engine where it applies.
  `errors/tests.rs`: every native refusal exits as its standard twin. Plants (`MPC_BETA_PLANT`), one per run, each caught by its row. `tests/person_writers.rs`'s
  native column delivers through the conformance hook (the same admission code), since a native
  doc's public sync calls refuse.

### The native engine's cut queue: its settings (additive)

`docs/SESSIONS.md`, "The native engine (a Beta)". No format change.

- **New `Config::native_cut_queue`, a `CutQueueSettings`,** listed by `capabilities` as
  `native_cut_idle_ms` (default 250), `native_cut_duty_pct` (default 10) and
  `native_cut_slow_ms` (default 250). They say when the host-wide cut queue runs a native
  document's cut: one at a time; one entry per document, forced ones first (twice the cadence,
  or past the size budget), then the most changes since the last cut, then the oldest; a cut
  that isn't forced only after `native_cut_idle_ms` with no native commit; at most
  `native_cut_duty_pct` of the time (after a cut of `d` ms, none for `d * (100 / duty - 1)` ms,
  forced ones included); a refused cut stays queued and is tried again after that gap; a cut
  over `native_cut_slow_ms` is reported slow.
- `CutQueueSettings::new` checks them: it refuses an idle window of 0 or past a minute, a duty
  outside 1% to 50% and a slow threshold of 0, with a `CutQueueError` naming the bound.
- **For hosts: one `SessionManager` per process.** The queue is per manager, so the
  one-cut-at-a-time rule holds for the whole host only with one manager in the process.
- The queue runs native cuts once the native engine's single-replica mode is in the kit; until
  then nothing is queued, and the settings change nothing.
- **Tests:** `session/cut_queue_tests.rs`, on a fake clock: one cut at a time, idle, a forced cut
  while editing goes on, the duty gap after a measured cut (a forced one waits it out too), the
  order, a refused cut kept and tried again, the slow report, one entry per document, and the
  settings' checks and defaults. Plants (`MPC_BETA_PLANT`), one per run, each caught at its
  row's assert: `queue-runs-two`, `idle-ignored`, `duty-ignored`, `forced-waits-for-idle`,
  `refused-dropped`.

### Freed memory handed back to the OS after a cut (glibc)

- `memory::release_freed` (internal, for the native engine's cut queue) asks the allocator to
  return every free page to the OS: glibc's `malloc_trim(0)`, which returns free pages from every
  arena, not only from the top of the heap. On other platforms it does nothing yet; a call is
  added there only where a row shows it returns at least a quarter of the freed memory.
- **The kit's first unsafe block: glibc `malloc_trim`, behind a target cfg.** The crate now
  denies unsafe code (`#![deny(unsafe_code)]`), with the one call allowed and its safety
  argued at the call.
- `libc` is a dependency on glibc targets only (MIT OR Apache-2.0; already in the lock through
  the store library).
- **Tests:** `memory::tests::freed_memory_leaves_the_process` (glibc): 256 MiB of small blocks
  resident, fifteen in sixteen freed, and resident memory drops by at least 64 MiB after the
  call. Plant `MPC_BETA_PLANT=release-skipped`, caught at the drop's assert.

### A native doc takes no snapshot

- **`DocStore::compact` refuses a native doc** before anything is written: its state is its
  engine's records, each under the engine's own checksum and MAC, never a snapshot. With the
  native engine it is `StoreError::NativeSnapshotRefused` (`MPC-E520` `engine_mismatch`, exit 6);
  without it, that doc's own `NativeEngineMissing` (`MPC-E169`). A Yrs doc compacts as before.
- **Tests:** a native doc's compact refused on both builds, plant
  `MPC_ENTRY_PLANT=native-snapshot-unguarded`, caught at its row's own assertion.

### The reference model at one engine commit (vendored; test-only)

- The reference model and the two layers it composes are vendored at one engine commit, each
  directory byte for byte that crate's directory there (`vendor/README.md`). Std only; nothing a
  host builds depends on them.


## v0.21.0

A host release on v0.20.0. The crate version is `0.21.0`; the client's is `0.21.0`. The live
wire stays version 3 (every addition optional). No store needs a migration: `STORE_FORMAT` stays
6, `STORE_FORMAT_BASE` 5 and `STORE_FORMAT_READ` 8. Pairs with the markdown, text, board, csv,
psd and pptx pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.20.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### What changes for K2

- **The version an app opened (additive).** A host that opens a file in an outside app calls
  `note_opened(conn, OpenedAt::FileNow | OpenedAt::Version(v))` on `Session` or `BlobSession`
  (with `clear_opened` and `opened`), and that app's next save merges from the version it opened
  rather than from the doc's latest. `OpenedAt::FileNow` takes in an unseen save, writes back,
  pins the version the file holds and holds write-backs for `Config::opened_hold_ms` (default
  10 s). The pin is kept with the doc's sync state, through a restart and a store rewrite.
  **A host that never calls it sees no change.** New errors: `MPC-E180`
  (`opened_version_unknown`, exit 4, `SessionError::OpenedUnknown`), `MPC-E181`
  (`opened_version_gone`, exit 8, `SessionError::OpenedGone`) and `MPC-E182` (`opened_no_file`,
  exit 2, `SessionError::OpenedNoFile`). New `AuditEvent::OpenedNoted` and `OpenedDropped`, and
  `Action::Opened`. All three enums are `non_exhaustive`; a host that maps them adds the new
  variants.
- **A call's result as data, `mpc.result/1` (additive).** `ToolOptions::result_json(true)`, or
  `--json` on a pack verb, makes a call return one JSON object (`ToolOutput::result`, written as
  stdout) with the verb, the exit, the versions, the units the change wrote, the payload, the
  error as `explain --json` has it, and notes. **Text stays the default.** A host that offers
  `--json` to agents passes it through, or sets `result_json` itself. `capabilities` names the
  schema (`result_schema`). Typed data is `data_schema: "typed"` with its shape in
  `data_spec` (`<name>/<version>`); a verb's text is `data_schema: "text"`.
- **A pack's refusals carry ids.** A pack lists them with `Pack::errors` (`<PACK>-Ennn`); a
  structured verb that prints `refused: <code>: <why>` on its own is reported under the pack's
  entry of that name, or as the new `MPC-E823` (`pack_refused`, exit 2). Exits and text are
  unchanged.
- **A keyed pack's verbs are in its registry**, under the new `Tier::Keyed` (`keyed` in
  `capabilities`), with the kit's verbs on a doc's presence and records.
- **Notice:** a grid's `cell get --json` and the tier-0 index verbs' `--json` print what they
  print in v0.20.0 for this release; the result shape is the host's option for them. **The next
  kit release makes `--json` on these verbs give the result shape.**
- **Change and claim events say more (additive).** `ChangeEvent::kept_pairs` says where an
  outside save's merge kept both sides (wire `kept_pairs`, left out when empty);
  `RosterEvent::ClaimStarted::bytes` gives a claim's span in UTF-8 bytes (wire `bytes`, left out
  when `None`). A host that matches `ClaimStarted` without `..` adds the field.
- **Client 0.21.0:** `ChangeInfo` (`onChange`) gains `version`, `kind`, `turn`, `verb` and
  `atMs`; `ChangeEvent.kept_pairs` and `claim_started.bytes` are typed.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.20.0 in every mode,
  including a store holding an opened version's pin.

### Overview

The version an app opened, so an outside app's save merges from what it opened; results and
refusals as data for agents (`mpc.result/1`, pack refusal ids, keyed packs' verbs in the
registry); change and claim events that say more (kept pairs, claim bytes, the client's
`ChangeInfo`); otherwise tooling (the kit's checks in one Rust tools binary).

### The version an app opened: an outside save merges from it (additive)

- **`note_opened(conn, OpenedAt::FileNow | OpenedAt::Version(v))`**, `clear_opened` and
  `opened`, on `Session` and `BlobSession`: a host says which version an app opened the file
  at, and a save from that app then merges from it (`docs/SESSIONS.md`, "The version an app
  opened"). `FileNow` takes in an unseen save, writes back, pins the version the file holds and
  holds write-backs for the new `Config::opened_hold_ms` (default 10 s).
- **The pin is kept with the doc's sync state** (slot 3 of its sync rows): a restart and a store
  rewrite keep it, and kits since v0.18.0 carry it through a rewrite. It is checked at every
  use and dropped when its version is gone. An outside save moves it to the save's version.
- **With a pin**, a blob doc's save merges from the pinned version through the new
  `BlobPack::merge3_known` (`SaveMerge`, `SaveBase::Opened`; default: `merge3_bytes` on that
  base), even over the kit's own write; a text doc's line merge and a grid's table merge take
  the pinned version as their base. A text save merged from the pin reports where it kept both
  sides in `ChangeEvent::kept_pairs`, as any outside save does. **With no pin nothing changes.**
- **A keyed doc's save with a pin merges field by field**: each change kept where the live doc
  still holds the opened value, a clash (keep both) where the kit changed it differently, an
  insert under a key the live doc holds is a clash too; the result is checked by the new
  `Pack::check_merged_view`, which **refuses by default** (a keyed pack opts in by checking its
  own rules).
- **A save's own stamp names its base**: the new `BlobPack::save_lineage` (`Lineage`, default
  none) lets the kit take the version a save names as its base (`SaveBase::Stamped`), ahead of a
  pin, among the doc's latest `LINEAGE_VERSIONS_READ`; a stamp naming none of them keeps both.
- N−1: `tools/n1/n1.sh layout-opened` / `node-opened` (the previous release carries the pin
  through a rewrite and a doc delete).
- New ids: `MPC-E180 opened_version_unknown` (exit 4), `MPC-E181 opened_version_gone` (exit 8),
  `MPC-E182 opened_no_file` (exit 2), with `SessionError::OpenedUnknown`, `OpenedGone`,
  `OpenedNoFile`. New `AuditEvent::OpenedNoted` and `OpenedDropped`, and `Action::Opened` (all
  `#[non_exhaustive]` enums).
- Test-only: the plant switch `differential::OpenedMutant` (`MPC_OPENED_MUTANT`), its plants in
  the `plants` part (`tests/opened.rs`).

### Change and claim events say more (additive)

- **The client's `ChangeInfo`** (`onChange`) gains `version`, `kind`, `turn`, `verb` and `atMs`,
  from the change event the host relays; `null` for a change that isn't one server change (the
  client's own edit, a refused change leaving, a reload's diff). `ChangeKind` is a named type.
- **`RosterEvent::ClaimStarted::bytes`**: the claim's span in UTF-8 bytes of its root's text at
  the event's `version` (the doc text: `\n` line breaks, no BOM; a CRLF file's `\r` not in it),
  taken with `version` at the take; `None` for a grid's line claim. On the wire, `bytes:
  {start, end}`, left out when `None` (WIRE.md). A host that matches the variant without `..`
  adds the field.

### An outside save's kept pairs (additive)

- **Where an outside save kept both sides.** When an outside save of a text doc overlaps live
  edits, the merge keeps both, in the merge's order (below). Its change event says where:
  `ChangeEvent::kept_pairs`, one `text::merge::KeptPair` per conflict region in which both
  sides' lines are kept, each with `live` and `file`, 1-based inclusive line ranges in the root's
  text after the change. Empty for every other change, for a save that merged cleanly, for a
  region where one side only deleted, and for a save the pack took whole (`check_merged`).
  `text::merge::merge3_keep_both_kept` returns the merge and its pairs; `merge3_keep_both` is
  unchanged. Both types are `#[non_exhaustive]`. On the wire the change carries them as
  `kept_pairs: [{live: {first, last}, file: {first, last}}]`, left out when there are none
  (WIRE.md; the client's `ChangeEvent.kept_pairs`).
- **What the merged text holds there**: only the two sides' own lines, no marker or separator,
  in the merge's order: the live side first at one point, but each side's text where its own
  change starts, so the file's lines can come first and one side's range can enclose the
  other's (docs/SESSIONS.md). A host uses the ranges, never an order of its own.

### What an agent gets back, as data (additive)

The first pieces of agent-facing results for every pack's verbs. Nothing changes for a host that
asks for none of it, except the notice below.

- **A keyed pack's verbs are in its registry.** `Registry::for_pack` takes the structured verbs
  of a pack whose roots are all lists or maps and isn't a table (a deck, a timeline, a scene),
  under a new tier, `Tier::Keyed` (`keyed` in `capabilities`), with the kit's verbs on a doc's
  presence and records (`open`, `leave`, `who`, `status`, `flush`, `log`, `version --name`,
  `approve`, `decline`, `log --pending`, `approvals`), so such a pack lists its verbs in
  `capabilities` (v0.20.0 refuses them). A text verb on a keyed pack is refused by name.
  `Tier::of_pack` says which tier a pack's docs are. `Tier` is `#[non_exhaustive]`, as in
  v0.20.0.
- **A pack's refusals carry ids.** A pack lists its refusals with `Pack::errors` (default none):
  each `ErrorEntry::pack(id, name, exit, message, next)`, the id `<PACK>-Ennn`, stable forever;
  `ErrorEntry::step` is public for them. A verb refuses with `StructOutput::refused(&entry,
  why, next)` (and `.detail(key, value)`): its stdout is the same two lines packs print in
  v0.20.0, `refused: <code>: <why>` and `next: <next>`, byte for byte, and `ToolOutput::refusal`
  holds the report, with the entry's id, the message and the details `code`, `next`, `verb`.
  `Registry::for_pack` checks the entries; `explain` and `capabilities` show them.
- **Until a pack registers, its text refusals are read.** A structured verb that prints
  `refused: <code>: <why>` itself, exit 2, is reported under the pack's entry of that name, or as
  the new **`MPC-E823 pack_refused`** (exit 2) with the code and the `next:` line as details,
  where v0.20.0 gives the host no report for it. Exits and text are unchanged.
- **A call's result as data, `mpc.result/1`.** `ToolOptions::result_json(true)`, or `--json` on a
  pack verb, makes the call return one JSON object (`ToolOutput::result`, written as stdout): the
  verb and its kind, the exit, the versions before and after (and the guards given), the units
  the change wrote by address and field with their values before and after
  (`tools::ChangedUnit`, also `ToolOutput::changed_units`), the verb's payload or its text, the
  next page, the error as `explain --json` has it, a `--base` conflict's units by field, notes
  and warnings. With it asked for, a refusal that a text call returns as an error comes back as
  the result's `error` with its exit. `capabilities` names the schema (`result_schema`).
  `docs/SESSIONS.md`, "A call's result as data", has the fields. Packs can fill typed data with
  `StructOutput::data` (and `VerbCtx::json`) and give the next page with
  `StructOutput::next_page`. **Typed data says so:** `data_schema` is `typed` (or `text` for a
  verb's stdout), and `data_spec` names the data's shape and its version, `<name>/<version>`, as
  a verb gives it with `StructOutput::data_spec(name, version)` (null when it names none).
- **Notice: verbs that print JSON in v0.20.0 keep it for this release.** A grid's `cell get
  --json` (`PackVerb::takes_json`) and the tier-0 index verbs' `--json` print what they print in
  v0.20.0; the result shape is the host's option for them (`ToolOptions::result_json`), holding
  their output as `data.text`. **The next kit release makes `--json` on these verbs give the
  result shape.**
- **Also changed**: a pack's refusal makes a refusal frame on the keyed watch, as the kit's own
  refusals do; `capabilities` gains `result_schema` and a keyed pack's rows; those kit verbs
  list `keyed` among their tiers, and so do the person actions they serve (open, close, who's
  here, saved, why live editing is off, history, restore, approve, pending).
- Test-only: the plant switch `differential::ErgoMutant` (`MPC_ERGO_MUTANT`), and its plants in
  the `plants` part (`tests/agent_results.rs`).

### Tools in Rust

Tooling only: no host builds or runs any of it.

- **`tools/mpc-tools`**, one tools binary with subcommands, in its own workspace, runs the kit's
  checks in Rust (tools/README.md, "mpc-tools"): `licences`, `pack-check`, `evidence check` and
  `evidence collect`, `unzip`, `edit replace-once` and `undo ck-from-log`.
- **The licence check covers every tracked lock** (`scripts/check-licences.sh`, which runs
  `mpc-tools licences`), each with all features. The allow-list takes `Unlicense` only beside an
  allowed licence in an OR, and parses expressions strictly (unbalanced parentheses refuse);
  NCSA passes only in a fuzz lock. The `--expr` mode checks single licence expressions by the
  same rule.
- **The evidence tools are `mpc-tools evidence check` and `evidence collect`**, with the same
  problems and the same records, byte for byte, as the Python tools they replace. They run in
  jobs (tools/evidence/README.md).
- **The pack template's checks** (`scripts/pack-checks/engine-free.sh`, `carrier-row.sh`)
  run `mpc-tools pack-check`; a missing pack folder is exit 4.
- **The test jobs' helpers run in the job or in shell:** `tools/n1/n1.sh`'s `node-both` restores
  the producer with `mpc-tools edit replace-once`, `tools/undo/g1.sh` rebuilds its checkpoint
  files from a stopped job's log with `mpc-tools undo ck-from-log`, and the fallback bundles
  ship sources whole (a part over a job's limit runs as a `--repo` job).
- **The tree holds no early engine spike** (`spike/`; git history keeps it): nothing reads its
  traces or calls its tools, and its Rust half's lock held MPL-2.0 and BSL-1.0 crates.

## v0.20.0

A host release on v0.19.0. The crate version is `0.20.0`; the client's is `0.20.0`. The live
wire stays version 3 (every addition optional). No store needs a migration: `STORE_FORMAT` stays
6, `STORE_FORMAT_BASE` 5 and `STORE_FORMAT_READ` 8. Pairs with the markdown, text, board, csv,
psd and pptx pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.19.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### What changes for K2

- **A rollback refuses native docs one by one, not the whole store.** This build opens a store
  holding docs the native engine wrote and refuses only those docs, each on its own:
  `SessionError::NativeEngineMissing` (`MPC-E169`, `native_engine_missing`, exit 6) on open and
  delete, `StoreError::NativeEngineMissing` from the store. Every other doc opens, edits and
  saves as usual, and a purge or compaction copies the native docs' rows through unchanged.
  Before this, the whole store was refused (`MPC-E502`). **So a K2 release that vendors 0.20.0
  is what lets the next release turn the native engine's Beta on:** rolling that release back
  to this one refuses its native docs one by one, never the store. Both enums are
  `non_exhaustive`; a host that maps session errors adds the new variant.
- **Person-writer reuse is on by default, with a day's park** (`Config::reuse_person_writers`
  true, `Config::person_writer_park_ms` 86,400,000). **A host that sends no device key sees no
  change:** a fresh writer per connection, as before. A host that sends one
  (`JoinOptions::device`, with `JoinOptions::reclaim` on a reconnect and `Session::release` on
  the client's `bye`) gets one writer per person per device. To keep the old behaviour, set the
  flag false. A stale reclaim never gets its own writer back unkept.
- **An agent that gains edit rights after a read-only join reuses its ID**
  (`Config::reuse_agent_clients`), where it took a new one per connection.
- **A writers cap per doc, off by default** (`Config::writers_max`, default 0: no cap). With a
  cap, a connection that would need a new writer past it joins read-only
  (`Joined::writers_full`), keeps presence and reads, and every edit is refused `MPC-E908`
  (`writers_doc_cap`, exit 2). A watcher's writer isn't counted until the host lets it in.
- **A new audit notice, on by default:** `AuditEvent::WritersNearCap`, once per load, when a doc
  holds 80% of `writers_max` (or of 8,192 with no cap). `AuditEvent` is `non_exhaustive`; a host
  that logs audit events logs it.
- **Agents count toward `Config::participants_max` exactly as persons do** (a test pins it; the
  session already counted them). An agent past the cap joins read-only and its edits are refused
  `MPC-E305`, as a person's are.
- **Client 0.20.0:** when a reconnect's welcome names the client's old writer with
  `kept: false`, the client caps its watches on that writer as it does for a new one.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.19.0 in every mode.

### Overview

Native docs refused one by one, so a store holding them opens on a build without the native
engine (the last step before a host may turn the native engine's Beta on); writer IDs that
follow people, devices and agents (person-writer reuse on by default, a writers cap off by
default, a near-cap notice); otherwise tests and tooling (every test target accounted for, the
plants and native-tests parts).

### Native docs, one by one: a store holding them opens on a build without the native engine

`docs/SESSIONS.md`, "The engine stamp" and "Store format 8". No format change.

- **This build opens a store holding native docs** (a store a later kit wrote with the native
  engine: format 8 with the `native` bit) and refuses only those docs, each on its own, with a
  new typed error: `MPC-E169` `native_engine_missing` (exit 6). A session gives it as its own
  variant, `SessionError::NativeEngineMissing { stored, stored_format }`, on open and on
  delete, never as `SessionError::Store(_)`; the store gives `StoreError::NativeEngineMissing`
  (load, delete). A native doc is known by its engine stamp. Its rows are never read or
  changed. The store's other docs open, edit and save as usual. Before this, the whole store
  was refused at open (`NativeWithoutEngine`, `MPC-E502`).
- **A rewrite copies the engine store's tables through** byte for byte (`native_records`,
  `native_gens`, `native_nonces`), so a purge or a compaction of another doc keeps every native
  doc as it was. Before this, a rewrite of such a store was refused as holding tables this
  build doesn't know (`NewerData`); a table it doesn't know is still refused, never dropped.
- **The open still checks the native bit against the engine stores both ways**, by their rows:
  the bit with no engine store is `MPC-E502`, engine records without the bit are damage
  (`MPC-E503`).
- **For hosts:** match `SessionError::NativeEngineMissing` (non-exhaustive enum, additive). A
  release that turns native docs on should follow a release with this kit, so that a rollback
  of one release refuses those docs, not the store.
- **Tests:** `store/native_absent_tests.rs` (a store holding a native doc and a Yrs doc: the
  store opens, the Yrs doc edits and keeps its edit, the native doc is refused on open, delete
  and load with `MPC-E169` as the session's own variant, its rows unchanged, plant
  `MPC_FORMAT_PLANT=native-store-refused-whole`; a purge of the Yrs doc and a whole-store
  rewrite copy the native tables through, plant `MPC_FORMAT_PLANT=native-tables-dropped`); the
  `open_file` row of `tests/engine_stamp.rs` asserts per build. Opening a store the native
  engine itself wrote is checked with the native engine's own N−1 run, outside this tree.

### Writer ids per doc

Every writer (a connection's client ID) stays in a doc's state for good, so these changes keep a
doc's writers following people, devices and agents rather than connections.

- **A stale reclaim never gets its own writer back unkept** (person-writer reuse with
  `Config::person_writer_park_ms` set): when a parked writer was let go after the park, taken by
  another tab of the device and released, the returning tab's reclaim (`JoinOptions::reclaim`)
  takes another writer, never that one with `kept: false`. Its client still watches its old
  changes under that writer, and could read the other tab's changes as its own. The client
  (`client/CHANGELOG.md`) also caps its watches when a host gives a writer back unkept.
- **An agent that gains edit rights after a read-only join reuses its ID**
  (`Config::reuse_agent_clients`): its ID's owner and kind rows are written by the first call
  that sees the edit rights, as an own replica's are, not only at a join that has them. Before
  this, an agent whose every join was read-only (a host that grants edit inside the
  connection, after an approval) took a new ID per connection, each of unknown kind and never
  reused. A read-only agent that never gains edit rights still leaves no row. IDs such an agent
  took before this release stay of unknown kind (never reused); it takes one more, reused from
  then on.
- **Person-writer reuse is on by default, with a day's park** (`Config::reuse_person_writers`
  true, `Config::person_writer_park_ms` 86,400,000): **a host that sends no device key sees
  nothing of it**, a fresh writer per connection as before. A host that sends one
  (`JoinOptions::device`, with `JoinOptions::reclaim` on a reconnect and `Session::release` on
  the client's `bye`) gets one writer per person per device; a writer whose connection ended
  without a release goes to that device's next connection after a day, and the late reclaim is
  told when its lease ended. A host that wants the earlier behaviour sets the flag false, or
  the park 0 (a parked writer only ever goes back to its own reclaim).
- **A writers cap per doc** (`Config::writers_max`, **default 0: no cap**). With a cap, a
  connection that would need a new writer past it reads only (`Joined::writers_full`): it keeps
  presence and reads, and every edit is refused `MPC-E908` (`writers_doc_cap`, exit 2). At the
  cap a person's join with a device key first takes a writer the doc already holds: that
  person's free writer of the device, then of another device, then that person's oldest parked
  writer, let go early (its late reclaim keeps nothing and is told when its lease ended). The
  outside participant, the import and a tier-0 promotion's authors are never refused, and count;
  a promotion with more authors than the cap lands. **With no cap nothing changes**, except the
  notice below.
- **A watcher's writer isn't counted:** a connection watching past `Config::participants_max`
  can't write, so its writer isn't counted toward `writers_max` and gets no owner row, whatever
  its rights; it counts once the host lets it in (`Session::promote_watcher`), and past the cap
  its edits are then refused `MPC-E908` (`tests/writers_cap.rs`, planted fault
  `ClientMutant::WatcherCountsWriter`).
- **`AuditEvent::WritersNearCap`** (on by default): once per load, when a doc holds 80% of
  `writers_max`, or of `WRITERS_NOTICE_MAX` (8,192) when there is no cap, so a host sees a doc's
  writers grow before it sets a cap.
- **Tests:** `tests/writers_cap.rs` (planted faults `ClientMutant::CapUnchecked`,
  `CapTakesNewestParked`, `CapOnExternal`, `CapJoinOnly`, `CapOnPromotion`, `NoNearCapNotice`,
  each caught at its rows); `tests/person_writers.rs`, `soak_reconnects_one_person_ids_bounded`
  (one person reconnecting 200 times, five ways; planted faults `ClientMutant::CapUnchecked` and
  `ReclaimNeverKeeps`).
- **Tests:** `tests/person_writers.rs`, `r4_ended_process_then_expiry_tells_the_loaded_end`,
  `r8_long_history_with_expiry`, `defaults_reuse_person_writers_with_a_days_park` (planted faults
  `ClientMutant::ExpiryWithoutCap`, `NoLoadEnd`, `ExpiryIgnored`, each caught by the expiry
  rows); `r5_stale_reclaim_never_takes_its_writer_back_unkept`
  (planted fault `ClientMutant::StaleReclaimTakesOwn`, caught by that row);
  `client/test/writer-lease.test.ts`, `lease_unkept_same_writer`; `tests/client_reuse.rs`,
  `an_agent_gaining_edit_after_a_read_only_join_reuses_its_id` (planted fault
  `ClientMutant::NoAgentRowOnGain`, caught by that row).

### Tests: every test target accounted for (tooling; no library change)

- **The target scan:** `tools/targets.txt` lists every test target of the workspace and the
  fuzz crate, every feature of their manifests, and the parts of `tools/tests/full.sh` that run
  each (or why none does). `tools/check-targets.sh` reads the manifests, the sources and the
  parts' commands, with no build, and fails on a target or feature with no row, a row with no
  target, a part that doesn't run what its row says, a test-only feature no named part turns
  on, and an `#[ignore]` whose reason names no runner. The `evidence` part runs it and its
  self-test (`tools/test-check-targets.sh`). A row marked `pending ruling` waits on a decision;
  `CHECK_TARGETS_LANDING=1` refuses any, and none is left (tools/README.md, "What each part
  runs: the target scan").
- **The `plants` part:** each test-only plant alone over the suite whose rows must catch it,
  after a control of that suite: `planted-device-bugs` over `tests/roster.rs`, and the native
  engine's planted causal shortcuts (`planted-shortcuts`, `planted-shortcuts-checked`) over
  the kit's native unit tests and the conformance suite. A plant is caught only when exactly
  its named rows fail, each at its own assertion (a checked plant at the engine's cross-check,
  found by its file and its message, not a line, so a newer vendored engine keeps it).
  New row: an op whose past reaches another agent's insert only through one agent's line is
  now admitted on the native engine. The conformance suite's sync between kit docs is a named
  row assertion.
- **The fuzz crate's unit tests run** in the `fuzz-build` part.
- **The `native-tests` part:** every integration test of the kit with the native engine
  (feature `native`), so the native legs in them run.
- **`#[ignore]` reasons name their runner** (`tools/n1/n1.sh`, `tools/tests/grid.sh`,
  `tools/undo/traces-large.sh`, and the new `tools/perf/measure.sh`, which runs each
  measurement a note cites, one cell per job, in release). The measurements no note cites are
  removed, as are write --base --partial's two rows, which waited on a feature not built. The
  two one-trace replays of `crates/spec-undo` (`replay_one`, `grid_trace_one`) are `#[ignore]`d,
  run by name (tools/README.md, "One trace, by name"), and fail without their variable instead
  of passing with nothing checked.

## v0.19.0

A host release on v0.18.0. The crate version is `0.19.0`; the client's is `0.19.0`. The live
wire stays version 3 (every addition optional). No store needs a migration: `STORE_FORMAT` stays
6 and `STORE_FORMAT_BASE` 5. This build reads up to format 8 (`STORE_FORMAT_READ`), and writes it
only once a host turns chunked versions on, which is off by default. Pairs with the markdown,
text, board, csv, psd and pptx pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.18.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### What changes for K2

- **`RosterEvent::ClaimEnded` and `AuditEvent::ClaimEnded` gain `addresses`**, the claim's
  element addresses as they were. A host that builds or matches either without `..` adds the
  field. A host that encodes the roster for its clients adds `keys` to `claim_ended` when
  `addresses` isn't empty, and maps `how` by `ClaimEnd::as_str`, as before. `ClaimEnd` gains
  `GoneOutsideSave` (`"gone_outside_save"`; the enum is `non_exhaustive`): an element claim whose
  last live element an outside save removed ends with it, where it ended `Deleted` before.
- **`ContainerFields`, `BlobView` and `ViewSchema` gain fields** (`ContainerFields::parent`,
  `BlobView::container_parent`, `ViewSchema::container_derived`, `ViewSchema::nesting` and
  `ViewSchema::time_unit`): a full literal of any of them stops compiling, so build them with
  `..X::NONE`, as v0.18.0 asked. `Container` (its `parent` and derived values), `ViewIndex`
  (`refusals`) and `Device` (`view_address`, `time`) gain fields too: build them with
  `..X::default()`.
- **Chunked version storage, off by default.** `BlobConfig::chunked_versions` (and
  `chunk_min_bytes`) store a large version as content-defined chunks. **Turning it on stamps the
  store format 8 at its first chunked version, which v0.18.0 and earlier refuse to open:** turn
  it on only once the release a host would roll back to reads format 8. Off, nothing changes,
  byte for byte. A format-7 store (a format no kit writes) is refused at open, and a format-8
  store's required-features word is checked before anything reads it. `StoreError` gains
  variants; `BlobStats` gains counters; `BlobStore::rechunk` converts a doc's versions on a
  host's request. No new error id.
- **Two new frames on the keyed watch, both unnumbered:** `outside_save` (`WatchFrame::OutsideSave`,
  after an outside save's change frame: ingested, kept both with the copy, declined or missing)
  and `verb_refused` (`WatchFrame::Refused`, a structured edit verb a refusal stopped). Each goes
  only to the watches whose connection may read the doc. `Host::document_at(file)` (default
  `None`) lets the first name a conflict copy's doc. A host that forwards watch frames forwards
  these; a client that doesn't know them ignores them.
- **Two new presence values, behind wire feature names:** a participant's view address
  (`Session::set_view_address`, feature `view_address`) and a time position, a generic playhead
  (`Session::set_time_position`, feature `time_position`). A host that takes the client frames
  lists the names in `welcome.features` and routes `view_address` and `time_position` to the
  setters. An agent's are set from its verb's output; a person's presence is never moved.
- **Additive, nothing to do:** containers inside containers and derived values per container on
  the view index (snapshot and change frames, only when set), the view schema's `nesting`, a
  container's layout checked as a layout reference when the index is built, and a pack's chunk
  cut hints (`BlobPack::chunk_hints`, default none). Client 0.19.0 reads every new field and
  frame; with a kit that sends none it behaves as 0.18.0.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.18.0 in every mode.

### Overview

New presence values (view address and time position) and two keyed-watch frames (outside saves,
refused verbs), all additive on wire 3; a claim gone in an outside save gets its own end reason
and its element addresses; containers inside containers, derived values per container and the
view schema's nesting on the view index (additive API and wire fields, no store change); store
format 8 (its required-features word) read, and written only by chunked tier-0 versions behind
`BlobConfig::chunked_versions` (off by default); format 7 refused as never written; otherwise
tests, text and tooling.

### A claim gone in an outside save (additive)

`client/WIRE.md`, "Claims on elements". Still wire 3, no store change.

- **`ClaimEnd::GoneOutsideSave`** (`"gone_outside_save"`; `ClaimEnd` is `non_exhaustive`): an
  element claim ends with it when an **outside save's** apply removed its last live element (an
  app deleted it, or a pack's pairing gave a renamed element a new key, so the old key went). A
  live delete still ends `ClaimEnd::Deleted`. Before this, both ended `Deleted`: the outside save's
  apply (`ingest_structured`) runs the same end as a live change (`elem_claims_after`, through
  `grid_presence_after`), so the person couldn't tell an app's save from a collaborator's delete.
  A reservation still ends `ReservationEnd::Deleted` either way; claims on a grid's rows or cells
  and a blob doc's node claims are unchanged.
- **The end events carry the claim's element addresses as they were**:
  `RosterEvent::ClaimEnded::addresses` and `AuditEvent::ClaimEnded::addresses` (by
  `Pack::address_of`, in key order; a blob doc's node claim, its node keys; empty for a claim on
  lines or cells), so a view can name what went. On the wire as `claim_ended`'s `keys`, only when
  there are some. `at_rev` is the outside save's change for `gone_outside_save`.
- **Older clients:** no feature name gates the new reason. A client reads a `how` it doesn't know
  as an ended claim (`deleted`); the v0.18.0 client folds `claim_ended` by its `claim` alone, so
  it drops the claim as before (`client/test/n1-wire.test.ts`).
- **For hosts:** `RosterEvent::ClaimEnded` and `AuditEvent::ClaimEnded` gain a field
  (`addresses`): a host that builds or matches them without `..` adds it. A host that encodes the
  roster for its clients adds `keys` to `claim_ended` when `addresses` isn't empty
  (`client/WIRE.md`), and maps `how` by `ClaimEnd::as_str`, as before.
- The client: `ClaimEndReason`, `claimEndReason` (an unknown reason is `deleted`),
  `PresenceChannel.onClaimEnded` (`ClaimEnded {claim, who, how, keys}`), `claim_ended.keys`.
- **Tests:** `tests/claim_gone_outside.rs` (an outside save that removes a claim's last element
  ends it `gone_outside_save` with its addresses; one that leaves one of two keeps it; a live
  delete still ends `deleted`; plants `ClaimMutant::OutsideEndReportedDeleted` and
  `EndAddressesDropped`, each caught by its row); `tests/wire_golden.rs`,
  `a_claim_gone_in_an_outside_save_is_golden`
  (`client/test/golden/roster_events_claim_gone_outside.json`;
  `roster_events_claims_elems.json` gains `keys` on its two ends and nothing else); the test pack
  `tests/keyed_pack::KeyedFile` (an outside save by key, addresses `@item:<key>`);
  `client/test/claim-gone-outside.test.ts`; an N-1 row in `client/test/n1-wire.test.ts`.

### Store format 8: the required-features word

The first step of version storage by chunks (`docs/design/version-storage-r2.md`, section 10).
A store's first chunked version stamps format 8
([Chunked tier-0 versions](#chunked-tier-0-versions),
below), behind a host setting that is off by default.

- **Format 8** means the store's `meta` holds a word of required features. Two are defined:
  `FEATURE_CHUNKED` (the store holds chunked versions) and `FEATURE_NATIVE` (the store holds
  engine stores). From 8 on, a new on-disk capability is a feature, not a format number.
  `DocStore::required_features` reads the word (0 below format 8).
- **Checked at open, before anything reads or writes the store.** A word naming a bit this
  build doesn't know is refused by name: `StoreError::UnknownFeatures` (`MPC-E502`, exit 6;
  `details.features` lists the unknown bits, such as `bit 5`). A format-8 store without its
  word, and a word in a store below format 8, are refused as damage (`MPC-E503`).
- **The native bit.** This build stores no engine records, so a store that sets the bit holds
  no engine store and is refused: `StoreError::NativeWithoutEngine` (`MPC-E502`, exit 6; a
  rollback onto this kit is the likely reader, so it says to use the newer build, and
  `details.features` names `native`). The check against the engine's own first record
  replaces it with the change that first stores engine records.
- **Format 7 is retired, never written.** It was declared for the native engine's stores, and
  no shipped host turned that engine on. A format-7 store is now refused at open, before
  anything reads it: `StoreError::NeverWritten` (`MPC-E503`, exit 6), "store is format 7,
  which no kit writes". Kits v0.10.1 to v0.18.0 opened it read-only.
- **Read and written.** `STORE_FORMAT_READ` is 8; the new `STORE_FORMAT_CHUNKED` (8) is what a
  store's first chunked version stamps. `STORE_FORMAT` is unchanged at 6, the format a store
  with no opt-in feature is stamped (`STORE_FORMAT_BASE` 5 for text docs only,
  `STORE_FORMAT_STRUCTURED` 6 once it holds a structured doc; a format-8 store keeps 8). A
  format-8 store naming only `chunked` opens, is swept and is rewritten (its word with it) as
  any store this build writes, so a purge runs on it. A store past 8 is refused as before
  (`MPC-E502`): "store is format 9; this build reads up to 8"; every message and detail saying
  what this build reads or rewrites names `STORE_FORMAT_READ`.
- **N-1.** Kits v0.14.0 to v0.18.0 read up to format 7 and refuse a format-8 store at open with
  one line ("store is format 8; this build reads up to 7"), its bytes unchanged; this build
  opens a store they wrote as before (`tools/n1/n1.sh layout-format`, then `node-format`).
- **For hosts:** `StoreError` gains three variants (it is `non_exhaustive`); no new error id.
  **`STORE_FORMAT` is unchanged at 6**; a host's own "reads up to" message names
  `STORE_FORMAT_READ` (8), the highest format this build opens. A host that turns chunked
  versions on must first have a rollback target that reads format 8.
- **Tests:** `store/features_tests.rs` (plants `MPC_FORMAT_PLANT`: `format-7-read-only`,
  `unknown-feature-ignored`, `native-unchecked`, each caught by its row);
  `tests/engine_stamp.rs`: a format-8 store serves its Yrs doc and is rewritten and purged
  with its stamp and word kept; a format-7 store is refused untouched.

### Chunked tier-0 versions

`BlobConfig::chunked_versions` (default off) stores a tier-0 version of
`BlobConfig::chunk_min_bytes` (default 1 MiB) or more as content-defined chunks in segment
files, so a small edit of a large file stores the few chunks it changed (`docs/SESSIONS.md`,
"Chunked versions"; `docs/design/version-storage-r2.md`, sections 1, 5, 7 to 9). Off, nothing
changes, byte for byte.

- **What is chunked:** a blob doc's writes, saves recorded with a ticket, outside saves taken
  in, merges, restores, and the first version of an import. Smaller versions, named blobs,
  `retain`ed blobs and live carriers stay whole-file blobs. A version's file id is still the
  whole file's keyed id.
- **On disk:** `<blob dir>/segments/<space no>/<doc>/<n>.seg`; four per-doc tables
  (`doc_chunk_keys`, `doc_chunks`, `doc_manifests`, `doc_segments`), created by a store's
  first chunked version, in the transaction that stamps it format 8 with `FEATURE_CHUNKED`.
- **Writes** mark what they name waiting, write one segment (synced, renamed, its folder
  synced), and commit everything with the version in one transaction, refusing the whole write
  when a chunk it reuses is gone (`StoreError::ChunkGone`, `MPC-E501`). `gc` and
  `BlobStore::open` remove segments no row names (never one a write waits on).
- **The cheap path comes from the writer's rights:** a write reuses only the chunks of the
  versions its connection may read, worked out by the session at each write from the rights
  then in force: every version's with the history right (a restore of a large version writes no
  second copy), else the current version's; the kit's own writes (an outside save, a merge)
  reuse the current version's. A writer without history does the work of new content for any
  other chunk, and for a whole file equal to an older version (the commit writes every chunk
  row and the manifest either way, then raises the manifest's count), so its work, reply,
  status and events don't say whether its bytes were in the history. A write's wall time for a
  history-only chunk against a brand-new one is published, not asserted
  (`docs/evidence/chunked-write-timing.md`; `tests/chunked_timing.rs`, ignored by default).
- **Reads** open every segment first, read ahead, and check each chunk before handing out its
  bytes: `BlobStore::reader_of`, `read_to_vec_of` and `copy_to_of` read a version of a doc,
  chunked or not; `reader`, `read_to_vec` and `copy_to` stay for whole-file blobs.
- **Retention** lowers counts, and keeps the row of a chunk a write waits on (at no
  references; `gc` removes it once nothing waits), so a save never fails because retention
  ran beside it. It never compacts: dropped chunks stay as dead ranges until `gc` (a segment
  less than half live) or the next purge compacts them.
- **The structure index** of a chunked version builds and reads as a whole-file one's: the
  manifest row stands in for the blob row, and the index row goes in the transaction that
  drops the last manifest of its bytes.
- **Purge** of chunked versions: under the doc's internal lock (taken before the session's,
  so it waits for a write's commit), one transaction redacts the versions, erases the rows of
  the chunks only they held and their manifests, and compacts every segment of the doc
  holding a purged chunk or any dead range into a new one holding its kept chunks only: when
  it ends, no byte of any segment of the doc is unnamed, duplicate copies and retention's dead
  ranges included. The old segments go on a **removal list** (`doc_removals`, a fifth per-doc
  table), deleted after the commit and finished by every open; the purge ends with a sweep of
  the doc's segment folder, and the store file's rewrite (asked for at the next open too).
  `NotReached::SharedBlobs` counts a version only when another one names the very same bytes.
  A chunk's `since` is reset when a version takes a reference to it. With no blob store open,
  a chunked purge is refused before anything changes (`StoreError::ChunkedPurge`, `MPC-E501`).
- **Reads** beside a compaction take the rows again (once) when a segment they named is gone.
- **Deletes:** a doc delete takes its chunk rows and segment folder; a space delete
  (`SessionManager::delete_space`) removes its docs' segment folders in the same step.
- **For hosts:** `BlobConfig` gains `chunked_versions` and `chunk_min_bytes` (it is
  `non_exhaustive`: set them on `BlobConfig::default()`). **Turning the setting on stamps the
  store format 8 at its first chunked version, which v0.18.0 and earlier refuse to open**:
  turn it on only once the release a host would roll back to reads format 8. `StoreError`
  gains `ChunkGone` and `ChunkedPurge`. `BlobStats` gains `chunked_busy_fallbacks`: the
  versions stored whole since the blob store's open because a chunked write found its doc's
  lock busy. `gc` also runs the compaction retention deferred and retries the removal list.
- **Rechunk (additive):** `BlobStore::rechunk(space, doc)` converts a doc's existing
  whole-file versions to chunked ones on request, each distinct file in its own transaction
  (file ids unchanged; every version naming the bytes moves its reference to the manifest),
  and removes the whole-file blob it dropped at once when nothing else references it. It skips
  versions under the minimum, purged ones, and those whose doc lock a write holds (reported
  busy, never waited on); a stop at any step leaves every version whole. Returns a
  `RechunkReport`; refused with `RechunkError::ChunkingOff` when the setting is off. Only a
  host calls it (no verb, tool or watch frame reaches it), so its refusal is a host-side error
  with no error id. A rechunk cuts by content only: the blob store holds no pack, so a pack's
  cut hints don't apply to it. A whole-file blob's removal (by `gc` or a rechunk) now keeps
  the structure index rows of its bytes while a manifest of them remains.
- **Cut hints (additive for packs):** `BlobPack::chunk_hints(bytes)` (a new default method:
  none) gives allowed cut points from the file's own structure, such as a zip's member
  boundaries, for versions of at most `BlobPack::chunk_hints_up_to()` bytes (default 0: never
  asked). The kit checks them (any bad one and all are ignored, counted in the new
  `BlobStats::chunk_hints_ignored`), merges those closer than the chunk minimum, caps their
  number and cuts by content between them; a span over the maximum is still cut at it. The
  hook is pure, and reading a version never needs it.
- **Tests:** `tests/chunked_versions.rs` (plants `MPC_CHUNK_PLANT`:
  `gc-ignores-waiting-segment`, `commit-skips-chunk-check`, `reader-skips-chunk-check`,
  `cheap-path-beyond-current`, `reader-opens-lazily`, `retention-ignores-chunk-marks`,
  `delete-space-leaves-segments`, `purge-skips-compaction`,
  `removal-list-not-finished-at-open`, `compaction-skips-dead-ranges`,
  `compaction-output-not-waiting`, `history-chunk-cheap`, `manifest-reuse-skips-full-write`,
  `cheap-set-ignores-history-right`, `rechunk-keeps-old-blob`, `hints-required`,
  `hints-unmerged`, each caught by its row after a passing control step; the
  purge-beside-a-write row plants `purge-skips-doc-lock` itself). Test-only work counters,
  `differential::ChunkWork` and `take_chunk_work` (segment bytes, file and folder syncs, rows
  written, commits), back the equal-work rows.

### The outside-save frame on the keyed watch (additive)

`client/WIRE.md`, "Outside saves". Still wire 3; a client that doesn't know the frame ignores it.

- **`outside_save`** (`WatchFrame::OutsideSave`, `WatchOutsideSave`, `OutsideSaveOutcome`,
  `OutsideSaveCopy`): one unnumbered frame when the kit handles an outside save, after the save's
  own change frame: `at_ms`, `doc`, `outcome` `ingested` (`rev`), `kept_both` (`rev`, `kept`,
  `copy {name, path, doc?}`), `declined` (`code`, `message`) or `missing`; the last two once, when
  the file stops decoding or goes. Nothing for an unchanged file or the kit's own write. Sent to
  the watches whose connection may read the doc then.
- **`Host::document_at(file)`** (default `None`): the doc a file is, when the host serves it as a
  kit document; the frame names a conflict copy's doc address from it.
- The keyed watch's goldens (`keyed_frames.json`) and vectors (`keyed_watch.json`) gain the
  frame after their outside saves; the `ops_change` frames are unchanged.
- The client: `OutsideSaveFrame`, `KeyedWatch.onOutsideSave`.
- **Tests:** `tests/keyed_watch.rs`, `each_outside_save_is_one_outside_save_frame_with_its_outcome`.

### View address and time position presence (additive)

`client/WIRE.md`, "Presence frames". Still wire 3; every field optional, and a client or host that
doesn't know them reads every frame as before.

- **View address**: the view a participant is looking at, by address, on every participant's
  presence: `Session::set_view_address(conn, Option<&str>)` (any address the kit's grammar reads,
  at most `PRESENCE_ADDRESS_MAX`, 512 bytes; `MPC-E114` otherwise), `RosterEntry::view_address`,
  `Device::view_address`, `RosterEvent::ViewAddress`. The line-range `view` is unchanged.
- **Time position** (a playhead, generic): `TimePosition {at, state, at_ms, in?, rate?}` (`at` a
  decimal integer string in the pack's unit, `PlayState::{Playing, Paused}`, `at_ms` stamped by the
  kit, `rate` a decimal ratio, 1 when absent), `Session::set_time_position`, `RosterEntry::time`,
  `Device::time`, `RosterEvent::TimePosition`. The pack names its unit:
  `ViewSchema::time_unit` (`"tick"`), on the wire as `time_unit` only when declared, and checked.
- **An agent's**, from its verb: `StructOutput::viewing(address)` (read and edit verbs: a still)
  and `StructOutput::time_position(time)`, set on the acting agent once the call succeeds, as its
  focus and selection are; a person's presence is never moved. A host sets an agent's directly
  with the setters (when it renders a still for it).
- **The wire**: client frames `view_address {req, address}` and `time_position {req,
  time_position}` behind the feature names `view_address` and `time_position`; roster kinds
  `view_address` and `time_position`; entries and devices carry them only when set.
- **The client**: `PresenceChannel.setViewAddress`, `setTimePosition`, `PresencePeer.viewAddress`
  and `timePosition`, `VIEW_ADDRESS_FEATURE`, `TIME_POSITION_FEATURE`, `PRESENCE_ADDRESS_MAX`,
  `TimePositionJson`, `TimePositionSet`.
- **Tests:** `tests/presence_view_time.rs`; `tests/wire_golden.rs`,
  `view_address_and_time_position_are_golden` (`client/test/golden/roster_events_view_time.json`);
  `client/test/presence-view-time.test.ts`; an N-1 row in `client/test/n1-wire.test.ts`.

### A container's layout is a layout reference (checked when the index is built)

- **`ContainerFields::layout`** is checked when the view index is built, on both tiers: the
  value must parse as an address in the kit's grammar (`addr::parse` with native IDs; a bare
  key is one) and, where the index holds layouts (`ViewIndex::layouts`), name one of them.
- **One that isn't is not carried** (`Container::layout` is `None`) and is recorded on the
  index for the host: `ViewIndex::refusals`, `ViewRefusal::Layout {container, value, why}`,
  `LayoutRefused::NotAnAddress` / `NoSuchLayout`. A test build (and the `differential`
  feature) fails on it loudly, naming the container and the value, as for a placement cycle;
  the plant `ViewMutant::LayoutUnchecked` turns that off. An index kept through a change
  refuses what a whole build does.
- Not on the wire: the snapshot and frames are unchanged. A pack whose layouts resolve (the
  deck's `@layout:<id>`) sees no change.
- **Tests:** `tests/view_index.rs`, `a_bad_layout_fails_a_test_build`,
  `plant_layout_unchecked_is_caught`,
  `a_bad_layout_is_not_carried_and_is_recorded_where_unchecked`.

### Refused verbs on the keyed watch (additive)

`client/WIRE.md`, "Refused verbs". Still wire 3; a client that doesn't know the frame ignores it.

- **`verb_refused`** (`WatchFrame::Refused`, `WatchRefusal`): a pack's structured edit verb that
  a refusal stopped (another's claim, a stale version, a limit, rights, its arguments, its own
  "no") is one unnumbered frame on every keyed watch: who (`actor`, `client`, `turn`), the
  `verb`, the `unit` it addressed by address, the refusal's `code` (`MPC-E301` for a claim), and
  the `holder` and `claim` when a claim or lease held by someone else refused it.
- **Only to viewers who may read the doc** when it is refused: the kit asks the host's rights
  then, so a watcher whose rights went since it opened gets none. An App viewer gets handles and
  no turn, as on a change frame. A full queue drops the refusal, not the watch.
- **The unit**: a pack may name it (`StructOutput::addressing`); else the kit names the first
  element the verb's edits touched under the refusing claim, else the first one they touched.
- `WatchFrame::seq` is 0 for the unnumbered frame; `WatchFrame::numbered` says which frames
  count. The client: `VerbRefusedFrame`, `KeyedWatch.onVerbRefused`.
- **Tests:** `tests/keyed_watch.rs`,
  `a_refused_verb_is_one_refusal_frame_for_viewers_with_rights_only`.

### Derived values per container (additive)

`client/WIRE.md`, "Derived values per container". Still wire 3, no store change, no format bump:
the container half of the units' derived values, under the same rules.

- **`ViewSchema::container_derived`** names them (at most `CONTAINER_DERIVED_MAX`, 8; each 1 to
  `DERIVED_NAME_MAX`, 64, bytes of ASCII letters, digits, `_`, `-` or `.`; none twice); the
  derive hook sets them with `Container::set_derived` (`Container::derived`,
  `Container::derived_value`). The kit keeps the declared names only, in the schema's order,
  each once, each text cut to `VIEW_TEXT_MAX`. (`Derived::containers` is unchanged: it names
  the view fields the hook sets, not values.)
- **Carried** in the index (and `Session::view_container`), on the snapshot's containers
  (`derived`, `{name: text}`, only when the container has some), and on each change frame's
  `derived.containers.<container>.<name>` (the text, or `null` when it has none any more), for
  exactly the values the change set or removed, beside the pack's own keys there, which win. The
  schema's JSON names them in `container_derived` only when there are some.
- **No format bump**: never stored; an older client ignores the new keys
  (`client/test/n1-wire.test.ts`).
- **For pack authors:** `ViewSchema` gains a field (build it with `..ViewSchema::NONE`).
- The client: `ViewContainer.derived`, `ViewSchemaJson.container_derived`,
  `KeyedWatch.containerDerived(name, container)`; `KeyedView.applyDerived` keeps them.
- **Tests:** `tests/view_containers.rs` (plants `ViewMutant::ContainerDerivedUndeclaredKept`,
  `ContainerDerivedOrderLost`, `ContainerDerivedDroppedOnWire`, `FrameMissesContainerDerived`,
  each caught by its row); the client's golden
  `test/golden/keyed_frames_container_derived.json` (`test/container-derived.test.ts`) and an
  N-1 row.

### Containers in containers (additive)

`client/WIRE.md`, "Containers in containers". Still wire 3, no store change, no format bump.

- **`Container::parent: Option<String>`**: the container a container sits inside, by address
  (`None`: the top level), so an outliner draws the containers' tree (a scene's collections
  inside collections) from the index alone. `position` stays the flat order.
- **Where it comes from**: `ContainerFields::parent` (live: a value the kit reads is the
  parent's **key**, which the kit maps to its address with `Pack::address_of`; `Src::Derived`:
  the derive hook sets the address) and `BlobView::container_parent` (blob: the parent's node
  key). Both default to none.
- **Checked when the index is built, on both tiers, as one tree**: the parent must be another
  container of the same index, no cycle, at most `CONTAINER_DEPTH_MAX` (32) levels deep. One
  that isn't is not carried (the container is listed at the top level, in its place in the flat
  order) and is recorded: `ViewRefusal::Parent {container, value, why}`, `ParentRefused`
  `NotAKey` / `NoSuchContainer` / `OwnParent` / `Cycle` / `TooDeep`. A cycle is broken at its
  first container in the containers' order (the others keep their parents); a container one
  level too deep is at the top level and those inside it count from it. A test build (and the
  `differential` feature) fails on a refused parent loudly, naming the container, the value and
  why; an index kept through a change refuses what a whole build does.
- **On the wire only when set**: a container's `parent` in the snapshot; the schema's
  `container_fields.parent` only when declared; a change frame's container delta carries
  `parent_after` (the address, or `null` at the top level now) on `add` and `restore` and when
  the parent changed, and a container whose parent changed with no op of its own (a parent the
  pack derives from another root) gets a `props` delta naming no field, with it. A new carrier
  that changes a parent with no change frame cuts the watches (a fresh snapshot), as for a
  placement.
- **No format bump**: the view index is built on read and never stored; on the blob tier the
  parent is read from the structure index row's node attributes, which every row format already
  stores. An older kit reads the same rows; an older client ignores the new fields
  (`client/test/n1-wire.test.ts` runs the previous client on the new frames).
- **For pack authors:** `ContainerFields` and `BlobView` gain a field: a pack that builds either
  as a full literal stops compiling; build them with `..ContainerFields::NONE` and
  `..BlobView::NONE`.
- The client: `ViewContainer.parent`, `ContainerDelta.parent_after`,
  `ViewSchemaJson.container_fields.parent`, `CONTAINER_DEPTH_MAX`; `KeyedView` applies
  `parent_after` and never reads a parent from the keyed state.
- **Tests:** `tests/view_containers.rs` (plants `ViewMutant::ParentUnchecked`,
  `ParentNotValidated`, `ParentCycleAccepted`, `ParentDepthUnbounded`, `ParentDroppedOnWire`,
  `FrameMissesParent`, each caught by its row); the client's golden
  `test/golden/keyed_frames_container_parent.json` (`test/container-parent.test.ts`) and an N-1
  row.

### Nesting in the view schema (additive)

`client/WIRE.md`, "Nesting". Still wire 3, no store change.

- **`ViewSchema::nesting`** (`Nesting {kind, parent}`): which unit kinds nest inside which, for a
  view to draw the hierarchy (a unit's own `parent` still says which unit it is in). Checked:
  at most `NESTING_MAX` (16), each kind 1 to `NESTING_KIND_MAX` (64) bytes, no pair twice.
- **On the wire** as the schema's `nesting` only when there are some: a schema without it is
  the same bytes as before, and an older client reads a snapshot that carries it as before
  (`client/test/n1-wire.test.ts`).
- **For pack authors:** a pack that builds its `ViewSchema` as a full literal stops compiling
  (a new field); build it with `..ViewSchema::NONE`.
- The client: `ViewSchemaJson.nesting`, `NestingJson`.
- **Tests:** `tests/keyed_watch.rs`,
  `a_view_schema_declares_nesting_and_one_without_it_is_unchanged` (and the client's golden
  `test/golden/keyed_snapshot_nesting.json`); `client/test/view-nesting.test.ts`.

### Engine, tooling and text

- **One form for the kit's test plants, `plant_on!`.** Every function that answers "is this
  plant on?" reads its switch through the macro (`crates/base/src/lib.rs`): the read itself with
  the `differential` feature or in the kit's tests, `false` in every other build. The same
  switches, read the same way. Clippy's `needless_bool` lint is enforced again across the
  workspace. No behaviour change.
- **The fuzz targets are built in every gate run** (main's tooling, not in this tag): the lint
  part checks the fuzz crate as the fuzz runs build it, and a separate part builds every target
  with the nightly toolchain and its fuzzing flags, so a fuzz target that doesn't build is
  found at the commit that breaks it. A chunk table's fuzz entry passes its value to the
  encoder.
- **The conformance suite's coverage document names no dates** (main's suite, not in this tag):
  each awaited rule says what it depends on, never when, and a test of the matrix fails on any
  date in a target or in the document.
- **The error registry's next steps for `MPC-E164` and `MPC-E165`** read in words (a version
  vector too large for one frame; a peer that has compacted the history asked for). The ids,
  messages and exit codes are unchanged.
- **The labels rule** (`tools/check-public-cut.sh`) also blocks parity-row, finding and run
  labels, branch names and engine codes with no entry in the public registry; docs, comments
  and test names say them in words.
- **The published wall time of a chunked write**, a history-only chunk against a brand-new one
  (`docs/evidence/chunked-write-timing.md`; `tests/chunked_timing.rs`, ignored by default).
- `tools/public-text-base` is v0.18.0's tag^ (`e5daca6e`): the text rule's base must be an
  ancestor of the cut.

## v0.18.0 (2026-11-12)

A host release on v0.17.0. The crate version is `0.18.0`; the client's is `0.18.0`. The live
wire stays version 3 (every addition optional). The store format is the same (`STORE_FORMAT` 6,
`STORE_FORMAT_READ` 7, `STORE_FORMAT_BASE` 5) and no store needs a migration. Pairs with the
markdown, text, board, csv, psd and pptx pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.17.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). The native engine stays off.

### What changes for K2

- **Build the view index's structs with `..X::default()`.** The kit adds fields to them in
  minor releases: this one adds `Unit::derived`, which breaks a full `Unit { .. }` literal. Build
  `Unit`, `Container`, `ViewIndex`, `FontUse`, `Layout`, `Bbox`, `Flip`, `UnitText` and
  `Placeholder` with `..X::default()`, and the schema's parts (`ViewSchema`, `UnitFields`,
  `ContainerFields`, `Derived`, `BlobView`, `GridSlot`) with `..X::NONE`. Each struct's docs say
  so; `tests/view_structs.rs` builds every one that way.
- **A keyed doc's `MPC-E822` is `SessionError::KeyedAtLimit { limit, would_be, from }`**, where it
  was `AtLimit { what: "elements and fields", .. }`. Same id, exit code (2) and message. A host
  that matched `AtLimit` for a keyed doc matches `KeyedAtLimit`. `MPC-E822`'s and `MPC-E415`'s
  details gain `limit` and `limit_source` (`host_per_pack`, `pack_default` or `host_wide`), and
  `MPC-E415`'s message names the limit in words. `explain` points at `details.limit`.
- **The keyed size limit per file type, and who sets it.** The host owns it:
  `Config::keyed_max_entries_by_pack` (by pack name; it wins, and it is the only way a type gets
  more than the host-wide limit), else the lower of the pack's recommended
  `Pack::keyed_entries_default()` and the host-wide `Config::keyed_max_entries`.
  `Config::keyed_limit(&dyn Pack)` resolves it. A host that sets no per-type limit, with packs
  that recommend none, sees no change. **A fix:** a limit past `i64::MAX` (`usize::MAX`, the usual
  "no limit") refused every growing change; it doesn't any more.
- **`Doc::state_vector` is deprecated:** use `Doc::try_state_vector` (typed) or
  `Doc::state_vector_parts`. It works as before on Yrs (the only engine a host runs); a host that
  calls it sees a deprecation warning. Before any host turns the native engine on, a breaking
  release removes it or makes it fallible.
- **Four new error ids, all of the native engine (off for every host):** `MPC-E162
  root_kind_not_native`, `MPC-E163 replica_lagging`, `MPC-E164 vector_needs_parts`, `MPC-E165
  needs_state_transfer`. None is reachable with the native engine off.
- **Additive, nothing to do:** a unit's placement in 3D (`ViewSchema::space`, the wire's
  `placement`), a pack's verb state (`Pack::verb_state_build`), derived values per unit and the
  view index kept through changes, and the view hooks reading the carrier's pairing. Client
  0.18.0 reads the new wire fields.
- **Fixes with no API change:** an outside save's change frame is built on its carrier (it went
  out before the kit kept the save, so it could carry the old file's facts); a single list insert
  or delete plans in O(log n) on Yrs; a build of the kit's lib without test code has no dead-code
  warning (`steps` in the list plan's index), and the lint now builds it that way.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.17.0 in every mode.

### Overview

New API and wire fields for the view index (additive: the live wire stays version 3, no store
change); a keyed doc's live size limit per file type (additive API) and a fix to size limits past
`i64::MAX`; a pack's verb state kept by the kit (additive API); four new kit error ids, `MPC-E162`
to `MPC-E165`, and ten engine entries, all behind the `native` feature (not for hosts until the
switch); the vendored native engine moves to a newer version (off, and out of the public cut); a
single list insert or delete planned in O(log n); otherwise text and tooling.

### View slots for a live structured doc (additive)

All additive: the live wire stays version 3, no store change, and a v0.17.0 host or client
reads a doc from this kit as before (`docs/SESSIONS.md`, "The view index").

- **Derived values per unit.** `ViewSchema::unit_derived` names them (at most
  `UNIT_DERIVED_MAX`, 8; `DERIVED_NAME_MAX`, 64 bytes a name); the derive hook sets them with
  `Unit::set_derived` (`Unit::derived`, `Unit::derived_value`). They ride each unit of a units
  page (`derived`) and each change frame's `derived.units.<unit>.<name>`
  (`ViewChange::unit_derived`, `DerivedChanges`). The schema's JSON names them in `unit_derived`
  only when there are some.
- **Reads by unit and by container**: `Session::view_unit`, `Session::view_container`,
  `Session::view_derived` (`UnitsDerived`); `ViewIndex::unit`, `container`, `derived_of`.
- **The view index kept through changes**: `Pack::view_derive_change` (default `false`: built
  whole, as before) and `ViewTouched`; the count `differential::take_view_unit_builds`; plants
  `ViewMutant::FrameMissesUnitDerived`, `UpdateSkipsTouched`, `UpdateRebuildsAll`.
- **A geometry field with no 2D slot** (a scene's transform): a change to it is a `move_resize`
  delta naming the field. The kit read the 2D slot's parts for any field of class `geometry`,
  so with no slot it built no change frame.
- **The client** (`client/CHANGELOG.md`): `KeyedWatch.unitDerived`, `KeyedWatch.unit`; the
  wire's `unit_derived` and a unit's `derived`.
- `docs/design/view-3d-slot-r0.md`: a proposal for a unit's placement in 3D (not built).
- **For pack authors: build the view index's structs with `..X::default()`.** Adding
  `Unit::derived` breaks a pack that builds a `Unit` as a full literal (a deck's group children
  did). The kit adds fields to these structs in minor releases: build `Unit`, `Container`,
  `ViewIndex`, `FontUse`, `Layout`, `Bbox`, `Flip`, `UnitText` and `Placeholder` with
  `..X::default()`, and the schema's parts (`ViewSchema`, `UnitFields`, `ContainerFields`,
  `Derived`, `BlobView`, `GridSlot`) with `..X::NONE`, setting only the fields you have. Each
  struct's docs say so; `tests/view_structs.rs` builds every one of them that way. Where you
  set every field a struct has today, allow `clippy::needless_update`: the update is what keeps
  the next field from breaking you.

### A pack's verb state (additive)

`docs/SESSIONS.md`, "Verb state". For a pack whose verbs need an index beside the doc (a
timeline's items by track and start), kept by the kit so a verb costs what it touches:

- `Pack::verb_state_build(&View) -> Option<Box<dyn VerbState>>` (new; default `None`: nothing
  changes for a pack that doesn't use it): built from the live view of a keyed doc that isn't
  a table, the first time a structured verb runs on it.
- `Pack::verb_state_change(&mut dyn VerbState, &[Applied], &View) -> bool` (new; default
  `false`, built again on the next verb): called on every change to the doc, in its commit
  (verbs, a host's edits, outside saves, undo and redo, restore and revert).
- `VerbState` (new): any `Send + Sync + Debug + PartialEq` type, by a blanket impl;
  `downcast_ref` and `downcast_mut` on `dyn VerbState`.
- `VerbCtx::verb_state::<T>()` (new): a verb's read-only borrow of it.
- Dropped with the live view (a reload, a purge or erasure, a session that closes); never
  stored, never on the wire. Test builds (and the `differential` feature) compare it with a
  fresh build after every change and at every hand-out, and fail loudly on a difference;
  `differential::VerbStateMutant::OutsideSaveSkipped` and `differential::take_verb_state_builds`
  (new) are its plant and its count.

### A keyed doc's size limit per file type (additive), and a fix

- **A size limit past `i64::MAX` doesn't refuse every change that grows a doc any more** (fix):
  `Config::keyed_max_entries`, `grid_max_cells`, `grid_max_cols` and `grid_max_rows` were
  compared after a cast to `i64`, so a limit that didn't fit one (`usize::MAX` among them, the
  usual "no limit") wrapped negative and every growing change was refused (`MPC-E822`, or the
  grid's `AtLimit`). The comparison is now exact for every limit; the count a refusal names
  is the same.
- **A keyed doc's live size limit per file type** (additive; `docs/SESSIONS.md`, "The limit
  per file type"). The host owns the limits; a doc's is:
  - `Config::keyed_max_entries_by_pack: BTreeMap<String, usize>` (new; default empty): the
    host's limit for a file type, by its pack's name (`Pack::schema().name`); when set it wins
    whatever it is, and it is the only way a type gets more than the host-wide limit;
  - else the lower of `Pack::keyed_entries_default() -> Option<usize>` (new; default `None`:
    the pack's recommended limit from its own measured shapes) and the host-wide limit: a
    pack may lower the limit, never raise it (a default at or above it reads `host_wide`);
  - else `Config::keyed_max_entries`, the host-wide limit, as before.

  `Config::keyed_limit(&dyn Pack) -> KeyedLimit` resolves it (`KeyedLimit { limit, source }`,
  `LimitSource::{HostPerPack, PackDefault, HostWide}`, both `#[non_exhaustive]`). A host that
  sets no per-type limit, with packs that recommend none, sees no change.
- **The size refusals name the limit that applied and where it came from.** `MPC-E822`'s and
  `MPC-E415`'s details gain `limit` and `limit_source` (`host_per_pack`, `pack_default` or
  `host_wide`); `MPC-E822` keeps `what` and `would_be`. A keyed doc's `MPC-E822` is now
  `SessionError::KeyedAtLimit { limit, would_be, from }` (new; the variant is
  `#[non_exhaustive]`, and `SessionError` already is): before, `AtLimit { what: "elements and
  fields", .. }`. The id, exit code (2) and message are the same; a host that matched
  `AtLimit` for a keyed doc matches `KeyedAtLimit` (no host or pack of ours does). The kit
  doesn't return `AtLimit` with `elements and fields` any more; one a host builds still maps to
  `MPC-E822`. `MPC-E415`'s message names the limit in words: "past the live limit (6 elements
  and fields; the host's limit for this file type)", where it said "(6)".
- **Why `MPC-E415` reads its limit back from its message (for a later release):** `Declined`
  is a public struct that packs build with a literal, so a new field would break every pack
  that builds one. The kit writes the limit and its source in fixed words and the report's
  details read them back. `Declined` should get a non-breaking way to carry more (the
  pattern `view_index::Unit` is getting for the same problem) in a follow-up; then the
  details come from fields, not the message.
- **`explain` for `MPC-E415` and `MPC-E822`:** the next text points at `details.limit` and
  `details.limit_source` instead of `capabilities`' `keyed_max_entries`, which isn't the
  limit for a type the host or its pack sets one for. `capabilities` still lists the host-wide
  limit; the per-type map isn't listed (it isn't one number).

### Placements in 3D (additive)

The view schema's 3D slot (`docs/design/view-3d-slot-r1.1.md`; `docs/SESSIONS.md`, "The view
index"; `client/WIRE.md`, "Placements in 3D"). Additive: the wire stays version 3, no store
change, a 2D pack's frames unchanged.

- `k2_mpc_base::placement`: `Placement`, `Step`, `Rotation`, `EulerOrder`, `Space`, `Axis`,
  `Handed`, `Precision`, `SpaceSlot`, `SpaceSrc`, `ExactF64`, `PLACEMENT_STEPS_MAX`, `Mat4`,
  `Link`, `world_matrix`, `mul`, `sin_cos`.
- `ViewSchema::space`, `Pack::view_placement`, `Pack::view_space`; `Unit::placement`,
  `Container::placement`, `ViewIndex::space`, `ViewSnapshot::space`, `ViewIndex::world_matrix`;
  `UnitOp::Place`, `UnitDelta::placement_after`, `ContainerDelta::placement_after`,
  `ViewDeltas::placed_through`, `WatchChange::space_after`.
- A new carrier that moves placements no frame names makes every keyed watch take a snapshot.
- A cycle among a pack's placements fails a test build loudly, naming its elements; a release
  build keeps the index, and nothing placed through the cycle has a world matrix.
- Plants `PlaceMutant::*` and the count `differential::take_placement_asks`.
- `docs/design/view-3d-slot-r1.md` and `r1.1.md`: the design after review (r1.1 built).
- **An outside save's frame is built on its carrier** (fix): the keyed watch's change frame for
  an outside save went out before the kit kept the save as the carrier, so its view index (the
  placements, render hashes and units' derived values the frame reads from it) was the old
  carrier's: a unit the save added with a placement only the file holds went out with
  `placement_after: null`, and the watch then took a snapshot. The frame now goes out after the
  carrier and its pairing are kept, on a by-key save and on a keyed doc's text merge alike; a
  save that moves a placement through the carrier alone, with no delta naming it, still makes
  the watch take a snapshot.
- **The view hooks read the carrier's pairing** (additive; `docs/SESSIONS.md`, "The view hooks
  read it"): `ViewCarrier::pairing() -> Option<&Pairing>` (new), for `view_derive`,
  `view_derive_change` and `view_placement`, so a pack that pairs finds a unit's file-only facts
  by key, and keeps the index through units that come, go or move between carriers instead of
  having it built whole. `None` for a pack that doesn't pair. No wire or store change.

### Engine, tooling and text

- **A single list insert or delete plans in O(log n)** (Yrs engine; no API, wire or store
  change). The engine planned every list change by reading the whole root: every slot of its
  array and its position and delete registers, for the place a key's slot sits at, the shown
  elements before it (the applied stream's indices) and which keys exist or are deleted. Each
  list root now keeps a slot index between changes (`engine/slot_index.rs`, an order-statistic
  tree of its slots), and a change whose only op on a root is one insert or one delete is
  planned from it. Measured on a build node (release, checks off): the plan of a single insert
  takes 1.6 us at 1,000 elements and 2.1 us at 10,000 (the full read: 654 us and 9,320 us).
  Moves, sets, restores, drops and several ops on a root are planned by the full read as
  before, and leave the index of their result (in memory, no read). The index is built from
  one full read the first time, and dropped by a merged update, a reset or a change that
  fails. `differential::with_list_plan_fast` (the full read for every change, the reference)
  and `differential::ListPlanMutant` are its test switch and plant (`tests/list_plan.rs`).
- **The native engine is vendored at a newer version; it stays off and out of the public cut.** The
  adapter is on its store-head interface: the calls that read or write records take the doc's
  records; every new store gets a **fresh 16-byte store nonce the kit makes** (a new doc, the
  relay-based load, the empty replica after a failed reopen; never reused; the engine takes no
  randomness); and the kit never raises the engine's load ceiling.
- **A version vector past one frame goes in parts** (feature `native`; a vector too large for one
  frame, about 10,000 writers): `Doc::state_vector_parts` gives kind-4 parts, and the kit's own
  catch-up sends them when the one frame won't do; the peer answers with diff parts, applied as one
  request. Never a smaller or empty vector. A peer that has compacted the history a replica asks for
  is `MPC-E165 needs_state_transfer` (`DocError::NeedsStateTransfer`): not retried, the peer not
  closed.
- **`Doc::state_vector` is deprecated** (every build): use `Doc::try_state_vector` (new, typed:
  on the native engine past one frame it refuses as `MPC-E164 vector_needs_parts`; Yrs's never
  fails) or `Doc::state_vector_parts` (new: the native engine's kind-4 parts, Yrs's one
  vector). `Doc::state_vector` keeps its signature and works as before on Yrs; on a native doc
  past one frame (which nothing reaches: no host turns the native engine on) it fails hard
  naming `MPC-E164`. **Before any host turns the native engine on, `Doc::state_vector` is
  removed or made fallible in a breaking release, and hosts move to the typed forms.** The
  kit's own callers have moved.
- **The engine's host-facing codes have their own entries** (`docs/ERRORS.md`): `MPCE-E561
  native_store_corrupt` (before: `MPC-E146 load_failed` with the engine's code), `E571
  store_keys_wrong` (a store under another secret or digest key), `E573
  stable_part_of_change`, `E580 s_record_missing`, `E581 checkpoint_drops_s_record`, `E582
  store_nonce_invalid`, `E987 part_cap_below_entry`, `E995 merge_not_a_fold` (from a peer),
  `E997 causal_check_over_limit` and four-digit `E1000 own_line_parents`. A code the
  registry doesn't list (`MPC-E161 engine_unlisted`) carries the engine's own next step in
  `details.engine_next_step`. `DocError::Engine` gains `next_step` (the native feature's
  API).
- **`MPC-E163 replica_lagging`**: `MPCE-E995` from the host (a relay, not a peer) means this
  replica runs an older engine than the host: upgrade it (`DocError::ReplicaLagging`).
- `tools/public-text-base` is v0.17.0's tag^ (`9b04ba33`, the twin of main's `9a246ccf`, same tree):
  the text rule's base must be an ancestor of the cut.
- **`Engine::catch_up_from` without a one-arm match on the public cut:** its native-engine case is
  a cfg'd `if let` statement, not a cfg'd match arm, so the cut's tree (which drops the cfg'd
  code) passes `clippy -D warnings`. `tools/README.md`, "The public cut", names the cut's lint as
  its own check. No behaviour change.
- **A lint step for the kit's lib as a host builds it** (`tools/tests/full.sh`, part `lint`):
  `cargo clippy -p k2-mpc-base --lib -- -D warnings`, no test cfg and no feature another member
  turns on. It caught `steps` in `engine/slot_index.rs` read only by the test-build counter
  (`engine_visits!`, which now reads its value in every build).
- Two released CHANGELOG lines (v0.10.0's "The cost", v0.9.1's "Undo spec suite") are wrapped
  to 100 characters, each put as "not ... any more" where it read otherwise (same meaning).
- **`MPC-E162 root_kind_not_native`** (feature `native`): `Doc::new_on` and `Doc::load_on` refuse
  the native engine for a file type with a list or map root, naming the root and its kind
  (`DocError::RootKindNotNative`; `DocError` is `#[non_exhaustive]`, so this adds a variant
  without a break). Such a file type isn't switched to the native engine until lists get the
  engine's placement rules. Before, the same call refused with `MPC-E150 unsupported_kind`.
  Yrs is unchanged.
- **The native engine's vendoring is one checked step** (out of the public cut).
  `scripts/check-licences.sh --expr` checks licence expressions with the dependency check's
  rule and no cargo.
- **Public text in plain words.** Comments, docs and test names say "the carrier" and "the
  carrier's pairing" where they used plan labels, and describe a process that stopped before its
  write-back, a corner outside the sheet and the like in plain words. One test of
  `tests/demote.rs` is renamed, the same row: `row_closed_unwritten_change_is_kept`.
- **`explain` for `MPC-E208` and `MPC-E521`:** the `rule` field reads "the carrier's bytes" and
  "the carrier's pairing". The message, next text and exit code are unchanged.
- `docs/design/carrier-synced-content-r0.md` describes the pairing as built in v0.17.0.
- The CHANGELOGs' lines are wrapped to at most 100 characters.
- **No internal labels in public text.** Requirement ids stay only in the error registry's rule
  column, which the registry now defines: a stable key for the requirement an error enforces.
  Docs, comments and messages say in words what the ask labels, plan slices, numbered contract
  gaps and other internal references pointed at. `tools/check-public-cut.sh` gains a `labels`
  rule that blocks them.
- **`explain`'s `rule` field** for `MPC-E116`, `E305`, `E412`, `E720`, `E802`–`E807`, `E820`
  and `E821` reads in words or drops a label (for example `MPC-E305`: "the participant cap
  (participants_max)"; `MPC-E820`: "writer-tagged keys"). The message, next text and exit code
  are unchanged.

## v0.17.0 (2026-11-12)

A host release on v0.16.0. The crate version is `0.17.0`; the client's is `0.17.0`. A minor
version: new API (text → tier 0, the carrier's pairing, the live grid) and one behaviour
change on keyed docs (below). The live wire stays version 3 (every addition optional). One new
error id, `MPC-E521`. The store format is the same (`STORE_FORMAT` 6, `STORE_FORMAT_READ` 7,
`STORE_FORMAT_BASE` 5) and no store needs a migration; two tables are added without a format
bump (`carrier_pairings`, `demoted_changes`). Pairs with the markdown, text, board, csv, psd and
pptx pack tags of this release.

**This tag's tree doesn't include the vendored native engine,** as in v0.16.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it).

### What changes for K2

- **The demote path for a text doc whose type now routes to tier 0.** The text pack sends CAD,
  BIM and circuit-board files (`.ifc`, `.step`, `.dxf`, the `.kicad_*` files and others) to
  tier 0. When `open_blob_file` answers `PackMismatch` with `stored` the text pack, call
  `SessionManager::demote_text_file(text_pack, AnyFile, &key, path, root)` once, then open the
  file at tier 0 as usual (a second call is a no-op, `Demotion::already`). The same key and doc
  id become a tier-0 doc: every text version a tier-0 version, the live state (unwritten changes
  included) the current one, written to the file. An outside save since the last sync keeps
  both (`Demotion::kept_both`, a conflict copy, `AuditEvent::BlobConflict`). The change log
  stays readable as `BlobSession::live_log(conn)`. The text session closes with
  `RosterEvent::Closed { reason: "demoted" }`, `AuditEvent::SessionClosed` and
  `AuditEvent::Demoted`; tell those tabs to reopen at tier 0 (the old handle's calls fail
  `MPC-E519`). This replaces the interim rule (close, rekey the text doc aside, import), and the
  409 K2 answers today. `docs/SESSIONS.md`, "Text → tier 0".
- **An outside save on unwritten changes keeps both** on a keyed doc that isn't a table (decks
  and boards included): when the doc holds changes the file doesn't, a save that changes the
  content becomes the live doc exactly, and the kit's state is kept as a version and a conflict
  copy (one `BlobConflict`, `DiskCheck::Conflict`), as a save over the kit's write already was.
  Before, the save was merged by key onto the unwritten changes. Tables unchanged.
- **The live grid** (additive, wire 3): the grid slot in the view schema, cells presence
  (`Session::set_cells`, `RosterEvent::Cells`), cells claims (`Session::claim_cells`,
  `RosterEvent::ClaimStartedCells`), what a verb wrote, and typed derived values. Its client
  frames are behind the features `cells` and `cell_claims` in `welcome.features`.
  `client/WIRE.md`, "The live grid"; `client/PRESENCE.md`, "Cells in a live grid".
- **An outside save on a carrier pack's doc (a deck) is the carrier first** (fix): the kit took
  a save as the carrier only after rendering the doc with the one before, so a carrier pack's
  doc holding the save could be marked dirty. Now every rendering from the save on is handed
  the save. No API change.
- **The carrier's pairing** is for packs (no host call): a new error,
  **`MPC-E521` `pairing_mismatch`** (exit 1; the file is left as it was, the doc keeps its
  changes), may reach a host from a pack that pairs. No pack in this release pairs.
- **Pack authors:** the live grid adds two `ViewSchema` fields (`field_class_prefix`, `grid`).
  A pack whose `ViewSchema { .. }` literal names every field without `..ViewSchema::NONE`
  stops compiling: build it from `..ViewSchema::NONE`, as the doc comment on `ViewSchema`
  says, so a field the kit adds has its default. The pptx pack's v0.1.4 does.
- **No store migration, wire still 3.** N−1 is checked both ways against v0.16.0: stores, the
  wire, carriers, index rows, pairings and demoted docs.

### The carrier's pairing

**The carrier's pairing**, for a carrier pack that mints its keys (rows, timeline items, scene
objects), and two outside-save fixes. One additive table (`carrier_pairings`, no format bump), one
new error id (`MPC-E521`), no wire change. A pack that doesn't opt in sees no change but the
keep-both rule below. Design: `docs/design/carrier-synced-content-r0.md`; `docs/SESSIONS.md`, "The
carrier's pairing".

- **An outside save is the carrier, with its pairing, before anything renders the doc that took
  it** (fix): the settle after a save (is the doc clean?) rendered the doc with the
  carrier and pairing of the sync point before, and only then took the save as the carrier. For
  a pack that re-keys on a save (keys that follow the file, as a workbook's rows), the old
  pairing named keys gone from the doc: the pack's splice refused it, rightly, and the kit kept
  that refusal, so the next read failed with `MPC-E521` (and the doc was marked dirty). For a
  carrier pack without pairing, the settle spliced into the old bytes and could mark a doc
  holding the save dirty. Now `settle_saved` keeps the save as the carrier (pairing made from
  the replica, which holds the save) first; a rendering refused with a carrier or pairing that
  has since been replaced isn't reported any more. The pack's refusal is unchanged. Rows:
  `live_paired` (a re-keying save, read, written and `file_at` byte for byte; the same on
  unwritten changes, both kept) and `live_carrier` (every rendering from the save on is handed
  the save); plant `KeyedMutant::SettleBeforeCarrier`, caught by name in both.
- **The pairing** (`Pack::wants_pairing`, default `false`; `Pack::pairing`, default
  `list_pairing`; `Pack::encode_from_paired`, default `encode_from`): the carrier's keys in the
  order the pack's decode yields its units, made once per sync point (a kit write, an outside
  save taken in, a change that moved no byte, an open with none stored) and kept with every
  carrier, live and per version, in memory and in `carrier_pairings`. Every rendering of the
  live doc and every version's file hands the carrier's own pairing to `encode_from_paired`, so
  a version cut between two syncs pairs exactly (`file_at`, the conflict copies' comparisons).
  Keys only: a purge has nothing in it to erase. New types `Pairing` (with `to_bytes` /
  `from_bytes`) and `SpliceError`.
- **A pairing that doesn't fit is refused, never spliced by guess:**
  `SpliceError::PairingMismatch` from the pack; the live doc's rendering fails with
  `SessionError::PairingMismatch`, **`MPC-E521` `pairing_mismatch`** (exit 1; the file is left as
  it was, the doc keeps its changes); a version's file is `MPC-E208`.
- **Whether the doc holds an outside save** is a keyed comparison for a keyed doc that isn't a
  table (the synced state with the save's edits, on a throwaway copy of the replica), with no
  pack call: rendering into the save's bytes paired the doc with keys the replica didn't hold
  yet. Tables unchanged.
- **An outside save on unwritten changes keeps both** (behaviour change, every keyed doc that
  isn't a table, decks and boards included): when the doc holds changes the file doesn't, a save
  that changes the content keeps both, as a save over the kit's write already did (a version
  and a conflict copy of the kit's version, one `BlobConflict`, `DiskCheck::Conflict`), and the
  save becomes the live doc exactly, by identity. Before, it was merged by key onto the
  unwritten changes: a minted key could collide, the merge could break the pack's own rules,
  and a live value was overridden with no copy. The kit writes no merge after such a save
  now; a stored `kit:merge` author from an older store is still read as the kit's write.
- **Plants** (`KeyedMutant`): `SyncedContentStale`, `SpliceWithoutMatch`,
  `PairingMissingOnVersion`, `HoldsFileCallsPack`, `DirtySaveMergedByKey`; each caught by name
  (`tests/live_paired.rs`, `tests/keyed_live.rs`). `MergeWriteNotRecorded` is retired with the
  merge write it planted. Counters: `differential::take_pairings_made`,
  `differential::take_held_renders`.
- **`carrier_judge`** checks the paired hook for a pack that pairs.
- **N−1:** `tools/n1/n1.sh layout-pairing` / `node-pairing` (`tools/n1/consumer-pairing`). A
  previous release ignores `carrier_pairings` (it reads and writes such a store as before),
  refuses a rewrite and so a purge naming it (`MPC-E502`), and its doc delete leaves the rows,
  which this build's open sweeps; this build makes again a pairing an older write left stale,
  and makes one for an older store's doc. A version cut before the doc had a pairing stays
  `MPC-E208`.
- `docs/PACK_TEMPLATE.md`: "A carrier pack that mints its keys: the pairing".

### A live grid: the grid slot, cells presence, cells claims, what a verb wrote

What a view needs to draw a live spreadsheet (or any sparse grid) with no pack code, and the
presence and claims that name its cells. Additive: wire 3 stays (new frame and event kinds,
new fields), no store format change, no new error id.

- **The grid slot** (`k2_mpc_base::grid::GridSlot`, `ViewSchema::grid`): which roots hold the
  rows (the units), columns and cells, the cell field's column template, the **numbering rule**
  (each keyed row's and column's gap field: within a container, in list order, `number =
  previous + gap + 1`, the first `gap + first`), the column labels, the last row and column, a
  cell's JSON parts, the row and column field names (hidden, height, width, with units), how to
  read a viewport (`"cells"`), the style hint's table, and `cell_derived` (the derived values
  that are maps by cell field). The snapshot's schema carries it as `grid`, only when declared;
  `ViewSchema::check` checks it. `GridNumbers` (one container's keyed rows and columns with their
  numbers, hidden ones marked), `col_letters`, `a1`; `Session::grid_numbers_at`.
- **A field's class by prefix**: `ViewSchema::field_class_prefix` (ordered `(prefix, class)`
  rules, after the exact `field_class`), `ViewSchema::class_of_field`, the class
  `FieldClass::Cell` and the unit delta op `UnitOp::Cell` (`"cell"`), so a cell write is classed
  as one in view deltas (it was `props`). The snapshot's schema carries `field_class_prefix`
  only when declared.
- **Cells presence** (`Cells`, `Corner`, `CellRange`, `CELLS_RANGES_MAX` = 16):
  `Session::set_cells` / `set_cells_at`, `RosterEntry::cells`, `Device::cells`,
  `RosterEvent::Cells` (in a flush after `Selection`, before `Range`). A corner is the last keyed
  row and column at or before the cell plus offsets, in normal form. Setting cells sets the focus
  to their container; a focus on another container drops them; a range is dropped, never moved,
  when a corner's row or column goes or a change breaks its form (the caret on its own).
  Refusals: `SelectionTooLarge` past 16 ranges (`MPC-E310`, whose message and next steps
  now name a grid's cell ranges beside a selection's elements), `BadPosition` (`MPC-E114`),
  `Watching` (`MPC-E305`).
- **What a verb wrote** (`CellsReport`, `StructOutput::writing`): a grid verb hands the kit the
  ranges it wrote, the band it inserted or deleted, or the container it added or renamed; the
  acting **agent's** cells and focus follow (ranges with the caret at the first's start; an
  inserted band as whole rows or columns; a delete's caret on the first visible cell after the
  band; a container's first cell unless its cells are already there). A person's are never moved.
- **Cells claims**: `Session::claim_cells` / `claim_cells_at` (one range), `ClaimSpan::CellRange`,
  `RosterEvent::ClaimStartedCells` with the actor and times; `claim_ended`'s `lines` are the
  range's first and last row numbers. Under `ClaimPolicy::Respect` it refuses another agent's
  write to a cell inside it and delete of a row or column inside it (`ElemsClaimed`, `MPC-E301`),
  never a person's; inserts inside grow it; its owner's write inside ends it unless held; a broken
  corner ends it (`deleted`). An agent's claim moves its focus and cells to the range.
- **A change frame's derived values, typed**: `FrameDerived` (per-unit `a1`, per-container
  `merges` and `styles` as `StyleHint`s, `written`, `bands` as `Band`s), put on the frame with
  `WatchExtras::grid`.
- **A snapshot page's derived values**: `Pack::page_derived` gives per-unit values by field (a
  formula cell's A1 text); the kit puts each on the page of the units' fields that holds its
  entry, as `derived.<name>.<unit address>.<field>` (`WatchPage::derived`), counted in the page's
  budget, so a formula never shows as its cached number while the snapshot loads.
- Rows: `tests/grid_view.rs` against the shared vector `client/test/vectors/grid_view.tsv` (row
  numbers and column letters at three revs; every corner and range's A1; refused corners; an
  agent's cells after its claim and each verb; a person's never moved; corners dropped, never
  moved; what a cells claim holds off; the claim's actor; a page's formula text; a cell write
  classed `cell`), `wire_golden.rs`'s `roster_events_cells.json`, and the plants
  `GridMutant::GapIgnored`, `CornersMoveNotDrop` and `ClaimCellsActorMissing`, which they catch.

### Text → tier 0

`SessionManager::demote_text_file` and `demote_text`: a live text doc becomes a tier-0
doc under the same key and doc id, its history kept in place. One additive table
(`demoted_changes`, no format bump), one new audit event (`AuditEvent::Demoted`), no new error
id, no wire change. `docs/SESSIONS.md`, "Text → tier 0".

- **Why, and when K2 calls it.** The text pack now routes CAD, BIM and circuit-board files
  (`.ifc`, `.step`, `.dxf`, the `.kicad_*` files and others) to tier 0; a doc already stored as
  text under such a name was stuck (`open_blob_file` said `PackMismatch`, and K2 answered 409).
  When `open_blob_file` says `PackMismatch` with `stored` a text pack, call
  `demote_text_file(pack, AnyFile, &key, path, root)` once, then open at tier 0 as usual. It
  replaces the interim rule (close, rekey the text doc aside, import the file), which moved the
  history off the file's key.
- **Nothing is lost.** Every text version becomes a tier-0 version holding exactly what its
  `file_at` gave; the live doc's final state becomes current (a change the file never got,
  unwritten or left when a process stopped before its write-back, included) and is written to the
  file (`Demotion::written`; retried on `tick` when the file can't be written).
- **An outside save since the last sync is never merged by lines:** tier 0's keep-both rule (the
  save current, the live state a version and a conflict copy; `Demotion::kept_both`,
  `AuditEvent::BlobConflict`).
- **The change log stays readable** as the live log, `BlobSession::live_log(conn)` (history
  right): `LogItem::Change` rows with their approval states. Content between versions, blame,
  revert records and undo aren't kept.
- **The text session ends cleanly:** roster `Closed { reason: "demoted" }`, claims ended
  (`left`), `AuditEvent::SessionClosed { reason: "demoted" }` and `AuditEvent::Demoted { key,
  connections, versions, changes, kept_both }`; later calls on the old handle fail with
  `SessionError::Closed { reason: "demoted" }` (`MPC-E519`). Tell those tabs to reopen the file
  at tier 0.
- A second call is a no-op (`Demotion::already`). A structured doc or a pack with a carrier is
  refused `Declined { code: "not_text" }` (`MPC-E407`); another pack `PackMismatch`.
- **Erasure and purge** reach everything it keeps (the tier-0 purge, and `erase_participant`,
  which renames the erased pid in the live log too).
- **Plants** (`DemoteMutant`): `DropsUnwritten`, `MergesByLine`; each caught by name
  (`tests/demote.rs`).
- **N−1:** an older kit (v0.16.0 and before) opens a demoted doc as the plain tier-0 doc it is
  (every version and the current one read exactly) without its live log; it refuses a rewrite,
  and so a purge or an erasure, before changing anything (`NewerData`, `MPC-E502`), and leaves a
  deleted doc's live-log rows, which this build's open sweeps. `tools/n1/n1.sh layout-demote` /
  `node-demote` (`tools/n1/consumer-demote`) checks both ways.

## v0.16.0 (2026-11-12)

A host release on v0.15.1. The crate version is `0.16.0`; the client's is `0.16.0`. A minor
version: new API (the view schema, addresses and `ViewIndex`; the keyed watch; view deltas;
roster addresses; the structure index's row format 2) and a new wire surface behind a feature
name. The live wire stays version 3 (every addition optional). No new error id. The store format
is the same (`STORE_FORMAT` 6, `STORE_FORMAT_READ` 7, `STORE_FORMAT_BASE` 5) and no store needs
a migration; the structure index gains a row format (2) without a store format bump (below).
Pairs with the markdown, text, board, csv and psd pack tags of this release, and with the pptx
pack's v0.1.3 (beside `k2-mpc-ooxml` v0.1.1).

**This tag's tree doesn't include the vendored native engine,** as in v0.15.1 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it).

### What changes for K2

- **The keyed watch replaces the welcome's previews and the bare change frames** for a live
  keyed doc that isn't a table (a deck, a board). Today a host builds a preview of the doc for
  the welcome and forwards change events with no state to apply them to. With this kit:
  - list `"keyed_watch"` (`KEYED_WATCH_FEATURE`) in `welcome.features` for a doc whose
    `Session::keyed_watchable()` is true (never a text doc or a table);
  - on the client's `watch` frame, open the watch once per connection,
    `Session::watch_keyed(conn, &WatchOptions::new(&features))` (needs read), and send
    `Session::watch_snapshot`'s frames: one `keyed_snapshot` (the schema, the view index's
    snapshot when the pack declares a view schema, the page count) and its `keyed_page`s (the
    keyed state, then the view's units by container);
  - then forward each `KeyedWatch::try_frames` / `recv_frames` frame (`ops_change`, one per
    change, numbered by `seq`), and send a fresh snapshot whenever `needs_snapshot()` says so
    (none sent yet, the watch fell behind, a frame that would pass the frame cap). A refused
    snapshot (`MPC-E401`) goes out as `watch_refused`; the client's `report` may be ignored;
  - every frame is `WatchFrame::to_json()`, byte for byte. The client side is `KeyedWatch`
    (`client/src/keyed.ts`): it builds the state and the view from the frames, so the editor
    needs no preview of its own.

  `client/WIRE.md`: "The keyed watch" (the gate, the frames, numbering, pages, a change frame's
  fields) and "View deltas" (`units`, `containers_changed`, `containers` on `ops_change`), and
  "Wire versions" for N−1. Goldens `client/test/golden/keyed_snapshot.json` and
  `keyed_frames.json`.
- **Presence speaks addresses:** every selection, focus, range and element claim the kit hands
  out (the roster, its events, the setters' replies, `ClaimInfo`) carries its elements'
  addresses beside the keys (`ElemSelection::addresses`, `ElemFocus::address`,
  `ElemRange::address`, `ClaimInfo::addresses`, `RosterEvent::ClaimStartedElems::addresses`).
  Send them as the wire's `keys` / `key`: a host doesn't call `Session::address_of` per frame
  any more. Setters by address: `Session::set_focus_at`, `set_selection_at`, `set_range_at`,
  `claim_elems_at`, and `Session::resolve_address`. For a pack that maps no addresses they are
  the keys, so nothing on the wire changes there.
- **The structure index's row format 2:** a pack that bumps its index format for view items
  (the pptx pack's 2) writes new rows; versions indexed before read from their old rows,
  `Ready`, with fewer details in the blob tier's view (no fonts, render hashes, inherited or
  composed boxes...), never wrong ones. Re-indexing them is optional:
  `BlobSession::index_job(Some(version))` offers a job for such a version. An older kit never
  reads a format-2 row (it re-indexes under its own format).
- **Packs:** `Pack::view_schema`, `view_derive`, `address_of` / `key_of` and `watch_extras` are
  new provided methods with defaults, so a pack built against v0.15.1 builds as is. Against the
  kit's main between v0.15.1 and this tag, `watch_extras` gained a last argument
  (`Option<ViewChange>`) and `ViewSource::Live` a carrier (`ViewCarrier`): a pack that
  implements them must add those (the pptx pack's v0.1.3 does).
- **A structured verb sees the doc's sync point** (`VerbCtx::synced()`, for packs; no host-facing
  change).
- **No store migration, no new error id, wire still 3.** N−1 is checked both ways against
  v0.15.1 and v0.14.0: stores, the wire, carriers and the index rows.

### The structure index's view items (row format 2)

The blob tier's view index can now show what the live tier reads from the file's bytes (a
deck's fonts and render hashes, field kinds, inherited boxes, a group's children on the page,
flips, `editable_text`, a table's text): the pack's index reader works them out where it parses
the file (the host's index worker), and the kit stores them in the version's index. **Row
format 2**, additive: no store format change, no new table, no new error id; the worker's wire
gains a version 2 for a draft that carries them (one with none is still version 1). A pack that
hands none sees nothing change: its rows are written as format 1, byte for byte as before.

- **Types** (`k2_mpc_base::index`): `NodeView` (a node's `geom: Option<ViewGeom>`, `fields`,
  `text` and `text_truncated`, `editable_text`, `fonts`), `ViewGeom` (box, rotation, flips,
  `inherited`, `approx`), `DraftNode::view` and `DraftNode::render` (a raw digest, keyed by the
  kit like every digest), `IndexDraft::fonts` and `IndexDraft::has_view_items`; read back as
  `IndexNode::view`, `IndexNode::render_hash` and `IndexView::fonts`. `INDEX_ROW_FORMAT` (2) and
  `INDEX_FONTS_MAX`. The draft's check bounds them (`MPC-E714` when malformed).
- **`ViewIndex::blob`** takes them over the plain fields: the doc's fonts, a container's render
  hash, fonts and `has_fields`, a unit's box (none when its view gives none), rotation, flips,
  `bbox_inherited`, `approx_box`, field kinds, and its text and `editable_text` when given.
- **`IndexSpec::with_older_format(older)`**: a pack hands view items only from a format it bumped
  to for them, naming the format before (`IndexSpec::older_format`); a draft with items from a
  spec that names none is refused (`MPC-E714`). A version whose only row is of the older format
  reads from it, `Ready`, the items absent, and `BlobSession::index_job` offers a job that
  indexes it again (optional).
- **A row of format 1 still reads**, with every item absent: the view shows what it showed
  before.
- **Older kits (N−1):** a kit before row format 2 never reads one: it asks only for the older
  format, so a version indexed only under the new one reads as pending there; it indexes it
  under its own format, and its commit removes the newer row; back on this kit the version reads
  from that row, items absent, until indexed again. `tools/n1/n1.sh layout-index` /
  `node-index` checks both ways (`n1_index_producer`, `n1_index_after`, `tools/n1/consumer-index`).
- Rows: `tests/blob_index_v2.rs` (the items through the job, the row and the blob view; a format-1
  row read with them absent; items from a pack that names no older format refused; a pack before
  the bump never reading a format-2 row), the row and wire round trips in `index::row` and
  `index::draft`, and the plant `index-v2-field-dropped` (`IndexMutant::IndexV2FieldDropped`: a
  stored row drops a node's view geometry), which they catch.
- **What changes for K2:** nothing to do to keep working. Re-indexing old versions is optional:
  `index_job(Some(version))` now offers a job for a version read from an older format's row, and
  until it runs, old versions show fewer details in the blob tier's view (no fonts, render
  hashes, inherited or composed boxes...), never wrong ones. A pack that bumps its format (the
  pptx pack's 2) makes its versions' new rows; a host rolled back to an older kit re-indexes the
  versions it opens under the older format, as for any format bump.

### The view index, addresses, the keyed watch and view deltas

The view index K2 asked for, for its rendering data. Additive: no store format change
(nothing is stored), no wire change (still version 3), no new error id. A pack that doesn't
declare a view schema sees nothing change.

- **The view schema:** `Pack::view_schema()` (default `None`: no view index) declares
  once the list root of the containers (a deck's slides) and of the units (its shapes), the
  unit field that names a unit's container (and the container field it matches), the page size
  and native unit, where each view field is (kind, name, alt, text, hidden, editable text,
  placeholder; a container's title, hidden and layout), the one geometry field and its parts,
  each field's class, and how a version's structure index maps to the same view (`BlobView`).
  `BlobPack::view_schema()` gives it to the blob tier. Types in the new module
  `k2_mpc_base::view_index`; a pack writes its schema as a `const` from `ViewSchema::NONE`.
- **Focus follows the schema:** with a view schema, a unit's container (for a range's focus,
  an agent's claim and its verbs) is the one its container field names, so a pack needs no
  `Pack::container_of` for its units; `container_of` is asked for the other roots, as before.
- **Addresses:** `Pack::address_of(root, key)` and its inverse `Pack::key_of(root,
  address)` (default: the address is the key). The kit's edge: `Session::address_of`,
  `Session::resolve_address(conn, address)` (needs read; `MPC-E114` when no element, or
  elements of two roots, have it), `view_index::key_candidates`, and the presence setters and
  claims by address: `Session::set_focus_at`, `set_selection_at`, `set_range_at` and
  `claim_elems_at`, each returning what it set by address. A mapping that isn't one to one is
  refused as the pack's bug (`SessionError::Pack`).
- **`ViewIndex`:** one shape from either source. `Session::view_index(conn)` builds it
  from the live doc's keyed state (`tier` live, `rev`), `BlobSession::view_index(conn,
  version)` from a version's structure index (`tier` blob, `version`), both through the
  schema, then the pack's `view_derive` hook (what only the pack works out), then the kit
  (units in their containers' order, `unit_count`). Pure builders for a host: `ViewIndex::live`
  and `ViewIndex::blob`. Containers in order, units by container back to front, every key an
  address; text, names, titles and alt cut to `VIEW_TEXT_MAX` (1,024 bytes); a container's
  `render_hash` is a keyed hash only. `to_value()` gives its JSON.
- **The paged form** for a watch: `ViewIndex::snapshot()` (everything but the units;
  `ViewSnapshot::to_value(&schema)` adds the schema) and `ViewIndex::units_page(container,
  from, budget)` (a container's units in pages of at most `budget` bytes of their compact JSON,
  at least `VIEW_PAGE_MIN`, and at least one unit; `next` for the following page).
- **The derive hook reads the carrier** (on the live tier): for a pack that
  keeps one, `ViewSource::Live` now carries the doc's carrier with the keyed state
  (`ViewSource::Live(state, Option<&ViewCarrier>)`, a change to the hook's match), for what
  only the file's bytes hold (fonts, render hashes, field kinds, a group's children, boxes
  inherited from a layout). The rule: the live store wins, and the carrier supplies only what
  the store doesn't model. `ViewCarrier::decoded` runs the pack's decode once per carrier and
  keeps the result with it; `ViewCarrier::render_hash` keys a raw digest under the doc's key;
  `ViewCarrier::id` is the keyed hash of its bytes. `ViewIndex::live_carried` is the builder
  with a carrier (`ViewIndex::live` passes none). The blob tier is unchanged (no file bytes).
- **`Session::view_index` keeps the index by (carrier id, rev)** and gives it back until either
  changes: N reads of an unchanged doc build nothing and decode the carrier zero times; a
  change of the doc builds it again from the kept decode; only a new carrier (the kit's write,
  an outside save, a reload, a purge's new doc key) is decoded again. The kept index goes with
  the live view. `encode_from` is unchanged.
- **The keyed watch reads the same kept index:** `Session::watch_state` (a snapshot's view
  index) goes through it, and each change frame reads the index before and after the change
  (`view_index::ViewChange`, the last argument of `Pack::watch_extras`), so a pack's `derived`
  values can carry what its derive hook read from the carrier with no second build. When the pack
  gives no `render_hashes`, the kit's are the containers whose render hash changed
  (`ViewChange::render_hashes`). While a watch is open, a view-schema doc's index is built once per
  change for the frame; the next read gets it kept.
- Test-only: `differential::ViewMutant` (`SchemaFieldIgnored`, `AddressNotInverse`,
  `DeriveIgnoresCarrier`, `DeriveCacheStale`, `FrameMissesRenderHash`), caught by
  `tests/view_index.rs`, and
  `differential::take_carrier_decodes` (the carrier decodes a view index read ran). The rows:
  the derive reads the carrier, N reads of an unchanged doc decode it zero times (one decode per
  carrier), a change reaches what is derived from it (a nested unit placed from its parent's
  new live box), and a change frame carries the re-rendered page's new render hash.

- **The keyed watch** (`client/WIRE.md`, "The keyed watch"; `session/live/watch.rs`): what a
  person watching a live keyed doc that isn't a table (a deck, a board) needs, as numbered frames.
  Additive within wire 3 (`WIRE_VERSION` stays 3): new frame types behind a new feature name.
  - `Session::keyed_watchable`, `Session::watch_keyed(conn, &WatchOptions)` (needs read; `None`
    unless the host lists `KEYED_WATCH_FEATURE`, `"keyed_watch"`, in the connection's
    `welcome.features`, and for a text doc or a table) and `Session::watch_snapshot`. A
    `KeyedWatch` numbers its frames (`seq`, from 1 per watch) and drains them
    (`try_frames`, `recv_frames`); `needs_snapshot` says when the host must send a fresh
    snapshot (none sent yet; the watch fell behind its `Config::subscriber_queue`; a change whose
    frame would pass the frame cap).
  - `WatchFrame`: `keyed_snapshot` (the schema, the view index's snapshot when the pack declares
    a view schema, and the page count, at version `upto`), `keyed_page` (one root's run of the
    keyed state, under `WatchOptions::page_bytes`, 1 MiB by default; an entry whose page would
    pass `WatchOptions::frame_max` refuses the snapshot, `MPC-E401`; then each container's units
    of the view index, `ViewIndex::units_page`, `WatchPart::Units`), and
    `ops_change`, one per change: `rev`, `at_ms`, `change_kind`, the actor (id, name, kind, colour;
    an outside save is kind `file`), `client`, `via`, `on_behalf_of`, `turn`, `verb`, the actor's
    own `claim` on what it touched, and the ops by root, key and field with their values
    (`WatchOp`). Every element is named by its address (`Pack::address_of`), in the snapshot
    and the ops. `WatchFrame::to_json` is the wire's JSON. A change already in a snapshot never
    goes out after it.
  - `Pack::watch_extras` (default: none): a pack's `derived` values and `render_hashes` on each
    change frame (`WatchExtras`).
  - The snapshot reads the doc in one place (`Session::watch_state`): the live view's keyed state
    and the view index built from it (as `Session::view_index` builds it), at one version.
  - `watch_state_json(pack, state)`: a keyed state as canonical JSON by address, what a watcher's
    state is compared by (`keyed_state_json`: the same by the live store's keys).
  - A catch-up from a version is not built: every `watch` gets a snapshot.
  - Planted fault `WatchMutant::SnapshotOffByOne` (`differential`), caught by
    `tests/keyed_watch.rs`.
  - Client: `KeyedWatch` (`src/keyed.ts`) and `keyedStateJson`; the client's changelog has them.
  - Goldens `client/test/golden/keyed_snapshot.json` and `keyed_frames.json`; seeded traces in
    `client/test/vectors/keyed_watch.json`, which the client replays to the kit's own state,
    byte for byte.
  - No store change: no format bump, no new table, no new error id.

- **View deltas in the change frames** (K2's ask;
  `k2_mpc_base::view_delta`; `client/WIRE.md`, "View deltas"): for a pack with a view schema,
  each `ops_change` carries what the change did to the view, worked out by the kit from the
  change's ops and the schema, generically, so a watcher keeps its view without running pack
  code.
  - `units`: one `UnitDelta` per unit whose place in the view changed (`key`, `container`,
    `ops` from `add`, `delete`, `restore`, `reorder`, `move_resize`, `rotate`, `flip`, `text`,
    `style`, `hide_show`, `props`; `geom_before` and `geom_after` as the geometry field's own
    string; `fields` by name; `hidden_after`; `placeholder_type`). A unit is shown when it is
    live and its container is; one that comes or goes with its container gets `restore` or
    `delete`.
  - `containers_changed`: one `ContainerDelta` per op on the containers' list (`add`, `delete`,
    `restore`, `reorder`, `props`) with its 1-based positions before and after, in apply order,
    then one per container whose fields changed (`hide_show` with `hidden_after`, or `props`).
  - `containers`: the render hint (every container a delta names, then `render_hashes`').
  - `WatchChange::deltas`; the pure functions `view_deltas` and `view_deltas_run` (a run of
    changes committed together, each read against the doc right after it).
  - A pack without a view schema gets none of the three fields: its frames are as before, byte
    for byte.
  - The snapshot's `schema` gains `composite` (`[{map, list}]`, `Pack::composite`), so a client
    reads an element's fields by address.
  - Rows `tests/view_deltas.rs`: K2's scripted turn and three of its cases on a deck with its
    view schema, against its hand-made deltas
    (`tests/golden/view_turn.json`); seeded traces on both container namings with every list
    and field op, undo and redo, each change's deltas checked against the view index before and
    after it; no schema, no deltas. Planted fault `WatchMutant::DeltaGeomStale`, caught by both.
  - Vectors `client/test/vectors/view_deltas.json` (the traces' frames and the view index at
    checkpoints): the client's view equals the kit's at each.
- **Roster events speak addresses:** every selection, focus, range and element claim the kit
  hands out (`Session::roster` rows and devices, the roster's `Snapshot`, the `Selection`,
  `Focus`, `Range` and `ClaimStartedElems` events, the setters' replies, `ClaimInfo`) carries its
  elements' addresses (`Pack::address_of`) beside the keys: `ElemSelection::addresses`,
  `ElemFocus::address`, `ElemRange::address`, `ClaimInfo::addresses`,
  `RosterEvent::ClaimStartedElems::addresses`. A host sends them as the wire's `keys` / `key`
  and converts nothing. For a pack whose addresses are its keys (the default) they are the same
  strings, so the wire is unchanged there (the wire goldens didn't move). The three presence
  values compare by their keys, as before. Row `roster_events_speak_addresses`
  (`tests/keyed_watch.rs`), planted fault `ViewMutant::RosterKeysLeak`.
- **N-1:** additive within wire 3, no store change. A client before the view deltas ignores
  the new fields; this client on an older kit keeps the snapshot's view and says it isn't
  current (`KeyedWatch.viewCurrent`). Roster addresses are the keys for a pack that maps none.
- **A structured verb sees the doc's sync point:** `VerbCtx::synced()`, the version the
  doc's file last held (the kit's last read of the file or write to it); `None` for a doc with
  no file. A change after it isn't in the file yet. Needs no history right. A pack's verb that
  must tell written changes from unwritten ones reads it (a spreadsheet pack refuses a row
  delete on a sheet with unwritten row inserts, and the reverse, until the doc is flushed).
  Additive: no store, wire or error-id change. Rows `tests/verb_sync_point.rs`: at the open,
  after unwritten edits, after a flush, in a read verb and an edit verb; none with no file.

## v0.15.1 (2026-11-12)

A host release on v0.15.0. The crate version is `0.15.1`; the client's stays `0.15.0` (its code
is unchanged since v0.15.0; only its N−1 test moved). No public API change, no wire change (still
version 3) and no new error id. The store format is the same (`STORE_FORMAT` 6, `STORE_FORMAT_READ`
7, `STORE_FORMAT_BASE` 5) and no store needs a migration; one added row comes without a format
bump (below). Pairs with the markdown, text, board, csv and psd pack tags of this release, and
with the pptx pack's v0.1.2.

**This tag's tree doesn't include the vendored native engine,** as in v0.15.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it).

For a host, in short (details below):

- **Speed on keyed docs that aren't tables (a deck, a board):** a pack's structured read, its
  field verbs and a one-element list change (insert, move, delete) cost the kit what they
  touch, not the whole doc; opening an untouched file renders nothing (it compares the file by
  a stored hash). Tables and text docs are unchanged. No call, event or error changes.
- **A list root in the view a pack's verb reads carries no `places`** (empty, as a version's
  view already was); its live order and deleted elements are exact. No pack in this release
  reads them.
- **The pack contract gains the carrier's byte-exact row:** a pack that keeps a carrier runs the
  kit's `session::test_support::carrier_judge` (feature `test-support`, test builds only) over
  its corpus, and the template's `scripts/pack-checks/carrier-row.sh` fails one that doesn't.
- **Going back to v0.15.0 or v0.14.0:** the store's sync table gains a third row per doc (what
  the doc is known to render as). An older kit reads the first two rows only, moves, copies and
  deletes a doc's rows by range, and leaves the third as it was; this release then renders to
  compare whenever that row doesn't name the synced bytes at the doc's version.

- **The N−1 rows depend on the previous release** (tooling and tests only). `tools/n1/n1.sh`
  reads each kit's crate version from its manifest and hands it to the rows (`K2MB_N1_READER`,
  `K2MB_N1_PREV`), and the wire row reads the previous client's `WIRE_VERSION`. Against a
  release that doesn't know a table (v0.14.0: the turn, verb and carrier tables, wire 2) the
  rows assert what they did; against one that knows them (v0.15.0 and later) they assert plain
  round trips: its delete takes a doc's turn rows, its rewrite takes a store with carriers, its
  delete gives a carrier's count back, and this client reads its wire-3 turns and claim times.
  Changed: `n1_turns_after` (`tests/grid_store.rs`), `n1_carrier_after`
  (`tests/live_carrier.rs`), `tools/n1/consumer-carrier` and `client/test/n1-wire.test.ts`.
- **A one-element list change on a keyed doc costs the kit what it touches** (performance; no
  API change). On a live keyed doc that isn't a table, an insert, move or delete in v0.15.0
  re-read the whole list into the live view, copied the doc before every agent's list change,
  and rendered the file to see whether the disk replica should re-sync. Now:
  - the live view replays each list op from its positions before and after (no element read),
    and the read bases' journal keeps the ops, not a copy of the list. A keyed doc's live view
    carries no `places` (empty, as a version's view);
  - an agent's change copies the doc first only when it can cut a version, decided
    from the view's lengths;
  - the disk replica's re-sync after an insert, a delete, a restore or a drop renders only when
    the doc's content digest (kept by the view) equals the synced content's;
  - presence reads liveness from the view, so the engine's list order isn't rebuilt for it;
  - a move's revert record asks the engine for the moved element's left neighbours
    (`Doc::left_chain`) instead of copying the list's places.
  The engine's own work stays O(the list) on Yrs (its plan reads the whole list); it is
  measured and counted apart (`differential::take_engine_visits`). Plant
  `KeyedMutant::ListReReadsRoot`. Details: `docs/SESSIONS.md`, "A keyed doc's live view".
- **The carrier pack's byte-exact row is part of the pack contract** (new, test-only API; packs
  that keep a carrier must add it). `docs/PACK_TEMPLATE.md`: a pack whose `wants_carrier()` is
  true MUST show that an untouched doc writes back byte for byte, which the kit relies on when
  it opens an untouched carrier file without rendering it. The row is the kit's:
  `session::test_support::carrier_judge(pack, corpus)` (feature `test-support`) checks, per
  file, the hook (`encode_from` of the file's own content is the file), the file opened live
  through the kit (it renders as itself and a flush leaves it), and the same after a restart,
  reporting every failure by file and leg; it returns `CarrierVerdict::NotACarrierPack` and
  checks nothing for any other pack. `scripts/pack-checks/carrier-row.sh`, now in the
  template's check script, fails a pack that keeps a carrier and never calls the judge (its
  self-test runs in the evidence part). The kit runs the judge on its own carrier packs.
  **The pptx pack needs the row before it pins this kit.**
- **A structured call on a keyed doc costs what it touches** (performance; no API change). On a
  live keyed doc that isn't a table (a deck, a board), a pack's structured read and verb in
  v0.15.0 rebuilt a view of every root from the engine on every call (and a read a second copy,
  for its read base): about 1 µs per element and field of the whole doc per call. Now:
  - the session keeps the doc's keyed state as a **live view**, built once on first need and
    kept in step by every change from its applied stream (a map op sets or removes its key; a
    list or text root a change touched is read afresh, O(that root)); a verb's `View` shares it,
    with no copy;
  - a read records its version as a **read base without a copy**: while a base lives the
    session keeps the reverse of each change since, and a verb naming positions at an older
    base gets that version rebuilt (O(doc), only then). Bases are kept and counted as before;
  - the `keyed_max_entries` count comes from the view's lengths, never a recount;
  - the Yrs adapter keeps the doc's deleted units for a change's write identity instead of
    reading the whole delete set on every structured edit.
  Tables are unchanged. Details: `docs/SESSIONS.md`, "A keyed doc's live view".
- **Opening an untouched file renders nothing** (performance; an additive store row). The open
  rendered the doc twice to check that the file holds it. The store now records what the doc
  is known to render as (its log position, the bytes' keyed hash, the codec settings: a third
  row of the sync table, no format bump), at a write-back, at an open that rendered to compare,
  and at a carrier pack's import (the carrier's contract: an untouched doc writes back byte for
  byte). When it names the synced bytes at the doc's version, the open compares the file by
  hash only; otherwise (an outside save, a process stopped before a write-back, an older kit's
  store) it
  renders as before, and records what it learnt. An older kit reads the sync table's first two
  rows only. Details: `docs/SESSIONS.md`, "The carrier".
- **Test-only counts and plants** (`differential`): `take_entry_visits`, the entries the kit
  read or copied in bulk on a thread, and `set_view_checks`, which switches off the checks a
  test build makes of the live view (every hand-out against the engine doc, every rebuilt read
  base against a snapshot, the kept deleted units against the delete set). Plants
  `KeyedMutant::ReadVisitsWholeDoc` and `KeyedMutant::OpenRendersToCompare`. The rows:
  `tests/keyed_cost.rs` (a one-entry read and verb visit at most 16 entries at 1,000 and 50,000
  elements and fields; an untouched open renders nothing), with the measurement
  (`keyed_cost_measure`, ignored).

## v0.15.0 (2026-11-11)

A host release on v0.14.0. The crate version is `0.15.0`; the client's is `0.15.0`. Pairs with
the markdown, text, board, csv and psd pack tags of this release, and with the pptx pack's
first tag. The store format is the same (`STORE_FORMAT` 6, `STORE_FORMAT_READ` 7,
`STORE_FORMAT_BASE` 5) and no store needs a migration; six new tables come without a format
bump (below). The live wire is version 3, every addition optional. Four error ids are new:
`MPC-E208`, `MPC-E415`, `MPC-E603` and `MPC-E822`.

**This tag's tree doesn't include the vendored native engine,** as in v0.14.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it).

For a host, in short (details below):

- **Behaviour changes:**
  - **Keyed docs that aren't tables (a deck, a board): an app's save over the kit's own write
    keeps both,** the rule tier 0 took in v0.14.0. The save becomes the live doc, and the kit's
    version is kept as a version and as a conflict copy. You see `DiskCheck::Conflict` and one
    `AuditEvent::BlobConflict` where you saw `DiskCheck::Ingested` and
    `AuditEvent::ExternalEdit` alone (`ExternalEdit` still comes). Such copies are removed on
    `tick` and reached by a purge, as tier 0's are.
  - **A size limit for keyed docs that aren't tables** (`Config::keyed_max_entries`, default
    25,000). A file over it doesn't open live: `open_file` declines it with `live_too_large`
    (`MPC-E415`), so open it at tier 0 as any declined file. A change that would grow a live doc
    past it is refused, nothing applied (`MPC-E822`).
  - **An agent's verbs and claims move its presence:** its focus goes to the container it last
    touched or claimed, its selection to the elements. A person's never do.
  - **`purge_space_key` always rewrites the store file,** so the destroyed key's bytes leave it.
  - **A structured read never renders the file** (faster on large decks); it reports the last
    rendering's failure while the doc is at that version.
  - **A blob a put is waiting to reference is never removed at once** by a purge or a replaced
    carrier; a purge reports it as `NotReached::SharedBlobs`.
- **New, additive APIs:**
  - **Turns:** pass a host turn id (`Turn`) on every write call (`ToolOptions`, `EditOptions`,
    `UndoRequest`, `GridWriteOptions`, the new `WriteOptions` and `BlobWriteOptions`, and the
    new `revert_with` and `restore_with`); it shows on `ChangeEvent::turn`, `ChangeInfo::turn`,
    `BlobVersion::turn` and `BlobLogEntry::turn`.
  - **The pack verb on a change:** `ChangeEvent::verb` and `ChangeInfo::verb`, set by the kit.
  - **Focus and range:** `Session::set_focus` and `Session::set_range`, `Pack::container_of`,
    `RosterEntry::focus` and `range`, `RosterEvent::Focus` and `RosterEvent::Range`.
  - **Claim events carry who and when** (`ClaimActor`, `started_ms`, `at_ms`, `at_rev`); the
    watcher count (`RosterEvent::Watching`) is pinned.
  - **Host cache keys:** `BlobStore::host_cache_key` and `host_cache_key_at`, for a host's own
    caches of a space's content (`MPC-E603` once the space key is destroyed).
  - **The carrier:** `Pack::wants_carrier` and `Pack::encode_from`, for a pack that writes
    the file back by splicing (the pptx pack); a version without its carrier is refused with
    `MPC-E208`. `DocStore::live_copies` lists a live doc's conflict copies.
- **The live wire is version 3.** Everything new is additive. An older client on this host
  ignores what it doesn't know; a new client on an older host (no `welcome.wire`) sends no
  `focus` or `range`. K2's own wire goldens pin wire 2 and change only where K2 adds the new
  fields.
- **New error ids:** `MPC-E208 version_no_carrier`, `MPC-E415 declined_live_too_large`,
  `MPC-E603 cache_key_destroyed`, `MPC-E822 keyed_at_limit`.
- **Going back to v0.14.0:** no store format constant changes. Six new tables come without a
  bump: `change_turns`, `blob_turns`, `change_verbs`, `live_copies`, `live_carriers` and
  `version_carriers`. v0.14.0 opens, reads and writes such a store as before, but refuses a
  store-file rewrite (and so a purge or an erasure) with `MPC-E502` once one of them holds a
  row; it keeps the carriers through GC and never removes live conflict copies (they stay,
  nothing is lost), and what it leaves behind this release sweeps on its next open.
  `tools/n1` checks both ways (`node-both`, `node-wire`, `node-carrier`).
- **Client 0.15.0:** wire 3 (`turn`, `verb`, focus and range, claims, the watcher count); with
  a wire-2 host it behaves as 0.14.0.

In detail:

- **A live revert and restore take the host's turn** (new, additive; K2's ask).
  `Session::revert_with(conn, change, partial, &HistoryOptions)` and
  `Session::restore_with(conn, vid, &HistoryOptions)` record the host-given turn
  (`HistoryOptions::turn`) on the change they make, on text and keyed docs, so the activity line
  credits the right turn. `revert` and `restore` are unchanged and record none. Tier 0's
  `BlobSession::restore_with` already took one.
- **An agent's verbs now move its presence** (K2's ask; a behaviour change,
  no new API). When a pack's verb run through `run_tool` changes elements of a keyed doc
  that isn't a table, the kit sets the acting agent's focus to the container
  (`Pack::container_of`) of the last element the change touched (the last container touched
  wins), and its selection to the live elements it touched in that element's list. A verb that
  touches no element moves nothing, and a raw write, an undo, a redo, a revert or a restore is
  no verb. A person's own verb never moves their presence. The roster carries the new focus and
  selection as usual. Details: `docs/SESSIONS.md`, "Presence".
- **The pack verb on a change** (new, additive; K2's ask). A change a
  pack's own verb made (`run_tool` running a structured verb such as `shape move`, or a text
  pack's verb such as `replace-section`) now carries the verb's id: `ChangeEvent::verb` (the
  push and `events_since`) and `ChangeInfo::verb` (the log).
  - It is `None` for a raw write (`edit_ops`, `replace_text`, `write --base`), the kit's own
    tools, an outside save, an undo, a redo, a revert, a restore and the import.
  - `ChangeEvent::via` keeps meaning the participant's client (`cli`), never the verb.
  - The kit sets it, never a host. An App viewer sees it too: it names a verb, not a person.
  - A participant's erasure removes the verbs of their changes, and of those made on their
    behalf, as it removes their turns. A content purge leaves them.
  - The store keeps verbs in a new table, `change_verbs`, with no format bump, on the terms of
    `change_turns`: an older kit refuses a store-file rewrite with `MPC-E502` once a change has a
    verb. Details: `docs/SESSIONS.md`, "Verbs".
- **The watcher count, pinned** (K2's ask). The roster's
  `RosterEvent::Watching { count }` (`watching {count}` on the wire) comes whenever the number
  of participants watching past `Config::participants_max` changes, and a snapshot carries it.
  A watcher gets no roster row, no `Joined` or `Left`, and no presence. New rows pin this; the
  kit's behaviour is unchanged.
- **Claim events carry who and when** (additive; K2's ask). The roster's
  `ClaimStarted` and `ClaimStartedElems` gain `actor` (the new `ClaimActor`: name, kind and
  colour; the id is `who`) and `started_ms`. `ClaimEnded` gains `actor`, `at_ms`, and `at_rev`,
  the change that ended it (`Edited`, `Deleted`) or `None`. On the wire, the claim events and a
  snapshot's `elem_claims` carry them (wire 3). A host that matches these variants with `..`
  builds as is.
- **An agent's claim on elements now moves its presence.** `claim_elems` by an agent replaces
  its selection with the claimed keys, and sets its focus to the claim's first container in
  document order (by `Pack::container_of`). A person's claim leaves their presence alone. This
  is a behaviour change: before, an agent's claim left its selection as it was.
- **Focus and range** (new, additive; K2's ask, the kit's side).
  - `Session::set_focus(conn, Option<ElemFocus>)`: the container a connection has in view (a
    deck's slide). Needs read. It goes when its element stops being live.
  - `Session::set_range(conn, Option<ElemRange>)`: a caret or selection inside one element's
    text field, in UTF-8 bytes. Needs edit, as a caret does. Setting a range also sets the
    focus to the element's container, through the new `Pack::container_of` (default: the
    element itself). A range is never moved: when its field changes, or its element goes, the
    kit drops it, and the owner sends a new one.
  - A watcher is refused both (`MPC-E305`), as every element setter.
  - The roster carries them: `RosterEntry::focus` and `range` (and the same on `Device`), and the
    new `RosterEvent::Focus` and `RosterEvent::Range`.
  - Keys are the live store's own for now; addresses come in a later release.
- **The live wire is version 3** (`session::WIRE_VERSION`; `client/WIRE.md`, "Wire versions").
  Everything new is additive: a change's `turn`, the roster's `focus` and `range`, the claim
  events' actor and times, the client frames `focus` and `range` behind the features of the
  same names, and `welcome.wire` for a host to say which version it speaks.
  - **An older client on a new host** ignores what it doesn't know and works as before.
  - **A new client on an older host** (no `welcome.wire`) sends no `focus` or `range`, and reads
    the missing fields as absent.
  - K2's own wire goldens pin wire 2. They change only where K2 adds the new fields.
- **Turns** (new, additive; K2's ask). A host can now pass a turn id, the
  id of one agent step, on every write call, so a view can group one agent run's changes.
  - The kit never makes one up and never reads one from a client. `None` is the default, and a
    call without one records none. The type is `Turn` (`Turn::new`: 1 to 64 bytes of printable
    ASCII, no spaces).
  - Where to pass it: `ToolOptions::turn`, `EditOptions::turn`, `UndoRequest::turn`,
    `GridWriteOptions::turn`, and the new `WriteOptions` (with the new
    `replace_text_with_options` and `write_base_with_options`). At tier 0, the new
    `BlobWriteOptions`, with the new `write_with`, `write_bytes_with` and `restore_with`. The old
    calls are unchanged and record no turn.
  - Where it shows: `ChangeEvent::turn` (the push and `events_since`), `ChangeInfo::turn` (the
    log), `BlobVersion::turn` and `BlobLogEntry::turn`. On the wire, a change event's `turn`
    (`client/WIRE.md`). An App viewer always gets none: turn ids would link an agent's run across
    docs.
  - **A participant's erasure removes the turns** of their changes and of the changes an agent
    made on their behalf. A content purge leaves turns, as it leaves authors and times.
  - The store keeps turns in two new tables, `change_turns` and `blob_turns`, with no format
    bump. An older kit reads and writes the store as before, but refuses a store-file rewrite
    (and so a purge or an erasure) with `MPC-E502` once a change has a turn. Details:
    `docs/SESSIONS.md`, "Turns".
- **Host cache keys** (new, additive). A host can now encrypt its own caches of a space's
  content (rendered previews, for one) under a key the kit derives from the space's key, so
  that destroying the space key leaves those caches unreadable too.
  - `BlobStore::host_cache_key(space, label)` gives the key for sealing new entries, and
    `BlobStore::host_cache_key_at(space, label, generation)` gives it for opening entries
    sealed earlier. `label` is the host's name for the use, e.g. `b"host-cache/previews/v1"`.
  - The same space and label always give the same key, across restarts; another label or
    space gives an unrelated one. The kit never stores the key, and `HostCacheKey`'s `Debug`
    prints only its generation.
  - Record the key's `generation()` beside what you seal. Once the space key is destroyed
    (`purge_space_key`), opening with that generation refuses with the new id
    **`MPC-E603 cache_key_destroyed`** (exit 4): drop that cache. It never hands out a key for
    a destroyed generation, even after the space gets a new key.
  - The kit has no cipher of its own: seal with your own, under `secret_bytes()`.
  - Details: `docs/SESSIONS.md`, "Host cache keys".
- **A behaviour change: `purge_space_key` always rewrites the store file,** so the destroyed
  key's bytes leave it. Before, it rewrote the file only when structure-index rows went with
  the space; otherwise the old key could stay in a freed page. `PurgeReport::store_rewritten`
  is now `true` after any purge that destroyed a key, unless the rewrite failed. A rewrite
  copies the whole store file, and other store calls wait while it runs.
- New error id `MPC-E603` (above); new variant `BlobError::CacheKeyDestroyed` (the enum is
  `#[non_exhaustive]`, so a host's code builds as is).
- **A size limit for keyed docs that aren't tables** (a deck, a board). Until now such a doc
  had no size limit in a live session. `Config::keyed_max_entries` (new; default 25,000,
  the CSV grid's measured cap until a measurement on a deck-shaped doc sets it) bounds its
  elements and fields: every list element, deleted ones included until dropped, plus every
  map entry. A file over it doesn't open live: `open_file` returns
  `SessionError::Declined` with code `live_too_large`, the new id **`MPC-E415`
  `declined_live_too_large`** (exit 4), and nothing is stored, so the host opens the file at
  tier 0 (or 0.5) as it does for any declined file. A change that would grow a live doc past
  it is refused, nothing applied: `AtLimit { what: "elements and fields" }`, with its own new
  id **`MPC-E822` `keyed_at_limit`** (exit 2; its next step reads the doc and removes
  elements or fields, not the table's `rows drop-deleted`). An outside save past it declines the
  file (`DiskCheck::Declined`, code `live_too_large`); the file is never written over. Tables keep
  their own limits, as before. `capabilities` lists the new limit.
- **A behaviour change for keyed docs that aren't tables: an app's save over the kit's own
  write keeps both,** the rule tier 0 took in v0.14.0. Until now an outside save of such a
  doc was applied straight onto the live doc by key, with no merge, no note and no audit
  event, so an app that still had the file open from before the kit's write could undo an
  agent's change unseen. Now, when the kit has written the file since the last outside save
  (decided from the store, so a restart between the write and the save changes nothing), an
  outside save that differs keeps both: the save becomes the live doc as before, and the
  kit's version is kept as a version and as a **conflict copy** beside the file (or in the
  host's conflict folder; named by `Host::conflict_copy_name`, then the kit's default, after
  the kit version's last writer). The host sees `DiskCheck::Conflict { version, kept, copy }`
  (`version`: the doc version the save made; `kept`: the id of the version holding the kit's
  version) and **one** `AuditEvent::BlobConflict { current, kept, copy, lease_holder: None }`
  (both ids are version ids) where it saw `DiskCheck::Ingested` and
  `AuditEvent::ExternalEdit` alone; `ExternalEdit` is still emitted. When the kit wrote
  nothing since the last outside save, the save applies as before; a save of exactly what
  the kit wrote is still nothing; tables keep their own three-way merge, unchanged.
  The kit's write of a merge counts too, across a restart: an outside save that lands while
  the doc holds changes the file doesn't is applied by key as before, and its version (by
  `kit:external`) also names **`kit:merge`** among its authors (tier 0's author of its
  merges): the kit writes that merge next, so the app's next stale save keeps both. A host
  that lists a version's authors shows it there. Whatever the store can't say counts as the
  kit's write (keep both). One noisy case stays: after a restart, a save of the kit's content
  in other bytes still reads as the kit's write, so the app's next differing save keeps both
  (an extra copy, never a loss).
- **A live keyed doc's conflict copies are removed on `tick`,** as tier 0's are: after
  `Config::conflict_copy_keep_ms`, only a copy still holding exactly its kept version's
  content, never one a person edited or whose version is named or purged. They are listed in
  `TickReport::conflict_copies_removed`, and a live doc's conflicting save in
  `TickReport::conflicts` too. The kit records each copy in a new per-doc table,
  `live_copies`, created with the first copy, **without a format bump** (the terms of
  `client_kinds` and `doc_blob_refs`): a previous release reads and writes such a store as
  before and never removes these copies (they stay; nothing is lost), refuses a rewrite and
  so a purge before changing anything (`MPC-E502`, naming the table), and leaves the rows of
  a doc it deletes, which this release's open removes. `DocStore::live_copies(doc)` (new,
  read only) lists a doc's recorded copies.
- **A purge of a live doc reaches its conflict copies,** as tier 0's purge does: a copy still
  holding exactly its kept version's content, when the purge erased that content (the
  version's content redacted or gone), is deleted with its record and counted in
  `PurgeCounts::conflict_copies`; a copy whose bytes are anything else (a person edited it)
  may hold erased content the kit can't vouch for, so it is left and reported as
  `NotReached::ConflictCopies`; a copy holding nothing the purge erased stays. A doc without
  its file reports its recorded copies as `NotReached::File`.
- **The carrier (new, additive): a pack that models its format only in part can write the
  file back by splicing.** Two new `Pack` methods, both with defaults, so a pack that doesn't
  opt in builds and behaves as before:
  - `wants_carrier(&self) -> bool` (default `false`): opt in.
  - `encode_from(&self, carrier: &[u8], content: &[(String, Neutral)], codec: &[u8]) ->
    Result<Vec<u8>, PackError>` (default: `encode`, ignoring the carrier). For a pack that opts
    in, the kit calls it instead of `encode` wherever it renders the live doc as a file (the
    write-back, the checks of whether the file holds the doc, `read`'s bytes, `file_now`, a
    conflict copy of the kit's version), passing exactly the bytes it last read from the file
    or wrote to it, and wherever it renders a version, with that version's own carrier.
    Untouched must be byte for byte; the pack decodes the carrier itself.
  - The kit takes the carrier at every read of the file at the sync point and every kit
    write, keeps it as a blob in the doc's space (a blob store is needed for it to survive a
    reload; without one it is kept in memory) and holds one count on it, so blob GC keeps it.
    A carrier it replaces is removed at once when nothing else references it, so **a purge's
    write-back leaves no erased byte in the blob folder**; a doc delete gives the count back.
  - **Each version keeps its carrier** (the file's bytes as of that version), so
    `Session::file_at` of such a pack is that version's file byte for byte, also after later
    edits and an app's save that changed the opaque bytes. A version with none (cut by an older
    kit or before the doc had a carrier, or erased by a purge) is refused with the new id
    **`MPC-E208` `version_no_carrier`** (exit 4; new variant `SessionError::VersionNoCarrier`),
    never `encode`'s partial file. A version's carrier goes with the version (retention, a doc
    delete); a purge drops the carriers of the versions it erased and of any whose bytes hold a
    purged text, and removes their blobs at once.
  - **Conflict copies** of such a pack are compared, on `tick` and by a purge, with what the kit
    wrote (the kept version spliced into its carrier): an untouched copy is removed after its
    keep, and a purge removes one holding erased content, as for any pack.
    On reload a carrier that doesn't hash to the sync point (an older kit wrote the file
    since) is dropped and the file's next read becomes the carrier.
  - Two new per-doc tables, `live_carriers` and `version_carriers`, created with the first
    carrier, **without a format bump**: a previous release reads and writes such a store as
    before (its GC keeps the carriers; its writes leave the live carrier stale, which this
    release drops; its retention leaves version rows, which this release's next load sweeps;
    the versions it cuts have no carrier), refuses a rewrite and so a purge before changing
    anything (`MPC-E502`, naming the table), and leaves the rows and counts of a doc it
    deletes, which this release's open gives back. `NotReached::File` now
    also covers the carrier when the file can't be written back. `Status::estimated_bytes`
    counts the carrier's bytes. Details: `docs/SESSIONS.md`, "The carrier"; pack authors:
    `docs/PACK_TEMPLATE.md`. `tools/n1/n1.sh layout-carrier` checks N−1 both ways.

- **A removal at once never takes a blob a put is waiting to reference** (a behaviour change
  for the blob store, additive in the API). A purge, and now a replaced carrier, remove a blob
  that has no reference left at once instead of after GC's grace. Before, another writer that
  had just put the same bytes in the same space, and not yet referenced them, could then find
  them gone (`NoBlob`). Now every put is *waiting* until its writer has taken its reference
  (the kit's own writers settle it: tier-0 writes, saves, merges, imports, carriers) or
  `BlobConfig::gc_grace_ms` has run out; a removal at once leaves a waiting blob, which a
  purge reports as `NotReached::SharedBlobs`. A host that puts a blob and names it in a value
  keeps the existing rule: reference it within the grace.

- **A structured read never renders the file** (a performance fix; no API change). A pack's
  structured read verb (`PackVerb::structured_read`) read the doc's whole content and encoded
  the file on every call, though it reads the keyed state only (the pptx pack measured 125 ms
  a read at 2,500 shapes). On a keyed doc that isn't a table it now reads the keyed state and
  the version only: no `encode`, no `encode_from`, no check of whether the file holds the doc.
  A failure the last rendering of the doc found (at a write-back, a sync check, a flush) is
  kept with the doc's version, and such a read reports it while the doc is at that version.
  `read`, `file_now`, `file_at`, the text read verbs and a table's read verbs (they print and
  take the file's line numbers) still render the file. And a `flush` renders the
  doc once, not twice: its hash is the file's when the doc is clean. Edits never rendered the
  file; the write-back does, once per sync point (the durability promise is unchanged: a
  change is in the store when its call returns, the file follows at the next write-back).

## v0.14.0 (2026-11-11)

A host release on v0.13.1. The crate version is `0.14.0`; the client's is `0.14.0`. Pairs with
the markdown, text, board, csv and psd pack tags of this release. The store format is the same
(`STORE_FORMAT` 6, `STORE_FORMAT_READ` 7, `STORE_FORMAT_BASE` 5) and no store needs a
migration. No error id is added. The wire gains only what person-writer reuse uses (the
`welcome` frame's `lease`, `kept` and `prevEnded`, and the client's `bye` frame), and a host
that leaves reuse off sends none of it.

**This tag's tree doesn't include the vendored native engine,** as in v0.13.1 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it).

For a host, in short (details below):

- **A behaviour change in tier 0: an app's save over an agent's write keeps both.** When the
  kit has written a file (an agent's whole-file write, a restore, a merge) and no lease is
  held, an app that still has the older file open and saves it keeps the agent's version
  too. The app's save stays in the file and is the current version, and
  the agent's version is kept as a version and as a **conflict copy** beside the file (or in
  the host's conflict folder). The host sees `DiskCheck::Conflict` and
  `AuditEvent::BlobConflict { current, kept, copy, lease_holder: None }` where it saw
  `DiskCheck::Ingested` and `AuditEvent::ExternalEdit`. **`AuditEvent::BlobConflict` is
  emitted once per copy**, with `copy` set (`None` only if the copy couldn't be written): show
  it to the person once, from that event. A save of exactly what the kit wrote is still
  nothing, and a save over another outside save is still a plain new version.
- **`Host::conflict_copy_name(file_name, at_ms, who) -> Option<String>`** (new provided
  method, default `None`): the host names conflict copies in its own words, e.g.
  `notes (Ada's version).md`. The host's name comes first, then the pack's, then the kit's
  default; a name that is empty, `.`, `..` or holds a path separator is passed over, and an
  existing file is never overwritten.
- **`Pack::check_merged(root, text) -> Result<(), Declined>`** (new provided method; the
  default passes every text): a pack can refuse a whole-line merge whose result isn't valid
  for its format. The text pack of this release uses it for JSON (`.json`, `.ipynb`,
  `.geojson`, `.webmanifest`). A refused outside save is taken whole (the replaced text stays
  in the "before external edit" version, with the usual removal notes); a refused
  `write --base` is a conflict, nothing applied.
- **Person-writer reuse, off by default** (`Config::reuse_person_writers`,
  `Config::person_writer_park_ms`): one writer per person per device, so a long-lived doc's
  writer count follows people and devices, not connections. A host turns it on with its own
  pieces (a device key in `JoinOptions`, `Session::release` on the wire's `bye`); off, nothing
  changes. Client 0.14.0 carries the client side and behaves as 0.13.2 with a host that
  doesn't reuse writers.
- **Going back to v0.13.1:** no store format constant changes. A store written by this build
  opens in v0.13.1 as it is; person-writer rows (kind 2 in `client_kinds`) are copied there and
  never reused. `tools/n1` checks both ways.
- **Tooling:** `tools/private-allow.txt` says what `check-private.sh` skips (only the
  allowlist itself).

In detail:

### Tier 0: an outside save over a kit write

- **An app's save over an agent's write keeps both.** Before this release, when the kit had
  written a file and no lease was held, an app that still had the older file open could save
  over it, and its save became the new version: the agent's change was left only in history.
  Now that save keeps both, as a save under a lease always has: **for a person**, the app's
  save stays in the file and is the current version, and the agent's version is kept as a
  version **and** as a conflict copy. The kit decides from the stored version (its reason and
  `via`: a write, a restore, a merge or an import from bytes count as the kit's; an outside
  save or an import from disk as the file's), so a restart between the write and the save
  changes nothing. A save over the file as first opened is still a plain new version. A pack
  that can merge (tier 0.5) is not asked over a kit write with no lease, since the base would
  be the kit's own version. Conservative until the outside-editor bridge: an app that
  reopened the file after the kit's write and then saved also gets a copy.
- **`Host::conflict_copy_name`:** `who` is the replaced version's author name (or the lease
  holder's). Precedence: the host's name, then the pack's (`BlobPack::conflict_copy_name`),
  then the kit's default; ` 2`, ` 3` are added rather than overwrite a file.
- **Tests:** `tests/tier0.rs`: the stale-save rows (kept both, not merged, across a restart,
  with the event once and its copy), an outside save over an outside save still a plain
  version, a save identical to the kit's write still nothing, the host's copy names, and the
  planted fault "stale-after-kit-write ignored" (`differential::Tier0Mutant`) caught by name.
  `a_save_recreating_a_write_of_ours_after_an_outside_save_is_kept`: the person's first save
  over the kit's write is now a conflict (was `Ingested`); its point, the second save not an
  echo, is unchanged.
- **Going back (N-1):** no store format or stored-data change; the decision reads fields
  every version already stores.

### `Pack::check_merged`

- Some formats' lines depend on each other (JSON's commas and brackets), and a whole-line merge
  of two valid texts could leave an invalid one on disk. The kit now asks the pack about a text
  doc's merged text, and acts only when the merge fails while both merged texts pass:
  - **an outside save** whose merge onto the live text is refused is **taken whole**. The
    live text it replaces stays in the "before external edit" version, and the existing
    removal notes tell each author what went and which version holds it;
  - **`write --base`**'s clean merge, refused, is a conflict (`WriteOutcome::Conflicts`, exit
    1, nothing applied): one region over every line either side changed
    (`text::merge::refused_merge`, new).
- `tests/check_merged.rs` (new) has both paths with a test pack that refuses, the agent's
  text recovered from the version the note names, and the planted fault
  (`differential::CheckMergedMutant::Skipped`: the call skipped) caught by name.
  `docs/SESSIONS.md` now says an outside save's merge can be refused by the pack.
- **Going back (N-1):** no store format or stored-data change.

### Person-writer reuse (off by default)

- **`Config::reuse_person_writers`.** One writer per person per device
  (`docs/design/person-writer-reuse-r0.md`, `docs/SESSIONS.md`). A join with a device key
  (`JoinOptions::device`, `device_stateless`) takes a free writer of that person on that
  device; `Session::release` (the wire's new `bye`) frees it; any other end parks it for the
  client's own reclaim (`JoinOptions::reclaim`, `Joined::lease`, `kept`, `prev_ended`), which
  hands a still-held writer over under the session lock. With a device key a bare `prev`
  closes nothing. `Config::person_writer_park_ms` (0: never) lets a parked writer go after
  that long. Without a device key, or off: as before.
- **Store:** `client_kinds` gains kind 2 (`PersonWriterRow`, `DocStore::person_writers`), with
  no format bump; the rows ride in a change's commit and the session's close, and only for
  writers that can write (a read-only join leaves no row). An older kit never reuses or hands
  out such an ID and copies the rows on a rewrite (`tools/n1` checks both ways).
- **Client 0.14.0:** `welcome.lease`, `kept`, `prevEnded`; `ConnectOptions.lease`;
  `LiveText.bye()` and the `bye` frame; a kept reconnect settles the edit in flight for good,
  and a lost change's watch is capped where its writer's lease ended (`client/CHANGELOG.md`).
- **Tests:** `tests/person_writer_leases.rs` (leases and rows), `tests/person_writers.rs` (the
  rows R1–R13 on Yrs, the native engine's column, three planted faults),
  `tests/person_writers_cost.rs` (bytes per writer, join and leave latency),
  `client/test/writer-lease.test.ts`.

### Tooling

- `tools/private-allow.txt`'s header says what `tools/check-private.sh` skips: only the
  allowlist itself (it may quote a match). The script checks itself like any other file,
  since its patterns live outside the tree.

## v0.13.1 (2026-11-10)

A host release on v0.13.0. The crate version is `0.13.1`; the client's stays `0.13.2` (no file
under `client/` changed since v0.13.0). No API, wire frame or store format change since
v0.13.0. Pairs with the markdown, text, board and csv pack tags of this release.

- **`tools/check-private.sh` reads its patterns from outside the tree.** The first of
  `MPC_PRIVATE_PATTERNS` (the patterns, one per line), the file `MPC_PRIVATE_PATTERNS_FILE`
  names, or `~/.config/k2-mpc/private-patterns.txt`: one extended regex per line, matched
  case-insensitively, `#` comments and blank lines ignored. With none it fails closed (exit 2)
  and names the three places; it never prints a pattern. The allowlist,
  `tools/private-allow.txt`, works as before. `tools/test-check-private.sh` (new) is its
  self-test, in the evidence part and `scripts/check.sh`.
- **`tools/public-cut.sh --hold FILE`** (or `PUBLIC_CUT_HOLD`): an optional hold list kept
  outside the tree; paths named in it are taken from the given ref, and a patch after them is
  applied, before the cut's own steps. Without one the script runs as before.
- **Docs:** `Rights::comment`'s doc comment and `docs/SESSIONS.md` say comment is implied by
  edit by a product ruling.

## v0.13.0 (2026-11-10)

A host release on v0.12.0. The crate version is `0.13.0`; the client's stays `0.13.2` (no file
under `client/` changed since v0.12.0). Pairs with the markdown, text, board and csv pack tags
of this release. No wire frame changes, and the store format is the same (`STORE_FORMAT` 6,
`STORE_FORMAT_READ` 7, `STORE_FORMAT_BASE` 5): the structure index's and the comment threads'
tables are additive and made by their first write, so a store needs no migration.

**This tag's tree doesn't include the vendored native engine,** as in v0.12.0 (the release commit is
cut by `tools/public-cut.sh`; main keeps all of it). The cut leaves out what v0.12.0's did: the
native engine's vendored crates and licence texts (`vendor/mpce-model` stays), the kit's
native-engine adapter (features `native`, `fuzzing`), the conformance crate, the fuzz trees and the
evidence run records; and the engine-feature guard with its self-test
(`tools/check-engine-features.sh`, `tools/test-engine-features.sh`).

For a host, in short (details below):

- **The structure index, opt-in per pack:** a pack that indexes turns a blob version into a
  tree of nodes the host indexes with its own worker; `BlobSession` reads it with rights and
  paged answers (`index_list`, `index_nodes`, `index_diff`, `index_blame`), anchors a comment
  on a node or the whole doc (`crate::anchor`), keeps comment threads when the host adopts
  them (`crate::annot`, `Registry::with_annotations`), and takes advisory claims by node.
  `AnyFile` and a pack with no index are unchanged, as is `Registry::for_tier`. The new enum
  variants and struct fields are on `#[non_exhaustive]` types, so a host's code builds as is.
- **What a host may want to adopt:** `Rights::comment` (implied by edit; a read-only share
  comments only when granted), `Need::Comment`, `Action::Annotate`; `AuditEvent::IndexWanted`
  (the host runs the index job), `AuditEvent::Annotation` and `AuditEvent::AnchorState`;
  `PurgeReport::index_rows_removed` and `store_rewritten` (a `purge_space_key` that removed
  index rows rewrites the store file; `false` with rows removed means the rewrite failed and
  their old bytes stay in freed pages until a later rewrite).
- **New `Config` fields, with defaults** (in `capabilities` with the other limits):
  `blob_index_max_bytes`, `blob_index_max_nodes`, `blob_index_max_stored_bytes`,
  `annot_body_max_bytes`, `annot_threads_max`, `annot_comments_max`, `annot_thread_log_max`.
  New error ids `MPC-E116`, `E117`, `E174`, `E175`, `E320`, `E321`, `E711` to `E717` and
  `E907`; `MPC-E172` and `E173` are live (reserved until now); a claim on a node another holds
  is `MPC-E301` (exit 5), as for elements. The ID kind `annotation` (`ann-12`) is new.
- **For packs, the parity harness:** `BlobCase` gains `next` (with `then(bytes)`; build a case
  with `BlobCase::new`, as before), and `BlobPresenceObs` is `#[non_exhaustive]` and gains
  `claims`.
- **A test row for keys in `Debug`** (`key_debug_tests`): no kit type holding key material
  prints it in `Debug`, in any spelling of known bytes.
- **Client 0.13.2:** unchanged.
- **Going back to v0.12.0:** no store format constant changes. A store with no index and no
  comment is one v0.12.0 reads as before. One with the new tables follows the additive rules
  (`docs/SESSIONS.md`): v0.12.0 opens it, reads and edits work, and its purge and rewrite
  refuse.
- **Main-only tooling** (not in this cut): the engine-feature guard also refuses the engine
  crates' `test-support` and `verify-...` hooks; the fuzz night reads a forking target's
  counters from its last progress line (`src=beat`), keeps libFuzzer's dictionary entry out of
  its heartbeat lines, and lists the PSD pack's targets and the kit's `index_result` target in
  the ledger.

In detail:

### Tooling: the fuzz night (main-only, not in this cut)

- **A forking target's counters** (`-fork`, as `outside_edit` runs) come from its parent's last
  progress line: the night's `FUZZ` line says `src=beat`; `src=final` for an in-process run's
  final stats, and `src=-` when there is no line of either (every counter `?`, and the ledger's
  "executed nothing" rule fires as before). Self-test: `tools/fuzz/test-ledger.py`.
- **A heartbeat line leaves out libFuzzer's dictionary entry** (`DE: "..."`, a few bytes of a
  mutation), so job logs carry no input bytes.
- **The ledger's paths** list the PSD pack's targets, and the kit's `index_result` target (a
  structure-index worker's result: a typed refusal or a checked draft).

### Anchors and comment threads

- **Anchors (`crate::anchor`, pure):** an anchor on the whole doc or on a node by its key, and
  its state in a version, exactly one of attached, hint-stale, suspect (duplicate key, weak key,
  kind changed, identity doubt, candidate only, hints purged), hidden, orphaned, by a fixed
  precedence; candidates are shown and never taken. `index::Fingerprint` gains `content` (the
  node's keyed content hash), so the judge needs only the anchor and the later node.
  `BlobPack::judge_anchor` and `anchor_candidate` (defaulted: no pack changes) are the file type's
  half.
- **Comment threads (`crate::annot`), opt-in per host:** `BlobSession::comment`, `reply`,
  `resolve`, `reopen`, `delete_comment`, `thread`, `comments`, `annotation_log`, `reanchor`,
  `purge_annotations`, `erase_participant`, and `run_annotation` (the verbs over the same calls;
  their rows are `parity::ANNOTATION_VERBS`, added by `Registry::with_annotations`, no tier, so
  `Registry::for_tier` is unchanged). Three additive per-doc tables (`annot_threads`,
  `annot_comments`, `annot_log`), created by a doc's first comment: a store with none is as
  before. A comment makes no version and no undo step; an agent's comment on someone's behalf
  starts no approval; an agent's re-anchor is a proposal a person confirms; deleting clears the
  body at once (no undo). Purge and participant erasure clear bodies and rewrite the store; a
  version purge takes the anchors' copies of that version's node names and hints.
- **Rights:** `Rights::comment` (`with_comment`, `may_comment`): implied by edit; a read-only
  share comments only when granted. `Need::Comment`, `Action::Annotate`.
- **New:** `AuditEvent::Annotation` (with mentions, no body) and `AuditEvent::AnchorState`;
  `PurgedScope::Annotations`, `AnnotationText`; `NotReached::SentNotifications` (on annotation
  purges only); `PurgeCounts::comments`, `anchors`; `Config::annot_body_max_bytes`,
  `annot_threads_max`, `annot_comments_max`; error ids `MPC-E116`, `E117`, `E320`, `E321`,
  `E907` (`E906` belongs to other work); the ID kind `annotation` (`ann-12`). No store format bump.
- **Review round:** five tables (a thread-history index and a per-thread change row, so
  `comments --since` sees every change and reads one row a thread); `annot_thread_log_max`;
  `Registry::for_blob_pack` lists the annotation verbs and `BlobSession::run_tool` runs them;
  a promoted doc's text purges and erasure reach the threads it kept; attribution ids bounded at
  the write, display fields cut; anchors written only from a live version; index records
  resolved off the transaction and applied by compare-and-set; blob erasure refuses what
  Session's does and waits for comment writes already admitted, and clears bodies written on
  the erased person's behalf; readers without history see only the current version's node
  copies; a purge's clear of an anchor sticks until a re-anchor.
### The structure index: reading it and claims by node

- **Typed reads with rights**, on `BlobSession`: `index_list` (the outline, depth-first, each
  node's `#N`), `index_nodes` (several addresses, `None` per miss with candidates), `index_diff`
  (node by node: added, removed, moved, renamed, props, content, text), `index_blame` (per node,
  the version that made it as it is; only indexes that exist), `node_labels` (for a roster whose
  regions are node keys). Read, plus history for another version, a diff and a blame, asked on
  every call; App viewers see handles. `BlobStatus::index` says where the current version's
  index stands.
- **The verbs: `BlobSession::run_tool`** (new), a parser over those calls under the names the
  pack gives (`IndexVerbs`), with `outline` and `node` as the kit's names. A partial
  `node` read exits 4 with `MPC-E172 address_not_found` and lists every address. Reads page under
  `ToolOptions::read_budget` (default `READ_BUDGET`, 32,768 bytes of the page's compact JSON) with
  opaque cursors bound to the doc, version, verb and arguments: `MPC-E173
  stale_cursor`, `MPC-E174 bad_cursor`, `MPC-E175 over_budget` (`--no-truncate`). A node larger
  than a page comes alone without its text, which `--text` reads whole.
- **`Registry::for_blob_pack`** (new): tier 0's verbs, plus a pack's index verbs and node claims
  when it has an index. `for_tier(Tier::Blob)` is unchanged, as is a pack with no index.
  `parity::kit_index_cases` checks the verbs against the typed calls on two versions of a file
  the pack gives.
- **Claims by node**: `BlobSession::claim_nodes`, `renew_claim`, `release_claim`, `claims`, and
  `claim --node`, `claim --renew`, `claim --release`. Advisory: a claim never refuses a write,
  a restore or a lease; another's claim on the same node refuses a new one
  (`SessionError::NodesClaimed`, `MPC-E301`, exit 5). New `ClaimSpan::Nodes`; the roster lists
  node claims; they end on release, expiry, leaving, and `Deleted` when an indexed version holds
  none of their nodes.
- **Export by node, contract only:** `export::NODE_OPTION` and `ExportError::NeedsRenderer`
  (`MPC-E716 export_needs_renderer`, exit 4). The pack's exporter and the host's worker run it.
- New: `SessionError::IndexRead` (an address or cursor refusal with its own id), the `@index`
  group in the error registry; `MPC-E172` and `MPC-E173` are live; `MPC-E711`, `E712`, `E713`
  name the index verbs. `ToolOptions` gains `read_budget` and `json`.
- **Every page fits the budget, in text and JSON**, whatever one item holds: an item counts
  as the larger of its two forms, and one that couldn't fit a page alone is shortened a step at
  a time (its text, then its descendants to a `--under` continuation, then every field cut to
  256 bytes and every list to 16 entries, then a stub), each step naming the read that returns
  what it left out: `<show> ADDR --text`, `<show> ADDR --whole` (new: the node and its
  ancestors in full), `<list> --under ADDR` (new; `ListQuery::under`). `READ_BUDGET_MIN` is
  16,384, room for any frame and a stub. `--no-truncate` refuses whenever the whole answer is
  over the budget, a shortened item included. Text pages print the same shortened items.
- **Candidates capped:** at most `CANDIDATES_MAX` (20) for a missing or an
  ambiguous address, with `candidates_total` (`MissingAddress::candidates_total`, `MPC-E170`'s
  `details.candidates_total`). A node read's misses are on its first page only, at most a
  quarter of the budget, with `missing_total`; `MPC-E172` lists the first
  `MISSING_LISTED_MAX` (20).
- **Claims decode no index under the session lock:** a claim is checked against the current
  version's index off the lock (then the lock is taken again and everything re-checked), and a
  new version's index is decoded once for the claims, before the roster, `claims` or `tick`,
  not on every call.
- **Parity harness:** `BlobCase` gains `next` and `then(bytes)` (a second version for the
  setup); `BlobStage` gains `index_verbs`, `index_all` and `write_next_as`; `BlobPresenceObs`
  gains `claims` and is `#[non_exhaustive]` (the harness builds it). `BlobCase` stays open: its
  callers vary a case with struct-update syntax (`BlobCase { person, ..case }`), which
  `non_exhaustive` forbids outside the crate; add fields with a default in `BlobCase::new`.
  `parity::kit_index_cases` (new).

### The structure index

- **New, opt-in per pack:** `BlobPack::index_spec`, `index`, `agent_writes` (all defaulted, so
  `AnyFile` and every existing pack are unchanged). A pack that indexes turns a blob version into
  a tree of generic nodes (`crate::index`), kept in the additive, lazily created tables
  `blob_struct` (content-keyed) and `blob_struct_keys` (per doc, minted keys). A host runs the
  work itself: `AuditEvent::IndexWanted`, then `BlobSession::index_job` and `IndexJob::run` or
  (sandboxed worker) `open` / `index::index_to_wire` / `commit_wire`. Malformed worker results are
  refused and record nothing; caps (nodes, depth, stored bytes, file size) are typed refusals
  recorded as `NotIndexed`. Hashes stored are keyed per space; a purge removes the row in the
  blob's own transaction, and the byte scan proves nothing is left. New `Config` fields
  (`blob_index_max_bytes`, `blob_index_max_nodes`, `blob_index_max_stored_bytes`), events, and
  error ids `MPC-E711`, `E712`, `E713`, `E714`, `E715`, `E717`. No store format bump. A
  `purge_space_key` that removed index rows also rewrites the store file.
- **Hardening in this release:** key overlays record their pack and format (an overlay of another
  format is absent, never laid over a new row); the digest key is derived with the space key as the
  message, so no blob id can equal it; the orphan sweep finds rows in a read transaction; minted
  keys are written once per format, and a late job changes nothing already shown; a deleted version
  loses its overlay; a stored row with a bad parent is `Corrupt` at decode; a worker's result is
  capped field by field and on total decoded size.

### Guards: the engine's test hooks, and keys in `Debug`

- `tools/check-engine-features.sh` also refuses `test-support` and `verify-...` on the native
  engine's crates (a kit feature forwarding one, a dependency enabling one, one on the
  shipped graph). The allowed `verify-shortcuts` dev-dependencies and the kit's own `test-support`
  still pass. `tools/test-engine-features.sh` is its self-test (evidence part, `scripts/check.sh`).
  Both scripts are main-only: the public cut leaves them out.
- A new test row, `key_debug_tests`: no kit type holding key material prints it in `Debug`
  (`SpaceKey`, and with `native` `NativeKeys`, `EngineChoice`, and the native engine's store secret
  and digest key), in any spelling of known bytes, and each `Debug` is held to its exact layout.

## v0.12.0 (2026-11-07)

A host release on v0.11.1. The crate version is `0.12.0`; the client's is `0.13.2`. Pairs with
the markdown, text and board pack tags of this release. **One breaking change, for a host that
builds or reads a kit `LogEntry`:** its `update` bytes are now `body: EntryBody` (below). No
wire frame changes, and the store format is the same: Yrs log entries stay record version 3,
byte for byte, so a store needs no migration.

**This tag's tree doesn't include the vendored native engine,** as in v0.11.1 (the release commit is
cut by `tools/public-cut.sh`; main keeps all of it). The cut leaves out what v0.11.1's did: the
native engine's vendored crates and licence texts (`vendor/mpce-model` stays), the kit's
native-engine adapter (features `native`, `fuzzing`), the conformance crate, the fuzz trees and the
evidence run records.

For a host, in short (details below):

- **The log entry's body is a type (breaking).** `LogEntry.update: Vec<u8>` is
  `LogEntry.body: EntryBody`: `EntryBody::Yrs(update)` for a doc on Yrs, which is every doc a
  session stores today. `LogEntry::yrs(author, client, at_ms, via, update, on_behalf_of)`
  builds one; a reader matches `EntryBody`, or takes `EntryBody::yrs_len`. A host whose code
  neither builds nor reads a `LogEntry` changes nothing: a build that names `update` fails to
  compile (`E0609`), so the build is the check. `DocStore::load` refuses a log that mixes
  engines, and a doc another engine stamped (`MPC-E520`).
- **A grid's rows are a limit of their own: `Config::grid_max_rows`** (default 100,000 rows,
  live or deleted, blank ones included). A change past it is `AtLimit` (`MPC-E802`, exit 2,
  `what` `rows`), with the same message and next step as cells and columns; an import past it
  opens as text (`too_large`), and an outside save past it declines the file. A host that
  shows `MPC-E802` by its `what` adds `rows`; one that shows the row's message changes
  nothing. In `capabilities` with the other limits.
- **A grid's paths are linear in its stored cells and rows** (render, line maps, the aligner,
  the merge, blame, the conflict listing, the row list, an axis's inserts and moves, and the
  rekey of dropped rows). Results are unchanged. `Table::row_cells` and `Table::row_quotes`
  are new (additive).
- **Not for hosts yet:** `NativeKeys` and `session::NativeKeysFor` are behind the `native`
  feature, which this cut leaves out; `MPCE-E566`'s next step says the host supplies a native
  store's secret and keeps it outside the store.
- **Client 0.13.2** (`client/CHANGELOG.md`): the client's code is as in 0.13.1; a wording fix
  in 0.13.0's entry.
- **Going back to v0.11.1:** no store format constant changes (`STORE_FORMAT` 6,
  `STORE_FORMAT_READ` 7, `STORE_FORMAT_BASE` 5). No session makes a native entry, so a store
  this release has run on holds Yrs entries only, record version 3 byte for byte, which
  v0.11.1 reads.
- **Main-only tooling** (not in this cut): the fuzz night's record carries each target's
  capped-input count and the host's load while fuzzing, its counters are read from libFuzzer's
  own lines only, and a build that changes the fuzz crate's lock fails the night.
- **Docs:** `docs/evidence/README.md` (new) says the run records the evidence notes cite live
  on the kit's main branch only, as a public cut leaves them out; `docs/SESSIONS.md` states
  the erasure limit; `docs/POSITIONS.md` is read against the code.

In detail:

### Grids: render and reconcile linear in a grid's stored cells and rows

- **`Table::render`, `Table::line_map`, the aligner (`table::reconcile`), the structured
  pack's default reconcile, `merge3_text` / `merge3_outside_text` and a grid's blame** read
  each row's own present cells, placed by a column-position map built once: every path is
  linear in the grid's stored cells, rows and columns, plus the records it writes, and a
  blank row costs O(1). Before this release each visited every live column of every row,
  both keys cloned per lookup, so a wide grid of blank rows (which hold no cell, so the cell
  limit didn't bound them) cost rows × columns per call. Results are unchanged: the same
  records, line maps, pairings (the aligner's fingerprints keep their equality, and its diff
  budget still counts their full length), edits, conflicts and blame. Measured with
  `tests/grid_scaling.rs` (allocations, timing-free; its regression holds each path to a
  fixed multiple of the stored size, and the per-column walk is a planted fault,
  `differential::GridCostMutant::PerColumn`).
- **`Table::row_cells`** and **`Table::row_quotes`** (new): a row's present cells and its
  quote exceptions, in column-key order, O(log n + the row's entries).
- **An axis's places are a linked list over an arena**, each known key's place in a map, so
  finding a place, inserting, moving and dropping are O(1): a change that inserts or moves
  many rows costs O(its ops), not ops × rows. `Axis::live_index` and `LineMap::row_spans`
  (new) read each live key's index and each row's lines in one pass. The merge's conflicts,
  the session's conflict listing (`write --base`), the undo report's regions, an outside
  save's overrides and `row list` read their indexes once and each row's own cells. Outputs
  are unchanged; the walks stay as planted faults (`GridCostMutant::AxisWalk`,
  `ConflictWalk`, `RowListWalk`).
- **The rekey of the rows and columns the live side dropped** (an outside save, or a live
  write from an older version) reads each version's live places once, a row's cells by its own
  range and the candidate columns' cells in one pass: O(the tables) however many were
  dropped, where each dropped element scanned both versions' cells. Keys are minted in the
  same order; the walk stays as the planted fault `GridCostMutant::RekeyWalk`.
- **`Config::grid_max_rows`** (new knob, default 100,000): at most this many rows, live or
  deleted, blank ones included. A change past it is `AtLimit` (`MPC-E802`, exit 2, `what`
  `rows`; the same message and next step as cells and columns), an import past it is declined
  `too_large` (it opens as text), and an outside save past it declines the file. In
  `capabilities` with the other limits.

### The log entry's body (breaking for hosts that build a `LogEntry`)

- **`LogEntry.update: Vec<u8>` is replaced by `LogEntry.body: EntryBody`**, with
  `EntryBody::Yrs(update)` for a doc on Yrs and `EntryBody::Native(NativeRef { at })` for a
  doc on the native engine (the change's apply position; no bytes). A host that builds a
  `LogEntry` changes its code: `LogEntry::yrs(author, client, at_ms, via, update,
  on_behalf_of)` builds a Yrs entry. A host that reads one matches `EntryBody`
  (`EntryBody::yrs_len` gives a Yrs update's length).
- **A replay takes the log's bytes through `Loaded::yrs_updates` only**, all or nothing: a
  log holding a native entry is refused (`Corrupt("log entry engine")`) before anything is
  replayed, and `Doc::from_store` gives `DocError::Malformed`.
- **`DocStore::load` checks every entry against its doc's engine stamp:** a log that mixes
  engines is `Corrupt("log entry engine")`, and a doc another engine stamped is refused whole
  (`ForeignEngine`, `MPC-E520`) before any entry is decoded. A purge's log pass checks the
  same.
- **Storage:** Yrs entries stay log record version 3, byte for byte, so a store with no native
  doc is unchanged and the previous release reads it. Native entries are a new version 4 (body
  tag 2, the position); versions 1 to 3 read as Yrs. No session makes a native entry yet: a
  native doc can't be stored until a later release stamps it.

### Native documents' keys (feature `native`, not for hosts until the switch)

- **`NativeKeys`** (new): a document's two keys on the native engine, its digest key and its
  store secret, per document for its whole life, host-made and kept outside the store.
  `Debug` prints neither (`NativeKeys { .. }`); it isn't serialisable or cloneable, and it
  converts into `EngineChoice::Native`. **`session::NativeKeysFor`** (new) is the host trait
  that gives a document's keys; sessions don't call it until they store native docs. The kit
  makes no keys and has no random source for them. `docs/SESSIONS.md`, "Native documents:
  keys", states the host's duties, with `MPCE-E566`.
- **`MPCE-E566`'s next step** says the host supplies the store's secret and keeps it in a file
  of its own, never in the store, and never makes a new secret for a store that has native
  records (its explain golden with it).
- **`docs/SESSIONS.md`, "Erasure":** a purge holds against the store as it stands, not
  against an older copy of it. On the native engine a record put back or edited in place is
  refused at load, but a restored older copy, or one cut short at its end, loads as the doc
  was before the purge: erasing backups and older copies is the host's job. A row in the
  table of what purge doesn't reach; `purge_docs_state_the_erasure_limit` holds the text.

### Tooling and docs

- **The fuzz night (main only; not in a public cut):** each target's capped-input count
  (`capped=m/n`) and the host's load while fuzzing (a `LOAD` line) are in the night's record
  and the ledger; the record's counters are read from libFuzzer's own lines only (digits or
  `?`), with a self-test in the evidence part; and a build that changes the fuzz crate's lock
  fails the night (`exit=lock`).
- **`docs/evidence/README.md`** (new): the run records the evidence notes cite live on the
  kit's main branch only; a public cut leaves `docs/evidence/runs/` out.
- **`docs/POSITIONS.md`** read against the code: its file and line references, the client's
  presence and marks data in UTF-16 code units, and the grid verb header's own declaration.
- **The undo model's job tools:** `tools/undo/expect.sh` refuses a second source, and
  `expect_mutants` prints its variant and refuses an unknown one.

### Corrections (the spec-undo large trace runs, kit `856e5000`)

- **`d1_traces_large`:** the v0.9.7 note that `d1_traces_large` ran 50,000 traces on a node had
  no recorded run; it was first run of record at `856e5000` (job `19c369bd`): 50,000 traces,
  0 mismatches, D1's classes hit (`d1_widened` 1,078,940, `d1_widened_then_undone` 474,091),
  every floor met.
- **The undo spec's Volume target (at least 10^4 per case class) is not met for every
  class.** The runs now named (`tools/undo/traces-large.sh`) enforce each class at half its
  measured rate at their own seeds, and reach:
  - `trace_equivalence_large`, 400,000 traces in five parts (jobs `66c739e3`, `6cc3792a`,
    `d9b2296a`, `1e6e6175`, `61eff0f4`), 0 mismatches: `redo_after_partial` enforced at about
    2,500 and reached 5,417; `concurrent_restore_o4a` enforced at about 7,750 and reached
    14,504; every other class enforced above 10^4;
  - `group_trace_equivalence_large`, 10,000 traces (job `c74bd3f6`), 0 mismatches: its rarest
    grouping class, a redo of a group, reached 1,227 (each grouping class at least 100, and
    at least half its measured rate);
  - `end_break_trace_class_large`, 50,000 traces (job `c844bc1b`), 0 mismatches: each of its
    classes at least 10^4 (the least, `write_base_shared_start_was_conflict`, 12,270).
  The `#[ignore]`d large runs had floors scaled from 256 traces and no script. Class counts
  are repeatable: the trace harness seeds client IDs (compaction timing reads ID-dependent
  snapshot sizes).
- Compaction timing in the kit depends on client IDs through snapshot sizes (by design; no
  effect on outcomes).

## v0.11.1 (2026-11-04)

A host release on v0.11.0. The crate version is `0.11.1`; the client's is `0.13.1`. Pairs with
the markdown and text pack tags of this release. **A host changes nothing in its own code:** no
public item is added, removed or renamed, no wire frame changes, and the store format is the
same.

**This tag's tree doesn't include the vendored native engine,** as in v0.11.0 (the release
commit is cut by `tools/public-cut.sh`; main keeps all of it). **New in this cut:** it also
leaves out the evidence run records (`docs/evidence/runs`, about 16 MB of job records). The
evidence notes stay in the tree; main keeps the records and checks every citation.

For a host, in short (details below):

- **The conflict details of a refused `write --base` (`MPC-E111`) are complete (fixed).**
  `current_lines` and `theirs` now include every change of the current text that sits inside a
  conflict region, also one that clashes with nothing (an insertion between two clashing
  lines); `your_lines` and `ours` do the same for the writer's text. Before, such a change was
  left out, so they could name too few lines. Nothing written changes, and which writes are
  refused doesn't change; the regions and their base lines are as in v0.11.0.
- **Conflict regions that overlap or touch are one region (O6).** The undo spec states it now
  (`docs/SPEC-UNDO.md`); the kit has grouped them this way since its first cut. No behaviour
  change.
- **Client 0.13.1** (`client/CHANGELOG.md`). No export is added, removed or renamed, and no
  frame changes.
  - **A closed `LiveText` sends nothing:** after `close()`, `undo()` and `redo()` throw (as
    `applyLocal` does), and `reset()`, `setCursor()`, `setVisible()` and `setView()` do nothing.
  - **The own change in a `since` reply (fixed):** after a reconnect, the reply's change from
    this client is taken for the edit in flight only when it is an edit. An undo or redo with
    the same text is applied as a change, and the edit goes to `onRejected`.
  - **Pinned regressions, tests only:** a held undo or redo always goes out, on every path, and
    an undo or redo sent before a reconnect is settled, not sent again. Every public cut must
    carry them (`tools/public-cut-regressions.txt`), with this release's `write --base` tests.
- **`timers`:** the finish of a timed scope is `#[cold]`, so with timers off a scope stays one
  load and a branch. No behaviour change.
- **Going back to v0.11.0:** nothing to do. No store format, table or wire change; a v0.11.0
  build opens, reads, writes and purges a store this release has run on.
- **Model evidence, tests and docs only** (nothing a host builds changes): the undo model
  check's stated claims corrected to what its runs check (`docs/MODEL-CHECK.md`), with new
  mutants for its gaps and a debug-assertions run (`tools/undo/debug.sh`); the provenance of
  every cited job run marked; every spec-undo trace comparison counted and asserted at a stated
  floor, with `write --base` regions compared exactly against the spec oracle and grid holders'
  authors, `on_behalf_of` and `restored_by` compared; and a main-only check that the vendored
  engine's test-only features reach no build of the kit (`tools/check-engine-features.sh`, not
  in a public cut).

In detail:

- **A public cut carries the evidence notes without their run records.** `docs/evidence/runs/`
  (about 16 MB of job records) joins `DROPPED_TREES` in `tools/check-public-cut.sh`, so
  `tools/public-cut.sh` removes it and the checker blocks a cut that still has a file there.
  The notes (`docs/evidence/*.md`) stay. `python3 tools/evidence/check.py`, which needs the
  records, sits in `public-cut: drop` markers in `scripts/check.sh` and in the `evidence` part
  of `tools/tests/full.sh`, so a cut runs the evidence self-test, check-private, the
  Windows-names checks and oss-check, and main still runs the citation check on every record.
  The public-cut gate reads the dropped trees it plants under by position (the first and the
  second), so a third tree leaves its plants as they were. Nothing in the kit's crates or the
  JS client changes.

- **`write --base` conflict regions: a doc and spec clarification, no behaviour change.**
  Conflicting hunks of the two sides whose ranges overlap **or touch** are one region, with its
  base range and its lines (in the base, in ours and in theirs) the union of those hunks'. The
  kit has grouped them this way all along; `text::merge`'s doc comments said "overlapping", and
  undo spec §4 had no rule. Now O6 states it (`docs/SPEC-UNDO.md`), and the spec oracle
  (`spec_undo::model::merge::write_base_regions`) and the trace test compare a refused write's
  regions exactly.

- **Fixed: a refused `write --base` (`MPC-E111`) could name too few lines in the current
  text.** When someone else's change inserted lines inside a conflict region without
  clashing with the write (between two clashing lines), `current_lines` stopped short of the
  region's last line in the current text and `theirs` left the inserted lines out; the same
  held for `your_lines` and `ours` on the writer's side. Each side now holds every change of
  that side inside the region, and its lines are where that text stands in that side's text
  (undo spec O6). Nothing else changes: the same writes are refused, and the regions and
  their base lines are as before.

## v0.11.0 (2026-11-04)

The first host release after v0.10.2. The crate version is `0.11.0`; the client's is `0.13.0`.
Pairs with the markdown and text pack tags of this release. The production timers of the
v0.10.1 line ship here, off by default.

**This tag's tree doesn't include the vendored native engine** (its crates and licence texts,
the `native` feature and the conformance crate,
which needs it): the release commit is cut by `tools/public-cut.sh`, and
`tools/check-public-cut.sh` blocks the tag if any of it is left, if the licence files are
missing, or if a pinned regression test of an earlier host tag is gone. Main keeps all of it.
The cut drops the conformance crate whole, so this tag runs none of the engine-generic
conformance suite (its Yrs columns included); main runs all of it.

For a host, in short (details in the sections below):

- **What a host's code may need, at compile time:** `errors::Value`, `parity::Source` and
  `VerbSpec` are now `#[non_exhaustive]`. A `match` on `Value` or `Source` needs a wildcard arm
  (`_ =>`), and a `VerbSpec` can't be built as a literal outside the kit; `==` and reading the
  fields work as before. No pack matches on either enum or builds a `VerbSpec`. Every other type
  that gains a field or a variant here was already `#[non_exhaustive]`, and no public function,
  field or item is removed.
- **What a host may see at run time:**
  - The JS client sends an undo or redo made while edits are queued after them, in order, so the
    server undoes the step the person meant (`client/CHANGELOG.md`; the same fix as v0.10.2.1).
  - Client ID `2^53 - 1` is refused (`DocError::BadClient`) from a host or a client's update, as
    0 and 1 are. The kit's client IDs are `2..=2^53 - 2` (`doc::MAX_KIT_CLIENT`); the top one
    is reserved (`doc::IMPORT_CLIENT`).
  - `capabilities`: `who` and `activity` forms split by tier (read a form's `tier`, which was
    `null`), the new unit `doc_coords`, and a row for every pack verb of a pack's registry, one
    that declares nothing as `"undeclared": true` with an empty `forms`.
  - `MPC-E114`'s `next_step` runs `cat` in place of `wc -l` (the id, name, exit and verbs are
    the same).
  - A read verb that fails moves nothing: no activity, no last read.
  - `DocStore::client_owner` and `clients_of` have no row for a client ID that hasn't written,
    unless it is an agent's or an own replica's with edit rights.
  - On grids only: line numbers on a file holding a lone CR (`MPC-E176`, `MPC-E177`, `row
    list` printing `-`, `GridConflict.lines` `None`).
  - The positional default stays `Warn` (`tools::POSITIONAL_DEFAULT`), as in v0.10.2. The flip
    of the positional default to `Refuse`, announced in v0.10.2, is deferred to a later release;
    hosts will be told before it flips.
- **Going back to v0.10.2 or before:** two new store tables, `client_kinds` and
  `doc_blob_refs`, with no format bump. An older kit opens, reads and writes such a store, but
  refuses a store-file rewrite (and so a purge) with `MPC-E502` `store_newer` before changing
  anything, until the upgrade; routine history compaction is unaffected. Its doc delete leaves
  those tables' rows, which this kit's next open removes or gives back.
- **New, additive:**
  - recording a save the host made itself (`BlobSession::expect_save` and `record_saved`,
    `MPC-E413`, `MPC-E414`, `Config::save_ticket_ms`);
  - element presence (`Session::set_selection`, `set_pointer`, `set_viewport`, `MPC-E310`) and
    claims on elements (`Session::claim_elems`, `Pack::composite`);
  - values naming blobs (`Pack::blob_ref_roots`, `Session::blob_refs`, `MPC-E720`);
  - keys two writers never mint alike (`mint`, `Pack::tagged_keys`, `Session::mint_key`,
    `MPC-E820`, `MPC-E821`) and references to elements (`refs`);
  - a file type's own verbs in `capabilities` (`Registry::for_file`, `FileVerb`,
    `Source::File`), with the board file type's error rows (`MPC-E830` to `MPC-E859`); a file
    registry alone gets a page without the kit's own sections;
  - pack verbs' position units (`PackVerb::positions`);
  - fractions in error details (`Value::float`, `Finite`);
  - the production timers (`k2_mpc_base::timers`, off by default);
  - the audit event `AuditEvent::NotesWithoutChange`.
- **Fixes:** a change, an outside save or a purge that landed in the store returns `Ok`;
  grids' claims, remaps, failed reads, line numbers and inserts with `--base`; a change that
  doesn't land leaves nothing for the next one; a failed read counts as no read; an own
  replica's client ID after a reload; a client ID's store row lands with its first change.
- **Client 0.13.0, additive only:** `PresenceChannel` (selection, pointer, viewport and
  claims), its frames and `welcome.features`, and the `mint` and `refs` helpers, checked
  against the kit's shared vectors (`client/CHANGELOG.md`).
- **The native engine** stays on main only (feature `native`, test-only); it is not in this
  tag's tree (see above). Yrs is the engine for hosts, at `k2-yrs-0.28.0-k2.6.5`, the same as
  in v0.10.2 (`vendor/README.md`).
- **Licence: MIT OR Apache-2.0**, checked by `tools/oss-check.sh`.

## v0.11.0: fractions in error details

- **`errors::Value` gains `Float(Finite)`, new.** A refusal's details can carry a number with a
  fraction (a board object's `x` and `y`, say): `ErrorReport::with("x", Value::float(12.5)?)`.
  `Finite` holds a finite `f64` only: `Finite::new(f64) -> Option<Finite>` and `.get()`;
  `Value::float(f64) -> Option<Value>` gives `None` for NaN and the infinities, and there is no
  `From<f64>`, so every `Value` prints as JSON. `Value::as_f64` reads a float, or an integer an
  `f64` holds exactly (at most 2^53 from zero).
- **Printed in Rust's shortest form that reads back as the same `f64`,** never with an exponent:
  `0.5`, `-2.75`, `1.0` as `1`, `1e21` as `1000000000000000000000`. `Finite` is equal by bit
  pattern, so `Value` and `ErrorReport` keep `Eq`, and `-0.0` and `0.0` are two values: `-0.0`
  prints as `-0`, a JSON number.
- **`Value` is now `#[non_exhaustive]`.** A host that matches on it needs a wildcard arm
  (`_ =>`). Building one and `==` work as before. No pack matches on it.
- No change to any existing output: every id, golden and wire frame is as before.
- **Tests:** `errors::json::tests` (each value printed and read back by its bits, NaN and the
  infinities refused, equality by bits, `as_f64`), `tests/error_ids.rs`
  `float_details_print_as_json_numbers` (the new golden `float_values.txt`, `-0` among them, and
  a refusal carrying float details in `to_json` and `to_json_for`).

## v0.11.0: recording a save the host made

- **A host that writes a tier 0 file itself records the save in two steps, new.**
  `BlobSession::expect_save(conn, base, bytes) -> SaveTicket` before it writes, then
  `BlobSession::record_saved(conn, ticket)` after its rename. The version is the connection's
  participant's: pid, name, `via`, `on_behalf_of` (an assistant's save on someone's behalf
  records a `Pending` approval), reason `Write`, the file's mtime. Returns `BlobWriteResult`;
  `written` is always false. The kit never writes the file here.
- **The same rules as a write:** edit rights, someone else's lease refuses the ticket
  (`Leased`), and `base` must be current (`Stale`), at the ticket and again at the record.
  With no lease held, the ticket takes one for its own length. Bytes equal to the current
  version's record nothing (`changed: false`).
- **The watcher in between is harmless:** while the ticket is open, a `file_changed`,
  `check_disk` or `tick` that finds exactly the ticket's bytes waits for the record (one
  version, the person's; no conflict copy under a lease), and the kit's write-back waits too.
  Other bytes are an outside save, as before.
- **A ticket ends** when recorded or refused, when the same participant takes a new one,
  after `Config::save_ticket_ms` (new, 30 s, host clock), and on unload. Its bytes, if the
  host wrote them, then come in as an outside save at the next look at the file, so nothing
  is lost.
- **A good ticket that recorded nothing is `Ok`:** `changed: false` and the new
  `BlobWriteResult::not_recorded` (`NotRecorded`: `Lapsed`, `Replaced`, `Reloaded`; `None`
  on every other result). A host may show "saved, not credited". A ticket from an earlier
  load of the doc reads as `Reloaded`; within a load the doc remembers its last
  `SAVE_TICKETS_KEPT` (1,024) ended tickets, and an older one reads as invalid.
- **After the record** the bytes count as the kit's own write, as a write-back's do: the
  save's change event records nothing more, and an app saving them again later is an echo.
- **New refusals:** `MPC-E413` `save_mismatch`, exit 1 (the file holds other bytes, or no
  file), and `MPC-E414` `save_ticket_invalid`, exit 2, a host bug (a ticket recorded twice,
  another connection's, or not this doc's). Nothing is recorded.
- No store format change, no wire change.
- **Tests:** `tests/tier0.rs` `record_saved_*`; the error walk reaches `MPC-E413` and
  `MPC-E414`.

## v0.11.0: the board file type's error rows, and Registry::for_file

- **30 new ids, MPC-E830 to E859, for the board file type's own agent verbs** (8xx,
  structured): conflicts exit 1 (`conflict` and `exists` for objects, `busy`, `merge_conflict`,
  `board_conflict`, `settings_conflict`, `person_exists`), a locked object and voting 5
  (`locked`, `voting_off`, `no_votes_left`), a missing object or a file that can't be read 4
  (E838 to E846), the caller's input or an over-limit write 2 (E847 to E855), `no_free_id` and
  `board_io` 1. Format refusals are grouped by family, the exact code in `details.code`, as for
  a pack's declines (E407). Rights are the kit's E101 to E106. E860 to E869 are reserved for
  the board's live verbs; E822 to E829 stay free. Each row's verbs are `@file`, then the board
  verbs that can return it, as data: the kit doesn't depend on the board crate. 17 rows carry a
  `next_step` naming a board verb; 13 have nothing to run.
- **`Registry::for_file(&[FileVerb])`**: a file type's own verbs from its host's data (name,
  kind, usage, exits, and the file type's parity rows by name), as `Source::File` verbs with
  `VerbSpec::serves_named`. `capabilities` lists them as it lists the kit's, `source` `file`,
  with the ids of the rows that name them; a kit row never lists under a file verb, nor a file
  row under a kit verb, and a host serving both lists a kit verb and a file verb of one name
  apart. The kit's parity harnesses refuse a file registry.
- **A file registry alone gets its own page:** `capabilities` with only file verbs leaves out
  the kit's doc-level rows (`errors_any_verb`), `parity`, `limits` and `positions`, in the JSON
  and in the text form, whose verbs header says they are the file type's own. With a kit
  registry beside it, the page has them all, and the kit's own page is unchanged.
- **For a host:** `Source` gains `File`, and `VerbSpec` gains `serves_named` (it is
  `non_exhaustive`). The kit's own `capabilities` and every existing id's `explain` are
  unchanged; `docs/ERRORS.md` gains the rows and a paragraph on `@file`.
- **`Source` is now `#[non_exhaustive]`.** A host that matches on `Source` needs a wildcard
  arm (`_ =>`); `==` and `!=` work as before. No pack matches on it.
- **Tests:** `tests/error_ids.rs` (`board_rows_name_the_boards_verbs`,
  `capabilities_lists_a_file_registry` (which also checks a file registry's page leaves out
  those sections and the kit's page keeps them), with the board's verbs as data in
  `tests/board_verbs/`), `parity::registry::tests::a_file_registry_is_the_hosts_data`.

## v0.11.0: client rows land with a client ID's first change

- **A join that never writes adds no row to the store's client table.** A client ID's owner row
  is written with its first change, in the change's own transaction, so the two land together or
  not at all. A read-only participant, a watcher past `participants_max` or a person who never
  edits leaves no row and costs no write commit at join, however often it reconnects.
- **Rows at join stay for what needs them:** an agent or an own replica with edit rights has its
  owner and kind rows from its join, change or no change. An own replica that joined read-only has
  them from the first call that sees its edit rights (any call, `join`, `recheck_rights` or
  `promote_watcher`).
- **What a host sees:** `DocStore::client_owner` and `clients_of` have no row for a client ID that
  hasn't written, unless it is one of those. A read-only agent draws a fresh ID per join and isn't
  reused. A drawn ID that has no row can come round again: the doc holds nothing of it. No format
  bump.
- **A bound on the client table stays open.** `tests/join_store_row.rs` `joins_1m` shows 10^6
  read-only joins leave the table as it was. Writers who reconnect aren't covered: each
  connection that writes adds a row, and a writer own replica adds one per join, until a later
  change.
- **Tests:** `tests/join_store_row.rs`, `store::tests::an_edit_and_its_row_land_together`.
  `tools/tests/wire-guard.sh` pins that a host's build has no `unsynced_for_tests`, the only switch
  for a store commit that skips its sync to disk.

## v0.11.0: paths a Windows checkout can hold

- **`tools/check-windows-names.sh` and its self-test** fail on any path Windows can't check out, in
  the evidence part, `scripts/check.sh` and the public cut's checks.

## v0.11.0: blob references from doc versions

- **A doc's values can hold a blob.** A pack declares the roots
  (`Pack::blob_ref_roots`, default none). A list or map value there that is exactly
  `blob:<64 hex>` (`BlobId::to_ref`, `BlobId::parse_ref`) names that blob of the doc's space, and
  the doc holds one count on it, taken in the change's own transaction, while any record names
  it: the live doc, a version or a revert record. No wire change, no format bump.
- **Reads say whether the bytes are there.** `Session::blob_refs` and `Session::blob_refs_at`
  (`DocStore::blob_refs`, `DocStore::blob_refs_at`) return each blob as `Present` or `Missing`,
  with a count of the missing. `blob_refs` needs read, and shows every name on record only with
  the history right; without it, only what a read of the doc shows.
- **New refusal: `MPC-E720` `blob_ref_unknown`, exit 4.** A connection's edit naming a blob the
  doc's space doesn't hold; nothing applied. An import, an outside save, an undo, a redo, a revert
  or a version restore is never refused for one: the value stays, and `blob_refs` reports it
  `Missing`. If the blob arrives, this kit's GC doesn't collect it: the doc takes its count.
- **A purge recomputes the references from what's left,** and a blob nothing references any more
  goes at once (with no blob store on the session: `NotReached::LeftForBlobGc`, new). A doc delete
  gives its counts back; a load takes what's missing for the live doc.
- **Known growth:** retention doesn't release a reference. A blob replaced many times stays until
  a purge or the doc's delete.
- **Store: a new table, `doc_blob_refs` (no format bump),** created by the first name. The counts
  are in `blobs.refs`.
- **Store: one orphan sweep.** An open's removal of a deleted doc's rows is one pass over a list
  of additive per-doc tables (`client_kinds`, `doc_blob_refs`), with no sweep per table.
- **N−1:** an older kit (v0.10.2 and before) opens, reads and writes such a store, and its blob GC
  honours the references this kit took. It refuses a store-file rewrite (and so a purge) with a
  typed error before changing anything (`StoreError::NewerData`, `MPC-E502`); routine history
  compaction is unaffected. Its doc delete leaves the rows
  and their counts, so those blobs stay until this kit's next open gives them back.
- **N−1, the limit: a name written on an older kit is unprotected until this kit's next load of
  that doc, and a blob already collected can't come back.** An older kit takes no reference for a
  value it writes, so with no other reference its GC collects the blob past its grace while the
  doc still names it. After the upgrade such a name reads `Missing`. A load heals what the live doc
  names, so a name only a version or a revert record holds stays unprotected until a purge. K2 and
  the kits it vendors have no pack writing these values, so it takes another host writing them on an
  older kit. `tools/n1/n1.sh node-refs` pins both this and the case that holds.
- **Tests:** `session::blob_ref_tests`, `tests/blob_refs.rs` and the store's. Planted faults
  (`MPC_STRUCT_MUTANT`, feature `differential`): `ref-after-commit` (the reference taken after the
  commit: an interrupted write leaves a named blob with none), `sweep-skips-table` (the sweep leaves
  a table on its list), `no-heal-at-load` (a load takes no hold), `gc-ignores-names` (GC collects
  a blob whose name is on record), `stale-strict-flag` (an undo refused for a missing blob) and
  `refs-ignore-rights` (a reader shown what only history holds).

## v0.11.0: element claims

- **Claims on elements.** `Session::claim_elems` claims named keys of a list root on a keyed doc
  that isn't a table, needing edit. `ClaimSpan` gains `Elems`, and `Session::claim_span` takes it. A
  claim follows moves and never grows, and it ends `deleted` when none of its elements is live.
  Waiters, hand-off reservations and the caps work as for lines.
- **`Pack::composite`** declares the Map roots whose keys belong to list elements
  (`<element>\x1f<rest>`, `session::elem_of_map_key`), so a map write touches its element. A key
  that maps to no element isn't covered by any claim.
- **The claim policy** on a structured edit covers these claims: under `ClaimPolicy::Respect`, an
  agent's change touching another's claim is refused before anything applies.
- **`SessionError::ElemsClaimed`** is `MPC-E301`, exit 5, with `root` and `keys` in its details in
  place of `lines`. Refusals otherwise reuse `MPC-E304` (no keys, more than `CLAIM_KEYS_MAX`, a note
  over `CLAIM_NOTE_MAX` bytes, a table doc), `MPC-E114` (a key that isn't live, a root that isn't a
  list), `MPC-E303`, `MPC-E903` and `MPC-E904`. No registry change.
- `RosterEvent` gains `ClaimStartedElems`; `ClaimStarted` keeps its lines. `ClaimEnd::Deleted` now
  also covers these claims.
- The client's `PresenceChannel.claim` and `release`, `"claims"` in `welcome.features`, the frames
  `claim`, `claimed`, `release` and `released`, the roster kind `claim_started_elems` and the entry
  field `elem_claims` (`client/CHANGELOG.md`). A host builds `elem_claims` from `entry.claims`.

## v0.11.0: element presence

- **Element presence.** `Session::set_selection`, `set_pointer` and `set_viewport`, needing read:
  a connection's elements by key (at most 256), its pointer, and the area it is looking at. A
  selection follows moves, and a key leaves it when its element stops being live. They stay outside
  the doc's numbering.
- `RosterEntry` and `Device` gain `selection`, `pointer` and `viewport`. `RosterEvent` gains
  `Selection`, `Pointer` and `Viewport`.
- `MPC-E310 selection_too_large`. `MPC-E114` also covers these calls.
- Grid verbs `select`, `pointer` and `viewport`, in the caret and viewing-range parity rows. A
  grid pack's parity list names them.
- The client's `PresenceChannel`, `welcome.features` and the frames `select`, `pointer`,
  `viewport` and `presence_refused` (`client/CHANGELOG.md`).
- A tier 0 doc (`BlobSession`) has none of the three calls.
- The golden files hold whole coordinates only. Parity cases and the client's tests cover
  fractions, and the kit's JSON writer carries them as `errors::Value::Float` (below).
- `MPC-E114`'s `next` and `next_step` now cover a line, a position or an element: the step is
  `cat`, in place of `wc -l`. The id, name, exit and verbs are unchanged.

## v0.11.0: keys two writers never mint alike

- **Tagged keys, new (`k2_mpc_base::mint`).** A pack that wants list keys two writers can never
  mint alike opts a list root in with `Pack::tagged_keys` (default none), with its prefixes. A
  key there is `<prefix><n>.<tag>`: the tag is the writer's client ID in base 36
  (`mint::WriterTag`), and `n` one past the highest the writer can see, live, deleted or
  retired (`mint::next_for`). There is no shared counter, so concurrent inserts share no facet.
- **The writer-tag rule:** a connection's insert into such a root must name a declared prefix
  and the connection's own tag, or the change is refused whole with the new `MPC-E820`
  `key_not_own` (exit 2), nothing applied. It is checked in the session only: stored records,
  imports, outside saves and keys the list already knows aren't checked.
- **`Session::writer_tag` and `Session::mint_key`:** the connection's tag, and the next key for
  it from every element the list knows and the retired marks. The new `MPC-E821`
  `key_space_full` (exit 2) when no key fits 24 bytes. Neither id has a step to run: no kit
  verb mints.
- **Retired marks:** `Pack::mint_marks` names the map root that holds them, one key per dropper
  (`mint::mark_key`, `retired_mark`, `marks_for_drops`), read as the highest over every
  dropper's, so two concurrent drops can't lower it.
- **Client IDs:** every ID the kit hands out (participants, the import, the disk participant,
  the write-back replicas) comes from one helper, in `2..=2^53 - 2` (`doc::MAX_KIT_CLIENT`),
  drawn again when a draw falls outside, never shifted into the range. `doc::IMPORT_CLIENT`
  (`2^53 - 1`) is reserved, and a doc refuses it from a host or a client's update
  (`DocError::BadClient`), as it refuses 0 and 1.
- The client's `mint` helpers, checked against the kit's shared vectors
  (`client/test/vectors/mint.json`, `client/CHANGELOG.md`).
- No store format change, no wire change.
- **Tests:** `tests/mint.rs` (the rule, `mint_key`, marks under concurrent drops, the ID draws,
  join churn that never puts two present connections on one ID, and four replicas minting,
  dropping and syncing in any order: no key minted twice, and they converge),
  `tests/mint_vectors.rs`; the error walk reaches `MPC-E820` and `MPC-E821`. Planted faults
  (`MPC_STRUCT_MUTANT`, feature `differential`): `any-writer-tag`, `single-lww-mark`,
  `client-id-zero`, `shift-not-resample` and `reuse-held-client`.

## v0.11.0: references to elements

- **`k2_mpc_base::refs`, new:** the state of a pack value that names a list element (a link's
  end, a parent), so such a reference is never moved or deleted silently. `Ref { root, key,
  hint }` (`Ref::new`, `with_hint`); `RefState` is `Attached`, `Suspect` (`SuspectWhy`:
  `KindChanged`, `ContainerGone` or `Cycle`) or `Orphaned`.
- `refs::resolve`, `resolve_all` (one entry per reference, orphans included, hints kept),
  `resolve_parents` (a missing parent is `ContainerGone`; each cycle of live parents is cut at
  its smallest key, whatever the list's order) and `orphaned_by` (what a change's applied
  stream detached, for notices).
- Computed from the converged doc, never stored, the same on every replica. Nothing is refused
  at write time, and the registry is unchanged.
- The client's `resolveRef`, `resolveRefs` and `resolveParents`, checked against the kit's
  shared vectors (`client/test/vectors/refs.json`, `client/CHANGELOG.md`).
- No store format change, no wire change.
- **Tests:** `tests/refs.rs` (orphans keep their links and hints and come back on a restore; a
  cycle of three is cut at its smallest key in either list order; three replicas adding,
  deleting, restoring, linking, re-parenting and syncing at random keep every rule at every
  step, with identical states once converged), `tests/refs_vectors.rs`. Planted faults:
  `orphan-drops-link` and `cycle-by-order`.

## v0.11.0: fixes (outside saves, purges, grids, claims, reads and capabilities)

- **Outside saves: a save in the store returns `Ok`.** After taking an outside save the kit works
  out whether the doc still differs from the file, and if the pack's `encode` failed there the check
  returned the error for a save that had landed. It now returns the check's usual result, the
  failure goes to `AuditEvent::DiskError`, and the doc counts as dirty, so the write-back looks
  again. Nothing landed twice before this either: a repeat check found the save already taken. It
  affects text docs as well as table docs, and only a pack whose `encode` fails. K2's packs fail
  there only on damaged codec settings, or the text pack on a doc holding a CR; K2 logs a check's
  error and now logs and audits the same failure as a disk error.
- **Grids: an outside save that fails before the store reloads the doc.** A failure after the doc
  took a save and before the store commit (the pack's `encode`, or a read) left the doc holding a
  save the store never got, and a repeat check said `Unchanged`, so it was never logged. The doc now
  reloads and the save stays in the file for the next check. Every claim ends `Reset` and the undo
  stacks clear, as for a failed store commit. The bug is ours. It affects table docs only.
- **Purges: an erasure doesn't depend on rendering the file.** A purge whose write-back and re-sync
  failed because the pack's `encode` failed returned the error with the live removal landed and the
  history not erased, and a purge with `and_remove` could never get past it while `encode` kept
  failing. It now reports an `AuditEvent::DiskError`, erases what it can reach and lists
  `NotReached::File`: the file and the kit's copy of it still hold the text until a write-back, and
  purging again then takes the freed pages too.
- **Grids: a change in the store returns `Ok`, whatever happens to the disk replica after it.** A
  grid change that re-identified elements re-synced the disk replica after its store commit, and if
  the pack's `encode` failed there the call returned the error. The change had landed, so a host
  that retried made it twice, and it wasn't on the undo stack. The bug is ours. The call now returns
  `Ok` and the change can be undone; the failure goes to `AuditEvent::DiskError`, and an outside
  save or a write-back re-syncs first. It affects table docs only, with a pack whose `encode` can
  fail: text docs don't re-sync there, and K2 has no table callers.
- **Grids: a claim ends `Edited` only when its owner's change lands.** A change that landed nothing
  (restoring a live row, deleting a cell that isn't there) or that the doc refused left the claim
  marked, so the next change by anyone ended it `Edited`, and a hand-off reservation passed on the
  same way. The bug is ours. An op with no effect beside one that lands now touches nothing either,
  as for an element claim; the claim policy still judges what a change names. It affects table docs
  only: text docs mark after the engine applied the change, and K2 has no table callers.
- **Grids: a failed read after the doc took a change reloads the doc.** A write into a deleted row
  reads who deleted it, and an undo, redo, revert or restore reads the row marks, both after the doc
  took the change. If that read failed, the call returned an error and the doc kept a change the
  store never got. The doc now reloads, as for a failed store commit: every claim ends `Reset` and
  the undo stacks clear. Only a failing store read reaches this. It affects table docs only.
- **Claims:** a host that passes a claim `note` over 1024 bytes (`CLAIM_NOTE_MAX`) now gets
  `MPC-E304 bad_claim`, and nothing is taken.
- **Grids: a refused `write --base live` marks nothing.** The kit recorded a column remap before its
  last checks. So a `Session::write_live_grid` call with columns that map non-exactly, refused at a
  limit or by a claim, or changing nothing under a pack whose `reconcile` leaves `cols_exact` false,
  marked the next change by anyone as a remap: keyed writes from an older base were refused as
  `rewritten_row`, and a person's undo of that change could conflict once someone wrote a cell after
  it. The bug is ours. The remap is now recorded after those checks and that return. It affects
  table docs only; text docs never set it, and K2 has no table callers.
- **Sessions: a change that doesn't land leaves nothing for the next one.** A call's one-shot
  settings for its change (its label, leaving the undo stack, row and column marks, the overwrite
  notes it will store) are now set right before the commit and taken first in it. Before, a commit
  that failed at its apply, or a read error in `rows drop-deleted`, could pass them to the next
  change: a wrong label, an edit off the undo stack, a false mark, or a stored note telling someone
  a write overwrote their value when it never landed. No public call was found that reaches these.
  It affects table docs only.
- **Reads: a read that fails tells nothing away.** A read verb that failed after the doc was read
  (`sed s//x/`, `diff` with old text that isn't UTF-8, an output over the limit, a pack's read verb
  returning an error, a grid's refused `LN`) still counted as the caller's read: it logged activity
  and moved their last read, so their next read didn't say who had changed the doc. No content was
  affected. It now moves nothing. In every kit release we checked: v0.10.2 and v0.10.0.2.
- **`capabilities`: positions it had wrong.** New unit `doc_coords`, with rows for `pointer --at`
  and `viewport --rect`, which take a doc's own coordinates and had no row. `who` and `activity`
  said `stdout lines`, but on a grid they report rendered lines: their forms now split by tier (text
  `lines`, grid `rendered_lines`), and `who` adds grid `doc_coords`. Read a `who` or `activity`
  form's `tier`, which was `null`. No row still means a verb takes no position; `docs/POSITIONS.md`
  lists the verbs that print positions with no row yet.
- New audit event `AuditEvent::NotesWithoutChange { key, change, version, authors }`: overwrite
  notes were planned and the commit changed nothing. Metadata only. It isn't expected to fire, and
  it changes nothing the kit does. If it does fire, report it: it's a bug in the kit.

### Grids: line numbers from other tools now name the right row

- **The bug (ours, in the kit).** A lone CR (inside a quoted value, or ending a file's records)
  ends a line for the table's line map but not for `cat -n`, `grep -n`, `view` and the other
  verbs that mirror a real tool. Where the two counts met, the numbers were mixed up: `--ids`
  could print the wrong row's key on later lines, `view`, `grep` and `--for-edit` could set a view
  or take a claim on the wrong row, `claim --lines` claimed by the wrong count, and an `LN` taken
  from `cat -n` could name another row, or none.
- **The fix.** The mirror verbs and `--lines`, `--at`, `insert` and `sed -i` stay on `\n`, so they
  print what the real tool prints. The line map doesn't change. `--ids`, the lines a read looked
  at and `claim --lines` now convert between the counts, and `--ids` names every row a printed
  line holds (`header,r1,r2`). `LN` refuses on a file holding a lone CR, with the new
  `MPC-E176 line_address_unreliable` (exit 2): use the row's key or `#N`, which `row list` shows.
  `docs/POSITIONS.md` has the two counts.
- **A refusal that now says what works.** An edit by text that ends lines in `\n` still refuses
  on a file whose records end in CRLF or a lone CR (`MPC-E122`, mixed line endings). The reason now
  names what works for that verb: `row insert` or `insert --at`, `row insert --end` or `append`,
  or `cell set`, `row insert` and `row delete`. Other refusals there keep their text.
- **Line numbers that could hit the wrong row.** On a file holding a lone CR, `row list` said a
  row was on `L5` and `delete --lines 5` removed the next one, exit 0, because `--lines` counts
  `\n`. `row list` now prints `-` in its line column there, and a conflict report gives no line
  (`GridConflict.lines` and `Overwrite.line` are `None`, a `ConflictRegion`'s `lines` `(0, 0)`).
  Read `lines` as absent on those files. Blame, claims, views, the roster and `activity` keep their
  rendered numbers, for reading: don't pass them to `--lines` on such a file.
- **When the file can't be rendered, no line number.** If the pack's encode fails, the kit can't
  tell whether the file holds a lone CR. `LN` and `claim --lines` refuse, with the new
  `MPC-E177 line_address_unknown` (exit 2, `SessionError::LinesUnknown`, next step `blame
  --structure`): use the row's key or `#N`. `row list` prints `-` if it gets that far, and a
  conflict report gives no line, which can't fail a save.
- **A known limit.** `LN` under `--base` asks about the base's content with the live codec: a file
  that was all-CR at the base and whose terminators a later save changed reads as holding none, and
  `LN` runs there. `docs/POSITIONS.md` states it, and a test pins it.
- **Who is affected.** Agents and hosts using grids through the kit, including the csv pack, on a
  file holding a lone CR. Every other file prints what it did, byte for byte
  (`tests/golden/grid_lines.golden`, blessed before the change). K2 doesn't vendor grids yet, so
  no K2 build reaches this path.

### Pack verbs declare their position units

- **The gap.** `capabilities` lists position units from the kit's table, so a pack verb had no
  row at all: `replace-section --nth K`, say, was silently left out.
- **New, additive:** `PackVerb::positions(&[...])`, `PackVerb::declared_positions()`,
  `PositionForm::new`, the unit `Unit::PackNth` (`pack_nth`, 1-based: the Nth of a pack's own
  objects), `parity::positions::{stated, check, usage_names}`, and `VerbSpec::positions`.
- **`capabilities`, for a host that passes a pack's registry:** every pack verb has a row. One
  that declares nothing shows as undeclared (`"undeclared": true`, `forms` empty; text:
  `stitch: undeclared (the pack states no position units)`) and still runs. `.positions(&[])`
  gives an empty `forms` and `takes and prints no positions`. `positions.units` gains
  `pack_nth`. A row with an empty `forms` is new: read `undeclared` before treating it as "no
  positions". No kit verb's row changes, and on a grid `col list` gains a row (`stdout`,
  `ordinal`) and `row list` two forms (`stdout`, `ordinal` and `rendered_lines`).
- **`VerbSpec` is now `#[non_exhaustive]`.** Other crates can read its fields and destructure it
  with `..`; the kit builds every one (`KIT_VERBS`, `Registry::for_pack`). No pack repo builds a
  literal, so a pack bump needs no change for this.
- **For a pack bump.** Existing packs compile and run unchanged, and their verbs show as
  undeclared until they declare. To finish: add `.positions(...)` to each verb and run
  `parity::positions::check` on the pack's registry in a test. Code that matches `Unit`
  already needs a `_` arm (`#[non_exhaustive]`).

### Grids: an insert with `--base` doesn't conflict on the table's key counter

- **The bug (ours, in the kit).** Every insert writes the table's key counter
  (`meta.next_row`, `meta.next_col`), and the per-facet base check counted that write
  as a facet of the edit. So once anyone else had inserted a row after the version an agent read,
  the agent's own `row insert … --base V` was refused with `MPC-E803 facet_conflict` (exit 1,
  nothing applied), though no cell, row or position overlapped: `1 element changed since your
  base: meta next_row value written by p in change 2`. The same for `col insert` and `next_col`.
- **The fix.** In a table, the base check leaves out those two keys of the `meta` root. They are
  bookkeeping: minting skips every key the axis knows whatever the counter says, and a merge
  takes the higher counter, so a counter written after the base can't make an edit wrong. Every
  other facet is judged as before: an insert anchored on a row moved since the base is still
  `MPC-E803`, and one anchored on a row deleted since the base is still `MPC-E805`.
- **Who is affected.** Agents editing a grid through the kit, including the csv pack, on every
  tag with grid sessions, through v0.10.2. K2 doesn't vendor grids yet, so no K2 build reaches
  this path. No refusal code, verb or golden changes.

## v0.11.0: an own replica's client ID after a reload

- **An own replica's client ID is never handed to another connection after a reload (a
  lost-edit risk).** Agent client-ID reuse (`Config::reuse_agent_clients`, v0.9.7) kept own
  replicas' IDs from reuse only in the session's memory, while the client-to-pid owner records
  persist in the store. After the doc's session unloaded and loaded again (a host's close, an
  idle unload, a restart), the same agent's next plain connection could be handed the ID its
  old replica (`Participant::own_replica`) may still write as: two writers on one client ID,
  and the engine drops one's content silently. Not reachable from K2 today (K2 creates no
  own-replica participants). Plain agents keep their reuse across reloads, as before.
- **Store: a new table, `client_kinds` (no format bump),** as v0.9.7 added `authored`: `(doc,
  client)` → what the ID was minted for (an agent that edits through the session alone, or an
  own replica), written with the owner record in its transaction, and created lazily by the
  first such join. Reuse takes only an ID recorded as an agent's. An owner record without a
  kind (one a kit before this wrote, before or between this kit's opens) is never reused: the
  agent draws one fresh ID the first time after the upgrade, reused from then on. New store
  API: `DocStore::set_client_kind`, `DocStore::reusable_clients`.
- **N−1: an older kit refuses to rewrite such a store, and leaves rows on a delete.** An older
  kit (v0.10.2 and before) opens, reads and writes a store holding the table as before (its
  joins write no kinds, so this kit never reuses their IDs). It refuses a store-file rewrite
  (and so a purge) with a typed error before changing anything (`StoreError::NewerData`,
  `MPC-E502` `store_newer`), as v0.9.5 and v0.9.6 did for `authored`; routine history
  compaction is unaffected. No data is lost, and rewrites and purges work again on upgrade.
  **A rollback means no purge until the upgrade:**
  the first agent join creates the table, so this reaches most stores in use. Its doc delete
  isn't refused and can't erase rows of a table it doesn't know, so they outlive the doc, and
  they show which of its client IDs were own replicas'. Every open by this kit removes them
  (no once-per-store marker, since a downgrade can come after any open), at the cost of one
  seek and one doc lookup per doc holding rows. Until that open, they stay in the file.
- **Tests** (`tests/client_reuse.rs`): an own replica's ID is not reused after a close and
  reopen or a store reopen (both failed on 8b9b1418); a plain agent's is reused within a load,
  after a close and reopen, and after a store reopen; an older kit's owner records (before and
  between this kit's loads) are never reused, while the fresh ID is. Planted faults
  (`MPC_STRUCT_MUTANT`, feature `differential`): `reuse-across-reload` (reuse ignores the
  recorded kinds), `no-reuse-after-reload` (no ID from before the load is
  reused) and `no-orphan-sweep` (an open leaves a deleted doc's rows).

## v0.11.0: the native engine (test-only vendoring; feature `native`)

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte (`mpce-model` keeps its own version). Hardening.
- **The error table gains `MPCE-E568`.**

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte (`mpce-model` keeps its own version). Hardening and performance.
- **The native adapter takes each refusal's code from the engine**, with no table of its own.
  A test walks every code the engine lists through the kit's report path.

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte (`mpce-model` keeps its own version). Hardening.
- **`EngineChoice::Native` now requires a store secret** (`store_secret`, the native engine's
  store secret, re-exported by the kit under feature `native`), and `EngineChoice` is `Clone`,
  not `Copy`. Two new refusal codes in the error table: `MPCE-E566` and `MPCE-E567`.

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte. Test-only additions in the engine: a checked planted-shortcuts feature and
  its plant counters. The default API is unchanged; no new refusal
  code (the error table is unchanged).

### Tests: the native engine's causal shortcuts cross-checked; planted shortcuts (test-only)

- **`verify-shortcuts` in test builds:** `crates/base` and `crates/conformance` take
  the native engine as a dev-dependency with its `verify-shortcuts` feature, so every test
  that drives the native engine cross-checks each causal shortcut against the plain walk and
  fails on a disagreement. No behaviour change. A dev-dependency's features reach test builds
  only (resolver 2): a host's build and the kit's own non-test builds are unchanged.
- **`planted-shortcuts` (test-only, never in a build):** a kit feature (and the conformance
  suite's, forwarding to it) that plants one of the engine's broken causal shortcuts in every
  native doc the adapter makes or loads, chosen at run time by `MPC_PLANTED_SHORTCUT`, to show
  the kit's tests reach the shortcuts' paths.
- **The planted head shortcut is caught:** the native adapter's refusal tests gain a case, so a
  planted run fails at its own assertion. Under the feature, each installed plant and the
  engine's count of planted answers are noted on stderr.
- **`planted-shortcuts-checked` (test-only, never in a build):** the kit's and the conformance
  suite's forward of the engine's feature: the plant stays on and its cross-check against the
  plain walk runs, so a planted run fails at the engine's cross-check, showing it fires in the
  kit's build.

## v0.11.0: production timers (from the v0.10.1 line)

The timers from the v0.10.1 line. They are off by default. Their overhead gate is in
`docs/evidence/timers-gate.md`: green for timers off, with its limits stated there.

`k2_mpc_base::timers` adds per-path call counts, total and longest time, and p50/p95/p99 on
32 paths (`docs/SESSIONS.md`, "Timers").

- **In every build:** no feature and no test-only hook. They are what the Yrs speed baseline
  reads in hook-free release builds.
- **Off by default:**
  - Off, a timed path costs one relaxed load and a branch.
  - On, two clock reads and four relaxed atomic adds.
  - `timers::calibrate` measures both.
- **API:** `set_enabled` / `enabled`, `read(path) -> (count, total_ns)` (no allocation, for
  per-call deltas), `snapshot()` (also `SessionManager::timers()`), `reset()`, `calibrate()`,
  and `HOOKS` (whether the test-only hooks are compiled in).
- **Histogram:** 16 log-linear buckets per power of two, so percentiles are within about 3%;
  about 4.2 KiB of static memory per path.
- **The paths sit at the kit's seam:**
  - `edit_as` (`Doc::edit_made`, under `edit_as` and `edit_applied`), `commit`, `store.txn_commit`;
  - `blame`, `prov.lookup`, `prov.rebuild`;
  - `write_back`, `write_back.file`;
  - `write_base` with its base text and merge;
  - `load` with its store read and `load.doc`, `join`;
  - `ingest` with its diff, merge and canonical step;
  - `tick` with its history and disk work;
  - `undo.plan`, `undo`, `revert`;
  - `notes.deliver`, `notes`;
  - `events_since` with its window;
  - the grid's `write_base`, ingest and undo planning.
- **`prof!` is unchanged:** the test-only scopes stay the developer's profile.
- **Tests:**
  - unit tests for the histogram (buckets, exact counts, threads, and a proptest for
    percentiles within a bucket);
  - `tests/timers.rs`: every path counts once per call that reaches it, nothing while off,
    and a path without a row fails;
  - `tools/timers-check`: the timers in a host's build, with no features.

## v0.10.2 (2026-11-02)

The first host release after v0.10.0.3: the v0.10.0.3 disk-replica fix (kept, not
rolled back), the client as a library only, live presence as data, per-connection carets and
the caret re-send. v0.10.1 was not released to hosts: what the docs mark "kit v0.10.1"
(`client/WIRE.md`, `client/PRESENCE.md`) first ships here. **The timers from the unreleased
v0.10.1 line are not in this release; they ship later after their overhead gate.** The crate
version is `0.10.2`; the client's is `0.12.0`. Pairs with the paired markdown and text pack
tags of this release.

**This tag's tree doesn't include the vendored native engine** (its crates and licence texts,
the `native` feature and the conformance crate,
which needs it): the release commit is cut by `tools/public-cut.sh`, and
`tools/check-public-cut.sh` blocks the tag if any of it is left, if the licence files are
missing, or if a pinned regression test of the previous host tag is gone. Main keeps all of it.
The cut drops the conformance crate whole, so this tag runs none of the engine-generic
conformance suite (its Yrs columns included); main runs all of it.

For a host, in short (details in the sections below):

- **The client is a library; the host draws the editor.** The live editor component is gone
  from `k2-mpc-client` (`client/src/editor/*`, `client/src/codemirror.ts`, the
  `k2-mpc-client/editor` entry, the CodeMirror peer dependencies, the browser tests). What
  stays is the library: `LiveText`, the wire and transport contract, Re-apply, presence and
  marks. **`client/HOST.md`** (new) is the handover for a host that draws its own editor: the
  calls an editor makes and the rules it must keep (no editor-side undo history: the server
  owns undo; one transport object across reconnects; `connect()` while open replaces the
  connection; frames are dropped, never buffered, while closed; one edit in flight).
- **Presence and marks as data** (no DOM): `participants()` / `onParticipants(cb)` (one entry
  per remote connection and per other connection of the same person; `colour` is the roster's
  identity value for the host's own palette; offsets in UTF-16 of the current text) and
  `marks()` / `onMarks(cb)` (each rejected change with its range, reason and actions), both
  batched to one callback per frame; the scheduler is a `LiveOptions.schedule` option (a host
  without frames can deliver at once). "Insert my text here" is library logic now:
  `insertHerePreview(id)` and `insertHere(id)`. The person's own typing and caret moves don't
  fire `onParticipants`.
- **Per-connection carets:** the same person on two devices or tabs sees each other
  connection's caret. The kit adds `RosterEntry::devices` (one `Device { client, cursor, label }`
  per connection on text docs) and `client` on `RosterEvent::Cursor`; an older consumer that
  ignores `client` still ends each flush on the person's caret. A host can label a connection
  at join: `Session::join_with(participant, stay, &JoinOptions)` with `.label(..)`,
  `.shared(..)` and `.replacing(prev)` (a replacing join's label is the one given at that
  join); `join`, `join_shared` and `join_replacing` are unchanged. The client marks each caret
  `isSelf` / `sameUser` / `client` / `label` and never returns the connection's own. Rust code
  that builds `RosterEvent::Cursor` or matches it without `..` needs one more field.
- **The caret re-send** (client 0.11.1, first released here): after remote changes the client
  re-sends its caret mapped the way an editor maps its selection, once per version, throttled
  (250 ms, with a trailing send), never while its own edit is in flight.
- **New client exports:** `setDefaultLiveTimers` (the default clock of a `LiveText`, for tests;
  not the timers above); `k2-mpc-client/test` (the test helpers a
  host's editor tests use, each also at its old path) and `k2-mpc-client/test/jsdom-shims`.
  `client/test/exports.test.ts` pins the exported names and the files a host build reads by
  path, the wire goldens (`client/test/golden/*.json`) among them.
- **`MPC-E000 not_applied`** is in the error registry (`explain MPC-E000` works): the client's
  own id for a change whose reply was lost and that the record doesn't list.
- **The native engine** stays on main only (feature `native`, test-only); it is not in this
  tag's tree (see above). Yrs is the engine for hosts. The engine's error codes (`MPCE-E…`)
  stay listed in `docs/ERRORS.md`: they name refusals, not engine source.
- **Licence: MIT OR Apache-2.0** (as in kit tag v0.10.0.2), checked by `tools/oss-check.sh`.

## v0.10.2: the disk replica after a restart (also in v0.10.0.3)

- **The merge base is right after a restart (a lost-edit risk).** An outside save
  merged while the doc held changes not yet written back leaves the disk replica (the base the
  next outside save is merged onto) holding the file's text, recorded at the merged change's
  version. A reopen took that equal version as "the replica is the doc's state", so the first
  write-back after it kept the old replica: the file got the doc and the session reported
  `Clean`, while the merge base still held the previous file's text. The next outside save was
  then merged onto a stale base, and could undo what the write-back wrote or lose part of the
  save. Affected: a file-backed doc (`SessionManager::open_file`, text or grid) that took an
  outside save into unsaved changes, then was reopened from the store before its next
  write-back (the process stopped inside the write delay, or the unload's write-back failed);
  since v0.9.7, when the write-back began to keep the replica in step instead of reloading it.
  Fixed where the reopen decides: the stored replica counts as the doc's state only if it was
  synced at the doc's version **and** the doc renders the synced bytes; otherwise the first
  write-back reloads it from the doc, as before v0.9.7.
- **New audit event `AuditEvent::DiskReplicaRepaired { key, doc_len, replica_len }`**
  (non-breaking: `AuditEvent` is `#[non_exhaustive]`). The first sync after a reopen (a
  write-back, or an outside save the doc then holds) now compares the replica's texts with the
  doc's, O(doc) once per load, and when they differ reports it and reloads the replica before
  the doc is declared in sync. Byte lengths only, never content. It should never fire: if it
  does, it is a bug in the kit (report it). `DiskReplicaMismatch` is unchanged.
- **Tests** (`tests/disk.rs`): `an_outside_save_then_a_restart_keeps_the_replica_the_docs_text`
  pins the property's minimal failing case (it failed on v0.10.0.1 and v0.10.0.2);
  `a_stale_replica_after_a_restart_is_reported_and_repaired` plants the old reopen (test-only:
  `differential::with_reopen_step_by_version`) and sees the event fire once with the sync check
  clean after the repair, and with the fix sees no event.

## v0.10.2: the conformance suite's engine half v2 rules

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte.
  Hardening, and a test-only feature. Native-only; in no K2 build. No new refusal code (the error
  table is unchanged).

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte.
  Hardening. Native-only; in no K2 build. No new refusal code (the error table is unchanged).

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte.
  Hardening. No new refusal code and no public API change.

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte.
  Hardening. No new refusal code (the error table is unchanged), no dependency or licence change.

### The native engine re-vendored (test-only vendoring; feature `native`)

- **Re-vendored** (`vendor/README.md`): the native engine at a newer version, by git subtree,
  byte-for-byte; only its core changed. Hardening, with two new refusal codes. No
  dependency or licence change.
- **Error table** (append-only): the engine's table lists its first two codes, `MPCE-E199` and
  `MPCE-E980`; the native adapter names them. `docs/ERRORS.md` regenerated.

### Hardening: a refused apply leaves no trace

- **`core.refused_no_trace`** (new conformance rule; gated on the native adapter; needs
  Authorship and Canonical): after every refused delivery or client update, the replica's
  text, version, encoded state vector and encoded state are equal to before, byte for byte,
  and it takes valid batches after. Passes on the reference model and on the kit Doc on the
  native engine; pinned on Yrs. `docs/CONFORMANCE.md` gains a "Hardening rules" table.
- **The native adapter** (`engine/mpce.rs`, feature `native`; in no host build): hardening,
  with tests.

### The client: a library only (hosts own their editor UI)

- **The live editor component is removed from the client** (`client/src/editor/*`,
  `client/src/codemirror.ts`, the `k2-mpc-client/editor` export, the CodeMirror peer
  dependencies, the Playwright browser tests). Hosts draw their own editor over the library:
  **`client/HOST.md`** (new) has the calls an editor makes, the rules it keeps and the tests that
  cover them. `client/EDITOR.md` is a pointer to it. The library is unchanged: `LiveText`, the
  wire and transport contract, Re-apply, presence and marks as data.
- **New client exports:** `setDefaultLiveTimers` (root and `core`); `k2-mpc-client/test`, the
  test helpers a host's editor tests run on (each also at its old path), and
  `k2-mpc-client/test/jsdom-shims`. `client/test/exports.test.ts` pins the names and the files a
  host build reads by path (the wire goldens among them).
- The editor's library coverage runs on a DOM-free test host (`client/test/live-host.ts`):
  `live-convergence.test.ts` and the caret re-send scenarios. Details: `client/CHANGELOG.md`.

### `MPC-E000 not_applied` in the error registry

- **`MPC-E000` is registered** (`not_applied`, exit 1, `(sync)`, reached indirectly): the JS
  client's own id for a change whose reply was lost and that a `since` reply doesn't list
  (`client/src/wire.ts` `ERR.NOT_APPLIED`, `client/WIRE.md`). `explain MPC-E000` explains it;
  `docs/ERRORS.md`, the explain and capabilities goldens and the registry snapshot are
  regenerated. The id was never allocated (append only holds). The tests that used it as an
  unknown id use `MPC-E999` now.

### Licence: MIT OR Apache-2.0

- **Licence: MIT OR Apache-2.0**, as the engine (also in kit tag v0.10.0.2): `LICENSE-MIT` and
  `LICENSE-APACHE` at the root, `license` in every crate of ours (the workspace's three by
  `license.workspace`; `fuzz`, the D0 spike and the N−1 consumers in their own manifests) and in
  the client's `package.json` and its lockfile's root entry (and the spike's). README and
  NOTICE.md say so; `vendor/` keeps its own licences. No code change.
- **The licence is checked:** `tools/oss-check.sh` fails unless `LICENSE-MIT` and
  `LICENSE-APACHE` are at the root (and are those licences), NOTICE.md is there, every crate of
  ours says `license = "MIT OR Apache-2.0"` (directly or by the workspace) and `publish = false`,
  every `package.json` of ours (and its lockfile's root entry) says the same licence, and a
  package that lists a licence file in `files` has a byte-identical copy beside it. `vendor/`,
  `node_modules/` and test corpus folders are skipped. `tools/test-oss-check.sh` plants each
  failure (16 cases: a missing file, a wrong or missing field, a client copy that differs, and
  the skipped folders). Both run in `scripts/check.sh` and the `evidence` part.
- **The client package carries the licence files:** `client/LICENSE-MIT` and
  `client/LICENSE-APACHE`, byte-identical to the root's (oss-check compares them), are in
  `package.json`'s `files`, so `npm pack` includes them (npm packs only what is under the
  package's folder).

### Positions bound to a version, with one release of warnings (R-1, R-2)

- **R-1, grids:** a grid edit's `#N` and `LN` count in its `--base` version (was: the live
  table when the verb applied). A positional edit whose base the kit no longer holds is
  refused, exit 8, `MPC-E112` (new `SessionError::BaseExpired`; `MPC-E112` now also lists
  `@pack`). An element named by position at the base and deleted since: exit 4.
- **R-2, text and pack verbs:** an agent's edit by line or range, a `sed -i` line address, or
  a pack verb's declared positional address (new `PackVerb::positional`, for Markdown's
  `--nth`) with no `--if-version` is checked by the new `ToolOptions::positional`
  (`PositionalMode`). **This release: `Warn`** (`tools::POSITIONAL_DEFAULT`): it applies as
  before and the new `ToolOutput::warnings` (`tools::Warnings`) carries one stderr `note:` line
  (`tools::positional_note`). `Refuse`: `MPC-E171 positional_needs_version`, exit 2, nothing
  applied (now live). **The next release flips the default to `Refuse`.** People's edits are
  unchanged.

### One address grammar and its error ids

- **`addr`** (new module): the kit's one address grammar, `addr::parse(s, Forms)`: a scoped
  native ID `@<kind>:<id>[/…]`, a minted key `@k:<key>` (or `key:<key>`), `#N`, `LN`, a forced
  `name:<text>`, else a bare word (a key, else a unique name; **never a position**). `Forms`
  says which forms a context has. `addr::unique` refuses two or more matches; `addr::ambiguous`
  builds the refusal; `Candidate` is one match's most durable address and position.
  `docs/SESSIONS.md`, "Addresses".
- **New error ids** (append only; `registry.snapshot` grows by four lines): `MPC-E170
  ambiguous_address` (exit 2, live), `MPC-E171 positional_needs_version` (exit 2, reserved
  for positions bound to a version; live with R-1/R-2 below), `MPC-E172 address_not_found`
  (exit 4, reserved for `node`), `MPC-E173 stale_cursor` (exit 1, reserved for paged reads'
  cursors). Exit 2 here is usage, not held: a host prints no permission line for them.
- **`ToolError::Address`** (new variant; `ToolError` is `#[non_exhaustive]`): an address
  refused with its own report and details, exit 2, its message the verb's own line.
- **The grid verbs** (`table::verbs`) parse their addresses through the grammar: rows and
  columns also take `@k:<key>` (rows also `key:<key>`). An ambiguous column name or `--where`
  still exits 2 with the same line, now as `MPC-E170` (was `MPC-E122 tool_usage`) with
  `details.candidates` (each match's `@k:` address and `#N`). A bare word starting with `@`
  is still a column name on a grid (no native IDs there).

### Structured next steps: `next_step` beside `next`, additive

- **New wire field, additive**: an error id gets a `next_step`, `{verb, args, why}`
  (`errors::NextStep`; `ErrorEntry::step: Option<NextStep>`, `ErrorReport::next_step()`).
  `verb` is always a kit verb by its registry name (`cat`, `undo --partial`, `row list`,
  `explain`); `args` is an argv list with no host prefix and no doc path (`{key}` stands for
  the refusal's `details.key`, which the host fills in); `why` is plain prose that names no
  command line. An id with nothing to run (wait, ask a person, report it, use ordinary file
  tools) has no `next_step`: the key is left out. 59 of the registry's 123 entries have one
  (`errors/registry.rs`). Unit tests (`errors/tests.rs`) hold every entry to this: the step
  is absent, or its verb is in the kit's verb registries with a reason; no host command
  text appears in a message, `next`, `why` or argument; placeholders are plain detail keys.
- **`next` is unchanged**: its prose is byte for byte as before, and no existing key changes
  name, type or order. `ErrorReport::to_json` / `to_value` / `to_json_for` add `next_step`
  right after `next` when the id has one: `{id, exit, message, next, next_step, rule,
  details}`. `with_next(surface, …)` still overrides only the prose; `next_step` is the
  registry's on every surface. `explain` prints a `step:` line after `next:`, `explain
  --json` a `next_step`, and `docs/ERRORS.md` a **Next step** line, for each id that has
  one; `capabilities` (`error_ids.json_shape`) lists `next_step`. The explain goldens,
  `capabilities.json`, `docs/ERRORS.md` and the wire golden `client/test/golden/errors.json`
  were re-blessed; the registry snapshot (ids, names, exits) and `capabilities.txt` are
  unchanged.
- **Client**: `ErrorReport.next_step?: NextStep` (`client/src/wire.ts`; `NextStep` exported,
  `verb: string`), optional: absent for an id with nothing to run, and from a host built on
  an older kit; `client/WIRE.md` updated.
- **For a host**: K2 may render `next_step` in its own syntax (its command name, the doc path)
  instead of `next`. `next` converts or retires only in a later release, and only after K2
  renders `next_step`.

### The position encoding, stated

- **`capabilities` states how positions are counted**: a new top-level `positions` key in
  `--json` (and a "Positions" section in the text form): `internal` and `wire` `utf8_bytes`
  (the kit's API and the hunk channel), `line_col` (1-based lines; 1-based columns in Unicode
  scalar values), `yjs_wire` `utf16_code_units` (inside the update bytes exchanged with Yjs
  clients only; engine-internal units are not stated, as they differ between engines), every
  unit with its base, and per verb served each flag that takes or prints a position with its
  unit (`insert --at`: `line_col`; `head -c`: `file_bytes`, the file's bytes with BOM and
  CRLF; `wc -m`: `file_chars`; grid `--row`: `grid_row`...). Nothing else in `capabilities`
  changes.
- **One table**: `parity::positions` (`VERB_POSITIONS`, `Unit`), public: the kit's verbs and the
  grid verbs (`table::verbs::VERBS`), whose rows appear with a grid pack's registry.
- **`docs/POSITIONS.md`**: surface, unit, base and where each is set, with file:line references.
- **Tests** (`tests/positions.rs`, in base-a2): every verb whose usage names a position has a
  row and every row's flags are in its usage; on `é中🎉` text and a BOM + CRLF file through
  `run_tool`, `insert --at` and `--range` count an emoji as one column, `head -c`/`tail -c`/
  `wc -c` count the file's bytes and `wc -m` its characters; byte offsets and `line:col`
  round-trip; a Yrs client's insert at a UTF-16 index lands at the byte the CLI's `line:col`
  names, and the kit's byte offset is the client's UTF-16 index. No behaviour changes.

### The ID table and the ID-churn judge

- **`ids`** (new module, `crates/base/src/ids/mod.rs`): `IdKind { kind, example, class, scope,
  accepted_by, note }` and `IdClass` (durable, file-local, unique-name, positional, session,
  stamp; most durable first). `ids::KIT` lists every ID and address kind the kit's verbs take
  or print: text lines and `L:C`, exact `--old` text, grid row and column keys, `#N`, `LN`,
  header names, `--where`, versions, changes, participants and claim ids. `ids::markdown`
  renders a table, `ids::check` holds one to a verb registry.
- **`docs/IDS.md`** (new), generated by `ids::kit_markdown()`; `tests/ids.rs`
  (`table_matches_docs`) holds the committed file to it and checks every verb a row names
  (`MPC_BLESS=1` regenerates it).
- **`Pack::ids()`** (new, default none: additive, no pack changes): a pack's own native ID
  kinds, in the same form, for its `docs/IDS.md`.
- **`ids::churn::judge`** (feature `test-support`): the ID-churn harness for packs' tests. Over
  a corpus, a no-op save (decode, encode, read back) must give the file's bytes back and keep
  every ID, and each one-unit edit (a verb's edits written back, or an outside save read
  back) must keep the ID of every object it doesn't touch. Run on the text pack and the grid
  test pack (zero churn); planted renumberings (a read-back that re-keys a row, a save that
  renumbers a line's ID) fail it. **Not yet:** refusing such a write at run time: it needs a
  pack hook and a new error id, so it waits for the first pack whose file carries native IDs.

### The native engine re-vendored (hardening; the engine's licence)

- **Re-vendored** (test-only, `vendor/README.md`): the native engine at a newer version, by git
  subtree, byte-for-byte: hardening in its core, no public API change and no new refusal, so nothing
  in the conformance crate changes. No dependency changed; the lockfile is unchanged.
- **Licence**: the engine is now MIT OR Apache-2.0 (each manifest's `license` field). The
  licence texts, at the engine repo's root, are copied byte-for-byte beside the vendored
  crates (`LICENSE-MIT`, `LICENSE-APACHE`), outside the crate directories.
  `scripts/check-licences.sh` still skips every native-engine package by name (as "our own"), so
  it doesn't check this field yet.

### The native engine's sync layer: the re-base epoch moves onto it

- **Re-vendored** (test-only, `vendor/README.md`): the native engine at a newer version, by git
  subtree, byte-for-byte. The engine's refusals gain a new code; the suite's two matches on
  them (in `sync.rs`, on the send log's results) name only the false-receipt refusal and keep a
  catch-all, as the engine's refusal type is non-exhaustive. Their manifests
  are standalone now, so the kit's stand-in `vendor/Cargo.toml` is gone. No new dependency
  (`count-alloc` is only their dev-dependency and is not vendored); the lockfile is unchanged
  but for the sync layer's version.
- **`sec.rebase_required` on the native engine's epoch**: `sync::SyncRebase` now takes
  `recvOp`'s decision from the engine's sync layer (resend, equivocation, hold, inline; the
  fence `u.ep < x.ep` inside it), and the client's epoch and the epoch a send carries from it
  too (raised on a re-base, kept through reconnects). The host keeps what the sync layer leaves
  to it: the re-issue over mpce-model's text and its ID map, the held queue (always empty), the
  decision's inputs (the update against the log, its dependency) and the typed frame.
- **An overstated base**: a new step: an update whose base names a send never made is refused as an
  implicit false receipt (the engine's false-receipt refusal). A base
  naming a send that was made but whose content the update lacks is the engine's admission's (it
  refuses an update that lacks a send's content; the native engine), which this layer doesn't
  call: it waits for the kit on the native engine.
- **Negatives**: the no-fence negative is now the native engine's own planted
  no-epoch-fence mutant (the host's `NoEpochFence` is gone); `EpochPerConnection` is a
  host that makes each connection a new participant; `NoEpochBump` a host that appends a
  re-base without telling the native engine's tracker; new `TrustClaimedBase` (the base taken
  as claimed, not through the send log). `IntegrateInline`, `NoDepMap`, `ForgetOnAck` and
  `ReadmitBelowS` as before.

### The native engine behind a feature (a step of the engine migration, in progress)

- **Feature `native`, off by default and not for hosts until the switch to the native
  engine.** It adds the native engine (vendored) behind the kit's
  engine seam. Until a later step, **sessions refuse to store a doc on it**
  (`Doc::check_storable`, at import and promotion: it would be stamped as Yrs's). Without the
  feature nothing changes, and the kit's normal dependency tree has no native-engine crate:
  `tools/tests/wire-guard.sh` checks it, in the repo and as a host builds it.
- **Seam prep, the same on Yrs:** `Engine::yrs()` is an `Option`; provenance counts in the
  doc's own units (`Prov` carries `Doc::units()`; the build-wide `UNITS` is gone); the edit
  path reports each text hunk's first inserted clock as applied (`Made::firsts`) instead of
  predicting it after the fact, and undo's planner, the one caller that needs IDs before
  applying, uses the doc's `IdOrder` (a test checks both against blame).
- **Hardening** in the native engine's adapter (client updates' rights and which ops a client
  may send). Native-only; in no host build.
- **Errors:** a native refusal is `DocError::Engine { code, message, kit }`: the kit's
  id with `details.engine_code` where the kit has the meaning, else `MPC-E161
  engine_unlisted` with the code (no registry rows yet).
- **The native conformance column** (`NativeText`, empty deviation list): the core rules,
  `core.authorship` with no pin, and `core.sec`. `core.steady_plain` awaits a later engine
  milestone; the suite measures and prints it every run (`native::core_steady_plain_measured`)
  and gates on it once the column claims Steady. Never a pin.

### `sec.rebase_required`: the re-base epoch as the engine's model has it

K2 Engine's answer: the engine's client-protocol model is right, and the rule
now follows it (on `sync::SyncRebase`; the engine's sync layer has no epochs yet, so the epoch
is the test host's until it carries it):

- **Every re-base raises the client's epoch by one** (`dRebase`, `ep: x.ep + 1`): the
  pipelined update is re-based in epoch 2, not again in epoch 1.
- **The fence** (`recvOp`, `u.ep < x.ep`), three new cases: (a) an old-epoch copy of an update
  already in the log is ignored, never equivocation; (b) an old-epoch update not in the log is
  held and re-based into a newer epoch; (c) the epoch survives a reconnect: a client whose
  frames were lost with its connection resends after the reconnect, and its re-based updates
  are ignored while its offline one is re-based, in epoch 3.
  `Rebase` gains `disconnect`, `reconnect` (the handshake: resends, then a catch-up) and
  `insert_offline`. A known update with other content is equivocation (the model's
  `sameContent`).
- **Negatives**: `NoEpochFence` is now the model's `client-no-epoch-fence` (no fence: the
  resend of a re-based update is reported as equivocation); new `EpochPerConnection` (the
  model's `cp_lost_frame`); the old `NoEpochFence` (references to re-based events left
  unmapped) is `NoDepMap`.

### The client protocol's rules on the native engine's sync layer (test-only)

- **Vendored** (test-only, `vendor/README.md`): the native engine, by git subtree, byte-for-byte.
  Only `k2-mpc-conformance` depends on it; the kit's build doesn't see it
  (`cargo tree -p k2-mpc-base -e normal`: no native-engine crate). New in the lockfile: `hmac`
  0.12.1, `sha2` 0.10.9 and their tree, all on the licence allow-list. `vendor/Cargo.toml` (ours) carries the engine
  root's workspace fields the three crates inherit.
- **`sec.false_receipt`** runs on the native engine's send log (`sync::SyncReceipts`): the refusal
  is its false-receipt refusal, checked to record nothing, and a true report is checked to resolve
  to the position the send carried.
- **`sec.rebase_required`** runs on the native engine's tracker (`sync::SyncRebase`): S, the joins,
  the drop from S (a grace that ran out), the acks and the re-admission after them are the
  tracker's, and an update integrates inline only when its base (an implicit report from the
  send log) is at or past S read before the step. The epoch and its fence, the
  re-issue and the retry map are the host's in that version of the sync layer, so a test host over
  mpce-model's text does them. The rule gains a third lagging client: one client's re-base
  must not take S back and let the next one's update in inline.
- **The matrix**: a layer column, the native engine's sync layer, that runs the Protocol
  rules only ("–" for the others; a layer needs no Text). On the kit (Yrs) the rules still
  await the native engine; the reason names what the layer doesn't have.
- **Negatives**: the re-base mutants (`IntegrateInline`, `NoEpochBump`, `NoEpochFence`,
  `ForgetOnAck`; with the epoch fix below, also `EpochPerConnection` and `NoDepMap`) are in the
  sync layer's host; the sync layer's own planted re-admission below S (its
  `planted` feature, a dev-dependency of the suite's tests) fails the rule; the receipt
  mutants stay on `seams::RefReceipts` (the sync layer's receipt check has no planted variant).
  `seams::RefRebase` is gone.

### Eight new conformance rules (engine half v2)

`docs/CONFORMANCE.md` lists each with its gate and its delivery target. Each has a planted
negative that must fail it.

- **`core.authorship`**: the agents whose new events a batch carries,
  reported before anything changes; a batch on agent A's connection may carry new events by A
  only. Runs on both engines; on the kit it is the kit's own client check, through the
  `conformance` feature's hooks `replica::check_from` / `replica::deliver_from`, which call
  the same code as `Doc::check_client_update` / `apply_client_update` (one body, shared;
  `untrusted-wire` stays off in the suite). Pinned on Yrs.
  Mutants: `TipOnly`, `KnownAsNew`.
- **`core.sec`**: byte-identical canonical state whatever the delivery
  order: 1,680 interleavings of a concurrent script and 100 causal orders of a seeded run.
  Passes on both engines (the kit's `encode_state` included). Mutant: `ArrivalOrder`.
- **`core.steady_plain`**: a loaded fragmented doc against
  1.2 × text + 24 B × index runs + 64 KiB, with the counting allocator. Pinned on Yrs; planted pair:
  plain text and index pass (198 KB), 16 B per character fails (478 KB).
- **`prov.record`**: one record per change, its author the agent that made it, on whose
  behalf, host time, and time never in a merge; on a kit session. Pinned on
  the kit: its record has no person-or-agent kind (a later interface's typed core will).
  Mutants: `BehalfAsAuthor`, `RecordPerHunk`.
- **`perm.predicate_called`**, **`perm.refused_whole`**:
  awaiting the kit's write predicate and the native engine's; they run on a test
  seam, `seams::GatedYrs`. Six mutants.
- **`sec.false_receipt`**, **`sec.rebase_required`**: not
  applicable on Yrs (no stability frontier); awaiting the kit on the native engine's client
  protocol; they run on the native engine's sync layer (below). Nine mutants.
- The kit Doc's deviation list is fifteen entries (`core.authorship`, `core.steady_plain`,
  `prov.record` added).
- `tools/tests/wire-guard.sh` checks the hook can't leak: a host build without features has no
  client API and no hook; with `conformance` alone it still has no client API, and the
  feature builds without the wire cfg. A kit unit test,
  `conformance_hook_is_the_client_check`, holds the hook's answers and resulting state equal
  to the client API's on every outcome.

## v0.10.2: the seam prep for the native engine

### N−1 both ways (tools/n1)

`tools/n1/n1.sh layout-both <prev tag> <dir>` and `node-both <dir>` check N−1 in both
directions between two builds that both read format 6:

- Each build's `n1_producer` writes stores for the other build's consumer
  (`tools/n1/consumer-both`). With `K2MB_N1_RESTORE`, the producer also reverts its revert, so
  the stores hold provenance rows and a record with roots.
- The consumer opens both stores, blames the restored line, reverts the other build's record,
  then writes, restores and rewrites.
- For a previous release without the env flag (v0.10.0), the layout adds the restore to its
  copy of the producer.

The existing `layout` and `node` modes (a previous release that refuses format 6) are
unchanged.

### Format 7 is read, never written: the downgrade guard for the native switch

The native switch will write store format 7. This build already handles a format-7 store: it
reads it, but never writes the format and never changes what it can't read.

- **Read vs write formats.** `STORE_FORMAT` (6) is the highest format this build writes.
  `STORE_FORMAT_READ` (7) is the highest it reads. Format 8 and later are refused as before
  (`MPC-E502`); the message now reads "this build reads up to 7".
- **Opening a format-7 store writes nothing.** There is no upgrade, no table creation and no
  authored-hunks sweep, so the file's bytes are unchanged. A rewrite that a newer build left
  pending is not finished here: the open is refused with the new
  `StoreError::RewriteNewer { found }` (`MPC-E502`, exit 6), and the file is left as it is.
- **No rewrite or purge of a format-7 store.** `rewrite_file` and `check_rewritable` refuse it
  with `RewriteNewer` before anything changes. A purge checks `check_rewritable` first, so it
  refuses the same way.
- **Another engine's doc is refused on delete and purge.** `SessionManager::delete_doc` and
  `delete_space` refuse a doc whose stamp this build doesn't read with `MPC-E520`, as on open.
  A space holding such a doc is refused whole, before any session closes.
  - The store's own `delete_doc`, `delete_space` and purge check again inside their
    transaction, with the new `StoreError::ForeignEngine` (`MPC-E520`).
  - Tier-0 docs (stamp `none`) and this engine's own docs pass (`engine::owns_stamp`).
  - Otherwise a delete would erase only the rows this build knows and leave the native
    engine's rows behind.
- **The DocMeta v1 layout is frozen.** A golden record carrying a native stamp pins it, with a
  strict decode, so this build can always read the stamp of a doc it can't load.
- **Tests:**
  - `tests/engine_stamp.rs` makes a format-7 store by hand. Its Yrs doc is served (read,
    edited, blamed). Its native doc is refused with `MPC-E520` on open, `delete_doc` and
    `delete_space`. A whole-store rewrite and a purge are refused with `MPC-E502`. The store's
    bytes are unchanged after an open.
  - A pending rewrite on a format-7 store fails the open and leaves the bytes as they were.
    Format 8 is refused.
  - Existing tests that pinned "format 7 is refused" now use format 8.

### Engine units: u64 clocks and `units_of` (no stored byte changes)

Element IDs keep their `(client, clock)` shape, and **the clock is now a u64 in the engine's
units**. `engine::Units` and `units_of` / `char_units` come from the engine, not from UTF-16.
On Yrs the units are UTF-16 code units, as before. A native engine counts scalars. The
widening goes through the engine seam (`BlameRun.clock`, `clock_of`, `insert_clocks`),
`ElemId`, `IdRun`, `ClockSpan`, `ProvRow`, the revert record's `RootRuns`, and blame. Every
UTF-16 count on a clock path now goes through `units_of`. `CharBlame.clock` and
`root_clock` (feature `conformance`) and `differential::EditRecord.clock_before` are u64.

**Stored bytes are unchanged; text stores stay at format 5.**
- The change-clocks and provenance tables already stored u64; their reads no longer narrow
  to u32.
- A revert record's runs keep their u32 layout (records v2 and v3). The units and clock are
  narrowed at encode, checked. A value over `u32::MAX` refuses the change with
  `StoreError::ClockOverflow` (`MPC-E501` `store_failed`, exit 6) before anything is
  written; it is never stored truncated.
- Yrs clocks are u32, so a Yrs doc never reaches that refusal. The u64 record layout comes
  with the native records, at the switch to the native engine.
- `StoreError` gains `ClockOverflow(u64)`; it is `#[non_exhaustive]`.
- **Reads keep v0.10.0's refusal.** A change-clocks or provenance row with a clock past the
  doc's engine's (`EngineDoc::max_clock`; Yrs: `u32::MAX`) can only be damage. Blame,
  revert, undo and the outside-save note read those rows and refuse it as
  `StoreError::Corrupt` (`"change clocks"`, `"provenance row"`), as v0.10.0's narrowing
  reads did. No kit clock is ever passed into Yrs: the engine is driven by text positions.

**Tests:** `store::history` pins the stored bytes of a revert record with runs, a provenance
row and a change-clocks value: they are the bytes v0.10.0's own encoders write. It also
checks that a run over u32 is refused at encode, that a commit holding one writes nothing,
and that clock spans and provenance past u32 round-trip under a u64 bound and are refused
under Yrs's. `session::live::blame` plants such rows in a Yrs doc's store: blame and revert
refuse them as `Corrupt`.

### The seam's `new` and `text` return `Result`

`EngineDoc::new` and `EngineDoc::text` are fallible, so an engine can refuse a schema it can't
hold. A text root the doc doesn't hold is `DocError::Malformed`. `Doc::new` and `Doc::text` returned
`Result` already; the public API is unchanged. `Doc` checks the root's kind first, so no caller of
`Doc` reaches the new error.

### `Doc` runs on an engine enum

`Doc` holds `engine::Engine`, an enum with one variant per engine this build carries. Its only
variant is `Yrs`. The seam's calls, and the checks and comparisons the kit runs on the engine,
dispatch through it. A second engine is a second variant, and each `match` must then say what
it does on that engine. The Yrs-only hooks of test, `differential` and `conformance` builds go
through `Engine::yrs()`. No behaviour changes.

### The engine stamp is checked on load

**Fixed:** a stored doc's engine stamp (`DocMeta::engine` / `engine_format`) was written and
never checked. A session now refuses a doc another engine, or another stored-state format,
wrote, before reading any of it: `SessionError::EngineMismatch { stored, stored_format,
engine, engine_format }`, **`MPC-E520 engine_mismatch`** (exit 6), "this doc was written by
engine loro-1.0 (format 1); this kit runs engine yrs-0.28 (format 2)". Checked on every load
from the store (`open`, `open_file`, the reopen after a promotion) and on the reload after a
failed store write. The same engine is the same family (`yrs`, the name up to its first `-`)
and format; the library version after the `-` doesn't count. An unstamped doc (an empty
name; no store the kit wrote has one) loads as this engine's. Tier 0 reads no engine state:
no stamp applies. No store format change; no new dependency (docs/SESSIONS.md, "The engine
stamp").

**Tests:** `tests/engine_stamp.rs` (another engine through `open` and `open_file`, another
format, another library version, an unstamped doc); `engine::tests` (the rule); the error
goldens and `docs/ERRORS.md` gain `MPC-E520`.

## v0.10.2: the live editor's items

- **A removed-text note tells a reader without history who can help, every time.** On a text
  doc, a note for a reader without the history right (`version: null`, `restore_from` set) now
  says "Someone with history access can restore it from version N." right after the place
  (after "The text is no longer held here." when withheld), once: a cut note keeps its last
  sentence ("N more characters not shown; someone with history access can restore it from
  version N.") and adds nothing. Readers with history and roll-ups: unchanged. Only `message`
  changes on the wire.
- **`Session::set_cursor_at(conn, root, version, anchor, head)`**: a caret given in byte
  offsets of the root's text at `version` (a live client's `cursor` frame) is mapped through
  the changes since on that root and set, under one lock. Returns `CursorAtVersion::Set` or
  `Dropped(CursorDrop::Ahead | PastWindow)` (nothing set; the host sends no reply). A host
  passes the frame through instead of mapping it itself (`client/WIRE.md`, "Carets").
- **`Session::join_replacing(participant, stay, prev)`**: a reconnect's join that also closes
  the pid's connection holding client ID `prev`, as `leave` does, under the same lock (the pid
  stays present: no participant `Left`). `Joined` gains `replaced: Option<ConnId>` (`None` from
  `join` and `join_shared`). A `prev` held by no present connection, or by another pid's, is
  ignored silently. The host no longer calls `leave` for `prev` itself (`client/WIRE.md`,
  "Reconnecting").
- **`ChangeEvent.kind`** (`ChangeKind`): every change event says what kind of change it is
  (`edit`, `undo`, `redo`, `revert`, `restore`, `external`, `import`), live and from
  `events_since` (read from the change records, so also after a restart); the App lens passes
  it through. New `ChangeKind::as_str` gives the wire's name. Wire: `ChangeEvent` gains
  `kind` (`client/WIRE.md`); the client's `ChangeEvent` type gains an optional `kind`. Golden
  `change_events.json` and `change_events_app.json` re-blessed (each event gains `kind`).
- **The client reads `kind`** (client `0.12.0`; `client/CHANGELOG.md`): with a v0.10.2
  host an undo still in flight on a dropped connection no longer forces "check and retype".
  A lost change is recognised only by an `edit`-kind change from its client ID; an `undo` or
  `redo` from that ID is not it, so Re-apply places the change when nothing else came. Any
  other kind under a client's own ID concludes nothing. With an older kit (no `kind`) the
  client behaves as before.
- **Correction to the v0.10.0.1 notes:** "undo and redo marked in a change's `via`" is
  `kind`, as above; **`via` is unchanged**.

**Fixed (grid merges)**

- Grid: the keyed `table::merge3` reports a `cell` conflict again when the live side switched a
  cell between absent and `""` byte-neutrally and ours set it (the per-cell neutral rule applies
  to `write --base` through a text only); mutant (xxxviii).
- Grid: the keyed `table::merge3_outside` reports the file's override of a byte-neutral live
  switch between absent and `""` (or of a live empty value the file emptied the other way) as an
  overridden `cell` (the neutral rule applies to an outside save through a text only; the
  session's outside save, `merge3_outside_text`, is unchanged); mutant (xxxix).

**Per-connection carets (the kit).** The same person on two devices or in two tabs now sees
their other connection's caret, as Google Docs shows one login's other tabs. All additive: a
host or client that ignores the new fields behaves exactly as before.

- **`RosterEntry::devices`** (text docs): one `Device { client, cursor, label }` per
  connection of the participant, in join order, each with its own caret. Roster rows stay one
  per pid; `RosterEntry::cursor` is still the most active connection's caret. Empty on a grid
  (grid carets, `RosterEntry::cell` / `RosterEvent::CellCursor`, stay per participant: out of
  scope) and for tier 0.
- **`RosterEvent::Cursor::client: Option<ClientId>`:** the connection whose caret moved. The
  roster flush keeps what it told per (pid, connection), so each connection's caret change is
  its own event (a connection that leaves with a caret: its `None`). **For a consumer that
  ignores `client`** (one caret a person, the last event's): within a flush a participant's
  events end with the one whose caret is `RosterEntry::cursor`, and when the last
  per-connection event isn't that caret (a connection left or dropped its caret while another
  still has one) one more `Cursor` follows with `client: None` and the participant's caret.
  After every flush such a consumer holds exactly what it held before; a participant with one
  connection gets exactly the events it got before, each with its `client`. A Rust consumer
  that matches `RosterEvent::Cursor { who, cursor, version }` without `..` needs `..` (or
  `client`); constructing the variant needs the field.
- **Join labels:** `Session::join_with(p, stay, &JoinOptions)` (new; `join` and `join_shared`
  are it with the defaults, their signatures unchanged). `JoinOptions::label("another tab")`
  is kept on the connection and carried on its `Device::label`; control characters are
  dropped, it is trimmed and cut at `DEVICE_LABEL_MAX` (64) characters, and an empty one is
  none: a label never fails a join. It is the host's text, never doc text. A shared join with
  a label relabels its connection.
- join_with's JoinOptions gains .replacing(prev); a replacing join's label is the one given at
  that join
- **Wire goldens** (`client/test/golden/`): every text-doc member gains `devices` (a
  `{client, cursor, label?}` a connection, client IDs fixed as before) and every `cursor`
  event its `client`; nothing else changed. The file scenarios' client IDs are now fixed too
  (the main scenario's numbers per pid), since their members carry them.
  `roster_events_devices.json` (new): one person on three connections, two labelled.
- **Tests:** `tests/roster.rs`: two connections of one pid each get their own device caret; a
  move on connection 2 is `Cursor { client: Some(c2) }` alone; a consumer ignoring `client`
  keeps the person's caret over 120 random moves, joins, leaves and edits; one connection is
  unchanged; the label round-trips. `--features planted-device-bugs` (test-only: every
  connection shows the most active one's caret, "first connection only") makes them fail.

## client 0.11.1: the caret re-send (ships in v0.10.2; not released to hosts before)

v0.10.0.1 plus the JS client 0.11.1: remote carets are re-sent after remote changes, mapped to
the caret's place at the new version and throttled (at most one per 250 ms, a trailing one at
the last version), so a caret frame a host dropped as behind no longer leaves an idle person
unseen (`client/CHANGELOG.md`, 0.11.1). Client only: no change to the kit's Rust library,
store, merge or wire.

## v0.10.0.1 (2026-10-31)

v0.10.0 plus the JS client 0.11.0: the live editor's M2 (below). Client only: no change to the
kit's Rust library, store, merge or wire, and no new dependency. The crate version is
`0.10.0+1` (the git tag is v0.10.0.1); the client's is `0.11.0`. `client/WIRE.md` now says the
kit's versioned caret call (`set_cursor_at`) is planned for kit v0.10.1 (it said v0.9.9; there
is no v0.9.9, its list folds into v0.10.1). Until then a host maps or drops a caret frame that
is behind, as before; the client's frames don't change when it lands.

**For a host wiring the live editor in** (`mountLiveEditor`, `k2-mpc-client/editor`; `EDITOR.md`):

- **Peers:** `@codemirror/view` ≥ 6.27 and the host's single `@codemirror/state`; leave
  CodeMirror's `history()` out (mounting with it throws `MPC-EDITOR-HISTORY`: the server owns
  undo).
- **Transport** (`WIRE.md`, "The live transport"): the token and auth stay the host's;
  `connect({resume, prev})` is one attempt, and a `connect()` while a connection is open
  replaces it. Before the welcome, or any `since` answer, on the new connection the host marks
  the connection named by `prev` dead and calls `leave(conn)`; frames still arriving on the old
  socket are dropped unapplied.
- **Whether to retry a close is the close table's**, never a transport's own `retry`.
- **Deferred to kit v0.10.1, each with a safe fallback today:**
  - a versioned caret call (`set_cursor_at`): a `cursor` frame that is behind is mapped forward
    by the host (`events_since` + `position::transform`) or dropped; the client re-sends its
    caret after its next change, so a dropped one only shows late;
  - undo and redo marked in a change's `via`: until then an undo still unanswered on a dropped
    connection makes Re-apply answer `unknown` ("check and retype") instead of guessing;
  - the kit's own join that replaces a connection: until then the host calls `leave(conn)` for
    `prev`, as above;
  - the wording of a removed-text note for a reader without history (`version: null`,
    `restore_from` set): the fields are final, only the message text changes.

### The live editor, M2 (client/EDITOR.md; design: docs/design/live-editor-m2.md)

- **Browser tests run Chromium only by default:** `MPC_BROWSERS` (comma-separated: `chromium`,
  `webkit`) names others, and an unknown name fails the run. A bare `playwright test` no longer
  selects WebKit, which needs system libraries most runners lack. `tools/README.md` gives the
  full-checkout gate and the WebKit client-only job as two separate commands.
- **The replica's text is a rope** (M2): `LiveText` keeps its texts in a persistent chunked
  rope (`src/rope.ts`) instead of rebuilding a whole string on every edit. A keystroke, a remote
  change and a `since` replay no longer scan the document. Measured on a build node, one core,
  p95 per acknowledged keystroke: 0.37 ms at 1 MB (was 18.1 ms) and 0.77 ms at 4 MB (was
  73.2 ms); a 500-event `since` at 1 MB: 73 ms (was 6.9 s). The API is unchanged: `text` is still
  a string, built on its first read after a change and cached.
- **Re-apply by the server's rule** (M2): a rejected change goes back where the server would put
  it: the change as sent, rebased from its base through every change since (`rebaseOurs`), then
  through the person's own pending changes, sent fresh at the current version as its own undo
  step. Deletions, replacements, changes with several hunks and ties all go back in place;
  `already-present` when nothing is left to apply. Refused: `conflict` (it met someone else's
  change) and `cant-place` (a reload since, or past the 512-change history). This replaces
  v0.9.8.2's single-hunk guards; `LiveText.placeRejection` and `Placement` are new, and
  `reapplyGuard` is deprecated (below).
  The Re-apply scan (seeds 1–500) gives back every change the guards refused that the rule
  places, with 0 differences from the rule.
- **A faster `since` replay:** `recarry`'s self-check is a length check on the plain incoming
  path (nothing pending), and the rope's edit copies its unchanged chunk list instead of
  rebuilding it. A 500-event `since` at 1 MB, p50 of 20 runs, interleaved in one job on a busy
  build node: ASCII 78–98 ms before, 53–63 ms after; CJK/emoji 41–47 ms before, 17–21 ms after.
- **The rope has two levels** (M2, step d): chunks in blocks of about 32, each block with its
  own starts. An edit rebuilds the chunks it touches in at most three blocks and copies the list
  of blocks (about 30 at 1 MB), not the list of chunks. A 500-event `since` at 1 MB, p50 of 20
  runs interleaved in one job against the one-level rope: ASCII 50 ms → 23–33 ms; CJK/emoji
  13 ms → 16 ms (both well under the 100 ms target). The property test gains runs with tiny
  blocks (edits across block edges) and a planted fault in the block sums it must catch.
- **No re-index during an IME composition:** while remote changes are held, the binding keeps
  the held document (the editor's plus the held changes) and its byte index up to date change by
  change; a keystroke no longer builds a byte index of the whole document. A test checks the
  held document and index against `live.text` after every change of a composition; the bench
  prints the composition keystroke at 1 MB and 4 MB.
- **The editor's byte index keeps line lengths in blocks**: a keystroke copies the touched block
  and the list of blocks, not every line's length (O(lines / 256)).
- Tests: a differential property test of both against plain strings (random edits, CJK, emoji
  and surrogate pairs at chunk edges, empty and 1 MB documents, every offset conversion), with
  a planted fault (a chunk's cached UTF-8 length off by one) it must catch. The bench
  (`test/bench/live-text-bench.ts`, `M2_BENCH_GATE=1`) fails when a target is missed: p95 per
  keystroke under 1 ms at 1 MB and 2 ms at 4 MB, a 500-event `since` under 100 ms at 1 MB.
- **"Insert my text here"** (M2): a single-hunk action the person chooses, beside Re-apply. It
  first shows the typed text in the editor where it would go (announced in the note,
  `role="status"`; for a replacement or deletion the note adds that the removal isn't redone),
  and inserts it only on **Insert** (its own undo step); **Cancel** or Escape inserts nothing.
  Presence is checked first for a lost change. Handle: `insertHere(id)`,
  `confirmInsert()`, `cancelInsert()`, `InsertResult`; class `cm-mpc-insert-preview`.
- **Copy** on every rejection with text (`copyRejected(id)`): to the clipboard, or the text
  selected when the clipboard refuses. The row's text is selectable.
- New strings: `insertHere`, `insertConfirm`, `insertCancel`, `insertPreview`, `insertRemovedText`,
  `insertMultiHunk`, `copy`, `copied`, `copyFailed`, `conflictReapply`, `cantPlace`.

**Deprecated**
- `LiveText.reapplyGuard(id)` and the type `ReapplyGuard` (from v0.9.8.2): a stub over
  `placeRejection` for this release (`ok`, `conflict` or `cant-place`; the old reasons are never
  returned). Use `LiveText.placeRejection(id)` and `Placement`; `reapplyGuard` goes in the
  following release. `ReapplyResult`'s old reasons and their strings stay in the types, unused,
  and go with it.

## v0.10.0 (2026-10-30)

Structured kinds and the grid: the grid line (grids and the CSV seam) on top of v0.9.7.1.
Pairs with k2-mpc-csv 0.1.0, k2-mpc-markdown 0.7.8 and k2-mpc-text 0.2.9.

**Includes v0.9.7.1's fixes** (its section below): `write --base` merges by whole lines
(`merge::write_base_sides`; two changes to one line are a conflict, nothing is merged inside
a line: the splice of lines that start alike, possible text corruption since v0.1.0, is
gone), with the actionable `MPC-E111` conflict (`refusal_at`, the current version and each
region's lines); edits given as a new text pair whole lines first (`lines::edit_hunks`), so
blame no longer credits a line's deleter with a neighbour that starts alike. On a grid,
`write --base` is the merge by key (`table::merge3`), cell by cell, as before: a cell is
never merged inside its value.

**Also includes v0.9.8, v0.9.7.2, v0.9.8.1 and v0.9.8.2** (their sections below), merged in. The
`Overwritten` notes are one set of types for both kinds of doc (`Overwrite`, `OverwriteCell`,
`NoteKind::Overwritten`, `RosterEvent::Overwritten`, `Snapshot.overwritten`,
`Session::take_notes`): a text doc's (v0.9.7.2: in memory, `removed_text` / `replaced_text`)
and a grid's (this release: stored, the grid rules) each keep their own rules, wording,
delivery and storage; `Overwrite` carries both field sets (a grid's note has
`restore_from: None`, `not_shown: 0`, `no_history: false`). A text doc never touches the
store's `overwrite_notes` table and never needs format 6. `differential::MergeMutant` holds
both series: v0.9.8's (i) `CharSides`, (ii) `EndFollowsTheirs` and the grid's (xxxii)–(xxxvii);
a plain build carries neither (the text merge's planted checks are now compiled out there too,
no behaviour change).

**A row is merged whole through a text (the grid's row-splice fix).** `table::reconcile` maps a
file's rows onto keys, and a wrong pairing could merge a row someone rewrote with another row's live
cells: a row nobody wrote. A text can't say otherwise ("A' moved here" and "B edited in place" can
be the same bytes), so safety no longer rests on pairing:
- **`write --base`:** a row the text didn't leave exact and the live side changed too is one
  `edited_row` conflict, nothing of it applied, `--partial` included; the message names the row by
  key and line and the way out (re-read, or `cell set --base`).
- **An outside save** (and a stale `write_live_grid`, a revert, a version restore): the text's row
  wins whole; every live cell that differs gets the Overwritten note, and a version holds it. A live
  switch between empty and absent in one cell isn't a live edit of that cell when the base row, with
  only that cell switched, renders the same bytes. A blank row given a `""` cell (blank to `,`)
  changes bytes and is an edit. Before, such a switch was judged on the whole row's bytes, so
  another live edit in the row made it count: a spurious note and audit event for the cell.
- **The cost:** two people's different cells in one row don't auto-merge through a text any more.
  Grid verbs that name a cell keep the per-cell merge. It can be relaxed later without a store
  format change.
- **"Exact" compares decoded field values exactly** (a trailing space counts, a blank line isn't
  `""`), under an exact column mapping (own position, or unique header name). How a field is quoted
  is no change: a tool requoting a whole file overwrites nothing and bounces nothing.
- **Row marks** (`row_marks`, in format 6): a keyed write whose base predates a text's non-exact
  pairing of a row it names is refused (`edited_row`), even on a cell the text didn't change; a
  non-exact column mapping marks the whole doc. An undo, revert or restore that changes a marked row
  marks it again. `EditOptions::seen`: a person's edit is guarded the same way if the host passes
  the version their view last rendered; **no client sends it yet, and without it an edit lands at
  current.**
- **Undo (spec S13, grid rows only; text undo is untouched):** undoing a change on a re-paired row
  with later edits conflicts (`rewritten_row`); `--partial` skips it and reports the skip; a revert
  rewrites the row whole.
- **Pairing decides identity and blame only.** A wrong pair costs blame on that key: stated, not a
  safety cost. Exact duplicates can't be told apart. Native merges would bypass the server's checks
  and don't exist today (the untrusted-wire path doesn't reach sessions).

**Host notes**
- **Overwritten notes reach a connected editor:** `RosterEvent::Overwritten`, to its author's own
  subscriptions, and `RosterEvent::Snapshot::overwritten` (`client/WIRE.md`). They persist across a
  restart in the change's own transaction, one per row per author, values capped (256 bytes, 32
  cells),
  at most 64 per author with the overflow rolled up, expiring with the revert window. A purge strips
  their values; `Debug` doesn't print them. Someone who can't read the doc gets nothing, not even a
  count, until they can. `Session::take_notes` and a verb's notes still deliver them; a push doesn't
  consume one, so dedupe on `(change, row)`. `rows drop-deleted` can't be undone, so it notes what
  it removes: `dropped_row` (a deleted row's cells) and `dropped_col` (a deleted column's cells in
  rows that stay), to whoever wrote them; the note carries the value and names no version. A
  delete's note carries the live values it hid.
- **`--partial` applies nothing of a text when a column layout change on either side conflicts with
  it:** any conflict at a column (`edited_col`: the text deleted a column the live side wrote into;
  `deleted_col`: the text moved a column the live side deleted; `moved_col`: both moved it), or a
  `deleted_col` at a cell or a name (the text wrote into, or renamed, a column the live side
  deleted). The layout every row of the text assumes changed, so `write --base --partial` returns
  the same conflicts as plain `write --base`. Before, it applied the text's rows beside a held
  column's old cells: a row nobody wrote. A text that never touches a column the live side deleted
  applies as before, every field by key. A text's column insert on its own never conflicts. Beside a
  live column delete, though, the text's fields can still meet the deleted column (`deleted_col` at
  its cells). Before, `--partial` then applied the insert, so a new empty column took the deleted
  one's place. Now it applies nothing. An `edited_col` conflict now names the writers of the cells
  it met.
- A grid's overwrite note to a reader without history now clears `version` and `restore_from`
  and sets `no_history`, on every delivery path. A text note is unchanged (it clears `version`
  only); whether a text reader without history should keep `restore_from` is under review for
  v0.9.9.
- **A purge on a grid now also withholds the pushes not yet sent** (as a text doc's): the
  values go, and a withheld note names no version and no `restore_from`.
- Test-only: a wire golden for a grid's `overwritten` notes (`roster_events_overwritten_grid.json`,
  a `cell` and a `dropped_row` note, live and waiting in a snapshot), and
  `client/test/golden-types.ts`, which holds every golden file to `wire.ts`'s types at compile
  time (`tsc --noEmit`; `golden.test.ts` reads them untyped and checks the values). A field typed
  wrong in `wire.ts` (`cells[].col: null`, say, where a grid sends a number) now fails the
  typecheck, naming the file. It covers v0.9.8.2's goldens too; a hand check of every field in
  that release found no type drift there.
- **A raw keyed cell write under a row or column the table doesn't know** (dropped, or never in
  it) was taken as a hidden write: an orphan cell nobody could see, restore or sweep, reported
  with a `row restore` / `col restore` that refuses a dropped key. `edit_ops` now refuses the
  whole edit (`SessionError::UnknownElem`, `MPC-E805 no_element`, exit 4, as the grid verbs'),
  naming the drop's change and who ran it while its record is in the revert window, else saying
  the key isn't in the table; nothing applies. A deleted key the table still knows stays a hidden
  write with its message, and a key the same edit inserts takes the write. Map deletes are
  not checked (they store nothing). Since grids landed; grids have not shipped.
- **A reversal over a re-paired row the doc has dropped:** a revert that rewrites such a row
  whole (S13) returned an engine refusal (`UnknownKey`) instead of an outcome, and an undo or redo
  called it `rewritten_row`. Every reversal now reports it `dropped` (S11 before S13); under
  `--partial` it is skipped as `dropped`. Since S13 (659a7e9); grids have not shipped.
- **`write --base` over a byte-neutral live switch:** a live switch of a cell between empty and
  absent that renders the same bytes (an undo, a revert or a restore of a `""`, say) is no live
  edit of the cell, so a text writing that cell applies with no conflict. Before, it was a cell
  conflict.
- **`HiddenCellWrite::message`:** the kit now supplies the sentence for a cell written into a
  deleted row or column, so every host and agent shows the same
  text: `r2: c1 set by p2 in a row deleted by p1; row restore --row r2 brings it back` (a column:
  `… in a column deleted by bot; col restore --col c2 brings it back`; both: `… in a row deleted by
  p1 and a column deleted by bot; row restore --row r2 and col restore --col c2 bring it back`).
  Keys and pids only: never a value or a column name (both are content). An App viewer's lens
  names its handles in it. The data fields are unchanged.
- **Header-less columns at the same width pair by position:** `reconcile` pairs columns by
  content only when the width changed, to find the column that went or came. The text's width is
  its widest row; the base's is its live columns (the version read, or the file as last synced).
  With a header, columns pair by unique name as before. Content compares a column's values
  sorted, so a match at the same width could be coincidence. Before, a header-less text whose
  column happened to hold another column's values was read as a column move: a row that renders
  as the live row conflicted (`edited_row`), and an outside save moved a column and sent notes.
  The cost: a real header-less column move reads as value rewrites, so a row the live side edited
  since is changed by both (noted in an outside save); nothing is lost.
- **An outside save's whole row covers a column the live side deleted:** the file's row wins over
  the hidden cell too (noted), so restoring the column shows the file's row, never the live
  side's hidden value beside it. A live value between empty and absent that the file's row
  renders anyway is no override (no note).
- **No false `DiskReplicaMismatch` on a dirty grid outside save:** while the doc is dirty the
  replica holds the file's state as its own ops, so its state vector can't match the doc's; that
  cycle now reloads the replica from the doc instead of comparing. Before, every outside save to
  a dirty grid doc raised the event (which hosts are told to report as a kit bug). Text never
  raised it.
- **A text row that renders as the live row is the same row:** where the text's row and the live
  row render the same decoded fields (an empty field and an absent cell alike), there's no conflict
  on any of its cells and the cells are the text's. Before, `write --base` refused it (the live side
  had undone a value to absent, the text had `""`), and an outside save noted an override.
- **A version restore notes a hidden value it takes:** a restore's overrides now include cells
  under a deleted row, so the value's author gets the note with the value. Before, a restore that
  brought a deleted row back over a hidden value noted no one.
- **Rows both sides changed, when the live side deleted a column it wrote into:** the both-changed
  test now reads the text's kept columns that the live side knows, a deleted one included. Before,
  it read the live side's live columns and missed the write. `write --base` now gives `edited_row`
  there (`--partial` skips the row). An outside save gives the file's row whole, and the live value
  gets a note. Before, the file brought the column back with the live side's hidden value beside its
  own row: a row nobody wrote. The outside save's whole-row loop also reads the file's columns
  live in the result.
- **A reversal never writes under a dropped key:** an undo, redo or revert of a batch that touched a
  dropped row or column conflicts (`dropped`) for its cells too; `--partial` skips them (spec S11).
  `AuditEvent::OutsideOverride` records each, with no content. No `ChangeEvent` field carries it.
- **Store format 6, lazy and one-way** (`store::STORE_FORMAT` 6, `STORE_FORMAT_BASE` 5): a
  store is stamped 6, and gains the `elem_writes` table, only in the transaction that writes
  its first structured doc (a grid). A store that never holds one stays at 5 byte for byte,
  so v0.9.7 still reads and writes it. A format-5 build refuses a format-6 store with one
  line (`StoreError::Newer`, "store is format 6; this build reads up to 5"): roll the host
  back only with stores that never opened a grid.

**Breaking**
- **`session::Reconciled` gains `exact_rows` and `cols_exact`**, and `table::Reconciled` too. A
  table pack with its own `reconcile` must pass them on from `table::reconcile`: the defaults
  (empty, false) are the strict reading, every row merged whole and every keyed write from an older
  base refused, so a pack can't turn the row rule off by leaving them out.
- **`Session::write_live_grid` takes `seen`**, the version the body was made from. Stale, it takes
  the outside-save path, with notes; unknown, `GridWrite::BaseExpired`.
- **`RosterEvent::Snapshot` gains `overwritten`** (a `match` naming every field needs `..`).
- **`FacetConflict` gains `rewritten_row`.**
- **`store::VersionReason` gains `BeforeDrop`** (stored as 8): `rows drop-deleted` cuts that version
  first; its cell map holds the dropped rows' and columns' cells (restoring an element from it
  arrives in v0.10.1). An exhaustive `match` needs the new arm.
- **`PersonAction` gains the grid tier's ten rows** (`RowInsert` … `DropDeleted`), on
  purpose: the enum is exhaustive, so every pack's `match` stops compiling until it maps
  them. A text or Markdown pack maps them N/A (`ParityRow::not_applicable`, verbs absent:
  `row insert`, …, `rows drop-deleted`).
- **`At` is no longer `Copy`** (it gained `At::Elem { root, key }`, a grid cell by key). Fix:
  clone the caret where it is used twice, `set_cursor(c, root, at.clone(), at)`.
- `Tier` gained `Tier::Grid` and `ClaimEnd` / `ReservationEnd` gained `Deleted` (all
  `#[non_exhaustive]`: additive for matches with a wildcard).
- **`StructEdit` takes a `&VerbCtx`** instead of `(&View, &[u8])`: `ctx.view()`,
  `ctx.stdin()`, plus the pack's block decoder (`ctx.decode_block`) and line map
  (`ctx.line_map`). Fix: `|view, stdin| …` becomes `|ctx| { let (view, stdin) =
  (ctx.view(), ctx.stdin()); … }`.

**Added (grid presence, parity, verbs and blame)**
- Presence by key on grids: `Session::set_cell_cursor`, `RosterEntry.cell`
  (`CellCursor`), `ClaimSpan` and `Session::claim_span`, grid views, notes and
  `--for-edit`; claims expand at their ends and end `Deleted`; hand-off reservations of
  grid claims; the claim policy on structured edits and on `write_base_grid`
  (`GridWriteOptions::policy`); `GridConflict.claimed_by`.
- The parity grid tier: `Tier::Grid`, `parity::check_grid`, `GridCase`,
  `kit_grid_cases`, `observe_grid_case`; kit grid verbs `rows drop-deleted`, `blame --cell`,
  `blame --structure`.
- Structured pack verbs: `PackVerb::structured_edit` / `structured_read`,
  `StructEdit`, `StructOutput`.
- Element blame: line blame on tables, `Session::blame_structure`.

**Added (the grid verbs)**
- `table::verbs::VERBS`: the grid verbs for any table pack, as structured pack verbs: `cell
  get`, `cell set`, `cells set`, `cells clear`, `row insert|move|delete|restore|list`, `col
  insert|move|delete|restore|rename|list`, `sort`, `header on|off`. A pack lists them from
  its `verbs()`. Addressing: keys, `#N`, `LN`, column names, `key:` / `name:`,
  `--where C=V`; exits: 0, 1 (`--base`), 2 (usage, ambiguous), 4 (unknown or deleted,
  with the restore hint). `row_listing` / `col_listing` are the listings a tab shows.
- `Pack::decode_block(bytes, codec)`: a block of records in the doc's dialect (`cells set`'s
  stdin); the kit never parses a file format itself. Default: declined (`cells set` exits 2).
- `VerbCtx` (see Breaking), `StructOutput::shown` / `StructOutput::showing`: a read verb's
  rows or cells by key; `--for-edit [--ttl]` on `row list` / `cell get` claims them under
  the read's lock (`ToolOutput::for_edit`), and the read gets the notes about those rows.
- `--ids` on `cat`, `head`, `tail`, `grep` and `view` on a table: each line prefixed with its
  row's key and a tab (`header` for the header, `-` for grep's `--`), from the pack's line
  map under the read's lock. On a doc with no rows, or with output that isn't doc lines
  (`head -c`, `grep -c`/`-l`/`-q`), exit 2.
- Text edit verbs on a table (`sed -i`, `patch`, `append`, `replace`, …) run on its rendering;
  the result is reconciled by key against the live view (`Pack::reconcile`) and applies as
  one change, with `--if-version`, the claim policy and notes. A result that isn't a valid
  table is exit 2, nothing applied.
- Who and when on the grid verbs: `cell get --json` adds `version` (the doc's, a `--base`
  for the next write), `author` and `change` (the cell's last write, as `blame --cell`; null
  without the history right); a deleted row's or column's refusal (exit 4) names who deleted
  it and in which change (`row r9 is deleted (by ana, v12); row restore --row r9`), with the
  history right. `VerbCtx::version`, `VerbCtx::last_write`.
- `Pack::set_header(t, on, codec)`: the `header on|off` verb asks the pack, so a format with
  its own rule for the switch holds the kit's op to it (default: the kit's op);
  `VerbCtx::set_header`.
- `Session::write_live_grid` (`write --base live` on a grid): the body replaces the grid,
  reconciled by key against the live table (rows matched keep their keys; a missing row is
  deleted, restorable), with the limits and claim policy of every grid write.

**Changed (what the CSV binding found)**
- **The aligner keeps a row's key by position in its unchanged slot** (`table::reconcile`'s
  last pairing step): a run of new rows between the same two kept rows (or table ends) as an
  equal run of old rows pairs in order, whatever they hold. Was: a blank line edited, or any
  cell edited in a one-column table (no field shared), became a delete and an insert. A row
  moved and fully rewritten is still a new row (its slot changed).
- **The header line is written only when the table has a live row or a name on a live
  column** (`Table::header_renders`, the codec's rule; `render`, `line_map`). Was: a blank
  header line for a header-on table with live columns and nothing else. A lone blank line
  is no header: `Table::import` reads it header off, and `table::reconcile` switches a
  header-on table to off for a file that is one blank line (names kept, unrendered).
- **`Table::set_header` never changes the rendering** (the codec's rule, and so the default
  `Pack::set_header`'s): `on → off` where no header line is written only changes the setting
  (was: a row for the names, a line more); `off → on` where the only live row is blank is
  refused, `TableError::HeaderOnlyRowBlank` (exit 2 at the `header` verb, nothing applied).
  `Table::reidentify_onto` keeps its header where the switch would be refused.
- **`write --base live` and outside saves:** a row replaced in its own slot keeps its key
  (above); `tests/grid_verbs.rs`'s `write --base live` case now adds its new row elsewhere.

**Added (from the CSV binding)**
- `View::codec()` / `View::with_codec`: the doc's codec settings on the views the session
  hands an edit closure (`edit_ops_with`) or a read verb; `VerbCtx::codec()`.
- `LineOwner::Preamble`: a line a codec writes of its own in front of the records (CSV's
  `sep=;`), for a pack's `line_map` (was: given to `Header`). `LN` on it is exit 4; `--ids`
  prints `preamble`; `blame` gives it no write.
- `Config::grid_max_value_len` (default 64 KiB, `table::DEFAULT_MAX_VALUE_LEN`; `MAX_VALUE_LEN`
  is kept as the default's earlier name): structured changes writing a longer cell value or
  name are `AtLimit { what: "value bytes" }` (exit 2; message "a value or name is N bytes;
  the limit is M bytes"); imports and outside saves holding one are declined; the outside
  save's aligner reads the file under the knob (raised above 64 KiB, longer values are
  accepted). `Table::max_value` / `with_max_value`, `Table::import_with_max_value`,
  `View::with_max_value`, `table::check_value_max`.
- Feature `test-support`: `session::test_support::import_edits(content)`, the edits a
  session's import applies, for packs' tests.
- **A re-save that changes only the codec settings is an attributed outside change** (the
  CSV binding's rule (b)): a version (reason `External`, by `EXTERNAL_PID`) holding the new
  settings (no change record: the doc's log doesn't move); `LogItem::Settings { version,
  description }`, `Session::settings_change(conn, vid)` (`show`'s note),
  `Pack::describe_codec_change(old, new)` (default "codec settings changed"),
  `AuditEvent::OutsideSettings`, and `Session::subscribe_settings` delivering
  `SettingsChanged` (a distinct event, not a `ChangeEvent`: the re-save has no change record,
  so it has no new version). No store format change.

**Breaking (from the CSV binding)**
- `LineOwner` gained `Preamble` and is now `#[non_exhaustive]`: a `match` needs a wildcard
  arm. `Table` gained a private field (its value limit): build one with `Table::new`,
  `Table::import` or `Table::from_keyed`, not a struct literal.

**Changed (the grid line's merge with main)**
- `header on` with no row, over names the columns kept while the header was off, is refused:
  `TableError::HeaderNamesUnrendered`, `MPC-E806 header_names_unrendered` (exit 2, nothing
  applied; "the columns have names that would appear as a header line; clear them or add a
  row first"). Was: the switch wrote them as a header line, changing the file (the property
  `the_header_switch_never_changes_the_rendering` found it at 10^4; its seed is kept). The
  kit never clears the names itself: that would rewrite someone else's work.
- `header on` where the only live row is blank keeps its refusal (exit 2, nothing applied)
  and gets its own id, `MPC-E807 header_only_row_blank` (was `MPC-E122 tool_usage`), with a
  message that says to add a non-blank row first, so a host words it apart from `MPC-E806`.
- Grid outside saves: after a person's save, a file that returns to an earlier write of the
  kit's is taken as their edit, as for text (v0.9.5's echo rule; was: the kit's echo,
  ignored). `grid_store.rs`: `after_a_persons_save_an_earlier_kit_write_is_their_edit`.

**Changed (grids)**
- **A grid's outside save is a keyed three-way merge**, as v0.9.5 makes it for text: base =
  the grid as the file held it (the disk replica), ours = the file aligned on it, theirs =
  the live grid, dirty or not; merged by key (`table::merge3_outside`) and applied to the live
  doc as the external participant; the replica then takes the file's grid. Where both sides
  changed one element the file wins (the later write) and the live value is never lost: a
  version cut before the save, a heads-up note to its author (`NoteKind::Overwritten`,
  `Note::overwrite`: `Overwrite`; delivered with the author's next notes or
  `Session::take_notes`) and `AuditEvent::OutsideOverride`. Never refused, never dropped.
  The file's placed rows land right after their left neighbour in the file; at a gap where
  both sides inserted rows, the file's rows come first (text outside saves put live first;
  both deterministic).
- The disk replica's re-sync marker is the change counter (`synced_seq` vs `version`), as in
  v0.9.5, and a grid change that re-identifies elements re-syncs it at once when the doc
  renders the synced bytes; rows the live doc re-identified with edits pending take the live
  keys before the merge: rows and columns (and a header switch) the live doc gave new keys
  while edits were pending are paired with the base's by what they hold (the same values,
  else at least half of them, else the one new element at their place; `Table::
  reidentify_onto`), and the file's changes to an element the live doc dropped since come
  back under a fresh key. One marker for text and grids: the grid line's merge with main
  kept one copy of `resync_if_synced` and `resync_replica` (main's), shared by both ingests.

**The grid line's merge with main (v0.9.5, the K work)**
- **Error ids for the grid (on main's registry):** `MPC-E801 needs_base`, `E802 at_limit`
  and `E803 facet_conflict` go live (reserved on main); new `MPC-E153 bad_root_name`,
  `E154 malformed_doc`, `E804 op_refused` (`DocError::RootName`, `Malformed`, `Op`) and
  `E805 no_element` (a grid verb's exit 4: an unknown or deleted row, column or cell; with
  `details.verb` and `details.line`). A structured verb's exit 1 is `E127 pack_verb_no`, as
  for any pack verb. `ToolOutput::refusal` is set for both. `FacetConflict` details: `count`,
  `where`, `conflicts` (root, key, facet, by, change, now); `AtLimit`: `what`, `limit`,
  `would_be`; an undo conflict region on a grid adds `key` and `why`. `capabilities` lists
  the grid tier on the verbs that serve it and the grid knobs (`grid_max_cells`,
  `grid_max_cols`, `grid_max_value_len`, and `agent_version_elems`).
- **Roster push on a grid:** `RosterEvent::CellCursor { who, cell, version }`, coalesced
  as `Cursor` is (one per participant per flush, the last place; told again when its position
  changes under it; `None` when it is dropped). A grid claim's `ClaimStarted` is pushed (its
  `ClaimEnded` already was). `RosterEvent` is `#[non_exhaustive]`: additive.
- **Handles on a grid:** an App viewer sees handles in heads-up notes (the ones an outside
  save left waiting too, `take_notes`), grid claims, `ChangeEvent.hidden[].deleted_by`,
  `LogItem::Settings`, `FacetConflict.by`, `blame_cell` / `blame_elem` /
  `blame_structure`, and `write_base_grid` / `write_live_grid` conflicts, and in everything
  `run_tool` hands it: the grid verbs' printed attributions (`cell get --json`'s author, a
  deleted element's "by" line: the verb's history is lensed under its lock), the fence's
  contributors, a `--for-edit` claim or conflict, hidden writes' deleters and refusals
  naming someone (`tests/app_view.rs`, `an_app_viewers_grid_verbs_hold_no_pid`).
- **The live window on a structured doc:** `events_since` / `edit_at_base` with a base
  behind the doc are `MPC-E112 base_expired` (read again): a grid's changes are ops by key,
  which the window can't replay (it returned an empty list before).
- **Outside saves on a grid follow v0.9.5's echo rule:** an ingest clears the kit's recent
  writes, so one coming back after a person's save is their save, not an echo.
- **Undo grouping:** a structured edit never joins a person's open undo group (a step of
  its own).
- **Exports reach grids; the export row is live for csv:** the `export` and `export
  --version` verbs serve the grid tier too (a grid's version renders with the codec it
  stored), and a grid pack's parity list maps the export row live.
- **Exports on a grid (the merge with main's first exports work, 319a9af):** `Session::file_at` on
  a structured doc encodes the version with the codec settings it stored (format 6) and
  says `FileCodec::Exact`; a text doc's versions are as on main. Tested at every version
  before and after structural changes (`tests/export.rs`,
  `grid_file_at_round_trips_every_version`).
- **Purge rewrite:** a guard test holds that every store table (format 6's `elem_writes`
  included) is in the rewrite's list and copied, and a grid doc's purge rewrite is tested end
  to end (`a_grid_purge_rewrite_keeps_every_table`).

**Known limits (v0.10.0)**
- Live windows on grids: a later release. `events_since` / `edit_at_base` with a base behind
  a structured doc are `MPC-E112 base_expired` (read again).
- **Restoring a dropped row or column from its `BeforeDrop` version:** v0.10.1. The version's
  cell map holds the dropped cells, but its lists don't keep their positions.
- **`row restore` / `col restore` of a dropped element:** today it refuses with a bare
  `TableError` (`rows: r2 is unknown`). In v0.10.1 the refusal says the row (or column) was
  dropped, when and by whom, and points to restoring it from the `BeforeDrop` version, with
  the restore-from-version verb.
- Known limits for v0.10.1: the kit keeps no durable record of dropped keys, so once a drop's
  record leaves the revert window, a refused write under that key can't say it was dropped.
  `row restore` / `col restore` of a dropped key say 'unknown', not who dropped it and when.
- v0.10.1 (speed): outside saves and the tick grow with history (3–6× over 600 s, same in
  v0.9.8); live edits stay flat.
- v0.10.1 (test-only): the grid traces count a kit `Err` on a revert as a mismatch instead of
  panicking (`grid_traces.rs` unwraps the revert), so a planted (xxxv) shows as counted
  mismatches, not only as a test failure.

## v0.9.8.2 (2026-10-30)

v0.9.8.1 plus the JS client 0.10.0: the live wire contract and the live editor, M1. No change
to the kit's Rust library, store or merge; no new dependency. The crate version is `0.9.8+2`
(the git tag is v0.9.8.2); the client's is `0.10.0`.

### Wire goldens for `overwritten` and `closed`

The client's types for the roster's `overwritten` and `closed` events were checked only against
WIRE.md; now they are checked against the kit's own JSON too.

**Tests**
- `crates/base/tests/wire_golden.rs` encodes `RosterEvent::Overwritten` and `Closed`, and a
  snapshot's waiting notes, and writes four new goldens from a session opened on a file:
  `roster_events_overwritten.json` and its App lens `roster_events_overwritten_app.json`
  (outside saves that remove and replace an author's text: the author's own pushes live, the
  same notes waiting in a later snapshot, then the host's `SessionManager::close`), and
  `roster_events_closed.json` / `roster_events_closed_app.json` (a later subscription: its
  snapshot with `overwritten`, then `closed`, after which it ends). A fifth,
  `roster_events_rollup.json`, holds one person's notes past the cap of 64 (the roll-up). The
  existing goldens are unchanged.
- `client/test/golden.test.ts` checks every field of `overwritten` and of its cells against key
  lists that `satisfies` the type in `wire.ts` (a renamed field fails to compile, or fails the
  key-set check; the editor's own `RemovedTextNote` is held to the same names), the snapshot's
  `overwritten` (present only with waiting notes, deduped on `(change, row)`), `closed {reason}` as
  the last event, and the roll-up's counts (64 waiting, the last `rule: "rollup"`, `cells: []`,
  every place counted).

**Docs**
- `client/WIRE.md`: notes of kind `note` are the ones passed through untyped; `overwritten`,
  and its waiting copies in a snapshot, are typed in full. A snapshot's `overwritten` is present
  only when the subscriber has waiting notes; a missing field means none. The same wording in
  `wire.ts` and `src/editor/notes.ts`.
- `client/WIRE.md`, "JSON encodings": rows for the `overwritten` event (where each field comes
  from in the kit's `Overwrite`; `cells` is the first place, then `also`, in order; `message` is
  its `Display`) and for a snapshot's `overwritten` (bare `Overwrite`s: the host adds `kind` and
  `to`, the subscriber). The roster: members' order isn't significant (compare by `handle`).

### The JS client 0.9.9 and the live wire contract (client/WIRE.md)

What a host needs to carry a live editor over a socket: the handshake, closing, reconnecting,
the own-edit filter, undo's reply, carets at a version, keepalive and a 4 MiB frame cap. No
Rust change in the kit; no new dependency; the client's existing frames are unchanged.

**Added (client/WIRE.md, `client/src/wire.ts`)**
- **Connecting:** `welcome {root, version, text?, client, self, rights}`, the first frame on
  every connection, after the host's authentication, `join`, `subscribe` and
  `subscribe_roster`, then `read`; an optional `hello {root, resume?}`; `snapshot {op}` →
  `snapshot {op, text, version}`.
- **Closing:** `closed {reason, error?}` with the reasons `left`, `idle`, `kicked`,
  `rights_lost`, `deleted`, `moved`, `released`, `cut_off`, `declined`, `not_found`, `auth`,
  `network` and `closed` (`SessionManager::close`), which ones allow a retry (`canRetry`), and
  `error {error}` for a frame with no `op` to answer.
- **The own-edit filter, exactly:** on an edit applied at V the host forwards the queued events
  before V, skips the one at V on that connection, then sends `ack`; nothing applied: all of
  them, then `ack` at the version unchanged. Undo and redo come back as `change`: filter by
  version, never by client ID.
- **Reconnecting:** a new connection and a new client ID; frames sent while disconnected are
  dropped, never replayed (no idempotency key).
- **Undo's reply:** `undone {op, steps, version, notice?}` after the `change` frames it made;
  `steps: 0` is "nothing to undo" (with `MPC-E206` / `MPC-E207` as `notice` when the kit says
  why); conflicts stay `refused` with `MPC-E201`.
- **Carets:** the kit's `set_cursor` resolves offsets against the current text and takes
  no version, so the host maps a stale `cursor` frame through `events_since` with
  `position::transform` (`Bias::After`, as the kit carries carets) or drops it. A caret frame
  has no reply.
- **`visible {visible}`, `view {root?, first, last}`**; no app-level ping (the host pings at
  the socket level; the component resyncs on focus, `online` and visibility).
- **The frame cap: 4 MiB** (`FRAME_MAX`). An `edit` over it is refused with `MPC-E113
  bad_edit`; any other frame with `error`; a `change` too big isn't sent (the client asks
  `since`), a `since` reply too big is `MPC-E112` (reload), and a doc whose text doesn't fit
  one frame is `closed {reason: "declined"}` with `MPC-E401` at `welcome` or `snapshot`.
- **`LiveTransport`** (`connect`, `send`, `onFrame`, `onClose`, `close`) for a live editor
  component; `Transport` is unchanged and is its base.
- **JSON encodings pinned:** `who` (a pid, or an App viewer's handle), `rights` (`edit`,
  `read`, `none`), `role`, `kind`, `reason`, `how`, a caret's `line:col`, `on_behalf_of: null`,
  and `RosterMember` → roster entry.

**Added (client 0.9.9)**
- `LiveText.reconnected(welcome)`: takes the new client ID (the edit in flight is still
  recognised under the one it was sent with), re-sends the caret, visibility and view, then
  resyncs (or resets, when the welcome carries text).
- `LiveText.reload()` (the answer to `onReload`), `onUndo`, `onClosed`, `setVisible`,
  `setView`, the `client` getter; `undo()` and `redo()` return their op.
- An edit whose frame would pass the cap is refused by the client before sending
  (`MPC-E113`) and goes to `onRejected` with its text.
- A roster `cursor` event's `version` beside the caret is taken when the caret has none.

**Tests**
- `crates/base/tests/wire_golden.rs` writes `client/test/golden/*.json` from a real session
  (change events in both lenses, roster events, the refusals) with the kit's JSON writer;
  `MPC_BLESS=1` regenerates. `client/test/golden.test.ts` parses them with `wire.ts`'s types,
  replays the changes to the kit's text and folds the roster to the kit's own.
- `client/test/wire.test.ts`: a reconnect mid-edit (applied, and lost), frames dropped while
  down, the caret re-sent, a resume that can't, the own-edit filter with a lagging writer
  (both outcomes), the client's own undo as a `change`, `undone` with 0 steps, the reload
  past the window, and the frame cap (client-side, host-side, a doc too big at `welcome`, a
  doc that grows past it, and the sizes of the largest frames).
- The fake host follows the host rules exactly: a queue per connection drained by its writer,
  undo per participant with `undone`, reconnects, the cap. Its successful undo now sends
  `undone` and a nothing-to-undo answers `undone {steps: 0}`, so the seeded convergence runs
  that undo draw different latencies than before: their counts differ from earlier runs.

### The live editor, M1 (client/EDITOR.md)

**The live editor, M1** (`k2-mpc-client/editor`, `client/EDITOR.md`): `mountLiveEditor(parent,
opts)`, a CodeMirror 6 editor for one text root in plain DOM, with the connection's life inside
it. The kit's Rust side is unchanged.

**Added**
- **The connection's states:** connect, welcome, live; on a drop, a jittered backoff (1–15 s)
  and `connect({resume})` with a new client ID, then a resync. An edit made while the editor
  knows it is down is sent once after the welcome; an edit already handed to the transport is
  never sent again (the resync settles it: applied, or `MPC-E000` with its text kept).
  `MPC-E112` reloads by snapshot, with a fresh connect as the fallback. A close for good shows a
  banner and freezes the editor read-only.
- **Markdown mode** (highlighting, list keys), line wrapping, CodeMirror's default keymap
  without history; **read-only** and live rights changes.
- **Remote carets** with names readable on the participant's colour (black or white, whichever
  contrasts more), never cut off on line 1; label modes `always`, `active`, `hover`. A basic
  roster strip.
- **Rejections** listed with **Re-apply** and **Dismiss** buttons (keyboard-reachable); `MPC-E111`
  also as a `conflict` event. A change whose reply was lost with a connection (`MPC-E000`) is
  marked `mayHaveApplied`; if it lands after all (a change from the client ID it went out on)
  it leaves the list and the new **`landed`** event fires; Re-apply applies it only when the
  server's record shows it absent (`ReapplyResult`: `applied`, `already-present`, `unknown`,
  `unavailable`). On a host breaking the reconnect rule, a late landing after a Re-apply fires
  **`duplicateRisk`**.
  A rejection with more than one hunk is refused by Re-apply in this version (`unknown`,
  `reason: "multi-hunk"`; `LiveText.reapply` returns `null`) rather than put back in one place;
  restoring each hunk in place through the server's rebase rule is planned.
  **Re-apply now applies only a plain lost insertion** (typed text whose reply was lost, or that
  was refused for a reason other than someone else's change, with nothing changed around it
  since); every other rejected change is refused with "check and retype" until M2, which puts
  them back where the server would.
  **Re-apply's guards (single hunk):** it re-inserts the typed text only for a pure insertion,
  with no reload since, nothing else inserted at its place since and no text around its place
  (or at its edge) deleted or replaced since; anything else is `unknown` with `reason`
  `not-insertion`, `reloaded`, `tie` or `deleted-around`, the rejection kept
  (`LiveText.reapplyGuard`). Each guard closes an M1 case that put the wrong text in: a deletion
  said `applied` and changed nothing, a replacement's text went in beside the old, a change pending
  at a reload went in at the start of the document, at a tie the text went before the other
  insertion where the server's rule puts it after, and a place inside deleted text went to the
  deletion's start (and stayed there when that deletion was refused). The Re-apply scan (seeds
  1–500, strict host): every Re-apply that still applies matches the server's rebase rule.
- **The reconnect rule** (WIRE.md, Reconnecting): `prev=<old client ID>` on every reconnect
  (`ConnectOptions.prev`); the host leaves that connection first and drops its late frames.
  `LiveText.disconnected()` keeps edits made with no connection in its queue until
  `reconnected`; `LiveText.presence`, `onLanded`, `onDuplicateRisk`.
- **Events:** `state`, `roster`, `rights`, `change`, `pending`, `ack`, `rejected`, `conflict`,
  `note` (the kit's removed-text note, `overwritten`, only to this participant, once per
  `(change, row)`), `claim`, `reload`, `closed`, `error`, `participantClick`.
- **Theming** by CSS variables on `.mpc-editor` (`--mpc-bg`, `--mpc-fg`, `--mpc-font`, ...) and
  stable classes.

**Fixed (`LiveText`)**
- **One event per root per version.** A change touching several text roots is one event per
  root at one version; `LiveText` dropped a root's event when another root's event at that
  version came first (the change lost, or an own edit taken for lost: `MPC-E000`). Each
  (root, version) is now applied once, in whatever order, live and inside a `since` reply; a
  repeat is ignored. WIRE.md (Units and ordering) says so.

**Changed (the binding inside the editor)**
- No `doc.toString()` per keystroke: a byte index converts offsets from the changed lines.
- Remote changes are held during an IME composition and dispatched after it; nothing typed is
  lost or doubled.
- A host's CodeMirror history can't change the shared text: undo and redo go to the server.
- `codemirrorLive` (the old binding) is unchanged.

**Tests and checks**
- jsdom: `test/editor.test.ts`; three mounted editors through the convergence harness with
  dropped sockets and resumes (`test/editor-convergence.test.ts`).
- Browsers (Playwright 1.47.2, pinned, all under `client/`): `test/browser/editor.spec.ts`,
  `test/browser/ime.spec.ts` (Chromium: synthetic and Chromium's own IME input; WebKit:
  synthetic). Linux WebKit is WebKitGTK, not macOS WKWebView: a WebKit IME pass is an early
  signal only, and the real IME check is the host application's on macOS.
- The client's licence check is narrowed to MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib and
  Unicode-3.0, with one named dev-only exception (`@csstools/color-helpers`, MIT-0, through
  jsdom).

**Host notes**
- Every `@codemirror/*` package is a peer dependency; the editor needs `@codemirror/view`
  ≥ 6.27, `state` ^6.4, `commands` ^6.6, `language` ^6.10, `lang-markdown` ^6.2 (tested with
  view 6.43.15, state 6.7.6, commands 6.6.0, language 6.10.2, lang-markdown 6.2.5).
- **`MPC-EDITOR-HISTORY`:** the editor refuses to mount with CodeMirror's `history()` among the
  host's extensions (the server owns undo; CodeMirror's history commands can't be
  intercepted). Leave `history()` and `historyKeymap` out.
- The editor takes `wire.ts`'s `LiveTransport` (above) and `LiveText.reconnected`; `connect()`
  while a connection is open replaces it (WIRE.md, "The live transport").
- **Retry is the close table's:** the editor decides whether to reconnect from the close's
  reason (`canRetry`), not from a transport's own `retry` flag, and reports a disagreement once
  as `MPC-EDITOR-CLOSE-RETRY`. WIRE.md's host notes (extra fields ignored, `closed` gains
  `code?` and `path?`, a restart with no frame is `network`, an auth pre-check after a failed
  dial, a hard close with `cut_off`, clients key on error ids) are in the wire types too.

## v0.9.8.1 (2026-10-30)

v0.9.8 plus what v0.9.7.2 added to v0.9.7.1 (the same commits, ported): an outside save that may
have removed someone's text now tells them, and gives them the text; and a revert of an outside
save or of a promoted version puts back the roots it removed. No merge change, no store or wire
format change, no new dependency. A new roster event, so a host matching `RosterEvent::Snapshot`
exhaustively needs `..`.

The crate version is `0.9.8+1` (Cargo takes three numbers; the build metadata resolves as
0.9.8); the git tag is v0.9.8.1. It closes v0.9.8's known issue "stale outside saves remove
text with no note" for text written through the kit (the limits below remain), and its
known issue "revert of an outside save loses roots".

**Fixed**
- **A revert of an outside save, or of a version promoted from tier 0, put the text back as
  new text instead of what the change removed** (undo spec U23; affected: **outside saves from
  v0.9.0, which added provenance (store format 5), and promotions from v0.9.2, through
  v0.9.7.1**). The change's revert record kept no roots,
  so its revert's characters were their own roots: blame credited the reverter instead of
  the text's author, and the rules that read roots (O4a, I1, U10a, U8) couldn't see that the
  text was already live under its own root, so the same line could land twice (found by the
  32-participant soak's history arm, seed 1, call 43,002: the owner's revert of an outside
  delete beside a restored copy of the line). Both records now keep, per hunk, the roots it
  deleted and the elements it inserted, as an edit's always did, so the revert, its undo and
  its redo put back the removed roots. No store format change: records written before this
  release still hold no roots and revert as before. An outside save whose identity map was
  dropped (an `IdentityMismatch` event) still keeps none.

**Added**
- **When an outside save removes someone's text, they're told** (`docs/SESSIONS.md`, "When an
  outside save removes someone's text"). A writer holding older text saves it plus its own change;
  the kit reads that as the file deleting what others wrote since, and until now their line left the
  live doc with `Ingested`, no conflict, and nothing said to them, a person's acknowledged typing
  included. The rule covers **authors who wrote through the kit**: each character's own author, and
  the person an assistant acted for (only the text made on their behalf). It applies to text written
  since the last outside save or within `Config::outside_note_window_ms` (24 h). Whitespace-only
  changes, and adding to a line, say nothing.
- **The note carries the author's own removed text**, exact, at most 16 KiB, and never someone
  else's from the same block, so they can paste it back with an ordinary edit. With the history
  right it names the version that holds it; without it, it never points at a version they can't
  open, and says who can restore what was cut. The words are "may have": a moved line reads as
  removed. No single call restores the text and keeps the saver's change; `restore` of the version
  replaces it, and the note says so.
- **Delivery to people and agents:** `RosterEvent::Overwritten` (with `Snapshot.overwritten`), the
  next verb's `Notes`, `Session::take_notes`; `NoteKind::Overwritten`, `Overwrite`, `OverwriteCell`;
  `AuditEvent::OutsideOverride`, line numbers only. `Debug` prints no text. No read right, nothing
  delivered. A purge takes the text; erasing a participant takes their notes.
- **Held across an unload**, text included, in the manager's memory: agents working one verb at a
  time usually come back after it. 24 h (`Config::outside_note_text_keep_ms`), 1 MiB of text per doc
  and 64 MiB across docs, oldest first; past those the note says the text is no longer held.

- **The host can close a doc's live session, members or not** (`docs/SESSIONS.md`, "Closing a
  doc's live session"): `SessionManager::close(key, reason)` and `close_where(reason, |key, pack|
  ..)`, for a host switching a file type off. A pending write-back is written first; every
  roster subscription gets `RosterEvent::Closed { reason }` last; the host gets
  `AuditEvent::SessionClosed { key, connections, reason }`; every later call, `file_changed`
  included, fails with the new `SessionError::Closed { reason }` (`MPC-E519`, exit 1). Outside
  saves stop merging until the host opens the doc again, which takes them in as usual; waiting
  notes are kept for that open. All additive: `SessionError`, `RosterEvent` and `AuditEvent`
  are `#[non_exhaustive]`.

**Known limits**
- Text past 32 places in one note isn't carried (it is counted, and the note says so).
- Roll-ups carry no text: past 64 waiting notes per person per doc, the rest are a count.
- Typing in flight at the start of a removed line lands on the next line with no signal.
- Notes are lost on a daemon restart (the version, the log and the audit row remain).
- **File-tool text has no author to tell:** text that came in through a file save is `kit:external`,
  so two agents overwriting each other through their own file tools get no note, only an audit row.

**Tests**
- `tests/revert_external.rs`: a revert of an outside save (a typed line; a line an undo had
  put back), with and without `--partial`, with and without a restart between the save and
  the revert, each followed by an undo and a redo of the revert, checked by roots
  (`char_blame`), blame's `restored_by` and I1; the control, a revert of an edit. On v0.9.7.1
  the two outside-save tests fail on U23 and the control passes.
- `tests/promote.rs`: a revert of a promoted deletion (the same checks); fails on v0.9.7.1.
- `crates/spec-undo/tests/revert_external_property.rs`: seeded scripts through a real
  session on a file (agents' edits, undo and redo, whole-text outside saves through the real
  ingest, the owner's revert of any recent change with its undo and redo, restores, a planted
  secret purged, restarts) against `spec_undo::model`: after every step the same text, every
  live character's root on the same change and restored by the same participant, and blame's
  `restored_by` per line by blame's documented rule (the newest root change; on a tie a
  restored character). 300 scripts of 40 steps.
- A planted fault, `differential::ProvMutant::OutsideSaveKeepsNoRoots` (test builds only),
  drops the roots again; the named tests and the property each catch it.
- Test builds only: `crates/base`'s dev-dependency on itself also enables the `conformance`
  feature (`Session::char_blame`); the feature only adds functions.

**Evidence scope.** The undo model check's full-bound runs (724.6M states) and the trace test have
no outside-save step and no promotion, so they say nothing for or against this path; the
property above is the kit-vs-oracle check for it. An outside-save step in the trace
generator is a follow-up.

## v0.9.7.2 (2026-10-30)

A note and a fix on v0.9.7.1: an outside save that may have removed someone's text now tells them,
and gives them the text; and a revert of an outside save or of a promoted version puts back the
roots it removed. No merge change, no store or wire format change, no new dependency. A new roster
event, so a host matching `RosterEvent::Snapshot` exhaustively needs `..`.

The crate version is `0.9.7+2`; the git tag is v0.9.7.2.

**Fixed**
- **A revert of an outside save, or of a version promoted from tier 0, put the text back as
  new text instead of what the change removed** (undo spec U23; affected: **outside saves from
  v0.9.0, which added provenance (store format 5), and promotions from v0.9.2, through
  v0.9.7.1**). The change's revert record kept no roots,
  so its revert's characters were their own roots: blame credited the reverter instead of
  the text's author, and the rules that read roots (O4a, I1, U10a, U8) couldn't see that the
  text was already live under its own root, so the same line could land twice (found by the
  32-participant soak's history arm, seed 1, call 43,002: the owner's revert of an outside
  delete beside a restored copy of the line). Both records now keep, per hunk, the roots it
  deleted and the elements it inserted, as an edit's always did, so the revert, its undo and
  its redo put back the removed roots. No store format change: records written before this
  release still hold no roots and revert as before. An outside save whose identity map was
  dropped (an `IdentityMismatch` event) still keeps none.

**Added**
- **When an outside save removes someone's text, they're told** (`docs/SESSIONS.md`, "When an
  outside save removes someone's text"). A writer holding older text saves it plus its own change;
  the kit reads that as the file deleting what others wrote since, and until now their line left the
  live doc with `Ingested`, no conflict, and nothing said to them, a person's acknowledged typing
  included. The rule covers **authors who wrote through the kit**: each character's own author, and
  the person an assistant acted for (only the text made on their behalf). It applies to text written
  since the last outside save or within `Config::outside_note_window_ms` (24 h). Whitespace-only
  changes, and adding to a line, say nothing.
- **The note carries the author's own removed text**, exact, at most 16 KiB, and never someone
  else's from the same block, so they can paste it back with an ordinary edit. With the history
  right it names the version that holds it; without it, it never points at a version they can't
  open, and says who can restore what was cut. The words are "may have": a moved line reads as
  removed. No single call restores the text and keeps the saver's change; `restore` of the version
  replaces it, and the note says so.
- **Delivery to people and agents:** `RosterEvent::Overwritten` (with `Snapshot.overwritten`), the
  next verb's `Notes`, `Session::take_notes`; `NoteKind::Overwritten`, `Overwrite`, `OverwriteCell`;
  `AuditEvent::OutsideOverride`, line numbers only. `Debug` prints no text. No read right, nothing
  delivered. A purge takes the text; erasing a participant takes their notes.
- **Held across an unload**, text included, in the manager's memory: agents working one verb at a
  time usually come back after it. 24 h (`Config::outside_note_text_keep_ms`), 1 MiB of text per doc
  and 64 MiB across docs, oldest first; past those the note says the text is no longer held.

- **The host can close a doc's live session, members or not** (`docs/SESSIONS.md`, "Closing a
  doc's live session"): `SessionManager::close(key, reason)` and `close_where(reason, |key, pack|
  ..)`, for a host switching a file type off. A pending write-back is written first; every
  roster subscription gets `RosterEvent::Closed { reason }` last; the host gets
  `AuditEvent::SessionClosed { key, connections, reason }`; every later call, `file_changed`
  included, fails with the new `SessionError::Closed { reason }` (`MPC-E519`, exit 1). Outside
  saves stop merging until the host opens the doc again, which takes them in as usual; waiting
  notes are kept for that open. All additive: `SessionError`, `RosterEvent` and `AuditEvent`
  are `#[non_exhaustive]`.

**Known limits**
- Text past 32 places in one note isn't carried (it is counted, and the note says so).
- Roll-ups carry no text: past 64 waiting notes per person per doc, the rest are a count.
- Typing in flight at the start of a removed line lands on the next line with no signal.
- Notes are lost on a daemon restart (the version, the log and the audit row remain).
- **File-tool text has no author to tell:** text that came in through a file save is `kit:external`,
  so two agents overwriting each other through their own file tools get no note, only an audit row.

**Tests**
- `tests/revert_external.rs`: a revert of an outside save (a typed line; a line an undo had
  put back), with and without `--partial`, with and without a restart between the save and
  the revert, each followed by an undo and a redo of the revert, checked by roots
  (`char_blame`), blame's `restored_by` and I1; the control, a revert of an edit. On v0.9.7.1
  the two outside-save tests fail on U23 and the control passes.
- `tests/promote.rs`: a revert of a promoted deletion (the same checks); fails on v0.9.7.1.
- `crates/spec-undo/tests/revert_external_property.rs`: seeded scripts through a real
  session on a file (agents' edits, undo and redo, whole-text outside saves through the real
  ingest, the owner's revert of any recent change with its undo and redo, restores, a planted
  secret purged, restarts) against `spec_undo::model`: after every step the same text, every
  live character's root on the same change and restored by the same participant, and blame's
  `restored_by` per line by blame's documented rule (the newest root change; on a tie a
  restored character). 300 scripts of 40 steps.
- A planted fault, `differential::ProvMutant::OutsideSaveKeepsNoRoots` (test builds only),
  drops the roots again; the named tests and the property each catch it.
- Test builds only: `crates/base`'s dev-dependency on itself also enables the `conformance`
  feature (`Session::char_blame`); the feature only adds functions.

**Evidence scope.** The undo model check's full-bound runs (724.6M states) and the trace test have
no outside-save step and no promotion, so they say nothing for or against this path; the
property above is the kit-vs-oracle check for it. An outside-save step in the trace
generator is a follow-up.

## v0.9.8 (2026-10-30)

**`write --base` stays whole-line, as in v0.9.7.1:** two changes to one line are a
conflict, and a clean merge holds only lines someone wrote.

**Changed**
- **`Doc::replace_text_as` pairs lines first**, as the session's edits have since v0.9.7.1
  (`lines::edit_hunks`): a line deleted or inserted next to a line with the same start is that
  line, so the neighbour keeps its elements. One text applied to the text it replaces; nothing
  merges.
- **MPC-E111's explain text** names `--if-version` for `replace --lines` and `patch`, which take
  no `--base`. Only the text changes; hosts that pin it re-read the golden.

**Host notes**
- `k2 doc write --base` help, one line (K2's wording): "If the text changed at its end while
  you were writing, K2 keeps your new last line separate from theirs (it never joins two
  lines)."
- When an agent's line lands after a last line someone left open, that line's blame names the
  agent, because the line break ending it is the agent's. The person's characters and their
  undo are unaffected.

**Known issues** (found after this release's code was fixed; both are in v0.9.7.1 as well)
- **A revert of an outside save puts the text back as new text.** The outside save's record
  keeps no roots, so the revert can't say which text it restores (U23): the line can end up
  twice, and blame credits the reverter. Nothing is lost. Fixed in the next release.
- **Undo or revert can leave a line twice** when someone put the same text back next to the
  undone region in the meantime. Undo again to remove it. A spec change, planned.
- An outside save from a stale buffer can remove text written since, with no note to its
  author. The next release tells the author what was removed and which version holds it.

**Tests**
- `tests/write_base_end.rs`: the end of the text (amendment O5 end-of-text r3) through a
  session: its table, with the shapes that showed a character merge's faults; the writer's
  undo and revert removing the line break; the live window; D1's word case with its control; a
  CRLF document.
- Trace classes `end_break_a`…`end_break_d`, `write_base_shared_start`,
  `write_base_shared_start_was_conflict`, `write_base_words_same_line` and
  `write_base_multiline_rewrite`, with two planted faults (`differential::MergeMutant`: v0.9.7's
  character merge, and the final line break always as the live side left it).
- `tools/tests/soak.sh`: the throughput guard defaults to 8x (a value set in the environment
  wins); the throughput check runs last in the soak, so it never hides another check.
- `tools/README.md`: the gate, one green job at a round's exact head before the round.
- `docs/design/write-base-word-gate.md`: merging inside a line, deferred, with its reasons.
- The soak's D1 replay regression is `soak_replay_seed_3_no_d1` (was
  `soak_replay_seed_3_known_d1`): seed 3's one known D1 violation (call 25,877) has been gone
  since v0.9.7's D1 amendment (`84ebc1d`; its parent `351e237` still has it), so the replay
  now pins that it stays gone. It had failed since v0.9.7 unseen: the v0.9.7 and v0.9.7.1
  soaks ran `soak_32` alone, not `tools/tests/soak.sh`.
- `SOAK_KNOWN_D1` defaults to 0: a D1 mismatch fails the soak (600 s soaks at seeds 1, 3, 4
  and 5 saw none); 1 turns the tolerance back on.
- `tools/README.md`: a release soak runs `tools/tests/soak.sh` whole.

### The kit's cost as a doc's history grows

No store format change; no exit code change; no new dependency; no public API change.
Measured with `tests/perf_kit.rs` at 10k, 50k and 100k versions
(`docs/evidence/perf-kit-v0.9.8.md`, "v0.9.8 quick fixes").

**Changed**
- A commit reads the snapshot's log position in place instead of copying the whole engine
  state: the store's part of a commit no longer grows with the doc's history.
- The history tick checks the revert window's oldest record first and stops there when
  nothing can leave the window. It no longer reads every record each minute.
- A write-back writes the synced row from the bytes it just recorded instead of reading
  them back. A re-sync is one transaction. The stored rows are the same.
- Compaction: from `Config::compact_entries` entries, once the log's bytes reach a quarter
  of the snapshot's; always at 8 × `compact_entries` entries or at `compact_bytes`. A small
  doc compacts where it did.
- A reopened doc keeps its disk replica encoded until the disk path first needs it.
- Reading one participant's clock (twice per edit and once per undo) reads that client's
  state-vector entry (`ReadTxn::client_state`, k2.6.5) instead of building the whole vector,
  O(clients). A doc's clients grow with its joins.
  - In the 32-participant soak, the edit path's `apply_local` went from 107 → 988 µs per edit
    (v10k → v90k) to 69 → 408 µs.
  - These figures are rough: one run each, one after another, on a shared node.
- A damaged sync state is reported at the first write-back after a reopen, not at open. It is
  the same error with the same id. Every write-back and outside save then fails the same way,
  and nothing is written to the file or the store.

- **Engine:** k2-yrs k2.6.5 (`k2-yrs-0.28.0-k2.6.5`).
  - **k2.6.4:** a text walk crosses a run of deleted blocks in one step, so a lookup or edit
    on a doc with a long history (a doc just loaded, the disk replica) no longer steps through
    every deleted block. Updates are byte-identical.
  - **k2.6.5:** adds `ReadTxn::client_state`, one client's state-vector entry read with one
    lookup. No behaviour change.
  - Vendored without the engine's own `yrs/tests/` (`docs/YRS-FORK.md`).

**Host notes**
- After a crash, reopening a doc may replay up to 8,000 log entries (8 × `compact_entries`,
  the new hard bound) instead of at most 1,000. So the first open after a crash on a large doc
  can take longer. A clean unload still compacts (`unload_compact_entries`), so a normal
  restart is unaffected.
- A doc whose stored sync state is damaged now opens. Its first write-back (`flush`, or the
  tick's) or outside save fails with the error opening used to return. Treat that error as the
  open's: the doc is not synced with its file.

## v0.9.7.1 (2026-10-30)

A hotfix on v0.9.7: two fixes, one to `write --base` (possible text corruption, every
release from v0.1.0) and one to blame, and a clearer `write --base` conflict. No store or
wire format change, no new dependency.

The crate version is `0.9.7+1` (Cargo takes three numbers; the build metadata resolves as
0.9.7); the git tag is v0.9.7.1.

**Fixed**
- **An agent's `write --base` could splice two different lines that start alike into one
  garbled line** (possible text corruption; affected: **v0.1.0 through v0.9.7**). The merged
  lines were wrong: what the two lines said is gone from the live text (earlier versions keep
  it). `write --base` merged the agent's text and the
  live text through sides diffed to the character, which pairs lines that share their first
  characters one line off: when one side inserted a line between two lines the other
  rewrote into one (`p26:95` / `p05:90` became `p20:90`), the merge came out "clean" as
  `p213:102` / `p0:90`, lines nobody wrote (found by the 32-participant soak, seed 1). The
  write now pairs whole lines first and merges **by whole lines** (`merge::write_base_sides`:
  the outside save's whole-line sides): two changes to the same line are a conflict, and
  nothing is ever merged inside a line, so a clean merge holds only lines someone wrote.
  Changes to different lines, adjacent or not, still merge; two different appends at the end
  of the text still both land. The final line break follows whoever wrote the last line, so
  the v0.9.7 limitation where `write --base` could join two lines at the end of a text is
  gone. **Correction:** the v0.9.7 known-issue note ("can be refused with a conflict it
  didn't cause when nearby lines start with the same characters") understated this as a
  refusal; the same pairing could also give a clean merge with a spliced line.
- **Blame could credit a person who deleted a line with the neighbouring line when lines
  start alike; no text was ever changed.** Edits given as a new text applied a diff trimmed
  to the character: deleting `seed:2` from `seed:1` / `seed:2` / `seed:3` deleted
  `2\nseed:` instead of the line, the next line's `3` joined text of the deleted line, and
  D1 made the joined word `seed:3` the deleter's, so blame named a person who never wrote
  it. Verb edits (`edit_with` and the verbs through it), `replace_text`, restore and tier
  0's promotion now pair lines first (`lines::edit_hunks`: `diff::line_hunks`, then the
  character or word diff inside each changed group of lines). The text of every edit is
  unchanged; only which characters an edit names can differ, next to lines with the same
  start. The undo spec's oracle and the conformance suite's restore use the same function.

**Behaviour**
- **Agent writes that touch a line someone else changed since the agent's version are
  refused as a conflict; re-read and retry.** Two agents (or an agent and a person) editing
  different words of one line now conflict instead of merging: nothing is applied, and the
  second one reads the current version and writes again.
- **The conflict says what to do** (`MPC-E111 write_conflict`: same id, exit code 1; the
  message, details and `next` changed). It names the conflicting lines in the agent's text
  and in the current text, the current version, and the retry: read it again with `cat`,
  redo the change on it, then `write --base <that version>`. `details.current`, and each
  conflict's `your_lines` and `current_lines`. Two fresh agents, given only the refusal and
  asked what they would do next, each described the re-read and the retry
  (`docs/evidence/write-conflict-blind.md`: their answers, nothing was run).
- A clean `write --base` applies the verb edits' diff from the live text to the merged text
  (`lines::edit_hunks`), so it names only the characters it changed; blame and undo of a
  write are as for `replace_text`.

**Added**
- `lines::edit_hunks` (public: the hunks an edit given as a new text applies).
- `merge::Conflict::ours_lines` and `theirs_lines` (the region's lines in the writer's text
  and in the live text). A host that builds a `Conflict` by hand adds them.
- `WriteOutcome::refusal_at(base, current)`: the refusal with the current version
  (`refusal(base)` stays).
- The reference host (`examples/mpc.rs`) prints `refusal_at` and has a `write-conflict`
  scenario.

**Tests**
- `tests/write_base_lines.rs` (new): an independent judge of `write --base`'s merge on
  random lines that share a start (`pNN:N`, some with words): each base line kept, deleted
  or rewritten by one side, rewritten by both, or both changing different words of it; new
  lines in the gaps. Every line of a clean merge must be a whole line of base, ours or
  theirs, and the merge must pass the line-LCS rule over every alignment (no deleted line
  back, no line lost; as the outside-save fuzz oracle's `check_lcs`); a refusal passes.
  Both ways round, 2,048 cases by default (`WRITE_BASE_CASES`). It fails on v0.9.7 and
  passes here. Pinned: the soak's splice, two sides rewriting one line into several lines,
  different words of one line (a conflict) and different lines (clean).
- `text::merge`: `adjacent_lines_changed_by_each_side_merge` (new); six tests that relied
  on merging inside a line now assert the conflict, each with the same edits on separate
  lines still merging; the symmetry property reads the merge's own sides. The undo spec's
  oracle merges by whole lines too (`sides_by_whole_lines`), and the fuzz merge checks
  include "every clean line is a whole line".
- D1 class (b) (`undo_foreign_tokens`, `d1_authored_ranges`): the three writers' words are
  on adjacent lines (they still merge, and each undo removes only its own word); the same
  words in one line are a conflict (`class_b_in_one_line_the_write_conflicts`).
- `tests/blame.rs`:
  `deleting_a_line_next_to_one_with_the_same_start_keeps_its_neighbour_its_authors`
  (a verb delete, the deleter's undo, a verb insert and `replace_text` on `seed:0`…`seed:4`:
  every line keeps its author); `lines::edit_hunks_pair_lines_first`.

## v0.9.7 (2026-10-29)

The Live-tab release: the kit's live typing APIs, D1, the speed work, outside saves merged
line by line, the first exports work (the kit's side) and the vendored engine at k2-yrs k2.6.3.

**Breaking for packs:** markdown and text must map the export parity row live
(`export`, `export --version`); a pack that still lists it as N/A, with `export` among its
absent verbs, fails its parity check (below, "Exports, the kit's side").

**Headlines**
- **Live typing APIs:** the hunk channel (`ChangeEvent`'s hunks, what
  editors rebase over), `edit_at_base` and `events_since`, undo over the channel,
  the `run_tool` lens for App viewers (handles, never pids, in `run_tool` output),
  roster push and the participant cap (`Config::participants_max`). Below: "The
  hunk channel", "`run_tool`'s output for App viewers" and "The agent error surface".
- **D1 (undo spec v1.10):** a change is never built from someone else's text; an undo
  never changes a word someone else wrote, and says so instead. Below: "D1".
- **Speed:** the incremental disk write-back and blame, the engine's position index
  (k2.3), an outside save's change at identity from the merge's own hunks, and the edit
  check's sampling adapting to the doc's history. **The 2× soak target is not met:** the
  32-participant soak slows 6.9× over 600 s at about 100,000 versions (v0.9.5: about 11×;
  gate job `01e766d0`, a regression guard at 8× from this release); edits are about 8×
  cheaper than in v0.9.3. What still grows with the doc: the engine's per-edit transaction,
  the full-state encode at each write-back, and outside saves (each encodes, renders and
  hashes the doc). Below: "Speed".
- **Outside saves merge line by line:** no line joined, spliced, lost or brought back by
  an outside save's merge (both fuzz corpora: 0 of each). Below: "Speed" (Fixed).
- **Exports, the kit's side,** and the export row's `export` verbs. Below: "Exports, the
  kit's side".
- **Host guidance, paths and hard links.** Below.
- **Stable error ids** (`MPC-Ennn`, `docs/ERRORS.md`): the first tagged release with the
  registry. Below: "The agent error surface".
- **Engine:** k2-yrs k2.6.3 (hardening; warning-free), `k2-yrs-0.28.0-k2.6.3`, vendored
  without the engine's own `yrs/tests/` (`docs/YRS-FORK.md`).

### D1: a change is never built from someone else's text

Undo spec v1.10, the authored-ranges amendment (`docs/SPEC-UNDO.md`, D1a).
Found by the 32-participant soak (seed 1, call 241,426; released v0.9.4 fails the same way):
a line replace could keep another participant's characters, and that participant's later
undo then changed the replacer's text (`p14:3169` became `p14:3167`).

**The promise:** an undo never changes text someone else wrote, at token granularity. The
sentence hosts pin verbatim (`k2 doc --help`):

> Undo only reverses your own changes and never alters a word someone else wrote: if
> someone has since changed one of your words, that word is theirs, and your undo leaves it
> as it is and tells you.

**What changes for people:**
- When a change touches a word someone else wrote (typing into it, onto its end, or
  replacing part of it), the whole word becomes the change's author's: in blame, and for
  undo. An undo by the word's earlier author then reports a conflict instead of changing it.
- Deleting part of someone's word leaves the rest theirs; joining two words makes the joined
  word yours.
- Editing an imported file's word makes that word yours in blame.
- Restoring a version makes the words it changes yours in blame; an undo by the original
  authors of those words then reports a conflict instead of changing them.
- Edits to one's own text are unchanged.

**Rule:** a fresh change (no roots given: edits, verbs, `write`, `write --base`'s own hunks,
a version restore, a live edit at a base, a person's grouped keystrokes, an outside save, a
tier-0 version a promotion replays) is widened to the tokens of the new text that hold what it
inserted or that it joined; inside them, only the changer's own characters are kept, and the rest
are deleted and inserted afresh with the change. Undo, redo, revert and purge are left as they
are. Tokens are the pack's (`Pack::tokens`): words by default, `Tokens::Code` for code,
`Tokens::Cells` for cells; `TextFilePack::with_tokens` chooses one.

**Host notes:**
- The hunk channel (`ChangeEvent`) carries the hunks the participant authored, not the
  widened ones (live rules r2.1): the same text, and what clients rebase over. So does the live
  window (`events_since`, and what `edit_at_base` rebases over), read from the store, after
  a restart too. Carets, views and claims follow the authored hunks; undo regions and revert
  records follow the elements.
- **Outside saves** are widened like any diff-based change (the merge's hunks are the
  authored ranges, as `write --base`'s ours): a save that changes a word someone wrote in
  the kit makes the word the external participant's, so their undo conflicts instead of
  changing it. A save that touches only the external participant's own words, or none, is
  reported exactly as before.
- **Tier-0 promotion** widens each version it replays as a change by its author (ownership
  by each character's client: a promoted doc has no restored text), so a revert of promoted
  history never changes a word another version's author rewrote; the widened change's
  authored hunks (the diff of the two versions' texts) go in the `authored` table.
- **Grouped edits** (U3) are widened too; the closers read what the person typed, and
  joining reads the change as made (the oracle the same way).
- **Additive:** `Pack::tokens` (default words), `TextFilePack::with_tokens`,
  `text::lines::{Tokens, authored_hunks}`.
- **Store: no format bump.** A widened change's authored hunks go in a new `authored` table
  (`DocStore::authored_hunks`, `DocStore::change_hunks`), created by the first widened
  change: a store no change was ever widened in has none. Every open sweeps it (a row whose
  record is gone is deleted; an insert its redacted record no longer holds is blanked) and,
  when that changed anything, rewrites the file; a failed rewrite fails the open and is
  retried at the next.
- **Downgrading** to v0.9.5 or v0.9.6 after any doc had a word-level rewrite: reads and
  edits work, purges are refused before anything changes (`MPC-E502 store_newer`, table
  `authored`) until you upgrade again; deleted docs' authored rows are cleaned on the next
  open by ≥ v0.9.7. `authored` is now this kit's own table: no longer refused as a newer
  kit's, and covered by delete, purge, rewrite and erasure.

**Tests and evidence:** the shared function's unit and property tests (D1 under
`Tokens::Chars` is the identity, so the undo model check's full-bound evidence stands as taken); the
oracle applies the same function; classes (a), (b) and (c) on the oracle, the kit and the
conformance suite (`hist.undo_own_only_tokens`); the three D1 mutants
(`reuse-foreign-elements-in-changed-token`, `rewrite-whole-line`,
`rewrite-merged-ranges`) each fail their class in the oracle and the kit; the trace test
(oracle vs kit) on words, the grouping class included, and a 50,000-trace run on a node
(`d1_traces_large`); an outside save's widening with its negative control; blame, the log
and the live window after a reopen and after a purge. D1 is not yet in the TLC cross-check (the
Quint model follows); its three mutants live outside `Mutant::ALL` until it is.

### `run_tool`'s output for App viewers

- **`run_tool`'s output names others by handle for an App viewer** (text docs;
  `Session::tool_output_as_seen`): a read's fence (its contributors), a `--for-edit` claim or
  conflict (or the refusal it carries), and a session refusal (`app_view.rs`,
  `an_app_viewers_tool_output_holds_no_pid`; was: pids there).

### Speed: write-back, blame and outside saves

**Fixed**
- U19: the "cleared by purge" notice owed to someone who had left was lost when the idle
  sweep (`expire_idle`) forgot their emptied stacks, so their next undo said "nothing to
  undo" (or "history lost"); the sweep now keeps a participant while the notice is owed.
- The outside-save fix released in v0.9.6 (a line's deletion lost, its retype then held
  twice; `diff::line_hunks`) is in this release too.

**Fixed: an outside save merges line by line** (`text::merge::merge3_keep_both`; outside
saves only: `write --base`, undo and `diff::hunks` are unchanged). Every change of the file
and of the live doc now covers whole lines, so where both changed the same line, both
versions of it are kept, each a line of its own, the live one first. This removes three
kinds of wrong result an outside save could give over unwritten edits (v0.9.6's known issue,
two people's changes on one line, among them):
- **lines joined:** a line the file left open glued to the live side's next line (the file's
  "aaaaaaaaaaaaa" landing on the live "lo" as "loaaaaaaaaaaaaa");
- **lines spliced:** two people's changes to one line merged character by character (a
  typed prefix and a typed suffix giving "XdY");
- **a deleted line brought back:** keeping both sides by region could restore a line one
  side deleted and the other left untouched. Base "aX\nA\nb\n", the file "X\nA" (it edits
  the first line and drops the last), the live text "Z" (it replaces everything): v0.9.5
  and v0.9.6 give "Z\nX\nA", though the live side deleted "A" and the file never touched
  it; now "Z\nX".
- The end of the text is merged on its own: the result ends without a final newline only
  when the side that wrote its last line left its own text so (agrees with `write --base`'s
  end-of-text rule, O5a). A line one side deleted no longer comes back because the other
  side only added a final newline after it, or wrote lines after it.
- Where the other side's lines can be read more than one way (repeated or blank lines), the
  merge takes the kit's reading; the fuzz oracle accepts any reading (over every alignment).
- What the save changes in the doc is only what differs: each of the merge's whole-line hunks
  is trimmed to the characters it changes before it is applied (the text is the same byte for
  byte). So a save that changes one word leaves the rest of the line, and every line it kept,
  with their authors, and D1 widens it to tokens as for any diff-based change: blame and undo
  stay at word level.

**Blame's provenance map is kept up to date through outside edits, and checked.** Additive
for hosts: three `Config` fields and their default constants, two `AuditEvent` variants
(`ProvenanceMismatch`, `IdentityMismatch`; the enum is `#[non_exhaustive]`). No store format
change (still 5); no dependency changes.

**Changed**
- **An outside edit keeps blame's provenance map** (each live character's element and
  root) instead of dropping it. Until now the next blame (or edit) after an outside edit
  rebuilt the map from scratch: a throwaway replica of the doc, O(the doc's history). The
  outside edit's change at element identity (v0.9.3) now updates the map like any other
  change: its inserted elements are fresh (their own roots), with the IDs the engine gave
  them where the merge put them. That includes a root whose text the merge left as it was on
  new elements (text deleted and typed again). The map is still built from scratch on first
  need after a load, a purge or a recovery (the cold path).
  - Blame after an outside edit, p50 on a build node (release): 1.29 → 0.85 ms at 1.2k changes,
    9.4 → 1.1 ms at 40k, 83 → 1.3 ms at 300k (`docs/evidence/v0.9.7-blame.md`).
  - The session's own changes (edits, `write --base`, undo, redo, revert, restore) already
    kept it up to date as they applied (v0.9.0); blame never rebuilt it after them.
  - An outside save that the identity check samples (below) also compares the map's element
    IDs with the engine's, which it reads anyway: a difference is reported (below) and the
    map rebuilt.
- **Blame reads approvals per change** (one read for the changes it names), not the doc's
  whole approval table, which grows with the history.
- **perf: an outside save no longer recomputes its change at element identity from the whole
  engine state.** Since v0.9.3 the change was found by blaming the doc twice (each time a
  throwaway replica of its whole state, every element ever inserted included) and comparing
  the element IDs of the old and new texts character by character. That was 65–94% of an
  outside save: 433 ms at 1 KB × 300k changes, 977 ms at 1 MB × 40k. Since v0.9.5 the merge
  is applied to the live doc itself at the positions its hunks give, so those hunks, in
  canonical form (no hunk that changes nothing, touching hunks joined) and handed to the
  engine as such, are the change at element identity. They are used as such, after a check
  that they give the new text; they also update blame's provenance map (above) in O(the
  change), the way the session's own edits do.
  - One outside save in `Config::edit_check_every` (default 1,000) still computes the change
    in full and compares, in canonical form. A difference is reported
    (`AuditEvent::IdentityMismatch { key, root, change }`, new, metadata only); the merge's
    hunks are kept for the change's record and blame's provenance map is dropped (rebuilt
    from the engine on next need).

**Added**
- **Blame's runtime check.** `Config::blame_check_every` (default 1 in 100,
  `session::DEFAULT_BLAME_CHECK_EVERY`; 0: never sampled), adaptive: from
  `Config::blame_check_large_at` changes on (default 20,000), one blame in
  `Config::blame_check_every_large` (default 1,000). A sampled blame also rebuilds the
  map from the engine and the store and compares the two (runs and live restorations by
  root). The rebuild, O(doc), runs **off the session's lock** on a copy of the doc, so a
  sampled blame never stalls other participants' edits (only its own caller waits). On every
  blame, sampled or not, each root's map must be as long as its text. A mismatch is never
  served: the blame is computed again from a sound map, the kept map is replaced by the
  rebuild (or dropped, if the doc changed meanwhile), and the session reports
  `AuditEvent::ProvenanceMismatch { key, version, what }` (`what`: a root name and a byte
  offset, no doc content). Lock hold of a sampled blame at 300k changes (a build node, p50):
  104 ms → 21 ms; what remains is encoding the doc's state for the copy.
- **Tests:** `tests/blame_incremental.rs`: the differential test (random histories: edits,
  multi-hunk changes, `write --base`, undo, redo, revert, restore, purge, outside edits,
  approvals, compaction, restarts; after every step the kept map equals a rebuild and blame
  of every line and of a random `-L` range equals blame from the rebuild in every field),
  and three planted faults caught by it and by the runtime check. `tests/blame_cost.rs`
  (ignored): blame's cost at 1.2k, 40k and 300k changes. The `differential` feature gains
  `with_prov_mutant`, `with_blame_check_hook` and three test-only `Session` hooks
  (`blame_rebuilt_for_tests`, `prov_differs_for_tests`, `prov_built_for_tests`);
  `DocStore::unsynced_for_tests` is available under it too.
- **The edit path's canary adapts to the doc's size:** the canary's interval grows with the
  doc's history past `Config::edit_check_scale_units` (default 50,000 UTF-16 units, about
  the history at v10k in the 32-participant soak; `doc::DEFAULT_EDIT_CHECK_SCALE_UNITS`; 0: a
  fixed rate), to `edit_check_every x history / scale` edits, never fewer than
  `edit_check_every`, so its amortised cost per edit stays about what it is at v10k (a
  sampled edit is O(history): 3.7 ms at v10k, 128 ms at v100k in the soak's profile). Below
  the scale a doc is checked as often as before. The first sample after a load is placed
  from the history alone (`edit_check_every` edits in below the scale; past it at a phase
  spread over the interval), so the long-run rate does not depend on how often the doc is
  reloaded: before, each reload restarted the count, and the soak's 63 restarts kept the
  canary at about one check per reload whatever the interval. The history is the sum of
  the state vector's clocks, read only on a sampled edit or a load.
  `Doc::set_edit_check_scale`; the session sets it on every doc it makes. Outside saves'
  identity check keeps the fixed rate.

**Known issues**
- If someone rewrites a line live while an outside editor deletes it and types it back, both
  lines are kept (the live rewrite and the retyped line).
- An agent's write --base can be refused with a conflict it didn't cause when nearby lines
  start with the same characters; nothing is applied, and writing again against the current
  version succeeds (fixed in v0.9.8).

### Exports, the kit's side

**Additive** for hosts; no exit code changes; no store format change; no new dependency;
no change to existing behaviour.

**Added**
- **`Session::file_at(conn, vid)`**: the file's bytes at a stored version, as a write-back
  of that version's content writes them (the pack's encoding with the doc's codec
  settings), in a `FileBytes { bytes, version, codec }`. `version` is the doc version a
  person sees in `log` / `show` (the version's log position): name an export after it, never
  after the internal version id. Needs history, like `show`; an unknown version is
  `NoSuchChange` (exit 4). Reads only: no version, change, lease, claim or read base.
- **`Session::file_now(conn)`**: the current file's bytes and the doc version they are
  ("now" resolved first). Needs read only.
- **Streaming copies, `file_at_to` / `file_now_to`** on `Session` and on `BlobSession` (tier
  0): the file at a version (or now) written to any `io::Write`, returning a `FileCopy
  { version, bytes, codec }`. For tier 0 the bytes stream from the blob store in bounded
  memory (256 KiB at a time; the session lock isn't held while copying) and are verified as
  they stream: the check (the blob's HMAC and size) completes at the clean end, so **only
  `Ok` means the copy is good; on an error (`BlobError::Damaged`, exit 1) the caller must
  discard what was written** (copy to a temporary file, rename on `Ok`). For multi-GB
  derivatives and exports. Rights as `read_version` (history) and `read` (read).
- **`FileCodec`**: how the codec settings were applied. `Exact` (the current doc, as the
  write-back writes it); `Current` (a past version rendered with today's line endings, BOM
  and final newline: exact unless an outside save changed them since); `ChangedSince` (the
  log shows a settings change since that version; returned once the log records settings
  changes, in a later release).
- **Module `export`**, the contract between a host and format exporters (which live outside
  the kit): `Exporter` (`id`, `targets(kind)`, `export(src, target, opts, sink, ctl)`),
  `ExportTarget`, `Fidelity`, `OptionSpec` / `OptionKind` / `Options` (checked before any
  work), `SourceKind`, `Source` (`Bytes` or a text doc's `Text` roots), `ExportCtl` (cancel
  flag, progress, `ExportLimits`, host `Resources`), `Sink` (`finish` only after `Ok`),
  `ExportReport` (losses, notes, pages, bytes written), `ExportError`; `ExportRegistry`
  (register, `targets_for`, `export`: options checked, a cancel honoured, output past
  `max_output_bytes` refused at the sink as `Limit`, a panicking exporter caught as
  `Internal`). The kit's only exporter is `AsIs` (target `as-is`: the bytes unchanged).
  Every public type is `#[non_exhaustive]`.
- **Ids `MPC-E001`–`MPC-E007`**, a new area `0xx: exports` (`errors::Area::Export`):
  `export_unsupported` (2), `export_declined` (2), `export_limit` (2), `export_cancelled`
  (1), `export_decode` (1), `export_io` (1), `export_internal` (1); `ExportError::exit()`
  and `Refusal` for `ExportError`. The registry snapshot, `docs/ERRORS.md` and
  `capabilities` carry them.

- **"Export" is a live parity row** (was N/A, "cat is the file's bytes"): kit verbs
  `export [--to as-is]` (read) and `export --version V [--to as-is]` (history), on both tiers,
  bound to `file_now` / `file_at` (tier 0: `file_now_to` / `file_at_to`) and then
  `ExportRegistry::export` (`parity::Api::FileNow`, `Api::FileAt`; `VerbValue::Export`). The
  parity cases export now and at a past version on a live doc and a tier-0 file, the person's
  path and the agent's through the registry, and compare the bytes, the version and the codec
  flag. `--version` takes the version id; the export is named after the doc version
  (`FileBytes::version`). The kit exports `as-is` only; other targets are the host's
  exporters.

**Breaking for packs' parity lists**
- A pack that lists the export row as N/A with `export` among its absent verbs now fails its
  parity check
  (the kit registers `export`): map it live, `ParityRow::live(a, &["export", "export
  --version"])` (k2-mpc-markdown and k2-mpc-text at their next kit bump).

**Tests**
- `tests/export.rs`: `file_at` round trips at several versions of a text doc and of a
  Markdown doc with a BOM and CRLF (the bytes then, the doc version, `Exact` for the latest,
  `Current` for older ones); an unknown version; rights (history for `file_at`, read for
  `file_now`); reading and exporting leave the doc as it was (status, versions, log); the
  contract through a test exporter (listing and de-duplicating targets, duplicate ids,
  option checks, cancel before and during, the input and output limits, a caught panic, the
  identity exporter on bytes and on text); every export error's id and exit.
  The streaming copies: a tier-0 version and the current file copied out byte for byte
  (and a live doc's, equal to `file_at`), history needed for a past version; a blob whose
  last byte was flipped fails the copy at its end (`Damaged`), after every byte reached the
  writer. `tests/tier0_large.rs`: a 50 MB file's versions copied out with `file_at_to` /
  `file_now_to` in bounded memory (the allocator's peak), compared as they stream.
  `tests/error_ids.rs` reaches the seven ids through `ExportRegistry::export`.
- Grid docs' `file_at` round trip comes with the grid work (its round with main).

**Known limits**
- **Per-version codec settings:** a version stores its content, not the codec settings in
  effect then, so `file_at` renders a past version with today's (`FileCodec::Current`). On the
  next store format bump's list: store the codec settings with each version, making every
  `file_at` exact.

### Host guidance, paths, hard links

**Additive** for hosts; no exit code changes; no store format change; no new dependency.

**Added**
- **`next` per surface:** `ErrorReport::with_next(surface, text)`, `next_for(surface)`,
  `to_json_for(surface)`, `to_value_for(surface)`. A host's own guidance for a surface (a
  tab, an App); the id, exit code, message, rule and details never change (a test holds it
  for every refusal).
- **No absolute paths:** `errors::redact(text, root)`, `ErrorReport::redacted(root)`,
  `SessionError::redacted(root)`: a path inside the workspace root becomes relative to it,
  any other absolute path `<outside>`. The kit applies it itself where it knows the root:
  `SessionManager::open_file` / `open_blob_file`, and a file session's (text or tier 0)
  `file_changed`, `check_disk`, `file_moved` and `flush`. A registry-wide test feeds a
  root through every refusal; `tests/app_view.rs` checks no absolute path reaches an App
  viewer.
- **Hard links:** `open_file` and `open_blob_file` decline a file with more than one
  name: `Declined` `hard_linked`, new id **`MPC-E412`** (`declined_hard_linked`, exit 4).
  `session::FileIdentity` (device and inode), `session::file_identity(path)`,
  `Session::file_identity()`, `BlobSession::file_identity()`.

**Host notes**
- One new id, `MPC-E412`; the registry snapshot, `docs/ERRORS.md` and `capabilities` carry
  it. Refusal texts from `open_file` and file sessions now name files relative to the
  workspace root: a host that parsed absolute paths out of them must use the paths it
  holds instead. `AuditEvent::DiskError` keeps real paths (operator only).
- `docs/HOST-ERRORS.md`: "Guidance per surface, paths and hard links".

**Merged from v0.9.5** (the hotfix's refusals get registry ids; their exit codes and messages
are unchanged)
- **`MPC-E152 unaccepted`** (exit 3): `DocError::Unaccepted`, an update this build does not
  accept, refused before anything is applied. `doc::RESERVED_ERROR_ID_UNACCEPTED` is now
  `Some("MPC-E152")`.
- **`MPC-E518 quarantined`** (exit 3): `DocError::Quarantined`, a stored doc set aside for
  review and not loaded. `doc::RESERVED_ERROR_ID_QUARANTINED` is now `Some("MPC-E518")`.
- `StoreError::NewerData` reports as **`MPC-E502 store_newer`** (exit 6), with details
  `{table}`; its message is the upgrade guidance.
- The registry snapshot, the `explain` goldens, `docs/ERRORS.md` and `capabilities` carry the
  two new ids.

### The hunk channel (live typing)

**Additive** for hosts; no exit code changes; no store format change; no new dependency.

**Added**
- **`Session::edit_at_base(conn, root, base, hunks, &EditOptions)`**: a live edit made at a
  base version, rebased positionally through every change since
  (`session::rebase`: the change applied first stays left at a tie; deletion vs deletion is a
  no-op; others' insertions are never deleted; a replacement's text at the left end) and
  applied as one attributed change. Refusals: `MPC-E111` only for competing text (a
  replacement meeting a range deleted since, or an insert strictly inside one) with the
  region; `MPC-E112` past the window; `MPC-E151` for hunks invalid at the base.
- **`Session::events_since(conn, version)`**: the `ChangeEvent`s since a version, from
  the store, bounded by `Config::live_window_events` (1,000) and `live_window_bytes`
  (4 MiB); `MPC-E112` past them.
- **`session::rebase`** (`rebase_ours`, `rebase_incoming`, `map_point`, `map_range`): the
  pure rules, shared with the JS client through `crates/base/tests/vectors/live_rebase.json`.
- **Tests:** `tests/live_edit.rs`: each rule; the property (3 clients modelling the JS client, mixed
  with agent edits, undo and outside edits) for convergence, refusals, others' text kept and a
  refusal changing nothing; the six mutants; the vectors; a trace hook (`LIVE_TRACES`) for K2
  Engine's exhaustive model. Clarification r2.1: the window is exactly the hunk sets broadcast
  (checked against every subscriber, every kind of change); clients typing in the same multi-author
  word never bounce; the mutant `window-holds-expanded-hunks` is caught.
- **Live rules r2.3: each hunk is carried as sent** (r2.2's rule (1), "re-canonicalize at every
  step", withdrawn on both sides, after K2 Engine's exhaustive model found it wrong both ways
  at 2 clients × 2 changes). `session::rebase::Carried` (new): a change while it is carried,
  its hunks keeping their identities through every step (`through` for the server's role and
  the client's own pending change, `mirrored` for an incoming change, `gaps` for the pending
  change's gap form, `hunks` for the result, canonical). So a deletion of text another change
  replaced no longer bounces when ours' own insertion came to touch it, and a replacement
  split around others' text still conflicts with a replacement of its split-off part.
  `LiveConflict::ours` is the hunk's index as sent. Vectors **119** (now refused at step 1)
  and **137** (the conflict names hunk 1, not the merged hunk 0) change; five cases are
  appended (209–213: the two counterexamples, an insert at each edge of a deleted range (S3),
  a fully absorbed change (S4)).
- **Live rules r2.3 S3: "strictly inside" under both tie-break placements.** An insertion conflicts
  with a range a change since deleted (rule 4 (b) ii) only if it is strictly inside wherever
  a tie put it among others' insertions at its point: each insertion is also carried with the
  other bias (its left placement), and both must have deleted characters on both sides. So
  an insertion tied with someone's text no longer bounces when a later deletion takes that
  text and only one of its neighbours. Vectors **83** (the conflict now names hunk 1) and
  **149** (now applied) change; **214** is appended (K2 Engine's case). The ID oracle judges
  by both placements (K2 Engine's markers); the mutant `inside-by-tie-break` (the right
  placement alone) is caught. K2 Engine's r2.3 exports (k2-mpc-engine 9e2dd2b: 2×1 539,
  3×1 21,399, 2×2 8,811 cases) replay with 0 server and 0 mirror disagreements.
  - **Tests:** an ID-level oracle (K2 Engine's: a refusal only on a conflict by intent,
    and every such conflict refused) judges every live edit of the property, every stream;
    a new stream of mixed edits at overlapping ranges reaches the shapes rule (1) got wrong;
    the mutants `recanon-server-only` (caught by the client's and the server's disagreement)
    and `recanon-both-sides` (caught by the oracle); the trace hook counts server and mirror
    disagreements and replays K2 Engine's exported cases (`LIVE_TRACES`); a TP1 property
    along histories for the carried forms.
- **Undo grouping for streamed edits** (undo spec U3): `EditOptions::group(token)`.
  Consecutive edits from one connection with the same token (through `edit_at_base` or
  `edit_with_options`) are **one undo step**. The session closes a group on idle on the host
  clock (`Config::undo_group_idle_ms`, 1 s), a non-contiguous edit, a word boundary typed (a
  space, tab or punctuation after a word character starts the next step), a newline (a step
  of its own), another token or any other change by the person, an undo or redo, and anyone
  else's change inside the group's span or touching it, in apply order (U3 (g)). The group is
  one region whose provenance roots compose exactly, so its undo and redo are its edits',
  element for element; a group that undid itself leaves no step. Agents never group. Only
  the undo stack push changes (`session/live/group.rs`, `undo::compose_group`).
- **Tests:** `tests/history.rs`, `hist_undo_group_person_*`: each closer, agents, a group
  that undid itself, a group over someone else's text (by root, I1); the trace class
  (`GROUP_TRACES` seeded histories, default 96: a person typing with tokens on two
  connections, another person, idle gaps, undo and redo, against a string model of each
  step's window); three planted faults (`differential::GroupMutant`: never closes on a
  foreign change, on idle, on a word boundary), each caught by its scenario and by a trace.

**Host notes**
- Two `Config` fields (`live_window_events`, `live_window_bytes`) in `capabilities`.
- `EditAtBase` (new), `ChangeEvent` unchanged. The JS client must reproduce the vectors.
- **`RosterEvent::Cursor` gains `version: Seq`** (the roster push is unreleased, so this is
  free): the doc version the caret's byte offsets are in, read with the caret at delivery
  (after any edits that carried it). A `Snapshot`'s carets are at the snapshot's existing
  `version`. A client maps a remote caret to its own text through the changes between.
  Patterns on `Cursor` need the new field or `..`.
- **`EditOptions::group(token)`** (new; `EditOptions` stays `Copy`, the token is kept as a
  hash) and **`Config::undo_group_idle_ms`** (new, default 1,000; in `capabilities`). Pass the
  client's `group` from each `edit` frame (client/WIRE.md). Without a token nothing changes.

**Changed**
- **The vendored engine is updated to k2-yrs k2.6.3** (`k2-yrs-0.28.0-k2.6.3`, from k2.2 on
  main and k2.3.2 on the v0.9.7 speed work; k2.6.1–k2.6.3 add a warning-free build and
  wording only; `tools/vendor-yrs.sh`, `--check` passes; `docs/YRS-FORK.md`). The vendored
  copy leaves out the engine's own tests (`yrs/tests/`: they run in k2-yrs), as v0.9.6 did.
  No kit API, store format or dependency change; `Cargo.lock` is unchanged.

### The agent error surface

**Additive** for hosts; **no exit code changes**; no store format change; no new dependency.

**Added**
- **Stable error ids** (`k2_mpc_base::errors`, `docs/ERRORS.md`, generated): one registry of
  every refusal, `MPC-Ennn` grouped by area (1xx sessions and edits, 2xx history and undo, 3xx
  claims, 4xx disk, 5xx store, 6xx erasure, 7xx tier 0, 8xx structured and CSV, 9xx limits):
  97 ids, 93 live, 4 reserved (`MPC-E801`–`E803` for the CSV grid refusals, `E161` for engine
  codes this build doesn't list). Each names the rule it enforces and says what to do next.
  `MPCE-Ennn` is reserved for the engine (no ids yet); its codes cross as strings
  (`ErrorReport::engine`).
- **Every refusal carries its id**: `report()` on `SessionError`, `ToolError`, `ToolRunError`,
  `StoreError`, `BlobError`, `DocError`, `Declined`, `PackError` (`errors::Refusal`);
  `refusal()` on `WriteOutcome` and `UndoOutcome`; `ToolOutput::refusal` for an edit verb's
  "no" (a failed patch, exact `replace` with no or several matches, a pack verb that applied
  nothing, `patch --partial` rejects). `ErrorReport::plain()` is the message as it always read
  with the id appended, `… (MPC-E110)`; `to_json()` is `{"id", "exit", "message", "next",
  "rule", "details"}`.
- **`explain` and `capabilities`** (`errors::run_meta`): an entry, or what the build can do
  (verbs, flags, parity rows, the ids each verb can return, the limits in effect, the kit
  version), as text or JSON. `Config::limits()` lists every setting.
- `docs/HOST-ERRORS.md`: how a host's CLI surfaces them (`k2 doc explain`, `k2 doc
  capabilities`, `--json`, the catalog rows). `crates/base/examples/mpc.rs`: a reference host
  (the blind-agent test's CLI).
- **The roster push**: `Session::subscribe_roster(conn)` and
  `SessionManager::subscribe_roster(conn)` return a `Receiver<RosterEvent>`. The first event is
  a full `Snapshot` (the `roster()` rows with each participant's rights, at the doc's
  version); then deltas: `Joined`, `Left`, `Rights`, `Role`, `Away`, `Cursor` (anchor and
  head), `ClaimStarted` and `ClaimEnded` (with lines and version), and `Note` (a heads-up
  delivered to someone). Every event names its participant as a `Who` (`Who::Pid` today;
  `#[non_exhaustive]`, so an opaque handle can join it later without breaking matches).
  - Coalesced over `Config::roster_interval_ms` (new, default 250): at most one delivery per
    interval per doc; a caret moved many times is one event at its last place. Joins, leaves,
    claim starts and ends and notes are never coalesced away.
  - Bounded: `Config::roster_queue` (new, default 256) events per subscriber, and recorded
    between two deliveries. A receiver that falls that far behind gets a fresh `Snapshot`
    once it has room, instead of what it missed; a burst over it in one interval sends every
    subscriber a `Snapshot`.
  - Every subscriber of a doc sees the same events in the same order. Dropping the receiver
    unsubscribes; it closes when the connection leaves, is kicked or loses read. Needs read.
  - Built on the session's own presence and claims (`roster()` and the push share one fold);
    no new state beyond what subscribers were last told, and nothing when nobody subscribes.
  - No new agent verb: it is a host's feed, not a person's action, so no parity row.
- **Opaque handles for App viewers**: a viewer whose `Participant::kind` is
  `ParticipantKind::App` never sees a pid, its own included. Every pid the kit hands it
  becomes a handle, `p1`, `p2`...: the roster and its push (`Who::Handle`, new), change
  events' author and `on_behalf_of`, `read`'s contributors, `blame`, `log` (and its `--by` /
  `--on-behalf-of` filters, which take the viewer's handles; a raw pid matches nothing),
  `claims`, `reservations`, `activity`, approvals, heads-up notes, `write --base` conflict notes,
  and refusals that name someone (`Claimed`'s `by`, in the error and its report), and
  `run_tool`'s output (v0.9.7, below).
  - Per doc and per viewer: numbered in the order that viewer first sees each pid; the same
    for the same pid across the viewer's reconnects while the doc is loaded; independent
    between viewers. The kit's own markers (`kit:import`, `kit:external`...) pass unchanged.
  - Erasing a participant forgets its handles.
- **The participant cap**: `Config::participants_max` (new, default 32) distinct
  participants per doc. A new pid joining past it is a **watcher**: read-only whatever the
  host grants, no caret, view or claims, no roster row (the roster shows only a count:
  `Session::watching`, `RosterEvent::Snapshot { watching }`, `RosterEvent::Watching`).
  - Its edits, carets and claims are refused with **`MPC-E305 watching`** (exit 3), the next
    id in the presence area (appended to the registry).
  - A slot that frees promotes nobody by itself: the host calls
    `Session::promote_watcher()`, which lets the earliest watcher in (its rights are the
    host's again from its next call) when a slot is free.
  - No person action, so no parity row.

**Host notes**
- Drive the roster push from your clock: call `SessionManager::flush_rosters()` (cheap; `tick`
  calls it too) at least every `roster_interval_ms`. The kit starts no thread and no timer;
  any call on a doc also delivers what is due. `capabilities` lists the two new limits.
- **App viewers see handles, never pids.** Stamp `ParticipantKind::App` at join for anyone
  who comes through an App; the kit does the rest. Match `Who` with a wildcard arm. A handle
  is meaningful only to the viewer that received it (pass it back on that viewer's own
  calls, as `log --by p2`).
- **New refusal:** `MPC-E305 watching` (exit 3) for an edit by a viewer past
  `participants_max`. New limit in `capabilities`: `participants_max` (32). Promote a watcher
  with `Session::promote_watcher()` when you want to: the kit never does it by itself.
- Tier 0 (blob docs) uses handles too: an App viewer's roster, leases (`LeaseInfo.pid`),
  reads, versions, log, blame, approvals and `Leased` refusals (`holder`) name others by
  handle. Participant overflow is text docs only for now.

**Changed**
- `ToolOutput::stderr_summary` ends with the id when the call was refused in-band: `nothing
  applied to n.md · v12 (MPC-E126)`. Nothing else is reworded.

**Build and test tooling** (the kit's own builds only: a host that vendors the kit builds with
its own profiles; release builds are byte-identical)
- `Cargo.toml`: dev builds with `debug = "line-tables-only"` and every dependency at
  `opt-level = 1`. With cargo-nextest (`.config/nextest.toml`: no retries), the suite's
  parts ran in 201 s against 387 s, cold build plus suite 384 s against 498 s, `target/`
  2.9 GB against 5.0 GB; the cold build alone is slower (183 s against 111 s). Median of 3 on
  one build node, `tools/README.md` ("Build settings").
- Node jobs run each part under nextest when the node has it (`--needs cargo-nextest`), else
  `cargo test`; build jobs and test threads follow the job's CPU quota; the job prints its
  memory peak.

## v0.9.6 (2026-10-29)

A maintenance release on v0.9.5 with **one behaviour fix** (an outside save that deleted a
line and put it back could leave the line twice). No API, wire, error or store format change
(still 5); v0.9.5's tests are unchanged, and the fix adds three.

**Fixed**
- **fix: an outside save that deleted a line and then put it back could leave the line twice
  (in v0.9.5); the delete is now applied.**
  - Two saves from a plain editor while the doc had edits not yet written back: the first
    deletes a line, the second types it back. When the doc's lines start alike (`p05:…`,
    `p10:…`), the live side's change was read as rewriting that line, so the delete was lost,
    and typing it back added a second copy.
  - The outside-save merge now diffs each side on whole lines (`text::diff::line_hunks`):
    a line both texts keep is never read as rewritten. Only the outside-save merge uses it;
    `text::diff::hunks` (`write --base`, undo) is unchanged.
  - Found by the soak (seed 1, call 176,147).

**Known issues**
- If someone rewrites a line live while an outside editor deletes it and types it back, both
  lines are kept (the live rewrite and the retyped line).
- An outside save merged with live edits can still put two people's changes on one line in rare
  cases; no text is lost (as in v0.9.5).

**Changed**
- **Documentation wording** for v0.9.5's hardening (the CHANGELOG, `NOTICE.md`,
  `docs/SESSIONS.md`, `docs/YRS-FORK.md`, `docs/ARCHITECTURE.md`, the `untrusted-wire`
  feature's comment, and code and test comments). The reserved error ids cite main's registry.
- **The vendored engine is updated to k2-yrs k2.2.4** (hardening; warning-free, no change in
  behaviour): `tools/vendor-yrs.sh`, `--check` passes; `docs/YRS-FORK.md`. The workspace
  builds under `RUSTFLAGS='-D warnings'`, with default features and every kit feature; the
  kit's own crates were already warning-free (clippy `-D warnings`, mutants too).
- **`tools/vendor-yrs.sh`:** `vendor/yrs` no longer carries the engine's own tests
  (`yrs/tests/`: they run in k2-yrs, the kit never builds a dependency's tests, and compute
  jobs never carried them); any tag, older ones included, re-vendors.
- **`tools/test-bundle.sh`: `INLINE`** (as on main) carries a part's base64 past `MAX_CHUNKS`
  inside the job's command argument: with k2.2.3's engine patch, `base-a`, `conformance` and
  `spec-undo` need 65–66 chunks (`INLINE=120000`; `tools/README.md`).
- **No internal build-host names in the tree:** neutral wording in the CHANGELOG, docs and
  tools (job ids kept), and `tools/check-private.sh` (from main), run by `scripts/check.sh`
  and the evidence part.

**Tests**
- `tests/disk.rs`: `an_outside_delete_and_retype_over_unwritten_edits_never_duplicates_a_line`
  (a session: live edits, two outside saves) and
  `an_outside_delete_of_a_line_the_live_side_kept_lands` (the merge); both fail on v0.9.5.
- `text::diff`: the property `line_hunks_round_trip` (valid, a round trip, deterministic,
  canonical).

## v0.9.5 (2026-10-29)

A second hotfix for the outside-edit bug of v0.9.4: **v0.9.4 narrowed the window to the write
delay; v0.9.5 closes it.** Also: fixes to echo detection and to the disk stat check (a
same-size save within the timestamp's resolution), hardening (client updates carrying
content the kit never uses are refused), and the soak now includes outside saves.
**For hosts:** an `AuditEvent` variant and a `SyncDrift` variant (both enums are
`#[non_exhaustive]`), two `DocError` variants, and `DocError` becomes `#[non_exhaustive]` (so this
is the last break of its kind; see Host notes), and public functions in `text::merge` and
`text::lines`. **No store format change** (still 5). The vendored engine is updated to
k2-yrs k2.2.2; no other dependency change.

**Fixed**
- **fix: an outside save after a byte-neutral change and an edit not yet written back could
  be corrupted (since v0.1.0).**
  - **Repro:** a plain text doc with lines `line 1` … `line 6`.
    - A participant deletes `line 3` and undoes it (the bytes are as they were).
    - Then they edit `line 6`, and that edit is not yet written back. The doc is dirty for
      up to the write delay (`write_quiet_ms`, 1 s; at most `write_max_delay_ms`, 5 s).
    - In that window, a person saves `line 3` → `LINE three` from a plain editor.
    - The doc and the file became `LINE three3`.
    - The same happens with the order changed: delete `line 3`, edit `line 6` (not written
      back), undo the delete. The doc never renders the synced bytes at a commit, yet
      `line 3` lives on new elements.
  - **Why v0.9.4 didn't close it:** an outside save was made on the **disk replica** (the
    doc as the file held it) and merged into the doc as a CRDT update. v0.9.4 re-synced the
    replica only when the doc rendered the synced bytes. A dirty doc never did, so the
    replica kept the text on elements the doc had deleted.
  - **Fix:** an outside save is now a **three-way merge applied to the live doc**, with no
    stale base in the loop.
    - Base = the text the file held at the last sync (only the replica's *text* is used,
      never its elements). Ours = the file's new text. Theirs = the live text, dirty or not.
    - The result is `text::merge::merge3_keep_both`. When clean, it is `merge3_hunks` with
      the pair rules of `write --base`: the save is applied as a `write --base` of the file
      made on the synced text.
    - Its hunks are on the live text and are applied to the live doc as they are, as
      `kit:external` (same attribution, versions and events as before).
    - Both sides are diffed from the base with pure line insertions slid to line starts (as
      outside edits always were) and pure line deletions slid to whole lines
      (`text::lines::align_deletions`, new).
  - **Overlapping changes keep today's behaviour, keep-both**, now at line granularity.
    - Before v0.9.5, a true concurrent outside edit (the save and an unwritten change touch
      the same text) was merged as a CRDT update. Both sides' insertions were kept, and what
      either side deleted was gone. It was never refused and made no conflict copy (those
      are tier 0's).
    - v0.9.5 keeps exactly that. Every hunk meeting an overlap is widened to whole lines
      (`outside_merge_sides`), then both sides' lines are kept: at one point, pure
      insertions before replacements, and the live side first within each.
    - The order is now deterministic. Before, it depended on client IDs.
    - Two sides' lines are never spliced into one. The soak's new outside saves showed that
      a character-level union could splice them (`p12:63ed:95`).
  - **The disk replica after a save:** when the doc holds the file's text, the replica *is*
    the doc. Otherwise the save is made on the replica as a client of its own, one the doc
    never holds, and the next write-back replaces the replica whole. Nothing is ever merged
    from the replica into the doc.
  - **The replica's staleness marker is now the change counter** (`synced_seq` against the
    doc's version; O(1), and stored with the sync state, so a restart keeps it).
    - v0.9.4 compared state vectors. A **delete-only** change leaves those equal. For text,
      such a change always moves bytes. In a structured doc, deleting an empty or absent cell
      moves none, and the stale replica stayed.
    - The state vectors remain a debug cross-check. That check found that the new ingest
      first compared the file with the pre-merge text; that was fixed before release.
- **fix: a same-size save within the file timestamp's resolution could be missed; on tier 0
  a write-back could overwrite it.** Text docs only left it un-ingested. (Git's "racy
  clean" problem.)
  - **Cause:** the kit took a file as unchanged when its size, mtime, inode and device
    equalled what it last saw, without reading it. A same-size save in place, made within
    the filesystem's mtime granularity of the kit's last look, leaves all four equal.
  - **Repro (tier 0):** a blob doc synced with its file; a person saves same-size bytes in
    place in the same tick; an agent's version then arrives. The write-back's stat-only
    pre-check saw "unchanged" and wrote over the person's save, with no conflict copy.
  - **On text docs** nothing was lost: a write-back, and `file_changed`, always read first.
    But the poll (`tick`, `check_disk`) left the save un-ingested until the file changed
    again. The conflict-copy cleanup kept an edited-back copy for the same reason.
  - **Fix:** a stat taken within 2 s of its own mtime (or ahead of it, or with none) is
    **racy**. An equal racy stat proves nothing, so the file is read and hashed; the mark
    clears at the first look after mtime + 2 s.
    - This covers text docs, tier 0 (poll, open, write-back pre-check) and the
      conflict-copy cleanup.
    - The cost is at most about 2 s of re-hashing after each sync.
    - 2 s covers every common filesystem's granularity (ext4/XFS a kernel tick, APFS
      nanoseconds, HFS+ 1 s, FAT/exFAT 2 s).
    - Tier 0 stores only a stat that can vouch for the content (a racy one is stored as
      none, so the next open hashes the file). The record format is unchanged.
    - **Residual:** a racy stat stored by v0.9.4 or earlier can still be trusted once per
      doc, at the first open after the upgrade; from then on only vouching stats are kept.
  - Tests: deterministic racy cases in `tests/disk.rs` and `tests/tier0.rs`.
- **fix: an outside save's merge could splice two sides' lines at the end of a text with no
  final newline.** Where the live and the file side's groups of whole lines met at one point
  and the first didn't end with "\n", the second was joined onto its line: base "a\nb",
  file "a\nB1", live "a\nB2" gave "a\nB2B1". Nothing was lost. Now one "\n" keeps them
  apart ("a\nB2\nB1"), in either order, always (a group starting with "\n" keeps its own
  blank line: base "", file "\n\n", live "aaaaa" gives "aaaaa\n\n\n"). This also covers two
  appends at such an end (O5).
  - The contract: clean = merge3_hunks, except that, in an outside save, a line break
    separates text one side left without a final line break (when the original text ended
    with one) from a change of the other side that is inserted at the end of the text. For
    example, base "\n", file "x\ny", live "" gives "x\ny", where the live side's delete to
    the end had joined the file's lines. Everything else on the last line merges character
    by character, as merge3_hunks does:
    - a change that starts earlier and runs to the end (base "\n", file "b\n", live "aaa"
      gives "baaa"; base "foo\n", file "Xfoo\n", live "foo bar" gives "Xfoo bar");
    - a base with no final break (base "b", file "xb", live "c" gives "xc").
    `write --base` is unchanged.
  - Tests: the cases as unit tests, and the property
    `no_line_of_the_merge_is_spliced_from_two_sides` (every line of the result is a line of
    a side).
  - One side removing the final newline while the other appends a line now gives separate
    lines (base "a\n", file "a", live "a\ng" gives "a\ng"; a character merge joined them).
    An append to an open last line by one side alone still stays on that line.
- **fix: echo detection discarded a person's deliberate save.**
  - A file holding one of the kit's own recent writes (the last 8, up to 60 s) was always
    taken as an echo, even when a person had saved in between.
  - So a person who saved, then saved again the bytes of an earlier kit write (an undo, or
    retyping what they had deleted), had that second save discarded, and the next
    write-back overwrote it.
  - Now a recent write counts as an echo only if no outside save was taken in since it.
    Every outside save taken in forgets the set. This applies to text docs and tier 0.
  - Every suppressed echo is reported once, as `AuditEvent::EchoSuppressed { key }`, never
    silently.
  - A true stale buffer (the kit writes W1, then W2, and an editor saves W1) is still an
    echo.

**Hardening**
- **Client updates carrying content the kit never uses (subdocuments) are refused, and
  stored content of that kind is quarantined.**
  - Every ingress that takes update bytes from outside the kit decodes them one way (v1
    only) and refuses such an update before anything is applied: `Doc::check_client_update`
    and `Doc::apply_client_update` (`untrusted-wire`), and the test-only in-process
    delivery.
  - The update checked is the update applied (decoded once).
  - The refusal is `DocError::Unaccepted`, "update contains content this host doesn't
    accept", exit 3. Its error id is reserved (`doc::RESERVED_ERROR_ID_UNACCEPTED`) until
    the error-surface registry assigns it (MPC-E152 on main's registry).
  - **Store-load scan:** a stored doc holding such content (a stored state, or a log entry
    replayed at load) is quarantined, never loaded as live: `DocError::Quarantined`, exit 3,
    its error id reserved as well (MPC-E518 on main's registry). Expected never to fire.
  - The check is the fork's `Update::has_subdoc_content()` (decode-only).
- **The vendored engine is updated to k2-yrs k2.2.2** (`tools/vendor-yrs.sh`, `--check`
  passes; `docs/YRS-FORK.md`).

**Added**
- **The lost-edit audit:** `SyncDrift::Pending { replica }`, in any disk state, when the doc or
  the disk replica holds an update it couldn't integrate (missing dependencies) or is poisoned.
- **`text::merge::merge3_keep_both`, `outside_sides`, `outside_merge_sides`;
  `text::lines::align_deletions`.**
- **The soak replays and diagnoses** (from v0.9.6's tooling): `SOAK_CLIENT_SEED` (exact
  replay), `SOAK_MAX_OPS`, `SOAK_DUMP_AT_OP`, `SOAK_TRACE_KEEP`; a failing call dumps the last
  calls, the engine's text against the cached one, the participant's undo and redo stacks
  and the blame of any line written twice. `SOAK_OUTSIDE=0` turns the plain editor off.
- **The soak includes outside saves.** A plain-editor participant (1 call in 40) reads the
  file and saves it, often inside the write delay while the doc is dirty.
  - The edits: a line added or deleted; byte-neutral pairs of saves (a line added and taken
    out again, a line deleted and typed back); and a stale buffer of an earlier kit write,
    which must be suppressed as an echo.
  - Each save is checked by the soak's line oracle: exactly its lines gone or there, and
    nothing else lost.
  - The sync check runs after every save.

**Tests**
- `tests/disk.rs`:
  - the two repros, `an_outside_edit_after_a_no_byte_change_and_an_unwritten_edit_is_kept`
    and `an_outside_edit_after_an_undo_behind_an_unwritten_edit_is_kept` (with a restart);
  - `an_overlapping_outside_save_keeps_both_sides`, which pins the overlap behaviour;
  - `an_outside_save_of_identical_text_touches_nothing`: the same bytes (in place or by
    rename) and the same text in other line endings change no element (blame unchanged),
    commit no change (no version, log entry, revert record or undo region) and send no
    event, with and without an unwritten change;
  - `the_disk_replica_follows_the_change_counter_across_restarts`;
  - the echo tests, `a_save_that_recreates_a_kit_write_from_before_an_outside_save_is_kept`
    and `a_stale_buffer_of_an_older_kit_write_is_still_an_echo`.
- The property **`an_outside_save_amid_any_mix_of_changes_is_merged`**:
  - It runs a random mix of byte-neutral and real changes, some written back and some not,
    with waits inside and past the write delay, flushes, restarts, and outside saves (with
    the doc open, or saved while it was closed).
  - At each save it asserts:
    1. the result is `merge3_hunks`'s when the merge is clean;
    2. and 3. the doc is the keep-both model, so no outside change and no unwritten change
       is lost;
    4. no update is left pending (the ingest succeeds, the sync check has no `Pending`, and a
       reopen from the store gives the same text).
  - It runs 256 cases by default (`NO_BYTE_CASES`). It ran 10^4 on nodes, together with
    v0.9.4's no-byte property at 10^4: four shards of 2,500 each, `PROPTEST_RNG_SEED` 1–4,
    on k2.2.2 (kit `b4d6db7`): `ad9e3ff3`, `296531ee`,
    `74f39969` and `16fc34ca`.
  - The two repros, the overlap test and this property fail on the v0.9.4 ingest.
- `text::merge`: keep-both is the character model, it is merge3's result on hunk sets that
  don't overlap, and what still overlaps is whole lines (property). `text::lines`:
  `align_deletions` (example and property).
- `tests/tier0.rs`: the echo rule for blob docs.
- `tests/subdoc_ingress.rs`, built through Yrs's normal API:
  - a client update carrying a subdocument (in a map, beside ordinary text, nested) is
    refused unapplied, nothing is pending, and the doc stays usable;
  - every ordinary update kind is still accepted;
  - a stored doc holding one is quarantined at load, as a log entry and as a compacted
    state.
  - The engine's unit test `stored_subdocument_content_is_quarantined` covers the same.
- `tests/pending_retry.rs`:
  - Client updates delivered out of order (A:1 before A:0, B:0 depending on A:0) are
    refused (`MissingDeps`) or poison the doc loudly (`Pending`). An update is never held
    pending silently.
  - A resync by state vector lands every edit, and nothing is left pending.
  - It pins the kit's own guards and needs no engine change.
- Changed expectations:
  - `tests/outside_edit_undo.rs`: the merge order is now deterministic (the live line
    first, for every client seed).
  - `session/one_writer_tests.rs`, check 3: the replica may hold only its own clients
    beyond the doc.
- The suite on nodes, every part passes on k2-yrs k2.2.2 (kit `b4d6db7`): base-a `f1d826af`,
  base-b `9a84b940`, conformance `df49f642`, spec-undo `89d06961` (mutants too), evidence
  `2bbb7b25`; base-c `c56b40eb` at `7485a20` (the later commits change
  only the soak and its job).
- The soak on nodes (kit `7485a20`; release, 600 s, `SOAK_CLIENT_SEED=1`,
  `SOAK_MAX_SLOWDOWN=1000`), with outside saves. Each job also ran the CI soak and the
  replay regression; all pass.
  - seed 1: `00dff865` (2,529 outside saves; 1 known D1 violation, call 52,156);
  - seed 3: `792a65f2` (2,398; 1, call 25,877);
  - seed 4: `8f4fe623` (0);
  - seed 5: `7828f71f` (2,490; 0);
  - seed 1 with the editor off: `0366a000` (1, call 55,516).
  - Seed 2 (editor on, before the D1 mode, k2.2.1) ran invariant-clean: `4dd41374`.
- fmt, clippy (`-D warnings`, mutants too) and the licence check pass locally.
- `tools/tests/full-paths.sh`: the conformance part ships without the kit's `differential`
  module (feature-gated). It was 65 chunks, one over a job's limit.

**Host notes**
- **Rolling back onto a newer kit's store:** a store holding a table this kit doesn't know
  (v0.9.7's `authored`, added without a format bump) refuses a purge or a rewrite, before
  anything changes, with `StoreError::NewerData { table }`. In the error registry this is
  `MPC-E502` `store_newer`, exit 6. The message: "This store holds data from a newer kit
  (table "<name>"). Upgrade K2 to a version with kit ≥ v0.9.7 to purge or rewrite it;
  reading and editing still work." Any other unknown table says "a newer kit" with no
  version. Before, the rewrite failed with a bare store error, after the purge had already
  removed its rows. Tests: `store::tests` (both branches) and `tests/erasure.rs`.
- **Match `DocError` with a wildcard arm.** It gains `Unaccepted` and `Quarantined` and is now
  `#[non_exhaustive]`, so an exhaustive match stops compiling once, and never again for a new
  variant. No pack needed a change.
- Optionally, log `AuditEvent::EchoSuppressed`. The file came back holding a recent write of
  the kit's and was not taken as an edit.

**Known issues**
- **The D1 gap (amendment approved; fixed in v0.9.7).** A character diff can build a change's new
  text from characters already there, another participant's included.
  - Undoing such a change can change text someone else wrote. For example, p18 changes
    `1` to `4` in its line; p15 rewrites the line and the diff keeps that `4`; p18's undo
    puts its `1` back inside p15's line.
  - A D1 undo can also rebuild another participant's text from their characters: in the
    soak's seed 3, an undo rebuilt `p16:347` out of p16's `p16:341` before p16 had written
    that token.
  - Present since the character diff (v0.9.4 too); unchanged by v0.9.5.
  - The soak runs with the known-D1 mode, `SOAK_KNOWN_D1` (default 1 in v0.9.5; 0 fails
    them). A violation of the undo promise that matches exactly this signature (an undone
    or redone batch with a region that isn't whole lines) is counted and printed with its
    call number. Every soak call writes whole lines, so such a region means reused
    characters. Every other invariant fails as before. The report ends with
    `known D1 violations: N`.
  - The soak's tokens never take a text it has already recorded, so a rebuilt text can't
    later read as a line written twice. That set is the soak's own record, never a read of
    the doc.
  - The regression `soak_replay_seed_3_known_d1` (run by `tools/tests/soak.sh`) replays
    seed 3 for 27,000 calls. It must see exactly one known violation (call 25,877) and no
    other failure.
  - v0.9.7, which closes the gap, removes the mode.
- **Throughput:** the soak's 2× throughput check is v0.9.6's (performance) gate. v0.9.5's
  correctness soaks run with `SOAK_MAX_SLOWDOWN=1000`. The 600 s seed-2 run fell 9.8× from
  its first window to its last (`4dd41374`), recorded for information.

**Open**
- **Grids:** the same three-way design applies to the keyed reconcile. It is
  not in v0.9.x, which has no grid docs.

## v0.9.4 (2026-10-29)

A hotfix for one correctness bug, plus the audit check that would have caught it.
**Additive** for hosts: one `Session` method and one enum (`#[non_exhaustive]`). **No store
format change** (still 5) and no dependency change.

**Fixed**
- **fix: an outside edit after a byte-neutral change could be corrupted (since v0.1.0).**
  The bug came in with disk sync (69a17ca) and is in every release since v0.1.0.
  - **Repro:** a plain text doc with lines `line 1` … `line 6`. A participant makes a change
    that leaves the bytes as they were, for example deleting `line 3` and undoing it,
    deleting it and typing it again, or rewriting it and back. Then a write-back (or a
    flush) finds nothing to write. A person then changes `line 3` to `LINE three` in a
    plain editor. The doc and the file became `LINE three3`, `line 3\nLINE three4` or
    `LINEline 3three`. The doc reported clean, so comparing the file with the doc couldn't
    see it.
  - **Cause:** an outside edit is made on the **disk replica** (the doc as the file holds
    it) and merged into the live doc. A change that moves no byte puts the text back on
    **new** elements, and the doc deletes the old ones. The write-back found the bytes
    unchanged and returned early, so the replica kept the text on the old elements. When
    the outside edit was merged, its deletes hit elements the doc had already deleted, so
    they did nothing, and its inserts landed beside the restored copy.
  - **Fix:** whenever the doc renders to the synced bytes, the replica is the doc itself:
    the same elements, not only the same text.
    - The marker is the two state vectors, the replica's and the doc's. They are equal at
      every sync. The replica only ever holds what the doc holds, so a later change moves
      them apart exactly when it inserted. A change that only deletes can't leave the
      bytes as they were.
    - Cost: O(clients) when nothing changed. When something did, one full encode and load,
      stored as the synced state (the write-back's own `set_pending_sync` and
      `promote_pending_sync`), so a restart can't bring the old replica back.
    - Where it runs: in the write-back's no-write path, after an outside edit leaves the
      doc clean, before an outside edit is merged (so a change not yet written back is
      covered too), when a doc is opened (this heals stores written before v0.9.4), and in
      a purge when the file doesn't hold the live text.
  - Every other path where the doc changes was checked for the same stale replica: undo,
    redo, restore, revert, outside edits, store recovery, purge, compaction, rights
    changes, renames, unload, import and promotion from tier 0. None needs more than the
    above.

**Added**
- **The sync check:** `Session::audit_disk_sync() -> Vec<SyncDrift>`. It runs when the
  doc is `Clean`, and checks that the disk replica holds the doc's text **on the doc's
  elements**.
  - It returns `SyncDrift::Text { root }` or `SyncDrift::Elements { root }` for each root
    that differs (metadata only). It is empty otherwise, and when the session has no file
    or isn't clean.
  - It is read-only and O(doc): one blame of each root on the doc and on the replica.
    Measured on a build node (release; a 5,000-line, 344 KB doc with 1,001 changes; job
    `9ca0ff5c`): **6.5 ms per audit**.
  - Measured in the same job, a write-back that finds nothing to write: 4.5 ms with
    nothing changed, and 8.4 ms after a change that moved no byte (the re-sync's encode
    and load).
  - Meant for a nightly audit or a test, not the edit path. The disk simulation and the
    soak now run it after every flush.

**Tests**
- `tests/disk.rs`:
  - `an_outside_edit_after_a_change_that_moved_no_byte_is_kept` (the repro: four changes,
    with and without a flush);
  - `the_sync_audit_reports_a_stale_or_wrong_disk_replica` (planted violations through the
    test-only hook `Session::plant_disk_replica_for_tests`, `differential` feature);
  - `an_outside_edit_after_any_change_that_moved_no_byte_is_kept`, a property test.
    - It makes a random change that moves no byte: an edit and its undo, a delete and its
      undo, a delete and a retype, a rewrite and back, a revert, or a restore.
    - Then nothing, a tick, a flush, a restart, or a flush and a restart.
    - Then a random outside edit. The doc and the file must both equal the outside edit
      applied to the text before the change, and the audit must find nothing.
    - It runs 256 cases by default (`NO_BYTE_CASES`). It ran 10^4 on nodes: four shards of
      2,500 with `PROPTEST_RNG_SEED` 1–4 (`9ca0ff5c`, `b6cdeb82`,
      `f6b6d9b0`, `d91dbe21`).
  - Without the fix, all three new tests fail.
  - `audit_cost` (ignored) measures the audit and the re-sync.
- The suite on nodes, all six parts pass: base-a `6d56f8db`, base-b `07b954a0`,
  base-c `96b5034e`, conformance `1a2e24c1`, spec-undo `81e3cb1e`, evidence `c21c64ae`.
- fmt, clippy (`-D warnings`, mutants too) and the licence check pass locally.
- `tools/tests/full-paths.sh`: the spec-undo part now ships only `spec/quint/undo_diff.qnt`,
  the one file its tests read. The whole folder put the part over a job's limit (65
  chunks).

**For hosts**
- Nothing to do. A doc whose stored replica predates the fix is healed the next time it is
  opened.
- Optionally, run `audit_disk_sync()` in a nightly audit. Any drift it reports is a bug:
  file it with the event.

**Open**
- **The grid twin of the property test** turns a header off and on, and sets a cell back to
  its value. It lands with the grid work, which brings grid docs to the kit; v0.9.4 has none.
- **Not this bug:** an outside edit that arrives while a participant's changes are still
  unwritten, where those changes deleted and retyped the same text along with other edits
  (so the bytes did change). That is a true concurrent edit, and the merge keeps both
  inserts, as any CRDT merge does.

## v0.9.3 (2026-10-29)

A patch: **agent edits are made on the doc itself** (no replica), with the engine vendored and
patched, a sampled runtime check against the old path, and the evidence for it. **Additive**
for hosts: a `Config` field, two `AuditEvent` variants (the enum is `#[non_exhaustive]`), two
`Doc` methods and a constant. **No store format change** (still 5). The engine is now a
path dependency (`vendor/yrs`: Yrs v0.28.0 plus the k2 patches, MIT); no other dependency
changes. Evidence: `docs/evidence/v0.9.3-edit-path.md` (every figure with its node, settings
and run record).

**Changed**
- **Each edit is made on the doc itself, as the participant's client** (ARCHITECTURE rule 6,
  new wording). `Doc::edit_as` makes the change in one `transact_mut_as` transaction on the
  doc (the k2-yrs patch: `docs/YRS-FORK.md`). Until v0.9.2 it went through a throwaway
  replica of the whole doc, so every edit, undo, redo and revert cost O(history).
  - Per edit, measured on a build node (release, warm, the `Doc` API alone; job `5d15b747`):
    1.2k changes 0.39 ms → 0.010 ms; 40k changes 12.0 ms → 0.18 ms; 300k changes 110 ms →
    2.0 ms.
  - The new path still grows with the items before the edit's position (the engine walks
    to it), far more slowly. These are measured figures, not a complexity class.
  - Memory: nothing per participant, as before; no replica at all now.
  - Results are byte-identical to the old path: the differential test (90,020 agent edits at
    doc level, 4,452 engine edits at session level), the soak's shadow mode (every edit of a
    120 s run) and k2-yrs's property test (~571k plain ops; the evidence note).
  - The doc is the only writer of every client ID it edits as (held exclusively while it
    edits; it holds everything that client wrote): `session/one_writer_tests.rs`.
- **The engine is vendored:** `vendor/yrs` at `k2-yrs-0.28.0-k2.2`, copied and checked by
  `tools/vendor-yrs.sh` (`vendor/yrs/VENDORED`). `NOTICE.md` carries Yrs's MIT notice and
  the fork's changes; the licence check now covers it.

**Added**
- **The edit path's runtime check (the canary).** `Config::edit_check_every` (default 1 in
  1,000; 0 is never; `Doc::set_edit_check_every`, `DEFAULT_EDIT_CHECK_EVERY`).
  - A sampled edit checks the engine's text cache against a fresh read, before and after it.
  - It is also made the old way, a throwaway replica from the state before it, and the two
    updates are compared byte for byte.
  - A mismatch is never served: the doc takes the replica's update, the session reports
    `AuditEvent::EditPathMismatch { key, client, change }` (or `TextCacheMismatch { key,
    root, change }`: metadata only), and the doc makes its edits through the replica (or
    reads its texts afresh) until it is reloaded. `Doc::edit_path_fallback()` says so.
  - Cost at the default rate (5,000 edits each timed alone): p99 −8% / +0.5% /
    +3.2% at 1.2k / 40k / 300k changes, mean +13% / +6% / +7%. A sampled edit costs what
    every edit cost before (it shows at p99.9).

**For hosts**
- Nothing to do. `Config::edit_check_every` defaults to 1 in 1,000. On
  `AuditEvent::EditPathMismatch` or `TextCacheMismatch`, file a bug with the event: the
  results stay correct, but that doc takes the slow path until reloaded.
- The kit now builds the engine from `vendor/yrs` (a path dependency inside the kit's tree):
  vendoring the kit by subtree brings it along; no registry access needed for yrs.

**Open**
- **The soak's 2× check fails** (above). Planned for v0.9.4, gated on the soak's 2× check:
  - a position index in the engine (Yjs's "search markers": Yrs has none), a k2-yrs patch;
  - the disk replica kept in step by the changes since the last write-back, instead of a
    whole re-encode and reload (the kit);
  - blame's throwaway replica.
- **The canary's p99 at 300k changes:** +3–8% across runs, from the edits around a sampled one.
- **The structured kinds' branch:** when it is rebased on v0.9.3, its edit block moves to the doc
  itself, with the canary and a differential test for the structured kinds.

**Fixed**
- **An undo after an outside edit could conflict on text nobody had touched** (found by the
  one-writer workload; `tests/outside_edit_undo.rs`). An outside edit is merged into the live
  doc by the engine, and its change was reported to undo regions (and to subscribers) as a
  diff of the live text before and after. In repeated text that diff can name other
  elements than the engine inserted: when the merge put a copy of a participant's line just
  before it, the diff reported it after, and the participant's region was carried onto the
  copy. The release build then refused that participant's undo (`Overlap`) where the spec's
  oracle applies it (`crates/spec-undo/tests/outside_edit.rs`); the undo merge's debug
  cross-check (element identity against by root) is what caught it. The change is now
  reported **at element identity**, as SPEC-UNDO's preamble requires: from each element's
  ID before and after the merge (one blame of each edited root before and after, O(doc),
  on the outside-edit path only). Undo never removed another participant's text here
  (by root); a region with unknown roots (a revert record from before store format 5)
  could have, and no longer can.

**The soak (the 2× check still fails)**
- **Result:** 10 minutes on a build node (`a5d53680`): 193,214 calls, 2.7× v0.9.2's; version
  100,194. Calls/s 1,363 in the first window, 154 in the last 150 s: 8.85× slower (the check allows
  2×).
- **Why:** a profile shows the replica's cost is gone. Two whole-history costs remain on the
  path:
  - **The engine's own position lookup.** Yrs walks a text root's items from the start,
    deleted ones included, to find an index: 4.4 ms per applied change at 78k changes.
  - **The write-back to the file.** It re-encodes the whole doc and reloads the disk replica
    from it: 65 ms at 1.26 MB.
- At equal history the new path is 55–68× faster per edit (40k–300k changes). v0.9.3 does
  **not** claim the soak condition; see "Open".

**CSV `cell set` at 100k cells** (the measurement of the structured kinds on their side branch;
the same node for both): p99 185.1 ms → **1.3 ms**, peak extra heap per edit 57 MiB →
0.8 MiB. These are from a scratch merge of that branch with this release's engine and
edit path; the merged code will re-run it.

**Tests and tools**
- **The differential test** (`tests/edit_paths.rs`; test-only `differential` feature): one
  seeded sequence through the new path and the throwaway replica, compared at every step.
  - Doc level: agent multi-hunk edits on two roots, people's concurrent updates, reloads.
  - Session level: edits, `write --base`, undo, redo, revert, restore, versions, purge,
    outside edits, flushes, ticks, leaves, restarts; every outcome, update, state, blame and
    log compared.
  - Both drivers catch two planted wrong paths. The soak's `SOAK_SHADOW=1` checks every edit
    of a run.
- **`tests/edit_cost.rs`** measures 1.2k / 40k / 300k changes on both paths, and the
  canary's cost.
  - Its history is built on plain engine docs, checked byte-identical to the kit's at 1.2k.
- **The canary's tests** (`tests/edit_check.rs`, `session/engine_check_tests.rs`): a planted
  divergence (test builds only), caught and never served; the control serves it unchecked.
- **The one-writer invariant** (`session/one_writer_tests.rs`): why it matters, the checks'
  negative controls, and the checks after every call of a random file-backed workload.
- **Evidence records** (`tools/evidence/`, K2 Engine's, as is): the kit's runs print one
  `RUNREC` line each (`undo_enum`, `undo_itf_replay`, `edit_cost`, `edit_paths`, the
  soak); `docs/evidence/runs/` holds them, and every count in `docs/evidence/` cites its run
  (`tools/evidence/check.py`, in `scripts/check.sh` and the node suite's base-a).
  - `undo_enum` and `undo_itf_replay` now print `SUMMARY:` (with a colon), which the collector
    never parses as another enumerator's summary.
- **The undo export guard's check** runs against an Apalache server with `tlc.sh`'s heap, and
  a tool error is retried (quint's compile fails at random on its own server).
- **`undo_enum`: a panic in the oracle is a violation** (`panic@<depth>` with its message,
  `panic="..."`), and every mode exits promptly. A walker's panic used to leave the process
  running until the job's timeout (job `7fc87257`). The harness mutant
  `harness-oracle-panics` tests every mode. Judged without a mutant (`9bfb3b47`): 300k walks at
  the full bound and 300k with a restart, no violation, every walk ran to its end. Under mutant (iv)
  `window.rs`'s shared-edge assertion now ends the run at once (`panic@5`).
- **Node jobs:** the kit plus yrs is over a job's bundle, so `tools/test-bundle.sh` ships
  `vendor/yrs` without `src/`. The job rebuilds it from the public crate (sha256 checked) and
  `vendor/yrs/K2.patch`, checked file by file.
  - The suite's parts other than base-a leave out the kit's `cfg(test)` module files.
  - New part `base-c`.
  - A part at 64 chunks takes its `PART` in the commands file.
- Tests use neutral names.
- **The Quint export guard** (`tools/quint/`): every undo model-check job lints `undo.qnt`
  at its start and checks the model's TLA+ export against its Quint source before any TLC run
  (a guard against a Quint→TLA+ export issue); a finding stops the job (`tools/README.md`).

## v0.9.2 (2026-10-28)

A patch: the v0.9.1 soak's follow-ups (claims at a shared edge; why throughput falls with
history), tier 0 → text promotion keeping history, and `approvals --all`. **Additive** for
hosts: new calls, a new `AuditEvent::Promoted` and `NotReached::Tier0History` variant (both
enums are `#[non_exhaustive]`), a new `LogQuery` field (`#[non_exhaustive]`). **No store
format change** (still 5). No new dependency. `docs/SESSIONS.md`, amendment 16.

**Added**
- **Tier 0 → text promotion keeping history.** `SessionManager::promote_blob_file(pack, &key,
  path, root) -> (Arc<Session>, Promotion)` (`promote_blob` without a file) turns a blob doc
  that now reads as text into a text doc with the **same key and doc id**, in **one store
  transaction** (a crash before the commit leaves the blob doc exactly as it was). Each blob
  version that decodes becomes a text version: one change per version whose text differs,
  by its original author at its original time (`via`, `on_behalf_of`, name kept), its
  approval carried in the state it was in. Versions that don't decode, purged versions and an
  on-behalf version with no text change stay as the doc's **tier-0 history** (same ids,
  bytes, approvals, purge state): `Session::tier0_log`, `read_tier0_version`,
  `set_tier0_approval`, `purge_tier0`. Nothing purged is re-imported; a text purge of the
  history purges the kept tier-0 versions too. `Promotion { key, versions: Vec<(VersionId,
  Promoted)>, changes, blobs_removed, connections }`; `Promoted::{Text, NonText, Purged}`;
  `AuditEvent::Promoted`. Errors change nothing: `NotFound`, `PackMismatch` (not a blob
  doc), `Leased`, `Declined` (the current version doesn't decode), `Disk`, `NoBlobStore`.
- **`approvals --all`** (= `log --pending --all`): every approval record, settled ones too
  (approved, declined), newest first. `approvals` alone stays pending-only (the pending-approvals
  row).
  `LogQuery::settled` / `LogQuery::with_settled()`; tier 0: the log's entries with an
  `approval`, newest first. The pending-approvals row gains a case for each name, both tiers.
- **`tests/edit_cost.rs`** (ignored): an agent edit's cost against the doc's history, through
  the public `Doc` API.

**Fixed**
- **Two participants' claims could come to overlap** (the v0.9.1 soak's finding). Text typed
  where two claims meet (the text between them was deleted, or one claim's text was) now
  joins **one** claim: the typist's own if they hold one of them, otherwise the claim
  **acquired first**. The other carries the text outside it and isn't touched by it (no
  `Edited` end, no `Respect` refusal). One participant's own claims there all take it, as
  before. The soak now asserts claims never overlap.
- **Reading a text root walked the engine's whole history.** The engine adapter keeps each
  root's text as last read or written, so an agent edit no longer walks every item of the
  doc, deleted ones included, twice (on the Mac: 0.3 → 1.2 ms of each call over 4 minutes,
  now none; on a Linux node a few per cent). One copy of the text, not of the doc.

**Throughput and history (the v0.9.1 soak: ≈ 250 → 43 calls/s over 10 minutes)**
- **Root cause: the per-verb agent replica.** Every edit (an agent's, a person's through the
  kit, an undo) is made on a throwaway engine replica built from the doc's **full** engine
  state, then dropped: the ephemeral-replica rule, kept on purpose so memory is never the doc
  times its agents (`tests/ephemeral_replicas.rs`). The engine state keeps an item for every
  insertion ever made, deleted ones included (GC drops their content, not their IDs and
  lengths), so under churn it grows with the number of changes while the text stays the same
  size, and each edit's replica with it. **Linear in history, not a cache effect**: measured
  without session, store or fsync (`tests/edit_cost.rs`): 105 µs per edit at 1,200
  changes, 969 at 10,200, 4,837 at 40,200 (state 8 → 83 KB, text constant). In the soak, the
  replica build is 46% of a call at 100 s and 82% at 270 s; the store commit stays flat
  (≈ 2 ms on the Mac, its fsync); the kit's own session code stays under 0.3 ms.
- **The bound:** none from the kit. Retention, purge and compaction remove history and
  content, not the engine's items. Memory is bounded by the pool (`memory_budget_bytes`
  unloads idle docs); an agent edit's latency grows ≈ 0.1–0.9 µs per earlier change
  (machine and history shape), so a doc with a few hundred thousand changes reaches the
  150 ms agent-edit budget.
- **Removing it needs an engine decision** (see "Open"): making an edit as any client on the
  doc itself, without a replica.
- **The 10-minute soak, before and after, on one node** (seed 1, 600 s, release):
  v0.9.1 (job `8b6111ac`): 68,981 calls, 495 calls/s in the first 30 s, 60 in the last
  150 s (8.25×), an insert 3.2 → 33.2 ms; v0.9.2 (job `503bab46`): 72,376 calls (35,749
  changes, v37,556), 539 → 61 calls/s (8.89×, with 5% more changes), an insert 2.9 →
  31.7 ms. Joins (a store commit, no replica) stay flat at ≈ 0.1 ms. The throughput check
  fails (most 2×): the growth is the replica's. Everything else held: 25 restarts each equal
  to live, 290 blame and file checks, claims never overlapped (255 calls after which two
  shared an edge), peak RSS 61 MB, peak kit estimate 26.6 MB of 256 MB.

**For hosts**
- **Promotion:** when `open_file` with a text pack returns `PackMismatch { stored:
  "kit:blob" }` and the file now decodes, call `promote_blob_file` (check rights yourself,
  as for `delete_doc`), then send the tabs named in `AuditEvent::Promoted::connections` to
  the text doc. Offer the tier-0 history (`tier0_log`...) beside the text log for a promoted
  doc. A v0.9.1 build ignores a promoted doc's tier-0 history (and still deletes it fully).
- **`approvals --all`:** bind it to `log(LogQuery::default().with_settled())`; keep
  `approvals` pending-only.
- **Claims:** nothing to change; a claim's range at a shared edge may now stop short of text
  another participant typed there.
- **No migration:** store format 5 as before.

**Tests and tools**
- The soak prints, per progress window, its call rate, where the loop's time went (the kit
  call apart from the harness's checks) and the mean cost of each kind of call, and asserts
  the first window's rate is at most `SOAK_MAX_SLOWDOWN` (default 2) times the last 150 s's.
- **Undo spec suite:** `concurrent_restore`'s three parts are case classes of their own in
  `undo_enum` (`bfs`, `walk`: progress and `SUMMARY` lines) and the trace test:
  `concurrent_restore_o4` (O4 took a restoration once, by root), `concurrent_restore_o4a`
  (O4a fired), `concurrent_restore_base_by_root`; the total stays. Checked hit in the CI
  trace run (256 traces: 72 / 12 / 102), a tiny `bfs` and a short walk of the binary
  (`tests/walk_classes.rs`). No behaviour change.
- The full suite on nodes: base-a 345 tests, base-b 65, conformance 96, spec-undo
  41 and its mutants: all pass. `tools/tests/full.sh`'s base-b part now also carries
  promotion, both parity harnesses and erasure (base-a was over a job's variable limit).
- `tests/promote.rs` and a simulated crash mid-promote (`session/promote_crash_tests.rs`);
  shared-edge claim cases in `tests/presence.rs` (a claimant, a third agent, a person, an
  emptied claim, acquire order, the policy).

**Open**
- The soak's throughput check (2×) fails at 10 minutes while the per-verb replica stays
  (8.89× at this release): see the numbers above. Options: a client-per-transaction patch in the
  engine (no replica at all), or a bounded pool of warm replicas (the rule today says no).
- Imported text versions fall under text retention by their original times (blob versions
  were never thinned).
- The tier-0 history calls on a text session have no agent verbs yet.

## v0.9.1 (2026-10-28)

A patch: small asks from a host's integration and the Markdown demo, the 32-participant soak, and
the undo spec v1.9. **Additive**: no field added to a public struct, no new enum variant, no
store format change (still 5), no new dependency. `docs/SESSIONS.md`, amendment 15.

**The promise, corrected.** v0.9.0's wording said a conflicted undo applies what it cleanly can; a
plain undo applies nothing (exit 1), and only `--partial` applies the clean parts. The promise, as
it stands:

Undo, redo and revert only ever remove text that the change they reverse put there. Your undo never
removes text someone else wrote, even if it is identical to yours, and someone else's undo never
stands in the way of yours. Text put back by an undo or redo keeps its original author: blame shows
who wrote it and who restored it, and later undos and reverts treat it as the original author's. If
someone else has since changed the same lines, a plain undo applies nothing and reports the
conflict; with --partial it applies the parts it cleanly can and reports the rest. It never
overwrites their change. Nothing is lost: undone text stays in history and can be restored. This
holds across restarts.

**Added**
- **`Session::edit_with_options(conn, root, &EditOptions, f)`**: `edit_with` with the claim
  policy and a version guard, **both checked under the session lock**, as `run_tool_with`
  checks them for edit verbs. `EditOptions::new().policy(p).if_version(v)`
  (`#[non_exhaustive]`; defaults `Ignore`, no guard). A doc no longer at V gives
  `EditWithError::Session(SessionError::Stale { expected, actual })` with nothing applied;
  the order is admission, version, approvals backlog, `f`, claims.
- **`VerbSpec::runs_through_run_tool()`**: whether a host routes a verb to `run_tool`. Data
  (`parity::RUN_TOOL_VERBS`: the kit's text tools; every pack verb is one), held to
  `Session::run_tool` by a test both ways (each flagged verb runs there, every other
  registered verb is refused there with nothing applied).
- **`approvals [--on-behalf-of me|P]`**: a host-neutral alias of `log --pending` (the
  pending-approvals row), both tiers, the same row and binding (`parity::ALIASES`,
  `VerbSpec::alias_of()`). The kit's parity harnesses run a case for each name. A pack's parity list
  may list it or leave it out: **packs need no change**.
- **`Pack::writes_bom(root, codec)`** (a provided method) and `codec::text_file::BOM`.

**Fixed**
- **Double BOM.** `cat` prints the file's bytes, BOM included; a `write` of those bytes
  (`write --base V` or `--base live`) put the BOM into the text, and the file got two. Now,
  when the doc's codec writes a BOM in front of that root, one leading U+FEFF is taken off a
  `write` body. Without a BOM in the file, a leading U+FEFF is text and stays. The default
  `writes_bom` (one text root: encode it empty and look for the BOM's bytes) covers a pack
  delegating to `TextFilePack` (Markdown) and a pack with its own BOM codec (a text pack):
  no pack change.
- **Blame lost "restored by" on a partly restored line** (a Markdown demo finding). Characters tied
  on the newest root change kept the line's first, usually not a restored one. Ties now go
  to a restored character (the most recently restored, then the first).
- **"Last edit" ranges grew over others' newer text** (a Markdown demo finding). A participant's
  last-edit span was an expanding anchor, so after their text was undone and someone typed
  there, notes and the roster said they had edited the other's line, sometimes a line past
  the end. Last-edit spans no longer expand, are dropped once their text is all deleted or
  replaced, and their lines are clamped to the text's. Claims and views keep expanding
  anchors.

**For hosts**
- **A direct edit route** (a tab's save, a CLI edit from a read): pass the version the text
  came from as `EditOptions::if_version` and the claim policy as `policy`, and drop any text
  comparison of your own: the kit checks both under the lock. Map `Stale` to exit 1 (as
  `--if-version`).
- **Routing verbs:** `runs_through_run_tool()` says which verbs go to `run_tool` /
  `run_tool_with`; the rest to their session calls (`VerbSpec::api`).
- **`approvals`** can be offered beside `log --pending`; bind it the same way
  (`LogQuery::pending()`, tier 0: `BlobSession::log` filtered to pending).
- **Notes and the roster's `last_edit`** may now say less: no range for text that is gone,
  never one past the end.
- **Blame:** a line partly put back by an undo, redo or revert now shows `restored_by` /
  `restored_in`.

**Tests and tools**
- **The 32-participant soak** (`tests/soak.rs`): 16 people and 16 agents on several
  connections, a seeded mix of every session call (reads with `--for-edit`, every edit verb,
  `write --base`, `edit_with_options`, undo / redo / `--partial`, claims, cursors, leave and
  rejoin, ticks, flushes, restarts). Checked after every call: nothing acknowledged is lost
  (every line is a token naming its writer; what appeared and went is explained by that call
  alone; undo and redo remove only lines their participant's changes put there); store
  reload equals live state; the file equals the doc after a flush; claims belong to live
  connections and a new one never overlaps another participant's (carried overlaps are
  counted: see "Found"); the roster is exactly who is connected; the kit's memory estimate
  stays in budget. `soak_short` (5 s) runs in CI; `soak_32` (ignored) runs 10 minutes on a
  node. **Run at this release (seed 1, 600 s, release build): passed.** 43,568 calls (21,477
  changes applied, v22,576), 15 restarts each equal to live, 175 file and blame checks, 1,944
  joins and 1,261 leaves, 0 carried claim overlaps; peak RSS 40 MB, peak kit estimate 16 MB
  of a 256 MB budget. Memory grows with the edit history (RSS 9.5 → 38 MB, estimate 3.4 →
  16 MB, the text staying near 300 lines) and the call rate falls with it (≈ 250/s in the
  first 30 s, ≈ 43/s in the last 150 s): bounded by the budget, not flat.
- **GNU parity:** `scripts/gnu-parity.py` no longer passes the doc path as grep's pattern
  when a case has none (`grep-no-pattern`); live on a node: **314 of 314 cases match GNU**.
- **`tools/test-bundle.sh`** and `tools/tests/` (the full suite in four parts, the soak, the
  live GNU parity): jobs for nodes with no checkout, as an xz tarball in environment chunks,
  sha256-checked; it stops with a clear message over 64 chunks (a runner refuses more than
  about 66 variables). `tools/README.md`.

**Undo spec suite (SPEC-UNDO v1.9; no change to the kit's behaviour).** From the model check's
fidelity review of v1.8: U10a (`hist.undo_own_only`) is read over the regions a step **applies** (a
region skipped under `--partial` doesn't lend its roots any more), in the oracle, the Quint model,
the enumerator and the trace test's kit-side check; **I1 is checked after every step** of an undo or
redo (the kit commits each step as its own change); O4a's wording says the live root is looked for
in the text as the step merges it, window included. New mutant **(xi) `xi-no-o4a`** (O4 on roots, no
O4a) fails I1 at depth 4 on TLC and the enumerator (2 × 1 + 1 undo + 1 revert), its mid-text witness
at depth 5 in `tests/mutants.rs`; it joins every mutant loop in `tools/undo`. TLC = enumerator at
all seven small bounds with v1.8's counts, every violation set unchanged; trace test 256 traces +
restart twin, 0 mismatches. The Quint model's IR is 19.87 MB of Apalache's 20 MB JSON limit;
`smoke.sh` prints it.

**Found, not fixed here**
- **Two participants' claims can come to overlap.** Once the text between two claims is
  deleted they share an edge, and text typed there lands in both (both anchors expand),
  although a new claim overlapping another is refused. Reproduced by the soak and by
  `tests/presence.rs::adjacent_claims_never_overlap_after_typing_at_their_shared_edge`
  (ignored; it states the wanted behaviour). Who gets the shared edge is a design decision.

## v0.9.0 (2026-10-28)

A minor release for two behaviour changes in undo, redo and revert: they **never remove
another participant's text**, and text they put back **keeps its provenance**. **Store format
5** (an in-place upgrade from 4; no downgrade). No new dependency. Undo spec v1.8
(`docs/SPEC-UNDO.md`).

**The promise.** Undo, redo and revert only ever remove text that the change they reverse put there.
Your undo never removes text someone else wrote, even if it is identical to yours, and someone
else's undo never stands in the way of yours. Text put back by an undo or redo keeps its original
author: blame shows who wrote it and who restored it, and later undos and reverts treat it as the
original author's. If someone else has since changed the same lines, the undo applies what it
cleanly can and reports the rest as a conflict; it never overwrites their change. Nothing is lost:
undone text stays in history and can be restored. This holds across restarts. (corrected in v0.9.1)

**Public API.** Additions only:
- `text::merge::merge3_hunks` (a merge of two exact hunk sets) and `merge3_hunks_by` (the same
  with the caller's "identical" predicate, rule O4).
- `BlameLine::restored_by` and `BlameLine::restored_in` (the change that put the text back,
  and its author); `CharBlame` (feature `conformance`) gains the root's client, clock, pid and
  change.
- `store::STORE_FORMAT` is 5.

`ConflictRegion`, `UndoOutcome` and the session methods are as before; their results change
where noted below.

**Fixed**
- **Undo, redo and revert could delete another participant's identical element.** They
  found the merge's sides by re-diffing strings in the region's window; when someone's
  insertion at the region's start repeated its text (`b` and an inserted `b`), the diff put
  that insertion after the region's own element, and the undo deleted the other
  participant's. The text could even be wrong (`ab`, p1 replaces `a` with `b`, p2 types `b` in
  front, p1's undo gave `abb`; now `bab`), blame was swapped, and that participant's later undo
  conflicted. Now each side is an **exact hunk set**: ours is the inverse of the change's own
  hunk, theirs the later hunks that touched the region's window, at element identity
  (`session/foreign.rs`); `diff::hunks` stays only for `restore` and `write --base`.
- **Two conflicts the exact-sides fix alone would have brought** (they never shipped): with restored
  text counted as the undoer's own new text (U18a), (a) a revert of a change that was undone
  and then redone conflicted, and (b) when b edited inside a's region and then undid it, a's
  undo of that region conflicted (b's restoration counted as b's), as did one person undoing
  two edits of the same text. Both now apply: see provenance below.

**Added: provenance for restored text** (spec U23)
- Text an undo, redo or revert puts back is a **new** element (new ID, the undoer's change; no
  tombstone comes back) that records the **root** it restores: the original element,
  path-compressed when made, never a chain. The kit stamps it, only in its own undo, redo and
  revert; no API or client update can set it (conformance `sec.client_provenance_refused`).
- The rules follow the root: a region's window is read **by root** (U8), so text someone
  removed and put back counts as the region's again; undo and redo remove only elements whose
  root the reversed change inserted (U10a); **revert of a change acts on the live elements
  whose root it inserted** (U22).
- **O4 is judged on (position, root)**, not on text: a fresh `X` someone typed where an undo
  would put `X` back is no longer taken as the same change (mid-text it now conflicts; at the
  doc's end both land, O5). **O4a:** an undo, redo or revert never puts back a root that is
  live; that region conflicts (`overlap`). So at most one live element per root (I1).
- The kit keeps a live map of element IDs and roots per text root (built lazily, once per
  load, from the engine's runs and the store; chunked runs, so a change costs O(chunks)) and
  an index of live restored elements by root.

**Changed (behaviour)**
- **Blame by root (hosts: visible change).** A restored character is blamed on its **root**:
  the change that first wrote it, and that change's author, kind, time and on-behalf; the
  change that put it back is in `restored_by` / `restored_in`. A line goes to its character
  with the newest root change. `render` adds `(restored by <who> in <change>)`; `porcelain`
  adds `restored-by` and `restored-in` lines. Before, restored text was blamed on the undoer.
- Where an undo, redo or revert used to remove the wrong copy of repeated text, it now
  removes its own: the resulting text can differ.
- A conflict can appear where a re-diff saw none, and vice versa, wherever the elements in a
  region's window differ from what its text suggests.
- An insertion at the start of a replaced region by someone else lands in front of the
  restored text (before: wherever a diff of the window put it).
- Undo history bytes (U6) count each region's carried foreign hunks and roots.
- The first edit after a doc loads pays one O(doc) walk to build the element map (before,
  only blame did).

**Store format 5**
- A new table `provenance` (each restored run's root: element IDs only, no content; a purge has
  nothing more to redact), written in the change's own transaction, deleted with the doc,
  copied by `rewrite_file`. Revert records are written as version 2, with each hunk's inserted
  and deleted elements' roots.
- **Upgrade in place:** a format-4 store opens and is stamped 5 in the open's transaction (the
  table is created; existing elements have no provenance, so each is its own root). Version-1
  revert records still decode; changes made before the upgrade revert as in v1.7 (no roots),
  and what such a revert puts back is fresh text.
- **No downgrade:** a format-4 build (v0.8.x and earlier) refuses a format-5 store with
  `StoreError::Newer`, instead of undoing without provenance or dropping the table in a
  rewrite.

**Undo spec suite** (`crates/spec-undo`, never published)
- `docs/SPEC-UNDO.md` v1.8: U23 provenance; U8 by root; U10a and U22 restated on roots; U18a
  blame by root; O4 on (position, root); O4a; invariant I1; the case class "concurrent
  restores of one root"; mutant (x) `provenance-ignored-in-O4`. v1.7 (exact hunk sets, U10a
  normative, U12 on hunk sets, mutants (viii) and (ix)) is in this release too.
- The oracle implements provenance on its own (its own root naming; theirs read by root at
  merge time, the v1.7 carried theirs kept as a cross-check); U10a by root and I1 enforced.
- The model check: roots in the bounded state (as labels, machine-checked exact); I1 stops
  every run; TLC's state count equals the enumerator's at seven bounds; every mutant, (x)
  included, fails in both inside an exhaustive bound (`docs/MODEL-CHECK.md`).
- The trace test: 256 traces and the 64-trace restart twin, 0 mismatches, with each live character's
  root, I1 and U10a by root compared on both sides; the new class hit 186 times.

**Conformance**
- `hist.undo_own_only` reads roots and covers the repeated-text case, (a) and (b) with the expected
  outcomes.
- `sec.client_provenance_refused` declared: on the kit it holds by construction (no API takes
  provenance); native: awaiting the engine's.
- `text.blame_char` and `hist.blame_grain` restated: a character maps to its provenance-root
  insert, plus the restoring event.

## v0.8.2 (2026-10-28)

A patch: a revert's conflicts no longer change across a host restart. No API change for
hosts; no store format change (still 4); no new dependency. Undo spec v1.6
(`docs/SPEC-UNDO.md`).

**Fixed**
- **A revert's conflicts could differ after a host restart.** When the revert window moves
  past a pending on-behalf change, its record's carried bounds are persisted. They kept one
  taint flag per region, and a rebuild of the revert index (a host restart) tainted both
  edges of a region tainted at one. A neighbouring region that came to touch its clean edge
  then inherited a taint (spec U22a) and the revert reported a conflict it doesn't report
  without the restart. The carry now keeps each edge's taint, as the live index does
  (`store/history.rs`, `session/carry.rs`, `session/live/history.rs`), so a restart changes
  no revert's outcome.
- **Stores written by v0.8.1 or earlier:** a carry already persisted holds one flag per
  region; it keeps reading as both edges tainted (the old, conservative reading), and so
  does that record from then on. Taint that arises after the upgrade is kept per edge.
  Same carry record version, same store format; v0.8.1 reads a new carry's taint byte as
  tainted at both edges (its own reading), so going back is safe.

**Undo spec suite** (`crates/spec-undo`, never published)
- `docs/SPEC-UNDO.md` v1.6: U22a's restart sentence dropped; restart transparency is a
  tested property. The oracle's restart touches no record.
- The trace test's **restart twin**: stack-free traces also run on a session that never
  restarts, and every outcome and text must match; the class `revert_restart_transparent`
  (a persisted carry, restarts between the change and the revert and between later
  changes). On v0.8.1's kit 43 of 64 twin traces fail it; here 0.
- The model check `MC text-undo` (`docs/MODEL-CHECK.md`): `spec/quint/undo.qnt`, the
  bounded world over the oracle, the exhaustive enumerator `undo_enum`, ITF replay
  `undo_itf_replay`, six mutants (feature `mutants`), and K2-agnostic node jobs (`tools/`).
  ITF replay 0 mismatches (the full bound included); TLC's exact counts equal the enumerator's at
  four small bounds; every mutant fails in both at the same depth. The full bound itself is
  beyond exhaustive reach; its plan is in the doc.
- Two findings, reported, not fixed here: **undo can delete another participant's
  identical character** when their insertion at the region's start repeats its text
  (`hist.undo_own_only` per element; the kit too); and **U12's equivalence note fails in
  repeated text**.

**Conformance**
- `erase.span_includes_tombstones` runs on the reference model (mpce-model v0.4.1, vendored;
  `Doc::span_ids`); it stays "never" on Yrs.

## v0.8.1 (2026-10-28)

A patch: undo, redo and revert remove only their own text. No API change for hosts; no
store format change (still 4); no new dependency. Undo spec v1.5 (`docs/SPEC-UNDO.md`).

**Fixed**
- **Undo, redo and revert could remove another participant's identical text.** A clean
  region applied a fresh diff of its window instead of the merge's own hunks (spec U10):
  the text was the same, but in repeated text (two identical lines, a run like `aaaa`) the
  diff could name another participant's identical characters. Now the merge's own hunks
  are applied (`session/undo.rs`). `write --base` was **not** affected: it already applied
  the merge's own hunks (there is no `write --base --partial` in the kit).
- **Stores written by v0.8.0 or earlier:** where an undo, redo or revert removed one of two
  identical runs of text, it may have removed the wrong one. The text right after it was
  as reported, but blame on those characters is swapped, and a later undo or revert of the
  other participant's change there may report a conflict, report nothing to undo, or
  **apply while leaving that participant's text in place** (two participants append the
  same line and each undoes their own: one line stays). So the visible text after such a
  later undo can differ from what the undo should give. No automatic repair: the intended
  copy can't be recovered from the store.

**Undo spec suite**
- `docs/SPEC-UNDO.md`: the kit's undo spec, v1.5 (U10 clarified; **U22a**, revert's carried
  taint, exactly, with its property: revert's conflicts ⊇ undo's on the same state, and the
  same text wherever both apply; U19's notice: "cleared by purge" is said once and
  withdrawn by the holder's next batch).
- `crates/spec-undo` (never published): `spec_undo::model`, the spec's oracle, and the
  trace-equivalence test against the kit's sessions on Yrs: 256 seeded traces in CI, every
  case class hit (a repeated-text class among them), 0 mismatches.
- Conformance: `hist.undo_own_only` (per element, through blame), `hist.restore_minimal`;
  restore is the kit's operation on every engine; `erase.span_includes_tombstones`
  declared (awaiting mpce-model v0.4.1).

**Feature `conformance` only** (off by default; no host enables it; the default API is
unchanged): test hooks for the undo spec's trace test and the per-element rule.
- `Session::undo_depths(pid)`: a pid's undo and redo stack depths in the doc.
- `Session::char_blame(root)` and `CharBlame`: every live character with its element
  (client, clock), owner pid and change.
- `DocStore::unsynced_for_tests()`: from then on, commit without syncing the store file
  (in-process test runs; a sync per change was most of the trace test's time).

## v0.8.0 (2026-10-28)

Version guards for agents' edits, and claim ends that say where the range is and what
version the doc is at. Planned as v0.7.1; it is **v0.8.0** because two event variants gain
fields (below). No store format change (still 4); no new dependency; no new exit code.
`docs/SESSIONS.md`, amendment 14.

**What's in it**
- **`--if-version V` on every edit verb** through `run_tool`: `sed -i`, `patch`/`apply`
  (`--dry-run` too), `append`, `insert`, `replace` (every mode), `delete`, and every pack
  verb (e.g. Markdown's `replace-section`, with no pack change). Apply only if the doc is
  still at version V.
  - **Checked under the session lock**, in the edit's critical section, by the same check
    as `Session::edit`'s `if_version`: nothing can land between the check and the apply.
  - **Stale:** `SessionError::Stale { expected: V, actual: W }`, exit 1, nothing applied, no
    new version. One line, **`doc changed since V (now W)`**, never any doc text.
  - **First:** after admission (rights, a declined doc, the edit rate), it wins over every
    other refusal: the approvals backlog, the verb's own "no" (no match, not unique, a
    failing patch), a tool error found on the live text, and `Respect` claims. A no-op
    (`--old` equal to `--new`) still checks it.
  - **V** is decimal, or `vN` as the summary prints it. Kit verbs parse it as any long
    option (`--if-version=V`, a unique prefix). For pack verbs, the kit takes out
    `--if-version V` / `--if-version=V` (exact spelling, before `--`) before the pack parses.
  - **Read verbs refuse it**, exit 2 (`--if-version is for edit verbs; cat is a read (it
    prints the version)`), so agents learn where it applies. Twice, missing or bad: exit 2.
  - `tools::Tool::if_version()`; the parity registry's edit-verb usages name it.
- **`ClaimEnded` and `ReservationEnded` carry `lines` and `version`**: the range's current
  lines as it ends (after the edits made while it was live moved its anchors; `LineRange`,
  as `ClaimTaken`'s) and the doc's version at that moment (after the ending edit, for
  `Edited`).
- **`SessionError::Stale` reads `doc changed since {expected} (now {actual})`** everywhere
  (was `doc moved: expected version …, it is at …`): `edit`'s `if_version`, tier 0's stale
  base, and `--if-version`.

**For hosts**
- **Which version to record.** "The version the agent last read" is `ToolOutput::version`
  from a read verb through `run_tool` (also in `stderr_summary`, `notes.md v1842`, and
  `Fence::version`), or `ReadResult::version` from `Session::read`. After an edit,
  `ToolOutput::version` is the version after it. All are the session's one version, the
  number `--if-version` compares and the events carry. `Status::version` is the same number
  now, but it is not a read: don't use it as what the agent saw.
- **Pass `--if-version` through in `argv`**, as given. Map `Stale` (`ToolRunError::Session(
  SessionError::Stale { expected, actual })`) to exit 1 and print its `Display` on stderr; it
  is one line and holds no doc text. Re-read and retry is the agent's move.
- **Telling bounced agents.** On `ClaimEnded` (or `ReservationEnded`), "lines
  `lines.first`–`lines.last` are free; the doc is at `version`; re-read" uses the same
  identity: an edit with `--if-version <version>` applies if nothing landed since, and one
  with an older read's version is refused with `doc changed since V (now W)`, W =
  `version` when nothing landed since.
- **Breaking:** `AuditEvent::ClaimEnded { .., lines, version, .. }` and
  `ReservationEnded { .., lines, version, .. }`. The enum is `#[non_exhaustive]`, but these
  variants aren't: a pattern that lists their fields without `..`, or code that builds
  them, must add the two fields.
- If you show the `Stale` text verbatim or match on it, it changed (above).
- `sed --i` no longer abbreviates `--in-place` (it is ambiguous with `--if-version`, exit
  2). `sed -i` and `--in-place` are unchanged.

**For packs**
- Nothing to change: pack verbs get `--if-version` from the kit. A pack verb can't take a
  value that is literally `--if-version` (the kit takes it first); `--` still ends options.
- A pack whose lockfile pins the kit needs the lock bumped to 0.8.0 (path dependency).

## v0.7.0 (2026-10-28)

Agent presence: agents have no cursor, so the kit now tells them (and the people beside
them) who is looking at, has just edited, or is about to edit the same lines, and lets an
agent "take a turn" on a range. **Ephemeral awareness, not locking and not history:**
nothing goes to the store, the log, versions or the engine, and the kit never blocks a
person. No store format change (still 4); no new dependency. `docs/SESSIONS.md`,
amendment 13.

**What's in it**
- **Claims.** `Session::claim(conn, lines, ttl_ms, note, hold)` takes an anchored,
  expiring claim (default 30 s, at most 10 min; the agent picks its TTL).
  - **Anchored, expanding.** The range follows concurrent edits. Its start sticks to the
    character before and its end to the character after, so text typed at an edge lands
    inside, and it never inverts.
  - **Ends:** the claimant's own edit touching it (unless `hold`), its time running out,
    `release_claim`, or the connection leaving. Also `renew_claim`, `claims`, and
    `claim_remaining_ms` (deterministic on the host's clock).
  - **Overlap:** another participant's live overlapping claim refuses a new one with
    `SessionError::Claimed { id, by, name, lines: (first, last), remaining_ms, note }`.
  - **Waiters:** `register_waiter` / `cancel_waiter` keep a FIFO queue per claim.
    Anti-starvation: while anyone else waits, the owner can't extend the claim or give it
    `hold` (`ClaimContended`). It still ends normally.
  - **Hand-off:** when a claim with waiters ends, its range is reserved for the first
    waiter for `claim_handoff_ms` (2 s). Meanwhile other agents' overlapping claims and
    `Respect` edits there are refused (`Claimed`, with the reservation's time left), so
    the owner can't grab it back. The waiter's first edit there uses it (and passes it on);
    its first claim there uses it (and the rest of the queue waits for that claim). It
    lapses at its deadline, or when the waiter cancels or leaves, then passes to the next
    waiter. `Session::reservations` lists them.
- **A claim policy on edits, chosen by the host.**
  - `ClaimPolicy::Ignore` (the default, and what every existing call uses) never refuses.
  - `ClaimPolicy::Respect` refuses an **agent's** edit into another participant's live claim
    (or a reservation held for someone else) with `Claimed`, applying nothing.
  - **Claims coordinate agents; they never block anyone else.** Only
    `ParticipantKind::Agent` is ever refused or reserved against: a person, an app guest or
    any other kind never is, whatever the policy.
- **`--for-edit [--ttl MS]`** on `cat`, `head`, `tail`, `view`, `grep`, `sed` (no `-i`): the
  read also takes a short implicit claim (30 s) on the lines shown. The claimant's next edit
  there releases it. Another participant's claim there takes nothing: the read still
  succeeds and reports the conflict (`ToolOutput::for_edit: Option<Box<ForEdit>>`).
- **Heads-up notes.** Every read and edit through `run_tool` returns `ToolOutput::notes`:
  who else is viewing, just edited, has claimed, or (a person) has a caret or selection on
  the lines it touched.
  - **Deduped** per (reader, other, kind, lines) for 60 s.
  - **Rate-limited** per (reader, doc): one batch per 5 s, at most 5 notes, then
    `note: +N more`.
  - **One line each**, starting `note: `, for stderr only. `stdout` never changes.
- **`write --base` conflicts name names.** `write_base_with` returns a `WriteReport` whose
  `conflict_notes` say who claimed or last edited each conflicting region.
- **Activity.** `Session::activity(conn, n)`: a per-doc ring (256 rows) of reads, edits and
  claims (who, verb, lines, when). It is dropped on unload, and erasure drops the pid's rows.
- **Richer roster rows.** `RosterEntry::claims` and `last_edit` (anchored lines). Agents'
  carets are at their last edit, as before.
- **Parity.** New verbs: `claim`, `claim --renew` and `claim --release` serve the caret row ("say
  what I'm working on"); `activity` serves the who's-here row. `--for-edit` is a flag on the read
  verbs. The kit has 61 cases now, and the harness compares the actor's claims (`ClaimObs`).

**For hosts**
- **The policy parameter.** Pass a `ClaimPolicy` on agent edits: `run_tool_with(conn,
  root, argv, stdin, &ToolOptions::new().policy(p))`, `edit_with_policy`,
  `write_base_with`, `replace_text_with`. Map `--respect-claims` (or your default) to
  `Respect`. The old calls stay `Ignore`. An auto-wait ("agents respect each other's claims
  with a bounded wait") is yours to build:
  1. on `Claimed`, call `register_waiter(conn, id)`;
  2. wait for `AuditEvent::Reserved` naming your `conn` for that claim (or `ClaimEnded`), at
     most its `remaining_ms` (or your timeout), then `cancel_waiter`;
  3. retry the edit within the reservation (2 s). Only agents wait: a person or app guest
     never does.
- **Events.** `AuditEvent::ClaimTaken { claim, pid, conn, lines, expires_ms, hold, for_edit }`
  and `ClaimEnded { claim, pid, how, next_waiter, waiters }`. `waiters` is in FIFO order.
  The hand-off adds `Reserved { claim, pid, conn, lines, until_ms, waiters }` (each grant)
  and `ReservationEnded { claim, pid, how, next_waiter }`.
  They come with the session locked: queue the wake-up, and don't call the kit from
  `on_event`. The claim's note is not in events; read it with `claims`.
- **Exit codes.**
  - `Claimed` is **5**, the same code as `Leased` (someone else holds it for now; retry
    after `remaining_ms`). No new code.
  - `NoClaim` is 4; `ClaimContended` and `BadClaim` are 2.
  - `claim --release` exits 1 when you held none.
- **Notes rendering.**
  - Print `ToolOutput::notes.to_string()` (and `for_edit`, also a `note: ` line) on
    **stderr**, never stdout. Nothing is printed for an empty batch.
  - For `--presence json`, serialise `Note`, `ClaimInfo`, `ActivityEntry` in their field
    order. Enums go as `as_str()`. The kit has no serde.
  - `held` counts notes the rate limit kept back; it is useful for JSON only.
- **CLI verbs.** Route these to the session calls:
  - `claim --lines a,b [--ttl MS] [--note T] [--hold]` → `claim`;
  - `claim --renew [--id ID] [--ttl MS] [--hold|--no-hold]` → `renew_claim`;
  - `claim --release [--id ID]` → `release_claim`;
  - `activity [-n N]` → `activity`;
  - `who` → the roster, with its new columns.
  `ClaimId` displays as `claim-12`, and `ClaimId::parse` reads that or `12`. Pass
  `--for-edit` through in `argv`.
- **Idle.** A connection holding or waiting for a live claim doesn't idle out until the
  claim ends. That stays bounded by `claim_max_ms`.
- **Conflict notes** name only participants still present; a one-shot editor that has
  left isn't named (its change is in `log` and `blame`).
- **Config:** `claim_default_ms` (30 s), `claim_max_ms` (10 min), `for_edit_ms` (30 s),
  `claims_max` (256), `claims_per_participant` (16), `claim_waiters_max` (64),
  `claim_handoff_ms` (2 s; 0 turns hand-off off),
  `note_edit_window_ms` (60 s), `note_repeat_ms` (60 s), `note_interval_ms` (5 s),
  `note_batch_max` (5), `activity_max` (256).

**For packs**
- A text pack's parity list changes:
  - the caret row is `live(&["cursor", "claim", "claim --renew", "claim --release"])` (the
    `intent` placeholder goes);
  - the who's-here row adds `activity`.
  Otherwise `check` fails with `VerbNotListed`.
- `PersonAction` is unchanged; tier 0 is unaffected.
- `parity::PresenceObs` gains `claims`, which matters only if you build one by hand.

## v0.6.0 (2026-10-27)

Exact-text replace for agents, `replace --old TEXT --new TEXT [--all]`: "replace this exact
text with that", with no regex to escape. Also the engine-generic conformance suite and its
test-only hooks (below). No store format change (still 4); no new dependency.

**What's in it**
- **`replace --old TEXT --new TEXT [--all]`**, a third mode of `replace`, exclusive with
  `--lines` / `--range`.
  - **Exact.** `--old` is matched byte for byte (case, whitespace, line breaks), with no
    regex or escapes, on the doc text the other edit verbs see: `\n` line breaks, no BOM.
    A CRLF file's `\r\n` and its BOM are restored when it is written.
  - **Unique by default.** Matches are counted left to right without overlaps. One match
    applies. None, or more than one, applies nothing and exits 1. An only match that
    overlaps another occurrence (`aa` in `aaa`) counts as more than one.
  - **`--all`** replaces every non-overlapping match, left to right (`str::replace`).
  - **`--old` must be non-empty; `--new` may be empty** (deletes the match). Both are
    required.
  - **`-` reads one of them from stdin** (not both). Without `-`, stdin is not read.
  - One atomic, attributed change (one undo), through `edit_with` on the live text, like
    every edit verb. `--old` equal to `--new` succeeds with no new version.
- **Refusals an agent can retry from.** The report goes on `stdout`, the
  `stderr_summary` says `nothing applied to …`, and the doc's text is never quoted:
  - **More than one match:** the count and each match's lines, for example
    `replace: --old matches 3 times in notes.md (at lines 2, 5–6, 9 (×2)); nothing
    applied`. Up to 20 are listed, then "and N more". It then says to add surrounding text
    or pass `--all`.
  - **No match:** says so, with hints that never apply anything:
    - `--old` has `\r` (and whether it matches with `\n`);
    - it starts with a BOM;
    - it would match with whitespace loosened, case ignored, or both (count and lines).
- `--all` prints `replaced N matches in notes.md (at lines … before the edit)` on stdout.
  Otherwise a successful replace prints nothing on stdout, like the other edit verbs.
- **Parity:** `replace` now serves the find-and-replace row too, with three cases: one
  match, `--all`, and a two-line `--old` from stdin. Its exits are `0, 1`.

**For hosts**
- Route `<host> doc replace --old TEXT --new TEXT [--all]` to `run_tool` as given. `argv`
  is a list, so `--old`/`--new` may hold newlines and start with `-`; no shell quoting is
  involved.
- **`--old-file PATH` / `--new-file PATH` are the host's flags.** The kit does no I/O:
  read the file and pass its text as `--old` / `--new`, so `argv` never names a path.
  Keep `-` for stdin as is.
- **Exit codes:** 0 applied (or a no-op); 1 no match or not unique (nothing applied: print
  `stdout`, the report, as you print `patch`'s); 2 usage. Usage errors are an empty
  `--old`, a missing `--old` or `--new`, one given twice, both `-`, a mix with
  `--lines`/`--range`, or stdin that isn't UTF-8. Session refusals (3, 4, 5, 6) as for any
  edit verb. `tools::exit` is unchanged.
- `run_tool` has no `if_version` guard for any verb, and this mode adds none. The match is
  found in the live text, so a replace applies where the text is now.
- **Parity list:** add `replace` to your find-and-replace row (`["sed -i", "replace"]`), or `check`
  fails with `VerbNotListed`. Hosts checking their command table against
  `Registry::kit()` see `replace`'s new usage line and exits.

**For packs:** a text pack's find-and-replace row must list `replace` (above). `PersonAction` is
unchanged; tier-0 lists are unaffected.

**Conformance suite** (folded from Unreleased):
- **`conformance` feature (off by default; no host enables it).** Test-only hooks in
  `conformance::replica` (`sync` between two of the kit's docs in one process, `state` as
  sorted `(client, clock)` pairs) for the new `crates/conformance` suite. The default API and
  build are unchanged; `conformance::KNOWN_DEVIATIONS` stays (the kit Doc's list).
- **`conformance` feature, more hooks** (still test-only, off by default):
  `conformance::replica::{events_since, deliver}` (an update past a state the doc has seen;
  delivery with the per-client continuity check client updates get, then a trusted apply),
  `blame` (the blame runs per text root) and `anchor` / `resolve` (relative positions, with
  `Anchor` and `Stick`). The client-update check's continuity loop is now one private
  function shared by both paths (no behaviour change). `conformance::KNOWN_DEVIATIONS` gains
  three pinned entries: `core.causal`, `core.no_equivocation`, `hist.undo_atomic`.
- The conformance suite gains capabilities (Events, Blame, Anchors, Undo), the rules
  `core.idempotent`, `core.causal`, `core.no_equivocation`, `text.scalar_safe`,
  `text.cursor_stable`, `text.blame_char`, `text.edit_atomic_split`, `hist.undo_atomic`,
  and a coverage matrix enforced by a test (`docs/CONFORMANCE.md`).
- `vendor/mpce-model` bumped to **mpce-model-v0.2.1**; its adapter uses the model's native
  `edit_as`, `diff_events` / `deliver_all`, `blame` and anchors (deviation list still empty).
- The text conformance rules moved from `crates/base/src/conformance/text.rs` to
  `k2-mpc-conformance` (never published), generic over engines: they run on the kit's `Doc`
  and on the vendored reference model `mpce-model` (empty deviation list, asserted).

**Tests (new):** 49 golden cases (`exact-*` in `tests/golden/tools.golden`). These cover
unique, no match with and without each hint, 2+ matches (same line, multi-line, capped),
`--all` (overlapping `aa` in `aaa`), empty and missing flags, multi-line, non-ASCII and
emoji (ZWJ, combining mark), CRLF and BOM, stdin for either side, both `-`, `old == new`,
and the exclusion with `--lines`/`--range`. `tools_session`: `exact_replace_*`
(attribution, caret, undo, `--all` as one change, refusals apply nothing, the no-op,
stdin, a CRLF+BOM file round trip, the doc changing between read and replace, rights) and
`concurrent_threads_exact_replace_and_typing_lose_nothing`. Parity: the find-and-replace row's
three `replace` cases on both paths.

## v0.5.0 (2026-10-27)

Approvals for on-behalf blob writes, on its own because it changes the store
format and decides who may settle an approval. The API contract is `docs/SESSIONS.md`
(amendment 11). **Store format 4**; no new dependency.

**What's in it**
- **On-behalf blob writes get a pending approval.** A `write` or `restore` by an assistant
  whose participant has `on_behalf_of` applies at once (never held) and records
  `Approval { Pending, by: None, at_ms: None, author, on_behalf_of }` for that version in the
  same transaction: the text doc's record and rules.
- **Only the named person settles it**: `BlobSession::set_approval(conn, version,
  Approved | Declined)`. The agent, another person or an owner get
  `Forbidden { need: Approver }` and an `AuditEvent::Refused`; `Pending` is no target; once
  settled it can't flip (`BadApproval`), the same way again is a no-op; no approval is
  `NoApproval` (exit 4). `approval` and `approvals` with text's read rules.
- **Decline keeps history** (tier 0): the result, `BlobApproval`, names the
  version to restore (`restore_to`) and the versions after it (`later`, which a restore also
  replaces: blobs don't merge). Nothing is restored automatically.
- **Surfaced:** `BlobLogEntry::approval`, `BlobBlame::approval` (current version); `who`
  shows the agent for the person, as before.
- **Parity:** `approve`, `decline`, `log --pending` on both tiers; tier-0 cases for the approve and
  pending-approvals rows (31 cases over 16 live rows); the blob observation compares approval
  records.
- **Purge reaches it**: a purged version's approval record is kept as metadata
  (no content, no blob id) and counted (`PurgeCounts::approvals`); delete and the file
  rewrite cover the new table.

**Store format 4 (no downgrade).** New table `blob_approvals` keyed `(doc, version)`, apart
from the text `approvals` keyed `(doc, seq)`, so no text reader can take a blob version for
a change. Opening a format-3 store **upgrades it in place** (one transaction: the table, a
`Pending` approval for every on-behalf blob version it already holds, the stamp). **A v0.4.x
build refuses a format-4 store** (`StoreError::Newer { found: 4 }`, checked against the real
v0.4.0 code): to roll back, restore the store from a backup taken before the upgrade.

**For hosts**
- Route `<host> doc approve|decline <version>` on a blob doc to `BlobSession::set_approval`;
  on a decline print the way back from `restore_to` / `later` ("restore version N to undo it;
  versions M... came after it and would be replaced too"). Don't restore for the person.
- `log --pending --on-behalf-of me` on a blob doc: `BlobSession::log` filtered to
  `approval == Some(Pending)` (and `on_behalf_of`); a tab's list: `approvals`.
  `AuditEvent::Approval { seq }` and `NoApproval { seq }` carry the version for a blob doc.
- Tier-0 parity list: make the approve row (`approve`, `decline`) and the
  pending-approvals row (`log --pending`) live; a
  deferred row now fails as stale. Exit codes are unchanged (`tools::session_exit`).
- Back up the store before the first open with v0.5.0 if you may need to roll back.

**For packs:** nothing changes for text packs (their registry and list are the same).

**Not built:** a pending cap for blob docs (text's `approvals_backlog`); proposed in
amendment 11.

**Tests (new):** `tier0::on_behalf_blob_write_pending_approval`,
`on_behalf_blob_approval_only_by_person` (incl. after a restart),
`decline_then_restore_after_restart`, `a_v040_store_upgrades_to_format_4_in_place` (a
store written by v0.4.0, `tests/fixtures/store-format3-v040`);
`erasure::tier0_purge_reaches_on_behalf_versions_and_approvals_no_bytes_behind` (byte scan);
`store::tests::on_behalf_blob_versions_get_a_pending_approval_in_the_same_commit`,
`a_format_3_store_is_upgraded_in_place_with_its_approvals`;
`parity_blob::an_approval_verb_missing_or_different_fails_loudly`.

## v0.4.0 (2026-10-27)

Tier-0 kit asks from the first host. The API contract is `docs/SESSIONS.md` (amendment 10). **Store
format stays 3**, no store change; no new dependency.

**What's in it**
- **Per-verb tiers.** `parity::Tier { Text, Blob }`; `VerbSpec::tiers` (`applies_to`) says
  which docs a verb works on, so a host classifies its commands from data. Both tiers:
  `open`, `leave`, `cat`, `who`, `write`, `status`, `flush`, `log`, `show --version`,
  `restore`, `version --name`, `mv`, `blame`. `Registry::for_tier(tier)`;
  `Registry::for_pack` keeps working (the text tier plus the pack's verbs).
- **Lease verbs** (tier 0): `lease [--ttl MS]`, `lease --renew`, `lease --release`,
  `lease --break` (owner, as `BlobSession::break_lease`), and their person actions
  **`Lease`, `ReleaseLease`, `BreakLease`**.
- **Tier-aware parity.** Each `PersonAction` has `tiers()`; a list needs rows only for its
  registry's tier, so the Markdown pack doesn't cover the leases. A tier-0 harness
  (`parity::check_blob`, `kit_blob_cases`: 27 cases over 14 live rows, 8 for the leases)
  runs the person path and the agent path on blob docs; `tests/parity_blob.rs` holds tier 0's
  list for any file, with negative controls.
- **`BlobSession::log`**: one `BlobLogEntry` per version, oldest first (who, when, kind:
  import / write / restore / external / conflict-kept / merged, `of`, `on_behalf_of`, name,
  `redacted`, size, the conflict copy). Same rights as `versions`; built from what's stored.
  It is the blob `log` verb.
- **Conflict copies:** `<name> (conflict <who> YYYY-MM-DD HHMMSS).<ext>` (UTC),
  in the folder the host chooses; listed in `BlobStatus::conflict_copies`; deleted on `tick`
  after 30 days (`Config::conflict_copy_keep_ms`) unless named, edited, or not the kit's;
  reported in `TickReport::conflict_copies_removed`. The purge byte scan and
  `conflict_copies` find copies in the host's folder and old ones beside the file.

**For hosts**
- **New `Host` method:** `fn conflict_dir(&self, file: &Path) -> Option<PathBuf>`, default
  `None` (copies beside the file, as before). Return your folder (the kit creates it). The
  copy is recorded relative to the file's folder, so `BlobVersion::conflict_copy` and
  `AuditEvent::BlobConflict::copy` can be `../…` paths; `DiskCheck::Conflict::copy` is the
  resolved path.
- **The name changed** from `name (conflict <time> from <who>).ext` to
  `name (conflict <who> <time>).ext`. Copies written by v0.3.x keep their names and are still
  found (and cleaned up).
- **The registry's tier field:** classify `<host> doc` verbs with `VerbSpec::tiers`; check the
  tier-0 command table against `Registry::for_tier(Tier::Blob)`. `Registry::kit()` is now
  the text tier only.
- Show `BlobStatus::conflict_copies`; route the lease verbs and `log` on blob docs.

**For packs:** map the three lease rows in your `parity_row` match (a text pack: N/A, "leases are
tier 0's") and build your list with `PersonAction::for_tier(Tier::Text)`. `VerbSpec` literals need
`tiers`.

**Behaviour changes:** the conflict-copy name; copies in the host's folder when it gives
one; the 30-day cleanup on `tick`.

**Verified locally:** the full suite (298 tests, 1 ignored; new: `tests/parity_blob.rs` with 3,
7 in `tests/tier0.rs` incl. `conflict_copy_naming_cleanup`,
`parity::registry::tiers_are_consistent`), clippy `-D warnings`, `cargo fmt --check`.

## v0.3.1 (2026-10-27)

Patch: exit codes only (`docs/SESSIONS.md`, amendment 9). No API or store change; the store
format stays 3. No new dependency.

**Fixed:** `tools::session_exit` gave **6** (store unavailable) for a version that doesn't
exist (`StoreError::NoVersion`, e.g. a tier-0 `read_version`, `restore`, `name_version` or
`purge` of version 99). Everything that means "the thing you named doesn't exist" now exits
**4**:

| Error | Was | Now |
|---|---|---|
| `Store(NoVersion \| NoDoc \| NoApproval \| NoBlob \| NoBlobSpace)` | 6 | 4 |
| `NoApproval { seq }` (approving a change made on nobody's behalf) | 2 | 4 |
| `Blob(NotFound)` | 1 | 4 |
| `Blob(Store(e))` | 1 | as `Store(e)`: 4 for a missing thing, 6 for a store failure |

Real store failures (`Db`, `Corrupt`, `Newer`, `Locked`, `NoRandom`...) stay 6;
`Store(CurrentVersion)` stays 2 and `Store(PurgedVersion)` 4.

**Tests:** `tools::error::missing_things_exit_4_and_store_failures_exit_6` (every variant of
the sweep, from the session, the store and the blob store),
`tier0::a_missing_version_exits_4_not_6`.

**Verified locally:** the full suite (287 tests, 1 ignored), clippy `-D warnings`,
`cargo fmt --check`.

## v0.3.0 (2026-10-27)

Blame (its parity row live). The API contract is `docs/SESSIONS.md` ("Blame"; amendment 8, additive,
lists every change). **Store format stays 3**: one new table, so a v0.2.0 build still reads
and writes a v0.3.0 store (N−1). No new dependency.

**What's in it**
- **`Session::blame(conn, root, lines)`** (needs history): each line's most recent author at
  the character's grain. The engine gives every live character's client and clock (Yrs
  `diff_range`, on a throwaway replica, O(doc) per call; the live doc is untouched); the store
  maps clients to pids and clocks to changes. A line goes to its character with the greatest
  change id (apply order, immune to host clock skew; time is display only), its `\n`
  included. Each line carries the change id, kind, surface, time,
  `on_behalf_of` and approval state, and `erased` when a purge reached its change. Imports
  blame as `kit:import`, outside edits as `kit:external`, unknown clients as `kit:unknown`
  (never a guess), erased participants by pseudonym; undo, redo, restore and revert as
  whoever ran them (U18a). `-L a,b` ranges.
- **`Blame::render(label)`**, a `git blame`-like listing with the host's labels, and
  **`Blame::porcelain()`**, shaped like `git blame --line-porcelain`.
- **Tier 0:** `BlobSession::blame`: the current version's setter and the pointer's history
  summarised.
- **The `blame [-L a,b]` verb** in `KIT_VERBS` (history kind), with three kit parity cases
  (the gutter vs the verb, `-L` vs the lines in view, refused without history). The
  stale-deferral control now runs on the row that flipped.
- **Store:** `change_clocks`, written in each change's transaction, erased with the doc,
  copied by `rewrite_file`.

**For packs:** mark the blame row live: `ParityRow::live(P::Blame, &["blame"])`. A pack still
listing it deferred fails its parity test (`UnexpectedVerb`, `StaleDeferral`).

**Behaviour changes:** none for existing calls. Text inserted before v0.3.0 (and by a v0.2.0
build writing a v0.3.0 store) has no clock rows: it blames to its client's owner without a
change id, and ranks below recorded changes in a line. A v0.2.0 build's `rewrite_file`
refuses the new table (an error, nothing dropped).

**Verified locally (CI-size):** the full suite (285 tests, 1 ignored; new: `tests/blame.rs`
with 8, including a 48-case property test against a model of who typed each character; 3
in-crate; the parity harness at 54 kit cases over 27 live rows), clippy `-D warnings`,
rustdoc, rustfmt and the licence gate. The Markdown pack v0.2.0 passes against it (60 cases).

**Known weak spot:** the "live doc untouched" test compares the state vector and encoding,
but running `diff_range` on the live doc leaves both byte-identical too (block splits are
in-memory only), so that test can't catch a regression to the live doc; the throwaway
replica is held by code review.

**Not in this release:** blame of a version (`--version`), blame through the wire, char
blame export at the native migration; the WS sync protocol and JS client.

## v0.2.0 (2026-10-27)

Erasure. The API contract is `docs/SESSIONS.md` ("Erasure"; amendment 7 lists every
change). **Store format 3** (a format-2 store opens and is upgraded; a v0.1.0 build refuses a
format-3 store). No new dependency.

**What's in it**
- **Purge** (`Session::purge`, owner only, audited as metadata only): the whole history, a
  range of changes, a range of versions, a named text (`--text`, optionally
  `and_remove`), or a participant. Live text is not changed: edit it out first, or use
  `and_remove`. One store transaction redacts the log (a snapshot re-encoded from the live
  doc replaces it; change ids, authors, lengths and times stay), versions (entries kept,
  spans cut, `VersionInfo::redacted`), revert records (a revert over them reports
  `erased`) and the sync replica; the file is written back first; every undo and redo stack
  of the doc is cleared (U19); the read-base cache is dropped.
- **Participant erasure:** a pid becomes a pseudonym in every attribution the store keeps,
  counts kept; their content is purged; their connections are removed.
- **No bytes behind:** `DocStore::rewrite_file` copies the store into a fresh file and
  renames it over the old one (redb's compaction doesn't zero freed pages: a deleted row's
  bytes stay in the file until then). `DocStore::scan_for`, `store::scan_file` and
  `scan_bytes` find a needle whole or by any 8-byte window (split secrets included).
- **Keyed, expiring hashes:** sync and own-write hashes are HMAC-SHA256 under a per-doc key
  (format 3), replaced by every purge; own-write hashes expire after 60 s
  (`Config::own_write_ttl_ms`). A format-2 plain hash is read once, then re-keyed.
- **Tier 0:** `BlobSession::purge` for one version (the current one is refused) or the whole
  history: entries kept (`BlobVersion::redacted`), unreferenced blobs removed at once, kit
  conflict copies holding purged bytes deleted (compared by keyed id), shared blobs and
  other copies reported. `BlobSession::conflict_copies` for a scan.
- **What purge doesn't reach** is returned with every purge (`NotReached`): git history,
  transcripts, search indexes, OS snapshots, synced folders, earlier backups, offline
  clients, other apps, the host's rows, freed disk blocks; plus the file when it couldn't be
  rewritten, and shared blobs or changed conflict copies. Rotate the secret.
- Yrs GC is pinned on (`gc_is_on`, with a `skip_gc` control that must fail).

**Refusal:** a `PurgeScope::Text` shorter than 8 bytes is refused (`BadEdit`, exit 2:
"purge text of at least 8 bytes, or purge by change/version/participant"); it would redact
innocent text across the whole history.

**Behaviour changes:** `FlushOutcome::hash` of a text doc is the keyed hash (still 64 hex
characters); an editor saving one of our writes older than 60 s is an outside edit; a
redacted version is never a `write --base` (`BaseExpired`); `session_exit` maps
`Store(CurrentVersion)` to 2 and `Store(PurgedVersion)` to 4.

**Verified locally (CI-size):** the full suite (274 tests; 16 new in `tests/erasure.rs`,
each byte-scan gate with a negative control that finds the secrets before the purge), plus
four mutants that each fail the gate (no file rewrite, no version redaction, no
revert-record redaction, stacks not cleared); clippy `-D warnings`, rustdoc, rustfmt and the
licence gate.

**Pending, to run on a Linux compute node:** the erasure gate at full size,
`MPC_ERASURE_SCALE=200 cargo test -p k2-mpc-base --release --test erasure` (the `tests/erasure.rs`
module docs give the `k2 compute run` line).

**Not in this release:** a `purged` event to subscribers and the `purged` frame,
the orphaned-doc lifetime, backups, participant erasure on blob
docs, approvals for on-behalf blob writes; blame; the WS sync protocol and JS client.

## v0.1.0 (2026-10-27)

First release of the Multi-Player Cores (MPC) base kit. The API contract is
`docs/SESSIONS.md` (amendments 1–6 are listed in its changelog).

**What's in it**
- **Docs on K2-defined primitives.** Text is implemented; the other kinds are specified in
  `docs/PRIMITIVES.md`. The CRDT engine (Yrs 0.28) is private.
- **Sessions.**
  - Presence per connection; rights per participant through the `Host` trait.
  - Three edit paths: `edit_with`, `edit` + `if_version`, and `write_base` (three-way merge,
    no last-writer-wins).
  - Change events; a memory pool with LRU unload; rate limits.
- **Disk sync.**
  - Atomic write-back that keeps the file mode; echo suppression.
  - Outside edits merged as attributed changes, with versions around them.
  - Load reconciliation; crash-safe write-back.
- **Text tools (`run_tool`).**
  - Read verbs `cat/head/tail/wc/grep/sed -n/diff/view`; edit verbs
    `sed -i/patch/append/insert/replace/delete`, each one atomic change.
  - Dangerous `sed` commands are refused; an injection fence with a per-call nonce.
- **History.**
  - Automatic and named versions with retention; `log`, `diff`, `show`.
  - Restore as a forward change.
  - Per-person undo to the undo spec v1.4: per-hunk regions, `--partial`, `--drop`, redo.
  - `revert` of a named change, from a persisted record with an O(log R) position carry.
- **On-behalf edits and approvals; a history right; a host key-value table.**
- **Tier 0 for any file.** Keyed content-addressed blobs, presence, a lease, versions and
  restore, plus conflict copies when a desktop app ignores the lease.
- **Manager.** Listing and rights re-check; `delete_doc` / `delete_space` across every kit
  table.
- **Agent parity framework.** `PersonAction`, 31 rows (exhaustive), the `KIT_VERBS` registry,
  pack verbs (`Pack::verbs`), and the harness comparing the person path with the agent path.
- **Conformance suite** for text. Two genuine Yrs deviations are pinned:
  `text.yata_separating_case` and `text.yata_paper_a19`.
- **Hardening.**
  - Shape pre-checks on outside bytes; client-byte decoding gated behind the off-by-default
    `untrusted-wire` feature.
  - A deterministic, counted-work diff.
  - Encode paths never panic on a pending doc.

**Verified locally (CI-size):** the full test suite (unit, integration, property, the
convergence simulator at 40 seeds, the disk simulator, tier-0 large-file and crash tests,
golden tool parity against hand-written GNU expectations, 57 parity cases), plus clippy
`-D warnings`, rustdoc, and the licence gate.

**Pending, to run on a Linux compute node** (the same tag; any failure ships as v0.1.1):
- the live GNU comparison of the 407 tool cases (`scripts/gnu-parity.py`);
- the convergence sweep at ≈ 200 seeds and the disk simulator at 40+ seeds;
- a 10-minute, 32-participant soak;
- the revert-carry benchmark at R = 10^5 (≤ 20 µs p95).

**Not in this release:** erasure (purge + compaction + byte scan); blame; the WS sync protocol
and JS client; the rich-text, list, map and tree kinds.

# Sessions

The contract a host codes against: `k2_mpc_base::session`. A session is one loaded doc
plus the people and agents in it. The host owns identity, permission, time, transport and
files; the kit owns the doc, its history, presence and merge rules.

This slice is the **command-line path**: participants send text edits and the kit makes every
engine update itself. Nothing here decodes bytes from a client (`untrusted-wire` stays off).

## Pieces

| Type | Who provides it | What it is |
|---|---|---|
| `Host` | host | `rights(who, key)`, `now_ms()`, `on_event(&AuditEvent)` |
| `Pack` | each file type | `schema()`, `decode(bytes)`, `encode(content, codec)`, `normalize_input(root, text)` |
| `TextFilePack` | kit | a plain-text pack in one line: `TextFilePack::new("markdown", 4 << 20)` (root `source`) |
| `SessionManager` | kit | one per store: `open`, `open_file`, `get`, `rekey`, `tick`, `poll_disk`, `expire_idle`, `unload_idle`, `stats`, `release`; tier 0: `with_blob_store`, `open_blob`, `open_blob_file`, `get_blob` |
| `Session` | kit | one per loaded doc (`Arc<Session>`, `Send + Sync`) |
| `ConnId` | kit | one connection (a tab, a `watch`, a CLI caller); `Copy`, passed to every call |
| `Config` | host | every limit below; `Config::default()` then set fields |
| `tools` | kit | the text verbs (`cat`, `grep`, `sed`, `patch`...) and `Session::run_tool` ([Tools](#tools)) |
| history | kit | `Session::undo_with`, `redo_with`, `revert`, `restore`, `name_version`, `versions`, `log`, `diff_version`, `show` ([Undo and redo](#undo-and-redo), [Versions and history](#versions-and-history), [Revert](#revert)) |

**Host rules.** `Host` methods are called **with a session locked**. They must be quick and
must not call back into the kit, or they deadlock. Events carry metadata only, never doc content.

## Lifecycle

1. `SessionManager::new(Arc<DocStore>, Arc<dyn Host>, Config)`.
2. `open(pack, &key, initial_bytes)`:
   - returns the loaded session if there is one;
   - otherwise loads the doc from the store;
   - otherwise imports `initial_bytes`. The pack decodes them, and the doc, the import update
     (by the reserved participant `kit:import`, via `disk`) and an `Import` version are stored
     in **one transaction**.

   Errors:
   - `Declined { code, message }`: the pack refused the file (`too_large`, `not_utf8`,
     `binary`, `mixed_line_endings`). Nothing is stored.
   - `NotFound`: no stored doc and no bytes.
   - `PackMismatch`: the stored doc belongs to another pack.

   `open_file(pack, &key, path, root)` is the same for a doc that lives in a file, which
   the session then keeps in sync ([Disk sync](#disk-sync)). `open` stays for docs without a
   file (tests, in-memory use).
3. Join to get a **connection**. Both calls return `Joined { conn: ConnId, client, version }`.
   - `join(participant, Stay)` always opens a new connection. Use it for anything with its own
     lifetime: a tab's socket (`Stay::Held`), `watch` or `open` (`Stay::Held`).
   - `join_shared(participant, Stay)` reuses the pid's existing shared connection with the same
     `via`, or opens one. Use it for one-shot verbs (`Stay::UntilIdle`), so an agent running many
     verbs keeps one connection and one client ID. Every client ID stays in the doc's state
     for good.
4. Every per-participant call takes the `ConnId`. It is `Copy`, doesn't borrow the session,
   and is unique in the process. A connection that left, expired or was kicked, or that
   belongs to another session or to before an unload, gets `NotJoined { conn }`: join again.
5. Leaving:
   - `leave(conn)` closes one connection. The pid stays present while it has others.
   - `kick(pid)` closes all of the pid's connections.
   - If the host stops granting read, all of the pid's connections close.
   - Idle expiry: `expire_idle()` closes connections without a hold after `idle_leave_ms`
     (60 s) with no call. `touch(conn)` keeps a connection present.
6. Unloading. A session can be unloaded when it has **no connections and no handle outside the
   pool**. It unloads after `idle_unload_ms` (10 min, `unload_idle()`), or least recently used
   first while the pool is over `memory_budget_bytes` (checked on every `open`). Unloading
   loses nothing, and it compacts a log of `unload_compact_entries` or more first.
7. `release()` is for process hand-off. Every file is written back, every session closes,
   and later calls on them fail with `Released`. The kit drops its store handles. Once the host drops its own, the redb file
   is free for the next process (`DocStore::open_with_retry`).

The host drives time: call `tick()` on a timer (about once a second). It closes idle
connections, cuts idle versions, runs version retention and the revert window (every
`history_tick_ms`), runs the disk stat backup and due write-backs, and unloads idle sessions,
all on `Host::now_ms`, and returns a `TickReport { left, written, ingested, unloaded, errors,
versions_cut, versions_thinned, reverts_dropped, .. }`.
(`expire_idle()` and `unload_idle()` remain for hosts without files.) The kit starts no
threads of its own. `recheck_rights()` per session is the host's periodic rights re-check
(PRD §11.2).

## Identity, connections and attribution

- `Participant { pid, name, colour, kind, via, home, on_behalf_of }` is **stamped by the host**
  from its own authentication: `Participant::new(pid, name, colour, kind, via)`, then
  `.home(..)` / `.on_behalf_of(..)`. `pid` is the host's stable id. It must not be empty or
  start with `kit:` (the kit's own: `kit:import`, `kit:external`).
- **One pid, many connections.** A person may have two tabs, or run a CLI call while a
  `watch` runs. Each connection gets its own fresh, random 53-bit client ID. It is never 0
  or 1, and never one this doc has used before (the store's client table is checked).
  - The client ID is recorded durably as the pid's (`store.set_client`).
  - Two tabs never share a client ID, which a Yjs doc requires.
- Every change is logged with `author = pid`, the connection's client ID, `at_ms`, `via` and
  `on_behalf_of`. Blame reads the client ID→pid mapping, so all of a pid's connections
  attribute to that pid.

### On-behalf edits and approvals (K-ID-10, K-ID-11)

- The host sets `on_behalf_of: Some(person_pid)` **only** for an assistant agent acting for
  that person. It must not be empty, reserved, or the agent's own pid (`BadParticipant`).
- The change is **the agent's own**: its pid, client ID, undo stack, rate limit, and its own
  rights (the host is asked about the agent; acting for someone never widens them, K-PERM-10).
- Edits are **never held**: they apply at once like any edit. The log entry carries
  `on_behalf_of`, and the store records `Approval { state: Pending, .. }` keyed `(doc, seq)`
  **in the same transaction** as the entry. `ChangeEvent` carries `on_behalf_of`, and the
  roster's `participant.on_behalf_of` shows "agent for person" (K-PRES-13).
- `join_shared` reuses a connection only for the same `(pid, via, on_behalf_of)`.
- `set_approval(conn, seq, Approved | Declined)`: only the connection's pid **equal to that
  change's `on_behalf_of`** may do it; anyone else (the agent included) gets
  `Forbidden { need: Approver }` and an `AuditEvent::Refused`. Pending → approved or declined,
  **once**: repeating the same state returns the record, the other state or `Pending` is
  `BadApproval`, and a change without an approval is `NoApproval { seq }`. It emits
  `AuditEvent::Approval { key, seq, pid, state }` (metadata only). A declined change stays
  applied: the person's `revert`, an explicit edit, a restore or the agent's undo reverses it.
  New on-behalf edits are held past the pending cap ([Revert](#revert)).
- `approval(conn, seq) -> Option<Approval>` and `approvals(conn)` (for `log --pending`) need
  read; the author and the person see their own records, everything else needs `history`.
- `Approval { state, by, at_ms, author, on_behalf_of }` survives compaction (the log entry may
  not).

**What is per pid and what is per connection:**

| Per participant (pid) | Per connection | Per doc |
|---|---|---|
| rights (asked on every call), undo/redo stacks, edit rate limit, "contributors since last read", roster entry | client ID, caret, viewing range, visibility, idle timer, hold, subscriptions | read bases |

## Rights

`Rights { read, edit, history }` (`#[non_exhaustive]`): `Rights::none()`, `read_only()`,
`editor()` (read + edit), `full()` (read + edit + history), and `.with_history(bool)`.
**History is never implied** by read or edit; the host grants it explicitly (K-PERM-9: owner
and editors by default, not App guests or read-only shares). It gates the history reads
(`versions`, `log`, `diff_version`, `show`; blame later), `restore` (with edit), and other
people's approval records. `revert` needs edit and is allowed for the change's author, the
person it was made for, or an owner (`Rights::owner`; refused as `Forbidden { need: Owner }`).

- The host is asked on **every** call, for the connection's pid: read for every call, and
  edit on every edit call (`edit`, `edit_with`, `replace_text`, `write_base`, `undo`, `redo`,
  `revert`, `restore`, `name_version`, `set_cursor`), and history on history reads.
- A refusal returns `Forbidden { need }` and emits `AuditEvent::Refused { pid, action, need }`.
- A pid found without read is removed on the spot, with **all** its connections
  (`Left { reason: RightsLost }` for each), and their subscriptions close.
- `recheck_rights()` asks once per pid and returns `RightsChange { pid, rights }` for each pid
  whose rights changed, so the host can tell its tabs they went read-only.

## Reading

`read(conn)` → `ReadResult`:
- `bytes`: the file's bytes now (`pack.encode`, with BOM and CRLF restored);
- `text_by_root`;
- `version`;
- `contributors_since_last_read`: other pids that changed the doc since this pid's previous
  read (through any of its connections), oldest first, excluding the import. On a first read,
  it covers changes since the session loaded.

Every read records a **read base**: that version's text, valid for `write_base`. The cache
belongs to the doc. It keeps the last `read_bases_max` (64) versions, each for
`read_base_ttl_ms` (30 min), and at most `read_bases_max_bytes` (16 MiB) in all. The live
version and any stored version (import, automatic and named versions) are always valid bases;
an unknown or expired base is `BaseExpired`, exit 8.

## Editing

Every edit call is **one atomic change**:
- inserted text is normalised by the pack;
- it is applied as the connection's client ID;
- it is appended to the store in **one durable transaction before the call returns**.

It returns `EditResult { version, changed: Vec<LineRange>, lines_added, lines_removed }`. The
`(+a −r)` counts match a line diff. A call that changes nothing returns the same version and
an empty `changed`.

| Call | Use |
|---|---|
| `edit_with(conn, root, f)` | **Read live, compute, apply, atomically.** Runs `f(&live_text) -> Result<String, E>` under the session lock, diffs to minimal hunks, and applies them as one change. Returns `Ok(None)` when nothing changed. `f`'s error comes back as `EditWithError::Compute(e)`, and refusals as `EditWithError::Session(_)`; nothing is applied in either case. |
| `edit(conn, Vec<Edit>, if_version)` | Hunks in byte offsets, at most one `Edit` per root. With `if_version: Some(V)`, a doc that has moved past V gives `Stale { expected, actual }` and nothing is applied. Invalid hunks (out of range, inside a character, overlapping) change nothing. |
| `write_base(conn, root, base, text)` | `write --base V`: `merge3(text at V, text, live)`. Applies only what the caller changed since V. Overlap → `Conflicts(Vec<Conflict>)` and nothing applied; two different appends at the end of the text both land, the live one first (rule O5). Unknown or expired V → `BaseExpired` ("re-read and retry"). |
| `replace_text(conn, root, text)` | Make a root equal `text` through a minimal diff. Last-writer-wins for that root: for people who mean "replace whatever is there" (`write --base live`). |

### Which to use

| Caller | Path | Why |
|---|---|---|
| **Edit verbs** (`sed -i`, `patch`, `insert`, `append`, `replace-section`; the C6 tools) | `edit_with` | The verb computes on exactly the text it changes. Nothing can slip in between. |
| **Clients sending hunks** (positions from a read or an editor) | `edit` + `if_version` | Positional hunks are only right at the version they were computed on. `Stale` means re-read and recompute. |
| **Whole-file rewrites from an older read** (`write --base V`, `cat … \| tool \| write`) | `write_base` | The caller's changes since V merge with everyone else's, and overlaps are reported, never overwritten. |
| `--base live` | `replace_text` | Deliberate last-writer-wins. Agents get a warning from the host. |

`edit_with` rules: `f` runs **with the session locked**. It must be bounded (no I/O; regexes
with size limits) and must not call back into the kit. A panic in `f` is re-raised to the
caller after the lock is released, so the session stays usable.

After an edit, the connection's caret sits at the end of its last hunk.

Failure handling: if the store write fails, or the engine reports the in-memory doc
**poisoned** (`DocError::Pending`: an apply left unresolved dependencies, failed or panicked
after decoding), the session reloads from the store, so memory again matches what is durable.
A poisoned doc is never encoded, compacted or written back from. Carets, views and undo history are reset
(`AuditEvent::Recovered`). If the reload also fails, the session closes (`Broken`).

## Undo and redo

Undo follows the kit's undo spec v1.4 (`.k2/prds/spec-undo-v1.md`, U1–U22, O1–O5), implemented
once in `session/undo.rs` over the applied-hunk stream.

- **Own batches only, per pid** (U2). One edit call (or verb, restore, revert) is one batch; a
  pid's tabs, devices and CLI calls share one stack. Undo never reverts another pid's change.
  `kit:import` and `kit:external` changes are on no stack (U4): `restore` or `revert` reverse
  them.
- **Regions are canonical hunks** (U8): a `sed -i` touching lines 3 and 300 is two regions.
  Each region keeps its text before and after the change and its bounds, carried through every
  later change like a caret (an insertion exactly at an edge is pulled inside; a deletion
  across an edge **taints** it, U9). Two regions of one batch that come to touch keep an
  insertion at their shared edge in the left one only (m-d).
- **Undoing a region** is `merge3_window(after, before, live[s'..e'], at_end)`, where `at_end`
  holds only when the region ends at the root's end **and** the root was non-empty after the
  change (the merge's base, U10, m11). Within a step, regions apply back to front.
- **Conflicts** are `ConflictRegion { root, range (live bytes), lines (1-based), kind: Overlap |
  Taint | Erased, base, ours, theirs }`: one per region, never a sub-range (U11).
- **Redo** (U17) reapplies exactly the regions the undo applied (skipped ones are not redone).
  A new batch by the pid clears their redo stack (U7). Undo and redo are themselves not new
  undoable batches.
- **Bounds** (U6): `undo_depth` (100) batches per stack and `undo_max_bytes` (4 MiB) per pid
  per doc, both stacks together; that pid's oldest batches go first. Stacks survive leave and
  rejoin, and are lost on unload, recovery and restart.
- Applied hunks are validated and made canonical (touching hunks coalesced) before they reach
  the engine, so every carry (carets, stacks, revert records) sees the same hunks.

`merge3` is built on `diff::hunks`, which is a pure function of its input (undo spec D1–D3):
no clock, a counted work budget, a pinned tie-break. So undo, redo and `write --base` give the
same result on every machine and under any load.

### Undo modes

`undo(conn, n)` / `redo(conn, n)` are all-or-nothing; `undo_with(conn, &UndoRequest)` /
`redo_with` take `UndoRequest { steps, mode, for_person }` (`UndoRequest::steps(n)
.mode(..).for_person(..)`).

| Mode | Behaviour | Outcome |
|---|---|---|
| `UndoMode::All` (`undo`) | Plan the newest N batches newest first on a scratch copy; any conflicting region applies **nothing** and reports **every** conflicting region of the first conflicting step (U13). Otherwise all N steps apply as one store append. | `Applied { steps, result, skipped: [], blocked: None }` or `Conflicts { step, conflicts }` |
| `UndoMode::Partial` (`--partial`; a person's Ctrl+Z) | Per step, apply every clean region; skip and report the rest — exactly the set `All` would report (U14, U15). A step with any applied region is consumed (redo gets only those). A step whose **every** region conflicts is not consumed: it stays on top and planning stops there. | `Applied { skipped, blocked: Some(..) if a later step stopped it }`, or `Blocked { conflicts }` when the top step is blocked |
| `UndoMode::Drop` (`--drop`) | Remove the top step without applying it (the escape from a blocked step, m-c). It doesn't go to redo. | `Dropped { change }` |
| `for_person: Some(p)` (`--for p`) | Only batches the assistant made on behalf of `p` (U2, K-HIST-21). | as above |

Nothing to do: `Nothing`; `HistoryLost` when the pid had changes before the session (re)loaded
(U18: "lost with the session; use revert"); `ClearedByPurge` once after
`Session::clear_undo_history()` (the purge hook, U19). `UndoOutcome::exit()`: 1 when anything
conflicted, was skipped or blocked, else 0.

## Versions and history

A **version** is an engine-neutral snapshot (each root's text) at a log position.

| Cut | When (K-HIST-4) | `VersionReason` |
|---|---|---|
| import | the doc is created | `Import` |
| idle | `idle_version_ms` (2 min) after the last change, on `tick` | `Idle` |
| before an agent edit | an Agent's edit changing more than `agent_version_lines` (20) lines or `agent_version_percent` (10 %) of the doc's lines (max of added, removed), in the edit's transaction | `BeforeAgent` |
| outside edit | before (when the doc moved since the last version) and after it | `BeforeExternal`, `External` |
| restore | before it, in its transaction | `BeforeRestore` |
| named | `name_version(conn, name, None)` cuts one of the live doc; `Some(v)` names version `v` (needs edit) | `Named` |

Each cut is skipped when the newest version is already at the current log position.
**Retention** (K-HIST-7, every `history_tick_ms` on `tick`): every version for
`version_keep_all_ms` (30 days), then the newest per hour to `version_hourly_until_ms` (90 days),
then the newest per day; then at most `version_max_count` (500) versions and
`version_max_bytes` (64 MiB) per doc, oldest unnamed first. Named versions are never removed.
Stored versions are always valid `write_base` bases.

**History reads** (need `history`):

| Call | Verb | Returns |
|---|---|---|
| `versions(conn)` | `log` (versions part) | `Vec<VersionInfo { id, seq, at_ms, reason, name, authors, bytes }>`, oldest first |
| `log(conn, &LogQuery)` | `log [--by P] [--since T] [-n N] [--pending] [--on-behalf-of P]` | `Vec<LogItem>`, newest first: `Change { info: ChangeInfo { seq, kind, of, author, via, at_ms, on_behalf_of, lines_added, lines_removed }, approval }` and `Version(VersionInfo)`; a version at position S lists before change S. `--pending`/`--on-behalf-of` list changes only |
| `diff_version(conn, root, from, to, context)` | `diff --version V` (`to: None` = live) | `DiffOutput { text (diff -u), differs }`; `exit()` 0 same, 1 different |
| `show(conn, version)` | `show --version V` | each root's text |

`ChangeKind`: `Edit`, `Undo`, `Redo`, `Restore` (`of` = version), `Revert` (`of` = change),
`External`, `Import`. Change summaries are written in each change's transaction and kept for the
doc's life (compaction never loses them). `approvals(conn)` keeps amendment 1's rule (own records
without history).

**Restore** (`restore(conn, version)`, needs edit and history; K-HIST-8): a version of the doc
as it is is cut, then "now → V" applies as a **new change by the restorer** (`ChangeKind::Restore`).
History is never rewritten; the restorer's `undo` reverses it.

## Revert

`revert(conn, change, partial)` (K-HIST-19, spec U22) reverses one named change (its log
position, the stable change id) days later and after restarts, independent of undo stacks.

- **The record.** Every change (edits, undo/redo, restore, revert, outside edits, the import)
  writes a revert record in its own transaction: canonical hunks with the deleted text, author,
  `on_behalf_of`. It lives in the store, so purge and the byte scan reach it.
- **The carry** (m9: no per-keystroke work over all records). Bounds live in an **offset tree**
  per root and bias (`session/carry.rs`): a treap over the positions in order, each node holding
  the gap to its predecessor; a hunk splits by value and shifts everything after it by changing
  one gap, and collapses the positions strictly inside a replaced range with one lazy
  "collapse + taint" tag. **O(log R) expected per hunk** for R records; reading a record walks its
  parent pointers (O(log R)); taint is resolved lazily on read. The tree is built from the store
  on the first revert (or window pass) of a session — replaying the stored hunk shapes from the
  window's cut, O(L log R) — and then kept up to date by every change. Measured: about 160 node
  visits per change at R = 10^4 (115 at 10^3); p95 ≈ 10 µs in a debug build. The spec's BM (≤ 20 µs
  p95 at R = 10^5) is the ignored test `revert_carry_per_apply_r1e5` (release, compute node).
- **Applying** uses undo's region rules (U8–U11): all or nothing, or `partial` for every clean
  region (a fully conflicting revert applies nothing: `Blocked`, exit 1). A region over redacted
  text is `ConflictKind::Erased` (after a purge). The revert is a new change by the caller
  (`ChangeKind::Revert`, `of` = the change), on their undo stack.
- **Rights:** edit, and the caller is the change's author, its `on_behalf_of` person, or an
  owner. Otherwise `Forbidden { need: Owner }`.
- **The window** (K-HIST-23, a `tick` duty): records of changes newer than `revert_keep_ms` (30
  days) **or** among the last `revert_keep_changes` (10,000), whichever reaches further, plus
  every **pending** on-behalf change's; at most `revert_max_bytes` (64 MiB) per doc, oldest
  non-pending dropped first. Pending records older than the cut get their carried bounds
  persisted (so a rebuild never replays past it). Outside the window: `RevertTooOld { seq }`,
  **exit 8**, never retry; the way back is `restore --version`. Unknown positions:
  `NoSuchChange`, exit 4.
- **The pending cap:** while a doc holds `pending_max_records` (1,000) pending on-behalf records
  or `pending_max_bytes` (16 MiB) of them, new on-behalf edits (edit, `edit_with`, tools,
  `replace_text`, `write_base`, `restore`) are refused with `Held { reason: "approvals_backlog" }`,
  exit 2, until the person approves or declines some. Undo, redo and revert are not held (they
  reverse changes). A pending record is kept whatever the window says, and released to it when
  settled.

## Presence

`roster(conn)` needs read. It has **one entry per participant (pid)**, however many connections
they have. Editors come first, then viewers, then away participants; within each group the
order is by first join. Each `RosterEntry` has:
- `connections`: how many connections the pid has open;
- `participant`: as stamped on the most active connection;
- `role`: from the most active connection (latest edit or caret move). It is `Editor` when the
  pid has edit rights **and** that connection edited or placed a caret within
  `editor_window_ms` (30 s). Otherwise `Viewer`. Readers are always viewers.
- `away`: true only when **every** connection has been not visible
  (`set_visible(conn, false)`) for `away_after_ms` (60 s);
- `cursor: Option<CursorAt>`: the caret in byte offsets plus `line:col` for `follow`, from the
  most active connection that has one;
- `view: Option<LineRange>`: the lines they are looking at, chosen the same way;
- `last_edit_ms`: the latest across connections;
- `stay`: `Held` if any connection is held.

Carets and views (per connection):
- `set_cursor(conn, root, anchor: At, head: At)` needs edit. `At::Byte(n)` or
  `At::Line(LineCol { line, col })`, 1-based, with columns counted in characters.
- `set_view(conn, LineRange)` needs read. Readers use this instead of a caret.
- Stored positions are byte offsets carried through every change by
  `position::transform`:
  - before a hunk: unchanged;
  - strictly inside a replaced range: snaps to the hunk's start;
  - after it: shifted.
- Helpers in `session::position`: `line_col`, `offset`, `line_of`, `line_start`, `line_count`.

## Events

`subscribe(conn)` needs read. It returns a `Receiver<ChangeEvent { version, pid, client, via,
root, hunks, at_ms, on_behalf_of }>` with one event per root per change. The hunks are in the coordinates of
the text before the change.
- Each receiver queues `subscriber_queue` (1024) events. A receiver that falls that far behind
  is **cut off**: it drains what was queued, then sees the channel disconnected, and should
  re-read and subscribe again.
- Versions are consecutive, so a gap is visible.
- A subscription closes when its connection leaves, or when its pid is kicked or loses read.

Audit events `Joined` and `Left` carry both `pid` and `conn`.

## Disk sync

For a session opened with `open_file`, **the file on disk stays true**: the doc is written
back to it, and edits made to it by other programs are merged in. The kit does the file I/O
(std only) but never watches: the host's watcher calls `file_changed()`, and `tick()` runs a
cheap stat backup. The three edit paths above are unchanged; disk sync sits beside them.

**Opening.** The host resolves and jails `path`. The kit checks again: `root` must be a
folder, and `path` must be a regular file (not a folder, device or pipe) resolving, symlinks
included, **inside `root`**. Otherwise `BadPath`.
- A new doc is imported from the file (it must exist; `Declined` as for `open`).
- A stored doc is **reconciled**: if the file isn't what the doc last synced (`synced_hash`),
  the difference is merged in as an outside edit (an edit made while no session was loaded),
  and changes the file never got (a crash before write-back, K-DISK-10) are written by the
  next write-back. A file that is gone opens `Missing`.
- Opening and closing an untouched file **never writes it** (bytes and mtime unchanged).

**Write-back (doc → file).** Any change marks the session dirty. It is written when the doc
has been quiet `write_quiet_ms` (1 s), or `write_max_delay_ms` (5 s) after the first unwritten
change, on `flush(conn)`, on unload, and on `release()`. Each write:
1. checks the file first: an outside edit is merged in before anything is written, so the
   kit never overwrites content it hasn't seen (the window left is between that read and the
   rename);
2. renders with `pack.encode` (BOM and line endings restored) and skips if the bytes equal
   what the file holds;
3. is **atomic**: a temp file beside it (`.<name>.~mp<pid>-<n>.tmp`; `is_write_temp(name)`
   tells a watcher to ignore it), the old file's permissions, fsync, rename over, fsync the
   folder. Symlinks are resolved, so a link stays a link and its target is written. Temp files
   a crash left are removed on open;
4. records the new synced hash, position and state. The record is written **before** the
   rename and promoted after it, so a crash in between is recognised on the next open (the
   file matches the write in flight) instead of looking like an outside edit;
5. remembers the last 8 written hashes for **echo suppression**.

If the file was deleted, replaced by a folder, or re-pointed outside the root, nothing is
written: the state is `Missing`, the doc and its edits are kept, `AuditEvent::FileMissing
{ unflushed }` is emitted, and the host decides the grace period (K-DISK-7, -17). When the
file comes back, it is checked like any outside edit.

**Outside edits (file → doc).** On `file_changed()`, or when `check_disk()` / `tick()` finds
the size, mtime or inode changed (a rename-save changes the inode):
- the file holds the last synced content → nothing;
- it holds an older write of the kit's (an echo, or an editor saving a stale buffer) → not
  an edit (`OwnWrite`); the next write-back puts the doc back;
- otherwise: a version "before external edit" is cut (when the doc moved since the last
  version), the diff from the **last synced text to the new file text** is applied as the
  reserved participant `kit:external` (`EXTERNAL_PID`, its own client ID per doc,
  `via: "disk"`, no `on_behalf_of`), and a version "external edit" is cut, all in one store
  transaction. The diff is made on the doc **as the file held it** and merged into the live
  doc as a CRDT change, so edits typed since the last write-back are kept, and the next
  write-back writes the merged result. Subscribers get a `ChangeEvent` from `kit:external`;
  the host gets `AuditEvent::ExternalEdit { seq }`.
- A file that is the same text in other bytes (CRLF ↔ LF, a BOM added) adopts the new format.

The disk is never the merge authority: the session is never replaced with the file's content
wholesale, and no CRDT metadata is ever written into the file.

**Declined.** A file that stops decoding (binary, not UTF-8, too large, mixed line endings) is
never written. The doc is marked `declined` (stored), edits are refused with `Declined`, and
`AuditEvent::FileDeclined { code }` is emitted for the host to tell participants. When the file
decodes again it is merged in and the doc is live again (`FileRestored`).

**Renames.** A rename the host paired: `SessionManager::rekey(old, new)` for the key and
`Session::file_moved(new_path)` for the file (re-checked like `open_file`). An unpaired rename
is a delete (`Missing`) plus a new file.

**Line-aligned edits.** A minimal diff can express "add a line" as an insertion that starts
mid-line (`"7\nagent-1"` after `agent-1`). As a CRDT change, a concurrent line added at the
same spot would then land inside it. So the edit paths that diff (`edit_with`,
`replace_text`, outside edits) slide pure line insertions to line starts
(`text::lines::align_insertions`; same resulting text). `edit` takes hunks as given.

| Call | What it does |
|---|---|
| `SessionManager::open_file(pack, &key, path, root)` | open, import or reconcile, as above |
| `SessionManager::tick()` | idle expiry, stat backup (every `disk_poll_ms` while anyone is present; also on join), due write-backs, idle unload |
| `SessionManager::poll_disk()` | the stat check on every loaded file now |
| `Session::file_changed()` | the host's watcher saw a change: read and merge |
| `Session::check_disk()` | read only if size, mtime or inode changed |
| `Session::file_moved(new_path)` | the file now lives at `new_path` |
| `Session::flush(conn)` | needs read; `FlushOutcome { written, hash (SHA-256 hex of the doc's bytes), disk }` |
| `Session::file_path()` | the path as given |

`DiskState`: `Clean`, `Dirty { since_ms }`, `Missing`, `Declined(reason)`. `DiskCheck`:
`Unchanged`, `OwnWrite`, `Ingested { version }`, `Missing`, `Declined(Declined)`.

Host duties: watch the file's folder (editors save by rename) and call `file_changed()`, with
a short debounce, since a watcher may fire mid-write and the kit merges what it reads; ignore
`is_write_temp` names; key sessions by file identity; decide the grace after `Missing`.

## Tools

`Session::run_tool(conn, root, argv, stdin) -> Result<ToolOutput, ToolRunError>` runs one
command-line verb on a live doc (`k2_mpc_base::tools`). `argv[0]` is the verb; the
host strips the doc path and passes the rest as given. The verbs are pure functions over
text (no I/O, no programs run, linear-time regexes with size limits, output at most
`tools::MAX_OUTPUT_BYTES`); every text pack reuses them.

| Verb | Kind | Supported (GNU semantics for this subset) |
|---|---|---|
| `cat` | read | `-n -b` |
| `head` | read | `-n [-]N`, `-c [-]N`, `-NUM` (GNU size suffixes) |
| `tail` | read | `-n N`, `-n +N`, `-c [+]N`, `-NUM` (`-f` is the host's `watch`) |
| `wc` | read | `-l -w -c -m` (prints the doc name, as `wc FILE`) |
| `grep` | read | `-E -F -G -e -i -y -v -w -x -c -o -q -s -l -L -H -h -n -m N -A/-B/-C N -NUM --color=never` |
| `sed` | read | as `sed -i` below, printing the result |
| `diff` | read | `-u -U N --unified[=N] --label L` (twice); the old text on stdin; exit 1 when different |
| `view` | read | `--lines a,b` (`$` allowed) |
| `sed -i` | edit | `s///` (`g N i I p`, `& \0-\9 \n \t`), `d p a i c y = q`, `#` comments; addresses `N $ /re/ \cREc` (`I`), `a,b`, `a,+N`, `!`; `-n -e -E -r -s --sandbox`; GNU `-i` and BSD `-i ''` |
| `patch` / `apply` | edit | one file's unified diff on stdin; `-p N -R --dry-run (--check) -F N --partial -s` |
| `append` | edit | stdin's text; `--ensure-newline` |
| `insert` | edit | `--line N`, `--after N` (whole lines, like `sed 'Ni\'`/`'Na\'`), `--at L:C` (as is) |
| `replace` | edit | `--lines a,b` (like `sed 'a,bc\'`), `--range L:C-L:C` (half-open, as is) |
| `delete` | edit | `--lines a,b` (like `sed 'a,bd'`), `--range L:C-L:C` |

- **Read verbs** need read. They read as `read` does, in the same lock: a **read base** is
  recorded (so `write_base(V)` works from the version printed), "contributors since last
  read" moves on, and line-ranged reads (`head -n`, `tail -n`, `view`, `sed -n 'a,bp'`)
  set the connection's **viewing range** (P4). `ToolOutput.fence` carries the fence.
- **Edit verbs** need edit and run inside `edit_with`: the tool computes on the live text
  and the result applies as **one change** (or nothing does). The caret ends at the last
  hunk; one `undo` reverts one verb. `patch --dry-run` is an edit verb that never applies
  (it still needs edit and takes a rate-limit token).
- **Bytes and text.** Byte tools (`cat` without flags, `wc`, `head -c`, `tail -c`) see the
  file's exact bytes (`pack.encode`: BOM and CRLF restored). Line tools and edit verbs see
  the doc text (`\n`, no BOM); edits are encoded back by the pack, so a CRLF file stays CRLF.
- **Refused, nothing applied** (exit 2): unsupported flags or commands
  (`ToolError::Unsupported { flag, hint }`); `sed` `e w r R W` and the `s` flags `e w`, backup
  suffixes (`-i.bak`, `-i .bak`, `-in`, `--in-place=SUF`: versions replace backups) and
  backreferences in a pattern (`ToolError::Refused`); malformed input (`Usage`); bounds
  (`Limit`). A pattern or script is compiled before the doc is touched.
- **Patches are all or nothing** by default: a failing hunk applies nothing, exit 1, and the
  report shows each failing hunk with the live lines where it was expected. `--partial`
  applies what fits and prints the rejects. Hunks are located as GNU patch does (offsets,
  fuzz up to `-F` (2), start/end anchoring for one-sided context).
- **Regexes** are GNU BRE/ERE translated to a linear-time engine, matched leftmost-longest
  as POSIX requires. Refused: backreferences; escapes GNU doesn't define (`\d`).

`ToolOutput { stdout, stderr_summary, exit, version, changed: Option<EditResult>, fence:
Option<Fence> }`:
- `stdout` is byte-exact (for edit verbs, `patch`'s report; else empty);
- `stderr_summary`: `notes.md v1842` after a read; `edited notes.md lines 12–18 (+3 −1) ·
  v1843` after an edit (the host adds `as <name>`); `no change to …` / `nothing applied to …`;
- `exit`: 0, or 1 for the tool's "no" (grep found nothing, diff found differences, a patch
  hunk failed). A "no" is not an error.

**The fence** (PRD §9.6, K-SEC-8). `Fence { nonce, name, version, contributors }`: a fresh
64-bit nonce per call from `/dev/urandom` (or, when that can't be read, a per-process random
hasher key mixed with the clock and a counter), redrawn if it occurs in the output;
`with_nonce(..)` takes the host's own. `begin(prefix, label)` / `end(prefix)` render the
lines; the host picks the prefix (its CLI's name), renders pids (`Sam (person)`) and decides
`--fence` / `--no-fence`. Control characters in names are replaced, so a name can't break a
fence line. The doc's display name is the key's `path`.

**Exit codes** are mapped in one place, the requirements' one table (K-EXIT-1):
`tools::exit::{OK 0, NO 1, USAGE 2, HELD 2, REFUSED 3, NOT_FOUND 4, LEASED 5, STORE 6,
LINK_LOST 7, TOO_OLD 8}`, `ToolError::exit()` (always 2), `ToolRunError::exit()`,
`tools::session_exit(&SessionError)` (total: every variant is listed, see Errors),
`WriteOutcome::exit()` (conflict 1, `BaseExpired` 8), `UndoOutcome::exit()` and
`DiffOutput::exit()`. Exit 8 never carries a retry-after. Usage errors exit 2 for every verb,
where GNU sed and the coreutils use 1.

**History verbs are session methods, not tools** (decision, C7a): `undo`, `redo`, `revert`,
`restore`, `version --name`, `log`, `diff --version` and `show --version` need rights the text
tools don't (history, owner) and return structured values the host renders; `run_tool` stays
the pure text verbs.

**One doc per call.** A second file operand is unsupported; a host grepping several docs
runs the verb per doc, passes `-H` when there are several, and combines the exits.

## Status and the pool

- `status()` is metadata only and needs no connection. It returns
  `Status { key, version, participants (distinct pids), connections, log_entries,
  estimated_bytes, declined, disk }`. When `declined` is set, edits are refused with
  `Declined`. `disk` is `None` without a file, else a `DiskState` (see Disk sync).
- `SessionManager::stats()` returns `PoolStats { loaded, estimated_bytes, budget_bytes,
  unloaded_total }`.
- `SessionManager::expire_idle()` returns `(DocKey, ConnId, pid)` for each connection closed.
- A session's estimate is its encoded state (snapshot plus log, plus the disk-sync state for
  a file) × `memory_factor` (20). The host exposes `memory_budget_bytes` (256 MiB) as its
  knob.

## Limits (`Config`)

| Field | Default | Effect |
|---|---|---|
| `editor_window_ms` | 30 s | editor role window |
| `idle_leave_ms` | 60 s | connections without a hold close |
| `away_after_ms` | 60 s | not visible → away |
| `read_bases_max` / `read_base_ttl_ms` / `read_bases_max_bytes` | 64 / 30 min / 16 MiB | `write_base` bases |
| `undo_depth` / `undo_max_bytes` | 100 / 4 MiB | undo history, per pid per doc (spec U6) |
| `idle_version_ms` | 2 min | idle version after the last change |
| `agent_version_lines` / `agent_version_percent` | 20 / 10 | version before a large agent edit |
| `version_keep_all_ms` / `version_hourly_until_ms` | 30 / 90 days | retention bands |
| `version_max_count` / `version_max_bytes` | 500 / 64 MiB | version cap per doc |
| `revert_keep_ms` / `revert_keep_changes` / `revert_max_bytes` | 30 days / 10,000 / 64 MiB | the revert window |
| `pending_max_records` / `pending_max_bytes` | 1,000 / 16 MiB | the pending on-behalf cap (`approvals_backlog`) |
| `history_tick_ms` | 1 min | how often `tick` runs retention and the window per loaded doc |
| `edit_rate_per_s` / `edit_burst` | 10 / 10 | Edit calls per **pid** per doc, across their connections. A token bucket on the host clock; the bucket survives leave and rejoin. Over the limit → `RateLimited { retry_after_ms }`. A call takes a token once rights pass, even if it then fails. |
| `subscriber_queue` | 1024 | events per subscriber |
| `compact_entries` / `compact_bytes` | 1,000 / 2 MiB | compact after an append at either threshold |
| `unload_compact_entries` | 100 | compact on unload |
| `memory_budget_bytes` / `memory_factor` / `idle_unload_ms` | 256 MiB / 20 / 10 min | the pool |
| `write_quiet_ms` / `write_max_delay_ms` | 1 s / 5 s | write-back after this much quiet, or this long after the first unwritten change |
| `disk_poll_ms` | 3 s | stat backup interval while anyone is present |
| `disk_read_max_bytes` | 64 MiB | larger files are declined unread (`too_large`) |

## Errors (`SessionError`)

| Error | Meaning | Exit (`session_exit`) |
|---|---|---|
| `Forbidden { need }` | No right (`Read`, `Edit`, `History`, `Approver`: not the person the change was for; `Owner`: a revert by someone not its author or person). For read, the pid is also removed. | 3 |
| `Held { reason }` | `approvals_backlog`: too many pending on-behalf changes; nothing applied | 2 |
| `RevertTooOld { seq }` | the change is outside the revert window; use `restore --version` | 8 (never retry) |
| `NoSuchChange(n)` | no change (revert) or version (restore, show, diff, name) `n` | 4 |
| `NotJoined { conn }` | The connection isn't in this doc: join again | 2 (or rejoin silently) |
| `Stale { expected, actual }` | `edit(.., if_version)` but the doc moved; nothing applied | 1 (re-read and retry) |
| `RateLimited { retry_after_ms }` | over the edit rate | 2 |
| `Declined(Declined)` | file can't be live, or live editing is off | 4 or "opens as today" |
| `NotFound` | no doc and no bytes | 4 |
| `PackMismatch` | stored doc belongs to another pack | 4 |
| `BadParticipant` / `BadPosition` / `BadEdit` / `Doc(_)` | bad input; nothing changed | 2 |
| `NoApproval { seq }` / `BadApproval` | no approval to settle / already settled the other way | 2 |
| `BadPath` | `open_file` / `file_moved`: not a regular file inside the root | 2 |
| `Disk(_)` | reading or writing the file failed; nothing lost, write-back retries | 1 |
| `Store(_)` | store failure; nothing acked | 6 |
| `Pack(_)` | pack bug or damaged codec settings | 1 |
| `Unloaded` / `Released` / `Broken` | stale handle: open again | retry |
| `Leased` / `NoLease` / `Blob(_)` / `NoBlobStore` | tier 0: see [Tier 0](#tier-0-any-file) | 5 / 2 / 1 (`TooLarge` 4) / 4 |

`edit_with` returns `EditWithError<E>`: either `Session(SessionError)` or `Compute(E)` (the
tool's own error, e.g. "pattern not found": exit 1).

A conflict is not an error. It is a value (`WriteOutcome::Conflicts`, `UndoOutcome::Conflicts`
/ `Blocked`): exit 1, and nothing is applied. `WriteOutcome::BaseExpired` is exit 8.

## Parity rows served (`k2-mpc-markdown/docs/PARITY.md`)

| # | Verb | Session API |
|---|---|---|
| P1 | `open` (any verb joins) | `SessionManager::open` + `join` (`Stay::Held`) for `open`; `join_shared` (`UntilIdle`) for one-shot verbs |
| P2 | `leave` / idle | `leave(conn)`; `expire_idle` |
| P3 | `cat [-n]` | `run_tool` (`cat`: `read`'s bytes) |
| P4 | `head`, `tail`, `sed -n`, `view --lines` | `run_tool` (read + viewing range) |
| P5 | `grep` | `run_tool` (`grep`) |
| P6 | `who` | `roster` (one entry per pid, with a connection count) |
| P7 | `follow` | `roster` (`CursorAt` line:col; an agent's caret is at its last edit) |
| P8 | `cursor --at L:C` / `--select` | `set_cursor(At::Line(..))`; readers: `set_view` |
| P9–P11 | `insert`, `replace`, `delete` | `run_tool` (inside `edit_with`) |
| P12 | `sed -i` | `run_tool` (inside `edit_with`) |
| P13 | `patch`, `append`, `replace-section` | `run_tool` (inside `edit_with`); `replace-section` is the Markdown pack's |
| P14 | `write --base V` / `--base live` | `write_base(V)` / `replace_text` |
| P15 | formatting | as P9–P13 (Markdown formatting is text) |
| P16 | `undo`, `redo`, `undo --drop` | `undo` / `redo`, `undo_with` / `redo_with` (`UndoMode::All`, `Partial`, `Drop`; `for_person`) |
| P17 | `status`, `flush` | `status` (`disk`); `flush(conn) -> FlushOutcome { written, hash, disk }` |
| P18 | `watch`, `tail -f` | `join(Stay::Held)` + `subscribe` |
| P19 | `log`, `diff --version`, `show --version` | `log(LogQuery)`, `diff_version`, `show`, `versions` (need history) |
| P20 | `restore`, `version --name` | `restore` (a forward change by the restorer, after a version), `name_version` |
| P21 | `mv` | `SessionManager::rekey` + `Session::file_moved` |
| P22 | `blame` | later (client→pid mapping is recorded now) |
| P24 | export | `read` (`bytes`) |
| P25 | why live editing is off | `open` → `Declined { code, message }`; `status().declined` |
| P26 | Ctrl+Z (`undo --partial`) | `undo_with(UndoMode::Partial)` (the native tab's grouping is G3) |
| P31 | `revert <change>` | `revert(conn, change, partial)` (author, the person, or owner; too old: exit 8) |

## Not in this slice

- **Disk, host side and later rows:** watching (the host's), the 24 h grace after a delete
  and K-DISK-17's named version of unflushed edits (the host, from `FileMissing { unflushed }`),
  one session per file identity (K-DISK-14: the host keys by device + inode), hardlinks
  (K-DISK-15), synced-folder conflict copies (K-DISK-16), the git branch-switch rule
  (K-DISK-13).
- **C5, wire:** the socket, Yjs sync and awareness, each socket being a `join` connection.
  This needs the strict decoder.
- **C6, rest:** `write --base --partial` (K-BR-9), intent ranges; `replace-section` (the
  Markdown pack's, on `edit_with`); the `spec_undo::model` oracle and trace test (K-HIST-14/15).
- **After C7a:** blame (C7b); purge (redacting revert records, versions, the byte scan:
  `DocStore::revert_records` is the hook, `clear_undo_history` clears the stacks); `diff
  --version` from the **disk** (the host reads the file and uses the `diff` tool); thinning
  change summaries (kept for the doc's life today); retention for docs that are never loaded
  again (it runs on `tick` for loaded ones); the native tab's grouping and Ctrl+Z with unacked
  ops (G3).

## Tier 0: any file

Every file type gets presence, a lease, versions and restore on day one, with **no type
pack**: images, PDFs, office files, binaries. A **blob doc** is a pointer to a blob; its
versions are the pointer's history. Blobs are never CRDT values (`PRIMITIVES.md`).

| Type | Who provides it | What it is |
|---|---|---|
| `blob::BlobStore` | kit, opened by the host | content-addressed bytes: `BlobStore::open(Arc<DocStore>, dir, BlobConfig, clock)` |
| `BlobPack` | file type (optional) | `can_merge`, `merge3_bytes(base, ours, theirs)`, `conflict_copy_name`; all defaulted |
| `AnyFile` | kit | tier 0 for any file: no merge, the default conflict-copy name |
| `BlobSession` | kit | one per loaded blob doc (`Arc`, `Send + Sync`) |

The host gives the manager a blob store once: `SessionManager::new(..).with_blob_store(blobs)`.
Then `open_blob_file(Arc::new(AnyFile), &key, path, root)` (path rules as `open_file`), or
`open_blob(pack, &key, Option<&mut dyn Read>)` for a doc without a file. A text doc and a blob
doc never open as each other (`PackMismatch`; blob docs are stored with pack `kit:blob`).

### The blob store

- **Where the bytes live:** files under a folder the host chooses
  (`<dir>/<space no>/<xx>/<64 hex>`), **not in redb**: a 1 GB video in the store would bloat
  its file, compactions and every backup. Only the **index** is in the `DocStore` (each
  blob's size and reference count, the per-space key, blob docs' versions and sync state), so
  a version and the reference it takes on its blob commit in one transaction. Keep the folder
  on the store's volume and back them up together.
- **Keyed addresses:** a blob's id is `HMAC-SHA256(space key, bytes)`, with a random 32-byte
  key per `DocKey::space`, kept in the store. Equal files in two spaces get unrelated ids (no
  cross-space equality), and a small file can't be brute-forced from its id (K-T0-4).
  `purge_space_key(space)` destroys the key and every blob stored under it; the same bytes
  stored again get a new, unrelated id. (K-T0-4 names HMAC-BLAKE3; HMAC-SHA256 is used here,
  std only, on the kit's FIPS-checked SHA-256; the record carries an algorithm byte.)
- **Verified reads:** `BlobReader` recomputes the HMAC and checks the length as it streams;
  the read that reaches the end fails (`InvalidData`) on a mismatch, and every later read
  fails too. A host forwarding a stream must treat only a clean end as success. The kit's own
  copies (write-back, conflict copies) go to a temp file renamed only after a verified end,
  and `read_to_vec` returns only verified bytes, so a damaged blob is never served or
  written.
- **Streaming:** `put(space, &mut dyn Read)`, `id_of`, `reader`, `copy_to` use 64 KiB
  buffers. A 50 MB file imports, reads, and is replaced and written back in under 100 KB of
  heap (`tests/tier0_large.rs`).
- **References and GC:** blob-doc versions hold references (and `retain`/`release`, for a
  pack's own uses). `gc()` removes a blob only after it has been unreferenced for
  `BlobConfig::gc_grace_ms` (1 h, host clock), so a writer's upload is never collected before
  it commits; it also sweeps files without an index row (a crash between storing and
  indexing) and purged spaces' folders. `BlobConfig::max_blob_bytes` (4 GiB) caps a blob.
- `stats()` (and `PoolStats::blob_count`, `blob_bytes`, `blob_unreferenced_bytes`).

### Versions

Each `BlobVersion` records `{ blob, size, mtime_ms, at_ms, author, author_name, via,
on_behalf_of, reason, name, restored_from, conflict_copy }`. The current pointer is the latest
version (`status().current`). Reasons: `Import`, `Write`, `External`, `Restore`, `Merged`.

- `versions(conn)`, `read_version(conn, v)` need history.
- `restore(conn, v)` needs edit and history: a **new** version pointing at v's blob,
  attributed to the caller (`Restore`, `restored_from: v`). History is never rewritten. The
  lease rule applies; no base (deliberate, like `--base live`).
- `name_version(conn, v, name)` needs edit. Retention reuses the store's rule:
  `DocStore::delete_blob_version` refuses a named version (`NamedVersion`) and the current one
  (`CurrentVersion`), and releases the blob's reference. Nothing deletes versions on its own.

### Presence

The same presence code as text sessions (`session/presence.rs`): one roster entry per pid,
connections, holds, away, idle expiry, rights asked on every call, `kick`, `recheck_rights`.
There are no carets. `set_region(conn, Option<String>)` (needs read) sets an opaque view
region such as `"page 3"` (at most 256 bytes), shown as `RosterEntry::region`. The lease
holder has `holds_lease` and shows as an editor. Alone, the roster lists only you (K-PRES-5).
`join` returns `client: 0` (a blob doc has no CRDT clients).

### The lease

| Call | Rights | What it does |
|---|---|---|
| `acquire_lease(conn, ttl)` | edit | take it, or extend it if your pid holds it. `ttl` defaults to `lease_default_ms` (5 min, K-T0-2), capped at `lease_max_ms` (1 h) |
| `renew_lease(conn, ttl)` | edit | extend your lease; `NoLease` when nobody holds it |
| `release_lease(conn)` | read | release yours; false if you don't hold it |
| `break_lease(conn)` | **owner** | break whoever's (audited `LeaseBroken { by, holder }`); returns it |
| `lease()` | none | the lease now (metadata, also in `status()`) |

- **One holder at a time**, a pid: any of its connections may write, renew or release. It
  survives `leave` (a CLI agent leases in one call and writes in the next) and keeps the
  session loaded; it ends on expiry (host clock: checked on every call and on `tick`,
  `LeaseEnded { how: Expired }`, `TickReport::leases_expired`), release, a break, or when the
  holder is kicked or loses read.
- Someone else holding it gives **`SessionError::Leased { holder, name, expires_ms }`**:
  suggested **exit 5** (`leased`), printing the holder and expiry. **Leases never block
  reads.**
- `Rights::owner` is new and never implied (`Rights::full().with_owner(true)`); the host
  decides who owns a doc.
- Leases live in memory, like presence: a restart drops them.

### Writing

`write(conn, &mut dyn Read, base)` / `write_bytes(conn, &[u8], base)`, needs edit:

1. **The lease:** someone else holding it gives `Leased`, nothing written. With no lease held,
   the write takes one for its own length (`auto`, at most `lease_auto_ms`, 2 min), so a
   second writer is told at once instead of after uploading; it is released when the write
   ends.
2. **A current base:** `base` must be the current version, else **`Stale { expected: base,
   actual }`** (exit 1: re-read and retry). Checked before the upload and again at commit.
   Never last-writer-wins.
3. The upload streams into the blob store **without the session lock** (presence calls go
   on). The new version is durable before the call returns. Equal bytes record nothing
   (`changed: false`).
4. Write-back to the file: atomic (C4's helper, streaming from the verified blob), the write
   in flight recorded before the rename so a crash is recognised on the next open, the file's
   stat recorded after. A failed write-back leaves the version recorded and the disk `Dirty`;
   `tick` retries. Returns `BlobWriteResult { version, blob, size, changed, written }`.

### Disk: outside saves, conflicts, merges

As for text docs, the kit reads and writes the file but never watches: the host calls
`file_changed()`, and `tick()` runs the stat backup (`disk_poll_ms`). The file is hashed only
when its size, mtime or inode changed (the synced stat is stored, so reopening an unchanged
1 GB file doesn't re-hash it).

- The file holds the current version, or what the kit last synced → `Unchanged`.
- It holds an older write of the kit's (an app saving a stale buffer, a late echo; the last 8,
  in memory) → `OwnWrite`; the next write-back puts the current version back.
- Otherwise it is an **outside save**: the file is stored as a version by `kit:external`
  (`via: "disk"`, `External`, the file's mtime) → `Ingested { version }`,
  `AuditEvent::ExternalEdit { seq: version }`.
- **Desktop apps ignore leases.** An outside save is a **conflict** when it lands while
  anyone holds the lease, or while the doc has a version the file doesn't have yet (a
  write-back pending or failed). Then:
  - **tier 0.5:** if the pack `can_merge()` and base, ours and theirs are each at most
    `blob_merge_max_bytes` (32 MiB), `merge3_bytes(base, ours, theirs)` is asked. `Merged(b)`
    records the save (`External`) and the merge (`Merged`, by `kit:merge`), writes the merge
    back, and returns `DiskCheck::Merged { version }` (`AuditEvent::BlobMerged`);
  - **tier 0 (no merge, or `Conflict`/`Unsupported`):** **the save stays current**, and what
    it replaced stays a version **and** is written to a **conflict copy** beside the file:
    `name (conflict YYYY-MM-DD HHMMSS from <who>).ext` (UTC, host clock; `who` is the replaced
    version's author name, or the lease holder's when the kit wrote it; never over an existing
    file; `BlobPack::conflict_copy_name` may rename it). The copy's name is recorded on the
    save's version (`conflict_copy`, so a purge can find it). Returns
    `DiskCheck::Conflict { version, kept, copy }` and emits `AuditEvent::BlobConflict
    { current, kept, copy, lease_holder }`: tell both the lease holder and the person.
    The holder's next write names the old base and gets `Stale`.
  - **Why the save wins:** the person just saved it and their app shows it; putting the
    session's version back would overwrite the file under their app, which would save
    again. Nothing is lost either way: both are versions, and the session's is also a file
    beside theirs. If even the copy can't be written (a read-only folder), the version is
    still kept (`copy: None`, `DiskError` event).
- A file over `max_blob_bytes` is declined (`FileDeclined { code: "too_large" }`); writes
  are refused until it shrinks. A missing file stops write-back (`Missing`), as for text.

`flush(conn)` writes the current version now (`FlushOutcome::hash` is the blob id).
`file_moved`, `file_path`, `check_disk` work as for text docs. A blob doc can't be
`rekey`ed to another space (its blobs are keyed by space).

### Status, config and errors

`status()` → `BlobStatus { key, version, current, participants, connections, lease, disk }`.

| `Config` field | Default | Effect |
|---|---|---|
| `lease_default_ms` / `lease_max_ms` | 5 min / 1 h | lease length when not given / the cap |
| `lease_auto_ms` | 2 min | a write's own lease, if the writer dies mid-upload |
| `blob_merge_max_bytes` | 32 MiB | tier-0.5 merges are tried only below this |

| Error | Meaning | Suggested exit |
|---|---|---|
| `Leased { holder, name, expires_ms }` | someone else holds the lease; nothing written | **5** |
| `Stale { expected, actual }` | `write` from an old base; nothing written | 1 |
| `NoLease` | renewing when nobody holds it | 2 |
| `Forbidden { need: Owner }` | breaking a lease without `Rights::owner` | 3 |
| `Blob(BlobError)` | `TooLarge` (4), `NotFound`/`Damaged`/`Io`/`Store` (1) | 1 or 4 |
| `NoBlobStore` | the host gave the manager no blob store | 4 |

**Proposed for the host:** a "lease requested, waiting for the holder" state (a card) is the
host's, not the kit's; the kit only ever says `Leased`. Exit 2 already means "bad input /
rejoin / rate limited", so a separate code (e.g. 6) or `Leased` plus a `pending_request`
field in `--json` would be clearer than reusing 2.

**Not yet (tier 0):** per-blob purge and the purge byte scan over conflict copies
(K-PURGE-3; `conflict_copy` records the names), approvals for on-behalf blob writes (the
version records `on_behalf_of`), automatic version retention, the K-T0-4 fuzz target, and
hashing outside the session lock (a large outside save is hashed under it today).

## Store: history tables

Five tables beside the log, created on open (the store format stays 2): `changes` (summary and
hunk shape per change; `DocStore::change`, `scan_changes`), `reverts` (revert records),
`revert_index`, `revert_carry`, `revert_stats`; plus `DocStore::version_infos` (version headers
without content). Content lives only in `reverts` (and versions); purge's byte scan must cover
both. Changes made before this slice have no summary or record (`revert` says too old).

## Store: the host's table

`DocStore` has a small key-value table for the host's own records next to the docs (same
transactions, hand-off and backups): `host_put(ns, key, value)`, `host_get`, `host_delete`,
`host_scan(ns, prefix) -> Vec<(key, value)>` (key order, one consistent read), and
`host_batch(&[HostOp])` for several writes in one transaction. Each call is one durable
transaction. Namespaces and keys are opaque strings: **the kit never reads, interprets or
erases these rows**, so erasing them (a person's data, a deleted workspace) is the host's job.

## Changelog

### Amendment 4: history (C7a)

**Breaking:**
1. `UndoOutcome` is `#[non_exhaustive]`; `Applied` gains `skipped` and `blocked`, `Conflicts`
   carries `Vec<ConflictRegion>` (one per region, live coordinates, a `kind`) instead of
   merge3's `Vec<Conflict>`; new variants `Blocked`, `Dropped`, `HistoryLost`,
   `ClearedByPurge`.
2. `Config::undo_max_bytes` is now **per pid per doc** (both stacks), default **4 MiB** (was
   32 MiB per session), per spec U6.
3. `tools::session_exit` follows K-EXIT-1: `Store` exits **6** (was 1), `Leased` **5** (was 1),
   `Blob(TooLarge)` and `NoBlobStore` 4.
4. Behaviour: whole-text `merge3` (and `write_base`) on an **empty** base no longer lands two
   different writes (O5 needs a non-empty doc, K-BR-11): it conflicts.
5. Behaviour: undo regions are per canonical hunk (a two-place edit can now partly conflict);
   hunks given to `edit` are coalesced when touching before they apply.

**Additive:** `Session::undo_with`, `redo_with`, `revert`, `restore`, `name_version`,
`versions`, `log`, `diff_version`, `show`, `clear_undo_history`; `UndoRequest`, `UndoMode`,
`ConflictRegion`, `ConflictKind`, `LogQuery`, `LogItem`, `DiffOutput`, `WriteOutcome::exit`,
`UndoOutcome::exit`; `store::ChangeInfo`, `ChangeKind`, `VersionInfo`, `DocStore::change`,
`scan_changes`, `version_infos`; `SessionError::Held`, `RevertTooOld`, `NoSuchChange`;
`Action::Revert`, `History`; `TickReport::versions_cut`, `versions_thinned`,
`reverts_dropped`; `tools::exit::{HELD, LEASED, STORE, LINK_LOST, TOO_OLD}`; `Config` fields
in the Limits table. Store: five new tables (format stays 2). Versions are now cut
automatically (idle, before large agent edits, before restores).

### Rename (2026-10-27)

The repo and crate are now `k2-mpc-base` (lib `k2_mpc_base`); the Markdown pack is
`k2-mpc-markdown` (lib `k2_mpc_markdown`). Paths: `../k2-mpc-base/crates/base`.
**Breaking:** `StoreError` is `#[non_exhaustive]` (add a wildcard arm if you match it).

### Amendment 3 (additive): tier 0

Nothing existing changes behaviour. New: the `blob` module (`BlobStore`, `BlobId`,
`BlobConfig`, `BlobError`, `BlobReader`, `BlobInfo`, `GcReport`, `PurgeReport`, `BlobStats`,
`Clock`); `session::BlobSession`, `BlobPack`, `AnyFile`, `BlobMerge`, `BlobRead`,
`BlobStatus`, `BlobWriteResult`, `LeaseInfo`, `LeaseEnd`, `conflict_copy_name`, `MERGE_PID`,
`REGION_MAX_BYTES`; `SessionManager::with_blob_store`, `blob_store`, `open_blob`,
`open_blob_file`, `get_blob`; `store::BlobVersion`, `BlobReason`, `BLOB_DOC_PACK`;
`DocStore::blob_versions`, `blob_version`, `blob_current`, `name_blob_version`,
`delete_blob_version`. Fields: `Rights::owner` (+ `with_owner`; never implied, not in
`full()`), `RosterEntry::region`, `holds_lease`, `PoolStats::blob_sessions`, `blob_count`,
`blob_bytes`, `blob_unreferenced_bytes`, `TickReport::leases_expired`, `conflicts`,
`Config::lease_default_ms`, `lease_max_ms`, `lease_auto_ms`, `blob_merge_max_bytes`.
Variants (on `#[non_exhaustive]` enums): `Need::Owner`; `Action::Lease`, `BreakLease`,
`Restore`, `Versions`; `AuditEvent::LeaseAcquired`, `LeaseEnded`, `LeaseBroken`,
`BlobConflict`, `BlobMerged`; `DiskCheck::Conflict`, `Merged`; `SessionError::Leased`,
`NoLease`, `Blob`, `NoBlobStore`. `StoreError::NoBlobSpace`, `NoBlob`, `CurrentVersion` are
added the way amendment 1 added `NoApproval` (that enum isn't `#[non_exhaustive]`; a host
matching it exhaustively needs a new arm). Store: four new tables, created on open; the format
stays 2 (nothing existing is re-encoded, and an older build reads a blob doc as another
pack's, `PackMismatch`). Internally, presence moved to `session/presence.rs`, shared by text
and blob sessions.

### Amendment 2 (additive): tools (C5 + C6)

`Session::run_tool`; the `tools` module: `Tool` (`parse`, `kind`, `verb`, `is_dry_run`,
`read`, `edit`), `ToolDoc`, `ReadOutput`, `EditOutput`, `Kind`, `VERBS`, `ToolOutput`,
`ToolError`, `ToolRunError`, `Fence`, `random_nonce`, `unified`, `exit`, `session_exit`,
`MAX_OUTPUT_BYTES`. Dependency: `regex-automata` 0.4 (MIT OR Apache-2.0). Nothing existing
changed behaviour; `read` now shares its code with the tools' read path.

### Engine hardening (after C4)

- **Breaking (Rust API, not the session contract):** `Doc::encode_state()` returns
  `Result<Vec<u8>, DocError>`. yrs 0.28 panics encoding a doc that holds pending updates
  (an unwrap at transaction.rs:408); every engine encode now runs under `catch_unwind`, and a
  doc an apply left pending (or that failed or panicked mid-apply) is **poisoned**: it refuses
  `encode_state`, `encode_diff`, `edit_as` and further applies with `DocError::Pending`
  until reloaded. A decode error poisons nothing.
- `check_client_update` (`untrusted-wire`) accepts blocks from other clients that the doc
  already has, as real Yjs resyncs carry held pending updates; anything past the doc's state
  from another client is still `ForeignClient` (rule in `ARCHITECTURE.md`, rule 4).
  `inserts_from` lists only genuinely new content.
- Sessions treat `Pending`, `Panic` and `Apply` from the engine as poisoning: the call fails,
  the session reloads from the store (`AuditEvent::Recovered`), and compaction or write-back
  never encodes from such a doc (`CompactFailed` then reload).

### C4: disk sync (additive)

`SessionManager::open_file`, `tick`, `poll_disk`, `TickReport`; `Session::file_changed`,
`check_disk`, `file_moved`, `flush`, `file_path`; `Status::disk`; `DiskState`,
`DiskCheck`, `FlushOutcome`, `is_write_temp`; `Config::write_quiet_ms`,
`write_max_delay_ms`, `disk_poll_ms`, `disk_read_max_bytes`; `Action::Flush`;
`AuditEvent::ExternalEdit`, `FileMissing`, `FileDeclined`, `FileRestored`, `DiskError`;
`SessionError::BadPath`, `Disk`; `DocStore::client_owned_by`, `last_version`;
`text::lines::align_insertions`. Imports now also record the disk-sync state. Behaviour:
`edit_with` and `replace_text` line-align pure line insertions (same text, different hunks).

### Amendment 1 (after `e307e43`)

Breaking changes, and only these:

1. **`Rights`** gains `history` and is `#[non_exhaustive]`: build it with `Rights::none()`,
   `read_only()`, `editor()`, `full()`, `.with_history(bool)` instead of a struct literal.
   `Rights::NONE` / `READ` / `EDIT` remain as aliases of the first three (no history).
2. **`Participant`** gains `on_behalf_of: Option<String>` and is `#[non_exhaustive]`: build it
   with `Participant::new(pid, name, colour, kind, via)` and `.home(..)` / `.on_behalf_of(..)`.
3. **`LogEntry`** gains `on_behalf_of: Option<String>`. Log records are now version 2;
   version-1 records still decode (as `None`). The store format is 2: a format-1 store is
   stamped 2 on open with nothing rewritten, and a build that reads only format 1 refuses a
   format-2 store (`StoreError::Newer`).
4. **`Need`** gains `History` and `Approver`, and **`Action`** gains `Approve` and
   `ReadApproval`; both enums are now `#[non_exhaustive]` (add a `_` arm), so later variants
   don't break hosts again.

Additive: `ChangeEvent::on_behalf_of`; `Session::set_approval`, `approval`, `approvals`;
`Approval`, `ApprovalState` (in `store`, re-exported from `session`);
`AuditEvent::Approval`; `SessionError::NoApproval`, `BadApproval`;
`StoreError::NoApproval`; `DocStore::approval`, `approvals`, `set_approval`; the host table
(`host_put`, `host_get`, `host_delete`, `host_scan`, `host_batch`, `HostOp`);
`EXTERNAL_PID` (`kit:external`, used by disk sync).

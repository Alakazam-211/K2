# Sessions

The contract a host codes against: `k2_multiplayer_base::session`. A session is one loaded doc
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
| `SessionManager` | kit | one per store: `open`, `open_file`, `get`, `rekey`, `tick`, `poll_disk`, `expire_idle`, `unload_idle`, `stats`, `release` |
| `Session` | kit | one per loaded doc (`Arc<Session>`, `Send + Sync`) |
| `ConnId` | kit | one connection (a tab, a `watch`, a CLI caller); `Copy`, passed to every call |
| `Config` | host | every limit below; `Config::default()` then set fields |

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
connections, runs the disk stat backup and due write-backs, and unloads idle sessions, all on
`Host::now_ms`, and returns a `TickReport { left, written, ingested, unloaded, errors }`.
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
  applied: an explicit edit, a restore or the agent's undo reverses it.
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
and editors by default, not App guests or read-only shares). It gates history reads (versions,
log, diff, show, blame: C7a) and other people's approval records today.

- The host is asked on **every** call, for the connection's pid: read for every call, and
  edit on every edit call (`edit`, `edit_with`, `replace_text`, `write_base`, `undo`, `redo`,
  `set_cursor`), and history on history reads.
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
version and any stored version (the import today; versions in C7a) are always valid bases.

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

`undo(conn, steps)` and `redo(conn, steps)` act only on the caller's own batches. One edit call
is one batch.
- **Undo is per participant (pid), across their connections.** It is person-level: undo
  through one tab reverts the pid's last edit, whichever of their tabs or CLI calls made it.
  It never reverts another pid's change.
- Undo is `merge3(after, before, live)`, applied through the calling connection. If someone
  else's later edit overlaps, it returns `UndoOutcome::Conflicts { step, conflicts }` and
  applies nothing.
- `steps > 1` is all or nothing: every step is planned before any is applied, then each step
  is logged as its own change in one store append.
- Redo reverses undo. A new edit by the pid clears their redo stack.
- `UndoOutcome::Nothing` means the stack is empty.
- The stacks survive leave and rejoin, but not an unload.
- Bounds: `undo_depth` (100) per pid per stack, and `undo_max_bytes` (32 MiB) per session,
  oldest batches first.

A batch stores only the **region** it changed (text before and after), not whole texts. The
region's bounds move through every later change. A later deletion that crosses a region's
edge makes that undo report a conflict (conservative; see `src/session/undo.rs`). The region
is merged with `merge3_window`, so the end-of-text rule (O5) applies only when the region ends
where the doc does, as in a whole-text merge.

`merge3` is built on `diff::hunks`, which is a pure function of its input (undo spec D1–D3):
no clock, a counted work budget, a pinned tie-break. So undo, redo and `write --base` give the
same result on every machine and under any load.

### Undo modes

The requirements PRD fixes one undo semantics for both engines:
- **`undo`, all or nothing (built here).** Every requested step applies, or none does, and the
  overlapping regions are reported as conflicts.
- **`undo --partial` (C6/C7a, not built yet).** Applies what it can and reports the rest.
- A person's Ctrl+Z in a native editor is **partial**: their editor's undo manager reverts
  what it can of their own changes.

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
| `undo_depth` / `undo_max_bytes` | 100 / 32 MiB | undo history |
| `edit_rate_per_s` / `edit_burst` | 10 / 10 | Edit calls per **pid** per doc, across their connections. A token bucket on the host clock; the bucket survives leave and rejoin. Over the limit → `RateLimited { retry_after_ms }`. A call takes a token once rights pass, even if it then fails. |
| `subscriber_queue` | 1024 | events per subscriber |
| `compact_entries` / `compact_bytes` | 1,000 / 2 MiB | compact after an append at either threshold |
| `unload_compact_entries` | 100 | compact on unload |
| `memory_budget_bytes` / `memory_factor` / `idle_unload_ms` | 256 MiB / 20 / 10 min | the pool |
| `write_quiet_ms` / `write_max_delay_ms` | 1 s / 5 s | write-back after this much quiet, or this long after the first unwritten change |
| `disk_poll_ms` | 3 s | stat backup interval while anyone is present |
| `disk_read_max_bytes` | 64 MiB | larger files are declined unread (`too_large`) |

## Errors (`SessionError`)

| Error | Meaning | Suggested exit |
|---|---|---|
| `Forbidden { need }` | No right (`Read`, `Edit`, `History`, or `Approver`: not the person the change was for). For read, the pid is also removed. | 3 |
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
| `Store(_)` | store failure; nothing acked | 1 |
| `Pack(_)` | pack bug or damaged codec settings | 1 |
| `Unloaded` / `Released` / `Broken` | stale handle: open again | retry |

`edit_with` returns `EditWithError<E>`: either `Session(SessionError)` or `Compute(E)` (the
tool's own error, e.g. "pattern not found": exit 1).

A conflict is not an error. It is a value (`WriteOutcome::Conflicts`, `UndoOutcome::Conflicts`):
exit 1, and nothing is applied.

## Parity rows served (`k2-multiplayer-markdown/docs/PARITY.md`)

| # | Verb | Session API |
|---|---|---|
| P1 | `open` (any verb joins) | `SessionManager::open` + `join` (`Stay::Held`) for `open`; `join_shared` (`UntilIdle`) for one-shot verbs |
| P2 | `leave` / idle | `leave(conn)`; `expire_idle` |
| P3 | `cat [-n]` | `read` (`bytes`) |
| P4 | `head`, `tail`, `sed -n`, `view --lines` | `read` + `set_view` |
| P5 | `grep` | `read` (+ the tools module, C6) |
| P6 | `who` | `roster` (one entry per pid, with a connection count) |
| P7 | `follow` | `roster` (`CursorAt` line:col; an agent's caret is at its last edit) |
| P8 | `cursor --at L:C` / `--select` | `set_cursor(At::Line(..))`; readers: `set_view` |
| P9–P11 | `insert`, `replace`, `delete` | `edit_with` |
| P12 | `sed -i` | tools (C6) inside `edit_with` |
| P13 | `patch`, `append`, `replace-section` | tools (C6) inside `edit_with` |
| P14 | `write --base V` / `--base live` | `write_base(V)` / `replace_text` |
| P15 | formatting | as P9–P13 (Markdown formatting is text) |
| P16 | `undo`, `redo` | `undo`, `redo` (per pid; `--partial` later) |
| P17 | `status`, `flush` | `status` (`disk`); `flush(conn) -> FlushOutcome { written, hash, disk }` |
| P18 | `watch`, `tail -f` | `join(Stay::Held)` + `subscribe` |
| P19 | `log`, `diff --version`, `show --version` | versions and history (C7a); the store already lists versions |
| P20 | `restore`, `version --name` | C7a (restore = `replace_text` as a `Restore` participant, after a version) |
| P21 | `mv` | `SessionManager::rekey` + `Session::file_moved` |
| P22 | `blame` | later (client→pid mapping is recorded now) |
| P24 | export | `read` (`bytes`) |
| P25 | why live editing is off | `open` → `Declined { code, message }`; `status().declined` |

## Not in this slice

- **Disk, host side and later rows:** watching (the host's), the 24 h grace after a delete
  and K-DISK-17's named version of unflushed edits (the host, from `FileMissing { unflushed }`),
  one session per file identity (K-DISK-14: the host keys by device + inode), hardlinks
  (K-DISK-15), synced-folder conflict copies (K-DISK-16), the git branch-switch rule
  (K-DISK-13).
- **C5, wire:** the socket, Yjs sync and awareness, each socket being a `join` connection.
  This needs the strict decoder.
- **C6, tools:** grep, sed, patch and replace-section (run through `edit_with`), and
  `undo --partial`.
- **C7a, history:** automatic and named versions, restore, history and diff, blame, stored
  versions as `write --base` bases beyond the import, and possibly `undo --partial`.

## Store: the host's table

`DocStore` has a small key-value table for the host's own records next to the docs (same
transactions, hand-off and backups): `host_put(ns, key, value)`, `host_get`, `host_delete`,
`host_scan(ns, prefix) -> Vec<(key, value)>` (key order, one consistent read), and
`host_batch(&[HostOp])` for several writes in one transaction. Each call is one durable
transaction. Namespaces and keys are opaque strings: **the kit never reads, interprets or
erases these rows**, so erasing them (a person's data, a deleted workspace) is the host's job.

## Changelog

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

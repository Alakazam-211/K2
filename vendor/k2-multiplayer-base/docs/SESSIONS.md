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
| `SessionManager` | kit | one per store: `open`, `get`, `rekey`, `expire_idle`, `unload_idle`, `stats`, `release` |
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
7. `release()` is for process hand-off. Every session closes, and later calls on them fail
   with `Released`. The kit drops its store handles. Once the host drops its own, the redb file
   is free for the next process (`DocStore::open_with_retry`).

The host drives time: call `expire_idle()` and `unload_idle()` on a timer (a few seconds is
fine). `recheck_rights()` per session is the host's periodic rights re-check (PRD §11.2).

## Identity, connections and attribution

- `Participant { pid, name, colour, kind, via, home }` is **stamped by the host** from its own
  authentication. `pid` is the host's stable id. It must not be empty or start with `kit:`.
- **One pid, many connections.** A person may have two tabs, or run a CLI call while a
  `watch` runs. Each connection gets its own fresh, random 53-bit client ID. It is never 0
  or 1, and never one this doc has used before (the store's client table is checked).
  - The client ID is recorded durably as the pid's (`store.set_client`).
  - Two tabs never share a client ID, which a Yjs doc requires.
- Every change is logged with `author = pid`, the connection's client ID, `at_ms` and `via`.
  Blame reads the client ID→pid mapping, so all of a pid's connections attribute to that pid.

**What is per pid and what is per connection:**

| Per participant (pid) | Per connection | Per doc |
|---|---|---|
| rights (asked on every call), undo/redo stacks, edit rate limit, "contributors since last read", roster entry | client ID, caret, viewing range, visibility, idle timer, hold, subscriptions | read bases |

## Rights

- The host is asked on **every** call, for the connection's pid: read for every call, and
  edit on every edit call (`edit`, `edit_with`, `replace_text`, `write_base`, `undo`, `redo`,
  `set_cursor`).
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
| `write_base(conn, root, base, text)` | `write --base V`: `merge3(text at V, text, live)`. Applies only what the caller changed since V. Overlap → `Conflicts(Vec<Conflict>)` and nothing applied. Unknown or expired V → `BaseExpired` ("re-read and retry"). |
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

Failure handling: if the store write fails, the session reloads from the store, so memory
again matches what is durable. Carets, views and undo history are reset
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
edge makes that undo report a conflict (conservative; see `src/session/undo.rs`).

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
root, hunks, at_ms }>` with one event per root per change. The hunks are in the coordinates of
the text before the change.
- Each receiver queues `subscriber_queue` (1024) events. A receiver that falls that far behind
  is **cut off**: it drains what was queued, then sees the channel disconnected, and should
  re-read and subscribe again.
- Versions are consecutive, so a gap is visible.
- A subscription closes when its connection leaves, or when its pid is kicked or loses read.

Audit events `Joined` and `Left` carry both `pid` and `conn`.

## Status and the pool

- `status()` is metadata only and needs no connection. It returns
  `Status { key, version, participants (distinct pids), connections, log_entries,
  estimated_bytes, declined }`. When `declined` is set, edits are refused with `Declined`.
- `SessionManager::stats()` returns `PoolStats { loaded, estimated_bytes, budget_bytes,
  unloaded_total }`.
- `SessionManager::expire_idle()` returns `(DocKey, ConnId, pid)` for each connection closed.
- A session's estimate is its encoded state (snapshot plus log) × `memory_factor` (20). The
  host exposes `memory_budget_bytes` (256 MiB) as its knob.

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

## Errors (`SessionError`)

| Error | Meaning | Suggested exit |
|---|---|---|
| `Forbidden { need }` | No right. For read, the pid is also removed. | 3 |
| `NotJoined { conn }` | The connection isn't in this doc: join again | 2 (or rejoin silently) |
| `Stale { expected, actual }` | `edit(.., if_version)` but the doc moved; nothing applied | 1 (re-read and retry) |
| `RateLimited { retry_after_ms }` | over the edit rate | 2 |
| `Declined(Declined)` | file can't be live, or live editing is off | 4 or "opens as today" |
| `NotFound` | no doc and no bytes | 4 |
| `PackMismatch` | stored doc belongs to another pack | 4 |
| `BadParticipant` / `BadPosition` / `BadEdit` / `Doc(_)` | bad input; nothing changed | 2 |
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
| P17 | `status`, `flush` | `status`; `flush` is disk sync (C4) |
| P18 | `watch`, `tail -f` | `join(Stay::Held)` + `subscribe` |
| P19 | `log`, `diff --version`, `show --version` | versions and history (C7a); the store already lists versions |
| P20 | `restore`, `version --name` | C7a (restore = `replace_text` as a `Restore` participant, after a version) |
| P21 | `mv` | `SessionManager::rekey` |
| P22 | `blame` | later (client→pid mapping is recorded now) |
| P24 | export | `read` (`bytes`) |
| P25 | why live editing is off | `open` → `Declined { code, message }`; `status().declined` |

## Not in this slice

- **C4, disk sync:** write-back, external edits, `flush`, setting `declined` on a live doc.
- **C5, wire:** the socket, Yjs sync and awareness, each socket being a `join` connection.
  This needs the strict decoder.
- **C6, tools:** grep, sed, patch and replace-section (run through `edit_with`), and
  `undo --partial`.
- **C7a, history:** automatic and named versions, restore, history and diff, blame, stored
  versions as `write --base` bases beyond the import, and possibly `undo --partial`.

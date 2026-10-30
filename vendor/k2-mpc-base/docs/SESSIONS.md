# Sessions

The contract a host codes against: `k2_mpc_base::session`. A session is one loaded doc
plus the people and agents in it. The host owns identity, permission, time, transport and
files; the kit owns the doc, its history, presence and merge rules.

This slice is the **command-line path**: participants send text edits and the kit makes every
engine update itself. Nothing here decodes bytes from a client (`untrusted-wire` stays off).

## Pieces

| Type | Who provides it | What it is |
|---|---|---|
| `Host` | host | `rights(who, key)`, `now_ms()`, `on_event(&AuditEvent)`; tier 0: `conflict_dir(file)` (default: beside the file) |
| `Pack` | each file type | `schema()`, `decode(bytes)`, `encode(content, codec)`, `normalize_input(root, text)`, `verbs()` (its own edit verbs, default none), `writes_bom(root, codec)` (default: from `encode`; [Writing a BOM file](#writing-a-bom-file)) |
| `TextFilePack` | kit | a plain-text pack in one line: `TextFilePack::new("markdown", 4 << 20)` (root `source`) |
| `SessionManager` | kit | one per store: `open`, `open_file`, `get`, `loaded_keys`, `rekey`, `tick`, `poll_disk`, `expire_idle`, `unload_idle`, `recheck_rights`, `delete_doc`, `delete_space`, `stats`, `release`; tier 0: `with_blob_store`, `open_blob`, `open_blob_file`, `get_blob`, `promote_blob_file`, `promote_blob` ([Tier 0 → text](#tier-0--text-v092)) |
| `Session` | kit | one per loaded doc (`Arc<Session>`, `Send + Sync`) |
| `ConnId` | kit | one connection (a tab, a `watch`, a CLI caller); `Copy`, passed to every call |
| `Config` | host | every limit below; `Config::default()` then set fields |
| `tools` | kit | the text verbs (`cat`, `grep`, `sed`, `patch`...) and `Session::run_tool` ([Tools](#tools)) |
| `parity` | kit | `PersonAction` (P1–P34, each for text, blob or both: `Tier`), the verb `Registry` (`for_pack`, `for_tier`), the harnesses `parity::check` and `check_blob` ([Parity](#parity)) |
| history | kit | `Session::undo_with`, `redo_with`, `revert`, `restore`, `name_version`, `versions`, `log`, `diff_version`, `show` ([Undo and redo](#undo-and-redo), [Versions and history](#versions-and-history), [Revert](#revert)) |
| agent presence | kit | `Session::claim`, `renew_claim`, `release_claim`, `claims`, `claim_remaining_ms`, `register_waiter`, `cancel_waiter`, `activity`; `ClaimPolicy` on edits; `Notes` on `run_tool` ([Agent presence](#agent-presence-amendment-13)) |

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
8. Deleting: `delete_doc(&key)` / `delete_space(space)` close the loaded sessions of those
   docs, then erase them from the store ([Deleting docs](#deleting-docs)).

The host drives time: call `tick()` on a timer (about once a second). It closes idle
connections, cuts idle versions, runs version retention and the revert window (every
`history_tick_ms`), runs the disk stat backup and due write-backs, and unloads idle sessions,
all on `Host::now_ms`, and returns a `TickReport { left, written, ingested, unloaded, errors,
versions_cut, versions_thinned, reverts_dropped, .. }`.
(`expire_idle()` and `unload_idle()` remain for hosts without files.) The kit starts no
threads of its own. `recheck_rights()` per session, or `SessionManager::recheck_rights()` over
every loaded doc, is the host's periodic rights re-check (PRD §11.2).
`SessionManager::loaded_keys()` lists the loaded docs (text and tier 0), sorted by space then
path; it never loads anything.

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
- **`approvals --all`** (= `log --pending --all`, v0.9.2): every approval record, settled
  ones too (approved, declined), newest first; `approvals` alone stays pending-only (P30).
  Bind it to `log(LogQuery::default().with_settled())` (`LogQuery::settled`); tier 0:
  `BlobSession::log` keeping the entries with an `approval`, newest first.
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
person it was made for, or an owner (`Rights::owner`; refused as
`Forbidden { need: AuthorOrOwner }`: "only the author, the person it was made for, or an owner
can revert this").

- The host is asked on **every** call, for the connection's pid: read for every call, and
  edit on every edit call (`edit`, `edit_with`, `replace_text`, `write_base`, `undo`, `redo`,
  `revert`, `restore`, `name_version`, `set_cursor`), and history on history reads.
- A refusal returns `Forbidden { need }` and emits `AuditEvent::Refused { pid, action, need }`.
- A pid found without read is removed on the spot, with **all** its connections
  (`Left { reason: RightsLost }` for each), and their subscriptions close.
- `recheck_rights()` asks once per pid and returns `RightsChange { pid, rights }` for each pid
  whose rights changed, so the host can tell its tabs they went read-only.
- `SessionManager::recheck_rights() -> Vec<(DocKey, RightsChange)>` does that for every loaded
  doc, text and tier 0, sorted by doc. A session whose re-check fails is skipped and reported
  as `AuditEvent::RecheckFailed { key, error }` (the others are still re-checked); one that
  closed meanwhile (unloaded, released, deleted) is skipped silently.

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
| `edit_with_options(conn, root, &EditOptions, f)` | `edit_with` with the host's [claim policy](#the-claim-policy-on-edits) and a version guard, both checked **under the session lock** (as `run_tool_with` does for edit verbs). `EditOptions::new().policy(p).if_version(v)` (`#[non_exhaustive]`; defaults `Ignore`, no guard). A doc no longer at V gives `EditWithError::Session(Stale { expected, actual })` and nothing applies; the version is checked before the backlog, `f` and the claims. A host's own edit route (a tab's save, a CLI's direct edit) passes the version its text came from and needs no text comparison of its own. A whole-text replacement: `|_| Ok::<_, Infallible>(new_text)`. |
| `edit(conn, Vec<Edit>, if_version)` | Hunks in byte offsets, at most one `Edit` per root. With `if_version: Some(V)`, a doc that has moved past V gives `Stale { expected, actual }` and nothing is applied (the same check as the edit verbs' `--if-version`, amendment 14). Invalid hunks (out of range, inside a character, overlapping) change nothing. |
| `write_base(conn, root, base, text)` | `write --base V`: `merge3(text at V, text, live)`. Applies only what the caller changed since V. Overlap → `Conflicts(Vec<Conflict>)` and nothing applied; two different appends at the end of the text both land, the live one first (rule O5). Unknown or expired V → `BaseExpired` ("re-read and retry"). **Known limitation until v0.9.8:** at the end of a text that ended with a line break, `write --base` can join two lines: the live text's last line left open (base `b\n`, live `a`) and the writer's line appended (`b\nZ\n`) gives `aZ\n`; the writer's text without the final break (`x`) and a line appended live (`x\nT\n`) gives `xT\n`; both ending open (base `x\n`, live `x\nA`, the writer's `x\nB`) gives `x\nAB`. Under D1 the joined word's attribution follows the writer. v0.9.8 adds outside saves' end-of-text line break to `write --base` (amendment O5 end-of-text); until then an agent appending uses `append --ensure-newline`. |
| `replace_text(conn, root, text)` | Make a root equal `text` through a minimal diff. Last-writer-wins for that root: for people who mean "replace whatever is there" (`write --base live`). |

### Which to use

| Caller | Path | Why |
|---|---|---|
| **Edit verbs** (`sed -i`, `patch`, `insert`, `append`, `replace-section`; the C6 tools) | `edit_with` | The verb computes on exactly the text it changes. Nothing can slip in between. |
| **Clients sending hunks** (positions from a read or an editor) | `edit` + `if_version` | Positional hunks are only right at the version they were computed on. `Stale` means re-read and recompute. |
| **Whole-file rewrites from an older read** (`write --base V`, `cat … \| tool \| write`) | `write_base` | The caller's changes since V merge with everyone else's, and overlaps are reported, never overwritten. |
| `--base live` | `replace_text` | Deliberate last-writer-wins. Agents get a warning from the host. |
| **A host's direct edit route** (new text computed from a read at V, under the host's claim policy) | `edit_with_options` | Both checks inside the lock: nothing lands between them and the apply. |

### Writing a BOM file

The doc text never holds a byte-order mark; the codec writes it back (`TextFilePack`: BOM and
line endings restored). `cat` prints the file's bytes, **BOM included**, so an agent that
pipes `cat` into `write` sends a body starting with U+FEFF. When the doc's codec writes a
BOM in front of that root (`Pack::writes_bom(root, codec)`), `write --base V` and
`write --base live` (`write_base`, `write_base_with`, `replace_text`, `replace_text_with`)
take **that one leading U+FEFF** off the body before merging, so the file keeps exactly one
BOM. Without a BOM in the file, a leading U+FEFF is the caller's text and stays. Other edit
calls are unchanged (their text never comes from `cat` whole).

`Pack::writes_bom` is a provided method: `TextFilePack` reads its settings; the default, for
a pack whose schema is one text root, encodes that root empty with the codec and checks for
the BOM's bytes (`codec::text_file::BOM`). So a pack delegating to `TextFilePack` (Markdown)
or with its own BOM-writing encoder (a text pack) needs no change.

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

Undo follows the kit's undo spec v1.8 (`docs/SPEC-UNDO.md`, U1–U23, O0–O5, O4a, I1),
implemented once in `session/undo.rs` over the applied-hunk stream.

- **Own batches only, per pid** (U2). One edit call (or verb, restore, revert) is one batch; a
  pid's tabs, devices and CLI calls share one stack. Undo never reverts another pid's change.
  `kit:import` and `kit:external` changes are on no stack (U4): `restore` or `revert` reverse
  them.
- **Regions are canonical hunks** (U8): a `sed -i` touching lines 3 and 300 is two regions.
  Each region keeps its text before and after the change and its bounds, carried through every
  later change like a caret (an insertion exactly at an edge is pulled inside; a deletion
  across an edge **taints** it, U9). Two regions of one batch that come to touch keep an
  insertion at their shared edge in the left one only (m-d).
- **Undoing a region** merges two **exact hunk sets** on its `after` text (U10, v1.7): ours,
  the inverse of the change's own hunk, and theirs, every later hunk that touched the
  region's window, carried at element identity (`text::merge::merge3_hunks`). No string is
  diffed: in repeated text a diff can't tell another participant's identical element from
  the region's own (finding F3), so **undo, redo and revert never remove an element they
  didn't write** (U10a). `at_end` holds only when the region ends at the root's end **and**
  the root was non-empty after the change (the merge's base, U10, m11). Within a step,
  regions apply back to front, and a clean region applies the merge's own hunks.
- **Provenance, by root** (v1.8, U23; `session/prov.rs`). Undo, redo and revert put text back
  as **new elements** (new IDs, the undoer's change, U18a) that record the **root** they
  restore: the element they put back, or its own root if that was itself a restoration
  (never a chain). Every other element (edits, `write --base`, `replace_text`, `restore`, the
  import, outside edits) is its own root. A region keeps the roots of its texts, and theirs
  is read off the live window **by root**: scanning it left to right, an element whose root
  is base element *i*'s counts as base element *i* when *i* is past the last one counted;
  every other element is foreign. So text someone removed and put back is the region's
  again: when b edits inside a's region and then undoes it, **a's undo applies**; and an
  undo removes only elements whose root the change it reverses inserted (U10a). O4 takes a
  change once only when both sides make it with the same range **and the same roots** (a
  fresh `X` typed where the undo would put `X` back is not that `X`); **O4a** refuses
  (`Overlap`) to put back a root that is live, so at most one live element has any root
  (I1, asserted in debug builds). The session keeps a map of every live element's ID and
  root (chunked runs; a change costs O(chunks) to find its place and O(one chunk) to splice,
  never O(doc)) and an index of the live restored elements by root; both are built from the
  engine and the store's `provenance` rows on first need after a load (O(doc), once), then
  kept exact by every change; an outside edit or a reload drops them for a rebuild. Only
  the kit stamps provenance: no API takes a root, and nothing from a client ever does.
- **Conflicts** are `ConflictRegion { root, range (live bytes), lines (1-based), kind: Overlap |
  Taint | Erased, base, ours, theirs }`: one per region, never a sub-range (U11).
- **Redo** (U17) reapplies exactly the regions the undo applied (skipped ones are not redone).
  A new batch by the pid clears their redo stack (U7). Undo and redo are themselves not new
  undoable batches.
- **Bounds** (U6): `undo_depth` (100) batches per stack and `undo_max_bytes` (4 MiB) per pid
  per doc, both stacks together; that pid's oldest batches go first. Stacks survive leave and
  rejoin, and are lost on unload, recovery and restart.
- **Person grouping** (U3, v0.9.7, K4): an edit through `edit_with_options` or
  `edit_at_base` with `EditOptions::group(token)` (the JS client sends one per edit) joins
  its connection's **open group**, the top of the person's undo stack, so a run of
  keystrokes is one undo step. The session closes the group (the next edit is a step of its
  own) on: the group's last edit `undo_group_idle_ms` (1 s) old on the host clock (a); an
  edit not overlapping or touching the group's span (b); a space, tab or punctuation typed
  after a word character, which starts the next step (c); a newline, a step of its own (d);
  another token, no token, or any other push by the person, from any connection (e); an
  undo or redo by them (f); anyone else's change inside the span or touching it, in apply
  order (g, read off the group's batch as every change carries it). Joining composes the
  two regions into one with exact roots (`undo::compose_group`), so undoing the group is
  undoing its edits element for element; a group that undid itself (typed, then deleted)
  leaves nothing on the stack. Agents never group. Only the stack push changes: the edit
  itself, its change record and its event are as without a token.
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
  `on_behalf_of`, and (record version 2, store format 5) per hunk the **roots** of the
  elements it inserted and deleted. It lives in the store, so purge and the byte scan reach
  it (roots are element IDs and lengths, never content).
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
- **Never more lenient than undo.** For the same change and live text, revert's conflicting
  regions include undo's. Undo applies the shared-edge rule (m-d) after every change, so once
  two regions of a change touch, a deletion across their shared edge taints both; the carry
  tree moves each position on its own, so at revert time a region whose carried start touches
  the previous region's end takes that end's taint (`undo::revert_regions`). It can be
  stricter than undo (a deletion across the left region's end from before they touched also
  counts); it is never laxer (a property test over random histories holds it). This is
  the spec's rule **U22a** (v1.6), exactly; the oracle implements it and the trace test
  compares it. A persisted carry keeps **each edge's** taint (v0.8.2; a carry written by
  v0.8.1 or earlier reads as both edges tainted, its conservative reading), so a restart
  changes no revert's outcome (the trace test's restart twin checks it). **Theirs** for a
  revert (v1.7) comes from replaying the record's regions through the stored shape of every
  later change (positions and lengths, kept for the doc's life): O(L) in the changes since,
  at revert time. **By root** (v1.8): the region is read by the record's roots, so a revert
  acts on the live elements whose root its change inserted: a change undone and then redone
  has its roots live again (the redo put them back), so **its revert applies** (v1.7
  conflicted), and what a revert puts back records the roots it restores. A record written
  before store format 5, or of an outside edit (its hunks are a diff of the texts, not
  element positions), has no roots: it is reverted as in v1.7, and what it puts back is
  fresh text.
- **Rights:** edit, and the caller is the change's author, its `on_behalf_of` person, or an
  owner. Otherwise `Forbidden { need: AuthorOrOwner }` (and `AuditEvent::Refused` with it).
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

## Blame

`blame(conn, root, lines: Option<LineRange>) -> Blame` (K-HIST-11, K-HIST-18, P22) gives
each line of a text root its **most recent author**. Needs read and **history** (K-PERM-9;
otherwise `Forbidden { need: History }` and `AuditEvent::Refused { action: Blame }`). It
only reads: nothing in the doc, the store or presence changes.

- **Grain: the character.** The engine reports, for every live character, the client ID and
  clock that inserted it (Yrs `Text::diff_range(now, empty)`, D0's Q5, with GC on). That
  call splits blocks, so it runs on a **throwaway replica** (a full copy of the doc): O(doc),
  once per load, when the session's provenance map is built (then kept exact by every
  change); the live doc's state vector and encoded state are byte-identical after (tested).
  The map adds each character's **provenance root** (U23: itself, or for text an undo, redo
  or revert put back, the element it restores; from the store's `provenance` rows). The
  store maps a **client to its pid** (the client table: `join`, the import, outside edits)
  and a **clock to its change** (`change_clocks`: the clocks each change's inserted text
  occupies, written in the change's own transaction).
- **The rule.** A line's characters are its text **and its `\n`**: a line split by a typed
  `\n` goes to its typist, as in `git blame`. Each character carries its **root's** change
  (U18a, v1.8): the change that first inserted the element it is or restores; the line
  goes to the character with the **greatest root change id** (change ids increase in the
  daemon's apply order, so this is the true most recent, immune to host clock skew: a later
  change stamped with an earlier host time still wins; `at_ms` is shown, never ranked).
  Characters whose change isn't recorded (inserted before v0.3.0) rank below every recorded
  one. **Ties** (characters of the same root change) go to a restored character when there
  is one (the most recently restored, then the first), else to the first: a line only partly
  put back by an undo, redo or revert still shows `restored_by` (v0.9.1).
- **Undo, redo, revert** insert new elements (by the caller, in a new change), and the line
  blames to the **root**: the author, change, kind and time of the change that first inserted
  the text, with **`restored_by`** (the caller) and **`restored_in`** (the undo, redo or
  revert change) naming the restoration (undo spec U18a, v1.8). `restore` is a new edit: its
  text is the restorer's, with no `restored_by`. Blame by root works the same after a reload
  or a restart (the roots are in the store).
- **`Blame { root, version, lines: Vec<BlameLine> }`**, `BlameLine { line (1-based), text
  (without its \n), author, change, kind, via, at_ms, on_behalf_of, approval, erased,
  restored_by, restored_in }`.
  `author` is a pid, `kit:import` (the file's first content), `kit:external` (an outside
  edit), an erased participant's pseudonym, or **`kit:unknown`** (`UNKNOWN_PID`) when the
  store has no owner for the client: never a guess. `change`, `kind`, `via`, `at_ms` are
  `None` for text from before v0.3.0. Lines are counted as `git blame` counts them: a text
  ending in `\n` has no empty last line; an empty text has none.
- **`-L a,b`** (`lines: Some(LineRange { root, first, last })`): 1-based and inclusive;
  `first` must be a line of the text (`BadPosition` otherwise, also for line 0, a reversed
  range, or another root's range); `last` past the end is clamped.
- **Erasure** (amendment 7's metadata rules). `erased: true` when a purge reached the change
  behind the line (its revert record is redacted): its recorded content is gone; its id, time
  and author stay, and after participant erasure the author is the **pseudonym** (the client
  table and change summaries are renamed). The line's **text is the live text**, which a purge
  never changes: to take it out, edit it out or purge with `and_remove` (then the line is
  gone). Not known once the change's record has left the revert window.
- **Rendering.** `Blame::render(label)` prints the `git blame`-like listing a host shows (the
  change id, the author's label from `label(pid)` or the pid, `for <person>` and `(pending)` /
  `(declined)` for on-behalf changes, `(restored by <label> in <change>)` for text an undo,
  redo or revert put back, the UTC date, `[erased]`, the line number, the text):
  `7 (Alice              2026-10-27 09:20:00 2) Intro, revised.`. `Blame::porcelain()` is
  `git blame --line-porcelain`-shaped: `<change> <line> <line> 1`, then `author`,
  `author-time` (s), `author-time-ms`, `kind`, `via`, `on-behalf-of`, `approval`, `erased`,
  `restored-by <pid>`, `restored-in <change>`, then `\t<text>`. **Labels are the host's**: the
  kit only knows pids.
- **Cost.** The provenance map's range (one full copy and walk of the doc to build it, once
  per load), then one store read for the runs of text in the range (runs are per client,
  consecutive clocks and root, not per character). Budgeted on native
  (E-PERF-18: 50 ms for 1 MiB); a host computes it on demand (the gutter, the verb).

**Tier 0** (whole-blob grain): `BlobSession::blame(conn) -> BlobBlame { version, current:
BlobVersion, versions, authors: Vec<BlobAuthor { pid, versions, last_ms }> }`: who set the
current version, when, through what and why, and the pointer's history summarised (how many
versions; everyone who set one, most recent first). Needs read and history.

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
- `stay`: `Held` if any connection is held;
- `claims: Vec<ClaimInfo>`: their live claims, oldest first (amendment 13);
- `last_edit: Option<LineRange>`: the lines their latest change touched, where that text is
  now, from the most active connection that has one. **Not expanding** (v0.9.1): text others
  type at its edges stays out, and once its text is all deleted or replaced (an undo of it,
  someone's rewrite) it is gone; it never names a line past the doc's end. An agent's caret is at
  the end of its last edit, so agents have carets in `cursor` too.

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

Audit events `Joined` and `Left` carry both `pid` and `conn`. Claims add `ClaimTaken` and
`ClaimEnded` ([Agent presence](#agent-presence-amendment-13)).

### The roster push

`subscribe_roster(conn)` (on the session, or on the manager, which finds the doc by
connection) needs read. It returns a `Receiver<RosterEvent>`:
- first a `Snapshot { version, members }`: the `roster()` rows, each with its `Who` and rights;
- then deltas: `Joined`, `Left { reason }`, `Rights`, `Role`, `Away`, `Cursor { who,
  cursor, version }` (anchor and head, or none), `ClaimStarted` and `ClaimEnded` (lines and
  the doc's version), and `Note` (a heads-up delivered to `to` about `from`).
- **Carets come with their version** (v0.9.7): a `Cursor`'s byte offsets (and `line:col`) are
  positions in the root's text at its `version`, and every caret in a `Snapshot` is at the
  snapshot's `version`. Both are read together, under the session lock, when the event is
  built at delivery: a caret carried through edits that landed after it was set is delivered
  at its carried place, with the version after those edits. A client whose text is at
  another version maps the caret through the changes between (`events_since`, or the
  `ChangeEvent`s it has); an edit past a caret moves nothing and sends no new event.
- **Coalesced** over `roster_interval_ms`: role, rights, away and the caret are compared with
  what was last told, so a caret moved ten times is one event at its last place. Joins,
  leaves, claim starts and ends and notes are recorded as they happen and **never** coalesced.
- **Delivered when the host drives it**: `SessionManager::flush_rosters()` (or `tick`, or
  `Session::flush_roster()`) at least every interval; any call on the doc also delivers what
  is due. The kit starts no thread; every timer is the host's clock.
- **Bounded**: a receiver holds at most `roster_queue` events. One that falls that far behind
  gets a fresh `Snapshot` once it has room, instead of what it missed (and every subscriber
  does when more than `roster_queue` events happen inside one interval).
- Every subscriber sees the same events in the same order. Dropping the receiver
  unsubscribes; the subscription closes when its connection leaves, or its pid is kicked or
  loses read.
- `Who` is `Who::Pid(pid)`, or `Who::Handle(h)` for an App viewer, and `#[non_exhaustive]`:
  match it with a wildcard arm.
- A participant past `participants_max` (a watcher) has no row and no event of its own; the
  snapshot's `watching` and `RosterEvent::Watching { count }` are all the roster shows of them.

### App viewers: handles, never pids (K5)

A connection whose participant kind is `ParticipantKind::App` never sees a pid, its own
included. Each pid becomes a handle, `p1`, `p2`..., numbered per doc and per viewer in the
order that viewer first sees it: the same for the same pid across the viewer's reconnects
while the doc is loaded, and independent between viewers. It covers the roster and its push,
change events, `read`'s contributors, `blame`, `log` (whose `by` / `on_behalf_of` filters take
the viewer's handles; anything else matches nobody), `claims`, `reservations`, `activity`,
heads-up notes, `write --base` conflict notes, and refusals naming someone (`Claimed.by`).
The kit's markers (`kit:import`, `kit:external`, `kit:unknown`) are not pids and pass. Tier 0
(blob) docs use the same handles: the roster, `LeaseInfo`, reads, versions, the log, blame,
approvals and `Leased.holder`. Approvals on text docs (`approval`, `approvals`,
`set_approval`) name their author, approver and person by handle too.

### Participant overflow (K11)

A doc admits `participants_max` (32) distinct participants with presence. A new pid joining
past it **watches**: it reads and follows changes, read-only whatever its rights; it has no
caret, view or claims and no roster row (`Session::watching` counts them). Its edits, carets
and claims are refused `MPC-E305 watching` (exit 3). A connection of a pid already here is
what that pid is. When a participant leaves, nobody moves up by themselves:
`Session::promote_watcher()` (the host's call) lets the earliest watcher in when a slot is
free, and from its next call its rights are the host's again.

## The hunk channel: live edits at a base (E-LIVE, v0.9.7)

For live tabs and app widgets: the client keeps a replica of a text root (a snapshot, then
`ChangeEvent` hunks), types locally, and sends its edits as hunks made **at a base version**.
The kit (server side) and the JS client (`client/`, K1) apply the same rebase
(`session::rebase`, amendment E-LIVE r2), so both compute the same text.

- **`Session::events_since(conn, version) -> Result<Vec<ChangeEvent>, ErrorReport>`** (K3):
  every event since `version`, in order (one per text root a change touched), read from the
  store, so it works after a restart. A reconnecting client replays them onto its text at
  `version`. Needs read.
- **`Session::edit_at_base(conn, root, base, hunks, &EditOptions) -> Result<EditAtBase,
  ErrorReport>`** (K2): `hunks` are in UTF-8 byte offsets of the root's text **at `base`**;
  they are carried through every change since, in server order, and applied as one change by
  `conn` (attributed, versioned, broadcast; undoable, its authored ranges being the hunks as
  applied, `EditAtBase::hunks`).
  - **Rule 0:** one change in flight per client; `base` is the latest version it has seen,
    its own acknowledged changes included.
  - **The window** (rule 1): at most `Config::live_window_events` (1,000) changes and
    `live_window_bytes` (4 MiB) of events since `base`, all still in the revert window and
    unpurged. Otherwise `MPC-E112 base_expired` (exit 8): reload.
  - **Ties** (rule 3): two inserts at one point: the change applied first stays left; the
    client mirror (`rebase::rebase_incoming`) gives an incoming change the left.
  - **Never a bounce** for typing near someone, at the same point, or deleting what they
    deleted (rule 4 (a): deletion vs deletion is a no-op).
  - **`MPC-E111 write_conflict`** (exit 1, `details.conflicts` with the region, why, and the
    change met) only when ours **replaces** text a change since deleted or replaced, or
    **inserts strictly inside** a range a change since deleted (rule 4 (b)), under **both**
    tie-break placements (S3, r2.3): wherever a tie with others' insertions at its point put
    it, before or after their text, the characters on both its sides are deleted; an insert
    at either edge of a deleted range, under either placement, never is. Overlap is judged stepwise,
    **each hunk as sent** (E-LIVE r2.3, r2.2's rule (1) withdrawn): an insertion that comes to
    touch its own change's deletion stays an insertion, and a replacement split by rule 5
    stays a replacement on every piece, so deleting text someone replaced never bounces and a
    split replacement still competes. Only the result is made canonical
    (`rebase::Carried`).
  - **Fully absorbed** (S4): a change whose every deletion is already done and that inserts
    nothing applies nothing: `EditAtBase::hunks` empty, no new version.
  - **Others' insertions survive** (rule 5): a deletion over them is split around them; a
    replacement's text goes at the left end of its range.
  - **Validation first** (rule 7): hunks out of order or range, or off a character
    boundary, **at `base`**: `MPC-E151` (exit 2). Then rights, the rate (`MPC-E901`), the
    backlog, `EditOptions`' version guard and claim policy, as for every edit.
  - A refusal applies nothing.
- **Shared vectors** for the JS client: `crates/base/tests/vectors/live_rebase.json`
  (`[start, end, insert]` hunks; the server's result or conflict, and the mirror's).
- **Parity:** a person's live edit is rows P9–P11; the agent verbs are `insert --at`,
  `replace --range` and `delete --lines` (same text, same attribution), and for an edit
  made from an older read, `write --base V` (P14), which is merge3 (two inserts at one point
  conflict there, by O5: right for an agent rewriting a doc, wrong for live typing).

## Agent presence (amendment 13)

Agents have no cursor. This gives them, and the people beside them, the same feel as seeing
someone's cursor in a shared doc: who is looking at, has just edited, or is about to edit the
same lines. **Ephemeral awareness, not locking and not history:**
- Nothing here is written to the store, the update log, versions or the engine. It lives in
  the loaded session's memory and is gone when the doc unloads (a test holds the store
  file's bytes unchanged through a session full of reads, claims and notes).
- **Claims coordinate agents; they never block anyone else.** A claim (or a hand-off
  reservation) refuses an edit only when the **host** asks for that on the edit
  (`ClaimPolicy::Respect`), and only an **agent's** (`ParticipantKind::Agent`): a person,
  an app guest or any other kind is never refused or reserved against, whatever the
  policy. The kit never sleeps or waits: a host that holds an agent's edit until a claim
  ends builds that on the events below.
- Visible to whoever can read the doc (the roster's rights).

**Anchored ranges, expanding.** A claim's span (and a view's; not a last edit's, below) is a byte
span carried through every change by `position::transform`: its start with `Bias::Before`
(it sticks to the character before; Yrs `Assoc::Before`), its end with `Bias::After` (the
character after; `Assoc::After`). Text typed at either edge lands inside; deleted text
collapses it to a point; it never inverts (the opposite convention inverts an empty or
near-empty range on the first keystroke at its edge, which a test shows). The kit carries
byte offsets as it does for carets, so the engine stays hidden (no Yrs `StickyIndex`);
the rules are the same.

**Two participants' claims never overlap** (v0.9.2). Claims can come to share an edge: the
text between them is deleted (or one claim's text is, leaving it a point on the other's
edge). Text then typed at that edge joins **one** claim, never both:
- the **typist's own**, if they hold one of the claims meeting there (the first they
  acquired, if several);
- otherwise (a person, a third agent, an outside edit from disk) the claim **acquired
  first** (claim ids are in acquire order), whichever side it is on.

The other claim carries the text outside it (its end stays before the text, its start
moves past it, a point moves to the side away from the claim that took it), and the text
doesn't touch it: no `Edited` end for its owner, no `Respect` refusal for it. One
participant's own claims meeting there (nobody else's) all take it, as before. A
reservation's range keeps plain expanding anchors.

**Last edits, not expanding** (v0.9.1). A participant's last-edit span (the roster's
`last_edit`, `Edited` notes) is about text they wrote, so it is carried the other way: its
start moves past text inserted at or replacing it, its end stays before it. A span that
held text and holds none any more is dropped: what is there now is someone else's (before,
an undo of A's line followed by B typing there gave "A edited" notes about B's line). Its
lines are clamped to the text's lines.

### Claims

`claim(conn, LineRange, ttl_ms: Option<u64>, note: Option<String>, hold: bool) ->
ClaimInfo` needs edit. "I'm working on lines 40–60."
- **TTL:** `None` is `Config::claim_default_ms` (30 s); asks over `claim_max_ms` (10 min) are
  clamped; `Some(0)` is `BadClaim`. The agent picks its own.
- **Overlap at acquire:** another participant's live overlapping claim gives
  `SessionError::Claimed { id, by, name, lines: (first, last), remaining_ms, note }`
  (the claim's lines now), **exit 5** (as `Leased`: someone else holds it for now). Adjacent lines don't overlap. The caller's own
  claims may overlap, except one that others wait for (`ClaimContended`, below).
- **Limits:** `claims_max` (256) per doc, `claims_per_participant` (16): `BadClaim`.
- **Ends**, each one `AuditEvent::ClaimEnded`:
  - `Edited`: by default, when the claimant's (pid's) own change touching it lands (any edit
    path, undo, redo, revert, restore). With `hold`, it survives its owner's edits;
  - `Expired`: on the host's clock, checked in `expire_idle` (so the manager's `tick`) and
    lazily on every claim, roster, activity, read and edit call;
  - `Released`: `release_claim(conn, id: Option<ClaimId>) -> Vec<ClaimId>` (needs read;
    `None`: all the pid's claims here; empty when none was the caller's: the verb exits 1);
  - `Left`: the connection that took it leaves, is kicked, idles out or loses read; and when
    the session closes (unload, delete, release);
  - `Reset`: a store write failed mid-edit and the session reloaded (positions can't be
    trusted then).
  Someone else's edit, a person's included, never ends a claim.
- **"Touching"**: a replacement or deletion that changes a character inside the span, or an
  insertion inside it or at either edge (at an edge shared with another participant's
  claim, only the claim the text joins, above). Deleting only the newline after the last
  line, or text before the first, doesn't touch it.
- `renew_claim(conn, id: Option<ClaimId>, ttl_ms, hold: Option<bool>) -> ClaimInfo` (needs
  edit; `None` id: the caller's only claim here, else `NoClaim(None)` / `BadClaim`): expiry
  becomes `now + ttl`, and `hold` is set or cleared. Someone else's claim: `BadClaim`.
- `claims(conn) -> Vec<ClaimInfo>` (needs read): every live claim, oldest first.
  `claim_remaining_ms(conn, id) -> u64`: `expires_ms - now_ms` on the host's clock,
  deterministic; `NoClaim(Some(id))` (exit 4) once it ended.
- `ClaimInfo { id, pid, name, conn, lines, start, end, taken_ms, expires_ms, remaining_ms,
  note, hold, for_edit, waiters }`: where it is **now**, in lines and bytes. `ClaimId`
  displays as `claim-12`; `ClaimId::parse` reads that or `12`.
- A connection that holds or waits for a live claim **doesn't idle out** (bounded by the
  claim's TTL), so a one-shot CLI connection keeps its claim.

**Waiters and anti-starvation.** `register_waiter(conn, id) -> Waiting { claim, position }`
(needs edit) puts the connection in the claim's FIFO queue (again: a no-op reporting the
place; the caller's own pid's claim, or a full queue of `claim_waiters_max` (64): `BadClaim`).
`cancel_waiter(conn, id) -> bool` (needs read). A waiter that leaves the doc leaves the
queue. **While another participant waits, the owner can't extend the claim** (a later
expiry) **or give it `hold`**, through `renew_claim` or a new overlapping claim of its own:
`SessionError::ClaimContended { id, waiters, remaining_ms }`, exit 2, nothing changes. A
shorter TTL or clearing `hold` is allowed. The claim still ends normally.

**Hand-off reservations.** When a claim with waiters ends (any way but `Reset` or the
session closing), its range (anchored, as the claim's was) is **reserved for the first
waiter still present** for `Config::claim_handoff_ms` (2 s, from `Host::now_ms`; 0 turns
this off), so the owner can't grab it back before the waiter acts.
- During it, an **agent's** new overlapping claim, or an agent's `Respect` edit touching it,
  by anyone but the reserved waiter is refused with `Claimed { id: <the ended claim>, by:
  <the waiter>, remaining_ms: <the reservation's remainder>, note: None }`. Anyone else (a
  person, an app guest) is never affected; `Ignore` edits never are.
- The reserved waiter **uses it** with its first edit touching the range (any policy; it
  then passes on) or its first claim overlapping it (the rest of the queue moves, in order,
  onto that new claim as its waiters).
- It **lapses** at its deadline (checked as claims expire), or ends when the waiter
  `cancel_waiter`s or leaves. Except when used by a claim, it then **passes to the next
  waiter** still present, FIFO, with a fresh deadline.
- Events: `AuditEvent::Reserved { key, claim, pid, conn, lines, until_ms, waiters }` at each
  grant (wake `conn`), `ReservationEnded { key, claim, pid, how: ReservationEnd, lines,
  version, next_waiter }` (`Edited`, `Claimed`, `Lapsed`, `Cancelled`, `Left`, `Reset`;
  `lines` and `version` as in `ClaimEnded`, below).
- `reservations(conn) -> Vec<ReservationInfo { claim, pid, name, conn, lines, start, end,
  until_ms, remaining_ms, waiters }>` (needs read). A reserved or queued connection doesn't
  idle out. A late agent can't join a reservation's queue (`register_waiter` names live
  claims only): it retries after `remaining_ms`.

**Events** (metadata only; the claim's note is not in them):
- `AuditEvent::ClaimTaken { key, claim, pid, conn, lines, expires_ms, hold, for_edit }`;
- `AuditEvent::ClaimEnded { key, claim, pid, how: ClaimEnd, lines: LineRange, version: Seq,
  next_waiter: Option<(ConnId, String)>, waiters: Vec<(ConnId, String)> }`, waiters first
  in line first. A host waiting for a claim wakes its waiters from this, in order, and never
  polls. Like every event it comes with the session locked: queue the wake-up; don't call
  back into the kit from it. The range is then reserved for `next_waiter` (above): wake it
  on `Reserved`.
- **`lines` and `version` as it ends** (amendment 14), on `ClaimEnded` and
  `ReservationEnded`: `lines` is the range's current lines (1-based, inclusive), after the
  edits made while it was live moved its anchors; `version` is the doc's version at that
  moment (after the edit that ended it, for `Edited`). It is the same number reads report
  (`ReadResult::version`, `ToolOutput::version`, the fence's) and `--if-version` compares,
  so a host can tell a bounced agent "lines X–Y are free; the doc is at `version`; re-read",
  and the agent's next edit guarded by the version it last read is refused with `doc
  changed since V (now W)` (W: this `version`, if nothing landed since). For `Reset` (a
  reload after a failed write), both are the reloaded doc's (lines clamped to its text).

### The claim policy on edits

`ClaimPolicy::{Ignore, Respect}` (default `Ignore`), chosen by the host per edit:
`edit_with_policy(conn, root, policy, f)`, `edit_with_options(conn, root,
&EditOptions::new().policy(p), f)` (with `if_version` too), `replace_text_with(.., policy)`,
`write_base_with(.., policy) -> WriteReport`, and `run_tool_with(conn, root, argv, stdin,
&ToolOptions::new().policy(p))`. The old calls (`edit_with`, `replace_text`, `write_base`,
`run_tool`) are these with `Ignore`: behaviour unchanged. `edit`, undo, redo, revert and
restore always ignore claims.
- `Ignore`: claims refuse nothing (notes still say who is there).
- `Respect`: an **agent's** edit that touches another participant's live claim, or a
  reservation held for someone else, is refused with `Claimed` (exit 5) and nothing
  applies. **Only `ParticipantKind::Agent` is ever refused**: a person, an app guest or any
  other kind never is. Only the policy changes the outcome: with `Ignore`, the same edit
  applies.
- A host's auto-wait (an agent waits up to the claim's remaining time, FIFO, then edits) is
  `Respect` + `register_waiter` + `Reserved` (or `ClaimEnded`), in the host.
- The claim at acquire is separate from the policy: a new claim overlapping another
  participant's live claim is refused (`Claimed`) for every kind. A refused claim never
  stops anyone editing.

### `--for-edit` on read verbs

`cat`, `head`, `tail`, `view`, `grep` and `sed` (without `-i`) take `--for-edit [--ttl
MS]`: the read also takes a short implicit claim on the lines it showed
(`Config::for_edit_ms`, 30 s, unless `--ttl`: milliseconds, or `Nms`, `Ns`, `Nm`). `cat`
claims the whole doc, `grep` the span from its first to its last selected line, the others
their lines. The claimant's next edit touching it ends it (`Edited`). `ToolOutput::for_edit:
Option<Box<ForEdit>>` (boxed: rare and large):
- `Claimed(ClaimInfo)`;
- `Conflict(ClaimInfo)`: another participant's live claim overlaps; **the read succeeds**
  unchanged and nothing is taken;
- `NotTaken(SessionError)`: no lines shown (`head -c`, a script that isn't `a,bp`), no edit
  right (`Forbidden`), or a claim refusal (`BadClaim`, `ClaimContended`).
`Display` is one `note: for-edit: …` line for stderr. `--ttl` without `--for-edit`,
`--for-edit` with `sed -i`, and `--for-edit` on `wc` or `diff` are usage errors (exit 2).

### Heads-up notes

Every read and edit through `run_tool` returns `ToolOutput::notes: Notes` about the lines it
touched (a read: the lines shown, `cat` the whole doc, `grep` its selected span; an edit:
the lines it changed). A note is another participant's:
- live **claim** (`Claimed`, with its note and time left);
- a **person's** caret (`Caret`) or selection (`Selection`);
- **recent edit** (`Edited`: their latest change, within `note_edit_window_ms`, 60 s);
- **view** (`Viewing`).
The caller's own presence never makes a note. `Note { pid, name, kind, participant_kind,
lines, age_ms, claim, claim_note, remaining_ms }`, ordered claims, selections, carets,
edits, views, then newest first.
- **Deduped** per (reader pid, other pid, kind, lines) within `note_repeat_ms` (60 s).
- **Rate-limited** per (reader pid, doc): at most one non-empty batch per
  `note_interval_ms` (5 s), of at most `note_batch_max` (5); the rest of a batch is
  `more` (`note: +N more`). Notes the interval holds back are counted in `held`, aren't
  marked as given, and come in a later batch if they still apply. A 20-read loop over a
  busy doc prints a handful of lines, not a wall.
- **Rendering:** `Display` for `Note` is **one line starting `note: `** (control characters
  escaped), e.g. `note: Agent B (agent-b) claimed lines 40–60 "rewriting intro" (12 s ago,
  18 s left)`, `note: Sam (sam) viewing lines 40–62 (8 s ago)`, `note: Ana (ana) has a caret
  at line 45 (3 s ago)`. `Notes` displays one such line per note, then `note: +N more`;
  nothing for an empty batch. **For stderr only: `stdout` never changes** (a test compares
  it byte for byte with and without others present).
- **JSON** (`--presence json`): the kit has no serde. `Note`, `ClaimInfo` and
  `ActivityEntry` list their fields in a stable order for the host to serialise; enums as
  `NoteKind::as_str` (`claimed`, `selection`, `caret`, `edited`, `viewing`),
  `ClaimEnd::as_str`, `ActivityKind::as_str`.

**`write --base` conflicts.** `write_base_with` returns `WriteReport { outcome,
conflict_notes: Vec<(usize, Note)>, notes }`. For `WriteOutcome::Conflicts`, each
conflicting region (carried from the base onto the live text) names who has claimed it and
who last edited it (`Claimed` / `Edited` notes, whatever the edit's age): "lines 40–60
claimed by agent-b, 12 s ago". **Only participants still present are named**: an editor
whose connections have all left (a one-shot CLI call that idled out) is not; the change
itself is in `log` and `blame`. These are the conflict's own
report: not deduped or rate-limited. An applied write carries heads-up `notes`.

### Activity

`activity(conn, n) -> Vec<ActivityEntry>` (needs read, as the roster): the last `n` rows of
a per-doc ring of `activity_max` (256) rows, oldest first. `ActivityEntry { pid, name, kind,
verb, lines, at_ms, claim }`, `kind`: `Read` (a `run_tool` read verb), `Edit` (every change:
`verb` is the tool's form, `sed -i`, or the call's kind, `edit`, `write --base`, `undo`,
`restore`...), `Claim` (taken or renewed: `claim`, `claim --renew`, or the read verb for
`--for-edit`), `ClaimEnded(ClaimEnd)`. Line numbers are as they were then (a log, not
anchors). Memory only: dropped when the doc unloads; a participant erasure drops that pid's
rows (and its note dedupe entries).

## Disk sync

For a session opened with `open_file`, **the file on disk stays true**: the doc is written
back to it, and edits made to it by other programs are merged in. The kit does the file I/O
(std only) but never watches: the host's watcher calls `file_changed()`, and `tick()` runs a
cheap stat backup. The three edit paths above are unchanged; disk sync sits beside them.

**Opening.** The host resolves and jails `path`. The kit checks again: `root` must be a
folder, and `path` must be a regular file (not a folder, device or pipe) resolving, symlinks
included, **inside `root`**. Otherwise `BadPath`.
- A new doc is imported from the file (it must exist; `Declined` as for `open`).
- A stored doc is **reconciled**: if the file isn't what the doc last synced (`synced_hash`:
  HMAC-SHA256 under the doc's own key, never a plain content hash; K-PURGE-13),
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
   what the file holds. A skipped write still brings the **disk replica** (the doc as the
   file holds it, below) up to the doc when the doc changed since the replica was synced: a
   change that moved no byte (an edit and its undo, a delete and a retype, a restore or
   revert to the file's text) puts the same text back on new elements (v0.9.4). Since
   v0.9.5 an outside edit uses only the replica's text, so this keeps the replica exact for
   K-AUD-1, not for correctness. The marker is the doc's change counter (v0.9.5; O(1)): the
   replica was synced at version `synced_seq`, stored with it, and any change moves the
   version past it, a delete-only one included (v0.9.4's state vectors missed those). A
   re-sync is one full encode, stored as the synced state;
3. is **atomic**: a temp file beside it (`.<name>.~mp<pid>-<n>.tmp`; `is_write_temp(name)`
   tells a watcher to ignore it), the old file's permissions, fsync, rename over, fsync the
   folder. Symlinks are resolved, so a link stays a link and its target is written. Temp files
   a crash left are removed on open;
4. records the new synced hash (keyed), position and state. The record is written **before**
   the rename and promoted after it, so a crash in between is recognised on the next open
   (the file matches the write in flight) instead of looking like an outside edit;
5. remembers the last 8 written hashes for **echo suppression**: keyed, in memory only, each
   for `own_write_ttl_ms` (60 s), and forgotten by a purge and by every outside save taken
   in (v0.9.5).

If the file was deleted, replaced by a folder, or re-pointed outside the root, nothing is
written: the state is `Missing`, the doc and its edits are kept, `AuditEvent::FileMissing
{ unflushed }` is emitted, and the host decides the grace period (K-DISK-7, -17). When the
file comes back, it is checked like any outside edit.

**Outside edits (file → doc).** On `file_changed()`, or when `check_disk()` / `tick()` finds
the size, mtime or inode changed (a rename-save changes the inode):
- the file holds the last synced content → nothing;
- it holds an older write of the kit's from the last 60 s, made **since the last outside
  save** (an echo, or an editor saving a stale buffer) → not an edit (`OwnWrite`), reported
  once as `AuditEvent::EchoSuppressed`; the next write-back puts the doc back. An older one,
  one from before a purge, or one from before an outside save that was taken in, is an
  outside edit (v0.9.5: before, a person who saved, then saved again the bytes of an earlier
  kit write, an undo for instance, had that save discarded and overwritten);
- otherwise: a version "before external edit" is cut (when the doc moved since the last
  version), the save is merged into the live doc as the reserved participant `kit:external`
  (`EXTERNAL_PID`, its own client ID per doc, `via: "disk"`, no `on_behalf_of`), and a
  version "external edit" is cut, all in one store transaction. Subscribers get a
  `ChangeEvent` from `kit:external`; the host gets `AuditEvent::ExternalEdit { seq }`.
- **The merge (v0.9.5)** is three-way, onto the live doc: base = the **last synced text**
  (what the file held), ours = the file's new text, theirs = the **live text**, dirty or
  not (`text::merge::merge3_keep_both`). Both sides are diffed from the base with pure line
  insertions slid to line starts and pure line deletions to whole lines (`outside_sides`).
  Its hunks are on the live text and are
  applied to the live doc as they are, so edits typed since the last write-back are kept,
  and the next write-back writes the merged result.
  - **Clean** (no overlap): `write --base`'s merge and pair rules (O1–O5): the save lands
    exactly as the plain editor made it.
  - **Overlap** with a change not yet written back: **both are kept**, as the CRDT merge
    of v0.1.0–v0.9.4 kept them, at **line granularity**: each hunk that meets an overlap is
    widened to whole lines (`outside_merge_sides`), then every line either side deleted is
    gone and every line either side wrote lands whole. At one point, pure insertions go
    before replacements, and the live side first within each. A change made identically
    on both sides is taken once. It is never refused and makes no conflict copy (those are
    tier 0's). Two sides' lines are never spliced into one (a text diff of a multi-line
    change, merged at the character, could: `p12:63ed:95`).
  - Before v0.9.5 the save was made on the **disk replica** (the doc as the file held it)
    and the CRDT update merged in. A change that moved no byte puts the text back on new
    elements, so a replica synced before it held the old ones. The save's deletes then
    missed and its inserts landed beside the copy (`LINE three3`). v0.9.4 re-synced the
    replica whenever the doc rendered the synced bytes. That left the write delay open: a
    change that moved no byte, then an edit not yet written back. Now only the replica's
    **text** is used, as the base.
  - The replica then takes the file's text. When the doc holds the file's text, the
    replica becomes the doc (same elements). Otherwise the save is made on the replica as a
    client of its own, one the doc never holds. The doc can't render the synced bytes
    again without a change or a write-back, and either replaces the replica whole. Nothing
    is ever merged from the replica into the doc.
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
| `Session::flush(conn)` | needs read; `FlushOutcome { written, hash (HMAC-SHA256 hex of the doc's bytes under the doc's key: compare only with this doc's hashes; a purge changes it), disk }` |
| `Session::file_path()` | the path as given |
| `Session::audit_disk_sync()` | K-AUD-1's sync check (v0.9.4): when the doc is `Clean`, the disk replica must hold the doc's text on the doc's elements; returns `SyncDrift::Text { root }` / `Elements { root }` for each root that doesn't (metadata only), empty otherwise. In any disk state (v0.9.5), `SyncDrift::Pending { replica }` when the doc or the replica holds an update it couldn't integrate (missing dependencies) or is poisoned. Read-only, O(doc) (a blame of each root on both): for a nightly audit or a test, not the edit path |

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
| (read verbs) | read | `cat`, `head`, `tail`, `view`, `grep`, `sed` without `-i`: `--for-edit [--ttl MS]` ([amendment 13](#--for-edit-on-read-verbs)) |
| `wc` | read | `-l -w -c -m` (prints the doc name, as `wc FILE`) |
| `grep` | read | `-E -F -G -e -i -y -v -w -x -c -o -q -s -l -L -H -h -n -m N -A/-B/-C N -NUM --color=never` |
| `sed` | read | as `sed -i` below, printing the result |
| `diff` | read | `-u -U N --unified[=N] --label L` (twice); the old text on stdin; exit 1 when different |
| `view` | read | `--lines a,b` (`$` allowed) |
| `sed -i` | edit | `s///` (`g N i I p`, `& \0-\9 \n \t`), `d p a i c y = q`, `#` comments; addresses `N $ /re/ \cREc` (`I`), `a,b`, `a,+N`, `!`; `-n -e -E -r -s --sandbox`; GNU `-i` and BSD `-i ''` |
| `patch` / `apply` | edit | one file's unified diff on stdin; `-p N -R --dry-run (--check) -F N --partial -s` |
| `append` | edit | stdin's text; `--ensure-newline` |
| `insert` | edit | `--line N`, `--after N` (whole lines, like `sed 'Ni\'`/`'Na\'`), `--at L:C` (as is) |
| `replace` | edit | `--lines a,b` (like `sed 'a,bc\'`), `--range L:C-L:C` (half-open, as is); or `--old TEXT --new TEXT [--all]` (exact text, below) |
| `delete` | edit | `--lines a,b` (like `sed 'a,bd'`), `--range L:C-L:C` |
| (edit verbs) | edit | every edit verb above, and every pack verb: `--if-version V` ([below](#--if-version-on-edit-verbs-amendment-14)) |

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
- **Exact replace** (`replace --old TEXT --new TEXT [--all]`, P12 find and replace): the
  agent's "replace this text with that", with no regex or escaping.
  - **Exact.** `--old` is matched byte for byte (case, whitespace, line breaks) on the doc
    text the other edit verbs see: `\n` line breaks, no BOM. A CRLF file's `\r\n` and its BOM
    are restored when it is written, so `--old` with `\r\n` doesn't match (the report says
    so, below).
  - **Unique unless `--all`.** Matches are counted left to right without overlaps (what
    `--all` replaces, like `str::replace`). Exactly one applies; when the only one overlaps
    another occurrence of `--old` (`aa` in `aaa`), that counts as more than one. With
    `--all`, every match is replaced.
  - **Refused, nothing applied, exit 1**, with a report on `stdout`:
    - **More than one match:** the count, and each match's line or lines (`1, 3–4, 7 (×2)`,
      in the text before the edit). The first 20 are listed, then "and N more". The report
      says to add surrounding text to `--old`, or to pass `--all`.
    - **No match:** says so, with hints that never apply anything: `--old` has `\r` (and
      whether it matches with `\n`), it starts with a BOM, or it would match with
      whitespace loosened (runs as one space, ends trimmed), case ignored, or both (count
      and lines). With no hint, it says how to copy the text exactly.
    - The report never quotes the doc's text (it is not fenced): use `view --lines a,b`.
  - **`--old` must be non-empty; `--new` may be empty** (deletes the match). Both are
    required. Each is given once. `--old`/`--new` exclude `--lines`/`--range`. Any of
    these is exit 2 (`ToolError::Usage`), as is stdin that isn't UTF-8 when read.
  - **`-` reads one of them from stdin** (not both; `--old -` with stdin `-` matches a
    literal `-`). Without `-`, stdin is not read. Values may start with `-`
    (`--old --lines`, `--old=-x`), since `--old`/`--new` always take the next argument.
  - **Success:** exit 0, one change, attributed and undone like any edit verb. `stdout` is
    empty, except `--all`, which prints `replaced N matches in notes.md (at lines …
    before the edit)`. `--old` equal to `--new` succeeds as a no-op: no new version,
    `no change to …`.
  - The match is found in the **live** text inside `edit_with`, so an edit by someone else
    since the agent's read moves the match, and the replace applies where it now is, unless
    the call says `--if-version V` (below).
  - **`--old-file` / `--new-file` are host flags.** The kit does no I/O: the host reads
    the file and passes its text as `--old` / `--new`.
- **Regexes** are GNU BRE/ERE translated to a linear-time engine, matched leftmost-longest
  as POSIX requires. Refused: backreferences; escapes GNU doesn't define (`\d`).

### `--if-version` on edit verbs (amendment 14)

`--if-version V`: apply only if the doc is still at version V, the version a read printed
(`ReadResult::version`, `ToolOutput::version`, the fence's, `notes.md v1842` on stderr).
For an agent that reads, thinks, then edits: anything that landed in between refuses the
edit instead of being edited over.
- **Every edit verb** through `run_tool`: `sed -i`, `patch`/`apply` (`--dry-run` too),
  `append`, `insert`, `replace` (every mode), `delete`, and every pack verb. `V` is decimal,
  or `vN` as printed. Kit verbs parse it as any long option (`--if-version=V`, a unique
  prefix such as `--if-v`, anywhere before `--`). For a **pack verb**, the kit takes
  `--if-version V` / `--if-version=V` (that exact spelling, before `--`) out of `argv` before
  the pack parses the rest, so packs need no change; a pack verb can't take a value that is
  literally `--if-version`.
- **Checked under the session lock**, in the edit's critical section, with the same check
  and version as `edit`'s `if_version`: nothing can land between the check and the apply.
- **Stale:** `SessionError::Stale { expected: V, actual: W }` (as
  `ToolRunError::Session`), **exit 1**, nothing applied, no new version. It displays as one
  line, `doc changed since V (now W)`, with no doc text. Re-read and retry.
- **Order:** after admission (rights, a declined doc, the edit rate: as `edit`), and **before
  every other refusal**: the approvals backlog (`Held`), the verb's own "no" (no match, not
  unique, a failing patch), a tool error found on the live text, and the claim policy
  (`Claimed`). An `--old` equal to `--new` (a no-op) still checks it. Usage errors in `argv`
  come first, as they are found before the doc is touched.
- **Read verbs refuse it** (`ToolError::Usage`, exit 2: `--if-version is for edit verbs; cat
  is a read (it prints the version)`), whatever the version, so agents learn where it
  applies. `sed` without `-i` is a read. Twice, a missing value or a bad one: exit 2.

`ToolOutput { stdout, stderr_summary, exit, version, changed: Option<EditResult>, fence:
Option<Fence>, notes: Notes, for_edit: Option<Box<ForEdit>> }` (`notes` and `for_edit`:
[amendment 13](#heads-up-notes); print them on stderr only):
- `stdout` is byte-exact (for edit verbs, `patch`'s report, or exact `replace`'s refusal
  report or `--all` count; else empty);
- `stderr_summary`: `notes.md v1842` after a read; `edited notes.md lines 12–18 (+3 −1) ·
  v1843` after an edit (the host adds `as <name>`); `no change to …` / `nothing applied to …`;
- `exit`: 0, or 1 for the tool's "no" (grep found nothing, diff found differences, a patch
  hunk failed, exact `replace` found no match or more than one). A "no" is not an error.

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

**A pack's own verbs** (amendment 6). `Pack::verbs()` returns `&'static [tools::PackVerb]`:
edit verbs only the pack has (Markdown's `replace-section`). `PackVerb::edit(name, usage,
serves, exits, parse)`: `parse(args)` checks `argv[1..]` before the doc is touched (a
`ToolError`, exit 2) and returns a `PackEdit`, `(doc name, live text, stdin) -> EditOutput`
(`EditOutput::new(text, stdout, exit)`; `text: None` applies nothing, exit 1 for the
tool's "no"). `run_tool` looks a verb up there **only when no kit tool has that name**
(`tools::is_kit_verb`), and runs it exactly like a kit edit verb: needs edit, inside
`edit_with`, one change, caret at its last hunk, one `undo` reverts it, the same
`stderr_summary`. The host routes `<host> doc <verb>` to `run_tool` for these as for any tool.

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

## Deleting docs

`SessionManager::delete_doc(&key)` and `delete_space(space)` delete docs (text and tier 0)
for good:

1. Every **loaded** session of those docs closes first, under the pool lock (no open can
   race it), with no write-back and no compaction. Its participants are told with
   `AuditEvent::DocDeleted { key, connections: Vec<(ConnId, pid)> }`; their subscriptions end
   (the receivers disconnect), a lease ends with it, and every later call on a handle the host
   still holds fails with `SessionError::Deleted` (exit 4).
2. Then **one store transaction** per call erases every kit row of those docs
   (`DocStore::delete_doc(id, now_ms)` / `delete_space(space, now_ms)`, also callable
   directly): `paths`, `docs`, `snapshots`, `updates`, `clients`, `versions`, `approvals`,
   `sync` (the disk replica), `doc_keys`, `changes`, `reverts`, `revert_index`, `revert_carry`,
   `revert_stats`, `blob_versions`, `blob_sync`, `blob_approvals`. A blob doc's versions **release** their blob
   references (the blob's grace starts then), so `BlobStore::gc` removes a blob nothing else
   references once `gc_grace_ms` has passed; a blob another doc still uses stays.

Left alone: the file on disk (the host's: the kit neither writes it back nor removes it), the
host's table (`host`: the kit never erases it), the store's `meta` (doc ids are never reused)
and the space's blob key (`BlobStore::purge_space_key` destroys it and every blob of the space
at once). `delete_doc` on a key with no doc is `NotFound`; `delete_space` returns the keys it
deleted (empty when none). On a store error nothing is erased (a closed session reopens on
the next `open`). A key can hold a new doc right after.

## Erasure

Agents paste secrets and people paste personal data. **Purge** removes content from
everything the kit holds for a doc; **participant erasure** removes a person from its
attribution. Both are owner only (`Rights::owner`; others get `Forbidden { need: Owner }`
and `AuditEvent::Refused { action: Purge }`) and audited as metadata only.

### Purge removes history, not live text

What the doc says **now** stays; every other copy goes. To take text out of the doc as well,
**edit it out first, then purge**: the deletion is an ordinary attributed change, and the
purge then removes the text from that change's record too. `PurgeRequest::and_remove` does
both for a text scope: it deletes every live copy of the text as the owner's change (which
needs edit too), then purges, under the session's lock (no other call interleaves). It is two
store transactions: a crash between them leaves the deletion done and the purge not.
**Retrying the same request completes it**: with no live copy left, `and_remove` deletes
nothing (`removed: None`) and the purge runs; a purge repeated after a complete one changes
no content (it is idempotent).
`PurgeOutcome::live_matches` counts the spans of live text the purged content still matches
(whole or by an 8-byte window): non-zero means it is still in the doc.

### `Session::purge(conn, PurgeRequest) -> PurgeOutcome`

`PurgeRequest::new(scope)` (`.and_remove()`), with `PurgeScope`:

| Scope | Taken whole | Searched for elsewhere |
|---|---|---|
| `History` | every revert record and every version's content: the whole history before now | (nothing needed) |
| `Changes { from, to }` | those changes' revert records | the text they inserted and deleted (pieces of 8+ bytes) |
| `Versions { from, to }` | those versions' content (entries and names stay) | the text of the changes between them |
| `Text(String)` | (none) | the text (`purge --text`, read from stdin), at least 8 bytes |
| `Participant { pid, pseudonym }` | the revert records of every change `pid` authored | their text; plus the renaming below |

"Searched for" means: wherever any **8-byte window** of the text appears in a version's text or another change's revert record, that span or hunk is
redacted too, so a copy made by a later change, a version cut while it was live, or the
record of the change that deleted it are reached. `Debug` of a request never prints the
text. Bad scopes: `NoSuchChange(n)` (4), `BadEdit` (exit 2: `and_remove` without a text
scope; a `Text` shorter than 8 bytes, which would match innocent text across the whole
history: "purge text of at least 8 bytes, or purge by change/version/participant"),
`BadParticipant` (an empty or `kit:` pseudonym, erasing yourself).

In order, under the session's lock:
1. **The file is written back** (after merging any outside edit), so it holds the live text
   and the sync replica is the live doc.
2. **Every undo and redo stack of the doc is cleared** (spec U19): each holder's next undo or
   redo says `UndoOutcome::ClearedByPurge`. The read-base cache is dropped (a pending
   `write --base` gets `BaseExpired`).
3. **One store transaction** (`DocStore::purge_doc`):
   - the **log**: every entry up to now becomes `redacted {byte length}` (author, client,
     time, surface and `on_behalf_of` stay); a **snapshot** re-encoded from the live doc
     replaces what they held. The engine runs with GC on (K-PURGE-11), so deleted text is
     not in that state: items keep their ids and lengths, not their content (K-PURGE-5: a
     replica synced before the purge still merges);
   - **versions**: the targeted ones lose all content, the others the matched spans; the
     entry, its name and its authors stay, with `VersionInfo::redacted` (root, start,
     length, in the coordinates of the text before any purge). `show`, `diff` and `restore`
     see the text without the spans (a restore restores everything else); such a version is
     never a `write --base` (`BaseExpired`);
   - **revert records**: targeted and matched hunks are blanked and marked redacted; a
     `revert` over one is a conflict of kind `ConflictKind::Erased` (with empty `base` and
     `ours`); change summaries (`log`) keep every change's id, author, time and hunk lengths;
   - the **sync table**: the replica and its hash; any write-back in flight is dropped;
   - the doc's **hash key** is replaced, so every earlier keyed hash of it (synced, own
     writes) means nothing: an echo of a write made before the purge is an outside edit.
4. The in-memory doc and replica are reloaded from what was stored; own-write hashes are
   forgotten.
5. **The store file is rewritten** (below), so no freed page keeps a purged byte.
6. `AuditEvent::Purged { key, by, scope: PurgedScope, counts: PurgeCounts }`: the scope
   without its text (`Text { bytes }`, `Participant { pseudonym }`), counts only.

`PurgeOutcome { counts, live_matches, removed, not_reached, rewrite }`. `PurgeCounts`:
`log_entries`, `versions`, `revert_hunks`, `bytes`, `attributions`, `undo_batches`,
`read_bases` (and the tier-0 counts below: `blob_versions`, `blobs_removed`,
`conflict_copies`, `approvals`). `not_reached` lists `NotReached::ALWAYS` and what
applied to this purge (below); the host prints it with `NotReached::describe` and tells the
owner to **rotate the secret** (K-PURGE-10).

A store upgraded from format 2 keeps a doc's plain SHA-256 sync hash until that doc is next
opened with its file (the hash is re-keyed when the file is read) or purged.

If the file can't be written back (missing, declined, or this session has no file), it still
holds the text as last written, and so does the sync replica the kit keeps of it (re-encoded,
its hash re-keyed or blanked): `NotReached::File`. Attach and flush the file, then purge
again. Targeted changes whose records already left the revert window can't be identified:
`NotReached::ChangesOutsideWindow(n)` (purge by text, or `History`).

### Participant erasure (K-PURGE-7, U20)

`PurgeScope::Participant { pid, pseudonym }` replaces `pid` by `pseudonym` in the log, change
summaries, revert records, approvals (author, person, settler), versions' authors and the
client table, keeping every count, and purges the content of their changes as above. The
erased person's connections are removed (`Left { Kicked }`) and their undo history dropped
(every other stack is cleared anyway, U19). The pseudonym is the host's choice; the audit
event carries only it.

### No bytes behind: the store file rewrite

`DocStore::rewrite_file() -> RewriteReport { bytes_before, bytes_after, rows }` copies every
row of every kit table into a fresh redb file beside the store (`<file>.rewrite`), commits it
(durable), renames it over the store file (atomic: the path names the old store or the new
one, never a partial one), syncs the folder, and closes the old file. The new file is written
only from rows that exist at that moment, so it holds no byte of anything deleted or redacted
before. A crash before the rename leaves the old store whole and the temp file, which `open`
removes. Every other store call waits while it runs (a full copy): it is for purges, not
routine use; a host may call it too. A table the kit doesn't know stops it (error) rather
than being dropped.

**Why not redb's compaction:** `Database::compact` relocates live pages towards the start of
the file and truncates the tail; it doesn't zero freed pages, and redb never updates a page
in place (copy-on-write), so a deleted row's bytes stay in freed pages until reused. The tests
show a deleted row is still in the file until the rewrite.

**What the rewrite doesn't reach:** the old file's disk blocks are released to the file
system, not overwritten (raw-device forensics, until reused); APFS snapshots and backups
taken before; the kit's process memory (freed, not scrubbed).

### The byte scan

`DocStore::scan_for(needles: &[&[u8]]) -> Vec<Finding { needle, offset, len }>` scans the raw
store file (with the store's write guard, so no commit lands mid-scan); `store::scan_file`
and `store::scan_bytes` scan any file or bytes. A needle matches **whole or by any 8-byte
window** (`MIN_MATCH`), so a secret typed in two batches, or split across two records or
pages, is still found; matches of one needle are merged. A scan reports, it never judges:
gate tests plant random 32-byte secrets and expect zero findings; a production check shows
findings outside live text to a person (a short needle can match innocent text).

### Tier 0 (K-T0-9)

`BlobSession::purge(conn, Some(version))` purges one version; `None` the whole history
(every version but the current). Owner only. The **current version is refused**
(`StoreError::CurrentVersion`, exit 2: write or restore another first). Each purged version
keeps its entry (who, when, why, `size`) with `BlobVersion::redacted` set and the zero id:
its keyed id is a digest of the content, so it goes too. Reading or restoring it is
`StoreError::PurgedVersion` (exit 4). The file is written back first. A released blob that
nothing else references is removed **at once** (index and file, no GC grace); one still
referenced (the same bytes in another version or doc of the space, or a pack's `retain`)
stays: `NotReached::SharedBlobs(n)`. The doc's recorded **conflict copies**
(`BlobVersion::conflict_copy`, beside its file or in the host's `conflict_dir`) are compared by keyed id: a copy whose bytes
are a purged blob is deleted (`counts.conflict_copies`); others (edited since, or the current
content) are left (`NotReached::ConflictCopies(n)`). `BlobSession::conflict_copies(conn)`
lists the recorded copies that exist (needs history), for a byte scan. The store file is
rewritten after. Blob ids stay keyed per space; `BlobStore::purge_space_key` still destroys a
whole space. A purged on-behalf version's **approval record** is reached and kept, as its
entry is: it holds only who (the agent), for whom, the state, who settled it and when, never
content or a blob id, so nothing in it quotes the content (`counts.approvals`; the byte scan
test checks the content and the blob id, raw and hex, are gone from the store file, the blob
folder and the workspace).

A text doc **promoted from tier 0** (v0.9.2, [Tier 0 → text](#tier-0--text-v092)) keeps its
purged blob versions purged and imports nothing of them. Its kept tier-0 versions are purged
with `Session::purge_tier0` (owner) or by a `PurgeScope::History` purge; other scopes report
`NotReached::Tier0History(n)`.

### What purge does not reach (K-PURGE-12)

| Copy | Reached? | How |
|---|---|---|
| Update log, snapshots, versions, revert records, read-base cache, undo/redo stacks, sync replica and its hash, in-memory doc and replica, own-write hashes | **Yes** | above; the byte scan checks the store file |
| The doc's file | **Yes**, when it can be written back | otherwise `NotReached::File` |
| Tier-0 versions, blobs, kit conflict copies | **Yes** | `BlobSession::purge`; shared blobs and changed copies are reported |
| Old store file pages | **Yes** | the rewrite; its released disk blocks are not (`FreedDiskBlocks`) |
| Echo-suppression hashes | **Not kept** | in memory, keyed, expire after 60 s, forgotten by a purge |
| Connected clients (`purged` frame), offline clients | **Not yet** / when they reconnect | the wire isn't built; `OfflineClients` |
| The host's own rows (`host_*`) | **No** | opaque to the kit: `HostRows` |
| Git history; agent transcripts and terminal scrollback; Spotlight; APFS snapshots and Time Machine; synced-folder history; K2 backups made before the purge; other apps and sent notifications | **No** | outside the kit: `NotReached::ALWAYS`; **rotate the secret** |

## Limits (`Config`)

| Field | Default | Effect |
|---|---|---|
| `editor_window_ms` | 30 s | editor role window |
| `idle_leave_ms` | 60 s | connections without a hold close |
| `away_after_ms` | 60 s | not visible → away |
| `read_bases_max` / `read_base_ttl_ms` / `read_bases_max_bytes` | 64 / 30 min / 16 MiB | `write_base` bases |
| `undo_depth` / `undo_max_bytes` | 100 / 4 MiB | undo history, per pid per doc (spec U6) |
| `undo_group_idle_ms` | 1 s | a person's undo group closes after this idle (spec U3 (a); 0: never merge) |
| `idle_version_ms` | 2 min | idle version after the last change |
| `agent_version_lines` / `agent_version_percent` | 20 / 10 | version before a large agent edit |
| `version_keep_all_ms` / `version_hourly_until_ms` | 30 / 90 days | retention bands |
| `version_max_count` / `version_max_bytes` | 500 / 64 MiB | version cap per doc |
| `revert_keep_ms` / `revert_keep_changes` / `revert_max_bytes` | 30 days / 10,000 / 64 MiB | the revert window |
| `pending_max_records` / `pending_max_bytes` | 1,000 / 16 MiB | the pending on-behalf cap (`approvals_backlog`) |
| `history_tick_ms` | 1 min | how often `tick` runs retention and the window per loaded doc |
| `edit_rate_per_s` / `edit_burst` | 10 / 10 | Edit calls per **pid** per doc, across their connections. A token bucket on the host clock; the bucket survives leave and rejoin. Over the limit → `RateLimited { retry_after_ms }`. A call takes a token once rights pass, even if it then fails. |
| `subscriber_queue` | 1024 | events per subscriber |
| `roster_interval_ms` | 250 ms | the roster push's coalescing interval (at most one delivery per doc per interval) |
| `roster_queue` | 256 | roster events per subscriber, and recorded between two deliveries; past it, a fresh `Snapshot` |
| `participants_max` | 32 | distinct participants with presence per doc; a new pid past it watches (read-only, counted, `MPC-E305` on edit) until the host promotes it |
| `compact_entries` / `compact_bytes` | 1,000 / 2 MiB | compact after an append at either threshold |
| `unload_compact_entries` | 100 | compact on unload |
| `memory_budget_bytes` / `memory_factor` / `idle_unload_ms` | 256 MiB / 20 / 10 min | the pool |
| `write_quiet_ms` / `write_max_delay_ms` | 1 s / 5 s | write-back after this much quiet, or this long after the first unwritten change |
| `disk_poll_ms` | 3 s | stat backup interval while anyone is present |
| `disk_read_max_bytes` | 64 MiB | larger files are declined unread (`too_large`) |
| `own_write_ttl_ms` | 60 s | how long an own write's (keyed) hash suppresses its echo |
| `claim_default_ms` / `claim_max_ms` | 30 s / 10 min | a claim's TTL when none is asked; the longest (amendment 13) |
| `for_edit_ms` | 30 s | `--for-edit`'s claim without `--ttl` |
| `claims_max` / `claims_per_participant` / `claim_waiters_max` | 256 / 16 / 64 | live claims per doc, per pid per doc; waiters per claim |
| `claim_handoff_ms` | 2 s | a hand-off reservation for the next waiter when a claim with waiters ends (0: off) |
| `note_edit_window_ms` | 60 s | an edit this recent is a heads-up note |
| `note_repeat_ms` / `note_interval_ms` / `note_batch_max` | 60 s / 5 s / 5 | note dedupe window; one batch per reader per doc per interval; notes per batch |
| `activity_max` | 256 | activity rows per doc (memory only) |
| `edit_check_every` | 1,000 | the edit path's runtime check (v0.9.3): one edit in this many is also made through a throwaway replica and the updates compared byte for byte, and the engine's cached texts against a fresh read. A mismatch is never served: `AuditEvent::EditPathMismatch` / `TextCacheMismatch` (metadata only), and the doc takes the replica path (or reads its texts afresh) until reloaded. The same rate samples outside saves: one in this many also computes its change at element identity in full from the engine and compares it with the merge's hunks (`AuditEvent::IdentityMismatch { key, root, change }`; the merge's hunks are kept for the record and blame's provenance map is dropped). 0: never |
| `edit_check_scale_units` | 50,000 | the edit path's check on a large doc (v0.9.7): past this much history (UTF-16 units, the sum of the doc's state vector's clocks: every unit ever inserted, deleted or not), the interval between sampled edits grows in proportion to it, `edit_check_every x history / this` edits and never fewer than `edit_check_every`. A sampled edit is O(history), so this keeps the check's amortised cost per edit about what it is at this size (the default is about the history at version 10,000 in the 32-participant soak); a doc below it is checked one edit in `edit_check_every`. The first sample after a load is placed from the history alone (`edit_check_every` edits in below this; past it at a phase spread over the interval), so the long-run rate does not depend on how often the doc is reloaded. Outside saves keep the fixed rate. 0: a fixed rate |
| `writeback_check_every` | 100 | the file write-back's check (v0.9.7): the disk replica is kept in step with the changes since the last sync instead of reloaded from a full encode; every sync compares its state vector with the doc's (O(clients)), and one in this many also compares its deletions and texts (not the encodings: equal content may be split into blocks differently). A mismatch reloads it: `AuditEvent::DiskReplicaMismatch` (metadata only). 0: never (the state vectors are still compared) |
| `position_check_every` | 1,000 | the engine's position index check (v0.9.7, k2-yrs `k2.3`): one marker lookup in this many also walks the text from its start and compares. A mismatch is never served (the walk's position is used, the text's markers dropped): `AuditEvent::PositionIndexMismatch` (the root and indices only). 0: never |

## Errors (`SessionError`)

Every error below, and every refusal outcome, has a stable id (`MPC-Ennn`; amendment 18):
`docs/ERRORS.md` lists them with the rule each enforces and what to do next.

| Error | Meaning | Exit (`session_exit`) |
|---|---|---|
| `Forbidden { need }` | No right (`Read`, `Edit`, `History`, `Approver`: not the person the change was for; `AuthorOrOwner`: a revert by someone not its author, its person or an owner; `Owner`: breaking a lease, purging). For read, the pid is also removed. | 3 |
| `Held { reason }` | `approvals_backlog`: too many pending on-behalf changes; nothing applied | 2 |
| `RevertTooOld { seq }` | the change is outside the revert window; use `restore --version` | 8 (never retry) |
| `NoSuchChange(n)` | no change (revert) or version (restore, show, diff, name) `n` | 4 |
| `NotJoined { conn }` | The connection isn't in this doc: join again | 2 (or rejoin silently) |
| `Stale { expected, actual }` | `edit(.., if_version)`, `edit_with_options` with `if_version`, or an edit verb's `--if-version` (amendment 14), but the doc moved; nothing applied. Displays as one line, `doc changed since {expected} (now {actual})`, with no doc text | 1 (re-read and retry) |
| `RateLimited { retry_after_ms }` | over the edit rate | 2 |
| `Declined(Declined)` | file can't be live, or live editing is off | 4 or "opens as today" |
| `NotFound` | no doc and no bytes | 4 |
| `PackMismatch` | stored doc belongs to another pack | 4 |
| `BadParticipant` / `BadPosition` / `BadEdit` / `Doc(_)` | bad input; nothing changed | 2 |
| `Doc(DocError::Unaccepted)` | hardening: a client update carries content the kit never uses (subdocuments): refused at ingress, before anything is applied (v0.9.5). Error id `MPC-E152` `unaccepted` (`doc::RESERVED_ERROR_ID_UNACCEPTED`) | 3 |
| `Doc(DocError::Quarantined)` | hardening: a stored doc holds such content: quarantined, never loaded as live (the store-load scan, v0.9.5). Error id `MPC-E518` `quarantined` (`doc::RESERVED_ERROR_ID_QUARANTINED`) | 3 |
| `NoApproval { seq }` | no approval to settle (the change wasn't made on anyone's behalf, or doesn't exist) | 4 |
| `BadApproval` | already settled the other way | 2 |
| `BadPath` | `open_file` / `file_moved`: not a regular file inside the root | 2 |
| `Disk(_)` | reading or writing the file failed; nothing lost, write-back retries | 1 |
| `Store(_)` | store failure; nothing acked. A store error naming something that doesn't exist (`NoDoc`, `NoVersion`, `NoApproval`, `NoBlob`, `NoBlobSpace`, `PurgedVersion`) is **4**; `CurrentVersion` (purge) is 2 | 6 |
| `Pack(_)` | pack bug or damaged codec settings | 1 |
| `Deleted` | the doc was deleted while this session was loaded | 4 |
| `Unloaded` / `Released` / `Broken` | stale handle: open again | retry |
| `Leased` / `NoLease` / `Blob(_)` / `NoBlobStore` | tier 0: see [Tier 0](#tier-0-any-file) | 5 / 2 / 1 (`TooLarge`, `NotFound` 4; `Store(e)` as `Store(e)`) / 4 |
| `Claimed { id, by, name, lines: (first, last), remaining_ms, note }` | someone else's live claim, or a hand-off reservation held for someone else (an agent's claim or `Respect` edit): nothing applied | 5 (as `Leased`) |
| `NoClaim(Option<ClaimId>)` | no live claim by that id (or, no id: you hold none) | 4 |
| `ClaimContended { id, waiters, remaining_ms }` | renewing or holding a claim others wait for | 2 |
| `BadClaim(_)` | not your claim, waiting on your own, a zero TTL, too many claims, several and no id | 2 |

`edit_with` returns `EditWithError<E>`: either `Session(SessionError)` or `Compute(E)` (the
tool's own error, e.g. "pattern not found": exit 1).

A conflict is not an error. It is a value (`WriteOutcome::Conflicts`, `UndoOutcome::Conflicts`
/ `Blocked`): exit 1, and nothing is applied. `WriteOutcome::BaseExpired` is exit 8.

## Parity

`k2_mpc_base::parity` (K-PAR-1…3) proves the parity rule: everything a person can do in an
editor, an agent can do through a verb, with the same effect.

- **`PersonAction`**: one variant per row, P1–P34 (`ALL`, `number()`, `describe()`,
  `Display` = `P13`, `tiers()`, `in_tier(t)`, `for_tier(t)`). P32–P34 are tier 0's leases
  (`Lease`, `ReleaseLease`, `BreakLease`); P1–P31 are text actions, and those a blob doc
  also has are tagged both (P1–P4, P6, P14, P17–P22, P24, P25, P28–P30). **Not** `#[non_exhaustive]`, on purpose: packs map it with an
  exhaustive `match`, so adding a row breaks every pack's build until it is mapped
  (a `compile_fail` doctest holds that).
- **The verb registry** (K-PAR-2). `VerbSpec { name, kind: Read | Edit | History, needs:
  [Need], api: [Api], serves: [PersonAction], usage, exits, source: Kit | Pack, tiers:
  [Tier] }` (`applies_to(tier)`; hosts classify verbs from it).
  `runs_through_run_tool()`: whether a host routes it to `run_tool` (a flag per verb, as
  data: `RUN_TOOL_VERBS`, the kit's text tools, plus every pack verb; tests hold it to
  `run_tool` itself, both ways). `alias_of()`: `ALIASES`, verbs that are another name for a
  verb, the same row and binding (`approvals` = `log --pending`, P30, host-neutral). Both tiers: `open`,
  `leave`, `cat` (tier 0: the file's bytes), `who`, `write`, `status`, `flush`, `log`,
  `show --version`, `restore`, `version --name`, `mv`, `blame`, `approve`, `decline`,
  `log --pending` and `approvals` (tier 0: by version); tier 0 only: `lease`,
  `lease --renew`, `lease --release`, `lease --break` (owner); every other verb is text.
  `KIT_VERBS` are the kit's (the tools, the session verbs: `open`, `leave`, `who`,
  `follow`, `cursor`, `write`, `undo` / `redo` / `undo --drop` / `undo --partial`,
  `status`, `flush`, `watch`, `tail -f`, `log`, `diff --version`, `show --version`,
  `restore`, `version --name`, `mv`, `approve`, `decline`, `log --pending`, `approvals`, `revert`,
  `blame`, `claim` / `claim --renew` / `claim --release` (P8), `activity` (P6), and tier
  0's lease verbs).
  `Registry::for_tier(tier)` is the kit's verbs of one tier; `Registry::kit()` is the text
  tier, and `Registry::for_pack(&pack)` adds the pack's `verbs()` to it (refusing a name
  any kit verb has).
  A verb's **name is its CLI form**: `verb_form(argv)` gives it (`sed -i`, `undo --partial`,
  `log --pending`...), so a host's command table can be checked against the registry.
- **A pack's list as data**: `ParityRow::live(action, verbs)`, `live_but(action, verbs,
  later)`, `deferred(action, later, until, why)`, `not_applicable(action, shown_by, absent,
  reason)`. A list may name an alias under its row or leave it out; the kit's cases run
  both names.
- **The harness**, `parity::check(pack, rows, &registry, extra_cases) -> Result<Report,
  Vec<Failure>>`, collects every failure:
  1. exactly one row per action **of the registry's tier** (rows for other tiers' actions
     are optional, and checked if present);
  2. the list and the registry agree both ways: a live row's verbs are registered and
     serve it; every registered verb is listed under each action it serves; a deferred or
     N/A row's verbs are **not** registered (when one lands, the row must go live), and a
     deferred row the kit has cases for is stale;
  3. every listed verb has a case, and every case agrees. A `Case` runs twice, on two
     fresh docs (a store, a manager, a file in a temp folder, an observer): the **person
     path** (`Person`: a tab, `via: "tab"`, the session API directly, byte positions,
     Ctrl+Z as `undo_with(Partial)`) and the **agent path** (`Agent`: registered verbs
     only, `Agent::verb(argv, stdin)`; a form missing from the registry fails as
     `MissingVerb`; the reference binding may call only the declared `api` (any verb may
     `join_shared`, P1); an exit not in `exits` fails, unless it is a session refusal).
     Then it compares the **text, file bytes, key, disk state, changes** (kind, author,
     on behalf of, ± lines, approval), **versions** (reason, name, authors; a
     `BeforeAgent` cut is an agent-only safety net and left out), **approvals**, the
     **actor's roster entry** (name, colour, role, caret line:col, viewing range,
     connections, away), the **exit code** and the **output**.
  The actor has the same pid, name and colour in both worlds; a change by the actor that
  came through another surface than its path's handle fails the case.
- **The kit's cases** are one exhaustive `match` over `PersonAction` (`kit_cases`): 61 cases
  over the 27 rows the kit binds (amendment 13 added `view --for-edit` under P4, `activity`
  under P6, and five claim cases under P8; the actor's roster entry now compares its claims:
  `ClaimObs { lines, hold, note, ttl_ms }`); empty for annotations (P23, P27) and purge (P28), which a
  pack lists as deferred. P22's person path is what a tab's blame gutter shows (the same
  `Session::blame` call: every line, then the lines in view), its agent path the `blame` verb
  (`-L a,b`); a third case checks both are refused without the history right. A pack adds `Case`s for its own verbs.
  `check_list` runs steps 1–2 and case coverage without opening docs; `run_case` runs one.
  Both refuse a registry of another tier (`Failure::Registry`).
- **Tier 0** (`check_blob(blob_pack, rows, &Registry::for_tier(Tier::Blob), extra)`,
  `check_blob_list`, `run_blob_case`): the same three steps on blob docs. A `BlobCase` runs
  on two fresh blob worlds (`BlobStage`: a store, a blob store with a 64 KiB limit, a file
  `report.pdf`, an observer); the person path is `BlobPerson` (the `BlobSession` API, a tab),
  the agent path `BlobAgent::verb` (registered verbs only). Compared: key, file bytes, disk
  state, current version, every version (without `via`, checked against the path, and the
  keyed blob id), the lease, every approval record (version, state, by), the actor's roster
  entry (`holds_lease`, `region`...), exit and output. `kit_blob_cases` (one exhaustive
  `match`): 31 cases over 16 live rows, 8 of them for the leases (take, held by someone
  else: 5, renew, renew with none: 2, release, not yours: 1, break as owner, break without
  owner: 3) and 4 for the approvals (approve, decline with a later write, approve a write
  made for someone else: 3, `log --pending`). `BlobStage::assistant(pid, person)` joins an
  assistant acting for a person.
- Hunk shapes may differ between the paths (a typed hunk against a verb's minimal diff);
  the observable result may not. Edits are compared as `edit_summary` (version, ± lines,
  the lines touched), undo outcomes as `undo_summary`.

## Parity rows served (`k2-mpc-markdown/docs/PARITY.md`)

| # | Verb | Session API |
|---|---|---|
| P1 | `open` (any verb joins) | `SessionManager::open` + `join` (`Stay::Held`) for `open`; `join_shared` (`UntilIdle`) for one-shot verbs |
| P2 | `leave` / idle | `leave(conn)`; `expire_idle` |
| P3 | `cat [-n]` | `run_tool` (`cat`: `read`'s bytes) |
| P4 | `head`, `tail`, `sed -n`, `view --lines` (`--for-edit`: a short claim too) | `run_tool` (read + viewing range) |
| P5 | `grep` | `run_tool` (`grep`) |
| P6 | `who`, `activity` | `roster` (one entry per pid, with a connection count, claims, last edit); `activity` |
| P7 | `follow` | `roster` (`CursorAt` line:col; an agent's caret is at its last edit) |
| P8 | `cursor --at L:C` / `--select`; `claim`, `claim --renew`, `claim --release` (K-PRES-12) | `set_cursor(At::Line(..))`; readers: `set_view`; `claim` / `renew_claim` / `release_claim` |
| P9–P11 | `insert`, `replace`, `delete` | `run_tool` (inside `edit_with`) |
| P12 | `replace --old/--new [--all]`, `sed -i` | `run_tool` (inside `edit_with`) |
| P13 | `patch`, `apply`, `append`, `replace-section` | `run_tool` (inside `edit_with`); `replace-section` is the Markdown pack's (`Pack::verbs`) |
| P14 | `write --base V` / `--base live` | `write_base(V)` / `replace_text` |
| P15 | formatting | as P9–P13 (Markdown formatting is text) |
| P16 | `undo`, `redo`, `undo --drop` | `undo` / `redo`, `undo_with` / `redo_with` (`UndoMode::All`, `Partial`, `Drop`; `for_person`) |
| P17 | `status`, `flush` | `status` (`disk`); `flush(conn) -> FlushOutcome { written, hash, disk }` |
| P18 | `watch`, `tail -f` | `join(Stay::Held)` + `subscribe` |
| P19 | `log`, `diff --version`, `show --version` | `log(LogQuery)`, `diff_version`, `show`, `versions` (need history) |
| P20 | `restore`, `version --name` | `restore` (a forward change by the restorer, after a version), `name_version` |
| P21 | `mv` | `SessionManager::rekey` + `Session::file_moved` |
| P22 | `blame [-L a,b]` | `blame(conn, root, lines)` (needs history); tier 0: `BlobSession::blame` |
| P23, P27 | `comment`, `suggest`... | later (M5, the annotation layer) |
| P24 | `export [--to as-is]`, `export --version V [--to as-is]` | `file_now` / `file_at` (tier 0: `BlobSession::file_now_to` / `file_at_to`), then `export::ExportRegistry::export` (the kit's target: `as-is`); the person's export and the agent's give the same bytes, doc version and codec flag; `--version` needs history |
| P25 | why live editing is off | `open` → `Declined { code, message }`; `status().declined` |
| P26 | Ctrl+Z (`undo --partial`) | `undo_with(UndoMode::Partial)`; keystrokes grouped by `EditOptions::group` (U3, v0.9.7) |
| P28 | `purge` | later (redaction) |
| P29 | `approve`, `decline` | `set_approval(conn, seq, Approved \| Declined)` (only the person it was made for); tier 0: `BlobSession::set_approval(conn, version, ..)` |
| P30 | `log --pending --on-behalf-of me`, or its alias `approvals --on-behalf-of me`; `--all` for settled ones too | `log(LogQuery::pending().on_behalf_of(pid))`; a tab may use `approvals`; tier 0: `BlobSession::log` (`approval`) / `approvals` |
| P31 | `revert <change>` | `revert(conn, change, partial)` (author, the person, or owner; too old: exit 8) |

## Not in this slice

- **Disk, host side and later rows:** watching (the host's), the 24 h grace after a delete
  and K-DISK-17's named version of unflushed edits (the host, from `FileMissing { unflushed }`),
  one session per file identity (K-DISK-14: the host keys by device + inode), hardlinks
  (K-DISK-15), synced-folder conflict copies (K-DISK-16), the git branch-switch rule
  (K-DISK-13).
- **C5, wire:** the socket, Yjs sync and awareness, each socket being a `join` connection.
  This needs the strict decoder.
- **C6, rest:** `write --base --partial` (K-BR-9); the `spec_undo::model`
  oracle and trace test (K-HIST-14/15).
- **Agent presence, host side:** `claim --wait [--timeout]` and an auto-wait under
  `Respect` (the host, on `register_waiter` and `ClaimEnded`); rendering notes and
  `--presence json`; gutter marks for agents' views and claims in tabs. Claims and notes on
  tier-0 blob docs.
- **Erasure, later rows:** a `purged` event to subscribers (K-PURGE-6: `ChangeEvent` is a
  struct, so the host relays `AuditEvent::Purged` for now) and the `purged` frame (G4);
  the 30-day lifetime of an orphaned doc's data (K-PURGE-8); backups (K-PURGE-9); held
  refused client ops (V4: no wire yet); annotations (not built); participant erasure on
  blob docs.
- **Blame, later:** blame of a version (`blame --version V`, with E-ERASE-7's `[erased]`
  spans); blame through the wire (a Yjs client's own updates must record their clock spans
  too when C5 lands); char-level blame export at the native migration (K-FMT-5, G5).
- **After C7a:** `diff
  --version` from the **disk** (the host reads the file and uses the `diff` tool); thinning
  change summaries (kept for the doc's life today); retention for docs that are never loaded
  again (it runs on `tick` for loaded ones); Ctrl+Z with unacked ops (G3). Person grouping
  is in v0.9.7 (`EditOptions::group`).

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
- `log(conn)` (needs history, the `log` verb on a blob doc): one `BlobLogEntry` per version,
  **oldest first**: `{ id, kind, of, author, author_name, via, at_ms, on_behalf_of, name,
  redacted, size, mtime_ms, conflict_copy }` (names as the text log's `ChangeInfo` /
  `VersionInfo`). `BlobLogKind`: `Import`, `Write`, `Restore` (`of` = the version restored),
  `External`, `ConflictKept` (an outside save that conflicted; what it replaced is the
  previous version and the conflict copy), `Merged` (tier 0.5; the save is the entry
  before). Built from the stored versions: a conflict whose copy couldn't be written logs
  as `External`. `on_behalf_of` is what the version recorded.
- `restore(conn, v)` needs edit and history: a **new** version pointing at v's blob,
  attributed to the caller (`Restore`, `restored_from: v`). History is never rewritten. The
  lease rule applies; no base (deliberate, like `--base live`).
- `name_version(conn, v, name)` needs edit. Retention reuses the store's rule:
  `DocStore::delete_blob_version` refuses a named version (`NamedVersion`) and the current one
  (`CurrentVersion`), and releases the blob's reference. Nothing deletes versions on its own.

### On-behalf writes and approvals (K-ID-10, K-ID-11, K-T0-9; amendment 11)

The text doc's rules ([On-behalf edits and approvals](#on-behalf-edits-and-approvals-k-id-10-k-id-11)),
with the version in place of the change:

- A `write` or `restore` by a participant with `on_behalf_of` applies **at once**, like any
  write (never held), and the store records `Approval { state: Pending, by: None, at_ms:
  None, author: agent, on_behalf_of: person }` for that version **in the same transaction**
  (table `blob_approvals`, keyed `(doc, version)`). Equal bytes record no version and no
  approval. Versions the kit makes (import, outside saves, merges) never have one.
- `set_approval(conn, version, Approved | Declined) -> BlobApproval`: needs read; only the
  connection's pid **equal to that version's `on_behalf_of`**; anyone else (the agent, other
  people, owners) gets `Forbidden { need: Approver }` and `AuditEvent::Refused { action:
  Approve, need: Approver }`. `Pending` as a target, or the other state once settled, is
  `BadApproval`; settling it again the same way returns the record unchanged (no event); a
  version without one is `NoApproval { seq: version }` (exit 4). It emits
  `AuditEvent::Approval { key, seq: version, pid, state }`. The lease doesn't apply.
- **Decline keeps history** (K-HIST-19 for tier 0): the version stays; blobs have no
  `revert`, so `BlobApproval { version, approval, restore_to, later }` says how to reverse it:
  `restore_to` is the version before it (`None` if that one was purged) and `later` the
  versions written after it, which a `restore --version restore_to` would also replace as the
  current content (blobs don't merge; they stay versions). The kit never restores on its own.
- `approval(conn, version)` and `approvals(conn)` (the tab's "waiting for me" list: the
  `Pending` ones for my pid) need read; the agent and the person see their own records,
  everything else needs history. `log` entries carry `approval`, `blame` the current
  version's; `who` shows "agent for person" (`participant.on_behalf_of`), as for text.

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
- It holds an older write of the kit's made since the last outside save (an app saving a
  stale buffer, a late echo; the last 8, in memory) → `OwnWrite`, reported as
  `AuditEvent::EchoSuppressed`; the next write-back puts the current version back. Each
  outside save taken in forgets them (v0.9.5).
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
    it replaced stays a version **and** is written to a **conflict copy** (K-A11Y-5):
    `name (conflict <who> YYYY-MM-DD HHMMSS).ext` (UTC, host clock; `who` is the replaced
    version's author name, or the lease holder's when the kit wrote it; never over an existing
    file: ` 2`, ` 3`... is added; `BlobPack::conflict_copy_name` may rename it). It goes in
    the folder `Host::conflict_dir(real file path)` returns (the kit creates it; a host keeps
    it out of version control), or beside the file when that is `None` (the default). The
    copy is recorded on the save's version (`conflict_copy`) as its **path relative to the
    file's folder**: a plain file name when beside it (as every copy written by v0.3.x and
    earlier is), so old records still resolve. If the folder can't be made, no copy is
    written (`copy: None`, a `DiskError`); the content is a version either way. Returns
    `DiskCheck::Conflict { version, kept, copy }` and emits `AuditEvent::BlobConflict
    { current, kept, copy, lease_holder }`: tell both the lease holder and the person.
    The holder's next write names the old base and gets `Stale`.
  - **Why the save wins:** the person just saved it and their app shows it; putting the
    session's version back would overwrite the file under their app, which would save
    again. Nothing is lost either way: both are versions, and the session's is also a file
    (the conflict copy). If even the copy can't be written (a read-only folder), the version is
    still kept (`copy: None`, `DiskError` event).
- A file over `max_blob_bytes` is declined (`FileDeclined { code: "too_large" }`); writes
  are refused until it shrinks. A missing file stops write-back (`Missing`), as for text.

`flush(conn)` writes the current version now (`FlushOutcome::hash` is the blob id).
`file_moved`, `file_path`, `check_disk` work as for text docs. A blob doc can't be
`rekey`ed to another space (its blobs are keyed by space).

### Status, config and errors

`status()` → `BlobStatus { key, version, current, participants, connections, lease, disk,
conflict_copies }`: `conflict_copies` lists the recorded copies that exist now as regular
files (`ConflictCopy { path, version, kept, at_ms }`: the save that made it, the version it
holds), oldest first; `conflict_copies(conn)` (history) gives their paths.

**Conflict-copy cleanup** (K-A11Y-5): on `tick` (at most every `history_tick_ms`), a recorded
copy older than `conflict_copy_keep_ms` (30 days) is deleted when it is still a regular file
holding exactly its version's bytes (compared by keyed id). Never deleted: a file the kit
didn't record, a copy a person edited (remembered by its stat, not re-hashed until it
changes; edited back to the version's bytes it is the kit's again), a symlink, a copy whose
version (the content or the save) is named, a copy of a purged version. Removals are
reported in `TickReport::conflict_copies_removed`. Versions are never touched. It runs for
loaded blob docs only (as text retention does).

| `Config` field | Default | Effect |
|---|---|---|
| `lease_default_ms` / `lease_max_ms` | 5 min / 1 h | lease length when not given / the cap |
| `lease_auto_ms` | 2 min | a write's own lease, if the writer dies mid-upload |
| `blob_merge_max_bytes` | 32 MiB | tier-0.5 merges are tried only below this |
| `conflict_copy_keep_ms` | 30 days | conflict copies older than this are cleaned up on `tick` |

| Error | Meaning | Suggested exit |
|---|---|---|
| `Leased { holder, name, expires_ms }` | someone else holds the lease; nothing written | **5** |
| `Stale { expected, actual }` | `write` from an old base; nothing written | 1 |
| `NoLease` | renewing when nobody holds it | 2 |
| `Forbidden { need: Owner }` | breaking a lease without `Rights::owner` | 3 |
| `Blob(BlobError)` | `TooLarge`, `NotFound` (4); `Store(e)` as `Store(e)` (4 for a missing thing, else 6); `Damaged`/`Io` (1) | 1, 4 or 6 |
| `NoBlobStore` | the host gave the manager no blob store | 4 |

**Proposed for the host:** a "lease requested, waiting for the holder" state (a card) is the
host's, not the kit's; the kit only ever says `Leased`. Exit 2 already means "bad input /
rejoin / rate limited", so a separate code (e.g. 6) or `Leased` plus a `pending_request`
field in `--json` would be clearer than reusing 2.

**Not yet (tier 0):** a pending cap for on-behalf blob writes (text's `approvals_backlog`;
see amendment 11), participant erasure on blob docs, automatic version retention, the K-T0-4
fuzz target, and hashing outside the session lock (a large outside save is hashed under it
today). Purge is in [Erasure](#erasure).

### Tier 0 → text (v0.9.2)

A file refused as text once (mixed line endings, a NUL byte, too large) lives as a blob doc.
When it reads as text again, the host **promotes** it: the same key and doc id become a text
doc of a pack, and its tier-0 history is kept.

`SessionManager::promote_blob_file(pack, &key, path, root) -> (Arc<Session>, Promotion)`
(`promote_blob(pack, &key)` for a doc without a file). The host calls it when `open_file`
with a text pack says `PackMismatch { stored: "kit:blob" }` and the file now decodes. It is a
manager call with no participant, like `delete_doc`: the host checks its own rights first.

1. The blob doc is loaded and reconciled with its file (an outside save, such as the fix, is a
   version first) and written back, under the blob session's lock.
2. Every version is read and decoded by the pack, oldest first (a version over
   `disk_read_max_bytes` isn't read).
3. **Each version that decodes becomes a text version**, built as an import chain through the
   engine (no crafted bytes):
   - one change per version whose text differs from the one before (the first is
     `ChangeKind::Import`; then `Edit` for a write, `External` for an outside save, `Restore`
     of the text version for a restore of a version that became text, else `Edit`);
   - each change is by the version's **author**, at its **time**, with its `via` and
     `on_behalf_of`; blame, `log` and revert see them like any change (their revert records
     are diffs of the texts, like an outside edit's);
   - the version is cut at that change's position, with the blob version's time, author and
     **name**. Its reason is `Import` for the first, `External` for an outside save or a merge,
     `Idle` otherwise. A version whose text equals the one before (only its line endings or BOM
     changed) gets a version and no change;
   - an on-behalf version's **approval** moves to its change, in the state it was in
     (approved, declined or pending; a settled one doesn't count against the pending cap).
4. **Versions that don't decode stay as tier-0 versions, listed as the doc's tier-0
   history**: same ids, entries, bytes and approvals. So do purged versions (their entries
   only) and an on-behalf version with no text change (no change can carry its approval).
   They are not text versions: a text doc can't restore bytes that aren't text, and the
   text version list stays restorable. Keeping them as they were means nothing is converted
   or lost, and their ids, approvals and purge state carry over unchanged.
5. **One store transaction** (`DocStore::promote_blob_doc`) writes the text doc (metadata,
   clients, log, change summaries, revert records, approvals, versions, the sync state and
   a fresh hash key) and removes the blob doc's sync state and the rows of the versions that
   moved, releasing their blobs. A crash before it leaves the blob doc exactly as it was; after
   it, the doc is text. **No store-format change** (format 5): no new table or record version.
6. After the commit: the moved versions' blobs that nothing else uses are removed **at once**
   (`Promotion::blobs_removed`), so a later purge of the text can't miss their bytes. The blob
   session closes (later calls: `Unloaded`) and `AuditEvent::Promoted { key, connections,
   text_versions, changes, tier0_kept }` names the connections that were in it: tell those
   tabs to open the text doc. Then the text session opens on the file.

`Promotion { key, versions, changes, blobs_removed, connections }` lists every tier-0 version
and what it became (`Promoted`): `Text { version, seq, change, tier0_kept }`, `NonText { code }`
(the pack's reason: `mixed_line_endings`, `binary`, `not_utf8`, `too_large`; `unreadable` for a
damaged blob), or `Purged`.

**Errors** (nothing changed): `NotFound`; `PackMismatch` when the doc isn't a blob doc;
`Leased` while anyone holds the lease (break or wait for it); `Declined` when the current
version doesn't decode; `Disk` when the file can't be written back first; `NoBlobStore`. An
error opening the text session after the switch leaves the doc promoted: open it with
`open_file`.

**Rights** are per key, so they carry over as they are. **Purge** carries over: a purged
version stays purged (its entry in the tier-0 history) and nothing of it is imported.

**The tier-0 history** on the text `Session` (empty for a doc that was never tier 0):

| Call | Rights | What it does |
|---|---|---|
| `tier0_log(conn)` | history | the kept versions as `BlobLogEntry`s, oldest first (as the blob doc's `log`) |
| `read_tier0_version(conn, v)` | history | one kept version's bytes, streamed and verified (`PurgedVersion` once purged; `NoVersion` for one that became text: read it with `show`) |
| `set_tier0_approval(conn, v, state)` | the person | settle a kept on-behalf version's approval (the blob rule; `restore_to` is `None`: a text doc can't restore it) |
| `purge_tier0(conn, Option<v>)` | owner | purge one kept version, or all, as `BlobSession::purge` does (no version is current here) |

A text `purge` with `PurgeScope::History` purges the tier-0 history too. Other scopes don't
search it and say so with `NotReached::Tier0History(n)`.

**What changes for the history:**
- text retention (K-HIST-7) applies to the imported versions from now on, by their original
  times (blob versions had none); named ones are kept;
- a text version holds the text, not the bytes: restoring one writes it in the doc's
  current format (line endings and BOM of the current version);
- conflict copies recorded on versions that moved are no longer tracked: their files stay
  where they are, and the kit's cleanup won't delete them.

An older build (v0.9.1) reads a promoted doc as a text doc and ignores its tier-0 rows;
deleting the doc there still releases their blobs (deletion erases every blob row of a doc).

## Store: history tables

Seven tables beside the log, created on open: `changes` (summary and hunk shape per change;
`DocStore::change`, `scan_changes`), `change_clocks` (the engine clocks each change's
inserted text occupies: blame's index, v0.3.0), `provenance` (store format 5: for each run of
elements an undo, redo or revert put back, the root it restores; element IDs only, written
in the change's own transaction, left alone by a purge), `reverts` (revert records; version
2 adds each hunk's roots), `revert_index`, `revert_carry`, `revert_stats`; plus
`DocStore::version_infos` (version headers without content). **Store format 5** (reads 4;
**no downgrade**): a format-4 store is stamped 5 in place on open (the `provenance` table is
created; nothing is backfilled, since every element made before is its own root); a
format-4 build refuses a format-5 store with `StoreError::Newer { found: 5 }`. Content lives only in `reverts` (and versions, the log, snapshots and the sync
replica); a purge redacts them all ([Erasure](#erasure)). Changes made before this slice have no summary or record (`revert` says too old).
Deleting a doc erases its rows here too ([Deleting docs](#deleting-docs)).

## Store: the host's table

`DocStore` has a small key-value table for the host's own records next to the docs (same
transactions, hand-off and backups): `host_put(ns, key, value)`, `host_get`, `host_delete`,
`host_scan(ns, prefix) -> Vec<(key, value)>` (key order, one consistent read), and
`host_batch(&[HostOp])` for several writes in one transaction. Each call is one durable
transaction. Namespaces and keys are opaque strings: **the kit never reads, interprets or
erases these rows**, so erasing them (a person's data, a deleted workspace) is the host's job.

## Changelog

### Amendment 18 (unreleased, additive): stable error ids

- Every refusal has a stable id, `MPC-Ennn`, in one registry (`k2_mpc_base::errors`,
  `docs/ERRORS.md`): never reused, never removed, never changing an exit code. `report()` on
  every error type, `refusal()` on `WriteOutcome` and `UndoOutcome`, and
  `ToolOutput::refusal` for an edit verb's in-band "no" give an `ErrorReport` (plain text
  with the id appended, or the `--json` object). `explain` and `capabilities`
  (`errors::run_meta`) are generated from the registries. Hosts: `docs/HOST-ERRORS.md`.
- `ToolOutput::stderr_summary` of an in-band refusal ends with ` (MPC-Ennn)`.

### Amendment 17 (v0.9.3, additive): edits on the doc itself, the runtime check, outside edits at element identity

- **Edits on the doc itself:** every edit a session makes (verbs, `write --base`, undo, redo,
  revert, restore, outside edits, import, promotion) is made on the doc as the participant's
  client (`transact_mut_as`), not on a throwaway replica of the whole doc. Results are
  byte-identical; the cost no longer follows the doc's history the way the replica's did
  (`docs/evidence/v0.9.3-edit-path.md`). Nothing changes for hosts.
- **The runtime check:** `Config::edit_check_every` (default 1,000; [Limits](#limits-config)),
  `AuditEvent::EditPathMismatch { key, client, change }` and `TextCacheMismatch { key, root,
  change }` (additive: `AuditEvent` is `#[non_exhaustive]`).
- **Outside edits** are reported to undo regions and subscribers at element identity (from
  each element's ID before and after the merge), not as a diff of the live text: in repeated
  text the diff named other elements, and an undo could conflict on text nobody had touched.

### Amendment 16 (v0.9.2)

- **Claims at a shared edge** ([Agent presence](#agent-presence-amendment-13)): two
  participants' claims never overlap. Text typed where they meet joins the typist's own
  claim if they hold one there, otherwise the one acquired first; the other claim carries
  it outside and isn't touched by it (`Edited`, `Respect`). Was: both took it, and the two
  claims overlapped (the v0.9.1 soak's finding).
- **`approvals --all`** (and `log --pending --all`): settled approval records too, newest
  first; P30 gains a case for each name, both tiers. `LogQuery::settled` /
  `LogQuery::with_settled()` (additive: `LogQuery` is `#[non_exhaustive]`).
- **Tier 0 → text promotion keeping history** ([Tier 0 → text](#tier-0--text-v092)):
  `SessionManager::promote_blob_file` / `promote_blob` turn a blob doc that now reads as text
  into a text doc with the same key and doc id, in one store transaction (no format change).
  Versions that decode become text versions (one change each, by the original author at the
  original time, approvals carried); the rest stay as the doc's tier-0 history
  (`Session::tier0_log`, `read_tier0_version`, `set_tier0_approval`, `purge_tier0`). New:
  `Promotion`, `Promoted`, `AuditEvent::Promoted`, `NotReached::Tier0History`.

### Amendment 15 (v0.9.1, additive): one guarded edit call, a `run_tool` flag, `approvals`, one BOM

- **`Session::edit_with_options(conn, root, &EditOptions, f)`**: `edit_with` with the claim
  policy and `if_version`, both under the lock ([Editing](#editing)).
- **`VerbSpec::runs_through_run_tool()`** (data: `parity::RUN_TOOL_VERBS`, plus pack verbs)
  and **`VerbSpec::alias_of()`** (`parity::ALIASES`).
- **`approvals [--on-behalf-of me|P]`**: a host-neutral alias of `log --pending` (P30), both
  tiers. Pack lists may leave it out.
- **`Pack::writes_bom(root, codec)`** (provided) and `codec::text_file::BOM`: a `write` body
  read with `cat` no longer doubles the BOM ([Writing a BOM file](#writing-a-bom-file)).
- **Fixed:** blame keeps `restored_by` on a partly restored line ([Blame](#blame): ties);
  last-edit ranges no longer expand over others' text or point past the end
  ([Last edits](#agent-presence-amendment-13)).

### Amendment 14 (v0.8.0): `--if-version` on edit verbs; claim ends carry lines and version

**Additive (tools):** [`--if-version V`](#--if-version-on-edit-verbs-amendment-14) on every
edit verb through `run_tool` (kit and pack verbs): checked under the session lock with
`edit`'s `if_version` check, before every other refusal but admission; stale is
`SessionError::Stale` (exit 1), nothing applied. Read verbs refuse it (exit 2).
`tools::Tool::if_version()`. No new error variant, exit code, or store change (format 4).

**Behaviour:** `SessionError::Stale` displays as `doc changed since {expected} (now
{actual})` (was `doc moved: expected version {expected}, it is at {actual}`), for `edit`'s
`if_version` and tier 0's stale base too. `sed --i` (an abbreviation of `--in-place`) is now
ambiguous with `--if-version` (exit 2); `sed -i` and `--in-place` are unchanged. The parity
registry's edit verb usages name `[--if-version V]`.

**Breaking (events):** `AuditEvent::ClaimEnded` and `AuditEvent::ReservationEnded` gain
`lines: LineRange` and `version: Seq`, the range's current lines and the doc's version as
it ends ([Claims](#claims)). The variants aren't `#[non_exhaustive]`, so a host that
destructures them without `..`, or builds them, must add the fields.

### Amendment 13 (v0.7.0): agent presence (claims, the claim policy, notes, activity)

**Additive (Rust API):** [Agent presence](#agent-presence-amendment-13). `Session::claim`,
`renew_claim`, `release_claim`, `claims`, `claim_remaining_ms`, `register_waiter`,
`cancel_waiter`, `reservations`, `activity`; `edit_with_policy`, `replace_text_with`, `write_base_with`
(`WriteReport`), `run_tool_with` (`tools::ToolOptions`); types `ClaimId`, `ClaimInfo`,
`ClaimPolicy`, `ClaimEnd`, `Waiting`, `ReservationInfo`, `ReservationEnd`, `ForEdit`, `Note`, `NoteKind`, `Notes`,
`ActivityEntry`, `ActivityKind`. Fields on `#[non_exhaustive]` types: `RosterEntry::claims`,
`last_edit`; `ToolOutput::notes`, `for_edit`; `ReadOutput::touched`; `Config` (twelve, see
Limits). Enum variants (all `#[non_exhaustive]`): `SessionError::Claimed` (5), `NoClaim`
(4), `ClaimContended` (2), `BadClaim` (2); `AuditEvent::ClaimTaken`, `ClaimEnded`,
`Reserved`, `ReservationEnded`;
`Action::Claim`, `Activity`; `parity::Api::Claim`, `RenewClaim`, `ReleaseClaim`, `Activity`;
`parity::VerbValue::Claim` (boxed), `ClaimsReleased`, `Activity`. **No new exit code:** `Claimed` is
5, `LEASED`'s meaning (someone else holds it for now). Read verbs take `--for-edit [--ttl
MS]`. Memory only; **no store change** (format 4).

**Behaviour:** none for existing calls (the old edit calls are `ClaimPolicy::Ignore`), except
that a connection holding, waiting for, or reserved a claim's range doesn't idle out until
that ends. **Claims coordinate agents only:** `Respect` and reservations refuse
`ParticipantKind::Agent` and nothing else.

**Breaking (text parity lists, harness struct):** the registry gains `claim`, `claim
--renew`, `claim --release` (serving P8) and `activity` (P6). A text pack's P8 row must
list the claim verbs (`ParityRow::live(P8, &["cursor", "claim", "claim --renew", "claim
--release"])`; the old `intent` placeholder goes) and its P6 row `activity`, or `check`
reports `VerbNotListed`. `parity::PresenceObs` gains `claims: Vec<ClaimObs>` (a pub struct
without `#[non_exhaustive]`: a pack that builds one by hand must add it). `PersonAction`
is unchanged.

### Amendment 12 (v0.6.0): exact-text replace (`replace --old/--new [--all]`)

**Additive (tools):** a third mode of `replace`, exclusive with `--lines`/`--range`
([Tools](#tools)). Exit 0 applied (or a no-op when `--old` equals `--new`), 1 no match or
more than one (nothing applied, report on `stdout`), 2 usage. No new type or error variant;
`ToolOutput.stdout` / `EditOutput.stdout` now also carry its report. No store change.

**Breaking (text parity lists only):** `replace` serves P12 (find and replace) besides
P10 and P15, with exits `0, 1`, and P12 has three `replace` cases in `kit_cases`. A text
pack's P12 row must list `replace` (`ParityRow::live(P12, &["sed -i", "replace"])`), or
`check` reports `VerbNotListed`. `PersonAction` is unchanged; tier 0 is unaffected.

### Amendment 11 (v0.5.0): approvals for on-behalf blob writes (K-T0-9); store format 4

**Additive (Rust API):** `BlobSession::set_approval`, `approval`, `approvals`;
`BlobApproval { version, approval, restore_to, later }`; fields on `#[non_exhaustive]` types:
`BlobLogEntry::approval`, `BlobBlame::approval`, `PurgeCounts::approvals`,
`parity::BlobObservation::approvals`; `parity::VerbValue::BlobApproval`;
`BlobStage::assistant`; `DocStore::blob_approval`, `blob_approvals`, `set_blob_approval`.
No new error variant: `NoApproval { seq }`, `StoreError::NoApproval(doc, seq)` and
`AuditEvent::Approval { seq }` carry the version for a blob doc (as `ExternalEdit` does).

**Breaking (tier-0 parity lists only):** `approve`, `decline` and `log --pending` serve both
tiers (`VerbSpec::tiers`) and P29 and P30 have tier-0 cases (`kit_blob_cases`), so a tier-0
list that defers them now fails (`StaleDeferral`, and the verbs are in
`Registry::for_tier(Tier::Blob)`): make them live. A text pack's list is unchanged.

**Store format 4** (reads 3; **no downgrade**): a new table `blob_approvals` keyed
`(doc, version)`, the same record as `approvals`. Why a table and not a tagged key in
`approvals`: every reader of `approvals` (text purge, the revert window, the pending cap,
`approvals(conn)`) keys it by text change position; a separate table means none of them can
take a blob version for a change, with no key-format change to the existing table. A
format-3 store is upgraded **in place** on open, in the open's transaction: the table is
created and every on-behalf blob version it holds gets its `Pending` approval (nobody could
settle them before), then it is stamped 4. A format-3 build (v0.4.x and older) refuses a
format-4 store with `StoreError::Newer { found: 4 }` instead of writing on-behalf blob
versions without approvals, leaving `blob_approvals` rows behind on a delete, or failing in
`rewrite_file` mid-purge. Keep the v0.4 build's backups if you might roll back.

**Behaviour:** an on-behalf blob write or restore records a pending approval (it still
applies at once); a purge counts the purged versions' approval records (kept, metadata only);
`delete_doc` / `delete_space` and `rewrite_file` cover the new table.

**Not built (proposed):** a pending cap for blob docs like text's `approvals_backlog` (text
caps pending **revert records**, which hold content; a blob version is kept anyway, nothing
is lost by a long pending list). Say if you want one.

**Hosts:** route `<host> doc approve|decline <version>` on a blob doc to
`BlobSession::set_approval` and print `restore_to` / `later` on a decline ("restore version N
to undo it; versions M… came after it and would be replaced too"); `log --pending
--on-behalf-of me` from `BlobSession::log` (`approval == Some(Pending)`), or the tab's list
from `approvals`; show `approval` in the blob log and blame; make the P29/P30 rows of your
tier-0 list live and run `parity::check_blob`.

### Amendment 10 (v0.4.0): tier flags, lease verbs, the blob log, conflict copies (K-A11Y-5)

**Breaking (Rust API):**
1. `VerbSpec` gains `tiers: &'static [Tier]` (a struct literal needs it).
2. `PersonAction` gains `Lease` (P32), `ReleaseLease` (P33), `BreakLease` (P34): every
   pack's exhaustive `match` must map them (a text pack: N/A), and a pack's list should keep
   to its tier (`PersonAction::for_tier(Tier::Text)`).
3. `Registry::kit()` is the **text** tier's verbs (`KIT_VERBS` now holds tier 0's too);
   `Registry::for_pack` refuses a pack verb named like any kit verb (`lease`...).
4. `parity::check` / `check_list` refuse a registry of another tier.

**Additive:** `parity::Tier`, `VerbSpec::applies_to`, `PersonAction::tiers`, `in_tier`,
`for_tier`, `Registry::for_tier`, `tier`; `Api::AcquireLease`, `RenewLease`, `ReleaseLease`,
`BreakLease`; the lease verbs in `KIT_VERBS`; the tier-0 harness (`check_blob`,
`check_blob_list`, `run_blob_case`, `kit_blob_cases`, `BlobCase`, `BlobStage`, `BlobPerson`,
`BlobAgent`, `BlobObservation`, `BlobVersionObs`, `BlobPresenceObs`, `BLOB_PATH`,
`BLOB_DOC`, `BLOB_MAX_BYTES`); `VerbValue::Bytes`, `BlobWrite`, `BlobStatus`, `BlobLog`,
`BlobBlame`, `Lease`, `Released`. `Host::conflict_dir` (defaulted); `BlobSession::log`,
`BlobLogEntry`, `BlobLogKind`; `BlobStatus::conflict_copies`, `ConflictCopy`;
`Config::conflict_copy_keep_ms`; `TickReport::conflict_copies_removed`.

**Behaviour:**
1. The conflict copy's name is `<name> (conflict <who> YYYY-MM-DD HHMMSS).<ext>` (was
   `(conflict YYYY-MM-DD HHMMSS from <who>)`).
2. With `Host::conflict_dir`, copies go to the host's folder, recorded relative to the
   file's folder (`BlobVersion::conflict_copy`, `AuditEvent::BlobConflict::copy` may now hold
   `../...`); without it, nothing changes.
3. `tick` deletes conflict copies after 30 days (see above).

**Store:** no change (format stays 3). A v0.3.x build reads a v0.4.0 record by joining it
to the file's folder, which resolves through the host's folder too. No new dependency.

**Hosts:** return your conflict folder from `Host::conflict_dir`; show
`BlobStatus::conflict_copies`; route `<host> doc lease [--ttl] | --renew | --release | --break`
to the lease calls (`lease --break` needs `Rights::owner`) and `<host> doc log` on a blob doc to
`BlobSession::log`; check your tier-0 command table against `Registry::for_tier(Tier::Blob)`
and run `parity::check_blob`.

### Amendment 9 (v0.3.1, patch): missing things exit 4

**Behaviour (exit codes only):** `tools::session_exit` maps every error that means "the thing
you named doesn't exist" to **4** (K-EXIT-1), wherever it comes from:
`Store(NoVersion | NoDoc | NoApproval | NoBlob | NoBlobSpace)` (were 6),
`NoApproval { seq }` (was 2), `Blob(NotFound)` (was 1). A store error inside the blob store,
`Blob(Store(e))`, now exits as `Store(e)` does (4 for a missing thing, 6 for a real store
failure; was 1). Real store failures (`Db`, `Corrupt`, `Newer`, `Locked`...) stay 6.
No API or store change.

**Hosts:** nothing to change if you print `session_exit`; a host matching exit 6 to say
"store unavailable" no longer says it for a version that doesn't exist.

### Amendment 8 (additive): blame (P22)

**Additive:** `Session::blame`, `Blame` (`render`, `porcelain`), `BlameLine`, `UNKNOWN_PID`;
`BlobSession::blame`, `BlobBlame`, `BlobAuthor`. Variants on `#[non_exhaustive]` enums:
`Action::Blame`, `parity::Api::Blame`; `parity::VerbValue::Blame`. The `blame` verb in
`KIT_VERBS` (history kind, needs read and history, serves P22) with kit parity cases.

**Store:** one new table, `change_clocks` (client, first clock → end clock, change), written
in each change's transaction, erased with the doc, copied by `rewrite_file`. **No format
bump** (store format stays 3; K-FMT-10): a v0.2.0 build reads and writes a v0.3.0 store
(its changes get no clock rows, so blame shows their text by client owner without a change
id); only its `rewrite_file` refuses the table it doesn't know (an error, nothing dropped).
Changes made before v0.3.0 have no clock rows either. No new dependency.

**Behaviour:** none changes for existing calls.

**Hosts:** route `k2 doc blame [-L a,b]` to `Session::blame` (history right) and print
`Blame::render` with your name lookup (or `porcelain` for scripts), fenced for agents as
`show`'s text is (it holds doc text); the tab's blame gutter
calls the same method for the lines in view; tier-0 files call `BlobSession::blame`.

### Amendment 7: erasure (purge, participant erasure, keyed hashes; store format 3)

**Additive:** `Session::purge`, `PurgeRequest`, `PurgeScope`, `PurgeOutcome`, `NotReached`;
`BlobSession::purge`, `BlobSession::conflict_copies`; `DocStore::rewrite_file`,
`RewriteReport`, `DocStore::scan_for`, `DocStore::path`, `store::scan_file`, `scan_bytes`,
`Finding`, `MIN_MATCH`, `PurgeCounts`, `Redaction`; `Config::own_write_ttl_ms`. Fields and
variants on `#[non_exhaustive]` types: `VersionInfo::redacted`, `BlobVersion::redacted`,
`Action::Purge`, `AuditEvent::Purged` (with `PurgedScope`), `StoreError::NoRandom`,
`PurgedVersion`.

**Store format 3** (reads 2; a format-2 build refuses it with `Newer`): `doc_keys` (a random
per-doc hash key, erased with the doc); log entries v3, versions v2, blob versions v2 (all
can hold redactions); sync hashes keyed. A format-2 doc's plain SHA-256 sync hash is read
once, when its file is next read, and replaced by a keyed one. No new dependency.
**Until then a format-2 store keeps a plain SHA-256 sync hash for each doc not reopened
with its file (or purged) since the upgrade.**

**Behaviour:**
1. Sync and own-write hashes are HMAC-SHA256 under the doc's key, never a plain content hash
   (K-PURGE-13). `FlushOutcome::hash` for a text doc is that keyed hash (still 64 hex
   characters), no longer the plain SHA-256: compare it only with the same doc's hashes.
2. Own-write hashes expire after `own_write_ttl_ms` (60 s): an editor saving a write of ours
   older than that is an outside edit (it was an echo, up to 8 writes back, before).
3. A version a purge redacted is never a `write --base` (`BaseExpired`).
4. `DocStore` transactions hold a read guard on the database (for the file swap of
   `rewrite_file`); no API change.
5. `session_exit`: `Store(CurrentVersion)` is 2, `Store(PurgedVersion)` is 4.
6. A `PurgeScope::Text` shorter than 8 bytes is refused (`BadEdit`, exit 2).

**Hosts:** map `k2 doc purge` (`--range`, `--version`, `--text` from stdin, `--and-remove`)
to `Session::purge` and print `not_reached` with "rotate the secret"; give `Rights::owner` to
the workspace owner and admins (Q21); choose pseudonyms for participant erasure; audit
`Purged`.

### Amendment 6 (additive): the parity framework and packs' own verbs (C-P)

**Additive:** `Pack::verbs()` (default `&[]`); `tools::PackVerb` (`PackVerb::edit`, `name`,
`usage`, `serves`, `exits`, `parse`), `tools::PackEdit`, `tools::is_kit_verb`,
`EditOutput::new`; `ToolError::unsupported`, `refused`, `usage` are now public. The
`parity` module: `PersonAction`; `Registry`, `RegistryError`, `VerbSpec`, `VerbKind`, `Api`,
`Source`, `KIT_VERBS`, `verb_form`; `ParityRow`, `RowStatus`; the harness `check`,
`check_list`, `run_case`, `render`, `kit_cases`, `Case`, `Stage`, `Person`, `Agent`,
`VerbOut`, `VerbValue`, `Step`, `Output`, `Observation` and its parts, `Failure`, `Side`,
`Report`, `edit_summary`, `undo_summary`, `rename_target`, `ACTOR`, `OBSERVER`, `DOC`,
`DOC_PATH`. No store change (format stays 2), no new dependency.

**Behaviour:** `run_tool` with a verb no kit tool has asks the pack before refusing it
(`Unsupported`, exit 2, as before when the pack has no such verb). A kit verb's name
always runs the kit's verb.

**Hosts:** route `<host> doc <verb>` for a pack's verbs to `run_tool` as for any tool; check
the host's command table against `Registry::for_pack` with `verb_form` (K-PAR-5); run the same
rows through the CLI against a headless daemon (K-PAR-4).

### Amendment 5 (additive): manager listing, delete, revert (C7b)

**Additive:** `SessionManager::loaded_keys`, `recheck_rights`, `delete_doc`, `delete_space`;
`DocStore::delete_doc(id, now_ms)`, `delete_space(space, now_ms)`. Variants (on
`#[non_exhaustive]` enums): `Need::AuthorOrOwner`; `AuditEvent::RecheckFailed`, `DocDeleted`;
`SessionError::Deleted` (exit 4). No store format change (format stays 2).

**Behaviour:**
1. A refused `revert` is `Forbidden { need: AuthorOrOwner }` (was `Need::Owner`), in the
   error and in `AuditEvent::Refused`. A host matching `Owner` for revert refusals switches
   to `AuthorOrOwner`; `Owner` now only means breaking a lease.
2. `revert` is never more lenient than undo: in C7a's edge case (two regions of one change came
   to touch, someone inserted at the shared edge, a deletion then crossed it) the right region
   now conflicts as `Taint`, as undo reports it, instead of merging (and, with `partial`,
   applying). Persisted carried bounds of pending records (one taint flag per region) now
   taint both edges when the index is rebuilt (conservative).

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
  `Result<Vec<u8>, DocError>`. A doc holding pending updates can't be encoded reliably;
  every engine encode now runs under `catch_unwind`, and a
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

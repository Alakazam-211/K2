# Changelog

## v0.9.1 (2026-10-28)

A patch: small asks from a host's integration and the Markdown demo, the 32-participant soak, and
the undo spec v1.9. **Additive**: no field added to a public struct, no new enum variant, no
store format change (still 5), no new dependency. `docs/SESSIONS.md`, amendment 15.

**The promise, corrected.** v0.9.0's wording said a conflicted undo applies what it cleanly can; a plain undo applies nothing (exit 1), and only `--partial` applies the clean parts. The promise, as it stands:

Undo, redo and revert only ever remove text that the change they reverse put there. Your undo never removes text someone else wrote, even if it is identical to yours, and someone else's undo never stands in the way of yours. Text put back by an undo or redo keeps its original author: blame shows who wrote it and who restored it, and later undos and reverts treat it as the original author's. If someone else has since changed the same lines, a plain undo applies nothing and reports the conflict; with --partial it applies the parts it cleanly can and reports the rest. It never overwrites their change. Nothing is lost: undone text stays in history and can be restored. This holds across restarts.

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
- **`approvals [--on-behalf-of me|P]`**: a host-neutral alias of `log --pending` (P30), both
  tiers, the same row and binding (`parity::ALIASES`, `VerbSpec::alias_of()`). The kit's
  parity harnesses run a case for each name. A pack's parity list may list it or leave it
  out: **packs need no change**.
- **`Pack::writes_bom(root, codec)`** (a provided method) and `codec::text_file::BOM`.

**Fixed**
- **Double BOM.** `cat` prints the file's bytes, BOM included; a `write` of those bytes
  (`write --base V` or `--base live`) put the BOM into the text, and the file got two. Now,
  when the doc's codec writes a BOM in front of that root, one leading U+FEFF is taken off a
  `write` body. Without a BOM in the file, a leading U+FEFF is text and stays. The default
  `writes_bom` (one text root: encode it empty and look for the BOM's bytes) covers a pack
  delegating to `TextFilePack` (Markdown) and a pack with its own BOM codec (a text pack):
  no pack change.
- **Blame lost "restored by" on a partly restored line** (Markdown demo, F1). Characters tied
  on the newest root change kept the line's first, usually not a restored one. Ties now go
  to a restored character (the most recently restored, then the first).
- **"Last edit" ranges grew over others' newer text** (Markdown demo, F2). A participant's
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

**Undo spec suite (SPEC-UNDO v1.9; no change to the kit's behaviour).** From the model check's fidelity review of v1.8: U10a (`hist.undo_own_only`) is read over the regions a step **applies** (a region skipped under `--partial` no longer lends its roots), in the oracle, the Quint model, the enumerator and the trace test's kit-side check; **I1 is checked after every step** of an undo or redo (the kit commits each step as its own change); O4a's wording says the live root is looked for in the text as the step merges it, window included. New mutant **(xi) `xi-no-o4a`** (O4 on roots, no O4a) fails I1 at depth 4 on TLC and the enumerator (2 × 1 + 1 undo + 1 revert), its mid-text witness at depth 5 in `tests/mutants.rs`; it joins every mutant loop in `tools/undo`. TLC = enumerator at all seven small bounds with v1.8's counts, every violation set unchanged; trace test 256 traces + restart twin, 0 mismatches. The Quint model's IR is 19.87 MB of Apalache's 20 MB JSON limit; `smoke.sh` prints it.

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

**The promise.** Undo, redo and revert only ever remove text that the change they reverse put there. Your undo never removes text someone else wrote, even if it is identical to yours, and someone else's undo never stands in the way of yours. Text put back by an undo or redo keeps its original author: blame shows who wrote it and who restored it, and later undos and reverts treat it as the original author's. If someone else has since changed the same lines, the undo applies what it cleanly can and reports the rest as a conflict; it never overwrites their change. Nothing is lost: undone text stays in history and can be restored. This holds across restarts. (corrected in v0.9.1)

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
- **F3: undo, redo and revert could delete another participant's identical element.** They
  found the merge's sides by re-diffing strings in the region's window; when someone's
  insertion at the region's start repeated its text (`b` and an inserted `b`), the diff put
  that insertion after the region's own element, and the undo deleted the other
  participant's. The text could even be wrong (`ab`, p1 replaces `a` with `b`, p2 types `b` in
  front, p1's undo gave `abb`; now `bab`), blame was swapped, and that participant's later undo
  conflicted. Now each side is an **exact hunk set**: ours is the inverse of the change's own
  hunk, theirs the later hunks that touched the region's window, at element identity
  (`session/foreign.rs`); `diff::hunks` stays only for `restore` and `write --base`.
- **Two conflicts the F3 fix alone would have brought** (they never shipped): with restored
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
  removes its own: the resulting text can differ (F3).
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
- The trace test: 256 traces and the 64-trace restart twin, 0 mismatches, with each live character's root, I1 and U10a by root compared on both sides; the new class hit 186 times.

**Conformance**
- `hist.undo_own_only` reads roots and covers F3, (a) and (b) with the expected outcomes.
- `sec.client_provenance_refused` declared: on the kit it holds by construction (no API takes
  provenance); native: awaiting (G2, the engine's).
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
  ITF replay 0 mismatches (G1 bound included); TLC's exact counts equal the enumerator's at
  four small bounds; every mutant fails in both at the same depth. The G1 bound itself is
  beyond exhaustive reach; its plan is in the doc.
- Two findings, reported, not fixed here: **F3**, undo can delete another participant's
  identical character when their insertion at the region's start repeats its text
  (`hist.undo_own_only` per element; the kit too); **F4**, U12's equivalence note fails in
  repeated text.

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
- **Parity.** New verbs: `claim`, `claim --renew` and `claim --release` serve P8 ("say
  what I'm working on"); `activity` serves P6. `--for-edit` is a flag on the read verbs. The
  kit has 61 cases now, and the harness compares the actor's claims (`ClaimObs`).

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
  - P8 is `live(&["cursor", "claim", "claim --renew", "claim --release"])` (the `intent`
    placeholder goes);
  - P6 adds `activity`.
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
- **Parity:** `replace` now serves P12 (find and replace) too, with three cases: one
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
- **Parity list:** add `replace` to your P12 row (`["sed -i", "replace"]`), or `check`
  fails with `VerbNotListed`. Hosts checking their command table against
  `Registry::kit()` see `replace`'s new usage line and exits.

**For packs:** a text pack's P12 row must list `replace` (above). `PersonAction` is
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
`concurrent_threads_exact_replace_and_typing_lose_nothing`. Parity: P12's three `replace`
cases on both paths.

## v0.5.0 (2026-10-27)

Approvals for on-behalf blob writes (K-T0-9), on its own because it changes the store
format and decides who may settle an approval. The API contract is `docs/SESSIONS.md`
(amendment 11). **Store format 4**; no new dependency.

**What's in it**
- **On-behalf blob writes get a pending approval.** A `write` or `restore` by an assistant
  whose participant has `on_behalf_of` applies at once (never held) and records
  `Approval { Pending, by: None, at_ms: None, author, on_behalf_of }` for that version in the
  same transaction: the text doc's record and rules (K-ID-10).
- **Only the named person settles it** (K-ID-11): `BlobSession::set_approval(conn, version,
  Approved | Declined)`. The agent, another person or an owner get
  `Forbidden { need: Approver }` and an `AuditEvent::Refused`; `Pending` is no target; once
  settled it can't flip (`BadApproval`), the same way again is a no-op; no approval is
  `NoApproval` (exit 4). `approval` and `approvals` with text's read rules.
- **Decline keeps history** (K-HIST-19 for tier 0): the result, `BlobApproval`, names the
  version to restore (`restore_to`) and the versions after it (`later`, which a restore also
  replaces: blobs don't merge). Nothing is restored automatically.
- **Surfaced:** `BlobLogEntry::approval`, `BlobBlame::approval` (current version); `who`
  shows the agent for the person, as before.
- **Parity:** `approve`, `decline`, `log --pending` on both tiers; tier-0 cases for P29 and
  P30 (31 cases over 16 live rows); the blob observation compares approval records.
- **Purge reaches it** (K-PURGE-3): a purged version's approval record is kept as metadata
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
- Tier-0 parity list: make P29 (`approve`, `decline`) and P30 (`log --pending`) live; a
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

Tier-0 kit asks from the first host. The API contract is `docs/SESSIONS.md` (amendment 10). **Store format
stays 3**, no store change; no new dependency.

**What's in it**
- **Per-verb tiers.** `parity::Tier { Text, Blob }`; `VerbSpec::tiers` (`applies_to`) says
  which docs a verb works on, so a host classifies its commands from data. Both tiers:
  `open`, `leave`, `cat`, `who`, `write`, `status`, `flush`, `log`, `show --version`,
  `restore`, `version --name`, `mv`, `blame`. `Registry::for_tier(tier)`;
  `Registry::for_pack` keeps working (the text tier plus the pack's verbs).
- **Lease verbs** (tier 0): `lease [--ttl MS]`, `lease --renew`, `lease --release`,
  `lease --break` (owner, as `BlobSession::break_lease`), and their person actions
  **P32 `Lease`, P33 `ReleaseLease`, P34 `BreakLease`**.
- **Tier-aware parity.** Each `PersonAction` has `tiers()`; a list needs rows only for its
  registry's tier, so the Markdown pack doesn't cover the leases. A tier-0 harness
  (`parity::check_blob`, `kit_blob_cases`: 27 cases over 14 live rows, 8 for the leases)
  runs the person path and the agent path on blob docs; `tests/parity_blob.rs` holds tier 0's
  list for any file, with negative controls.
- **`BlobSession::log`**: one `BlobLogEntry` per version, oldest first (who, when, kind:
  import / write / restore / external / conflict-kept / merged, `of`, `on_behalf_of`, name,
  `redacted`, size, the conflict copy). Same rights as `versions`; built from what's stored.
  It is the blob `log` verb.
- **Conflict copies per K-A11Y-5:** `<name> (conflict <who> YYYY-MM-DD HHMMSS).<ext>` (UTC),
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

**For packs:** map P32–P34 in your `parity_row` match (a text pack: N/A, "leases are tier
0's") and build your list with `PersonAction::for_tier(Tier::Text)`. `VerbSpec` literals
need `tiers`.

**Behaviour changes:** the conflict-copy name; copies in the host's folder when it gives
one; the 30-day cleanup on `tick`.

**Verified locally:** the full suite (298 tests, 1 ignored; new: `tests/parity_blob.rs` with 3,
7 in `tests/tier0.rs` incl. `conflict_copy_naming_cleanup`, `parity::registry::tiers_are_consistent`),
clippy `-D warnings`, `cargo fmt --check`.

## v0.3.1 (2026-10-27)

Patch: exit codes only (`docs/SESSIONS.md`, amendment 9). No API or store change; the store
format stays 3. No new dependency.

**Fixed:** `tools::session_exit` gave **6** (store unavailable) for a version that doesn't
exist (`StoreError::NoVersion`, e.g. a tier-0 `read_version`, `restore`, `name_version` or
`purge` of version 99). Everything that means "the thing you named doesn't exist" now exits
**4** (K-EXIT-1):

| Error | Was | Now |
|---|---|---|
| `Store(NoVersion \| NoDoc \| NoApproval \| NoBlob \| NoBlobSpace)` | 6 | 4 |
| `NoApproval { seq }` (approving a change made on nobody's behalf) | 2 | 4 |
| `Blob(NotFound)` | 1 | 4 |
| `Blob(Store(e))` | 1 | as `Store(e)`: 4 for a missing thing, 6 for a store failure |

Real store failures (`Db`, `Corrupt`, `Newer`, `Locked`, `NoRandom`...) stay 6;
`Store(CurrentVersion)` stays 2 and `Store(PurgedVersion)` 4.

**Tests:** `tools::error::missing_things_exit_4_and_store_failures_exit_6` (every variant of
the sweep, from the session, the store and the blob store), `tier0::a_missing_version_exits_4_not_6`.

**Verified locally:** the full suite (287 tests, 1 ignored), clippy `-D warnings`, `cargo fmt --check`.

## v0.3.0 (2026-10-27)

Blame (P22 live). The API contract is `docs/SESSIONS.md` ("Blame"; amendment 8, additive,
lists every change). **Store format stays 3**: one new table, so a v0.2.0 build still reads
and writes a v0.3.0 store (N−1, K-FMT-10). No new dependency.

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

**For packs:** mark P22 live: `ParityRow::live(P::Blame, &["blame"])`. A pack still listing it
deferred fails its parity test (`UnexpectedVerb`, `StaleDeferral`).

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

**Not in this release:** blame of a version (`--version`), blame through the wire (C5), char
blame export at the native migration (K-FMT-5); the WS sync protocol and JS client.

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

**Not in this release:** a `purged` event to subscribers and the `purged` frame (K-PURGE-6),
the orphaned-doc lifetime (K-PURGE-8), backups (K-PURGE-9), participant erasure on blob
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
- **Agent parity framework.** `PersonAction` P1–P31 (exhaustive), the `KIT_VERBS` registry,
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

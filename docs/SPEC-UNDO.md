# Spec: undo and redo (v1.5)

**v1.5 (2026-10-28):** U10 clarified (a clean region applies the merge's own hunks, never a
re-diff of the window); U22a added (revert's carried taint, exactly, with its superset
property); U19's notice settled (said once, withdrawn by the holder's next batch). The
trace test (§8) gains a repeated-text case class.

The kit's definition of undo, redo and revert. Its oracle of record is `spec_undo::model`
(`crates/spec-undo`); the kit's implementation is `crates/base/src/session/undo.rs`. Rule ids
(U\*, O\*, D\*) are stable; the oracle's doc comments and the trace test cite them.

One definition of undo, redo and revert for **every engine** behind the kit, written over
**plain values**: text strings and the hunks that changed them. **Undo is implemented once, in
the kit** (`session/undo.rs`) and runs unchanged over every engine. The engine's only part is
to report, for **every change it applies** (agent edits, restores, undo/redo/revert
applications and, later, concurrent client batches integrated by CRDT merge), the hunk
sequence in **host apply order at element-identity positions**: each inserted or deleted
element reported at its actual index, so that replaying the hunks over the previous text gives
the new text byte for byte *and* repeated text can't hide a wrong position. It also delivers
atomic multi-root apply, group IDs, stable change IDs and purge notices. No undo logic lives
in the engine. The kit owns this spec; a change to it is reviewed by the engine's owners.

**What "one semantics on every engine" means.** Undo is one function of the applied-hunk
stream. Engines agree wherever their merges agree, and differ exactly where the Fugue paper's
Def. 4 says YATA (Yrs) is wrong: YATA and FugueMax order concurrent same-spot inserts
differently by design, so under concurrency their texts, and therefore their undo outcomes,
can differ there. The test requires **equality on serialised traces** and, under concurrency,
**equality wherever the merged texts agree** (the generator is biased so that ≥ 90% of
concurrency traces agree), with every other difference classified by a Def. 4 checker.
Demanding exact equality would demand YATA's interleaving.

Notation: a *hunk* is `(start, end, insert)` on UTF-8 byte offsets (`text/diff.rs`).
`merge3(base, ours, theirs)` is the kit's three-way merge (`text/merge.rs`);
`merge3_window(base, ours, theirs, at_end)` is the same merge on a slice of a doc, where
`at_end` says whether the slice ends at the doc's end (§4).

**Scope: which paths use this spec.** Only **`undo`, `redo`, `revert` and `write --base`** go
through `merge3` and this spec. **An editor client's keystroke stream stays a CRDT branch
merge**: concurrent typing never conflicts, and both end-of-text appends land in the engine's
order.

**The diff is deterministic** (D1–D3, §4a): `merge3` is built on `diff::hunks`, so the spec
pins that function.

## 1. Batches and stacks

| # | Rule |
|---|---|
| U1 | A **batch** is one change by one participant: one `edit` / `edit_with` / `write --base` / `replace_text` / `revert` call, one verb (CLI), one `restore`, or one person **undo group** (U3). A batch may touch several roots; it is undone and redone as one unit. Every batch has a **stable change ID** that outlives the session. |
| U2 | Every batch is recorded on the **undo stack of its pid** (not its connection): one pid's tabs, devices and terminal share one stack. *(Open question: whether a per-surface stack is better; this spec changes if it is.)* An assistant agent has one stack for all the people it acts for; its plain `undo` undoes its newest batch whoever it was for, and `undo --for <person>` restricts it to batches made on that person's behalf. |
| U3 | **Person grouping (editor clients).** The **client proposes** groups: consecutive local edits from one connection form one group until any of (a) **500 ms** with no edit; (b) a non-contiguous edit; (c) a **word boundary** (a space, tab or punctuation typed after a word character); (d) a newline; (e) a command that isn't typing (paste, cut, format, find-replace, drag); (f) an undo or redo. **(g)** The **host-side kit splits** a group where another participant's change lands inside or touching the group's span **in host apply order** (the client can't know this; its keystrokes may integrate after foreign ops it hasn't seen). The engine records client group IDs and timestamps and never groups on its own. Agents never group: one verb = one batch. |
| U4 | **Not undoable by anyone:** `import` and `external` changes (no participant stack). They are reversed with `restore --version` (a version is cut before every external edit). A raw-tool write attributed to a merely *probable* agent is **not** put on that agent's stack. `revert` (U22) can reverse them where the rights allow. |
| U5 | `restore` and `revert` are the caller's batches and are undoable by them. Undo and redo are themselves recorded as the undo/redo stacks, not as new undoable batches: undoing an undo is `redo`. |
| U6 | **Bounds.** Per pid per doc: at most **100 batches** and **4 MiB** of retained region text; the oldest batches are dropped first. Stacks live in the session: they **survive leave and rejoin**, and are **lost on session unload and host restart** (every engine alike). `revert` (U22) doesn't depend on them. |
| U7 | Any new batch by a pid **clears that pid's redo stack**. Other participants' edits don't. |

## 2. What undo computes (text)

For a batch B on root r with text `S_before` before it and `S_after` after it, and the live
text `L` now:

| # | Rule |
|---|---|
| U8 | **Regions are hunks.** Each **canonical hunk** of B on r, as the applied stream reports it (D2: sorted, non-overlapping, coalesced when touching), is **one region** `[s, e)` with `R_before = S_before[s..e_before]` and `R_after = S_after[s..e]`. A batch has as many regions as hunks; a `sed -i` touching lines 3 and 300 is two regions. Each region's bounds are carried through **every later change to r, by anyone**, like a caret: a later hunk entirely before the region shifts it; an insertion **exactly at the region's start or end is pulled inside it**; a later hunk entirely after leaves it alone. **Shared edges within one step:** canonical hunks don't touch when recorded, but later deletes by others can make two regions of the same step touch; a foreign insertion exactly at their shared edge is pulled into the **left** region only, never both. |
| U9 | **Taint.** If any later hunk deletes across a region edge (it starts strictly before an edge and ends strictly after it), the region is **tainted**. A tainted region always conflicts (U11). |
| U10 | **Undo of one region** = `merge3_window(base = R_after, ours = R_before, theirs = L[s'..e'], at_end)`, where `[s', e')` are the carried bounds and **`at_end` is true only if `e'` equals the live doc's length and the whole doc was non-empty in the merge's base**. "Non-empty" is judged on the merge's base, the whole doc as it stood when the undone change was made, not on the live doc: `at_end = e' == live.len() && !merge_base_doc_was_empty`. Case: B deletes the whole doc, then someone appends; undoing B's delete is judged with base = "" and is a conflict, consistently with U12's whole-text merge3; the region's own base `R_after` may be empty, e.g. after B deleted the last line, and that must not block O5. **Within one step, regions apply back to front** (highest `s'` first), each application carrying the bounds of the regions not yet applied. So the end-of-text rule (O5) applies only at the doc's real end, never at the end of a region in mid-text. A clean merge gives the inverse hunks (offset by `s'`); otherwise the region conflicts. **The step applies exactly those hunks** (v1.5): the merge's own, ours' changes placed in theirs' coordinates, **never a fresh diff of the window**. Hunks name elements (element identity): in repeated text a diff of the window can name another participant's identical characters, giving the same text with the wrong elements, so that participant's own undo later finds nothing to remove. The same holds for redo and revert, and for `write --base`'s clean merge. |
| U11 | **Conflict region (what is reported).** For each conflicting region: `{root, live byte range [s', e'), live lines (first, last) 1-based, kind: overlap / taint}`. One entry per conflicting region (= hunk), never sub-ranges of a hunk. Every engine reports exactly this wherever their merged texts agree. |
| U12 | **Equivalence note.** Outside the taint case, U10 equals whole-text `merge3` of the same inverse on L, because changes outside a region can't overlap it (§4), and because U10 passes `at_end` exactly when the region ends at the doc's end **and the whole doc's base was non-empty**, which is exactly when whole-text O5 can fire. **Worked case:** B deletes the last line `x\n` of `a\nx\n`; someone appends `y\n`; undo calls `merge3_window("", "x\n", "y\n", at_end = true)` (the doc's base `a\n` is non-empty) and lands `a\ny\nx\n`, theirs first, exactly as whole-text merge3 does. The region form is the definition; it costs memory in proportion to the edit, not the doc. |

## 3. Modes

| # | Rule |
|---|---|
| U13 | **`undo [--steps N]`** (default N = 1) is **all-or-nothing**. Plan the newest N batches newest-first against L, applying each planned step to a scratch copy and carrying the remaining regions through it (U8). If **any** region of any step conflicts, apply **nothing**, report every conflicting region of the **first** conflicting step, and exit 1 (`undo_conflict`). Otherwise apply all N steps as **one log append**, recorded as N redo entries. |
| U14 | **`undo --partial [--steps N]`** applies, per step, every region that merges cleanly and skips and reports every region that conflicts; exit 0 if nothing was skipped, 1 if anything was. A step with at least one applied region is consumed (moved to redo with its skipped regions marked). **A step whose every region conflicts is not consumed:** it stays on top of the stack, nothing is applied, and the reply says so; with N > 1 planning stops there. **Escape:** `undo --drop` removes the blocked top step without applying it (reported; it does not go to redo); in an editor, a second Ctrl+Z on the same blocked step offers "skip this change", which is `--drop`. |
| U15 | **Identical reports.** For the same batch and the same L, the conflicting regions reported by `undo` (U13) and skipped by `undo --partial` (U14) are the same set. With N > 1 this holds for the **first** conflicting step. |
| U16 | **A person's Ctrl+Z in an editor is `undo --partial` with N = 1**, subject to U21. Ctrl+Shift+Z is `redo` with N = 1. |
| U17 | **`redo [--steps N]`** reapplies, newest-undone first, **exactly the regions that the undo applied** (skipped regions are not redone). Redo of a region is `merge3_window(base = R_before', ours = R_after', theirs = live, at_end)`, where **`R_before'` is the region's live text just after the undo applied it and `R_after'` its text just before**, both carried since (U8). So redo inverts what the undo actually did. Redo is all-or-nothing by default and has `--partial` with the rules of U13–U15. Because U14 never consumes an all-conflicting step, a redo entry always has at least one region. **Ordering note:** undoing a delete of the last line after someone else appended at the doc's end re-inserts the line **after** their append (O5: theirs first), **cleanly** (O5's non-empty base is the whole doc's, not the region's). |
| U18 | Nothing to undo or redo: exit 0 with `nothing to undo`. After a host restart the reply says the history was lost with the session (U6) and points at `revert`. |
| U18a | **Blame after undo:** undo and redo re-insert text as ordinary hunks, so restored characters are **new content attributed to the undoer**, on every engine; cursors or anchors that pointed into the deleted text don't come back. |
| U21 | **Ctrl+Z with unacked or offline edits.** Ctrl+Z targets **acked** batches only. An unsent local batch is cancelled on the client before anything is sent; a sent but unacked one is waited for. The undo request carries the client's last group ID, and the host applies it only after integrating that client's ops up to it. **Offline**, Ctrl+Z only drops the client's own unacked local ops; past them it is disabled with a hint until reconnect. **Re-based ops:** **the host is the only re-base owner**: a client never re-bases; the host re-bases a dropped client's ops against the recorded view and the client adopts the typed `rebased{epoch,…}` frame. Re-based ops **keep their client group IDs**, and an undo request waiting on a group that is being re-based waits for the host's `rebased` frame. |
| U22 | **`revert <change>`.** Reverses **one named change** (its stable change ID) using **U8–U11 on its persisted revert record**: the change's hunks, deleted text, author and `on_behalf_of`, and its regions' **carried bounds and taint flag**. **No per-keystroke work over all records:** a record is written once, at the change; its bounds are **relative positions** (Yrs `StickyIndex`; element IDs on an engine that has them) or live in an **offset tree with O(log R) work per apply** (R = records held) with batched persistence; taint is marked **lazily** from an interval index of later deletions and resolved when the record is used. So log compaction of later changes can't strand the carry, and a busy doc's keystroke cost doesn't grow with R (benchmark target: ≤ 20 µs p95 per apply at R = 10^5). Revert works days later and after host restarts, independent of stacks, **inside the revert window**: all pending on-behalf changes, plus changes from the last 30 days or the last 10,000 changes per doc (whichever reaches further), ≤ 64 MiB per doc. Outside it, revert returns **`revert_too_old`**, **exit 8** (never a retry-after, unlike conflicts' exit 1) pointing at `restore --version`. **Pending on-behalf records are capped at 1,000 records or 16 MiB per doc**; past it new on-behalf edits are refused (`held: approvals_backlog`, exit 2) so revertability is never silently lost. A region overlapping a **redacted** span is a conflict of kind **`erased`**. On an engine whose positions derive from element IDs, reach may differ; the trace test compares only inside the shared retention. `revert --partial` follows U14 (except that "not consumed" doesn't apply: a fully conflicting revert applies nothing and exits 1). Allowed for the change's author, its `on_behalf_of` person and the doc's owner; recorded as a new batch of the reverter. While an on-behalf change is `pending`, its revert record is kept whatever the window says. Revert records are purge-reachable (U19). |
| U22a | **Revert's carried taint (v1.5), exactly.** A revert record's region edges are carried each on its own (no shared-edge clamp while carrying; the clamp of U8 applies when the record is used), and each edge has its own taint flag (U9 per edge). When the record is used, its regions are taken in record order (by root, then start); a region whose carried start is at or before the previous region's carried end on the same root **inherits that end's taint**; then the shared-edge clamp (U8) gives the bounds, and U10–U11 apply. A carry persisted when the revert window moves (records kept as pending on-behalf, older than the move) keeps **one taint flag per region**; reloaded (after a host restart), a region tainted then is tainted at both edges. This is the conservative price of U22's O(log R) carry. **Property** (tested): on the same state, revert's conflicting regions are a superset of undo's for the same change, with the same bounds, and every region both apply gives the same text. |

## 4. Overlap rules (merge3, shared with `write --base`)

| # | Rule |
|---|---|
| O1 | Two non-empty ranges that intersect conflict. |
| O2 | An insertion strictly inside the other side's non-empty range conflicts. |
| O3 | Touching at a boundary does not conflict: an insertion at the start of the other side's range lands before it; one at its end lands after it. |
| O4 | An identical change on both sides is taken once. |
| O5 | Two different insertions at the **same point** conflict, **except at the end of the live doc when the whole doc was non-empty before the merge**, where both land: **theirs first, then ours** (theirs is already live). This makes "two agents append to a log" non-conflicting. **Not** at the end of a window that isn't the doc's end (`merge3_window(.., at_end = false)`), and **not** when the **whole doc** was empty before the merge (the doc's base, never a region's): two `write --base` calls on a new, empty doc conflict (CT `merge.empty_base_same_point_conflicts`). merge3 is therefore **symmetric up to the order of two different end-of-text appends** (`clean_merges_are_symmetric_up_to_eof_order`), and O5 never changes a merge that was already clean without it (`eof_rule_only_adds_clean_merges`). |

## 4a. The diff is deterministic

| # | Rule |
|---|---|
| D1 | The edit pipeline is **`align_insertions(diff::hunks(old, new))`** (pure line insertions are anchored at line starts), and every part of it is a **pure function of its inputs**. `diff::hunks(old, new)` is a **pure function of its inputs**: no wall-clock deadlines, no machine-load dependence. The search is bounded by a **counted work budget** (`text/myers.rs`: 2^22 units plus 16 per input byte per call), the same on every machine; past it, the result is a coarser but still correct diff, identically everywhere. *(Root cause recorded: an earlier merge3 identity failure came from wall-clock deadlines.)* |
| D2 | **Canonical hunks:** sorted, non-overlapping, never empty, coalesced when touching, on character boundaries. For repeated substrings the **tie-break is the implementation's, pinned by golden tests** (`tie_break_is_pinned`): the common prefix is trimmed first, so `"aa"→"aaa"` inserts at position 2, which keeps an appended duplicate last line an end-of-text append for O5. **`spec_undo::model` calls `diff::hunks`; it doesn't reimplement it**, so the diff is a shared dependency whose correctness is carried by the diff's own tests. Changing the tie-break is a spec change. |
| D3 | `merge3` inherits D1–D2, so undo, redo, revert and `write --base` give the same result on every machine. The trace test (§8) runs each trace twice under different CPU load and on aarch64 and x86_64 and requires byte-identical outcomes. |

## 5. Erasure

| # | Rule |
|---|---|
| U19 | **A purge clears every undo and redo stack for that doc, and redacts the purged content in revert records.** Holders see `undo history cleared by purge` **once**, on their next undo or redo that finds nothing; a new batch by the holder withdraws it (from then on, plain `nothing to undo`) (v1.5). This is the simplest rule that guarantees no purged content, not even part of a secret typed in two batches, can return through undo; a purge is a rare owner action. Stacks keep **no content hash** of anything. `revert` (U22) still works for every other change: it reads the log, where purged content is redacted, so a revert can never restore it either. |
| U20 | **Participant erasure** keeps stacks keyed by the pseudonymised pid; the erased person's own stack is dropped. |

## 6. Non-text kinds (definition level)

The kit's rule for every kind: **undo restores your own previous state only where nobody has
changed it since. It never overwrites another participant's later change**; that case is a
conflict (`--partial` skips it). This deliberately differs from many apps' register undo
(Stewen & Kleppmann, PaPoC 2024, §2): an agent's undo must not clobber a person. Each element
is a region.

| Kind | A batch records | Undo of one element | Conflict when |
|---|---|---|---|
| Map (per key) | key, value before, value after | set the key back to "before" (or delete it if it was absent) | the key's live value ≠ "after" |
| List | inserted item IDs; deleted items with neighbours; set-item before/after | delete own inserted items still present; re-insert own deleted items next to their surviving left neighbour; set items back | an own inserted item was changed by someone else; a re-insert's left and right neighbours are both gone; a set item ≠ "after" |
| MovableList | the moved item, position before, position after | move it back to "before" | **someone else moved the item since** (its position register was written after this batch), or the item is gone. Index changes from others' inserts or deletes elsewhere are not conflicts |
| Tree | the node, parent and index before and after | move back | **someone else moved the node since**, the old parent is gone, or the move back would create a cycle. Sibling index changes from others' edits are not conflicts |
| Table | per its parts: rows and columns (MovableList), cells (Map) | per part | per part |
| Blob pointer | pointer before and after | set it back | the live pointer ≠ "after" |

The modes (§3), identical reports (U15), redo (U17), revert (U22) and purge (§5) apply
unchanged, with "region" read as "element".

## 7. Reference algorithm

**The kit's implementation:** `session/undo.rs`, used for every engine: `Region::of_change`,
`Region::transform` (edge insertions pulled inside: start `Bias::Before`, end `Bias::After`; a
deletion across an edge taints), `Region::revert` (`merge3_window(after, before,
live[s..e], at_end)`) and `plan` (newest first, scratch copy, later regions carried).

**What the implementation must have** (each item is a rule above):
1. One region per canonical hunk (U8).
2. One conflict per region in live coordinates, not merge3's sub-conflicts offset by the
   region start (U11).
3. `--partial` (U14), stays-on-top, redo after partial (U17): a partial planner that skips
   conflicting regions, records them on the redo entry, and leaves an all-conflicting step on
   the stack.
4. The end-of-text rule (O5) with `at_end` and a non-empty whole-doc base: `merge3_window`'s
   **caller** folds the whole-doc empty-base flag into `at_end` (the window function itself
   must not test the region's base). Tests: the empty-base conflict and
   `hist.undo_last_line_after_foreign_append`.
5. Bounds (U6): 4 MiB per pid per doc.
6. Purge (U19): a session method that clears every stack for the doc.
7. Revert (U22) and `undo --for` (U2): persisted revert records with carried bounds as
   relative positions or an O(log R) offset tree and lazy taint, the revert window and
   `revert_too_old` (exit 8); the pending on-behalf cap; records kept for pending on-behalf
   changes; purge redacts them.
8. Grouping (U3) and Ctrl+Z with unacked ops (U21): editor clients and the host-side split
   (later).
9. A deterministic diff (D1–D3).
10. The escape from a blocked step (U14): `undo --drop`.
11. Within-step order and shared edges (U8, U10): back-to-front application with carried
    bounds; a shared-edge insertion goes to the left region only.

## 8. Trace-equivalence test plan (the proof)

| Part | What |
|---|---|
| **Oracle** | `spec_undo::model`: a pure-Rust implementation of U1–U22 and O1–O5 over `String`s and hunks (no engine), written from this document. It **calls the kit's own pipeline, `align_insertions(diff::hunks(..))`** (D1–D2), never a second diff implementation. It is the **oracle of record**, and it implements the `at_end` window flag independently of `merge3_window`'s caller logic. |
| **Generator** | Seeded traces of 2–5 pids × 1–3 connections each × 20–200 steps. Each step is one of: edit (insert, delete or replace at random spots, including end-of-text appends, edge-touching inserts, deletes across others' regions, and **multi-hunk batches** such as a `sed -i` touching distant lines), `undo`, `undo --partial`, `redo`, `redo --partial`, `--steps N`, `revert <change>`, `undo --for`, leave/rejoin, host restart (stacks lost; revert still works), purge (of a random 32-byte secret), and `write --base` including **two writes to an empty doc**; **undo of a last-line delete after a foreign append**; **two regions of one step made to touch by a foreign delete**; **revert outside the window**; `undo --drop`. The alphabet includes multi-byte characters and `\n`. |
| **Run** | Replay each trace through (a) the spec model, (b) the kit sessions on Yrs, (c) the kit sessions on another engine (later). After **every** step compare: the text of every root; the outcome (applied / conflict / partial / stays-on-top / nothing, with its notice); the reported conflict regions (U11) as an exact set; and each pid's stack depths. Every revert also checks U22a's property, and reports U22a's cost: how often revert conflicts on a region where undo of the same change on the same state would apply. |
| **Concurrency class** | Later: concurrent client batches integrated between undo steps, with the model consuming the host's applied hunks. It runs on every engine (Yrs integrating client batches in test builds only, behind `untrusted-wire`): **equal wherever the merged texts agree** (generator biased to ≥ 90% agreement) and every other difference **classified by a Def. 4 checker**. It also covers Ctrl+Z with unacked ops (U21) and undo while another client has unacked ops. |
| **Positions** | A property test on the low-entropy alphabet `{a, \n}` compares each engine's reported hunk positions with a reference model's element positions; a **mutant adapter that derives hunks by diffing text must fail it**. |
| **Volume** | ≥ 10^5 traces nightly (256 in CI) for (a) vs (b), and later for (a) vs (c), with ≥ 10^4 traces per case class (append-at-end, insert inside another's range, multi-hunk partial and stays-on-top, grouping with the (g) split, redo after partial, revert after restart, undo across purge, multi-connection pid, empty-base double write, last-line-delete-after-append, **whole-doc-delete-then-append**, touching regions in one step, revert beyond compaction and outside the window, **repeated text** (an alphabet of `{a, b, \n}`, runs like `aaaa` and identical lines, so that a clean region's merge hunks differ from a re-diff of its window: element identity, U10), and the concurrency class; per-kind classes later). **0 mismatches** (concurrency class: as above). Determinism (D3): each trace also runs twice, under load and on aarch64 + x86_64. |
| **Model check** | **`MC text-undo` is the kit's**: a Quint model of this spec, replayed against `spec_undo::model` via ITF traces at small bounds (3 pids × 3 events each, plus 2 undos and 1 revert). The engine holds no undo logic, so there is no engine refinement check. |
| **Mutants** | The test must fail on: (i) a whole-text merge3 with no taint rule; (ii) an end-of-text rule without O5; (iii) partial reporting a different set than all-or-nothing; (iv) redo re-trying skipped regions; (v) a diff with a wall-clock deadline (D1), run under a CPU-load harness; **(vi) O5 applied at every window end**; **(vii) O5 conditioned on the region's base instead of the whole doc's**. Each mutant is kept with its failing seed. |
| **CT names** | `hist.undo_own_only`, `hist.undo_atomic`, `hist.undo_partial`, `hist.undo_partial_stays_on_top`, `hist.undo_reports_identical`, `hist.redo`, `hist.redo_after_partial`, `hist.undo_eof_append`, `hist.undo_taint`, `hist.undo_purge_clears`, `hist.undo_not_external`, `hist.revert_declined_after_restart`, `hist.revert_too_old`, `hist.revert_over_erased`, `hist.undo_drop_blocked`, `hist.undo_last_line_after_foreign_append`, `hist.undo_touching_regions_one_step`, `merge.empty_base_same_point_conflicts`, `hist.undo_group_person` (editor clients only), `hist.ctrl_z_unacked` (editor clients only). |
| **`write --base`** | The same model replays `write --base` and `write --base --partial`, since they share merge3 and O1–O5. **Every engine keeps merge3 for `write --base`** (not a CRDT branch merge), so agents see one semantics on every engine. A CRDT branch merge is a separate, explicit verb. |

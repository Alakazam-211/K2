# Conformance coverage

Generated from `crates/conformance/src/matrix.rs`; a test keeps this file equal to it (regenerate with `CONFORMANCE_BLESS=1 cargo test -p k2-mpc-conformance --test matrix`).

A rule runs on every engine that has the capabilities it needs. A rule that runs on no engine fails the suite unless it is listed as awaiting, with a reason and a target. A pinned deviation is asserted to still happen.

| Rule | Requirement | Needs | kit Doc (yrs) | mpce-model (FugueMax) |
|---|---|---|---|---|
| `text.converge` | E-CORE-1, E-TEXT-2 | Text | pass | pass |
| `text.no_interleave_forward` | E-TEXT-3 | Text | pass | pass |
| `text.no_interleave_backward` | E-TEXT-1, K-CT-5 | Text | pass | pass |
| `text.no_interleave_backward_multi` | E-TEXT-1, K-CT-5(a) | Text | pass | pass |
| `text.keep_concurrent_insert` | E-TEXT-6 | Text | pass | pass |
| `text.delete_once` | E-TEXT-7 | Text | pass | pass |
| `text.edit_atomic` | E-CORE-10 | Text | pass | pass |
| `text.yata_separating_case` | E-TEXT-1, K-CT-5(b) | Text | **deviation** (pinned) | pass |
| `text.yata_paper_a19` | K-CT-5(e) | Text | **deviation** (pinned) | pass |
| `text.scalar_safe` | E-TEXT-9 | Text | pass | pass |
| `text.edit_atomic_split` | K-CT-3 | Events | pass | pass |
| `core.idempotent` | E-CORE-2 | Events | pass | pass |
| `core.causal` | E-CORE-3 | Events | **deviation** (pinned) | pass |
| `core.no_equivocation` | E-CORE-4 | Events | **deviation** (pinned) | pass |
| `text.blame_char` | E-TEXT-11, undo spec U18a, U23 | Blame | pass | pass |
| `text.cursor_stable` | E-TEXT-10 | Anchors | pass | pass |
| `hist.undo_atomic` | E-HIST-6, K-CT-4 | Undo | **deviation** (pinned) | never |
| `hist.undo_partial` | K-HIST-13, K-CT-4 | Undo | pass | never |
| `hist.redo` | E-HIST-6, K-HIST-2 | Undo | pass | never |
| `hist.undo_own_only` | E-HIST-5, undo spec U10, U10a (by root, v1.8), U18a, U23 | Undo + Blame | pass | never |
| `hist.undo_own_only_tokens` | undo spec v1.10, D1 (authored ranges, by token) | Undo + Blame | pass | never |
| `hist.version_stable` | E-HIST-1 | Versions | pass | pass |
| `hist.restore_forward` | E-HIST-2, K-HIST-8 | Versions | pass | pass |
| `hist.restore_minimal` | K-HIST-8, undo spec D1 | Versions | pass | pass |
| `hist.blame_grain` | E-HIST-4 | BlameAt | never | pass |
| `hist.branch_merge` | E-HIST-8 | Branches | pass | pass |
| `hist.merge_any_base` | E-HIST-9 | BranchAt | never | pass |
| `sec.duplicate_ranges` | E-SEC-6 | Wire | awaiting: native decoder, G2 | never |
| `sec.base_too_old` | E-SEC-12 | Wire | awaiting: native decoder, G2 | never |
| `sec.client_provenance_refused` | undo spec U23 (v1.8) | Wire | awaiting: native decoder, G2 (the engine's) | never |
| `erase.purge` | E-ERASE-1, K-PURGE-5 | Purge + Blame + Anchors | pass | pass |
| `erase.participant` | E-ERASE-5, K-PURGE-7 | Erasure | pass | never |
| `erase.no_readmit` | E-ERASE-6 | Purge | pass | pass |
| `erase.versions` | E-ERASE-7, K-PURGE-4 | HistoryPurge | pass | pass |
| `erase.span_includes_tombstones` | E-ERASE-1 | SpanIds | never | pass |
| `mlist.converge` | E-CORE-1, E-MLIST-1, E-MLIST-2 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.move_no_duplicate` | E-MLIST-1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.delete_wins` | E-MLIST-2, engine answer E1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.insert_beside_moved` | E-MLIST-2 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.move_and_set` | E-MLIST-1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.move_lww` | E-MLIST-1 | MList | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.block_contiguous` | E-LIST-1, engine answer E9 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.chain_one_run` | engine answers E8, E9 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `mlist.yata_separating_case` | E-LIST-1 (as E-TEXT-1) | MList | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `map.lww` | E-MAP-1 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `map.delete_vs_set` | E-MAP-1, engine answer E5 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `map.steady_bounded` | E-MAP-3 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.cell_kept` | E-TABLE-1, engine answer E3 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.cell_vs_row_delete` | E-TABLE-2, K-CSV-4, engine answer E3 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.cell_vs_col_delete` | E-TABLE-2 (extended), K-CSV-4 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.sort_atomic` | K-CSV-3, K-PAR-8 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.block_contiguous` | E-TABLE-1, engine answer E9 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.cell_lww` | E-TABLE-2, E-MAP-1 | Table | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.row_identity` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.sort_vs_cell` | K-CSV-4 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.row_move_keeps_cells` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.col_move_keeps_cells` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.col_move_no_duplicate` | K-CSV-4, E-MLIST-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.delete_wins_restorable` | E-MLIST-2, K-CSV-4 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.insert_beside_moved_row` | E-MLIST-2 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `table.ragged_render` | CSV contract §1.5, K-CSV-2 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `stream.merged_replays` | CSV seam design §4.2, A5 | Merged + MList + Map | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `stream.merged_matches_local` | CSV seam design §4.2, engine answer E4 | Merged + MList + Map | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models |
| `stream.merged_writes` | engine answer E4 | Merged + MList + Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models |

## Engines

- **kit Doc (yrs)**: Text, Events, Blame, Anchors, Undo, Versions, HistoryPurge, Erasure, Branches, Purge, MList, Map, Table, Merged.
- **mpce-model (FugueMax)**: Text, Events, Blame, Anchors, Versions, HistoryPurge, Branches, BranchAt, Purge, BlameAt, SpanIds.

## Pinned deviations

- `text.yata_separating_case` on kit Doc (yrs): yrs (YATA) orders the separating script [pfae], splitting one writer's run `fe`; FugueMax gives [apfe]. Wire-format bound: fixed only by a FugueMax-class engine.
- `text.yata_paper_a19` on kit Doc (yrs): yrs (YATA) orders the Fugue paper's A.1.9 example `axb` (as the paper reports for Yjs); FugueMax gives `xab`. Wire-format bound.
- `core.causal` on kit Doc (yrs): the kit checks an update's dependencies per client (each client's clocks must continue the doc's), which refuses a skipped own predecessor with MissingDeps; an event whose parent is another client's event the doc lacks passes that check, yrs holds it as pending, and the kit then poisons the doc (Pending: reload from the store) instead of refusing it with nothing changed. The native engine checks every parent.
- `core.no_equivocation` on kit Doc (yrs): yrs identifies content by (client, clock) only: a known ID with different content is skipped as already known, silently (the first content stays), not refused and reported. The native engine keeps a keyed digest per run (E-CORE-4).
- `hist.undo_atomic` on kit Doc (yrs): a session applies `undo --steps N` (and redo) as N engine batches, N versions and N log entries, committed together in one store transaction under the session lock; the rule asks one batch (one log append). A single step over several roots is one batch attributed to the undoer, as asserted.
- `mlist.move_lww` on kit Doc (yrs): a move writes the element's position register, a yrs map entry, and yrs orders concurrent writes of one map entry by client ID (YATA's tie-break), not by (lamport, agent): a move made after more of its writer's own changes can lose. Wire-format bound.
- `mlist.yata_separating_case` on kit Doc (yrs): the list's slots are a yrs array, so YATA orders them: the separating script gives [lb p f a e rb], splitting one writer's run `f e`; FugueMax gives [lb a p f e rb] (as text.yata_separating_case). Wire-format bound.
- `map.lww` on kit Doc (yrs): yrs orders concurrent sets of one map key by client ID (YATA's tie-break), not by (lamport, agent). Wire-format bound.
- `map.delete_vs_set` on kit Doc (yrs): a yrs map delete removes only the entry it saw, so a concurrent set always survives it, whatever (lamport, agent) say. (A list element's delete is a set of its delete flag, and wins, as E-MLIST-2 asks.)
- `map.steady_bounded` on kit Doc (yrs): each overwrite of a map key leaves a content-free item stub (content is garbage-collected, K-PURGE-11), in the live doc and in a doc loaded from its state (the stored v2 state compresses a regular run of them). Bounded on the interim by the CSV caps.
- `table.cell_lww` on kit Doc (yrs): cells are a kit Map, so concurrent sets of one cell are ordered by client ID (map.lww).
- `stream.merged_writes` on kit Doc (yrs): a merged update's stream (`Doc::applied_since`, a keyed diff) names each write by the facet's winner; on yrs a restore removes the delete flag and a map delete removes the entry, and yrs keeps no identity for a removal, so those two ops carry no write (`None`), as `winner(Existence)` of a live element is `None`. The native engine reports every merged op's event ID (E4).

## Awaiting

- `sec.duplicate_ranges` on kit Doc (yrs): decoding untrusted update bytes is the native decoder's; this suite feeds engines only bytes they made. Target: native decoder, G2.
- `sec.base_too_old` on kit Doc (yrs): decoding untrusted update bytes is the native decoder's; this suite feeds engines only bytes they made. Target: native decoder, G2.
- `sec.client_provenance_refused` on kit Doc (yrs): holds by construction in the kit: provenance (undo spec U23) is stamped only by the session's own undo, redo and revert (`Session::apply_local` names the elements a step puts back and writes their `provenance` rows in the change's own transaction, `store::history::put_changes`); no public API takes a root, and a client update (`Doc::check_client_update` / `apply_client_update`) carries engine content only and never reaches the provenance table or the session's map. Refusing a root a client sends on the wire is the native decoder's. Target: native decoder, G2 (the engine's).
- `mlist.converge` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.move_no_duplicate` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.delete_wins` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.insert_beside_moved` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.move_and_set` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.move_lww` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.block_contiguous` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.chain_one_run` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `mlist.yata_separating_case` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `map.lww` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `map.delete_vs_set` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `map.steady_bounded` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.cell_kept` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.cell_vs_row_delete` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.cell_vs_col_delete` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.sort_atomic` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.block_contiguous` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.cell_lww` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.row_identity` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.sort_vs_cell` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.row_move_keeps_cells` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.col_move_keeps_cells` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.col_move_no_duplicate` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.delete_wins_restorable` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.insert_beside_moved_row` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `table.ragged_render` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `stream.merged_replays` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `stream.merged_matches_local` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.
- `stream.merged_writes` on mpce-model (FugueMax): mpce-model (v0.4.1) has no list, mlist or map model yet; K2 Engine is adding them (mlist with `undel`, engine answer E1; then map). Target: mpce-model, after K2 Engine's list/mlist/map models.

## Never

- `erase.span_includes_tombstones` on kit Doc (yrs): Yrs has no element-level purge event: the kit's purge deletes the span and re-encodes the doc with garbage collection on (K-PURGE-11), so no tombstone keeps content, inside the span or outside it (erase.purge, erase.versions).
- `sec.duplicate_ranges` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `sec.base_too_old` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `sec.client_provenance_refused` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `erase.participant` on mpce-model (FugueMax): the kit's mapping from agent IDs to people, not the model's.
- `hist.undo_atomic` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.undo_partial` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.redo` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.undo_own_only` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.undo_own_only_tokens` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.blame_grain` on kit Doc (yrs): blame at a version needs that version's items with their IDs. Yrs keeps no engine state of a past version: garbage collection is on (K-PURGE-11), so deleted items keep no content, and the kit's versions are engine-neutral text (K-HIST-5) with no attribution. The kit blames the live doc (text.blame_char; erase.purge checks a purge moves no other character's blame).
- `hist.merge_any_base` on kit Doc (yrs): a branch from a past version needs that version's engine state. Yrs keeps none: garbage collection is on (K-PURGE-11), and the kit's versions are engine-neutral text (K-HIST-5) over a log that is compacted and, at a purge, replaced. The kit has no branch verb (K-BR-6 is the native engine's, G2a); a branch from the current state runs (hist.branch_merge).

## The native engine

Not a column yet. Its deviation list must be empty (K-CT-2). The structured rules await it:

- `mlist.*`: the native engine's mlist (with `undel`, engine answer E1), G2b.
- `map.*`: the native engine's map, G2b.
- `table.*`: the native engine's mlist and map (G2b); the table composition model, `MC table` (engine answer E3).
- `stream.*`: the native engine's applied stream for list and map ops (engine answer E4), G2b.

## Undo spec trace test

`crates/spec-undo` (`docs/SPEC-UNDO.md` §8): seeded traces replayed through the oracle `spec_undo::model` and the kit's sessions on Yrs, compared after every step (every root's text, the outcome, the conflict regions as an exact set, every pid's stack depths); 256 traces in the normal run, every sequential case class hit; a large run (10^4 hits per class) is `#[ignore]`d, for the compute nodes.

Later (not in this suite yet):

- The **concurrency class** of the trace test: concurrent thin-client batches integrated between undo steps, the oracle consuming the daemon's applied hunks; equal wherever the merged texts agree, other differences classified by the Def. 4 checker; Ctrl+Z with unacked ops (U21). Target: G3.
- The trace test's **native leg** (oracle vs the kit on the native engine), and the **positions PT**: each engine's reported hunk positions against `k2e-model`'s element positions on the alphabet `{a, \n}`, with a text-diffing mutant adapter that must fail (N4, E-HIST-5). Target: native engine, G2a / G3.
- Person grouping (U3) in the oracle's trace test (the kit's closers and the host-side (g) split are in v0.9.7, `hist.undo_group_person` in `crates/base/tests/history.rs` with its trace class) and Ctrl+Z with unacked ops; `hist.ctrl_z_unacked`. Target: native tab, G3.

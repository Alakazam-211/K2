# Conformance coverage

Generated from `crates/conformance/src/matrix.rs`; a test keeps this file equal to it (regenerate with `CONFORMANCE_BLESS=1 cargo test -p k2-mpc-conformance --test matrix`).

A rule runs on every engine that has the capabilities it needs. A rule that runs on no engine fails the suite unless it is listed as awaiting, with a reason and a target. A pinned deviation is asserted to still happen. A layer (one part of an engine) runs its capabilities' rules only; "–" is a rule that isn't its part.

| Rule | Requirement | Needs | kit Doc (yrs) | mpce-model (FugueMax) | mpce-sync 0.1.1 (sync layer) | kit Doc (native) |
|---|---|---|---|---|---|---|
| `text.converge` | E-CORE-1, E-TEXT-2 | Text | pass | pass | – | pass |
| `text.no_interleave_forward` | E-TEXT-3 | Text | pass | pass | – | pass |
| `text.no_interleave_backward` | E-TEXT-1, K-CT-5 | Text | pass | pass | – | pass |
| `text.no_interleave_backward_multi` | E-TEXT-1, K-CT-5(a) | Text | pass | pass | – | pass |
| `text.keep_concurrent_insert` | E-TEXT-6 | Text | pass | pass | – | pass |
| `text.delete_once` | E-TEXT-7 | Text | pass | pass | – | pass |
| `text.edit_atomic` | E-CORE-10 | Text | pass | pass | – | pass |
| `text.yata_separating_case` | E-TEXT-1, K-CT-5(b) | Text | **deviation** (pinned) | pass | – | pass |
| `text.yata_paper_a19` | K-CT-5(e) | Text | **deviation** (pinned) | pass | – | pass |
| `text.scalar_safe` | E-TEXT-9 | Text | pass | pass | – | pass |
| `text.edit_atomic_split` | K-CT-3 | Events | pass | pass | – | pass |
| `core.idempotent` | E-CORE-2 | Events | pass | pass | – | pass |
| `core.causal` | E-CORE-3 | Events | **deviation** (pinned) | pass | – | pass |
| `core.no_equivocation` | E-CORE-4 | Events | **deviation** (pinned) | pass | – | pass |
| `core.authorship` | E-CORE-5 | Authorship | **deviation** (pinned) | pass | – | pass |
| `core.refused_no_trace` | E-CORE-3, E-CORE-4, E-CORE-5; hardening: a refused apply leaves no trace | Authorship + Canonical | **deviation** (pinned) | pass | – | pass |
| `core.sec` | E-CORE-1 | Canonical | pass | pass | – | pass |
| `core.steady_plain` | E-TEXT-13, E-PROOF-10 | Steady | **deviation** (pinned) | never | – | awaiting: the native engine's G2a M4 (~11-10 to 11-12) |
| `prov.record` | E-PROV-1, review C9, C20 | Provenance | **deviation** (pinned) | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `perm.predicate_called` | E-PERM-1, K-PERM-15 (review), C20 | Admission | awaiting: the kit's K-PERM-15 and the native engine's E-PERM, T = G2a M2 (~11-18) | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `perm.refused_whole` | E-PERM-2, K-PERM-15 (review) | Admission | awaiting: the kit's K-PERM-15 and the native engine's E-PERM, T = G2a M2 (~11-18) | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `sec.false_receipt` | E-CLI-14 | Protocol | awaiting: the kit on the native engine's client protocol, T = G2a M2 (~11-18) | never | pass | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `sec.rebase_required` | E-SEC-14, E-SEC-16, E-SEC-17 (amended G1-3), E-CORE-4 | Protocol | awaiting: the kit on the native engine's client protocol, T = G2a M2 (~11-18) | never | pass | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `text.blame_char` | E-TEXT-11, undo spec U18a, U23 | Blame | pass | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `text.cursor_stable` | E-TEXT-10 | Anchors | pass | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `hist.undo_atomic` | E-HIST-6, K-CT-4 | Undo | **deviation** (pinned) | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.undo_partial` | K-HIST-13, K-CT-4 | Undo | pass | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.redo` | E-HIST-6, K-HIST-2 | Undo | pass | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.undo_own_only` | E-HIST-5, undo spec U10, U10a (by root, v1.8), U18a, U23 | Undo + Blame | pass | never | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `hist.undo_own_only_tokens` | undo spec v1.10, D1 (authored ranges, by token) | Undo + Blame | pass | never | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `hist.version_stable` | E-HIST-1 | Versions | pass | pass | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.restore_forward` | E-HIST-2, K-HIST-8 | Versions | pass | pass | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.restore_minimal` | K-HIST-8, undo spec D1 | Versions | pass | pass | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `hist.blame_grain` | E-HIST-4 | BlameAt | never | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `hist.branch_merge` | E-HIST-8 | Branches | pass | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `hist.merge_any_base` | E-HIST-9 | BranchAt | never | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `sec.duplicate_ranges` | E-SEC-6 | Wire | awaiting: native decoder, G2 | never | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `sec.base_too_old` | E-SEC-12 | Wire | awaiting: native decoder, G2 | never | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `sec.client_provenance_refused` | undo spec U23 (v1.8) | Wire | awaiting: native decoder, G2 (the engine's) | never | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `erase.purge` | E-ERASE-1, K-PURGE-5 | Purge + Blame + Anchors | pass | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `erase.participant` | E-ERASE-5, K-PURGE-7 | Erasure | pass | never | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `erase.no_readmit` | E-ERASE-6 | Purge | pass | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `erase.versions` | E-ERASE-7, K-PURGE-4 | HistoryPurge | pass | pass | – | awaiting: the native engine behind the kit's sessions (C4, C5) |
| `erase.span_includes_tombstones` | E-ERASE-1 | SpanIds | never | pass | – | awaiting: C5: the native column's remaining adapters (engine migration, kit side) |
| `mlist.converge` | E-CORE-1, E-MLIST-1, E-MLIST-2 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.move_no_duplicate` | E-MLIST-1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.delete_wins` | E-MLIST-2, engine answer E1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.insert_beside_moved` | E-MLIST-2 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.move_and_set` | E-MLIST-1 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.move_lww` | E-MLIST-1 | MList | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.block_contiguous` | E-LIST-1, engine answer E9 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.chain_one_run` | engine answers E8, E9 | MList | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `mlist.yata_separating_case` | E-LIST-1 (as E-TEXT-1) | MList | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `map.lww` | E-MAP-1 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `map.delete_vs_set` | E-MAP-1, engine answer E5 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `map.steady_bounded` | E-MAP-3 | Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.cell_kept` | E-TABLE-1, engine answer E3 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.cell_vs_row_delete` | E-TABLE-2, K-CSV-4, engine answer E3 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.cell_vs_col_delete` | E-TABLE-2 (extended), K-CSV-4 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.sort_atomic` | K-CSV-3, K-PAR-8 | Table + Merged | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.block_contiguous` | E-TABLE-1, engine answer E9 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.cell_lww` | E-TABLE-2, E-MAP-1 | Table | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.row_identity` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.sort_vs_cell` | K-CSV-4 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.row_move_keeps_cells` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.col_move_keeps_cells` | K-CSV-1, E-TABLE-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.col_move_no_duplicate` | K-CSV-4, E-MLIST-1 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.delete_wins_restorable` | E-MLIST-2, K-CSV-4 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.insert_beside_moved_row` | E-MLIST-2 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `table.ragged_render` | CSV contract §1.5, K-CSV-2 | Table | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `stream.merged_replays` | CSV seam design §4.2, A5 | Merged + MList + Map | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `stream.merged_matches_local` | CSV seam design §4.2, engine answer E4 | Merged + MList + Map | pass | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |
| `stream.merged_writes` | engine answer E4 | Merged + MList + Map | **deviation** (pinned) | awaiting: mpce-model, after K2 Engine's list/mlist/map models | – | awaiting: the native engine's mlist and map (G2b), then the structured adapter |

## Engines

- **kit Doc (yrs)**: Text, Events, Blame, Anchors, Undo, Versions, HistoryPurge, Erasure, Branches, Purge, MList, Map, Table, Merged, Authorship, Canonical, Steady, Provenance.
- **mpce-model (FugueMax)**: Text, Events, Blame, Anchors, Versions, HistoryPurge, Branches, BranchAt, Purge, BlameAt, SpanIds, Authorship, Canonical.
- **mpce-sync 0.1.1 (sync layer)** (a layer: these capabilities' rules only): Protocol.
- **kit Doc (native)**: Text, Events, Authorship, Canonical.

## Pinned deviations

- `text.yata_separating_case` on kit Doc (yrs): yrs (YATA) orders the separating script [pfae], splitting one writer's run `fe`; FugueMax gives [apfe]. Wire-format bound: fixed only by a FugueMax-class engine.
- `text.yata_paper_a19` on kit Doc (yrs): yrs (YATA) orders the Fugue paper's A.1.9 example `axb` (as the paper reports for Yjs); FugueMax gives `xab`. Wire-format bound.
- `core.causal` on kit Doc (yrs): the kit checks an update's dependencies per client (each client's clocks must continue the doc's), which refuses a skipped own predecessor with MissingDeps; an event whose parent is another client's event the doc lacks passes that check, yrs holds it as pending, and the kit then poisons the doc (Pending: reload from the store) instead of refusing it with nothing changed. The native engine checks every parent.
- `core.refused_no_trace` on kit Doc (yrs): Hardening; pinned on Yrs. A host calling Doc::apply_client_update directly must reload on Pending; a session does it itself.
- `core.no_equivocation` on kit Doc (yrs): Pinned on Yrs. The native engine conforms (E-CORE-4).
- `core.authorship` on kit Doc (yrs): Hardening; pinned on Yrs. The native engine conforms (E-CORE-5).
- `core.steady_plain` on kit Doc (yrs): Pinned on Yrs: E-TEXT-13's budget is the native engine's target.
- `prov.record` on kit Doc (yrs): the kit's change record (`ChangeInfo`, one per change: the author's pid, on whose behalf, host time, the change's kind and surface) doesn't say whether the author is a person or an agent: the host knows that from the pid. Interface r1.7's typed core carries it (review C9).
- `hist.undo_atomic` on kit Doc (yrs): a session applies `undo --steps N` (and redo) as N engine batches, N versions and N log entries, committed together in one store transaction under the session lock; the rule asks one batch (one log append). A single step over several roots is one batch attributed to the undoer, as asserted.
- `mlist.move_lww` on kit Doc (yrs): a move writes the element's position register, a yrs map entry, and yrs orders concurrent writes of one map entry by client ID (YATA's tie-break), not by (lamport, agent): a move made after more of its writer's own changes can lose. Wire-format bound.
- `mlist.yata_separating_case` on kit Doc (yrs): the list's slots are a yrs array, so YATA orders them: the separating script gives [lb p f a e rb], splitting one writer's run `f e`; FugueMax gives [lb a p f e rb] (as text.yata_separating_case). Wire-format bound.
- `map.lww` on kit Doc (yrs): yrs orders concurrent sets of one map key by client ID (YATA's tie-break), not by (lamport, agent). Wire-format bound.
- `map.delete_vs_set` on kit Doc (yrs): a yrs map delete removes only the entry it saw, so a concurrent set always survives it, whatever (lamport, agent) say. (A list element's delete is a set of its delete flag, and wins, as E-MLIST-2 asks.)
- `map.steady_bounded` on kit Doc (yrs): each overwrite of a map key leaves a content-free item stub (content is garbage-collected, K-PURGE-11), in the live doc and in a doc loaded from its state (the stored v2 state compresses a regular run of them). Bounded on the interim by the CSV caps.
- `table.cell_lww` on kit Doc (yrs): cells are a kit Map, so concurrent sets of one cell are ordered by client ID (map.lww).
- `stream.merged_writes` on kit Doc (yrs): a merged update's stream (`Doc::applied_since`, a keyed diff) names each write by the facet's winner; on yrs a restore removes the delete flag and a map delete removes the entry, and yrs keeps no identity for a removal, so those two ops carry no write (`None`), as `winner(Existence)` of a live element is `None`. The native engine reports every merged op's event ID (E4).

## Awaiting

- `perm.predicate_called` on kit Doc (yrs): the kit has no write predicate yet: rights are per document (`Host::rights`); the host's predicate over (writer, root, region), called for every kit-path edit, client update and fork merge, is K-PERM-15 (review C-c), and the native engine's admission E-PERM-1. The rule runs today on a test seam (`seams::GatedYrs`: the kit's Doc behind a predicate call) with its planted negatives. Target: the kit's K-PERM-15 and the native engine's E-PERM, T = G2a M2 (~11-18).
- `perm.refused_whole` on kit Doc (yrs): the kit has no write predicate yet: rights are per document (`Host::rights`); the host's predicate over (writer, root, region), called for every kit-path edit, client update and fork merge, is K-PERM-15 (review C-c), and the native engine's admission E-PERM-1. The rule runs today on a test seam (`seams::GatedYrs`: the kit's Doc behind a predicate call) with its planted negatives. Target: the kit's K-PERM-15 and the native engine's E-PERM, T = G2a M2 (~11-18).
- `sec.false_receipt` on kit Doc (yrs): not applicable on Yrs: Yrs integrates an update from any base against its full merge state, so the kit on Yrs has no stability frontier S, no received-frontier reports and no server re-base; they are the native engine's client protocol. The rule runs on the engine's sync layer (mpce-sync 0.1.1: its send log, MPCE-E185, its tracker of S, the per-client re-base epoch and recvOp's decision with the fence, `Tracker::route_op`). Not on it: the re-issue, the held queue and the typed frame are the host's in mpce-sync (`sync::SyncRebase` is a test host over mpce-model's text), and the native engine's integrate and re-base (mpce-core's graph: MPCE-E182, the defence against an overstated base; `Engine::set_stable`) and its wire frames are not in the suite. Target: the kit on the native engine's client protocol, T = G2a M2 (~11-18).
- `sec.rebase_required` on kit Doc (yrs): not applicable on Yrs: Yrs integrates an update from any base against its full merge state, so the kit on Yrs has no stability frontier S, no received-frontier reports and no server re-base; they are the native engine's client protocol. The rule runs on the engine's sync layer (mpce-sync 0.1.1: its send log, MPCE-E185, its tracker of S, the per-client re-base epoch and recvOp's decision with the fence, `Tracker::route_op`). Not on it: the re-issue, the held queue and the typed frame are the host's in mpce-sync (`sync::SyncRebase` is a test host over mpce-model's text), and the native engine's integrate and re-base (mpce-core's graph: MPCE-E182, the defence against an overstated base; `Engine::set_stable`) and its wire frames are not in the suite. Target: the kit on the native engine's client protocol, T = G2a M2 (~11-18).
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
- every rule that needs Steady on kit Doc (native): E-TEXT-13: measured each run, gating once claimed. Target: the native engine's G2a M4 (~11-10 to 11-12).
- every rule that needs Blame on kit Doc (native): blame per character through the native adapter. Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs Anchors on kit Doc (native): relative positions (`RelPos`) through the adapter. Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs Undo on kit Doc (native): undo over a session on the native engine. Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs Versions on kit Doc (native): versions over a session on the native engine. Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs HistoryPurge on kit Doc (native): history purge over native records (C3). Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs Erasure on kit Doc (native): participant erasure over a session on the native engine. Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs Branches on kit Doc (native): branches (`Engine::fork`) through the adapter. Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs BranchAt on kit Doc (native): a branch at a past version (`Engine::fork(Some(v))`). Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs Purge on kit Doc (native): purge through `PurgeSpan` and the record rewrite (C3). Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs BlameAt on kit Doc (native): blame at a past version (`History::text_at`). Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs Wire on kit Doc (native): the native decoder on the kit's wire (C6). Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs SpanIds on kit Doc (native): purge by the element IDs of a span (`ReadText::span_ids`). Target: C5: the native column's remaining adapters (engine migration, kit side).
- every rule that needs MList on kit Doc (native): movable lists on the native engine. Target: the native engine's mlist and map (G2b), then the structured adapter.
- every rule that needs Map on kit Doc (native): maps on the native engine. Target: the native engine's mlist and map (G2b), then the structured adapter.
- every rule that needs Table on kit Doc (native): the CSV table on the native engine. Target: the native engine's mlist and map (G2b), then the structured adapter.
- every rule that needs Merged on kit Doc (native): the applied stream of merged list and map ops. Target: the native engine's mlist and map (G2b), then the structured adapter.
- every rule that needs Provenance on kit Doc (native): the typed change record over a native session. Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs Admission on kit Doc (native): the host write predicate (K-PERM-15). Target: the native engine behind the kit's sessions (C4, C5).
- every rule that needs Protocol on kit Doc (native): the client protocol (mpce-sync) behind the kit's sessions. Target: the native engine behind the kit's sessions (C4, C5).

## Never

- `core.steady_plain` on mpce-model (FugueMax): the reference model keeps every event with its origins by design (it is the oracle of merge semantics, not a steady state); E-TEXT-13 is the native engine's.
- `prov.record` on mpce-model (FugueMax): provenance is metadata outside merge semantics; the model's events carry only their agent.
- `perm.predicate_called` on mpce-model (FugueMax): the model has no host and no admission step; the predicate is not in merge (E-PERM-4).
- `perm.refused_whole` on mpce-model (FugueMax): the model has no host and no admission step; the predicate is not in merge (E-PERM-4).
- `sec.false_receipt` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `sec.rebase_required` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
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

The kit's Doc on the native engine is a column (C2; its sync layer, mpce-sync, is the layer column). Its deviation list must be empty (K-CT-2). The structured rules await the engine's G2b:

- `mlist.*`: the native engine's mlist (with `undel`, engine answer E1), G2b.
- `map.*`: the native engine's map, G2b.
- `table.*`: the native engine's mlist and map (G2b); the table composition model, `MC table` (engine answer E3).
- `stream.*`: the native engine's applied stream for list and map ops (engine answer E4), G2b.

## Engine half v2 rules

The rules the engine half v2 (draft 2.1) adds to the kit's suite, with the gate its row sets (the contract) and the suite's delivery target.

| Rule | Gate (PRD) | Target |
|---|---|---|
| `core.authorship` | G2-core | T = G2a M2 (~11-18) |
| `core.sec` | G2-core | T = G2a M2 (~11-18) |
| `core.steady_plain` | G2a | T = G2a M2 (~11-18) |
| `prov.record` | G2a | T = G2a M2 (~11-18) |
| `perm.predicate_called` | G2a | T = G2a M2 (~11-18) |
| `perm.refused_whole` | G2a | T = G2a M2 (~11-18) |
| `sec.false_receipt` | G2-core | T = G2a M2 (~11-18) |
| `sec.rebase_required` | G2-core | T = G2a M2 (~11-18) |

## Hardening rules

The kit's hardening rules (a refused apply leaves no trace), with the gate they hold from and the suite's delivery target.

| Rule | Gate | Target |
|---|---|---|
| `core.refused_no_trace` | G2a (the native adapter, C2) | T = G2a M2 (~11-18) |

## Undo spec trace test

`crates/spec-undo` (`docs/SPEC-UNDO.md` §8): seeded traces replayed through the oracle `spec_undo::model` and the kit's sessions on Yrs, compared after every step (every root's text, the outcome, the conflict regions as an exact set, every pid's stack depths); 256 traces in the normal run, every sequential case class hit; a large run (10^4 hits per class) is `#[ignore]`d, for the compute nodes.

Later (not in this suite yet):

- The **concurrency class** of the trace test: concurrent thin-client batches integrated between undo steps, the oracle consuming the daemon's applied hunks; equal wherever the merged texts agree, other differences classified by the Def. 4 checker; Ctrl+Z with unacked ops (U21). Target: G3.
- The trace test's **native leg** (oracle vs the kit on the native engine), and the **positions PT**: each engine's reported hunk positions against `k2e-model`'s element positions on the alphabet `{a, \n}`, with a text-diffing mutant adapter that must fail (N4, E-HIST-5). Target: native engine, G2a / G3.
- Person grouping (U3) in the oracle's trace test (the kit's closers and the host-side (g) split are in v0.9.7, `hist.undo_group_person` in `crates/base/tests/history.rs` with its trace class) and Ctrl+Z with unacked ops; `hist.ctrl_z_unacked`. Target: native tab, G3.

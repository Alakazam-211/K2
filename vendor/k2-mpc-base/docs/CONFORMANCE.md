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
| `text.blame_char` | E-TEXT-11 | Blame | pass | pass |
| `text.cursor_stable` | E-TEXT-10 | Anchors | pass | pass |
| `hist.undo_atomic` | E-HIST-6, K-CT-4 | Undo | **deviation** (pinned) | never |
| `hist.undo_partial` | K-HIST-13, K-CT-4 | Undo | pass | never |
| `hist.redo` | E-HIST-6, K-HIST-2 | Undo | pass | never |
| `hist.undo_own_only` | E-HIST-5, undo spec U10, U18a | Undo + Blame | pass | never |
| `hist.version_stable` | E-HIST-1 | Versions | pass | pass |
| `hist.restore_forward` | E-HIST-2, K-HIST-8 | Versions | pass | pass |
| `hist.restore_minimal` | K-HIST-8, undo spec D1 | Versions | pass | pass |
| `hist.blame_grain` | E-HIST-4 | BlameAt | never | pass |
| `hist.branch_merge` | E-HIST-8 | Branches | pass | pass |
| `hist.merge_any_base` | E-HIST-9 | BranchAt | never | pass |
| `sec.duplicate_ranges` | E-SEC-6 | Wire | awaiting: native decoder, G2 | never |
| `sec.base_too_old` | E-SEC-12 | Wire | awaiting: native decoder, G2 | never |
| `erase.purge` | E-ERASE-1, K-PURGE-5 | Purge + Blame + Anchors | pass | pass |
| `erase.participant` | E-ERASE-5, K-PURGE-7 | Erasure | pass | never |
| `erase.no_readmit` | E-ERASE-6 | Purge | pass | pass |
| `erase.versions` | E-ERASE-7, K-PURGE-4 | HistoryPurge | pass | pass |
| `erase.span_includes_tombstones` | E-ERASE-1 | SpanIds | never | pass |

## Engines

- **kit Doc (yrs)**: Text, Events, Blame, Anchors, Undo, Versions, HistoryPurge, Erasure, Branches, Purge.
- **mpce-model (FugueMax)**: Text, Events, Blame, Anchors, Versions, HistoryPurge, Branches, BranchAt, Purge, BlameAt, SpanIds.

## Pinned deviations

- `text.yata_separating_case` on kit Doc (yrs): yrs (YATA) orders the separating script [pfae], splitting one writer's run `fe`; FugueMax gives [apfe]. Wire-format bound: fixed only by a FugueMax-class engine.
- `text.yata_paper_a19` on kit Doc (yrs): yrs (YATA) orders the Fugue paper's A.1.9 example `axb` (as the paper reports for Yjs); FugueMax gives `xab`. Wire-format bound.
- `core.causal` on kit Doc (yrs): the kit checks an update's dependencies per client (each client's clocks must continue the doc's), which refuses a skipped own predecessor with MissingDeps; an event whose parent is another client's event the doc lacks passes that check, yrs holds it as pending, and the kit then poisons the doc (Pending: reload from the store) instead of refusing it with nothing changed. The native engine checks every parent.
- `core.no_equivocation` on kit Doc (yrs): yrs identifies content by (client, clock) only: a known ID with different content is skipped as already known, silently (the first content stays), not refused and reported. The native engine keeps a keyed digest per run (E-CORE-4).
- `hist.undo_atomic` on kit Doc (yrs): a session applies `undo --steps N` (and redo) as N engine batches, N versions and N log entries, committed together in one store transaction under the session lock; the rule asks one batch (one log append). A single step over several roots is one batch attributed to the undoer, as asserted.

## Awaiting

- `sec.duplicate_ranges` on kit Doc (yrs): decoding untrusted update bytes is the native decoder's; this suite feeds engines only bytes they made. Target: native decoder, G2.
- `sec.base_too_old` on kit Doc (yrs): decoding untrusted update bytes is the native decoder's; this suite feeds engines only bytes they made. Target: native decoder, G2.

## Never

- `erase.span_includes_tombstones` on kit Doc (yrs): Yrs has no element-level purge event: the kit's purge deletes the span and re-encodes the doc with garbage collection on (K-PURGE-11), so no tombstone keeps content, inside the span or outside it (erase.purge, erase.versions).
- `sec.duplicate_ranges` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `sec.base_too_old` on mpce-model (FugueMax): the reference model has no wire format (native decoder / client protocol).
- `erase.participant` on mpce-model (FugueMax): the kit's mapping from agent IDs to people, not the model's.
- `hist.undo_atomic` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.undo_partial` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.redo` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.undo_own_only` on mpce-model (FugueMax): undo is the kit's, over a session (undo spec); the model has no sessions.
- `hist.blame_grain` on kit Doc (yrs): blame at a version needs that version's items with their IDs. Yrs keeps no engine state of a past version: garbage collection is on (K-PURGE-11), so deleted items keep no content, and the kit's versions are engine-neutral text (K-HIST-5) with no attribution. The kit blames the live doc (text.blame_char; erase.purge checks a purge moves no other character's blame).
- `hist.merge_any_base` on kit Doc (yrs): a branch from a past version needs that version's engine state. Yrs keeps none: garbage collection is on (K-PURGE-11), and the kit's versions are engine-neutral text (K-HIST-5) over a log that is compacted and, at a purge, replaced. The kit has no branch verb (K-BR-6 is the native engine's, G2a); a branch from the current state runs (hist.branch_merge).

## Undo spec trace test

`crates/spec-undo` (`docs/SPEC-UNDO.md` §8): seeded traces replayed through the oracle `spec_undo::model` and the kit's sessions on Yrs, compared after every step (every root's text, the outcome, the conflict regions as an exact set, every pid's stack depths); 256 traces in the normal run, every sequential case class hit; a large run (10^4 hits per class) is `#[ignore]`d, for the compute nodes.

Later (not in this suite yet):

- The **concurrency class** of the trace test: concurrent thin-client batches integrated between undo steps, the oracle consuming the daemon's applied hunks; equal wherever the merged texts agree, other differences classified by the Def. 4 checker; Ctrl+Z with unacked ops (U21). Target: G3.
- The trace test's **native leg** (oracle vs the kit on the native engine), and the **positions PT**: each engine's reported hunk positions against `k2e-model`'s element positions on the alphabet `{a, \n}`, with a text-diffing mutant adapter that must fail (N4, E-HIST-5). Target: native engine, G2a / G3.
- Person grouping (U3, the daemon-side (g) split) and its case class; `hist.undo_group_person`, `hist.ctrl_z_unacked`. Target: native tab, G3.

# Changelog: mpce-model

Reference models for the MPC engine (the oracle of record). Std only, `#![forbid(unsafe_code)]`,
never published, never a dependency of shipped code. Consumers pull this crate by git subtree on
the `crates/mpce-model` prefix and pin a tag `mpce-model-vX.Y.Z`.

## v0.4.1 (tag `mpce-model-v0.4.1`)
Span purge, the restore contract note, and one informational field on tree rescue reports.
Std only, `#![forbid(unsafe_code)]`.
- **`text::Doc::span_ids(start, end) -> Result<Vec<Id>, DocError>`** for the kit's span purge
  (`erase.span_includes_tombstones`): `purge_as(agent, &span_ids(start, end)?)`. Offsets are
  UTF-8 byte offsets into the current visible text, `start < end`. The result, in list order
  (tombstones included), is the element of the span's first visible character through the
  element of its last, inclusive, plus every tombstone strictly between them; tombstones just
  outside either end are excluded (purge by text, `find_content`, reaches those). Elements in
  that range already purged (or content-less and not yet deleted) are **included**;
  `purge_as` accepts them (unchanged). Errors: `OutOfRange`, `NotCharBoundary { offset }`
  (offsets are checked first, `start` then `end`), and **`BadHunks`** for an empty or reversed
  span (`start >= end`; no new `DocError` variant, so exhaustive matches keep compiling).
  Read-only.
- **`restore_as` is model-internal, not the `hist.restore_*` contract.** Restore is a kit
  operation for every engine, `edit_as(agent, diff::hunks(text(), text_at(V)))`, so
  `hist.restore_minimal` holds by construction. Behaviour unchanged; doc comment says so.
- **`tree_move`: rescue reports carry `walked_past: Vec<NodeId>`** (approved by K2 Cores),
  the deleted nodes the projection's display walk passed on its way to the visible parent,
  nearest first ("shown under V because B was deleted, and its former parents C, D"). On
  `Rescue` (the projection's display info) and on `ConflictReport::Rescue { node, delete,
  walked_past }`. **Not part of the report's identity:** `ConflictReport` now implements
  `PartialEq`/`Eq`/`Ord`/`Hash` by hand over `ConflictReport::key() -> ReportKey` (`CycleSkip
  { mover, node, op }` / `Rescue { node, delete }`), so a `BTreeSet<ConflictReport>` still
  holds one rescue per (node, core-path delete); compare `walked_past` explicitly.
  `ConflictReport` is no longer `Copy` (it holds a `Vec`); `VisibleTree::rescues()` is
  unchanged. New invariant in `check_replica`, `rescue_walked_past`: distinct deleted nodes,
  starting with the deleted node on the root's core path. The fixtures assert it (nested
  deletes, the oldp loop, the four-node fixtures). The Quint model has no such field;
  `tree_itf_replay` compares rescues as (node, delete) pairs, as before.
- Tests: `tests/text_v4.rs` (a typo-fixed secret inside the selection is erased, tombstoned
  typo included, by the byte scan of `tests/text_v3.rs`; a deleted copy just outside the
  selection survives the span purge and `find_content` + `purge_as` then reaches it; boundary
  refusals; multi-byte text; a span over another replica's concurrent tombstones after sync;
  a seeded property, `MPCE_PROP_SEEDS`, default 200); `tests/tree_move.rs` (`walked_past` in
  the fixtures; the report identity ignores it).

## v0.4.0 (tag `mpce-model-v0.4.0`)
The tree reference model, `tree_move` (the oracle of record for trees; PRD E-TREE-1..5, amendment
`g1-tree-causality` r2 for E-CORE-12 and E-TREE-4). Std only; nothing in `text`, `tree` or
`check` changed. Cross-checked against `spec/quint/tree.qnt` by ITF replay.
- **`fig4`**: Kleppmann et al., TPDS 2021, Fig. 4 transcribed line by line and generic over
  `T: Ord`, `N`, `M` (`Move`, `LogMove`, `Tree`, `State`, `get_parent` with the exact `∃!parent.
  ∃!meta` semantics, `ancestor`, `do_op`, `undo_op`, `redo_op`, recursive `apply_op`, `apply_ops`,
  `unique_parent`, `acyclic`). The log is a `Vec`, newest first.
- **Composition.** `NodeId` (`ROOT`, `TRASH`), `Timestamp { lamport, agent }`, `TreeEvent { id,
  lamport, parents, op: TreeOp::{Move { child, parent, pos: Position }, Other} }`; meta = the
  move's own event ID, its element in the new parent's FugueMax list (`crate::tree::Tree`; a
  `Position` is Algorithm 1's placement plus the left origin). Create = move of a fresh ID, delete
  = move under TRASH, restore = move out of it.
- **`Replica`**: `with_variant`, `move_as`/`create_as`/`delete_as`/`other_as(agent, ..)`,
  `deliver`/`deliver_all` (causal; Fig. 4 `apply_op` on the resident log), `position`, `tree`,
  `log`, `parent_of`, `list`, `live_elements`, `children`, `nodes`, `received`, `frontier`,
  `event`, `seq_log` (do_op over the op set in timestamp order, with skip status).
- **Causal skeleton.** On integration each event gets `T(e)` (maximal tree ops in past-or-self)
  and each move its `tparents`, from the T column only; `happened_before`/`concurrent` by skeleton
  reachability; `t_of`, `tparents_of`, `skeleton_ancestors`.
- **Compaction and restart.** `compact(&cv)`: drops the parents of a critical version and
  truncates its resident-log entries (S = `stability_point()`); T and the skeleton stay. A later
  move below S is refused (`MoveError::BelowStability`). `snapshot()`/`Replica::load()`: what an
  engine persists (no parents for compacted events); the per-parent lists are rebuilt.
- **Projection** (daemon-side): `projection()` / `projection_with(Causality::{Skeleton,
  Graph(&g)})` -> `VisibleTree { nodes: NodeView { status, parent, rescue: Option<Rescue {
  anchor, deletes }>, reasons }, children }`; `reports()` / `reports_of(&VisibleTree)`:
  `ConflictReport::{CycleSkip { mover, node, op }, Rescue { node, delete }}`. The rules are **K2
  Cores' core-path ruling (2026-10-28)**, identical in `spec/quint/tree.qnt`:
  1. **The walk stops only at a core-visible node** (one reaching ROOT through core parents),
     else ROOT (revisit, TRASH, no live old parent). It decides only where a rescue root is
     shown, so rescued subtrees always hang off the core-visible tree and the visible tree is
     acyclic. (The earlier walk stopped at the first *visible* node; with 4 nodes two rescued
     roots could be each other's visible parent: `tf_mutual_rescue_cycle`.)
  2. **Rescue is decided on the core path:** a live node that is not core-visible is a rescue
     root iff its effective move is concurrent with the delete of every deleted node on its core
     path (its core ancestors whose parent is TRASH; the core tree is a forest, so one) and its
     core parent is not visible. Deletes further up the pre-delete (old-parent) chain no longer
     count: a node whose mover had seen such a delete, but none on its core path, is now
     rescued. Rescue reports and hidden reasons name the core path's deletes.
  3. A rescue root nested in another rescued subtree shows **beside** it, under the nearest
     core-visible node, with its own conflict report.
  4. **Same-spot order** (`SameSpotOrder::PreDeletePreorder`, the default; also for roots with
     no spot): the pre-order of the rebuilt pre-delete tree (live nodes at their core parent,
     deleted nodes re-hung at their delete's old parent; siblings by FugueMax list position,
     then node ID; a node whose pre-delete parent chain comes back to it, the oldp loop, is a
     root of the rebuild; roots ROOT, then by node ID). `SameSpotOrder::NodeId` orders by node
     ID (for comparison).
- **Invariants** `check_replica(&r, Option<&Graph>)` (the Quint model's `inv` per replica, incl.
  skeleton = happened-before and projection unchanged by compaction; `projection_acyclic` always;
  `rescue_sound` on the core path: a rescue reports exactly its core path's deletes, each
  concurrent with its move; `rescue_under_core_visible`; `rescue_complete` on the core path) and
  `check_converge`.
- **Mutants** `Variant::{NoUndoDoRedo, NoCycleCheck, LwwByArrival, AncestorCheckStale,
  RescueDropsMovedIn, RescueIgnoresCausality, RescueAnyNotEvery, RescueWalkUnbounded,
  CompactTreeStructure}` (the Quint names via `quint_name`/`from_quint`). Under the core-path
  rule `RescueAnyNotEvery` tests "any delete the display walk passes" (over the core path's
  single delete "any" would equal "every").
- Tests (`tests/tree_move.rs`): Fig. 4 over every delivery order; the 19 fixtures of
  `tree_fixtures.qnt` (`tree_fixtures`, 3 nodes; `tree_fixtures4`, 4 nodes), invariants after
  every step, including `tf_mutual_rescue_cycle` (now X and Y both under ROOT, acyclic),
  `tf_rescued_despite_seen_delete_up_the_walk`, `tf_nested_rescue_beside`,
  `tf_same_spot_oldp_loop` and `tf_stable_pair_pulled_into_rescue`; every mutant fails fixtures
  and the property test; property tests (`MPCE_PROP_SEEDS`, default 100; `MPCE_MUTANT_SEEDS`,
  default 30): 3 replicas, 3 or 4 seeded nodes (creates up to 6), random
  moves/creates/deletes/restores/non-tree events, random causal delivery and partial syncs,
  compaction at random critical points, events integrated over compacted parents, restarts from
  snapshots, an uncompacted twin per replica; full sync at the end.
- Fixture results changed by the ruling: `tf_nested_outer_after_move` (C is now rescued: its
  core path has only B, whose delete is concurrent; A's delete, which saw the move, is up the
  walk); `tf_nested_both_concurrent` and `tf_truncation_counterexample` report only the core
  path's delete.
- Tools: `src/bin/tree_itf_replay.rs` (ITF replay of `tree.qnt` through
  `tools/refmodel/itf_tree.qnt`, which exports the projection as a lagged variable),
  `tools/refmodel/tree_itf.sh`; the ITF JSON reader moved to `src/bin/shared/itf_json.rs` (shared
  with `itf_replay`, now with boolean values).

## v0.3.0 (tag `mpce-model-v0.3.0`)
Text versions, branches, restore and purge on `text::Doc` (inherent methods, std only; PRD engine
half E-HIST-1/2/4/8/9, E-ERASE-1..8).
- **Versions.** `pub type Version = BTreeSet<Id>` (a set of event IDs standing for its causal
  closure; normally a frontier). `version() -> Version` (the frontier).
  `text_at(&v) -> Result<String, DocError>`, `list_at(&v) -> Result<Vec<Id>, DocError>` (visible
  IDs) and `blame_at(&v) -> Result<Vec<(char, Id)>, DocError>`: the state of exactly the closure
  of `v`, minus content purged as far as this replica knows (E-HIST-1, E-ERASE-7). A version
  naming an unknown event is refused with the new `DocError::UnknownVersion { id }`.
- **Restore forward.** `restore_as(agent, &v) -> Result<Vec<Event>, DocError>`: one atomic
  `edit_as` batch by `agent` after which (with no concurrent events) `text() == text_at(v)`;
  visible text not at `v` is deleted, text at `v` but deleted now is inserted again as new
  content (new IDs, attributed to the restorer; U18a) at the gap where it was; purged content
  comes back as nothing. History is never rewritten; nothing to do gives no events.
- **Branches.** `fork_at(&v) -> Result<Doc, DocError>`: a replica holding exactly the closure of
  `v` (same agent, sibling rule and purge rule; content purged here stays purged), editable with
  `edit_as`. Its seqs per agent start after the highest the origin had, so an agent can edit a
  branch without reusing the origin's seqs (an agent must still not edit two replicas
  concurrently). `merge_from(&other) -> Result<usize, DocError>` =
  `deliver_all(&other.diff_events(self.frontier()))`; idempotent; any base.
- **Purge.** `purge_as(agent, &[Id]) -> Result<Event, DocError>`: one event, new
  `Op::Purge { targets: BTreeSet<Id> }` (IDs only; parents = the frontier), that deletes every
  still-live target and erases every target's content wherever it is delivered. Targets must be
  known inserts (`UnknownTarget`), and not none (new `DocError::EmptyPurge`).
  **Breaking:** `Op::Insert { value }` is now `Option<char>`; `None` = purged (IDs, parents,
  origins and placement are kept, so merges, positions and anchors keep working). Purged inserts
  are stored, logged and sent content-less (`log()`, `event()`, `diff_events`). Purge knowledge
  only grows: a content-less copy of a known insert erases the local content; a content-bearing
  copy of a purged insert is accepted as known and its content dropped (never re-admitted, never
  a `Conflict`, E-ERASE-6); any other difference under a known ID is still a `Conflict`. A
  content-less insert received before its purge event is invisible (not yet deleted); once every
  replica has every event all agree. No digest of purged content is kept (E-ERASE-3/9).
  `is_purged(id)`. `find_content(needle) -> Vec<Vec<Id>>`: every occurrence in all content held,
  tombstones included (the list order with tombstones, the visible text of every state in this
  replica's log, and each event's author's state), for a purge by text.
- `PurgeRule { Erase, KeepContent }` with `set_purge_rule`/`purge_rule`: `KeepContent` is a
  mutant (a purge that deletes but keeps the characters) for the byte scan's planted negative,
  as `SiblingRule::Fugue` is for FugueMax.
- Tests (`tests/text_v3.rs`): unit tests for each method with multi-byte text; property tests
  (`MPCE_PROP_SEEDS`, default 200): purge convergence (3 replicas, random edits, partial syncs,
  random purges, re-delivery of old content-bearing events, versions, restores,
  purge∘merge = merge∘purge with the purge at any point of the merge, full sync; convergence,
  `def4`, `strong_list_spec`, version stability, a byte scan for a purged 32-letter secret that
  was whole, split across two edits or typo-fixed, maybe deleted), its planted negative (the
  `KeepContent` mutant must be caught by the scan), and branches (forks at any past version of
  main or of a branch, restores and round trips, purges; merge = direct delivery, idempotent,
  convergence, version stability, `text_at(v) == fork_at(v).text()`).

## v0.2.2 (tag `mpce-model-v0.2.2`)
- Tests only: the property test now also checks that an **expanding** anchored range (start `Side::Left` at s, end `Side::Right` at e) **never inverts** under random concurrent edits, partial syncs and deletes covering the range, after every step and after the full sync; plus a unit test showing the opposite (contracting) convention inverts when someone types into an empty range. Ranges for claims and views must be anchored expanding (adopted in the kit's presence brief).

## v0.2.1 (tag `mpce-model-v0.2.1`)
- `edit_as` accepts **touching** hunks exactly as the kit's `diff::validate` does: the next hunk may start at the previous hunk's end, including an insert followed by a hunk starting at the same offset; only overlaps and **two pure inserts at one offset** are refused (`BadHunks`). Touching hunks apply in order, the same as one coalesced hunk (spec D2). v0.2.0 also refused an insert followed by a non-empty hunk at the same offset.

## v0.2.0 (tag `mpce-model-v0.2.0`, after review)
New on `text::Doc` (inherent methods, std only):
- `Doc::replica()` (and `replica_with_rule`): a replica with no fixed agent. `Doc::new(agent)` is
  unchanged. **Breaking:** `agent()` now returns `Option<u64>` (`None` for `replica()`);
  `insert`/`delete` on a `replica()` return `DocError::NoAgent`.
- `edit_as(agent, &[Hunk { start, end, insert }]) -> Result<Vec<Event>, DocError>`: UTF-8 byte
  offsets into the current visible text; hunks sorted by `start` (strictly increasing) and
  non-overlapping (`end <= next.start`; adjacent is fine), else `BadHunks`; an offset past the end
  is `OutOfRange`, one inside a scalar is `NotCharBoundary { offset }` (E-TEXT-9). Each hunk deletes
  its range, then inserts its text at that spot; one event per character, consecutive seqs for
  the agent, continuing after the agent's highest known seq (a returning agent). One atomic batch:
  on any error nothing changes (no events, no seq consumed).
- `diff_events(&remote_frontier) -> Vec<Event>`: the events this replica has that a replica at
  `remote_frontier` lacks, in causal (delivery) order. Frontier IDs unknown here are ignored.
- `deliver_all(&[Event]) -> Result<usize, DocError>`: any input order, duplicates allowed;
  delivered in a causal order; atomic (on `MissingDeps`/`Conflict` nothing changes); returns the
  number of new events.
- `blame() -> Vec<(char, Id)>`: each visible character with its inserting event (E-TEXT-11).
- Anchors (E-TEXT-10): `anchor(byte_offset, Side) -> Result<Anchor, DocError>` and
  `resolve(&Anchor) -> Result<usize, DocError>` (a byte offset). `Anchor { elem: Option<Id>, side }`
  (`Side` is `tree::Side`): `Right` sticks to the character after the offset, `Left` to the one
  before; `elem: None` is the document start (`Left`) or end (`Right`). A deleted element resolves
  to the gap where it was. `resolve` returns `UnknownTarget` if the element has not been received.
- New `DocError` variants: `NotCharBoundary { offset }`, `BadHunks`, `NoAgent`.
- `itf_replay`: dead-field warning fixed.
- Tests: byte-offset edits with multi-byte text, boundary and ordering refusals, batch atomicity,
  seq continuity, `diff_events` + `deliver_all` = `sync_from`, `deliver_all` atomicity, blame after
  concurrent edits, anchors across concurrent inserts and deletes (incl. deleted anchors and
  unknown elements), and a seeded property test (200 seeds; 3 replicas, random multi-byte edits,
  random partial syncs, random anchors; convergence, blame and anchor agreement, `check::def4` and
  `strong_list_spec` over every list state seen).

## v0.1.1 (tag `mpce-model-v0.1.1`)
- `Cargo.toml` is standalone (explicit `edition = "2021"`, `publish = false`, `version = "0.1.1"`, lints inlined under `[lints]`), so a git-subtree copy of `crates/mpce-model` builds on its own. No code changes.

## v0.1.0 (tag `mpce-model-v0.1.0`, commit 266e2a6)
- `tree`: the Fugue tree (Weidner & Kleppmann, Alg. 1) with FugueMax's Def. 6 right-sibling rule;
  plain Fugue's ID order kept as `SiblingRule` for mutant tests.
- `text::Doc`: `new(agent)`, `with_rule`, `insert(pos, ch)`, `delete(pos)` (char positions),
  `deliver(&Event)` (causal; `MissingDeps` and same-ID-different-content `Conflict` refused;
  re-delivery is a no-op), `sync_from(&Doc)`, `text`, `visible`, `list_with_tombstones`,
  `frontier`, `log`, `has`, `event`, `is_deleted`, `origins`.
- Agent IDs are `u64`; `Id(agent, seq)` orders lexicographically, agent numerically: the same order
  as the kit's Yrs client IDs, so scripts with clients 101/102/103 run unchanged.
- `check`: `def4`, `def4_with` (Def. 4 (1)–(3) with Lemma 5's exception) and `strong_list_spec`,
  returning typed violations.
- Validated by: fixture tests (separating case `[apfe]`, paper A.1.9 `xab`, three-writer script
  ×6 permutations, Figs. 6/7, Fugue vs FugueMax), 3,000 Quint ITF traces with 0 mismatches, and
  `mpce-enum`'s exact TLC counts.

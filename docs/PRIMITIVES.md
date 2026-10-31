# Primitives

A pack's schema is a set of named roots, each one primitive kind. **Kinds are defined by K2's
merge semantics, written as a conformance suite** (`crates/conformance`, generic over engines) that any
engine must pass. They are not the lowest common denominator of the engines. Where today's
engine falls short, the kit emulates the rule (and the suite proves it), or the deviation is
listed in that engine's `KNOWN_DEVIATIONS` with its reason. A new engine must have an empty
list; the reference model (`mpce-model`, FugueMax) runs every rule with an empty one.

Status: **Text, MovableList and Map are implemented, each with its suite.** MovableList and
Map (kit v0.10 work, for the CSV grid): ops, readers and the applied stream in `elem`; the
`mlist.*`, `map.*`, `table.*` (the CSV table's composition) and `stream.*` rules in the
conformance suite, over engine-neutral structured capabilities, with planted negatives, the
Yrs deviations pinned in the kit Doc's `KNOWN_DEVIATIONS`, and mpce-model and the native
engine awaiting their list and map primitives (`docs/CONFORMANCE.md`). The others are
specified here and are built, each with its suite, when the first pack needs them. Declaring an unimplemented kind fails loudly at `Doc::new`, and so does a
root name that is empty, repeated, or holds `/`.

| Kind | Model | Edits (data) | K2 semantics (conformance rules) | Notes on Yrs today |
|---|---|---|---|---|
| **Text** | Unicode string | `Edit::Text { root, hunks }` | converge; no interleaving of concurrent runs typed forward or backward; text inserted into a concurrently deleted range survives; a concurrent double delete deletes once; a multi-hunk edit is atomic | Native. **Deviations:** two scripts where YATA's order differs from FugueMax's (`text.yata_separating_case`, `text.yata_paper_a19`; wire-bound) |
| **RichText** | Text plus marks over ranges, plus embedded blocks | text edits; add/remove a mark over a range; insert/remove/move a block | Peritext: **per-mark expand rules set by the pack's schema** (bold expands, a link doesn't); concurrent formatting never diverges; marks survive concurrent text edits | Emulated in the kit (Yrs attributes inherit from the previous char and can diverge, yjs#291) |
| **List** | Ordered sequence of values | insert at, delete range, set item | Converge; concurrent inserts both kept, not interleaved | Native |
| **MovableList** | Ordered sequence with a **true move**; elements named by caller keys | `Edit::List { root, ops }`: insert (at an anchor: start, end, after or before a key), move, delete, restore, set, by key | A concurrent move never duplicates or loses an item; the last move wins; a delete beats a concurrent move or restore, and restore brings the same element back in place (undelete); an insert next to a moved item stays at the old spot. Keys are never reused. **Multi-element moves are forward chains** (`elem::chain`, `elem::reorder`): each move right after the previous one's new place | Emulated: a list of slots plus a per-key LWW position register (Kleppmann 2020) and a delete flag, four Yrs roots per list root (`engine/structured.rs`); the live order is cached. **Deviations (pinned):** concurrent moves ordered by client ID, not (lamport, agent) (`mlist.move_lww`); YATA's order on the separating script (`mlist.yata_separating_case`); a merged restore names no write (`stream.merged_writes`) |
| **Map** | String keys to values | `Edit::Map { root, ops }`: set key, delete key | LWW per key, attributed; nested primitives merge on their own; overwrite history doesn't grow without bound (compaction) | Native `YMap`; each register holds its winning write (`Doc::winner`). **Deviations (pinned):** concurrent sets ordered by client ID (`map.lww`, and `table.cell_lww` for cells); a concurrent set always beats a delete (`map.delete_vs_set`); each overwrite leaves an item stub in memory, live and after a reload (`map.steady_bounded`; the stored state compresses them); a merged map delete names no write (`stream.merged_writes`) |
| **Tree** | Nodes with ordered children and a **true move** | create, delete, move, set node data | A concurrent move never duplicates a node and never creates a cycle (the daemon rejects cycles); a deleted node's moved-in children are kept or reported | Emulated: {parent, fractional index} per node in one LWW property (Figma, Kleppmann 2020) |

## Not kinds

- **Blobs are a kit service, not a CRDT value.** Big bytes in a CRDT bloat its history forever.
  Instead, the kit keeps:
  - a content-addressed blob store;
  - a pointer in the doc (LWW);
  - a lease;
  - versions as pointer history.

  This is tier 0's mechanism, built in `src/blob.rs` (keyed CAS files under a host folder,
  the index in the store) and `session::BlobSession` (see SESSIONS.md, "Tier 0").
- **Tables are composites, not one kind** (the kit's `table` module composes them, engine-free:
  the CSV contract's six roots, render rules, key minting, grid ops, the aligner
  `table::reconcile`, the grid three-way merge `table::merge3`, and `table::hidden_cell_writes`):
  - grid structure: two MovableLists (rows, columns), keyed by row and column keys;
  - cell values: a Map keyed by (row key, column key), LWW or Text per cell;
  - formulas: stored as text and **recalculated by the daemon**, never merged as values;
  - merges, styles and named ranges: Maps.

## Rules for every kind

- Every edit names the participant (client ID). Attribution can't be forged.
- Every kind supports versions (whole-doc checkpoints), restore as a forward change, and blame
  at its natural grain.
- Positions never cross the public API as engine offsets. List elements are named by keys
  the caller chooses (they live in the values, so they survive an engine switch), and writes
  by opaque `WriteId`s.
- Every change reports what it did, op by op (`Doc::edit_applied`, the applied stream): for
  text its hunks, for lists and maps each op with its write, live indices and values before
  and after, and its effect (shown, hidden, or none, with the current winner).
- **A merged update reports what it did too** (another replica's changes, applied as bytes):
  `Doc::applied_since(&before)` derives the stream by a keyed diff of the state around it
  (`Doc::keyed_state`, `elem::keyed_diff`; CSV seam design §4.2). Keys are identities, so the
  diff is exact up to the choice of moves (a minimal set); each op's write is the facet's
  winner, `None` where the engine keeps no identity (on Yrs, a restore and a map delete). It
  reports what readers can see: a move of a deleted element, and writes that changed nothing,
  are not in it. For one change delivered alone it equals the author's own stream
  (`stream.merged_matches_local`).

## Value checks belong to the pack (E10)

The kit checks **identities**, never **values**:

- the kit refuses what would break its own model: a list key that isn't 1–24 bytes of
  `[0-9a-z._-]` (`Key`), a map key over 256 bytes or holding NUL (`check_map_key`), a reused
  list key, an anchor next to itself, an unknown key;
- the kit stores **any** `Value` as given: a NUL inside a value, and a value of any size, are
  stored and read back unchanged (`tests/structured.rs`,
  `the_kit_leaves_value_checks_to_the_pack`).

Value rules are the pack's, in its per-op schema check (K-SCH-4; the table layer for the CSV
grid, `table::check_value`): the CSV contract's "NUL is refused" and "one value: 64 KiB"
(exit 2) live there. Per the
engine answer E10, such a check is a **pure function of the op and its causal past**, never
of concurrent state: key format, value size, no NUL, "a set carries the element's own key",
"an insert doesn't reuse a key in its causal past". "The row is live" or "this column exists
here" are not allowed: a cell set into a row deleted concurrently must stay legal (it is
kept, hidden, and reported: `table.cell_vs_row_delete`).

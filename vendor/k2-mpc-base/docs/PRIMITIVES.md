# Primitives

A pack's schema is a set of named roots, each one primitive kind. **Kinds are defined by K2's
merge semantics, written as a conformance suite** (`crates/conformance`, generic over engines) that any
engine must pass. They are not the lowest common denominator of the engines. Where today's
engine falls short, the kit emulates the rule (and the suite proves it), or the deviation is
listed in that engine's `KNOWN_DEVIATIONS` with its reason. A new engine must have an empty
list; the reference model (`mpce-model`, FugueMax) runs every rule with an empty one.

Status: **Text is implemented and has a suite.** The others are specified here and are built,
each with its suite, when the first pack needs them. Declaring an unimplemented kind fails
loudly at `Doc::new`.

| Kind | Model | Edits (data) | K2 semantics (conformance rules) | Notes on Yrs today |
|---|---|---|---|---|
| **Text** | Unicode string | `Edit::Text { root, hunks }` | converge; no interleaving of concurrent runs typed forward or backward; text inserted into a concurrently deleted range survives; a concurrent double delete deletes once; a multi-hunk edit is atomic | Native. **Deviations:** two scripts where YATA's order differs from FugueMax's (`text.yata_separating_case`, `text.yata_paper_a19`; wire-bound) |
| **RichText** | Text plus marks over ranges, plus embedded blocks | text edits; add/remove a mark over a range; insert/remove/move a block | Peritext: **per-mark expand rules set by the pack's schema** (bold expands, a link doesn't); concurrent formatting never diverges; marks survive concurrent text edits | Emulated in the kit (Yrs attributes inherit from the previous char and can diverge, yjs#291) |
| **List** | Ordered sequence of values | insert at, delete range, set item | Converge; concurrent inserts both kept, not interleaved | Native |
| **MovableList** | Ordered sequence with a **true move** | insert, delete, move item, set item | A concurrent move never duplicates or loses an item; the last move wins | Emulated: per-item LWW position (fractional index) on a map, the daemon as authority |
| **Map** | String keys to values | set key, delete key | LWW per key, attributed; nested primitives merge on their own; overwrite history doesn't grow without bound (compaction) | Native; overwrite growth handled by re-snapshotting |
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
- **Tables are composites, not one kind:**
  - grid structure: two MovableLists (rows, columns);
  - cell values: a Map keyed by (row ID, column ID), LWW or Text per cell;
  - formulas: stored as text and **recalculated by the daemon**, never merged as values;
  - merges, styles and named ranges: Maps.

## Rules for every kind

- Every edit names the participant (client ID). Attribution can't be forged.
- Every kind supports versions (whole-doc checkpoints), restore as a forward change, and blame
  at its natural grain.
- Positions never cross the public API as engine offsets.

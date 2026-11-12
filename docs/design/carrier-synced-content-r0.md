# The carrier's synced keys, r0 (design)

Status: **draft**, built on branch `feat/carrier-synced-content` from kit main `44c3c9ff`. Folds in
the live-plans review's three kit blockers (K-B1 a version's keys, K-B2 the held check, K-B3 an
outside save on unwritten changes).

**Goal.** The carrier hook (`Pack::encode_from(carrier, content, codec)`, `docs/SESSIONS.md`,
"The carrier (K1)") gets the file's bytes and the live content. A pack whose keys are the file's
own ids (a deck's shape ids) pairs the two by decoding the carrier. A pack that **mints** its keys
(rows with no ids, timeline items, scene objects) can't: once a live insert or an app's save has
moved the elements, `decode(carrier)` mints other keys than the live doc holds, so the pack can't
tell which file element a live key is.

The first draft handed the hook the whole keyed content as of the sync point, read from the disk
replica. That covers the live doc only: a version's carrier row holds the version's seq, not the
seq its carrier was synced at, and no content is kept per sync point, so `file_at` and the
conflict-copy comparison have nothing to pair with (K-B1). **This design keeps a pairing instead:
the carrier's keys, nothing else, stored with every carrier row.**

---

## 1. The hook: a pairing, not a copy of the content

```rust
/// The keys of the units a carrier holds, root by root, in the order the pack's decode of the
/// carrier yields them. Keys only: never a value.
pub struct Pairing { pub roots: Vec<(String, Vec<Key>)> }

/// Whether the kit keeps a pairing with the carrier (needs wants_carrier). Default false.
fn wants_pairing(&self) -> bool { false }

/// The pairing of `content`, as the file it renders to would hold it. Default: each list
/// root's keys in list order.
fn pairing(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Pairing, PackError>;

/// `encode_from` with the carrier's pairing. Default: `encode_from`, ignoring the pairing.
fn encode_from_paired(&self, carrier: &[u8], pairing: &Pairing,
    content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, SpliceError>;

pub enum SpliceError {
    /// The pairing doesn't fit the carrier (another count of units): nothing was spliced.
    PairingMismatch(String),
    Pack(PackError),
}
```

- A pack with `wants_pairing() == false` (pptx) sees no change: the kit calls `encode_from` as
  today, makes no pairing and stores none.
- A pack with `wants_pairing() == true` gets `encode_from_paired` wherever the kit calls
  `encode_from` today. It decodes the carrier, **takes every value from the carrier**, and puts
  the pairing's keys on its units in order. A pairing whose shape doesn't fit (another number of
  units in a root) is `PairingMismatch`. Then it diffs that keyed carrier against `content` by
  key and splices the diff. The rules of `encode_from` stand: untouched is byte for byte, the
  result decodes to the content, a failure leaves the file.

**Why a pairing and not the content (the review's recommendation, taken):**
1. The values the pack diffs against come from the carrier alone, so they can't drift from it.
   There is no second copy of the file's content to check against the file.
2. It is small and holds no user content (keys only, in the key grammar), so it can be stored
   with **every** carrier row, live and per version, at the cost of a few bytes per unit, and a
   purge has nothing in it to erase.
3. Versions, restarts and N−1 follow the carrier's own lifecycle, row for row: one rule.

## 2. Where the pairing comes from and where it is kept

**Made** at every sync point, from the content the file holds there, by `Pack::pairing`:

| Sync point | The content it is made from |
|---|---|
| A kit write | the content just rendered and written |
| An outside save taken in | the doc once it holds the save (always, under K-B3 below) |
| A change that moved no byte (`resync_replica`: same bytes, new keys) | the doc, which the replica becomes |
| An open whose stored pairing doesn't name the sync point (an older kit's write, no row) | the stored synced state (the disk replica) |

**Kept** in memory beside the carrier (`State::pairing`, with its seq) and in the store, in one
additive table, `carrier_pairings`, created lazily, no format bump: `(doc, slot)` → (the carrier
blob it pairs, the sync seq, the keys). The live doc's slot is `u64::MAX`; a version's is its id.
It follows the carrier rows in the same transactions:

- a new live carrier: the live row, and every version at that seq takes it (as it takes the blob);
- same bytes at a new sync point (`resync_replica`): the live row and the versions at that seq;
- a version cut: takes the live row (the carrier it takes is the live one, from the last sync
  point, so its pairing is that point's: exact for a version cut between two syncs, K-B1);
- a version that goes (retention, purge, sweep, doc delete): its row goes;
- an open: the stored live row is used only if it names the stored carrier blob and the sync
  seq; otherwise it is made again from the stored synced state, and stored.

`version_file` (`file_at`, the conflict-copy comparisons) splices a version into its carrier
with **its own** pairing. A version with a carrier and no pairing, or one that names another
blob, is `MPC-E208`, never paired by guess.

## 3. Exact across every lifecycle event

| Event | Carrier | Pairing |
|---|---|---|
| Import | the file read | the decoded content's keys |
| Kit write | the bytes written | the keys of the content written |
| Outside save | the save | the keys of the doc holding the save |
| A change that moved no byte | unchanged | the doc's keys (new); stored again at the new seq |
| Restart | its blob, if it hashes to the sync point | its row, if it names that blob and seq; else made from the stored synced state |
| Version cut between syncs | the live one (an earlier sync point) | the live one (the same point) |
| Purge | written back first, then the purged doc is the carrier | made at that write; keys are never content |
| Purge with the file missing | stays (reported `NotReached::File`) | stays: it holds no values, so a redacted replica can't make it disagree (the review's hypothesis; a row) |
| Participant erasure | as a purge | as a purge |

**Erasure** reaches nothing new: the pairing holds keys only. Rows of erased versions go with
their carriers. A pack must mint keys that hold no content (`PACK_TEMPLATE.md`).

## 4. Cost

Made once per sync point (O(units), from content the kit already holds at that point), kept in
memory, never per call. An open uses the stored row: no replica decode. N reads make nothing
(test builds count each pairing made, `differential::take_pairings_made`). For a pack without
`wants_pairing`, nothing at all.

## 5. A pairing that doesn't fit

The pack refuses, typed (`SpliceError::PairingMismatch`), and never splices blind; the kit never
falls back to `encode_from` or `encode` (that fallback is the plant `SpliceWithoutMatch`). For the
live doc the rendering fails with **`MPC-E521` `pairing_mismatch`** (exit 1): nothing written,
the file as it was, the doc's changes durable in the store; the next outside save sets carrier
and pairing again. For a version: `MPC-E208`. It means a pack bug or a damaged store.

## 6. K-B2: whether the doc holds an outside save, with no pack call

`holds_file` spliced the live content into **the new save's bytes**, before the replica had taken
the save. For a keyed doc that isn't a table it now compares keyed content: the doc after the
save's edits against the synced state with the save's edits applied (on a throwaway copy of the
replica), key for key. No pack call (plant `HoldsFileCallsPack`). Tables keep their check.

## 7. K-B3: an outside save on unwritten changes keeps both

Today "keep both" fires for a keyed doc that isn't a table only when the kit wrote the file since
the last outside save. A save landing on **unwritten** live changes is merged by key, which can
collide minted keys (the pack mints from the replica), break the pack's own rules in the merged
state, and override a live value with no copy or note.

**One rule now:** for a keyed doc that isn't a table, an outside save that changes the content
keeps both when the kit wrote since the last outside save **or the doc holds changes the file
doesn't** (its version past the sync point, after the usual re-sync check). The kit's version is
kept as a version and a conflict copy, one `BlobConflict`, `DiskCheck::Conflict`; the save becomes
the live doc **exactly**: the doc is moved to the synced state plus the save's edits by identity
(`restore_edits`: live-only units deleted, the save's inserted, values set), so the save is never
merged with unwritten changes. The doc then holds the file, so no merge is written. Plant:
`DirtySaveMergedByKey`. A save whose content equals the synced state (other bytes, same content)
changes nothing, as before.

**What changes for existing rows.**
- Kit `tests/keyed_live.rs`: the two "stale save after a merged write" rows assumed the old merge;
  they become "a save on unwritten changes keeps both" (in memory and across a restart). The
  merge write can't happen for these docs any more, so the plant `MergeWriteNotRecorded` is
  retired; `kit_wrote_file` still reads a stored `kit:merge` author from an older store.
- pptx: none of its rows saves over unwritten changes (its outside-save rows save after a flush
  or before any edit), so none should change; its `check.sh` run confirms.
- Boards (any keyed pack that isn't a table) get the same rule.

## 8. N−1

One additive table, no format bump, registered with the doc tables (delete, rewrite) and the
orphan sweep. An older kit on this store: keeps no pairing; its writes leave the live row naming
another blob or seq, which this kit makes again at the next open; it refuses a rewrite, and so a
purge, naming the table (`MPC-E502`), as for the carrier tables; its doc delete leaves rows the
orphan sweep removes. This kit on an older store: no rows; the open makes the live pairing from
the stored synced state; versions an older kit cut have no pairing, so their `file_at` is
`MPC-E208` for a pack that wants one (as they have no carrier today).

## 9. Plants and rows

Rows on a toy minted-key pack (`tests/live_paired.rs`: rows with no ids, opaque lines, padding
the pack doesn't model), each red on a scaffold commit first:

| Row | Plant that turns it red |
|---|---|
| A live insert, written, then a second edit, written: both splices exact | `SyncedContentStale` (a kit write keeps the pairing from before) |
| An app's save that inserts above a live-edited row, then a live edit: spliced into the right row | |
| A restart, then a verb; N reads after the restart make no pairing | |
| A purge, then a verb; a purge with the file missing, the file back, then a verb; byte scan clean | |
| A version cut between two syncs: `file_at` is the bytes the kit wrote for it | `PairingMissingOnVersion` (a version cut doesn't take the live pairing) |
| A pairing that doesn't fit: refused typed (`MPC-E521`), the file as it was | `SpliceWithoutMatch` (fallback to the carrier-only hook) |
| An outside save is taken in with no pack call for the held check | `HoldsFileCallsPack` |
| A save on unwritten changes keeps both; the save is the live doc exactly | `DirtySaveMergedByKey` |

pptx unchanged and green against the kit by path.

# Version storage that grows with what changed (the third revision, shipped in v0.19.0)

Status: shipped in kit v0.19.0, off by default (`BlobConfig::chunked_versions`; section 10 says
what came first). This third revision replaces the first two (removed once this one became the
design of record; git history keeps them), whose choice was signed off: content-defined
chunks, deduplicated within one doc, reference counts, no deltas. The second revision settled
erasure of digests, the stop between a purge's commit and its removals, the writer and remover
protocol, the manifest's own count, the format number and the durable write cost. This one
settles what segment files opened: a chunk's location lives in its chunk row only; a purge
leaves no dead byte in any segment of the doc; the cheap dedup path is checked against the
connection's rights, never assumed. Measurements come from a prototype on a branch and a
standalone program (cited at the end).

What changed from the second revision: sections 1, 2, 4, 5, 6, 7, 8, 9 and 10. Signed off to
build, with two fixes made at the start of the build (a write equal to an unreadable old
version; a read beside a compaction).

## 1. The shape

- A version (tier 0) or a carrier (live) of a chunking doc is stored as **chunks** cut by
  content (a gear rolling hash with a minimum, an average and a maximum size), kept in
  **segment files** (section 7), and a **manifest**: the version's chunk ids, in order, each
  with its length. **A manifest never names a location.**
- **A chunk's location lives in its chunk row only**: `doc_chunks (doc, chunk id) → (segment,
  offset, length, references, since)`. A read takes the manifest's ids and looks each up in
  the chunk rows. So compaction (section 4) moves a chunk by rewriting its row, never a
  manifest, and a writer can't commit a location that compaction removed: it commits ids,
  and the rows say where they are.
- **Each segment has a row listing everything physically in it**: `doc_segments (doc, segment)
  → [(chunk id, offset, length)]`, every chunk the file holds, a duplicate copy (section 6)
  included, whether a chunk row points at it or not. A range of the file no chunk row points
  at is **dead**.
- **Segment names are a per-doc counter**, never a hash of their bytes (a hash would be one
  more digest to erase).
- Chunks are deduplicated **within one doc only**, and only in the ways section 6 allows.
- A version's **file id** stays `HMAC-SHA256(space key, whole file bytes)`: version rows, blob
  references, conflict copies, `file_at`, host caches and `BlobStore::host_cache_key` keep
  naming it. Only where the bytes live changes.
- Small files (under 1 MiB), named blobs and blobs held by `BlobStore::retain` stay whole-file
  blobs, exactly as today.
- Chunk sizes: a 64 KiB average (16 KiB minimum, 256 KiB maximum) for a file under 64 MiB, a
  256 KiB average (64 KiB, 1 MiB) at 64 MiB and above. The sizes are fixed per doc at its
  first chunked version and kept in each manifest, so they never change under a doc.
- **The doc's lock** in this text is a short internal lock per doc, held from a write's first
  waiting mark to its commit, by a purge, and by every compaction. It is not the
  user-facing tier-0 write lease a person's connection holds while editing: a purge is never
  held off by someone who keeps a lease.

## 2. Erasure of content and of digests

A chunk id and a manifest are digests of the content they name, at a 64 KiB grain: much easier
to confirm a guess against than today's whole-file id. So they are erased as content:

- **A random key per doc.** Each chunking doc has a random 32-byte chunk key in its doc row,
  made at its first chunked version. Chunk ids are `HMAC-SHA256(doc chunk key, chunk bytes)`.
  The key is not derived from the space key, so a deleted doc's ids are unconfirmable once its
  row is gone (in a backup, or a freed page before the rewrite), and a reused doc number can
  never confirm an old doc's ids.
- **A purge erases them.** The purged version's manifest row, the rows of chunks it alone
  referenced, and their bytes in segment files (section 4) go in the purge, as today's purged
  file id is zeroed because "its keyed id is a digest of the content".
- **The segment lists are digests too**: a purged chunk id leaves every segment's list, in
  the purge's transaction (section 4 compacts the segment, so the id's bytes go as well).
- **The rewrite after a purge** copies `doc_chunks`, `doc_manifests` and `doc_segments`
  without the erased rows and list entries, as it copies every other table without purged rows
  today.
- **The byte-scan test** looks for every purged chunk id, raw and hex, and for the purged
  manifest's bytes, in the store file (the segment lists included) and every segment file, as
  it looks for the file id.
- **Erasing a doc** deletes its rows (and with them its chunk key) and its segment folder.
- **Purge and erasure end with a sweep** of the doc's segment folder: a segment no row names
  goes, and so do temp files. Content written for a write that failed or was refused (and so
  never became a version) can't survive a purge by text that can't know it holds it.

What remains, as today: an unlink doesn't scrub disk blocks. File times and a chunk row's
`since` value are metadata: a kept chunk's `since` would show that a region already existed
when a purged version was written, so `since` is reset whenever a version takes a reference.

**"Shared" is redefined.** Today `SharedBlobs(n)` means the purged bytes are in another
version. With chunks, sharing is the normal case: a purge reports `shared` only when the whole
manifest is still referenced (another version has exactly these bytes). Shared chunks are not
"not reached".

## 3. A stop between a purge's commit and its removals

Today a removal at once removes the index row, then the file; open clears only the temp
folder; a file without a row waits for the host's `gc`. With chunks that is hundreds of
removals per purge.

- **The removal list is written inside the purge's transaction**: the segment files to delete
  and the segments to compact (section 4), in a `doc_removals` table.
- After the commit the kit performs the list, then clears it in its own transaction.
- **Every open finishes a list it finds**, before anything else reads or writes the doc
  (store open, then the blob store's open, before any session).
- Row: stop after the purge's commit (a plant), reopen, and the byte scan finds no purged byte,
  id or manifest anywhere.

## 4. Segments, compaction and erasure

**The rule: when a purge ends, every byte of every segment of the doc belongs to a kept chunk.**

Two kinds of bytes are named by no chunk row, so working from chunk rows alone would miss
them: a duplicate copy the equal-work rule writes (section 6), and a dead range retention left
behind when it deferred compaction. A purge by text can't know either holds its text. So:

- **A purge works from the segment lists**, not from chunk rows. It compacts **every segment
  of the doc that holds a purged chunk id or any dead range**: it writes a new segment with the
  segment's kept chunks only (synced, renamed, folder synced), points their chunk rows at it,
  writes the new segment's list, drops the old segment's row, all in the purge's transaction,
  and puts the old segment file on the removal list (section 3). A segment whose every chunk is
  purged or dead is just removed.
- **After a purge, no segment of the doc has a dead range.** Retention may still defer
  compaction while a segment is mostly live (a threshold, proposed 50%), but the next purge
  compacts it whatever its live fraction.
- **Every compaction runs under the doc's lock**: a purge's, retention's deferred one, and any
  merge of small segments. A write never starts a compaction, and none runs on a write's path
  (a write's time would show it).
- **A compaction's output segment is marked waiting** (by its file name, section 5) from its
  creation to the commit that names it, so `gc` never removes it in between.

Rows (to be built with the store code):
- **Duplicates are compacted away.** Write a chunk that exists only in history (so the
  equal-work rule writes a duplicate copy), then purge every version holding that chunk: the
  byte scan finds the chunk's bytes and id nowhere.
- **Dead ranges are compacted away.** Retention drops a version holding a marker string and
  defers compaction; a purge by that text then leaves the marker nowhere (store file, every
  segment file, every removal list).

## 5. The writer and remover protocol

Two facts of today's code force it: each put indexes its blob in its own durable transaction,
which per chunk would be thousands of commits for one version; and `gc` removes a file with no
row at once, with no grace, which would remove a chunked write's files before its commit.

1. **The writer takes the doc's lock** (section 1: internal and short, not the person's lease)
   for the write. A purge, an erasure and every compaction take the same lock, so **nothing is
   waiting on a doc being purged or compacted**, and a purge never leaves "not reached" chunks.
2. **Every chunk id the write will name is marked waiting**, new or already stored, before
   the writer checks whether it exists, and so is **the file name of the segment** it is about
   to write: `gc` can't see inside a rowless file, so the file name is the mark it honours.
   Waiting marks are in memory (per doc, under the blob store's lock, as today's waiting puts),
   refreshed while the write runs, so a long write never lapses.
3. **The write's new bytes go to one new segment** (temp file, write, sync, rename, sync its
   folder; the parent of any folder made is synced too). No row yet.
4. **One transaction**: the manifest row (made, or its count raised: on the cheap path, and
   after the full work for a file equal to an unreadable old version, section 6), each chunk
   row made or its count raised, the segment row with its list, the
   version row, as today's version commits with its blob reference. It **fails closed**: a
   chunk row it expected (one it marked waiting as already stored) that is gone refuses the
   write, and nothing is half-referenced.
5. **The waiting marks are released after the commit** (or the refusal).
6. **`gc` and removal at once honour waiting marks**: a segment whose file name is waiting, or
   a chunk row whose id is waiting, is never removed.
7. **After a restart nothing is waiting**, so every rowless segment file is an orphan and goes
   at once, at open.

Rows and plants (to be built with the store code):
- `gc` during a chunked write: the write commits, and every chunk it names reads back.
- `gc` during a purge's compaction: the output segment survives to its commit.
- A purge of the version a write deduplicates against: held off by the doc's lock until the
  write commits; with the lock plant off, the write fails closed.
- A waiting mark that would lapse during a long write is refreshed (plant: no refresh, and the
  write's segment is removed under it: caught as a refused commit, never a version naming lost
  bytes).
- A stop after each step (temp, rename, folder sync, commit, mark release): the reopened store
  holds the version whole or not at all, and no orphan survives the open.

## 6. What a writer without history access can learn

A K2 share can grant write without history (the host keeps the two rights apart). With chunk
dedup inside a doc, such a writer could test a guess about an **old** version one chunk at a
time: write the guess and watch whether it stored a chunk more (by time, size, quota or a count
in a reply). So the cheap path is narrowed, and **checked, never assumed**.

**How the store knows the connection's rights.** The kit already holds each connection's rights
(`Rights { read, edit, history, owner, ... }`, from `Host::rights(participant, key)` at its
join, refreshed by `Session::recheck_rights`). A chunked write is always made by a session on
behalf of one connection, so the session passes the writer's **cheap set** to the store with the
write: the chunk ids (and manifest ids) the cheap path may use. The store never decides rights;
it only refuses the cheap path for any id outside the set it was given.

**The cheap set** (a reference count raised, no bytes written):
- **A connection with the history right**: the chunks of **every** version of the doc. It can
  read all of them anyway, so a restore of a 300 MB version writes no 300 MB of duplicates.
- **A connection without it**: the chunks of the doc's **current** version, plus a version
  named with `--base` **only when that connection may read that version** (the session checks
  it as it checks any read of that version by that connection). For a carrier write-back, the
  version being edited is the carrier it replaces (the file as it was synced), which every
  writer of the doc can read.
- A write made by the kit itself for no connection (an outside save taken in, a carrier
  write-back after it) uses the empty-history set: the current version or carrier only.

**Everything outside the cheap set gets the full write work**, the same as brand-new content:
- a chunk that exists only in versions the connection can't read is written to the write's
  segment and synced with it, listed in the segment's row, counted in the same transaction
  shape, and the reply and every figure are those of new content;
- **a write whose whole file equals an old version's** outside the cheap set gets the same:
  every chunk and the manifest's own chunks are written to the write's segment and listed in
  its row, then the existing manifest row's count is raised (the table is keyed by doc and
  file id, so there is one row); the copies written are dead ranges that compaction reclaims;
- the kit may keep the extra copy until a compaction reclaims it (section 4), never within the
  write, and **no write starts a compaction**.

**No API, size, quota or usage figure exposes per-version stored bytes, or bytes newly stored by
a write, to a role without history.** Storage statistics stay owner-only, as today.

One existing difference, at whole-file grain and outside this design: a tier-0 version whose
bytes equal an old version's skips the structure index build, a large timing difference. It
exists today; a whole file is hard to guess. The kit's docs say so.

**Rows** (to be built with the store code):
- **Counted work is equal.** A deterministic row in the debug suite writes the same chunk from
  a connection without history: once brand-new, once present only in an older version. It
  counts the work through test-only counters (bytes written to segments, file syncs, folder
  syncs, rows written (made or updated), transaction commits) and asserts the two counts are
  equal, field for field. The same for a whole file equal to an old version's (the manifest
  case). A chunk of the current version takes the cheap path, and its counts differ (allowed).
- **The `--base` check.** A connection without history names an old version as its base: the
  write is refused (it can't read that version), or, where the base is one it may read (its own
  read base), the cheap path is taken; the counts show which.
- **History connections.** A connection with the history right restores a large old version:
  no chunk bytes are written (counted). A restore needs the history right, so a connection
  without it is refused; writing the same bytes, it writes them all.
- **Plants:** a history-only chunk takes the cheap path; the manifest reuse skips the full
  write; the session passes a cheap set that ignores the history right. Each caught by name.
- **Published timing (release build, not an assertion).** On a build node, in a release build,
  many runs (proposed: 2,000 of each, interleaved in random order, checks off) of a write whose
  only difference is a chunk that exists only in history versus a brand-new chunk. Published
  per class: p10, p50, p90 and p99 of the write's wall time, and the overlap as a two-sample
  Kolmogorov-Smirnov D with its p-value. A figure, published with each release that changes
  the write path; not a pass or fail in the suite.

The share test across workspaces is K2's; the kit gives the guarantee and the measurements.

## 7. Durable writes: segments, not one file per chunk

Today's put syncs the file and its folder per blob. Per chunk, that is thousands of syncs for a
first version. Measured on the 300 MB model's size (346,398,283 bytes), durable first write,
`File::sync_all` as the kit's put (F_FULLFSYNC on macOS):

| Where | Chunks | Whole file (today) | One file per chunk, folder synced each | Folders synced once at the end | One segment file |
|---|---|---|---|---|---|
| Build node (Linux, md RAID) | 3,242 (64 KiB avg) | 0.81 s | 1.41 s | 1.19 s | 0.77 s |
| Build node | 811 (256 KiB avg) | 0.81 s | 0.82 s | 0.76 s | 0.76 s |
| Mac (APFS, F_FULLFSYNC) | 3,242 | 0.12 s | 32.93 s | 18.92 s | 0.13 s |
| Mac | 811 | 0.11 s | 8.65 s | 6.38 s | 0.12 s |

The program draws chunk sizes around the average (3,242 chunks at 64 KiB; the real cut of
the model makes 4,609), so per-chunk costs above are understated by about a third. One file per
chunk is up to 270 times today's write on a Mac, and 150 times with each folder synced once.
**So the new chunks of one write go in one segment file**: one write, one sync, one rename, one
folder sync, as today's put. The manifest's entry is the chunk id and its length; the chunk row
holds its segment and offset (section 1). Every folder the kit makes has its parent synced
(today a new folder's parent is never synced: a power loss could keep a commit whose folder
entry is lost).

A version written later holds the few chunks it changed in its own small segment, so a version
read spans several segments; the reader reads ahead (section 9).

## 8. The manifest's own count, and what hangs off the blob row

- **A manifest has its own reference count.** Two versions can hold the same bytes (a restore
  makes one), so `doc_manifests (doc, file id)` counts the versions (and the carrier slots)
  naming it. Purging one of them lowers the count; the manifest and its chunk references go
  only at zero.
- **The manifest is itself chunked** when it is large (a 300 MB version at a 64 KiB average is
  about 4,600 entries): its entries are cut by content and stored as chunks of the same doc, so
  consecutive versions share almost all of it. Measured below: a flat manifest is 162 KiB of a
  300 MB version's 233 KiB at a 64 KiB average; at 256 KiB it is 41 KiB of 306 KiB.
- **Structure index rows** live while their blob row does today. For a chunked version, the
  manifest row plays that part: a structure index row of a chunked version exists while its
  manifest row does, and goes in the transaction that drops the manifest.
- **Reads by space and id.** `BlobStore::reader(space, id)` takes no doc. Every caller already
  knows the doc whose version it reads: `file_at` and `file_at_to` (the session's doc), a
  conflict copy (the doc it is copied from), the purge scan (the doc being purged), a structure
  index build (the version's doc), the carrier load (the session's doc). They move to
  `reader_of(space, doc, id)`; `reader(space, id)` stays for whole-file blobs (a host's named
  blobs, `retain`). **No space-wide table from file id to doc** is added: it would be a
  cross-doc equality index.

## 9. Reads (the first revision's cold numbers corrected)

Measured on the build node with each file's page cache dropped before the read (GNU `dd
iflag=nocache`), 346 MB:

| Layout | Cold | Warm |
|---|---|---|
| Whole file (today) | 292 ms | 142 ms |
| One file per chunk, 64 KiB, in order | 1,365 ms | 47 ms |
| One file per chunk, 64 KiB, four readers ahead | 323 ms | 19 ms |
| One file per chunk, 256 KiB, four readers ahead | 156 ms | 16 ms |
| One segment | 289 ms | 145 ms |

- **A read looks up each chunk's row** for its segment and offset: one indexed read per chunk,
  all in one read transaction.
- **A read never fails because of a compaction or a purge running beside it.** The reader opens
  every segment it needs before it streams (an open file stays readable after an unlink), or,
  if a segment is missing when it opens it, takes the rows again in a new read transaction and
  continues from the chunk it had reached. Where an open file can't be unlinked, the removal
  list keeps the entry and retries it later. Row: a read of a kept version during a compaction
  of its segments returns it whole; never a spurious `file_at` failure.
- **Read-ahead** (the reader opens and reads the next chunks while the caller consumes the
  current one) brings a cold read within 1.1 times the whole file's. With 256 KiB chunks above
  64 MiB, it is faster than today's single read.
- **No kit cache.** Previews and renders are the host's cache, keyed by file id, which stays
  valid. An old version is usually cold, so cold is the case `file_at` is designed for.
- A Mac cold read wasn't measured: dropping a file's cache there needs privileges a build job
  doesn't have. The Mac's warm reads: whole 38 ms, one segment 24 ms, 64 KiB chunks with
  read-ahead 24 ms.
- The first revision's "3.9 s against 0.56 s" was the slowest of fifteen reads with neither
  side's cache dropped: not a cold measurement. Withdrawn.

## 10. The format: 8, a required-features word

Agreed with the engine's owners:

- **Format 8** means the store's meta holds a **required-features word**. A kit refuses a
  store whose word names a feature it doesn't know, by name, before reading anything.
- Features: **`chunked`** (the store holds chunked versions) and **`native`**. `native` is a
  container-level guard ("this store holds engine stores"), not a claim about the engine's own
  features.
- **Format 7 is retired, unwritten.** No shipped host ever turned the native engine on, so no
  format-7 store exists in the field. **This kit refuses a format-7 store** at open, as a format
  it never wrote ("store is format 7, which no kit writes"), before reading anything. Any
  store holding engine stores is format 8 with `native`; a chunked store is format 8 with
  `chunked`; both is format 8 with both. The stamp is raised lazily (the transaction that
  writes the first chunked version, or the first engine store) and never lowered.
- **The engine's open path never reads the kit's word, and the kit never writes or infers the
  engine's own header bits** (its required and optional words in its store header, bits 0 to
  3). **Record 0 stays the only authority on an engine store's contents**; the kit never
  mirrors the engine's required bits.
- **At open, the kit checks its `native` bit against record 0** (the native engine's magic and
  name) and refuses on any mismatch: the bit set with no engine record 0, or an engine
  record 0 without the bit.
- **Chunking never touches engine records** or the bytes the engine's stamped-log MAC covers.
  An engine purge rewrites named records in place, by index, so: the container keeps each
  engine record byte for byte at the same index, with a true record count, and swaps atomically
  at a checkpoint cut (the chained MAC binds bytes, index and record 0). Engine records never
  enter the chunk store; routing them through it is refused.

**What a shipped kit does with format 8 (checked in the code):** v0.14.0 to v0.18.0 read up to
format 7 (`STORE_FORMAT_READ = 7` at each tag) and, at open, refuse a higher format with
`StoreError::Newer` ("store is format 8; this build reads up to 7") from a read transaction,
before any write transaction (v0.14.0 `store/mod.rs:1632-1633`). They refuse to open; they
don't open it read-only (only format 7 itself opens read-only in them).

**Rows:**
- **N-1, both ways:** a format-8 store (chunked; native; both) is refused in one line by the
  previous release, and its bytes are unchanged; this kit opens a format-6 store as before and
  stamps 8 only on its first chunked version.
- **Unknown feature:** a store naming a feature this kit doesn't know is refused by name.
- **Format 7:** a store stamped 7 is refused at open, and its bytes are unchanged.
- **Native bit against record 0:** the bit with no record 0, and record 0 without the bit, are
  each refused at open.
- **Engine records never chunked:** a purge of a native doc leaves no old copy of a rewritten
  record anywhere (store file, segment files, removal lists); an attempt to route engine
  records through the chunk store is refused.

**Built first, with the native check narrowed.** The kit stores no engine records yet, so the
first step of the build has: the word with both bits, an unknown bit refused by name; format 7
refused at open as never written, before the path for a format the kit reads but doesn't write;
N-1 both ways; and for `native` one check: a store that sets it while holding no engine store
is refused (with no engine records stored, that is every store that sets it). The kit reads
format 8 from then on and stamps it only with its first chunked version.

**On the engine switch checklist:** the check of the `native` bit against record 0 (the
native engine's magic and name), both ways, and the row "engine records never chunked" land with the
change that first stores engine records.

## 11. Turning it on: N-1 and rollback

A shipped kit refuses the whole store (it can't know which docs are chunked). The real cost is
rollback: one chunked version, and a host that goes back a release can't open the store at
all, plain text docs included. **The rule: a host turns chunking on
(`BlobConfig::chunked_versions`, off by default) only once its rollback target reads format 8.**
Once on, a carrier write-back of any structured doc over 1 MiB stamps the store, so most
stores are stamped soon.

## 12. Migration

- Existing whole-file versions stay as they are and keep reading as today. Nothing converts on
  its own.
- `BlobStore::rechunk(space, doc)` converts a doc's old versions on request, each in its own
  transaction (segment written, manifest committed, the whole-file reference dropped); the
  dropped whole-file blob is removed at once, not after the grace.
- A way back (`BlobStore::unchunk(space, doc)`: rebuild whole files for a doc) is a later
  maintenance call. The store's stamp stays.

## 13. Zip-based formats and compressed files

- **Zip member boundaries are allowed cut points, not required ones.** A pack may give hints
  (`BlobPack::chunk_hints(bytes) -> Vec<offset>`, on the tier-0 pack, since carriers stay
  whole-file; from the zip's own central directory, not the carrier's pairing). The kit checks the offsets, merges hints closer than the minimum size,
  caps their number, and cuts by content between them; a zip of 100,000 tiny members never
  makes 100,000 chunks. The hook is pure, so boundaries still depend only on the version's
  bytes. Measured on the first revision's deck-shaped series: 0.012 of today per member against
  0.026 without hints.
- **A file compressed whole** (gzip, zstd) shares chunks only up to its first changed byte.
  Nothing here helps it; it costs about today's size plus its manifest, the floor.

## 14. Costs

All series store 101 versions (the first plus 100 edits) unless stated.

| Series | Today | Chunked | Ratio | Per edit, p50 |
|---|---|---|---|---|
| IFC 300 MB model, its single-property edits, 64 KiB avg | 33,365.5 MiB | 353.6 MiB | 0.0106 | 233.1 KiB |
| Same, 256 KiB avg | 33,365.5 MiB | 360.2 MiB | 0.0108 | 305.5 KiB |
| IFC 10 MB model, 64 KiB avg | 1,059.5 MiB | 18.3 MiB | 0.0172 | 76.0 KiB |
| PSD and BLEND samples, synthetic edits (the first revision) | | | 0.010 to 0.025 for 10 to 83 MiB files | 169 to 246 KiB |
| IFC certification scene, one app's re-exports (2x3 to 4, 4 to 4x3, 2x3 to 4x3), 2 versions | 0.2 to 0.3 MiB | the same | 1.0008 each | the whole file |

**Real app saves.** The public corpora hold no two saves of one large file by its app. The
closest pairs are the buildingSMART certification scene exported by one authoring app to IFC
2x3, IFC 4 and IFC 4x3 (92,542, 142,325 and 141,965 bytes): stored as two versions of one file,
each pair shares nothing (1.0008 of today: the second version costs its whole size plus its
manifest). Those files are under the 1 MiB threshold, so they would stay whole anyway, but they
show the floor: an export that rewrites ids and headers throughout dedups nothing. A series of
real incremental saves of a large file (a BLEND or a model saved twice by its app after a small
change) is owed by the packs that can make them before any ratio is promised.

**What a figure promises.** The edit series are the kit's own write-backs (the IFC pack's splice)
and small synthetic edits. An app's own save can change bytes throughout a file (pointers in a
scene file, renumbered ids in a model export, a recompressed file), and then little dedups. The
floor is safe: about today's size plus the manifest. No ratio is promised to K2 from the
synthetic series alone.

## 15. Evidence

- Durable writes and reads: a standalone program with no kit source, run on a Linux build node
  (cold and warm) and on a Mac (warm).
- IFC at 256 KiB, and the app-saved pairs: the kit's prototype measurement beside the IFC pack
  and its public corpus (fetched by its `scripts/fetch-corpus.sh`), on a Linux build node.
- The first revision's series: two runs of the same measurement on a Linux build node.
- Shipped kits on format 8: `STORE_FORMAT_READ = 7` at v0.14.0, v0.15.0, v0.16.0, v0.17.0 and
  v0.18.0;
  the refusal at v0.14.0 `store/mod.rs:1632-1633` and `:1647`.

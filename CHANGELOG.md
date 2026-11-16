# Changelog

## v0.1.0: pairs with kit v0.21.0

The first release of the timeline pack. It builds with **`k2-mpc-base` v0.21.0** checked out
beside this repo (a host vendors both side by side: `vendor/k2-mpc-base` beside
`vendor/k2-mpc-timeline`); `Cargo.lock` and `fuzz/Cargo.lock` name that kit.

- **What a host gets:** `OtioPack`, `FcpxmlPack` and `PrprojPack`, the kit's blob packs for OTIO,
  FCPXML and PRPROJ files: each version is read with exact time into one neutral timeline model
  and indexed into a tree of sequences, tracks, clips and markers, for comments, diffs and blame.
  And `live::TimelineLive`, the kit's live pack for OTIO files, off by default: one timeline as a
  live doc, its tracks as lanes and its items as bars, written back by splicing only what changed
  into the file, every other byte kept. A host opens a file live only when it chooses to.
- **What an agent gets:** on a live OTIO timeline, `timeline info`, `track list`, `clip list`,
  `clip at`, `clip show`, `marker list` and `media list` to read; `clip move`, `clip trim`,
  `clip rename`, `clip enable`, `clip delete`, `clip add`, `marker add`, `marker set`,
  `marker delete` and `track rename` to edit, each at the version the agent read. Each verb's
  result comes as data too (the kit's `mpc.result/1`), and each refusal has a registered id. On
  every format's blob tier, `clip list`, `clip show`, `clip diff` and `clip blame`.
- **What this release leaves out:** FCPXML and PRPROJ files are read and indexed only; nothing
  writes them. The pack handles timeline structure only, never media: no decoding, frames,
  thumbnails or waveforms.

### Exact time

- `time`: `Time` is a reduced `i64 / u64` of seconds, every operation checked. `Rate` is whole,
  NTSC, from a frame duration, or from a floating-point rate only when it names one rate exactly.
  `Timecode` is non-drop or drop-frame, both ways. `parse_time` reads what a verb takes
  (`HH:MM:SS:FF`, `HH:MM:SS;FF`, `123f`, `12.5s`, `1001/24000s`, signed for relative times). A
  time between frames is refused with the frames around it unless the caller snaps.
- **The tick** (`live/ticks.rs`): every live time is a whole number of ticks, 1/254,016,000,000 s,
  stored as a decimal string. Every whole frame rate, the 1001 family and the common sample rates
  are whole ticks. On the public corpus no file is off the grid (0 of 98: `tests/tick_grid.rs`).
- **Drop-frame (DF/NDF)** counts only at a rate with a drop-frame form, `30000/1001` or
  `60000/1001`; at any other rate, or with no rate, a timeline is non-drop. One rule decides it
  for every reader (`model::drop_frame_at`). FCPXML carries it as `tcFormat="DF"`. OTIO's schema
  has no drop-frame field: a timeline's `metadata` may hold a drop-frame (DF/NDF) entry, read
  defensively and never written: exactly `DF` at one of those two rates means drop-frame; `NDF`,
  no entry, lower case, any other value, and `DF` at any other rate mean non-drop, never declined
  and never an error. PRPROJ carries none that the pack reads.

### The model and the readers

- `model`: the neutral timeline model. A document holds timelines and the media they use; a
  timeline holds tracks and markers; a track holds items (clips, gaps, transitions, titles,
  captions, generators, nests) at absolute record times. Every node has a key in the kit's
  address grammar: the format's own ID where it has one, else derived from what the node is
  (never a hash; the table is in the module's documentation), unique in the document.
  Fingerprints digest what the model understands; what it doesn't is carried opaque as digests
  and sizes, and digested separately. `Document::check` checks the invariants; a track converts
  to its sequential form (gaps, items, transitions at cuts) and back.
- **Track order:** every reader puts a timeline's tracks in stacking order, `last_on_top`: a later
  track composites over an earlier one (OTIO's meaning). FCPXML's lanes are read the lowest
  first; PRPROJ's tracks by their own index within each group (`V1` first), whatever order the
  group lists them in.
- `otio`: reads OTIO JSON (`Timeline.1`, or a `SerializableCollection.1` of them) with a bounded
  JSON reader of our own that keeps numbers as written, so times read exactly, and takes the
  non-finite tokens OTIO writes. It reads tracks, clips (`Clip.1` and `Clip.2`), gaps,
  transitions, nested stacks (a stack's clips and gaps as layers), markers and media references;
  metadata, effects and inactive references are carried. Unknown object types and versions,
  missing fields, negative durations and rates it can't name are refused, typed (`ReadError`),
  under `ReadLimits`.
- **XML attributes by their plain name**: the FCPXML and PRPROJ readers read an attribute only
  by its name in no namespace (`xml::PlainAttr`); a namespaced attribute that shares the local
  name (`x:name` beside `name`) is never read in its place, wherever it sits.
- `fcpxml`: reads FCPXML 1.8 to 1.13 with our own types, checked against our own schema
  (`fcpxml::SCHEMA`: the attributes each element needs and the children it may hold); no DTD is
  in the tree or used. Projects and compound clips are timelines; lanes are tracks; story
  elements are placed from their offsets, connected clips and secondary storylines on their
  lanes, transitions on their cuts, auditions by their active pick; markers, to-dos, chapters,
  keywords and ratings in media time. Adjustments, filters, metadata and clip contents are
  carried. A document type with its own declarations is refused before parsing.
- `prproj`: reads PRPROJ files, gzip or plain XML, learnt from the files' own structure and
  permitted samples. The document version (`3`) and each class it reads, at the one class ID and
  version it knows (`prproj::CLASSES`), are checked; a known class at another version, a track
  item class it doesn't know, a reference to no object and an end before a start are refused.
  It reads sequences, video, audio and caption tracks, clips (a nested sequence is a nest),
  transitions on their cuts, sequence markers and the source's markers on each clip, at ticks of
  254,016,000,000 a second. Keys are the file's GUIDs (`@seq:`, `@track:`, `@clip:`, `@mk:`,
  `@media:`), which survive saves; each clip's component chain is carried. The decompressed
  document is held to the size limit.

### The structure index (every format)

- `pack`: `OtioPack`, `FcpxmlPack` and `PrprojPack` are the kit's `BlobPack`s for these files
  (index names `otio`, `fcpxml`, `prproj`; verbs `clip list`, `clip show`, `clip diff`,
  `clip blame`). Each version indexes into a tree: sequences, their tracks, the tracks' items
  (gaps stay in the model) and markers, a nest's tracks under it. Keys are the model's; a
  repeated or over-long derived key is left to the kit, which mints and carries its own.
- Times are exact in the attributes, with the start's timecode when the timeline has a rate
  (drop-frame where the timeline is); media shows its name and file name only; bounds are frames
  across and tracks down; the record and content digests are the model's fingerprint and carried
  pieces. Each sequence says `stacking` = `last_on_top`, `drop_frame`, and `end` (the latest end
  of any item, with `end_from` = `items`); `duration` is the file's stated one when it states one,
  else the end.
- A file a reader refuses is refused to the kit as unsupported (an unknown version or object
  type) or unreadable, with fixed text that quotes nothing from the file. Agents can't write
  these files.

### Live OTIO (structured mode), off by default

`live::TimelineLive` (`crates/timeline/src/live/`), the kit's live `Pack` for OTIO files.

- **The store** (`live/model.rs`): `tracks`, `items`, `markers` and `media`, MovableLists with
  minted keys (`t<n>`, `i<n>`, `k<n>`, `m<n>`, in file order; marks in `timeline` that never go
  down), each with a composite field map. An item's `geom` is one field, `start,dur`, record
  ticks from the timeline's zero; `src` its media time; `track`, `name`, `media`, `off`, `tr` (a
  transition's offsets), `rate` (the rate its times are written at), `edit` (`1`, or `0` for a
  transition, a nest or a retimed clip) and `retimed`. An item's or a marker's identity for
  pairing a save (what it is, its media's name and source in; a marker's owner, name and time) is
  worked out from its live fields when a save is paired, never stored. **The items list's order
  means nothing**: reads and the write-back sort a track's items by start. Gaps aren't stored:
  they are the time between items. Markers carry their owner (an item, a track or the timeline)
  and their time (media time on an item); media their name and file name, never a path. Effects,
  metadata, nests' insides, transitions' parameters and every byte the store doesn't model stay
  in the carrier.
- **What a player reads**, in the live doc's keyed state, which every watcher's snapshot and
  change frames carry:
  - the `timeline` map's `rate`, `tc` (start timecode), `drop_frame` (`1`/`0`), `stacking`
    (`last_on_top`), and `end_from` (`items`): the timeline's end is the latest end (record start
    + duration) of any item on any track, and `timeline info` prints it (`length`,
    `length ticks`); a duration the file stores stays in the file;
  - a track's `enabled` and `muted`, read-only from the file: a track the file disables stays a
    container of the view, never hidden (a player passes over it; how it looks is the host's
    call), and `track list` marks it `disabled`;
  - a media's `start`, its available range's start in ticks, so a clip's place in its media is
    its `src` less it, exactly; a media with no available range has no `start` and its `origin`
    is `unknown` (else `range`); `media list` prints it;
  - an item's `retimed` (`1`/`0`): `1` on an item with its own time effect (a speed change, a
    freeze frame). `clip add` writes `0`; `clip list` marks a retimed item `retimed`.

  No verb writes `stacking`, `drop_frame`, `enabled`, `muted`, `locked`, `edit` or `retimed`, and
  a write-back refuses a doc where one of them was changed.
- **Declines**, typed: `live_many_timelines` (a collection), `live_structure` (a stack holding
  something other than tracks, or trimming itself), `live_time_off_tick`, `live_time_range` (a
  time past an `i64` of ticks, or a track past 2^53 ticks, about 9.85 hours, so the view's record
  geometry stays exact), `live_too_large` (over 32 MiB, 2,500 items, 2,000 markers or 200
  tracks), and the reader's own (`unsupported`, `unreadable`).
- **The write-back** (`live/splice.rs`): `wants_carrier` and `wants_pairing`; `pairing` works the
  read's order out of the content (tracks in order, each track's items by start, markers by
  owner, media by first use). `encode_from_paired` reads the carrier with every value's byte
  range (a span-keeping JSON reader, `json::parse_spanned`), puts the pairing's keys on its units
  (a count that doesn't fit is `SpliceError::PairingMismatch`, the kit's `MPC-E521`), diffs by
  key and rewrites only what changed: a clip's source range values, name and `enabled`; a
  track's children when its items moved, were added or removed (each item's own bytes kept, gaps
  recomputed from the starts, a gap whose neighbours and length held kept byte for byte, one
  whose length changed resized in place, a trailing gap keeping the track's length); markers
  arrays; a track's name. Times are written at the item's own rate with the rate token copied
  byte for byte (`29.97` stays `29.97`) and values in the file's style (`86.0`). Each touched
  item gets our stamp in its metadata, `"mpc": {"key", "doc"}` (the doc id is a digest of the
  file as first read). Untouched is the carrier byte for byte; the result is read back and must
  hold the live content (the read-back parses and walks the whole result and compares the units
  the splice touched). `encode` refuses: a timeline isn't built from the doc alone.
- **Outside saves** (`reconcile`): an item pairs by our stamp only when its doc id is this doc's
  and exactly one item of the save claims the key (a copied clip's stamp, or another doc's,
  pairs nothing), else by its derived identity when exactly one item on each side has it, else it
  is new, keyed past the marks. Two items of one identity (the same shot used twice) pair by
  their stamps or not at all. Media pair by a digest of their location, markers by owner, name
  and time.
- **Tracks pair by name and kind** (`live::pair_tracks`, which `reconcile` goes through):
  1. a track whose name and kind (`video`, `audio`, `other`) are those of exactly one track in
     the doc and one in the save pairs with it, wherever either sits in the stack;
  2. where the name can't decide (no name, or two or more tracks of that name and kind on either
     side), the tracks of that name and kind pair in their order, bottom first; the fallback
     never reaches across names or kinds;
  3. every other track of the save is new, under a key minted past the marks, and a track of the
     doc paired with none goes. A track renamed in another app is a new track and its old key
     goes; a rename made live (`track rename`) keeps its key.

  So a save that adds a video track at the bottom keeps every track's key and contents, and a
  claim held on a track before the save is on the same track after it. A claim on a track renamed
  in another app ends `gone_outside_save` (the kit's `ClaimEnd::GoneOutsideSave`), naming the
  track as it was (`@trk:N`) on its end events.
- **The view** (`live/view.rs`): tracks are the containers (name as title), items the units
  (kind, name, `off` as hidden, `edit` as editable text), geometry `geom` as `[X, W]` in ticks.
  Addresses `@trk:N`, `@item:N`, `@mk:N`, `@med:N` (the blob tier's are the model's keys). The
  view schema declares `time_unit` `tick`, so a client knows what a playhead's position counts
  in.
- **Derived values** (the kit's derived values per unit and per container), on the view index, a
  watcher's snapshot and each change frame, for only the units and tracks a change moved:
  - each unit's `tc`, its record start at the timeline's rate counted from the start timecode,
    `HH:MM:SS:FF` (drop-frame `HH:MM:SS;FF`): a clip one second into a 24 fps timeline that
    starts at 01:00:00:00 reads `01:00:01:00`. A unit off the rate's frame grid, or a timeline
    with no rate or no start timecode, has none. Each unit's `geom` page carries its span
    (`HH:MM:SS:FF-HH:MM:SS:FF`);
  - each track's `kind_label` (`V1`, `A2`: its kind's letter and its place among the tracks of
    its kind, counted from the bottom; the label `track list` prints, never the track's name) and
    `kind` (`video`, `audio` or `other`).
- **The track index** (`live/index.rs`): the verbs' verb state, kept by the kit beside the live
  doc. Each track's live items ordered by (start or a transition's cut, transition first, key),
  each item's kind, place, rate and media, and every key the items list ever held; updated from
  each change's ops in O(log n) per op. Every verb reads it: the overlap check, the neighbours a
  gap is written between, a transition on a cut, `clip list` and `clip at`, a media's users, the
  next key.

### The agent's verbs

- **Reads:** `timeline info`, `track list`, `clip list` (a track's items in time order),
  `clip at`, `clip show`, `marker list`, `media list` (pages of 200).
- **Edits:** `clip move` (one field set, and `track` with `--track`), `clip trim`,
  `clip rename`, `clip enable`, `clip delete` (a lift, with its markers, and its media when
  nothing else uses it), `clip add`, `marker add`, `marker set`, `marker delete`,
  `track rename`. Times are the timeline's timecode, frames or seconds, or relative; one between
  frames is refused unless `--snap`. Each edit takes the agent's base.
- **Refusals are registered** (`live::errors::ERRORS`, the kit's `Pack::errors`), each with a
  stable id, `TIMELINE-OTIO-E001` to `E011`, its message and what to run next: `tl_overlap`,
  `tl_off_grid`, `tl_time`, `tl_transition`, `tl_track_kind`, `tl_not_editable`,
  `tl_track_not_editable`, `tl_media_range`, `tl_range`, `tl_rate` (a source in, a duration or a
  gap that the clip's own rate, or its neighbours', can't write exactly) and `tl_not_found` (an
  address that names nothing live, exit 4). Each prints `refused: <code>: <why>` and
  `next: <call>`; the host gets the entry's report, with the details `code`, `next` and `verb`.
- **Results as data:** with `--json` (or the host's choice), each verb's call returns the kit's
  result object (`mpc.result/1`), whose `data` is the verb's own payload (`data_schema`
  `typed`, `data_spec` `timeline.<verb>/1`, as `timeline.clip-list/1`): a time as `{shown,
  ticks}`, an item as `{address, kind, track, start, duration, name, off, editable, retimed}`,
  each list's rows, and each edit's element and new values; a list's next page is its
  `page.next`. The text form is the same either way.
- **The agent's playhead:** each edit leaves the agent's playhead (the kit's time position
  presence) at its edit point, in record ticks, paused, in the track it worked on (`@trk:N`):
  `clip move` at the new start; `clip trim` at the new start with `--in`, at the new end with
  `--out` alone; `clip add` at its start; `clip delete` where the item started; `clip rename` and
  `clip enable` at the item's start; `marker add`, `marker set` and `marker delete` at the
  marker's record time. `track rename` and the reads move nothing. A person's playhead is theirs:
  no verb moves it. A refused verb, a missing element and a usage error move nothing. The table at
  the top of `live/verbs.rs` lists each.
- **Claims:** an agent's claim with `hold` holds other agents off what it holds: another agent's
  edit of it is refused with the kit's `MPC-E301`, naming the holder and the claim.

### Tests

- **Rows** (`tests/live.rs`, `tests/live_support/rows.rs`): the store with every field, the view,
  the declines, bounded reads; a move's one change event and the watcher, what is refused, time
  order on read, keys minted past the marks; untouched files byte for byte (the kit's
  `carrier_judge`), a move rewriting only its gaps, our stamp pairing a re-save, a copied stamp
  pairing neither, the rate token kept, a time of day past 2^53 ticks through the store, the wire
  and a write-back, a pairing that doesn't fit; outside saves taken in by key and kept beside;
  track pairing by name and kind; a restart, a delete with its markers and media, the
  differential oracle against the blob tier's model. The tests run with the kit's `differential`
  checks, which compare the kept track index with a fresh build after every change and at every
  hand-out.
- **Golden view snapshots** (`tests/kit_view.rs`, `tests/kit_view_golden/`): the kit's view
  index of the rows' timeline, with its markers and keyed state, pinned whole in the kit's own
  pretty JSON: `cut` as it opens, `cut-edited` after an agent's move, trim, lift, add, enable
  (`--off`) and marker add, `cut-trim-in` after a trim of a clip's start with the change frame a
  keyed watch gets for it.
- **A watched turn** (`tests/watch_turn.rs`,
  `tests/watch_turn_golden/trim_held_outside_save.json`): an agent holds a clip, an app's save
  renaming another clip is taken in by key, the agent trims, moves, renames, turns off and marks
  the clip it holds, a second agent's trim of it is refused, the edits are written back and the
  hold is let go. Per step it pins the verb's arguments and exit, every keyed watch frame a
  watching person gets (`ops_change` with the claim, `verb_refused` with the holder and claim,
  `outside_save`) and every roster event. Client IDs and claim ids are numbered in the order they
  appear and the host's clock is fixed, so two runs are byte-identical.
- **Each verb's result** (`tests/results.rs`, `tests/results_golden/`): the result of every
  verb, of a list's first page, and of a refusal of eight codes, each on a fresh copy of the
  rows' timeline, pinned whole; every code the verbs print is a registered entry, and the kit's
  registry takes the pack.
- Any golden difference fails, naming each path; `MPC_BLESS_GOLDEN=1` re-blesses a change made on
  purpose.
- **Planted bugs** (`tests/mutants.rs`, feature `mutants`, test-only), each caught by its row,
  among them `TickRounded`, `ReadUnbounded`, `TicksAsDouble`, `OverlapAllowed`, `OffGridSnapped`,
  `MintBelowMark`, `ListOrderTrusted`, `EncodeReserialises`, `GapNotRebuilt`, `KeyNotInMetadata`,
  `StampFirstWins`, `RateTokenRewritten`, `PairByPosition`, `RateGridIgnored`,
  `IdentityFirstWins`, `ReadBackSkipped`, `DiffStaleUnit`, `IndexKeptStale`,
  `TurnRefusalDropped` and `AttrByLocalName` (an attribute looked up by its local name, which a
  namespaced attribute can answer).
- **The public corpus:** 98 files fetched by reference (`tests/corpus/manifest.tsv`: source at a
  fixed commit, licence, sha256; never committed): OTIO's own sample data and its FCPXML
  adapter's (Apache-2.0), and a third-party reader's generated PRPROJ sample projects (MIT).
  `scripts/public-corpus.sh [DIR]` fetches them (`scripts/fetch-corpus.sh`) and runs the tests
  that need them, each by name; without the files those tests fail, so the gate leaves them out
  (`#[ignore]`d). `tests/corpus.rs` reads each, runs the fuzz properties and the index on it, and
  compares what it holds with an independent count by `scripts/corpus-expected.py`, a Python test
  checker that shares no code with the readers and is never shipped. Every public OTIO file opens
  live and is byte exact through the kit.
- `fuzz/`: cargo-fuzz targets (`publish = false`, never shipped) on the properties in
  `k2_mpc_timeline::fuzzing` (feature `fuzzing`, test-only): a reader refuses or returns a model
  that reads the same twice, passes the checks, has keys in the kit's grammar and round-trips its
  tracks. The seed tests run the same properties on every truncation and one-byte change of each
  seed.
- **The target scan:** `scripts/targets.txt` lists every test target of the pack and its fuzz
  crate, every feature of their manifests, and the gate part that runs each, or why none does.
  `scripts/check-targets.sh` reads the manifests, the sources and `scripts/check.sh`'s commands,
  with no build, and fails on a target or feature with no row, a row with no target, a part that
  doesn't run what its row says, a test-only feature no part turns on, and an `#[ignore]` whose
  reason names no runner. `scripts/test-check-targets.sh` plants one case of each in a copy of
  the tree. A tree without `fuzz/` leaves the fuzz crate's rows unchecked.
- **Measurements** (`tests/live_scale.rs`, `#[ignore]`d; `scripts/measure.sh` runs them in a
  release build): a timeline of 2,000 items (`SCALE_ITEMS` sets another count), 600 markers and
  300 media, and the heap after 10,000 sets of one clip's geometry. In a 4-CPU release build: 10
  tracks and 2,000 items, a 6,558,935-byte file; open 438.7 ms, heap 40.1 MiB live and 57.6 MiB at
  peak; `clip move` p50 0.332 ms and p99 0.476 ms, `clip trim` 0.332 and 0.472 ms, `clip add`
  0.640 and 2.620 ms, `clip delete` 0.674 and 0.877 ms (to the watcher's event); a `clip list`
  page of 200, p50 0.518 ms and p99 0.672 ms; the view index first 29.20 ms, unchanged 0.647 ms,
  after a move 2.54 ms; the carrier's read 85.2 ms, the splice of 1, 10 and 50 moved clips p50
  124.2, 124.0 and 125.4 ms; 10,000 sets of one clip's geometry in 3.4 s, the heap grown
  0.00 MiB. No debug suite asserts a time.

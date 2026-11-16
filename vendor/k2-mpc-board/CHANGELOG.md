# Changelog

## v0.1.15

Pairs with kit v0.21.0 (`9e6f6419`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's pack API additions have defaults (`Pack::errors`,
`Pack::check_merged_view`, `BlobPack::merge3_known`, `BlobPack::save_lineage`), and its other
changes are the host's (the version an app opened, a call's result as data, kept pairs and claim
bytes on events); the bridge uses none of them.

- The board crate and the bridge 0.1.15 (`Cargo.lock`, `bridge/Cargo.lock`, `fuzz/Cargo.lock`);
  `bridge/Cargo.lock`: k2-mpc-base 0.21.0 (only the kit's crate version moves).
  `bridge/Cargo.toml`'s pin comment and `bridge/README.md`'s kit line name v0.21.0 and its cut
  `9e6f6419`.

## v0.1.14

Pairs with kit v0.20.0 (`475a1755`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes are the host's (a store holding native-engine docs
opens and refuses only those docs; writer IDs that follow people, devices and agents; a writers cap
per doc, off by default, with a near-cap audit notice); the bridge uses none of them.

- **The target scan** (tooling; no library change): `scripts/targets.txt` lists every test target of
  the board crate, the bridge and the fuzz crate (lib unit tests, doc examples, integration tests,
  examples, fuzz targets), every feature of their manifests, and the gate part that runs each, or
  why none does. `scripts/check-targets.sh` reads the manifests, the sources and
  `scripts/check.sh`'s commands, with no build, and fails on a target or feature with no row, a row
  with no target, a part that doesn't run what its row says, a test-only feature no part turns on,
  and an `#[ignore]` whose reason names no runner; `CHECK_TARGETS_LANDING=1` also refuses a row
  waiting on a ruling. `scripts/test-check-targets.sh` plants one case of each in a copy of the
  tree. Both run first in `scripts/check.sh`. The kit's `tools/check-targets.sh`, with the pack's
  gate shape. It prints the md5 of its shared copy (the script but for its two settings) and of the
  self-test, the same in every pack, so a drift shows.
- `scripts/check.sh` runs the board crate's doc example (`cargo test --doc`), and checks the
  fuzz crate's lib and targets under the fuzzing cfg at its locked dependencies
  (`cargo check --cfg fuzzing`, a target dir of its own).
- The board crate and the bridge 0.1.14 (`Cargo.lock`, `bridge/Cargo.lock`, `fuzz/Cargo.lock`);
  `bridge/Cargo.lock`: k2-mpc-base 0.20.0 (only the kit's crate version moves).
  `bridge/Cargo.toml`'s pin comment and `bridge/README.md`'s kit line name v0.20.0 and its cut
  `475a1755`.

## v0.1.13

Pairs with kit v0.19.0 (`b8db699e`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes for a pack are additive with defaults (container
parents and derived values, the view schema's nesting and time unit, a claim's element addresses
at its end, presence view address and time position, two keyed-watch frames, chunked version
storage off by default); the bridge uses none of them. Public text: the reaction test names its
people by const, and `scripts/check-labels.sh` also blocks parity-row, finding and run labels,
branch names and engine codes.

## v0.1.12 (2026-11-12)

Pairs with kit v0.18.0 (`efc6377e`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes for a pack are additive with defaults (the view
index's structs built with `..X::default()`, a per-pack keyed limit, verb state, derived values per
unit, placements in 3D), and a keyed doc's `MPC-E822` is `SessionError::KeyedAtLimit`; the bridge
uses none of them. Public text in plain words: internal labels in docs, comments and test names are
now plain descriptions, `scripts/check-labels.sh` (run by `scripts/check.sh`) fails on one, and the
CHANGELOG's lines are wrapped to at most 100 characters, words unchanged.

## v0.1.11 (2026-11-12)

Pairs with kit v0.17.0 (`0a78505a`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes are text → tier 0 (`demote_text_file`), the
carrier's pairing (opt-in), the live grid (additive) and an outside save on unwritten changes
keeping both on a keyed doc that isn't a table; the bridge keeps no carrier, pairs nothing and
declares no grid, and a live board's outside save on unwritten changes now keeps both (the kit's
rule). One new error id (`MPC-E521`, for a pack that pairs); wire still 3.

## v0.1.10 (2026-11-12)

Pairs with kit v0.16.0 (`d2245444`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes are additive (the view schema, addresses and view
index, the keyed watch behind the feature `keyed_watch` with view deltas, roster addresses, the
structure index's row format 2); the pack hooks it adds are provided methods with defaults, and the
bridge uses none of them. No new error id; wire still 3.

## v0.1.9 (2026-11-12)

Pairs with kit v0.15.1 (`e31ed4d9`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's changes are speed on keyed docs that aren't tables and the
pack contract's carrier row for a pack that keeps a carrier; no kit API, wire or error id changed,
and the bridge keeps no carrier.

## v0.1.8 (2026-11-11)

Pairs with kit v0.15.0 (`07a2fa63`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's additions are additive or provided methods with defaults
(turns, the pack verb on a change, focus and range, `Pack::container_of`, the carrier hook, host
cache keys), and the bridge uses none of them.

## v0.1.7 (2026-11-11)

Pairs with kit v0.14.0 (`30ddf8fc`, the public cut). The bridge's lock and kit pin text move to it;
the pack's code doesn't change. The kit's additions are provided methods with defaults
(`Pack::check_merged`, `Host::conflict_copy_name`) and person-writer reuse, off by default, and the
bridge uses none of them. The kit's tier-0 change (an app's save over an agent's write keeps both,
with a conflict copy) reaches a board file a host serves through the kit's blob sessions, with no
change here. `scripts/private-allow.txt`'s header says what `scripts/check-private.sh` skips (only
the allowlist itself), and v0.1.6's line names the kit tag alone.

## v0.1.6 (2026-11-10)

Pairs with kit v0.13.1 (no kit API change since v0.13.0). The bridge's lock and kit pin text move to
it; the pack's code doesn't change. `scripts/check-private.sh` is the kit's (k2-mpc-base
`tools/check-private.sh`): its patterns are no longer in the tree. It reads them from
`MPC_PRIVATE_PATTERNS`, else the file `MPC_PRIVATE_PATTERNS_FILE` names, else
`~/.config/k2-mpc/private-patterns.txt`, and with none it exits 2; `scripts/private-allow.txt` works
as before.

## v0.1.5 (2026-11-10)

Pairs with kit v0.13.0 (`8bcbc39b`, the public cut). The bridge's lock moves to it; the pack's code
doesn't change. The kit's additions (the structure index, anchors, comment threads and claims by
node, opt-in per pack and per host) are additive, and the bridge uses none of them, so nothing here
changes.

## v0.1.4 (2026-11-07)

Pairs with kit v0.12.0 (`178b449e`, the public cut). The bridge's lock moves to it; the pack's code
doesn't change. The bridge builds and reads no kit `LogEntry` and holds no kit grid, so the kit's
breaking change (`LogEntry.body: EntryBody`) and its grid row limit don't reach it.

## v0.1.3 (2026-11-04)

Pairs with kit v0.11.1 (`d9927a6e`, the public cut). The bridge's lock moves to it; the pack's code
doesn't change.

## v0.1.2 (2026-11-04)

The bridge: the board's file verbs on the multiplayer kit's verb features, so a host serves board
files with the same habits as its other docs. The format doesn't change.

- `bridge/` (`k2-mpc-board-bridge`): its own workspace, never a member of the repo's, with the board
  crate and the kit (`k2-mpc-base` v0.11.0, its checkout beside this repo's) as path dependencies.
  The board crate and the repo's lock stay free of the kit, and `scripts/check-no-kit.sh` holds
  both; the licence, private-name, host-form, Windows-name and credit checks cover `bridge/` and its
  lock.
- Stable error ids: each refusal the board crate gives is one of the kit's rows, `MPC-E830` to
  `MPC-E859`, by the call it came from: a file read, the caller's input, `apply`'s raw ops, or a
  write. A format refusal's exact code goes in `details.code`. Every report sets each key its row's
  message names, from the refusal's own values. A conflict the board crate never gives, a change to
  a person or an add of the settings, is a `HostBug`, never a row.
- `value_out`: the one conversion of board values, for reports and the read verbs' JSON alike. A
  number is always a float, and -0.0 is 0, as the file writes it, so a report's values match
  `read`'s.
- `parse`: a host's argv to a call, built on `verbs::ARGS` and the schema, with a host's own
  spellings as data (`Names`). An unknown verb or flag is the kit's `MPC-E120`, a missing or bad
  argument `MPC-E122`.
- `dry_run::run`: open, read the guard apart, plan, apply, save. `dry_run::dry_run`: the same with
  the save into nowhere, every refusal shown with its row and exit, and the answer says so. `json`:
  a refusal is the kit's report with the row's `code` and the echo; an answer is the echo and the
  value. `registry`: the board's file verbs as the kit's file registry, for `capabilities` and
  `explain`.
- The board crate:
  - `VerbError::Refused` gains `facts: Box<Facts>`, what the refusal is about as data, and
    `VerbError` is `#[non_exhaustive]`. **A `match` on `VerbError::Refused { code, detail }` without
    `..` must add it, and a `match` on `VerbError` needs a wildcard arm.**
  - `verbs::JSON_ARGS`, the arguments with no text form, with why. `ALL`, `as_str` and `parse` on
    `AlignTo`, `Axis` and `Side`. `Object::from_value`, and `Removed::to_value` and `from_value`.
    `verbs::settings_flags`, the settings' counterpart of `verbs::flags`, each setting read by its
    own kind.
  - `GROUP_REFUSALS`: every code `apply_group` can refuse a group with, held to the code by
    `tests/group_refusals.rs`, one group each, and by the fuzz oracle.
  - The views' JSON trees keep a -0.0 as it is in memory, and `json::to_canonical` writes it as 0,
    as the file does (docs).
- `fuzz/`: the raw-ops and verbs oracle holds every refused group's code to `GROUP_REFUSALS`, and
  fails on a change to a person or an add of the settings.
- The sweep's tools: a catch is read against the mutated file's own text (`catch.run_lines`), and a
  sweep refuses to start with a mutant that changes the line count of a Rust file holding test code
  (`catch.moves_test_lines`). A sweep ends by its verdict's exit, 3 when no mutant ran and 4 when
  one needs a look, and prints `mutants finished` only at 0; it writes no bytecode. `catch.py` reads
  `bridge/` paths. The sweep is in three parts.
- The mutant sweep at this release, `e2314b0`, under the written catch rule, in three parts that add
  up to the whole of 1003 with no overlap: `@part1` (run `0f1e1682`) `total=368 caught=365
  survived=0 equivalent=2 stale=0 build_failed=0 no_assert=0 aborts_expected=1`; `@part2` (run
  `3cf4b9ba`) `total=380 caught=380 survived=0 equivalent=0 stale=0 build_failed=0 no_assert=0
  aborts_expected=0`; `@part3` (run `36b0bdf1`) `total=255 caught=255 survived=0 equivalent=0
  stale=0 build_failed=0 no_assert=0 aborts_expected=0`. In all: `total=1003 caught=1000 survived=0
  equivalent=2 stale=0 build_failed=0 no_assert=0 aborts_expected=1`.

## v0.1.1 (2026-11-04)

The header read, for a host's listing of many files. The format doesn't change. The bridge comes in
v0.1.2.

- `read_header`: the central directory and the manifest, with the checks an open makes before
  `board.json`: the versions, the guard, the parts, the preview's size. It never reads `board.json`.
- `read_preview`: the preview, held to a bound the host gives and to its hash.
- `read_summary`: the number of objects and the title, from one streaming pass over `board.json` in
  bounded chunks, its hash checked against the manifest, under the format's bounds.
- None is an open: each gives `checked: false`, and a full open may still refuse the file. A host
  keys its cache by path and guard, and takes last-changed from the file's own time.
  `docs/HEADER.md` says what each reads and what it doesn't check, and what each costs.
- `vectors/zip/manifest-other-board`: one board's manifest on another's `board.json`. No other
  vector changes.
- A zip entry streams to a sink, held to its declared size, the ratio and its CRC as it goes; an
  open and a header read share the checks made before `board.json`. Neither changes what an open
  accepts or refuses.
- `fuzz/`: a `header` target. `tools/sweep.py`: a `@header` list.
- The mutant sweep at this release, `0a6291e`, under the written catch rule, in two parts that add
  up to the whole of 748 with no overlap: `@part1` (run `876554f6`) `total=368 caught=365 survived=0
  equivalent=2 stale=0 build_failed=0 no_assert=0 aborts_expected=1`; `@part2` (run `09aaea29`)
  `total=380 caught=380 survived=0 equivalent=0 stale=0 build_failed=0 no_assert=0
  aborts_expected=0`. In all: `total=748 caught=745 survived=0 equivalent=2 stale=0 build_failed=0
  no_assert=0 aborts_expected=1`.

## v0.1.0 (2026-11-04)

Pack v0.1. The format doesn't change, apart from test data for a converter in the `widget` vectors.
The header read and the bridge come in v0.1.1.

- `BoardFile`: open a file, apply groups, and save behind the guard. An unchanged board writes
  nothing, and a group that doesn't fit is held with its ops kept.
- `BoardFile` opens the `FileTarget` the host names, and every answer and error is `Echoed` with its
  kind, the path as given, the guard and the mode. An empty target is refused, and live is never
  implied.
- `verbs`: what an agent can do with a file, with no live session. Reading, editing, arranging,
  copying, templates and raw ops as JSON, each planned as a `Group` that `apply_group` applies, so a
  live session can run the same code. `inverse` undoes a group. `docs/VERBS.md` has them all.
- `parity`: everything a person can do and how an agent does it, with a test that runs each both
  ways. `docs/PARITY.md` is generated from it.
- `merge3` and `BoardFile::merge`: a whole-object merge. One writer at a time, with no presence. An
  object changed on both sides is a typed conflict with every side kept, and it's never
  last-writer-wins.
- A merge's error keeps the first `MERGE_CONFLICTS_MAX` (256) conflicts with every side, in a stated
  order, and counts every conflict by kind, with `truncated` when the lists stop short. Every object
  of the largest board in conflict merges in about 340 ms, and the process peaks at 0.94 GiB, the
  three boards themselves.
- `vectors/fidelity`: a corpus of what's easy to lose, in Rust and JavaScript, and through a file,
  field by field and verb by verb.
- `fuzz/`: targets for the decoders, raw ops and verbs. `tools/sweep.py`: the mutant sweep.
- The sweep's catch rule is written down, in `tools/catch.py` with its self-test, and
  `tools/README.md` gives it. A catch counts in test code, or in a unit test's module in library
  source, at an assert macro, an `expect_err` or a `panic!` with a string literal, or an `expect`
  whose literal starts `check:`, the test's own stated check that a call succeeds. In
  `tests/support/` only an assert or a `check:` message counts. A script's or the JavaScript
  checker's `FAILED` line counts when it names its case. Each `CAUGHT` line names its kind.
- Read under that rule, the format's freeze sweep of 341 (run `a64c0005`) has 23 of its 338 catches
  that don't count, judged by each one's first failing line, since its log holds no others. The
  648-mutant sweep at `33b32b6` has 33 of its 645 that don't count, judged by every failing line,
  and the verb runs after it 5 more. A frozen commit's tests can't change, so the tests change in
  this tree: an assert that states what each of those mutants breaks, or `check:` where the call
  succeeding is the test's own check. Rerun by name at `99633e2`, all of them are caught at a line
  that counts: none of the freeze's 23 and none of the later 38 is left, `file-merge-keeps-listing`
  having been caught at an assert since `cee72a3`. The full sweep at the tag gives the whole count.
- The templates, the verbs' rules and the parity list are ported from Board, and the widget pairs
  describe its store's shapes: see `THIRD-PARTY-NOTICES.md`. `scripts/check-credit.sh`, run by
  `scripts/check.sh`, holds every ported file to a credit in its header and every file generated
  from ported data to the credit, and refuses a file that names Board as a source without being on
  its list.
- The mutant sweep at this release, `ef0678e`, under the written catch rule, in two parts that add
  up to the whole of 720 with no overlap: `@part1` (run `f9134f27`) `total=367 caught=364 survived=0
  equivalent=2 stale=0 build_failed=0 no_assert=0 aborts_expected=1`; `@part2` (run `9d23db32`)
  `total=353 caught=353 survived=0 equivalent=0 stale=0 build_failed=0 no_assert=0
  aborts_expected=0`. In all: `total=720 caught=717 survived=0 equivalent=2 stale=0 build_failed=0
  no_assert=0 aborts_expected=1`.

## Gaps found after the tag

- The format and code were right. The frozen vectors didn't hold the whole-number half of
  `perPerson` alone: `refused-per-half` holds it now, with the mutant `voting-per-whole`.

## format-v1.0

Format v1.

- `docs/FORMAT.md`: the spec. `schema/board-schema.json`: its fields, limits and refusal codes as
  data.
- `vectors/`: conformance vectors, with `vectors/check.mjs` for the layers JavaScript can run.
- `crates/board` (`k2-mpc-board`): the model, a strict zip reader and writer, canonical JSON, limits
  held before and while inflating, typed refusals, a save guard over the canonical manifest, object
  and settings revisions, and `apply_group`, which applies a group of edits all or nothing.
- `docs/FREEZE-v1.md`: the evidence for this freeze.


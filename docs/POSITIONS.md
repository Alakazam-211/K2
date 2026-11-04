# Positions

How each surface of the kit counts positions (K-ADDR-4). In one line: **UTF-8 bytes inside
the kit and on the hunk channel; `line:col` (1-based lines, 1-based columns in Unicode scalar
values) for people and command-line verbs; UTF-16 code units only inside the update bytes
exchanged with Yjs clients.** What an engine counts internally (element clocks, provenance) is
not part of this contract.

`capabilities` states this as data: its `positions` key (`--json`), or the "Positions"
section of its text form. The rows for the kit's verbs and its grid verbs come from one table,
`crates/base/src/parity/positions.rs` (`VERB_POSITIONS`), and a pack verb's from its own
declaration (the "Pack verbs" note below). `crates/base/tests/positions.rs` checks them: every
verb whose usage names a position has a row, every row's flags are in that verb's usage, and each
unit is the one the verbs actually use.

| Surface | Unit (`capabilities` word) | Base | Where it is set and checked |
|---|---|---|---|
| Kit API: edit hunks (`Edit::Text`, `text::diff::Hunk`), carets (`Cursor`, `At::Byte`), claims, views' spans, `ChangeEvent` hunks | UTF-8 byte offsets of the root's doc text, on character boundaries (`utf8_bytes`) | 0 | `session/position.rs`:3-5; `text::diff::validate` refuses an offset inside a character |
| `line:col` (`insert --at`, `replace`/`delete --range`, `cursor --at`/`--select`; `At::Line`; `CursorAt.anchor`/`head`) | 1-based line; 1-based column counted in Unicode scalar values (`line_col`) | 1 | `session/position.rs`:57-63 (`LineCol`), 79 (`line_col`), 112 (`offset`); `tools/edit.rs`:9-11, 108-117 |
| Line tools and line forms (`cat -n`, `head`/`tail -n`, `view --lines`, `grep -n`, `sed` addresses, `insert --line`/`--after`, `replace`/`delete --lines`, `claim --lines`, `blame -L` on text, a diff's hunk headers) | lines of the doc text (on a grid, of its rendered file), `\n` ending a line; ranges inclusive, `$` the last (`lines`) | 1 | `tools/edit.rs`:3-7; `tools/read.rs`:3-5; on a grid, "A grid's two line counts" below |
| Byte tools (`cat` with no flags, `head -c`, `tail -c`, `wc -c`) | bytes of the file as the pack encodes it, BOM and CRLF included (`file_bytes`) | counts | `tools/mod.rs`:31-33; `tools/read.rs`:3-5, 386 |
| `wc -m` | Unicode scalar values of the file's bytes: a BOM is one, a CR one (`file_chars`) | counts | `tools/read.rs`:383 |
| The hunk channel (`client/WIRE.md`): `edit` hunks, `cursor` frames, `ChangeEvent`s, roster carets | UTF-8 byte offsets, as the kit's API (`utf8_bytes`); a caret's `anchor_lc`/`head_lc` are `line:col`, for display only | 0 | `client/WIRE.md`:59-60, 405; `docs/SESSIONS.md`, "The hunk channel" |
| JS client library (`client/src`): `LiveText.edit`, `setCursor`, `RemoteCaret`, `transform`, `apply` | UTF-8 byte offsets of the replica's text (JS strings are UTF-16: the library converts with `byteToIndex`/`indexToByte`, `hunk.ts`:36, 60) | 0 | `client/src/hunk.ts`:1-5; `live.ts`:108, 288, 323; `position.ts`:1-3 |
| A host's editor binding (the host's own code: the client is a library with no editor; a host draws its editor over it) | The editor's own units (CodeMirror's positions are UTF-16 code units); the binding converts between them and bytes, with the library's `byteToIndex`/`indexToByte` | 0 | `client/HOST.md`:3-12 |
| The Yjs wire: sync updates exchanged with Yjs clients (feature `untrusted-wire`; `Doc::encode_diff`, `Doc::apply_client_update`) | UTF-16 code units inside the update bytes, as Yjs counts (`yjs_wire`: `utf16_code_units`); never an offset of the kit's API | 0 | the kit's Yrs runs with `OffsetKind::Utf16` to match (`engine/yrs.rs`:4, 217, 1677); bytes to UTF-16 at the adapter, `engine/yrs.rs`:389-404 (`to_utf16`) |
| Grid rows and columns (`--row`, `--col`, `--after`, `--before`; `claim --rows`/`--cols`) | a key (`r17`, `c3`: identity, never shifts), `#N` (the Nth live row or column), `LN` for a row (the row rendered line N belongs to; refused on a file holding a lone CR, `MPC-E176`), a header name for a column (`grid_row`, `grid_col`) | `#N`, `LN`: 1 | `table/verbs.rs`:6-13, 306-316 (`parse_row`) |
| Grid cells (`--from R,C`, `--to R,C`, `cursor --cell`, `claim --cells`, `blame --cell`) | `R,C`: a row and a column as above (`grid_cell`) | as its parts | `table/verbs.rs`:6-11 |
| Grid insert and move targets (`row`/`col insert --at #N`, `move --to #N`) | `#N` (`ordinal`) | 1 | `table/verbs.rs` (`VERBS`) |
| Grid rendered lines (`LN`, `row list`, `blame`, conflict reports, and the lines a grid's views, claims and roster report; `row list` and conflict reports print none on a file holding a lone CR, and `LN` refuses there: "A grid's two line counts") | lines of the rendered file as the pack's line map counts them: the header's first, and a line break inside a value (LF, CR or CRLF) starts a line (`rendered_lines`) | 1 | `table/mod.rs`:741-744 (`line_map`), 862-872 |
| A pack's own objects (`--nth K` on a pack verb that declares it) | the Nth of them, as the verb's usage names them (`pack_nth`) | 1 | `parity/positions.rs` (`Unit::PackNth`); `PackVerb::positions` |
| Doc coordinates (`pointer --at X,Y`, `viewport --rect X,Y,W,H`, and what `who` prints for them on a grid) | a place or an area in the doc's own coordinates: finite numbers the kit stores and doesn't interpret (`doc_coords`) | none | `session/participant.rs`:235, 274 (`Point`, `Rect`) |
| Structured ops' applied stream (`ListApplied.index_before`/`index_after`) | positions in the live order (Rust indices) | 0 | `elem.rs`:488-490 |

## Notes

- **Bytes of the doc text vs bytes of the file.** The doc text has `\n` line breaks and no BOM
  (`codec/text_file.rs`:1-5). So `head -c 3` on a file with a BOM prints the BOM, while
  `insert --at 1:1` inserts before the first character after it, and every API offset is
  into the doc text. A pack's codec puts the BOM and CRLF back on write.
- **A column is a Unicode scalar value**, not a UTF-16 unit and not a grapheme: `é` written as
  one code point is one column, `e` plus a combining accent is two, and an emoji is one (a
  flag or a ZWJ sequence is several). The column after a line's last character is
  `chars + 1`; a range `L:C-L:C` is half-open.
- **Where UTF-16 meets bytes.** On the server, only the engine adapter converts
  (`engine/yrs.rs`:389-404, and the sticky positions of the conformance hooks,
  `doc.rs`:876, 891-900); `engine/mod.rs`:61-66 says the kit never assumes UTF-16 outside
  the Yrs adapter. Another engine (the native one, feature `native`) may count differently
  inside; that stays behind the seam, so nothing in this table changes for a doc on it. In clients, the JS library converts between JS strings and bytes, and a
  host's editor binding between its editor's units and bytes.
- **Engine-internal units are not a contract.** Element clocks, provenance (`CharBlame.clock`,
  feature `conformance`, test-only) and the history size behind
  `Config::edit_check_scale_units` are in the doc's engine's own units, which differ between
  engines; none is an offset into the text, and `capabilities` states none of them.
- **A grid's read verbs** (`cat`, `grep -n`, `view`, `head -n`...) see the rendered file; their
  lines end at `\n`, and the `view` verb counts as they do. `LN`, `row list`, blame, conflict
  reports and the lines a grid's views and claims report use the pack's line map; see "A grid's
  two line counts".
- **Pack verbs** declare their own units with `PackVerb::positions`, in the same vocabulary
  as the kit's table: `.positions(&[PositionForm::new("--nth", Unit::PackNth)])`, or
  `.positions(&[])` for a verb that takes and prints none. `capabilities` lists a row for every
  pack verb. A verb that declares nothing still runs, and its row says `undeclared` (`--json`:
  `"undeclared": true`), so its units are unknown, not zero. A pack's test runs
  `parity::positions::check` on `Registry::for_pack(&pack)?` to refuse that, and to hold each
  declared flag to the verb's usage. The kit's grid verbs keep their rows in the kit's table.

## A grid's two line counts

A grid's rendered file is counted two ways. They agree unless the file holds a **lone CR** (a
CR with no LF after it), inside a quoted value or ending its records (a file saved by classic
Mac software, where the whole file has no `\n`).

| Count | A line ends at | Used by | Why |
|---|---|---|---|
| `\n` lines (`lines`) | `\n` | `cat`, `head`, `tail`, `wc`, `grep`, `sed`, `diff`, `view` and every `-n` number; `--lines`, `--at`, `insert`, `sed -i` addresses and `patch`; the lines `claim --lines` takes | The mirror verbs print what the real tool prints on the same bytes (K-PAR-7), and an edit by line must hit the line `cat -n` printed |
| Rendered lines (`rendered_lines`) | LF, CRLF or a lone CR, and one per record whatever ends it | The line map: `LN`, `row list`, `blame`, conflict reports, and the lines claims, views, the roster and `activity` report | A row needs lines of its own whatever ends the records |

Where the two meet:

- **`LN` refuses on a file holding a lone CR** (`MPC-E176 line_address_unreliable`, exit 2),
  every verb that takes one, since a reader of `cat -n` and the line map would mean different
  rows. Use the row's key or `#N`; `row list` shows both. Under `--base` it asks the file at
  that version, where `LN` counts. **A known limit:** that file is the base's content rendered
  with the doc's codec now, since a read base keeps no codec. So a file that was all-CR at the base
  and whose terminators a later save changed reads as holding none, and `LN` runs at that base, on
  the line map, though `cat -n` at that base printed one line. `tests/grid_lines.rs` pins it
  (`known_limit_ln_under_base_asks_with_the_live_codec`) as a limit, not as wanted.
- **When the file can't be rendered now, it offers no number,** since it can't tell whether the
  file holds a lone CR. `LN` and `claim --lines` refuse (`MPC-E177 line_address_unknown`, exit 2),
  and `LN` doesn't go on to the line map. Keys and `#N` don't ask; `blame --structure` lists both.
  A read verb renders the file first, so `cell get --row LN` and `row list` fail at that render
  (`MPC-E517`), and if only the later check's render fails, `cell get --row LN` refuses and `row
  list` prints `-`. A conflict report gives no line, and never fails a save for it. A flush has to
  render the file, so it returns that error too; a purge, a tick and an unload go on.
- **`row list` and conflict reports give no line number on such a file.** Given to `--lines`,
  a rendered line number would hit another row, with no refusal: `row list` said a row was on
  `L5` and `delete --lines 5` removed the next one. So `row list` prints `-` in its line column;
  a conflict's `lines` is `None`, `(0, 0)` in a `ConflictRegion`, and an overwrite note's `line`
  is `None`. Each still names the row by key.
- **Blame, claims, views, the roster and `activity` keep their rendered line numbers there.** On
  such a file, don't give them to `--lines`. Their numbers are for reading, and for a host
  drawing on a person's editor; to act on a row, use its key, which a claim's `span` and a note
  give. Blame names no key per line: use `row list` for the row.
- **`--ids`** prefixes each printed line, a `\n` line, with its row's key. A `\n` line can hold
  several rows when the file has a lone CR, and then the prefix names each in order, joined by
  commas: `header,r1,r2`. `grep -o --ids` names every row of the matching line, not only the
  row the match is in.
- **The lines a read looked at** (`view`, `--for-edit`, heads-up notes, the activity ring) and
  **`claim --lines`** take `\n` lines, as the caller read them; `table::NlLines` converts them to
  the rendered lines they hold, and the rows come from those. A claim reports its rendered lines,
  which on such a file differ from the numbers it was given. `Session::set_view`, a host's
  scrolled tab, takes rendered lines.
- **On a file whose records end in a lone CR, line 1 is the whole file.** `delete --lines 1`
  removes every row and its report says how many (`rows −3`). `insert --line`, `insert --after`
  and `append` of a block ending in `\n` mix terminators, so the pack declines the result and
  nothing applies (exit 2, `MPC-E122`); the same goes for any file whose records don't end in
  `\n`, CRLF included, and for any edit verb that reaches it the refusal names what works
  instead. `insert --at L:C` and `append` with text ending in the file's own line ending apply,
  as do `cell set`, `row insert`, `row delete` and `sed -i s/…/…/`, which keep every other
  terminator.

## What a row says

A verb's row in `capabilities` states the positions it takes, and for some verbs what it prints
(a `stdout` form). **No row means the verb takes no position. It doesn't mean the verb prints
none.** `select --keys` has no row because it takes keys only, and a key never shifts.

These verbs print positions and have no row yet, so `capabilities` doesn't state them. Their rows
come in Wave 1c:

| Verb | What it prints |
|---|---|
| `write`, `append`, `restore` | the lines a change touched (`EditResult.changed`); `write` also a conflict's lines |
| `undo`, `undo --partial`, `redo`, `revert` | a conflict's byte range and lines (`ConflictRegion`) |
| `claim --renew` | the claim's lines (`ClaimInfo.lines`) |
| `watch`, `tail -f` | byte offsets (`ChangeEvent.hunks`) |
| `blame --structure` | where each row and column is (`PlaceBlame.at`) |

Some verbs with a row print positions their row doesn't state yet, also for Wave 1c: `insert`,
`replace`, `delete`, `sed -i` and `patch` print the lines a change touched, and `claim` its lines.

# Positions

How each surface of the kit counts positions (K-ADDR-4). In one line: **UTF-8 bytes inside
the kit and on the hunk channel; `line:col` (1-based lines, 1-based columns in Unicode scalar
values) for people and command-line verbs; UTF-16 code units only inside the update bytes
exchanged with Yjs clients.** What an engine counts internally (element clocks, provenance) is
not part of this contract.

`capabilities` states this as data: its `positions` key (`--json`), or the "Positions"
section of its text form. Its per-verb rows come from one table,
`crates/base/src/parity/positions.rs` (`VERB_POSITIONS`), and
`crates/base/tests/positions.rs` checks it: every verb whose usage names a position has a row,
every row's flags are in that verb's usage, and each unit is the one the verbs actually use.

| Surface | Unit (`capabilities` word) | Base | Where it is set and checked |
|---|---|---|---|
| Kit API: edit hunks (`Edit::Text`, `text::diff::Hunk`), carets (`Cursor`, `At::Byte`), claims, views' spans, `ChangeEvent` hunks | UTF-8 byte offsets of the root's doc text, on character boundaries (`utf8_bytes`) | 0 | `session/position.rs`:3-5; `text::diff::validate` refuses an offset inside a character |
| `line:col` (`insert --at`, `replace`/`delete --range`, `cursor --at`/`--select`; `At::Line`; `CursorAt.anchor`/`head`) | 1-based line; 1-based column counted in Unicode scalar values (`line_col`) | 1 | `session/position.rs`:57-63 (`LineCol`), 79 (`line_col`), 112 (`offset`); `tools/edit.rs`:9-11, 108-117 |
| Line tools and line forms (`cat -n`, `head`/`tail -n`, `view --lines`, `grep -n`, `sed` addresses, `insert --line`/`--after`, `replace`/`delete --lines`, `claim --lines`, `blame -L` on text, a diff's hunk headers) | lines of the doc text, `\n` ending a line; ranges inclusive, `$` the last (`lines`) | 1 | `tools/edit.rs`:3-7; `tools/read.rs`:3-5 |
| Byte tools (`cat` with no flags, `head -c`, `tail -c`, `wc -c`) | bytes of the file as the pack encodes it, BOM and CRLF included (`file_bytes`) | counts | `tools/mod.rs`:31-33; `tools/read.rs`:3-5, 386 |
| `wc -m` | Unicode scalar values of the file's bytes: a BOM is one, a CR one (`file_chars`) | counts | `tools/read.rs`:383 |
| The hunk channel (`client/WIRE.md`): `edit` hunks, `cursor` frames, `ChangeEvent`s, roster carets | UTF-8 byte offsets, as the kit's API (`utf8_bytes`); a caret's `anchor_lc`/`head_lc` are `line:col`, for display only | 0 | `client/WIRE.md`:59-60, 405; `docs/SESSIONS.md`, "The hunk channel" |
| JS client library (`client/src`): `LiveText.edit`, `setCursor`, `RemoteCaret`, `transform`, `apply` | UTF-8 byte offsets of the replica's text (JS strings are UTF-16: the library converts with `byteToIndex`/`indexToByte`, `hunk.ts`:36, 60) | 0 | `client/src/hunk.ts`:1-5; `live.ts`:108, 288, 323; `position.ts`:1-3 |
| The editor binding (`client/src/codemirror.ts`, `editor/bytes.ts`; K2 owns the editor) | CodeMirror positions are UTF-16 code units; converted to bytes there and only there | 0 | `codemirror.ts`:1-2; `editor/bytes.ts`:1, 38, 46 |
| The Yjs wire: sync updates exchanged with Yjs clients (feature `untrusted-wire`; `Doc::encode_diff`, `Doc::apply_client_update`) | UTF-16 code units inside the update bytes, as Yjs counts (`yjs_wire`: `utf16_code_units`); never an offset of the kit's API | 0 | the kit's Yrs runs with `OffsetKind::Utf16` to match (`engine/yrs.rs`:4, 217, 1677); bytes to UTF-16 at the adapter, `engine/yrs.rs`:389-404 (`to_utf16`) |
| Grid rows and columns (`--row`, `--col`, `--after`, `--before`; `claim --rows`/`--cols`) | a key (`r17`, `c3`: identity, never shifts), `#N` (the Nth live row or column), `LN` for a row (the row rendered line N belongs to), a header name for a column (`grid_row`, `grid_col`) | `#N`, `LN`: 1 | `table/verbs.rs`:6-11, 227-240 |
| Grid cells (`--from R,C`, `--to R,C`, `cursor --cell`, `claim --cells`, `blame --cell`) | `R,C`: a row and a column as above (`grid_cell`) | as its parts | `table/verbs.rs`:6-11 |
| Grid insert and move targets (`row`/`col insert --at #N`, `move --to #N`) | `#N` (`ordinal`) | 1 | `table/verbs.rs` (`VERBS`) |
| Grid rendered lines (`LN`, `blame` and views on a grid) | lines of the rendered file as the pack's line map counts them: the header's first, and a line break inside a value (LF, CR or CRLF) starts a line (`rendered_lines`) | 1 | `table/mod.rs`:741-744 (`line_map`), 858-869 |
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
  inside; that stays behind the seam, so nothing in this table changes for a doc on it. In clients, the JS library converts between JS strings and bytes, and the
  editor binding between CodeMirror and bytes.
- **Engine-internal units are not a contract.** Element clocks, provenance (`CharBlame.clock`,
  feature `conformance`, test-only) and the history size behind
  `Config::edit_check_scale_units` are in the doc's engine's own units, which differ between
  engines; none is an offset into the text, and `capabilities` states none of them.
- **A grid's read verbs** (`cat`, `grep -n`, `view`, `head -n`...) see the rendered file; their
  lines end at `\n`. `LN`, blame and views on a grid use the pack's line map, which also ends
  a line at a lone CR inside a quoted value. The two agree unless a value holds a lone CR.
- **Pack verbs** outside the kit (a Markdown pack's, say) have no row in the kit's table yet:
  `capabilities` lists rows only for the kit's verbs and the kit's grid verbs.

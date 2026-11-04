# The `.k5board` format, version 1

A `.k5board` file is one whiteboard: a zip holding a manifest, the board as canonical JSON, and images named by their hash. This spec is enough to build a reader or a writer; no other software is needed. `schema/board-schema.json` holds the same fields, limits and refusal codes as data, and `vectors/` holds files to test against.

**The rule throughout: refuse, typed; never cut or drop.** A reader that can't take a file as it is refuses the whole file and says why. It doesn't repair it, and it doesn't open it with something missing.

"Must" and "must not" are requirements. Each one a reader enforces has a refusal code, in `code` font; §10 lists them all.

## 1. What's in the file

| Entry | Holds |
|---|---|
| `manifest.json` | Always the first entry. What the file is, and every other part with its hash |
| `board.json` | The board: objects in z-order, frames, people, settings |
| `blobs/sha256-<hex>` | Images, each named by the hash of its bytes |
| `preview.png` | Optional. A picture of the board at some moment. Readers may ignore it |
| anything else listed | A part from a newer writer: kept by writers, ignored by readers |

- **Media type:** `application/vnd.k5.board+zip`. **Extension:** `.k5board`.
- **No host's identifiers.** The file names no workspace, server, room, path, access list or account. Every host enforces its own access when it opens a file.
- **No collaboration state.** No history, clocks, version vectors, tombstones or engine state. A deleted object is simply absent. Order is plain array order.
- **No derived state.** Nothing that can be computed is stored: a pen stroke's box comes from its points, and a reference's state comes from whether its target is there.

## 2. The container

The container is a strict subset of zip. Stock tools read these files. A file made by a stock tool is accepted only if it keeps to this subset.

**Every byte belongs to something.** From the first byte of the file: entries, each a local header, its name, its extra field and its data, with nothing between them; then the central directory; then the end record, with nothing after it.

A reader must refuse:

| What | Code |
|---|---|
| No end record in the file's last bytes, including a file cut short | `zip_no_end_record` |
| Bytes outside any entry: a prefix, a gap between entries, bytes between the central directory and the end record, bytes after the end record, an archive comment | `zip_stray_bytes` |
| Entries sharing bytes, or an entry running into the central directory | `zip_overlap` |
| Local header and central directory disagreeing on name, method, flags, CRC or either size | `zip_header_mismatch` |
| A broken record: bad signatures, short headers, offsets outside the file, a count that doesn't match, a malformed extra field, an entry comment | `zip_malformed` |
| Two entries with one name, exactly or under case folding (`board.json` and `BOARD.json`) | `zip_name_duplicate` |
| A name outside the grammar below | `zip_name_invalid` |
| A symlink, directory or other non-regular entry, by Unix type, DOS directory attribute or Windows reparse attribute | `zip_not_regular` |
| The encryption, strong encryption or masked header flags | `zip_encrypted` |
| Any other flag but the deflate level hints (bits 1 and 2) and UTF-8 names (bit 11). A data descriptor (bit 3) is refused here | `zip_flags` |
| More than one disk | `zip_multi_disk` |
| ZIP64 in any form: a marker value in a size, offset or count, a ZIP64 extra field, a ZIP64 locator | `zip64` |
| A method other than STORE (0) or DEFLATE (8) | `zip_method` |
| Data that doesn't match its CRC-32 | `zip_crc` |
| Data that doesn't end exactly at its declared sizes | `zip_size_mismatch` |
| Broken deflate data | `zip_deflate_invalid` |

**Names.** One to 4 segments joined by `/`, at most 128 bytes in all. A segment is 1 to 80 bytes of `[A-Za-z0-9._-]`, doesn't start or end with `.`, and isn't a name Windows reserves: `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9` or `LPT0`-`LPT9`, in any case, with or without an extension. That rules out traversal, absolute paths, backslashes, drive letters and directory entries. Extra fields are allowed and ignored, except ZIP64's.

**Sizes can lie, so limits hold twice.** Before inflating anything, a reader judges every entry from the central directory: entry count, each size, the total, and the declared ratio (§10). Then, while inflating:

- it stops at the declared size and refuses if there's more (`zip_size_mismatch`), and refuses if the data ends short of it;
- the compressed data must end exactly at its compressed size;
- the ratio holds at every step, not only at the end: once output passes `ratio_floor`, output so far must stay at or below `ratio` times the compressed bytes consumed so far (`limit_ratio`). A file whose start inflates far past the limit is refused even if its whole ratio would pass.

A writer compresses `board.json` with DEFLATE when that helps and its own result would pass these checks, and stores it otherwise. It stores everything else.

## 3. The manifest

`manifest.json` is JSON (§6), always the first entry, both in the file and in the central directory (`manifest_not_first`). It is usually stored, so a tool can identify a file from its first bytes.

| Field | Holds |
|---|---|
| `format` | Exactly `"k5board"` (`manifest_format`) |
| `version` | The highest format version that has written this file, a whole number from 1 |
| `min_reader` | The oldest reader version that may read this file, from 1, at most `version` |
| `lineage` | 32 lower-case hex digits: 128 random bits minted once, which tell copies of one board apart |
| `base` | Optional: `{"sha256": <64 hex>}`, the hash of the `board.json` this copy left from, so a returned copy can merge against it |
| `parts` | Every entry but the manifest itself, one object each |
| anything else | Kept by writers |

A part:

| Field | Holds |
|---|---|
| `name` | Its entry name |
| `media_type` | A lower-case media type |
| `role` | `board`, `blob`, `preview`, or a name of `[a-z0-9-]` a reader doesn't know |
| `bytes` | Its uncompressed size |
| `sha256` | 64 lower-case hex digits: the hash of its uncompressed bytes |
| anything else | Kept by writers |

**Read `format` and `min_reader` first.** A `min_reader` above the reader's own version is `min_reader_too_high`, whatever else the manifest holds: a newer format may change anything else. Other malformed fields are `manifest_invalid`.

**The parts and the zip must agree:**

| What | Code |
|---|---|
| A part listed but not in the zip | `part_missing` |
| An entry in the zip but not listed | `part_unlisted` |
| A name listed twice, exactly or under case folding | `part_duplicate` |
| `bytes` differing from the zip's size | `part_disagree` |
| A part's bytes not matching its `sha256`, or a blob's name not matching its `sha256` | `part_hash` |
| A role that doesn't fit its name: `board` goes with `board.json` and exactly one part has it; `preview` goes with `preview.png`, at most one; `blob` goes with names under `blobs/`, `sha256-<hex>`; and the manifest doesn't list itself | `part_role` |

A reader checks `board.json`'s hash on opening. It may check other parts when it reads them, and must check before using their bytes.

## 4. `board.json`

JSON (§6), an object with exactly these required members, plus any others, which are kept (`board_shape`):

| Member | Holds |
|---|---|
| `objects` | An array of objects, **back to front**: array order is z-order |
| `frames` | An array of frame object ids, in presentation order. It lists every object of type `frame` exactly once, and nothing else (`frames_mismatch`) |
| `people` | An object: person token to person |
| `settings` | An object: the board's settings (§10) |

An object is a JSON object with `id`, `type` and its other fields. **An absent field stays absent:** a writer doesn't fill in defaults, and a reader uses the default in §10. `null` is not a value for a known field (`field_kind`): absent means none.

**Coordinates** are board units, a plane with `x` growing right and `y` growing down. `x` and `y` are an object's top-left corner, `w` and `h` its size, `rot` its rotation in degrees. A pen stroke's `points` are relative to its `x` and `y`; its box comes from the points.

### Ids

An id is 1 to 24 bytes of `[0-9a-z._-]` (`id_invalid`). **Ids are opaque:** a reader must not parse one for meaning, and a writer may mint any id that fits. Once minted, an id is never reused for something else, and it stays with its thing across edits and copies of the file.

- **Object ids** are unique in the file (`id_duplicate`).
- **Sub-item ids** (table rows and columns, replies, check items) are unique within their list. Identity is never a position.
- **Group ids** name a group. Objects holding the same `group` are one group. A group is not an object.
- **Person tokens** are ids too.

**A refusal names the bad ids.** With `id_invalid`, a reader reports every id that breaks the rule, so a host can offer to re-key them in one go. It doesn't re-key them itself, since it can't find references in unknown fields. The list holds each id once, in the order of the file's canonical form (§6), so spelling doesn't change it. It's capped: the first 1,000, each cut at 64 code points and ended with `…` if it's longer, with the total of all of them and a flag saying the list stopped short. Ids are told apart by their full text, so two long ones that share their first 64 code points count as two, and the list shows the same cut text twice. Only text can be named: an `id` that's a number or absent is refused, and its place is reported, but it isn't in the list.

### People

`people` maps a token to `{"name": <display name>, "kind": <kind>}`. Both are optional. `kind` is a name: `person` and `agent` are known, and others are kept. Removing every name still leaves one token per person, so reactions, votes and authorship keep meaning one person each. A token is local to the file and says nothing about any account. A writer should mint tokens from random bytes, so that two copies of a board never give one token to two different people; two copies' tables then merge by union. A token named anywhere must be in `people` (`person_unknown`).

### References

A connector's `from` and `to`, a comment's `target`, a mind node's `parent` and any object's `frame` name an object by id. **A missing target is a state, not an error.** A reader computes each reference's state when it reads, and nothing is stored:

| State | When | How it's drawn |
|---|---|---|
| attached | the target is there | on the target |
| suspect | the target is there but doesn't fit: a `frame` that names a non-frame, or a `parent` or `frame` chain that loops, cut at the member with the smallest id (bytewise) | at its last good place, flagged |
| orphaned | the target isn't there | at the end's `hint` if it keeps one; never moved or deleted silently |

A connector end's `hint` is `{x, y}`, the last point it resolved to. An object without a `frame` field belongs to whichever frame it sits inside at read time, by geometry. An object with one belongs to that frame.

### Field kinds

The tables in §10 name each field's kind. A value of the wrong JSON type is `field_kind`; a value of the right type that breaks the rule is `field_value`; a list or text over its limit is `limit_field`. A required member missing inside a kind is `field_kind`.

| Kind | Holds |
|---|---|
| text | A string, limited in **code points**, so an emoji counts as one |
| name | A string of `[a-z0-9-]`, 1 to 40 bytes. The listed values are known. **Others are kept**, and a renderer falls back to the field's default |
| colour | A name as above, or `#rrggbb` |
| object reference | An id |
| group id | An id |
| tags | At most 200 texts of at most 80 code points |
| points | At most 5000 `[x, y]` pairs of numbers, exactly two each |
| connector end | `{id, anchor?, hint?}`, or `{x, y}` for a free end, never both. `anchor` is a name: `auto`, `top`, `right`, `bottom` or `left` are known |
| check items | At most 50 `{id, text, done}`, with `text` at most 500 code points and `done` a boolean |
| columns | At most 30 `{id, width?}`, with `width` not negative |
| rows | At most 100 `{id, cells?}`. `cells` maps a column id of the same table to a text of at most 2000 code points (`cell_column_unknown`). An absent cell is empty |
| reactions | An object: reaction (1 to 32 bytes, no control characters) to its person tokens, **sorted and unique**. At most 64 reactions, and 10,000 people on each |
| votes | An object: person token to a whole number of votes from 1. At most 10,000 people |
| replies | At most 1000 `{id, by, at, text}`, with `by` a token, `at` a timestamp and `text` at most 5000 code points |
| crop | `{x, y, w, h}`, fractions of the image from 0 to 1. `w` and `h` are at least 0.01; `x + w` and `y + h` are at most 1.0001 |
| link | An `http://` or `https://` address with no whitespace, at most 2000 code points |
| blob reference | `{"blob": "sha256-<hex>"}` with no other member (§7) |
| created | `{by, at}`, a person token and a timestamp |
| voting | `{active, perPerson?}`: a boolean, and a whole number from 1 |

A **timestamp** is RFC 3339: `YYYY-MM-DDTHH:MM:SS[.fraction]` and `Z` or `±HH:MM`, at most 40 bytes.

Every object-shaped value keeps members it doesn't know.

**Field names.** A field of an object, or of `settings`, is named by 1 to 200 bytes with no control characters (`field_name_invalid`).

## 5. Unknown things are kept

A reader doesn't fail on what it doesn't know, and a writer that round-trips a file keeps it:

- **Unknown fields**, on an object, in `settings`, on a person, at the top level, in the manifest, on a part: kept exactly.
- **Unknown object types:** a `type` of `[a-z0-9-]`, 1 to 40 bytes (`type_invalid` otherwise). Kept with all its fields. A renderer shows a placeholder at its position.
- **Unknown names** in a name or colour field: kept, drawn with the field's default.
- **Unknown part roles:** kept with their name, role, media type and bytes.

A change an older reader can't safely ignore raises `min_reader`, and an older reader then refuses the file. It never opens it with fields dropped.

## 6. JSON

Both `manifest.json` and `board.json` are UTF-8 JSON (RFC 8259), read strictly. A reader must refuse:

| What | Code |
|---|---|
| Bytes that aren't UTF-8 | `json_utf8` |
| Anything outside the grammar, a byte order mark included | `json_syntax` |
| A member name appearing twice in one object, after unescaping | `json_duplicate_key` |
| A `\u` escape that is a lone surrogate | `json_surrogate` |
| Nesting past `depth` | `limit_depth` |
| More values than `values` in one document | `limit_values` |
| A number literal over 64 bytes | `limit_number_literal` |
| A number that fails the rule below | `json_number_inexact`, `json_number_range` |

Text is kept as written: no normalisation of any kind.

### The number rule

Numbers are IEEE 754 doubles. **A literal with more digits than a double holds is refused, not cut.** 17 significant digits is enough to print any double exactly, so a writer that prints 17 digits is fine. **Significant digits** run from the first non-zero digit to the last: zeros at either end don't count, since the value is exact without them, so `1.00000000000000000000` has one and `1000e-3` has one.

The exact test, for a literal in JSON's grammar:

1. If every digit before the exponent is zero, the value is `0`. Stop.
2. If the literal's **exact value** is past 2^53 (9007199254740992) in magnitude, refuse with `json_number_range`. Judge this on the literal, not on the parsed double: `9007199254740993` parses to 2^53, and a check on the double would silently change that integer.
3. If it has more than 17 significant digits, refuse with `json_number_inexact`.
4. Parse it to the nearest double. If that's zero, refuse with `json_number_inexact`: a non-zero value mustn't silently become zero.
5. Otherwise it's accepted, as that double. A writer prints it in its shortest form (ECMAScript's `Number::toString`).

| Literal | Result | Prints as |
|---|---|---|
| `0.1` | accepted | `0.1` |
| `0.10000000000000001` | accepted: 17 digits, the double nearest is `0.1` | `0.1` |
| `1.0` | accepted | `1` |
| `1E2` | accepted | `100` |
| `-0`, `-0.0` | accepted: zero has no sign here | `0` |
| `1.00000000000000000000` | accepted: one significant digit | `1` |
| `0.30000000000000004` | accepted | `0.30000000000000004` |
| `9007199254740992` | accepted | `9007199254740992` |
| `1.0000000000000001` | accepted, and see the limit below | `1` |
| `0.1000000000000000055` | `json_number_inexact`: 19 digits | |
| `1e-400` | `json_number_inexact`: it would become zero | |
| `9007199254740993` | `json_number_range`: past 2^53, though it parses to 2^53 | |
| `9007199254740992.5` | `json_number_range`: past 2^53 | |
| `1e400` | `json_number_range`: past 2^53 | |

This applies to every number, in unknown fields too.

**The limit, stated plainly.** A literal of up to 17 digits that didn't come from a double, typed by hand for instance, lands on the nearest double, and this rule accepts that. `1.0000000000000001` becomes `1`. The rule catches digits a double can't hold in number, not every value a double can't hold exactly; no decimal format of doubles could do otherwise and still take `0.1`.

A file in another spelling gets new canonical bytes when it's written, and a new guard once (§8).

**JavaScript readers, take note.** `JSON.parse` keeps the last of two duplicate keys and takes a number of any length, so neither check can happen after it. A JavaScript reader has to check the text: `vectors/check.mjs` is a working example of both, in about a hundred lines.

### Canonical form

A writer stores `manifest.json` and `board.json` in the canonical form of RFC 8785 (JCS):

- no whitespace;
- members sorted by their names' **UTF-16 code units**, as JavaScript's default sort does (not by UTF-8 bytes, which order some characters differently);
- numbers printed as `Number::toString` prints them;
- strings with only `"`, `\` and control characters below U+0020 escaped: `\b \t \n \f \r` where they exist, and lower-case `\u00xx` otherwise. Nothing else is escaped.

Arrays keep their order: it is meaning. In a canonical manifest, `parts` is sorted by `name`, bytewise. A reader accepts any spelling of the JSON, and hashes what it read.

The canonical bytes are the same in every implementation: `vectors/canon/` holds cases that Rust and JavaScript both check. The zip around them isn't: DEFLATE output can differ between libraries and versions, so no conformance test compares zip bytes. One writer at one version is deterministic, with a fixed timestamp (1980-01-01), fixed attributes, and entries in order: manifest, board, then the rest sorted by name.

## 7. Blobs

A blob's entry is `blobs/sha256-<hex>`, where `<hex>` is the hash of its bytes. An object names one with `{"blob": "sha256-<hex>"}`. Any JSON object whose only member is `blob`, holding that form, counts as a reference wherever it sits, unknown fields included, so a writer sees references it doesn't understand.

- **A file with a dangling reference still opens.** A reader reports the missing hashes, as a state of the open file, so a host can draw a placeholder. **A writer refuses to write it** (`blob_missing`), so a dangling reference is never made or kept silently. To save such a board, clear the reference: a known blob field is removed, and a reference inside an unknown field becomes `null`.
- A blob that nothing references is allowed. A writer keeps it unless the host removes it.
- `media_type` says what the bytes are. PNG, JPEG, WebP and GIF are for display. Anything else, SVG included, is kept and not rendered.

## 8. Saving: the guard and revisions

A host that lets two surfaces save one file compares a **guard** before it replaces the file. The guard is the sha256 of the canonical form of the manifest (§6).

The manifest lists every part with its hash, so **one compare covers any part added, removed or changed by a tool that updates the manifest**, not only `board.json`. To save: keep the guard from when the file was read; just before replacing it, read the file's manifest again and compare; if they differ, the file changed since it was read, and the save must not overwrite it. Reading the guard needs only the central directory and the manifest.

**It's stable.** `lineage` and `base` are in the manifest and a writer passes them through, so opening and writing a canonical file unchanged gives the same guard. A host that sets a new `base` gets a new guard, as it should.

**It doesn't cover:**

- entry order, compression and JSON spelling: no content changes. A file in another spelling gets a new guard the first time it's written canonically, because `board.json`'s bytes differ, then stays stable;
- a tool that changes a part's bytes without updating the manifest: the guard can't see that, and the part's own hash refuses it on reading (`part_hash`);
- the moment between comparing and replacing: the host's atomic replace has to close that;
- file metadata such as its name and timestamps.

The guard is not `base`. `base` names a `board.json` by its hash, for a merge; the guard names the whole package, for a save.

### Revisions

An object's **revision** is the sha256 of its canonical JSON (§6): the object with its `id`, its `type` and every field, unknown ones included. **Position isn't part of it**, or every insert would make false conflicts. The settings' revision is the sha256 of the canonical JSON of `settings`.

An edit names the revision it read. If the object or the settings have moved on, the edit conflicts and changes nothing. **So no edit overwrites a change it hasn't seen.** Saves from one read never resolve by last-writer-wins, with one exception:

### Order is the one exception

A move doesn't name a revision, and it never conflicts. A move says where an item goes: the top, the bottom, or directly above the first of some named neighbours still there, nearest first; if none is there, the bottom. The result is deterministic, and nothing is lost or duplicated. If two saves move one object, **the later save's position stands, and that's reported to whoever made it.** This holds for z-order and for `frames` alike.

### Locks

An object whose `locked` is true refuses edits (`object_locked`): field changes, deletion and moves. Only setting `locked` to false, and changing `votes` or `reactions`, pass. Clearing a dangling blob reference (§7) passes too, since otherwise a locked picture would make a board unsaveable.

### No limit on one object

There's no limit on an object's total size beyond the limits on its fields (§10) and on `board.json`.

## 9. Versions

- `version` is the highest format version that has written the file. **A writer never lowers `version` or `min_reader`:** each stays the higher of the file's and the writer's own, so a file never claims to be older than its contents.
- A reader whose version is at least `min_reader` must read the file, keeping whatever it doesn't know.
- A change that an older reader can safely ignore raises `version` only. A change it can't raises `min_reader` too.
- A limit raised in a later version needs nothing else: an older reader refuses a file over its own limit, typed.

## 10. Limits, fields and refusal codes

Limits are the format's ceilings. A host may tighten them for its own reasons, and may not raise them. Sizes are bytes, uncompressed; `objects`, `people` and `limit_field`'s lists are counts.

Text limits below are in code points. Defaults are what a reader uses when a field is absent. Portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT; see `THIRD-PARTY-NOTICES.md`.

<!-- generated:begin -->

### Limits

| Limit | Most |
|---|---|
| `entries` | 65534 |
| `manifest_bytes` | 33554432 |
| `board_json_bytes` | 268435456 |
| `part_bytes` | 134217728 |
| `total_bytes` | 2147483648 |
| `ratio` | 300 |
| `ratio_floor` | 1048576 |
| `objects` | 100000 |
| `people` | 100000 |
| `depth` | 64 |
| `values` | 20000000 |

### Fields on every object

| Field | Holds | When absent |
|---|---|---|
| `x` | number | `0` |
| `y` | number | `0` |
| `rot` | number | `0` |
| `locked` | bool | `false` |
| `group` | group id | none |
| `frame` | object reference | none |
| `tags` | tags | none |
| `reactions` | reactions | none |
| `votes` | votes | none |
| `link` | link | none |
| `created` | created | none |

### `sticky`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `200` |
| `h` | number | `200` |
| `text` | text, at most 5000 | `""` |
| `color` | colour: yellow, orange, pink, red, violet, blue, cyan, green, lime, gray, white | `"yellow"` |
| `fontSize` | number | `0` |
| `align` | name: left, center, right | `"center"` |

### `text`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `240` |
| `h` | number | `40` |
| `text` | text, at most 20000 | `""` |
| `color` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"ink"` |
| `fontSize` | number | `24` |
| `bold` | bool | `false` |
| `italic` | bool | `false` |
| `align` | name: left, center, right | `"left"` |
| `checks` | check items | none |

### `shape`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `180` |
| `h` | number | `120` |
| `shape` | name: rect, round, ellipse, triangle, diamond, parallelogram, trapezoid, pentagon, hexagon, octagon, star, cross, arrow-right, arrow-left, chevron, cylinder, cloud, speech, document, flow-start, flow-process, flow-decision, flow-data, flow-predefined | `"round"` |
| `text` | text, at most 5000 | `""` |
| `fill` | colour: none, white, gray, red, orange, yellow, green, teal, blue, violet, pink, ink | `"white"` |
| `stroke` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"ink"` |
| `strokeWidth` | number | `2` |
| `dash` | name: solid, dashed, dotted | `"solid"` |
| `color` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"ink"` |
| `fontSize` | number | `18` |

### `connector`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `0` |
| `h` | number | `0` |
| `from` | connector end | none |
| `to` | connector end | none |
| `route` | name: straight, elbow, curved | `"elbow"` |
| `stroke` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"ink"` |
| `strokeWidth` | number | `2` |
| `dash` | name: solid, dashed, dotted | `"solid"` |
| `startCap` | name: none, arrow, triangle, circle, diamond, bar | `"none"` |
| `endCap` | name: none, arrow, triangle, circle, diamond, bar | `"arrow"` |
| `label` | text, at most 500 | `""` |

### `pen`

| Field | Holds | When absent |
|---|---|---|
| `points` | points | none |
| `stroke` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"ink"` |
| `strokeWidth` | number | `3` |
| `highlighter` | bool | `false` |

### `frame`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `960` |
| `h` | number | `600` |
| `title` | text, at most 200 | `""` |
| `fill` | colour: none, white, gray, red, orange, yellow, green, teal, blue, violet, pink, ink | `"white"` |

### `card`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `280` |
| `h` | number | `150` |
| `title` | text, at most 500 | `""` |
| `text` | text, at most 5000 | `""` |
| `color` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"blue"` |
| `assignee` | text, at most 80 | `""` |
| `due` | text, at most 40 | `""` |
| `status` | text, at most 40 | `""` |
| `checks` | check items | none |

### `table`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `480` |
| `h` | number | `160` |
| `columns` | columns | none |
| `rows` | rows | none |
| `fill` | colour: none, white, gray, red, orange, yellow, green, teal, blue, violet, pink, ink | `"white"` |

### `mind`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `180` |
| `h` | number | `48` |
| `text` | text, at most 1000 | `""` |
| `parent` | object reference | none |
| `color` | colour: ink, gray, red, orange, yellow, green, teal, blue, violet, pink, white | `"violet"` |

### `comment`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `32` |
| `h` | number | `32` |
| `text` | text, at most 5000 | `""` |
| `replies` | replies | none |
| `resolved` | bool | `false` |
| `target` | object reference | none |

### `image`

| Field | Holds | When absent |
|---|---|---|
| `w` | number | `320` |
| `h` | number | `240` |
| `src` | blob reference | none |
| `alt` | text, at most 300 | `""` |
| `crop` | crop | none |

### `settings`

| Field | Holds | When absent |
|---|---|---|
| `title` | text, at most 200 | `""` |
| `voting` | voting | none |

### Refusal codes

- `zip_no_end_record`
- `zip_stray_bytes`
- `zip_overlap`
- `zip_multi_disk`
- `zip64`
- `zip_encrypted`
- `zip_flags`
- `zip_method`
- `zip_malformed`
- `zip_header_mismatch`
- `zip_name_invalid`
- `zip_name_duplicate`
- `zip_not_regular`
- `zip_crc`
- `zip_size_mismatch`
- `zip_deflate_invalid`
- `manifest_not_first`
- `manifest_invalid`
- `manifest_format`
- `min_reader_too_high`
- `part_missing`
- `part_unlisted`
- `part_duplicate`
- `part_disagree`
- `part_hash`
- `part_role`
- `blob_missing`
- `limit_entries`
- `limit_part_bytes`
- `limit_total_bytes`
- `limit_ratio`
- `limit_objects`
- `limit_people`
- `limit_depth`
- `limit_values`
- `limit_number_literal`
- `limit_field`
- `limit_raised`
- `json_utf8`
- `json_syntax`
- `json_duplicate_key`
- `json_surrogate`
- `json_number_inexact`
- `json_number_range`
- `board_shape`
- `id_invalid`
- `id_duplicate`
- `type_invalid`
- `field_name_invalid`
- `field_kind`
- `field_value`
- `person_unknown`
- `frames_mismatch`
- `cell_column_unknown`
- `object_locked`
- `io_error`

<!-- generated:end -->

## 11. Conformance vectors

`vectors/index.json` lists every case. A case has a `layer`, a `name` and an `input`, and then what it expects: `refusal`, the expected code, or `canonical`, the file holding the expected canonical bytes, with their `sha256`. `limits` tightens limits for that case, so its file can stay small. `bad_ids` names a file with the list a refusal must give: `ids`, `total` and `truncated`.

| Layer | Input | Expected |
|---|---|---|
| `canon` | Any JSON | Canonical bytes, or a JSON refusal. For any language |
| `json` | A `board.json` | The board's canonical bytes, or a refusal |
| `zip` | A `.k5board` | The board's canonical bytes and the `guard`, or a refusal. A case with `stable_guard` keeps its guard through an unchanged open and write. A case with `missing` opens and reports those hashes; a write refuses it with `blob_missing`; clearing them gives the board in `cleared`, which writes |
| `rev` | A `board.json` | `revs`: each object's revision, and `settings_rev`. For any language |
| `fidelity` | A `board.json`, full of what's easy to lose: every field, unknown things, text in many scripts, number spellings, order, broken references, locks, every template | The board's canonical bytes. For any language |
| `widget` | A pair: an object in Board's widget store, and the same in a file | For a host that converts between them. `file` is a valid `board.json`, `widget` the other side, with neutral forms where a host puts its own (`vectors/widget/README.md`), `people_map` the person keys and tokens, `lossy` what doesn't survive in words, and `widget_back` and `file_back` what the way back gives where it differs. `lossy_direction` says which way the conversion holds. A pair with `file_refused` has `file_text` in place of `file`, which a reader must refuse with that code. These members are test data for a converter, not part of the format |

Each refusing case breaks one rule, so its code is exact. A reader that checks in another order may meet a different rule first only where one file breaks two, and these files don't.

`node vectors/check.mjs` runs the `canon`, `rev` and `fidelity` layers and every `bad_ids` case in JavaScript with no code from the Rust crate.

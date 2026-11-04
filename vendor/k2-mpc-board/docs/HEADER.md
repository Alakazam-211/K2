# Header, preview and summary

A host's listing shows many boards at once: a name, a title, a count, a picture. Opening each file to get them reads and checks every byte of every board, so the crate has three smaller reads for it. The format doesn't change for them: they read what a file already holds.

**None of them is an open.** Each gives `checked: false`. The board isn't validated, and a full open may still refuse the file, so a host acts on a file, by showing it as open, editing it or saving it, only through `Package::open` or `BoardFile`. Nothing these return can be turned into either.

## The three calls

| Call | Reads | Checks | Gives |
|---|---|---|---|
| `read_header(src, limits)` | The central directory and `manifest.json` | Everything an open checks before `board.json`: the zip's entries, sizes, total and ratio as declared, the manifest's size, `format`, `min_reader`, and the parts agreeing with the zip | The version and `min_reader`, the guard, the parts by name, role, media type and size, and the preview's size |
| `read_preview(src, limits, max_bytes)` | The same, then `preview.png` | As the header, and the preview's size against the host's bound, then its bytes against the manifest's hash | The preview's bytes and media type, or none if the file has no preview |
| `read_summary(src, limits)` | The same, then `board.json`, streamed in chunks of at most 64 KiB and never held whole | As the header; `board.json`'s size, ratio and CRC as it inflates, and its hash against the manifest; the JSON's syntax, depth and number of values; the number of objects; and the title's field limit | The guard, the number of objects, and `settings.title` |

- **The header never reads `board.json`'s bytes.** It reads the directory, the local headers and the manifest.
- **The preview's bound is the host's.** The crate has no default. A host showing thumbnails might pass 2 MiB:

  ```rust
  let preview = read_preview(File::open(path)?, &Limits::V1, 2 * 1024 * 1024)?;
  ```

  A preview over it is refused with `limit_part_bytes`, before it's read. A preview whose bytes don't match the manifest's hash is refused with `part_hash`, and no bytes are given.
- **The summary checks `board.json`'s hash.** A manifest copied from another board names that board's parts, so it gives that board's guard. Without the hash, such a file would put another board's title under a guard a host already knows. Its bytes are refused with `part_hash`, as an open refuses them; `vectors/zip/manifest-other-board.k5board` is that file.
- **The summary takes any JSON spelling.** Canonical order puts `settings` last, so the title comes at the end of the pass; any other order reads the same. It doesn't build the board: it counts the elements of `objects` and keeps the title, and its memory is the 64 KiB chunks, the nesting, and the title.

## What they don't check

The board's own rules: ids, types, fields, references, people, frames. Duplicate member names, apart from the members the summary reads. Numbers, beyond their syntax and length. Blobs and other parts' bytes. A file these accept can still be refused by an open. That's why `checked` is `false`, and why the calls are for listing, never for acting.

## A host's cache

- **Key it by the file's path and its guard.** The guard is the hash of the canonical manifest, and the manifest lists every part with its hash, so any change to any part gives a new guard. A summary carries its guard, so the key and the value come from one read.
- **Cache a title and a count from `read_summary`, and nothing else.** A header's guard alone doesn't tie the manifest to the board's bytes; the summary's hash check does.
- **Last changed is the host's,** from the file's modification time. The format records no time.

## What they cost

`crates/board/examples/header_cost.rs` measures each call on one file, each in its own process, so a line's peak memory is that call's.

At 100,000 objects, on one Linux machine (an AMD Ryzen 7 3800X, with 4 CPUs), a release build, for a file of 8.0 MiB with a 21.6 MiB `board.json` and a 1 MiB preview, each the median of 7 runs:

| Call | Time | Peak memory of the process |
|---|---|---|
| `read_header` | 0.03 ms | 2.6 MiB |
| `read_preview`, 2 MiB bound | 1.3 ms | 3.6 MiB |
| `read_summary` | 196 ms | 2.7 MiB |
| `Package::open`, for comparison | 816 ms | 351 MiB |

The summary's time is one pass over `board.json`: inflating, hashing and scanning it. Its memory doesn't grow with the board.

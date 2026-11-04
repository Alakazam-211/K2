# Conversion pairs

Each file here is one shape as Board's widget stores it and as this format holds it, for a host that
converts between them. This crate has no converter; it checks that each `file` side is a valid board,
and that each `file_refused` pair is refused with its code.

The host's own forms are neutral here: `key-ann` and `key-bot` stand for however a host keys its
people, and `hostblob:` for its blob scheme. A host swaps in its own, and tests those on its side.

- `widget` and `file`: the two sides. `people_map` maps the host's person keys to the file's tokens.
- `lossy`: what doesn't survive, in words.
- `lossy_direction`: which way the conversion and its `lossy` line hold. `widget_to_file`: converting `widget` gives `file`, or `file` with `file_back` over it. `file_to_widget`: converting `file` gives `widget`, or `widget` with `widget_back` over it. `both`: both of those. A test runs a pair only the way it names. This is test data: no file carries it.
- `widget_back`, `file_back`: what the way back gives, where it differs. They hold the members that differ, at the level those members sit: `table-sparse`'s `file_back` is a member of the row `rw-0`, not of the object.
- `widget_when_absent`: what the widget should show for a field the file leaves out.
- `widget_back_refused`: why the way back can't hold it.
- `file_refused`, with `file_text` and `widget_text`: a value the widget's store takes and this format refuses. `file_text` is the `board.json` a direct conversion would write, as text, and reading it gives the refusal code in `file_refused`. Both sides are text because the value can be one a strict JSON reader won't parse. These pairs have no `file` or `widget` member.

The shapes are Board's: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT. See
`THIRD-PARTY-NOTICES.md`.

# Conversion pairs

Each file here is one shape as Board's widget stores it and as this format holds it, for a host that
converts between them. This crate has no converter; it checks that each `file` side is a valid board.

The host's own forms are neutral here: `key-ann` and `key-bot` stand for however a host keys its
people, and `hostblob:` for its blob scheme. A host swaps in its own, and tests those on its side.

- `widget` and `file`: the two sides. `people_map` maps the host's person keys to the file's tokens.
- `lossy`: what doesn't survive, in words.
- `widget_back`, `file_back`: what the way back gives, where it differs.
- `widget_when_absent`: what the widget should show for a field the file leaves out.
- `widget_back_refused`: why the way back can't hold it.

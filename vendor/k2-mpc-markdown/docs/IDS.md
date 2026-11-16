# IDs and addresses

<!-- Generated from crates/markdown/src/section.rs (`section::IDS`) by the lib test `the_id_table_is_the_docs`; don't edit by hand. Regenerate: MPC_BLESS=1 cargo test. -->

The Markdown pack's own address kinds (`Pack::ids`). The kit's kinds (lines, `L:C`, versions, changes...) apply as well: see the kit's `docs/IDS.md`.

| Kind | Example | Class | Scope | Accepted by | Note |
|---|---|---|---|---|---|
| `heading-address` | `--heading @h:goals` | unique-name | a Markdown doc's headings | `replace-section` | `@h:<slug>`, GitHub's anchor rule on the heading's source text: lowercased; letters, numbers, combining marks, `-` and `_` kept; each space a `-`; anything else dropped. A repeat gets `-1`, `-2` in document order. A rename, or a repeat added or removed above, changes it; one that names no heading is refused (`MARKDOWN-E001`), never the nearest |
| `heading-text` | `--heading '## Goals'` or `--heading Goals` | unique-name | a Markdown doc's headings | `replace-section` | the level and the text, or the text at any level, whitespace collapsed; several matching: exit 1 with their lines, then `--nth K` (a position: name the version with `--if-version`) |

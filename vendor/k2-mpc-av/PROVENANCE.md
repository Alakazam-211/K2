# Provenance

Everything here is our own Rust, written by us; no code, text, tables or figures are copied
from another project or document.

| Part | Written from |
|---|---|
| `crates/av` (devices, boundaries and pulls, flags, title blocks, the sheet index) | our own design, in our own words, of how AV system drawings are commonly read: device identifiers, dotted boundaries a cable is pulled through, off-sheet connectors, revision blocks |
| the configuration format | our own design |
| tests and their drawing sets | our own; the sets are drawn by the CAD repo's generator with neutral names |

No project's own conventions (its tag, layer or block names) are in this repo: they come from a
configuration file the caller passes, kept with the project.

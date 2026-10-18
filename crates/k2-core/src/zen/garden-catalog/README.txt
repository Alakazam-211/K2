Garden catalog (prd-zen-user-widgets-v2, R5 as changed 2026-10-08).

One file per ready-made Garden version: <short>-<n>.toml, a page template
(id = "k2.<short>@<n>") plus a [catalog] table. build.rs picks up every
*.toml here; adding a Garden needs no Rust edit. See
crates/k2-core/src/zen/garden_catalog.rs for the format and the rules.
Released files are immutable: ship <short>-<n+1>.toml instead.

The first entry, diary-1.toml (template k2.diary@1, widget k2:diary@1),
is written by B2 (page) and B1 (widget) during the Zen v2 build.
kanban-1.toml (template k2.kanban@1, widget k2:kanban@1) is the Kanban
board, new in 0.45.7: its board lives in Garden data, not in the file.
kanban-2.toml (template k2.kanban@2, widget k2:kanban@2, 0.46.1) is the
same page on the Kanban whose Thread panel wears K2's Thread look in K2
Companion's colours; a synced Kanban Garden moves to it, an own copy keeps
@1 (Garden sync never reverts or overwrites a copy).

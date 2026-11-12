# deckcraft, vendored

A copy of the PPTX reader and the CPU renderer of DeckCraft, version 0.18.1, for K2's live
`.pptx` previews. Vendored, not depended on, on the Kessel Steward's ruling of 2026-11-11.

- **Source:** `storytold/deckcraft v0.18.1 (7cc73ab)`: https://github.com/storytold/deckcraft,
  tag `v0.18.1`, which points at commit `7cc73ab1895ed2e30c8afbcada20c70b9ea0a440`
- **How:** `git subtree add --squash` of a cut of that commit: the paths below, byte for byte
  (cut tree `fd6f52d46a6b92adc861546f0a3bd37c4147710f`; rebuild it with `git read-tree` of those
  paths from the commit and compare `git write-tree`).
- **Licence:** MIT OR Apache-2.0, "ArtCraft Team and the DeckCraft contributors". `LICENSE-MIT` and
  `LICENSE-APACHE` here are upstream's; `NOTICE` is upstream's, trimmed (below).

## What is here

| Path | Crate | Why |
|---|---|---|
| `crates/pptx` | deckcraft-pptx | reads `.pptx` packages into the model |
| `crates/render` | deckcraft-render | draws a slide with `vello_cpu` (CPU only) |
| `crates/anim`, `crates/color`, `crates/fonts`, `crates/geom`, `crates/metafile`, `crates/model`, `crates/text` | deckcraft-* | the in-repo crates those two need |
| `Cargo.toml` | (workspace) | workspace inheritance for the crates above |
| `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` | | upstream's licence texts and notice |
| `clippy.toml`, `rustfmt.toml` | | upstream's, so this copy keeps upstream's lints and formatting |

## Left out

- `crates/engine`, `apps/` (the app and `deckcraft-cli`), `crates/media` (it brings symphonia,
  MPL-2.0) and the video codec crates, `crates/pdf`, `crates/format`, `crates/mcp`,
  `crates/ui-egui`, `xtask`: not needed, and they bring the craft-libs git dependencies.
- `Cargo.lock`: upstream's names git sources (the egui fork and craft-libs). K2's root `Cargo.lock`
  pins every version this copy builds with.
- `assets/`, `docs/` (with the ArtCraft brand files), `packaging/`, `licenses/`, `contributors/`,
  `README.md`, `ROADMAP.md`, `ATTRIBUTION.md`, `AGENTS.md`, `CLAUDE.md` and the CI files. No ArtCraft or
  DeckCraft logos are in this copy.

## Local changes (each changed file says so on its first line)

| File | Change |
|---|---|
| `Cargo.toml` | Members are the nine crates above. The `[workspace.dependencies]` the cut doesn't use are removed, with every git source (craft-libs) and the whole `[patch.crates-io]` (the egui and winit forks, krilla). `zip` is `9` (was `3`: RUSTSEC-2026-0367, -0368, -0369, -0429), same features. |
| `NOTICE` | Trimmed to this copy: the copyright and licence lines are kept; the brand, epaint fonts, craft-fonts, Lucide, symphonia, FilmCraft/craft-libs, krilla and egui icon entries (none of that material is here) are removed; the Liberation Sans entry, the test-fixture line and the line saying the epaint_default_fonts fonts were removed are ours. |
| `crates/fonts/Cargo.toml` | `epaint_default_fonts` is removed. |
| `crates/fonts/src/fontdb.rs` | `LAST_RESORT_FAMILY` is `"Liberation Sans"`; `bundled()` and `last_resort_face()` use Liberation Sans 2.1.5 (Regular first, then Bold, Italic, Bold Italic) instead of `epaint_default_fonts::UBUNTU_LIGHT`. Two functions are added: `FontDb::init_global(db)` (install the process-wide database before first use: K2's renderer passes one with no font folders, so it never reads the system's fonts) and `FontDb::add_static_font(&'static [u8])` (add compiled-in font bytes without copying them). |
| `crates/fonts/src/tests.rs` | The concurrent-lookup test installs the repository's `legacy-regular.ttf` fixture (family "DeckCraft Legacy") instead of epaint's Hack; the Embedded OpenType test wraps Liberation Sans instead of Ubuntu Light. |
| `crates/fonts/build.rs` | `CRAFT_FONTS_DIR` or `CRAFT_FONTS_REQUIRED` set is a build error, so `CRAFT_FONTS` is always empty and no other font family can be embedded. |
| `crates/fonts/liberation-fonts-ttf-2.1.5/` | New: Liberation Sans 2.1.5 (below), with our `SHA256SUMS` and `.gitattributes` (`-text`, so git never converts a byte). |
| `crates/pptx/Cargo.toml` | The `deckcraft-engine` dev-dependency is removed. |
| `crates/pptx/src/opc.rs` | zip 9's `ZipFile::name()` returns a `Result`: an entry whose name can't be decoded is skipped (logged) like an unreadable entry. Nothing else in the crate's own code changes for zip 9. |
| `crates/pptx/src/cfb.rs` | The test `a_password_protected_presentation_is_named_as_such` (it opens the file through deckcraft-engine) is removed. |
| `crates/pptx/tests/it/main.rs` | The modules `anchor`, `kept_parts`, `sample`, `smartart`, `sniff` and `tags` are removed with their files: every test in them drives deckcraft-engine. |
| `crates/pptx/tests/it/roundtrip.rs` | The ten tests that drive deckcraft-engine are removed (`text_transparency_round_trips`, `transition_sound_is_set_saved_and_removed`, `transition_sound_options_round_trip`, `chartex_waterfall_reads_and_round_trips`, `chartex_new_charts_save_as_their_own_type`, `chartex_levels_and_quartiles_round_trip`, `radar_stock_and_combo_round_trip`, `reuse_slides_from_another_presentation`, `compress_pictures_shrinks_the_file_and_keeps_shared_originals`, `zoom_inserts_linked_slide_pictures`); the zip 9 `name()` change below. |
| `crates/pptx/tests/chart_data_labels.rs` | The test `chart_options_data_labels_on_overrides_series_off` (deckcraft-engine) is removed; the zip 9 `name()` change below. |
| `crates/pptx/tests/it/malformed.rs` | `fuzz_mutations` mutates a new deck (`deckcraft_model::defaults::new_presentation`) instead of the engine's sample deck; the zip 9 `name()` change below. |
| `crates/pptx/tests/it/corpus_themes.rs` | zip 9's `file_names()` yields `Result`s; the zip 9 `name()` change below. |
| `crates/pptx/tests/it/{anim_auto_reverse,chart_embed,comments,corpus,corpus_kinds,corpus_media,fills,locks,math_ns,media_timing,notes,ole,ole_vml,parent_delays,placeholders,table_styles,transition_xml}.rs` | zip 9's `ZipFile::name()` returns a `Result`: `.name()` is `.name().expect("entry name")`. |

## Test fixtures, CC0-1.0, not shipped

Upstream's three test fonts stay in this copy, for upstream's own tests only:

| Path (under `vendor/deckcraft`) | What |
|---|---|
| `crates/fonts/tests/fixtures/legacy-regular.ttf` | test font, family "DeckCraft Legacy" (Regular) |
| `crates/fonts/tests/fixtures/legacy-semibold.ttf` | the same family, SemiBold |
| `crates/render/tests/fixtures/colr-squares.ttf` | COLRv0 colour-glyph test font |

Test fixtures, CC0-1.0 (DeckCraft contributors, built by the scripts beside them), not shipped.
They are read only by `#[cfg(test)]` modules (`crates/fonts/src/tests.rs`,
`crates/render/src/tests.rs`) and by one `k2-deckrender` integration test
(`tests/render.rs`, as a stand-in embedded font). They must never be bundled: `k2-deckrender`
compiles in only the files `crates/k2-deckrender/fonts/manifest.tsv` lists (sha256-pinned) and
deckcraft-fonts' Liberation Sans, and its packaging ships only the binary.

## Liberation Sans 2.1.5

The last-resort face (and the face the fallback chain already names for Arial, Calibri and
Helvetica), replacing upstream's Ubuntu Light from `epaint_default_fonts` (Ubuntu Font Licence),
which also brought Hack (Bitstream Vera) and egui's icon font. No other font family is compiled in.

- **Source:** https://github.com/liberationfonts/liberation-fonts/releases/tag/2.1.5, the asset
  `liberation-fonts-ttf-2.1.5.tar.gz` (sha256
  `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`).
- **Files:** `LiberationSans-Regular.ttf`, `-Bold.ttf`, `-Italic.ttf`, `-BoldItalic.ttf`, with
  `LICENSE`, `AUTHORS` and `ChangeLog`, each unmodified and whole (no subsetting, no renaming);
  per-file sha256 in `crates/fonts/liberation-fonts-ttf-2.1.5/SHA256SUMS`. K2's `k2-deckrender`
  tests check the compiled-in bytes against those pins.
- **Licence:** SIL Open Font License 1.1. Digitized data copyright (c) 2010 Google Corporation,
  with Reserved Font Arimo, Tinos and Cousine; copyright (c) 2012 Red Hat, Inc., with Reserved Font
  Name Liberation. Never a 1.x release (those were GPL with a font exception).
- **Build:** with `CRAFT_FONTS_DIR` unset (`build.rs` refuses it).

## Updating

Cut the new tag the same way (the paths above), `git subtree merge --squash` it, keep this file,
`NOTICE` and `crates/fonts/liberation-fonts-ttf-2.1.5/`, reapply the changes above, and rerun K2's
licence gate (`crates/k2-deckrender/deny.toml`) at the new versions. Watch upstream for security
fixes in the parser.

# Changelog

## v0.7.1 (2026-10-28)

**Requires `k2-mpc-base` v0.9.1** (additive: `edit_with_options`, the `run_tool` flag, the
`approvals` alias of `log --pending`, one BOM on `write`; fixes for blame's `restored by` on a
partly restored line and for last-edit notes). No pack code change: the double-BOM fix comes
through the kit's `TextFilePack`, and the P30 row may leave the `approvals` alias out. The
demo's transcript now shows both demo findings fixed (`restored by` on scenes 6 and 8; no
`edited lines 25–26` notes), and the demo fails if a partly restored line lacks it.

**A multiplayer demo** (`docs/DEMO.md`): three agents and one person on one Markdown doc through the kit's sessions, headless and deterministic. It covers presence notes; a claim, a bounce and a retry with `--if-version`; a person never refused; a `write --base` merge and a conflict; exact `replace`; undo with provenance; blame with `restored by`; and a revert. The example is `crates/markdown/examples/multiplayer_demo.rs`. `tests/demo_transcript.rs` compares its transcript with `tests/golden/multiplayer_demo.txt`. `docs/DEMO.md` maps each scene to a host's doc CLI, so it can be replayed as a dogfood script. No pack or API change; no new dependency.

## v0.7.0 (2026-10-28)

**Requires `k2-mpc-base` v0.9.0** (undo, redo and revert never remove another participant's text, and text they put back keeps its provenance: blame names who first wrote it and who put it back; a behaviour change in undo and blame, hence the minor bump). The base's **store format is 5** (an in-place upgrade from 4; an older build refuses it). `Cargo.lock` only; no pack change.

## v0.6.2 (2026-10-28)

**Requires `k2-mpc-base` v0.8.2** (the revert fix: a revert's conflicts no longer change across a host restart). `Cargo.lock` only; no pack change.

## v0.6.1 (2026-10-28)

**Requires `k2-mpc-base` v0.8.1** (the undo fix: undo, redo and revert remove only their own text). `Cargo.lock` only; no pack change.

## v0.6.0 (2026-10-28)

**Requires `k2-mpc-base` v0.8.0** (`--if-version V` on every edit verb; the kit strips it before the pack parses argv, so `replace-section` takes it too). The usage line and `docs/PARITY.md` name `[--if-version V]`; `Cargo.lock` moves to base 0.8.0. No other change.

## v0.5.0 (2026-10-28)

**Requires `k2-mpc-base` v0.7.0** (agent presence). P8 lists `cursor`, `claim`, `claim --renew`, `claim --release`; P6 adds `activity`; `docs/PARITY.md` updated. No other change.

## v0.4.0 (2026-10-28)

**Requires `k2-mpc-base` v0.6.0**, whose `replace` also serves P12 (exact text,
`replace --old TEXT --new TEXT [--all]`). The P12 row lists `replace` beside `sed -i`
(`parity.rs`, `docs/PARITY.md`) and `Cargo.lock` moves to base 0.6.0. No other change.

## v0.3.1 (2026-10-27)

**Requires `k2-mpc-base` v0.5.0** (store format 4; P29/P30 gain blob rows, text packs unaffected). Only `Cargo.lock` and the crate version change; no code change.

## v0.3.0 (2026-10-27)

**Requires `k2-mpc-base` v0.4.0.** The base added tier 0's lease rows (P32-P34) to
`PersonAction`: `parity_row` maps them N/A (a Markdown doc merges; leases are tier 0's) and
`parity()` lists the text tier's actions (`PersonAction::for_tier(Tier::Text)`), so the list,
`docs/PARITY.md` and the tests stay P1-P31. No other change.

## v0.2.0 (2026-10-27)

P22 (blame) is live. **Requires `k2-mpc-base` v0.3.0**, checked out beside this repo
(`../k2-mpc-base`).

- **`parity_row(P22)`** is `live(&["blame"])`, and `docs/PARITY.md` names `blame [-L a,b]`
  on `Session::blame`. The kit's three P22 cases run through `tests/agent_parity.rs` on the
  Markdown pack: the tab's blame gutter against the verb, the lines in view against `-L`, and
  both refused without the history right.
- **Negative control:** v0.1.0's deferred P22 row now fails against base v0.3.0
  (`UnexpectedVerb`, `StaleDeferral`), and the live row fails when `blame` is taken out of
  the registry (`MissingVerb`).
- The deferred rows are now annotations (P23, P27) and purge (P28).

## v0.1.0 (2026-10-27)

First release of the Markdown pack for Multi-Player Cores (MPC). It requires `k2-mpc-base`
v0.1.0, checked out beside this repo (`../k2-mpc-base`).

- **`MarkdownPack` (`PACK`):** one text root over the Markdown source. The codec round-trips
  BOM and CRLF byte for byte, with a 4 MiB limit and decline rules; `.md` and `.markdown`,
  not `.mdx`.
- **`replace-section`:** a Markdown-native verb. It handles ATX and setext headings, ignores
  fenced code and front matter, and supports `--nth` and `--include-heading`.
- **Fidelity corpus:** byte-exact through doc and store; a one-word edit changes only that
  word.
- **`docs/PARITY.md` P1–P31 as data,** with `tests/agent_parity.rs` running the base harness.
  The deferred rows (blame, annotations, purge) are asserted absent.
- **`scripts/check.sh`:** tests, the licence gate, and the base's engine-free check.

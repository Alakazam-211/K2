# Changelog

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

# Changelog

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

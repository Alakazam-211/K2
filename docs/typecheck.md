# Type-checking the renderer

`bun run typecheck:web` checks `src/renderer` + `src/shared` against
`tsconfig.web.json`. It runs **[tsc-rs](https://github.com/pingdotgg/ts-rust)**
(npm `tsc-rs`, a Rust port of the TypeScript 7 compiler), pinned to an exact
version in `package.json`. CI (`checks.yml`, `styles-ci.yml`) and the
`release.sh` pre-flight gate run the same script.

- **TypeScript 7 rules.** tsc-rs matches Microsoft's TS 7 (`tsc` 7.1-dev) output,
  not TS 5.x. Newer `lib.dom` types can flag code TS 5.9 accepted.
- **Fallback:** `bun run typecheck:web:tsc` runs the old TS 5.9 `tsc` (the
  `typescript` package stays installed; editors use it). Use it to compare, or on
  a platform tsc-rs has no binary for (0.2.0 ships macOS arm64, Linux x64/arm64
  only; not Intel Mac or Windows).
- **Mismatches:** if tsc-rs and TS 7 disagree (or tsc-rs crashes), report it
  upstream at https://github.com/pingdotgg/ts-rust/issues with a small repro.
  If it blocks a release, switch `typecheck:web` back to `tsc` in one line.
- **Cache:** both checkers write `out/tsconfig.web.tsbuildinfo`; each sees the
  other's file as a version mismatch and does a full check, so switching is safe.
- **Install script:** `tsc-rs` has a `postinstall` that only rewrites its own
  `bin/tsc-rs` launcher into a shell shim (no download). Bun blocks it, since it
  is not in `trustedDependencies`. The plain Node launcher works without it. The
  native binary comes from the optional `@tsc-rs/<platform>` npm package, pinned
  by integrity hash in `bun.lock`.
- **Bumping the pin:** check the new release's `SHA256SUMS` against the npm
  tarballs, then run `typecheck:web` and `typecheck:web:tsc` and compare.

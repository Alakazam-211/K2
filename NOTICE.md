# Third-party notices

| Dependency | Licence | Used for |
|---|---|---|
| [yrs](https://github.com/y-crdt/y-crdt) | MIT | The CRDT engine (Yjs-compatible), private to this crate; **vendored and modified** in `vendor/yrs` (v0.9.3) |
| [thiserror](https://github.com/dtolnay/thiserror) | MIT OR Apache-2.0 | Error types |
| [regex-automata](https://github.com/rust-lang/regex) | MIT OR Apache-2.0 | Linear-time regexes for the text tools (`grep`, `sed`) |
| [redb](https://github.com/cberner/redb) | MIT OR Apache-2.0 | The store (embedded key-value database) |

**Yrs, modified.** `vendor/yrs` is Yrs v0.28.0 with changes by Alakazam Labs (k2-yrs, tag
`k2-yrs-0.28.0-k2.6.3`), under the same MIT terms: Yrs's licence and copyright are in
`vendor/yrs/LICENSE`, and what was changed is in `vendor/yrs/NOTICE` (`Doc::transact_mut_as`; a position index for text; per-transaction
work proportional to the clients touched; hardening; warning-free).
See `docs/YRS-FORK.md`.

Transitive dependencies are all MIT, Apache-2.0, BSD, ISC or Zlib (checked by
`scripts/check-licences.sh`), with one exception: **`unicode-ident`** is
`(MIT OR Apache-2.0) AND Unicode-3.0`, used only at build time by procedural macros.
Unicode-3.0 is an OSI-approved permissive licence.

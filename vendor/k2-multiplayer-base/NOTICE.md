# Third-party notices

| Dependency | Licence | Used for |
|---|---|---|
| [yrs](https://github.com/y-crdt/y-crdt) | MIT | The CRDT engine (Yjs-compatible), private to this crate |
| [similar](https://github.com/mitsuhiko/similar) | Apache-2.0 | Text diffs |
| [thiserror](https://github.com/dtolnay/thiserror) | MIT OR Apache-2.0 | Error types |

Transitive dependencies are all MIT, Apache-2.0, BSD, ISC or Zlib (checked by
`scripts/check-licences.sh`), with one exception: **`unicode-ident`** is
`(MIT OR Apache-2.0) AND Unicode-3.0`, used only at build time by procedural macros.
Unicode-3.0 is an OSI-approved permissive licence.

# Notices

This repository is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README). It holds no third-party code.

Crates it depends on keep their own licences: all permissive, checked transitively at the lock's
exact versions by `scripts/check-licences.sh`. The shipped crate uses the multiplayer kit
(`k2-mpc-base`) and the shared package layer (`k2-mpc-opc`, from `k2-mpc-ooxml`), both
MIT OR Apache-2.0, and `roxmltree`. Tests also use the `zip` crate (MIT) as a second reader and
writer. The fuzz crate (`fuzz/`, its own workspace, never shipped) also uses `libfuzzer-sys`,
`(MIT OR Apache-2.0) AND NCSA`; the gate allows NCSA there alone.

The external test checkers (the Open XML SDK's validator, MIT; Apache POI, Apache-2.0) are not
in this repository: they run from `k2-mpc-ooxml`'s `checkers/` on build nodes only, as separate
processes, and nothing of theirs is linked, shipped or copied.

docx, pptx and xlsx are file formats specified by ECMA-376 and ISO/IEC 29500. This project is
not affiliated with, endorsed by or sponsored by the formats' original vendor.

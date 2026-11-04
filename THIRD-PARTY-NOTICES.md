# Third-party code in this repository

## Board

Portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT.

The object types, field names, field limits, known values and defaults in
`crates/board/src/schema.rs` (and what is generated from it: `schema/board-schema.json` and the
field tables in `docs/FORMAT.md`) are derived from Board's `widget/schema.js`, ported to Rust.

The templates in `crates/board/src/verbs/templates.rs` are ported from the same file. The rules of the
verbs in `crates/board/src/verbs/edit.rs`, `arrange.rs` and `clone.rs` are derived from Board's `bin/board`
and `widget/app.js` and `widget/ui.js`, and the list of actions in `crates/board/src/parity.rs` from its
`PARITY.md`, all ported to Rust; `docs/PARITY.md` is generated from that list. The conversion pairs in
`vectors/widget/`, written by `crates/board/tests/support/cases.rs`, describe Board's widget store
shapes.

MIT License

Copyright (c) 2026 Baden Ballard

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

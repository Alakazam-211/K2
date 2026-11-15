# NOTICE

**K2 by Alakazam Labs** — AI workspace orchestration.
Copyright 2025-2026 Alakazam Labs.

## Licensing

K2 is licensed under the **MIT License**, from 0.46.10 on. See
[LICENSE.md](LICENSE.md). Releases up to and including 0.46.9 were
licensed under FSL-1.1-Apache-2.0 and stay under it; each of those
releases becomes available under Apache 2.0 two years after its release.

## Trademarks

"K2 by Alakazam Labs" and the K2 logo are trademarks of Alakazam Labs;
see [TRADEMARKS.md](TRADEMARKS.md). The MIT License does not grant
trademark rights.

## Third-party: Seti UI file icons (MIT)

The Files drawer uses the **Seti** icon font for file-type glyphs
(same family as VS Code’s default file icon theme).

| | |
|--|--|
| **What** | `seti.woff` + generated name/extension maps under `src/renderer/assets/seti/` and `src/renderer/lib/seti-file-icons/` |
| **Upstream** | [Seti UI](https://github.com/jesseweed/seti-ui) (Jesse Weed), packaged via VS Code `extensions/theme-seti` |
| **License** | **MIT** (Copyright 2014 Jesse Weed) — full text in `src/renderer/assets/seti/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. MIT assets do **not** relicense the product. |
| **Not** | Not affiliated with Microsoft or Visual Studio Code. Do not use those trademarks for K2 branding. |

When redistributing K2 builds that include the Seti font, ship the MIT
notice (this section and/or `THIRD_PARTY_NOTICES.txt`).

## Third-party: LiteLLM model price table (MIT)

The Usage page's estimated API-equivalent cost uses per-token rates from
LiteLLM's `model_prices_and_context_window.json`.

| | |
|--|--|
| **What** | `crates/k2-core/data/llm_prices.json` (a trimmed copy, bundled) and the daemon's daily refresh into `~/.k2/usage/prices.json` |
| **Upstream** | [LiteLLM](https://github.com/BerriAI/litellm) (Berri AI) |
| **License** | **MIT** (Copyright (c) 2023 Berri AI) — full text in `crates/k2-core/data/LLM_PRICES_NOTICE.txt` |
| **K2 code** | Is **MIT**. The MIT data does **not** relicense the product. |

## Third-party: Lucide icons (ISC)

The top bar's icons, the tab Pin / Unpin, the Design tab palette, the
Design note tiles' edit, send and delete, the
Workspace drawer's phone push toggle, the setup checklist's rail button
(icon style A3) and the Mail page's folder and message-action icons are
Lucide icons,
copied in as SVG paths.
Home, Agents, Projects, Tickets, Wiki, Server, Admin profile, Run, Drawer,
the stopwatch clock, the Keep awake mug, Help, Pin, Unpin, the palette, the
pencil, the up arrow, the trash can, the checklist and
the smartphone (on, and off with a slash) are edited (corners squared: square caps, mitred
joins, no corner radius). Settings is Lucide's `settings` gear, unedited. The Gardens ensō
is K2's own (`ZenIcon`), not Lucide.

| | |
|--|--|
| **What** | SVG paths in `src/renderer/components/icons/K2Icons.tsx`, from `lucide-static@1.58.0/icons/` (`house`, `bot`, `layout-dashboard`, `ticket`, `library`, `settings`, `server`, `circle-user`, `play`, `panel-left`, `clock`, `coffee`, `circle-help`, `pin`, `pin-off`, `palette`, `smartphone`, `pencil`, `trash-2`, `arrow-up`, `list-todo`; for Mail `inbox`, `send`, `file-text`, `archive`, `octagon-alert`, `folder`, `folder-input`, `reply`, `reply-all`, `forward`, `star`, `flag`, `paperclip`, `refresh-cw`, `square-pen`, `mail-open`, `download`, `x`) |
| **Upstream** | [Lucide](https://github.com/lucide-icons/lucide) |
| **License** | **ISC** (Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.) — full text below |
| **K2 code** | Is **MIT**. The ISC icons do **not** relicense the product. |
| **Board widget** | Board's `crop` icon (`crates/k2-core/src/zen/builtin_widgets/board-1/icons.js`) is Lucide's; see the Board entry below. Board's other icons are its own drawing. |

> ISC License
>
> Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as
> part of Feather (MIT). All other copyright (c) for Lucide are held by
> Lucide Contributors 2022.
>
> Permission to use, copy, modify, and/or distribute this software for any
> purpose with or without fee is hereby granted, provided that the above
> copyright notice and this permission notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
> WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
> MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
> ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
> WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
> ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
> OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## Third-party: lucide-react, Home group icons (ISC)

The Home group icons and the All groups modal draw each group's icon from a
curated list of about 240 neutral Lucide icons (no brand or company logos),
imported by name from the `lucide-react` npm package and drawn squared
(square caps, mitred joins, stroke 1.75) like the icons above.

| | |
|--|--|
| **What** | npm `lucide-react` **1.73.0** (pinned exactly), the icons named in `src/renderer/lib/group-icons.ts` |
| **Upstream** | [Lucide](https://github.com/lucide-icons/lucide) (`packages/lucide-react`) |
| **License** | **ISC** (Copyright (c) 2026 Lucide Icons and Contributors), with the Feather-derived icons under **MIT** (Copyright (c) 2013-present Cole Bemis) — full text, unedited, in `src/renderer/assets/lucide/LICENSE` |
| **K2 code** | Is **MIT**. The ISC/MIT icons do **not** relicense the product. |

When redistributing K2 builds, ship this notice and/or
`src/renderer/assets/lucide/LICENSE`.

## Contributed: Board (Garden widget) — MIT

The Board Garden's whiteboard widget is **Board by Baden Ballard (MIT)**.

| | |
|--|--|
| **What** | `crates/k2-core/src/zen/builtin_widgets/board-1/` (the `k2:board@1` built-in widget) and `crates/k2-core/src/zen/builtin_widgets/board-file-1/` and `board-file-2/` (the Board tab for `.k5board` files), ported from Board's own widget; and Board code used by the Org Chart in `src/renderer/components/OrgChart/`: `org-camera.ts` and `OrgMinimap.tsx` (adapted from Board's camera and minimap) and `board-geom.ts` (which loads Board's `board-file-1/geom.js`) |
| **Author** | Baden Ballard |
| **License** | **MIT** (granted to K2 in writing by the author, 2026-11-01) |

Copyright (c) 2026 Baden Ballard. Licensed under MIT.

Board's `crop` icon is from Lucide (ISC). Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.

> ISC License
>
> Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## Third-party: Public Suffix List (MPL-2.0) and BIP-39 word list (MIT)

The vault (`crates/k2-vault`) embeds two data files:

| | |
|--|--|
| **Public Suffix List** | `crates/k2-vault/data/public_suffix_list.dat`, from [publicsuffix.org](https://publicsuffix.org/list/public_suffix_list.dat), Mozilla Public License 2.0 (header kept in the file). Used to refuse wildcard hosts over shared suffixes. Refreshed each release. |
| **BIP-39 English word list** | `crates/k2-vault/data/bip39-english.txt`, from [bitcoin/bips](https://github.com/bitcoin/bips/blob/master/bip-0039/english.txt), MIT (per BIP-0039). Used for the 24-word vault recovery phrase. |
| **K2 code** | Is **MIT**. |

## Third-party: wry (Apache-2.0 OR MIT)

K2 vendors a patched copy of **wry** 0.57.0 under `third_party/wry` so a
nil `NSURL` (`URLWithString`, IPC, navigation, `window.open`) cannot abort
the macOS app. crates.io 0.57.0 still unwraps those.

| | |
|--|--|
| **What** | `third_party/wry` (crates.io wry 0.57.0 + nil-URL no-abort patch + macOS IPC from the main frame only + empty-URI-path guard until the Tauri 2.12.5 move) |
| **Upstream** | [wry](https://github.com/tauri-apps/wry) |
| **License** | **Apache-2.0 OR MIT** (Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy) — full text in `third_party/wry/LICENSE-MIT`, `third_party/wry/LICENSE-APACHE`, and `third_party/wry/LICENSE.spdx` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: Sew native voice crates (MIT OR Apache-2.0; Apache-2.0)

K2's desktop app can carry voice calls with Sew's client crates (the `native-voice` build feature), from Sew's public mirror, [Alakazam-211/sew-crates](https://github.com/Alakazam-211/sew-crates), pinned by commit in the root `Cargo.toml`.

| | |
|--|--|
| **What** | `tauri-plugin-sew-voice`, `sew-call-client`, `sew-mls-crypto`, `sew-e2ee`, `sew-proto` (MIT OR Apache-2.0); `sew-lk-compat`, `sew-lkproto` (Apache-2.0) |
| **Upstream** | Alakazam Labs, Sew |
| **Notice** | `sew-lk-compat` and `sew-lkproto` hold code ported from or generated from LiveKit (Apache-2.0, Copyright LiveKit, Inc.). `sew-mls-crypto` holds code derived from openmls_rust_crypto (MIT, Copyright OpenMLS Authors). Each crate's NOTICE ships in the app under `resources/notices/`. Not affiliated with or endorsed by LiveKit, Inc. |
| **K2 code** | Is **MIT**. These crates do **not** relicense the product. |

## Third-party: objc2 (MIT)

K2 vendors a patched copy of **objc2** 0.6.5 under `third_party/objc2` so a
null return from `sel_registerName` cannot abort the macOS app. crates.io
0.6.5 still expects that pointer. The crates.io crate ships no LICENSE file.

| | |
|--|--|
| **What** | `third_party/objc2` (crates.io objc2 0.6.5 + null-selector no-abort patch) |
| **Upstream** | [objc2](https://github.com/madsmtm/objc2) |
| **License** | **MIT** (Copyright (c) Mads Marquart) — full text in `third_party/objc2/LICENSE` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: tao (Apache-2.0)

K2 vendors a patched copy of **tao** 0.37.1 under `third_party/tao` so a null
superclass pointer is not dereferenced. A null `class_getSuperclass` does not
fall back to the receiver's own class. crates.io 0.37.1 still dereferences
that pointer.

| | |
|--|--|
| **What** | `third_party/tao` (crates.io tao 0.37.1 + null-superclass patch) |
| **Upstream** | [tao](https://github.com/tauri-apps/tao) |
| **License** | **Apache-2.0** (Copyright 2021-2023 Tauri Programme within The Commons Conservancy) — full text in `third_party/tao/LICENSE` and `third_party/tao/LICENSE.spdx` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: vte (Apache-2.0 OR MIT)

K2 vendors a patched copy of **vte** 0.15.0 (the terminal parser inside
alacritty_terminal) under `third_party/vte` so a UTF-8 character split
across two PTY reads cannot drop the ASCII byte that follows it. crates.io
0.15.0 still drops it.

| | |
|--|--|
| **What** | `third_party/vte` (crates.io vte 0.15.0 + split-UTF-8 byte-drop fix, `third_party/upstream/vte-0001-partial-utf8-ascii-drop.patch`) |
| **Upstream** | [vte](https://github.com/alacritty/vte) |
| **License** | **Apache-2.0 OR MIT** (Copyright (c) 2016 Joe Wilm) — full text in `third_party/vte/LICENSE-MIT` and `third_party/vte/LICENSE-APACHE` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: Mermaid (MIT)

The file viewer draws Mermaid diagrams with a vendored build of **Mermaid**.

| | |
|--|--|
| **What** | `src/renderer/public/vendor/mermaid.min.js` (Mermaid 11.16.1, upstream `dist/mermaid.min.js` byte for byte, loaded by `components/FileViewerPane/DiagramViewer.tsx`; its inlined DOMPurify is 3.4.0, as upstream built it) |
| **Upstream** | [mermaid](https://github.com/mermaid-js/mermaid) |
| **License** | **MIT** (Copyright (c) 2014 - 2022 Knut Sveidqvist) — full text in `src/renderer/public/vendor/mermaid.LICENSE.txt`. The prebuilt file also carries Mermaid's own dependencies (among them d3, dagre-d3-es, cytoscape, khroma, DOMPurify, KaTeX, marked), each under its own permissive licence. |
| **K2 code** | Is **MIT**. Mermaid does **not** relicense the product. |

## Third-party: CAD file previews (MIT)

The file viewer previews DXF drawings and 3D models (STL, OBJ, 3MF, PLY,
glTF/GLB) with these npm packages, bundled into lazy chunks that load only
when such a file opens.

| | |
|--|--|
| **What** | `three` 0.186.0 (core, OrbitControls, STL/OBJ/3MF/PLY/GLTF loaders, and the **fflate** 0.8.2 copy three ships for 3MF) · `dxf-parser` 1.1.2 (with its dependency `loglevel` 1.9.2). Used by `src/renderer/components/FileViewerPane/` (`DxfViewer.tsx`, `dxfModel.ts`, `ModelViewer.tsx`, `modelParse.ts`, `modelScene.ts`) |
| **Upstream** | [three.js](https://github.com/mrdoob/three.js) · [fflate](https://github.com/101arrowz/fflate) · [dxf-parser](https://github.com/gdsestimating/dxf-parser) · [loglevel](https://github.com/pimterry/loglevel) |
| **License** | **MIT** (Copyright © 2010-2026 three.js authors; Copyright (c) 2023 Arjun Barrett; Copyright (c) 2015 GDS Storefront Estimating; Copyright (c) 2013 Tim Perry) — full texts in `src/renderer/components/FileViewerPane/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. These packages do **not** relicense the product. |

The same three.js version is also vendored for Garden widgets (the Zen
widget library below). That copy is a pre-bundled classic script for sealed
widget frames, ships with the desktop app only, and lacks the 3MF and PLY
loaders, so the file viewer bundles its own ES-module copy.

## Third-party: CodeMirror merge view (MIT)

The Changes tab's diff viewer (inline and side-by-side) uses
`@codemirror/merge`, bundled into a lazy chunk that loads only when a
Changes tab shows a diff.

| | |
|--|--|
| **What** | `@codemirror/merge` 6.12.2, used by `src/renderer/components/Changes/viewer/` |
| **Upstream** | [codemirror/merge](https://github.com/codemirror/merge) |
| **License** | **MIT** (Copyright (C) 2018-2022 by Marijn Haverbeke and others) — full text in the package's `LICENSE` |
| **K2 code** | Is **MIT**. This package does **not** relicense the product. |

## Third-party: T3 Code device-frame numbers (MIT)

The Mobile tab's phone frame is K2's own CSS. Its bezel and corner-radius
fractions per device kind, and the fit rule (fit by height, then clamp to
the width left beside the control rail), are taken from T3 Code. No T3
Code device models or other assets are used; Apple's 3D device models are
not used at all.

| | |
|--|--|
| **What** | Shape numbers from `packages/client-runtime/src/device/shapeProfile.ts` and `fitDeviceFrame` from `apps/web/src/components/device/deviceFrameLayout.ts`, adapted in `src/renderer/components/Mobile/device-frame.ts` |
| **Upstream** | [pingdotgg/t3code](https://github.com/pingdotgg/t3code) |
| **License** | **MIT** (Copyright (c) 2026 T3 Tools Inc.) — full text below |
| **K2 code** | Is **MIT**. This does **not** relicense the product. |

> MIT License
>
> Copyright (c) 2026 T3 Tools Inc.
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Third-party: Wiki 3D view (MIT, BSD-3-Clause)

The Wiki page's 3D map uses these npm packages, bundled into a lazy chunk
that loads only when the 3D view opens (three.js is shared with the file
viewer above).

| | |
|--|--|
| **What** | `react-force-graph-3d` 1.29.2 · `3d-force-graph` 1.80.1 · `three-forcegraph` 1.43.6 · `three-render-objects` 1.43.0 · `three-spritetext` 1.10.0 · `data-bind-mapper` 1.0.3 · `@tweenjs/tween.js` 25.0.0 · `polished` 4.3.1 (+ `@babel/runtime` helpers) · `ngraph.forcelayout` 3.3.1 · `ngraph.graph` 20.1.2 · `ngraph.events` 1.4.0 · `ngraph.random` 1.2.0 · `ngraph.merge` 1.0.0. Used by `src/renderer/components/Wiki/WikiGraph3D.tsx` and `wiki-3d-scene.ts` |
| **Upstream** | [react-force-graph](https://github.com/vasturiano/react-force-graph) · [3d-force-graph](https://github.com/vasturiano/3d-force-graph) · [three-spritetext](https://github.com/vasturiano/three-spritetext) · [ngraph](https://github.com/anvaka/ngraph.forcelayout) · [polished](https://github.com/styled-components/polished) · [tween.js](https://github.com/tweenjs/tween.js) |
| **License** | **MIT** (Copyright (c) 2017-2024 Vasco Asturiano; Brian Hough and Maximilian Stoiber; Sebastian McKenzie and other contributors; Tween.js authors; Andrei Kashcha for ngraph.merge) and **BSD-3-Clause** for ngraph.forcelayout, ngraph.graph, ngraph.events and ngraph.random (Copyright (c) Andrei Kashcha) — full texts in `src/renderer/components/Wiki/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. These packages do **not** relicense the product. |

## Multiplayer kit: k2-mpc-base v0.19.0 (vendored)

Live docs (Markdown, plain text and any file) run on Alakazam Labs' multiplayer kit,
vendored with `git subtree`.

| | |
|--|--|
| **What** | `vendor/k2-mpc-base` (sessions, presence, store, disk sync, history, blame, erasure, text tools, parity, the export contract, the structure index, comments and claims by node), with its own `NOTICE.md` and `CHANGELOG.md`; and its JS client `k2-mpc-client` 0.19.0 (`vendor/k2-mpc-base/client/src`: the library under the app's Live editor, `src/renderer/components/LiveEditor`) |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-base`, tag `v0.19.0` (`b8db699`; crate version `0.19.0`, client `0.19.0`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |

Its Rust dependencies, in K2's `Cargo.lock`:

| Crate | Version | Licence | Used for |
|---|---|---|---|
| [yrs](https://github.com/y-crdt/y-crdt) | 0.28.0 | MIT | The CRDT engine (Yjs-compatible), private to the kit; a modified copy vendored inside the kit (`vendor/k2-mpc-base/vendor/yrs`, a path dependency since kit v0.9.3), with its licence and the changes listed in the kit's `NOTICE.md` |
| [regex-automata](https://github.com/rust-lang/regex) | 0.4 | MIT OR Apache-2.0 | Linear-time regexes for the text tools (`grep`, `sed`); already in the tree |
| [redb](https://github.com/cberner/redb) | 2 | MIT OR Apache-2.0 | The docs store (`~/.k2/docs.redb`); the copy k2-core already links |
| thiserror | 2 | MIT OR Apache-2.0 | Error types |
| dashmap (via yrs) | 6.2.1 | MIT | |
| smallstr (via yrs) | 0.3.1 | MIT OR Apache-2.0 | |

`unicode-ident` is `(MIT OR Apache-2.0) AND Unicode-3.0`, used only at build
time by procedural macros; Unicode-3.0 is OSI-approved and permissive.

The JS client adds no npm dependency, and since client 0.12.0 it has no peer
dependencies either: it is a library with no DOM. The Live editor is K2's own code, on
the app's own `@codemirror/*` packages (`view`, `state`, `commands`, `language`,
`lang-markdown`; MIT).

## Multiplayer kit: k2-mpc-markdown v0.7.22 (vendored)

The Markdown type pack for the kit (schema, file codec, its own verbs such as
`replace-section`, the parity list), vendored beside it.

| | |
|--|--|
| **What** | `vendor/k2-mpc-markdown` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-markdown`, tag `v0.7.22` (`495631c`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | k2-mpc-base only (above): yrs (MIT), regex-automata, redb and thiserror (MIT OR Apache-2.0), and `unicode-ident` (build time only, Unicode-3.0) |

## Multiplayer kit: k2-mpc-text v0.2.24 (vendored)

The plain-text type pack for the kit (code, config, markup, diagram sources,
`.txt`, and `.csv`/`.tsv` as text: a per-extension file-type table, a
byte-exact codec, the text tier's parity list), vendored beside it.

| | |
|--|--|
| **What** | `vendor/k2-mpc-text` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-text`, tag `v0.2.24` (`9f24dba`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | k2-mpc-base (above) and serde_json (MIT OR Apache-2.0; strict JSON for the merges of JSON types, already in K2's tree); its tests use proptest (MIT OR Apache-2.0) |

## Multiplayer kit: k2-mpc-board v0.1.12 (vendored)

The `.k5board` file format and its pack (a strict zip reader and writer,
canonical JSON, typed refusals, edits with per-object revisions, the agent
verbs, a three-way merge, and a bridge to the kit's verb features), vendored
beside the kit.

| | |
|--|--|
| **What** | `vendor/k2-mpc-board`, with its own `NOTICE.md`, `THIRD-PARTY-NOTICES.md` and `CHANGELOG.md`. Its `fuzz/` workspace isn't built by K2 |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-board`, tag `v0.1.12` (`7f5754a`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Credit** | Parts of this crate come from Board, Copyright (c) 2026 Baden Ballard, MIT: the schema tables, the templates, the verbs' rules, the parity list, and the shapes the widget conversion pairs describe. See `THIRD-PARTY-NOTICES.md` for which files, and the licence. |
| **Dependencies** | miniz_oxide (MIT OR Zlib OR Apache-2.0), sha2 (MIT OR Apache-2.0); its `bridge/` adds the kit (k2-mpc-base, above) |

## Multiplayer kit: k2-mpc-psd v0.1.7 (vendored)

The kit's pack that reads PSD and PSB files: each version is read into the
kit's structure index (a tree of layers), streaming, never holding pixels; it
never writes a file. Vendored beside the kit from its public cut, which has no
`fuzz/` and no test-only checker scripts.

| | |
|--|--|
| **What** | `vendor/k2-mpc-psd` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-psd`, tag `v0.1.7` (the cut `b7fa53e`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Credit** | Its file reader is a copy of the PSD crate of PhotoCraft 0.9.0 (Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors, MIT OR Apache-2.0), with the changes listed in `vendor/k2-mpc-psd/vendor/photocraft-psd/VENDORED.md` (since v0.1.2 also one change from upstream commit `a7301f1d`: the `UnFl` count's reservation cap); its licence texts and `NOTICE` are beside it |
| **Dependencies** | flate2 (MIT OR Apache-2.0, with miniz_oxide: MIT OR Zlib OR Apache-2.0), thiserror (MIT OR Apache-2.0), and the kit (above) |

## Multiplayer kit: k2-mpc-ooxml v0.1.1 (vendored)

The package layer the kit's office-file packs share (docx, pptx and xlsx): a bounded zip
reader and byte-exact writer, content types, relationships, and a three-way package merge.
Vendored beside the kit from its public cut, which has no `fuzz/` and no external test
checkers.

| | |
|--|--|
| **What** | `vendor/k2-mpc-ooxml` (the crate `k2-mpc-opc` 0.1.1), with its own `NOTICE.md` and `CHANGELOG.md`. `corpus/` is a manifest of test files fetched for a test run, never committed or built |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-ooxml`, tag `v0.1.1` (the cut `2362f7c`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | roxmltree 0.21 (MIT OR Apache-2.0), flate2 (MIT OR Apache-2.0, with miniz_oxide: MIT OR Zlib OR Apache-2.0), crc32fast (MIT OR Apache-2.0) |

## Multiplayer kit: k2-mpc-pptx v0.1.5 (vendored)

The kit's pack for `.pptx` decks: a deck as a live keyed doc (slides and shapes by their own
ids), agent verbs, and the file written back by splicing only what changed into its own
bytes. In K2 it runs behind the Slides core (Settings → Cores → Multiplayer). Vendored beside
the kit from its public cut, which has no `fuzz/`.

| | |
|--|--|
| **What** | `vendor/k2-mpc-pptx` (depends on `../k2-mpc-base/crates/base` and `../k2-mpc-ooxml/crates/opc`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-pptx`, tag `v0.1.5` (the cut `1482b6c`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | the kit and k2-mpc-opc (above), and roxmltree 0.21 (MIT OR Apache-2.0) |
| **Formats** | docx, pptx and xlsx are file formats specified by ECMA-376 and ISO/IEC 29500. The pack is not affiliated with, endorsed by or sponsored by the formats' original vendor |

Update these sections on every kit bump.

## Third-party: deckcraft v0.18.1, for .pptx previews (MIT OR Apache-2.0, vendored)

K2 draws previews of `.pptx` slides with the PPTX reader and the CPU renderer of
**DeckCraft**, vendored (a `git subtree`, squashed), run only inside the separate
`k2-deckrender` process.

| | |
|--|--|
| **What** | `vendor/deckcraft`: the crates deckcraft-pptx and deckcraft-render and the in-repo crates they need (anim, color, fonts, geom, metafile, model, text), with upstream's `NOTICE` (trimmed) and `VENDORED.md` |
| **Upstream** | `storytold/deckcraft v0.18.1 (7cc73ab)`: [storytold/deckcraft](https://github.com/storytold/deckcraft), tag `v0.18.1`, commit `7cc73ab1895ed2e30c8afbcada20c70b9ea0a440` |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 ArtCraft Team and the DeckCraft contributors), full texts in `vendor/deckcraft/LICENSE-MIT` and `vendor/deckcraft/LICENSE-APACHE` |
| **Local changes** | Listed in `vendor/deckcraft/VENDORED.md`, and each changed file says so on its first line: the last-resort font is Liberation Sans 2.1.5 (below) in place of `epaint_default_fonts`, and craft-fonts input is refused; zip 9 in place of zip 3; the workspace keeps only these crates, with no git sources and no `[patch]`; the tests that need deckcraft-engine (not vendored) are removed |
| **Not included** | deckcraft-engine, the CLI and app, deckcraft-media and the codec crates, and every asset and brand file. K2 is not affiliated with or endorsed by the ArtCraft Team |
| **K2 code** | Is **MIT**. The vendored crates do **not** relicense the product. |

## Third-party: Liberation Sans 2.1.5 (SIL Open Font License 1.1)

The `.pptx` previews' last-resort font, compiled into `k2-deckrender` (through the vendored
deckcraft-fonts): the four Liberation Sans styles of liberation-fonts **2.1.5**, unmodified and
whole.

| | |
|--|--|
| **What** | `vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/` (`LiberationSans-Regular.ttf`, `-Bold.ttf`, `-Italic.ttf`, `-BoldItalic.ttf`, with upstream's `LICENSE`, `AUTHORS` and `ChangeLog`; every file pinned by sha256 in `SHA256SUMS` there) |
| **Upstream** | [liberationfonts/liberation-fonts](https://github.com/liberationfonts/liberation-fonts), release 2.1.5 (`liberation-fonts-ttf-2.1.5.tar.gz`, sha256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`) |
| **Copyright** | Digitized data copyright (c) 2010 Google Corporation, with Reserved Font Arimo, Tinos and Cousine. Copyright (c) 2012 Red Hat, Inc., with Reserved Font Name Liberation. |
| **License** | **SIL Open Font License, Version 1.1**, full text in `vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/LICENSE` and below. The fonts are not renamed, subset or modified; "Liberation" is a Reserved Font Name |
| **K2 code** | Is **MIT**. The font files keep their own licence and do **not** relicense the product. |

<details>
<summary>SIL Open Font License, Version 1.1</summary>

```
Digitized data copyright (c) 2010 Google Corporation
	with Reserved Font Arimo, Tinos and Cousine.
Copyright (c) 2012 Red Hat, Inc.
	with Reserved Font Name Liberation.

This Font Software is licensed under the SIL Open Font License,
Version 1.1.

This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007

PREAMBLE The goals of the Open Font License (OFL) are to stimulate
worldwide development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to provide
a free and open framework in which fonts may be shared and improved in
partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves.
The fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works.  The fonts and derivatives,
however, cannot be released under any other type of license.  The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

 

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such.
This may include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components
as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting ? in part or in whole ?
any of the components of the Original Version, by changing formats or
by porting the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical writer
or other person who contributed to the Font Software.


PERMISSION & CONDITIONS

Permission is hereby granted, free of charge, to any person obtaining a
copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,in
   Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
   redistributed and/or sold with any software, provided that each copy
   contains the above copyright notice and this license. These can be
   included either as stand-alone text files, human-readable headers or
   in the appropriate machine-readable metadata fields within text or
   binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
   Name(s) unless explicit written permission is granted by the
   corresponding Copyright Holder. This restriction only applies to the
   primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
   Software shall not be used to promote, endorse or advertise any
   Modified Version, except to acknowledge the contribution(s) of the
   Copyright Holder(s) and the Author(s) or with their explicit written
   permission.

5) The Font Software, modified or unmodified, in part or in whole, must
   be distributed entirely under this license, and must not be distributed
   under any other license. The requirement for fonts to remain under
   this license does not apply to any document created using the Font
   Software.


 
TERMINATION
This license becomes null and void if any of the above conditions are not met.

 

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT.  IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER
DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: the .pptx renderer's fonts (SIL Open Font License 1.1)

`k2-deckrender` draws slides with these fonts only (never the system's), compiled into the
binary from `crates/k2-deckrender/fonts/` (Liberation Sans: through the vendored deckcraft-fonts,
`vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/`). Every file is upstream's, byte for
byte: no subsetting, no WOFF2, no renaming, no table edits. `crates/k2-deckrender/fonts/manifest.tsv`
lists each file with its sha256, and `tests/fonts.rs` checks every file against it. Approved by the
Kessel Steward (2026-11-11); any other font goes to the Steward first. The OFL text is above (Liberation
Sans) and beside each family as shipped (`OFL.txt`, Liberation's `LICENSE`).

| Family | Copyright and Reserved Font Names | Licence files beside the fonts |
|---|---|---|
| Liberation Sans, Serif, Mono 2.1.5 | Digitized data copyright (c) 2010 Google Corporation, with Reserved Font Arimo, Tinos and Cousine. Copyright (c) 2012 Red Hat, Inc., with Reserved Font Name Liberation. | `LICENSE` (OFL-1.1), `AUTHORS`, `ChangeLog` |
| Carlito 1.104 | Copyright 2013 The Carlito Project Authors (https://github.com/googlefonts/carlito), with Reserved Font Name "Carlito". | `OFL.txt` |
| Caladea 1.001 | Copyright 2012 The Caladea Project Authors (https://github.com/huertatipografica/Caladea). No Reserved Font Name. | `OFL.txt`, `AUTHOR.txt`, `CONTRIBUTORS.txt` |
| Gelasio 1.008 | Copyright 2022 The Gelasio Project Authors (https://github.com/SorkinType/Gelasio). No Reserved Font Name. | `OFL.txt` |
| Roboto 3.015 | Copyright 2011 The Roboto Project Authors (https://github.com/googlefonts/roboto-classic). No Reserved Font Name. | `OFL.txt` |
| Poppins 4.004 | Copyright 2020 The Poppins Project Authors (https://github.com/itfoundry/Poppins). No Reserved Font Name. | `OFL.txt` |
| Noto Sans 2.015, Noto Serif 2.015, Noto Sans Mono 2.014, Noto Sans Symbols 2.003, Noto Sans Symbols 2 2.008, Noto Sans Math 3.000 | Copyright 2022 The Noto Project Authors (https://github.com/notofonts/latin-greek-cyrillic, …/symbols, …/math). No Reserved Font Name. | `OFL.txt` |

Every file (name, family and style, version, sha256, source):

| File | Family, style | Version | sha256 | Source |
|---|---|---|---|---|
| `LiberationSans-Regular.ttf` | Liberation Sans, Regular | 2.1.5 | `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-Bold.ttf` | Liberation Sans, Bold | 2.1.5 | `788abee4c806d660e8aee46689dd8540cd4bb98da03dcc9d171ce3efd99a9173` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-Italic.ttf` | Liberation Sans, Italic | 2.1.5 | `e5bae5c4cde31f22142753855f4f8fb86da6ff39955ed3c0a11248b0d16948b0` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-BoldItalic.ttf` | Liberation Sans, Bold Italic | 2.1.5 | `698da70fc191cc5f33ad4d6d3fe830fe4624b898ea2e3169955928b7c491f1ee` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationMono-Bold.ttf` | Liberation Mono, Bold | 2.1.5 | `bd62a0672d0b9b6710b01df434c80ad54fa5f0835207eb7b17b7a761463067bb` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Bold.ttf |
| `LiberationMono-BoldItalic.ttf` | Liberation Mono, Bold Italic | 2.1.5 | `79451f3c09fe25116098853b7a2ca6e2436220ccc11af022979adbcf195be130` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-BoldItalic.ttf |
| `LiberationMono-Italic.ttf` | Liberation Mono, Italic | 2.1.5 | `605c01c711b44480a7508d349dfbf3264e81fa43d69e61cfa7d10b86e764c4d1` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Italic.ttf |
| `LiberationMono-Regular.ttf` | Liberation Mono, Regular | 2.1.5 | `f2b83c763e8afd21709333370bed4774337fae82267937e2b5aea7e2fbd922c1` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Regular.ttf |
| `LiberationSerif-Bold.ttf` | Liberation Serif, Bold | 2.1.5 | `d754ba427cfe0bca54ae052384baa8f842da5bd6550ad4da024ac441e7a7d5ce` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Bold.ttf |
| `LiberationSerif-BoldItalic.ttf` | Liberation Serif, Bold Italic | 2.1.5 | `f17db8af71e24d2066b587546021d4f0b296be389512b658dec3c09affeb11a7` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-BoldItalic.ttf |
| `LiberationSerif-Italic.ttf` | Liberation Serif, Italic | 2.1.5 | `0e3dea9f8d613e006ccfa62201f33e265d19167bd0907725c3e145368b04fc2e` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Italic.ttf |
| `LiberationSerif-Regular.ttf` | Liberation Serif, Regular | 2.1.5 | `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Regular.ttf |
| `Carlito-Bold.ttf` | Carlito, Bold | 1.104 | `bb5d20f79b82599ec72983597437373a80f2d2085fa91fc144fd74e876a594db` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Bold.ttf |
| `Carlito-BoldItalic.ttf` | Carlito, Bold Italic | 1.104 | `b32928186c119599e03ca6a1ffc680fdcb7fac95772f4b95d989cf6cd3861517` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-BoldItalic.ttf |
| `Carlito-Italic.ttf` | Carlito, Italic | 1.104 | `0b019225e58d702bfedcbd35c21696769f8ee115cb6343f84c2f240312450d1c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Italic.ttf |
| `Carlito-Regular.ttf` | Carlito, Regular | 1.104 | `f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Regular.ttf |
| `Caladea-Bold.ttf` | Caladea, Bold | 1.001 | `ae3cb2dcbc925809dd29d2a44e9802211cab66be541bacbfc9c08c74b27c3742` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Bold.ttf |
| `Caladea-BoldItalic.ttf` | Caladea, Bold Italic | 1.001 | `ccabaa7b7e2fdf253d2b1a5fa699dd8a3df8d835a9eb285ad82631a677eb76c0` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-BoldItalic.ttf |
| `Caladea-Italic.ttf` | Caladea, Italic | 1.001 | `4359a8e24f748b6447b1ff6d7a174febe70961d29f8bb8634b56dacd740a3deb` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Italic.ttf |
| `Caladea-Regular.ttf` | Caladea, Regular | 1.001 | `f1e899278b7b4491aba5b6a8253c4b04c050cc59b21865be5c37559a775153cd` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Regular.ttf |
| `Gelasio-Italic[wght].ttf` | Gelasio, Italic | 1.008 | `52559e845a4d33514e5f93bb9ae7dbeae1894a53f2c565a15f18af40cd337c09` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/gelasio/Gelasio-Italic%5Bwght%5D.ttf |
| `Gelasio[wght].ttf` | Gelasio, Regular | 1.008 | `4daecea457258c9ebeb8bc99ed3fd24353618bfad3ea4b93fa0b5d0468fc04e4` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/gelasio/Gelasio%5Bwght%5D.ttf |
| `Roboto-Italic[wdth,wght].ttf` | Roboto, Italic | 3.015 | `9725a847af6b460ffca162ae66d20dad48b01876137947180b42d7dcd7887182` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/roboto/Roboto-Italic%5Bwdth,wght%5D.ttf |
| `Roboto[wdth,wght].ttf` | Roboto, Regular | 3.015 | `d7598e12c5dbef095ff8272cfc55da0250bd07fbdecbac8a530b9b277872a134` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/roboto/Roboto%5Bwdth,wght%5D.ttf |
| `Poppins-Black.ttf` | Poppins, Black | 4.004 | `d82aaaf98a9283f9a8edd24e51173337d8eaf09e25cd3d98831f8ec8461748a1` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Black.ttf |
| `Poppins-BlackItalic.ttf` | Poppins, Black Italic | 4.004 | `f4852ca89c29f69d800e14f097ed4d1f0a0cc454e9f77d73bfd6db1f71c287a0` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-BlackItalic.ttf |
| `Poppins-Bold.ttf` | Poppins, Bold | 4.004 | `983676516167748b74de6f4771fb384c664fd913acb8b471122ecacf5da5ea6c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Bold.ttf |
| `Poppins-BoldItalic.ttf` | Poppins, Bold Italic | 4.004 | `3572ac8116a0ac7317d342262b29937bcbaf94d8f03f90df6fe666fa7e2fb43a` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-BoldItalic.ttf |
| `Poppins-ExtraBold.ttf` | Poppins, ExtraBold | 4.004 | `f2ab17c1a63a0ecc12c2461848fc8a469395e3cd2d641803e889c643d9f958e1` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraBold.ttf |
| `Poppins-ExtraBoldItalic.ttf` | Poppins, ExtraBold Italic | 4.004 | `dc00a2eb988373e9f4e99bf8ff76c6315ee21d36b341cbbff024e4c18cc7ac03` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraBoldItalic.ttf |
| `Poppins-ExtraLight.ttf` | Poppins, ExtraLight | 4.004 | `55c03314cc754e26f741f97890e5e9cbe3b3278fe3abcecfecafd60111b2643d` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraLight.ttf |
| `Poppins-ExtraLightItalic.ttf` | Poppins, ExtraLight Italic | 4.004 | `7171a17e4cda8f6ded78a7a931f9b7f38987057f3ffd1fe07a71528ad0f01e49` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraLightItalic.ttf |
| `Poppins-Italic.ttf` | Poppins, Italic | 4.004 | `4fa76ae75b40f926420514044722cb97f32186cafd3b38263cc34dad7174d46d` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Italic.ttf |
| `Poppins-Light.ttf` | Poppins, Light | 4.004 | `650ba57fa99d12ec40c31ccfb680be656be4497fbe14164617d67e32ffe9cd46` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Light.ttf |
| `Poppins-LightItalic.ttf` | Poppins, Light Italic | 4.004 | `b8f9c5be59723fadf8e5447fa1245c2c53b60a3464a24d6ece9ee3c283d8917b` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-LightItalic.ttf |
| `Poppins-Medium.ttf` | Poppins, Medium | 4.004 | `90373e7d838d32468438fc3e152dca0bdb12edcab99ea639f158790b1ba1fd05` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Medium.ttf |
| `Poppins-MediumItalic.ttf` | Poppins, Medium Italic | 4.004 | `765addf6c7c11ec3c54325cacd68cabd05df8e4b6455302d812a9b9bafd1c614` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-MediumItalic.ttf |
| `Poppins-Regular.ttf` | Poppins, Regular | 4.004 | `7e65201e9b79159e2300267cc885e16c8dcef2424cdfa09a29bfb0980a94a7ba` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Regular.ttf |
| `Poppins-SemiBold.ttf` | Poppins, SemiBold | 4.004 | `d3bf1bdaf0550e83da9ac0b1d1d9fe6db086835a83aa28578e609a394b9a0286` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-SemiBold.ttf |
| `Poppins-SemiBoldItalic.ttf` | Poppins, SemiBold Italic | 4.004 | `16bb118aa232c9a13fa238027d24d7854dd1a1d9cbaf99b17fec4388d56b432c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-SemiBoldItalic.ttf |
| `Poppins-Thin.ttf` | Poppins, Thin | 4.004 | `6d8e5d9d29140cc93e321745fa1243c67889e6bc3639ec34db64f3da7a496352` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Thin.ttf |
| `Poppins-ThinItalic.ttf` | Poppins, Thin Italic | 4.004 | `46df80ac970f5e84829b868d283878b4d97e289c7b1245541d8bbf66b5d670a8` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ThinItalic.ttf |
| `NotoSans-Italic[wdth,wght].ttf` | Noto Sans, Italic | 2.015 | `0c9c7620381d710ac16269b1f1756f9ee05fa4c3bf7293ff04ef6723fcddbe9f` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip#NotoSans/unhinted/variable-ttf/NotoSans-Italic[wdth,wght].ttf |
| `NotoSans[wdth,wght].ttf` | Noto Sans, Regular | 2.015 | `511cc04b442ffcc3b24d65c2cc780bdffaf482b20ba0aebdc8f27b37017ff0f5` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip#NotoSans/unhinted/variable-ttf/NotoSans[wdth,wght].ttf |
| `NotoSerif-Italic[wdth,wght].ttf` | Noto Serif, Italic | 2.015 | `e5c3ade5b59b3af8c48c24915580cab99f083bd728a81c343741fc09a0c76c9f` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSerif-v2.015/NotoSerif-v2.015.zip#NotoSerif/unhinted/variable-ttf/NotoSerif-Italic[wdth,wght].ttf |
| `NotoSerif[wdth,wght].ttf` | Noto Serif, Regular | 2.015 | `8918809882ffe61eb32baf3f4b3420fda6c022f2219b1b0272755d70ecabc7de` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSerif-v2.015/NotoSerif-v2.015.zip#NotoSerif/unhinted/variable-ttf/NotoSerif[wdth,wght].ttf |
| `NotoSansMono-Bold.ttf` | Noto Sans Mono, Bold | 2.014 | `1100772b2f79c102402a1011df5e2226517d0f40ac553a25fb12fdbad8c11b85` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSansMono-v2.014/NotoSansMono-v2.014.zip#NotoSansMono/unhinted/ttf/NotoSansMono-Bold.ttf |
| `NotoSansMono-Regular.ttf` | Noto Sans Mono, Regular | 2.014 | `87f8ce0522a6c99b743ee5fc75b4073cfdd575639119672828b7b9944b65b4f4` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSansMono-v2.014/NotoSansMono-v2.014.zip#NotoSansMono/unhinted/ttf/NotoSansMono-Regular.ttf |
| `NotoSansSymbols[wght].ttf` | Noto Sans Symbols, Regular | 2.003 | `9dc0ef3c6eea7436e064409ae9d0e3cc1928e21ec5c1baf43096cfad208b2d0a` | https://github.com/notofonts/symbols/releases/download/NotoSansSymbols-v2.003/NotoSansSymbols-v2.003.zip#NotoSansSymbols/unhinted/variable-ttf/NotoSansSymbols[wght].ttf |
| `NotoSansSymbols2-Regular.ttf` | Noto Sans Symbols 2, Regular | 2.008 | `c4a0a80f0041ce4be81e2478faad22776d23edb98ae3f0d19bd37044820ecf9d` | https://github.com/notofonts/symbols/releases/download/NotoSansSymbols2-v2.008/NotoSansSymbols2-v2.008.zip#NotoSansSymbols2/unhinted/ttf/NotoSansSymbols2-Regular.ttf |
| `NotoSansMath-Regular.ttf` | Noto Sans Math, Regular | 3.000 | `b127e84699212b6b2ef50aff58e0ebebeec04ffe6db1b9eb9e209c8c3d97b4aa` | https://github.com/notofonts/math/releases/download/NotoSansMath-v3.000/NotoSansMath-v3.000.zip#NotoSansMath/unhinted/ttf/NotoSansMath-Regular.ttf |

Sources: Liberation from the liberation-fonts 2.1.5 release tarball (sha256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`); Carlito, Gelasio, Roboto and Poppins from google/fonts at commit `86aced91de05cd4f96a06838124cca45be337b2b` (`ofl/`); Caladea from huertatipografica/Caladea at commit `336a529cfad3d103d6527752686f8331d13e820a` (`fonts/ttf/`); Noto from the notofonts release zips named in each row (their `unhinted` builds). Zip sha256: NotoSans-v2.015 `0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5`, NotoSerif-v2.015 `0e9a43c8a4b94ac76f55069ed1d7385bbcaf6b99527a94deb5619e032b7e76c1`, NotoSansMono-v2.014 `090cf6c5e03f337a755630ca888b1fef463e64ae7b33ee134e9309c05f978732`, NotoSansSymbols-v2.003 `0c113cdcf6c31d050b80dac39fba2d804a6985281012e76e9220c0a00da007f3`, NotoSansSymbols2-v2.008 `346c930bbe8eb946701a05c54e9c11a2094dee1d93c387bf1771c0a3e335688f`, NotoSansMath-v3.000 `ac351837b41f8a897f020b97fb0f075ad574c1e9669fb5839ada1f92fd748356`.
K2's code is **MIT**. The font files keep their own licence and do **not** relicense the product.

## Third-party: jmap-webmail, in the mail reader (MIT)

The Mail tab's reader views (`sdk/mail-reader`, bundled into the app) are adapted from **jmap-webmail** by root-fr, by way of Alakazam Labs' adaptation of it. Which files, and what changed: `sdk/mail-reader/UPSTREAM.md`.

| | |
|--|--|
| **What** | `sdk/mail-reader/src/` (types, thread grouping, the sanitizer config, the folder list, email list, thread and message views) |
| **Upstream** | [jmap-webmail](https://github.com/root-fr/jmap-webmail) v1.7.3 |
| **License** | **MIT**, text below (also in `sdk/mail-reader/NOTICE`) |

> MIT License
>
> Copyright (c) 2025 Matthieu MALVACHE
> Copyright (c) 2026 Alakazam Labs
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Third-party: Stalwart mail server (AGPL-3.0-only)

Hosted mail (Email Hosting) and Email Notifications on Linux run the
**Stalwart** mail server as a separate program K2 installs and supervises.

| | |
|--|--|
| **What** | The `stalwart` server binary, version **0.16.25**, downloaded unmodified (sha256-checked) at install time from the release `v0.16.25` of [github.com/Alakazam-211/stalwart](https://github.com/Alakazam-211/stalwart), which mirrors upstream [stalwartlabs/stalwart](https://github.com/stalwartlabs/stalwart) byte for byte |
| **License** | **AGPL-3.0-only** (the Community Edition). No Enterprise licence key is ever installed |
| **Source** | The complete corresponding source is the `v0.16.25` tag: https://github.com/Alakazam-211/stalwart/tree/v0.16.25 (also printed by `k2 study email-notifications` and `k2 notifications email status`) |
| **How K2 uses it** | As a separate process over its loopback HTTP/JMAP management API only. Stalwart's code is **never linked, vendored or patched** into K2 |
| **K2 code** | Is **MIT**. Running Stalwart beside K2 as a separate program does **not** put K2 under the AGPL or otherwise relicense the product |

When redistributing K2 builds or boxes that install Stalwart, keep this
notice and the source pointer with them.

## Third-party: Zen widget library

<!-- zen-lib:begin (generated from src/shared/zen-lib.json by scripts/vendor-zen-lib.sh; do not edit by hand) -->

Libraries a Garden widget can ask for with `requires.libs` (the Zen widget
standard library). K2 inlines each one into the sealed widget frame; the frame
itself never touches the network. Each library folder keeps its full licence
text in `LICENSE.txt` (Apache-2.0 libraries include their upstream NOTICE).

| Library | Version | Licence | Copyright | Where |
|--|--|--|--|--|
| three.js (`three`) | 0.186.0 | MIT | Copyright © 2010-2026 three.js authors | shipped in the app, `src/renderer/zen-lib/three@0.186.0/` |
| globe.gl (`globe.gl`) | 2.46.2 | MIT | Copyright (c) 2019 Vasco Asturiano | shipped in the app, `src/renderer/zen-lib/globe.gl@2.46.2/` |
| cannon-es (`cannon-es`) | 0.20.0 | MIT | Copyright (c) 2015 cannon.js Authors | shipped in the app, `src/renderer/zen-lib/cannon-es@0.20.0/` |
| PixiJS (`pixi.js`) | 8.21.0 | MIT | Copyright (c) 2013-2023 Mathew Groves, Chad Engler | shipped in the app, `src/renderer/zen-lib/pixi.js@8.21.0/` |
| Phaser (`phaser`) | 4.2.1 | MIT | Copyright (c) 2026 Richard Davey, Phaser Studio Inc. | shipped in the app, `src/renderer/zen-lib/phaser@4.2.1/` |
| KAPLAY (`kaplay`) | 3001.0.19 | MIT | Copyright (c) 2025, KAPLAY Team and contributers | shipped in the app, `src/renderer/zen-lib/kaplay@3001.0.19/` |
| Matter.js (`matter-js`) | 0.20.0 | MIT | Copyright (c) Liam Brummitt and contributors. | shipped in the app, `src/renderer/zen-lib/matter-js@0.20.0/` |
| D3 (`d3`) | 7.9.0 | ISC | Copyright 2010-2023 Mike Bostock | shipped in the app, `src/renderer/zen-lib/d3@7.9.0/` |
| Chart.js (`chart.js`) | 4.5.1 | MIT | Copyright (c) 2014-2024 Chart.js Contributors | shipped in the app, `src/renderer/zen-lib/chart.js@4.5.1/` |
| Apache ECharts (`echarts`) | 6.1.0 | Apache-2.0 | Copyright 2017-2026 The Apache Software Foundation; This product includes software developed at The Apache Software Foundation (https://www.apache.org/). | shipped in the app, `src/renderer/zen-lib/echarts@6.1.0/` |
| uPlot (`uplot`) | 1.6.32 | MIT | Copyright (c) 2022 Leon Sorokin | shipped in the app, `src/renderer/zen-lib/uplot@1.6.32/` |
| Anime.js (`animejs`) | 4.5.0 | MIT | Copyright (c) 2025 Julian Garnier | shipped in the app, `src/renderer/zen-lib/animejs@4.5.0/` |
| Lottie (`lottie-web`) | 5.13.0 | MIT | Copyright (c) 2015 Bodymovin | shipped in the app, `src/renderer/zen-lib/lottie-web@5.13.0/` |
| perfect-freehand (`perfect-freehand`) | 1.2.3 | MIT | Copyright (c) 2021 Stephen Ruiz Ltd | shipped in the app, `src/renderer/zen-lib/perfect-freehand@1.2.3/` |
| Rough.js (`roughjs`) | 4.6.6 | MIT | Copyright (c) 2019 Preet Shihn | shipped in the app, `src/renderer/zen-lib/roughjs@4.6.6/` |
| howler.js (`howler`) | 2.2.4 | MIT | Copyright (c) 2013-2020 James Simpson and GoldFire Studios, Inc. | shipped in the app, `src/renderer/zen-lib/howler@2.2.4/` |
| Tone.js (`tone`) | 15.1.22 | MIT | Copyright (c) 2014-2020 Yotam Mann | shipped in the app, `src/renderer/zen-lib/tone@15.1.22/` |
| Leaflet (`leaflet`) | 1.9.4 | BSD-2-Clause | Copyright (c) 2010-2023, Volodymyr Agafonkin | shipped in the app, `src/renderer/zen-lib/leaflet@1.9.4/` |
| marked (`marked`) | 18.0.14 | MIT | Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/) | shipped in the app, `src/renderer/zen-lib/marked@18.0.14/` |
| DOMPurify (`dompurify`) | 3.4.17 | Apache-2.0 | (c) Cure53 and other contributors | shipped in the app, `src/renderer/zen-lib/dompurify@3.4.17/` |
| highlight.js (`highlight.js`) | 11.12.0 | BSD-3-Clause | Copyright (c) 2006, Ivan Sagalaev. | shipped in the app, `src/renderer/zen-lib/highlight.js@11.12.0/` |
| KaTeX (`katex`) | 0.18.9 | MIT | Copyright (c) 2013-2020 Khan Academy and other contributors | shipped in the app, `src/renderer/zen-lib/katex@0.18.9/` |
| Preact + htm (`preact`) | 10.29.8 | MIT AND Apache-2.0 | Copyright (c) 2015-present Jason Miller; htm: Copyright 2018 Google Inc. | shipped in the app, `src/renderer/zen-lib/preact@10.29.8/` |
| Lit (`lit`) | 3.3.3 | BSD-3-Clause | Copyright (c) 2017 Google LLC. All rights reserved. | shipped in the app, `src/renderer/zen-lib/lit@3.3.3/` |
| Caveat (handwriting font) (`font-caveat`) | 5.3.0 | OFL-1.1 | Copyright 2014 The Caveat Project Authors (https://github.com/googlefonts/caveat) | shipped in the app, `src/renderer/zen-lib/font-caveat@5.3.0/` |
| Babylon.js (`babylonjs`) | 8.52.1 | Apache-2.0 | Copyright 2023 The Babylon.js team | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/babylonjs@8.52.1/LICENSE.txt` |
| Plotly (`plotly.js-dist-min`) | 4.1.1 | MIT | Copyright (c) 2016-2024 Plotly Technologies Inc. | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/plotly.js-dist-min@4.1.1/LICENSE.txt` |
| K2 help button (widget kit) (`k2-help`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-help@1.0.1/` |
| K2 activity bar (widget kit) (`k2-activity`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-activity@1.0.1/` |
| K2 Thread panel (widget kit) (`k2-thread`) | 1.1.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-thread@1.1.1/` |
| K2 markdown (widget kit) (`k2-markdown`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-markdown@1.0.1/` |
| K2 PNG encoder (widget kit) (`k2-png`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-png@1.0.1/` |
| K2 diff view (widget kit) (`k2-diff`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-diff@1.0.1/` |

K2 code is **MIT**. These libraries keep their own
licences and do **not** relicense K2.

<!-- zen-lib:end -->

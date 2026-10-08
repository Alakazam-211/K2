# NOTICE

**K2 by Alakazam Labs** — AI workspace orchestration.
Copyright 2025-2026 Alakazam Labs.

## Licensing

K2 is licensed under the **Functional Source License, Version 1.1, with
Apache 2.0 Future License** (FSL-1.1-Apache-2.0). See
[LICENSE](LICENSE). Each version of K2 automatically becomes
available under Apache 2.0 two years after its release.

A standing [Commercial Hosting Grant](COMMERCIAL_HOSTING_GRANT.md)
permits hosting K2 for clients when remote access runs through the
official K2 Connect tunnel service. The air-gapped / LAN-only daemon
is not in that grant — it requires a written Enterprise agreement.

## Trademarks

"K2 by Alakazam Labs" and the K2 logo are trademarks of Alakazam Labs;
see [TRADEMARKS.md](TRADEMARKS.md). The license above does not grant
trademark rights.

## Third-party: Seti UI file icons (MIT)

The Files drawer uses the **Seti** icon font for file-type glyphs
(same family as VS Code’s default file icon theme).

| | |
|--|--|
| **What** | `seti.woff` + generated name/extension maps under `src/renderer/assets/seti/` and `src/renderer/lib/seti-file-icons/` |
| **Upstream** | [Seti UI](https://github.com/jesseweed/seti-ui) (Jesse Weed), packaged via VS Code `extensions/theme-seti` |
| **License** | **MIT** (Copyright 2014 Jesse Weed) — full text in `src/renderer/assets/seti/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Remains **FSL-1.1-Apache-2.0**. MIT assets do **not** relicense the product. |
| **Not** | Not affiliated with Microsoft or Visual Studio Code. Do not use those trademarks for K2 branding. |

When redistributing K2 builds that include the Seti font, ship the MIT
notice (this section and/or `THIRD_PARTY_NOTICES.txt`).

## Third-party: wry (Apache-2.0 OR MIT)

K2 vendors a patched copy of **wry** 0.57.0 under `third_party/wry` so a
nil `NSURL` (`URLWithString`, IPC, navigation, `window.open`) cannot abort
the macOS app. crates.io 0.57.0 still unwraps those.

| | |
|--|--|
| **What** | `third_party/wry` (crates.io wry 0.57.0 + nil-URL no-abort patch) |
| **Upstream** | [wry](https://github.com/tauri-apps/wry) |
| **License** | **Apache-2.0 OR MIT** (Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy) — full text in `third_party/wry/LICENSE-MIT`, `third_party/wry/LICENSE-APACHE`, and `third_party/wry/LICENSE.spdx` |
| **K2 code** | Remains **FSL-1.1-Apache-2.0**. The vendored crate does **not** relicense the product. |

## Third-party: objc2 (MIT)

K2 vendors a patched copy of **objc2** 0.6.4 under `third_party/objc2` so a
null return from `sel_registerName` cannot abort the macOS app. crates.io
0.6.4 still expects that pointer. The crates.io crate ships no LICENSE file.

| | |
|--|--|
| **What** | `third_party/objc2` (crates.io objc2 0.6.4 + null-selector no-abort patch) |
| **Upstream** | [objc2](https://github.com/madsmtm/objc2) |
| **License** | **MIT** (Copyright (c) Mads Marquart) — full text in `third_party/objc2/LICENSE` |
| **K2 code** | Remains **FSL-1.1-Apache-2.0**. The vendored crate does **not** relicense the product. |

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
| **K2 code** | Remains **FSL-1.1-Apache-2.0**. The vendored crate does **not** relicense the product. |

## Third-party: Mermaid (MIT)

The file viewer draws Mermaid diagrams with a vendored build of **Mermaid**.

| | |
|--|--|
| **What** | `src/renderer/public/vendor/mermaid.min.js` (Mermaid 11.16.0, prebuilt, loaded by `components/FileViewerPane/DiagramViewer.tsx`) |
| **Upstream** | [mermaid](https://github.com/mermaid-js/mermaid) |
| **License** | **MIT** (Copyright (c) 2014 - 2022 Knut Sveidqvist) — full text in `src/renderer/public/vendor/mermaid.LICENSE.txt`. The prebuilt file also carries Mermaid's own dependencies (among them d3, dagre-d3-es, cytoscape, khroma, DOMPurify, KaTeX, marked), each under its own permissive licence. |
| **K2 code** | Remains **FSL-1.1-Apache-2.0**. Mermaid does **not** relicense the product. |

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
| Anime.js (`animejs`) | 4.5.0 | MIT | Copyright (c) 2025 Julian Garnier | shipped in the app, `src/renderer/zen-lib/animejs@4.5.0/` |
| Lottie (`lottie-web`) | 5.13.0 | MIT | Copyright (c) 2015 Bodymovin | shipped in the app, `src/renderer/zen-lib/lottie-web@5.13.0/` |
| perfect-freehand (`perfect-freehand`) | 1.2.3 | MIT | Copyright (c) 2021 Stephen Ruiz Ltd | shipped in the app, `src/renderer/zen-lib/perfect-freehand@1.2.3/` |
| Rough.js (`roughjs`) | 4.6.6 | MIT | Copyright (c) 2019 Preet Shihn | shipped in the app, `src/renderer/zen-lib/roughjs@4.6.6/` |
| howler.js (`howler`) | 2.2.4 | MIT | Copyright (c) 2013-2020 James Simpson and GoldFire Studios, Inc. | shipped in the app, `src/renderer/zen-lib/howler@2.2.4/` |
| Tone.js (`tone`) | 15.1.22 | MIT | Copyright (c) 2014-2020 Yotam Mann | shipped in the app, `src/renderer/zen-lib/tone@15.1.22/` |
| Leaflet (`leaflet`) | 1.9.4 | BSD-2-Clause | Copyright (c) 2010-2023, Volodymyr Agafonkin | shipped in the app, `src/renderer/zen-lib/leaflet@1.9.4/` |
| marked (`marked`) | 18.0.14 | MIT | Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/) | shipped in the app, `src/renderer/zen-lib/marked@18.0.14/` |
| DOMPurify (`dompurify`) | 3.4.16 | Apache-2.0 | (c) Cure53 and other contributors | shipped in the app, `src/renderer/zen-lib/dompurify@3.4.16/` |
| highlight.js (`highlight.js`) | 11.12.0 | BSD-3-Clause | Copyright (c) 2006, Ivan Sagalaev. | shipped in the app, `src/renderer/zen-lib/highlight.js@11.12.0/` |
| KaTeX (`katex`) | 0.18.9 | MIT | Copyright (c) 2013-2020 Khan Academy and other contributors | shipped in the app, `src/renderer/zen-lib/katex@0.18.9/` |
| Preact + htm (`preact`) | 10.29.8 | MIT AND Apache-2.0 | Copyright (c) 2015-present Jason Miller; htm: Copyright 2018 Google Inc. | shipped in the app, `src/renderer/zen-lib/preact@10.29.8/` |
| Lit (`lit`) | 3.3.3 | BSD-3-Clause | Copyright (c) 2017 Google LLC. All rights reserved. | shipped in the app, `src/renderer/zen-lib/lit@3.3.3/` |
| Caveat (handwriting font) (`font-caveat`) | 5.3.0 | OFL-1.1 | Copyright 2014 The Caveat Project Authors (https://github.com/googlefonts/caveat) | shipped in the app, `src/renderer/zen-lib/font-caveat@5.3.0/` |
| Babylon.js (`babylonjs`) | 8.52.1 | Apache-2.0 | Copyright 2023 The Babylon.js team | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/babylonjs@8.52.1/LICENSE.txt` |
| Plotly (`plotly.js-dist-min`) | 4.1.1 | MIT | Copyright (c) 2016-2024 Plotly Technologies Inc. | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/plotly.js-dist-min@4.1.1/LICENSE.txt` |

K2 code remains **FSL-1.1-Apache-2.0**. These libraries keep their own
licences and do **not** relicense K2.

<!-- zen-lib:end -->

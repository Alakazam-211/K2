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

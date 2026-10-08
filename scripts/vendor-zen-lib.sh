#!/usr/bin/env bash
# Vendor the Zen widget standard library (prd-zen-user-widgets-v2 UWB12).
#
# Installs the pinned npm packages (scripts/zen-lib/vendor-package.json, every
# tarball checked against vendor-package-lock.json) in a temp folder, then
# writes:
#   src/renderer/zen-lib/<id>@<version>/   library files + LICENSE.txt (checked in)
#   src/shared/zen-lib.json                the manifest (sizes, sha256, licences)
#   NOTICE.md                              the generated "Zen widget library" section
#
# Never run in CI or at build time; its output is checked in, like
# src/renderer/public/vendor/mermaid.min.js. Needs network, npm and bun.
#
#   scripts/vendor-zen-lib.sh                 vendor and write
#   scripts/vendor-zen-lib.sh --check         rebuild in a temp folder; fail on any difference
#   scripts/vendor-zen-lib.sh --update-lock   re-resolve the lock after a version change
#
# The list of libraries, their versions and how each is packaged live in
# scripts/zen-lib/libs.ts. p5.js (LGPL, R4) and GSAP (licence) are refused.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v bun >/dev/null || { echo "vendor-zen-lib: needs bun" >&2; exit 1; }
command -v npm >/dev/null || { echo "vendor-zen-lib: needs npm" >&2; exit 1; }
exec bun "$ROOT/scripts/zen-lib/vendor.ts" "$@"

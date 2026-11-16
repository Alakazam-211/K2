#!/bin/sh
# Pack-template check: a type pack must never depend on or import a CRDT engine.
# The kit owns the engine (Rust) and the JS client + editor-binding adapters; packs bind to
# the kit's primitives. Run from a pack repo's root: ../k2-mpc-base/scripts/pack-checks/engine-free.sh
#
# Fails on:
#  - a Cargo dependency (any target, direct) on an engine crate in any pack crate;
#  - a JS dependency on an engine package in any package.json (outside node_modules);
#  - a source import/use of an engine in .rs/.ts/.tsx/.js/.mjs/.cjs files.
# `mpc-tools pack-check engine-free` (tools/README.md, "mpc-tools"). Exit 0 clean, 1 findings,
# 4 no such folder.
set -eu
KIT=$(cd "$(dirname "$0")/../.." && pwd)
exec sh "$KIT/tools/mpc-tools.sh" pack-check engine-free "${1:-.}"

#!/bin/sh
# Pack-template check: a pack that keeps a carrier runs the kit's byte-exact row.
# Run from a pack repo's root: ../k2-mpc-base/scripts/pack-checks/carrier-row.sh
#
# A pack whose source (outside tests/, target/ and the fuzz crate) implements
# `Pack::wants_carrier` with a body other than a plain `false` keeps a carrier, and must call
# `k2_mpc_base::session::test_support::carrier_judge` over its corpus in a test under a
# `tests/` folder (docs/PACK_TEMPLATE.md, "A format the pack models only in part"). Exit 0: no
# carrier, or the row is there; 1: a carrier pack without the row; 2: usage; 4: no such folder.
# `mpc-tools pack-check carrier-row` (tools/README.md, "mpc-tools").
set -eu
KIT=$(cd "$(dirname "$0")/../.." && pwd)
exec sh "$KIT/tools/mpc-tools.sh" pack-check carrier-row "${1:-.}"

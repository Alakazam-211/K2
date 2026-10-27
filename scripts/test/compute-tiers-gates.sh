#!/usr/bin/env bash
# The compute job tiers gates (prd-compute-job-tiers-v1), one step per run,
# meant for a compute node: `k2 compute run <node> --sha <sha> --needs
# rust@stable,cmake@3 -- bash scripts/test/compute-tiers-gates.sh <step>`.
#
# Steps: check | proto | node | core | daemon-unit | daemon-int | js | drill
#
# The node's job user has a nologin shell: SHELL is set here so tools that
# spawn "$SHELL" work. Every K2_* variable is dropped so nothing reaches a
# real ~/.k2.
set -euo pipefail
export SHELL=/bin/bash
for v in $(compgen -e); do
  case "$v" in K2_* | K2SO_*) unset "$v" ;; esac
done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
cd "$(dirname "$0")/../.."

step="${1:?step: check|proto|node|core|daemon-unit|daemon-int|js|drill}"
shift || true
case "$step" in
  check)
    RUSTFLAGS="-D warnings" cargo check -p k2-node-proto -p k2-node -p k2-core -p k2-daemon --tests --keep-going --message-format short
    ;;
  proto)
    cargo test -p k2-node-proto
    ;;
  node)
    cargo test -p k2-node --lib
    cargo test -p k2-node --test queue_shared
    cargo test -p k2-node --test golden_solo
    ;;
  core)
    cargo test -p k2-core --lib -- compute:: db::tests workspace::skill_regen permission::catalog
    ;;
  daemon-unit)
    cargo test -p k2-daemon --lib -- compute routes::route_policy permission::
    ;;
  daemon-int)
    cargo test -p k2-daemon --test compute_integration -- "$@"
    cargo test -p k2-daemon --test compute_shared_integration
    cargo test -p k2-daemon --test ticket_brief_integration
    cargo test -p k2-daemon --test permission_off_golden
    ;;
  js)
    bun install --frozen-lockfile
    node scripts/gen-cli-explorer-catalog.mjs --check
    bunx vitest run src/renderer/components/Settings/sections/ComputeSection.test.tsx \
      src/renderer/components/Settings/sections/ComputeServersParts.test.tsx \
      src/renderer/components/Settings/sections/compute-api.test.ts
    ;;
  drill)
    [ $# -gt 0 ] || set -- tiers-solo tiers-shared
    bash scripts/test/compute-two-servers.sh "$@"
    ;;
  *)
    echo "unknown step $step" >&2
    exit 2
    ;;
esac
echo "GATE $step: OK"

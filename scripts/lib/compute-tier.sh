#!/usr/bin/env bash
# compute-tier.sh — the queue-tier and placement flags for `k2 compute
# run`, for scripts that may run on a Mac whose K2 is a release behind.
# (k2_compute_avoid_args is at the end.)
#
# Sourced; sets no shell options.
#
#   k2_compute_tier_args <tier>   prints the flags, one per line:
#     ci        --tier ci   (K2 0.46.5+); --gate on an older CLI, which
#                           means the same: first in every node's queue
#     standard  --tier standard; nothing on an older CLI (its default)
#     priority / batch: --tier <tier>; refused (exit 2) on an older CLI
#
# An older CLI is one whose `k2 compute --help` prints help without
# --tier (0.46.4). A CLI that prints no help at all (a test stub) is taken
# to know --tier. K2_BIN picks the CLI (default k2).

k2_compute_cli_has_tier() {
    if [ -z "${K2_COMPUTE_CLI_HAS_TIER:-}" ]; then
        local help
        help="$("${K2_BIN:-k2}" compute --help 2>/dev/null </dev/null || true)"
        if [ -n "$help" ] && ! printf '%s' "$help" | grep -q -- '--tier'; then
            K2_COMPUTE_CLI_HAS_TIER=0
        else
            K2_COMPUTE_CLI_HAS_TIER=1
        fi
    fi
    [ "$K2_COMPUTE_CLI_HAS_TIER" = 1 ]
}

k2_compute_tier_args() {
    local tier="${1:?usage: k2_compute_tier_args <tier>}"
    if k2_compute_cli_has_tier; then
        printf '%s\n%s\n' --tier "$tier"
        return 0
    fi
    case "$tier" in
        ci) printf '%s\n' --gate ;;
        standard|normal) ;;
        *)
            echo "FATAL: this k2 CLI ($("${K2_BIN:-k2}" --version 2>/dev/null || echo old)) has no --tier $tier; update K2 (0.46.5+)" >&2
            return 2
            ;;
    esac
}

# k2_compute_avoid_args — `--avoid <node>` (two lines) for each node in
# K2_COMPUTE_AVOID (default k2-arch) that this workspace is granted, for a
# job K2 places itself (no node named). k2-arch is an Arch node: the Debian
# `sys:` packages K2's build and CI jobs ask for don't exist there. Only
# granted nodes are named, because --avoid on a node this workspace can't
# see is refused (node_not_found). Prints nothing when the CLI has no
# --avoid (before 0.46.8) or the node list can't be read: the grants are
# then the only fence. K2_COMPUTE_AVOID="" turns it off.
k2_compute_avoid_args() {
    local want="${K2_COMPUTE_AVOID-k2-arch}" help nodes n
    [ -n "$want" ] || return 0
    help="$("${K2_BIN:-k2}" compute --help 2>/dev/null </dev/null || true)"
    printf '%s' "$help" | grep -q -- '--avoid' || return 0
    nodes="$("${K2_BIN:-k2}" compute nodes --json 2>/dev/null </dev/null | python3 -c '
import json, sys
try:
    print("\n".join(n["name"] for n in json.load(sys.stdin)["nodes"]))
except Exception:
    pass' || true)"
    for n in $(printf '%s' "$want" | tr ',' ' '); do
        if printf '%s\n' "$nodes" | grep -qxF -- "$n"; then
            printf '%s\n%s\n' --avoid "$n"
        fi
    done
    return 0
}

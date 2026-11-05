#!/usr/bin/env bash
# compute-tier.sh — the queue-tier flags for `k2 compute run`, for scripts
# that may run on a Mac whose K2 is a release behind.
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

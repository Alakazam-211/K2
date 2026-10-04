#!/usr/bin/env bash
# Hermetic env for CLI tests that need NO daemon (help pages, usage errors,
# moved-verb redirects, hard-deprecations).
#
# cli/k2 refuses every verb outside a short allow-list (--schema, study,
# publish help, ...) unless it has a port AND a token, and it reads both
# from ~/.k2/heartbeat.{port,token} when the env has none. So these tests
# passed only on a machine with a real ~/.k2 — and then talked to that
# machine's daemon config. This helper points HOME at a throwaway dir and
# hands the CLI a dead port (1) plus a dummy token: offline paths run, and
# anything that does try the network fails loudly instead of reaching a
# real daemon.
#
# Usage (after `set -euo pipefail`):
#   source "$(dirname "$0")/_hermetic_cli.sh"
#   hermetic_cli_env
# Removes its HOME on EXIT. A script that sets its own EXIT trap afterwards
# must call `hermetic_cli_cleanup` from it.

hermetic_cli_env() {
    K2_TEST_HOME="$(mktemp -d -t k2-cli-home-XXXXXX)"
    export K2_TEST_HOME
    export HOME="$K2_TEST_HOME"
    # Never inherit a session's connection or identity from the caller.
    unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
        K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL
    export K2_PORT=1
    export K2_HOOK_TOKEN=test-token
    trap hermetic_cli_cleanup EXIT
}

hermetic_cli_cleanup() {
    if [ -n "${K2_TEST_HOME:-}" ] && [ -d "$K2_TEST_HOME" ]; then
        rm -rf "$K2_TEST_HOME"
    fi
}

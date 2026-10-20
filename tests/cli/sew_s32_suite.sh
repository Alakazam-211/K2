#!/usr/bin/env bash
# Sew S32 (K26/K27/K31) daemon + CLI checks in one run, for a compute node:
#   k2 compute run <node> --sha <sha> --needs rust@stable,cmake@3 --mem 7G -- tests/cli/sew_s32_suite.sh
# Builds k2-daemon, runs the hostchat shell tests (stub daemon + a sandbox
# daemon) and the integration tests the S32 code touches. Fails loudly on
# the first failure. Never touches the real ~/.k2 (each test uses a temp
# HOME).

set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

cargo build -p k2-daemon
tests/cli/hostchat_headless_smoke.sh
tests/cli/hostchat_s32_cli.sh
tests/cli/hostchat_cli.sh
cargo test -p k2-daemon --test grant_hook_integration --test chat_pass_mint_integration
echo "sew S32 suite: all passed"

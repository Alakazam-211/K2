#!/bin/sh
# TEST ONLY. Build the release-build check app with the CI files K2's Tauri
# template writes (k2/ci/*.sh, .k2/ci.toml; the template's test keeps them
# equal, app_templates::tests::ci_files_match_the_checked_app), on a K2
# Compute node, exactly as `k2 ci release` runs them.
#
# Linux (two nodes on different hosts; compare the sha256 lines they print):
#   k2 compute run --os linux --sha <commit> --cpus 6 --mem 16G \
#     --needs rust@1.99.1,node@22,profile:tauri-linux,sys:patchelf,sys:libfuse2t64,sys:file,sys:xdg-utils \
#     --artifact 'tests/app-template/tauri-ci-check/k2/out/*' -- sh tests/app-template/tauri-ci-check/run.sh
# macOS (unsigned compile check):
#   k2 compute run --os macos --sha <commit> --cpus 6 --mem 7G --needs rust@1.99.1,node@22 \
#     -- sh tests/app-template/tauri-ci-check/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
case "$(uname -s)" in
    Linux) bash k2/ci/build-linux.sh ;;
    Darwin) bash k2/ci/build-macos.sh ;;
    *) echo "FAIL: $(uname -s): Linux or macOS only" >&2; exit 1 ;;
esac
echo "PASS: the template's build script built the app ($(uname -s))"

#!/usr/bin/env bash
# k2-signer.sh — the release user's door to k2-signer, the Rust release
# signer that runs as the `_k2sign` role user beside k2sign
# (prd-ci-on-compute-for-clients-v1 C4; prd-compute-build-attestations-v1
# S1). Sourced; sets no shell options.
#
#   k2s_run <verb> [args…]                 stdin passes through; JSON or a
#                                          signature on stdout; exit 0 ok,
#                                          2 usage, 3 refused, 4 internal
#   k2s_verdict <asset> <version> <file>   0 = every check `sign` would run
#                                          passed for these bytes (project
#                                          k2); the refusal on stderr
#
# The signer reads its own installed policy, pins and evidence; nothing this
# user passes is trusted. Only `sudo -n -u _k2sign …/k2-signer` is allowed
# without a password (install-k2-signer.sh's sudoers drop-in).
#
# K2_SIGNER_CMD: a stand-in command for tests. release.sh refuses to run
# with it set (it is a test seam, like K2SIGN_TEST_ROOT).

k2s_run() {
    if [ -n "${K2_SIGNER_CMD:-}" ]; then
        "$K2_SIGNER_CMD" "$@"
    else
        # From /: _k2sign can't read this user's working folder.
        (cd / && /usr/bin/sudo -n -u _k2sign /usr/local/libexec/k2-signer/k2-signer "$@")
    fi
}

k2s_verdict() {
    local asset="$1" version="$2" file="$3" err
    [ -f "$file" ] || { echo "k2s_verdict: no file $file" >&2; return 1; }
    if ! err="$(k2s_run verdict k2 "$asset" "$version" < "$file" 2>&1 >/dev/null)"; then
        printf '%s\n' "$err" | sed $'s/\x1b\\[[0-9;]*m//g' >&2
        return 1
    fi
}

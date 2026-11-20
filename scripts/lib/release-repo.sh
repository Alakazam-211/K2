# shellcheck shell=bash
# release-repo.sh — the GitHub repo releases are published to and read from,
# for the release tooling (sourced; sets two names, runs nothing).
#
# The repo is moving from Alakazam-211/K2 to AlakazamLabs/K5 (a transfer and
# rename; GitHub redirects the old name). Until the move every default below
# stays the old name. On move day K2_RELEASE_REPO_DEFAULT becomes the new
# name: that one line is the whole change for release.sh, the sign-*.sh
# scripts, the Linux sums and Arch steps, compute-release.sh and k2sign's
# sitting.sh. K2_RELEASE_REPO in the environment still overrides it for a
# run (release.sh's bridge releases).
#
# Copies a test keeps equal (src/renderer/lib/brand.test.ts): release_repo!()
# and RELEASE_REPO_NAMES in crates/k2-client/src/brand.rs, _K2_RELEASE_REPO in
# cli/src/prelude.sh, and repo / repo_names in scripts/k2sign/policy.

# The repo a release is published to by default (owner/name).
K2_RELEASE_REPO_DEFAULT="Alakazam-211/K2"
# Every name the repo answers to, space-separated: a check that compares a
# repo name (a build attestation's identity, a recipe's source) accepts
# exactly these, never any other.
K2_RELEASE_REPO_NAMES="Alakazam-211/K2 AlakazamLabs/K5"

# k2_is_release_repo <owner/name>: exit 0 when it is one of the repo's names.
k2_is_release_repo() {
    local n
    for n in $K2_RELEASE_REPO_NAMES; do
        [ "$1" = "$n" ] && return 0
    done
    return 1
}

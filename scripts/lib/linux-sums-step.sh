#!/usr/bin/env bash
# linux-sums-step.sh — release.sh Step 10.6, sourced. bash 3.2-safe.
#
# After the tag, app-linux.yml and arch-package.yml put the Linux packages
# on the release, each on its own runner. By now the release is live, so
# nothing here can stop it; this tells the person running it what happened.
#
#   k2_linux_sums_step <version> <wait-min> [arch-later]
#
# arch-later (release.sh compute mode: deb/rpm/AppImage and their lines are
# on the release before this runs): when only the Arch package is missing,
# that is fine now (it builds on k2-arch on its own), ARCH_PKG_PENDING=1 and
# no wait. Anything else missing or wrong fails as usual.
#
# Runs `scripts/linux-sums.sh check` once per poll until it passes, a line
# is wrong, or <wait-min> runs out. When the tag's arch-package.yml run has
# failed, it says so at once and stops waiting for that package, but keeps
# checking the others. Without this, a failed Arch build sat out the whole
# wait and ended as "SHA256SUMS incomplete".
#
# Sets:
#   LINUX_SUMS_FAILED  1 when a package other than Arch's is missing or a
#                      line is wrong, or the check never passed
#   ARCH_PKG_FAILED    1 when the Arch build failed and its package is
#                      still not complete on the release
#   ARCH_PKG_RUN_URL   that run's page
#   ARCH_PKG_PENDING   1 (arch-later only) when the Arch package was still
#                      missing and not waited for
#
# Environment: K2_RELEASE_REPO (default Alakazam-211/K2),
#              K2_SUMS_POLL_SECS (default 30; tests set 0).

_K2_SUMS_STEP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/wait-progress.sh
source "$_K2_SUMS_STEP_DIR/wait-progress.sh"

# "<status> <conclusion> <run id> <url>" of the tag's arch-package.yml run.
# Empty when there is no run yet or gh failed: neither means the build failed.
_k2_arch_run() {
    local tag="$1"
    gh run list --repo "${K2_RELEASE_REPO:-Alakazam-211/K2}" --workflow arch-package.yml \
        --branch "$tag" --event push --limit 1 \
        --json status,conclusion,databaseId,url \
        --jq '.[0] | select(.) | [.status, (if (.conclusion // "") == "" then "none" else .conclusion end), (.databaseId | tostring), .url] | join(" ")' \
        2>/dev/null || true
}

# Say it once, plainly: what failed, what it means, what to run.
_k2_arch_failed_report() {
    local version="$1" conclusion="$2" run_id="$3" url="$4" pkg="$5"
    local repo="${K2_RELEASE_REPO:-Alakazam-211/K2}"
    {
        echo ""
        echo "  ✗ The Arch package build for v${version} FAILED (${conclusion}): ${url}"
        echo "    The release is live and stays live. This only means ${pkg} is not published."
        echo "    Arch installs still see v${version} offered, but Install & Relaunch stops with"
        echo "    \"has no line for ${pkg}\" and changes nothing on their machine."
        echo "    To publish it, re-run the build:"
        echo "      gh run rerun ${run_id} --failed --repo ${repo}"
        echo "    Or, if the fix is a new commit on main:"
        echo "      gh workflow run arch-package.yml --repo ${repo} -f tag=v${version} -f ref=main"
        echo "    Then check: bash scripts/linux-sums.sh check ${version} --wait-min 60"
        echo "    Not waiting for it. Still checking the other Linux packages."
    } >&2
}

k2_linux_sums_step() {
    local version="$1" wait_min="${2:-120}" arch_mode="${3:-}"
    local tag="v${version}" pkg="k2-${version}-x86_64.pkg.tar.zst"
    local poll="${K2_SUMS_POLL_SECS:-30}"
    local sums="$_K2_SUMS_STEP_DIR/../linux-sums.sh"
    local deadline=$(( $(date +%s) + wait_min * 60 ))
    local rc out findings others run status conclusion run_id url
    LINUX_SUMS_FAILED=""
    ARCH_PKG_FAILED=""
    ARCH_PKG_RUN_URL=""
    ARCH_PKG_PENDING=""
    while :; do
        rc=0
        out="$(bash "$sums" check "$version" 2>&1)" || rc=$?
        if [ "$rc" = 0 ]; then
            k2_wait_done
            printf '%s\n' "$out" >&2
            # A re-run can finish during this wait: the package is up now.
            ARCH_PKG_FAILED=""
            return 0
        fi
        # With no wait of its own the check ends each pass with "still
        # missing after 0 min". That line says nothing here.
        findings="$(printf '%s\n' "$out" | grep -v 'still missing after 0 min' || true)"
        if printf '%s\n' "$out" | grep -q ' is wrong '; then
            k2_wait_done
            printf '%s\n' "$findings" >&2
            LINUX_SUMS_FAILED=1
            return 0
        fi

        run="$(_k2_arch_run "$tag")"
        status=""; conclusion=""; run_id=""; url=""
        if [ -n "$run" ]; then
            read -r status conclusion run_id url <<EOF
$run
EOF
        fi
        if [ -z "$ARCH_PKG_FAILED" ] && [ "$status" = "completed" ]; then
            case "$conclusion" in
                success|skipped|neutral) ;;
                *)
                    k2_wait_done
                    ARCH_PKG_FAILED=1
                    ARCH_PKG_RUN_URL="$url"
                    _k2_arch_failed_report "$version" "$conclusion" "$run_id" "$url" "$pkg"
                    ;;
            esac
        fi

        # Everything still wrong that isn't the Arch package. A pass that
        # names no package at all (gh failed) proves nothing, so it needs
        # the Arch package named before it counts as "only Arch is missing".
        others="$(printf '%s\n' "$out" \
            | grep -E 'not on the release yet|no .* line for |malformed SUMS line|more than one |SUMS says' \
            | grep -vF "$pkg" || true)"
        if [ -n "$ARCH_PKG_FAILED" ] && [ -z "$others" ] && printf '%s\n' "$out" | grep -qF "$pkg"; then
            k2_wait_done
            echo "  ✓ deb, rpm and AppImage match SHA256SUMS-linux-x86_64.txt. Only the Arch package is missing." >&2
            return 0
        fi
        if [ "$arch_mode" = arch-later ] && [ -z "$others" ] && printf '%s\n' "$out" | grep -qF "$pkg"; then
            k2_wait_done
            ARCH_PKG_PENDING=1
            echo "  ✓ deb, rpm and AppImage match SHA256SUMS-linux-x86_64.txt. The Arch package (${pkg}) is still" >&2
            echo "    building on k2-arch (arch-package.yml) and is not waited for. Check it later:" >&2
            echo "      bash scripts/linux-sums.sh check ${version} --wait-min 60" >&2
            return 0
        fi

        if [ "$(date +%s)" -ge "$deadline" ]; then
            k2_wait_done
            printf '%s\n' "$findings" >&2
            if [ -n "$status" ] && [ "$status" != "completed" ]; then
                echo "  arch-package run is still ${status}: ${url}" >&2
            fi
            echo "  Linux packages or SUMS lines still missing after ${wait_min} min." >&2
            LINUX_SUMS_FAILED=1
            return 0
        fi

        k2_wait_tick "[linux-sums] waiting for app-linux.yml / arch-package.yml (v${version})..."
        if [ "$K2_WAIT_SAID" = 1 ]; then
            printf '%s\n' "$findings" | sed 's/^/    /' >&2
        fi
        sleep "$poll"
    done
}

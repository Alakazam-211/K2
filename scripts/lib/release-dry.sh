#!/usr/bin/env bash
# release-dry.sh — release.sh's rehearsal ("dry release"), sourced by
# scripts/release.sh at the very top. bash 3.2-safe. Sets no shell options.
#
#   ./scripts/release.sh --dry-run <version> [notes-file]
#   K2_RELEASE_DRY=1 ./scripts/release.sh <version> [notes-file]
#
# The whole release path runs for real on z3mbpZ, while the release is still
# being put together and beside its last tests, so the real run only repeats
# a known-good one. What runs and what doesn't:
#
#   runs      the release lock; Steps 0.4-0.6 (Rust pin, the update-key
#             preflight and key copies test, every pre-flight gate, the
#             signing preflights, the Linux gate); the version bump and
#             WHATS_NEW (in the scratch tree below); the K2 Compute Linux
#             builds and their checks (1.6c, 7.9c, 8.5); the Mac build,
#             Developer ID signing, the AMFI launch check, all three
#             notarizations and staples; the VM pairing smoke; both updater
#             manifests (to the run dir and daemon-dist); the release commit
#             as a commit object and the tree check against what compute
#             built; the Windows NSIS build on K2-WIN-AX41 (from that commit,
#             as the real run does from the tag; K2_SKIP_WINDOWS_NSIS=1 leaves
#             it out, as for a real run); publish-web-bundles.sh --dry-run;
#             read-only checks of what Step 9 would upload and of the
#             manifests; the staged Linux files' byte checks and, under
#             k2sign, the signer's read-only `k2sign compute-rehearse`.
#   would     every network write: git push, the tag (it would be a real tag
#             in the shared repo), gh release create/upload/edit, the repo
#             variable LINUX_BUILDS, the R2 upload, windows-patch-latest-json,
#             sign-daemon.sh / sign-root-helper.sh / sign-k2-node.sh /
#             sign-app-engine.sh (each needs the published release). Each one
#             prints "[dry] would: ...".
#   skipped   update-key signing and the approval (Step 1.55): the signing
#             user keeps ONE sha per asset per version and counts a version
#             whose approval signed anything as used (never replaced), so a
#             rehearsal signature would make the real release refuse
#             (REFUSED-SWAP). The .sig/.sig2 files and manifest signatures are
#             placeholders that say so. Steps 10 and 10.6 read the published
#             release, which a rehearsal doesn't have; their local
#             equivalents run instead.
#
# The real checkout is never touched: release.sh switches, before anything
# else, to a scratch git worktree (default <main checkout>/.claude/worktrees/
# release-dry/tree, beside the builders' worktrees: inside the K2 workspace,
# so its K2 Compute jobs ship ITS HEAD and uncommitted files, not the main
# checkout's; kept between rehearsals so target/ and node_modules stay
# warm), detached at the
# source checkout's HEAD plus its uncommitted and untracked files (a real
# release builds the working tree too), with .env linked and
# src-tauri/binaries copied in, then `bun install --frozen-lockfile`. All
# writes go there and to the run dir. The release lock is taken first, as
# for a real run, so the two never overlap.
#
# Settings:
#   K2_RELEASE_DRY_ROOT    default <main checkout>/.claude/worktrees/release-dry
#                          (gitignored there): tree/ (the scratch worktree)
#                          and runs/<version>-<UTC stamp>/ (the summary,
#                          would.txt, latest.json, the audit dir). Keep it
#                          inside the workspace the compute jobs come from;
#                          inside a checkout only where git ignores it.
#   K2_RELEASE_DRY_CLEAN=1 rehearse HEAD only: leave the source checkout's
#                          uncommitted and untracked files out
#   K2_RELEASE_RESUME      as for a real run (7.5 or 8): the scratch tree is
#                          reused as the last rehearsal left it (not reset)
# Set by the switch for the run in the scratch tree (never by hand):
#   K2_RELEASE_DRY_INNER=1, K2_RELEASE_DRY_SRC, K2_RELEASE_DRY_TREE,
#   K2_RELEASE_DRY_RUN_DIR, K2_RELEASE_DRY_BASE
#
# The summary at the end (also <run dir>/summary.txt) has each step's
# status and time, where the artifacts are, and DRY_EXIT=<n> (0 = every
# step that ran passed and nothing publishing slipped past a dry branch).

k2dry_on() { [ "${K2_RELEASE_DRY:-0}" = 1 ]; }

# ── The switch to the scratch worktree (runs in the source checkout) ──
# k2dry_prepare <source-checkout> <version> [notes-file]
# Execs the scratch tree's release.sh; returns only on an error (non-zero).
k2dry_prepare() {
    local src="$1" version="$2" notes="${3:-}" root tree run base common tcommon stamp n
    src="$(cd "$src" && pwd -P)" || return 1
    base="$(git -C "$src" rev-parse --verify 'HEAD^{commit}')" || { echo "ERROR: $src has no HEAD commit" >&2; return 1; }
    common="$(cd "$src" && cd "$(git rev-parse --git-common-dir)" && pwd -P)" || return 1
    if [ -n "${K2_RELEASE_DRY_ROOT:-}" ]; then
        root="$K2_RELEASE_DRY_ROOT"
    elif [ "$(basename "$common")" = .git ]; then
        # The main checkout (the workspace) holds the builders' worktrees too.
        root="$(dirname "$common")/.claude/worktrees/release-dry"
    else
        root="$HOME/k2-release-dry"
    fi
    case "$root" in /*) ;; *) root="$PWD/$root" ;; esac
    tree="$root/tree"
    # Inside a checkout only where git ignores it (never this checkout's
    # files, never its status).
    case "$tree/" in
        "$src/"*)
            git -C "$src" check-ignore -q "$tree" || {
                echo "ERROR: the scratch tree $tree would be inside $src, where git doesn't ignore it: set K2_RELEASE_DRY_ROOT elsewhere." >&2
                return 1
            } ;;
    esac
    case "$src/" in
        "$tree/"*) echo "ERROR: $src is inside the scratch tree $tree: run the rehearsal from the real checkout." >&2; return 1 ;;
    esac
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    run="$root/runs/${version}-${stamp}"
    mkdir -p "$run" || return 1

    echo ""
    echo "Dry release (rehearsal) of v${version}: switching to a scratch worktree; this checkout is not touched."
    echo "  source:  $src at ${base:0:12}"
    if [ -e "$tree" ]; then
        tcommon="$(cd "$tree" 2>/dev/null && cd "$(git rev-parse --git-common-dir 2>/dev/null)" 2>/dev/null && pwd -P)" || tcommon=""
        if [ "$tcommon" != "$common" ] || [ "$(cd "$tree" && pwd -P)" = "$src" ]; then
            echo "ERROR: $tree exists but is not a scratch worktree of this repository." >&2
            echo "  Move it away, or set K2_RELEASE_DRY_ROOT to another folder." >&2
            return 1
        fi
        if [ -n "${K2_RELEASE_RESUME:-}" ]; then
            [ "$(git -C "$tree" rev-parse 'HEAD^{commit}')" = "$base" ] || {
                echo "ERROR: K2_RELEASE_RESUME=${K2_RELEASE_RESUME}: the scratch tree is at $(git -C "$tree" rev-parse --short HEAD), this checkout at ${base:0:12}." >&2
                echo "  A resumed rehearsal reuses the last one's tree as it is: same base commit only. Run a full rehearsal instead." >&2
                return 1
            }
            echo "  scratch: $tree (K2_RELEASE_RESUME=${K2_RELEASE_RESUME}: reused as the last rehearsal left it)"
        else
            git -C "$tree" checkout -q --force --detach "$base" || return 1
            # Untracked files go; ignored ones (target/, node_modules) stay warm.
            git -C "$tree" clean -q -fd || return 1
            echo "  scratch: $tree (reset to ${base:0:12}; target/ and node_modules kept)"
        fi
    else
        [ -z "${K2_RELEASE_RESUME:-}" ] || { echo "ERROR: K2_RELEASE_RESUME=${K2_RELEASE_RESUME} but there is no earlier rehearsal at $tree" >&2; return 1; }
        mkdir -p "$root" || return 1
        git -C "$src" worktree prune || return 1
        git -C "$src" worktree add -q --detach "$tree" "$base" || return 1
        echo "  scratch: $tree (new worktree at ${base:0:12}; the first rehearsal builds cold)"
    fi
    echo "  run dir: $run"

    if [ -z "${K2_RELEASE_RESUME:-}" ]; then
        if [ "${K2_RELEASE_DRY_CLEAN:-0}" = 1 ]; then
            echo "  K2_RELEASE_DRY_CLEAN=1: rehearsing HEAD only (this checkout's uncommitted files left out)."
        else
            # What a real run from this checkout would build: HEAD plus the
            # working tree (staged and unstaged changes, untracked files).
            git -C "$src" diff --binary HEAD > "$run/source-uncommitted.patch" || return 1
            if [ -s "$run/source-uncommitted.patch" ]; then
                git -C "$tree" apply --binary --whitespace=nowarn "$run/source-uncommitted.patch" || {
                    echo "ERROR: the uncommitted changes did not apply in the scratch tree ($run/source-uncommitted.patch)" >&2
                    return 1
                }
                n="$(git -C "$src" diff --name-only HEAD | grep -c .)"
                echo "  carried: $n uncommitted file(s) from $src (K2_RELEASE_DRY_CLEAN=1 leaves them out)"
            else
                rm -f "$run/source-uncommitted.patch"
            fi
            (cd "$src" && git ls-files -z --others --exclude-standard) > "$run/source-untracked.z" || return 1
            if [ -s "$run/source-untracked.z" ]; then
                (cd "$src" && tar --null -T "$run/source-untracked.z" -cf -) | (cd "$tree" && tar -xpf -) || {
                    echo "ERROR: copying the untracked files into the scratch tree failed" >&2
                    return 1
                }
                tr '\0' '\n' < "$run/source-untracked.z" > "$run/source-untracked.txt"
                echo "  carried: $(grep -c . "$run/source-untracked.txt") untracked file(s) (list: $run/source-untracked.txt)"
            fi
            rm -f "$run/source-untracked.z"
        fi
        # The gitignored build inputs a fresh worktree lacks (wiki: Release -
        # Unattended release.sh): .env, the fetched sidecars, node_modules.
        if [ -f "$src/.env" ]; then
            ln -sfn "$src/.env" "$tree/.env" || return 1
            echo "  .env:    linked from $src/.env"
        fi
        if [ -d "$src/src-tauri/binaries" ]; then
            mkdir -p "$tree/src-tauri/binaries" && cp -Rp "$src/src-tauri/binaries/." "$tree/src-tauri/binaries/" || return 1
            echo "  sidecars: src-tauri/binaries copied in"
        fi
        echo "  bun install --frozen-lockfile (scratch tree)..."
        (cd "$tree" && bun install --frozen-lockfile) || { echo "ERROR: bun install --frozen-lockfile failed in $tree" >&2; return 1; }
    fi

    case "$notes" in ""|/*) ;; *) notes="$PWD/$notes" ;; esac
    export K2_RELEASE_DRY=1 K2_RELEASE_DRY_INNER=1 K2_RELEASE_DRY_SRC="$src" K2_RELEASE_DRY_TREE="$tree" \
        K2_RELEASE_DRY_RUN_DIR="$run" K2_RELEASE_DRY_BASE="$base"
    # The compute audit of a rehearsal never mixes with a real release's.
    export K2_RELEASE_AUDIT_DIR="$run/audit"
    echo ""
    if [ -n "$notes" ]; then
        exec "${BASH:-bash}" "$tree/scripts/release.sh" "$version" "$notes"
    fi
    exec "${BASH:-bash}" "$tree/scripts/release.sh" "$version"
}

# ── In the scratch tree: refuse anything but the tree the switch made ──
# k2dry_inner_check <project-dir>
k2dry_inner_check() {
    local proj gd cd
    proj="$(cd "$1" && pwd -P)" || return 1
    if [ -z "${K2_RELEASE_DRY_TREE:-}" ] || [ -z "${K2_RELEASE_DRY_RUN_DIR:-}" ] || [ -z "${K2_RELEASE_DRY_SRC:-}" ]; then
        echo "ERROR: K2_RELEASE_DRY_INNER is set by release.sh's dry-run switch, never by hand." >&2
        return 1
    fi
    if [ "$proj" != "$(cd "$K2_RELEASE_DRY_TREE" 2>/dev/null && pwd -P)" ] || [ "$proj" = "$K2_RELEASE_DRY_SRC" ]; then
        echo "ERROR: a dry release runs only in its scratch tree ($K2_RELEASE_DRY_TREE), not in $proj." >&2
        return 1
    fi
    gd="$(cd "$proj" && cd "$(git rev-parse --git-dir)" && pwd -P)" || return 1
    cd="$(cd "$proj" && cd "$(git rev-parse --git-common-dir)" && pwd -P)" || return 1
    if [ "$gd" = "$cd" ]; then
        echo "ERROR: $proj is a repository's main checkout, not a scratch worktree: refusing to rehearse in it." >&2
        return 1
    fi
    mkdir -p "$K2_RELEASE_DRY_RUN_DIR" || return 1
    : > "$K2_RELEASE_DRY_RUN_DIR/steps.tsv"
    : > "$K2_RELEASE_DRY_RUN_DIR/notes.tsv"
    : > "$K2_RELEASE_DRY_RUN_DIR/would.txt"
    K2DRY_T_START="$(date +%s)"
    echo "═══════════════════════════════════════════════════"
    echo "  DRY RUN (rehearsal). Nothing is pushed, tagged, uploaded or announced;"
    echo "  update-key signing is skipped (it would block the real release)."
    echo "  Every publishing step prints \"[dry] would: ...\"."
    echo "  scratch tree: $proj"
    echo "  run dir:      $K2_RELEASE_DRY_RUN_DIR"
    echo "═══════════════════════════════════════════════════"
}

# ── Steps, notes and the summary (dry only; no-ops otherwise) ──
K2DRY_CUR_ID=""
K2DRY_CUR_TITLE=""
K2DRY_CUR_T0=0
K2DRY_CUR_STATUS=""
K2DRY_T_START="${K2DRY_T_START:-0}"
K2DRY_FINISHED=""

_k2dry_note() { # <kind> <text>
    printf '%s\t%s\t%s\n' "${K2DRY_CUR_ID:--}" "$1" "$2" >> "$K2_RELEASE_DRY_RUN_DIR/notes.tsv" 2>/dev/null || true
}

_k2dry_close_step() { # [status]
    [ -n "$K2DRY_CUR_ID" ] || return 0
    local st="${1:-$K2DRY_CUR_STATUS}"
    printf '%s\t%s\t%s\t%s\n' "$K2DRY_CUR_ID" "$st" "$(( $(date +%s) - K2DRY_CUR_T0 ))" "$K2DRY_CUR_TITLE" \
        >> "$K2_RELEASE_DRY_RUN_DIR/steps.tsv" 2>/dev/null || true
    K2DRY_CUR_ID=""
}

# k2dry_step <id> <title>: the step that starts here (closes the last one).
k2dry_step() {
    k2dry_on || return 0
    [ -n "${K2_RELEASE_DRY_RUN_DIR:-}" ] || return 0
    _k2dry_close_step
    K2DRY_CUR_ID="$1"
    K2DRY_CUR_TITLE="$2"
    K2DRY_CUR_T0="$(date +%s)"
    K2DRY_CUR_STATUS=pass
    return 0
}

# k2dry_would <command and what it would do>: a publishing command not run.
k2dry_would() {
    echo "  [dry] would: $*"
    _k2dry_note would "$*"
    printf '%s\t%s\n' "${K2DRY_CUR_ID:--}" "$*" >> "$K2_RELEASE_DRY_RUN_DIR/would.txt" 2>/dev/null || true
}

# k2dry_skip <why>: part of a step not run in a rehearsal.
k2dry_skip() {
    echo "  [dry] skipped: $*"
    _k2dry_note skipped "$*"
}

# k2dry_skip_step <why>: the whole step not run (status "skipped").
k2dry_skip_step() {
    k2dry_skip "$@"
    K2DRY_CUR_STATUS=skipped
}

# k2dry_fail <why>: a check failed; the rehearsal goes on and ends DRY_EXIT=1.
k2dry_fail() {
    printf '\033[1;31m  [dry] ✗ %s\033[0m\n' "$*" >&2
    _k2dry_note fail "$*"
    K2DRY_CUR_STATUS=FAIL
}

# 0 when a check failed or the guard caught a command.
k2dry_failed() {
    [ -s "$K2_RELEASE_DRY_RUN_DIR/notes.tsv" ] || return 1
    awk -F'\t' '$2 == "fail" || $2 == "guard" { f = 1 } END { exit f ? 0 : 1 }' "$K2_RELEASE_DRY_RUN_DIR/notes.tsv"
}

# k2dry_finish <rc>: from the EXIT traps. Prints the summary once.
k2dry_finish() {
    k2dry_on || return 0
    [ "${K2_RELEASE_DRY_INNER:-0}" = 1 ] || return 0
    [ -n "${K2_RELEASE_DRY_RUN_DIR:-}" ] && [ -f "$K2_RELEASE_DRY_RUN_DIR/steps.tsv" ] || return 0
    [ -z "$K2DRY_FINISHED" ] || return 0
    K2DRY_FINISHED=1
    ( set +e; _k2dry_summary "${1:-0}" ) || true
    return 0
}

_k2dry_summary() {
    local rc="$1" dry_exit run="$K2_RELEASE_DRY_RUN_DIR" proj="${PROJECT_DIR:-$K2_RELEASE_DRY_TREE}" v="${VERSION:-?}"
    if [ "$rc" != 0 ]; then _k2dry_close_step FAIL; else _k2dry_close_step; fi
    dry_exit="$rc"
    if [ "$dry_exit" = 0 ] && k2dry_failed; then dry_exit=1; fi
    {
        echo ""
        echo "═══════════════════════════════════════════════════"
        echo "  DRY RELEASE SUMMARY: v${v} (rehearsal; nothing published)"
        echo "═══════════════════════════════════════════════════"
        printf '  %-6s %-8s %7s  %s\n' step status time "what (would = publishing commands not run)"
        awk -F'\t' -v notes="$run/notes.tsv" '
            BEGIN {
                while ((getline line < notes) > 0) {
                    split(line, f, "\t")
                    if (f[2] == "would") w[f[1]]++
                    if (f[2] == "skipped") s[f[1]]++
                    if (f[2] == "guard") g[f[1]]++
                }
            }
            {
                st = $2
                if (g[$1] > 0) st = "FAIL"
                extra = ""
                if (w[$1] > 0) extra = extra sprintf("; would: %d", w[$1])
                if (s[$1] > 0 && st != "skipped") extra = extra sprintf("; skipped parts: %d", s[$1])
                if (g[$1] > 0) extra = extra "; GUARD caught a publishing command"
                printf "  %-6s %-8s %4dm%02ds  %s%s\n", $1, st, int($3 / 60), $3 % 60, $4, extra
            }' "$run/steps.tsv"
        echo "  total: $(( ( $(date +%s) - ${K2DRY_T_START:-$(date +%s)} ) / 60 )) min"
        if [ -s "$run/notes.tsv" ] && awk -F'\t' '$2 == "fail" || $2 == "guard" { f = 1 } END { exit f ? 0 : 1 }' "$run/notes.tsv"; then
            echo "  failed checks:"
            awk -F'\t' '$2 == "fail" || $2 == "guard" { printf "    %s: %s\n", $1, $3 }' "$run/notes.tsv"
        fi
        echo "  artifacts:"
        local p
        for p in \
            "$proj/target/release/bundle/macos/K2.app" \
            "$proj/target/release/bundle/macos/K2.app.tar.gz" \
            "$proj/target/release/bundle/dmg/K2_${v}_aarch64.dmg" \
            "$proj/target/release/daemon-dist" \
            "$run/latest.json" \
            "$run/daemon-latest.json" \
            "$run/release-notes.md" \
            "$proj/dist-windows" \
            "$run/audit/${v}/compute" \
            "$run/would.txt"; do
            [ -e "$p" ] && echo "    $p"
        done
        echo "  the scratch tree (version files bumped, never committed to a branch): $proj"
        echo "  signatures: placeholders (update-key signing never runs in a rehearsal)"
        echo "DRY_EXIT=${dry_exit}"
    } | tee "$run/summary.txt"
}

# ── Guard: a publishing command with no dry branch is caught, not run ──
# Defined only in a dry run, only in release.sh's own process (not exported):
# every publishing line of release.sh has its own "[dry] would" branch, so
# the guard firing is a bug; it says so and the rehearsal ends DRY_EXIT=1.
_k2dry_guard() {
    echo "  [dry] would: $* (GUARD: a publishing command without a dry branch; not run)" >&2
    _k2dry_note guard "$*"
    printf '%s\t%s\n' "${K2DRY_CUR_ID:--}" "$* (guard)" >> "$K2_RELEASE_DRY_RUN_DIR/would.txt" 2>/dev/null || true
    return 0
}
if k2dry_on; then
    git() {
        local a sub="" skip=0 nargs=0 listing=0
        for a in "$@"; do
            if [ "$skip" = 1 ]; then skip=0; continue; fi
            if [ -z "$sub" ]; then
                case "$a" in
                    -C|-c|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix) skip=1 ;;
                    -*) ;;
                    *) sub="$a" ;;
                esac
                continue
            fi
            nargs=$((nargs + 1))
            case "$a" in -l|--list|--contains|--no-contains|--points-at|--merged|--no-merged|-n*|--sort=*|--format=*|--column*) listing=1 ;; esac
        done
        case "$sub" in
            push|send-pack|send-email|request-pull) _k2dry_guard git "$@"; return 0 ;;
            tag) if [ "$nargs" -gt 0 ] && [ "$listing" = 0 ]; then _k2dry_guard git "$@"; return 0; fi ;;
        esac
        command git "$@"
    }
    gh() {
        local a w=0 m=""
        case "${1:-} ${2:-}" in
            "release create"|"release upload"|"release edit"|"release delete"|"release delete-asset"|\
            "variable set"|"variable delete"|"secret set"|"secret delete"|"workflow run"|"workflow enable"|\
            "workflow disable"|"run rerun"|"run cancel"|"run delete"|"repo edit"|"repo create"|"pr create"|\
            "pr merge"|"pr edit"|"issue create"|"issue edit"|"issue comment"|"label create"|"cache delete")
                _k2dry_guard gh "$@"; return 0 ;;
        esac
        if [ "${1:-}" = api ]; then
            for a in "$@"; do
                if [ "$m" = next ]; then m="$a"; continue; fi
                case "$a" in
                    -X|--method) m=next ;;
                    -X*) m="${a#-X}" ;;
                    --method=*) m="${a#--method=}" ;;
                    -f|-F|--field|--raw-field|--input|-f*|-F*|--field=*|--raw-field=*|--input=*) w=1 ;;
                esac
            done
            case "$m" in
                ""|GET|get|HEAD|head) [ "$w" = 1 ] && [ -z "$m" ] && { _k2dry_guard gh "$@"; return 0; } ;;
                *) _k2dry_guard gh "$@"; return 0 ;;
            esac
        fi
        command gh "$@"
    }
    curl() {
        local a m="" next=0
        for a in "$@"; do
            if [ "$next" = 1 ]; then m="$a"; next=0; continue; fi
            case "$a" in
                -T|--upload-file|-T*|--upload-file=*|-d|--data|--data-*|-d*|-F|--form|--form-*|-F*|--json) _k2dry_guard curl "$@"; return 0 ;;
                -X|--request) next=1 ;;
                -X*) m="${a#-X}" ;;
                --request=*) m="${a#--request=}" ;;
            esac
        done
        case "$m" in ""|GET|HEAD) ;; *) _k2dry_guard curl "$@"; return 0 ;; esac
        command curl "$@"
    }
fi

# ── Signing and approval stand-ins (dry only; release.sh calls this right
# after it sources scripts/lib/sign-updater.sh and sign-approve.sh) ──
k2dry_install_overrides() {
    k2dry_on || return 0
    # A placeholder in place of each update-key signature. Real ones would
    # register this build's sha for the version with the signing user, and
    # the real release (another build: other bytes) would then be refused.
    k2u_sign() { # <slot> <file> <asset> <version> <out> <form>
        local slot="$1" file="$2" asset="$3" out="$5"
        [ -f "$file" ] || { echo "[sign-updater] ERROR: k2u_sign: no file $file" >&2; return 1; }
        printf 'K2-DRY-RUN-NOT-SIGNED %s %s' "$slot" "$asset" > "$out" || return 1
        k2dry_skip "update-key signature ${slot} of ${asset} (placeholder written: a rehearsal signature would block the real release's)"
    }
    k2u_verify_u1() {
        k2dry_skip "the U1 check of ${3:-$1} (not signed in a rehearsal)"
        return 0
    }
    k2u_verify_u2() {
        k2dry_skip "the U2 check of ${3:-$1} (not signed in a rehearsal)"
        return 0
    }
    # Step 1.55: no approval session (it would bind this version to this
    # base; signing under it marks the version used). Shows the signer's
    # state for the version, read-only, so the real run's prompt is no surprise.
    k2sa_open_session() { # <version> <base> <assets...>
        local version="$1" base="$2" out
        k2dry_skip_step "the signing approval (Touch ID) for v${version} at ${base:0:12}: nothing is signed in a rehearsal"
        if out="$(k2u_k2sign status "$version" 2>&1)"; then
            echo "  signer status for v${version} (read-only; the real run asks for its own approval):"
            printf '%s\n' "$out" | head -20 | sed 's/^/    /'
        else
            echo "  signer status for v${version}: unavailable ($(printf '%s' "$out" | head -1))"
        fi
        return 0
    }
}

# ── Step 9 (dry): what `gh release create` would upload must all be there ──
# k2dry_release_assets_check <asset>...
k2dry_release_assets_check() {
    local a n=0 bad=0
    for a in "$@"; do
        n=$((n + 1))
        if [ -s "$a" ]; then
            printf '    %s (%s bytes)\n' "$a" "$(wc -c < "$a" | tr -d ' ')"
        else
            k2dry_fail "Step 9 would upload $a, but it is missing or empty"
            bad=1
        fi
    done
    echo "  $n release assets checked."
    return "$bad"
}

# ── Step 10 (dry): the manifests as written, checked the way the updaters
# read them (minus the signatures, which are placeholders) ──
# k2dry_check_manifests <latest.json> <daemon-latest.json> <mac daemon> <version> <repo> <tag> <u2 key id>
k2dry_check_manifests() {
    python3 -I - "$@" <<'PY'
import hashlib, json, sys
latest, daemon, mac, version, repo, tag, u2 = sys.argv[1:8]
base = "https://github.com/%s/releases/download/%s" % (repo, tag)
bad = []
m = json.load(open(latest))
if m.get("version") != version:
    bad.append("latest.json version is %r" % m.get("version"))
e = (m.get("platforms") or {}).get("darwin-aarch64") or {}
if e.get("url") != base + "/K2.app.tar.gz":
    bad.append("latest.json darwin-aarch64 url is %r" % e.get("url"))
if not str(e.get("signature") or "").strip():
    bad.append("latest.json darwin-aarch64 has no signature field")
if u2 not in (e.get("k2_signatures") or {}):
    bad.append("latest.json darwin-aarch64 has no k2_signatures entry for U2 (%s)" % u2)
d = json.load(open(daemon))
if d.get("version") != version:
    bad.append("daemon-latest.json version is %r" % d.get("version"))
if not d.get("notes"):
    bad.append("daemon-latest.json has no notes")
a = (d.get("artifacts") or {}).get("macos-aarch64") or {}
sha = hashlib.sha256(open(mac, "rb").read()).hexdigest()
want = {"url": base + "/k2-daemon-macos-aarch64", "sig": base + "/k2-daemon-macos-aarch64.sig",
        "sig2": base + "/k2-daemon-macos-aarch64.sig2", "sha256": sha}
for k, v in want.items():
    if a.get(k) != v:
        bad.append("daemon-latest.json macos-aarch64 %s is %r, want %r" % (k, a.get(k), v))
for b in bad:
    print("  ✗ " + b, file=sys.stderr)
if bad:
    sys.exit(1)
print("  ✓ latest.json and daemon-latest.json: v%s, the URLs this release would publish, the macOS daemon's sha256, notes; "
      "signature fields present (placeholders)" % version)
PY
}

# ── Step 10.5 (dry): what can be checked before the release exists ──
# Nothing is signed or uploaded. The staged Linux files (compute mode) get
# the byte checks the sign-* scripts run before they sign, and the signing
# user's read-only `k2sign compute-rehearse` checks the compute evidence it
# would take at 10.5 (registering it is once per version, never replaced).
k2dry_linux_sign_rehearsal() {
    local arch f arches=x86_64 ev="$K2_RELEASE_DRY_RUN_DIR/compute-evidence.json"
    k2dry_would "bash scripts/sign-daemon.sh ${DAEMON_SIGN_ARGS[*]} (checks provenance, signs the Linux daemons U1+U2 under the approval, uploads the .sig/.sig2 and daemon-latest.json)"
    [ "${K2_SKIP_ROOT_HELPER_SIGN:-0}" = 1 ] || k2dry_would "bash scripts/sign-root-helper.sh ${VERSION} ${ROOT_KEY_SIGN_ARGS[*]-} (signs the root helper, uploads its .sig)"
    [ "${K2_SKIP_K2_NODE_SIGN:-0}" = 1 ] || k2dry_would "bash scripts/sign-k2-node.sh ${VERSION} ${ROOT_KEY_SIGN_ARGS[*]-} (signs k2-node, uploads its .sig)"
    [ "${K2_SKIP_APP_ENGINE_SIGN:-0}" = 1 ] || k2dry_would "bash scripts/sign-app-engine.sh ${VERSION} ${ROOT_KEY_SIGN_ARGS[*]-} (signs the App engine, uploads its .sig)"
    if [ "$K2_LINUX_BUILDS" != compute ]; then
        k2dry_skip "no Linux files to check: GitHub builds them after the tag (K2_LINUX_BUILDS=${K2_LINUX_BUILDS})"
        return 0
    fi
    [ "${K2_COMPUTE_ARM:-}" = compute ] && [ -f "$DIST_DIR/k2-daemon-linux-aarch64" ] && arches="x86_64 aarch64"
    for arch in $arches; do
        f="$DIST_DIR/k2-daemon-linux-$arch"
        if [ ! -s "$f" ]; then
            k2dry_fail "k2-daemon-linux-$arch was not staged at Step 8.5"
            continue
        fi
        if python3 -I "$PROJECT_DIR/scripts/lib/check-daemon-asset.py" "$f" "$arch" "$VERSION" \
            && ( source "$PROJECT_DIR/scripts/require-mail-oauth-build-env.sh"; assert_daemon_oauth_not_placeholder "$f" ); then
            echo "  ✓ k2-daemon-linux-$arch: the byte checks and the OAuth bake check sign-daemon.sh runs before it signs"
        else
            k2dry_fail "k2-daemon-linux-$arch failed the checks sign-daemon.sh runs before it signs (above)"
        fi
        f="$DIST_DIR/k2-node-linux-$arch"
        if [ -f "$f" ]; then
            bash "$PROJECT_DIR/scripts/sign-k2-node.sh" --check "$f" linux "$arch" "$VERSION" >/dev/null \
                && echo "  ✓ k2-node-linux-$arch: sign-k2-node.sh --check" \
                || k2dry_fail "k2-node-linux-$arch failed sign-k2-node.sh --check"
        fi
        f="$DIST_DIR/k2-app-engine-linux-$arch"
        if [ -f "$f" ]; then
            bash "$PROJECT_DIR/scripts/sign-app-engine.sh" --check "$f" "$arch" "$VERSION" >/dev/null \
                && echo "  ✓ k2-app-engine-linux-$arch: sign-app-engine.sh --check" \
                || k2dry_fail "k2-app-engine-linux-$arch failed sign-app-engine.sh --check"
        fi
    done
    if k2_ci_client; then
        echo "  k2-signer rehearsed this evidence at Step 7.9c (k2-signer rehearse k2: nothing registered or signed)."
        k2dry_would "register the evidence with k2-signer (sign-daemon.sh --ci-evidence: once per version, never replaced)"
    elif [ "$K2_SIGNER" = k2sign ]; then
        arches=x86_64
        [ -f "$DIST_DIR/k2-daemon-linux-aarch64" ] && [ -f "$K2_SHADOW_STATE/L2/receipt.json" ] && arches=x86_64,aarch64
        if ! python3 -I "$PROJECT_DIR/scripts/ci/compute_release.py" evidence-bundle --state "$K2_SHADOW_STATE" \
            --git "$PROJECT_DIR" --arches "$arches" > "$ev"; then
            k2dry_fail "could not build the K2 Compute evidence bundle from $K2_SHADOW_STATE (above)"
        elif k2u_k2sign compute-rehearse < "$ev" > "${ev%.json}.rehearsed.json"; then
            echo "  ✓ the signing user's compute-rehearse passed (${arches}; read-only: nothing registered or signed)"
        else
            k2dry_fail "the signing user's compute-rehearse refused the evidence (above): Step 10.5 would refuse to sign the Linux daemons"
        fi
        k2dry_would "register the evidence with the signing user (k2sign compute-evidence: once per version, never replaced)"
    else
        k2dry_skip "the signer's compute rehearsal (K2_SIGNER=keychain has no signing user)"
    fi
}

# ── Step 10.6 (dry): the staged packages vs the SHA256SUMS lines Step 9
# would upload (the published file is checked after a real release) ──
k2dry_linux_sums_local() {
    if [ "$K2_LINUX_BUILDS" != compute ] || [ ! -f "$DIST_DIR/SHA256SUMS-linux-x86_64.txt" ]; then
        k2dry_skip_step "the Linux packages and SHA256SUMS come from GitHub after the tag"
        return 0
    fi
    if (cd "$DIST_DIR" && { sha256sum -c --quiet SHA256SUMS-linux-x86_64.txt 2>/dev/null || shasum -a 256 -c --quiet SHA256SUMS-linux-x86_64.txt; }); then
        echo "  ✓ the staged deb/rpm/AppImage match SHA256SUMS-linux-x86_64.txt (the Arch line is added by arch-package.yml after the tag)"
    else
        k2dry_fail "the staged Linux packages don't match SHA256SUMS-linux-x86_64.txt"
    fi
    k2dry_skip "the published release's SHA256SUMS against GitHub's asset digests (no release in a rehearsal)"
}

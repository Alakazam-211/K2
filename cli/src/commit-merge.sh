# ── Phase 2.1: commit --merge flag handler ───────────────────────────

cmd_commit_v2() {
    # -h/--help anywhere prints the usage and commits nothing (0.46.17).
    if _wants_help "$@"; then
        echo "Usage: k2 commit [-m \"...\"] [--merge]   AI-assisted commit (--merge to also merge)"
        return 0
    fi
    local has_merge="false"
    for arg in "$@"; do
        [[ "$arg" == "--merge" ]] && has_merge="true"
    done
    if [[ "$has_merge" == "true" ]]; then
        # Strip --merge and call commit-merge.
        local rest=()
        for arg in "$@"; do
            [[ "$arg" == "--merge" ]] && continue
            rest+=("$arg")
        done
        cmd_commit_merge "${rest[@]}"
    else
        cmd_commit "$@"
    fi
}


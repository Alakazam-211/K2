# ── Phase 2.1: commit --merge flag handler ───────────────────────────

cmd_commit_v2() {
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


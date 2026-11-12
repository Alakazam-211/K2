# ── Onboarding (workspace-add three-option flow) ─────────────────────
#
# Thin shell wrappers around `/cli/onboarding/*` daemon routes. Logic
# lives in k2so-core::agents::onboarding so the Tauri modal and the
# CLI share a single implementation. Project path defaults to $PWD.

# k2 onboarding scan [project-path]
#   List workspace harness files (CLAUDE.md, GEMINI.md, .cursor/rules,
#   etc.) that have substantive user content. Empty result == no
#   prompt needed; a non-empty result is what the GUI modal shows in
#   the "Adopt" picker.
cmd_onboarding_scan() {
    local target="${1:-$PWD}"
    target="$(cd "$target" && pwd)"
    cli_request "/cli/onboarding/scan" "project=$(urlencode "$target")"
}

# k2 onboarding adopt <relative-or-absolute-path> [project-path]
#   Adopt the named harness file as the seed for .k2so/PROJECT.md and
#   trigger a workspace SKILL regen. The source file is archived to
#   .k2so/migration/ and removed (so the regen's existing migration
#   helpers don't double-import the same body).
cmd_onboarding_adopt() {
    local source="${1:-}"
    local target="${2:-$PWD}"
    if [ -z "$source" ]; then
        echo "Usage: k2 onboarding adopt <file> [project-path]" >&2
        echo "Run 'k2 onboarding scan' to see candidate files." >&2
        exit 1
    fi
    target="$(cd "$target" && pwd)"
    # Resolve to absolute path if user passed a relative one
    if [[ "$source" != /* ]]; then
        source="$target/$source"
    fi
    cli_request_post "/cli/onboarding/adopt" \
        "project=$(urlencode "$target")&source=$(urlencode "$source")"
}

# k2 onboarding later [project-path]
#   "Do it later" — drops .k2so/.skip-harness-management so K2 won't
#   touch CLAUDE.md / GEMINI.md / .cursor/rules / etc. on subsequent
#   regens. K2's internal SKILL still gets written so heartbeats and
#   agent launches keep working. Reversible by removing the flag.
cmd_onboarding_later() {
    local target="${1:-$PWD}"
    target="$(cd "$target" && pwd)"
    cli_request_post "/cli/onboarding/skip" "project=$(urlencode "$target")"
}

# k2 onboarding fresh [project-path]
#   Run the workspace regen normally, archiving any pre-existing
#   harness files to .k2so/migration/ and replacing them with K2
#   symlinks. Equivalent to the renderer's "Start Fresh" button.
cmd_onboarding_fresh() {
    local target="${1:-$PWD}"
    target="$(cd "$target" && pwd)"
    cli_request_post "/cli/onboarding/start-fresh" "project=$(urlencode "$target")"
}


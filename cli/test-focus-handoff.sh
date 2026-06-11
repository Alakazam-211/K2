#!/usr/bin/env bash
# Focus-window handoff smoke test.
#
# Verifies that opening a focus window on a workspace ADOPTS the
# daemon's live sessions instead of spawning duplicates.
#
# Usage:
#   ./cli/test-focus-handoff.sh <workspace-path>
#
# Example:
#   ./cli/test-focus-handoff.sh "/Users/z3thon/DevProjects/Alakazam Labs/K2SO"
#
# What it does:
#   1. Snapshots live session count for the workspace.
#   2. Picks the first session and writes a sentinel string to it
#      (NOT followed by \n, so it stays in the prompt buffer).
#   3. Reads back to confirm the sentinel is now in the grid.
#   4. Prompts you to right-click the workspace in the K2SO sidebar
#      and pick "Open in Focus Window" — then press Enter here.
#   5. Snapshots session count again.
#   6. Reads the same session's grid and asserts the sentinel is
#      still there. (If a fresh PTY were spawned, the sentinel
#      would be gone — the new shell starts with an empty buffer.)
#
# Exit codes:
#   0  — handoff worked: count unchanged + sentinel preserved.
#   1  — count changed (duplicate-spawn regression).
#   2  — sentinel missing in the live session post-focus (PTY swap).
#   3  — no live sessions to test against.
#   *  — setup error.

set -u

WORKSPACE="${1:-}"
if [ -z "$WORKSPACE" ]; then
    echo "Usage: $0 <workspace-path>" >&2
    exit 64
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2SO="${SCRIPT_DIR}/k2so"

# Use the workspace path for K2SO's --project context. Otherwise the
# CLI's PROJECT defaults to cwd which may not match.
export K2SO_PROJECT_PATH="$WORKSPACE"

separator() { echo "─────────────────────────────────────────────────────"; }

separator
echo "Focus-window handoff smoke test"
echo "Workspace: $WORKSPACE"
separator

# Step 1: snapshot before
echo
echo "[1/6] Snapshot BEFORE focus-window open"
"$K2SO" sessions live "$WORKSPACE"
COUNT_BEFORE=$("$K2SO" sessions live "$WORKSPACE" --count 2>/dev/null)
if [ -z "$COUNT_BEFORE" ] || [ "$COUNT_BEFORE" = "0" ]; then
    echo
    echo "✗ No live sessions in workspace. Open the workspace in K2SO and start"
    echo "  a terminal session first, then re-run this script."
    exit 3
fi
echo "  → $COUNT_BEFORE live session(s)"

# Step 2: pick first session, write sentinel
SENTINEL="K2SO-HANDOFF-$(date +%s)-$RANDOM"
echo
echo "[2/6] Writing sentinel \"$SENTINEL\" to first session (no newline)"
TARGET_ID=$("$K2SO" sessions live "$WORKSPACE" --json 2>/dev/null | python3 -c "
import json, sys
arr = json.loads(sys.stdin.read())
if not arr: sys.exit(1)
print(arr[0].get('sessionId') or arr[0].get('terminalId') or '')
")
if [ -z "$TARGET_ID" ]; then
    echo "✗ Could not extract target session id." >&2
    exit 65
fi
echo "  → target: $TARGET_ID"
"$K2SO" terminal write "$TARGET_ID" "$SENTINEL" >/dev/null

# Step 3: confirm sentinel landed
echo
echo "[3/6] Reading back grid to confirm sentinel landed"
sleep 0.5
GRID_PRE=$("$K2SO" terminal read "$TARGET_ID" --lines 80 2>/dev/null)
if echo "$GRID_PRE" | grep -q "$SENTINEL"; then
    echo "  ✓ sentinel visible in pre-focus grid"
else
    echo "  ✗ sentinel not visible — terminal may not be at a prompt accepting text"
    echo "  (continuing anyway; the duplicate-spawn check below is still meaningful)"
fi

# Step 4: prompt user to open focus window
echo
echo "[4/6] Now, in K2SO:"
echo "      → right-click the workspace in the sidebar"
echo "      → choose \"Open in Focus Window\""
echo "      → wait for the focus window to fully load"
echo
read -p "      Press Enter when the focus window is open and visible..." _

sleep 1  # give the renderer a moment to settle

# Step 5: snapshot after
echo
echo "[5/6] Snapshot AFTER focus-window open"
"$K2SO" sessions live "$WORKSPACE"
COUNT_AFTER=$("$K2SO" sessions live "$WORKSPACE" --count 2>/dev/null)
echo "  → $COUNT_AFTER live session(s)"

# Step 6: verify
echo
echo "[6/6] Verdict"
separator

if [ "$COUNT_BEFORE" != "$COUNT_AFTER" ]; then
    echo "✗ FAIL — session count changed (was $COUNT_BEFORE, now $COUNT_AFTER)"
    echo "    Opening the focus window spawned (or killed) sessions."
    echo "    Expected: count unchanged (adoption)."
    exit 1
fi

GRID_POST=$("$K2SO" terminal read "$TARGET_ID" --lines 80 2>/dev/null)
if ! echo "$GRID_POST" | grep -q "$SENTINEL"; then
    echo "✗ FAIL — sentinel \"$SENTINEL\" is no longer in session $TARGET_ID"
    echo "    The focus window may have swapped the PTY behind the same id."
    exit 2
fi

echo "✓ PASS — count unchanged ($COUNT_BEFORE) AND sentinel preserved"
echo "    The focus window adopted the existing daemon-side sessions."
separator
exit 0

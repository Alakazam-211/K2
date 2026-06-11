#!/usr/bin/env bash
# 0.39.0f Phase 2.1c — `k2so msg <ws> --inbox` writes to a remote
# workspace's inbox via POST /cli/inbox/compose?project=<target>.
# Replaces the Phase 2.1b transitional /cli/work/inbox/create route
# (now 410, see work_routes_410_gone.sh).
#
# This test:
#   1. Boots a sandbox daemon under $SANDBOX_HOME (acts as workspace A)
#   2. Creates a second workspace dir (B) inside the sandbox
#   3. POSTs to /cli/inbox/compose with project=B from "outside" B
#   4. Verifies the .md file landed in B/.k2so/inbox/
#   5. Verifies the file contents include the title + body
#
# We exercise the *daemon HTTP route* directly here (same as the CLI
# does via curl in cmd_msg_inbox_form). Calling the CLI binary against
# our sandbox daemon would require overriding $HOME/.k2so/daemon.port,
# which works in practice but is one more moving part — the HTTP probe
# is the authoritative check.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

base="http://127.0.0.1:${K2SO_PORT}"

# Workspace B lives inside the sandbox HOME but is treated as a
# separate workspace path. The compose handler creates the inbox
# directory on demand, so we just need the parent dir to exist.
WORKSPACE_B="${SANDBOX_HOME}/workspace_B"
mkdir -p "$WORKSPACE_B"

echo "→ POST /cli/inbox/compose with project=$WORKSPACE_B"
# Note: we deliberately set project= to workspace B's path (not the
# sandbox HOME). This is exactly what cmd_msg_inbox_form does when the
# user runs `k2so msg <target-ws> --inbox`.
#
# The daemon's inbox POST routes pull params from the URL query
# string (not the body) — same as /cli/inbox/respond etc. `curl -sG
# -X POST -d ...` makes -d become the query string instead of body.
# Match the cmd_msg_inbox_form invocation exactly.
compose_status="$(curl -sG -o /tmp/_k2so_compose.$$ \
    -w "%{http_code}" --connect-timeout 3 --max-time 10 \
    -X POST "${base}/cli/inbox/compose" \
    -d "token=${K2SO_TOKEN}&project=${WORKSPACE_B}&title=hello&body=from%20A&from=workspace_A&priority=normal")"
compose_body="$(cat /tmp/_k2so_compose.$$)"
rm -f /tmp/_k2so_compose.$$

echo "  HTTP $compose_status — body: $compose_body"
if [ "$compose_status" != "200" ]; then
    echo "FAIL: compose POST returned $compose_status; expected 200" >&2
    exit 1
fi

# Compose returns a serialized InboxItem JSON with a filename / path.
echo "$compose_body" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
# The compose handler returns the persisted InboxItem. We expect at
# minimum a title field.
title = data.get("title")
if title != "hello":
    print(f"FAIL: compose response missing or wrong title: {data}", file=sys.stderr)
    sys.exit(1)
print(f"OK: compose response title={title}")
' || exit 1

# ── Verify the file landed in workspace B's inbox dir ───────────────
inbox_dir="$WORKSPACE_B/.k2so/inbox"
if [ ! -d "$inbox_dir" ]; then
    echo "FAIL: $inbox_dir was not created" >&2
    ls -la "$WORKSPACE_B/.k2so/" 2>&1 || true
    exit 1
fi

# Find the newest .md file in the inbox top-level (compose puts it there).
created_file="$(find "$inbox_dir" -maxdepth 1 -name "*.md" -type f | head -1)"
if [ -z "$created_file" ]; then
    echo "FAIL: no .md file found in $inbox_dir" >&2
    ls -la "$inbox_dir" >&2 || true
    exit 1
fi
echo "→ Created file: $created_file"

# Check contents include the title + body.
if ! grep -q "hello" "$created_file"; then
    echo "FAIL: created file does not contain title 'hello'" >&2
    cat "$created_file" >&2
    exit 1
fi
if ! grep -q "from A" "$created_file"; then
    echo "FAIL: created file does not contain body 'from A'" >&2
    cat "$created_file" >&2
    exit 1
fi

# ── Verify the old route is 410 ─────────────────────────────────────
echo "→ Confirm /cli/work/inbox/create returns 410 Gone"
old_status="$(curl -s -o /tmp/_k2so_old.$$ -w "%{http_code}" \
    --connect-timeout 3 --max-time 10 \
    "${base}/cli/work/inbox/create?token=${K2SO_TOKEN}&project=${SANDBOX_HOME}&workspace=${WORKSPACE_B}&title=should&body=fail")"
old_body="$(cat /tmp/_k2so_old.$$)"
rm -f /tmp/_k2so_old.$$
if [ "$old_status" != "410" ]; then
    echo "FAIL: /cli/work/inbox/create returned $old_status; expected 410" >&2
    echo "      body: $old_body" >&2
    exit 1
fi
echo "  HTTP 410 — body: $old_body"

echo "OK: msg --inbox cross-workspace delivery via /cli/inbox/compose; old route 410"

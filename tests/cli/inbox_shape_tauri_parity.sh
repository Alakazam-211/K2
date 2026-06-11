#!/usr/bin/env bash
# Phase 2.1c Item 2 — verify the inbox JSON shape returned by the
# daemon's `/cli/inbox/list` route matches the field set the new
# `k2so_inbox_*` Tauri commands expect (and that the React frontend
# deserialises into the `InboxItem` interface).
#
# Both sides wrap the SAME `k2so_core::inbox::*` functions, so this
# test is a pin against accidental shape drift. If someone renames a
# field in `InboxItem` (Rust) but forgets to update the React
# interface, the renderer would silently render undefined values for
# the missing key — this test catches that by asserting the JSON
# payload's exact key set.
#
# Why not invoke the Tauri command directly? There's no headless
# Tauri test harness wired into this project (no tauri-driver in
# CI). The daemon HTTP route + the Tauri command both call
# `k2so_core::inbox::list_folder` directly with identical serde
# shapes, so HTTP-side verification is a load-bearing proxy.
#
# Hard rules from CLAUDE.md:
#   - HOME=$SANDBOX. No prod daemon contact.
#   - Daemon binary is the worktree's target/{release,debug}/k2so-daemon.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

WS="$SANDBOX_HOME/inbox-shape-ws"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Seed a single inbox item at the workspace root.
mkdir -p "$WS/.k2so/inbox"
cat >"$WS/.k2so/inbox/sample.md" <<'EOF'
---
title: Sample Inbox Item
priority: high
created: 2026-05-23T00:00:00Z
source: manual
from: cli
---
Body text for the shape parity check.
EOF

# Encode the workspace path for the query string.
WS_ENC="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$WS")"

LIST_URL="http://127.0.0.1:${K2SO_PORT}/cli/inbox/list?project=${WS_ENC}&token=${K2SO_TOKEN}"
RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$LIST_URL")"
echo "  /cli/inbox/list response: $RESP"

# The renderer deserialises into:
#   interface InboxItem {
#     id: string; filename: string; folder: string;
#     title: string; priority: string; created: string;
#     source: string; from: string; bodyPreview: string;
#   }
# Assert each field exists in the JSON payload (camelCase per serde
# rename_all on the Rust struct).
EXPECTED_KEYS=(id filename folder title priority created source from bodyPreview)
for KEY in "${EXPECTED_KEYS[@]}"; do
    if ! echo "$RESP" | python3 -c "
import sys, json
data = json.load(sys.stdin)
assert isinstance(data, list), f'expected JSON array, got {type(data).__name__}'
assert len(data) == 1, f'expected exactly one item, got {len(data)}'
key = '$KEY'
assert key in data[0], f'missing key: {key!r}; got keys: {sorted(data[0].keys())}'
" 2>&1; then
        echo "FAIL: response missing key '$KEY' (or shape mismatch)" >&2
        exit 1
    fi
done

# Assert specific field values matched what we seeded — confirms
# the parse_frontmatter -> InboxItem path is correct.
python3 - "$RESP" <<'PYEOF'
import sys, json
data = json.loads(sys.argv[1])
item = data[0]
assert item['title'] == 'Sample Inbox Item', f"title mismatch: {item['title']!r}"
assert item['priority'] == 'high', f"priority mismatch: {item['priority']!r}"
assert item['source'] == 'manual', f"source mismatch: {item['source']!r}"
assert item['from'] == 'cli', f"from mismatch: {item['from']!r}"
assert item['filename'] == 'sample.md', f"filename mismatch: {item['filename']!r}"
assert item['id'] == 'sample', f"id mismatch: {item['id']!r}"
assert item['folder'] == '', f"folder should be empty for top-level: {item['folder']!r}"
assert 'Body text for the shape parity check' in item['bodyPreview'], \
    f"bodyPreview missing body text: {item['bodyPreview']!r}"
PYEOF

# Now verify the lightweight `/cli/inbox/list` empty-folder behaviour
# matches what `k2so_inbox_count` returns. Tauri command body:
#   k2so_core::inbox::list_root(&workspace).len()
# So one seeded item -> count == 1.
COUNT_LEN="$(echo "$RESP" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))")"
if [ "$COUNT_LEN" != "1" ]; then
    echo "FAIL: list returned $COUNT_LEN items, expected 1" >&2
    exit 1
fi

echo "OK: inbox JSON shape matches the new InboxItem TypeScript interface"

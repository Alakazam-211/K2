#!/usr/bin/env bash
# 0.39.0g Phase 2.5b followup — verify the new Tauri `k2so_skills_list`
# command and the existing daemon `/cli/agents/list` route (which the
# `k2so skills list` CLI verb shells through) return the same set of
# skill names for a workspace post-consolidation.
#
# Why this matters: the workspace settings Skills section uses the
# Tauri verb (in-process, returns `SkillSummary[]`), while the CLI and
# the CLI-driven tests use the daemon HTTP surface (returns
# `K2soAgentInfo[]`). If one starts reading from a different folder
# than the other, the UI and CLI would silently disagree about which
# skills exist in a workspace. This test pins both to the same
# `.k2so/skills/<name>/` reality.
#
# Why we don't invoke the Tauri command directly: there's no headless
# Tauri test harness wired into this project. The Tauri verb is a
# one-line forward to `k2so_core::skills::crud::list`, which reads
# `.k2so/skills/<name>/` directly. The daemon `/cli/agents/list` route
# (post-2.5b consolidation) ALSO reads `.k2so/skills/<name>/` via
# `k2so_core::agents::commands::list`. So filesystem-level inspection
# of `.k2so/skills/` is a load-bearing proxy for the Tauri verb's
# answer — same source folder, same probe rules.
#
# Hard rules:
#   - HOME=$SANDBOX_HOME. No prod daemon contact.
#   - Daemon binary is the worktree's target/{release,debug}/k2so-daemon.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

WS="$SANDBOX_HOME/skills-parity-ws"
mkdir -p "$WS"
(cd "$WS" && git init -q && git commit --allow-empty -m "init" -q 2>/dev/null || true)

# Mark the older migrations done so the 2.5b consolidation has a clean
# `.k2so/skills/` to work with directly (no sweeping of pre-existing
# `.k2so/agents/` content).
mkdir -p "$WS/.k2so"
touch "$WS/.k2so/.unification-0.37.0-done" \
      "$WS/.k2so/.work-to-inbox-migration-v1-done" \
      "$WS/.k2so/.skills-consolidation-v1-done"

# Seed three distinct skills directly at the post-2.5b path.
mkdir -p "$WS/.k2so/skills/alpha" "$WS/.k2so/skills/beta" "$WS/.k2so/skills/gamma"
cat >"$WS/.k2so/skills/alpha/SKILL.md" <<'EOF'
---
name: alpha
role: alpha role
---

# alpha

body
EOF
cat >"$WS/.k2so/skills/beta/SKILL.md" <<'EOF'
---
name: beta
role: beta role
---

# Beta Heading

body
EOF
# `gamma` deliberately seeded with `AGENT.md` (transitional shape) so
# the test also pins that both readers tolerate mid-migration files.
cat >"$WS/.k2so/skills/gamma/AGENT.md" <<'EOF'
---
name: gamma
role: gamma role
---

# gamma

body
EOF

# Register the workspace via the live daemon.
REG_URL="http://127.0.0.1:${K2SO_PORT}/cli/projects/add-from-path?token=${K2SO_TOKEN}"
REG_BODY="$(python3 -c "import json,sys; print(json.dumps({'path': sys.argv[1]}))" "$WS")"
REG_RESP="$(curl -sf --connect-timeout 5 --max-time 15 \
    -X POST -H "Content-Type: application/json" \
    -d "$REG_BODY" "$REG_URL")"
if ! echo "$REG_RESP" | grep -qF "\"path\":\"$WS\""; then
    echo "FAIL: add-from-path response did not echo our workspace path" >&2
    echo "response: $REG_RESP" >&2
    exit 1
fi

# Daemon-side view: what the CLI `k2so skills list` sees (it shells
# `/cli/agents/list`).
WS_ENC="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$WS")"
AGENTS_URL="http://127.0.0.1:${K2SO_PORT}/cli/agents/list?project=${WS_ENC}&token=${K2SO_TOKEN}"
AGENTS_RESP="$(curl -sf --connect-timeout 5 --max-time 15 "$AGENTS_URL")"
echo "  /cli/agents/list response: $AGENTS_RESP"

# Filesystem-side view: what the Tauri `k2so_skills_list` will see
# (it reads `.k2so/skills/<name>/` directly via
# `k2so_core::skills::crud::list`, skipping dotfile dirs).
FS_NAMES="$(ls -1 "$WS/.k2so/skills" | grep -v '^\.' | sort | tr '\n' ',' | sed 's/,$//')"
echo "  filesystem .k2so/skills/ basenames: $FS_NAMES"

# Extract daemon names, sort, normalize.
DAEMON_NAMES="$(echo "$AGENTS_RESP" | python3 -c "
import json, sys
data = json.load(sys.stdin)
names = sorted(item.get('name', '') for item in data if not item.get('name', '').startswith('.'))
print(','.join(names))
")"
echo "  /cli/agents/list names: $DAEMON_NAMES"

if [ "$FS_NAMES" != "$DAEMON_NAMES" ]; then
    echo "FAIL: skill name sets disagree" >&2
    echo "  filesystem: $FS_NAMES" >&2
    echo "  daemon:     $DAEMON_NAMES" >&2
    exit 1
fi

# Expected three skills, all present.
EXPECTED="alpha,beta,gamma"
if [ "$FS_NAMES" != "$EXPECTED" ]; then
    echo "FAIL: expected '$EXPECTED', got '$FS_NAMES'" >&2
    exit 1
fi

# Also verify the daemon answer carries the role field we seeded — pins
# that the post-2.5b reader actually parses the frontmatter (not just
# enumerates dirs).
ROLE_CHECK="$(echo "$AGENTS_RESP" | python3 -c "
import json, sys
data = json.load(sys.stdin)
by_name = {item['name']: item.get('role', '') for item in data}
assert by_name.get('alpha') == 'alpha role', f\"alpha role: {by_name.get('alpha')!r}\"
assert by_name.get('beta') == 'beta role', f\"beta role: {by_name.get('beta')!r}\"
assert by_name.get('gamma') == 'gamma role', f\"gamma role: {by_name.get('gamma')!r}\"
print('ok')
")"
if [ "$ROLE_CHECK" != "ok" ]; then
    echo "FAIL: role parity check failed: $ROLE_CHECK" >&2
    exit 1
fi

echo "OK: Tauri (filesystem) and daemon (/cli/agents/list) agree on 3 skills (alpha, beta, gamma)"

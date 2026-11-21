#!/usr/bin/env bash
# Agents reached for their harness's own scheduler (cron jobs, /loop,
# timers) because nothing they always read named K2's reminders. This test
# pins the teaching so a regen or a help rewrite can't drop it:
#   1. the k2-cli skill's "When the person asks → use" table, every row,
#      above `## Who am I`, and the line after it, in the skill's golden
#      (tests/golden/permission_off/daemon/teaching/k2-cli-skill.golden: what
#      a workspace gets; the daemon's permission_off_golden test holds the
#      generator to it, so a re-bless without the table fails here);
#   2. the reminders paragraph at the end of the always-on Tooling block in
#      the AGENTS.md golden (same directory, agents-md.golden);
#   3. the REMINDERS AND SCHEDULES block of `k2 --help` (_daily.txt);
#   4. the NOT YOUR HARNESS'S SCHEDULER block of `k2 study heartbeats`.
# The generator itself (the table, the description's trigger words and the
# always-on Tooling paragraph) is pinned by the k2-core test
# tooling_and_k2_cli_skill_send_reminders_to_k2 (skill_regen.rs).
#
# Run with: bash tests/cli/reminders_first.sh [root]   (default: this repo)
# bash-only: reads golden and text files; never runs the CLI.
set -euo pipefail
export LC_ALL=C

ROOT="${1:-$(cd "$(dirname "$0")/../.." && pwd)}"

python3 - "$ROOT" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
fails = []

def read(rel):
    p = root / rel
    if not p.is_file():
        fails.append(f"{rel}: missing")
        return ""
    return p.read_text(encoding="utf-8")

TABLE = [
    "## When the person asks → use\n| When | Use |\n|---|---|\n",
    '| "Remind me", follow up later, at a time | `k2 remind "…" --at 15:00` (or `--in 45m`, `--on 2026-10-30`) |\n',
    '| Repeat on a schedule ("every morning") | `k2 heartbeat schedule add --name <n> --daily --time 09:00 --instructions "…"` |\n',
    "| You need a secret from them | `k2 thread secret <addr> --name NAME` |\n",
    '| You need a decision from them | `k2 thread ask <addr> "…" --option "…" --option "…"` (a card) |\n',
    "| Send another agent a file | `k2 msg <handle> --inbox-wake <path>` |\n",
    '| Tell the person something | `k2 thread <addr> "…"` |\n',
    "Never your harness's own scheduler (cron jobs, /loop, timers): it dies with\n",
]

def table_on_top(label, text):
    top = text.find("## Who am I")
    if top < 0:
        fails.append(f"{label}: no `## Who am I` section")
        return
    for row in TABLE:
        if row not in text[:top]:
            fails.append(f"{label}: lacks above `## Who am I`: {row!r}")

table_on_top("k2-cli-skill.golden", read("tests/golden/permission_off/daemon/teaching/k2-cli-skill.golden"))

TOOLING = ("\n\nReminders, follow-ups and schedules: `k2 remind \"…\" --at 15:00` (once; or `--in 45m`) "
           "and `k2 heartbeat schedule add` (repeating). Never your harness's own scheduler (cron jobs, "
           "/loop, timers): those die with the session and your human can't see them. K2's survive "
           "restarts and show in K2.\n")
if not read("tests/golden/permission_off/daemon/teaching/agents-md.golden").endswith(TOOLING):
    fails.append(f"agents-md.golden: does not end with the Tooling reminders paragraph {TOOLING!r}")

daily = read("cli/text/help/_daily.txt")
for line in [
    "REMINDERS AND SCHEDULES (K2's, never your harness's cron, /loop or timers)\n",
    '  remind "text" --at 15:00              Once (or --in 45m, --on 2026-10-30); survives restarts\n',
    "  heartbeat schedule add --name <n> ... Repeating (--daily --time 09:00 ...): `k2 heartbeat --help`\n",
]:
    if line not in daily:
        fails.append(f"_daily.txt: lacks {line!r}")

study = read("cli/text/study/heartbeats.txt")
for line in [
    "NOT YOUR HARNESS'S SCHEDULER\n",
    "  Use these, never your harness's own scheduler (cron jobs, /loop, timers).\n",
    "  Those die with the session and your human can't see them. K2's reminders\n",
]:
    if line not in study:
        fails.append(f"heartbeats.txt: lacks {line!r}")

for f in fails:
    print("  FAIL: " + f, file=sys.stderr)
if fails:
    print(f"reminders_first: {len(fails)} failed", file=sys.stderr)
    sys.exit(1)
print("reminders_first: PASS (skill table and Tooling paragraph in the goldens, k2 --help block, study heartbeats block)")
PY

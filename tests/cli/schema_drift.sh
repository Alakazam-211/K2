#!/usr/bin/env bash
# Every top-level command word the router takes has a `k2 --schema` entry,
# or is named in cli/src/schema-head.sh as left out on purpose.
#
# The manifest is hand-written, so a new command could ship with no entry
# and nothing would say so. This reads the words from the router's
# top-level `case` in cli/src/router.sh (an arm like `doc|docs)` is one
# command: covered when any of its words is), the entries from
# `k2 --schema`, and the left-out list from schema-head.sh
# (BEGIN_SCHEMA_LEFT_OUT ... END_SCHEMA_LEFT_OUT). It fails on a word with
# neither, on a left-out or pending word that has an entry after all, and
# on a listed word the router no longer takes.
#
# No daemon: --schema needs none. HOME is sandboxed. Requires python3; a
# missing python3 is a FAIL, not a skip.
#
# SCHEMA_DRIFT_LIST=1 also prints what it read, one per line, before the
# verdict: `arm <words joined by |>`, `left-out <word> <reason>` and
# `alias <word> <target>`. The Rust CLI's route table test
# (crates/k5-cli/tests/route_table.rs) reads the router with its own port
# of this parser and checks the two read the same.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
ROUTER="$PROJECT_ROOT/cli/src/router.sh"
HEAD_PART="$PROJECT_ROOT/cli/src/schema-head.sh"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
[ -f "$ROUTER" ] || { echo "FAIL: $ROUTER not found" >&2; exit 1; }
[ -f "$HEAD_PART" ] || { echo "FAIL: $HEAD_PART not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-schema-drift-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
mkdir -p "$HOME"

"$K2_CLI" --schema >"$WORK/schema.json"

python3 - "$ROUTER" "$HEAD_PART" "$WORK/schema.json" <<'PY'
import json, os, re, sys

router_path, head_path, schema_path = sys.argv[1:4]

# Commands whose entries are still being written. This list may only
# shrink: a word leaves it when its entries land, and nothing new joins it
# (a new command gets entries, or a reason on the left-out list).
PENDING: set = set()

# Words with an arm of their own that run another command's verb.
ALIASES = {
    "reminders": "remind",          # = k2 remind list
    "help-advanced": "help",        # = k2 help --advanced
    "help-internal": "help",        # = k2 help --internal
    "help-deprecated": "help",      # = k2 help deprecated
}

# The router's top-level case: from `case "${1:-help}" in` to the `esac`
# at column 0. Its arms are the lines indented four spaces.
lines = open(router_path, encoding="utf-8").read().split("\n")
start = next(i for i, l in enumerate(lines) if l.startswith('case "${1:-help}" in'))
end = next(i for i in range(start + 1, len(lines)) if lines[i] == "esac")
arms = []
for l in lines[start + 1:end]:
    m = re.match(r"^    ([^\s#()][^()]*)\)\s*(;;.*)?$", l)
    if m:
        words = [w for w in m.group(1).split("|")]
        if words != ["*"]:
            arms.append(words)
if len(arms) < 80:
    print("FAIL: read only %d router arms from %s (the parser is out of date)" % (len(arms), router_path))
    sys.exit(1)

text = open(head_path, encoding="utf-8").read()
m = re.search(r"^# BEGIN_SCHEMA_LEFT_OUT\b.*?\n(.*?)^# END_SCHEMA_LEFT_OUT", text, re.M | re.S)
if not m:
    print("FAIL: no BEGIN_SCHEMA_LEFT_OUT ... END_SCHEMA_LEFT_OUT block in %s" % head_path)
    sys.exit(1)
left_out = {}
for l in m.group(1).split("\n"):
    if not l.strip():
        continue
    row = re.match(r"^#\s+(\S+)\s+(retired|stub|internal|person-only|beta)\s*$", l)
    if not row:
        print("FAIL: left-out line is not `#   <word>  <reason>`: %r" % l)
        sys.exit(1)
    left_out[row.group(1)] = row.group(2)

doc = json.load(open(schema_path, encoding="utf-8"))
heads = {c["name"].split()[0] for c in doc["commands"]}

if os.environ.get("SCHEMA_DRIFT_LIST") == "1":
    for arm in arms:
        print("arm " + "|".join(arm))
    for w, why in left_out.items():
        print("left-out %s %s" % (w, why))
    for alias, target in ALIASES.items():
        print("alias %s %s" % (alias, target))

problems = []
router_words = {w for arm in arms for w in arm}
for arm in arms:
    group = set(arm) | {ALIASES[w] for w in arm if w in ALIASES}
    has_entry = bool(group & heads)
    listed = group & (set(left_out) | PENDING)
    if has_entry and listed:
        problems.append("%s has a schema entry but is listed as %s" % (
            "|".join(arm), ", ".join(sorted(listed))))
    if not has_entry and not listed:
        problems.append("%s has no `k2 --schema` entry and no reason in cli/src/schema-head.sh (BEGIN_SCHEMA_LEFT_OUT)" % "|".join(arm))
for w in sorted(set(left_out) | PENDING):
    if w not in router_words:
        problems.append("%s is listed but the router has no such word" % w)
for alias, target in ALIASES.items():
    if alias not in router_words or target not in router_words:
        problems.append("alias %s -> %s: the router lacks one of them" % (alias, target))

if problems:
    for p in problems:
        print("FAIL: " + p)
    sys.exit(1)
print("PASS: %d router commands: %d with entries, %d left out on purpose, %d pending (%s)" % (
    len(arms),
    sum(1 for a in arms if (set(a) | {ALIASES[w] for w in a if w in ALIASES}) & heads),
    len(left_out), len(PENDING), ", ".join(sorted(PENDING))))
PY

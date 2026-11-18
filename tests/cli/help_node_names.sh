#!/usr/bin/env bash
# The CLI's help, study and comment text never names one of our own
# machines. Examples use generic names: mac-mini-1 (a Mac), linux-1 and
# linux-2 (Linux), win-1 (Windows), arch-1 (Arch).
#
# The patterns are shapes, not a list of hosts, so this file names no
# machine itself:
#   k2n<N>        a K2 compute node number (k2n<N>-<suffix> included)
#   <x>-vin       the -vin host suffix
#   k2-<os>       a K2 host named after its OS
# A real name that fits none of these: add a shape here, never the name.
#
# It scans cli/src/*.sh (the parts) and cli/k2 (the built file), every
# line, and fails naming file, line and the match. Before the real scan
# it runs on a scratch tree with planted names and fails unless it
# catches exactly those lines.
#
# Run with: bash tests/cli/help_node_names.sh
# bash-only: reads cli/src/*.sh and the built cli/k2 as text; never runs them.
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

# scan <root>: one "file:line: names a machine (match): text" per hit.
scan() {
    python3 - "$1" <<'PY'
import glob, os, re, sys

root = sys.argv[1]
shapes = re.compile(
    r"\bk2n[0-9]+"
    r"|[a-z0-9]-vin\b"
    r"|\bk2-(?:arch|win|mac|macos|linux|ubuntu|debian|fedora)\b",
    re.IGNORECASE,
)
files = sorted(glob.glob(os.path.join(root, "cli", "src", "*.sh")))
files.append(os.path.join(root, "cli", "k2"))
for path in files:
    rel = os.path.relpath(path, root)
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            m = shapes.search(line)
            if m:
                print("%s:%d: names a machine (%s): %s" % (rel, n, m.group(0), line.strip()[:160]))
PY
}

# Self-check: planted names must be caught, generic ones must not.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/cli/src"
cat >"$tmp/cli/src/planted.sh" <<'EOF'
  k2 compute run k2n99 -- make test
  placed on build7-vin (auto)
  k2 compute allow-nodes linux-1,arch-1
  # a comment about K2-WIN-TEST
  k2 compute run mac-mini-1 -- xcodebuild; win-1; k2-node; k2-daemon; vintage
EOF
printf '%s\n' 'the built file: K2N12-M4 in capitals' 'k2-core and linux-2 are fine' >"$tmp/cli/k2"
want='cli/src/planted.sh:1: names a machine (k2n99): k2 compute run k2n99 -- make test
cli/src/planted.sh:2: names a machine (7-vin): placed on build7-vin (auto)
cli/src/planted.sh:4: names a machine (K2-WIN): # a comment about K2-WIN-TEST
cli/k2:1: names a machine (K2N12): the built file: K2N12-M4 in capitals'
got="$(scan "$tmp")"
if [ "$got" != "$want" ]; then
    echo "FAIL: help_node_names self-check: the scan missed a planted name or flagged a generic one" >&2
    diff <(printf '%s\n' "$want") <(printf '%s\n' "$got") >&2 || true
    exit 1
fi
echo "  PASS: self-check catches the planted names and only those"

hits="$(scan "$ROOT")"
if [ -n "$hits" ]; then
    printf '%s\n' "$hits" >&2
    echo "FAIL: the CLI text names one of our machines; use a generic name (mac-mini-1, linux-1, win-1, arch-1)" >&2
    exit 1
fi
echo "  PASS: cli/src/*.sh and cli/k2 name no machine"

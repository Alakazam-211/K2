#!/usr/bin/env bash
# bash-only: reads cli/src as text; it never runs the CLI.
# A POST sent with cli_send (cli_request_post, cli_post_*) whose answer is
# piped into embedded Python: when the send may have run and isn't sent
# again, _cli_send_unanswered prints the one "may have been applied" line,
# writes $_K2_NOANSWER_MARK on stdout when the caller set _K2_PIPE_MARK=1,
# and ends the command. The reader must
# stay quiet on exactly that mark, so the command never prints a second
# line (a reader that ran before the command had ended used to add a JSON
# parse error, seen on macOS). Other empty or bad input keeps the reader's
# own message: the transport doesn't speak for every failure yet.
#
# Checks: the mark in cell-socket.sh is the one the readers compare with;
# _cli_send_unanswered writes it; every such reader compares its first
# stdin read with it before parsing. Fails naming file and line, and proves
# it bites on a scratch copy.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

scan() {
    python3 -I - "$1" <<'PY'
import glob, os, re, sys
src = sys.argv[1]
bad, found = [], 0
cs = open(os.path.join(src, "cell-socket.sh"), encoding="utf-8").read()
m = re.search(r"^_K2_NOANSWER_MARK=\$'\\036k2:not-answered\\036'$", cs, re.M)
if not m:
    bad.append("cell-socket.sh: _K2_NOANSWER_MARK is not $'\\036k2:not-answered\\036'")
fn = re.search(r"^_cli_send_unanswered\(\) \{\n(.*?)^\}", cs, re.M | re.S)
if not fn or 'if [ "${_K2_PIPE_MARK:-0}" = 1 ]; then printf \'%s\' "$_K2_NOANSWER_MARK"; fi' not in fn.group(1):
    bad.append("cell-socket.sh: _cli_send_unanswered doesn't write $_K2_NOANSWER_MARK")
want = "if _in == '\\x1ek2:not-answered\\x1e':"
for path in sorted(glob.glob(os.path.join(src, "*.sh"))):
    lines = open(path, encoding="utf-8").read().split("\n")
    name = os.path.basename(path)
    i = 0
    while i < len(lines):
        line = lines[i]
        if re.search(r"\bcli_send (cli_request_post|cli_post_\w+)\b", line) and not line.lstrip().startswith("#"):
            j, joined = i, line
            while joined.rstrip().endswith("\\") and j + 1 < len(lines):
                j += 1
                joined += "\n" + lines[j]
            if re.search(r"\|[^|]*\bpython3\b", joined):
                found += 1
                if not re.search(r"\b_K2_PIPE_MARK=1 cli_send\b", line):
                    bad.append("%s:%d: a POST piped into a reader doesn't set _K2_PIPE_MARK=1" % (name, i + 1))
                ok = False
                for k in range(j, min(j + 16, len(lines))):
                    if "sys.stdin" in lines[k]:
                        ok = (re.match(r"\s*_in = sys\.stdin\.read\(\)\s*$", lines[k]) is not None
                              and lines[k + 1].strip() == want
                              and lines[k + 2].strip().startswith("sys.exit(1)"))
                        break
                if not ok:
                    bad.append("%s:%d: a POST reader of cli_send doesn't stay quiet on the no-answer mark" % (name, i + 1))
            i = j + 1
            continue
        i += 1
print(found)
for b in bad:
    print(b, file=sys.stderr)
sys.exit(1 if bad else 0)
PY
}

T="$(mktemp -d -t k2-post-readers-XXXXXX)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/src"
cp "$ROOT"/cli/src/*.sh "$T/src/"
python3 -I - "$T/src/remind.sh" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
needle = "    if _in == '\\x1ek2:not-answered\\x1e':\n        sys.exit(1)"
assert needle in s, "no mark check to remove in remind.sh"
open(p, "w").write(s.replace(needle, "    if False:\n        pass", 1))
PY
if scan "$T/src" >/dev/null 2>"$T/err"; then
    echo "FAIL: the check passed on a copy with a mark check removed" >&2
    exit 1
fi
grep -q "remind.sh:" "$T/err" || { echo "FAIL: the bite didn't name remind.sh: $(cat "$T/err")" >&2; exit 1; }

n="$(scan "$ROOT/cli/src")" || { echo "FAIL: see the lines above" >&2; exit 1; }
[ "$n" -ge 7 ] || { echo "FAIL: found only $n POST readers; the scan is broken" >&2; exit 1; }
echo "cli_send_post_readers_quiet: $n POST readers checked, each quiet on the no-answer mark; the bite failed as it must"

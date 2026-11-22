#!/usr/bin/env bash
# What a brand's k5 binary prints, one file per command line, for the
# output judges (scripts/brand/output-judges.py).
#
#   bash scripts/brand/collect-outputs.sh <k5-cli binary> <brand folder> <out folder>
#
# Runs the binary under the brand's command name (a link named after it,
# so argv[0] is real) over every help and study form it answers: the
# forms of crates/k5-cli/src/help_native.txt and src/help_captured.txt,
# `study <topic>` for every cli/text/study page, and the bare command,
# --help, help, study, --schema and --version. Each form's stdout and
# stderr go to <out>/<form>.out and .err (the words joined by `+`).
#
# A scratch machine: a scratch HOME and K2 home with an owner token and a
# port on disk, a stub daemon on that port that answers every request with
# 200 and `{}`, stdin closed, and a PATH with no python3. A form the
# binary hands to the shell CLI then stops with its no-Python line (exit
# 69) and runs nothing: its output is not the binary's, so it is left out
# of the corpus and listed in <out>/../fell-through.txt (a partner build
# comes only from a tag where none falls through:
# scripts/brand/ceiling-zero-gate.sh).
#
# Runs the CLI: on K2 Compute only, never on a developer's machine.
set -euo pipefail

[ $# = 3 ] || { echo "usage: collect-outputs.sh <k5-cli binary> <brand folder> <out folder>" >&2; exit 2; }
BIN="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
BRAND="$2"
OUT="$3"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
[ -x "$BIN" ] || { echo "collect-outputs: no binary at $BIN" >&2; exit 2; }
TIMEOUT="$(command -v timeout)" || { echo "collect-outputs: needs timeout (GNU coreutils)" >&2; exit 2; }

name="$(python3 - "$BRAND/brand.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["cli"]["name"])
PY
)"
[[ "$name" =~ ^[a-z][a-z0-9]{1,11}$ ]] || { echo "collect-outputs: the brand's command $name is not a command name" >&2; exit 2; }

S="$(mktemp -d)"
stub_pid=""
cleanup() { [ -n "$stub_pid" ] && kill "$stub_pid" 2>/dev/null; rm -rf "$S"; }
trap cleanup EXIT
mkdir -p "$S/home/.k2" "$S/proj/.k2" "$S/tmp" "$S/bin" "$OUT"
ln -s "$BIN" "$S/bin/$name"

# The stub daemon: 200 and `{}` for every request; it writes its port.
python3 - "$S/port" <<'PY' &
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def answer(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        body = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = answer
    def log_message(self, *a):
        pass
s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1] + ".tmp", "w") as f:
    f.write(str(s.server_address[1]))
import os
os.rename(sys.argv[1] + ".tmp", sys.argv[1])
s.serve_forever()
PY
stub_pid=$!
for _ in $(seq 1 100); do [ -s "$S/port" ] && break; sleep 0.1; done
[ -s "$S/port" ] || { echo "collect-outputs: the stub daemon did not start" >&2; exit 1; }
printf 'owner-tok-7171\n' > "$S/home/.k2/heartbeat.token"
printf '%s\n' "$(cat "$S/port")" > "$S/home/.k2/heartbeat.port"

# The forms, one per line, words separated by single spaces.
forms="$S/forms"
{
    printf '%s\n' "" "--help" "help" "study" "--schema" "--version"
    grep -v '^#' "$ROOT/crates/k5-cli/src/help_native.txt" | grep -v '^$' | cut -d'|' -f1
    grep -E '^[FCMZ]	' "$ROOT/crates/k5-cli/src/help_captured.txt" | cut -f2
    for f in "$ROOT"/cli/text/study/*.txt; do
        t="$(basename "$f" .txt)"
        case "$t" in _*) ;; *) printf 'study %s\n' "$t" ;; esac
    done
} | awk '!seen[$0]++' > "$forms"

fell="$(dirname "$OUT")/fell-through.txt"
: > "$fell"
n=0
while IFS= read -r form; do
    file="$(printf '%s' "${form:-_bare}" | tr ' /' '+_' | tr -c 'A-Za-z0-9._+=-' '_')"
    read -r -a words <<< "$form"
    rc=0
    (cd "$S/proj" && env -i PATH="$S/bin" HOME="$S/home" TMPDIR="$S/tmp" LANG=C.UTF-8 \
        "$TIMEOUT" 20 "$S/bin/$name" ${words[@]+"${words[@]}"} \
        < /dev/null > "$OUT/$file.out" 2> "$OUT/$file.err") || rc=$?
    if [ "$rc" = 69 ]; then
        printf '%s\n' "$form" >> "$fell"
        rm -f "$OUT/$file.out" "$OUT/$file.err"
    elif [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
        echo "collect-outputs: $name $form ran past 20 s" >&2
        exit 1
    fi
    n=$((n + 1))
done < "$forms"
kept="$(find "$OUT" -name '*.out' | wc -l | tr -d ' ')"
echo "collect-outputs: $n forms, $kept printed by the binary, $(wc -l < "$fell" | tr -d ' ') fell through (listed in $fell)"
[ "$kept" -gt 1000 ] || { echo "collect-outputs: only $kept forms answered by the binary" >&2; exit 1; }

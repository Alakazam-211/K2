#!/bin/sh
# Self-test of tools/check-private.sh: where it reads its patterns from (and in which order),
# comments and blank lines, failing closed with none, the allowlist, a clean tree and a planted
# match. It uses a made-up pattern of its own (`plantedhost[0-9]+`), never the real ones, in
# throwaway trees (no git: the script's find mode). POSIX sh. Exit 0 when every case holds.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
FAIL=0
mk() { # $1: a tree name; a copy of the script and an empty allowlist, and one clean file
  mkdir -p "$T/$1/tools"
  cp "$HERE/check-private.sh" "$T/$1/tools/check-private.sh"
  printf '# allowlist\n' > "$T/$1/tools/private-allow.txt"
  printf 'nothing to see here\n' > "$T/$1/readme.txt"
}
# run NAME WANT [VAR=VALUE...]: the script on tree NAME, in an environment with none of the
# pattern sources but those given, and a HOME of its own
run() {
  name=$1; want=$2; shift 2
  out=$(env -u MPC_PRIVATE_PATTERNS -u MPC_PRIVATE_PATTERNS_FILE HOME="$T/home-$name" "$@" \
    sh "$T/$name/tools/check-private.sh" "$T/$name" 2>&1)
  rc=$?
  if [ "$rc" -eq "$want" ]; then echo "ok   $name: exit $rc"; else echo "FAIL $name: exit $rc, want $want: $out"; FAIL=1; fi
}
expect_out() { # $1: case name; $2: a fixed string the last output must hold
  case $out in *"$2"*) ;; *) echo "FAIL $1: output lacks \"$2\": $out"; FAIL=1 ;; esac
}
refuse_out() { # $1: case name; $2: a fixed string the last output must not hold
  case $out in *"$2"*) echo "FAIL $1: output holds \"$2\": $out"; FAIL=1 ;; esac
}
PAT='plantedhost[0-9]+'

# no patterns anywhere: exit 2, naming the three places, and nothing matched
mk none; printf 'see plantedhost7\n' > "$T/none/notes.md"
run none 2
expect_out none MPC_PRIVATE_PATTERNS; expect_out none MPC_PRIVATE_PATTERNS_FILE
expect_out none .config/k2-mpc/private-patterns.txt; refuse_out none plantedhost7
# an empty variable counts as unset; only comments and blank lines: still no patterns
run none 2 MPC_PRIVATE_PATTERNS=
printf '# a comment\n\n   \n  # another\n' > "$T/comments.txt"
run none 2 MPC_PRIVATE_PATTERNS_FILE="$T/comments.txt"
# a file named but missing: no patterns (the config file is not tried after it)
mkdir -p "$T/home-none/.config/k2-mpc"; printf '%s\n' "$PAT" > "$T/home-none/.config/k2-mpc/private-patterns.txt"
run none 2 MPC_PRIVATE_PATTERNS_FILE="$T/missing.txt"
rm -rf "$T/home-none"

# a clean tree: exit 0 from each source
mk clean
run clean 0 MPC_PRIVATE_PATTERNS="$PAT"
expect_out clean "check-private: clean"; refuse_out clean "$PAT"
printf '# hosts\n%s   # trailing comment\n\n' "$PAT" > "$T/pats.txt"
run clean 0 MPC_PRIVATE_PATTERNS_FILE="$T/pats.txt"
mkdir -p "$T/home-clean/.config/k2-mpc"; cp "$T/pats.txt" "$T/home-clean/.config/k2-mpc/private-patterns.txt"
run clean 0

# a planted match: exit 1 from each source, the line printed, the pattern never
mk planted; printf 'line one\nbuilt on PlantedHost42 last night\n' > "$T/planted/notes.md"
run planted 1 MPC_PRIVATE_PATTERNS="$PAT"
expect_out planted "notes.md:2:built on PlantedHost42"; refuse_out planted "$PAT"
run planted 1 MPC_PRIVATE_PATTERNS_FILE="$T/pats.txt"
mkdir -p "$T/home-planted/.config/k2-mpc"; cp "$T/pats.txt" "$T/home-planted/.config/k2-mpc/private-patterns.txt"
run planted 1
# several patterns, inline, one per line, with a comment line: the second one matches
run planted 1 MPC_PRIVATE_PATTERNS="$(printf '# two\nnomatch[0-9]\n%s\n' "$PAT")"
# the order: the variable wins over the file, the file over the config file
printf 'nomatch[0-9]\n' > "$T/other.txt"
run planted 0 MPC_PRIVATE_PATTERNS='nomatch[0-9]' MPC_PRIVATE_PATTERNS_FILE="$T/pats.txt"
run planted 1 MPC_PRIVATE_PATTERNS="$PAT" MPC_PRIVATE_PATTERNS_FILE="$T/other.txt"
run planted 0 MPC_PRIVATE_PATTERNS_FILE="$T/other.txt"

# the allowlist: PATH:TEXT keeps a matching line that holds TEXT, and only that one
mk allowed; printf 'a neutral plantedhost1 mention\nand plantedhost2 here\n' > "$T/allowed/notes.md"
printf 'notes.md:neutral plantedhost1\n' >> "$T/allowed/tools/private-allow.txt"
run allowed 1 MPC_PRIVATE_PATTERNS="$PAT"
expect_out allowed "notes.md:2:and plantedhost2"; refuse_out allowed "notes.md:1:"
printf 'notes.md\n' >> "$T/allowed/tools/private-allow.txt"
run allowed 0 MPC_PRIVATE_PATTERNS="$PAT"
# the allowlist itself is skipped (it may quote a match)
run allowed 0 MPC_PRIVATE_PATTERNS="$PAT"

if [ "$FAIL" -eq 0 ]; then echo "test-check-private: ok"; else echo "test-check-private: FAILED"; exit 1; fi

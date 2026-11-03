#!/bin/sh
# check-windows-names.sh's self-test: each planted name fails with its reason, and each near miss
# passes. Each case is a small git repo in a temporary folder whose index is written directly
# (`git update-index --cacheinfo`), so a name this disk can't hold, such as a case collision on a
# case-insensitive one, is still tested. Then the non-git mode, on real files. POSIX sh and git
# only; no cargo, no node.
set -eu
LC_ALL=C; export LC_ALL
HERE=$(cd "$(dirname "$0")" && pwd)
CHECK=$HERE/check-windows-names.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE 2>/dev/null || true
fails=0; n=0; relaxed=0

repo() { # $1: the repo's folder; the rest: the names its index holds
  d=$1; shift
  mkdir "$d"
  git -C "$d" init -q .
  git -C "$d" config core.ignorecase false
  blob=$(printf 'x\n' | git -C "$d" hash-object -w --stdin)
  for p in "$@"; do
    if ! git -C "$d" update-index --add --cacheinfo 100644 "$blob" "$p" 2>/dev/null; then
      # git's own guard on this platform; the name is what's under test
      git -C "$d" -c core.protectNTFS=false update-index --add --cacheinfo 100644 "$blob" "$p"
      relaxed=$((relaxed + 1))
    fi
  done
}
ok() { echo "ok   $1"; }
no() { echo "FAIL $1: $2"; [ -f "$3" ] && sed 's/^/     /' "$3"; fails=$((fails + 1)); }
bad() { # label, the text the output must hold, the names
  label=$1; why=$2; shift 2; n=$((n + 1))
  d=$TMP/r$n; repo "$d" "$@"
  got=0; sh "$CHECK" "$d" > "$d.out" 2>&1 || got=$?
  if [ "$got" -ne 1 ]; then no "$label" "exit $got, want 1" "$d.out"
  elif ! grep -qF -- "$why" "$d.out"; then no "$label" "no \"$why\" in the output" "$d.out"
  else ok "$label fails: $why"; fi
}
good() { # a name that must pass
  n=$((n + 1))
  d=$TMP/r$n; repo "$d" "$1"
  got=0; sh "$CHECK" "$d" > "$d.out" 2>&1 || got=$?
  if [ "$got" -ne 0 ]; then no "$1" "exit $got, want 0" "$d.out"
  elif ! grep -qxF "check-windows-names: clean (1 files checked)" "$d.out"; then no "$1" "not the clean line" "$d.out"
  else ok "$1 passes"; fi
}

bad 'nul.binary.json'  'nul.binary.json: reserved name nul' 'nul.binary.json'
bad 'NUL'              'NUL: reserved name nul'             'NUL'
bad 'con/x.txt'        'con: reserved name con'             'con/x.txt'
bad 'lpt0.log'         'lpt0.log: reserved name lpt0'       'lpt0.log'
bad 'com9.a.b'         'com9.a.b: reserved name com9'       'com9.a.b'
bad 'NUL.tar.gz'       'NUL.tar.gz: reserved name nul'      'NUL.tar.gz'
bad 'a stem with a trailing space' 'reserved name aux'      'aux .txt'
bad 'a<b'  'a<b: reserved character <'   'a<b'
bad 'a>b'  'a>b: reserved character >'   'a>b'
bad 'a:b'  'a:b: reserved character :'   'a:b'
bad 'a"b'  'a"b: reserved character "'   'a"b'
bad 'a\b'  'a\b: reserved character \'   'a\b'
bad 'a|b'  'a|b: reserved character |'   'a|b'
bad 'a?b'  'a?b: reserved character ?'   'a?b'
bad 'a*b'  'a*b: reserved character *'   'a*b'
bad 'a control character' 'control character 0x07' "$(printf 'a\007b')"
bad 'a newline in a name' 'control character 0x01' "$(printf 'a\nb')"
bad 'x.'     'x.: ends in a dot'         'x.'
bad 'x '     'x : ends in a space'       'x '
bad 'dir./y' 'dir.: ends in a dot'       'dir./y'
bad 'a/X + A/Y'     'a: case collision with A'     'a/X' 'A/Y'
bad 'Foo + foo/bar' 'foo: case collision with Foo' 'Foo' 'foo/bar'
# the superscripts, as bytes: this file stays ASCII
S1=$(printf '\302\271'); S2=$(printf '\302\262'); S3=$(printf '\302\263')
bad 'conin$'         'conin$: reserved name conin$'           'conin$'
bad 'CONOUT$.log'    'CONOUT$.log: reserved name conout$'     'CONOUT$.log'
bad 'sub/CoNiN$.txt' 'sub/CoNiN$.txt: reserved name conin$'   'sub/CoNiN$.txt'
bad 'COM superscript 1'       "COM$S1: reserved name com$S1"         "COM$S1"
bad 'com superscript 2 .txt'  "com$S2.txt: reserved name com$S2"     "com$S2.txt"
bad 'LPT superscript 3 .a.b'  "LPT$S3.a.b: reserved name lpt$S3"     "LPT$S3.a.b"
# lengths, in bytes
fill() { printf "%0$1d" 0 | tr 0 a; }
bad 'a path of 201 bytes' 'path is 201 bytes; the limit is 200' "$(fill 201)"
bad 'a deep path of 201 bytes' 'path is 201 bytes; the limit is 200' "$(fill 100)/$(fill 100)"
TWO=$(printf '\303\251')
MANY=$(printf '%0101d' 0 | sed "s/0/$TWO/g")
bad '101 two-byte characters: 202 bytes' 'path is 202 bytes; the limit is 200' "$MANY"
good 'conin'
good 'conout.txt'
good 'xconin$'
good "$(printf 'com\342\201\264.txt')"
good "$(printf 'lpt\342\201\260')"
good "$(fill 200)"
good "$(fill 99)/$(fill 100)"
good 'nul-byte.binary.json'
good 'console.log'
good 'auxiliary.c'
good 'com10.txt'
good 'lpt.txt'
good 'prn_x'
good 'conf/x'
good '.hidden'

# usage
n=$((n + 1)); got=0; sh "$CHECK" "$TMP/none" > "$TMP/u.out" 2>&1 || got=$?
if [ "$got" -eq 2 ]; then ok "no such directory is exit 2"; else no "no such directory" "exit $got, want 2" "$TMP/u.out"; fi
n=$((n + 1)); got=0; sh "$CHECK" a b > "$TMP/u.out" 2>&1 || got=$?
if [ "$got" -eq 2 ]; then ok "two arguments are exit 2"; else no "two arguments" "exit $got, want 2" "$TMP/u.out"; fi
n=$((n + 1)); mkdir "$TMP/empty"; got=0; sh "$CHECK" "$TMP/empty" > "$TMP/u.out" 2>&1 || got=$?
if [ "$got" -eq 2 ]; then ok "a tree with no files is exit 2"; else no "a tree with no files" "exit $got, want 2" "$TMP/u.out"; fi

# the non-git mode: real files, as in a node's unpacked bundle
plain() { # label, want, the text the output must hold, then the files
  label=$1; want=$2; why=$3; shift 3; n=$((n + 1))
  d=$TMP/f$n; mkdir "$d"
  for p in "$@"; do mkdir -p "$d/$(dirname "$p")"; printf 'x\n' > "$d/$p"; done
  got=0; sh "$CHECK" "$d" > "$d.out" 2>&1 || got=$?
  if [ "$got" -ne "$want" ]; then no "$label" "exit $got, want $want" "$d.out"
  elif ! grep -qF -- "$why" "$d.out"; then no "$label" "no \"$why\" in the output" "$d.out"
  elif ! grep -qxF "check-windows-names: reading names from find" "$d.out"; then no "$label" "not read from find" "$d.out"
  else ok "$label ($why)"; fi
}
plain 'find: a reserved name fails'  1 'nul.txt: reserved name nul' 'nul.txt'
plain 'find: a trailing dot fails'   1 'x.: ends in a dot'          'x.'
plain 'find: a reserved $ name fails' 1 'conin$: reserved name conin$' 'conin$'
plain 'find: 201 bytes fails'        1 'path is 201 bytes; the limit is 200' "$(fill 201)"
plain 'find: 200 bytes passes'       0 'clean (1 files checked)'    "$(fill 200)"
plain 'find: a near miss passes'     0 'clean (1 files checked)'    'console.log'
plain 'find: target/ is left out'    0 'clean (1 files checked)'    'a.txt' 'target/nul.txt'
# a case collision needs a disk that keeps both spellings
mkdir "$TMP/fold"; : > "$TMP/fold/Foo"
if [ -e "$TMP/fold/foo" ]; then
  echo "skip find: a case collision (this disk folds case; the index cases above cover it)"
else
  plain 'find: a case collision fails' 1 'case collision with' 'Foo' 'foo/bar'
fi

[ "$relaxed" -eq 0 ] || echo "note: $relaxed name(s) needed core.protectNTFS=false to enter the index"
if [ "$fails" -ne 0 ]; then
  echo "check-windows-names self-test: $fails of $n case(s) failed"
  exit 1
fi
echo "check-windows-names self-test: ok ($n cases)"

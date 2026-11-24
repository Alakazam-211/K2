#!/usr/bin/env bash
# Tests for scripts/brand/check-no-bash.py on made-up files: a file with no
# copy of the shell CLI passes; the script as text (the daemon's and the
# root helper's include_str!), gzipped (k5's embed), another release's
# copy, a .tar.gz holding it, the script or its alias shim as a file of
# their own and a folder holding one are refused, each naming the file and
# the byte. Bytes only: no cargo, and the CLI never runs.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
C="$HERE/scripts/brand/check-no-bash.py"
CLI="$HERE/cli/k2"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }
check() { rc=0; out="$(python3 "$C" "$@" 2>&1)" || rc=$?; }

# The made-up files: junk with a false gzip magic in it and the k5 version
# mark (which is not the shell's), around each way of holding the script.
python3 - "$T" "$CLI" "$HERE/cli/k2so" <<'PY'
import gzip, io, os, sys, tarfile
t, cli_path, shim_path = sys.argv[1:]
cli = open(cli_path, "rb").read()
shim = open(shim_path, "rb").read()
junk = os.urandom(4096) + b"\x1f\x8b\x08 not a gzip member" + os.urandom(4096)
mark = b"K5_CLI_VERSION=0.46.17\x00"
def put(name, data):
    os.makedirs(os.path.dirname(os.path.join(t, name)), exist_ok=True)
    with open(os.path.join(t, name), "wb") as f:
        f.write(data)
put("clean/k5-cli", junk + mark + junk)
put("clean/k2-daemon", junk + b"#!/bin/bash\necho a script that is not the CLI\n" + junk)
put("plain/k2-daemon", junk + cli + junk)
put("gz/k5-cli", junk + mark + gzip.compress(cli, mtime=0) + junk)
v = cli.split(b'\nK2_CLI_VERSION="', 1)[1].split(b'"', 1)[0]
old = cli.replace(b'\nK2_CLI_VERSION="' + v + b'"', b'\nK2_CLI_VERSION="0.0.1"', 1)
assert old != cli
put("old/k5-cli", junk + gzip.compress(old, mtime=0) + junk)
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:gz") as tar:
    info = tarfile.TarInfo("usr/lib/app/_up_/cli/k2")
    info.size = len(cli)
    tar.addfile(info, io.BytesIO(cli))
put("tgz/bundle.tar.gz", buf.getvalue())
put("pkg/usr/bin/xy", cli)
put("pkg/usr/share/doc/README", b"a package's other file\n")
put("shim/usr/bin/xy2", shim)
PY

check --cli "$CLI" "$T/clean/k5-cli" "$T/clean/k2-daemon"
[ "$rc" = 0 ] && [ "$out" = "check-no-bash: 2 files, no copy of the shell CLI" ] || die "no copy: $rc $out"; ok
check --cli "$CLI" "$T/clean"
[ "$rc" = 0 ] && [ "$out" = "check-no-bash: 2 files, no copy of the shell CLI" ] || die "a clean folder: $rc $out"; ok

check --cli "$CLI" "$T/plain/k2-daemon"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon: at byte 8213: the shell CLI's first two lines, as text"* ]] \
    && [[ "$out" == *"the shell CLI's version line, as text"* ]] && [[ "$out" == *"the shell CLI's update-key line, as text"* ]] \
    && [[ "$out" == *"REFUSED: the build carries the shell CLI"* ]] || die "the script as text: $out"; ok

check --cli "$CLI" "$T/gz/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"k5-cli: at byte 8236: a gzip member that inflates to the shell CLI, byte for byte"* ]] || die "the script gzipped: $out"; ok
check --cli "$CLI" "$T/old/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"a gzip member that inflates to a shell CLI script (another copy)"* ]] || die "another release's copy: $out"; ok
check --cli "$CLI" "$T/tgz/bundle.tar.gz"
[ "$rc" = 1 ] && [[ "$out" == *"bundle.tar.gz: at byte 0: a gzip member that holds the shell CLI's first two lines"* ]] || die "a .tar.gz holding it: $out"; ok

check --cli "$CLI" "$T/pkg"
[ "$rc" = 1 ] && [[ "$out" == *"pkg/usr/bin/xy: at byte 0: the file is a shell CLI script"* ]] \
    && [[ "$out" != *"README"* ]] || die "a package folder with the script under another name: $out"; ok
check --cli "$CLI" "$T/shim"
[ "$rc" = 1 ] && [[ "$out" == *"shim/usr/bin/xy2: at byte 0: the file is the shell CLI's alias shim"* ]] || die "the alias shim: $out"; ok
# One finding is enough: a clean file beside a dirty one still refuses.
check --cli "$CLI" "$T/clean/k5-cli" "$T/gz/k5-cli"
[ "$rc" = 1 ] && [[ "$out" != *"clean/k5-cli"* ]] || die "clean beside dirty: $out"; ok

check --cli "$CLI" "$T/nowhere"
[ "$rc" = 1 ] && [[ "$out" == *"nowhere: can't read it"* ]] || die "a missing file: $out"; ok
check --cli "$T/clean/k5-cli" "$T/clean/k5-cli"
[ "$rc" = 2 ] && [[ "$out" == *"does not start with #!/bin/bash: not the shell CLI"* ]] || die "--cli not the script: $out"; ok
mkdir "$T/empty"
check --cli "$CLI" "$T/empty"
[ "$rc" = 2 ] && [[ "$out" == *"no files to read"* ]] || die "nothing to read: $out"; ok
check "$T/clean/k5-cli"
[ "$rc" = 2 ] && [[ "$out" == usage:* ]] || die "usage: $out"; ok

# --bins: every program the list names must be in the folder, and each is
# checked; one the build left out is refused by name (fail closed), as is
# one that carries the script. The repo's own list names the root helper.
LIST="$HERE/scripts/brand/shipped-bins-linux.txt"
grep -qx 'k2-root-helper unmarked' "$LIST" || die "the shipped list names the root helper, unmarked"; ok
mkdir -p "$T/bins"
for b in $(sed -e 's/#.*//' "$LIST" | awk 'NF { print $1 }'); do cp "$T/clean/k5-cli" "$T/bins/$b"; done
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 0 ] && [[ "$out" == *"expecting 5 programs from $LIST: k2-daemon k2-node k2-app-engine k5-cli k2-root-helper"* ]] \
    && [[ "$out" == *"check-no-bash: 5 files, no copy of the shell CLI"* ]] || die "--bins, all there and clean: $out"; ok
rm "$T/bins/k2-root-helper"
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 1 ] && [[ "$out" == *"bins/k2-root-helper: expected (listed in $LIST) but not there: the build did not make it"* ]] \
    && [[ "$out" == *"REFUSED: a program the brand ships was not built, so it was not checked"* ]] || die "--bins, one missing: $out"; ok
cp "$T/plain/k2-daemon" "$T/bins/k2-root-helper"
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 1 ] && [[ "$out" == *"bins/k2-root-helper: at byte 8213: the shell CLI's first two lines, as text"* ]] || die "--bins, one carries it: $out"; ok
printf '# nothing\n' > "$T/empty.txt"
check --cli "$CLI" --bins "$T/empty.txt" "$T/bins"
[ "$rc" = 2 ] && [[ "$out" == *"names no program"* ]] || die "--bins, an empty list: $out"; ok
check --cli "$CLI" --bins "$LIST"
[ "$rc" = 2 ] && [[ "$out" == usage:* ]] || die "--bins, no folder: $out"; ok

echo "no-bash check tests: $pass passed"

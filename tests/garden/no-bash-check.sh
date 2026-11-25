#!/usr/bin/env bash
# Tests for scripts/brand/check-no-bash.py on made-up files: a file with no
# copy of the shell CLI passes; the script as text (the daemon's
# include_str!), packed by each way the check knows (gzip as k5 packs it,
# raw deflate as the root helper does, zlib, xz), its sha256 or BLAKE3 hex
# alone (the hashes the helper and k5 keep beside a copy), another
# release's copy, a .tar.gz holding it, the script or its alias shim as a
# file of their own and a folder holding one are refused, each naming the
# file and the byte. Bytes only: no cargo, and the CLI never runs.
#
# Most cases use a made-up CLI (the check hashes the script it is given,
# and BLAKE3 in Python takes seconds on the real 6 MB one); two use the
# real cli/k2, so its own markers and hashes are proved too.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
C="$HERE/scripts/brand/check-no-bash.py"
REAL="$HERE/cli/k2"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }
check() { rc=0; out="$(python3 "$C" "$@" 2>&1)" || rc=$?; }
CLI="$T/cli/k2"

# BLAKE3 as k5's build.rs takes it: the reference test vectors (input
# bytes i % 251, BLAKE3's own test_vectors.json), across one chunk, a
# chunk boundary and a tree of chunks.
python3 - "$C" <<'PY' || die "the check's BLAKE3 does not match the reference vectors"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("check_no_bash", sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
want = {
    0: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    1024: "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
    1025: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
    2048: "e776b6028c7cd22a4d0ba182a8bf62205d2ef576467e838ed6f2529b85fba24a",
    3072: "b98cb0ff3623be03326b373de6b9095218513e64f1ee2edd2525c7ad1e5cffd2",
    102400: "bc3e3d41a1146b069abffad3c0d44860cf664390afce4d9661f7902e7943e085",
}
for n, h in want.items():
    got = m.blake3_hex(bytes(i % 251 for i in range(n)))
    assert got == h, f"BLAKE3 of {n} bytes: {got}, want {h}"
assert m.blake3_hex(b"abc") == "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
PY
ok

# The made-up files: junk with a false gzip magic in it and the k5 version
# mark (which is not the shell's), around each way of holding the script.
# The made-up CLI is lines of base64, so packed it is a long near-random
# run as the real one is, and the raw deflate scan reads it.
python3 - "$T" "$REAL" <<'PY'
import base64, gzip, hashlib, io, lzma, os, sys, tarfile, zlib
t, real_path = sys.argv[1:]
lines = [b"#!/bin/bash", b"# A made-up shell CLI for the no-bash check's tests", b"", b'K2_CLI_VERSION="9.8.7"']
lines += [b"# " + base64.b64encode(os.urandom(57)) for _ in range(4000)]
lines += [b'_DAEMON_UPDATE_PUBKEYS="RWmadeup0000"', b"main \"$@\"", b""]
cli = b"\n".join(lines)
shim = b"#!/bin/bash\n# the made-up alias shim\nexec k2 \"$@\"\n"
real = open(real_path, "rb").read()
junk = os.urandom(4096) + b"\x1f\x8b\x08 not a gzip member" + os.urandom(4096)
mark = b"K5_CLI_VERSION=0.46.17\x00"
# Low-entropy bytes around a packed copy, of an odd length so the copy
# starts off a 4096-byte window.
pad = b"\0" * 10007 + b"a program's text " * 600
def put(name, data):
    os.makedirs(os.path.dirname(os.path.join(t, name)), exist_ok=True)
    with open(os.path.join(t, name), "wb") as f:
        f.write(data)
def raw(b):
    z = zlib.compressobj(9, zlib.DEFLATED, -zlib.MAX_WBITS)
    return z.compress(b) + z.flush()
put("cli/k2", cli)
put("cli/k2so", shim)
# Clean: junk, a long run of random bytes (not a stream) and a script
# that is not the CLI.
put("clean/k5-cli", junk + mark + pad + os.urandom(200 * 1024) + pad + junk)
put("clean/k2-daemon", junk + b"#!/bin/bash\necho a script that is not the CLI\n" + junk)
put("plain/k2-daemon", junk + cli + junk)
put("gz/k5-cli", junk + mark + gzip.compress(cli, mtime=0) + junk)
put("raw/k2-root-helper", pad + raw(cli) + pad)
# Right after other near-random bytes, inside one run: found by trying
# every byte of the run, not only where the run starts.
put("rawnext/k2-root-helper", pad + os.urandom(100003) + raw(cli) + pad)
put("zlib/k2-daemon", pad + zlib.compress(cli, 9) + pad)
put("xz/k2-daemon", pad + lzma.compress(cli) + pad)
# The hashes alone, with no copy beside them.
sha = hashlib.sha256(cli).hexdigest().encode()
put("sha/k2-root-helper", pad + b"cliSha256\0" + sha + pad)
put("shaup/k2-root-helper", pad + sha.upper() + pad)
v = cli.split(b'\nK2_CLI_VERSION="', 1)[1].split(b'"', 1)[0]
old = cli.replace(b'\nK2_CLI_VERSION="' + v + b'"', b'\nK2_CLI_VERSION="0.0.1"', 1)
assert old != cli
put("old/k5-cli", junk + gzip.compress(old, mtime=0) + junk)
put("oldraw/k2-root-helper", pad + raw(old) + pad)
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:gz") as tar:
    info = tarfile.TarInfo("usr/lib/app/_up_/cli/k2")
    info.size = len(cli)
    tar.addfile(info, io.BytesIO(cli))
put("tgz/bundle.tar.gz", buf.getvalue())
put("pkg/usr/bin/xy", cli)
put("pkg/usr/share/doc/README", b"a package's other file\n")
put("shim/usr/bin/xy2", shim)
# The real cli/k2: the daemon's plain copy and the root helper's raw one.
put("real/k2-daemon", junk + real + junk)
put("real/k2-root-helper", pad + raw(real) + pad)
put("realclean/k5-cli", junk + mark + junk)
PY
BLAKE="$(python3 - "$C" "$CLI" <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("check_no_bash", sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
print(m.blake3_hex(open(sys.argv[2], "rb").read()))
PY
)"
[[ "$BLAKE" =~ ^[0-9a-f]{64}$ ]] || die "the made-up CLI's BLAKE3: $BLAKE"
mkdir -p "$T/blake"
{ head -c 9000 /dev/zero; printf 'SCRIPT_BLAKE3 %s' "$BLAKE"; head -c 9000 /dev/zero; } > "$T/blake/k5-cli"
# Where a packed copy starts: after the pad (10007 zero bytes and 600
# times the 17 bytes of "a program's text ").
PADLEN=$((10007 + 17 * 600))

check --cli "$CLI" "$T/clean/k5-cli" "$T/clean/k2-daemon"
[ "$rc" = 0 ] && [ "$out" = "check-no-bash: 2 files, no copy of the shell CLI" ] || die "no copy: $rc $out"; ok
check --cli "$CLI" "$T/clean"
[ "$rc" = 0 ] && [ "$out" = "check-no-bash: 2 files, no copy of the shell CLI" ] || die "a clean folder: $rc $out"; ok

check --cli "$CLI" "$T/plain/k2-daemon"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon: at byte 8213: the shell CLI's first two lines, as text"* ]] \
    && [[ "$out" == *"the shell CLI's version line, as text"* ]] && [[ "$out" == *"the shell CLI's update-key line, as text"* ]] \
    && [[ "$out" == *"REFUSED: the build carries the shell CLI"* ]] || die "the script as text: $out"; ok

check --cli "$CLI" "$T/gz/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"k5-cli: at byte 8236: a gzip member that inflates to the shell CLI, byte for byte"* ]] \
    && [[ "$out" != *"raw deflate"* ]] || die "the script gzipped (and its deflate body not named twice): $out"; ok
check --cli "$CLI" "$T/raw/k2-root-helper"
[ "$rc" = 1 ] && [[ "$out" == *"k2-root-helper: at byte $PADLEN: a raw deflate stream that inflates to the shell CLI, byte for byte"* ]] \
    || die "the script raw-deflated (the root helper's embed): $out"; ok
check --cli "$CLI" "$T/rawnext/k2-root-helper"
[ "$rc" = 1 ] && [[ "$out" == *"at byte $((PADLEN + 100003)): a raw deflate stream that inflates to the shell CLI, byte for byte"* ]] \
    || die "the script raw-deflated, right after other random bytes: $out"; ok
check --cli "$CLI" "$T/zlib/k2-daemon"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon: at byte $PADLEN: a zlib stream that inflates to the shell CLI, byte for byte"* ]] \
    && [[ "$out" != *"raw deflate"* ]] || die "the script in zlib: $out"; ok
check --cli "$CLI" "$T/xz/k2-daemon"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon: at byte $PADLEN: an xz stream that inflates to the shell CLI, byte for byte"* ]] \
    || die "the script in xz: $out"; ok

check --cli "$CLI" "$T/sha/k2-root-helper"
[ "$rc" = 1 ] && [[ "$out" == *"k2-root-helper: at byte $((PADLEN + 10)): the shell CLI's sha256 (hex), as text"* ]] \
    && [[ "$out" == *"REFUSED: the build carries the shell CLI"* ]] || die "the script's sha256 alone: $out"; ok
check --cli "$CLI" "$T/shaup/k2-root-helper"
[ "$rc" = 1 ] && [[ "$out" == *"the shell CLI's sha256 (hex, upper case), as text"* ]] || die "the sha256 in upper case: $out"; ok
check --cli "$CLI" "$T/blake/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"k5-cli: at byte 9014: the shell CLI's BLAKE3 (hex), as text"* ]] || die "the script's BLAKE3 alone: $out"; ok

check --cli "$CLI" "$T/old/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"a gzip member that inflates to a shell CLI script (another copy)"* ]] || die "another release's copy: $out"; ok
check --cli "$CLI" "$T/oldraw/k2-root-helper"
[ "$rc" = 1 ] && [[ "$out" == *"a raw deflate stream that inflates to a shell CLI script (another copy)"* ]] \
    || die "another release's copy, raw-deflated: $out"; ok
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

# The real cli/k2: a clean file passes, and its plain copy (the daemon's
# way) and its raw-deflated copy with its sha256 (the root helper's way)
# are refused, in one run.
check --cli "$REAL" "$T/realclean/k5-cli"
[ "$rc" = 0 ] && [ "$out" = "check-no-bash: 1 files, no copy of the shell CLI" ] || die "the real CLI, a clean file: $rc $out"; ok
check --cli "$REAL" "$T/real"
[ "$rc" = 1 ] && [[ "$out" == *"real/k2-daemon: at byte 8213: the shell CLI's first two lines, as text"* ]] \
    && [[ "$out" == *"real/k2-daemon: at byte"*"the shell CLI's update-key line, as text"* ]] \
    && [[ "$out" == *"real/k2-root-helper: at byte $PADLEN: a raw deflate stream that inflates to the shell CLI, byte for byte"* ]] \
    || die "the real CLI, as text and raw-deflated: $out"; ok

check --cli "$CLI" "$T/nowhere"
[ "$rc" = 1 ] && [[ "$out" == *"nowhere: can't read it"* ]] \
    && [[ "$out" == *"REFUSED: a file could not be read, so the build was not checked"* ]] || die "a missing file: $out"; ok
check --cli "$CLI" "$T/nowhere" "$T/gz/k5-cli"
[ "$rc" = 1 ] && [[ "$out" == *"REFUSED: the build carries the shell CLI"* ]] || die "a missing file beside a dirty one: $out"; ok
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
for b in $(sed -e 's/#.*//' "$LIST" | awk 'NF { print $1 }'); do cp "$T/clean/k2-daemon" "$T/bins/$b"; done
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 0 ] && [[ "$out" == *"expecting 5 programs from $LIST: k2-daemon k2-node k2-app-engine k5-cli k2-root-helper"* ]] \
    && [[ "$out" == *"check-no-bash: 5 files, no copy of the shell CLI"* ]] || die "--bins, all there and clean: $out"; ok
rm "$T/bins/k2-root-helper"
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 1 ] && [[ "$out" == *"bins/k2-root-helper: expected (listed in $LIST) but not there: the build did not make it"* ]] \
    && [[ "$out" == *"REFUSED: a program the brand ships was not built, so it was not checked"* ]] || die "--bins, one missing: $out"; ok
cp "$T/raw/k2-root-helper" "$T/bins/k2-root-helper"
check --cli "$CLI" --bins "$LIST" "$T/bins"
[ "$rc" = 1 ] && [[ "$out" == *"bins/k2-root-helper: at byte $PADLEN: a raw deflate stream that inflates to the shell CLI"* ]] \
    || die "--bins, the root helper carries it: $out"; ok
printf '# nothing\n' > "$T/empty.txt"
check --cli "$CLI" --bins "$T/empty.txt" "$T/bins"
[ "$rc" = 2 ] && [[ "$out" == *"names no program"* ]] || die "--bins, an empty list: $out"; ok
check --cli "$CLI" --bins "$LIST"
[ "$rc" = 2 ] && [[ "$out" == usage:* ]] || die "--bins, no folder: $out"; ok

echo "no-bash check tests: $pass passed"

#!/usr/bin/env bash
# Print a folder of Postgres server binaries (initdb, pg_ctl, postgres) for a
# throwaway test server on a Linux x86_64 compute node: the system's when it
# has one, else the embedded-postgres build from Maven Central (zonky, the
# server binaries only), unpacked once into the job cache. Test rigs only:
# never shipped, never on a box.
set -euo pipefail
sys="$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -1 || true)"
if [ -n "$sys" ] && [ -x "$sys/initdb" ]; then echo "$sys"; exit 0; fi
V="${PG_EMBEDDED_VERSION:-17.12.0}"
DEST="${K2_JOB_CACHE:-${TMPDIR:-/tmp}}/pg-embedded-$V"
if [ ! -x "$DEST/bin/initdb" ]; then
    tmp="$(mktemp -d)"
    url="https://repo1.maven.org/maven2/io/zonky/test/postgres/embedded-postgres-binaries-linux-amd64/$V/embedded-postgres-binaries-linux-amd64-$V.jar"
    curl -fsSL -o "$tmp/pg.jar" "$url"
    python3 -I -c 'import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
names = [n for n in z.namelist() if n.endswith(".txz")]
assert len(names) == 1, names
open(sys.argv[2], "wb").write(z.read(names[0]))' "$tmp/pg.jar" "$tmp/pg.txz"
    mkdir -p "$DEST.part"
    tar -xJf "$tmp/pg.txz" -C "$DEST.part"
    rm -rf "$DEST" && mv "$DEST.part" "$DEST"
    rm -rf "$tmp"
fi
[ -x "$DEST/bin/initdb" ] || { echo "pg-binaries: no initdb in $DEST" >&2; exit 1; }
echo "$DEST/bin"

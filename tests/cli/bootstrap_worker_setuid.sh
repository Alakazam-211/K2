#!/usr/bin/env bash
# Hermetic test for step 6 of scripts/bootstrap-k2-dedicated.sh: placing the
# setuid-root k2-vmm-worker next to the daemon, in the daemon user's own
# (writable) bin dir. Root must never chown or setuid a file that is already
# there; it extracts the verified artifact into a root-only staging dir on the
# same filesystem, sets the bits there and renames into place.
#
# The step is cut out of the script and run alone with PATH shims (stat,
# find, chown, zstd, mv) against a temp tree. No root, no network.
# Run with: bash tests/cli/bootstrap_worker_setuid.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BOOT="$ROOT/scripts/bootstrap-k2-dedicated.sh"
[ -f "$BOOT" ] || { echo "FAIL: missing $BOOT" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

echo "== static shape =="
bash -n "$BOOT" || fail "bash -n"
BLOCK="$(sed -n '/^# ── 6\. k2-vmm-worker/,/^# ── 7\./p' "$BOOT")"
[ -n "$BLOCK" ] || fail "step 6 block not found"
CODE="$(printf '%s\n' "$BLOCK" | grep -Ev '^[[:space:]]*#')"
if printf '%s\n' "$CODE" | grep -E '^[[:space:]]*(chown|chmod)' | grep -q 'WORKER_DST'; then
	fail "step 6 chowns/chmods the file already in the daemon user's dir"
fi
ok "never chown/chmod the in-place worker"
if printf '%s\n' "$CODE" | grep -q 'WORKER_STAMP'; then
	fail "step 6 still trusts a stamp in the daemon user's dir"
fi
ok "no stamp shortcut"
if grep -Ev '^[[:space:]]*#' "$BOOT" | grep -q '/tmp/k2-provision'; then
	fail "bootstrap runs the provisioner from a fixed /tmp name"
fi
ok "no fixed /tmp provisioner path"

WORK="$(mktemp -d -t k2-bootstrap-worker-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_STAT="$(command -v stat)"
REAL_MV="$(command -v mv)"
mode_of() { "$REAL_STAT" -c '%a' "$1" 2>/dev/null || "$REAL_STAT" -f '%Mp%Lp' "$1"; }

cat >"$SHIMS/stat" <<SH
#!/bin/sh
# GNU stat -c '%u' / '%d' <path>. Owner of the staging parent from
# SHIM_PARENT_UID (default 0 = root); device 2 for SHIM_OTHER_DEV, else 1.
if [ "\$1" = "-c" ]; then
	case "\$2" in
		%u) echo "\${SHIM_PARENT_UID:-0}"; exit 0 ;;
		%d) if [ "\$3" = "\${SHIM_OTHER_DEV:-}" ]; then echo 2; else echo 1; fi; exit 0 ;;
	esac
fi
exec "$REAL_STAT" "\$@"
SH
cat >"$SHIMS/find" <<'SH'
#!/bin/sh
# find <dir> -maxdepth 0 -perm /022 -> prints <dir> when SHIM_GROUP_WRITABLE=1.
[ "${SHIM_GROUP_WRITABLE:-0}" = "1" ] && echo "$1"
exit 0
SH
cat >"$SHIMS/chown" <<SH
#!/bin/sh
echo "\$*" >>"$WORK/chown.log"
SH
cat >"$SHIMS/zstd" <<'SH'
#!/bin/sh
# zstd -dcq <file>: the test artifact is a plain tar.
for a in "$@"; do f="$a"; done
cat "$f"
SH
cat >"$SHIMS/mv" <<SH
#!/bin/sh
# GNU mv -fT src dst (BSD mv has no -T): refuse a directory destination.
if [ "\$1" = "-fT" ]; then
	[ -d "\$3" ] && [ ! -L "\$3" ] && { echo "mv: cannot overwrite directory '\$3'" >&2; exit 1; }
	exec "$REAL_MV" -f "\$2" "\$3"
fi
exec "$REAL_MV" "\$@"
SH
chmod +x "$SHIMS"/*

# Verified artifact (sha checks are fetch_asset's job, stubbed here).
ART="$WORK/art"
mkdir -p "$ART/src"
printf '\177ELF-genuine-k2-vmm-worker' >"$ART/src/k2-vmm-worker"
(cd "$ART/src" && tar -cf "$ART/k2-vmm-worker-v1-amd64.tar.zst" k2-vmm-worker)

STEP="$WORK/step.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[dedicated] %s\n" "$*"; }'
	echo 'die() { printf "[dedicated] ERROR: %s\n" "$*" >&2; exit 1; }'
	echo 'fetch_asset() { echo "$1" >>"$WORK/fetch.log"; }'
	printf '%s\n' "$BLOCK"
	echo 'echo "PLACED=$WORKER_DST"'
} >"$STEP"

new_box() {
	# new_box <name>: a fresh home tree with the daemon's bin dir.
	BOX="$WORK/$1"
	K2_HOME_T="$BOX/home/k2"
	BIN="$K2_HOME_T/.local/bin"
	mkdir -p "$BIN"
}

run_step() {
	set +e
	out="$(PATH="$SHIMS:$PATH" WORK="$WORK" K2_HOME="$K2_HOME_T" BIN_DIR="$BIN" \
		DL="$ART" WORKER_TAR="k2-vmm-worker-v1-amd64.tar.zst" bash "$STEP" 2>&1)"
	rc=$?
	set -e
}

echo "== a planted file + matching stamp is replaced, never chowned in place =="
new_box box1
printf 'attacker-binary' >"$BIN/k2-vmm-worker"
printf 'whatever' >"$BIN/.k2-vmm-worker.sha"
rm -f "$WORK/chown.log"
run_step
[ "$rc" -eq 0 ] || fail "step exit $rc: $out"
cmp -s "$ART/src/k2-vmm-worker" "$BIN/k2-vmm-worker" || fail "worker bytes are not the artifact's"
ok "worker is the verified artifact, not the planted file"
[ "$(mode_of "$BIN/k2-vmm-worker")" = "4755" ] || fail "mode $(mode_of "$BIN/k2-vmm-worker")"
ok "worker mode 4755"
grep -q 'root:root' "$WORK/chown.log" || fail "chown root:root never ran"
if grep -q "$BIN/k2-vmm-worker" "$WORK/chown.log"; then fail "chowned the in-place path: $(cat "$WORK/chown.log")"; fi
grep -q "$BOX/home/.k2-vmm-worker." "$WORK/chown.log" || fail "chown must target the staging dir: $(cat "$WORK/chown.log")"
ok "root:root is set in the root-only staging dir, not in the daemon user's dir"
[ ! -e "$BIN/.k2-vmm-worker.sha" ] || fail "stamp left behind"
[ -z "$(ls -A "$BOX/home" | grep -v '^k2$' || true)" ] || fail "staging dir left behind: $(ls -A "$BOX/home")"
ok "stamp removed, no staging dir left"

echo "== unsafe staging or placement: refuse before touching anything =="
new_box box2
printf 'attacker-binary' >"$BIN/k2-vmm-worker"
SHIM_PARENT_UID=1000 run_step
[ "$rc" -ne 0 ] || fail "non-root staging parent must fail: $out"
printf '%s' "$out" | grep -q 'must be root-owned' || fail "message: $out"
SHIM_GROUP_WRITABLE=1 run_step
[ "$rc" -ne 0 ] || fail "group-writable staging parent must fail: $out"
printf '%s' "$out" | grep -q 'must be root-owned' || fail "message: $out"
SHIM_OTHER_DEV="$BIN" run_step
[ "$rc" -ne 0 ] || fail "cross-filesystem must fail: $out"
printf '%s' "$out" | grep -q 'same filesystem' || fail "message: $out"
[ "$(cat "$BIN/k2-vmm-worker")" = "attacker-binary" ] || fail "refusals must not touch the bin dir"
ok "non-root, writable or cross-filesystem staging is refused"

new_box box3
mkdir -p "$BOX/elsewhere"
rm -rf "$BIN"
ln -s "$BOX/elsewhere" "$BIN"
run_step
[ "$rc" -ne 0 ] || fail "symlinked bin dir must fail: $out"
printf '%s' "$out" | grep -q 'missing or a symlink' || fail "message: $out"
[ -z "$(ls -A "$BOX/elsewhere")" ] || fail "nothing may land through the symlink"
ok "a symlinked bin dir is refused"

new_box box4
mkdir -p "$BIN/k2-vmm-worker"
run_step
[ "$rc" -ne 0 ] || fail "directory at the worker path must fail: $out"
[ -z "$(ls -A "$BIN/k2-vmm-worker")" ] || fail "worker moved into the planted directory"
ok "a directory planted at the worker path is not followed"

echo ""
echo "PASS: bootstrap worker setuid ($pass checks)"

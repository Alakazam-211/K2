#!/usr/bin/env bash
# Sew 0.3.1 → 0.5.0 (real releases) through K2's root helper, in a
# THROWAWAY LXD VM on the bare-metal sandbox lane (k2n9-sandbox). Never on
# a real box.
#
#   bash tests/linux/run_sew_upgrade_real.sh <in-dir> [ssh-host] [vm-name]
#
# <in-dir> holds: k2-root-helper-old (release pins, built at a main that
# pins Sew 0.3.1), k2-root-helper-new (release pins, this branch: 0.5.0),
# and the two x86_64 core tarballs with the two releases' SHA256SUMS
# (SHA256SUMS-0.3.1, SHA256SUMS-0.5.0). Build the helpers on K2 Compute:
#   cargo build --release -p k2-root-helper --bin k2-root-helper
# (NOT --features test-keys: that pins Sew's test fixtures, not releases).
#
# 1. Checks each tarball against its SHA256SUMS here, before upload.
# 2. Launches an Ubuntu 24.04 LXD VM (4 CPU, 8 GiB, 30 GiB), installs
#    postgresql-16, curl, jq, then runs tests/linux/sew_upgrade_real_in_vm.sh
#    as root in it.
# 3. Deletes the VM (also on failure; KEEP_VM=1 keeps it to look inside). LXD stays on the lane.
set -euo pipefail

IN="${1:?usage: $0 <in-dir> [ssh-host] [vm-name]}"
HOST="${2:-k2n9-sandbox}"
VM="${3:-sew050it-$(date +%H%M%S)}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=15 -o "UserKnownHostsFile=$HOME/.ssh/known_hosts.$HOST" -o StrictHostKeyChecking=yes "$HOST")

for f in k2-root-helper-old k2-root-helper-new sew-0.3.1-x86_64-unknown-linux-gnu.tar.gz sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz SHA256SUMS-0.3.1 SHA256SUMS-0.5.0; do
	[ -f "$IN/$f" ] || { echo "missing $IN/$f" >&2; exit 2; }
done
for v in 0.3.1 0.5.0; do
	t="sew-$v-x86_64-unknown-linux-gnu.tar.gz"
	want="$(awk -v f="$t" '$2 == f {print $1}' "$IN/SHA256SUMS-$v")"
	got="$(shasum -a 256 "$IN/$t" | awk '{print $1}')"
	[ -n "$want" ] && [ "$want" = "$got" ] || { echo "$t: sha256 $got is not SHA256SUMS-$v's '$want'" >&2; exit 1; }
	echo "== $t matches SHA256SUMS-$v ($got)"
done

STAGE="$(mktemp -d -t k2sew-it)"
trap 'rm -rf "$STAGE"' EXIT
cp "$IN/k2-root-helper-old" "$IN/k2-root-helper-new" "$IN"/sew-0.*-x86_64-unknown-linux-gnu.tar.gz "$STAGE/"
cp "$ROOT/scripts/install-root-helper.sh" "$ROOT/tests/linux/sew_upgrade_real_in_vm.sh" "$STAGE/"
touch "$STAGE/THROWAWAY"
R="/srv/k2-sandbox/sew-it-$VM"
COPYFILE_DISABLE=1 tar --no-xattrs -C "$STAGE" -czf - . | "${SSH[@]}" "mkdir -p $R && tar -xzf - -C $R"

"${SSH[@]}" bash -s -- "$R" "$VM" "${KEEP_VM:-0}" <<'REMOTE'
set -euo pipefail
R="$1"; VM="$2"; KEEP="$3"
cleanup() {
	if [ "$KEEP" = 1 ]; then echo "KEEP_VM=1: VM $VM kept (sudo lxc delete -f $VM)"; return; fi
	sudo -n lxc delete -f "$VM" >/dev/null 2>&1 || true; rm -rf "$R"
}
trap cleanup EXIT
sudo -n lxc launch ubuntu:24.04 "$VM" --vm -c limits.cpu=4 -c limits.memory=8GiB -d root,size=30GiB -q </dev/null
for _ in $(seq 1 120); do sudo -n lxc exec "$VM" -- true </dev/null 2>/dev/null && break; sleep 5; done
sudo -n lxc exec "$VM" -- bash -c 'cloud-init status --wait >/dev/null 2>&1 || true; export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && apt-get install -y -qq postgresql-16 curl jq xxd python3 >/dev/null' </dev/null
sudo -n lxc exec "$VM" -- mkdir -p /it </dev/null
for f in "$R"/*; do sudo -n lxc file push -q "$f" "$VM/it/" </dev/null; done
sudo -n lxc exec "$VM" -- bash /it/sew_upgrade_real_in_vm.sh </dev/null
REMOTE
if [ "${KEEP_VM:-0}" = 1 ]; then echo "== done (VM $VM kept on $HOST) =="; else echo "== done (VM $VM deleted) =="; fi

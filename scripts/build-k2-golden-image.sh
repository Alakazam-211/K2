#!/usr/bin/env bash
# build-k2-golden-image.sh — bake the K2 Cloud Standard golden snapshot.
#
# Creates a throwaway Hetzner Cloud VM, runs provision-k2-server.sh
# --bake on it via cloud-init, snapshots it as the golden image, and
# deletes the VM. Run once per K2 release (release checklist item).
#
# Requires: hcloud CLI (https://github.com/hetznercloud/cli) and
#           HCLOUD_TOKEN in the environment.
#
#   HCLOUD_TOKEN=... ./build-k2-golden-image.sh [k2-version]
#
# PRD: .k2/prds/prd-k2-cloud-hosted-servers-v1.md §4.1.
set -euo pipefail

K2_VERSION="${1:-latest}"
SERVER_TYPE="${K2_IMAGE_SERVER_TYPE:-cpx31}"      # 4 vCPU / 8 GB — Standard SKU
LOCATION="${K2_IMAGE_LOCATION:-ash}"              # Ashburn, near the relay
BASE_IMAGE="${K2_IMAGE_BASE:-ubuntu-24.04}"
NAME="k2-golden-bake-$$"

command -v hcloud >/dev/null 2>&1 || { echo "hcloud CLI not found" >&2; exit 1; }
[ -n "${HCLOUD_TOKEN:-}" ] || { echo "HCLOUD_TOKEN not set" >&2; exit 1; }

log() { printf '\033[1;36m[bake]\033[0m %s\n' "$*"; }

USER_DATA=$(cat <<EOF
#cloud-config
runcmd:
  - |
    set -e
    mkdir -p /opt/k2
    curl -fsSL https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/provision-k2-server.sh \
      -o /opt/k2/provision-k2-server.sh
    chmod +x /opt/k2/provision-k2-server.sh
    K2_VERSION="$([ "$K2_VERSION" = latest ] && echo "" || echo "$K2_VERSION")" \
      /opt/k2/provision-k2-server.sh --bake 2>&1 | tee /var/log/k2-bake.log
    touch /var/lib/k2-bake-done
  - poweroff
EOF
)

log "creating bake VM ($SERVER_TYPE, $BASE_IMAGE, $LOCATION)"
hcloud server create --name "$NAME" --type "$SERVER_TYPE" --image "$BASE_IMAGE" \
	--location "$LOCATION" --user-data "$USER_DATA" >/dev/null

cleanup() { hcloud server delete "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

log "waiting for bake to finish (VM powers itself off)"
for _ in $(seq 1 120); do
	STATUS=$(hcloud server describe "$NAME" -o format='{{.Status}}')
	[ "$STATUS" = "off" ] && break
	sleep 10
done
STATUS=$(hcloud server describe "$NAME" -o format='{{.Status}}')
[ "$STATUS" = "off" ] || { echo "bake VM never powered off (status: $STATUS)" >&2; exit 1; }

STAMP=$(date +%Y%m%d)
DESC="k2-golden-standard-${K2_VERSION}-${STAMP}"
log "snapshotting as: $DESC"
hcloud server create-image --type snapshot --description "$DESC" \
	--label "k2-golden=standard" --label "k2-version=${K2_VERSION}" "$NAME"

log "done — snapshot '$DESC' ready; bake VM deleted"

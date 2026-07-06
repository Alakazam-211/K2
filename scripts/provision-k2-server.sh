#!/usr/bin/env bash
# provision-k2-server.sh — turn a fresh Linux box into a K2 server.
#
# This is the K2 Cloud Standard-tier provisioner AND the self-host
# "sandboxes-OFF VPS" runbook-as-code (also works on a Raspberry Pi 4/5
# running 64-bit OS). Sandboxing is deliberately NOT configured here —
# that is the Dedicated-tier bootstrap (see
# .k2/notes/runbook-self-host-sandbox-server.md).
#
# PRD: .k2/prds/prd-k2-cloud-hosted-servers-v1.md (Phase 0).
#
# Usage (as root on Ubuntu 22.04/24.04, Debian 12, or Raspberry Pi OS 64):
#
#   # bake mode — golden-image prep, no per-customer state:
#   ./provision-k2-server.sh --bake
#
#   # full provision (bake steps are idempotent — safe on a baked image):
#   K2_TUNNEL_TOKEN=k2c_... K2_SUBDOMAIN=alice \
#   K2_OWNER_USER=alice \
#   ./provision-k2-server.sh
#
# Environment:
#   K2_TUNNEL_TOKEN    K2 Connect tunnel token (from the subdomains row)
#   K2_SUBDOMAIN       subdomain label (alice → alice.k2.dev)
#   K2_OWNER_USER      first owner login to create
#   K2_OWNER_PASSWORD  its password (omit → generated, printed/callback'd)
#   K2_CALLBACK_URL    control-plane callback POSTed on success (optional)
#   K2_CALLBACK_TOKEN  bearer for the callback (optional)
#   K2_VERSION         pin a daemon release (default: latest)
#   K2_RUN_USER        service account (default: k2)
#   FRP_VERSION        frpc version (default 0.61.1 — MUST match the relay)
#
# Idempotent: re-running converges; existing users/units are updated,
# never duplicated.
set -euo pipefail

K2_RUN_USER="${K2_RUN_USER:-k2}"
FRP_VERSION="${FRP_VERSION:-0.61.1}"
K2_VERSION="${K2_VERSION:-}"
RAW_BASE="https://raw.githubusercontent.com/Alakazam-211/K2/main"
BAKE_ONLY=0
[ "${1:-}" = "--bake" ] && BAKE_ONLY=1

# The helper python snippets read these from the environment.
export K2_TUNNEL_TOKEN="${K2_TUNNEL_TOKEN:-}"
export K2_SUBDOMAIN="${K2_SUBDOMAIN:-}"
export K2_OWNER_USER="${K2_OWNER_USER:-}"
export K2_OWNER_PASSWORD="${K2_OWNER_PASSWORD:-}"
export K2_CALLBACK_URL="${K2_CALLBACK_URL:-}"
export K2_CALLBACK_TOKEN="${K2_CALLBACK_TOKEN:-}"

log() { printf '\033[1;36m[provision]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[provision] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = "0" ] || die "run as root (cloud-init runcmd or sudo)"

case "$(uname -m)" in
	x86_64|amd64)   FRP_ARCH="amd64" ;;
	aarch64|arm64)  FRP_ARCH="arm64" ;;
	*) die "unsupported arch: $(uname -m)" ;;
esac

K2_HOME="/home/${K2_RUN_USER}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── 1. packages ──────────────────────────────────────────────────────
log "installing packages (curl minisign python3 openssl git)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl minisign python3 openssl git ca-certificates >/dev/null

# ── 2. service account ───────────────────────────────────────────────
if ! id "$K2_RUN_USER" >/dev/null 2>&1; then
	log "creating service user ${K2_RUN_USER}"
	useradd --create-home --shell /bin/bash "$K2_RUN_USER"
fi

# ── 3. frpc (version-pinned to the relay's frps) ─────────────────────
NEED_FRPC=1
if command -v frpc >/dev/null 2>&1 && frpc --version 2>/dev/null | grep -q "$FRP_VERSION"; then
	NEED_FRPC=0
fi
if [ "$NEED_FRPC" = 1 ]; then
	log "installing frpc v${FRP_VERSION} (${FRP_ARCH})"
	FRP_DIR="frp_${FRP_VERSION}_linux_${FRP_ARCH}"
	TMP=$(mktemp -d)
	curl -fsSL "https://github.com/fatedier/frp/releases/download/v${FRP_VERSION}/${FRP_DIR}.tar.gz" \
		-o "$TMP/frp.tar.gz"
	tar -xzf "$TMP/frp.tar.gz" -C "$TMP"
	install -m 0755 "$TMP/${FRP_DIR}/frpc" /usr/local/bin/frpc
	rm -rf "$TMP"
fi

# ── 4. daemon binary (minisign-verified via install-daemon.sh) ───────
INSTALLER="$SCRIPT_DIR/install-daemon.sh"
if [ ! -f "$INSTALLER" ]; then
	INSTALLER="/tmp/k2-install-daemon.sh"
	log "fetching install-daemon.sh from the repo"
	curl -fsSL "$RAW_BASE/scripts/install-daemon.sh" -o "$INSTALLER"
	chmod +x "$INSTALLER"
fi
log "installing k2-daemon (verified) as ${K2_RUN_USER}"
sudo -u "$K2_RUN_USER" env \
	K2_NO_SERVICE=1 \
	K2_VERSION="$K2_VERSION" \
	K2_BIN_DIR="$K2_HOME/.local/bin" \
	HOME="$K2_HOME" \
	sh "$INSTALLER"

# ── 5. k2 CLI ────────────────────────────────────────────────────────
if [ -f "$SCRIPT_DIR/../cli/k2" ]; then
	install -m 0755 "$SCRIPT_DIR/../cli/k2" /usr/local/bin/k2
else
	log "fetching k2 CLI from the repo"
	curl -fsSL "$RAW_BASE/cli/k2" -o /usr/local/bin/k2
	chmod 0755 /usr/local/bin/k2
fi

# ── 6. tunnel.json (before first daemon start, so boot auto-dials) ───
if [ -n "${K2_TUNNEL_TOKEN:-}" ] && [ -n "${K2_SUBDOMAIN:-}" ]; then
	log "writing tunnel.json for ${K2_SUBDOMAIN}.k2.dev"
	sudo -u "$K2_RUN_USER" mkdir -p "$K2_HOME/.k2"
	python3 - "$K2_HOME/.k2/tunnel.json" <<'PYEOF'
import json, os, sys
path = sys.argv[1]
cfg = {}
if os.path.exists(path):
    try:
        cfg = json.load(open(path))
    except Exception:
        cfg = {}
cfg["token"] = os.environ["K2_TUNNEL_TOKEN"]
cfg["subdomain"] = os.environ["K2_SUBDOMAIN"]
cfg["auto_start"] = True
cfg.setdefault("device_label", "k2-cloud")
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(cfg, f, indent=2)
os.replace(tmp, path)
PYEOF
	chown "$K2_RUN_USER:$K2_RUN_USER" "$K2_HOME/.k2/tunnel.json"
	chmod 0600 "$K2_HOME/.k2/tunnel.json"
fi

# ── 7. system-level supervisor unit (sandboxes OFF — no K2_SANDBOX*) ─
log "writing /etc/systemd/system/k2-daemon.service"
cat > /etc/systemd/system/k2-daemon.service <<EOF
[Unit]
Description=K2 daemon (headless server)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${K2_RUN_USER}
Environment=HOME=${K2_HOME}
Environment=PATH=/usr/local/bin:/usr/bin:/bin:${K2_HOME}/.local/bin
ExecStart=${K2_HOME}/.local/bin/k2-daemon
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable k2-daemon >/dev/null 2>&1

if [ "$BAKE_ONLY" = 1 ]; then
	log "bake complete — image is ready to snapshot (daemon enabled, not personalized)"
	exit 0
fi

log "starting k2-daemon"
systemctl restart k2-daemon

# ── 8. wait for daemon readiness ─────────────────────────────────────
log "waiting for daemon.port + daemon.token"
for _ in $(seq 1 60); do
	[ -s "$K2_HOME/.k2/daemon.port" ] && [ -s "$K2_HOME/.k2/daemon.token" ] && break
	sleep 1
done
[ -s "$K2_HOME/.k2/daemon.port" ] || die "daemon did not publish its port in 60s (journalctl -u k2-daemon)"
PORT=$(cat "$K2_HOME/.k2/daemon.port")
TOKEN=$(cat "$K2_HOME/.k2/daemon.token")
BASE="http://127.0.0.1:${PORT}"

# boot-status gate: wait until the daemon answers
for _ in $(seq 1 30); do
	curl -fsS "$BASE/boot-status" >/dev/null 2>&1 && break
	sleep 1
done

# ── 9. first owner user ──────────────────────────────────────────────
GENERATED_PW=0
if [ -n "${K2_OWNER_USER:-}" ]; then
	if [ -z "${K2_OWNER_PASSWORD:-}" ]; then
		K2_OWNER_PASSWORD=$(openssl rand -base64 18 | tr -d '/+=' | cut -c1-20)
		export K2_OWNER_PASSWORD
		GENERATED_PW=1
	fi
	log "creating owner user '${K2_OWNER_USER}'"
	BODY=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OWNER_USER"],"password":os.environ["K2_OWNER_PASSWORD"]}))')
	ADD_RES_FILE=$(mktemp)
	HTTP=$(curl -sS -o "$ADD_RES_FILE" -w '%{http_code}' -X POST "$BASE/cli/users/add?token=$TOKEN" -d "$BODY")
	if [ "$HTTP" != "200" ]; then
		grep -qi "exist" "$ADD_RES_FILE" \
			|| die "users/add failed ($HTTP): $(cat "$ADD_RES_FILE")"
		log "owner user already exists — continuing (idempotent re-run)"
	fi
	rm -f "$ADD_RES_FILE"
	ROLE_BODY=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OWNER_USER"],"role":"owner"}))')
	curl -fsS -X POST "$BASE/cli/users/set-role?token=$TOKEN" -d "$ROLE_BODY" >/dev/null \
		|| die "users/set-role owner failed"
fi

# ── 10. control-plane callback ───────────────────────────────────────
if [ -n "${K2_CALLBACK_URL:-}" ]; then
	log "posting provisioning callback"
	CB=$(python3 -c 'import json,os;print(json.dumps({
		"status":"online",
		"subdomain":os.environ.get("K2_SUBDOMAIN",""),
		"ownerUser":os.environ.get("K2_OWNER_USER",""),
	}))')
	curl -fsS -X POST "$K2_CALLBACK_URL" \
		${K2_CALLBACK_TOKEN:+-H "Authorization: Bearer $K2_CALLBACK_TOKEN"} \
		-H "Content-Type: application/json" -d "$CB" >/dev/null \
		|| log "WARN: callback failed (provisioning itself succeeded)"
fi

# ── 11. summary ──────────────────────────────────────────────────────
log "PROVISION COMPLETE"
echo "  daemon:    $(systemctl is-active k2-daemon) on 127.0.0.1:${PORT}"
[ -n "${K2_SUBDOMAIN:-}" ] && echo "  address:   https://${K2_SUBDOMAIN}.k2.dev"
if [ -n "${K2_OWNER_USER:-}" ]; then
	echo "  owner:     ${K2_OWNER_USER}"
	if [ "$GENERATED_PW" = 1 ] && [ -z "${K2_CALLBACK_URL:-}" ]; then
		echo "  password:  ${K2_OWNER_PASSWORD}   (generated — shown ONCE, store it now)"
	fi
fi
echo "  sandboxes: OFF (Standard-tier host — the Dedicated bootstrap adds them)"

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
# Unit name is parameterized so a test provision can run side-by-side
# with an existing daemon on the same box (e.g. K2_RUN_USER=k2std
# K2_UNIT_NAME=k2-daemon-std) without touching its unit.
K2_UNIT_NAME="${K2_UNIT_NAME:-k2-daemon}"
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
# Multi-relay failover (0.40.43): ordered fallback list, index 0 = primary.
# K2_RELAYS is "host[:port],host[:port]"; default = the two edge relays
# (k2e-01 primary, k2e-02 secondary). An empty value leaves relays unset so
# the daemon falls back to its single default endpoint (byte-identical old
# behavior). The primary MUST stay element 0 so a config push to an already-
# connected box doesn't move it until a real failure.
_relays_raw = os.environ.get("K2_RELAYS", "178.156.232.105,40.160.53.25")
_relays = []
for _r in (x.strip() for x in _relays_raw.split(",") if x.strip()):
    _host, _, _port = _r.partition(":")
    _entry = {"host": _host}
    if _port:
        _entry["port"] = int(_port)
    _relays.append(_entry)
if _relays:
    cfg["relays"] = _relays
else:
    cfg.pop("relays", None)
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(cfg, f, indent=2)
os.replace(tmp, path)
PYEOF
	chown "$K2_RUN_USER:$K2_RUN_USER" "$K2_HOME/.k2/tunnel.json"
	chmod 0600 "$K2_HOME/.k2/tunnel.json"
fi

# ── 7. system-level supervisor unit (sandboxes OFF — no K2_SANDBOX*) ─
log "writing /etc/systemd/system/${K2_UNIT_NAME}.service"
cat > "/etc/systemd/system/${K2_UNIT_NAME}.service" <<EOF
[Unit]
Description=K2 daemon (headless server)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${K2_RUN_USER}
Environment=HOME=${K2_HOME}
Environment=PATH=/usr/local/bin:/usr/bin:/bin:${K2_HOME}/.local/bin
# Standard-tier API surface: /v1/ping + canonical-message API (and, when
# built, host-sessions). Sandbox families stay dark — no K2_SANDBOX_API.
Environment=K2_API=1
# Host-session spawns mint per-session scoped hook tokens; without this the
# ambient hook token is the OWNER token, which /cli/respond refuses (it names
# no session) — so \`k2 respond\` read-back from API-spawned agents needs it.
Environment=K2_HOOK_SCOPED=1
ExecStart=${K2_HOME}/.local/bin/k2-daemon
Restart=always
RestartSec=2
# Shape B self-update spawns a detached swap helper that must OUTLIVE the
# daemon across the restart; the default control-group KillMode would
# kill it before it can swap the staged binary. The daemon reaps its own
# PTY children on graceful shutdown, so process-scoped kill is safe.
KillMode=process

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$K2_UNIT_NAME" >/dev/null 2>&1

if [ "$BAKE_ONLY" = 1 ]; then
	log "bake complete — image is ready to snapshot (daemon enabled, not personalized)"
	exit 0
fi

log "starting ${K2_UNIT_NAME}"
systemctl restart "$K2_UNIT_NAME"

# ── 8. wait for daemon readiness ─────────────────────────────────────
# The daemon rotates daemon.token on EVERY boot, so on a re-run the old
# files linger until the restarted daemon rewrites them. Re-read both
# each iteration and only accept a token the live daemon actually honors.
log "waiting for daemon readiness (fresh port + token)"
PORT=""; TOKEN=""; BASE=""
for _ in $(seq 1 90); do
	PORT=$(cat "$K2_HOME/.k2/daemon.port" 2>/dev/null || true)
	TOKEN=$(cat "$K2_HOME/.k2/daemon.token" 2>/dev/null || true)
	if [ -n "$PORT" ] && [ -n "$TOKEN" ]; then
		BASE="http://127.0.0.1:${PORT}"
		if curl -fsS "$BASE/boot-status" 2>/dev/null | grep -q '"phase":"ready"'; then
			AUTH_CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/auth/whoami?token=$TOKEN")
			[ "$AUTH_CODE" = "200" ] && break
		fi
	fi
	sleep 1
	PORT=""
done
[ -n "$PORT" ] || die "daemon not ready with a live token in 90s (journalctl -u ${K2_UNIT_NAME})"

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
	OWNER_CREATED=1
	HTTP=$(curl -sS -o "$ADD_RES_FILE" -w '%{http_code}' -X POST "$BASE/cli/users/add?token=$TOKEN" -d "$BODY")
	if [ "$HTTP" != "200" ]; then
		grep -qi "exist" "$ADD_RES_FILE" \
			|| die "users/add failed ($HTTP): $(cat "$ADD_RES_FILE")"
		log "owner user already exists — continuing (idempotent re-run, password unchanged)"
		OWNER_CREATED=0
	fi
	rm -f "$ADD_RES_FILE"
	ROLE_BODY=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OWNER_USER"],"role":"owner"}))')
	curl -fsS -X POST "$BASE/cli/users/set-role?token=$TOKEN" -d "$ROLE_BODY" >/dev/null \
		|| die "users/set-role owner failed"
fi

# ── 9b. website-management service user (K2 Cloud only) ─────────────
# Owner-role user the k2.dev control plane uses to proxy the server
# modal's actions (user management, subdomain re-pairing) over the
# tunnel. Customer-visible in their user list; deleting it opts out of
# website management. Only created when K2_OPS_USER is set (hosted
# provisioning); self-host runs never get it.
OPS_PASSWORD=""
if [ -n "${K2_OPS_USER:-}" ]; then
	OPS_PASSWORD=$(openssl rand -base64 24 | tr -d '/+=' | cut -c1-28)
	export K2_OPS_USER OPS_PASSWORD
	log "creating service user '${K2_OPS_USER}' (website management)"
	OPS_BODY=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OPS_USER"],"password":os.environ["OPS_PASSWORD"]}))')
	OPS_RES=$(mktemp)
	HTTP=$(curl -sS -o "$OPS_RES" -w '%{http_code}' -X POST "$BASE/cli/users/add?token=$TOKEN" -d "$OPS_BODY")
	if [ "$HTTP" != "200" ]; then
		grep -qi "exist" "$OPS_RES" || die "ops users/add failed ($HTTP): $(cat "$OPS_RES")"
		log "ops user already exists — leaving its password unchanged"
		OPS_PASSWORD=""
	fi
	rm -f "$OPS_RES"
	OPS_ROLE=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OPS_USER"],"role":"owner"}))')
	curl -fsS -X POST "$BASE/cli/users/set-role?token=$TOKEN" -d "$OPS_ROLE" >/dev/null \
		|| die "ops set-role owner failed"
fi

# ── 10. control-plane callback ───────────────────────────────────────
if [ -n "${K2_CALLBACK_URL:-}" ]; then
	log "posting provisioning callback"
	export CB_OPS_PASSWORD="${OPS_PASSWORD:-}"
	CB=$(python3 -c 'import json,os;print(json.dumps({
		"status":"online",
		"subdomain":os.environ.get("K2_SUBDOMAIN",""),
		"ownerUser":os.environ.get("K2_OWNER_USER",""),
		"opsUser":os.environ.get("K2_OPS_USER",""),
		"opsPassword":os.environ.get("CB_OPS_PASSWORD",""),
	}))')
	curl -fsS -X POST "$K2_CALLBACK_URL" \
		${K2_CALLBACK_TOKEN:+-H "Authorization: Bearer $K2_CALLBACK_TOKEN"} \
		-H "Content-Type: application/json" -d "$CB" >/dev/null \
		|| log "WARN: callback failed (provisioning itself succeeded)"
fi

# ── 11. summary ──────────────────────────────────────────────────────
log "PROVISION COMPLETE"
echo "  daemon:    $(systemctl is-active "$K2_UNIT_NAME") on 127.0.0.1:${PORT}"
[ -n "${K2_SUBDOMAIN:-}" ] && echo "  address:   https://${K2_SUBDOMAIN}.k2.dev"
if [ -n "${K2_OWNER_USER:-}" ]; then
	echo "  owner:     ${K2_OWNER_USER}"
	if [ "${OWNER_CREATED:-0}" = 1 ] && [ "$GENERATED_PW" = 1 ] && [ -z "${K2_CALLBACK_URL:-}" ]; then
		echo "  password:  ${K2_OWNER_PASSWORD}   (generated — shown ONCE, store it now)"
	fi
fi
echo "  sandboxes: OFF (Standard-tier host — the Dedicated bootstrap adds them)"

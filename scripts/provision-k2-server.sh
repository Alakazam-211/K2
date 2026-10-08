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
#   ./provision-k2-server.sh --bake --with-db   # also bake Postgres sidecar
#   K2_BAKE_DB=1 ./provision-k2-server.sh --bake
#
#   # full provision (bake steps are idempotent — safe on a baked image):
#   K2_TUNNEL_TOKEN=k2c_... K2_SUBDOMAIN=alice \
#   K2_OWNER_USER=alice \
#   ./provision-k2-server.sh
#
#   # self-host interactive path (after install, no K2_TUNNEL_TOKEN):
#   k2 users add alice --role owner    # prompts for password
#   k2 connect login                   # k2.dev account → pick subdomain → live
#
# Environment:
#   K2_TUNNEL_TOKEN    K2 Connect tunnel token (automation fallback; from
#                      the subdomains row — prefer `k2 connect login` for
#                      interactive self-host pairing)
#   K2_SUBDOMAIN       subdomain label (alice → alice.k2.dev)
#   K2_OWNER_USER      first owner login to create
#   K2_OWNER_PASSWORD  its password (omit → generated, printed/callback'd)
#   K2_CALLBACK_URL    control-plane callback POSTed on success (optional)
#   K2_CALLBACK_TOKEN  bearer for the callback (optional)
#   K2_VERSION         pin a daemon release (default: latest)
#   K2_RUN_USER        service account (default: k2)
#   K2_MAIL_HELPER_BIN absolute path to a built k2-mail-helper binary.
#                      Else beside this script (scripts/k2-mail-helper), else
#                      ../target/release/k2-mail-helper from a
#                      `cargo build -p k2-daemon --release --bin k2-mail-helper`,
#                      else the signed GitHub release asset for the SAME
#                      version as the installed daemon (install-mail-helper.sh;
#                      v0.44.1+). Every box (bake and full provision) gets the
#                      helper so its agents can enable hosted mail without ops.
#                      A failure is a hard failure that stops provisioning;
#                      after install the helper must be root:root 0755, its
#                      sudoers file 0440, and `visudo -c` must pass. The step
#                      never enables mail, touches DNS, or edits other sudoers.
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
WITH_DB=0
# Must still treat `--bake` as argv[1] (historical). Extra flags are
# parsed without breaking that; `K2_BAKE_DB=1` is the env alias.
for _arg in "$@"; do
	case "$_arg" in
		--bake) BAKE_ONLY=1 ;;
		--with-db) WITH_DB=1 ;;
	esac
done
[ "${K2_BAKE_DB:-}" = "1" ] && WITH_DB=1

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
# Root decides the version ONCE, here, and both the daemon (step 4) and
# the mail helper (step 7a) use it. Root never executes the installed
# daemon (it lives in the daemon user's writable home) to learn it.
# Plain x.y.z only: it becomes part of download URLs.
if [ -z "$K2_VERSION" ]; then
	log "resolving the latest daemon release"
	K2_VERSION="$(curl -fsSL --retry 3 --retry-delay 2 \
		"https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json" \
		| sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)" \
		|| die "could not resolve the latest daemon release; set K2_VERSION=<x.y.z>"
fi
K2_VERSION="${K2_VERSION#v}"
[[ "$K2_VERSION" =~ ^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$ ]] \
	|| die "K2_VERSION must look like 0.45.1 (got '$K2_VERSION')"
INSTALLER="$SCRIPT_DIR/install-daemon.sh"
if [ ! -f "$INSTALLER" ]; then
	INSTALLER="$(mktemp -d)/install-daemon.sh"
	chmod 0755 "$(dirname "$INSTALLER")"
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
# The daemon self-stages the CLI at every boot (cli_stage, 0.40.41+) so
# updates carry it forward — that only works if the DAEMON USER can write
# this file. Root-owned k2 = CLI silently frozen at provision-day version
# (bit nsi + rpmavs, 2026-07-15).
chown "$K2_RUN_USER:$K2_RUN_USER" /usr/local/bin/k2

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
# Linux update swaps the binary in-process before exit (0.40.43+).
# KillMode=control-group ensures orphan frpc dies with the unit so a
# restarted daemon cannot leave stale localPort dialers registered at frps.
KillMode=control-group

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$K2_UNIT_NAME" >/dev/null 2>&1

# ── 7a. mail root helper (always, not only --with-db) ───────────────
# Every provisioned box comes up with the hosted-mail root helper already
# installed, so the customer's agents can set up mail themselves: `k2
# hostmail enable` as user k2 calls `sudo -n /usr/local/libexec/k2-mail-helper`.
# install-mail-helper.sh installs it root:root 0755 plus
# /etc/sudoers.d/k2-mail-helper (visudo-checked, 0440). A local binary wins
# (K2_MAIL_HELPER_BIN, beside this script, ../target/release); otherwise the
# installer fetches the minisign-signed release asset for the SAME version
# as the daemon step 4 just installed (assets ship from v0.44.1).
# FAIL LOUD: any failure here stops provisioning (never skipped, never
# warned past). This step installs the helper only: it does not enable
# hosted mail, touch DNS, or write any other sudoers file (never widens
# /etc/sudoers.d/k2-pg-helper).
# K2_INSTALL_ROOT is install-mail-helper.sh's DESTDIR-style prefix for the
# hermetic tests (tests/cli/provision_mail_helper.sh); it is passed through
# to the installer. Production leaves it empty.
MAIL_HELPER_PATH="${K2_INSTALL_ROOT:-}/usr/local/libexec/k2-mail-helper"
MAIL_HELPER_SUDOERS="${K2_INSTALL_ROOT:-}/etc/sudoers.d/k2-mail-helper"
MAIL_HELPER_LINE=""
mail_helper_die() {
	die "mail helper: $* — provisioning stopped. Fix it and re-run this script (idempotent), or run install-mail-helper.sh --version <daemon version> by hand."
}
# Post-install check: the exact modes the daemon's `sudo -n` path relies on.
verify_mail_helper() {
	local got
	{ [ -f "$MAIL_HELPER_PATH" ] && [ ! -L "$MAIL_HELPER_PATH" ]; } \
		|| mail_helper_die "$MAIL_HELPER_PATH is missing after install"
	got="$(stat -c '%U:%G %a' "$MAIL_HELPER_PATH")"
	[ "$got" = "root:root 755" ] \
		|| mail_helper_die "$MAIL_HELPER_PATH is '$got', expected 'root:root 755'"
	{ [ -f "$MAIL_HELPER_SUDOERS" ] && [ ! -L "$MAIL_HELPER_SUDOERS" ]; } \
		|| mail_helper_die "$MAIL_HELPER_SUDOERS is missing after install"
	got="$(stat -c '%U:%G %a' "$MAIL_HELPER_SUDOERS")"
	[ "$got" = "root:root 440" ] \
		|| mail_helper_die "$MAIL_HELPER_SUDOERS is '$got', expected 'root:root 440'"
	visudo -c >/dev/null 2>&1 \
		|| mail_helper_die "visudo -c fails after the helper install (check /etc/sudoers and /etc/sudoers.d)"
}
if [ "$(uname -s)" != "Linux" ]; then
	# Hosted mail is Linux-only (the installer refuses anything else).
	log "mail helper: skipped — hosted mail is Linux-only (this is $(uname -s))"
	MAIL_HELPER_LINE="skipped (not Linux)"
else
	log "installing ${MAIL_HELPER_PATH}"
	# The version is the one root resolved before step 4 (K2_VERSION),
	# never read by running the daemon binary as root: it sits in the
	# daemon user's writable home. Plain x.y.z only.
	DAEMON_VERSION="${K2_VERSION#v}"
	[[ "$DAEMON_VERSION" =~ ^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$ ]] \
		|| mail_helper_die "K2_VERSION must be a plain x.y.z release (got '${K2_VERSION}')"
	# Cross-check that step 4 installed that version. The binary runs as
	# the daemon user (never root); 0.40.82+ prints and exits without
	# booting. Its answer is only compared, never used.
	INSTALLED_VERSION="$(runuser -u "$K2_RUN_USER" -- timeout 10 "$K2_HOME/.local/bin/k2-daemon" --version 2>/dev/null \
		| sed -n 's/^k2-daemon[[:space:]]\{1,\}\([0-9][^[:space:]]*\).*/\1/p' | head -n1 || true)"
	[ -n "$INSTALLED_VERSION" ] \
		|| mail_helper_die "could not read the installed daemon version from $K2_HOME/.local/bin/k2-daemon --version (run as ${K2_RUN_USER})"
	if [ "$INSTALLED_VERSION" != "$DAEMON_VERSION" ]; then
		mail_helper_die "K2_VERSION=${K2_VERSION} but the installed daemon reports ${INSTALLED_VERSION}; the helper must match the daemon"
	fi
	MAIL_HELPER_SRC=""
	if [ -n "${K2_MAIL_HELPER_BIN:-}" ]; then
		[ -f "$K2_MAIL_HELPER_BIN" ] || mail_helper_die "K2_MAIL_HELPER_BIN is set but missing: $K2_MAIL_HELPER_BIN"
		MAIL_HELPER_SRC="$K2_MAIL_HELPER_BIN"
	elif [ -f "$SCRIPT_DIR/k2-mail-helper" ]; then
		MAIL_HELPER_SRC="$SCRIPT_DIR/k2-mail-helper"
	elif [ -f "$SCRIPT_DIR/../target/release/k2-mail-helper" ]; then
		MAIL_HELPER_SRC="$SCRIPT_DIR/../target/release/k2-mail-helper"
	fi
	# The installer: the copy beside this script, else the signed release's
	# own copy for the same version. Downloaded into a fresh root-only
	# directory, never a fixed /tmp name another user could pre-create.
	MAIL_HELPER_INSTALLER="$SCRIPT_DIR/install-mail-helper.sh"
	if [ ! -f "$MAIL_HELPER_INSTALLER" ]; then
		MAIL_HELPER_TMP="$(mktemp -d)" || mail_helper_die "mktemp failed"
		MAIL_HELPER_INSTALLER="$MAIL_HELPER_TMP/install-mail-helper.sh"
		MAIL_HELPER_INSTALLER_URL="https://github.com/Alakazam-211/K2/releases/download/v${DAEMON_VERSION}/install-mail-helper.sh"
		log "fetching install-mail-helper.sh (${MAIL_HELPER_INSTALLER_URL})"
		curl -fsSL --retry 3 --retry-delay 2 "$MAIL_HELPER_INSTALLER_URL" -o "$MAIL_HELPER_INSTALLER" \
			|| mail_helper_die "could not download $MAIL_HELPER_INSTALLER_URL"
	fi
	if [ -n "$MAIL_HELPER_SRC" ]; then
		bash "$MAIL_HELPER_INSTALLER" --file "$MAIL_HELPER_SRC" --user "$K2_RUN_USER" \
			|| mail_helper_die "install-mail-helper.sh --file $MAIL_HELPER_SRC failed (output above)"
		MAIL_HELPER_LINE="installed (local build, daemon v${DAEMON_VERSION})"
	else
		log "no local k2-mail-helper — installing the signed v${DAEMON_VERSION} release asset"
		bash "$MAIL_HELPER_INSTALLER" --version "$DAEMON_VERSION" --user "$K2_RUN_USER" \
			|| mail_helper_die "install-mail-helper.sh --version $DAEMON_VERSION failed (output above)"
		MAIL_HELPER_LINE="installed (v${DAEMON_VERSION})"
	fi
	verify_mail_helper
	log "mail helper: ${MAIL_HELPER_LINE}"
fi

# ── 7b. optional Postgres sidecar bake (`--with-db` / K2_BAKE_DB=1) ──
# Distro packages (postgresql + postgresql-client), NOT postgresql-16
# (jammy/bookworm ship 14/15). Empty cluster. listen_addresses=localhost.
# Root helper + sudoers stub so User=k2 never apt-gets and never holds
# a superuser password. Mail stays GitHub-tarball at enable; db is bake-apt.
if [ "$WITH_DB" = 1 ]; then
	log "baking Postgres sidecar (postgresql + postgresql-client)"
	apt-get install -y -qq postgresql postgresql-client >/dev/null
	# Distro default PGDATA (never the workspace). Empty cluster.
	if [ -d /etc/postgresql ]; then
		for _conf in /etc/postgresql/*/main/postgresql.conf; do
			[ -f "$_conf" ] || continue
			if grep -q "^listen_addresses" "$_conf"; then
				sed -i "s/^listen_addresses.*/listen_addresses = 'localhost'/" "$_conf"
			else
				printf "\nlisten_addresses = 'localhost'\n" >> "$_conf"
			fi
		done
	fi
	systemctl enable postgresql >/dev/null 2>&1 || true
	HELPER_SRC="$SCRIPT_DIR/k2-pg-helper"
	if [ ! -f "$HELPER_SRC" ]; then
		HELPER_SRC="/usr/local/libexec/k2-pg-helper"
	fi
	if [ -f "$SCRIPT_DIR/k2-pg-helper" ]; then
		install -d -m 0755 /usr/local/libexec
		install -m 0755 "$SCRIPT_DIR/k2-pg-helper" /usr/local/libexec/k2-pg-helper
	fi
	# Sudoers stub: daemon User=k2 may run ONLY the helper (argv allowlist
	# inside the helper). Do not grant k2 full systemctl/psql.
	cat > /etc/sudoers.d/k2-pg-helper <<SUDO
# K2 Postgres sidecar helper (prd-workspace-data-sidecar-v1 D11).
# Daemon (User=${K2_RUN_USER}) may run ONLY this argv-allowlisted helper as root.
${K2_RUN_USER} ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper
SUDO
	chmod 0440 /etc/sudoers.d/k2-pg-helper
	# D29: MemoryHigh/Max drop-in + GUC caps. Helper body is canned
	# (daemon User=k2 cannot write systemd itself).
	if [ -x /usr/local/libexec/k2-pg-helper ]; then
		/usr/local/libexec/k2-pg-helper install-ram-fence
		systemctl daemon-reload
	fi
	log "Postgres sidecar baked (unit enabled, empty cluster, helper + sudoers, RAM fence)"
fi

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
echo "  mail:      helper ${MAIL_HELPER_LINE}"
echo "  sandboxes: OFF (Standard-tier host — the Dedicated bootstrap adds them)"
if [ -z "${K2_TUNNEL_TOKEN:-}" ] || [ -z "${K2_OWNER_USER:-}" ]; then
echo ""
echo "  Next (interactive self-host):"
[ -z "${K2_OWNER_USER:-}" ] && echo "    k2 users add <you> --role owner   # prompts for password"
[ -z "${K2_TUNNEL_TOKEN:-}" ] && echo "    k2 connect login                 # pair purchased *.k2.dev subdomain"
echo "  Automation fallback still works: K2_TUNNEL_TOKEN + K2_SUBDOMAIN env."
fi

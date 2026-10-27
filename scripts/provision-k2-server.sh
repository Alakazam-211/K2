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
#   K2_ROOT_HELPER_BIN absolute path to a built k2-root-helper binary.
#                      Else beside this script (scripts/k2-root-helper), else
#                      ../target/release/k2-root-helper from a
#                      `cargo build -p k2-root-helper --release`, else the
#                      signed GitHub release asset for the SAME version as the
#                      installed daemon (install-root-helper.sh; v0.45.4+).
#                      Every box (bake and full provision) gets K2's one root
#                      door so its agents can enable hosted mail and the db
#                      sidecar without ops. A failure is a hard failure that
#                      stops provisioning; after install the helper must be
#                      root:root 0755, its sudoers file 0440, daemon.toml
#                      0644, and `visudo -c` must pass. The step never
#                      enables mail, touches DNS, or edits other sudoers.
#   K2_SKIP_APPS       1 = skip step 8b (Apps (beta) box prep + App engine).
#                      Default: a full provision prepares the box for walled
#                      Apps and installs the App engine (0.46.3+ daemons).
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

# Credentials never ride curl's argv (any OS user can read argv with `ps`).
# k2_curl hands curl its URL (the owner token stays in the query, exactly
# as before), an optional Authorization header and an optional body as a
# config on stdin (`curl --config -`). printf is a builtin: nothing here
# puts the values on an argv. The wire request is unchanged.
#   k2_curl URL BODY BEARER [curl args...]   (BODY/BEARER may be "")
_curl_cfg_quote() {
	# \ and " escaped, newline -> \n (curl's config parser undoes exactly
	# that). sed reads the value on stdin; the sentinel x keeps a trailing
	# newline. Same behaviour on every bash (no ${s//} backslash quirks).
	local s
	s="$(printf '%sx' "$1" | LC_ALL=C sed -e 's/[\\"]/\\&/g' -e '$!s/$/\\n/' | LC_ALL=C tr -d '\n')"
	printf '"%s"' "${s%x}"
}
k2_curl() {
	local url="$1" body="$2" bearer="$3"
	shift 3
	{
		printf 'url = %s\n' "$(_curl_cfg_quote "$url")"
		if [ -n "$bearer" ]; then
			printf 'header = %s\n' "$(_curl_cfg_quote "Authorization: Bearer $bearer")"
		fi
		if [ -n "$body" ]; then
			printf 'data = %s\n' "$(_curl_cfg_quote "$body")"
		fi
	} | curl --config - "$@"
}

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
# install-daemon.sh runs as ${K2_RUN_USER}, who may not be able to read the
# script's own folder (e.g. scripts copied to /root/…). Stage a readable copy
# in a fresh 0755 temp dir first; the bytes are unchanged.
INSTALLER_STAGE="$(mktemp -d)"
chmod 0755 "$INSTALLER_STAGE"
install -m 0644 "$INSTALLER" "$INSTALLER_STAGE/install-daemon.sh"
INSTALLER="$INSTALLER_STAGE/install-daemon.sh"
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
# KillMode=process (k2-ptyd, prd-k2-ptyd-v1 P87): a stop, restart or crash
# signals only the daemon. k2-ptyd, the agents it hosts, frpc and Apps live
# on in the unit's cgroup and the next daemon adopts them; an explicit stop
# ends the agents itself (the daemon reads the systemd stop job). With
# control-group a restart would kill every agent. Runtime stage H's drop-in
# (<unit>.d/k2-handoff.conf) carries the same line, plus NotifyAccess=main
# and ExecReload=SIGUSR2 for the no-downtime systemctl reload.
KillMode=process
# A hand-off successor (systemctl reload) is not systemd's child: systemd
# can't wait for it and would SIGKILL it at stop/restart, skipping its
# graceful shutdown (k2-ptyd: a stop must end agents, a restart keeps
# them). systemd waits for its own ExecStop: SIGTERM the main process
# and wait until it is gone. The daemon hands off only with this line.
ExecStop=/bin/sh -c 'kill -TERM "\$\$MAINPID" 2>/dev/null; while kill -0 "\$\$MAINPID" 2>/dev/null; do sleep 0.2; done' k2-wait-mainpid
# Agents run in this unit's cgroup. The default OOMPolicy=stop turns one
# OOM-killed agent into a stop + restart of the whole daemon; continue lets
# only that process die. (The stage H drop-in carries the same line.)
OOMPolicy=continue
# The unit's own runtime dir (/run/k2; XDG_RUNTIME_DIR/k2 for a user unit)
# holds frpc.toml and agents' secret files: systemd-logind deletes
# /run/user/<uid> and the user's /dev/shm files when their last login
# session ends. Preserve keeps it across restarts (frpc and agents outlive
# the daemon process). (The stage H drop-in carries the same lines.)
RuntimeDirectory=k2
RuntimeDirectoryMode=0700
RuntimeDirectoryPreserve=yes

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$K2_UNIT_NAME" >/dev/null 2>&1

# ── 7a. root helper (always, not only --with-db) ────────────────────
# Every provisioned box comes up with K2's one root door already installed
# (prd-k2-root-helper-v1 §11.2), so the customer's agents can set up hosted
# mail and the Postgres sidecar without ops, and the daemon keeps the
# helper current by itself (signed self-update). install-root-helper.sh
# installs /usr/local/libexec/k2-root-helper root:root 0755, the one
# sudoers line /etc/sudoers.d/k2-root-helper (visudo-checked, 0440) and
# /etc/k2-root-helper/daemon.toml (who the daemon is: this user, this
# unit, system scope). A local binary wins (K2_ROOT_HELPER_BIN, beside
# this script, ../target/release); otherwise the installer fetches the
# signed release asset for the SAME version as the daemon step 4 just
# installed (assets ship from v0.45.4).
# FAIL LOUD: any failure here stops provisioning (never skipped, never
# warned past). This step installs the helper only: it does not enable
# hosted mail, touch DNS, or write any other sudoers file.
# K2_INSTALL_ROOT is install-root-helper.sh's DESTDIR-style prefix for the
# hermetic tests (tests/cli/provision_mail_helper.sh); it is passed through
# to the installer. Production leaves it empty.
ROOT_HELPER_PATH="${K2_INSTALL_ROOT:-}/usr/local/libexec/k2-root-helper"
ROOT_HELPER_SUDOERS="${K2_INSTALL_ROOT:-}/etc/sudoers.d/k2-root-helper"
ROOT_HELPER_TOML="${K2_INSTALL_ROOT:-}/etc/k2-root-helper/daemon.toml"
MAIL_HELPER_LINE=""
root_helper_die() {
	die "root helper: $* — provisioning stopped. Fix it and re-run this script (idempotent), or run install-root-helper.sh --version <daemon version> by hand."
}
# Post-install check: the exact modes the daemon's `sudo -n` path relies on.
verify_root_helper() {
	local got
	{ [ -f "$ROOT_HELPER_PATH" ] && [ ! -L "$ROOT_HELPER_PATH" ]; } \
		|| root_helper_die "$ROOT_HELPER_PATH is missing after install"
	got="$(stat -c '%U:%G %a' "$ROOT_HELPER_PATH")"
	[ "$got" = "root:root 755" ] \
		|| root_helper_die "$ROOT_HELPER_PATH is '$got', expected 'root:root 755'"
	{ [ -f "$ROOT_HELPER_SUDOERS" ] && [ ! -L "$ROOT_HELPER_SUDOERS" ]; } \
		|| root_helper_die "$ROOT_HELPER_SUDOERS is missing after install"
	got="$(stat -c '%U:%G %a' "$ROOT_HELPER_SUDOERS")"
	[ "$got" = "root:root 440" ] \
		|| root_helper_die "$ROOT_HELPER_SUDOERS is '$got', expected 'root:root 440'"
	{ [ -f "$ROOT_HELPER_TOML" ] && [ ! -L "$ROOT_HELPER_TOML" ]; } \
		|| root_helper_die "$ROOT_HELPER_TOML is missing after install"
	got="$(stat -c '%U:%G %a' "$ROOT_HELPER_TOML")"
	[ "$got" = "root:root 644" ] \
		|| root_helper_die "$ROOT_HELPER_TOML is '$got', expected 'root:root 644'"
	visudo -c >/dev/null 2>&1 \
		|| root_helper_die "visudo -c fails after the helper install (check /etc/sudoers and /etc/sudoers.d)"
}
if [ "$(uname -s)" != "Linux" ]; then
	# The root helper is Linux-only (the installer refuses anything else).
	log "root helper: skipped — Linux-only (this is $(uname -s))"
	MAIL_HELPER_LINE="skipped (not Linux)"
else
	log "installing ${ROOT_HELPER_PATH}"
	# The version is the one root resolved before step 4 (K2_VERSION),
	# never read by running the daemon binary as root: it sits in the
	# daemon user's writable home. Plain x.y.z only.
	DAEMON_VERSION="${K2_VERSION#v}"
	[[ "$DAEMON_VERSION" =~ ^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$ ]] \
		|| root_helper_die "K2_VERSION must be a plain x.y.z release (got '${K2_VERSION}')"
	# Cross-check that step 4 installed that version. The binary runs as
	# the daemon user (never root); 0.40.82+ prints and exits without
	# booting. Its answer is only compared, never used.
	INSTALLED_VERSION="$(runuser -u "$K2_RUN_USER" -- timeout 10 "$K2_HOME/.local/bin/k2-daemon" --version 2>/dev/null \
		| sed -n 's/^k2-daemon[[:space:]]\{1,\}\([0-9][^[:space:]]*\).*/\1/p' | head -n1 || true)"
	[ -n "$INSTALLED_VERSION" ] \
		|| root_helper_die "could not read the installed daemon version from $K2_HOME/.local/bin/k2-daemon --version (run as ${K2_RUN_USER})"
	if [ "$INSTALLED_VERSION" != "$DAEMON_VERSION" ]; then
		root_helper_die "K2_VERSION=${K2_VERSION} but the installed daemon reports ${INSTALLED_VERSION}; the helper must match the daemon"
	fi
	# Daemons older than the root helper (it ships from 0.45.4) have no
	# install-root-helper.sh in their release: they get that release's
	# protocol-2 k2-mail-helper exactly as it shipped (this provisioner is
	# fetched from raw main while K2_VERSION may still be older).
	ROOT_HELPER_FROM="0.45.4"
	ROOT_HELPER_MODE="root"
	IFS=. read -r _dv1 _dv2 _dv3 <<<"$DAEMON_VERSION"
	IFS=. read -r _rf1 _rf2 _rf3 <<<"$ROOT_HELPER_FROM"
	if [ "$_dv1" -lt "$_rf1" ] || { [ "$_dv1" -eq "$_rf1" ] && { [ "$_dv2" -lt "$_rf2" ] \
		|| { [ "$_dv2" -eq "$_rf2" ] && [ "$_dv3" -lt "$_rf3" ]; }; }; }; then
		ROOT_HELPER_MODE="legacy"
	fi
	if [ "$ROOT_HELPER_MODE" = "legacy" ]; then
		LEGACY_TMP="$(mktemp -d)" || root_helper_die "mktemp failed"
		LEGACY_URL="https://github.com/Alakazam-211/K2/releases/download/v${DAEMON_VERSION}/install-mail-helper.sh"
		log "daemon v${DAEMON_VERSION} predates the root helper — installing its k2-mail-helper (${LEGACY_URL})"
		curl -fsSL --retry 3 --retry-delay 2 "$LEGACY_URL" -o "$LEGACY_TMP/install-mail-helper.sh" \
			|| root_helper_die "could not download $LEGACY_URL"
		bash "$LEGACY_TMP/install-mail-helper.sh" --version "$DAEMON_VERSION" --user "$K2_RUN_USER" \
			|| root_helper_die "install-mail-helper.sh --version $DAEMON_VERSION failed (output above)"
		ROOT_HELPER_PATH="${K2_INSTALL_ROOT:-}/usr/local/libexec/k2-mail-helper"
		ROOT_HELPER_SUDOERS="${K2_INSTALL_ROOT:-}/etc/sudoers.d/k2-mail-helper"
		for _p in "$ROOT_HELPER_PATH" "$ROOT_HELPER_SUDOERS"; do
			{ [ -f "$_p" ] && [ ! -L "$_p" ]; } || root_helper_die "$_p is missing after the legacy install"
		done
		[ "$(stat -c '%U:%G %a' "$ROOT_HELPER_PATH")" = "root:root 755" ] \
			|| root_helper_die "$ROOT_HELPER_PATH is not root:root 755"
		[ "$(stat -c '%U:%G %a' "$ROOT_HELPER_SUDOERS")" = "root:root 440" ] \
			|| root_helper_die "$ROOT_HELPER_SUDOERS is not root:root 440"
		visudo -c >/dev/null 2>&1 || root_helper_die "visudo -c fails after the legacy helper install"
		MAIL_HELPER_LINE="installed legacy k2-mail-helper (v${DAEMON_VERSION} predates the root helper)"
		log "root helper: ${MAIL_HELPER_LINE}"
	else
	ROOT_HELPER_SRC=""
	if [ -n "${K2_ROOT_HELPER_BIN:-}" ]; then
		[ -f "$K2_ROOT_HELPER_BIN" ] || root_helper_die "K2_ROOT_HELPER_BIN is set but missing: $K2_ROOT_HELPER_BIN"
		ROOT_HELPER_SRC="$K2_ROOT_HELPER_BIN"
	elif [ -f "$SCRIPT_DIR/k2-root-helper" ]; then
		ROOT_HELPER_SRC="$SCRIPT_DIR/k2-root-helper"
	elif [ -f "$SCRIPT_DIR/../target/release/k2-root-helper" ]; then
		ROOT_HELPER_SRC="$SCRIPT_DIR/../target/release/k2-root-helper"
	fi
	# The installer: the copy beside this script, else the signed release's
	# own copy for the same version. Downloaded into a fresh root-only
	# directory, never a fixed /tmp name another user could pre-create.
	ROOT_HELPER_INSTALLER="$SCRIPT_DIR/install-root-helper.sh"
	if [ ! -f "$ROOT_HELPER_INSTALLER" ]; then
		ROOT_HELPER_TMP="$(mktemp -d)" || root_helper_die "mktemp failed"
		ROOT_HELPER_INSTALLER="$ROOT_HELPER_TMP/install-root-helper.sh"
		ROOT_HELPER_INSTALLER_URL="https://github.com/Alakazam-211/K2/releases/download/v${DAEMON_VERSION}/install-root-helper.sh"
		log "fetching install-root-helper.sh (${ROOT_HELPER_INSTALLER_URL})"
		curl -fsSL --retry 3 --retry-delay 2 "$ROOT_HELPER_INSTALLER_URL" -o "$ROOT_HELPER_INSTALLER" \
			|| root_helper_die "could not download $ROOT_HELPER_INSTALLER_URL"
	fi
	ROOT_HELPER_ARGS=(--user "$K2_RUN_USER" --unit "${K2_UNIT_NAME}.service" --scope system)
	if [ -n "$ROOT_HELPER_SRC" ]; then
		bash "$ROOT_HELPER_INSTALLER" --file "$ROOT_HELPER_SRC" "${ROOT_HELPER_ARGS[@]}" \
			|| root_helper_die "install-root-helper.sh --file $ROOT_HELPER_SRC failed (output above)"
		MAIL_HELPER_LINE="installed (local build, daemon v${DAEMON_VERSION})"
	else
		log "no local k2-root-helper — installing the signed v${DAEMON_VERSION} release asset"
		bash "$ROOT_HELPER_INSTALLER" --version "$DAEMON_VERSION" "${ROOT_HELPER_ARGS[@]}" \
			|| root_helper_die "install-root-helper.sh --version $DAEMON_VERSION failed (output above)"
		MAIL_HELPER_LINE="installed (v${DAEMON_VERSION})"
	fi
	verify_root_helper
	log "root helper: ${MAIL_HELPER_LINE}"
	fi
fi

# ── 7b. optional Postgres sidecar bake (`--with-db` / K2_BAKE_DB=1) ──
# Distro packages (postgresql + postgresql-client), NOT postgresql-16
# (jammy/bookworm ship 14/15). Empty cluster. listen_addresses=localhost.
# The daemon reaches Postgres as root ONLY through the root helper's `pg`
# namespace (7a): no k2-pg-helper copy and no second sudoers line any more
# (RH11). User=k2 never apt-gets and never holds a superuser password.
# Mail stays GitHub-tarball at enable; db is bake-apt.
# A box that ALREADY has a Postgres cluster (dtl, edf: postgresql@16 set
# up another way, no k2-pg-helper) is ADOPTED as is: no apt, no initdb, no
# listen_addresses rewrite, no RAM fence (those change a running database
# someone else sized). The daemon's db verbs reach it through the root
# helper's `pg` namespace (7a). Fence it later, after review, with
# `k2-root-helper pg install-ram-fence`. Never re-initialized.
PG_EXISTING=""
if [ "$WITH_DB" = 1 ]; then
	for _conf in /etc/postgresql/*/*/postgresql.conf; do
		[ -f "$_conf" ] && PG_EXISTING="$PG_EXISTING ${_conf#/etc/postgresql/}"
	done
fi
if [ "$WITH_DB" = 1 ] && [ -n "$PG_EXISTING" ]; then
	PG_EXISTING="$(printf '%s' "$PG_EXISTING" | sed 's#/postgresql\.conf##g; s/^ //')"
	log "Postgres: existing cluster(s) adopted, not re-initialized and not reconfigured: ${PG_EXISTING}"
	log "Postgres: review, then fence RAM with: sudo /usr/local/libexec/k2-root-helper pg install-ram-fence"
elif [ "$WITH_DB" = 1 ] && [ "${ROOT_HELPER_MODE:-root}" != "legacy" ] \
	&& [ -x /usr/local/libexec/k2-root-helper ] \
	&& /usr/local/libexec/k2-root-helper version 2>/dev/null | grep -Eq '"pgmajor": ?true'; then
	# 0.46.3: new boxes are born on Postgres 17 (prd-pg17-and-db-upgrade-v1
	# §5, V13). The root helper's `pg add-major` writes the pinned PGDG key
	# and source, the apt pins (PGDG never replaces the distro's packages)
	# and `create_main_cluster = false`, then installs postgresql-17 +
	# client; `pg ensure-cluster` creates 17/main on the first free port of
	# K2's band (5432 here), localhost only, RAM fence, scram, wal_level
	# logical. K2_PG_MAJOR overrides the major (supported ones only; the
	# helper refuses anything else). Air-gapped: install the two .deb files
	# from your mirror first, then re-run.
	PG_NEW_MAJOR="${K2_PG_MAJOR:-17}"
	log "baking Postgres ${PG_NEW_MAJOR} sidecar (PGDG, pinned key) through the root helper"
	/usr/local/libexec/k2-root-helper pg add-major "$PG_NEW_MAJOR" >/dev/null \
		|| die "root helper: pg add-major ${PG_NEW_MAJOR} failed (air-gapped? apt install ./postgresql-${PG_NEW_MAJOR}*.deb ./postgresql-client-${PG_NEW_MAJOR}*.deb, then re-run)"
	/usr/local/libexec/k2-root-helper pg ensure-cluster "$PG_NEW_MAJOR" >/dev/null \
		|| die "root helper: pg ensure-cluster ${PG_NEW_MAJOR} failed"
	systemctl enable postgresql >/dev/null 2>&1 || true
	systemctl daemon-reload
	log "Postgres ${PG_NEW_MAJOR} sidecar baked (cluster ${PG_NEW_MAJOR}/main, RAM fence via k2-root-helper)"
elif [ "$WITH_DB" = 1 ]; then
	# A root helper without `pgmajor` (or the legacy daemon): the distro's
	# Postgres, as before 0.46.3. Databases move to 17 later with
	# `k2 db upgrade` once the helper self-updates.
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
	if [ "${ROOT_HELPER_MODE:-root}" = "legacy" ]; then
		# A daemon older than the root helper still calls k2-pg-helper.
		# Same script as before, but always present (fetched next to the
		# provisioner when it ran alone) and a visudo-checked sudoers line.
		PG_HELPER_SRC="$SCRIPT_DIR/k2-pg-helper"
		if [ ! -f "$PG_HELPER_SRC" ]; then
			PG_TMP="$(mktemp -d)" || die "mktemp failed"
			PG_HELPER_SRC="$PG_TMP/k2-pg-helper"
			curl -fsSL --retry 3 "$RAW_BASE/scripts/k2-pg-helper" -o "$PG_HELPER_SRC" \
				|| die "could not download k2-pg-helper for the legacy daemon"
		fi
		install -d -m 0755 /usr/local/libexec
		install -m 0755 -o root -g root "$PG_HELPER_SRC" /usr/local/libexec/k2-pg-helper
		PG_SUDO_TMP="$(mktemp -d)/k2-pg-helper"
		printf '%s ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper\n' "$K2_RUN_USER" >"$PG_SUDO_TMP"
		visudo -cf "$PG_SUDO_TMP" >/dev/null || die "visudo rejected the k2-pg-helper sudoers line"
		install -m 0440 -o root -g root "$PG_SUDO_TMP" /etc/sudoers.d/k2-pg-helper
		/usr/local/libexec/k2-pg-helper install-ram-fence
		systemctl daemon-reload
		log "Postgres sidecar baked (legacy k2-pg-helper for daemon v${DAEMON_VERSION:-?})"
	else
		# D29: MemoryHigh/Max drop-ins + GUC caps, canned inside the root
		# helper (never follows a link under /etc/postgresql, RH9).
		[ -x /usr/local/libexec/k2-root-helper ] \
			|| die "--with-db needs the root helper from step 7a"
		/usr/local/libexec/k2-root-helper pg install-ram-fence >/dev/null \
			|| die "root helper: pg install-ram-fence failed"
		systemctl daemon-reload
		log "Postgres sidecar baked (unit enabled, empty cluster, RAM fence via k2-root-helper)"
	fi
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
			AUTH_CODE=$(k2_curl "$BASE/cli/auth/whoami?token=$TOKEN" "" "" -s -o /dev/null -w '%{http_code}')
			[ "$AUTH_CODE" = "200" ] && break
		fi
	fi
	sleep 1
	PORT=""
done
[ -n "$PORT" ] || die "daemon not ready with a live token in 90s (journalctl -u ${K2_UNIT_NAME})"

# ── 8b. Apps (beta): prepare the box + install the App engine ───────
# 0.46.3+: Apps are on by default; walled Apps need box prep (k2app user,
# subuids, delegation, nft table, Podman 5) and the guarded App engine,
# both through the root helper from step 7a. Run as the daemon user with
# the owner token the CLI reads from its own ~/.k2/daemon.token (never on
# an argv). Idempotent: prepare converges on a re-run; the engine is
# installed only when the doctor says it isn't answering at this version.
# K2_SKIP_APPS=1 skips it. A daemon that predates the Apps switch (pinned
# K2_VERSION, legacy root helper) is skipped with a line; any other
# failure stops provisioning.
APPS_LINE="skipped"
k2_as_daemon_user() {
	runuser -u "$K2_RUN_USER" -- env -i HOME="$K2_HOME" USER="$K2_RUN_USER" LANG=C.UTF-8 \
		PATH="$K2_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" /usr/local/bin/k2 "$@"
}
if [ "${K2_SKIP_APPS:-}" = "1" ]; then
	log "Apps: skipped (K2_SKIP_APPS=1)"
elif [ "${ROOT_HELPER_MODE:-root}" = "legacy" ]; then
	log "Apps: skipped (daemon v${DAEMON_VERSION:-?} predates the root helper's Apps verbs)"
	APPS_LINE="skipped (legacy daemon)"
else
	APPS_DOC=$(k2_curl "$BASE/cli/app/box?token=$TOKEN" "" "" -sS -w '\n%{http_code}') \
		|| die "Apps: GET /cli/app/box failed (journalctl -u ${K2_UNIT_NAME})"
	APPS_HTTP="${APPS_DOC##*$'\n'}"
	if [ "$APPS_HTTP" = "404" ]; then
		log "Apps: the daemon answers apps_disabled (a daemon older than 0.46.3, or Apps switched off): skipped"
		APPS_LINE="skipped (Apps off on this daemon)"
	elif [ "$APPS_HTTP" != "200" ]; then
		die "Apps: GET /cli/app/box answered HTTP ${APPS_HTTP}: ${APPS_DOC%$'\n'*}"
	else
		log "Apps: preparing the box (k2 app box prepare --install-podman)"
		APPS_PREP_OUT=$(mktemp)
		if ! k2_as_daemon_user app box prepare --install-podman --json >"$APPS_PREP_OUT" 2>&1; then
			cat "$APPS_PREP_OUT" >&2
			die "Apps: box prep failed (output above). Fix it, then re-run, or run as ${K2_RUN_USER}: k2 app doctor ; k2 app box prepare --dry-run"
		fi
		rm -f "$APPS_PREP_OUT"
		APPS_ENGINE_OK=$(k2_curl "$BASE/cli/app/box?token=$TOKEN" "" "" -fsS \
			| python3 -c 'import json,sys; d=json.load(sys.stdin); print("yes" if any(c.get("id")=="engine" and c.get("status")=="ok" for c in d.get("checks",[])) else "no")') \
			|| die "Apps: re-reading the doctor after box prep failed"
		if [ "$APPS_ENGINE_OK" = "yes" ]; then
			log "Apps: the App engine already answers at this version"
		else
			log "Apps: installing the App engine (k2 app engine install)"
			APPS_ENG_OUT=$(mktemp)
			if ! k2_as_daemon_user app engine install --json >"$APPS_ENG_OUT" 2>&1; then
				cat "$APPS_ENG_OUT" >&2
				die "Apps: App engine install failed (output above). Re-run, or run as ${K2_RUN_USER}: k2 app engine install"
			fi
			rm -f "$APPS_ENG_OUT"
		fi
		k2_as_daemon_user app doctor >/dev/null 2>&1 \
			|| log "WARN: Apps: k2 app doctor still reports items to fix (run it as ${K2_RUN_USER}); provisioning continues"
		APPS_LINE="prepared, App engine installed"
	fi
fi

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
	HTTP=$(k2_curl "$BASE/cli/users/add?token=$TOKEN" "$BODY" "" -sS -o "$ADD_RES_FILE" -w '%{http_code}' -X POST)
	if [ "$HTTP" != "200" ]; then
		grep -qi "exist" "$ADD_RES_FILE" \
			|| die "users/add failed ($HTTP): $(cat "$ADD_RES_FILE")"
		log "owner user already exists — continuing (idempotent re-run, password unchanged)"
		OWNER_CREATED=0
	fi
	rm -f "$ADD_RES_FILE"
	ROLE_BODY=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OWNER_USER"],"role":"owner"}))')
	k2_curl "$BASE/cli/users/set-role?token=$TOKEN" "$ROLE_BODY" "" -fsS -X POST >/dev/null \
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
	HTTP=$(k2_curl "$BASE/cli/users/add?token=$TOKEN" "$OPS_BODY" "" -sS -o "$OPS_RES" -w '%{http_code}' -X POST)
	if [ "$HTTP" != "200" ]; then
		grep -qi "exist" "$OPS_RES" || die "ops users/add failed ($HTTP): $(cat "$OPS_RES")"
		log "ops user already exists — leaving its password unchanged"
		OPS_PASSWORD=""
	fi
	rm -f "$OPS_RES"
	OPS_ROLE=$(python3 -c 'import json,os;print(json.dumps({"username":os.environ["K2_OPS_USER"],"role":"owner"}))')
	k2_curl "$BASE/cli/users/set-role?token=$TOKEN" "$OPS_ROLE" "" -fsS -X POST >/dev/null \
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
echo "  apps:      ${APPS_LINE}"
echo "  sandboxes: OFF (Standard-tier host — the Dedicated bootstrap adds them)"
if [ -z "${K2_TUNNEL_TOKEN:-}" ] || [ -z "${K2_OWNER_USER:-}" ]; then
echo ""
echo "  Next (interactive self-host):"
[ -z "${K2_OWNER_USER:-}" ] && echo "    k2 users add <you> --role owner   # prompts for password"
[ -z "${K2_TUNNEL_TOKEN:-}" ] && echo "    k2 connect login                 # pair purchased *.k2.dev subdomain"
echo "  Automation fallback still works: K2_TUNNEL_TOKEN + K2_SUBDOMAIN env."
fi

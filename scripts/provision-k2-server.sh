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
#   # full provision (bake steps are idempotent — safe on a baked image);
#   # no owner login: the k2.dev account holder claims it (step 9):
#   K2_TUNNEL_TOKEN=k2c_... K2_SUBDOMAIN=alice \
#   ./provision-k2-server.sh
#
#   # a deliberate non-App box: a NOT READY `k2 app doctor` is a warning
#   ./provision-k2-server.sh --allow-not-ready
#
#   # self-host interactive path (after install, no K2_TUNNEL_TOKEN):
#   k2 users add alice --role owner --keep-password   # prompts for password
#   k2 connect login                   # k2.dev account → pick subdomain → live
#
# Environment:
#   K2_TUNNEL_TOKEN    K2 Connect tunnel token (automation fallback; from
#                      the subdomains row — prefer `k2 connect login` for
#                      interactive self-host pairing)
#   K2_SUBDOMAIN       subdomain label (alice → alice.k2.dev)
#   K2_RELAYS          optional relay list "host[:port],…" for tunnel.json
#                      (unset: none; the daemon dials relay.k2.dev, then the
#                      list k2.dev serves for the token)
#   K2_OWNER_USER      retired in 0.46.18: set, the run stops before any
#                      work. No run makes an owner login (that takes a
#                      person present); the k2.dev dashboard makes a hosted
#                      server's first owner (K2_HOSTED_CLAIM), and a
#                      self-hosted owner runs k2 users add <you> --role
#                      owner --keep-password from their own SSH login.
#                      K2_OWNER_PASSWORD is ignored.
#   K2_HOSTED_CLAIM    1 = a hosted server: write the root-owned marker
#                      /etc/k2-root-helper/hosted-claim (0600) that lets
#                      the k2.dev dashboard claim its first owner, once
#                      (step 9). Self-hosted runs leave it unset.
#   K2_CALLBACK_URL    control-plane callback POSTed on success (optional)
#   K2_CALLBACK_TOKEN  bearer for the callback (optional)
#   K2_VERSION         pin a daemon release (default: latest)
#   K2_RUN_USER        service account (default: k2)
#   K2_ROOT_HELPER_BIN absolute path to a built k2-root-helper binary.
#                      Else beside this script (scripts/k2-root-helper), else
#                      ../target/release/k2-root-helper from a
#                      `cargo build -p k2-root-helper --release` (a local
#                      build must be the daemon's version), else the
#                      signed GitHub release asset for the SAME version as the
#                      daemon. install-daemon.sh installs it in step 4
#                      (through install-root-helper.sh); step 7a checks it.
#                      Every box (bake and full provision) gets K2's one root
#                      door so its agents can enable hosted mail and the db
#                      sidecar without ops. A failure is a hard failure that
#                      stops provisioning; after install the helper must be
#                      root:root 0755, its sudoers file 0440, daemon.toml
#                      0644, and `visudo -c` must pass. The step never
#                      enables mail, touches DNS, or edits other sudoers.
#   K2_SKIP_APPS       1 = skip step 8b (Apps (beta) box prep + App engine).
#                      Default: a full provision prepares the box for walled
#                      Apps and installs the App engine (0.46.3+ daemons),
#                      gives agent sessions their own memory budget (the
#                      agents leaf: --cores-mem-min sized by the box's RAM,
#                      then one k2-daemon restart while there are no guests
#                      yet) and requires `k2 app doctor` READY: NOT READY
#                      stops provisioning and names the failing checks.
#                      READY turns on automatic box prep (appsAutoPrepare)
#                      so the box stays ready after every update.
#   K2_ALLOW_NOT_READY 1 = (or --allow-not-ready) a deliberate non-App box:
#                      a NOT READY doctor at the end of 8b is a warning
#                      instead of a stop. Box prep and engine failures still
#                      stop provisioning (K2_SKIP_APPS=1 skips 8b entirely).
#   K2_CORES_MEM_MIN_MIB  the protected floor for K2's cores (MiB), passed
#                      to `k2 app box prepare --cores-mem-min`. Default:
#                      about 1/16 of the box's RAM, at least 2 GiB, at most
#                      8 GiB (2 GiB at 32 GB or less, 4 GiB at 64 GB, 8 GiB
#                      at 128 GB and up); a box under ~4 GiB gets a quarter
#                      of its RAM (the root helper refuses more than half).
#   K2_SKIP_SEW        1 = skip step 9c (Sew chat + voice, Home chat, voice
#                      rooms). Default: every full provision installs Sew
#                      (identity k2, K2's pinned version) and voice, takes
#                      the first Sew backup, turns on homeChat and
#                      voiceRoomsPreview and requires a clean `k2 hostchat
#                      doctor`; any failure stops provisioning before the
#                      callback. An existing Sew with another identity
#                      (e.g. standalone) is never converted: provisioning
#                      stops (that is a migration).
#   K2_SKIP_RECORDER   1 = skip step 9c.8 (Sew's call recorder, install
#                      only). Default: a box with 8 GiB of RAM or more where
#                      the speech model fits gets the recorder installed and
#                      recording left OFF (a person turns it on in Settings).
#                      Never fails provisioning: a box that can't take it is
#                      one log line.
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
ALLOW_NOT_READY=0
# Must still treat `--bake` as argv[1] (historical). Extra flags are
# parsed without breaking that; `K2_BAKE_DB=1` is the env alias.
for _arg in "$@"; do
	case "$_arg" in
		--bake) BAKE_ONLY=1 ;;
		--with-db) WITH_DB=1 ;;
		--allow-not-ready) ALLOW_NOT_READY=1 ;;
	esac
done
[ "${K2_BAKE_DB:-}" = "1" ] && WITH_DB=1
[ "${K2_ALLOW_NOT_READY:-}" = "1" ] && ALLOW_NOT_READY=1

# The helper python snippets read these from the environment.
export K2_TUNNEL_TOKEN="${K2_TUNNEL_TOKEN:-}"
export K2_SUBDOMAIN="${K2_SUBDOMAIN:-}"
export K2_OWNER_USER="${K2_OWNER_USER:-}"
export K2_OWNER_PASSWORD="${K2_OWNER_PASSWORD:-}"
export K2_CALLBACK_URL="${K2_CALLBACK_URL:-}"
export K2_CALLBACK_TOKEN="${K2_CALLBACK_TOKEN:-}"

log() { printf '\033[1;36m[provision]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[provision] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# ── GitHub fetch: begin (tests/cli/update_github_redirects.sh runs this block alone) ──
# The hosts a fetch from GitHub follows a redirect to: raw.githubusercontent.com
# (RAW_BASE: a renamed repository's raw URLs redirect to the new name) and
# GitHub's own four (api.github.com, github.com, objects.githubusercontent.com,
# release-assets.githubusercontent.com). Curl can't limit -L to a list of
# hosts, so provision_github_fetch never uses -L: it follows each hop itself,
# https only, after checking the host, and refuses a redirect anywhere else
# before that address is contacted. The same walk as the k2 CLI's
# _k2_github_curl (cli/src/update.sh) and install-daemon.sh's github_fetch;
# this script carries its own copy.
_PROV_GITHUB_HOSTS="raw.githubusercontent.com api.github.com github.com objects.githubusercontent.com release-assets.githubusercontent.com"
# The most redirects one fetch follows.
_PROV_GITHUB_MAX_REDIRS=5

# _prov_github_ok <url>: 0 when <url> is https on one of those hosts, with no
# name before an @ and no port.
_prov_github_ok() {
	local rest host h
	case "$1" in https://*) ;; *) return 1 ;; esac
	rest="${1#https://}"
	host="${rest%%[/?#]*}"
	for h in $_PROV_GITHUB_HOSTS; do
		[ "$host" = "$h" ] && return 0
	done
	return 1
}

# _prov_github_where <url>: the scheme and host of <url>, printable characters
# only, for an error line (never the path or the query).
_prov_github_where() {
	local rest
	case "$1" in
		*://*) rest="${1#*://}"; rest="${1%%://*}://${rest%%[/?#]*}" ;;
		*) rest="${1%%[/?#]*}" ;;
	esac
	printf '%s' "$rest" | LC_ALL=C tr -cd '[:graph:]'
}

# provision_github_fetch <url> <file> [curl args...]: GET <url> into <file>
# (each hop with those curl args too, such as --retry), following redirects
# on those hosts one hop at a time (at most _PROV_GITHUB_MAX_REDIRS), https
# only, also when a ~/.curlrc says -L. Every GitHub fetch in this script goes
# through it; none uses curl -L. Sends no token: plain curl, no
# config, no Authorization header. Returns 1 when curl fails (curl's own line
# on stderr). A redirect off the list, to anything but https, or past the cap
# prints one line on stderr and returns 2; nothing is sent to that address.
provision_github_fetch() {
	local url="$1" file="$2" hops=0 next
	shift 2
	while :; do
		next=$(curl --no-location --proto =https -fsS "$@" -o "$file" -w '%{redirect_url}' "$url") || return 1
		[ -n "$next" ] || return 0
		hops=$((hops + 1))
		if [ "$hops" -gt "$_PROV_GITHUB_MAX_REDIRS" ]; then
			echo "Error: GitHub redirected more than $_PROV_GITHUB_MAX_REDIRS times (the last to $(_prov_github_where "$next")); stopped there" >&2
			return 2
		fi
		case "$next" in
			https://*) ;;
			*) echo "Error: GitHub redirected to $(_prov_github_where "$next"), which is not https; nothing was sent there" >&2
			   return 2 ;;
		esac
		if ! _prov_github_ok "$next"; then
			echo "Error: GitHub redirected to $(_prov_github_where "$next"), which is not GitHub; nothing was sent there" >&2
			return 2
		fi
		url="$next"
	done
}
# ── GitHub fetch: end ──

# The agents leaf's protected floor for K2's cores, in MiB, from MemTotal
# in KiB (/proc/meminfo): about 1/16 of the RAM rounded to whole GiB, at
# least 2 GiB, at most 8 GiB (2 GiB at 32 GB or less, 4 GiB at 64 GB, 8 GiB
# at 128 GB and up). The root helper refuses a floor above half the box, so
# a box under ~4 GiB gets a quarter of its RAM. Same rule as the daemon's
# `k2_core::agents_leaf::cores_floor_mib` (the owner's one-click card).
k2_cores_mem_min_mib() {
	local kib="${1:-0}" ram_mib g n
	case "$kib" in ''|*[!0-9]*) return 1 ;; esac
	ram_mib=$(( kib / 1024 ))
	[ "$ram_mib" -gt 0 ] || return 1
	g=$(( (ram_mib + 8192) / 16384 ))
	[ "$g" -ge 2 ] || g=2
	[ "$g" -le 8 ] || g=8
	n=$(( g * 1024 ))
	[ "$n" -le $(( ram_mib / 2 )) ] || n=$(( ram_mib / 4 ))
	printf '%s\n' "$n"
}

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
# Step 9: this run makes no owner (claimed from k2.dev); stop before any work.
if [ "$BAKE_ONLY" != 1 ] && [ -n "${K2_OWNER_USER:-}" ]; then
	die "K2_OWNER_USER is set, but this script no longer makes an owner login: a server's first owner is claimed from the k2.dev dashboard of the account it is linked to (POST /cli/users/claim-first-owner). Unset K2_OWNER_USER and re-run. Self-hosted: after this run, k2 users add <you> --role owner --keep-password from your own SSH login"
elif [ "$BAKE_ONLY" != 1 ] && [ -n "${K2_OPS_USER:-}" ]; then
	die "K2_OPS_USER is set, but this script no longer makes the website-management login: the k2.dev claim makes it, an owner, with the first owner (POST /cli/users/claim-first-owner, opsUsername). Unset K2_OPS_USER and re-run"
fi

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
	provision_github_fetch "https://github.com/fatedier/frp/releases/download/v${FRP_VERSION}/${FRP_DIR}.tar.gz" \
		"$TMP/frp.tar.gz"
	tar -xzf "$TMP/frp.tar.gz" -C "$TMP"
	install -m 0755 "$TMP/${FRP_DIR}/frpc" /usr/local/bin/frpc
	rm -rf "$TMP"
fi

# ── 4. daemon binary + root helper (install-daemon.sh) ──────────────
# Root decides the version ONCE, here: install-daemon.sh installs the
# daemon AND the root helper at that version (step 7a only checks the
# result). Root never executes the installed daemon (it lives in the
# daemon user's writable home) to learn it.
# Plain x.y.z only: it becomes part of download URLs.
if [ -z "$K2_VERSION" ]; then
	log "resolving the latest daemon release"
	_MANIFEST_TMP="$(mktemp)" || die "mktemp failed"
	provision_github_fetch "https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json" \
		"$_MANIFEST_TMP" --retry 3 --retry-delay 2 \
		|| { rm -f "$_MANIFEST_TMP"; die "could not resolve the latest daemon release; set K2_VERSION=<x.y.z>"; }
	K2_VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$_MANIFEST_TMP" | head -n1)"
	rm -f "$_MANIFEST_TMP"
fi
K2_VERSION="${K2_VERSION#v}"
[[ "$K2_VERSION" =~ ^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$ ]] \
	|| die "K2_VERSION must look like 0.45.1 (got '$K2_VERSION')"
INSTALLER="$SCRIPT_DIR/install-daemon.sh"
if [ ! -f "$INSTALLER" ]; then
	INSTALLER="$(mktemp -d)/install-daemon.sh"
	chmod 0755 "$(dirname "$INSTALLER")"
	log "fetching install-daemon.sh from the repo"
	provision_github_fetch "$RAW_BASE/scripts/install-daemon.sh" "$INSTALLER" \
		|| die "could not fetch $RAW_BASE/scripts/install-daemon.sh"
	chmod +x "$INSTALLER"
fi
# The root helper's source: a local build wins (K2_ROOT_HELPER_BIN, beside
# this script, ../target/release; it must be the daemon's version), else
# install-daemon.sh installs the signed release asset of the same version.
ROOT_HELPER_SRC=""
if [ -n "${K2_ROOT_HELPER_BIN:-}" ]; then
	[ -f "$K2_ROOT_HELPER_BIN" ] || die "root helper: K2_ROOT_HELPER_BIN is set but missing: $K2_ROOT_HELPER_BIN"
	ROOT_HELPER_SRC="$K2_ROOT_HELPER_BIN"
elif [ -f "$SCRIPT_DIR/k2-root-helper" ]; then
	ROOT_HELPER_SRC="$SCRIPT_DIR/k2-root-helper"
elif [ -f "$SCRIPT_DIR/../target/release/k2-root-helper" ]; then
	ROOT_HELPER_SRC="$SCRIPT_DIR/../target/release/k2-root-helper"
fi
# Run as root with --user: the daemon's files are written as ${K2_RUN_USER}
# (install-daemon.sh drops to that user for them), the root helper is
# installed for this user, this unit, system scope, before the daemon is
# swapped. A failure on either stops provisioning here.
INSTALL_ARGS=(--user "$K2_RUN_USER" --no-service --version "$K2_VERSION"
	--bin-dir "$K2_HOME/.local/bin" --unit "${K2_UNIT_NAME}.service" --scope system)
[ -z "$ROOT_HELPER_SRC" ] || INSTALL_ARGS+=(--root-helper-file "$ROOT_HELPER_SRC")
log "installing k2-daemon and the root helper (verified) for ${K2_RUN_USER}"
sh "$INSTALLER" "${INSTALL_ARGS[@]}" \
	|| die "install-daemon.sh failed (output above) — provisioning stopped. Fix it and re-run this script (idempotent)."

# ── 5. k2 CLI ────────────────────────────────────────────────────────
if [ -f "$SCRIPT_DIR/../cli/k2" ]; then
	install -m 0755 "$SCRIPT_DIR/../cli/k2" /usr/local/bin/k2
else
	# The CLI from the same release as the daemon, not main: a release daemon
	# with main's CLI is a mix no release was tested as.
	log "fetching k2 CLI for v$K2_VERSION"
	provision_github_fetch "https://raw.githubusercontent.com/Alakazam-211/K2/v$K2_VERSION/cli/k2" \
		/usr/local/bin/k2 --retry 3 --retry-delay 2 \
		|| die "could not fetch the k2 CLI for v$K2_VERSION"
	chmod 0755 /usr/local/bin/k2
fi
# The daemon self-stages the CLI at every boot (cli_stage, 0.40.41+) so
# updates carry it forward — that only works if the DAEMON USER can write
# this file. Root-owned k2 = CLI silently frozen at provision-day version
# (bit two customer boxes, 2026-07-15).
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
# K2_RELAYS is "host[:port],host[:port]". 0.46.6: no default list. Unset,
# relays are left as they are (none on a fresh box): the daemon dials its
# built-in relay.k2.dev, then whatever k2.dev serves for the token. An
# explicit K2_RELAYS writes that list; an explicit empty value clears it.
# The primary MUST stay element 0 so a config push to an already-connected
# box doesn't move it until a real failure.
_relays_raw = os.environ.get("K2_RELAYS")
if _relays_raw is not None:
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
# helper current by itself (signed self-update). Step 4's install-daemon.sh
# installed it through install-root-helper.sh, before the daemon:
# /usr/local/libexec/k2-root-helper root:root 0755, the one sudoers line
# /etc/sudoers.d/k2-root-helper (visudo-checked, 0440) and
# /etc/k2-root-helper/daemon.toml (who the daemon is: this user, this
# unit, system scope), from a local build (K2_ROOT_HELPER_BIN, beside this
# script, ../target/release) or the signed release asset for the SAME
# version as the daemon. This step checks that result (one install path
# for provisioning and install-daemon.sh alone).
# FAIL LOUD: any failure here stops provisioning (never skipped, never
# warned past). This step does not enable hosted mail, touch DNS, or
# write any sudoers file.
# K2_INSTALL_ROOT is the DESTDIR-style prefix for the hermetic tests
# (tests/cli/provision_mail_helper.sh). Production leaves it empty.
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
	log "checking ${ROOT_HELPER_PATH}"
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
		provision_github_fetch "$LEGACY_URL" "$LEGACY_TMP/install-mail-helper.sh" --retry 3 --retry-delay 2 \
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
	# Step 4 installed it (install-daemon.sh, at the daemon's version):
	# check the result here and never install a second time. A helper a
	# package manager owns is left alone, as install-daemon.sh and the
	# daemon's auto-heal leave it.
	if [ -L "$ROOT_HELPER_PATH" ]; then
		MAIL_HELPER_LINE="left alone (managed by the package manager)"
	else
		verify_root_helper
		# The build marker (read, never executed): the daemon's version or newer.
		HELPER_VERSION="$(LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=' "$ROOT_HELPER_PATH" 2>/dev/null \
			| head -n1 | sed 's/^K2_ROOT_HELPER_BUILD v=\([0-9.]*\) proto=$/\1/' || true)"
		[[ "$HELPER_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
			|| root_helper_die "$ROOT_HELPER_PATH carries no readable build marker"
		IFS=. read -r _hv1 _hv2 _hv3 <<<"$HELPER_VERSION"
		IFS=. read -r _dv1 _dv2 _dv3 <<<"$DAEMON_VERSION"
		if [ "$_hv1" -lt "$_dv1" ] || { [ "$_hv1" -eq "$_dv1" ] && { [ "$_hv2" -lt "$_dv2" ] \
			|| { [ "$_hv2" -eq "$_dv2" ] && [ "$_hv3" -lt "$_dv3" ]; }; }; }; then
			root_helper_die "the root helper is v${HELPER_VERSION}, older than the daemon v${DAEMON_VERSION}"
		fi
		if [ -n "${ROOT_HELPER_SRC:-}" ]; then
			MAIL_HELPER_LINE="installed (local build v${HELPER_VERSION}, daemon v${DAEMON_VERSION})"
		else
			MAIL_HELPER_LINE="installed (v${HELPER_VERSION})"
		fi
	fi
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
# A box that ALREADY has a Postgres cluster (two customer boxes: postgresql@16 set
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
			provision_github_fetch "$RAW_BASE/scripts/k2-pg-helper" "$PG_HELPER_SRC" --retry 3 \
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
# Also after 8b's restart (the agents leaf): sets PORT, TOKEN and BASE.
k2_wait_ready() {
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
}
log "waiting for daemon readiness (fresh port + token)"
k2_wait_ready

# The k2 CLI as the daemon user, with the owner token it reads from its own
# ~/.k2/daemon.token (never on an argv). 8b and 9c use it.
# K2_CLI_TIMEOUT=<limit> (timeout(1) syntax) bounds one call (9c's long steps).
k2_as_daemon_user() {
	local _bound=()
	[ -z "${K2_CLI_TIMEOUT:-}" ] || _bound=(timeout --kill-after=30s "$K2_CLI_TIMEOUT")
	# ${a[@]+…}: an empty array under set -u is fine on any bash (<4.4 too).
	${_bound[@]+"${_bound[@]}"} runuser -u "$K2_RUN_USER" -- env -i HOME="$K2_HOME" USER="$K2_RUN_USER" LANG=C.UTF-8 \
		PATH="$K2_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" /usr/local/bin/k2 "$@"
}

# ── 8b. Apps (beta): prepare the box + install the App engine ───────
# 0.46.3+: Apps are on by default; walled Apps need box prep (k2app user,
# subuids, delegation, nft table, Podman 5) and the guarded App engine,
# both through the root helper from step 7a. Run as the daemon user with
# the owner token the CLI reads from its own ~/.k2/daemon.token (never on
# an argv). Idempotent: prepare converges on a re-run; the engine is
# installed only when the doctor says it isn't answering at this version.
# The agents leaf: prepare passes --cores-mem-min (k2_cores_mem_min_mib,
# about 1/16 of RAM), and the daemon is restarted once here so its unit
# starts in k2cores.slice with Delegate=yes (no guests are signed in yet,
# so nobody is signed out). At the end `k2 app doctor` must say READY:
# NOT READY stops provisioning with the failing checks, unless
# --allow-not-ready / K2_ALLOW_NOT_READY=1 (a deliberate non-App box).
# READY: automatic box prep goes on (`k2 app box auto-prepare on`,
# appsAutoPrepare) so the box stays ready after every update.
# K2_SKIP_APPS=1 skips it. A daemon that predates the Apps switch (pinned
# K2_VERSION, legacy root helper) is skipped with a line; any other
# failure stops provisioning.
APPS_LINE="skipped"
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
		# The agents leaf (S17): a protected floor for K2's cores, sized by
		# the box's RAM; it takes effect at the daemon's next start (below).
		APPS_CORES_MIB="${K2_CORES_MEM_MIN_MIB:-}"
		if [ -z "$APPS_CORES_MIB" ]; then
			APPS_CORES_MIB=$(k2_cores_mem_min_mib "$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)") \
				|| die "Apps: could not read MemTotal from /proc/meminfo to size the cores floor (set K2_CORES_MEM_MIN_MIB)"
		fi
		case "$APPS_CORES_MIB" in ''|*[!0-9]*) die "K2_CORES_MEM_MIN_MIB must be a number of MiB (got '${APPS_CORES_MIB}')" ;; esac
		log "Apps: preparing the box (k2 app box prepare --install-podman --cores-mem-min ${APPS_CORES_MIB})"
		APPS_PREP_OUT=$(mktemp)
		if ! k2_as_daemon_user app box prepare --install-podman --cores-mem-min "$APPS_CORES_MIB" --json >"$APPS_PREP_OUT" 2>&1; then
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
		# The agents leaf needs the daemon's unit started with the cores
		# drop-in (Delegate=yes). Restart it now: there are no guests or
		# agent sessions yet, so nobody is signed out.
		APPS_LEAF=$(k2_curl "$BASE/cli/app/box?token=$TOKEN" "" "" -fsS \
			| python3 -c 'import json,sys; d=json.load(sys.stdin); print(next((c.get("status") or "unknown" for c in d.get("checks",[]) if c.get("id")=="agentsLeaf"), "absent"))') \
			|| die "Apps: reading the agents leaf from the doctor failed"
		if [ "$APPS_LEAF" = "ok" ] || [ "$APPS_LEAF" = "absent" ]; then
			log "Apps: agent sessions already have their own memory budget"
		else
			log "Apps: restarting ${K2_UNIT_NAME} so agent sessions get their own memory budget (setup: no guests yet)"
			systemctl restart "$K2_UNIT_NAME"
			k2_wait_ready
			APPS_LEAF=$(k2_curl "$BASE/cli/app/box?token=$TOKEN" "" "" -fsS \
				| python3 -c 'import json,sys; d=json.load(sys.stdin); print(next((c.get("status") or "unknown" for c in d.get("checks",[]) if c.get("id")=="agentsLeaf"), "absent"))') \
				|| die "Apps: reading the agents leaf after the restart failed"
			[ "$APPS_LEAF" = "ok" ] \
				|| log "WARN: Apps: agent sessions still share the daemon's memory after the restart (k2 app doctor as ${K2_RUN_USER}: agentsLeaf)"
		fi
		# The doctor's verdict decides: NOT READY stops provisioning and
		# names the failing checks, unless this is a deliberate non-App box
		# (--allow-not-ready / K2_ALLOW_NOT_READY=1).
		APPS_DOC_OUT=$(mktemp)
		k2_curl "$BASE/cli/app/box?token=$TOKEN" "" "" -fsS >"$APPS_DOC_OUT" \
			|| die "Apps: GET /cli/app/box failed after box prep (journalctl -u ${K2_UNIT_NAME})"
		APPS_FAILING=$(python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
if d.get("ready") is True:
    sys.exit(0)
gate = d.get("blocking")
rows = [c for c in d.get("checks", []) if (c.get("id") in gate if isinstance(gate, list) else c.get("status") not in ("ok", "warn"))]
out = []
for c in rows:
    t = "%s (%s): %s" % (c.get("id"), c.get("status"), c.get("detail") or "")
    if c.get("fix"):
        t += " [fix: %s]" % c["fix"]
    out.append(t)
print("; ".join(out) or "the doctor says not ready without naming a check")
' "$APPS_DOC_OUT") || die "Apps: the doctor's answer is unreadable: $(cat "$APPS_DOC_OUT")"
		# 0.46.16+ daemons take `k2 app box auto-prepare on` (the doctor
		# names its route); older ones keep the owner's Settings switch.
		APPS_AUTO_ROUTE=$(python3 -c 'import json,sys; print((json.load(open(sys.argv[1])).get("autoPrepare") or {}).get("setRoute") or "")' "$APPS_DOC_OUT") \
			|| die "Apps: the doctor's answer is unreadable: $(cat "$APPS_DOC_OUT")"
		rm -f "$APPS_DOC_OUT"
		if [ -z "$APPS_FAILING" ]; then
			log "Apps: k2 app doctor: READY"
			# Keep it ready after every update (appsAutoPrepare, the owner's
			# switch; this provisioner acts as the owner).
			if [ -n "$APPS_AUTO_ROUTE" ]; then
				APPS_AUTO_OUT=$(mktemp)
				if ! k2_as_daemon_user app box auto-prepare on --json >"$APPS_AUTO_OUT" 2>&1; then
					cat "$APPS_AUTO_OUT" >&2
					die "Apps: turning on automatic box prep failed (output above). Re-run, or run as ${K2_RUN_USER}: k2 app box auto-prepare on"
				fi
				rm -f "$APPS_AUTO_OUT"
				APPS_AUTO_LINE="automatic prep on"
			else
				log "Apps: this daemon predates k2 app box auto-prepare on: automatic prep stays as set (Settings → Cores → Apps → This server)"
				APPS_AUTO_LINE="automatic prep unchanged (older daemon)"
			fi
			APPS_LINE="ready (box prepared, App engine installed, cores floor ${APPS_CORES_MIB} MiB, ${APPS_AUTO_LINE})"
		elif [ "$ALLOW_NOT_READY" = 1 ]; then
			# A deliberate non-App box: automatic prep is left off.
			log "WARN: Apps: k2 app doctor: NOT READY (allowed: --allow-not-ready): ${APPS_FAILING}"
			APPS_LINE="NOT READY (allowed): ${APPS_FAILING}"
		else
			die "Apps: k2 app doctor: NOT READY. Failing checks: ${APPS_FAILING}. Fix them and re-run (idempotent), or for a box that won't run Apps re-run with --allow-not-ready (K2_ALLOW_NOT_READY=1)"
		fi
	fi
fi

# ── 9. first owner user ──────────────────────────────────────────────
# This step makes no owner (0.46.18). Since 0.46.15 the owner token can't
# make an owner login without this computer's own user check, a person
# present, and an automated run is not one. A hosted server's first
# owners come from k2.dev: once this run is done and /boot-status is
# ready, the k2.dev dashboard's backend makes the buyer's owner login (the
# initial password it keeps for the buyer's reveal) and the
# website-management login (k2cloud, its password made by the server and
# returned to k2.dev) in one signed call: POST /cli/users/claim-first-owner,
# over the tunnel. The daemon opens that door only on a hosted server: a
# box with the hosted-claim marker below, which only root can write. The
# first claim burns the door for good (the daemon records it; deleting
# owners never reopens it), and the daemon never removes the marker.
# A self-hosted server's owner makes theirs from their own SSH login:
# k2 users add <you> --role owner --keep-password. K2_OWNER_USER and
# K2_OPS_USER stop the run before any work (above).
#
# K2_HOSTED_CLAIM=1 (hosted provisioning only; cloud-init sets it): write
# the marker /etc/k2-root-helper/hosted-claim, root:root 0600, in a
# root-owned 0755 folder. Self-hosted runs never write it.
HOSTED_CLAIM_DIR=/etc/k2-root-helper
HOSTED_CLAIM_MARKER="$HOSTED_CLAIM_DIR/hosted-claim"
if [ "${K2_HOSTED_CLAIM:-}" = 1 ]; then
	install -d -m 0755 -o root -g root "$HOSTED_CLAIM_DIR" || die "could not make $HOSTED_CLAIM_DIR"
	HOSTED_CLAIM_TMP=$(mktemp) || die "mktemp failed"
	printf 'K2 hosted first-owner claim marker (provision-k2-server.sh)\n' >"$HOSTED_CLAIM_TMP"
	install -m 0600 -o root -g root "$HOSTED_CLAIM_TMP" "$HOSTED_CLAIM_MARKER" || die "could not write $HOSTED_CLAIM_MARKER"
	rm -f "$HOSTED_CLAIM_TMP"
	log "hosted-claim marker written ($HOSTED_CLAIM_MARKER, root 0600): the k2.dev dashboard can claim this server's first owner, once"
fi
log "first owner: none made here; the k2.dev dashboard makes the owner and website-management logins (claim-first-owner; self-hosted: k2 users add <you> --role owner --keep-password, from your own SSH login)"

# ── 9c. Sew (chat + voice) + Home chat + voice rooms ───────────────
# Every server gets Sew by default (Rosson, 2026-11-17: "Sew should be set
# up by default for every server"). Runs after the daemon is ready (8),
# the root helper is checked (7a; the Sew/voice install goes through it)
# and the owner exists (9), and before the callback (10) flips the server
# online, so a failure here is a provisioning failure, not a half-set box.
# The CLI runs as the daemon user with a clean env (k2_as_daemon_user: the
# owner token from its own ~/.k2/daemon.token, never an agent's). State is
# read from `k2 hostchat status --json` (its last line). In order:
#   1. identity check: NEVER converts an existing Sew. state not-installed
#      → install it; identity k2 → skip 2–3; any other state with another
#      identity (e.g. degraded + standalone) → stop: that is a migration.
#      Unreadable status → stop (never guess).
#   2. hostchat preflight → first line `ready`
#   3. hostchat enable --identity k2 (20 min). No --tarball: enable
#      downloads the Sew release THIS K2 pins (sha256 built into K2), never
#      latest; installedVersion must then equal pinnedVersion.
#   4. hostchat backup — the first backup, right after enable and before
#      media (30 min; skipped when the last one is under 1 h old)
#   5. hostchat media install (15 min; skipped if voice.state is running),
#      then voice.state must be running
#   6. settings homeChat true + voiceRoomsPreview true — without
#      voiceRoomsPreview the app joins a call but never starts voice
#   7. hostchat doctor --json → verdict.failed must be []
#   8. (9c.8, prd-home-call-recording-v1 §7) hostchat recording preflight;
#      when `ready`, hostchat recording install (15 min), then status must
#      say installed. INSTALL ONLY: it never turns recording on. Under
#      8 GiB, a model that doesn't fit, or any failure here is one log line
#      and provisioning goes on (never fatal). K2_SKIP_RECORDER=1 skips it.
# then /boot-status must list home-chat-v1 within 60 s (voice-rooms-v1
# missing is only a NOTE: voice rooms are Pro-only). K2 0.46.16 makes the
# daemon default homeChat/voiceRoomsPreview ON once Sew is linked with
# identity k2; the switches then become no-ops, but the Sew install stays
# a provisioning step (it needs the root helper). Re-runnable: each step
# converges. K2_SKIP_SEW=1 skips it. A legacy daemon (pre-root-helper) is
# skipped with a line; any other failure stops provisioning (FAIL LOUD).
SEW_LINE="skipped"
REC_LINE="skipped (no Sew)"
sew_die() { # <step> [output file]: print the step's output, stop provisioning
	[ -z "${2:-}" ] || { cat "$2" >&2; rm -f "$2"; }
	die "sew step failed: $1 — provisioning stopped. Fix it and re-run this script (idempotent), or run as ${K2_RUN_USER}: k2 hostchat doctor"
}
# sew_load_status: SEW_ST = the last line of `k2 hostchat status --json`.
# A failed or timed-out call, or a last line that is not a JSON object,
# stops provisioning: never guess whether Sew is installed (a guess could
# run enable over an existing Sew). Re-read after every mutating step.
sew_load_status() {
	local _rc=0
	SEW_ST="$(K2_CLI_TIMEOUT=2m k2_as_daemon_user hostchat status --json 2>/dev/null | tail -n1)" || _rc=$?
	{ [ "$_rc" = 0 ] && python3 -I -c 'import json,sys
sys.exit(0 if isinstance(json.loads(sys.argv[1]), dict) else 1)' "$SEW_ST" 2>/dev/null; } \
		|| sew_die "hostchat status --json unreadable; refusing to guess whether Sew is installed"
}
# sew_field <status json> <dotted.path>: the field, "" when missing/null.
sew_field() {
	python3 -I -c 'import json,sys
try: d=json.loads(sys.argv[1])
except ValueError: d={}
for p in sys.argv[2].split("."):
    d=d.get(p) if isinstance(d,dict) else None
print("" if d is None else d)' "$1" "$2" 2>/dev/null || true
}
# sew_timed <limit> <label> <step> <cli args...>: run a long CLI step under a
# time limit; a timeout or failure stops provisioning with that step named.
sew_timed() {
	local _limit="$1" _label="$2" _step="$3" _rc=0
	shift 3
	K2_CLI_TIMEOUT="$_limit" k2_as_daemon_user "$@" >"$SEW_OUT" 2>&1 || _rc=$?
	case "$_rc" in
		0) ;;
		124|137) sew_die "${_step} timed out after ${_label}" "$SEW_OUT" ;;
		*) sew_die "${_step} (exit ${_rc})" "$SEW_OUT" ;;
	esac
}
if [ "${K2_SKIP_SEW:-}" = "1" ]; then
	log "Sew: skipped (K2_SKIP_SEW=1)"
	SEW_LINE="skipped (K2_SKIP_SEW=1)"
elif [ "${ROOT_HELPER_MODE:-}" != "root" ]; then
	log "Sew: skipped (no root helper on this box: ${MAIL_HELPER_LINE:-none})"
	SEW_LINE="skipped (no root helper)"
else
	SEW_OUT=$(mktemp)
	log "Sew [1/7]: identity check"
	# Sew is "not installed" ONLY when status says state not-installed
	# (K2 0.46.14: installedVersion/identityProvider null, pinnedVersion set).
	# Any other state (installed, degraded, …) without identity k2 is an
	# existing Sew: refuse (e.g. sewlab: degraded + standalone).
	sew_load_status
	SEW_STATE="$(sew_field "$SEW_ST" state)"
	SEW_INST="$(sew_field "$SEW_ST" installedVersion)"
	SEW_IDP="$(sew_field "$SEW_ST" identityProvider)"
	if [ "$SEW_STATE" != "not-installed" ] && [ "$SEW_IDP" != "k2" ]; then
		sew_die "Sew is installed with identity ${SEW_IDP:-unknown}; changing it is a migration, not a provisioning step"
	fi
	if [ "$SEW_STATE" != "not-installed" ]; then
		log "Sew [2-3/7]: Sew ${SEW_INST:-?} already installed with identity k2 (state ${SEW_STATE:-?}) — preflight and enable skipped"
	else
		log "Sew [2/7]: hostchat preflight"
		k2_as_daemon_user hostchat preflight >"$SEW_OUT" 2>&1 || true
		[ "$(head -n1 "$SEW_OUT")" = "ready" ] || sew_die "hostchat preflight (first line is not 'ready')" "$SEW_OUT"
		log "Sew [3/7]: hostchat enable --identity k2 (K2's pinned Sew)"
		sew_timed 20m "20 min" "hostchat enable --identity k2" hostchat enable --identity k2
	fi
	sew_load_status
	[ "$(sew_field "$SEW_ST" identityProvider)" = "k2" ] \
		|| sew_die "hostchat enable (status identityProvider is '$(sew_field "$SEW_ST" identityProvider)', not k2)"
	SEW_INST="$(sew_field "$SEW_ST" installedVersion)"
	SEW_PIN="$(sew_field "$SEW_ST" pinnedVersion)"
	log "Sew: installedVersion ${SEW_INST:-?} pinnedVersion ${SEW_PIN:-?}"
	{ [ -n "$SEW_INST" ] && [ "$SEW_INST" = "$SEW_PIN" ]; } \
		|| sew_die "Sew version check (installedVersion '${SEW_INST}' is not K2's pinnedVersion '${SEW_PIN}')"
	log "Sew [4/7]: hostchat backup (first backup)"
	SEW_AGE="$(sew_field "$SEW_ST" backup.ageMs)"
	SEW_AGE="${SEW_AGE%%.*}"
	if [[ "$SEW_AGE" =~ ^[0-9]+$ ]] && [ "$SEW_AGE" -lt 3600000 ]; then
		log "Sew: last backup is $((SEW_AGE / 60000)) min old — skipped"
	else
		sew_timed 30m "30 min" "hostchat backup" hostchat backup
		grep -q '"id"' "$SEW_OUT" || sew_die "hostchat backup (no backup id in its output)" "$SEW_OUT"
		sew_load_status
	fi
	log "Sew [5/7]: hostchat media install (voice)"
	if [ "$(sew_field "$SEW_ST" voice.state)" = "running" ]; then
		log "Sew: voice already running"
	else
		sew_timed 15m "15 min" "hostchat media install" hostchat media install
	fi
	sew_load_status
	[ "$(sew_field "$SEW_ST" voice.state)" = "running" ] \
		|| sew_die "hostchat media install (status voice.state is not running)"
	log "Sew [6/7]: settings homeChat + voiceRoomsPreview true"
	k2_as_daemon_user settings set homeChat true >"$SEW_OUT" 2>&1 \
		|| sew_die "settings set homeChat true" "$SEW_OUT"
	k2_as_daemon_user settings set voiceRoomsPreview true >"$SEW_OUT" 2>&1 \
		|| sew_die "settings set voiceRoomsPreview true" "$SEW_OUT"
	log "Sew [7/7]: hostchat doctor --json"
	# The gate is verdict.failed == [] only. On a fresh box (z3, 2026-11-17)
	# the verdict was failed [] with degraded drill.recent, keys.mirror,
	# keys.swap, search.encrypted_disk, media.turn, media.reachability;
	# k2_backup_offbox_none, no_offbox_key_copy, public_url_unset and
	# voice_unprobed are alarms/warnings/degraded, never in failed. The
	# off-box backup + key mirror becomes a later step (9d).
	# The doctor may exit non-zero when a check fails: its verdict decides.
	k2_as_daemon_user hostchat doctor --json >"$SEW_OUT" 2>/dev/null || true
	SEW_FAILED=$(tail -n1 "$SEW_OUT" | python3 -I -c 'import json,sys
f=json.load(sys.stdin)["verdict"]["failed"]
print(json.dumps(f) if f else "[]")' 2>/dev/null) \
		|| sew_die "hostchat doctor --json (unreadable verdict)" "$SEW_OUT"
	[ "$SEW_FAILED" = "[]" ] || sew_die "hostchat doctor (verdict.failed: ${SEW_FAILED})" "$SEW_OUT"
	# 9c.8: the recorder, install only (recording stays off). Never fatal.
	if [ "${K2_SKIP_RECORDER:-}" = "1" ]; then
		REC_LINE="skipped (K2_SKIP_RECORDER=1)"
	else
		log "Sew [9c.8]: hostchat recording preflight (the recorder, install only: recording stays off)"
		K2_CLI_TIMEOUT=2m k2_as_daemon_user hostchat recording preflight >"$SEW_OUT" 2>&1 || true
		if [ "$(head -n1 "$SEW_OUT")" != "ready" ]; then
			REC_WHY="$(grep -E '^  fail ' "$SEW_OUT" | sed -E 's/^  fail +//; s/ +/ /g' | head -n3 | paste -sd ';' - || true)"
			REC_LINE="skipped (not ready: ${REC_WHY:-preflight said no})"
		else
			log "Sew [9c.8]: hostchat recording install (15 min)"
			_rc=0
			K2_CLI_TIMEOUT=15m k2_as_daemon_user hostchat recording install >"$SEW_OUT" 2>&1 || _rc=$?
			if [ "$_rc" != 0 ]; then
				REC_LINE="not installed (hostchat recording install exit ${_rc}: $(tail -n1 "$SEW_OUT"))"
			else
				REC_ST="$(K2_CLI_TIMEOUT=2m k2_as_daemon_user hostchat recording status --json 2>/dev/null | tail -n1 || true)"
				REC_INST="$(sew_field "$REC_ST" installed)"
				REC_ON="$(sew_field "$REC_ST" enabled)"
				if [ "$REC_INST" = "True" ] && [ "$REC_ON" = "False" ]; then
					REC_LINE="installed, recording off (a person turns it on: Settings → Chat Hosting → Recording)"
				elif [ "$REC_INST" = "True" ]; then
					REC_LINE="installed, recording on (a person turned it on earlier)"
				else
					REC_LINE="not installed (status says installed=${REC_INST:-?} after the install)"
				fi
			fi
		fi
	fi
	log "Sew: recorder ${REC_LINE}"
	rm -f "$SEW_OUT"
	# Features follow the switches a moment later: poll /boot-status (60 s).
	log "Sew: checking /boot-status features (up to 60 s)"
	SEW_FEAT=""
	for _ in $(seq 1 12); do
		SEW_FEAT=$(curl -fsS -m 8 "$BASE/boot-status" 2>/dev/null | python3 -I -c 'import json,sys
f=json.load(sys.stdin).get("features") or []
print(" ".join(k for k in ("home-chat-v1","voice-rooms-v1") if k in f))' 2>/dev/null || true)
		case " $SEW_FEAT " in *" home-chat-v1 "*) break ;; esac
		sleep 5
	done
	case " $SEW_FEAT " in *" home-chat-v1 "*) ;; *) sew_die "/boot-status does not list home-chat-v1 after 60 s" ;; esac
	case " $SEW_FEAT " in
		*" voice-rooms-v1 "*) SEW_LINE="installed (Sew ${SEW_INST}, identity k2, voice running, home-chat-v1, voice-rooms-v1)" ;;
		*)
			log "NOTE: Sew: /boot-status does not list voice-rooms-v1 — voice rooms are Pro-only (not a failure)"
			SEW_LINE="installed (Sew ${SEW_INST}, identity k2, voice running, home-chat-v1; voice-rooms-v1 not listed: not Pro)"
			;;
	esac
	log "Sew: ${SEW_LINE}"
fi

# ── 10. control-plane callback ───────────────────────────────────────
if [ -n "${K2_CALLBACK_URL:-}" ]; then
	log "posting provisioning callback"
	# 0.46.18: no logins are made here (ownerUser/opsUser/opsPassword stay
	# in the shape, empty); the k2.dev claim makes them and returns
	# k2cloud's password to k2.dev itself.
	CB=$(python3 -c 'import json,os;print(json.dumps({
		"status":"online",
		"subdomain":os.environ.get("K2_SUBDOMAIN",""),
		"ownerUser":"",
		"firstOwner":"claim",
		"opsUser":"",
		"opsPassword":"",
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
echo "  owner:     none yet (claimed from the k2.dev dashboard of the linked account)"
echo "  mail:      helper ${MAIL_HELPER_LINE}"
echo "  apps:      ${APPS_LINE}"
echo "  sew:       ${SEW_LINE}"
echo "  recorder:  ${REC_LINE}"
echo "  sandboxes: OFF (Standard-tier host — the Dedicated bootstrap adds them)"
if true; then
echo ""
echo "  Next (hosted: the k2.dev dashboard makes the owner for the linked account; interactive self-host, from your own SSH login):"
echo "    k2 users add <you> --role owner --keep-password   # prompts for password"
[ -z "${K2_TUNNEL_TOKEN:-}" ] && echo "    k2 connect login                 # pair purchased *.k2.dev subdomain"
echo "  Automation fallback still works: K2_TUNNEL_TOKEN + K2_SUBDOMAIN env."
fi

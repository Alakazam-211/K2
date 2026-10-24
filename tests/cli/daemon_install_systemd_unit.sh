#!/usr/bin/env bash
# `k2 daemon install` on Linux (0.46.4): when a K2 daemon systemd unit is
# already installed — a SYSTEM k2-daemon.service from provisioning, or a
# user unit — the binary THAT unit runs is replaced in place and that unit
# is restarted (plain restart). No second unit, no second binary, and no
# K2SO names (akzm 2026-10-23: the old installer wrote
# ~/.local/bin/k2so-daemon and a user k2so-daemon.service beside the system
# unit). With no unit installed it writes ~/.local/bin/k2-daemon and the
# user unit k2-daemon.service, as before.
#
# Hermetic: temp HOME, every K2_*/K2SO_* var unset, and stubs on PATH for
# uname (Linux x86_64), systemctl (logs every call; answers `cat` from a
# fixture), minisign (verifies), sudo. No real systemd, no network: the
# manifest, binary and signature are file:// fixtures.
# Run with: bash tests/cli/daemon_install_systemd_unit.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
K2="$ROOT/cli/k2"
[ -f "$K2" ] || { echo "FAIL: $K2 not found" >&2; exit 1; }
bash -n "$K2" || { echo "FAIL: bash -n cli/k2" >&2; exit 1; }
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1  [$2]"; fi; }

WORK="$(mktemp -d -t k2-daemon-install-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# ── stubs ──
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
REAL_UNAME="$(command -v uname)"
cat > "$STUBS/uname" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
    -s) echo Linux ;;
    -m) echo x86_64 ;;
    *) exec "$REAL_UNAME" "\$@" ;;
esac
EOF
# systemctl: log argv; `cat <unit>` prints \$STUB_UNITS/<scope>/<unit> when it
# exists (exit 1 otherwise); `restart` fails when \$STUB_RESTART_FAIL=1.
cat > "$STUBS/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
scope=system
if [ "${1:-}" = "--user" ]; then scope=user; shift; fi
verb="${1:-}"; unit="${2:-}"
case "$verb" in
    cat)
        f="$STUB_UNITS/$scope/$unit"
        [ -f "$f" ] || exit 1
        echo "# $f"
        cat "$f"
        ;;
    restart)
        [ "${STUB_RESTART_FAIL:-0}" = "1" ] && exit 1
        exit 0
        ;;
    *) exit 0 ;;
esac
EOF
cat > "$STUBS/sudo" <<'EOF'
#!/usr/bin/env bash
echo "sudo $*" >> "$STUB_LOG"
[ "${STUB_RESTART_FAIL:-0}" = "1" ] && exit 1
exit 0
EOF
cat > "$STUBS/minisign" <<'EOF'
#!/usr/bin/env bash
echo "minisign $*" >> "$STUB_LOG"
exit 0
EOF
chmod +x "$STUBS"/*

# ── fixtures: a "new" daemon binary + manifest ──
FIX="$WORK/fixtures"
mkdir -p "$FIX"
printf 'new-daemon-bytes\n' > "$FIX/k2-daemon-linux-x86_64"
if command -v sha256sum >/dev/null 2>&1; then
    SHA="$(sha256sum "$FIX/k2-daemon-linux-x86_64" | awk '{print $1}')"
else
    SHA="$(shasum -a 256 "$FIX/k2-daemon-linux-x86_64" | awk '{print $1}')"
fi
printf 'untrusted comment: test signature\nRWQ-test\n' > "$FIX/k2-daemon.sig"
cat > "$FIX/daemon-latest.json" <<EOF
{
  "version": "0.46.4",
  "platforms": {
    "linux-x86_64": {
      "url": "file://$FIX/k2-daemon-linux-x86_64",
      "sig": "file://$FIX/k2-daemon.sig",
      "sha256": "$SHA"
    }
  }
}
EOF

# new_home <name> — a fresh HOME + unit fixture dirs; sets H, UNITS, LOG.
new_home() {
    H="$WORK/$1"
    UNITS="$WORK/$1-units"
    LOG="$WORK/$1.log"
    mkdir -p "$H/.k2" "$UNITS/system" "$UNITS/user"
    : > "$LOG"
}

# run_install <args…> — prints combined output; sets RC.
run_install() {
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin" HOME="$H" \
        STUB_LOG="$LOG" STUB_UNITS="$UNITS" STUB_RESTART_FAIL="${STUB_RESTART_FAIL:-0}" \
        bash "$K2" daemon install --manifest-url "file://$FIX/daemon-latest.json" "$@" 2>&1)"
    RC=$?
    set -e
}

no_k2so_names() {
    [ -z "$(find "$H" -name '*k2so*' 2>/dev/null)" ]
}

echo "== system k2-daemon.service installed: binary replaced in place, system unit restarted =="
new_home sys
mkdir -p "$H/.local/bin"
printf 'old-daemon-bytes\n' > "$H/.local/bin/k2-daemon"
cat > "$UNITS/system/k2-daemon.service" <<EOF
[Unit]
Description=K2 daemon

[Service]
User=k2
ExecStart=$H/.local/bin/k2-daemon
KillMode=process

[Install]
WantedBy=multi-user.target
EOF
run_install
check "exit 0" '[ "$RC" -eq 0 ] || { printf "%s\n" "$OUT" >&2; false; }'
check "the unit's binary holds the new bytes" '[ "$(cat "$H/.local/bin/k2-daemon")" = "new-daemon-bytes" ]'
check "binary is executable" '[ -x "$H/.local/bin/k2-daemon" ]'
check "no k2so-named file anywhere under HOME" 'no_k2so_names'
check "no user unit written" '[ ! -e "$H/.config/systemd/user" ]'
check "system unit restarted (no --user)" 'grep -qx "systemctl restart k2-daemon.service" "$LOG"'
check "no user-scope restart or enable" '! grep -q -- "--user restart\|enable" "$LOG"'
check "output names the restarted unit" 'printf "%s" "$OUT" | grep -q "Restarted k2-daemon.service"'
check "no sudo needed when systemctl restart works" '! grep -q "^sudo" "$LOG"'

echo "== system unit with a custom ExecStart (prefix + %h): that path is updated =="
new_home custom
mkdir -p "$H/opt/k2/bin"
printf 'old\n' > "$H/opt/k2/bin/k2-daemon"
cat > "$UNITS/system/k2-daemon.service" <<'EOF'
[Service]
ExecStart=
ExecStart=-%h/opt/k2/bin/k2-daemon --headless
EOF
run_install --bin-dir "$H/elsewhere"
check "exit 0" '[ "$RC" -eq 0 ] || { printf "%s\n" "$OUT" >&2; false; }'
check "the ExecStart binary was updated" '[ "$(cat "$H/opt/k2/bin/k2-daemon")" = "new-daemon-bytes" ]'
check "nothing written to --bin-dir" '[ ! -e "$H/elsewhere" ]'
check "nothing written to ~/.local/bin" '[ ! -e "$H/.local/bin" ]'
check "the ignored --bin-dir is said" 'printf "%s" "$OUT" | grep -q "is ignored"'
check "system unit restarted" 'grep -qx "systemctl restart k2-daemon.service" "$LOG"'

echo "== user k2-daemon.service installed: updated in place, user unit restarted =="
new_home user
mkdir -p "$H/.local/bin"
printf 'old\n' > "$H/.local/bin/k2-daemon"
cat > "$UNITS/user/k2-daemon.service" <<EOF
[Service]
ExecStart=$H/.local/bin/k2-daemon
EOF
run_install
check "exit 0" '[ "$RC" -eq 0 ] || { printf "%s\n" "$OUT" >&2; false; }'
check "binary updated" '[ "$(cat "$H/.local/bin/k2-daemon")" = "new-daemon-bytes" ]'
check "user unit restarted" 'grep -qx "systemctl --user restart k2-daemon.service" "$LOG"'
check "no system-scope restart" '! grep -qx "systemctl restart k2-daemon.service" "$LOG"'
check "unit file not rewritten (no user unit dir under HOME)" '[ ! -e "$H/.config/systemd/user/k2-daemon.service" ]'
check "no k2so-named file" 'no_k2so_names'

echo "== no unit installed: fresh install writes k2-daemon + user k2-daemon.service =="
new_home fresh
run_install
check "exit 0" '[ "$RC" -eq 0 ] || { printf "%s\n" "$OUT" >&2; false; }'
check "binary at ~/.local/bin/k2-daemon" '[ "$(cat "$H/.local/bin/k2-daemon")" = "new-daemon-bytes" ]'
check "user unit k2-daemon.service written" '[ -f "$H/.config/systemd/user/k2-daemon.service" ]'
check "unit runs ~/.local/bin/k2-daemon" 'grep -qx "ExecStart=$H/.local/bin/k2-daemon" "$H/.config/systemd/user/k2-daemon.service"'
check "no k2so-named file" 'no_k2so_names'
check "no restart on a fresh install" '! grep -q "restart" "$LOG"'

echo "== dry run with the system unit: names it, writes nothing =="
new_home dry
mkdir -p "$H/.local/bin"
printf 'old\n' > "$H/.local/bin/k2-daemon"
cat > "$UNITS/system/k2-daemon.service" <<EOF
[Service]
ExecStart=$H/.local/bin/k2-daemon
EOF
run_install --dry-run
check "exit 0" '[ "$RC" -eq 0 ] || { printf "%s\n" "$OUT" >&2; false; }'
check "install path is the unit's binary" 'printf "%s" "$OUT" | grep -q "install path:   $H/.local/bin/k2-daemon"'
check "names the installed system unit" 'printf "%s" "$OUT" | grep -q "installed system unit k2-daemon.service"'
check "restart command without --user" 'printf "%s" "$OUT" | grep -q "restart cmd:    systemctl restart k2-daemon.service"'
check "binary untouched" '[ "$(cat "$H/.local/bin/k2-daemon")" = "old" ]'
check "no restart" '! grep -q "restart" "$LOG"'

echo "== system unit restart refused (no root, no sudo): exit 1 with the root command =="
new_home norestart
mkdir -p "$H/.local/bin"
printf 'old\n' > "$H/.local/bin/k2-daemon"
cat > "$UNITS/system/k2-daemon.service" <<EOF
[Service]
ExecStart=$H/.local/bin/k2-daemon
EOF
STUB_RESTART_FAIL=1 run_install
check "exit 1 (got $RC)" '[ "$RC" -eq 1 ]'
check "binary still replaced" '[ "$(cat "$H/.local/bin/k2-daemon")" = "new-daemon-bytes" ]'
check "says how to restart as root" 'printf "%s" "$OUT" | grep -q "sudo systemctl restart k2-daemon.service"'
if [ "$(id -u)" != "0" ]; then
    check "tried sudo -n (never a prompt)" 'grep -qx "sudo -n systemctl restart k2-daemon.service" "$LOG"'
fi
check "no user unit written as a fallback" '[ ! -e "$H/.config/systemd/user" ]'

echo "== help says what happens on Linux =="
new_home help
run_install --help
check "help exit 0" '[ "$RC" -eq 0 ]'
check "help explains in-place update" 'printf "%s" "$OUT" | grep -q "replaced in place and the unit restarted"'

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

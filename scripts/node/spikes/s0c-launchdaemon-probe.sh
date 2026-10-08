#!/usr/bin/env bash
# S0(c) spike — prd-k2-compute-nodes-v1 §9.2 / §21.4.
#
# On a Mac mini: run a probe as a hidden service user from a LaunchDaemon
# (exactly how k2-node will run) and check what macOS lets it do:
#   - keep the Mac awake (`caffeinate -i -s`, seen in `pmset -g assertions`);
#   - find a job's memory by process group (`ps -axo pgid=,rss=`) and kill
#     the group when it passes a cap (macOS has no hard RAM cap);
#   - raise RLIMIT_NOFILE;
#   - NOT reach the human's keychain, home (when it is 700) or GUI session.
#
# Run ON the mini as root:
#   scp scripts/node/spikes/s0c-launchdaemon-probe.sh <user>@<mini>:
#   ssh -t <user>@<mini> sudo bash s0c-launchdaemon-probe.sh --human <user>
# Options: --human USER (whose home and keychain to probe), --keep,
#          --report PATH (default /var/tmp/k2-s0c-report.txt)

set -euo pipefail

HUMAN=""
KEEP=0
REPORT="/var/tmp/k2-s0c-report.txt"
while [ $# -gt 0 ]; do
    case "$1" in
        --human) HUMAN="${2:-}"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = "Darwin" ] || { echo "this spike is only for macOS" >&2; exit 2; }
[ "$(id -u)" = "0" ] || { echo "run as root (sudo): it creates a hidden user and a LaunchDaemon" >&2; exit 2; }
[ -n "$HUMAN" ] || { echo "--human <user> is required" >&2; exit 2; }
HUMAN_HOME="$(dscl . -read "/Users/$HUMAN" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
[ -n "$HUMAN_HOME" ] || { echo "no user $HUMAN" >&2; exit 2; }

REC="_k2spike"
HOME_DIR="/var/k2spike"
LABEL="dev.k2.spike"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"

cleanup() {
    local rc=$?
    launchctl bootout "system/${LABEL}" >/dev/null 2>&1 || true
    rm -f "$PLIST"
    if [ "$KEEP" = "0" ]; then
        dscl . -delete "/Users/$REC" 2>/dev/null || true
        dscl . -delete "/Groups/$REC" 2>/dev/null || true
        rm -rf "$HOME_DIR"
    fi
    exit "$rc"
}
trap cleanup EXIT

if ! dscl . -read "/Users/$REC" >/dev/null 2>&1; then
    uid=""
    for c in $(seq 400 -1 200); do
        if ! dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$c" && \
           ! dscl . -list /Groups PrimaryGroupID | awk '{print $2}' | grep -qx "$c"; then uid="$c"; break; fi
    done
    [ -n "$uid" ] || { echo "no free id in 200–400" >&2; exit 1; }
    dscl . -create "/Groups/$REC" PrimaryGroupID "$uid"
    dscl . -create "/Users/$REC"
    dscl . -create "/Users/$REC" RecordName "$REC" k2spike
    dscl . -create "/Users/$REC" UniqueID "$uid"
    dscl . -create "/Users/$REC" PrimaryGroupID "$uid"
    dscl . -create "/Users/$REC" UserShell /usr/bin/false
    dscl . -create "/Users/$REC" NFSHomeDirectory "$HOME_DIR"
    dscl . -create "/Users/$REC" IsHidden 1
fi
mkdir -p "$HOME_DIR/out"
cat > "$HOME_DIR/probe.sh" <<PROBE
#!/bin/bash
out="$HOME_DIR/out/probe.txt"
exec > "\$out" 2>&1
echo "id: \$(id)"
echo "HOME=\$HOME USER=\$USER"
echo "-- keep awake"
/usr/bin/caffeinate -i -s -t 15 &
cpid=\$!
sleep 2
/usr/bin/pmset -g assertions | grep -iE "caffeinate|PreventUserIdleSystemSleep|PreventSystemSleep" | head -5
kill \$cpid 2>/dev/null
echo "-- rlimit nofile"
ulimit -n 65536 && echo "raised to \$(ulimit -n)" || echo "could not raise (\$(ulimit -n))"
echo "-- rss cap by process group"
# Job control on: the hog gets its own process group (like a k2-node job),
# so killing the group never kills this probe.
set -m
/bin/sh -c 'exec /usr/bin/perl -e "my \\\$x = \"a\" x (400*1024*1024); sleep 30"' &
hog=\$!
pg=\$(ps -o pgid= -p \$hog | tr -d " ")
killed=no
for i in \$(seq 1 20); do
    sleep 1
    rss=\$(ps -axo pgid=,rss= | awk -v g="\$pg" '\$1==g {s+=\$2} END {print s+0}')
    echo "t=\${i}s pgid=\$pg rss_kb=\$rss"
    if [ "\$rss" -gt 204800 ]; then kill -KILL -- -\$pg 2>/dev/null || kill -KILL \$hog; killed=yes; break; fi
done
echo "killed over 200 MB: \$killed"
echo "-- the human's things (should all be refused)"
ls "$HUMAN_HOME" >/dev/null 2>&1 && echo "home: READABLE (\$(stat -f %Sp "$HUMAN_HOME"))" || echo "home: refused"
ls "$HUMAN_HOME/.ssh" >/dev/null 2>&1 && echo ".ssh: READABLE" || echo ".ssh: refused"
ls "$HUMAN_HOME/.k2" >/dev/null 2>&1 && echo ".k2: READABLE" || echo ".k2: refused"
/usr/bin/security list-keychains 2>&1 | head -3
/usr/bin/security find-generic-password -s dev.k2.connect.account "$HUMAN_HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 && echo "human keychain item: READABLE" || echo "human keychain item: refused"
launchctl print "gui/\$(id -u $HUMAN)" >/dev/null 2>&1 && echo "human GUI domain: visible" || echo "human GUI domain: refused"
echo "-- done"
PROBE
chmod 755 "$HOME_DIR/probe.sh"
chown -R "$REC:$REC" "$HOME_DIR"
chmod 700 "$HOME_DIR"

cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${LABEL}</string>
  <key>UserName</key><string>${REC}</string>
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>${HOME_DIR}/probe.sh</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict>
</plist>
PLIST
chmod 644 "$PLIST"
launchctl bootstrap system "$PLIST"
for _ in $(seq 1 60); do
    grep -q -- "-- done" "$HOME_DIR/out/probe.txt" 2>/dev/null && break
    sleep 1
done
{
    echo "S0(c) — LaunchDaemon probe as $REC on $(hostname) ($(sw_vers -productVersion))"
    echo "human: $HUMAN ($HUMAN_HOME, mode $(stat -f %Sp "$HUMAN_HOME"))"
    echo
    cat "$HOME_DIR/out/probe.txt" 2>/dev/null || echo "probe produced no output (did the LaunchDaemon start? launchctl print system/${LABEL})"
} > "$REPORT"
cat "$REPORT"

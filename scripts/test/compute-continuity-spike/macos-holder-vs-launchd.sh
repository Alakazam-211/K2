#!/usr/bin/env bash
# U0 spike (a) — prd-compute-update-continuity-v1 §5.2, §10 U0, R2.
#
# Question: on a Mac, does a job held by a double-forked `setsid` holder
# survive a launchd restart of the SYSTEM-domain LaunchDaemon that started
# it (the way k2-node runs: /Library/LaunchDaemons, a hidden user)? The
# ptyd S0 spike proved it for a GUI LaunchAgent only (ptyd-s0-results.md).
# Three restarts: `launchctl kickstart -k`, `kill -9` of the daemon, and
# `launchctl bootout` + `bootstrap`. Control row: a child left in the
# daemon's process group (launchd kills that group: expected to die).
# Also, under a Seatbelt profile like the job's:
#   - the job writes to an INHERITED fd of a spool file whose directory the
#     profile denies (U2: the holder never opens the spool path);
#   - `(deny signal (target others))` (C7): the job can't signal its
#     holder or an outside process, and can still signal its own children.
#
# It never touches the real node: label dev.k2.spike-node, hidden user
# _k2spike, home /var/k2spike; all removed at the end (also on failure).
#
# Needs: root on a Mac mini that is NOT mid-job for anyone (k2n2-m1 /
# k2n3-m1 when idle; ask Sew if its CI holds one). macOS 14+. ~2 minutes.
#
#   K2_SPIKE_OK=1 sudo -E bash macos-holder-vs-launchd.sh [--report PATH] [--keep]
#
# Exit 0 = every row ran (read the table); 1 = the spike itself failed.
# Report: /var/tmp/k2-spike-macos-holder.txt.
set -euo pipefail
cd /

REPORT=""
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --report) REPORT="${2:-}"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = "Darwin" ] || { echo "macOS only" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run as root (sudo): it makes a hidden user and a LaunchDaemon" >&2; exit 2; }
# The report: a fresh root-owned dir by default (never a fixed path in a
# world-writable /var/tmp, where a planted symlink would aim root's write;
# security review B-L7); a named --report must not exist yet.
if [ -z "$REPORT" ]; then
    REPORT="$(mktemp -d /var/tmp/k2-spike-macos-holder.XXXXXX)/report.txt"
elif [ -e "$REPORT" ] || [ -L "$REPORT" ]; then
    echo "--report $REPORT exists; name a new file" >&2; exit 2
fi
set -o noclobber
[ "${K2_SPIKE_OK:-}" = "1" ] || { echo "refusing: set K2_SPIKE_OK=1 (an idle test Mac only)" >&2; exit 2; }

REC="_k2spike"
HOME_DIR="/var/k2spike"
LABEL="dev.k2.spike-node"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
ROWS=()

cleanup() {
    local rc=$?
    launchctl bootout "system/${LABEL}" >/dev/null 2>&1 || true
    rm -f "$PLIST"
    # Every process the spike user still has (holders, jobs).
    pkill -KILL -u "$REC" >/dev/null 2>&1 || true
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
    for c in $(seq 450 -1 300); do
        if ! dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$c" && \
           ! dscl . -list /Groups PrimaryGroupID | awk '{print $2}' | grep -qx "$c"; then uid="$c"; break; fi
    done
    [ -n "$uid" ] || { echo "no free id in 300–450" >&2; exit 1; }
    dscl . -create "/Groups/$REC" PrimaryGroupID "$uid"
    dscl . -create "/Users/$REC"
    dscl . -create "/Users/$REC" UniqueID "$uid"
    dscl . -create "/Users/$REC" PrimaryGroupID "$uid"
    dscl . -create "/Users/$REC" UserShell /usr/bin/false
    dscl . -create "/Users/$REC" NFSHomeDirectory "$HOME_DIR"
    dscl . -create "/Users/$REC" IsHidden 1
fi
mkdir -p "$HOME_DIR/state/spool" "$HOME_DIR/state/hold" "$HOME_DIR/jobs/j1"
chown -R "$REC:$REC" "$HOME_DIR"
chmod 0700 "$HOME_DIR/state"

# The job's Seatbelt profile, like kn/wall.rs: default allow, the node's
# state denied, plus the C7 signal rule under test.
cat >| "$HOME_DIR/job.sb" <<EOF
(version 1)
(allow default)
(deny file-read* file-write* (subpath "$HOME_DIR/state"))
(deny signal (target others))
EOF

# The holder (§5.2): exec'd by the "node", double-forks and setsids before
# anything else, writes its facts into state/hold/, runs the job in its own
# process group with stdout/stderr = the inherited spool fds, waits, writes
# exit.json. Perl, so it has a real fork/setsid with no runtime threads.
cat >| "$HOME_DIR/holder.pl" <<'EOF'
use strict; use warnings; use POSIX qw(setsid setpgid _exit);
my ($hold, $profile, @job) = @ARGV;
my $p = fork(); die "fork: $!" unless defined $p; _exit(0) if $p;   # the node's child exits at once
setsid() or die "setsid: $!";
$p = fork(); die "fork: $!" unless defined $p; _exit(0) if $p;      # the session leader exits: no tty, ever
my $job = fork(); die "fork: $!" unless defined $job;
if ($job == 0) {
    setpgid(0, 0);
    exec('/usr/bin/sandbox-exec', '-f', $profile, @job) or _exit(127);
}
open(my $h, '>', "$hold/hold.json.tmp") or die "hold.json: $!";
print $h "{\"pid\":$$,\"job_pgid\":$job}\n"; close $h; rename("$hold/hold.json.tmp", "$hold/hold.json");
waitpid($job, 0); my $st = $?;
open(my $x, '>', "$hold/exit.json") or die; print $x "{\"status\":$st}\n"; close $x;
kill('KILL', -$job);
_exit(0);
EOF

# The stand-in node: opens the spool (state/spool is the node's, denied to
# the job), hands the fds to the holder, leaves a control child in its own
# process group, then idles as the daemon's main process.
cat >| "$HOME_DIR/node.sh" <<EOF
#!/bin/bash
cd "$HOME_DIR/jobs/j1"
if [ ! -f "$HOME_DIR/state/hold/hold.json" ]; then
    exec 3>>"$HOME_DIR/state/spool/j1.out"
    /usr/bin/perl "$HOME_DIR/holder.pl" "$HOME_DIR/state/hold" "$HOME_DIR/job.sb" \\
        /bin/sh -c 'i=0; while :; do i=\$((i+1)); echo "tick \$i"; sleep 1; done' >&3 2>&3 </dev/null
    exec 3>&-
fi
/bin/sleep 100000 &
echo \$! > "$HOME_DIR/control.pid"
exec /bin/sleep 100000
EOF
chmod 0755 "$HOME_DIR/node.sh"
chown "$REC:$REC" "$HOME_DIR/node.sh" "$HOME_DIR/holder.pl" "$HOME_DIR/job.sb"

cat >| "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>${LABEL}</string>
  <key>ProgramArguments</key><array><string>${HOME_DIR}/node.sh</string></array>
  <key>UserName</key><string>${REC}</string>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
  <key>Umask</key><integer>63</integer>
</dict></plist>
EOF
chmod 0644 "$PLIST"

row() { ROWS+=("$1|$2|$3"); echo "  $1: $2 ($3)"; }
alive() { kill -0 "$1" 2>/dev/null; }
daemon_pid() { launchctl print "system/${LABEL}" 2>/dev/null | awk '/^\tpid = /{print $3}'; }
lines() { wc -l < "$HOME_DIR/state/spool/j1.out" | tr -d ' '; }
wait_for() { for _ in $(seq 1 50); do "$@" && return 0; sleep 0.2; done; return 1; }

launchctl bootstrap system "$PLIST"
wait_for test -f "$HOME_DIR/state/hold/hold.json" || { echo "the holder never wrote hold.json" >&2; exit 1; }
HOLDER="$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$HOME_DIR/state/hold/hold.json")"
JOB="$(sed -n 's/.*"job_pgid":\([0-9]*\).*/\1/p' "$HOME_DIR/state/hold/hold.json")"
sleep 3
[ "$(lines)" -ge 2 ] || { echo "the job never wrote to its inherited spool fd under Seatbelt" >&2; exit 1; }
row "Seatbelt job writes an inherited fd under a denied state/" "yes" "$(lines) lines in state/spool/j1.out"
deny_read="readable"; sudo -u "$REC" /usr/bin/sandbox-exec -f "$HOME_DIR/job.sb" /bin/cat "$HOME_DIR/state/spool/j1.out" >/dev/null 2>&1 || deny_read="denied"
row "the same job opening the spool path itself" "$deny_read" "the profile denies state/ by path"

restart_case() { # <title> <how>
    local title="$1" how="$2"
    local ctl; ctl="$(cat "$HOME_DIR/control.pid")"
    local before; before="$(lines)"
    local d; d="$(daemon_pid)"
    case "$how" in
        kickstart) launchctl kickstart -k "system/${LABEL}" ;;
        kill9) kill -KILL "$d" ;;
        bootout) launchctl bootout "system/${LABEL}"; sleep 2; launchctl bootstrap system "$PLIST" ;;
    esac
    sleep 4
    local d2; d2="$(daemon_pid)"
    local h="dies"; alive "$HOLDER" && h="survives"
    local j="dies"; alive "$JOB" && j="survives"
    local c="dies"; alive "$ctl" && c="survives"
    local after; after="$(lines)"
    row "$title" "holder $h, job $j" "daemon pid $d→${d2:-none}; control child (daemon's group) $c; spool $before→$after lines"
}
restart_case "launchctl kickstart -k system/${LABEL}" kickstart
restart_case "kill -9 the daemon's main pid" kill9
restart_case "launchctl bootout + bootstrap" bootout

# C7: the signal rule. From inside the job's profile: the holder and an
# outside process can't be signalled; the job's own child can.
OUTSIDE_PID=""
sudo -u "$REC" /bin/sleep 300 &
OUTSIDE_PID=$!
sleep 0.5
sig_out="$(sudo -u "$REC" /usr/bin/sandbox-exec -f "$HOME_DIR/job.sb" /bin/sh -c "
    r=''
    kill -0 $HOLDER 2>/dev/null && r=\"\$r holder:allowed\" || r=\"\$r holder:denied\"
    kill -0 $OUTSIDE_PID 2>/dev/null && r=\"\$r outside:allowed\" || r=\"\$r outside:denied\"
    /bin/sleep 30 & c=\$!
    kill -TERM \$c 2>/dev/null && r=\"\$r own-child:allowed\" || r=\"\$r own-child:denied\"
    echo \$r" 2>&1 || true)"
kill "$OUTSIDE_PID" 2>/dev/null || true
row "(deny signal (target others))" "${sig_out# }" "want holder:denied outside:denied own-child:allowed"

# The holder's own end: cancel = SIGTERM the holder's job group, exit.json written.
kill -TERM -- "-$JOB" 2>/dev/null || true
wait_for test -f "$HOME_DIR/state/hold/exit.json" && row "job ends → holder writes exit.json" "yes" "$(cat "$HOME_DIR/state/hold/exit.json")" \
    || row "job ends → holder writes exit.json" "no" "no exit.json within 10 s"

{
    echo "U0 spike (a): macOS job holder vs a system LaunchDaemon restart — $(date -u +%FT%TZ)"
    echo "host $(hostname)  macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion))  $(uname -m)"
    echo
    printf '%-58s | %-26s | %s\n' "case" "result" "detail"
    for r in "${ROWS[@]}"; do IFS='|' read -r a b c <<<"$r"; printf '%-58s | %-26s | %s\n' "$a" "$b" "$c"; done
    echo
    echo "Read: the three restart rows must say 'holder survives, job survives' for §5.2 (R2)."
    echo "The control child is expected to die (launchd kills the daemon's process group)."
    echo "Signal row: 'holder:denied outside:denied own-child:allowed' = keep the C7 rule;"
    echo "then also run a real build under it (k2 compute run on this mini) before shipping it."
} > "$REPORT"
echo "report: $REPORT"
cat "$REPORT"

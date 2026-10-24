#!/usr/bin/env bash
# `k2 job` (prd-vault-gaps-v1 item 5) — headless test against a foreground
# sandbox daemon (temp HOME, no client attached), through the real CLI.
#
#   cargo build -p k2-daemon && bash tests/cli/job_headless.sh
#   JOB_HEADLESS_SKIP_SCHEDULE=1 …   skip the ~70 s wait for a scheduled run
#
# Checks: a job reads a granted env secret (prints only its length), an
# echo of the value is redacted in the log, no grant = nothing (the
# workspace's own secret doesn't reach a job), the run's passport works
# during the run (vault env-file, vault status) and only for use routes,
# and is refused once the run ends; timeout kills the process group;
# overlap refused; kill; log cap; restart marks a run interrupted; pause
# --all; a scheduled `--every 1m` run fires on its own; rm drops grants.
# Every check fails loudly (exit 1) with the body. Never ~/.k2, never the
# Keychain (a temp HOME keeps the vault key in a file).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2CLI="$PROJECT_ROOT/cli/k2"
# shellcheck source=/dev/null
source "$SCRIPT_DIR/_sandbox_daemon.sh"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
# The sandbox daemon must not touch the OS heartbeat job or wake events.
export K2_HEARTBEAT_NO_SELF_HEAL=1

WORK="$(mktemp -d -t k2-job-work-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$WORK"' EXIT

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail ---" >&2; tail -40 "$SANDBOX_HOME/daemon.log" >&2 || true; exit 1; }
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jget() { python3 -c 'import json,sys; j=json.loads(sys.argv[1]); print(eval(sys.argv[2], {"j": j}))' "$1" "$2"; }
# k2 … as the owner of the sandbox daemon (its token from the temp HOME).
k2() { env -u K2_HOOK_TOKEN HOME="$SANDBOX_HOME" K2_PORT="$K2SO_PORT" PATH="/usr/bin:/bin:/usr/sbin:/sbin" bash "$K2CLI" "$@"; }
BASE="http://127.0.0.1:${K2SO_PORT}"
TOK="$K2SO_TOKEN"

WSNAME="jobws$RANDOM"
WS="$WORK/$WSNAME"
mkdir -p "$WS/scripts"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$WS")" \
    "$BASE/cli/projects/add-without-git?token=$TOK")"
case "$reg" in *"$WS"*) ok "workspace registered" ;; *) fail "register: $reg" ;; esac

SECRET="k2test_JOBSECRET_$(python3 -c 'import secrets; print(secrets.token_hex(12))')"
printf '%s' "$SECRET" | k2 vault set JOB_SECRET --scope "ws:$WSNAME" --kind password --delivery env >/dev/null \
    || fail "vault set"
ok "a workspace secret with env delivery"

# wait_run NAME RUN → the run's final outcome (polls show, 30 s).
wait_run() {
    local name="$1" run="$2" i out
    for i in $(seq 1 150); do
        out="$(k2 job show "$name" --workspace "$WS" --json)"
        if python3 - "$out" "$run" <<'PY'
import json, sys
j = json.loads(sys.argv[1])["job"]
r = [x for x in j.get("runs", []) if x["id"] == sys.argv[2]]
sys.exit(0 if r and r[0]["outcome"] != "running" else 1)
PY
        then
            jget "$out" "[x for x in j['job']['runs'] if x['id']=='$run'][0]['outcome']"
            return 0
        fi
        sleep 0.2
    done
    fail "run $run of $name never finished: $out"
}
start_run() { k2 job run "$1" --workspace "$WS" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"])'; }
log_of() { k2 job logs "$1" --workspace "$WS" --run "$2" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["log"])'; }

echo "== a job reads only what is granted to it =="
out="$(k2 job add reader --workspace "$WS" --every 1h --timeout 20s --json -- \
    sh -c 'printf "len=%s\n" "${#JOB_SECRET}"; echo "echo:$JOB_SECRET"; echo "path:$PATH"; echo "pwd:$(pwd -P)"; echo "job:$K2_JOB"')"
# A fresh workspace has no pinned Thread yet, so the line may not post;
# the add is still in the Access Audit and in k2 job list.
note="$(jget "$out" "j['threadNote']")"
case "$note" in True|False) ok "add (Thread line posted: $note)" ;; *) fail "add: $out" ;; esac
grep -q '"job-added"' "$SANDBOX_HOME/.k2/auth-audit.jsonl" 2>/dev/null \
    || grep -rq 'job-added' "$SANDBOX_HOME/.k2" 2>/dev/null || fail "no job-added audit row"
ok "the add is in the Access Audit"
case "$(k2 job list --workspace "$WS")" in *reader*) ok "k2 job list shows it" ;; *) fail "list" ;; esac
run="$(start_run reader)"
[ "$(wait_run reader "$run")" = "ok" ] || fail "reader run 1"
log="$(log_of reader "$run")"
case "$log" in *"len=0"*) ok "no grant: the workspace's own secret does not reach the job" ;; *) fail "ungranted run: $log" ;; esac
k2 vault grant JOB_SECRET --scope "ws:$WSNAME" --to "job:$WSNAME/reader" --as env >/dev/null \
    || fail "grant to job:reader"
ok "granted --to job:$WSNAME/reader --as env"
run="$(start_run reader)"
[ "$(wait_run reader "$run")" = "ok" ] || fail "reader run 2"
log="$(log_of reader "$run")"
echo "$log" | sed 's/^/    | /'
case "$log" in *"len=${#SECRET}"*) ok "the job read the granted env secret (length ${#SECRET})" ;; *) fail "granted run: $log" ;; esac
case "$log" in *"$SECRET"*) fail "the secret value is in the log" ;; esac
case "$log" in *"echo:[k2v:JOB_SECRET]"*) ok "the echoed value is redacted in the log" ;; *) fail "redaction: $log" ;; esac
case "$log" in *"path:/usr/local/bin:"*) ok "PATH starts with the k2 CLI's home" ;; *) fail "PATH: $log" ;; esac
case "$log" in *"pwd:$WS"*) ok "the run's folder is the workspace" ;; *) fail "cwd: $log" ;; esac
show="$(k2 job show reader --workspace "$WS")"
case "$show" in *"JOB_SECRET (env"*) ok "show lists the job's grants" ;; *) fail "show: $show" ;; esac

echo "== the run's passport: use routes only, dead after the run =="
cat > "$WS/scripts/passport.sh" <<EOF
#!/bin/sh
printf '%s' "\$K2_HOOK_TOKEN" > "$WS/passport.txt"
bash "$K2CLI" vault env-file --secret JOB_SECRET > "$WS/envfile.txt" 2>&1; echo "envfile_rc=\$?"
f="\$(sed -n 's/.*\(\/[^ ]*\.env\).*/\1/p' "$WS/envfile.txt" | head -1)"
[ -n "\$f" ] && echo "envfile_len=\$(wc -c < "\$f" | tr -d ' ')"
bash "$K2CLI" vault status >/dev/null 2>&1; echo "status_rc=\$?"
bash "$K2CLI" vault list > "$WS/list.txt" 2>&1; echo "list_rc=\$?"
bash "$K2CLI" job list > "$WS/joblist.txt" 2>&1; echo "joblist_rc=\$?"
EOF
chmod +x "$WS/scripts/passport.sh"
k2 job add passport --workspace "$WS" --every 1h --timeout 60s -- ./scripts/passport.sh >/dev/null
k2 vault grant JOB_SECRET --scope "ws:$WSNAME" --to "job:$WSNAME/passport" --as env >/dev/null || fail "grant passport"
run="$(start_run passport)"
[ "$(wait_run passport "$run")" = "ok" ] || fail "passport run: $(log_of passport "$run")"
log="$(log_of passport "$run")"
echo "$log" | sed 's/^/    | /'
case "$log" in *"envfile_rc=0"*"envfile_len="*) ok "k2 vault env-file works with the job's passport and grant" ;; *) fail "env-file: $log / $(cat "$WS/envfile.txt")" ;; esac
case "$log" in *"status_rc=0"*) ok "k2 vault status works" ;; *) fail "status: $log" ;; esac
case "$log" in *"list_rc=3"*) ok "k2 vault list is refused for a job (use only)" ;; *) fail "list: $log / $(cat "$WS/list.txt")" ;; esac
grep -q "job_use_only" "$WS/list.txt" || fail "list refusal code: $(cat "$WS/list.txt")"
case "$log" in *"joblist_rc=3"*) ok "a job can't manage jobs" ;; *) fail "job list from a job: $log / $(cat "$WS/joblist.txt")" ;; esac
grep -q "job_cannot_manage_jobs" "$WS/joblist.txt" || fail "job refusal code: $(cat "$WS/joblist.txt")"
PASSPORT="$(cat "$WS/passport.txt")"
case "$PASSPORT" in job-*.*) ok "the run had a job passport" ;; *) fail "passport shape" ;; esac
st="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $PASSPORT" "$BASE/cli/vault/status")"
[ "$st" = "401" ] || [ "$st" = "403" ] || fail "the dead passport still works: $st"
ok "the passport is refused after the run ($st)"

echo "== timeout, overlap, kill =="
k2 job add sleeper --workspace "$WS" --every 1h --timeout 2s -- sh -c 'sleep 300 & echo "child=$!"; sleep 300' >/dev/null
run="$(start_run sleeper)"
set +e; o="$(k2 job run sleeper --workspace "$WS" 2>&1)"; rc=$?; set -e
[ "$rc" = "1" ] && case "$o" in *job_running*) true ;; *) false ;; esac || fail "overlap: rc=$rc $o"
ok "a second run while one is running is refused (job_running)"
[ "$(wait_run sleeper "$run")" = "timeout" ] || fail "timeout outcome"
log="$(log_of sleeper "$run")"
child="$(printf '%s' "$log" | sed -n 's/^child=\([0-9]*\).*/\1/p')"
[ -n "$child" ] || fail "no child pid: $log"
sleep 0.5
if kill -0 "$child" 2>/dev/null; then fail "the timed-out job's child $child is still alive"; fi
case "$log" in *"stopped: timeout"*) ok "timeout stopped the whole process group" ;; *) fail "timeout note: $log" ;; esac
k2 job edit sleeper --workspace "$WS" --timeout 60s >/dev/null
run="$(start_run sleeper)"
sleep 1
k2 job kill sleeper --workspace "$WS" >/dev/null
[ "$(wait_run sleeper "$run")" = "killed" ] || fail "kill outcome"
ok "k2 job kill stops the run"

echo "== log cap =="
k2 job add chatty --workspace "$WS" --every 1h -- sh -c 'head -c 2097152 /dev/zero | tr "\0" "a"' >/dev/null
run="$(start_run chatty)"
[ "$(wait_run chatty "$run")" = "ok" ] || fail "chatty run"
lp="$(find "$SANDBOX_HOME/.k2/jobs" -name "$run.log")"
sz="$(wc -c < "$lp" | tr -d ' ')"
[ "$sz" -lt 1100000 ] && [ "$sz" -ge 1048576 ] || fail "log size $sz"
bytes="$(jget "$(k2 job show chatty --workspace "$WS" --json)" "j['job']['runs'][0]['bytes']")"
[ "$bytes" = "2097152" ] || fail "bytes counted: $bytes"
grep -q "log capped at 1 MiB" "$lp" || fail "cap note"
ok "log capped at 1 MiB ($sz bytes on disk, $bytes counted)"

echo "== disable, pause --all =="
k2 job disable reader --workspace "$WS" >/dev/null
set +e; o="$(k2 job run reader --workspace "$WS" 2>&1)"; rc=$?; set -e
case "$o" in *job_disabled*) ok "a disabled job doesn't run" ;; *) fail "disabled: $rc $o" ;; esac
k2 job enable reader --workspace "$WS" >/dev/null
k2 job pause --all >/dev/null
set +e; o="$(k2 job run reader --workspace "$WS" 2>&1)"; set -e
case "$o" in *jobs_paused*) ok "pause --all stops runs" ;; *) fail "paused: $o" ;; esac
case "$(k2 job list --workspace "$WS")" in *"ALL JOBS PAUSED"*) ok "list says so" ;; *) fail "list paused" ;; esac
k2 job resume --all >/dev/null

if [ "${JOB_HEADLESS_SKIP_SCHEDULE:-0}" != "1" ]; then
    echo "== a scheduled run fires on its own (--every 1m; up to 100 s) =="
    k2 job add ticker --workspace "$WS" --every 1m -- sh -c 'echo tick' >/dev/null
    got=""
    for i in $(seq 1 100); do
        got="$(jget "$(k2 job show ticker --workspace "$WS" --json)" "(j['job'].get('lastRun') or {}).get('trigger','')")"
        [ "$got" = "schedule" ] && break
        sleep 1
    done
    [ "$got" = "schedule" ] || fail "no scheduled run within 100 s"
    ok "the daemon ran ticker on its schedule"
fi

echo "== restart marks a running run interrupted =="
run="$(start_run sleeper)"
sleep 1
sandbox_daemon_restart daemon2.log
st="$(jget "$(k2 job show sleeper --workspace "$WS" --json)" "[x for x in j['job']['runs'] if x['id']=='$run'][0]['outcome']")"
[ "$st" = "interrupted" ] || fail "after restart: $st"
ok "a run the old daemon left is interrupted"
child="$(sed -n 's/^child=\([0-9]*\).*/\1/p' "$(find "$SANDBOX_HOME/.k2/jobs" -name "$run.log")")"
[ -n "$child" ] || fail "no child pid in the interrupted run's log"
for i in $(seq 1 25); do kill -0 "$child" 2>/dev/null || break; sleep 0.2; done
if kill -0 "$child" 2>/dev/null; then fail "the interrupted run's processes survived the restart ($child)"; fi
ok "boot stopped the interrupted run's leftover processes"

echo "== rm drops the job and its grants =="
o="$(k2 job rm reader --workspace "$WS" --json)"
[ "$(jget "$o" "j['grantsRevoked']")" = "1" ] || fail "rm grants: $o"
set +e; o="$(k2 job show reader --workspace "$WS" 2>&1)"; rc=$?; set -e
[ "$rc" = "4" ] || fail "show after rm: $rc $o"
ok "rm removed the job and its grant"

echo "PASS: $pass checks"

# ── k2 ci (0.46.10, prd-ci-on-compute-for-clients-v1 C6a) ─────────────
# CI on K2 Compute, daemon-owned: the daemon watches the repo, runs each
# new tip's [checks.*] from .k2/ci.toml as compute jobs and posts GitHub
# commit statuses. The CLI only calls /cli/ci/* (on the compute router,
# the compute tool id: a K2 terminal sends its passport).
_ci_usage() {
    cat <<'EOF'
k2 ci — CI on K2 Compute: the daemon checks every push (no runner, no cron)
(preview: k2 settings set ciPreview true): off, every k2 ci verb exits 3
(ci_preview_off) and the daemon checks nothing; the owner turns it on
(compute must be on too)

  k2 ci init [--check NAME] [--needs a,b] [--os linux|macos] [--mem GB]
             [--cpus N] [--timeout 1h] [--branches main,integ/*]
             [--remote origin] [--statuses-token NAME] [--force]
             [-- <cmd> [args…]]
                                write .k2/ci.toml; commit it (K2 reads it
                                at each commit it checks)
  k2 ci run [<rev>] [--check a,b] [--ref BRANCH] [--tier ci|standard]
            [--wait [--wait-min 60]] [--json]
                                check one commit now (default HEAD; only
                                committed code is checked)
  k2 ci status [<run|commit|branch>] [--limit N] [--json]
                                the watch and recent runs, or one run
  k2 ci logs <run|commit|branch> [--check NAME] [--failures]
                                each check's job log
  k2 ci watch on [--remote origin] [--branches main,integ/*] [--poll 60]
                 [--tier ci|standard] [--max-inflight 2] [--skip-current]
  k2 ci watch off               the daemon polls the remote (git ls-remote)
  k2 ci watch                   and checks each new tip of those branches
  k2 ci freeze --until <time|3d|12h> [--why "…"] | k2 ci freeze off
                                flag a release freeze (no seat moves)
  --workspace <name>            people: which workspace (default: the
                                one this folder is in)

Release builds (a [release] section in .k2/ci.toml, a seat on the node
owner's lane; k2 study ci-release):
  k2 ci release <version> [<rev> | --sha <commit>] [--build a,b]
                [--tier ci|standard] [--plan] [--wait [--wait-min 180]]
                [--json]        build every [release.builds.*] of the
                                commit (default HEAD) on your seat, as one
                                set: a dual build's two jobs (L1, V1) on
                                nodes that differ (independent, default
                                host), the others beside them. --plan shows
                                where and submits nothing. Exit 0 = passed
                                (with --wait); 3 = no seat, seat_degraded,
                                no_independent_nodes
  k2 ci evidence <run> --out <dir> [--no-files] [--json]
                                once the run passed: the evidence bundle
                                (for the signer) and every job's files,
                                in <dir>/<label>/<their paths>. <dir> goes
                                inside the repo (k2 app updates push reads
                                files there). Signing is a person's, at
                                the signing machine (k2-signer)
  Seat, lane, node roster, L1/V1/L3, the signer, the steps and who does
  each: k2 study ci-release.

Signer pairing (a person at this server, never an agent; Touch ID / vault
password each step). On the signing machine first: sudo k2-signer-admin
pair-begin > pair.json
  k2 ci signer add <CODE.FP> --hello pair.json [--out offer.json]
  k2 ci signer reveal <pairing> --reveal reveal.json   prints the 6-digit
                                SAS here only; type it at the signer
  k2 ci signer finish <pairing> --done done.json
  k2 ci signers                 paired signers;  k2 ci signer remove <fp>

Each [checks.<name>] is one K2 Compute job on the commit (--tier ci,
detached; a newer push to the branch replaces its jobs that haven't
started). The jobs carry your workspace's compute grants: --tier ci needs
a grant with --ci (else use --tier standard). Statuses go to GitHub as
<context>/<check> with the token [statuses] token_secret names, from your
workspace's vault. Agents run all of this for their own workspace.
More: k2 study ci
EOF
}

_ci_exit_for() {
    case "$1" in
        200) echo 0 ;;
        400|413) echo 2 ;;
        404) echo 4 ;;
        000) echo 1 ;;
        *) echo 3 ;;
    esac
}

# _ci_call GET|POST <route> [query|json] → _CI_BODY; a refusal prints
# `code: hint` (and the permission line when cards are on) and exits.
_ci_call() {
    local method="$1" route="$2" payload="${3:-}" raw
    raw="$(_compute_send "$method" "/cli/ci/${route}" "$payload")"
    _CI_STATUS="${raw##*$'\n'}"
    _CI_BODY="${raw%$'\n'*}"
    [ "$_CI_STATUS" = "$raw" ] && _CI_BODY=""
    [ -z "$_CI_STATUS" ] && _CI_STATUS="000"
    if [ "$_CI_STATUS" != "200" ]; then
        if [ "$_CI_STATUS" = "000" ]; then
            echo "daemon_unreachable: the K2 daemon did not answer" >&2
            exit 1
        fi
        printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    v = json.loads(raw)
    e = v.get("error")
    if isinstance(e, dict):
        print("%s: %s" % (e.get("code", "error"), e.get("hint", "")), file=sys.stderr)
    elif isinstance(e, str):
        print("%s: %s" % (e, v.get("message", "")), file=sys.stderr)
    else:
        print(raw[:300], file=sys.stderr)
except Exception:
    print(raw[:300] or "the daemon did not answer", file=sys.stderr)
'
        _permission_note "$_CI_BODY"
        exit "$(_ci_exit_for "$_CI_STATUS")"
    fi
}

# Render a /cli/ci/* answer: $1 = status|run|watch|init|freeze.
_ci_render() {
    printf '%s' "$_CI_BODY" | K2_CI_VERB="$1" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
verb = os.environ["K2_CI_VERB"]
def ago(t):
    if not t: return "-"
    d = int(time.time()) - int(t)
    return "now" if d < 60 else ("%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400)))
def when(t):
    return time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(int(t)))
def watch_line(w):
    if not w:
        return "watch: off (k2 ci watch on)"
    if not w.get("on"):
        return "watch: off (was %s %s; k2 ci watch on)" % (w.get("remote"), ", ".join(w.get("branches") or []))
    line = "watch: on · %s %s · every %ss · %s tier · max %s in flight · last poll %s" % (
        w.get("remote"), ", ".join(w.get("branches") or []), w.get("pollSecs"), "CI" if w.get("tier") == "ci" else "Standard",
        w.get("maxInflight"), ago(w.get("lastPollAt")))
    if w.get("lastError"):
        line += "\n       last poll failed: %s" % w["lastError"]
    return line
def checks_short(r):
    return " ".join("%s=%s" % (c["name"], c["state"]) for c in r.get("checks") or []) or "-"
def run_block(r):
    ref = (" (%s)" % r["ref"]) if r.get("ref") else ""
    rel = r.get("release") if r.get("kind") == "release" else None
    what = ("release %s of %s" % (rel.get("version"), rel.get("project"))) if rel else r["trigger"]
    print("run %s · %s · %s%s · %s · %s tier · %s by %s" % (r["short"], r["state"], r["sha"][:12], ref, what, "CI" if r.get("tier") == "ci" else "Standard", ago(r.get("createdAt")), r.get("by")))
    if r.get("detail"):
        print("  " + r["detail"])
    if rel:
        print("%-5s %-7s %-16s %-8s %-9s %s" % ("JOB", "ROLE", "BUILD", "STATE", "JOB ID", "DESCRIPTION"))
        for c in r.get("checks") or []:
            print("%-5s %-7s %-16s %-8s %-9s %s" % (c["name"], c.get("role") or "-", c.get("build") or "-", c["state"], (c.get("job") or "-")[:8], c.get("description") or ""))
        if r["state"] == "running":
            print("follow: k2 ci status %s · logs: k2 ci logs %s" % (r["short"], r["short"]))
        elif r["state"] == "passed":
            print("next: k2 ci evidence %s --out <dir>   (the bundle and the files, for the signer)" % r["short"])
        else:
            print("logs: k2 ci logs %s --failures" % r["short"])
        return
    s = r.get("statuses")
    if s:
        print("statuses: github.com/%s as %s/<check> (token %s, from this workspace\x27s vault)" % (s["slug"], s["context"], s["tokenSecret"]))
    else:
        print("statuses: none ([statuses] in .k2/ci.toml, on a github.com remote)")
    cs = r.get("checks") or []
    if cs:
        print("%-16s %-8s %-9s %-8s %s" % ("CHECK", "STATE", "JOB", "STATUS", "DESCRIPTION"))
        for c in cs:
            print("%-16s %-8s %-9s %-8s %s" % (c["name"], c["state"], (c.get("job") or "-")[:8], c.get("status") or "-", c.get("description") or ""))
            if c.get("statusError"):
                print("%-16s status not posted: %s" % ("", c["statusError"]))
    if r["state"] == "running":
        print("follow: k2 ci status %s · logs: k2 ci logs %s" % (r["short"], r["short"]))
    elif cs and any(c.get("job") for c in cs):
        bad = [c["name"] for c in cs if c["state"] in ("failure", "error") and c.get("job")]
        print("logs: k2 ci logs %s%s" % (r["short"], (" --check %s --failures" % bad[0]) if bad else ""))
if verb == "status":
    if "run" in v:
        run_block(v["run"])
        sys.exit(0)
    print("workspace %s" % v.get("workspace"))
    print(watch_line(v.get("watch")))
    f = v.get("freeze")
    if f:
        print("freeze: until %s%s" % (when(f["until"]), (" (%s)" % f["why"]) if f.get("why") else ""))
    runs = v.get("runs") or []
    if not runs:
        print("no runs yet (k2 ci run checks HEAD now; k2 study ci)")
        sys.exit(0)
    print("%-9s %-8s %-12s %-16s %-7s %-8s %s" % ("RUN", "STATE", "COMMIT", "REF", "TRIGGER", "AGE", "CHECKS"))
    for r in runs:
        print("%-9s %-8s %-12s %-16s %-7s %-8s %s" % (r["short"], r["state"], r["sha"][:12], (r.get("ref") or "-")[:16], r["trigger"], ago(r.get("createdAt")), checks_short(r)))
elif verb == "run":
    run_block(v["run"])
elif verb == "release-plan":
    print("release %s of %s at %s on %s\x27s seat %s (lane %s); nothing submitted (--plan):" % (v["version"], v["project"], v["sha"][:12], v["owner"], v["seat"], v["lane"]))
    for p in v.get("placed") or []:
        print("  %-4s %-6s build %-16s on %-12s %s" % (p["label"], p["role"], p["build"], p["node"], p["why"]))
elif verb == "watch":
    w = v.get("watch")
    if "tips" in v:
        print(watch_line(w))
        tips = v.get("tips") or []
        if not tips:
            print("no branch matches yet; the first push to one is checked")
        elif v.get("skippedCurrent"):
            print("current tips skipped (%s); only new pushes are checked" % ", ".join("%s @ %s" % (t["branch"], t["sha"][:8]) for t in tips))
        else:
            print("current tips are checked on the next tick (about 15 s): %s" % ", ".join("%s @ %s" % (t["branch"], t["sha"][:8]) for t in tips))
        print("follow: k2 ci status")
    else:
        print("watch off" if v.get("changed") else "the watch was already off")
elif verb == "init":
    print("wrote %s:" % v["path"])
    print()
    sys.stdout.write(v["config"])
    print()
    if v.get("ignored"):
        print("WARNING: .gitignore hides .k2/ci.toml; K2 reads it at each commit, so un-ignore it (git check-ignore -v .k2/ci.toml)")
    steps = []
    if v.get("placeholder"):
        steps.append("put your test command in argv (the placeholder check fails until you do)")
    steps.append("git add .k2/ci.toml && git commit -m \"K2 CI\" && git push")
    steps.append("k2 ci run --wait (checks HEAD now)")
    steps.append("k2 ci watch on (checks every push to [watch] branches)")
    print("next:")
    for i, s in enumerate(steps, 1):
        print("  %d. %s" % (i, s))
elif verb == "freeze":
    f = v.get("freeze")
    print(("freeze until %s%s" % (when(f["until"]), (" (%s)" % f["why"]) if f.get("why") else "")) if f else "no freeze")
'
}

# 3d / 12h / 90m / epoch seconds / an RFC 3339 time → epoch seconds.
_ci_until() {
    python3 - "$1" <<'PY'
import re, sys, time
from datetime import datetime
s = sys.argv[1].strip()
m = re.fullmatch(r"(\d+)([dhm])", s)
if m:
    n = int(m.group(1)) * {"d": 86400, "h": 3600, "m": 60}[m.group(2)]
    print(int(time.time()) + n)
elif re.fullmatch(r"\d{9,11}", s):
    print(int(s))
else:
    try:
        t = datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        sys.exit("usage: --until 3d | 12h | 2026-11-20T00:00:00Z")
    if t.tzinfo is None:
        sys.exit("usage: --until needs a time zone (2026-11-20T00:00:00Z)")
    print(int(t.timestamp()))
PY
}

# k2 ci signer … (0.46.10 C3a, vs-live V17): pair a signing machine
# (k2-signer) with this server. People only: every step needs the owner or
# an Admin at this server, with Touch ID / the vault password. The SAS is
# printed only here, never sent back to whoever carries the files.
cmd_ci_signer() {
    local sub="$1"; shift
    [ "$sub" = "signers" ] && sub="list"
    if [ "$sub" = "signer" ]; then sub="${1:-list}"; [ $# -gt 0 ] && shift; fi
    case "$sub" in
        list|ls)
            _compute_call GET /cli/ci/signers ""
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("this server (controller): %s" % v.get("controllerFp"))
for s in v["signers"]:
    print("signer %s  %s  paired by %s" % (s["signerFp"], s["label"], s["pairedBy"]))
if not v["signers"]:
    print("no signer paired (on the signing machine: sudo k2-signer-admin pair-begin)")
'
            return 0 ;;
    esac
    # A person only (checked after the usage, before any prompt or request).
    _ci_signer_person() {
        if _compute_in_session; then
            echo '{"error":"person_only","hint":"signer pairing is a person'"'"'s, at this server: a terminal outside K2 (Touch ID / vault password)"}' >&2
            exit 3
        fi
    }
    case "$sub" in
        add)
            local code="${1:-}"; [ -n "$code" ] && shift
            local hello="" out="k2-signer-offer.json"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --hello) hello="${2:-}"; shift 2 ;;
                    --out) out="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 ci signer add <CODE.FP> --hello <pair-begin output file> [--out offer.json]" >&2; exit 2 ;;
                esac
            done
            { [ -n "$code" ] && [ -f "$hello" ]; } || { echo "usage: k2 ci signer add <CODE.FP> --hello <pair-begin output file> [--out offer.json]" >&2; exit 2; }
            _ci_signer_person
            local prefix="${code##*.}" hj tok
            hj="$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(json.dumps(v.get("hello", v)))' "$hello")" || { echo "usage: --hello is the JSON pair-begin printed" >&2; exit 2; }
            tok="$(_vault_confirm_token ci-signer "add:$prefix" "Pair a release signer with this server")" || exit 3
            _compute_call POST /cli/ci/signer/add "$(_compute_json K2_CJ_code="$code" K2_CJ_hello_j="$hj" K2_CJ_confirmToken="$tok")" tcp
            tok=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); json.dump(v["offer"], open(sys.argv[1], "w")); print("pairing %s: offer written to %s\non the signing machine: sudo k2-signer-admin pair-reveal < %s > reveal.json\nthen here: k2 ci signer reveal %s --reveal reveal.json" % (v["pairing"], sys.argv[1], sys.argv[1], v["pairing"]))' "$out"
            ;;
        reveal)
            local id="${1:-}"; [ -n "$id" ] && shift
            local rev=""
            [ "${1:-}" = "--reveal" ] && rev="${2:-}"
            { [ -n "$id" ] && [ -f "$rev" ]; } || { echo "usage: k2 ci signer reveal <pairing> --reveal reveal.json" >&2; exit 2; }
            _ci_signer_person
            local rj tok
            rj="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' "$rev")" || { echo "usage: --reveal is the JSON pair-reveal printed" >&2; exit 2; }
            tok="$(_vault_confirm_token ci-signer "$id" "Show the signer pairing code")" || exit 3
            _compute_call POST /cli/ci/signer/reveal "$(_compute_json K2_CJ_pairing="$id" K2_CJ_reveal_j="$rj" K2_CJ_confirmToken="$tok")" tcp
            tok=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("\n    %s\n\nType these 6 digits at the signing machine: sudo k2-signer-admin pair-accept %s > done.json\nthen here: k2 ci signer finish %s --done done.json" % (v["sas"], "<digits>", v["pairing"]))'
            ;;
        finish)
            local id="${1:-}"; [ -n "$id" ] && shift
            local done_f=""
            [ "${1:-}" = "--done" ] && done_f="${2:-}"
            { [ -n "$id" ] && [ -f "$done_f" ]; } || { echo "usage: k2 ci signer finish <pairing> --done done.json" >&2; exit 2; }
            _ci_signer_person
            local dj
            dj="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' "$done_f")" || { echo "usage: --done is the JSON pair-accept printed" >&2; exit 2; }
            _compute_call POST /cli/ci/signer/finish "$(_compute_json K2_CJ_pairing="$id" K2_CJ_done_j="$dj")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("paired: signer %s (%s)" % (v["signer"], v["label"]))'
            ;;
        remove|rm)
            [ -n "${1:-}" ] || { echo "usage: k2 ci signer remove <signer fingerprint>" >&2; exit 2; }
            _ci_signer_person
            _compute_call POST /cli/ci/signer/remove "$(_compute_json K2_CJ_signer="$1")" tcp
            echo "removed"
            ;;
        *) echo "usage: k2 ci signer [list|add|reveal|finish|remove] (k2 ci --help)" >&2; exit 2 ;;
    esac
}

cmd_ci() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _ci_usage; return 0 ;;
    esac
    shift
    # Signer pairing (0.46.10 C3a): its own verbs, people only.
    if [ "$sub" = "signer" ] || [ "$sub" = "signers" ]; then
        case "${1:-}" in -h|--help) _ci_usage; return 0 ;; esac
        cmd_ci_signer "$sub" "$@"
        return $?
    fi
    local a
    for a in "$@"; do
        [ "$a" = "--" ] && break
        case "$a" in -h|--help) _ci_usage; return 0 ;; esac
    done
    local as_json=0 ws="" pos=() argv=() have_cmd=0
    local check="" needs="" os="" mem="" cpus="" timeout="" branches="" remote="" token="" force=0
    local ref="" tier="" wait=0 wait_min="" limit="" failures=0 poll="" max_inflight="" skip=0 until="" why=""
    local sha="" builds="" plan=0 out_dir="" no_files=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; have_cmd=1; argv=("$@"); break ;;
            --json) as_json=1; shift ;;
            --force) force=1; shift ;;
            --plan) plan=1; shift ;;
            --no-files) no_files=1; shift ;;
            --wait) wait=1; shift ;;
            --failures) failures=1; shift ;;
            --skip-current) skip=1; shift ;;
            --workspace|--check|--checks|--needs|--os|--mem|--cpus|--timeout|--branches|--remote|--statuses-token|--ref|--tier|--wait-min|--limit|--poll|--max-inflight|--until|--why|--sha|--build|--builds|--out)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value (k2 ci --help)" >&2; exit 2; }
                case "$1" in
                    --workspace) ws="$2" ;; --check|--checks) check="$2" ;; --needs) needs="$2" ;; --os) os="$2" ;;
                    --mem) mem="${2%G}"; mem="${mem%g}" ;; --cpus) cpus="$2" ;; --timeout) timeout="$(_compute_secs "$2")" || exit 2 ;;
                    --branches) branches="$2" ;; --remote) remote="$2" ;; --statuses-token) token="$2" ;; --ref) ref="$2" ;;
                    --tier) tier="$2" ;; --wait-min) wait_min="$2" ;; --limit) limit="$2" ;; --poll) poll="$(_compute_secs "$2")" || exit 2 ;;
                    --max-inflight) max_inflight="$2" ;; --until) until="$2" ;; --why) why="$2" ;;
                    --sha) sha="$2" ;; --build|--builds) builds="$2" ;; --out) out_dir="$2" ;;
                esac
                shift 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 ci --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    for a in mem cpus wait_min limit max_inflight; do
        case "${!a}" in ''|*[!0-9]*) [ -z "${!a}" ] || { echo "usage: --${a//_/-} is a number" >&2; exit 2; } ;; esac
    done
    case "$tier" in ''|ci|standard|normal) ;; *) echo "usage: --tier ci|standard" >&2; exit 2 ;; esac
    ws="$(_compute_ws "$ws")"
    local q="" body=""
    case "$sub" in
        status)
            q="$(python3 -c '
import sys, urllib.parse
k = ["workspace", "run", "limit"]
print(urllib.parse.urlencode({kk: vv for kk, vv in zip(k, sys.argv[1:]) if vv}))' "$ws" "${pos[0]:-}" "$limit")"
            _ci_call GET status "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render status; fi
            ;;
        run)
            local dir=""
            if _k2_git_usable; then dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"; fi
            body="$(python3 -c '
import json, sys
ws, rev, ref, tier, checks, d = sys.argv[1:7]
o = {k: v for k, v in (("workspace", ws), ("rev", rev), ("ref", ref), ("tier", tier), ("dir", d)) if v}
if checks:
    o["checks"] = [c.strip() for c in checks.split(",") if c.strip()]
print(json.dumps(o))' "$ws" "${pos[0]:-}" "$ref" "$tier" "$check" "$dir")"
            _ci_call POST run "$body"
            if [ "$wait" = 1 ]; then
                local id deadline state
                id="$(printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"]["id"])')"
                deadline=$(( $(date +%s) + ${wait_min:-60} * 60 ))
                while :; do
                    state="$(printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"]["state"])')"
                    [ "$state" != "running" ] && break
                    if [ "$(date +%s)" -ge "$deadline" ]; then
                        [ "$as_json" = 1 ] && printf '%s\n' "$_CI_BODY" || _ci_render run
                        echo "still running after ${wait_min:-60} min: k2 ci status ${id:0:8}" >&2
                        exit 1
                    fi
                    sleep "${K2_CI_WAIT_SECS:-10}"
                    _ci_call GET status "$(python3 -c 'import sys, urllib.parse; print(urllib.parse.urlencode({k: v for k, v in (("workspace", sys.argv[1]), ("run", sys.argv[2])) if v}))' "$ws" "$id")"
                done
                if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render run; fi
                case "$state" in passed|skipped) return 0 ;; *) exit 1 ;; esac
            fi
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render run; fi
            ;;
        release)
            local version="${pos[0]:-}" rev="${pos[1]:-}"
            [ -n "$version" ] || { echo "usage: k2 ci release <version> [<rev> | --sha <commit>] [--build a,b] [--tier ci|standard] [--plan] [--wait [--wait-min 180]] [--json]" >&2; exit 2; }
            if [ -n "$sha" ] && [ -n "$rev" ]; then echo "usage: k2 ci release <version> takes <rev> or --sha, not both" >&2; exit 2; fi
            [ -n "$sha" ] && rev="$sha"
            local dir=""
            if _k2_git_usable; then dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"; fi
            body="$(python3 -c '
import json, sys
ws, version, rev, tier, builds, d, plan = sys.argv[1:8]
o = {k: v for k, v in (("workspace", ws), ("version", version), ("rev", rev), ("tier", tier), ("dir", d)) if v}
if builds:
    o["builds"] = [b.strip() for b in builds.split(",") if b.strip()]
if plan == "1":
    o["plan"] = True
print(json.dumps(o))' "$ws" "$version" "$rev" "$tier" "$builds" "$dir" "$plan")"
            _ci_call POST release "$body"
            if [ "$plan" = 1 ]; then
                if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render release-plan; fi
                return 0
            fi
            if [ "$wait" = 1 ]; then
                local id deadline state
                id="$(printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"]["id"])')"
                deadline=$(( $(date +%s) + ${wait_min:-180} * 60 ))
                while :; do
                    state="$(printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"]["state"])')"
                    [ "$state" != "running" ] && break
                    if [ "$(date +%s)" -ge "$deadline" ]; then
                        [ "$as_json" = 1 ] && printf '%s\n' "$_CI_BODY" || _ci_render run
                        echo "still running after ${wait_min:-180} min: k2 ci status ${id:0:8}" >&2
                        exit 1
                    fi
                    sleep "${K2_CI_WAIT_SECS:-10}"
                    _ci_call GET status "$(python3 -c 'import sys, urllib.parse; print(urllib.parse.urlencode({k: v for k, v in (("workspace", sys.argv[1]), ("run", sys.argv[2])) if v}))' "$ws" "$id")"
                done
                if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render run; fi
                [ "$state" = passed ] || exit 1
                return 0
            fi
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render run; fi
            ;;
        evidence)
            local run="${pos[0]:-}"
            { [ -n "$run" ] && [ -n "$out_dir" ]; } || { echo "usage: k2 ci evidence <run> --out <dir> [--no-files] [--json]" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; o={"run": sys.argv[2]}; o.update({"workspace": sys.argv[1]} if sys.argv[1] else {}); print(json.dumps(o))' "$ws" "$run")"
            _ci_call POST evidence "$body"
            mkdir -p "$out_dir" || { echo "can't make $out_dir" >&2; exit 1; }
            local file
            file="$(printf '%s' "$_CI_BODY" | python3 -c '
import json, os, sys
v = json.load(sys.stdin)
path = os.path.join(sys.argv[1], v["file"])
with open(path, "w") as f:
    json.dump(v["bundle"], f, indent=1, sort_keys=True)
    f.write("\n")
print(path)' "$out_dir")" || exit 1
            local fetched=0 jl jid
            if [ "$no_files" = 0 ]; then
                while IFS=$'\t' read -r jl jid; do
                    [ -n "$jid" ] || continue
                    ( cmd_compute_fetch "$jid" --out "$out_dir/$jl" >/dev/null ) || { echo "fetching job $jid's files failed: k2 compute fetch $jid --out $out_dir/$jl" >&2; exit 1; }
                    fetched=$((fetched + 1))
                done < <(printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
for label, j in sorted(json.load(sys.stdin)["jobs"].items()):
    print("%s\t%s" % (label, j["job"]))')
            fi
            if [ "$as_json" = 1 ]; then
                printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); v["path"]=sys.argv[1]; v.pop("bundle", None); print(json.dumps(v))' "$file"
            else
                printf '%s' "$_CI_BODY" | K2_CI_EV_PATH="$file" K2_CI_EV_DIR="$out_dir" K2_CI_EV_FETCHED="$fetched" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
print("evidence for %s %s (run %s): %s" % (v.get("project"), v.get("version"), v["short"], os.environ["K2_CI_EV_PATH"]))
for label, j in sorted(v["jobs"].items()):
    print("  %-4s %-6s build %-16s job %s on %s, %d file(s)%s" % (label, j["role"], j["build"], j["short"], j["node"], len(j["artifacts"]),
          (" -> %s/%s/" % (os.environ["K2_CI_EV_DIR"], label)) if os.environ["K2_CI_EV_FETCHED"] != "0" else ""))
for build, d in sorted((v.get("dual") or {}).items()):
    c = d["compare"]
    print("  dual %s: %s = %s on %d file(s)%s%s (checked here only; the signer decides)" % (build, d["build"], d["verify"], len(c["matched"]),
          (", DIFFER: " + ", ".join(c["differ"])) if c["differ"] else "", (", in one only: " + ", ".join(c["only_one"])) if c["only_one"] else ""))
for n in v.get("notes") or []:
    print("  note: " + n)
ev = os.path.basename(os.environ["K2_CI_EV_PATH"])
print("next, at the signing machine (a person; k2 study ci-release):")
print("  1. sudo -n -u _k2sign /usr/local/libexec/k2-signer/k2-signer rehearse %s < %s" % (v.get("project"), ev))
print("  2. the same with: evidence %s %s < %s, then challenge, approve and sign each file" % (v.get("project"), v.get("version"), ev))
print("  3. here: k2 app updates push <app> --channel beta --version %s --artifact <platform>=<file> --sig <platform>=<file>.sig --evidence %s" % (v.get("version"), v["short"]))'
            fi
            ;;
        logs|log)
            [ -n "${pos[0]:-}" ] || { echo "usage: k2 ci logs <run|commit|branch> [--check NAME] [--failures]" >&2; exit 2; }
            q="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.urlencode({k: v for k, v in (("workspace", sys.argv[1]), ("run", sys.argv[2])) if v}))' "$ws" "${pos[0]}")"
            _ci_call GET status "$q"
            local lines jid jhead n=0
            lines="$(printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)["run"]
want = [c for c in sys.argv[1].split(",") if c]
for c in r.get("checks") or []:
    if want and c["name"] not in want:
        continue
    print("%s\t%s · job %s · %s · %s" % (c.get("job") or "-", c["name"], (c.get("job") or "-")[:8], c["state"], c.get("description") or ""))' "$check")"
            [ -n "$lines" ] || { echo "ci_check_not_found: no such check in that run (k2 ci status ${pos[0]})" >&2; exit 4; }
            while IFS=$'\t' read -r jid jhead; do
                echo "── $jhead ──"
                if [ "$jid" = "-" ]; then
                    echo "(never submitted: no log)"
                    continue
                fi
                n=$((n + 1))
                if [ "$failures" = 1 ]; then ( cmd_compute_logs "$jid" --failures ) || true; else ( cmd_compute_logs "$jid" ) || true; fi
            done <<<"$lines"
            ;;
        watch)
            local on="${pos[0]:-}"
            if [ -z "$on" ]; then
                q="$(python3 -c 'import sys, urllib.parse; o = {"limit": "1"}; o.update({"workspace": sys.argv[1]} if sys.argv[1] else {}); print(urllib.parse.urlencode(o))' "$ws")"
                _ci_call GET status "$q"
                if [ "$as_json" = 1 ]; then printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("watch")))'; else printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
w = json.load(sys.stdin).get("watch")
print(json.dumps(w, indent=2) if w else "watch: off (k2 ci watch on)")'; fi
                return 0
            fi
            case "$on" in on|off) ;; *) echo "usage: k2 ci watch on|off" >&2; exit 2 ;; esac
            body="$(python3 -c '
import json, sys
ws, on, remote, branches, poll, tier, maxi, skip = sys.argv[1:9]
o = {"on": on == "on"}
if ws: o["workspace"] = ws
if remote: o["remote"] = remote
if branches: o["branches"] = [b.strip() for b in branches.split(",") if b.strip()]
if poll: o["pollSecs"] = int(poll)
if tier: o["tier"] = tier
if maxi: o["maxInflight"] = int(maxi)
if skip == "1": o["skipCurrent"] = True
print(json.dumps(o))' "$ws" "$on" "$remote" "$branches" "$poll" "$tier" "$max_inflight" "$skip")"
            _ci_call POST watch "$body"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render watch; fi
            ;;
        init)
            body="$(python3 -c '
import json, sys
ws, check, needs, os_, mem, cpus, timeout, branches, remote, token, force, have = sys.argv[1:13]
argv = sys.argv[13:]
o = {}
for k, v in (("workspace", ws), ("check", check), ("os", os_), ("remote", remote), ("statusesToken", token)):
    if v: o[k] = v
for k, v in (("memGb", mem), ("cpus", cpus), ("timeoutSecs", timeout)):
    if v: o[k] = int(v)
if needs: o["needs"] = [x.strip() for x in needs.split(",") if x.strip()]
if branches: o["branches"] = [x.strip() for x in branches.split(",") if x.strip()]
if force == "1": o["force"] = True
if have == "1": o["argv"] = argv
print(json.dumps(o))' "$ws" "$check" "$needs" "$os" "$mem" "$cpus" "$timeout" "$branches" "$remote" "$token" "$force" "$have_cmd" ${argv[@]+"${argv[@]}"})"
            if [ "$have_cmd" = 1 ] && [ "${#argv[@]}" -eq 0 ]; then
                echo "usage: k2 ci init … -- <cmd> [args…] (the check's command after --)" >&2
                exit 2
            fi
            _ci_call POST init "$body"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render init; fi
            ;;
        freeze)
            if [ "${pos[0]:-}" = "off" ]; then
                body="$(python3 -c 'import json,sys; o={"off": True}; o.update({"workspace": sys.argv[1]} if sys.argv[1] else {}); print(json.dumps(o))' "$ws")"
            else
                [ -n "$until" ] || { echo "usage: k2 ci freeze --until <3d|12h|2026-11-20T00:00:00Z> [--why \"…\"] | k2 ci freeze off" >&2; exit 2; }
                local secs
                secs="$(_ci_until "$until")" || exit 2
                body="$(python3 -c '
import json, sys
o = {"until": int(sys.argv[2])}
if sys.argv[1]: o["workspace"] = sys.argv[1]
if sys.argv[3]: o["why"] = sys.argv[3]
print(json.dumps(o))' "$ws" "$secs" "$why")"
            fi
            _ci_call POST freeze "$body"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else _ci_render freeze; fi
            ;;
        *) echo "usage: unknown k2 ci verb '$sub' (k2 ci --help)" >&2; exit 2 ;;
    esac
}


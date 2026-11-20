# ── k2 ci (0.46.10, prd-ci-on-compute-for-clients-v1 C6a) ─────────────
# CI on K2 Compute, daemon-owned: the daemon watches the repo, runs each
# new tip's [checks.*] from .k2/ci.toml as compute jobs and posts GitHub
# commit statuses. The CLI only calls /cli/ci/* (on the compute router,
# the compute tool id: a K2 terminal sends its passport).
_ci_usage() {
    cat <<'EOF'
%include cli/text/help/ci.txt
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
            # The code off argv: --code-stdin reads one line from stdin. A
            # <CODE.FP> word still works for now, with a note: a command line
            # is visible to every account on this computer while the command
            # runs.
            local code="" code_stdin=0 hello="" out="k2-signer-offer.json"
            local usage="usage: k2 ci signer add --code-stdin --hello <pair-begin output file> [--out offer.json] < code-file (or <CODE.FP> instead of --code-stdin, visible to every account on this computer)"
            if [ -n "${1:-}" ] && [ "${1#-}" = "$1" ]; then code="$1"; shift; fi
            while [ $# -gt 0 ]; do
                case "$1" in
                    --code-stdin) code_stdin=1; shift ;;
                    # A value flag with no word after it is the usage error
                    # (exit 2), never a shift 2 that can't shift.
                    --hello) [ $# -ge 2 ] || { echo "$usage" >&2; exit 2; }; hello="$2"; shift 2 ;;
                    --out) [ $# -ge 2 ] || { echo "$usage" >&2; exit 2; }; out="$2"; shift 2 ;;
                    *) echo "$usage" >&2; exit 2 ;;
                esac
            done
            [ "$code_stdin" = 0 ] || [ -z "$code" ] || { echo "k2 ci signer add: --code-stdin or <CODE.FP>, not both" >&2; exit 2; }
            { { [ -n "$code" ] || [ "$code_stdin" = 1 ]; } && [ -f "$hello" ]; } || { echo "$usage" >&2; exit 2; }
            _ci_signer_person
            if [ "$code_stdin" = 1 ]; then
                _cli_read_secret_stdin "Pairing code: "
                code="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
                [ -n "$code" ] || { echo "k2 ci signer add: --code-stdin: no code on stdin" >&2; exit 2; }
            else
                echo "note: <CODE.FP> is visible to every account on this computer while the command runs; use --code-stdin" >&2
            fi
            local prefix="${code##*.}" hj tok
            hj="$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(json.dumps(v.get("hello", v)))' "$hello")" || { echo "usage: --hello is the JSON pair-begin printed" >&2; exit 2; }
            tok="$(_vault_confirm_token ci-signer "add:$prefix" "Pair a release signer with this server")" || exit 3
            _compute_call POST /cli/ci/signer/add "$(_compute_json K2_CJ_code_s="$code" K2_CJ_hello_j="$hj" K2_CJ_confirmToken="$tok")" tcp
            tok=""; code=""
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
    local a
    # Signer pairing (0.46.10 C3a): its own verbs, people only.
    if [ "$sub" = "signer" ] || [ "$sub" = "signers" ]; then
        # -h/--help anywhere before a bare -- prints the usage and sends
        # nothing (0.46.17: only the next word was read, so `k2 ci signer
        # remove <fp> -h` removed the signer). No signer verb takes a --:
        # one is a usage error, so a -h after it is never sent as a word.
        if _wants_help "$@"; then _ci_usage; return 0; fi
        for a in "$@"; do
            [ "$a" = "--" ] && { echo "usage: ci signer takes no '--' (k2 help ci)" >&2; exit 2; }
        done
        cmd_ci_signer "$sub" "$@"
        return $?
    fi
    for a in "$@"; do
        [ "$a" = "--" ] && break
        case "$a" in -h|--help) _ci_usage; return 0 ;; esac
    done
    local as_json=0 ws="" pos=() argv=() have_cmd=0
    local check="" needs="" os="" mem="" cpus="" timeout="" branches="" remote="" token="" force=0
    local ref="" tier="" wait=0 wait_min="" limit="" failures=0 poll="" max_inflight="" skip=0 until="" why=""
    local sha="" builds="" plan=0 out_dir="" no_files=0 tests_filter="" days="" only_keys="" reported=""
    local branch="" mkind="" mcount="" mover="" mcooldown="" mto="" mfor=""
    local tadd="" tset="" tremove="" tset_given=0
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
            --flaky|--slow|--failing|--quarantined)
                [ -z "$tests_filter" ] || { echo "usage: one of --flaky, --slow, --failing, --quarantined (k2 ci tests)" >&2; exit 2; }
                tests_filter="${1#--}"; shift ;;
            --days)
                [ -n "${2:-}" ] || { echo "usage: --days needs a value (k2 ci --help)" >&2; exit 2; }
                days="${2%d}"; shift 2 ;;
            # Blacksmith class O1: stats --branch; monitor add/mute flags.
            --branch|--kind|--count|--over|--cooldown|--to|--for)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value (k2 ci --help)" >&2; exit 2; }
                case "$1" in
                    --branch) branch="$2" ;; --kind) mkind="$2" ;; --count) mcount="$2" ;; --over) mover="$2" ;;
                    --cooldown) mcooldown="$2" ;; --to) mto="$2" ;; --for) mfor="$2" ;;
                esac
                shift 2 ;;
            --workspace|--check|--checks|--needs|--os|--mem|--cpus|--timeout|--branches|--remote|--statuses-token|--ref|--tier|--wait-min|--limit|--poll|--max-inflight|--until|--why|--sha|--build|--builds|--out|--only-keys|--reported)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value (k2 ci --help)" >&2; exit 2; }
                case "$1" in
                    --workspace) ws="$2" ;; --check|--checks) check="$2" ;; --needs) needs="$2" ;; --os) os="$2" ;;
                    --mem) mem="${2%G}"; mem="${mem%g}" ;; --cpus) cpus="$2" ;; --timeout) timeout="$(_compute_secs "$2")" || exit 2 ;;
                    --branches) branches="$2" ;; --remote) remote="$2" ;; --statuses-token) token="$2" ;; --ref) ref="$2" ;;
                    --tier) tier="$2" ;; --wait-min) wait_min="$2" ;; --limit) limit="$2" ;; --poll) poll="$(_compute_secs "$2")" || exit 2 ;;
                    --max-inflight) max_inflight="$2" ;; --until) until="$2" ;; --why) why="$2" ;;
                    --sha) sha="$2" ;; --build|--builds) builds="$2" ;; --out) out_dir="$2" ;;
                    --only-keys) only_keys="$2" ;; --reported) reported="$2" ;;
                esac
                shift 2 ;;
            # Blacksmith class B2: k2 ci cache trust.
            --add|--set|--remove)
                [ "$sub" = "cache" ] || { echo "usage: $1 goes with k2 ci cache trust" >&2; exit 2; }
                [ -n "${2:-}" ] || { echo "usage: $1 needs a branch (k2 ci cache trust --help)" >&2; exit 2; }
                case "$1" in --add) tadd="$2" ;; --set) tset="$2"; tset_given=1 ;; --remove) tremove="$2" ;; esac
                shift 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 ci --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    for a in mem cpus wait_min limit max_inflight days mcount; do
        case "${!a}" in ''|*[!0-9]*) [ -z "${!a}" ] || { echo "usage: --${a//_/-} is a number" >&2; exit 2; } ;; esac
    done
    if [ "$sub" != "tests" ] && [ -n "$tests_filter" ]; then
        echo "usage: --flaky, --slow, --failing and --quarantined go with k2 ci tests" >&2; exit 2
    fi
    if [ -n "$days" ] && [ "$sub" != "tests" ] && [ "$sub" != "stats" ]; then
        echo "usage: --days goes with k2 ci tests and k2 ci stats" >&2; exit 2
    fi
    if [ -n "$branch" ] && [ "$sub" != "stats" ] && [ "$sub" != "monitor" ]; then
        echo "usage: --branch goes with k2 ci stats and k2 ci monitor add (watched branches: --branches)" >&2; exit 2
    fi
    if [ "$sub" != "monitor" ] && { [ -n "$mkind" ] || [ -n "$mcount" ] || [ -n "$mover" ] || [ -n "$mcooldown" ] || [ -n "$mto" ] || [ -n "$mfor" ]; }; then
        echo "usage: --kind, --count, --over, --cooldown, --to and --for go with k2 ci monitor" >&2; exit 2
    fi
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
                    sleep "$(_envn CI_WAIT_SECS 10)"
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
ws, version, rev, tier, builds, d, plan, keys = sys.argv[1:9]
o = {k: v for k, v in (("workspace", ws), ("version", version), ("rev", rev), ("tier", tier), ("dir", d)) if v}
if builds:
    o["builds"] = [b.strip() for b in builds.split(",") if b.strip()]
if plan == "1":
    o["plan"] = True
if keys:
    o["onlyKeys"] = [k.strip() for k in keys.split(",") if k.strip()]
print(json.dumps(o))' "$ws" "$version" "$rev" "$tier" "$builds" "$dir" "$plan" "$only_keys")"
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
                    sleep "$(_envn CI_WAIT_SECS 10)"
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
            { [ -n "$run" ] && [ -n "$out_dir" ]; } || { echo "usage: k2 ci evidence <run> --out <dir> [--no-files] [--reported <file>] [--json]" >&2; exit 2; }
            if [ -n "$reported" ] && [ "$no_files" = 1 ]; then
                echo "usage: --reported reads the fetched files: drop --no-files" >&2; exit 2
            fi
            body="$(python3 -c 'import json,sys; o={"run": sys.argv[2]}; o.update({"workspace": sys.argv[1]} if sys.argv[1] else {}); print(json.dumps(o))' "$ws" "$run")"
            _ci_call POST evidence "$body"
            # Every name the answer puts under --out is checked before
            # anything is written (exit 3 on the first one refused).
            printf '%s' "$_CI_BODY" | _compute_out_check evidence "$out_dir" || exit $?
            mkdir -p "$out_dir" || { echo "can't make $out_dir" >&2; exit 1; }
            local file
            file="$(printf '%s' "$_CI_BODY" | python3 -c "$_CI_OUT_PY" bundle "$out_dir")" || exit 1
            # Bundle v3: each build statement as <dir>/provenance/<label>.intoto.jsonl,
            # and <dir>/trust/ to check them without K2.
            local n_st n_trust
            n_st="$(printf '%s' "$_CI_BODY" | _compute_write_statements "$out_dir")" || exit 1
            n_trust="$(printf '%s' "$_CI_BODY" | _compute_write_trust "$out_dir")" || exit 1
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
            if [ -n "$reported" ]; then
                # The named file of each job that wrote one goes into the
                # bundle (base64): the signer hashes it to the artifact in
                # that job's signed receipt before it reads anything in it.
                python3 -I - "$file" "$out_dir" "$reported" <<'PY' || exit 1
import base64, json, os, sys
path, out, name = sys.argv[1:4]
if name.startswith("/") or ".." in name.split("/"):
    sys.exit("--reported %s: a path inside each job's files (dist/build-info.json)" % name)
b = json.load(open(path))
added = []
for label in sorted(b["receipts"]):
    f = os.path.join(out, label, name)
    if os.path.isfile(f):
        b.setdefault("reported", {}).setdefault(label, {})[name] = base64.b64encode(open(f, "rb").read()).decode()
        added.append(label)
if not added:
    sys.exit("--reported %s: no job of the run wrote that file" % name)
with open(path, "w") as fh:
    json.dump(b, fh, indent=1, sort_keys=True)
    fh.write("\n")
print("reported %s of %s carried in the bundle" % (name, ", ".join(added)), file=sys.stderr)
PY
            fi
            if [ "$as_json" = 1 ]; then
                printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); v["path"]=sys.argv[1]; v.pop("bundle", None); print(json.dumps(v))' "$file"
            else
                printf '%s' "$_CI_BODY" | K2_CI_EV_PATH="$file" K2_CI_EV_DIR="$out_dir" K2_CI_EV_FETCHED="$fetched" K2_CI_EV_ST="$n_st" K2_CI_EV_TRUST="$n_trust" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
print("evidence for %s %s (run %s): %s" % (v.get("project"), v.get("version"), v["short"], os.environ["K2_CI_EV_PATH"]))
print("  build statements: %s in %s/provenance/; %s file(s) to check them in %s/trust/" % (os.environ["K2_CI_EV_ST"], os.environ["K2_CI_EV_DIR"], os.environ["K2_CI_EV_TRUST"], os.environ["K2_CI_EV_DIR"]))
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
        provenance)
            # Build attestations S3: every job's build statement of a run, and trust/.
            local prun="${pos[0]:-}"
            { [ -n "$prun" ] && [ -n "$out_dir" ]; } || { echo "usage: k2 ci provenance <run> --out <dir> [--json]" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; o={"run": sys.argv[2]}; o.update({"workspace": sys.argv[1]} if sys.argv[1] else {}); print(json.dumps(o))' "$ws" "$prun")"
            _ci_call POST provenance "$body"
            printf '%s' "$_CI_BODY" | _compute_out_check provenance "$out_dir" || exit $?
            mkdir -p "$out_dir" || { echo "can't make $out_dir" >&2; exit 1; }
            local p_st p_trust
            p_st="$(printf '%s' "$_CI_BODY" | _compute_write_statements "$out_dir")" || exit 1
            p_trust="$(printf '%s' "$_CI_BODY" | _compute_write_trust "$out_dir")" || exit 1
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; return 0; fi
            printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d, n_st, n_trust = sys.argv[1:4]
print("build statements of run %s: %s in %s/provenance/; %s file(s) to check them in %s/trust/" % (v["short"], n_st, d, n_trust, d))
for name, s in sorted((v.get("statements") or {}).items()):
    print("  %-16s job %s, %d file(s) -> provenance/%s.intoto.jsonl" % (name, s["short"], s["files"], name))
for name, why in sorted((v.get("missing") or {}).items()):
    print("  %-16s no statement: %s" % (name, why))
print("check one: k2 compute verify %s/provenance/<check>.intoto.jsonl --file <its file>" % d)' "$out_dir" "$p_st" "$p_trust"
            ;;
        stats)
            # Blacksmith class O1 (§4.5.2): per check, from this workspace's runs.
            [ -z "${pos[0]:-}" ] || { echo "usage: k2 ci stats [--check NAME] [--branch B] [--days 14] [--json]" >&2; exit 2; }
            q="$(python3 -c '
import sys, urllib.parse
ws, check, branch, days = sys.argv[1:5]
print(urllib.parse.urlencode({k: v for k, v in (("workspace", ws), ("check", check), ("branch", branch), ("days", days)) if v}))' "$ws" "$check" "$branch" "$days")"
            _ci_call GET stats "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; return 0; fi
            printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def d(s):
    if s is None: return "-"
    s = int(s)
    return ("%ds" % s) if s < 60 else ("%dm%02ds" % (s // 60, s % 60)) if s < 3600 else ("%dh%02dm" % (s // 3600, (s % 3600) // 60))
def pc(x):
    return "-" if x is None else ("%d%%" % round(x * 100))
scope = "".join([(" · check " + v["check"]) if v.get("check") else "", (" · branch " + v["branch"]) if v.get("branch") else ""])
print("CI stats · %s · last %d days%s" % (v["workspace"], v["days"], scope))
rows = v.get("checks") or []
if not rows:
    print("  no ended checks in that window (k2 ci status shows runs)")
    sys.exit(0)
print("  %-16s %5s %7s %8s %8s %8s %8s %6s %6s %8s %6s" % ("check", "runs", "failed", "p50", "p95", "wait50", "wait95", "fail%", "flaky%", "cpu-h", "cache"))
for c in rows:
    print("  %-16s %5d %7d %8s %8s %8s %8s %6s %6s %8.2f %6s" % (c["check"][:16], c["runs"], c["failed"], d(c.get("p50Secs")), d(c.get("p95Secs")), d(c.get("p50WaitSecs")), d(c.get("p95WaitSecs")), pc(c.get("failureRate")), pc(c.get("flakyRate")), c.get("cpuHours") or 0, pc(c.get("cacheHitRate"))))
    if c.get("errors"):
        print("  %-16s %d ended in error (never ran, superseded or past the deadline)" % ("", c["errors"]))
print("  fail% = failed ÷ (passed + failed); flaky% = runs with a flaky test (k2 ci tests --flaky); cache = runs that started from a cache generation (- : none reported yet)")'
            ;;
        monitors)
            [ -z "${pos[0]:-}" ] || { echo "usage: k2 ci monitors [--json]" >&2; exit 2; }
            q="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.urlencode({"workspace": sys.argv[1]} if sys.argv[1] else {}))' "$ws")"
            _ci_call GET monitors "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; return 0; fi
            printf '%s' "$_CI_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
ms = v.get("monitors") or []
print("CI monitors · %s" % v["workspace"])
if not ms:
    print("  none: k2 ci monitor add <name> --kind fail-streak --branch main (k2 study ci-stats)")
    sys.exit(0)
now = int(time.time())
for m in ms:
    to = m["to"]
    where = ("Thread " + to["target"]) if to["kind"] == "thread" else ("workspace " + to["target"])
    muted = (" · muted %dh more" % max(1, (m["mutedUntil"] - now) // 3600)) if m.get("mutedUntil") and m["mutedUntil"] > now else ""
    c = m["cooldownSecs"]
    cd = ("%dh" % (c // 3600)) if c % 3600 == 0 else ("%dm" % (c // 60))
    print("  %-20s %-11s %s → %s · cooldown %s · fired %d%s" % (m["name"], m["kind"], m["watches"], where, cd, m["fired"], muted))'
            ;;
        monitor)
            local mact="${pos[0]:-}" mname="${pos[1]:-}"
            local musage="usage: k2 ci monitor add <name> --kind fail-streak|slow|queue-wait|new-flaky [--branch B] [--check C] [--count 3] [--over 1.5x|10m] [--cooldown 6h] [--to <workspace>|thread[:<addr>]]  |  k2 ci monitor mute <name> --for 1d|off  |  k2 ci monitor rm <name>"
            case "$mact" in add|mute|rm|remove) ;; *) echo "$musage" >&2; exit 2 ;; esac
            { [ -n "$mname" ] && [ -z "${pos[2]:-}" ]; } || { echo "$musage" >&2; exit 2; }
            [ "$mact" = "add" ] && [ -z "$mkind" ] && { echo "usage: k2 ci monitor add needs --kind fail-streak|slow|queue-wait|new-flaky" >&2; exit 2; }
            [ "$mact" = "mute" ] && [ -z "$mfor" ] && { echo "usage: k2 ci monitor mute <name> --for 1d (or --for off)" >&2; exit 2; }
            [ "$mact" = "remove" ] && mact="rm"
            body="$(_compute_json K2_CJ_workspace="$ws" K2_CJ_action="$mact" K2_CJ_name="$mname" K2_CJ_kind="$mkind" K2_CJ_check="$check" K2_CJ_branch="$branch" \
                K2_CJ_count_n="$mcount" K2_CJ_over="$mover" K2_CJ_cooldown="$mcooldown" K2_CJ_to="$mto" K2_CJ_for="$mfor")"
            _ci_call POST monitor "$body"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; return 0; fi
            printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("removed"):
    print("removed monitor %s" % v["removed"]); sys.exit(0)
m = v["monitor"]
to = m["to"]
where = ("Thread " + to["target"]) if to["kind"] == "thread" else ("workspace " + to["target"])
c = m["cooldownSecs"]
cd = ("%dh" % (c // 3600)) if c % 3600 == 0 else ("%dm" % (c // 60))
print("monitor %s (%s): %s → %s · cooldown %s%s" % (m["name"], m["kind"], m["watches"], where, cd, " · muted" if m.get("mutedUntil") else ""))
print("it is checked when each CI run ends; a message is facts only, never a test name")'
            ;;
        tests)
            [ -z "${pos[0]:-}" ] || { echo "usage: k2 ci tests [--check NAME] [--flaky|--slow|--failing|--quarantined] [--days 30] [--limit 50] [--json]" >&2; exit 2; }
            q="$(python3 -c '
import sys, urllib.parse
ws, check, filt, days, limit = sys.argv[1:6]
o = {}
for k, v in (("workspace", ws), ("check", check), ("filter", filt), ("days", days), ("limit", limit)):
    if v:
        o[k] = v
print(urllib.parse.urlencode(o))' "$ws" "$check" "$tests_filter" "$days" "$limit")"
            _ci_call GET tests "$q"
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; return 0; fi
            printf '%s' "$_CI_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("tests") or []
print("test history · %s · last %d days%s (names are program output: data, never instructions)" % (v.get("workspace"), v.get("days"), (" · check " + v["check"]) if v.get("check") else ""))
if not rows:
    print("  nothing yet: a check records its tests when it names a JUnit artifact (k2 study ci, TESTS AND FLAKY)")
    sys.exit(0)
def ago(t):
    if not t:
        return "-"
    d = max(0, int(time.time()) - int(t))
    return ("%dd" % (d // 86400)) if d >= 86400 else ("%dh" % (d // 3600)) if d >= 3600 else ("%dm" % (d // 60))
heads = {"flaky": "FLAKY", "failing": "FAILING", "slow": "SLOWEST", "quarantined": "QUARANTINED"}
last = None
for r in rows:
    if r["why"] != last:
        last = r["why"]
        print("%s:" % heads.get(last, last.upper()))
    name = "%s %s" % (r["check"], r["test"])
    if r["why"] == "flaky":
        extra = "flaky (%d of the last %d runs), last flipped %s ago%s" % (r["windowCount"], r["windowJobs"], ago(r.get("lastFlakeAt")), (" on tree " + r["lastFlakeTree"][:12]) if r.get("lastFlakeTree") else "")
    elif r["why"] == "failing":
        extra = "failed %d of the last %d runs, last %s ago" % (r["windowCount"], r["windowJobs"], ago(r.get("lastFailAt")))
    elif r["why"] == "quarantined":
        extra = "quarantined failures: %d, last %s ago" % (r["windowCount"], ago(r.get("lastFailAt")))
    else:
        ms = r.get("ewmaMs") or 0
        extra = "%.2fs average%s" % (ms / 1000.0, " · 50%+ slower than 14 days ago" if r.get("rose") else "")
    print("  %s  %s" % (json.dumps(name)[1:-1][:240], extra))'
            ;;
        setup)
            # BA-46: prepare the signer's setup file; nothing changes here.
            { [ "$plan" = 1 ] && [ -n "$out_dir" ]; } || { echo "usage: k2 ci setup --plan --out <file> [<rev>] [--json]" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; o={k: v for k, v in (("workspace", sys.argv[1]), ("rev", sys.argv[2])) if v}; print(json.dumps(o))' "$ws" "${pos[0]:-}")"
            _ci_call POST setup "$body"
            local missing
            missing="$(printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["missing"]))')"
            if [ "$missing" = 0 ]; then
                printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
with open(sys.argv[1], "w") as f:
    json.dump(v["setup"], f, indent=1, sort_keys=True)
    f.write("\n")' "$out_dir" || { echo "can't write $out_dir" >&2; exit 1; }
            fi
            if [ "$as_json" = 1 ]; then
                printf '%s' "$_CI_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); v.pop("setup", None); print(json.dumps(v))'
            else
                printf '%s' "$_CI_BODY" | K2_CI_SETUP_OUT="$out_dir" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
print("signer setup for project %s at %s (controller %s, owner %s, lane %s):" % (v["project"], v["sha"][:12], v["controllerFp"][:16] + "...", v["owner"], v["lane"]))
if v["missing"]:
    print("still missing on this server (no file written):")
    for m in v["missing"]:
        print("  - " + m)
    sys.exit(0)
r = v["roster"]
print("  node roster seq %s: %s" % (r["seq"], ", ".join("%s (%s)" % (e["name"], e["status"]) for e in r["entries"])))
out = os.environ["K2_CI_SETUP_OUT"]
print("wrote %s. A person at the signing machine (never an agent):" % out)
print("  1. sudo k2-signer-admin apply-setup %s        shows every statement; compare the fingerprints" % os.path.abspath(out))
print("  2. the same with the sha256 it prints, to apply them")'
            fi
            [ "$missing" = 0 ] || exit 3
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
        cache)
            [ "${pos[0]:-}" = "trust" ] || { echo "usage: k2 ci cache trust [--add B | --set a,b | --remove B] [--json]" >&2; exit 2; }
            if [ -z "$tadd$tremove" ] && [ "$tset_given" = 0 ]; then
                q="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.urlencode({"workspace": sys.argv[1]} if sys.argv[1] else {}))' "$ws")"
                _ci_call GET cache/trust "$q"
            else
                body="$(python3 -c '
import json, sys
ws, add, st, given, rm = sys.argv[1:6]
o = {"workspace": ws} if ws else {}
split = lambda x: [b.strip() for b in x.split(",") if b.strip()]
if add: o["add"] = split(add)
if given == "1": o["set"] = split(st)
if rm: o["remove"] = split(rm)
print(json.dumps(o))' "$ws" "$tadd" "$tset" "$tset_given" "$tremove")"
                _ci_call POST cache/trust/set "$body"
            fi
            if [ "$as_json" = 1 ]; then printf '%s\n' "$_CI_BODY"; else
                printf '%s' "$_CI_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v.get("branches") or []
print("trusted branches (%s): %s%s" % (v.get("workspace"), ", ".join(b) or "none", " (the default)" if v.get("default") else ""))
if v.get("by"): print("set by %s" % v["by"])
print("remote: %s" % (v.get("remote") or "none recorded yet (a person runs k2 ci cache trust --set <branches>; until then no check saves the shared copy)"))
print(v.get("note", ""))'
            fi
            ;;
        *) echo "usage: unknown k2 ci verb '$sub' (k2 ci --help)" >&2; exit 2 ;;
    esac
}


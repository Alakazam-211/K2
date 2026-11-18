# ── App runtime (prd-k2-app-runtime-v1 stage 1) ─────────────────────
# k2 app build/validate (S6), deploy/rollback/stop/start/restart/install
# (S7), list/inspect/history/logs/errors/uninstall (S8). Behind Apps (beta)
# (Settings apps, on by default; K2_APPS=1 forces it). Exit codes: 3 infra/permission,
# 2 bad usage or a pending state, 1 a failed check (validate errors, a
# failed health gate), 0 ok.

_app_rt_usage() {
    cat <<'EOF'
%include cli/text/help/app/_runtime.txt
EOF
}

# App runtime S10: the box doctor. Exit 0 ready, 1 not ready, 3 infra.
_app_box_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v.get("box", v)
linux = b.get("linux", True) is not False
if linux:
    print("App box: %s (runtime %s%s)" % ("ready" if b.get("ready") else "NOT READY", b.get("runtime", "?"),
          "" if b.get("runtimeRecorded") else ", default"))
else:
    print("App box: not a Linux box. Apps run on Linux only; there is no box prep to do here.")
for c in b.get("checks", []):
    line = "  %-5s %-17s %s" % (c.get("status", "?"), c.get("id", ""), c.get("detail", ""))
    if c.get("fix") and c.get("status") in ("fail", "warn", "unknown"):
        line += "  -> " + c["fix"]
    print(line)
# 0.46.3: the automatic box prep (the owner'"'"'s appsAutoPrepare).
ap = v.get("autoPrepare")
if isinstance(ap, dict) and linux:
    o = ap.get("outcome") or {}
    st = o.get("state") or "not run yet"
    extra = o.get("why") or o.get("error") or ", ".join(o.get("changed") or [])
    print("Automatic prep: %s; last: %s%s" % ("on" if ap.get("enabled") else "off (appsAutoPrepare)", st, (" — " + extra) if extra else ""))
    if ap.get("undoneAt"):
        print("  paused: the owner undid the box setup (k2 app box auto-prepare retry resumes it)")
    elif ap.get("blocked") or st == "blocked":
        print("  stopped for good: no retry until K2 updates (k2 app box auto-prepare retry)")
# 0.46.3: --cmd services no wall runs (not part of "ready").
legacy = v.get("legacyServices") or []
if legacy:
    # No cut-over is set (Rosson 2026-10-26): they keep running.
    print("Legacy --cmd services (unwalled; they keep running, move each to an App when ready):")
    for l in legacy:
        print("  %-20s %-14s %s  -> %s" % (l.get("name"), l.get("workspaceHandle") or "", l.get("label", ""), l.get("hint", "")))
'
}

# Lane P: what box prep offers to install (stdin = the podman_install_needed refusal).
_app_box_podman_offer() {
    python3 -c '
import json, sys
e = json.load(sys.stdin).get("error", {})
p = e.get("podman", {})
b = p.get("bundle", {})
print(e.get("hint", ""))
print("  K2 Podman %s (%s), sha256-pinned:" % (b.get("version"), b.get("source")))
print("    from   %s" % b.get("url"))
for w in b.get("writes", []):
    print("    writes %s" % w)
print("    %s" % b.get("distroPodman", ""))
'
}

# k2 app doctor <app> (0.46.11): is everything approved in the running App
# (box services, its database ports, a container's missing database door)?
# Exit 0 all applied, 1 something approved isn't in the running App.
_app_reach_doctor() {
    local app="$1" json_out="$2"
    _compute_call GET /cli/app/box-service "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); r=v.get("reach") or {}; r["app"]=v.get("app"); r["wall"]=v.get("wall"); print(json.dumps(r))'
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("reach") or {}
state = "running" if r.get("running") else "not running"
print("%s (%s, %s): %s" % (v.get("app"), v.get("wall"), state, "everything approved is applied" if r.get("ok") else "NOT ALL APPLIED"))
rule = r.get("rule")
if rule:
    db = rule.get("dbPorts")
    print("  rule      gateway %s, box ports %s, Postgres %s" % (rule.get("gwPort"), ", ".join(str(p) for p in rule.get("boxPorts") or []) or "none",
          "the whole band" if db is None else (", ".join(str(p) for p in db) or "none")))
door = r.get("dbDoor")
if door:
    print("  db door   127.0.0.1:%s in the wall, %s open session(s), databases: %s" % (door.get("inWallPort"), door.get("openSessions"),
          ", ".join(door.get("databases") or []) or "none"))
for f in r.get("findings") or []:
    print("  %-26s %s" % (f.get("id"), f.get("detail")))'
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if (json.load(sys.stdin).get("reach") or {}).get("ok", True) else 1)' || exit 1
}


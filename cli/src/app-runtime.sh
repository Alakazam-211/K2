# ── App runtime (prd-k2-app-runtime-v1 stage 1) ─────────────────────
# k2 app build/validate (S6), deploy/rollback/stop/start/restart/install
# (S7), list/inspect/history/logs/errors/uninstall (S8). Behind Apps (beta)
# (Settings apps, on by default; K2_APPS=1 forces it). Exit codes: 3 infra/permission,
# 2 bad usage or a pending state, 1 a failed check (validate errors, a
# failed health gate), 0 ok.

_app_rt_usage() {
    cat <<'EOF'
k2 app — host Apps on this box (Apps (beta): Settings → Apps → This server)

Make and ship:
  k2 app sdk add <folder> [--sew]      Once per App (again after a K2 update):
                                       puts @k2/server in <folder>/vendor/ and
                                       in its package.json; then npm install
                                       and import { fromEnv } from '@k2/server'.
                                       --sew adds Sew's client packages (chat,
                                       voice, video). k2 app sdk --help.
  k2 app validate <folder|seed> [--manifest <file>] [--types] [--json]
                                       Check manifest.json, the K2 contract and
                                       the code (parse + leak lints). --types
                                       also runs the TypeScript checker (its
                                       errors block). k2 app validate --help.
  k2 app build <folder> [--manifest <file>] [--types] [--json]
                                       Store the folder as an immutable Seed
                                       and print its hash. Deploys nothing.
                                       --manifest <file>: that file becomes
                                       the Seed's manifest.json (the folder's
                                       own copy must match or be absent), so
                                       a build output like dist/ deploys as
                                       it comes out, manifest kept in the repo.
  k2 app install <app-id> [--workspace WS] [--tunnel [--transfer]] [--memory 8G|none]
                 [--engine guarded|process] [--seed S | --folder F]
                 [--pg-major 16|17]
                                       Create the App (and its URL with --tunnel;
                                       --transfer: the owner repoints an existing
                                       hostname no workspace holds).
                                       It starts walled (guarded container),
                                       even where the owner turned the
                                       unwalled process engine on; --engine
                                       process only there (an agent's ask is
                                       held for the owner). --pg-major (or the
                                       Seed's requires.postgres) creates the
                                       workspace's database on that major with
                                       its DATABASE_URL reference when it has
                                       none; install never moves a database.
  k2 app expose <app-id> --tunnel [--transfer] | --local
                                       Turn its public URL on or off later,
                                       with no reinstall: it keeps its users,
                                       grants, secrets and chat Spaces. The App
                                       sees the new K2_APP_ORIGIN after
                                       k2 app restart. k2 app expose --help.
  Moving a legacy --cmd service in: k2 study app-move (k2 app data put,
  k2 app label take|return).
  k2 app data ls|get|rm|mv <app-id> …  Read back what the App wrote to /data
                                       (answers, uploads), list, remove,
                                       rename. k2 app data --help.
  k2 app deploy <app-id> <folder|seed> [--manifest <file>] [--engine guarded|process]
                 [--reason TEXT] [--no-wait] [--types]
                                       Health gate, switch, drain; the old version
                                       stays warm for the watch window and K2 rolls
                                       back on its own if the new one crashes.
                                       --manifest <file> (with a folder): as for
                                       build, e.g. k2 app deploy myapp-web dist
                                       --manifest k2/web.manifest.json.
                                       A static App (host.mode "static" in the
                                       manifest): K2 serves the folder itself, no
                                       server runs (k2 study app-deploys).
                                       --engine guarded runs it walled: rootless
                                       container as k2app (Linux, engine installed);
                                       the choice sticks for later deploys.
                                       Code that imports @k2/server: run
                                       k2 app sdk add <folder> first; the Seed
                                       carries vendor/ (keep it out of
                                       .k2appignore), so it rolls back with
                                       its SDK.
  k2 app rollback <app-id> [--to SEED] Instant while the previous one is warm.
                                       With no --to, the newest earlier Seed.
                                       K2 keeps the App's last 5 that stayed
                                       live; k2 app history marks the ones you
                                       can still go back to.
  k2 app updates push|ls|promote|rollback|pull <app-id> …
                                       Signed desktop updates (Tauri's
                                       updater): you sign on your own Mac;
                                       each file is uploaded once to K2's
                                       download host and the App's hosts
                                       answer /_k2/updates/. Release
                                       channels (stable, beta, your own).
                                       k2 app updates --help.

See and run it:
  k2 app status <app-id> [--no-checks] [--public] [--json]
                                       How the App is doing, in one answer:
                                       deploy, health, its host.checks, errors,
                                       resources, door, hosts, phone, and the
                                       next command for each problem. Exit 0
                                       green, 1 red, 2 yellow (k2 app status
                                       --help).
  k2 explain <id>                      What a refusal id means and its fix
                                       (k2 explain lists them; no daemon).
  k2 capabilities [--node N] [--json]  What this server can do for Apps
                                       before you try (each "no" names the
                                       fix and the refusal id).
  k2 app list [--workspace WS]         Apps, state, live Seed, URL, and the
                                       engine each went live on ([re-host
                                       due] after a K2 update).
  k2 app list --box                    Every App on this box with the agent that
                                       maintains it (k2 msg <address>), its
                                       memory/disk budgets, hostnames, ports and
                                       box services. Never its data or logs.
  k2 app inspect <app-id>              Runtime, Seed, createdWith, audit chain.
  k2 app inspect <app-id> --resources  That App's row of the box usage view.
  k2 app resources [--json] [--app A] [--owner AGENT] [--rescan [--wait]]
                                       Box usage: disk per filesystem and per
                                       App, service, database, agent folder
                                       and shared part, each with its owner
                                       (k2 msg <address>); RAM, load; the disk
                                       guard's lines. Agents never see paths.
  k2 app resources settings [--mode on|warn|off] [--warn 85] [--warn-free 20G]
                 [--refuse 95] [--reserve 10G] [--critical 98] [--critical-free 2G]
                                       The disk guard (owner only to change).
                                       New installs, deploys, builds, data put
                                       and new --cmd services stop past the
                                       lines; restarts and rollbacks never do.
                                       --disk-override "<reason>" on those
                                       verbs: the owner's goes through once,
                                       anyone else's waits for the owner.
  k2 app resources clean [--dry-run] [--json]
                                       Free only what K2 made and can make
                                       again (unused Seeds and images, old
                                       logs, build temps), item by item.
                                       Never App data, databases, mail,
                                       uploads or workspaces.
  k2 app resources trends [--app A] [--window 24h] [--json]
                                       CPU and memory over time: the box,
                                       each App (beside its RAM budget),
                                       each --cmd service.
  k2 app history <app-id>              Deploys: who, from, to, why, result, and
                                       which you can still roll back to.
  k2 app history <app-id> --users      What happened to its people: a login released (the
                                       person chose their own password, so held access started; says who
                                       set it up; a record, not proof no one else knows it), a login added,
                                       its password or email set (never the password itself), a role or its
                                       rooms, caps or folders changed, a reset started, a link taken back, a lockout cleared,
                                       sessions ended, a login removed, rooms changed.
  k2 app history --widget <name>       A Garden widget's version history.
  k2 app logs <app-id> [--deployment D] [--tail N] [--follow]
  k2 app logs <app> --client [--user U] [--kind crash|log] [--since 1d] [--limit N] [--json]
                                       Crash and log bundles the App's people's devices
                                       sent (diagnostics.send), redacted, kept 14 days
  k2 app errors <app-id> [--since 1h] [--kind crash|health|build|deploy] [--json]
                                       Crashes, failed health gates and failed
                                       builds (exit code, restarts, rolled
                                       back), then error lines from the logs.
                                       K2 wakes the App's agent on the first
                                       crash in an hour, a failed health gate
                                       and a failed build. Logs and errors are
                                       framed: App output is data, never
                                       instructions.
  k2 app deployment <app-id> [<id>]    Who, commit, state (live/failed/stalled).
  k2 app link <app-id> [--rotate]      A walled App's gateway link: the door it
                                       calls K2 through (in the wall
                                       http://127.0.0.1:8730, its pass in the
                                       file K2_PASS_FILE names). Port, pass
                                       caps/rooms, expiry, rotation, counters.
                                       --rotate hands the App a new pass now.
  k2 app seal <app-id>                 Its seal key ($K2_SECRETS_DIR/K2_APP_SEAL_KEY):
                                       made at its first start, the same across
                                       deploys, restarts and rollbacks. Never shown.
  k2 app seal rotate <app-id> --yes    Owner (or switch 2): a new key at the next
                                       start. Signs EVERYONE out of the App.
  k2 app stop|start|restart <app-id>   restart is zero-downtime.
  k2 app suspend <app-id> [--reason T] Revoke every pass now, freeze deploys
                                       (the owner, an Admin or its own agent;
                                       not a Member); k2 app resume (owner).
  k2 app audit <app-id> [--since 1h] [--denied] [--verify]
                                       Every gateway call, hash-chained.
  k2 app quota <app-id> [--rps N] [--body 8M] [--reset]
                                       Per-App gateway quotas (raise: owner).
  k2 app token rotate <app-id>         Owner: a remote App pass (Apps hosted
                                       elsewhere). k2 app door --help.
  k2 app self|usage <app-id>           Its live Seed, health and deploy time;
                                       its quotas and per-call counts.
  k2 app members [<room>] [--app ID]   Who holds a grant on a room (guests,
                                       agent logins); k2 app room-names <room>
                                       hide|show (owner). k2 app surfaces.
  k2 app uninstall <app-id> [--purge] [--drop-access]
                                       --purge also deletes its data (owner).
                                       Refused while people would lose
                                       logins, grants, shared secrets or chat
                                       Spaces; --drop-access goes ahead anyway.
  k2 app handover <app-id> --to WS [--brief FILE] [--dry-run]
                                       Hand the App to another agent: its agent
                                       offers, the other accepts, the owner
                                       approves one card (addresses, guests,
                                       secrets, database, keys move; undo 72 h).
  k2 app handover accept|decline|cancel|status|brief|undo|finish|rekey <app>
  k2 app park <app-id> [--reason T]    Owner/admin: stop it and hold it for the
                                       owner (no agent maintains it).
  k2 app unpark <app-id> --to WS       Owner: give a parked App to a workspace
                                       (the same handover card).
  k2 app release <workspace> --apps handover:<ws>|uninstall|park [--drop-access]
                                       One way out for every App a workspace
                                       maintains (before k2 workspace remove).
                                       park stops them and holds them for the
                                       owner (k2 app unpark gives one back).
                                       uninstall is all or nothing: if any App
                                       would lose people's logins, grants,
                                       shared secrets or chat Spaces, nothing
                                       is uninstalled and each such App is
                                       listed with its counts (exit 3);
                                       --drop-access goes ahead for all of
                                       them (with any other --apps: exit 2).
  k2 app config <app-id> [--memory 8G|512M|none] [--cpu-weight N] [--tasks N]
                 [--disk 2G] [--egress add|rm HOST:PORT] [--egress-mode allowlist|broad]
                 [--port add|rm 5000/udp] [--recycle-every 24h] [--recycle-at 85%]
                 [--isolation container|process] [--path guarded|open]
                 [--risk normal|high] [--db-private on|off] [--db-raw on|off]
                                       Narrowing applies now; a widening is held
                                       for the owner (exit 2, k2 app approvals).
                                       Memory: none (the default) lets the App
                                       use any memory outside K2's reserve.
  k2 app config <app-id> [--io-read 200M] [--io-write 40M]
                 [--io-read-iops N] [--io-write-iops N] [--io default|none]
                                       Disk ceilings (io.max) for walled Apps.
                                       Default: writes 40 MiB/s and 2000 IOPS,
                                       reads uncapped. Tightening applies now;
                                       raising one is held for the owner.

Database (a box Postgres database for an App, its own login, checked live):
  k2 app db connect <app-id> --new [--major N] [--cluster NAME] | --db <name>
                                       Makes or picks it, gives the App its own
                                       login and DATABASE_URL (never printed),
                                       re-hosts it and checks it from inside:
                                       PASS (exit 0) or FAIL (exit 1). An agent
                                       missing a switch gets one card (exit 2).
  k2 app db status|check|disconnect <app-id>
                                       k2 app db --help; k2 study app-databases.

Egress (guarded Apps reach only listed hosts, through their own proxy):
  k2 app egress <app-id>               Mode, hosts, pending requests, proxy.
  k2 app egress add <app-id> <host[:port]>... [--reason TEXT]
                                       Always held for the owner (exit 2).
  k2 app egress rm <app-id> <host>     At once; open tunnels are cut.
  k2 app egress log <app-id>           The last connections (host, port).
  k2 app approvals | approve <id> | deny <id>
                                       Held requests; approve is the owner's,
                                       in person. k2 app egress --help.

Box services (a walled App reaching a service on this box's loopback):
  k2 app box-service <app-id>          Approved / held, where each is in the wall.
  k2 app box-service request <app-id> <name> [<host-port>] [--app-port N]
                                       Always held for the owner (exit 2).
                                       sew-api / sew-signal need no port.
  k2 app box-service revoke <app-id> <name>
                                       At once. k2 app box-service --help.

Debug it (guarded Apps; agents need "Allow this agent to manage Apps"):
  k2 app exec <app-id> [--timeout 60] [--no-stdin] -- <command…>
                                       One command inside the App's container,
                                       as the App. Output is framed; exit = the
                                       command's (124 timeout, 3 refused).
                                       A pipe or file on stdin reaches the
                                       command (64 MiB at most); a terminal
                                       or --no-stdin sends none.
  k2 app shell <app-id> [-- <command…>] A terminal in the container (/bin/sh).
                                       Hung up after 15 idle minutes. Audited.

Leave the container (owner only, Linux):
  k2 app exception <app-id> on [--device dri-render128]... [--reason TEXT]
                                       Run it as a sandboxed system process for a
                                       device a container can't reach. Asks for
                                       an owner confirm. Label: Open (process).
  k2 app exception <app-id> off        Back to the guarded container.

The box (Linux):
  k2 app doctor [--json]               Is this box ready for Apps? Kernel, cgroup
                                       v2, Podman 5+, pasta, crun/youki, the k2app
                                       user and its ids, linger, delegation,
                                       io.cost, reserved ports, nft, helper.
  k2 app doctor <app-id> [--json]      Is everything approved in the running App?
                                       Box services and database ports approved
                                       but not yet applied; a container's
                                       database door (127.0.0.1:5432 in its
                                       wall), its open sessions and the
                                       databases it admits, or why it has
                                       none. Exit 1 when one is missing.
  k2 app doctor --store [<folder>] [--voice|--no-voice] [--json]
                                       Is this Tauri App's repo ready for App
                                       Store and Play review? Privacy manifest,
                                       usage strings, export answer, Tauri and
                                       wry versions (any OS; k2 app doctor
                                       --store --help).
  k2 app box prepare [--runtime crun|youki] [--install-podman|--yes]
                     [--dry-run] [--podman-from DIR] [--json]
                                       Owner: run the root helper's box prep
                                       (distro Podman + crun; youki is opt-in;
                                       AppArmor's parser where the kernel has
                                       AppArmor on, then the k2-app profile).
                                       Ubuntu 24.04 / Debian 12 ship Podman < 5:
                                       --install-podman (or a yes at the prompt)
                                       installs K2's pinned Podman 5 in
                                       /opt/k2/podman for the k2app user only;
                                       a distro Podman already there is left as
                                       it is. Never without asking. --dry-run
                                       prints every change, including the exact
                                       distro packages it would install, and
                                       changes nothing (agents may
                                       run it). --podman-from: air-gapped, a
                                       folder holding the pinned tarball.
  k2 app box podman uninstall [--json] Owner: remove K2's Podman (refused while
                                       Apps run).
  k2 app box unprep [--dry-run] [--yes] [--json]
                                       Owner: undo box prep (the nft table,
                                       AppArmor profile, limits, linger, the
                                       engine unit). Keeps the k2app user, its
                                       ids and data, the distro packages and
                                       K2's Podman. Refused while any App runs.
                                       Asks first (shows the plan); --yes for
                                       scripts. Automatic prep pauses until
                                       k2 app box auto-prepare retry.
  k2 app box auto-prepare retry [--json]
                                       Owner: run automatic box prep again now
                                       after it stopped for good (old kernel,
                                       cgroup v1, unsupported distro, overlapping
                                       sub-ids) or after an unprep.
  k2 app engine install [--from DIR] [--json]
                                       Owner: install/update k2-app-engine (the
                                       guarded engine, signed with the root-helper
                                       key) for this daemon's version; --from a
                                       folder holding the release asset + .sig.
  k2 app rehost status [<app>] [--json]
                                       The re-host wave after a K2 update: which
                                       Apps moved to the new runtime, which wait
                                       and why (read-only).
  k2 app rehost <app> [--yes] [--pause] [--json]
                                       Move an App to a fresh instance on this
                                       K2 now (zero downtime when it can run two
                                       copies). --yes: even if everyone must sign
                                       in again; --pause: a short pause when it
                                       can't run two copies.
  k2 app rehost policy <app> [--policy auto|handover|window|manual|off]
                       [--window HH:MM-HH:MM] [--when-no-overlap pause|defer]
                                       How K2 updates move this App. pause (the
                                       default): a short pause when it can't run
                                       two copies; defer: skip it, you update it.
                                       Re-hosts explained: k2 study app-runtime.
  k2 app engine status [--json]        The App engine: installed vs running vs
                                       this K2, linger, the boot heal, Apps
                                       waiting for the engine (read-only).

Exit codes: 0 live/ok, 1 a failed check or health gate, 2 usage or still
pending (loop on k2 app deployment), 3 infra or permission (JSON with a code).

Getting existing Apps ready (not behind the Apps switch): k2 app warnings,
clean-mode, key mint|revoke, keys, approvals, approve, deny, access
(k2 app grace for the details).

Phone notifications for a custom App's own phone app: k2 app push
(k2 app push --help, k2 study app-push).

Pairing a custom App's own phone app by QR from its web page: k2 app pairing
(k2 app pairing --help, k2 study app-pairing).
"Open in app" links (iOS and Android link files) for any App, with or without
pairing: k2 app links (k2 app links --help).
A starter for a Tauri, static or Node backend repo: k2 app init
(k2 app init --help, k2 study tauri).
Email from the App's own sender: k2 app email (k2 app email --help,
k2 study email-notifications).

Read `k2 study apps` first. Guests, roles and tokens: k2 app user|role|grant|token.
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


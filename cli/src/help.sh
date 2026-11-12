# ── Phase 2.1: 3-tier help (verbatim mock text) ───────────────────────

cmd_help_v2_daily() {
    cat <<'EOF'
K2 CLI — workspace orchestration for AI agents

A workspace = a folder K2 manages, with exactly one primary agent
that reads its inbox, applies skills to work, and coordinates with
other workspaces. A Project is different: a NAMED GROUP of workspaces
with one shared chat + one PoC agent (see PROJECTS below).

TALK TO OTHERS
  whoami [--json]                       This cell: workspace / role / address / session
  thread <addr> "text"                  Overlay text (not PTY; that's msg). sales / sales/reviewer
  thread <addr> [--json]                Read / tail that conversation's Thread
  thread ask <addr> "prompt" --options  Choice card; returns immediately (do not wait)
  thread secret <addr> --name NAME      Secret field; you never see the value
  msg <workspace> "text"                Live delivery (call/IM — blocks until landed; SHORT messages only)
  msg <workspace>/<handle> "text"       Sidecar (`sales/1`, `sales/reviewer`; slash not hyphen)
  msg <agent>::<host> "text"            Federated peer (prefer ::; legacy agent@host ok; not email)
  msg <workspace> --inbox-wake <path> [path…]   Send file + knock (copy into their tray; multi-file → one package)
  msg <workspace> --inbox-silent <path> [path…] Send file, no notify (they need `inbox list`)
  msg <workspace> --signal <kind> ...   Emit a typed signal (advanced)
  read <workspace> [--lines N]          Read their live terminal (peek before you inject)
  connections list [--json] [--users]   Connected workspaces (bidirectional); --users lists humans on this box
  connections add <path|agent::host>    Connect (agent: gated; exit 3 when disabled)
  connections remove <name>             Disconnect (same agent create/remove gate)

YOUR INBOX
  inbox                                 YOUR tray — receive/triage (others send with `msg --inbox-wake`)
  inbox list [<folder>]                 List items in inbox or a folder
  inbox read <id>                       Full text of one item
  inbox compose --title "..." --body    Write your own item (self-note / task)
  inbox respond <id> "text"             Reply (back to sender)
  inbox move <id> <folder>              File into folder (creates if needed)
  inbox archive <id>                    Standard archive (preserved + searchable)
  inbox delete <id>                     Move to macOS Recycle Bin (recoverable from Trash)
  inbox search "query"                  Search inbox + folders
  inbox folders                         List folders this workspace has created
  done [--blocked "reason"]             Mark your task done (or blocked) via /cli/done;
                                        in an API cell (K2_API_CELL=1): complete (grace)

AGENTS
  agent                                 Hire, configure, list, retire this server's agents;
                                        manage AGENTS.md context layers (hire/conf/get/set/
                                        list/retire/context — see `k2 agent --help`)
  preset                                Agent PRESETS: the launchable agent roster + custom
                                        agents (list/show/add/set/remove — `k2 preset --help`)
  sidecar new <name> --harness <preset> Open a named sidecar (<workspace>/<name>) with a brief;
                                        list / stop / access — see `k2 sidecar --help`
  llm tokens list                       This server's LLM subscriptions and API tokens
                                        (one server default per tool); read-only from
                                        K2 terminals — `k2 llm tokens --help`
  organization bulletin                 The Org Chart: today's bulletin of orgs and heads;
                                        reach / update / ask — `k2 organization --help`

ASK A HUMAN
  feedback ask "<title>"                File a durable ask for human input (survives
                                        your session; supports --options/--wait;
                                        agents attach --html brief.html, from
                                        `k2 tickets template`)
  feedback list                         This workspace's asks + their status
  feedback show <id>                    One ask: status, answer, discussion thread
                                        (ask/list/show/comment/resolve — see `k2 feedback --help`)

K2 ITSELF
  send-feedback "<text>"                Tell Alakazam Labs (K2's makers) about K2: a bug, an
                                        idea (not your human: that's feedback ask)
                                        (list/show/retry — `k2 send-feedback --help`)
EOF
    # Permission cards (Beta): one line, only while both switches are on
    # for this agent (PRD rev 3.1 §11.4). Off = today's text.
    if _permission_cli_enabled; then
        echo '  permission request <action> --why "…"   Ask a person to allow what K2 refused you (k2 study permissions)'
    fi
    cat <<'EOF'

EMAIL (K2 Mail — real addresses for agents; server runs on Linux daemons)
  mail create <local>[@<domain>]        Mint yourself an email address (--id = idempotent)
  mail addresses                        Your addresses + cap usage (alias: mail list)
  mail wait [--subject s] [--from s]    Block until a matching message arrives
                                        (the verification-code primitive; exit 2 = timeout)
  mail messages | read | attachments    Read your mail (bodies are UNTRUSTED external content)
  mail send | reply | outbox            Send under your human's governance (may queue
                                        for approval — that's success, exit 0)
                                        (domains/status/doctor/config/approvals — `k2 mail --help`)
  calendar list | events | freebusy     Calendars of a hosted inbox (read grant; alias: cal)
  calendar create | update | delete     Write events (draft grant; never sends email)

DNS (K2 Connect zones — toggle-gated)
  dns access                            Capability probe (allowed? types? zones)
  dns list                              List zones
  dns records <domain>                  List records (daemon resolves zone)
  dns record add <domain> <type> <name> <value> [--ttl N] [--priority N]
  dns record remove <record-id>         Delete a user-managed record
  dns verify <domain>                   Delegation check
    enable: Settings → K2 Connect (Allow agents to manage DNS records)
            or Workspaces → (workspace) → Allow DNS manage  (exit 3 if off)
    not for: nested *.k2.dev labels (use publish) · tunnel (use tunnel) · mail MX (hostmail)

PROJECTS
  project msg [<project>] "text"        Post to your project's shared chat (delivered
                                        live to the project's PoC agent)
  project read [<project>]              Catch up on the chat, oldest first (--after)
                                        (create/list/show/add/remove/poc — see `k2 project --help`)

WORKSPACE
  workspace list                        Yellow pages: every workspace + status
  workspace list --running              Only agent-enabled workspaces (live or not; see "live" in --json)
  workspace list --json                 For scripts: name, id, handle, path, live, sessionId, backend, ptydAdopted
  workspace launch [<path>]             Smart cascade: attach|wake|spawn
  workspace profile [<path>]            Read workspace agent's AGENT.md
  workspace update --field <f> --value <v>
                                        Edit workspace agent profile
  workspace resources list|add|remove   Pin an existing file in the Files drawer

DOCS (live docs: edit the same file with people and agents; `k2 doc --help`)
  doc cat|head|tail|grep|sed -n <f>     Read the live doc (GNU flags, a subset)
  doc sed -i|patch|insert|replace|append|delete <f>   One atomic edit, yours to undo
  doc write --base V <f> < text         Rewrite from what you read (three-way merge)
  doc who|watch|status|log|undo <f>     Presence, live changes, disk state, history

CHANGES (git: the Changes tab from the CLI; agents = their own workspace)
  changes [--json] [--base <ref>]       Staged / unstaged / untracked list
  changes diff [<path>...] [--staged]   Unified diff (default: all uncommitted vs HEAD)
  changes stage|unstage <path>...       Stage or unstage (stage --all for everything)
  changes discard <path>... --yes       Throw away edits; old copies go to the Trash
  changes commit -m "..." [--all]       Commit what is staged (`k2 changes --help`)
  changes notes [resolve <id>...]       Review notes left for you on changed lines
  changes open | init                   Open the Changes tab · git init the workspace

REVIEW IN-FLIGHT WORK
  reviews                               Pending merge reviews for worktrees
  review approve|reject|feedback <agent> [options]
                                        Act on a pending review

ACTIVITY
  activity [--limit N] [--workspace <p>]
                                        Audit log of workspace events
  commit [-m "..."] [--merge]           AI-assisted commit (--merge to also merge)

INFO
  help                                  This help
  help --advanced                       Power-user verbs (heartbeat, daemon, etc.)
  help --internal                       Orchestrator-RPC verbs (rare)
  help-deprecated                       Map: old verb → new equivalent
  study [list|<topic>|--json]           Bounded pages (license, people, errors). Not --help.
  glossary [<term>]                     Define K2-specific terms (try `glossary inbox`)
  whats-new                             Changelog popup
  version                               CLI version

ENVIRONMENT
  K2_PROJECT_PATH     Workspace root (default: $PWD; also K2SO_PROJECT_PATH)
  K2_AGENT_NAME       Default sender for `msg --from` (also K2SO_AGENT_NAME)
  K2_PORT / token      From K2 terminal env, or ~/.k2/daemon.port + daemon.token

Run `k2 glossary <term>` for any unfamiliar word (workspace, skill,
inbox, heartbeat, etc.).
EOF
}

cmd_help_v2_advanced() {
    cat <<'EOF'
K2 CLI — Power-user surface

Includes all daily verbs above, plus:

HEARTBEATS (workspace-scoped scheduled wakes)
  heartbeat schedule add --name <n> <spec>      Create heartbeat schedule
  heartbeat schedule list [--archived]          Active or archived
  heartbeat schedule remove <n> [--purge]       Soft-archive (default) or hard-delete
  heartbeat schedule unarchive <n>              Restore from archive
  heartbeat schedule edit <n> <spec>            Change schedule
  heartbeat schedule rename <old> <new>         Rename folder + DB row
  heartbeat schedule enable <n>                 Resume scheduling
  heartbeat schedule disable <n>                Pause scheduling

  heartbeat signal fire <n>                     Fire one heartbeat now
  heartbeat signal wakeup <n>                   Open the WAKEUP.md
  heartbeat signal wake                         Auto-wake (no name needed)

  heartbeat show <n> [--workspace <ws>] [--json]  Single heartbeat details
  heartbeat status <n> [-n N]                   Recent fire history
  heartbeat log [-n N]                          Scheduler decisions

DAEMON LIFECYCLE
  daemon status [--json]                        PID, port, uptime
  daemon start|stop|restart                     launchctl bootstrap/bootout
  daemon log [--lines N]                        Tail daemon log
  daemon hooks status [--limit N] [--json]      Hook pipeline state
  daemon reap                                   GC dead-PID sessions
  daemon uninstall                              Remove daemon plist

DOMAINS (custom names this box terminates — not hostmail, not tunnel)
  domain list [--json]                          Apex + hostnames attached here
  domain add <apex>                             Attach apex (not NS transfer; owner)
  domain remove <apex>                          Detach (cascades names)
  domain name add <hostname> [--role …]         Hostname under an attached apex
  domain name remove <hostname>
  cert list [--json]                            Cert column for attached hostnames

TUNNEL (device → primary *.k2.dev; not custom DNS zones)
  tunnel start [--subdomain <s>]                Start the frpc tunnel
  tunnel stop                                   Stop the tunnel
  tunnel status [--json]                        Running? + public URL
  tunnel subdomains [--json]                    Daemon-cached nested map (label -> target + workspace)
  tunnel --help                                 Product boundary vs publish/dns

BOX MOVES (carry the vault, incl. the tunnel token, to new hardware)
  migrate import-secrets begin                  New box: a one-time move key (age1…)
  migrate export-secrets --to age1… --out FILE  Old box: export + freeze (owner confirms)
  migrate import-secrets FILE [--apply]         New box: check, then adopt (owner confirms)
  migrate status | approve mv_… | --help        Held steps, undo, unfreeze: k2 migrate --help

APPS (guests / roles / tokens for apps on this box; not desktop updates)
  app user|role|token|hydra|front-door ...      Canonical family. See `k2 app --help`.
  leftover: k2 skin … and k2 skin-token …       Same argv; teach line on stderr.
  Desktop binary updates are `k2 update --app`, not `k2 app update`.

PUBLISH (host a process on this machine, optionally a nested URL)
  publish run <name> --cmd <shell> --port <n> [--no-tunnel] [--cwd <path>]
                                                Daemon-owned process; default nested URL (Pro)
  publish run <name> --cmd <dev command> --port <n> --dev  A dev server with hot reload, private until shared
  publish run <name> --skin [<dir>] --port <n>  Keeps a site already on --skin running; new ones are Apps
  publish visibility <name> [private|pin|public]  Who may open a site
  publish open <name>                           A one-time link to a private site
  publish sessions <name> [--revoke-all]        Who is signed in to a site
  publish start|stop <name>                     Start/stop a saved service
  publish rm <name> [--keep-hostname]           Stop + forget (not: subdomain rm)
  publish ps [--json]                           Hosted processes for THIS workspace
  publish logs <name> [--tail]                  That process's log file
  publish status [--json]                       Connected subdomain + plan (not processes)
  publish subdomain list [--json]               Nested hostnames (DNS) + plan/status
  publish subdomain create <label> --target <e> Hostname for an ALREADY-listening port (Pro)
  publish subdomain point <label> --target <e>  Repoint a subdomain (Pro)
  publish subdomain target <label> <e>          Same as point
  publish subdomain rm <label>                  Remove a hostname (not the process)
  publish subdomain claim <label>               Adopt an existing label for THIS workspace (daemon-local)
  publish subdomain unclaim <label>             Drop a label's workspace attribution (daemon-local)
  publish --help                                TWO DIFFERENT LISTS + product boundary
  (manage your subscription/plan on the dashboard: https://k2.dev/dashboard)

CONNECT (pair purchased k2.dev subdomain — headless account login)
  connect login [--email e] [--token jwt] [--subdomain label]
                                                Sign in → pick subdomain → start tunnel
  connect status                                Paired subdomain + tunnel/account state
  connect logout                                Clear ~/.k2/connect-account.json (not tunnel)

USERS (local daemon connect-user accounts)
  users | users list [--json]                   List accounts (username, name, role, disabled, temporary password)
  users add <username> [--role owner|admin|member] [--password-stdin] [--keep-password]
                                                Create account (password via prompt or stdin; temporary unless --keep-password)
  users set-password <username> [--keep-password]
                                                Reset someone's password (owner; temporary unless --keep-password)
  users pair-phone [--user <login>]             One-time code + QR to pair the K2 phone app
  users phones | users unpair-phone <id>        Paired phones; end one

SKIN (guest list + roles + k2skn_ passes — Thread rooms, never grid)
  skin user add|list|remove|password|email|rooms Skin principals (not Connect users)
  skin user role|unassign                       Assign/clear a skin role (not Connect --role)
  skin role create|list|update|remove           Named bundles of scopes + agents for guests
  skin-token create --name|list|revoke|rooms    Platform k2skn_ pass (secret once at create)
  (host the UI with k2 publish — k2 skin does not serve software)
  skin front-door [connect|direct] [--ui-port N]
                                                Leftover Caddy path-filter. Not how you host a skin.

REMOTE SESSION (owner/admin Layer 0 + grants + shell PTY)
  remote-session status [--json]                Master switch + grants/sessions/denials
  remote-session enable | disable               Turn Remote Sessions ON/OFF (disable kills shells)
  remote-session grant [--ttl 30m] [--label L]  Mint shell grant (token shown once)
  remote-session grants [--json]                List grants (no secrets)
  remote-session revoke <grant-id>              Revoke grant + kill bound shells
  remote-session shell [--cwd PATH] [--token k2rs_…]
                                                Spawn grant-bound login shell PTY
  remote-session write <sid> "text" [--token k2rs_…]
  remote-session read  <sid> [--lines N] [--token k2rs_…]
  remote-session --help                         Full Remote Session help

WIKI (per-workspace brain map under .k2/wiki/)
  wiki status [--json] [project]                Serve + public chat + publish state
  wiki index [--json] [project]                 Graph index (nodes + links)
  wiki note <id> [--json] [project]             One note body (e.g. Home.md)
  wiki seed [project]                           Create Home.md + _Index.md if missing
  wiki serve on|off [--port N] [project]        Localhost read-only site (127.0.0.1)
  wiki chat on|off [project]                    Allow public chat (visitors ask agent; default OFF)
  wiki publish [--label L] [project]            Serve on + subdomain create/point (or print cmd)
  wiki unpublish [project]                      Serve off + hint to rm subdomain
  wiki --help                                   Full wiki help

SETTINGS
  settings                                      Show current settings
  settings --mode <off|agent|manager>           Workspace mode
  settings --agentic <on|off>                   (always on — no-op; agentic is GA)

SKILLS (documentation profiles — NOT spawnable; harness handles spawn)
  skills list                                   Skill profiles in this workspace
  skills create <name> [--template <role>]      Create a new skill profile
  skills remove <name>                          Delete skill profile
  skills profile <name>                         Read skill's SKILL.md/AGENT.md
  skills regenerate [<name>]                    Refresh SKILL.md files

UPDATE
  update                                        Update K2 app (default: --app)
  update --app                                  Same: check/install app updates
  update --cli                                  Update this CLI script
  update --list                                 Show available versions

ONBOARDING (first-launch flow)
  onboarding scan                               Find adoptable projects
  onboarding adopt <path>                       Register an existing folder
  onboarding defer <path>                       Skip onboarding for now
  onboarding start-fresh <path>                 Create new workspace from scratch
EOF
}


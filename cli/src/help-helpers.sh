# ── Phase 2.1: help text helpers ──────────────────────────────────────

cmd_help_inbox() {
    cat <<'EOF'
k2 inbox [<subcommand>] [options]

YOUR tray. Not email. Not live chat.
Others SEND a file with:
  k2 msg <your-handle> --inbox-wake ./file.md
You cannot send a file with `k2 inbox`. Use `k2 msg <handle> --inbox-wake`.

Items arrive from other workspaces (via `k2 msg --inbox-wake|--inbox-silent`)
or are composed as self-notes (`k2 inbox compose`). Read, triage, file
into folders.

SUBCOMMANDS
  (none)                    Show inbox items (= `inbox list`)
  list [<folder>]           List items in inbox (or a specific folder)
  read <id>                 Full text of one item
  compose --title "..." --body "..."
                            Write your own inbox item (self-note / task)
  respond <id> "text"       Reply (back to sender)
  move <id> <folder>        File into a folder (creates folder if needed)
  archive <id>              Standard archive (preserved + searchable; goes to inbox/done/)
  delete <id>               Move to macOS Recycle Bin (recoverable from Trash)
                            Use `inbox archive` for "done but keep around";
                            use `inbox delete` only for "actually remove this."
  search "query"            Search inbox + all folders
  folders                   List folders this workspace has created

ORGANIZING YOUR INBOX
  K2 doesn't impose a folder taxonomy. Organize your tray the way
  you'd file notes — create folders that fit your workflow:
    inbox move <id> projects/    # active work items
    inbox move <id> reference/   # things to remember
    inbox move <id> issues/      # problems to address
    inbox move <id> done/        # completed work
  Folders are auto-created on first `inbox move` into them.

EXAMPLES
  k2 inbox                                  # show top-level inbox
  k2 inbox list projects                    # what's in projects/
  k2 inbox read 42                          # full text of item 42
  k2 inbox move 42 projects                 # file 42 into projects/
  k2 inbox compose --title "audit auth" --body "..."   # self-note
  k2 inbox respond 42 "thanks, looking into this"
  k2 inbox archive 42                       # done with this item
  k2 inbox search "oauth"                   # find all oauth-related

RECEIVING FROM OTHER WORKSPACES
  Other workspaces send to your inbox with:
    k2 msg <your-workspace> --inbox-wake ./brief.md
    k2 msg <your-workspace> --inbox-silent ./data.pdf
  Their items appear in your inbox alongside your own composes.

LEGACY ITEMS
  Pre-Phase-2.1 work items at .k2so/work/{inbox,active,done}/ have
  been migrated to .k2/inbox/{,active,done}/. If migration didn't
  run yet, trigger it manually: `k2 inbox` will reveal nothing
  until the daemon has converted the layout.
EOF
}

cmd_help_workspace() {
    cat <<'EOF'
k2 workspace <subcommand> [options]

Manage workspaces (project folders K2 knows about).

DAILY SUBCOMMANDS
  list                              Yellow pages: every workspace + status
  list --running                    Filter to live agents
  launch [<path>]                   Smart cascade: attach|wake|spawn
  profile [<path>]                  Read workspace agent's AGENT.md
  update --field <f> --value <v>    Edit workspace agent profile

LIFECYCLE (less common)
  create <path> [--no-wiki] [--no-agents-md] [--fanout]
                                    Create folder + register (wiki + cwd AGENTS.md
                                    seeded unless opted out; leftover fan-out off)
  open <path> [--no-wiki] [--no-agents-md] [--fanout]
                                    Register existing folder (same defaults)
  remove <path>                     Deregister (files stay)
  triage                            This workspace's inbox list (same as k2 inbox list)

FOLDER TRUST (who may start AI tools in a folder; k2 workspace trust --help)
  trust status [<folder>] [--json]  Clean / trusted / pending / denied + findings
  trust request <folder> --why "…"  Agents: ask the owner (a Thread card)
  trust <folder> --yes|--always|--no
                                    Owner: Trust it / Always / Don't trust
  trust reset <folder>              Owner: back to undecided

WORKSPACE RESOURCES
  resources list [--json]           Files pinned in the Files drawer for this workspace
  resources add <file> [--json]     Pin an existing file (absolute path from this cwd)
  resources remove <file> [--json]  Unpin that file
  --workspace / --project <path>    Override the workspace (default: this one)

API / HOST-SESSIONS
  api-guest-policy get <ws>         Show owner guest policy (default if unset)
  api-guest-policy set <ws> <text>  Set policy re-injected every /v1 turn
  api-guest-policy set <ws> --file <path>
  api-guest-policy set <ws> --default   Clear → platform default
  api-skip-permissions get <ws>     Show if /v1 host spawns keep auto-approve (default on)
  api-skip-permissions set <ws> on|off   Opt in/out of auto-approve on API host spawns
  host-session-cell-cap get <ws>    Show concurrent host-session cap (15 default)
  host-session-cell-cap set <ws> <n|default>  Set/clear per-ws live cell cap (max 512)
  default-model get <ws>            Show workspace default model (unset if empty)
  default-model set <ws> <model|clear>   Set or clear workspace default model
  force-model-on-resume get <ws>    Show if resume also splices the default model
  force-model-on-resume set <ws> on|off  Pass default model on dead resume (new PTY)

TABS (Workspace settings → Tabs; every window and person on this server)
  design-tab get <ws>               Show if the Design tab is pinned (default on)
  design-tab set <ws> on|off        Pin or remove the Design tab (old name: design-mode)
  mobile-tab get <ws>               Show the Mobile tab setting and if it shows here
  mobile-tab set <ws> on|off|auto   Pin, remove, or auto (pinned where a phone can run)

CANONICAL CHAT
  swap-canonical <ws> --agent <harness>
                                    Replace the pinned chat with a new session
                                    of that harness and send it a transcript seed.
                                    Does not resume the old chat. Does not move
                                    heartbeats.
      --recent                      Last turn only (default)
      --full                        Longer transcript tail
      --notes-file <path>           Extra notes appended to the seed
      --model <id>                  Start the new chat on this model, even if
                                    the preset names another (this start only)
      harness: claude, grok, cursor, gemini, pi, codex, hermes
                                    The same harness is a refresh (k2 agent
                                    refresh): the Thread comes along.

`<path>` defaults to K2_PROJECT_PATH or PWD.

EXAMPLES
  k2 workspace list
  k2 workspace launch
  k2 workspace launch /Users/me/Projects/k2so
  k2 workspace profile
  k2 workspace update --field role --value "frontend lead"
  k2 workspace api-guest-policy get my-wiki
  k2 workspace api-guest-policy set my-wiki "Read-only; use k2 respond --final"
  k2 workspace api-skip-permissions get my-api-ws
  k2 workspace api-skip-permissions set my-api-ws off
  k2 workspace host-session-cell-cap get sales-interview
  k2 workspace host-session-cell-cap set sales-interview 64
  k2 workspace host-session-cell-cap set sales-interview default
  k2 workspace resources add docs/brief.md
  k2 workspace resources list
  k2 workspace resources remove docs/brief.md
  k2 workspace swap-canonical my-agent --agent claude --notes-file handoff.md
EOF
}

cmd_help_connections() {
    cat <<'EOF'
k2 connections [list|add|remove] [workspace-name] [--json] [--users]

Manage this workspace's connections to other workspaces (agents).

  list [--json] [--users]  List connections for this workspace (exit 0).
                    --users also prints humans on this host (do not k2 msg those names).
  add <name>        Connect to another workspace (name, path, or <agent>::<host>)
                    Legacy <agent>@<host> still accepted.
  remove <name>     Disconnect from a workspace

AGENT GATE (create / remove)
  Adding and removing connections is OFF by default for agents.
  When gated, both `add` and `remove` exit 3 with:
    {"error":{"code":"agents_create_connections_disabled","hint":"…"}}
  Owner can enable: Settings → K2 Connect → Allow agents to create
  connections (or Workspaces → (workspace) → Allow agents to create
  connections). `list` is always allowed.

MSG / READ PEER GATE
  Live `k2 msg`, tray `k2 msg --inbox-wake|--inbox-silent`, and `k2 read` require a local
  connection to the target (or same-workspace). Unconnected peers
  return exit 3 with code `not_connected` — add them here first.

HANDLES ARE NEVER REUSED
  A connection to <agent>::<host> is to that WORKSPACE, not to the name.
  Handles are never given to another workspace (a removed workspace's
  handles are retired for good). If the workspace leaves the other host,
  `list` marks the connection GONE and sends fail with
  `remote_workspace_gone`; it never moves to a new workspace that has
  the name. Only your human (owner or admin) can `add` it again to
  connect to whichever workspace has that name now.

EXAMPLES
  k2 connections list
  k2 connections list --json
  k2 connections list --users
  k2 connections add "QA Bot"
  k2 connections add peer::other.k2.dev
  k2 connections remove scout
EOF
}


# Agent Orchestration System

> **Note (0.39.0):** As of 0.39.0, K2SO no longer manages worktree creation
> or sub-agent spawn directly. Modern harnesses (Claude Code's sub-agent
> feature, Cursor's worktree management) own that lifecycle now. K2SO is
> the workspace-orchestration layer: inbox, heartbeats, skills, reviews,
> cross-workspace messaging. The old top-level spawn verb has been retired —
> see `k2so help-deprecated` for the full retired-verb map.

## Overview

K2SO's agent orchestration enables autonomous AI agents to triage inbox work, coordinate across workspaces, communicate over terminal I/O, and complete tasks with minimal human intervention. The system is built on the `k2so` CLI tool, the per-workspace `.k2so/inbox/` queue, skill profiles in `.k2so/skills/`, and the K2SO daemon's HTTP surface.

## Architecture

```
User / Heartbeat Scheduler
        │
        ▼
  k2so heartbeat wake
        │
        ├─ Checks workspace inbox (.k2so/inbox/)
        ├─ Finds or launches coordinator terminal
        └─ Sends triage message
                │
                ▼
        Coordinator (persistent session)
                │
                ├─ k2so inbox                  → sees inbox work
                ├─ k2so workspace list         → discovers peer workspaces
                ├─ k2so skills profile <name>  → loads a role profile for the task
                │
                │   Spawn is now the harness's job:
                │   - Claude Code sub-agent (`Task` tool / worktree feature)
                │   - Cursor worktree management
                │   - Manual: open a Cmd+T tab and load the SKILL.md
                │
                └─ Monitors for completed work (review queue)
                        │
                        ├─ k2so review approve → merge + cleanup
                        └─ k2so review reject → discard + feedback
```

## Chat Tab Terminal Lifecycle

The Chat tab for both Coordinators and Worktrees follows this flow:

1. **Attach to live terminal** — if a terminal with the deterministic ID already exists, connect to it with real-time grid updates
2. **Resume previous session** — if no terminal exists, check Claude's `history.jsonl` for the most recent session in that directory and launch with `--resume`
3. **Start fresh** — if no terminal and no previous session, launch a new Claude session

### Terminal ID Conventions

| Context | Terminal ID | CWD |
|---------|-------------|-----|
| Coordinator | `agent-chat-coordinator` | workspace root |
| Worktree | `agent-chat-wt-{workspaceId}` | `.worktrees/{branch}/` |

When a harness spawns a sub-agent into a worktree, the Chat tab in K2SO uses the same terminal ID so it can attach to that live session.

## Completion Protocol

When a sub-agent finishes its task in a worktree, it follows the workspace's state-driven completion path. The pattern below is what K2SO recommends harnesses embed in the CLAUDE.md / SKILL.md they generate for the worktree:

### Auto Mode (Build state)
```
1. Commit changes
2. Run: k2so agent complete --agent <name> --file <filename>
   → Merges branch into main, cleans up worktree
3. Notify coordinator: k2so terminal write <coord-id> "Completed: <task>. Branch merged."
```

### Gated Mode (Managed Service state)
```
1. Commit changes
2. Run: k2so agent complete --agent <name> --file <filename>
   → Moves work to done, flags for human review
3. Notify coordinator: k2so terminal write <coord-id> "Ready for review: <task>."
```

### Off Mode
Work items with "off" capability sources are excluded from triage entirely.

## Heartbeat Wake Flow

`k2so heartbeat wake` automates the full coordinator wake cycle:

1. Check the workspace inbox (`.k2so/inbox/`) for new arrivals
2. If coordinator terminal is running → send triage message directly
3. If coordinator is asleep → launch with `--resume` (resumes previous session) + `--dangerously-skip-permissions`
4. Wait for terminal to be ready (polls every 5s, up to 60s)
5. Send triage message: "New work detected. Run `k2so inbox` and triage any pending items."

## Virtual Terminal I/O

Agents can communicate across workspaces. The two daily-tier verbs for live cross-workspace messaging:

```bash
k2so workspace list --running              # All workspaces with live agents right now
k2so msg <workspace> "text"                # Live delivery (call/IM — blocks until landed)
k2so msg <workspace> "text" --inbox        # Email-style drop into the recipient's inbox
```

For low-level PTY I/O (internal tier; used by orchestrators that integrate with K2SO):

```bash
k2so terminal read <id> --lines 50         # Read last N lines from a terminal buffer
k2so terminal write <id> "message"         # Send text to a PTY by id (raw keystrokes + Enter)
```

This enables:
- Coordinators messaging peers to merge or request changes
- Sub-agents notifying the coordinator of completion
- External scripts monitoring agent progress
- Future: remote access via the K2SO companion tunnel

## Session Detection

Claude Code stores sessions in `~/.claude/projects/{hash}/`. The hash converts paths by replacing `/` with `-` and stripping dots from hidden directories (`.k2so` → `--k2so`, `.worktrees` → `--worktrees`).

For subpaths (worktrees, agent dirs), session detection uses **exact path matching** to avoid resuming sessions from the wrong worktree.

## CLI Commands Reference

The 0.39.0 CLI exposes 24 verbs across three tiers. The summary below covers the orchestration-relevant subset. Run `k2so help`, `k2so help --advanced`, and `k2so help --internal` for the full surface, and `k2so help-deprecated` for the retired-verb map.

### Workspace Management (daily tier)
```
k2so workspace list                  Yellow pages: every workspace + status
k2so workspace list --running        Filter to workspaces with live agents
k2so workspace launch [<path>]       Smart cascade: attach|wake|spawn
k2so workspace profile [<path>]      Read workspace agent's AGENT.md
k2so workspace update --field <f> --value <v>
                                     Edit workspace agent profile
k2so workspace create <path>         Create folder + register
k2so workspace open <path>           Register existing folder
k2so workspace remove <path>         Deregister (keeps files)
k2so workspace triage                Plain-text summary of pending work
```

### Skills (documentation profiles — daily tier)
```
k2so skills list                     Skill profiles in this workspace
k2so skills create <name> [--template <existing-skill>]
                                     Create a new skill profile
k2so skills remove <name>            Delete a skill profile (sent to Trash)
k2so skills profile <name>           Read the skill's SKILL.md
k2so skills regenerate [<name>]      Refresh every skill's SKILL.md
```

### Inbox (per-workspace work queue)
```
k2so inbox                           Show inbox items (default action)
k2so inbox compose --title "..." --body "..."
                                     Write your own item (self-note / task)
k2so inbox move <id> <folder>        File into a folder (creates if needed)
k2so inbox archive <id>              Standard archive (preserved + searchable)
k2so inbox search "query"            Search inbox + all folders
```

### Cross-Workspace Messaging
```
k2so msg <workspace> "text"          Live delivery (blocks until landed)
k2so msg <workspace> "text" --inbox  Drop in their inbox (async, email-style)
k2so who                             Workspaces with live agents right now
k2so connections list|add|remove     Cross-workspace links
```

### Heartbeat (advanced tier)
```
k2so heartbeat wake                  Auto-wake coordinator
k2so heartbeat schedule add --name <n> <spec>
                                     Create a scheduled wake (daily/weekly/hourly/...)
k2so heartbeat schedule list         Active heartbeat schedules
k2so heartbeat signal fire <n>       Fire one heartbeat now
```

### Reviews
```
k2so reviews                         List pending reviews
k2so review approve <agent> <branch> Merge + cleanup
k2so review reject <agent>           Discard + feedback
k2so review feedback <agent> -m ".." Request changes
```

### Terminal I/O (internal tier — for orchestrators)
```
k2so terminal write <id> "message"   Send to a PTY by id
k2so terminal read <id> --lines N    Read terminal buffer
k2so terminal spawn --command "..."  Spawn a sub-terminal
```

### Settings
```
k2so settings                        Show current settings
k2so settings --mode <off|agent|manager>
                                     Workspace mode
k2so settings --state <build|managed|maintenance|locked>
                                     Workspace capability tier
k2so settings --agentic <on|off>     Global agentic systems toggle
```

## Review Cleanup

When `review approve` or `review reject` runs:
1. Git worktree is removed (recycled to Trash)
2. Git branch is deleted (merged or discarded)
3. **Workspace DB record is deleted** — worktree disappears from UI
4. Done items are archived (approve) or moved back to inbox (reject)

# Agent Orchestration System

> **Note (0.39.0):** As of 0.39.0, K2 no longer manages worktree creation
> or sub-agent spawn directly. Modern harnesses (Claude Code's sub-agent
> feature, Cursor's worktree management) own that lifecycle now. K2 is
> the workspace-orchestration layer: inbox, heartbeats, skills, reviews,
> cross-workspace messaging. The old top-level spawn verb has been retired —
> see `k2 help-deprecated` for the full retired-verb map.

## Overview

K2's agent orchestration enables autonomous AI agents to triage inbox work, coordinate across workspaces, communicate over terminal I/O, and complete tasks with minimal human intervention. The system is built on the `k2` CLI tool, the per-workspace `.k2/inbox/` queue, skill profiles in `.k2/skills/`, and the K2 daemon's HTTP surface.

## Architecture

```
User / Heartbeat Scheduler
        │
        ▼
  k2 heartbeat wake
        │
        ├─ Checks workspace inbox (.k2/inbox/)
        ├─ Finds or launches coordinator terminal
        └─ Sends triage message
                │
                ▼
        Coordinator (persistent session)
                │
                ├─ k2 inbox                  → sees inbox work
                ├─ k2 workspace list         → discovers peer workspaces
                ├─ k2 skills profile <name>  → loads a role profile for the task
                │
                │   Spawn is now the harness's job:
                │   - Claude Code sub-agent (`Task` tool / worktree feature)
                │   - Cursor worktree management
                │   - Manual: open a Cmd+T tab and load the SKILL.md
                │
                └─ Monitors for completed work (review queue)
                        │
                        ├─ k2 review approve → merge + cleanup
                        └─ k2 review reject → discard + feedback
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

When a harness spawns a sub-agent into a worktree, the Chat tab in K2 uses the same terminal ID so it can attach to that live session.

## Completion Protocol

When a sub-agent finishes its task in a worktree, it follows the workspace's state-driven completion path. The pattern below is what K2 recommends harnesses embed in the CLAUDE.md / SKILL.md they generate for the worktree:

### Auto Mode (Build state)
```
1. Commit changes
2. Run: k2 agent complete --agent <name> --file <filename>
   → Merges branch into main, cleans up worktree
3. Notify coordinator: k2 terminal write <coord-id> "Completed: <task>. Branch merged."
```

### Gated Mode (Managed Service state)
```
1. Commit changes
2. Run: k2 agent complete --agent <name> --file <filename>
   → Moves work to done, flags for human review
3. Notify coordinator: k2 terminal write <coord-id> "Ready for review: <task>."
```

### Off Mode
Work items with "off" capability sources are excluded from triage entirely.

## Heartbeat Wake Flow

`k2 heartbeat wake` automates the full coordinator wake cycle:

1. Check the workspace inbox (`.k2/inbox/`) for new arrivals
2. If coordinator terminal is running → send triage message directly
3. If coordinator is asleep → launch with `--resume` (resumes previous session) + `--dangerously-skip-permissions`
4. Wait for terminal to be ready (polls every 5s, up to 60s)
5. Send triage message: "New work detected. Run `k2 inbox` and triage any pending items."

## Virtual Terminal I/O

Agents can communicate across workspaces. The two daily-tier verbs for live cross-workspace messaging:

```bash
k2 workspace list --running                # All workspaces with live agents right now
k2 msg <workspace> "text"                  # Live delivery (call/IM — blocks until landed)
k2 msg <workspace> --inbox-silent --title "..." --body "text"     # Email-style drop into the recipient's inbox (--inbox-wake also wakes them)
```

For low-level PTY I/O (internal tier; used by orchestrators that integrate with K2):

```bash
k2 terminal read <id> --lines 50           # Read last N lines from a terminal buffer
k2 terminal write <id> "message"           # Send text to a PTY by id (raw keystrokes + Enter)
```

This enables:
- Coordinators messaging peers to merge or request changes
- Sub-agents notifying the coordinator of completion
- External scripts monitoring agent progress
- Future: remote access via the K2 companion tunnel

## Session Detection

Claude Code stores sessions in `~/.claude/projects/{hash}/`. The hash converts paths by replacing `/` with `-` and stripping dots from hidden directories (`.k2so` → `--k2so`, `.worktrees` → `--worktrees`).

For subpaths (worktrees, agent dirs), session detection uses **exact path matching** to avoid resuming sessions from the wrong worktree.

## CLI Commands Reference

The 0.39.0 CLI exposes 24 verbs across three tiers. The summary below covers the orchestration-relevant subset. Run `k2 help`, `k2 help --advanced`, and `k2 help --internal` for the full surface, and `k2 help-deprecated` for the retired-verb map.

### Workspace Management (daily tier)
```
k2 workspace list                    Yellow pages: every workspace + status
k2 workspace list --running          Filter to workspaces with live agents
k2 workspace launch [<path>]         Smart cascade: attach|wake|spawn
k2 workspace profile [<path>]        Read workspace agent's AGENT.md
k2 workspace update --field <f> --value <v>
                                     Edit workspace agent profile
k2 workspace create <path>           Create folder + register
k2 workspace open <path>             Register existing folder
k2 workspace remove <path>           Deregister (keeps files)
k2 workspace triage                  Plain-text summary of pending work
```

### Skills (documentation profiles — daily tier)
```
k2 skills list                       Skill profiles in this workspace
k2 skills create <name> [--template <existing-skill>]
                                     Create a new skill profile
k2 skills remove <name>              Delete a skill profile (sent to Trash)
k2 skills profile <name>             Read the skill's SKILL.md
k2 skills regenerate [<name>]        Refresh every skill's SKILL.md
```

### Inbox (per-workspace work queue)
```
k2 inbox                             Show inbox items (default action)
k2 inbox compose --title "..." --body "..."
                                     Write your own item (self-note / task)
k2 inbox move <id> <folder>          File into a folder (creates if needed)
k2 inbox archive <id>                Standard archive (preserved + searchable)
k2 inbox search "query"              Search inbox + all folders
```

### Cross-Workspace Messaging
```
k2 msg <workspace> "text"            Live delivery (blocks until landed)
k2 msg <workspace> --inbox-silent --title "..." --body "text"
                                     Drop in their inbox (async, email-style;
                                     --inbox-wake also wakes them)
k2 connections list|add|remove       Cross-workspace links
```

### Heartbeat (advanced tier)
```
k2 heartbeat wake                    Auto-wake coordinator
k2 heartbeat schedule add --name <n> <spec>
                                     Create a scheduled wake (daily/weekly/hourly/...)
k2 heartbeat schedule list           Active heartbeat schedules
k2 heartbeat signal fire <n>         Fire one heartbeat now
```

### Reviews
```
k2 reviews                           List pending reviews
k2 review approve <agent> <branch> Merge + cleanup
k2 review reject <agent>             Discard + feedback
k2 review feedback <agent> -m ".." Request changes
```

### Terminal I/O (internal tier — for orchestrators)
```
k2 terminal write <id> "message"     Send to a PTY by id
k2 terminal read <id> --lines N      Read terminal buffer
k2 terminal spawn --command "..."    Spawn a sub-terminal
```

### Settings
```
k2 settings                          Show current settings
k2 settings --mode <off|agent|manager>
                                     Workspace mode
k2 settings --state <build|managed|maintenance|locked>
                                     Workspace capability tier
k2 settings --agentic <on|off>       Global agentic systems toggle
```

## Review Cleanup

When `review approve` or `review reject` runs:
1. Git worktree is removed (recycled to Trash)
2. Git branch is deleted (merged or discarded)
3. **Workspace DB record is deleted** — worktree disappears from UI
4. Done items are archived (approve) or moved back to inbox (reject)

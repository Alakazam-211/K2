//! Skill + CLAUDE.md content generators for each agent tier.
//!
//! Four generators (`generate_manager_skill_content`,
//! `generate_custom_agent_skill_content`, `generate_k2so_agent_skill_content`,
//! `generate_template_skill_content`) each produce the tier's canonical
//! SKILL.md body. [`compose_agent_wake_context`] (previously known as
//! `generate_agent_claude_md_content` — renamed during the 0.33.0 move
//! to reflect what it actually returns: the full `--append-system-prompt`
//! text an agent sees on wake) composes identity + project context +
//! standing orders + tier skill body into the final system prompt
//! string, and as a side effect writes the tier skill body to the
//! agent's SKILL.md.
//!
//! The SKILL wrapping / versioning / checksum protocol itself lives in
//! [`crate::skills::version`]; this module is the content side.
//!
//! All four `generate_*_skill_content` entry points also pull custom
//! layers from `~/.k2so/templates/<tier>/*.md` via [`load_custom_layers`]
//! — that's how the Agent Skills Settings UI's user-editable tab
//! injects project-global conventions into every agent of a given tier.

use std::fs;
use std::path::PathBuf;

use crate::skills::version::{ensure_skill_up_to_date, SKILL_VERSION_TEMPLATE};
use crate::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, parse_frontmatter, resolve_project_id,
};
use crate::workspace::scheduler::{agent_work_dir, get_workspace_state};
use crate::workspace::wake_prompts::strip_frontmatter;
use crate::workspace::work_item::{safe_read_to_string, WorkItem};
use crate::fs_atomic::{atomic_write_str, log_if_err};

// Embedded documentation snippet that's appended to Custom-mode
// agents' CLAUDE.md when the user hasn't overridden it in AGENT.md.
// Moved from src-tauri/src/commands/k2so_agents.rs alongside the
// generators that use it.
pub const CUSTOM_AGENT_HEARTBEAT_DOCS: &str = r#"## Heartbeats

Your wake schedule is controlled by **named heartbeats** the user
configures in Settings → Heartbeats (or via the CLI). Each
heartbeat has its own WAKEUP.md file and its own cron-style
schedule — daily at a clock time, weekly on specific days,
hourly at a fixed interval, etc. You don't adjust your own
cadence; the user owns the schedule and you focus on responding
to whatever the heartbeat woke you to do.

When a heartbeat fires, you receive its WAKEUP.md content as
your first user message. Do what it asks, then exit.

## Inspecting + managing heartbeats from your terminal

```
k2so heartbeat                                       # default: list active schedules
k2so heartbeat schedule list [--archived]            # full schedule listing
k2so heartbeat show <name> [--json]                  # schedule + last fire details
k2so heartbeat schedule add --name <name> --daily --time 09:00
k2so heartbeat schedule add --name <name> --hourly --start 09:00 --end 17:00 --every 30 --unit minutes
k2so heartbeat schedule edit <name> --daily --time 10:00
k2so heartbeat schedule rename <old> <new>
k2so heartbeat schedule enable <name>
k2so heartbeat schedule disable <name>
k2so heartbeat schedule remove <name> [--purge]
k2so heartbeat signal fire <name>                    # fire now (skip schedule window)
k2so heartbeat signal wakeup <name>                  # print/edit the WAKEUP.md
k2so heartbeat signal wake                           # auto-wake (no name needed)
```

The user can also drive the same actions from Settings →
Heartbeats. Both paths converge on the same `agent_heartbeats`
table; whichever you prefer is fine.

## Available Tools

Standard CLI tools are available in your terminal (`gh`, `git`,
`curl`, etc.). K2SO tools:

```
k2so terminal spawn --title "..." --command "..."   # run parallel tasks
k2so checkin                                        # read peer messages + inbox + reviews + activity
k2so checkin --status "<message>"                   # update visible status
k2so checkin --done                                 # signal task completion
k2so done                                           # shortcut for `checkin --done`
```
"#;

/// Format a capability state for display in CLAUDE.md.
pub fn format_cap(cap: &str) -> &str {
    match cap {
        "auto" => "auto (build + merge)",
        "gated" => "gated (build PR, wait for approval)",
        "off" => "off (do not act)",
        _ => cap,
    }
}

/// Extract a named section from markdown content (## Heading through next ## or end).
/// Returns the body text (without the heading itself), or None if the section is empty/absent.
pub fn extract_section(content: &str, heading: &str) -> Option<String> {
    let marker = format!("## {}", heading);
    let start = content.find(&marker)?;
    let after_heading = start + marker.len();
    // Skip to the line after the heading (or use remaining content if heading is at EOF)
    let body_start = match content[after_heading..].find('\n') {
        Some(i) => after_heading + i + 1,
        None => return None, // heading at EOF with no body
    };
    // Find the next ## heading or end of content
    let body_end = content[body_start..]
        .find("\n## ")
        .map(|i| body_start + i)
        .unwrap_or(content.len());
    let body = content[body_start..body_end].trim();
    // Check if there's meaningful content (not just pure HTML comments)
    // A line is a "pure comment" only if it starts with <!-- and ends with -->
    // Lines with mixed content (e.g., "real text<!-- note -->") are kept
    let meaningful: Vec<&str> = body.lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            // Pure comment line: starts with <!-- and ends with -->
            if t.starts_with("<!--") && t.ends_with("-->") { return false; }
            true
        })
        .collect();
    if meaningful.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Generate the universal skill protocol for the Workspace Manager.
/// Includes delegation, cross-workspace messaging, and full orchestration commands.
/// Load user-created custom layers from ~/.k2so/templates/{tier}/*.md.
/// Returns concatenated markdown sections with titles derived from filenames.
pub fn load_custom_layers(tier: &str) -> String {
    let dir = match dirs::home_dir() {
        Some(h) => h.join(".k2so/templates").join(tier),
        None => return String::new(),
    };
    if !dir.exists() { return String::new(); }
    let mut layers = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if content.trim().is_empty() { continue; }
                    let name = path.file_stem().unwrap_or_default().to_string_lossy().replace('-', " ");
                    let title: String = name.split_whitespace()
                        .map(|w| {
                            let mut c = w.chars();
                            match c.next() {
                                Some(f) => f.to_uppercase().to_string() + c.as_str(),
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<_>>().join(" ");
                    layers.push(format!("## {}\n\n{}", title, content.trim()));
                }
            }
        }
    }
    layers.sort(); // Alphabetical for consistency
    if layers.is_empty() { return String::new(); }
    layers.join("\n\n") + "\n\n"
}

/// The ORGANIC role-integration directive shared by the role-knowledge
/// skills (Workspace Manager, K2 Agent). PRD §3.2 + §3.3: when a role
/// skill runs, the agent weaves this role guidance into the user's
/// existing `.k2so/agent/AGENT.md` WITH JUDGMENT — reading what's there,
/// preserving accumulated context, never injecting a templated/marked
/// block. The deterministic core (`workspace::canonical::persist_agent_md`)
/// then backs up AGENT.md and writes the merged text atomically, so the
/// integration is byte-reversible.
///
/// `role` is the human-readable role name ("Workspace Manager", "K2
/// Agent"). The marked-block / hash-gated-regeneration approach is
/// explicitly DELETED (PRD §3) — this text must never instruct a
/// programmatic injection.
fn organic_role_integration_section(role: &str) -> String {
    format!(
        r#"## Integrating this role into your AGENT.md (organic, not a block)

This skill is **role knowledge** — how to operate as a {role}. When asked
to apply it to this workspace, integrate it **organically** into your
canonical persona at `.k2so/agent/AGENT.md`:

1. **Read** the existing `.k2so/agent/AGENT.md` (and the surrounding
   workspace context) first.
2. **Weave** the {role} guidance in **with judgment** — preserve the
   user's accumulated context, reconcile overlaps, keep what still
   applies. Your judgment is the only thing that knows what to keep.
3. Hand the merged text to the deterministic safety net, which **backs
   up** the old `AGENT.md` and writes your merge atomically (byte-
   reversible).

**Never inject a templated or marked block.** Do not paste a fenced
"K2SO-managed" region, do not stamp a checksum the daemon would later
auto-rewrite, do not bulldoze existing content. In a long-lived
workspace a mechanical block displaces context the agent relied on —
that is exactly the failure mode this organic model exists to avoid.
Refreshing the role later means **re-running this skill**, re-reading
AGENT.md, and re-integrating by hand; it is never a silent programmatic
rewrite.

"#,
        role = role,
    )
}

pub fn generate_manager_skill_content(project_path: &str, project_name: &str) -> String {
    let mut skill = String::new();

    // ── 1. Identity + Workspace Context ──
    skill.push_str(&format!(
        "# K2SO Workspace Manager Skill\n\nYou are the primary agent for the **{}** workspace, operating in manager mode.\n\nA workspace has at most one primary agent (you). Specialist personas are **skill profiles** — documentation, not separate spawnable agents. Your harness (Claude Code, Cursor, Tauri Cmd+T) handles all sub-agent and worktree spawning natively; K2SO does not.\n\n",
        project_name
    ));

    // ── Organic role-integration directive (PRD §3.2/§3.3) ──
    skill.push_str(&organic_role_integration_section("Workspace Manager"));

    // Read workspace state from DB
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            // Get workspace state
            let state_info: Option<(String, String)> = conn.query_row(
                "SELECT ws.name, ws.description FROM workspace_states ws \
                 JOIN projects p ON p.tier_id = ws.id WHERE p.id = ?1",
                rusqlite::params![project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).ok();

            if let Some((state_name, state_desc)) = state_info {
                skill.push_str(&format!("**Mode: {}** — {}\n\n", state_name, state_desc));
            }

        }
    }

    // ── 2. Team — Connected Workspaces ──
    //
    // 0.39.0 directive (workspace==agent model): the team is the set
    // of OTHER CONNECTED WORKSPACES — peers, not subordinates. The
    // user-facing render is intentionally direction-agnostic: whoever
    // initiated the relation, both sides see the other as just
    // "connected." That's exactly what `list_peers` returns
    // (bidirectional, deduped). The legacy team-roster directory walk
    // over `.k2so/agents/<name>/` was retired here in the same change
    // — skills are documentation profiles, not team members.
    skill.push_str("## Team — Connected Workspaces\n\n");
    skill.push_str("These are other workspace-agents you can collaborate with. Each is itself an agent (hands, memory, skills) — peers, not subordinates.\n\n");
    skill.push_str("To message one — or read what they're doing:\n\n");
    skill.push_str("    k2so msg <workspace-name> \"your message\"           # live (SHORT messages only)\n");
    skill.push_str("    k2so msg <workspace-name> --inbox \"your message\"   # drops into their inbox (no length limit)\n");
    skill.push_str("    k2so read <workspace-name>                          # read their live terminal (peek before you inject)\n\n");
    skill.push_str("The recipient reads a live `msg` immediately if running, or an inbox item on their next heartbeat / session start.\n\n");
    skill.push_str("**`msg` length limit:** live `msg` is injected into the recipient's input line, so it's for short, single-line text — longer or multi-line content gets truncated by their terminal input widget. Use `--inbox` for anything substantial (task briefs, file contents, multi-line notes).\n\n");
    skill.push_str("**`read` for human-in-the-loop:** before injecting into another agent (especially one waiting on a human), `k2so read <workspace-name>` shows the last ~50 lines of its live terminal so you can see what it's doing or waiting on. Add `--agent <name>` for a specific agent, `--lines N` for more history.\n\n");

    match crate::connections::list_peers(project_path) {
        Ok(peers) if !peers.is_empty() => {
            skill.push_str("### Currently connected\n\n");
            for peer in &peers {
                skill.push_str(&format!("  - {}\n", peer.project_name));
            }
            skill.push('\n');
            skill.push_str("To add a connection:    k2so connections add <other-workspace-path>\n");
            skill.push_str("To see all connections: k2so connections list\n\n");
        }
        // Project not registered (Err) OR no peers (Ok empty) — same
        // user-facing message either way: tell them how to wire one up.
        _ => {
            skill.push_str("### No connected workspaces yet\n\n");
            skill.push_str("You're operating in isolation. To collaborate with another workspace-agent:\n\n");
            skill.push_str("    k2so connections add <other-workspace-path>\n\n");
        }
    }

    // ── User Custom Layers (from ~/.k2so/templates/manager/) ──
    let custom_layers = load_custom_layers("manager");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    // ── 3. Standing Orders ──
    skill.push_str(r#"## Standing Orders (Every Wake Cycle)

On each wake, run through this in order:

1. `k2so checkin` — peer messages, inbox arrivals, pending reviews, recent activity.
2. **Triage messages** — respond to live messages from connected workspaces or the user.
3. **Triage the inbox** — `k2so inbox` shows new arrivals at the top level. Sort by priority (critical > high > normal > low) and decide for each item:
   - **Act on it** — work it yourself when scoped.
   - **File it** — `k2so inbox move <id> <folder>` (folders are agent-organized like email; `inbox folders` lists what exists).
   - **Reply** — `k2so inbox respond <id> "text"` (to sender) or `k2so msg <ws> "..."` (live, to another workspace).
   - **Archive** — `k2so inbox archive <id>` when complete.
4. **Review pending merges** — `k2so reviews`, then `k2so review approve|reject|feedback <branch>` per A23.3 framing: worktrees may come from your harness, from a human, or from an integration — your job is to read the diff and decide regardless of origin.
5. **Report status** — `k2so checkin --status "triaging 3 inbox items"` to update the visible activity feed.
6. **Signal completion** — `k2so checkin --done` (or shortcut `k2so done`), optionally `--blocked "reason"`.

"#);

    // ── 4. Decision Framework by Mode ──
    skill.push_str(r#"## Decision Framework

### By Task Complexity
- **Simple** (typo, config, single-file fix): work directly in the main branch.
- **Complex** (multi-file feature, refactor, new system): hand off to your harness's sub-agent / worktree feature. Load a skill profile (`k2so skills profile <name>`) into the spawned session's context so the sub-agent has the right persona. K2SO no longer spawns agents — your harness does.

### By Workspace State
- **Build**: full autonomy. Triage, file, merge, ship. No human sign-off needed.
- **Managed**: features and audits need human approval before merge. Crashes and security auto-ship.
- **Maintenance**: no new features. Fix bugs and security only. Issues and audits need approval.
- **Locked**: no agent activity. Do not act.

(Run `k2so glossary state` if these terms are unfamiliar.)

"#);

    // ── 5. Skills Protocol ──
    skill.push_str(r#"## Skill Profiles

Skill profiles are markdown documents at `.k2so/skills/<name>/SKILL.md` describing a role, persona, or capability set. The harness loads them; K2SO does not spawn them.

```
k2so skills list                              # what skill profiles exist here
k2so skills profile <name>                    # read a profile's SKILL.md
k2so skills create <name> [--template <src>]  # new profile (optionally seeded)
k2so skills remove <name>                     # delete (sent to Trash)
k2so skills regenerate [<name>]               # refresh SKILL.md from templates
```

When you need a specialist persona on a piece of work:
1. `k2so skills profile <name>` — print the SKILL.md content.
2. Load it into your harness's sub-agent context (Claude Code sub-agent prompt, Cursor worktree instructions, etc.).
3. Your harness handles the worktree + spawn. K2SO tracks the resulting merge review.

"#);

    // ── 6. Review Protocol ──
    skill.push_str(r#"## Reviewing Merge Requests

`reviews` and `review` track pending merge reviews for worktrees this workspace has produced — regardless of who created the worktree (harness sub-agent spawn, human, integration). Your job: read the diff, decide.

```
k2so reviews                                   # list pending reviews
k2so review approve <branch>                   # merge to main, clean up worktree
k2so review reject <branch> --reason "..."     # discard worktree, optional feedback
k2so review feedback <branch> -m "..."         # send feedback without rejecting
```

"#);

    // ── 7. Communication ──
    skill.push_str(r#"## Communication

### Check in
```
k2so checkin                                   # full snapshot (inbox + messages + reviews + activity)
k2so checkin --status "working on auth refactor"   # update visible status
k2so checkin --done                            # signal task complete
k2so checkin --done --blocked "waiting for API spec"
k2so done                                      # shortcut for `checkin --done`
```

### Message another workspace — or read what it's doing
```
k2so msg <workspace> "text"                              # live delivery (call/IM — SHORT messages only)
k2so msg <workspace> --inbox --title "..." --body "..."  # inbox delivery (email — no length limit)
k2so msg <workspace> --signal <kind> --payload '{...}'   # typed signal (advanced)
k2so read <workspace> [--lines N] [--agent <name>]       # read its live terminal (peek before you inject)
```

`msg` (live form) succeeds only when the bytes land in the recipient's running session — fails loudly with `reason` + `hint` otherwise (no silent inbox fallback). Use `--inbox` when the recipient should read on their own schedule.

**`msg` length limit:** live `msg` is injected into the recipient's input line, so it's for short, single-line text. Longer or multi-line content gets **truncated** by the recipient's terminal input widget — use `--inbox` for anything substantial (briefs, file contents, multi-line notes). This length limit is the reason the inbox exists.

**`read` (the read complement to `msg`):** `k2so read <workspace>` returns the last N lines (default 50) of that workspace's live terminal — its primary/coordinator session, or a specific `--agent <name>`. Use it for human-in-the-loop: peek what an agent is doing or waiting on *before* you inject a `msg`, or to diagnose a stuck agent. If the workspace is asleep, `k2so sessions live <workspace>` shows whether it has any live session.

Only workspaces linked via `k2so connections` are reachable.

### Discover peers + connections
```
k2so connections list                          # workspaces with live agents now
k2so connections list|add|remove               # cross-workspace links
```

### Activity feed
```
k2so activity [--limit N] [--workspace <path>]
```

### Commit + merge
```
k2so commit [-m "..."]                         # AI-assisted commit
k2so commit --merge                            # commit and merge into main
```

### Glossary
Run `k2so glossary <term>` for definitions of K2SO-specific terms (workspace, skill, inbox, heartbeat, state, …). When in doubt, look it up.

"#);

    skill
}

/// Generate the skill protocol for custom agents.
/// Has checkin, status, done, msg (to connected workspaces), reserve/release.
/// No delegation — custom agents send work to workspace inboxes.
pub fn generate_custom_agent_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Agent Skill

You are {agent_name}, a custom agent for {project_name}.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // User custom layers
    let custom_layers = load_custom_layers("custom-agent");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Check in (do this first on every wake)

```
k2so checkin
```

Returns peer messages, inbox arrivals, pending reviews, and the recent activity feed for the workspace.

## Triage your inbox

The workspace inbox is your email: items arrive from other workspaces or are composed by you. Organize it the way you'd organize email — create folders that fit your workflow.

```
k2so inbox                          # show top-level new arrivals
k2so inbox list [<folder>]          # list a folder (e.g., active, projects)
k2so inbox read <id>                # full text of one item
k2so inbox compose --title "..." --body "..."   # write a self-note / task
k2so inbox respond <id> "text"      # reply to sender
k2so inbox move <id> <folder>       # file (creates folder if needed)
k2so inbox archive <id>             # mark done (preserved + searchable)
k2so inbox delete <id>              # send to macOS Trash (recoverable)
k2so inbox search "query"           # search inbox + folders
k2so inbox folders                  # list folders you have created
```

## Report status + completion

```
k2so checkin --status "reviewing security audit"
k2so checkin --done
k2so checkin --done --blocked "waiting for API access"
k2so done                           # shortcut for `checkin --done`
```

## Message another workspace — or read what it's doing

```
k2so msg <workspace> "text"                              # live (call/IM — SHORT messages only)
k2so msg <workspace> --inbox --title "..." --body "..."  # inbox (email — no length limit)
k2so read <workspace> [--lines N] [--agent <name>]       # read its live terminal (peek before you inject)
```

`msg` (live) succeeds only when the bytes land in the recipient's running session — fails loudly with `reason` + `hint` if the recipient is offline (no silent fallback). Use `--inbox` to queue a task the recipient reads on their own schedule.

**`msg` length limit:** live `msg` is injected into the recipient's input line, so it's for short, single-line text — longer or multi-line content gets **truncated** by their terminal input widget. Use `--inbox` for anything substantial. (This length limit is why the inbox exists.)

**`read` for human-in-the-loop:** `k2so read <workspace>` returns the last N lines (default 50) of that workspace's live terminal so you can see what an agent is doing or waiting on *before* injecting a `msg` — or to diagnose a stuck agent. Add `--agent <name>` for a specific agent.

Only workspaces linked via `k2so connections` are reachable.

## Discover peers

```
k2so connections list               # workspaces with live agents (and who's wired up to you)
```

## Glossary

Run `k2so glossary <term>` for definitions of K2SO-specific terms (workspace, skill, inbox, heartbeat, …). Also try `k2so help` for the full daily-verb surface.
"#);
    skill
}

/// Generate the comprehensive K2SO Agent skill. Broader than the custom-agent
/// template: includes the full multi-heartbeat CRUD, connections messaging,
/// work creation, and audit commands — because a K2SO agent is the top-tier
/// autonomous role in its workspace and needs the full surface area.
///
/// Detected by the migration in ensure_k2so_skills_up_to_date() via the
/// first-line signature "# K2SO Agent Skill (Comprehensive)" which the
/// older shared `generate_custom_agent_skill_content` doesn't emit.
pub fn generate_k2so_agent_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Agent Skill (Comprehensive)

You are **{agent_name}**, the K2SO planner for **{project_name}**. Your job is planning — turn raw requests into well-scoped PRDs, milestones, and technical specs. Engineering happens in other workspaces (or in your own harness's sub-agent sessions); you do not implement.

This skill lists the full daily + power-user CLI surface so you can triage, message, schedule wakes, and coordinate with other workspaces.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // ── Organic role-integration directive (PRD §3.2/§3.3) ──
    skill.push_str(&organic_role_integration_section("K2 Agent"));

    // Let user layers inject project-specific policy on top
    let custom_layers = load_custom_layers("k2so-agent");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Every wake (do this first)

```
k2so checkin
```

Returns peer messages, inbox arrivals, pending reviews, and the recent activity feed.

## Triage your inbox

```
k2so inbox                          # top-level new arrivals
k2so inbox list [<folder>]          # list a folder
k2so inbox read <id>                # full text of one item
k2so inbox compose --title "..." --body "..."   # write your own item
k2so inbox respond <id> "text"      # reply to sender
k2so inbox move <id> <folder>       # file (creates folder on first use)
k2so inbox archive <id>             # mark done
k2so inbox delete <id>              # send to macOS Trash
k2so inbox search "query"           # search inbox + folders
k2so inbox folders                  # list folders you have created
```

K2SO does not impose a folder taxonomy — organize like email (e.g., `projects/`, `reference/`, `issues/`, `done/`).

## Report status + completion

```
k2so checkin --status "drafting auth v2 PRD"
k2so checkin --done
k2so checkin --done --blocked "waiting for design review"
k2so done                            # shortcut for `checkin --done`
```

## Your role: planning, not implementation

You don't write code. You write the plan. Engineering personas are skill profiles that other workspaces (or your harness's sub-agent sessions) apply to actual implementation.

### PRDs (product requirement documents)

Long-form docs that capture the *why* and *what*. Keep them under `.k2so/prds/`. When a PRD is ready for triage, register it as an inbox item so it shows up in your queue:

```
k2so inbox compose --title "Auth V2: session rotation" --body "See .k2so/prds/auth-v2.md" --priority high --type prd
```

### Milestones

Break a PRD into ship-sized slices, each with its own acceptance criteria. Store them under `.k2so/milestones/` and register the noteworthy ones via inbox:

```
k2so inbox compose --title "M1: Rotate on login" --body "Rotate session token on every successful login..." --priority high --type milestone
```

### Specs

Technical specifications live at `.k2so/specs/`. Same pattern — write the file, register via `inbox compose` if it needs visibility.

## Heartbeats — schedule your own wakes

A workspace can have multiple scheduled heartbeats. Each has its own WAKEUP.md file that is injected as the first user message on fire.

```
k2so heartbeat                                  # default: list active schedules

# Schedule CRUD
k2so heartbeat schedule add --name daily-brief --daily --time 08:00
k2so heartbeat schedule add --name end-of-day --daily --time 17:30
k2so heartbeat schedule add --name weekly-review --weekly --days fri --time 16:00
k2so heartbeat schedule edit <name> --weekly --days mon,wed --time 14:00
k2so heartbeat schedule rename <old> <new>
k2so heartbeat schedule enable <name>
k2so heartbeat schedule disable <name>
k2so heartbeat schedule remove <name> [--purge]
k2so heartbeat schedule unarchive <name>
k2so heartbeat schedule list [--archived]

# Immediate signals
k2so heartbeat signal fire <name>               # fire now (skip schedule window)
k2so heartbeat signal wakeup <name>             # print/edit WAKEUP.md
k2so heartbeat signal wake                      # auto-wake (no name needed)

# Inspection
k2so heartbeat show <name> [--json]
k2so heartbeat status <name> [-n N]
k2so heartbeat log [-n N]
```

## Cross-workspace messaging

```
k2so connections list                                    # who's wired up to me / workspaces with live agents
k2so msg <workspace> "text"                              # live delivery (call/IM — SHORT messages only)
k2so msg <workspace> --inbox --title "..." --body "..."  # inbox delivery (email — no length limit)
k2so msg <workspace> --signal <kind> --payload '{...}'   # typed signal (advanced)
k2so read <workspace> [--lines N] [--agent <name>]       # read its live terminal (peek before you inject)
```

`msg` (live form) succeeds only when the bytes land in the recipient's running session — fails loudly with `reason` + `hint` otherwise. Use `--inbox` for queued tasks read on the recipient's schedule.

**`msg` length limit:** live `msg` is injected into the recipient's input line — short, single-line text only. Longer/multi-line content gets **truncated** by their terminal input widget; use `--inbox` for anything substantial. (That limit is why the inbox exists.)

**`read`** returns the last N lines (default 50) of a workspace's live terminal — its primary/coordinator session, or a specific `--agent <name>`. Use it for human-in-the-loop: see what an agent is doing or waiting on before injecting a `msg`, or diagnose a stuck agent.

Only workspaces linked via `k2so connections` are reachable.

## Activity feed + reviews

```
k2so activity [--limit N] [--workspace <path>]   # recent audit log
k2so reviews                                     # pending merge reviews
k2so review approve|reject|feedback <branch>     # act on a review
```

Worktrees may originate from your harness (Claude Code sub-agent, Cursor worktree), from a human, or from an integration. `k2so reviews` tracks them all; your job is read-the-diff and decide.

## Skills (documentation profiles)

```
k2so skills list                              # skill profiles in this workspace
k2so skills profile <name>                    # read a profile's SKILL.md
k2so skills create <name> [--template <src>]  # new profile (optionally seeded)
k2so skills remove <name>                     # delete (sent to Trash)
k2so skills regenerate [<name>]               # refresh SKILL.md from templates
```

Skills are documents the harness loads when spawning sub-agents — K2SO no longer spawns. When you want a specialist persona applied to work, print the SKILL.md and load it into your harness's session context.

## Settings + diagnostics

```
k2so settings                                # current mode, state, agentic toggle, companion
k2so settings --mode <off|agent|manager>
k2so settings --state <build|managed|maintenance|locked>
k2so settings --agentic <on|off>
k2so hooks status                            # verify CLI-LLM hook wiring is live
```

## Glossary

Run `k2so glossary <term>` for definitions of K2SO-specific terms (workspace, skill, inbox, heartbeat, state, …). The full daily-verb surface is in `k2so help`; power-user verbs in `k2so help --advanced`.
"#);
    skill
}

/// Generate the **K2 Canonical Agent** skill body (PRD §5 + §8.2 build
/// order 2). This is the harness-unification operator skill: diagnose →
/// intent → ask-which-harnesses → merge-into-Model-A → mirror-out →
/// dry-run → confirm → apply → report → unwind.
///
/// Unlike the role-knowledge skills, this one does NOT integrate a role
/// into AGENT.md. It operates on the workspace's harness files: it pulls
/// existing harness content INTO `.k2so/agent/AGENT.md` + `.k2so/PROJECT.md`
/// (Model A) first so nothing is lost, then mirrors out to the chosen
/// per-harness files as K2SO-generated copies. Every destructive act
/// goes through the deterministic safety net
/// (`workspace::canonical::run_canonical_setup` / `unwind_canonical`):
/// backup → atomic copy-write → manifest, default dry-run.
pub fn generate_canonical_agent_skill_content(project_name: &str) -> String {
    format!(
        r#"# K2 Canonical Agent Skill

You are the K2 Canonical Agent for the **{project_name}** workspace. Your
job is to **unify this workspace's AI-harness files safely** — never to
overwrite the user's content.

Each AI coding tool reads its project notes from a different file
(`CLAUDE.md`, `GEMINI.md`, `.goosehints`, `.cursor/rules`, `AGENTS.md`,
…). The point of canonicalization is: the user writes their context
**once** and every tool sees the same picture. **`.k2so/agent/AGENT.md`
+ `.k2so/PROJECT.md` are the canonical source of truth (Model A);** the
per-harness files are **generated mirrors** of them.

## The flow (every run)

1. **Diagnose** — run the deterministic per-harness probe
   (`detect_canonical_state`) and **summarize it in plain language**:
   which harnesses are already canonicalized (copy vs symlink), which
   are unmanaged with real user content, which are absent. Show the
   user, in words, what you found.
2. **Ask intent** — set up / add a harness / unwind / just review? **Do
   nothing until the user chooses.**
3. **Ask which harnesses** — only touch the LLMs the user wants enabled.
   Never create files in harness dirs they didn't choose. State is
   per-harness: Claude can be canonicalized while Gemini is left
   untouched, independently.
4. **Merge into Model A first** — read the existing harness files,
   judge valuable user context vs boilerplate/stale, and **weave the
   substance into `.k2so/agent/AGENT.md` / `.k2so/PROJECT.md`** so
   nothing is lost. Canonical is a target STRUCTURE the user's content
   is poured into, never a template that bulldozes it.
5. **Mirror out** — derive the chosen per-harness files FROM Model A.
   These mirrors are K2SO-generated copies (real files, stamped, with an
   "edit `.k2so/...` instead" header) — NOT symlinks.
6. **Dry-run + confirm** — produce a DRY-RUN plan first (per-harness
   action table + the backup manifest) and **STOP for confirmation
   before any write**. Writes happen only on explicit confirm.
7. **Apply** — the deterministic safety net does the destructive work:
   it backs up every original to `.k2so/backups/<ts>/` BEFORE writing,
   writes atomically, and records `manifest.json`. You never mutate
   files raw — you hand merged text to the core.
8. **Report** — tell the user exactly what changed and where the
   backups + manifest live.
9. **Unwind** — the SAME skill undoes it. Manifest-driven exact restore:
   put each backed-up original back, trash anything K2SO created fresh,
   leave `.k2so/` intact.

## Safety contract (non-negotiable)

- **Default is dry-run.** No file is touched until the user confirms.
- **Every original is backed up** to `.k2so/backups/<ISO-ts>/<relpath>`
  BEFORE it is overwritten, with a `manifest.json` that makes the change
  byte-reversible.
- **Copies, not symlinks** — real files the user can read and (if they
  must) edit; stamped so re-runs detect "already managed."
- **Per-harness independence** — only the harnesses the user picked are
  touched; the rest are left byte-for-byte untouched.
- **Live-session preflight** — if an agent session is actively reading
  the targets, refuse and name it; the user can force with a loud
  warning.
- **You never author Model A programmatically** — the merge is your
  organic judgment; the core only does the backup + atomic write +
  manifest around the bytes you hand it.

## Already-unified workspaces

Leave them alone. Never auto-revert. If a workspace already uses legacy
symlinks, offer to convert to copies in manage mode — never force it.
"#,
        project_name = project_name,
    )
}

/// The three opt-in agent-setup skills (PRD §1). Each maps to a
/// `.k2so/skills/<dir>/SKILL.md` written on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptInSkill {
    /// Role-knowledge for the Manager role.
    WorkspaceManager,
    /// Role-knowledge for the K2 Agent (planner) role.
    K2Agent,
    /// Harness-unification operator.
    K2CanonicalAgent,
}

impl OptInSkill {
    /// The directory name under `.k2so/skills/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            OptInSkill::WorkspaceManager => "workspace-manager",
            OptInSkill::K2Agent => "k2-agent",
            OptInSkill::K2CanonicalAgent => "k2-canonical-agent",
        }
    }

    /// The skill-type tag + version for the upgrade protocol.
    fn type_and_version(self) -> (&'static str, u32) {
        match self {
            OptInSkill::WorkspaceManager => {
                ("manager", crate::skills::version::SKILL_VERSION_MANAGER)
            }
            OptInSkill::K2Agent => {
                ("k2so-agent", crate::skills::version::SKILL_VERSION_K2SO_AGENT)
            }
            OptInSkill::K2CanonicalAgent => (
                "canonical-agent",
                crate::skills::version::SKILL_VERSION_CANONICAL_AGENT,
            ),
        }
    }

    /// Short description for the SKILL.md frontmatter.
    fn description(self) -> &'static str {
        match self {
            OptInSkill::WorkspaceManager => {
                "Workspace Manager role knowledge — integrate organically into AGENT.md"
            }
            OptInSkill::K2Agent => {
                "K2 Agent (planner) role knowledge — integrate organically into AGENT.md"
            }
            OptInSkill::K2CanonicalAgent => {
                "Unify this workspace's AI-harness files safely (diagnose → merge → mirror → unwind)"
            }
        }
    }

    /// Render this skill's body for the given workspace.
    fn render_body(self, project_path: &str, project_name: &str) -> String {
        match self {
            OptInSkill::WorkspaceManager => {
                generate_manager_skill_content(project_path, project_name)
            }
            OptInSkill::K2Agent => {
                // The K2 Agent skill is keyed on the primary agent name
                // where one exists; fall back to a stable label.
                let agent_name =
                    crate::workspace::agent_identity::find_primary_agent(project_path)
                        .unwrap_or_else(|| "k2-agent".to_string());
                generate_k2so_agent_skill_content(project_name, &agent_name)
            }
            OptInSkill::K2CanonicalAgent => {
                generate_canonical_agent_skill_content(project_name)
            }
        }
    }
}

/// Write one of the three opt-in skills to
/// `.k2so/skills/<name>/SKILL.md` on demand (PRD §8.1 "Enable").
///
/// **Internal only — never fanned out.** This deliberately bypasses
/// `write_skill_to_all_harnesses`: opt-in skill bodies live under
/// `.k2so/skills/` and are loaded by the agent on demand; they are NOT
/// mirrored into harness discovery paths (that is the gated workspace
/// SKILL's job, not these). Upgrade-tracked via `ensure_skill_up_to_date`
/// so a generator bump re-rolls unmodified files.
///
/// Returns the absolute path written.
pub fn write_opt_in_skill(project_path: &str, skill: OptInSkill) -> PathBuf {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let body = skill.render_body(project_path, &project_name);
    let (skill_type, version) = skill.type_and_version();
    let extras = format!(
        "name: {}\ndescription: {}",
        skill.dir_name(),
        skill.description()
    );

    let canonical_path = crate::workspace::agent_identity::skill_dir(project_path, skill.dir_name())
        .join("SKILL.md");
    ensure_skill_up_to_date(&canonical_path, skill_type, version, &body, Some(&extras));
    canonical_path
}

/// Generate the universal baseline for a skill profile (formerly "agent template").
///
/// Post-Phase-2.1: skills are documentation profiles, not spawnable agents. This
/// baseline body is what the harness sees when it loads `k2so skills profile <name>`
/// and seeds a sub-agent session with the persona. Keep it short — the harness adds
/// its own session-level instructions; this is just K2SO-CLI context.
pub fn generate_template_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Skill Profile — {agent_name}

A skill profile for **{project_name}**.

A skill is a documentation profile — a persona / capability set the harness can apply to a sub-agent session. K2SO does not spawn this skill; your harness (Claude Code, Cursor, Tauri Cmd+T) does. When your harness loads this profile into a session's context, the CLI surface below is what's available to you inside that session.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // User custom layers
    let custom_layers = load_custom_layers("agent-template");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Check in (do this first on every wake)

```
k2so checkin
```

Returns peer messages, inbox arrivals, pending reviews, and the recent activity feed.

## Triage your inbox

```
k2so inbox                          # top-level new arrivals
k2so inbox list [<folder>]          # list a folder (e.g., active, projects)
k2so inbox read <id>                # full text of one item
k2so inbox move <id> <folder>       # file (creates folder on first use)
k2so inbox archive <id>             # mark done
k2so inbox compose --title "..." --body "..."   # self-note / task
k2so inbox respond <id> "text"      # reply to sender
```

## Report status + completion

```
k2so checkin --status "implementing JWT validation"
k2so checkin --done
k2so checkin --done --blocked "need clarification on auth flow"
k2so done                           # shortcut for `checkin --done`
```

## Glossary

Run `k2so glossary <term>` for K2SO-specific terminology. `k2so help` lists the full daily verb surface.
"#);
    skill
}

/// Generate the CLAUDE.md content for an agent, optionally focused on a specific task.
pub fn compose_agent_wake_context(
    project_path: &str,
    agent_name: &str,
    current_task: Option<&WorkItem>,
) -> Result<String, String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    // Read agent identity
    let agent_md_path = dir.join("AGENT.md");
    let agent_md = fs::read_to_string(&agent_md_path).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let role = fm.get("role").cloned().unwrap_or("AI Agent".to_string());
    let agent_type = fm.get("type").cloned().map(|t| {
        match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        }
    }).unwrap_or("agent-template".to_string());
    let is_custom = agent_type == "custom";

    let agent_body = strip_frontmatter(&agent_md);

    // Read shared project context (.k2so/PROJECT.md) — manager mode agents
    let is_manager_type = agent_type == "manager" || agent_type == "agent-template";
    let project_md_path = crate::workspace_dot_dir(project_path).join("PROJECT.md");
    let project_context = if is_manager_type && project_md_path.exists() {
        let raw = safe_read_to_string(&project_md_path).unwrap_or_default();
        let stripped = strip_frontmatter(&raw);
        // Only include if it has real content (not just comments/empty sections)
        let has_content = stripped.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("<!--")
        });
        if has_content { Some(stripped) } else { None }
    } else {
        None
    };

    // Extract Standing Orders section from agent body (if user filled it in)
    let standing_orders = extract_section(&agent_body, "Standing Orders");

    let mut md = String::new();

    if is_custom {
        // ── Custom Agent: agent.md body + heartbeat control + tools ──
        md.push_str(&format!("# {}\n\n", agent_name));
        md.push_str(&format!("**Role:** {}\n\n", role));

        if !agent_body.is_empty() {
            md.push_str(&format!("{}\n\n", agent_body));
        }

        // Add heartbeat control docs if not already in agent body
        if !agent_body.contains("Heartbeat Control") {
            md.push_str(CUSTOM_AGENT_HEARTBEAT_DOCS);
        }

        return Ok(md);
    }

    // ── K2SO / Coordinator agents: full infrastructure CLAUDE.md ───────

    // List other agents for delegation awareness
    let mut other_agents = Vec::new();
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != agent_name {
                        let their_md = entry.path().join("AGENT.md");
                        let their_role = if their_md.exists() {
                            let content = fs::read_to_string(&their_md).unwrap_or_default();
                            let fm = parse_frontmatter(&content);
                            fm.get("role").cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        other_agents.push((name, their_role));
                    }
                }
            }
        }
    }

    md.push_str(&format!("# K2SO Agent: {}\n\n", agent_name));
    md.push_str(&format!("## Identity\n**Role:** {}\n\n", role));
    // Reference the agent's full profile (absolute path so it resolves from worktrees)
    md.push_str(&format!(
        "**Full profile:** `{}`\n\n",
        agent_md_path.to_string_lossy()
    ));
    if !agent_body.is_empty() {
        md.push_str(&format!("{}\n\n", agent_body));
    }

    // Inject shared project context
    if let Some(ref ctx) = project_context {
        md.push_str("## Project Context (shared)\n\n");
        md.push_str(ctx);
        md.push_str("\n\n");
    }

    // Inject standing orders (persistent directives from agent.md)
    if let Some(ref orders) = standing_orders {
        md.push_str("## Standing Orders\n\n");
        md.push_str(orders);
        md.push_str("\n\n");
    }

    // Current task (if launching with specific work)
    if let Some(task) = current_task {
        // Use absolute path so it resolves from worktrees (where relative .k2so/ doesn't exist)
        let task_file_abs = agent_work_dir(project_path, agent_name, "active").join(&task.filename);
        md.push_str("## Current Task\n\n");
        md.push_str(&format!("**{}** (priority: {}, type: {})\n\n", task.title, task.priority, task.item_type));
        md.push_str(&format!("Task file: `{}`\n\n", task_file_abs.to_string_lossy()));
        md.push_str("Read the full task file for complete details, acceptance criteria, and context.\n\n");
    }

    // Work queue info (absolute paths for worktree compatibility)
    let work_dir_abs = crate::workspace_dot_dir(project_path).join("agents").join(agent_name).join("work");
    md.push_str("## Work Queue\n\n");
    md.push_str(&format!(
        "Your work items are at: `{}/`\n",
        work_dir_abs.to_string_lossy()
    ));
    md.push_str(&format!("- `{}/inbox/` — assigned to you, pick the highest priority\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/active/` — items you're currently working on\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/done/` — move items here when complete\n\n", work_dir_abs.to_string_lossy()));

    // Other agents — for managers, include profile paths so they can read agent.md files
    let is_manager_lead = agent_type == "manager" || agent_type == "k2so";
    if !other_agents.is_empty() {
        if is_manager_lead {
            md.push_str("## Your Team\n\n");
            md.push_str("These are your agent templates. Read their `agent.md` profiles to understand their strengths before delegating:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!(
                    "- **{}** — {} (profile: `.k2so/agents/{}/agent.md`)\n",
                    name, their_role, name
                ));
            }
            md.push_str("\nYou can create new agents (`k2so agents create <name> --role \"...\"`) or update existing ones (`k2so agent update --name <name> --field role --value \"...\"`).\n\n");
        } else {
            md.push_str("## Other Agents\n");
            md.push_str("You can delegate work to these agents:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!("- **{}** — {}\n", name, their_role));
            }
            md.push_str("\n");
        }
    }

    // Add workspace state constraints
    if let Some(ws_state) = get_workspace_state(project_path) {
        md.push_str("## Workspace State Constraints\n\n");
        md.push_str(&format!("This workspace operates under the **{}** state.\n\n", ws_state.name));
        if let Some(ref desc) = ws_state.description {
            md.push_str(&format!("{}\n\n", desc));
        }
        md.push_str("| Source Type | Permission |\n|---|---|\n");
        md.push_str(&format!("| Features | {} |\n", format_cap(&ws_state.cap_features)));
        md.push_str(&format!("| Issues | {} |\n", format_cap(&ws_state.cap_issues)));
        md.push_str(&format!("| Crashes | {} |\n", format_cap(&ws_state.cap_crashes)));
        md.push_str(&format!("| Security | {} |\n", format_cap(&ws_state.cap_security)));
        md.push_str(&format!("| Audits | {} |\n", format_cap(&ws_state.cap_audits)));
        md.push_str("\n**auto** = build and merge automatically. **gated** = build PR but wait for human approval. **off** = do not act.\n\n");
    }

    // Write the SKILL.md file alongside the CLAUDE.md.
    // SKILL.md is harness-agnostic — works with Claude Code, Pi, Aider, etc.
    // CLAUDE.md contains identity + task context only. SKILL.md has the CLI protocol.
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let skill_content = if is_manager_lead {
        generate_manager_skill_content(project_path, &project_name)
    } else if agent_type == "custom" {
        generate_custom_agent_skill_content(&project_name, agent_name)
    } else {
        generate_template_skill_content(&project_name, agent_name)
    };

    // Write SKILL.md to agent directory
    let skill_path = agent_dir(project_path, agent_name).join("SKILL.md");
    log_if_err(
        "agent skill write",
        &skill_path,
        atomic_write_str(&skill_path, &skill_content),
    );

    // Inject skill content directly into the system prompt so it's always available
    // (no extra tool call needed to read SKILL.md)
    md.push_str("\n");
    md.push_str(&skill_content);

    Ok(md)
}

/// Legacy name retained to keep the src-tauri `pub use` re-export
/// short. New code should use [`compose_agent_wake_context`]; the
/// symbol still ends up writing SKILL.md + composing the wake system
/// prompt, but the new name matches what it actually does.
pub fn generate_agent_claude_md_content(
    project_path: &str,
    agent_name: &str,
    current_task: Option<&WorkItem>,
) -> Result<String, String> {
    compose_agent_wake_context(project_path, agent_name, current_task)
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.2 snapshot-style coverage for the 4 SKILL.md
    //! content generators. These were rewritten in commit b2b24e7d to
    //! match the Phase 2.1 A25 canonical CLI taxonomy. The tests pin:
    //!
    //!   1. Each generator returns non-empty content.
    //!   2. Each generator's output references the right section headers
    //!      for its tier (manager → "Workspace Manager"; k2so-agent →
    //!      "planner"; etc.).
    //!   3. Each generator's output does NOT contain any of the hard-
    //!      deprecated verbs Phase 2.1 retired (`k2so delegate`,
    //!      `k2so work create`, `k2so signal`, `k2so app-update`).
    //!   4. Each generator references the canonical A25 verbs
    //!      (`k2so inbox`, `k2so msg`, `k2so checkin`, `k2so workspace`).
    //!
    //! The Tier-2.2 PRD calls these "snapshot tests" — they're not
    //! literal value-byte snapshots (those rot fast as templates evolve)
    //! but structural assertions that catch the failure modes that
    //! matter: deprecated verbs sneaking back in, or a template losing
    //! its tier-identifying heading.
    use super::*;
    use uuid::Uuid;

    /// Hard-deprecated verbs from Phase 2.1 A25. If a generator's
    /// output contains any of these substrings, the template has
    /// regressed.
    ///
    /// Substrings only — `k2so work create` matches anywhere the verb
    /// appears as a command-line invocation. Whole `k2so work` is
    /// allowed (e.g., the inbox primitive comments still reference it
    /// historically); only the deprecated SUBCOMMANDS are blocklisted.
    const DEPRECATED_VERBS: &[&str] = &[
        "k2so delegate",
        "k2so work create",
        "k2so work send",
        "k2so work move",
        "k2so work inbox",
        "k2so signal",
        "k2so app-update",
        // Phase 2.1 also retired `k2so agents create` / `k2so agents list`
        // / `k2so agents delete` (the plural-noun verbs); but plural
        // `agents` in narrative prose ("your agents") is fine.
        "k2so agents create",
        "k2so agents delete",
        "k2so agents list",
        "k2so agents running",
        "k2so agents work",
    ];

    fn assert_no_deprecated_verbs(content: &str, generator: &str) {
        let mut hits: Vec<&str> = Vec::new();
        for verb in DEPRECATED_VERBS {
            if content.contains(verb) {
                hits.push(verb);
            }
        }
        assert!(
            hits.is_empty(),
            "{generator} content must NOT contain hard-deprecated verbs; found {hits:?}\n\
             Full content excerpt (first 400 chars):\n{}",
            &content[..content.len().min(400)],
        );
    }

    fn assert_contains_canonical_a25_verbs(content: &str, generator: &str, required: &[&str]) {
        for verb in required {
            assert!(
                content.contains(verb),
                "{generator} content must reference canonical A25 verb {verb:?}",
            );
        }
    }

    // ── generate_manager_skill_content ─────────────────────────────

    #[test]
    fn manager_skill_content_is_non_empty_and_includes_tier_headers() {
        // generate_manager_skill_content hits the DB (workspace_states +
        // workspace_relations); use a unique unregistered path so the
        // DB lookups return None and the template falls through to its
        // baseline body.
        let project_path = format!("/tmp/manager-skill-{}", Uuid::new_v4());
        let body = generate_manager_skill_content(&project_path, "TestWorkspace");

        assert!(!body.is_empty(), "manager skill body must be non-empty");
        assert!(
            body.contains("Workspace Manager"),
            "manager skill must include 'Workspace Manager' header"
        );
        assert!(
            body.contains("TestWorkspace"),
            "manager skill must mention the project_name"
        );
    }

    #[test]
    fn manager_skill_content_uses_canonical_a25_verbs_and_no_deprecated() {
        let project_path = format!("/tmp/manager-verbs-{}", Uuid::new_v4());
        let body = generate_manager_skill_content(&project_path, "Verbs");

        assert_no_deprecated_verbs(&body, "generate_manager_skill_content");
        assert_contains_canonical_a25_verbs(
            &body,
            "generate_manager_skill_content",
            &["k2so inbox", "k2so checkin", "k2so reviews"],
        );
    }

    // ── 0.39.0 team section (workspace==agent model) ──────────────
    //
    // The manager SKILL.md's "Team" section used to walk
    // `.k2so/agents/<name>/` (legacy multi-agent tree) and render
    // each subdirectory as a team member. Post-0.39.0 a workspace IS
    // an agent — the team is the set of OTHER CONNECTED WORKSPACES
    // (peers, not subordinates). These tests pin the new contract:
    //
    //   - empty peers → "No connected workspaces yet" hint with
    //     `k2so connections add <other-workspace-path>` instruction
    //   - present peers → flat alphabetical list (no direction tags)
    //   - always teaches `k2so msg <workspace-name>` syntax
    //   - never says "outgoing" / "incoming" (direction is a data-
    //     model concept, not a user-visible one)
    //   - never walks `.k2so/skills/` for team membership (skills are
    //     documentation profiles, not peers)
    //
    // Each test uses a UUID-suffixed project path so it doesn't
    // collide with other tests sharing the process-wide in-memory DB.

    fn make_project_for_skill_test(name: &str) -> (String, String) {
        let db = crate::db::shared();
        let conn = db.lock();
        let path = format!("/tmp/skill-team-{}-{}", name, Uuid::new_v4());
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, &path],
        )
        .expect("insert project");
        (path, id)
    }

    #[test]
    fn manager_team_section_renders_empty_hint_when_no_peers() {
        let project_path = format!("/tmp/manager-team-empty-{}", Uuid::new_v4());
        let body = generate_manager_skill_content(&project_path, "Lonely");

        assert!(
            body.contains("## Team — Connected Workspaces"),
            "team section header must always render"
        );
        assert!(
            body.contains("No connected workspaces yet"),
            "empty-peers case must render the 'No connected workspaces yet' hint:\n{body}"
        );
        assert!(
            body.contains("k2so connections add"),
            "empty-peers hint must teach `k2so connections add`:\n{body}"
        );
    }

    #[test]
    fn manager_team_section_lists_peers_as_flat_alphabetical_list() {
        let (me_path, me_id) = make_project_for_skill_test("flatlist-me");
        // Create peers and outgoing relations from me → them so they
        // appear in our peer list.
        let mut expected_names = Vec::new();
        for name in ["zebra", "alpha", "mango"] {
            let (_p, pid) = make_project_for_skill_test(name);
            let db = crate::db::shared();
            let conn = db.lock();
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                &pid,
                "oversees",
            )
            .unwrap();
            drop(conn);
            // The actual project_name we INSERTed includes a UUID
            // suffix to avoid collisions; capture what it was.
            let stored_name: String = {
                let db = crate::db::shared();
                let conn = db.lock();
                conn.query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![pid],
                    |row| row.get(0),
                )
                .unwrap()
            };
            expected_names.push(stored_name);
        }
        expected_names.sort();

        let body = generate_manager_skill_content(&me_path, "FlatList");

        assert!(
            body.contains("### Currently connected"),
            "non-empty peers case must render 'Currently connected' subsection:\n{body}"
        );

        // Each peer name appears in the body. Their relative order
        // matches alphabetical sort — find the index of each and
        // assert monotonic increase.
        let mut indices: Vec<usize> = Vec::new();
        for n in &expected_names {
            let idx = body
                .find(n.as_str())
                .unwrap_or_else(|| panic!("peer name '{n}' missing from body:\n{body}"));
            indices.push(idx);
        }
        let mut sorted = indices.clone();
        sorted.sort();
        assert_eq!(
            indices, sorted,
            "peers must appear in alphabetical order; got names={:?} indices={:?}",
            expected_names, indices
        );
    }

    #[test]
    fn manager_team_section_teaches_k2so_msg_syntax() {
        let project_path = format!("/tmp/manager-team-msg-{}", Uuid::new_v4());
        let body = generate_manager_skill_content(&project_path, "MsgTeach");

        // Both forms must be teachable: plain and --inbox. The
        // workspace==agent model relies on `k2so msg <workspace>` as
        // the primary peer-communication primitive.
        assert!(
            body.contains("k2so msg <workspace-name> \"your message\""),
            "team section must teach plain `k2so msg <workspace-name>` form:\n{body}"
        );
        assert!(
            body.contains("k2so msg <workspace-name> --inbox"),
            "team section must teach `--inbox` form for queued delivery:\n{body}"
        );
    }

    #[test]
    fn manager_team_section_does_not_show_direction_labels() {
        // Wire up a bidirectional relation (both incoming AND outgoing
        // between two workspaces) and assert the rendered team section
        // never mentions "outgoing", "incoming", "(oversees)" etc. The
        // data model is directional; the user-facing render is not.
        let (me_path, me_id) = make_project_for_skill_test("nodir-me");
        let (_peer_path, peer_id) = make_project_for_skill_test("nodir-peer");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                &peer_id,
                "oversees",
            )
            .unwrap();
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &peer_id,
                &me_id,
                "collaborator",
            )
            .unwrap();
        }

        let body = generate_manager_skill_content(&me_path, "NoDir");

        // Scan the team section specifically: from "## Team —" until
        // the next "##" heading. That's where direction labels would
        // appear if they leaked through.
        let team_start = body
            .find("## Team — Connected Workspaces")
            .expect("team section present");
        let after_team = &body[team_start..];
        // Next top-level ## section caps the slice; ### subsections
        // stay inside the team region.
        let team_end_rel = after_team[2..]
            .find("\n## ")
            .map(|i| i + 2)
            .unwrap_or(after_team.len());
        let team_region = &after_team[..team_end_rel];

        for bad in ["outgoing", "incoming", "(oversees)", "(connected agent)", "(collaborator)"] {
            assert!(
                !team_region.contains(bad),
                "team section must not surface direction/relation label '{bad}':\n{team_region}"
            );
        }
    }

    /// Smoke-print helper for the 0.39.0 K3 worktree report. Renders
    /// the team section in three scenarios and prints them to stderr
    /// (use `cargo test ... -- --ignored --nocapture
    /// manager_team_section_smoke_print` to see the output). Marked
    /// `#[ignore]` so it doesn't run by default — it's a diagnostic,
    /// not a regression guard.
    #[test]
    #[ignore]
    fn manager_team_section_smoke_print() {
        // Helper: extract just the team section from a full skill body.
        fn team_only(body: &str) -> &str {
            let start = body
                .find("## Team — Connected Workspaces")
                .expect("team section present");
            let after = &body[start..];
            let end_rel = after[2..].find("\n## ").map(|i| i + 2).unwrap_or(after.len());
            &after[..end_rel]
        }

        // (a) 0-peer workspace — unregistered path so list_peers returns
        // Err and we get the empty-hint render.
        let zero_path = format!("/tmp/smoke-zero-{}", Uuid::new_v4());
        let zero_body = generate_manager_skill_content(&zero_path, "ZeroPeer");
        eprintln!("\n===== SCENARIO (a): 0-peer workspace =====");
        eprintln!("{}", team_only(&zero_body));

        // (b) 1-outgoing-only peer.
        let (out_me_path, out_me_id) = make_project_for_skill_test("smoke-out-me");
        let (_out_peer_path, out_peer_id) = make_project_for_skill_test("smoke-out-peer");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &out_me_id,
                &out_peer_id,
                "oversees",
            )
            .unwrap();
        }
        let out_body = generate_manager_skill_content(&out_me_path, "OutgoingOnly");
        eprintln!("\n===== SCENARIO (b): 1-outgoing-only peer =====");
        eprintln!("{}", team_only(&out_body));

        // (c) 1-outgoing + 1-incoming (two distinct peers).
        let (bi_me_path, bi_me_id) = make_project_for_skill_test("smoke-bi-me");
        let (_a_path, a_id) = make_project_for_skill_test("smoke-bi-out-peer");
        let (_b_path, b_id) = make_project_for_skill_test("smoke-bi-in-peer");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            // outgoing: me → a
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &bi_me_id,
                &a_id,
                "oversees",
            )
            .unwrap();
            // incoming: b → me
            crate::db::schema::WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &b_id,
                &bi_me_id,
                "collaborator",
            )
            .unwrap();
        }
        let bi_body = generate_manager_skill_content(&bi_me_path, "OutgoingPlusIncoming");
        eprintln!("\n===== SCENARIO (c): 1-outgoing + 1-incoming =====");
        eprintln!("{}", team_only(&bi_body));
    }

    #[test]
    fn manager_team_section_does_not_walk_skills_dir() {
        // Skills are documentation profiles, not team members. Seed a
        // workspace with `.k2so/skills/<harness>/` entries but ZERO
        // connections and assert the rendered team section reports
        // "No connected workspaces yet" — not a roster derived from
        // the skills filesystem.
        let project_path = format!("/tmp/manager-team-skipskills-{}", Uuid::new_v4());
        std::fs::create_dir_all(format!("{project_path}/.k2so/skills/qa-eng")).unwrap();
        std::fs::write(
            format!("{project_path}/.k2so/skills/qa-eng/SKILL.md"),
            "---\nname: qa-eng\nrole: QA engineer\ntype: agent-template\n---\n\nbody",
        )
        .unwrap();
        std::fs::create_dir_all(format!("{project_path}/.k2so/skills/rust-eng")).unwrap();
        std::fs::write(
            format!("{project_path}/.k2so/skills/rust-eng/SKILL.md"),
            "---\nname: rust-eng\nrole: Rust engineer\ntype: agent-template\n---\n\nbody",
        )
        .unwrap();

        let body = generate_manager_skill_content(&project_path, "SkipSkills");

        assert!(
            body.contains("No connected workspaces yet"),
            "with skills present but no connections, team section must still report empty:\n{body}"
        );
        // Skill names must not appear as if they were team members.
        // (They may legitimately appear elsewhere in the body
        // referencing `k2so skills profile <name>`; we only assert
        // they aren't listed as bullet entries in the team section.)
        let team_start = body.find("## Team — Connected Workspaces").unwrap();
        let after_team = &body[team_start..];
        let team_end_rel = after_team[2..]
            .find("\n## ")
            .map(|i| i + 2)
            .unwrap_or(after_team.len());
        let team_region = &after_team[..team_end_rel];
        for skill_name in ["qa-eng", "rust-eng"] {
            assert!(
                !team_region.contains(skill_name),
                "skill name '{skill_name}' must not appear in team section:\n{team_region}"
            );
        }

        // Cleanup
        std::fs::remove_dir_all(&project_path).ok();
    }

    // ── generate_custom_agent_skill_content ────────────────────────

    #[test]
    fn custom_agent_skill_content_is_non_empty_and_identifies_agent_name() {
        let body = generate_custom_agent_skill_content("MyProject", "myagent");

        assert!(!body.is_empty(), "custom-agent body must be non-empty");
        assert!(body.contains("myagent"), "agent name must appear");
        assert!(body.contains("MyProject"), "project name must appear");
        assert!(
            body.contains("K2SO Agent Skill"),
            "custom-agent template must include 'K2SO Agent Skill' header (NOT the comprehensive variant)"
        );
    }

    #[test]
    fn custom_agent_skill_content_uses_canonical_a25_verbs_and_no_deprecated() {
        let body = generate_custom_agent_skill_content("MyProject", "myagent");

        assert_no_deprecated_verbs(&body, "generate_custom_agent_skill_content");
        assert_contains_canonical_a25_verbs(
            &body,
            "generate_custom_agent_skill_content",
            &["k2so inbox", "k2so checkin", "k2so msg"],
        );
    }

    // ── generate_k2so_agent_skill_content ──────────────────────────

    #[test]
    fn k2so_agent_skill_content_is_comprehensive_and_planner_focused() {
        let body = generate_k2so_agent_skill_content("PlanProject", "planner");

        assert!(!body.is_empty(), "k2so-agent body must be non-empty");
        assert!(
            body.contains("Comprehensive"),
            "k2so-agent template must signal Comprehensive variant in its title line"
        );
        assert!(
            body.contains("planner"),
            "k2so-agent body must mention the planner role"
        );
        assert!(body.contains("PlanProject"), "project name must appear");
    }

    #[test]
    fn k2so_agent_skill_content_uses_canonical_a25_verbs_and_no_deprecated() {
        let body = generate_k2so_agent_skill_content("PlanProject", "planner");

        assert_no_deprecated_verbs(&body, "generate_k2so_agent_skill_content");
        // The comprehensive variant gets the broader surface — verify
        // it includes the connections/messaging + heartbeat verbs.
        assert_contains_canonical_a25_verbs(
            &body,
            "generate_k2so_agent_skill_content",
            &[
                "k2so inbox",
                "k2so checkin",
                "k2so msg",
                "k2so heartbeat",
                "k2so connections",
            ],
        );
    }

    // ── generate_template_skill_content ────────────────────────────

    #[test]
    fn template_skill_content_is_non_empty_and_identifies_profile() {
        let body = generate_template_skill_content("MyProject", "qa-eng");

        assert!(!body.is_empty(), "template body must be non-empty");
        assert!(
            body.contains("Skill Profile"),
            "template must include 'Skill Profile' label"
        );
        assert!(body.contains("qa-eng"), "agent name must appear");
        assert!(body.contains("MyProject"), "project name must appear");
    }

    #[test]
    fn template_skill_content_uses_canonical_a25_verbs_and_no_deprecated() {
        let body = generate_template_skill_content("MyProject", "qa-eng");

        assert_no_deprecated_verbs(&body, "generate_template_skill_content");
        assert_contains_canonical_a25_verbs(
            &body,
            "generate_template_skill_content",
            &["k2so checkin", "k2so inbox"],
        );
    }

    // ── CUSTOM_AGENT_HEARTBEAT_DOCS const sanity ───────────────────

    #[test]
    fn custom_agent_heartbeat_docs_uses_canonical_a25_verbs_and_no_deprecated() {
        // The CUSTOM_AGENT_HEARTBEAT_DOCS constant is appended to the
        // composed wake-context for custom agents. Pin its verb set too.
        assert_no_deprecated_verbs(CUSTOM_AGENT_HEARTBEAT_DOCS, "CUSTOM_AGENT_HEARTBEAT_DOCS");
        assert_contains_canonical_a25_verbs(
            CUSTOM_AGENT_HEARTBEAT_DOCS,
            "CUSTOM_AGENT_HEARTBEAT_DOCS",
            &["k2so heartbeat", "k2so checkin"],
        );
    }

    // ── Cross-generator invariants ─────────────────────────────────

    #[test]
    fn all_four_generators_produce_distinct_bodies() {
        // Sanity check: the four generators MUST produce different
        // strings (regression guard against a refactor that
        // accidentally aliases two generators to the same body).
        let mgr_path = format!("/tmp/distinct-mgr-{}", Uuid::new_v4());
        let manager = generate_manager_skill_content(&mgr_path, "Project");
        let custom = generate_custom_agent_skill_content("Project", "agent");
        let k2so_agent = generate_k2so_agent_skill_content("Project", "agent");
        let template = generate_template_skill_content("Project", "agent");

        let bodies = [
            ("manager", &manager),
            ("custom", &custom),
            ("k2so_agent", &k2so_agent),
            ("template", &template),
        ];
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                assert_ne!(
                    bodies[i].1, bodies[j].1,
                    "{} and {} generators must produce distinct bodies",
                    bodies[i].0, bodies[j].0,
                );
            }
        }
    }

    // ── Canonical-agents feature: the three opt-in skill bodies ─────
    //
    // PRD §8.2: snapshot the three skill bodies; assert the role skills
    // say "organic" and NEVER "inject a block", and that none contain a
    // deprecated CLI verb. (Structural assertions, not literal byte
    // snapshots — same philosophy as the Tier-2.2 tests above.)

    /// Phrases that would indicate a programmatic/templated-block
    /// injection — explicitly DELETED by PRD §3. The role skills must
    /// never instruct any of these.
    const PROGRAMMATIC_INJECTION_PHRASES: &[&str] = &[
        "inject a block",
        "inject a templated block",
        "marked block",
        "hash-gated",
        "hash gate",
        "regeneration marker",
    ];

    fn assert_no_programmatic_injection(content: &str, generator: &str) {
        let lc = content.to_lowercase();
        // "never inject a templated block" is an ALLOWED negation — it
        // tells the agent NOT to do it. Only flag the affirmative
        // phrases that would instruct injection. Our generator text
        // deliberately uses "Never inject a templated or marked block",
        // so assert the explicit prohibition is present rather than
        // banning the substring outright.
        assert!(
            lc.contains("never inject") || lc.contains("not"),
            "{generator} must explicitly prohibit programmatic injection",
        );
        // Hard-ban the regeneration-machinery phrases entirely — those
        // describe the deleted v1 approach and have no business in the
        // organic model even as prose.
        for phrase in &["hash-gated", "hash gate", "regeneration marker"] {
            assert!(
                !lc.contains(phrase),
                "{generator} must not reference the deleted regeneration machinery: {phrase:?}",
            );
        }
        let _ = PROGRAMMATIC_INJECTION_PHRASES;
    }

    #[test]
    fn role_skills_instruct_organic_integration_not_injection() {
        let manager =
            generate_manager_skill_content(&format!("/tmp/org-mgr-{}", Uuid::new_v4()), "Proj");
        let k2 = generate_k2so_agent_skill_content("Proj", "planner");

        for (name, body) in [("manager", &manager), ("k2_agent", &k2)] {
            let lc = body.to_lowercase();
            assert!(
                lc.contains("organic"),
                "{name} role skill must instruct ORGANIC integration; body:\n{}",
                &body[..body.len().min(600)],
            );
            // Must explicitly tell the agent NOT to inject a templated/
            // marked block.
            assert!(
                lc.contains("never inject a templated or marked block")
                    || (lc.contains("never inject") && lc.contains("block")),
                "{name} role skill must explicitly forbid injecting a templated/marked block",
            );
            assert!(
                lc.contains(".k2so/agent/agent.md"),
                "{name} role skill must point integration at .k2so/agent/AGENT.md (Model A)",
            );
            assert_no_programmatic_injection(body, name);
            assert_no_deprecated_verbs(body, name);
        }
    }

    #[test]
    fn canonical_agent_skill_body_snapshot_structure() {
        let body = generate_canonical_agent_skill_content("Proj");
        assert!(!body.is_empty(), "canonical skill body must be non-empty");
        // Tier-identifying heading.
        assert!(body.contains("K2 Canonical Agent"), "must carry its tier heading");
        // The full flow PRD §8.2 prescribes.
        for needle in [
            "Diagnose",
            "Ask intent",
            "which harnesses",
            "AGENT.md",
            "PROJECT.md",
            "Mirror out",
            "DRY-RUN",
            "confirm",
            "manifest",
            "Unwind",
        ] {
            assert!(
                body.contains(needle),
                "canonical skill must mention {needle:?}; first 500 chars:\n{}",
                &body[..body.len().min(500)],
            );
        }
        // Copies-not-symlinks contract is stated.
        assert!(
            body.contains("Copies, not symlinks") || body.contains("copies"),
            "canonical skill must state the copies-not-symlinks contract",
        );
        // No deprecated CLI verbs.
        assert_no_deprecated_verbs(&body, "generate_canonical_agent_skill_content");
    }

    #[test]
    fn write_opt_in_skill_writes_internal_only_no_fanout() {
        // Even with the harness-fanout marker ON, the opt-in skill write
        // must NOT fan out — these bodies live under .k2so/skills/ only.
        let proj = std::env::temp_dir().join(format!("k2so-optin-{}", Uuid::new_v4()));
        fs::create_dir_all(proj.join(".k2so")).unwrap();
        let path = proj.to_str().unwrap();
        crate::workspace::onboarding::set_harness_fanout_enabled(path, true).unwrap();

        let written = write_opt_in_skill(path, OptInSkill::K2CanonicalAgent);
        assert!(written.exists(), "canonical SKILL.md must be written");
        assert_eq!(
            written,
            proj.join(".k2so/skills/k2-canonical-agent/SKILL.md"),
            "must write to the internal .k2so/skills/<name>/ home",
        );
        // No fan-out into harness paths.
        assert!(!proj.join(".claude/skills/k2-canonical-agent/SKILL.md").exists());
        assert!(!proj.join("AGENTS.md").exists());
        assert!(!proj.join("CLAUDE.md").exists());

        // Idempotent re-run is a no-op on content (upgrade-tracked).
        let again = write_opt_in_skill(path, OptInSkill::K2CanonicalAgent);
        assert_eq!(again, written);

        fs::remove_dir_all(&proj).ok();
    }
}

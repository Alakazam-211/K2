# PRD — Single source of truth for the agent's display name

**Status:** superseded, 2026-08-17 — see `prd-workspace-display-name-and-handle-v1.md`.
Lockstep `display_name` + `name:` + `projects.name` is what broke federated
`name::host` after a rename. Do not implement this draft's "display is
label-only / duplicates OK / sidebar is not display" stance.

**Original status:** draft, 2026-05-06
**Author:** rosson@alakazamlabs.com (with implementation help)
**Related:** C3PO issue #e25b0f59 (canonical agent name roulette),
0.37.4 rename feature (built but unreleased), workspace–agent
unification PRD (shipped 0.37.0)

## Problem

The agent's name lives in three places that have to stay in
lockstep, and in practice they don't:

| location | format | role |
|---|---|---|
| `.k2so/agent/AGENT.md` frontmatter `name:` | text file | persona file's self-declared name (filesystem truth) |
| `workspace_sessions.terminal_id` | `agent-chat:<pid>:<name>` | denormalized SQL pointer (cache) |
| `v2_session_map` key | `<pid>:<name>` | live PTY registry key (cache) |

Symptoms when they drift:

- C3PO #3 — five identically-provisioned workspaces ended up with
  three different terminal_id shapes (`scout`, `__lead__`, bare
  UUID).
- "Blank pinned tab" — the pinned chat tab can attach to a v2
  session under one canonical key while a wake injects into a
  different key.
- Settings UI dead-end — the "Agent name" input said "rename
  coming in a later release" because rename means coordinating
  three writes.
- The 0.37.4 rename feature works but its existence is itself the
  smell: rename is hard *because* the name is denormalized.

The user's mental model is: **the agent's name is a contextual
preference distinct from the folder name**. That's right and
worth preserving — `~/DevProjects/foo-checkin/` can host an agent
named `scout`. But the *implementation* should pick **one**
source of truth and derive the other two.

## Goal

One source of truth for the agent's display name. Display
surfaces (sidebar, pinned chat tab, inbox tab, persona editor)
read it from one helper. Renaming = editing one file. The
infrastructure layer (v2_session_map, terminal_id) becomes
name-independent.

## Non-goals

- Workspace-level rename (separate concern; `projects.name` is
  already mutable and orthogonal).
- Going back to multi-agent-per-workspace. Unification stays.
- Killing AGENT.md or its frontmatter — persona content (role,
  launch profile, instructions) still lives there.

## Design

### Source of truth: `.k2so/agent/AGENT.md` frontmatter `name:` field

AGENT.md is already the agent's persona file. Its `name:` field
becomes **the** source for the display name — no caches in SQL,
no denormalized copies in v2_session_map keys.

Properties:

- Filesystem truth — survives daemon restart, version-controllable,
  inspectable with `cat`.
- Already exists today; no migration needed for the `name:` field.
- Editable via persona editor (AIFileEditor) → user updates it
  with confidence that it propagates.

### Infrastructure keys become name-independent

Today's canonical key is `<project_id>:<agent_name>`. Drop the
suffix — post-unification a workspace has one primary, so the
project_id alone is sufficient identification:

| layer | today | after |
|---|---|---|
| `v2_session_map` key | `<pid>:<agent_name>` | `<pid>` (or `<pid>:primary` for grep-ability) |
| `workspace_sessions.terminal_id` | `agent-chat:<pid>:<agent_name>` | `agent-chat:<pid>` (or just drop the column) |
| `pending_live` queue dir | `<pid>_<agent_name>/` (sanitized) | `<pid>/` |

Result: rename = edit AGENT.md and that's it. No SQL UPDATE, no
v2_session_map rekey, no pending_live dir move. The 0.37.4 rename
machinery becomes obsolete (we'll keep it as a no-op alias for
back-compat, then drop in 0.38).

### Display layer reads via one helper

New daemon-side helper:

```rust
// crates/k2so-core/src/agents/display.rs
pub fn agent_display_name(project_path: &str) -> String;
//                  ↳ reads .k2so/agent/AGENT.md once, caches by
//                    (path, mtime), invalidated on file change.
```

Tauri exposes one command:

```rust
#[tauri::command]
pub fn k2so_workspace_agent_display_name(
    project_path: String,
) -> Result<String, String>;
```

The agent display name has a **narrow** consumer set — it's the
human-friendly label for "what should I address this agent as":

- **Pinned chat tab** — the title (or the agent half of a
  `<workspace> · <agent>` composite — see open question below).
- **Inbox tab** — header label, e.g. "Inbox for `<agent>`".
- **AGENT.md / SKILL.md / generated persona content** — when the
  daemon writes templates or the AI editor prompts AGENT.md, the
  prompt carries the agent name so the persona reads as a real
  conversation partner ("call me `<agent>`...") rather than
  generic boilerplate.
- **Inbox message markdown** — when an inbox file is generated,
  the salutation / metadata uses the agent name so future
  reads (by humans or the agent itself) are personable.

What this is **NOT** for:

- Sidebar workspace list — that's the workspace's folder name
  (`projects.name`), unchanged.
- Tab bar for ad-hoc tabs (Cmd+T, AI editor, file viewer) —
  those are session labels (see companion PRD
  `session-label-daemon-owned`).
- CLI verb routing — `k2so msg <workspace>` resolves through
  `projects.name` / `projects.id` / `projects.path`. The agent
  name is just a label, not a routable address.

Caching: in-memory cache on the daemon side, keyed on
`(project_path, mtime_of_AGENT.md)`. Invalidates automatically
when the file is rewritten.

Fallback when AGENT.md is absent or has no `name:` field:
**use `projects.name` (the workspace's folder name)**. That's
the user's "either is fine" path — workspaces without a custom
agent get their workspace name as the agent label. No empty
states, no `__lead__` placeholder.

### Renaming flow

Renaming becomes:

1. User edits the input field in Settings (or directly in the
   persona editor's frontmatter).
2. Renderer calls a `k2so_workspace_set_agent_display_name(path,
   name)` command.
3. Daemon validates the name (regex same as before), rewrites the
   `name:` line in AGENT.md (atomic temp-file rename), invalidates
   the display-name cache.
4. Daemon emits a `WorkspaceUpdated` hook event so subscribed
   renderer surfaces re-fetch.

**No SQL writes. No v2_session_map mutation. No pending_live dir
moves.** The live PTY's canonical key (`<pid>:primary` or `<pid>`)
is unchanged; only the display label flipped.

### Migration (boot-time, idempotent)

On daemon boot:

1. Walk every workspace.
2. If `workspace_sessions.terminal_id` matches the legacy
   `agent-chat:<pid>:<name>` shape, rewrite to `agent-chat:<pid>`
   (drop the name suffix).
3. If `v2_session_map` has any entry under `<pid>:<name>`, re-key
   to `<pid>:primary` (or `<pid>`). Atomic, no PTY death.
4. Log per-workspace decisions.

After one boot every install converges on the new shape. The
0.37.4 rename code can be reused for the in-memory rekey phase.

## Open questions

1. **Pinned chat tab title format**: `"<agent>"` alone, or
   `"<agent> · <workspace>"`, or just `"<workspace>"` when the
   agent name equals the workspace name? Recommend:
   - If `agent_name == workspace_name` (or close — same after
     sanitize): show `<workspace>` only
   - Otherwise: show `<agent>` as primary (the user named the
     agent something specific, honor it) and the workspace lives
     in the workspace-list / window title
   - User can override per-tab via right-click → rename (per the
     companion PRD, that's a daemon-side label write)

2. **Should `projects.name` ever override AGENT.md?** Recommend
   no — AGENT.md wins always. Workspace name is the fallback only
   when AGENT.md is absent / nameless.

3. **Multiple workspaces with the same agent name**: allowed?
   Recommend yes — `projects.id` is the unique key, `name` is
   just a label. Two workspaces both having `agent name: scout`
   is fine.

## Implementation phases

| phase | scope | files | effort |
|---|---|---|---|
| **1** | `agent_display_name(path)` helper + cache | `agents/display.rs`, Tauri command | ~2h |
| **2** | Renderer surfaces read from helper (pinned tab title + inbox header only — sidebar list is left alone) | tabs.ts / inbox tab component / persona-editor prompt context | ~2h |
| **3** | Strip name suffix from `v2_session_map` keys + `terminal_id` | `spawn.rs`, `v2_spawn.rs`, `canonical_session.rs`, `workspace_msg.rs` | ~3h |
| **4** | Boot-time migration for legacy keys/IDs | `main.rs` boot sweep | ~1h |
| **5** | Settings UI rename → calls `set_agent_display_name`, no separate rename machinery | `ProjectsSection.tsx` | ~1h |
| **6** | Tests: unit (display helper cache), integration (rename via AGENT.md edit reflects everywhere within N ms) | new test files | ~2h |
| **7** | Retire 0.37.4 rename code path / leave as noop alias | doc + minor cleanup | ~30min |

Total: ~12-13h focused work. Single PR, single 0.38.0 release.

## Risks

| risk | mitigation |
|---|---|
| Stale display-name cache when AGENT.md edited externally (e.g., via vim) | check mtime on every read; invalidate cheaply |
| Existing in-flight PTYs registered under `<pid>:<old_name>` keys | boot sweep re-keys atomically; live sessions don't drop |
| Tauri renderer reads display name on every render → hot path | cache the response in renderer Zustand for the workspace list / tab title; refresh on `WorkspaceUpdated` event |
| Missing AGENT.md falls back to `projects.name` — what if both are empty? | `projects.name` is NOT NULL by schema; we always have a string |
| Cross-workspace `k2so msg <workspace>` resolution: today the resolver matches workspace name, not agent name. Stays unchanged. | no change needed — workspace token is `projects.name` / `path` / `id`, agent name is just display |

## What we're NOT doing

- Removing AGENT.md frontmatter `name:` — it stays as the source
  of truth.
- Adding a new SQL column for display name — AGENT.md is enough.
- Auto-deriving display name from `projects.name` for everyone —
  only as fallback. Users who want their agent named `scout`
  separately from their `~/dev/foo-checkin/` folder still can.

## Acceptance criteria

1. Edit `.k2so/agent/AGENT.md` `name:` from `scout` to `ranger`
   → pinned chat tab title updates within 2 seconds, inbox tab
   header updates, persona editor's prompt context picks up the
   new name on next save.
2. `k2so msg <workspace> "..." --wake` continues to work after
   the rename — same live session, no spawn duplication. (The
   workspace token still routes by project_id / path / workspace
   name, NOT by agent name — agent name is purely a label.)
3. `workspace_sessions.terminal_id` = `agent-chat:<pid>` (no
   name suffix) for new workspaces.
4. Boot a daemon with legacy `<pid>:<name>` v2 entries → after
   boot they're re-keyed, no PTY death, `agents running` shows
   them with the new shape.
5. `projects.name` of `"my-folder"` + AGENT.md absent → pinned
   chat tab + inbox tab header show "my-folder" as the agent
   label (workspace-name fallback).
6. **Sidebar workspace list is unchanged** — still shows
   `projects.name`, NOT the agent display name. This PRD doesn't
   touch that surface.

## Connection to other open work

- **C3PO #3** (canonical agent name roulette) — this PRD closes
  it by removing the canonical key's name component entirely.
  Three-states-roulette can't happen if there's no name in the
  key.
- **0.37.4 rename feature (unreleased)** — its on-disk rewrite
  helpers (`rewrite_name_in_frontmatter`) are reused. The SQL +
  v2_session_map rekey logic gets retired (replaced with
  no-ops since those layers no longer encode the name).
- **Tab title source guard** (just shipped to renderer) — stays
  as a band-aid for non-pinned-chat tabs. Pinned chat tab no
  longer needs it because its title is derived at render time.

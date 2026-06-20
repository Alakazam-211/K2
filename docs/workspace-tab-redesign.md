# Workspace Tab Redesign (v0.23.0)

## Overview

v0.23.0 consolidated 11+ agent/worktree UI surfaces into a single **Workspace** panel tab. This document explains what changed, why, and critical dev-mode performance considerations.

## What Changed

### Terminology
| Before (v0.22.x) | After (v0.23.x) |
|---|---|
| Pod | Coordinator |
| Pod Leader | Coordinator |
| Pod Member | Agent Template |
| `agentMode: 'pod'` | `agentMode: 'coordinator'` |
| `pod_leader: true` | `coordinator: true` |
| `type: pod-leader` | `type: coordinator` |
| `type: pod-member` | `type: agent-template` |

Backwards compatibility: the Rust backend accepts both old and new values. DB migration `0018` converts existing records. Frontmatter parsing checks both `pod_leader` and `coordinator` keys.

### UI Consolidation
| Before | After |
|---|---|
| Agents sidebar tab | Workspace panel tab |
| Review sidebar tab | Absorbed into Workspace tab |
| Worktree tree expansion in nav | Flat project list + Workspace panel |
| 4 separate agent modes in sidebar | Workspace tab with Status/Coordinator/Worktrees sections |

### Sidebar Simplification
- All projects render as `SingleProjectItem` regardless of `worktreeMode`
- No more `ProjectItem` (expandable worktree tree) in the nav
- Worktrees are managed via the Workspace panel instead
- "Open Full Workspace" promotes a worktree to a persistent nav entry with branch icon

### Worktree Detail View
Clicking a worktree in the Workspace panel opens a tab with:
- **Task** -- the delegated work item `.md` (hidden if no assigned task)
- **Chat** -- terminal session running in the worktree
- **Review** -- checklist + "AI Merge" button (greyed unless review state)

## Architecture

### Nav-Visible Worktrees (DB-Backed)

When a user clicks "Open Full Workspace", the worktree gets a `nav_visible = 1` flag in the `workspaces` DB table. This makes it appear as a persistent indented row beneath its project in the sidebar nav.

**DB Schema:** Migration `0019_workspace_nav_visible.sql` adds `nav_visible INTEGER NOT NULL DEFAULT 0` to the workspaces table.

**Tauri Command:** `workspace_set_nav_visible(id, visible)` updates the flag.

**Frontend:** `addNavWorktree()` and `removeNavWorktree()` in `Sidebar.tsx` use optimistic local store updates (no full refetch).

### Panel Tab Migration

The panels store (`panels.ts`) migrates saved user settings:
- `'agents'` tab -> `'workspace'`
- `'reviews'` tab -> `'workspace'`
- Deduplicates (if both existed, keeps one)
- Workspace tab appears on left panel only

## Dev Mode Performance

### The Problem

React's development mode enables **Strict Mode** which double-mounts every component. Combined with Tauri's async IPC (`listen()`, `invoke()`), this means:

1. Every workspace switch unmounts terminal components (cleanup runs)
2. React immediately re-mounts them (setup runs)
3. Then unmounts again (strict mode)
4. Then re-mounts again (final mount)
5. Each mount creates 3+ Tauri event listeners (`terminal:grid:*`, `terminal:exit:*`, `tauri://drag-drop`)
6. If any `listen()` call is in flight when cleanup runs, the listener leaks

After ~10-15 rapid workspace switches, accumulated leaked listeners clog the Tauri IPC channel, causing exponentially increasing latency (4ms -> 30ms -> 200ms -> 5000ms+).

**Production builds do NOT have this problem** because React production mode doesn't double-mount.

### Critical Rule: Never Call `fetchProjects()` in Render-Adjacent Code

The `fetchProjects()` function refreshes ALL projects from the database. Every `SingleProjectItem` in the sidebar subscribes to the projects store. Calling `fetchProjects()` triggers a re-render of EVERY sidebar item simultaneously.

In v0.23.0, `addNavWorktree()` originally called `fetchProjects()` to refresh the nav-visible state. This caused:
1. Full re-render of all sidebar items
2. Each item re-evaluates `useGitInfo`, `useMemo`, etc.
3. In dev mode with double-mounting, this cascades into 100s of re-renders

**Fix (v0.23.1):** Use optimistic local store patching instead:

```typescript
// BAD -- triggers global re-render cascade
export function addNavWorktree(worktreeId: string): void {
  invoke('workspace_set_nav_visible', { id: worktreeId, visible: true })
  useProjectsStore.getState().fetchProjects() // <-- THIS KILLS DEV MODE
}

// GOOD -- surgical update, no cascade
function patchWorkspaceNavVisible(worktreeId: string, visible: boolean): void {
  const state = useProjectsStore.getState()
  const updated = state.projects.map((p) => ({
    ...p,
    workspaces: p.workspaces.map((ws) =>
      ws.id === worktreeId ? { ...ws, navVisible: visible ? 1 : 0 } : ws
    ),
  }))
  useProjectsStore.setState({ projects: updated })
  invoke('workspace_set_nav_visible', { id: worktreeId, visible }).catch(() => {})
}
```

### Guidelines for Future Development

1. **Never call `fetchProjects()` in response to user interactions** -- use optimistic local updates instead
2. **Use `React.memo` for sidebar sub-components** -- prevents unnecessary re-renders when parent state changes
3. **Use stable Zustand selectors** -- avoid selectors that create new objects/arrays (use `useCallback` or primitive selectors)
4. **Avoid `useEffect` for synchronous state resets** -- the original `if (prevRef !== current)` pattern during render is actually better than `useEffect` for state resets that should happen immediately
5. **Test in production builds for performance** -- dev mode is 2-4x slower due to strict mode double-mounting. If something is slow in dev but fine in production, the issue is likely React strict mode amplification, not a real bug.
6. **IPC calls in `useEffect` cleanup are fire-and-forget** -- Tauri `invoke()` calls can't be cancelled. Use `mounted` flags to prevent applying stale results, but accept that the IPC call itself will complete.

### Known Dev-Mode Limitations

- The `FileTree` component has a `clearSelection()` call during render (setState during render) that React dev mode warns about. This is a pre-existing pattern that works correctly but generates console warnings.
- React strict mode is disabled in dev (v0.23.2+) to prevent double-mount listener leaks that caused IPC degradation after 10+ workspace switches.

### v0.24.x Updates

- **Workspace panel redesigned again** (v0.24.3): merged coordinator sections into single status row with Inbox/Active/Review counters and Launch button
- **Chat tab connects to live terminals**: delegate-launched terminals use deterministic IDs matching the Chat tab, enabling real-time grid updates
- **Panel tab cycling fix**: `initFromSettings` guard prevents `sync:settings` → `initFromSettings` → `settings_update` infinite loop
- **File tree uses worktree path**: when viewing a worktree workspace, the file tree shows worktree files, not main repo
- See [docs/agent-orchestration.md](agent-orchestration.md) for the full agent automation system documentation

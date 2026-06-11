// Host-workspace resolution for the chat-history panel.
//
// ChatHistory is mounted per-workspace by LeftPanelContent /
// RightPanelContent, which hand it the host workspace's path via the
// `projectPath` prop (the same `rootPath` FileTree already takes). We
// resolve the project + workspace FROM that path rather than from the
// global `activeProjectId`/`activeWorkspaceId` pointers, which track the
// globally-active workspace and can diverge from the workspace the panel
// is actually rendering inside (e.g. K2 is globally active with running
// heartbeats while the user opens HK47's history panel). See issue #7.
//
// This lives in its own module (no React / Tauri imports) so it's
// trivially unit-testable in the node vitest environment — importing the
// full ChatHistory.tsx would transitively load tabs.ts → Tauri `listen`,
// which throws `window is not defined` under node.

/** Minimal shape needed to resolve a host workspace from a path. Kept
 *  structural (not tied to the store types) so tests don't need the store. */
export interface ResolvableWorkspace {
  id: string
  branch: string | null
  worktreePath: string | null
}
export interface ResolvableProject {
  id: string
  path: string
  workspaces: ResolvableWorkspace[]
}

export interface ResolvedHost {
  project: ResolvableProject | null
  workspace: ResolvableWorkspace | null
  /** The path that should drive `chat/list` + `chat/storage-paths`. */
  projectPath: string | undefined
}

/**
 * Resolve the host project + workspace for the chat-history panel.
 *
 * When `hostProjectPath` is provided (the normal mount path), we bind to
 * the workspace whose `worktreePath` matches it, else the project whose
 * `path` matches it (the main, non-worktree workspace). This is what makes
 * the panel show the chats of the workspace it lives in, not whatever is
 * globally active.
 *
 * When `hostProjectPath` is absent (defensive fallback for any caller that
 * doesn't pass it), we fall back to the global active pointers so behavior
 * is identical to the pre-#7 component for that case.
 */
export function resolveChatHistoryHost(
  projects: ResolvableProject[],
  hostProjectPath: string | undefined,
  activeProjectId: string | null,
  activeWorkspaceId: string | null,
): ResolvedHost {
  if (hostProjectPath) {
    // Prefer an exact worktree match (a real worktree workspace).
    for (const project of projects) {
      const ws = project.workspaces.find((w) => w.worktreePath === hostProjectPath)
      if (ws) return { project, workspace: ws, projectPath: hostProjectPath }
    }
    // Else the main workspace of the project whose path equals the host.
    const byPath = projects.find((p) => p.path === hostProjectPath)
    if (byPath) {
      // The main workspace has worktreePath === null; pick it if present so
      // cross-worktree fork logic still sees the right branch.
      const mainWs = byPath.workspaces.find((w) => w.worktreePath == null) ?? null
      return { project: byPath, workspace: mainWs, projectPath: hostProjectPath }
    }
    // Host path given but no matching project loaded yet — still scope the
    // daemon calls to that path (daemon filtering is project-family aware).
    return { project: null, workspace: null, projectPath: hostProjectPath }
  }

  // Fallback: global pointers (legacy behavior, no host binding available).
  const project = projects.find((p) => p.id === activeProjectId) ?? null
  const workspace = project?.workspaces.find((w) => w.id === activeWorkspaceId) ?? null
  const projectPath = workspace?.worktreePath ?? project?.path ?? undefined
  return { project, workspace, projectPath }
}

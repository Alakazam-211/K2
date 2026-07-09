// Pure wake / live-attach helpers for client-watching surfaces that
// surface a member workspace's canonical (or feedback) session.
//
// PRD §4.3.1 open/attach ⇒ activate: without POST projects/activate the
// daemon active_reaper treats the workspace as non-Active and force-
// closes the chat PTY after ~15s grace (Active-only reaping —
// subscribers do not spare). ensure-pinned-chat alone is not enough.

export type WakeCanonicalDeps = {
  /** POST projects/activate for the member workspace id (deduped upstream). */
  activateProject: (workspaceId: string) => void
  /** POST workspace/ensure-pinned-chat { project: path }. */
  ensurePinnedChat: (projectPath: string) => Promise<unknown>
}

/**
 * Wake a dormant canonical session for a client-watching surface.
 *
 * Activates the **member workspace id** (same id as attachAgentName /
 * lookup-by-agent — NOT a project-group id) **before** ensure-pinned-chat
 * so Active is set before the reaper can arm on the spawned/found PTY.
 */
export async function wakeCanonicalMemberSession(
  workspaceId: string,
  projectPath: string,
  deps: WakeCanonicalDeps,
): Promise<void> {
  // PRD §4.3.1 open/attach ⇒ activate; without it active_reaper reaps after 15s.
  deps.activateProject(workspaceId)
  await deps.ensurePinnedChat(projectPath)
}

/**
 * Passive attach of an already-alive session has the same Active hole as
 * wake: client is watching ⇒ mark Active. Safe / no-op when already active
 * (activateProject dedups).
 */
export function activateOnLiveSessionAttach(
  workspaceId: string,
  activateProject: (workspaceId: string) => void,
): void {
  // PRD §4.3.1 open/attach ⇒ activate; without it active_reaper reaps after 15s.
  activateProject(workspaceId)
}

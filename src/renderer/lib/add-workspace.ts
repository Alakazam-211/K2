// The "Add workspace" confirm step, shared by the Agents sidebar and the
// icon rail (0.46.10).
//
// The add is the daemon's `projects/add-from-path` (the projects store shows
// and selects the result). The ingest after it writes the workspace skill
// right away so the person sees it without a restart. It is BEST-EFFORT: the
// daemon already has the workspace, and a lost ingest request (WebKit gives
// "Load failed" for a dropped connection, often logged as a CORS failure)
// must never abort the add, hide the new workspace or wait the dialog.

import { daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

/** POST `agents/run-workspace-ingest` for `projectPath`; a failure is a
 *  warning, never a throw. */
export async function runWorkspaceIngestBestEffort(projectPath: string): Promise<void> {
  try {
    await daemonCliPost(primaryScope(), 'agents/run-workspace-ingest', { project_path: projectPath })
  } catch (err) {
    console.warn('[add-workspace] run ingest failed (best-effort; the workspace is added):', err)
  }
}

/** Add `folderPath` as a workspace, then start its ingest without waiting
 *  for it. Resolves to the added workspace, or `null` when the folder needs
 *  git first (the git-init dialog runs the ingest once it adds). A failed
 *  ADD still throws (the store has toasted it). */
export async function addWorkspaceWithIngest(
  folderPath: string,
  opts: { seedWiki?: boolean; seedAgentsMd?: boolean; fanout?: boolean },
): Promise<ProjectWithWorkspaces | null> {
  const added = await useProjectsStore.getState().addProject(folderPath, opts)
  if (added) void runWorkspaceIngestBestEffort(added.path || folderPath)
  return added
}

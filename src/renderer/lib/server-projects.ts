// Home M4 — one server's project list, read on that server's scope
// (prd-home-multi-server-client MS3, MS14 "projects: per server").
//
// A pinned room needs ITS server's `projects/list` to resolve its Home row
// (handle, then the stored id) and to answer every path lookup inside the
// room. This is the same read the window's projects store makes, without the
// store: newer daemons embed `workspaces` on every row; an older daemon gets
// one `workspaces/list` per project, as `stores/projects.ts` does.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'

type Workspace = ProjectWithWorkspaces['workspaces'][number]
type Project = Omit<ProjectWithWorkspaces, 'workspaces'>

/** `projects/list` (+ workspaces) from `scope`'s server. Throws on a
 *  failed request — the room shows its failure state, never A's list. */
export async function fetchServerProjects(scope: ServerScope): Promise<ProjectWithWorkspaces[]> {
  const raw = await daemonCliGet<Array<Project & { workspaces?: unknown }>>(scope, 'projects/list')
  if (!Array.isArray(raw)) {
    throw new Error(`projects/list on ${scope.label} did not return a list`)
  }
  if (raw.every((row) => row != null && Array.isArray(row.workspaces))) {
    return raw as ProjectWithWorkspaces[]
  }
  return Promise.all(
    raw.map(async (project) => {
      const ws = await daemonCliGet<Workspace[]>(scope, 'workspaces/list', { project_id: project.id })
      return { ...project, workspaces: Array.isArray(ws) ? ws : [] }
    }),
  )
}

/** The workspace a pinned room opens for a project: its first by tab
 *  order (the one the window's projects store selects for it). */
export function primaryWorkspaceOf(project: ProjectWithWorkspaces): Workspace | null {
  const sorted = [...(project.workspaces ?? [])].sort((a, b) => a.tabOrder - b.tabOrder)
  return sorted[0] ?? null
}

import { lazy, Suspense, useCallback, type JSX } from 'react'
import { useRoomProjects } from '@/components/Room/RoomContext'

// As the header picture: the bell's write pulls in the projects store.
const WorkspaceCompletionSoundBell = lazy(() =>
  import('@/components/WorkspacePanel/WorkspaceCompletionSoundToggle').then((m) => ({
    default: m.WorkspaceCompletionSoundBell,
  })),
)

/** Thread view's bell: the Workspace drawer's completion bell, same setting and
 *  behaviour, as a floating object (on split view's bar, a plain bar button).
 *  Found by the pinned chat's project id, or a tab's cwd (a worktree's path
 *  counts). */
export function ThreadFloatBell({
  projectId,
  cwd,
  floating = true,
}: {
  projectId?: string
  cwd?: string
  floating?: boolean
}): JSX.Element | null {
  const project = useRoomProjects(
    useCallback(
      (projects) =>
        projects.find((p) =>
          projectId ? p.id === projectId : p.path === cwd || p.workspaces?.some((w) => w.worktreePath === cwd),
        ) ?? null,
      [projectId, cwd],
    ),
  )
  if (!project) return null
  return (
    <Suspense fallback={null}>
      <WorkspaceCompletionSoundBell project={project} floating={floating} />
    </Suspense>
  )
}

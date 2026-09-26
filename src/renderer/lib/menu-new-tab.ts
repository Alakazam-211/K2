import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'

/** cwd File-menu actions use: active workspace, else project path, else '~'. */
export function workspaceCwdForMenu(): string {
  const ps = useProjectsStore.getState()
  const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
  const ws = proj?.workspaces?.find((w) => w.id === ps.activeWorkspaceId)
  return ws?.worktreePath ?? proj?.path ?? '~'
}

/** File → New Tab. Direct terminal create. Does not open the tab-bar plus menu. */
export function menuNewTab(): void {
  useTabsStore.getState().addTab(workspaceCwdForMenu())
}

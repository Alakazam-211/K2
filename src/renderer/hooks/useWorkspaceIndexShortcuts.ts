import { useEffect } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { getActiveBarItems } from '@/components/Sidebar/ActiveBar'
import { openHomeRow } from '@/lib/home-open'
import { homeRowOpenableNow } from '@/components/Home/home-room'

/**
 * Cmd+1–9 / Cmd+0 and Cmd+Option+1–9 — pick a workspace (or a Home row) by
 * index. ONE window-level handler (Home M3, MS18): these chords choose
 * which room to show, so they are never sent into a room, and two mounted
 * rooms must never both act.
 *
 * - Agents: unchanged. Cmd+N / Cmd+Option+N switch to the Nth pinned or
 *   active workspace (per Settings → shortcut layout).
 * - Home: Cmd+1–9 (and Cmd+0 for the tenth) selects Home row N, the same
 *   as clicking it (answer Q5). A row that cannot open (no access, gone,
 *   another server on web) does nothing, as its click does.
 */
export function useWorkspaceIndexShortcuts(): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      // Cmd+Option+1-9 (e.code: Option modifies e.key on macOS).
      if (e.metaKey && e.altKey && !e.shiftKey && !e.ctrlKey) {
        const digitMatch = e.code.match(/^Digit(\d)$/)
        const num = digitMatch ? parseInt(digitMatch[1], 10) : NaN
        if (!isNaN(num) && num >= 1 && num <= 9) {
          e.preventDefault()
          const layout = useTerminalSettingsStore.getState().shortcutLayout
          if (layout === 'cmd-active-cmdshift-pinned') {
            switchToPinnedByIndex(num - 1)
          } else {
            switchToActiveByIndex(num - 1)
          }
        }
        return
      }

      if (!e.metaKey || e.shiftKey || e.altKey) return
      const num = parseInt(e.key, 10)
      if (isNaN(num) || e.key.length !== 1) return
      e.preventDefault()
      const targetIdx = num === 0 ? 9 : num - 1

      if (usePageViewStore.getState().page === 'home') {
        const row = selectedHome(useHomesStore.getState()).rows[targetIdx]
        if (row && homeRowOpenableNow(row)) openHomeRow(row)
        return
      }

      const layout = useTerminalSettingsStore.getState().shortcutLayout
      if (layout === 'cmd-active-cmdshift-pinned') {
        switchToActiveByIndex(targetIdx)
      } else {
        switchToPinnedByIndex(targetIdx)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [])
}

function switchToPinnedByIndex(targetIdx: number): void {
  const projectsState = useProjectsStore.getState()

  // Agent workspaces (top of sidebar) + pinned workspaces
  const agentProjects = projectsState.projects.filter(
    (p) => p.agentMode === 'agent' || p.agentMode === 'custom',
  )
  const pinnedProjects = projectsState.projects.filter(
    (p) => p.pinned && p.agentMode !== 'agent' && p.agentMode !== 'custom'
  )
  const topProjects = [...agentProjects, ...pinnedProjects]

  // Build flat list of all workspaces across top-section projects
  let flatIdx = 0
  for (const project of topProjects) {
    const workspaces = project.worktreeMode === 1 && project.workspaces.length > 0
      ? project.workspaces
      : project.workspaces.slice(0, 1)

    for (const ws of workspaces) {
      if (flatIdx === targetIdx) {
        projectsState.setActiveWorkspace(project.id, ws.id)
        return
      }
      flatIdx++
    }
  }
}

function switchToActiveByIndex(targetIdx: number): void {
  const activeItems = getActiveBarItems()

  if (targetIdx < activeItems.length) {
    const project = activeItems[targetIdx]
    const firstWorkspace = project.workspaces[0]
    if (firstWorkspace) {
      const focusState = useFocusGroupsStore.getState()
      if (focusState.focusGroupsEnabled && project.focusGroupId !== focusState.activeFocusGroupId) {
        // autoActivate: false — the shortcut's target is activated
        // explicitly below (double-switch race otherwise).
        focusState.setActiveFocusGroup(project.focusGroupId, { autoActivate: false })
      }
      useProjectsStore.getState().setActiveWorkspace(project.id, firstWorkspace.id)
    }
  }
}

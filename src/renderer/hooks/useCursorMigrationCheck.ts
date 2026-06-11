import { useEffect, useRef } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'

/**
 * Checks if the active project has unmigrated Cursor IDE conversations.
 * Shows a toast notification once per project per session if found.
 * Clicking the toast navigates to the workspace settings page.
 */
export function useCursorMigrationCheck(): void {
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projects = useProjectsStore((s) => s.projects)
  const checkedProjects = useRef<Set<string>>(new Set())

  useEffect(() => {
    if (!activeProjectId) return
    if (checkedProjects.current.has(activeProjectId)) return

    const activeProject = projects.find((p) => p.id === activeProjectId)
    if (!activeProject?.path) return

    checkedProjects.current.add(activeProjectId)

    // Delay check slightly so it doesn't compete with project load
    const timer = setTimeout(async () => {
      try {
        const sessions = await daemonCliGet<any[]>('chat/discover-ide', {
          project_path: activeProject.path,
        })
        const unmigrated = sessions.filter((s: any) => !s.alreadyMigrated && s.migratable)
        if (unmigrated.length > 0) {
          useToastStore.getState().addToast(
            `${unmigrated.length} Cursor conversation${unmigrated.length !== 1 ? 's' : ''} found for ${activeProject.name}`,
            'info',
            8000,
            {
              label: 'Migrate',
              onClick: () => {
                useSettingsStore.getState().openSettings('projects', activeProject.id)
              },
            },
          )
        }
      } catch {
        // Silently ignore — Cursor may not be installed
      }
    }, 3000)

    return () => clearTimeout(timer)
  }, [activeProjectId, projects])
}

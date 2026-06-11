import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'
import { usePresetsStore } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { getActiveBarItems } from '@/components/Sidebar/ActiveBar'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import type { TerminalPane } from '@/stores/tabs'

/**
 * Registers global keyboard shortcuts for terminal tab/pane management.
 *
 * - Cmd+T         — New tab
 * - Cmd+W         — Close active tab
 * - Cmd+D         — Split pane vertically
 * - Cmd+Shift+D   — Split pane horizontally
 * - Cmd+Alt+Left  — Previous tab
 * - Cmd+Alt+Right — Next tab
 * - Cmd+1-9       — Switch to workspace by index
 * - Cmd+K         — Clear active terminal (sends clear sequence)
 * - Ctrl+1-9      — Launch preset by position
 */
export function useTerminalShortcuts(cwd: string): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      // Ctrl+1-9: launch preset by position
      if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
        const num = parseInt(e.key, 10)
        if (num >= 1 && num <= 9) {
          e.preventDefault()
          const presetsState = usePresetsStore.getState()
          const enabledPresets = presetsState.presets.filter((p) => p.enabled)
          const targetIdx = num - 1
          if (targetIdx < enabledPresets.length) {
            presetsState.launchPreset(enabledPresets[targetIdx].id, cwd, 'tab')
          }
          return
        }
      }

      // Cmd+Option+1-9: switch to pinned or active workspace (depends on layout)
      // (Cmd+Shift conflicts with macOS screenshots, Option+Shift produces UTF-8 chars)
      // Use e.code (Digit1-Digit9) instead of e.key because Option modifies key values on macOS
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
          return
        }
      }

      // Only handle Cmd (Meta) shortcuts
      if (!e.metaKey) return

      const state = useTabsStore.getState()

      switch (e.key) {
        case 't': {
          if (e.altKey) return
          e.preventDefault()
          if (e.shiftKey) {
            // Cmd+Shift+T: Launch default agent in new tab
            const presetsState = usePresetsStore.getState()
            const defaultAgent = useSettingsStore.getState().defaultAgent
            // Match the default agent setting to a preset by command name
            const defaultPreset = defaultAgent
              ? presetsState.presets.find((p) => p.command.split(/\s+/)[0] === defaultAgent && p.enabled)
              : null
            // Fall back to first enabled preset if default not found
            const preset = defaultPreset || presetsState.presets.find((p) => p.enabled)
            if (preset) {
              presetsState.launchPreset(preset.id, cwd, 'tab')
            }
          } else {
            // Cmd+T: New blank tab
            state.addTab(cwd)
          }
          break
        }

        case 'w': {
          if (e.shiftKey || e.altKey) return
          e.preventDefault()
          if (state.activeTabId) {
            state.removeTab(state.activeTabId)
          }
          break
        }

        case 'd': {
          // Let CodeMirror handle Cmd+D (select next occurrence) when focused in editor
          if (document.activeElement?.closest('.cm-editor')) return
          e.preventDefault()
          const activeTab = state.tabs.find((t) => t.id === state.activeTabId)
          if (!activeTab) return

          const firstPaneId = getFirstLeaf(activeTab.mosaicTree)
          if (!firstPaneId) return

          const newPaneId = crypto.randomUUID()
          const newPane: TerminalPane = { type: 'terminal', terminalId: newPaneId, cwd }
          const direction = e.shiftKey ? 'row' : 'column'

          state.splitPane(activeTab.id, firstPaneId, newPaneId, newPane, direction)
          break
        }

        case 'ArrowLeft': {
          if (!e.altKey) return
          e.preventDefault()
          const currentIdx = state.tabs.findIndex((t) => t.id === state.activeTabId)
          if (currentIdx > 0) {
            state.setActiveTab(state.tabs[currentIdx - 1].id)
          }
          break
        }

        case 'ArrowRight': {
          if (!e.altKey) return
          e.preventDefault()
          const curIdx = state.tabs.findIndex((t) => t.id === state.activeTabId)
          if (curIdx < state.tabs.length - 1) {
            state.setActiveTab(state.tabs[curIdx + 1].id)
          }
          break
        }

        case 'n': {
          if (e.shiftKey || e.altKey) return
          e.preventDefault()
          state.openUntitledDocument(cwd)
          break
        }

        case 'o': {
          if (e.shiftKey || e.altKey) return
          e.preventDefault()
          pickWorkspaceFolder().then((folderPath) => {
            if (folderPath) {
              useProjectsStore.getState().addProject(folderPath)
            }
          })
          break
        }

        case 'f': {
          // Cmd+Shift+F: Open current workspace in focus window
          if (!e.shiftKey || e.altKey) return
          e.preventDefault()
          const projectsState = useProjectsStore.getState()
          const activeProjectId = projectsState.activeProjectId
          if (activeProjectId) {
            invoke('projects_open_focus_window', { projectId: activeProjectId }).catch((e) => console.warn('[shortcuts]', e))
          }
          break
        }

        case 'k': {
          if (e.shiftKey || e.altKey) return
          // Cmd+K: let the terminal handle clear — we don't intercept this
          // The terminal component forwards keystrokes to the pty
          break
        }

        default: {
          // Cmd+1-9/0 — switch to active or pinned workspace (depends on layout)
          const num = parseInt(e.key, 10)
          if (!isNaN(num) && !e.shiftKey && !e.altKey) {
            e.preventDefault()
            const layout = useTerminalSettingsStore.getState().shortcutLayout
            const targetIdx = num === 0 ? 9 : num - 1
            if (layout === 'cmd-active-cmdshift-pinned') {
              switchToActiveByIndex(targetIdx)
            } else {
              switchToPinnedByIndex(targetIdx)
            }
          }
          break
        }
      }
    }

    window.addEventListener('keydown', handler)
    return () => {
      window.removeEventListener('keydown', handler)
    }
  }, [cwd])
}

// ── Shortcut helpers ─────────────────────────────────────────────────────

function switchToPinnedByIndex(targetIdx: number): void {
  const projectsState = useProjectsStore.getState()
  const agenticEnabled = useSettingsStore.getState().agenticSystemsEnabled

  // Agent workspaces (top of sidebar) + pinned workspaces
  const agentProjects = agenticEnabled
    ? projectsState.projects.filter((p) => p.agentMode === 'agent' || p.agentMode === 'custom')
    : []
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

// ── Helpers ──────────────────────────────────────────────────────────────

function getFirstLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getFirstLeaf((tree as { first: unknown }).first)
  }
  return null
}

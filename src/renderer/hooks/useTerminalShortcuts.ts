import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { usePresetsStore } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { pickWorkspaceFolder } from '@/lib/pick-workspace-folder'
import { resolveAgentPreset, readProjectDefaultAgent } from '@/lib/agent-resolve'
import { isFocusedRoom } from '@/stores/window-room'
import { roomActiveProject, type Room } from '@/stores/room'
import type { TerminalPane } from '@/stores/tabs'

/**
 * Registers keyboard shortcuts for one room's tab/pane management.
 *
 * Every mounted terminal area installs this listener, so EVERY handler
 * returns early unless its room is the window's focused room (Home M3,
 * MS18): with two rooms mounted, Cmd+T opens one tab, in the focused room.
 * The workspace-index chords (Cmd+1–9, Cmd+Option+1–9) pick which room to
 * show, so they live in one window-level hook (`useWorkspaceIndexShortcuts`).
 *
 * Single keyboard owner for these chords — native menu accelerators for
 * the same keys MUST stay unbound in menu.rs (duplicate menu+keydown
 * fire = multi-spawn / multi-note bugs). Menu clicks still emit
 * menu:new-document / menu:new-tab / … and App.tsx handles those.
 *
 * - Cmd+N         — New untitled document (note)
 * - Cmd+T         — New tab
 * - Cmd+Shift+T   — Launch default agent
 * - Cmd+W         — Close active tab
 * - Cmd+D         — Split pane vertically
 * - Cmd+Shift+D   — Split pane horizontally
 * - Cmd+O         — Open workspace
 * - Cmd+Alt+Left  — Previous tab
 * - Cmd+Alt+Right — Next tab
 * - Cmd+K         — Clear active terminal (sends clear sequence)
 * - Ctrl+1-9      — Launch preset by position
 */
export function useTerminalShortcuts(room: Room, cwd: string): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (!isFocusedRoom(room)) return
      const tabs = room.tabs

      // Ctrl+1-9: launch preset by position
      if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
        const num = parseInt(e.key, 10)
        if (num >= 1 && num <= 9) {
          e.preventDefault()
          const presetsState = usePresetsStore.getState()
          const enabledPresets = presetsState.presets.filter((p) => p.enabled)
          const targetIdx = num - 1
          if (targetIdx < enabledPresets.length) {
            presetsState.launchPreset(tabs, enabledPresets[targetIdx].id, cwd, 'tab')
          }
          return
        }
      }

      // Only handle Cmd (Meta) shortcuts
      if (!e.metaKey) return

      const state = tabs.getState()
      // Shift reports uppercase letters on some platforms ('T' not 't').
      const key = e.key.length === 1 ? e.key.toLowerCase() : e.key

      switch (key) {
        case 't': {
          if (e.altKey) return
          e.preventDefault()
          if (e.shiftKey) {
            // Cmd+Shift+T: Launch default agent in new tab — resolution goes
            // through the one seam (workspace default → global → first
            // enabled; id-first, legacy-token tolerant). The workspace
            // default is read from THIS room's project record (MS3).
            const presetsState = usePresetsStore.getState()
            const preset = resolveAgentPreset(
              presetsState.presets,
              useSettingsStore.getState().defaultAgent,
              readProjectDefaultAgent(roomActiveProject(room) ?? undefined),
            )
            if (preset) {
              presetsState.launchPreset(tabs, preset.id, cwd, 'tab')
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
          // Shift stays. Cmd+Shift+N is New Window (useNewWindowShortcut),
          // not a document. Deleting this return makes Cmd+Shift+N call
          // openUntitledDocument. Ctrl+Shift+N never reaches this switch
          // (`if (!e.metaKey) return` above).
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
          // Cmd+Shift+F: Open current workspace in focus window. The focus
          // window looks the project up on THIS computer's daemon and has no
          // host in its label (MS57), so only a room that may use local
          // commands offers it.
          if (!e.shiftKey || e.altKey) return
          e.preventDefault()
          if (!room.localCommands) return
          const activeProjectId = room.activeProjectId()
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
      }
    }

    window.addEventListener('keydown', handler)
    return () => {
      window.removeEventListener('keydown', handler)
    }
  }, [room, cwd])
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

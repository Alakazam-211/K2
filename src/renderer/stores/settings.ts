import { create } from 'zustand'
import { getDefaultKeybindings } from '@shared/hotkeys'
import type { AppSettingsResponse, EditorSettingsBackend } from '@shared/types'
// Phase 2 Unit 7a — settings live in the daemon; renderer hits
// `/cli/settings/{get,update,reset}` directly. The Tauri `settings_*`
// proxy commands still exist (back-compat for any caller not yet
// migrated) but the renderer prefers the daemon-direct path.
import {
  settingsGet,
  settingsUpdate,
  settingsReset,
} from '@/lib/daemon-settings'
// #625 — re-fetch settings against the NEW host on a host switch so the
// client is a pure view of the active host's daemon.
import { onActiveHostChange } from '@/stores/connect-host'

export type SettingsSection = 'general' | 'terminal' | 'code-editor' | 'editors-agents' | 'keybindings' | 'projects' | 'timer' | 'workspace-states' | 'agent-skills' | 'heartbeats' | 'companion' | 'wake-scheduler' | 'permissions' | 'dictation-lab' | 'connections' | 'k2-connect'

export interface TerminalSettings {
  fontFamily: string
  fontSize: number
  cursorStyle: 'bar' | 'block' | 'underline'
  scrollback: number
  naturalTextEditing: boolean
}

interface SettingsState {
  settingsOpen: boolean
  activeSection: SettingsSection

  // Terminal settings
  terminal: TerminalSettings

  // Keybindings: id -> key combo (overrides only; empty = use defaults)
  keybindings: Record<string, string>

  // Per-project settings
  projectSettings: Record<string, Record<string, any>>

  // AI Assistant
  aiAssistantEnabled: boolean

  // Master switch for the entire agent system (agents, coordinator, heartbeat, reviews)
  agenticSystemsEnabled: boolean

  // Claude Auth auto-refresh (background scheduler)
  claudeAuthAutoRefresh: boolean

  // P1.C — how long (hours) a workspace stays in the Active Bar after the
  // user last interacted with it (Active-Bar rule 2). Read by ActiveBar
  // (and a future reaper). Default 24, min 1.
  activeWindowHours: number

  // Editor settings
  editor: EditorSettingsBackend

  // Default agent CLI tool (e.g. 'claude', 'codex', 'gemini')
  defaultAgent: string

  // Pre-select a specific project in the projects section
  initialProjectId: string | null

  // 0.39.0: last-active project/workspace IDs cached here so projects.ts
  // doesn't re-invoke settings_get on hydration. Source of truth is the
  // daemon's settings.json — the persist path still goes through
  // settings_update (see useProjectsStore.setActiveProject).
  lastActiveProjectId: string | null
  lastActiveWorkspaceId: string | null

  // Loading state
  loaded: boolean

  // When true, GeneralSection should auto-trigger an update check on mount
  pendingUpdateCheck: boolean

  // Actions
  openSettings: (section?: SettingsSection, projectId?: string) => void
  closeSettings: () => void
  setSection: (section: SettingsSection) => void
  updateTerminalSettings: (partial: Partial<TerminalSettings>) => void
  updateKeybinding: (id: string, combo: string) => void
  resetKeybinding: (id: string) => void
  resetAllKeybindings: () => void
  updateProjectSetting: (projectId: string, key: string, value: string) => void
  setAiAssistantEnabled: (enabled: boolean) => void
  setClaudeAuthAutoRefresh: (enabled: boolean) => void
  setActiveWindowHours: (hours: number) => void
  updateEditorSettings: (partial: Partial<EditorSettingsBackend>) => void
  setDefaultAgent: (agent: string) => void
  resetAllSettings: () => void
  fetchSettings: () => Promise<void>
}

/** P1.C — Active-Bar tenure window default (hours) and floor. Mirrors the
 *  daemon's `default_active_window_hours()` (24) and the min-1 UI clamp. */
export const DEFAULT_ACTIVE_WINDOW_HOURS = 24
export function clampActiveWindowHours(h: number): number {
  if (!Number.isFinite(h)) return DEFAULT_ACTIVE_WINDOW_HOURS
  return Math.max(1, Math.floor(h))
}

const DEFAULT_TERMINAL: TerminalSettings = {
  fontFamily: 'MesloLGM Nerd Font',
  fontSize: 13,
  cursorStyle: 'bar',
  scrollback: 5000,
  naturalTextEditing: true
}

/**
 * Monotonic write counter. Incremented each time we write settings.
 * fetchSettings() captures this before its async call and skips applying
 * the result if another write happened in the meantime (the write's result
 * is more authoritative than a stale read).
 */
let _writeSeq = 0

/**
 * Helper: sends a partial update to the backend, which deep-merges it
 * with the current settings on disk and returns the canonical result.
 * We apply the returned state to the store, ensuring we stay in sync
 * with what was actually persisted — no extra fetchSettings round-trip.
 */
const DEFAULT_EDITOR: EditorSettingsBackend = {
  tabSize: 2, wordWrap: false, showWhitespace: false, fontSize: 12,
  indentGuides: true, foldGutter: true, autocomplete: true,
  bracketMatching: true, lineNumbers: true, highlightActiveLine: true,
  stickyScroll: false, minimap: false,
  theme: 'k2so-dark', fontFamily: 'MesloLGM Nerd Font', fontLigatures: false,
  cursorStyle: 'bar', cursorBlink: true,
  scrollPastEnd: false, scrollbarAnnotations: true, diffStyle: 'gutter',
  formatOnSave: false, vimMode: false,
}

/** Merge backend editor result with defaults so old settings files don't leave fields undefined */
function mergeEditorDefaults(result: Partial<EditorSettingsBackend> | undefined): EditorSettingsBackend {
  if (!result) return { ...DEFAULT_EDITOR }
  return { ...DEFAULT_EDITOR, ...result }
}

async function persistAndApply(
  set: (state: Partial<SettingsState>) => void,
  updates: Record<string, any>
): Promise<void> {
  _writeSeq++
  try {
    const result = await settingsUpdate(updates)
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? false,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      activeWindowHours: clampActiveWindowHours(result.activeWindowHours ?? DEFAULT_ACTIVE_WINDOW_HOURS),
      editor: mergeEditorDefaults(result.editor),
      lastActiveProjectId: result.lastActiveProjectId ?? null,
      lastActiveWorkspaceId: result.lastActiveWorkspaceId ?? null,
      loaded: true
    })
  } catch (e) {
    console.warn('[settings] persistAndApply failed:', e)
  }
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settingsOpen: false,
  activeSection: 'general',
  terminal: { ...DEFAULT_TERMINAL },
  keybindings: {},
  projectSettings: {},
  aiAssistantEnabled: true,
  agenticSystemsEnabled: false,
  claudeAuthAutoRefresh: false,
  activeWindowHours: DEFAULT_ACTIVE_WINDOW_HOURS,
  editor: { ...DEFAULT_EDITOR },
  defaultAgent: 'claude',
  initialProjectId: null,
  lastActiveProjectId: null,
  lastActiveWorkspaceId: null,
  loaded: false,
  pendingUpdateCheck: false,

  openSettings: (section?: SettingsSection, projectId?: string) => {
    set({
      settingsOpen: true,
      activeSection: section ?? get().activeSection,
      initialProjectId: projectId ?? null
    })
  },

  closeSettings: () => {
    set({ settingsOpen: false })
  },

  setSection: (section: SettingsSection) => {
    set({ activeSection: section })
  },

  updateTerminalSettings: async (partial: Partial<TerminalSettings>) => {
    const prev = get().terminal
    const newTerminal = { ...prev, ...partial }
    set({ terminal: newTerminal })
    try {
      await persistAndApply(set, { terminal: newTerminal })
    } catch (err) {
      console.error('[settings] Failed to persist terminal settings:', err)
      set({ terminal: prev })
    }
  },

  updateKeybinding: async (id: string, combo: string) => {
    const prev = get().keybindings
    const keybindings = { ...prev, [id]: combo }
    set({ keybindings })
    try {
      await persistAndApply(set, { keybindings })
    } catch (err) {
      console.error('[settings] Failed to persist keybinding:', err)
      set({ keybindings: prev })
    }
  },

  resetKeybinding: async (id: string) => {
    const prev = get().keybindings
    const keybindings = { ...prev }
    delete keybindings[id]
    set({ keybindings })
    try {
      await persistAndApply(set, { keybindings })
    } catch (err) {
      console.error('[settings] Failed to persist keybinding reset:', err)
      set({ keybindings: prev })
    }
  },

  resetAllKeybindings: async () => {
    const prev = get().keybindings
    set({ keybindings: {} })
    try {
      await persistAndApply(set, { keybindings: {} })
    } catch (err) {
      console.error('[settings] Failed to persist keybindings reset:', err)
      set({ keybindings: prev })
    }
  },

  updateProjectSetting: async (projectId: string, key: string, value: string) => {
    const prev = get().projectSettings
    const projectSettings = {
      ...prev,
      [projectId]: { ...prev[projectId], [key]: value }
    }
    set({ projectSettings })
    try {
      await persistAndApply(set, { projectSettings })
    } catch (err) {
      console.error('[settings] Failed to persist project setting:', err)
      set({ projectSettings: prev })
    }
  },

  setAiAssistantEnabled: async (enabled: boolean) => {
    const prev = get().aiAssistantEnabled
    set({ aiAssistantEnabled: enabled })
    try {
      await persistAndApply(set, { aiAssistantEnabled: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist AI assistant setting:', err)
      set({ aiAssistantEnabled: prev })
    }
  },

  setClaudeAuthAutoRefresh: async (enabled: boolean) => {
    const prev = get().claudeAuthAutoRefresh
    set({ claudeAuthAutoRefresh: enabled })
    try {
      await persistAndApply(set, { claudeAuthAutoRefresh: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist Claude auth auto-refresh setting:', err)
      set({ claudeAuthAutoRefresh: prev })
    }
  },

  setActiveWindowHours: async (hours: number) => {
    const prev = get().activeWindowHours
    const next = clampActiveWindowHours(hours)
    set({ activeWindowHours: next })
    try {
      await persistAndApply(set, { activeWindowHours: next })
    } catch (err) {
      console.error('[settings] Failed to persist active window hours:', err)
      set({ activeWindowHours: prev })
    }
  },

  updateEditorSettings: async (partial: Partial<EditorSettingsBackend>) => {
    const prev = get().editor
    const merged = { ...prev, ...partial }
    set({ editor: merged })
    try {
      await persistAndApply(set, { editor: merged })
    } catch (err) {
      console.error('[settings] Failed to persist editor settings:', err)
      set({ editor: prev })
    }
  },

  setDefaultAgent: async (agent: string) => {
    const prev = get().defaultAgent
    set({ defaultAgent: agent })
    try {
      await persistAndApply(set, { defaultAgent: agent })
    } catch (err) {
      console.error('[settings] Failed to persist default agent:', err)
      set({ defaultAgent: prev })
    }
  },

  resetAllSettings: async () => {
    const result = await settingsReset()
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      editor: mergeEditorDefaults(result.editor),
    })
  },

  fetchSettings: async () => {
    const seqBefore = _writeSeq
    const result = await settingsGet()
    // If a write happened while we were fetching, skip — the write's result is fresher
    if (_writeSeq !== seqBefore) return
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? false,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      activeWindowHours: clampActiveWindowHours(result.activeWindowHours ?? DEFAULT_ACTIVE_WINDOW_HOURS),
      editor: mergeEditorDefaults(result.editor),
      lastActiveProjectId: result.lastActiveProjectId ?? null,
      lastActiveWorkspaceId: result.lastActiveWorkspaceId ?? null,
      loaded: true
    })
  }
}))

/**
 * Get the effective key combo for a hotkey id,
 * falling back to the default if no override exists.
 */
export function getEffectiveKeybinding(
  keybindings: Record<string, string>,
  id: string
): string {
  if (keybindings[id]) return keybindings[id]
  const defaults = getDefaultKeybindings()
  return defaults[id] ?? ''
}

// Initialize on import
useSettingsStore.getState().fetchSettings()

// #625 — on a real active-host CHANGE, re-fetch ALL app settings from the
// NEW host's daemon. `fetchSettings()` does its own host-aware
// `settingsGet()` (reads `activeHost` at call time) and `onActiveHostChange`
// fires AFTER the flip, so this targets the new host. `_writeSeq` is a
// write-safety counter and is intentionally NOT reset.
onActiveHostChange(() => {
  void useSettingsStore.getState().fetchSettings()
})

// Load custom editor themes from ~/.k2so/themes/ after settings are ready
import('./custom-themes').then(({ useCustomThemesStore }) => {
  useCustomThemesStore.getState().loadCustomThemes()
}).catch(() => {})

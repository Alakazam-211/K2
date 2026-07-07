import { create } from 'zustand'
import { emit } from '@tauri-apps/api/event'
import { getDefaultKeybindings } from '@shared/hotkeys'
import type { AppSettingsResponse, EditorSettingsBackend, StyleSettingsBackend } from '@shared/types'
// Style System P3 — the live selection store. settings.ts owns
// persistence (daemon-canonical); style.ts owns the DOM stamping +
// localStorage mirror. One-way import (style.ts must never import us).
import { useStyleStore } from '@/stores/style'
import { parseSchemeMode } from '@/lib/style-resolve'
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

// NOTE the naming collision: 'projects' is the LEGACY workspaces section
// (label "Workspaces" — ProjectsSection.tsx); 'project-groups' is the
// Projects-V1 groups section (label "Projects" — Projects/ProjectSettings).
export type SettingsSection = 'general' | 'styles' | 'terminal' | 'code-editor' | 'editors-agents' | 'keybindings' | 'projects' | 'project-groups' | 'timer' | 'workspace-states' | 'agent-skills' | 'heartbeats' | 'companion' | 'wake-scheduler' | 'permissions' | 'dictation-lab' | 'connections' | 'k2-connect'

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

  // "Your display name" — the name K2 agents recognize you (the owner)
  // by. Used server-side as the composer `from` attribution. Empty
  // string = unset (the daemon falls back to "owner").
  ownerDisplayName: string

  // Composer 1c (D4) — per-host opt-in letting CONNECT-USERS instruct
  // agents via the composer. Owner is always allowed regardless; this
  // gates only remote multi-user instruction. DEFAULTS OFF. The daemon
  // enforces it server-side; the renderer-hide reads this same signal.
  allowRemoteInstruct: boolean

  // Cross-server federation master switch (K2 Connect → Enable federation).
  // Persisted per-server; the daemon's /cli/federation/* gate honors it via
  // federation::set_enabled(). DEFAULTS OFF (dark by default).
  federationEnabled: boolean

  // GH#8 — "Use local LLM to detect HITL" opt-in. Gates whether the
  // `talk` CLI's /cli/terminal/classify detection step runs the bundled
  // 1.5B model (ON) or stays regex-only (OFF). DEFAULTS OFF (inference
  // costs battery/CPU). The daemon reads this same flag server-side to
  // decide whether the model is ever consulted.
  useLlmHitlDetection: boolean

  // F4 — completion chime for UNSEEN agent completions only (an agent
  // finished while its pane wasn't being watched). The amber ActiveBar
  // dot always shows; this gates only the sound. DEFAULTS ON.
  completionSoundEnabled: boolean

  // Editor settings
  editor: EditorSettingsBackend

  // Style System P3 — the persisted Style selection (style / palette /
  // scheme mode / gaps preset). Daemon-canonical; stores/style.ts holds
  // the live resolved state + stamps <html>. Every read-back below
  // re-applies it there so the daemon wins over the localStorage mirror.
  style: StyleSettingsBackend

  // Default AI agent. Canonically an agent_presets id (UUID); legacy
  // settings.json values hold a command first-token like 'claude'.
  // Never compare this field directly — resolve it via @/lib/agent-resolve
  // (resolveAgentPreset / useResolvedAgentCommand), which accepts both.
  defaultAgent: string

  // Pre-select a specific project in the projects section
  initialProjectId: string | null

  // Pre-select a specific project GROUP in the project-groups section
  // (the Projects-page gear / right-click deep-link — prd-projects-v1 §6.5)
  initialProjectGroupId: string | null

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
  setOwnerDisplayName: (name: string) => void
  setAllowRemoteInstruct: (enabled: boolean) => void
  setFederationEnabled: (enabled: boolean) => void
  setUseLlmHitlDetection: (enabled: boolean) => void
  setCompletionSoundEnabled: (enabled: boolean) => void
  updateEditorSettings: (partial: Partial<EditorSettingsBackend>) => void
  updateStyleSettings: (partial: Partial<StyleSettingsBackend>) => void
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

/** "Your display name" — max length, mirrors the daemon's 64-char cap
 *  (`OWNER_DISPLAY_NAME_MAX` / `validate_display_name`). */
export const OWNER_DISPLAY_NAME_MAX = 64

/** Light client-side sanitize that MIRRORS the daemon's
 *  `sanitize_owner_display_name`: strip ESC + control chars (newlines,
 *  CR, tab), trim, cap length. The daemon re-sanitizes server-side
 *  regardless (D3 is enforced there); this just keeps the UI honest. */
export function sanitizeOwnerDisplayName(raw: string): string {
  // Drop C0 controls (incl. ESC 0x1b, newline, CR, tab) + DEL (0x7f),
  // then trim + cap. Char-code filter avoids embedding raw control bytes
  // in a regex literal.
  const stripped = Array.from(raw)
    .filter((ch) => {
      const code = ch.codePointAt(0) ?? 0
      return code >= 0x20 && code !== 0x7f
    })
    .join('')
  return stripped.trim().slice(0, OWNER_DISPLAY_NAME_MAX)
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

/** Style System P3 — mirrors the daemon's `StyleSettings::default()`
 *  (Square / charcoal / dark / base density). */
const DEFAULT_STYLE: StyleSettingsBackend = {
  id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '',
}

/** Merge backend style result with defaults (older daemons omit `style`
 *  entirely; partial snapshots may omit fields). */
function mergeStyleDefaults(result: Partial<StyleSettingsBackend> | undefined): StyleSettingsBackend {
  if (!result) return { ...DEFAULT_STYLE }
  return { ...DEFAULT_STYLE, ...result }
}

/** Push a daemon-canonical style read-back into the live style store —
 *  restamps <html> + refreshes the localStorage mirror, so the daemon's
 *  value wins over whatever the pre-paint mirror stamped. Called on
 *  EVERY settings read-back (fetch, write echo, reset) — applyStyle is
 *  idempotent, so redundant calls are free. */
function applyBackendStyle(result: Partial<StyleSettingsBackend> | undefined): void {
  const style = mergeStyleDefaults(result)
  useStyleStore.getState().applyStyle({
    styleId: style.id,
    paletteId: style.palette,
    schemeMode: parseSchemeMode(style.scheme),
    gapsPreset: style.gaps,
  })
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
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? true,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      activeWindowHours: clampActiveWindowHours(result.activeWindowHours ?? DEFAULT_ACTIVE_WINDOW_HOURS),
      ownerDisplayName: result.ownerDisplayName ?? '',
      allowRemoteInstruct: result.allowRemoteInstruct ?? false,
      federationEnabled: result.federationEnabled ?? false,
      useLlmHitlDetection: result.useLlmHitlDetection ?? false,
      completionSoundEnabled: result.completionSoundEnabled ?? true,
      editor: mergeEditorDefaults(result.editor),
      style: mergeStyleDefaults(result.style),
      lastActiveProjectId: result.lastActiveProjectId ?? null,
      lastActiveWorkspaceId: result.lastActiveWorkspaceId ?? null,
      loaded: true
    })
    // Echo-apply: the daemon's post-merge style is canonical — restamp
    // this window from it (covers writes made by OTHER settings keys
    // too, since the daemon echoes the full snapshot).
    applyBackendStyle(result.style)
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
  agenticSystemsEnabled: true,
  claudeAuthAutoRefresh: false,
  activeWindowHours: DEFAULT_ACTIVE_WINDOW_HOURS,
  ownerDisplayName: '',
  allowRemoteInstruct: false,
  federationEnabled: false,
  useLlmHitlDetection: false,
  completionSoundEnabled: true,
  editor: { ...DEFAULT_EDITOR },
  style: { ...DEFAULT_STYLE },
  defaultAgent: 'claude',
  initialProjectId: null,
  initialProjectGroupId: null,
  lastActiveProjectId: null,
  lastActiveWorkspaceId: null,
  loaded: false,
  pendingUpdateCheck: false,

  openSettings: (section?: SettingsSection, projectId?: string) => {
    // The optional preselect payload routes by section: the legacy
    // 'projects' (Workspaces) section consumes initialProjectId; the
    // 'project-groups' (Projects) section consumes initialProjectGroupId.
    set({
      settingsOpen: true,
      activeSection: section ?? get().activeSection,
      initialProjectId: section === 'project-groups' ? null : (projectId ?? null),
      initialProjectGroupId: section === 'project-groups' ? (projectId ?? null) : null
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

  setOwnerDisplayName: async (name: string) => {
    const prev = get().ownerDisplayName
    // Mirror the daemon's sanitize so the optimistic value matches what
    // gets persisted (the daemon re-sanitizes server-side regardless).
    const next = sanitizeOwnerDisplayName(name)
    set({ ownerDisplayName: next })
    try {
      // Empty string clears the override → daemon falls back to "owner".
      await persistAndApply(set, { ownerDisplayName: next })
    } catch (err) {
      console.error('[settings] Failed to persist owner display name:', err)
      set({ ownerDisplayName: prev })
    }
  },

  setAllowRemoteInstruct: async (enabled: boolean) => {
    const prev = get().allowRemoteInstruct
    set({ allowRemoteInstruct: enabled }) // optimistic
    try {
      // Partial update — the daemon deep-merges `allowRemoteInstruct`.
      // The daemon ALSO enforces the gate server-side; this toggle only
      // sets the host's opt-in (and drives the renderer-hide signal).
      await persistAndApply(set, { allowRemoteInstruct: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist allow-remote-instruct:', err)
      set({ allowRemoteInstruct: prev })
    }
  },

  setFederationEnabled: async (enabled: boolean) => {
    const prev = get().federationEnabled
    set({ federationEnabled: enabled }) // optimistic
    try {
      // Partial update — the daemon deep-merges `federationEnabled` and syncs
      // it into federation::set_enabled() so the /cli/federation/* gate flips
      // immediately on THIS host. persistAndApply is host-aware, so toggling
      // while connected to a remote server writes to THAT server's daemon.
      await persistAndApply(set, { federationEnabled: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist federation-enabled:', err)
      set({ federationEnabled: prev })
    }
  },

  setUseLlmHitlDetection: async (enabled: boolean) => {
    const prev = get().useLlmHitlDetection
    set({ useLlmHitlDetection: enabled }) // optimistic
    try {
      // Partial update — the daemon deep-merges `useLlmHitlDetection` and
      // reads it server-side on the /cli/terminal/classify path.
      await persistAndApply(set, { useLlmHitlDetection: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist use-llm-hitl-detection:', err)
      set({ useLlmHitlDetection: prev })
    }
  },

  setCompletionSoundEnabled: async (enabled: boolean) => {
    const prev = get().completionSoundEnabled
    set({ completionSoundEnabled: enabled }) // optimistic
    try {
      // Partial update — the daemon deep-merges `completionSoundEnabled`.
      await persistAndApply(set, { completionSoundEnabled: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist completion-sound-enabled:', err)
      set({ completionSoundEnabled: prev })
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

  updateStyleSettings: async (partial: Partial<StyleSettingsBackend>) => {
    const prev = get().style
    const merged = { ...prev, ...partial }
    set({ style: merged }) // optimistic
    // Apply live immediately (idempotent — the Settings page usually
    // already applied via the style store before committing here).
    applyBackendStyle(merged)
    try {
      await persistAndApply(set, { style: merged })
      // Cross-window sync: other windows re-fetch settings and restamp
      // their own <html> via the fetchSettings read-back. Fire-and-forget
      // (non-Tauri/web contexts have no event backend) — pattern per
      // stores/projects.ts emitProjectsChanged.
      void emit('sync:settings').catch((e) =>
        console.warn('[settings] sync:settings emit failed:', e),
      )
    } catch (err) {
      console.error('[settings] Failed to persist style settings:', err)
      set({ style: prev })
      applyBackendStyle(prev)
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
      ownerDisplayName: result.ownerDisplayName ?? '',
      allowRemoteInstruct: result.allowRemoteInstruct ?? false,
      federationEnabled: result.federationEnabled ?? false,
      useLlmHitlDetection: result.useLlmHitlDetection ?? false,
      completionSoundEnabled: result.completionSoundEnabled ?? true,
      editor: mergeEditorDefaults(result.editor),
      style: mergeStyleDefaults(result.style),
    })
    applyBackendStyle(result.style)
  },

  fetchSettings: async () => {
    const seqBefore = _writeSeq
    let result: Awaited<ReturnType<typeof settingsGet>>
    try {
      result = await settingsGet()
    } catch (e) {
      // Daemon unreachable, or the remote session is dead (settingsGet
      // throws on non-2xx). Keep the current snapshot; the recovery seams
      // (onDaemonConnected, the host-change/session-mint re-fire) retry.
      // Without this, the host-switch burst against a dead remote session
      // escaped as an unhandled rejection (the onActiveHostChange caller
      // is fire-and-forget).
      console.warn('[settings] fetchSettings failed:', e)
      return
    }
    // If a write happened while we were fetching, skip — the write's result is fresher
    if (_writeSeq !== seqBefore) return
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? true,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      activeWindowHours: clampActiveWindowHours(result.activeWindowHours ?? DEFAULT_ACTIVE_WINDOW_HOURS),
      ownerDisplayName: result.ownerDisplayName ?? '',
      allowRemoteInstruct: result.allowRemoteInstruct ?? false,
      federationEnabled: result.federationEnabled ?? false,
      useLlmHitlDetection: result.useLlmHitlDetection ?? false,
      completionSoundEnabled: result.completionSoundEnabled ?? true,
      editor: mergeEditorDefaults(result.editor),
      style: mergeStyleDefaults(result.style),
      lastActiveProjectId: result.lastActiveProjectId ?? null,
      lastActiveWorkspaceId: result.lastActiveWorkspaceId ?? null,
      loaded: true
    })
    // Daemon-canonical style wins over the localStorage-mirror boot
    // apply. Also the restamp path for cross-window `sync:settings`
    // (useWindowSync → fetchSettings → here).
    applyBackendStyle(result.style)
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

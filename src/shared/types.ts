/** Shape of the trpc bridge exposed via contextBridge in the preload script */
export interface TrpcApi {
  invoke: (type: 'query' | 'mutation', path: string, input: unknown) => Promise<unknown>
  subscribe: (
    path: string,
    input: unknown,
    callbacks: {
      onData: (data: unknown) => void
      onError: (error: { message: string; code: string }) => void
      onComplete: () => void
    }
  ) => () => void
}

/** Context menu item shape */
export interface ContextMenuItem {
  id: string
  label: string
  type?: string
  enabled?: boolean
}

/** Terminal zoom IPC listeners */
export interface TerminalZoomApi {
  onZoomIn: (callback: () => void) => () => void
  onZoomOut: (callback: () => void) => () => void
  onZoomReset: (callback: () => void) => () => void
}

/** Full window.api shape exposed by the preload script */
export interface WindowApi {
  trpc: TrpcApi
  showContextMenu: (items: ContextMenuItem[]) => Promise<string | null>
  terminalZoom: TerminalZoomApi
}

/**
 * Re-export path for AppRouter type.
 * Import directly from the router module for type inference:
 *
 *   import type { AppRouter } from '../../main/lib/trpc/router'
 *
 * This file exists so renderer code can import shared interfaces
 * without pulling in main-process Node.js code at runtime.
 */
export type { WindowApi as WindowApiType, TrpcApi as TrpcApiType }

// ── Backend settings types (mirrors Rust AppSettings) ─────────────────

export interface TerminalSettingsBackend {
  fontFamily: string
  fontSize: number
  cursorStyle: 'bar' | 'block' | 'underline'
  scrollback: number
  naturalTextEditing: boolean
}

export interface TimerSettingsBackend {
  visible: boolean
  countdownEnabled: boolean
  countdownTheme: string
  skipMemo: boolean
  timezone: string
  customThemes: Record<string, string>[]
}

/** Matches Rust `AppSettings` (camelCase via serde rename) */
export interface AppSettingsResponse {
  terminal: TerminalSettingsBackend
  keybindings: Record<string, string>
  projectSettings: Record<string, Record<string, string>>
  focusGroupsEnabled: boolean
  activeFocusGroupId: string | null
  sidebarCollapsed: boolean
  leftPanelOpen: boolean
  rightPanelOpen: boolean
  leftPanelActiveTab: string
  rightPanelActiveTab: string
  leftPanelTabs: string[]
  rightPanelTabs: string[]
  defaultAgent: string
  aiAssistantEnabled: boolean
  timer: TimerSettingsBackend
  agenticSystemsEnabled: boolean
  claudeAuthAutoRefresh: boolean
  lastActiveProjectId: string | null
  lastActiveWorkspaceId: string | null
  editor: EditorSettingsBackend
  // Optional: the daemon's `/cli/settings/get` includes this flag, but
  // older snapshots / partial responses may omit it. Read defensively.
  keepDaemonOnQuit?: boolean
  // P1.C — how long (hours) a workspace stays in the Active Bar after the
  // user last interacted with it (rule 2). Optional: older settings.json
  // snapshots omit it, so readers default to 24. Persisted via the
  // daemon's deep-merge of `settings.json` (no Rust struct field needed —
  // the JSON store keeps unknown keys verbatim).
  activeWindowHours?: number
}

export interface EditorSettingsBackend {
  tabSize: number
  wordWrap: boolean
  showWhitespace: boolean
  fontSize: number
  indentGuides: boolean
  foldGutter: boolean
  autocomplete: boolean
  bracketMatching: boolean
  lineNumbers: boolean
  highlightActiveLine: boolean
  // Phase 6
  stickyScroll: boolean
  minimap: boolean
  // Phase 7
  theme: string
  fontFamily: string
  fontLigatures: boolean
  cursorStyle: 'bar' | 'block' | 'underline'
  cursorBlink: boolean
  // Phase 8
  scrollPastEnd: boolean
  scrollbarAnnotations: boolean
  diffStyle: 'gutter' | 'inline'
  formatOnSave: boolean
  vimMode: boolean
}

export type EditorThemeId = 'k2so-dark' | 'one-dark' | 'dracula' | 'nord' | 'github-dark'

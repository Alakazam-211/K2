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
  skipMemo: boolean
  timezone: string
  // NOTE: the daemon's Rust TimerSettings still serializes legacy
  // countdown fields (countdownEnabled/countdownTheme/customThemes);
  // the renderer ignores them since the countdown feature was removed
  // (stopwatch rework, presence/multiplayer S0).
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
  // "Your display name" — the name K2 agents recognize the OWNER by.
  // Resolved server-side as the composer `from` attribution (the
  // daemon NEVER reads `from` from the request body, D3). Optional +
  // nullable: older snapshots omit it and the Rust field is
  // `Option<String>` (serializes to `null` when unset), so readers
  // treat unset/null/blank as "owner".
  ownerDisplayName?: string | null
  // Composer 1c (D4) — per-host opt-in that lets a CONNECT-USER instruct
  // agents via the composer route. The OWNER is always allowed regardless;
  // this gates ONLY remote multi-user instruction and DEFAULTS OFF (the
  // route instructs an agent running --dangerously-skip-permissions). The
  // renderer hides the composer when this is off on a remote host, but the
  // daemon enforces the gate server-side (renderer-hide is defense-in-depth).
  // Optional: older snapshots omit it → readers treat absent as false.
  allowRemoteInstruct?: boolean
  // DNS K1 — per-host opt-in that lets agents manage DNS records.
  // DEFAULTS OFF (deny-by-default). Optional: older snapshots omit it →
  // readers treat absent as false.
  dnsManageEnabled?: boolean
  // Federation (0.40.14+) — whether cross-server messaging is enabled on
  // this host. Optional: older snapshots omit it → readers treat absent
  // as false (the store reads it with `?? false`).
  federationEnabled?: boolean
  // 0.40.43 (1c) — public /v1 API master switch (K2 Connect → Enable
  // public API). ORed with the K2_API env flag server-side and checked
  // per request, so flipping it needs NO daemon restart. Owner/Admin-only
  // to write (the daemon 403s a Member touching it). Optional: older
  // snapshots omit it → readers treat absent as false (surface dark).
  apiEnabled?: boolean
  // GH#8 — "Use local LLM to detect HITL" opt-in (Settings → General).
  // Gates whether the `talk` CLI's /cli/terminal/classify detection step
  // runs the bundled 1.5B model (ON) or stays regex-only (OFF, default).
  // Optional: older snapshots omit it → readers treat absent as false.
  useLlmHitlDetection?: boolean
  // F4 — play a soft chime when an agent finishes while its pane isn't
  // being watched (the unseen-done fire; see completion-sound.ts).
  // Persisted via the daemon's settings.json deep-merge (no Rust struct
  // field needed). Optional: older snapshots omit it → readers treat
  // absent as true (default ON).
  completionSoundEnabled?: boolean
  // Style System P3 — the persisted Style selection. Backed by the typed
  // Rust `StyleSettings` struct in k2-core/app_settings.rs (a typed field
  // is REQUIRED there: AppSettings round-trips through serde and drops
  // unknown keys). Optional: daemons older than the style arc omit it →
  // readers fall back to the Square/charcoal/dark defaults.
  style?: StyleSettingsBackend
}

/** Matches Rust `StyleSettings` (camelCase via serde rename).
 *  `scheme` is the user's MODE ('dark' | 'light' | 'auto') — 'auto'
 *  resolution against the OS appearance happens renderer-side.
 *  `gaps` is '' (the style's base density) or one of the style's
 *  declared gap presets (e.g. 'regular' / 'spacious'). */
export interface StyleSettingsBackend {
  id: string
  palette: string
  scheme: string
  gaps: string
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

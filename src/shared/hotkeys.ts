// ── Hotkey configuration ──────────────────────────────────────────────

export interface HotkeyDefinition {
  id: string
  label: string
  defaultKey: string
  /** Default off macOS when it differs from `defaultKey` (a Control+Command
   *  chord on macOS is Ctrl+Alt on Linux and Windows). */
  defaultKeyOther?: string
  category: 'Terminal' | 'Tabs' | 'Navigation' | 'App'
}

/** Keys that cannot be rebound — they must pass through to the terminal */
export const RESERVED_KEYS = ['Ctrl+C', 'Ctrl+D', 'Ctrl+Z'] as const

export type ReservedKey = (typeof RESERVED_KEYS)[number]

export const HOTKEYS: HotkeyDefinition[] = [
  // ── Terminal ─────────────────────────────────────────────────────────
  { id: 'clearTerminal', label: 'Clear Terminal', defaultKey: 'Meta+K', category: 'Terminal' },
  {
    id: 'splitVertical',
    label: 'Split Pane Vertical',
    defaultKey: 'Meta+D',
    category: 'Terminal'
  },
  {
    id: 'splitHorizontal',
    label: 'Split Pane Horizontal',
    defaultKey: 'Meta+Shift+D',
    category: 'Terminal'
  },
  {
    id: 'launchDefaultAgent',
    label: 'Launch Default Agent',
    defaultKey: 'Meta+Shift+T',
    category: 'App'
  },

  // ── Tabs ─────────────────────────────────────────────────────────────
  { id: 'newTab', label: 'New Tab', defaultKey: 'Meta+T', category: 'Tabs' },
  { id: 'closeTab', label: 'Close Tab', defaultKey: 'Meta+W', category: 'Tabs' },
  { id: 'prevTab', label: 'Previous Tab', defaultKey: 'Meta+Alt+ArrowLeft', category: 'Tabs' },
  { id: 'nextTab', label: 'Next Tab', defaultKey: 'Meta+Alt+ArrowRight', category: 'Tabs' },
  // ── Navigation ──────────────────────────────────────────────────────
  // Note: ⌘1-9 and ⇧⌘1-9 for Active/Pinned workspaces are handled dynamically
  // based on the shortcut layout setting and are not individually rebindable.
  {
    id: 'toggleSidebar',
    label: 'Toggle Sidebar',
    defaultKey: 'Meta+B',
    category: 'Navigation'
  },
  {
    id: 'toggleLeftPanel',
    label: 'Toggle Left Panel',
    defaultKey: 'Meta+Shift+E',
    category: 'Navigation'
  },
  {
    id: 'toggleRightPanel',
    label: 'Toggle Right Panel',
    defaultKey: 'Meta+Shift+B',
    category: 'Navigation'
  },
  {
    id: 'toggleHiddenFiles',
    label: 'Show/Hide Hidden Files',
    defaultKey: 'Meta+Shift+.',
    category: 'Navigation'
  },
  // Page switch (0.46.0, Rosson 2026-10-16): the top bar's tabs in order.
  // ⌃⌘1–6 on macOS, Ctrl+Alt+1–6 on Linux / Windows; 5 Wiki and 6 Settings
  // since 0.46.4 (Rosson 2026-10-24). Handled by `useWorkspaceIndexShortcuts`
  // (lib/page-shortcuts.ts); no native menu accelerator, so a rebind here is
  // the only owner of the chord.
  { id: 'goToHome', label: 'Go to My Home', defaultKey: 'Ctrl+Meta+1', defaultKeyOther: 'Ctrl+Alt+1', category: 'Navigation' },
  { id: 'goToAgents', label: 'Go to Agents', defaultKey: 'Ctrl+Meta+2', defaultKeyOther: 'Ctrl+Alt+2', category: 'Navigation' },
  { id: 'goToProjects', label: 'Go to Projects', defaultKey: 'Ctrl+Meta+3', defaultKeyOther: 'Ctrl+Alt+3', category: 'Navigation' },
  { id: 'goToTickets', label: 'Go to Tickets', defaultKey: 'Ctrl+Meta+4', defaultKeyOther: 'Ctrl+Alt+4', category: 'Navigation' },
  { id: 'goToWiki', label: 'Go to Wiki', defaultKey: 'Ctrl+Meta+5', defaultKeyOther: 'Ctrl+Alt+5', category: 'Navigation' },
  { id: 'goToSettings', label: 'Go to Settings', defaultKey: 'Ctrl+Meta+6', defaultKeyOther: 'Ctrl+Alt+6', category: 'Navigation' },

  // ── App ──────────────────────────────────────────────────────────────
  { id: 'openSettings', label: 'Open Settings', defaultKey: 'Meta+,', category: 'App' },
  { id: 'newWindow', label: 'New Window', defaultKey: 'Meta+Shift+N', category: 'App' },
  { id: 'newDocument', label: 'New Document', defaultKey: 'Meta+N', category: 'App' },
  { id: 'openWorkspace', label: 'Open Workspace', defaultKey: 'Meta+O', category: 'App' },
  { id: 'focusWindow', label: 'Open in Focus Window', defaultKey: 'Meta+Shift+F', category: 'App' },
  { id: 'toggleServerSwitcher', label: 'Switch Server', defaultKey: 'Meta+L', category: 'App' },
  { id: 'toggleAssistant', label: 'Toggle Assistant', defaultKey: 'Meta+Shift+L', category: 'App' },
  { id: 'openProjects', label: 'Open Projects', defaultKey: 'Meta+P', category: 'App' }
]

/** Build a map of id -> default combo for quick lookup. `isMac` false
 *  picks `defaultKeyOther` where a hotkey has one. */
export function getDefaultKeybindings(isMac = true): Record<string, string> {
  const map: Record<string, string> = {}
  for (const h of HOTKEYS) {
    map[h.id] = !isMac && h.defaultKeyOther ? h.defaultKeyOther : h.defaultKey
  }
  return map
}

/** The combo in force for `id`: the user's override, else the platform's
 *  default ('' for an unknown id). */
export function effectiveKeybinding(keybindings: Record<string, string>, id: string, isMac: boolean): string {
  if (keybindings[id]) return keybindings[id]
  return getDefaultKeybindings(isMac)[id] ?? ''
}

/**
 * Format a key combo string for display.
 * Converts Meta+ to Cmd+ on mac, Ctrl+ on other platforms.
 * NOTE: This function uses `navigator` and should only be called from the renderer.
 */
export function formatKeyCombo(combo: string, isMac = true): string {
  return combo
    .replace(/Meta\+/g, isMac ? '\u2318' : 'Ctrl+')
    .replace(/Shift\+/g, isMac ? '\u21E7' : 'Shift+')
    .replace(/Alt\+/g, isMac ? '\u2325' : 'Alt+')
    .replace(/Ctrl\+/g, isMac ? '\u2303' : 'Ctrl+')
    .replace(/ArrowLeft/g, '\u2190')
    .replace(/ArrowRight/g, '\u2192')
    .replace(/ArrowUp/g, '\u2191')
    .replace(/ArrowDown/g, '\u2193')
}

/** Minimal interface for the keyboard event fields we need */
interface KeyEventLike {
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  shiftKey: boolean
  key: string
}

/**
 * Convert a KeyboardEvent into a normalized key combo string
 * matching the format used in HOTKEYS definitions.
 */
export function keyEventToCombo(e: KeyEventLike): string {
  const parts: string[] = []
  if (e.ctrlKey) parts.push('Ctrl')
  if (e.metaKey) parts.push('Meta')
  if (e.altKey) parts.push('Alt')
  if (e.shiftKey) parts.push('Shift')

  // Avoid adding modifier keys themselves as the key part
  const ignoredKeys = new Set(['Control', 'Meta', 'Alt', 'Shift'])
  if (!ignoredKeys.has(e.key)) {
    // Normalize single character keys to uppercase for letters
    const key = e.key.length === 1 ? e.key.toUpperCase() : e.key
    parts.push(key)
  }

  return parts.join('+')
}

/**
 * Check if a key combo is reserved and cannot be rebound.
 */
export function isReservedKey(combo: string): boolean {
  return (RESERVED_KEYS as readonly string[]).includes(combo)
}

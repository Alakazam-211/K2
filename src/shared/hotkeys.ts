// ── Hotkey configuration ──────────────────────────────────────────────

export interface HotkeyDefinition {
  id: string
  label: string
  defaultKey: string
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

  // ── App ──────────────────────────────────────────────────────────────
  { id: 'openSettings', label: 'Open Settings', defaultKey: 'Meta+,', category: 'App' },
  { id: 'newWindow', label: 'New Window', defaultKey: 'Meta+Shift+N', category: 'App' },
  { id: 'newDocument', label: 'New Document', defaultKey: 'Meta+N', category: 'App' },
  { id: 'openWorkspace', label: 'Open Workspace', defaultKey: 'Meta+O', category: 'App' },
  { id: 'focusWindow', label: 'Open in Focus Window', defaultKey: 'Meta+Shift+F', category: 'App' },
  { id: 'toggleAssistant', label: 'Toggle Assistant', defaultKey: 'Meta+L', category: 'App' },
  { id: 'reviewQueue', label: 'Review Queue', defaultKey: 'Meta+P', category: 'App' }
]

/** Build a map of id -> defaultKey for quick lookup */
export function getDefaultKeybindings(): Record<string, string> {
  const map: Record<string, string> = {}
  for (const h of HOTKEYS) {
    map[h.id] = h.defaultKey
  }
  return map
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

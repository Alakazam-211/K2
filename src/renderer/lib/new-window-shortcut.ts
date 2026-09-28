// Keyboard owner for New Window. One chord, one action.
// Cmd+N (no shift) stays the untitled-document shortcut elsewhere.
// The stored default stays `Meta+Shift+N`; display formatting is separate.

export type NewWindowChord = {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  isMac: boolean
}

/**
 * Ctrl+Shift+N with Meta up opens a window on every platform.
 * Cmd+Shift+N opens a window on macOS only.
 * Shift+N alone, Cmd+N, and Cmd+Shift+N off macOS do not.
 * Never the untitled-document action.
 */
export function decideNewWindowShortcut(e: NewWindowChord): 'window_new' | null {
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key
  if (key !== 'n' || !e.shiftKey || e.altKey) return null
  if (e.ctrlKey && !e.metaKey) return 'window_new'
  if (e.isMac && e.metaKey && !e.ctrlKey) return 'window_new'
  return null
}

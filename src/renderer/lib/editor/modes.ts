// Write | Read | Source (build-editors.md E1b; the notes' §8 and §9). A
// markdown file opens in Write, or in Read when you can't edit it; a code file
// is always Source and has no mode control. ⌘E toggles Write and Read, ⇧⌘E
// opens Source. A read-only file never reaches Write; Source stays.

import type { EditorKind } from './preview-switch'

export type EditorMode = 'write' | 'read' | 'source'

export function initialMode(kind: EditorKind, readOnly: boolean): EditorMode {
  if (kind === 'code') return 'source'
  return readOnly ? 'read' : 'write'
}

/** The mode a file can show: Write turns to Read when the file is read only. */
export function usableMode(mode: EditorMode, readOnly: boolean): EditorMode {
  return mode === 'write' && readOnly ? 'read' : mode
}

interface Keyish {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
}

/** The mode ⌘E or ⇧⌘E asks for (Ctrl on Windows and Linux), or null. */
export function modeForKey(mode: EditorMode, e: Keyish, readOnly: boolean): EditorMode | null {
  if (!(e.metaKey || e.ctrlKey) || e.altKey || e.key.toLowerCase() !== 'e') return null
  if (e.shiftKey) return 'source'
  if (mode === 'write') return 'read'
  if (mode === 'read') return readOnly ? null : 'write'
  return readOnly ? 'read' : 'write'
}

// Write | Read | Source (build-editors.md E1b; the notes' §8 and §9). A
// markdown file opens in Write, or in Read when you can't edit it; a code file
// is always Source and has no mode control. ⌘E toggles Write and Read, ⇧⌘E
// opens Source. A read-only file never reaches Write; Source stays. A file
// too large for Write (E2: over 1 MB or 20,000 lines) is Source only.

import type { EditorKind } from './preview-switch'

export type EditorMode = 'write' | 'read' | 'source'

export function initialMode(kind: EditorKind, readOnly: boolean, large = false): EditorMode {
  if (kind === 'code' || large) return 'source'
  return readOnly ? 'read' : 'write'
}

/** The mode a file can show: Write turns to Read when the file is read only, and both to Source when it's too large. */
export function usableMode(mode: EditorMode, readOnly: boolean, large = false): EditorMode {
  if (large) return 'source'
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
export function modeForKey(mode: EditorMode, e: Keyish, readOnly: boolean, large = false): EditorMode | null {
  if (!(e.metaKey || e.ctrlKey) || e.altKey || e.key.toLowerCase() !== 'e') return null
  if (e.shiftKey) return 'source'
  if (large) return null
  if (mode === 'write') return 'read'
  if (mode === 'read') return readOnly ? null : 'write'
  return readOnly ? 'read' : 'write'
}

// A live file's mode, kept for the plain editor that carries on when live
// editing turns off mid-session (build-editors.md E4a, "Live off"): it opens
// in the mode you were in, once.
const handover = new Map<string, EditorMode>()

export function keepModeForHandover(path: string, mode: EditorMode): void {
  handover.set(path, mode)
}

export function takeHandoverMode(path: string): EditorMode | undefined {
  const m = handover.get(path)
  handover.delete(path)
  return m
}

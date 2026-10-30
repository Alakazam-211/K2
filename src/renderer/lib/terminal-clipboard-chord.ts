// Windows terminal copy / paste chords, as Windows Terminal has them:
// Ctrl+Shift+C copies the selection, Ctrl+Shift+V pastes. Plain Ctrl+C
// and Ctrl+V stay with the shell (^C interrupts, ^V is literal-next),
// and every other Ctrl+letter keeps going to the PTY.
//
// macOS copies and pastes with ⌘C / ⌘V (the webview's own copy / paste
// events). Linux is unchanged for now (its terminals use the same chords;
// turning them on there is a separate call).

import type { DesktopOs } from '@/lib/desktop-chrome'

export type TerminalClipboardChord = 'copy' | 'paste'

type ChordKey = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey'>

export function terminalClipboardChord(e: ChordKey, os: DesktopOs): TerminalClipboardChord | null {
  if (os !== 'windows') return null
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return null
  const key = e.key.length === 1 ? e.key.toLowerCase() : ''
  if (key === 'c' || (key === '' && e.code === 'KeyC')) return 'copy'
  if (key === 'v' || (key === '' && e.code === 'KeyV')) return 'paste'
  return null
}

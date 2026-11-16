// The host's keys and focus around a mounted Live editor, shared by both
// homes it has (LiveDocView, today's view; the new editors' LiveEditorFile):
//
//   - Undo and redo are the editor's whenever the caret is in it, and every
//     press stops here: nothing to undo, a throw, or CodeMirror sitting the
//     key out (it ignores keys during a composition) must not hand Cmd+Z to
//     the app menu, whose native undo lands in another text box (the chat).
//     Capture, so CodeMirror's own binding doesn't run it a second time.
//     `blocked` (while disconnected: the server owns undo) swallows them.
//   - The caret fell out of the editor to nothing, and not by the person's
//     own click or a window switch: put it back, ahead of the app's poll that
//     gives an empty focus to the message box.

import { isMacPlatform } from '@/lib/desktop-chrome'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import type { LiveEditorHandle } from './mount'

/** How long after a pointer press outside the editor a focus loss counts as
 *  the person's own move (they clicked elsewhere): the editor leaves it. */
const OWN_MOVE_MS = 600

/** Undo or redo, by the platform's chord: Cmd+Z / Shift+Cmd+Z (and Cmd+Y) on
 *  macOS, Ctrl on the others. Null for any other key. */
export function undoChord(e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>, mac: boolean): 'undo' | 'redo' | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey
  if (!mod || e.altKey) return null
  const k = e.key.toLowerCase()
  if (k === 'z') return e.shiftKey ? 'redo' : 'undo'
  if (k === 'y' && !e.shiftKey) return 'redo'
  return null
}

/** Wire `el` (the editor's host) to the editor `current()` returns. Returns the unwire. */
export function wireLiveHost(el: HTMLElement, current: () => LiveEditorHandle | null, opts: { blocked?: () => boolean } = {}): () => void {
  const mac = isMacPlatform()
  const onKeyDown = (e: KeyboardEvent): void => {
    const chord = undoChord(e, mac)
    if (!chord) return
    e.preventDefault()
    e.stopPropagation()
    const now = current()
    if (!now || opts.blocked?.()) return
    try {
      if (chord === 'undo') now.undo()
      else now.redo()
    } catch (err) {
      console.error(`[live-doc] ${chord} failed:`, err)
    }
  }
  let pressedOutsideAt = -Infinity
  const onPointerDown = (e: Event): void => {
    if (!el.contains(e.target as Node | null)) pressedOutsideAt = Date.now()
  }
  const onFocusOut = (e: FocusEvent): void => {
    if (e.relatedTarget) return
    setTimeout(() => {
      const now = current()
      if (!now || !el.isConnected) return
      const active = document.activeElement
      if (active && active !== document.body) return
      if (Date.now() - pressedOutsideAt < OWN_MOVE_MS) return
      if (!document.hasFocus() || isEffectivelyHidden(el)) return
      now.view.focus()
    }, 0)
  }
  el.addEventListener('keydown', onKeyDown, true)
  el.addEventListener('focusout', onFocusOut)
  document.addEventListener('pointerdown', onPointerDown, true)
  document.addEventListener('mousedown', onPointerDown, true)
  return () => {
    el.removeEventListener('keydown', onKeyDown, true)
    el.removeEventListener('focusout', onFocusOut)
    document.removeEventListener('pointerdown', onPointerDown, true)
    document.removeEventListener('mousedown', onPointerDown, true)
  }
}

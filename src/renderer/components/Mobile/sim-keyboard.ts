// The computer's keyboard → the device (Rosson 10-30: "typing on the
// keyboard should type into the device, including paste"). Pure, so the
// rules are tested without a DOM. `useSimKeyboard` wires them to the
// focused device.
//
// While the device view has focus:
//  - printable characters, Return, Backspace/Delete, Tab and the arrows go
//    to the device, with Shift and Option (Shift+Arrow selects, Option+
//    Arrow jumps a word, Shift+Tab goes back);
//  - an input method (Japanese, Chinese, dead keys like Option+E) composes
//    in K2 and only the committed text is sent, once;
//  - ⌘ chords (Ctrl on Windows/Linux) stay with K2 (⌘W, ⌘T, ⌘L, ⌘1…), except
//    the ones that mean something inside a text field on the device:
//      ⌘V  pastes this computer's clipboard into the device;
//      ⌘A  select all · ⌘Z undo · ⇧⌘Z redo · ⌘X cut · ⌘C copy
//          (inside the device: ⌘C copies to the device's own clipboard);
//      ⇧⌘H Home · ⌘D the dev menu (shake);
//  - Esc stops typing into the device (clicking outside does too).

import type { SimEditOp, SimInput, SimKey } from './sim-stream'

/** One `text` frame (`stream::TEXT_MAX_CHARS`); longer text is a paste. */
export const TEXT_MAX_CHARS = 256
/** One `paste` frame (`stream::PASTE_MAX_BYTES`). */
export const PASTE_MAX_BYTES = 64 * 1024

export interface KeyLike {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  isComposing?: boolean
  /** 229 while an input method owns the key. */
  keyCode?: number
}

export type KeyVerdict =
  /** Goes to the device; the caller prevents the default. */
  | { send: SimInput }
  /** ⌘V: read the clipboard (only now) and paste it. */
  | 'paste'
  /** Esc: stop typing into the device. */
  | 'blur'
  /** An input method is composing: let it, send the commit later. */
  | 'ime'
  /** K2 keeps it. */
  | 'pass'

const NAMED_KEYS: Readonly<Record<string, SimKey>> = {
  Enter: 'enter',
  Backspace: 'backspace',
  Delete: 'delete',
  Tab: 'tab',
  ArrowUp: 'arrow-up',
  ArrowDown: 'arrow-down',
  ArrowLeft: 'arrow-left',
  ArrowRight: 'arrow-right',
}

/** ⌘<letter> → the editing command inside the device. */
const EDIT_CHORDS: Readonly<Record<string, SimEditOp>> = {
  a: 'select-all',
  z: 'undo',
  x: 'cut',
  c: 'copy',
}

const CHORD_HELP: ReadonlyArray<{ keys: string; does: string }> = [
  { keys: '⌘V', does: 'Paste this computer’s clipboard' },
  { keys: '⌘A ⌘X ⌘C', does: 'Select all, cut, copy (the device’s own clipboard)' },
  { keys: '⌘Z ⇧⌘Z', does: 'Undo, redo' },
  { keys: '⇧⌘H', does: 'Home' },
  { keys: '⌘D', does: 'Dev menu (shake)' },
  { keys: 'Esc', does: 'Stop typing into the device' },
]

/** The chords the device gets, for the ⋯ help (Ctrl spelled out off the Mac). */
export function deviceChords(mac: boolean): ReadonlyArray<{ keys: string; does: string }> {
  if (mac) return CHORD_HELP
  return CHORD_HELP.map((c) => ({ ...c, keys: c.keys.replace(/⇧⌘/g, 'Ctrl+Shift+').replace(/⌘/g, 'Ctrl+') }))
}

/**
 * What a keydown on the focused device means. `mac`: ⌘ is the chord key
 * (else Ctrl, and ⌘/Windows chords stay with the system).
 */
export function keyVerdict(e: KeyLike, mac: boolean): KeyVerdict {
  if (e.isComposing || e.keyCode === 229 || e.key === 'Process') return 'ime'
  if (e.key === 'Dead') return 'ime'
  if (e.key === 'Escape' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) return 'blur'
  // AltGr on Windows/Linux reports Ctrl+Alt: it makes a character (@, €).
  if (!mac && e.ctrlKey && e.altKey && !e.metaKey && isPrintable(e.key)) return { send: { t: 'text', text: e.key } }
  const primary = mac ? e.metaKey : e.ctrlKey
  const other = mac ? e.ctrlKey : e.metaKey
  if (other) return 'pass'
  const lower = e.key.length === 1 ? e.key.toLowerCase() : e.key
  if (primary) {
    if (e.altKey) return 'pass'
    if (!e.shiftKey && lower === 'v') return 'paste'
    if (e.shiftKey && lower === 'h') return { send: { t: 'button', button: 'home' } }
    if (!e.shiftKey && lower === 'd') return { send: { t: 'button', button: 'dev-menu' } }
    if (e.shiftKey && lower === 'z') return { send: { t: 'edit', op: 'redo' } }
    const op = e.shiftKey ? undefined : EDIT_CHORDS[lower]
    if (op) return { send: { t: 'edit', op } }
    return 'pass'
  }
  const named = NAMED_KEYS[e.key]
  if (named) {
    return { send: { t: 'key', key: named, ...(e.shiftKey ? { shift: true } : {}), ...(e.altKey ? { alt: true } : {}) } }
  }
  // One printable character (Shift and Option have already made it).
  if (isPrintable(e.key)) return { send: { t: 'text', text: e.key } }
  return 'pass'
}

/** One printable character (not a named key like "Shift" or "F5"). */
function isPrintable(key: string): boolean {
  return [...key].length === 1 && !/\p{Cc}/u.test(key)
}

/** Control characters the device can't take as text (tabs and newlines stay in a paste). */
const STRAY_CONTROLS = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g

const utf8Bytes = (s: string): number => new TextEncoder().encode(s).length

/** An IME commit → one frame: `text`, or `paste` when it is long or has lines. */
export function commitInput(data: string | null | undefined): SimInput | null {
  const text = (data ?? '').replace(STRAY_CONTROLS, '')
  if (!text) return null
  if ([...text].length > TEXT_MAX_CHARS || /[\r\n\t]/.test(text)) return { t: 'paste', text }
  return { t: 'text', text }
}

/** The clipboard's text → one `paste` frame, or why not. */
export function pasteInput(clip: string | null | undefined): { send: SimInput } | { error: string } | null {
  const text = (clip ?? '').replace(STRAY_CONTROLS, '')
  if (!text) return null
  if (utf8Bytes(text) > PASTE_MAX_BYTES) return { error: `The clipboard text is over ${PASTE_MAX_BYTES / 1024} KB; paste less at a time.` }
  return { send: { t: 'paste', text } }
}

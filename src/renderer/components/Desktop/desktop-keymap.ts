// Physical keys the Desktop tab may send (prd-remote-desktop-tab-v1 §6, §8).
//
// `DESKTOP_KEY_CODES` is exactly the `KeyboardEvent.code` list of
// `crates/k2-core/src/desktop/keymap.rs` (`KEYS`); its vitest reads that
// file and fails when the two differ. The daemon maps each code to the
// host's key numbers; this side only decides what to send.
//
// Pure (R14): no React, no DOM, no `window`.

export const DESKTOP_KEY_CODES: readonly string[] = [
  'KeyA', 'KeyS', 'KeyD', 'KeyF', 'KeyH', 'KeyG', 'KeyZ', 'KeyX', 'KeyC', 'KeyV',
  'IntlBackslash', 'KeyB', 'KeyQ', 'KeyW', 'KeyE', 'KeyR', 'KeyY', 'KeyT',
  'Digit1', 'Digit2', 'Digit3', 'Digit4', 'Digit6', 'Digit5', 'Equal', 'Digit9', 'Digit7', 'Minus', 'Digit8', 'Digit0',
  'BracketRight', 'KeyO', 'KeyU', 'BracketLeft', 'KeyI', 'KeyP', 'Enter', 'KeyL', 'KeyJ', 'Quote', 'KeyK',
  'Semicolon', 'Backslash', 'Comma', 'Slash', 'KeyN', 'KeyM', 'Period', 'Tab', 'Space', 'Backquote',
  'Backspace', 'Escape', 'MetaRight', 'MetaLeft', 'ShiftLeft', 'CapsLock', 'AltLeft', 'ControlLeft',
  'ShiftRight', 'AltRight', 'ControlRight', 'F17', 'NumpadDecimal', 'NumpadMultiply', 'NumpadAdd', 'NumLock',
  'AudioVolumeUp', 'AudioVolumeDown', 'AudioVolumeMute', 'NumpadDivide', 'NumpadEnter', 'NumpadSubtract',
  'F18', 'F19', 'NumpadEqual', 'Numpad0', 'Numpad1', 'Numpad2', 'Numpad3', 'Numpad4', 'Numpad5', 'Numpad6',
  'Numpad7', 'F20', 'Numpad8', 'Numpad9', 'IntlYen', 'IntlRo', 'NumpadComma', 'F5', 'F6', 'F7', 'F3', 'F8',
  'F9', 'Lang2', 'F11', 'Lang1', 'F13', 'F16', 'F14', 'F10', 'ContextMenu', 'F12', 'F15', 'Insert', 'Home',
  'PageUp', 'Delete', 'F4', 'End', 'F2', 'PageDown', 'F1', 'ArrowLeft', 'ArrowRight', 'ArrowDown', 'ArrowUp',
  'PrintScreen', 'ScrollLock', 'Pause',
]

const KNOWN: ReadonlySet<string> = new Set(DESKTOP_KEY_CODES)

/** The eight modifiers (released on every drop, R3). */
export const MODIFIERS: readonly string[] = [
  'ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight',
]

export function isKnownKey(code: string): boolean {
  return KNOWN.has(code)
}

export function isModifier(code: string): boolean {
  return MODIFIERS.includes(code)
}

/** A chord: the modifiers held and the key's `code`. */
export interface Chord {
  meta?: boolean
  ctrl?: boolean
  alt?: boolean
  shift?: boolean
  code: string
}

/** One entry in the toolbar's keys menu: a chord K2 (or the OS) would take
 *  before the page sees it, sent as key downs and ups instead. */
export interface KeysMenuEntry {
  id: string
  label: string
  chord: Chord
}

/** The chord that always leaves Capture keys (⌃⌥⌘Esc). */
export const ESCAPE_CHORD: Chord = { ctrl: true, alt: true, meta: true, code: 'Escape' }
export const ESCAPE_CHORD_LABEL = '⌃⌥⌘Esc'

/** Chords Capture keys can't take (vs-live R27): the OS takes ⌘Tab, ⌘Space
 *  and ⌥⌘Esc before any app; the native menu's accelerators
 *  (src-tauri/src/menu.rs: ⌘Q, ⌘, ⌘K ⌘J ⌘P ⌘B ⇧⌘F ⇧⌘= ⇧⌘- ⌃⌘Z) act before
 *  the webview sees the key. Each goes through the keys menu. */
export const CANT_CAPTURE: readonly KeysMenuEntry[] = [
  { id: 'cmd-tab', label: '⌘Tab', chord: { meta: true, code: 'Tab' } },
  { id: 'cmd-space', label: '⌘Space', chord: { meta: true, code: 'Space' } },
  { id: 'opt-cmd-esc', label: '⌥⌘Esc', chord: { meta: true, alt: true, code: 'Escape' } },
  { id: 'cmd-q', label: '⌘Q', chord: { meta: true, code: 'KeyQ' } },
  { id: 'cmd-comma', label: '⌘,', chord: { meta: true, code: 'Comma' } },
  { id: 'cmd-k', label: '⌘K', chord: { meta: true, code: 'KeyK' } },
  { id: 'cmd-j', label: '⌘J', chord: { meta: true, code: 'KeyJ' } },
  { id: 'cmd-p', label: '⌘P', chord: { meta: true, code: 'KeyP' } },
  { id: 'cmd-b', label: '⌘B', chord: { meta: true, code: 'KeyB' } },
  { id: 'shift-cmd-f', label: '⇧⌘F', chord: { meta: true, shift: true, code: 'KeyF' } },
  { id: 'shift-cmd-equal', label: '⇧⌘=', chord: { meta: true, shift: true, code: 'Equal' } },
  { id: 'shift-cmd-minus', label: '⇧⌘-', chord: { meta: true, shift: true, code: 'Minus' } },
  { id: 'ctrl-cmd-z', label: '⌃⌘Z', chord: { meta: true, ctrl: true, code: 'KeyZ' } },
]

/** The keys menu: the chords Capture keys can't take, plus Ctrl+Alt+Del
 *  where it means something (a Windows or Linux host). */
export function keysMenu(hostOs: 'macos' | 'linux' | 'windows'): KeysMenuEntry[] {
  const out = [...CANT_CAPTURE]
  if (hostOs !== 'macos') {
    out.push({ id: 'ctrl-alt-del', label: 'Ctrl+Alt+Del', chord: { ctrl: true, alt: true, code: 'Delete' } })
  }
  return out
}

function sameChord(a: Chord, b: Chord): boolean {
  return a.code === b.code && !!a.meta === !!b.meta && !!a.ctrl === !!b.ctrl && !!a.alt === !!b.alt && !!a.shift === !!b.shift
}

/** Is `c` a chord Capture keys can't take? */
export function isUncapturable(c: Chord): boolean {
  return CANT_CAPTURE.some((e) => sameChord(e.chord, c))
}

export function isEscapeChord(c: Chord): boolean {
  return sameChord(ESCAPE_CHORD, c)
}

/** What happens to one keydown in the pane.
 *  - `send`: goes to the remote desktop as key events.
 *  - `k2`: K2 keeps it (Capture keys off: ⌘ chords stay K2's, like Mobile).
 *  - `keys-menu`: Capture keys on, but K2 or the OS takes this chord
 *    first; offer it from the keys menu.
 *  - `release`: the escape chord: leave Capture keys. */
export type KeyRoute = 'send' | 'k2' | 'keys-menu' | 'release'

export function routeKey(c: Chord, capture: boolean, viewerIsMac: boolean): KeyRoute {
  if (!isKnownKey(c.code)) return 'k2'
  if (capture) {
    if (isEscapeChord(c)) return 'release'
    if (isUncapturable(c)) return 'keys-menu'
    return 'send'
  }
  // Capture keys off: the viewer's own command key keeps K2's shortcuts.
  const command = viewerIsMac ? c.meta : c.ctrl
  return command ? 'k2' : 'send'
}

/** The key events for a chord: modifier downs, the key down, the key up,
 *  then modifier ups in reverse. */
export function chordEvents(c: Chord): Array<{ code: string; down: boolean }> {
  const mods: string[] = []
  if (c.ctrl) mods.push('ControlLeft')
  if (c.alt) mods.push('AltLeft')
  if (c.shift) mods.push('ShiftLeft')
  if (c.meta) mods.push('MetaLeft')
  return [
    ...mods.map((code) => ({ code, down: true })),
    { code: c.code, down: true },
    { code: c.code, down: false },
    ...[...mods].reverse().map((code) => ({ code, down: false })),
  ]
}

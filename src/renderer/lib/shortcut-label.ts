// Platform-aware labels for the window's Meta chords (⌘T, ⌘N, ⇧⌘T, ⌘,).
//
// The webview handlers for these read `e.metaKey` on every platform
// (`useTerminalShortcuts`, `App.tsx`): Command on macOS, Super on Linux,
// the Windows key on Windows. They do NOT fire on Ctrl (Ctrl+T, Ctrl+N stay
// with the terminal), so painting "Ctrl+T" off macOS would name a chord
// that does nothing. New Window is the exception: it takes Ctrl+Shift+N
// everywhere (`lib/new-window-shortcut.ts`) and keeps its own label.
//
// Windows keeps Win+T, Win+N and Win+, for itself, so the app never sees
// them there: no label rather than a dead one.

import { formatKeyCombo } from '@shared/hotkeys'
import { desktopOsFromNavigator, type DesktopOs } from '@/lib/desktop-chrome'

export function currentShortcutOs(): DesktopOs {
  return desktopOsFromNavigator(typeof navigator === 'undefined' ? null : navigator)
}

const MOD_ORDER = ['Ctrl', 'Meta', 'Alt', 'Shift'] as const

/**
 * Label for a `Meta`-based combo in `@shared/hotkeys` form (e.g.
 * `Shift+Meta+T`). macOS: the glyphs, in the order written (write
 * `Shift+Meta+T` for Apple's ⇧⌘T). Linux / other: `Super+Shift+T`.
 * Windows: null (the OS takes Win+letter chords).
 */
export function metaChordLabel(combo: string, os: DesktopOs = currentShortcutOs()): string | null {
  if (os === 'mac') return formatKeyCombo(combo, true)
  if (os === 'windows') return null
  const parts = combo.split('+')
  // A trailing '+' key would split into empty parts; none of ours use it.
  const key = parts[parts.length - 1]
  const mods = new Set(parts.slice(0, -1))
  const names: Record<(typeof MOD_ORDER)[number], string> = {
    Ctrl: 'Ctrl',
    Meta: 'Super',
    Alt: 'Alt',
    Shift: 'Shift',
  }
  const out = MOD_ORDER.filter((m) => mods.has(m)).map((m) => names[m])
  return [...out, key].join('+')
}

/** "Settings (⌘,)" on macOS, "Settings (Super+,)" on Linux, "Settings" on Windows. */
export function withChord(label: string, combo: string, os: DesktopOs = currentShortcutOs()): string {
  const chord = metaChordLabel(combo, os)
  return chord ? `${label} (${chord})` : label
}

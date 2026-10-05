// @vitest-environment jsdom
//
// Omarchy additions 2 and 4 — the theme-cycle and cheat-sheet keys never
// collide with the window's chords, and switches go to this computer's
// daemon (`theme/set`, `theme/next`, `theme/prev`), scoped to the Home only
// when the Home has its own pick.

import { describe, expect, it, vi } from 'vitest'

const posts = vi.hoisted(() => [] as Array<{ hostKey: string; route: string; body: unknown }>)
vi.mock('@/lib/daemon-cli', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/daemon-cli')>()),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    posts.push({ hostKey: scope.hostKey, route, body })
    return { ok: true }
  }),
}))

import type { DesktopOs } from '@/lib/desktop-chrome'
import {
  cycleZenTheme,
  isZenCheatSheetKey,
  setZenTheme,
  zenShortcutGroups,
  zenThemeCycleDir,
  type ZenKeyLike,
} from './zen-theme-switch'
import { isZenChordNonMac } from './zen-shortcut'

function key(code: string, k: string, mods: Partial<ZenKeyLike> = {}): ZenKeyLike {
  return { code, key: k, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, ...mods }
}

const TAKEN_MAC: ZenKeyLike[] = [
  ...[1, 5, 9, 0].map((n) => key(`Digit${n}`, String(n), { metaKey: true })),
  ...[1, 9].map((n) => key(`Digit${n}`, '¡', { metaKey: true, altKey: true })),
  ...[1, 9].map((n) => key(`Digit${n}`, String(n), { ctrlKey: true })),
  key('KeyZ', 'z', { ctrlKey: true, metaKey: true }),
  key('KeyL', 'l', { metaKey: true }),
  key('KeyL', 'L', { metaKey: true, shiftKey: true }),
  key('KeyN', 'N', { metaKey: true, shiftKey: true }),
  key('Comma', ',', { metaKey: true }),
  key('BracketLeft', '[', { metaKey: true }),
  key('Period', '.', { metaKey: true }),
  key('Slash', '/', { metaKey: true }),
]
const TAKEN_OTHER: ZenKeyLike[] = [
  ...[1, 9].map((n) => key(`Digit${n}`, String(n), { ctrlKey: true })),
  key('KeyZ', 'z', { ctrlKey: true, altKey: true }),
  key('KeyL', 'l', { ctrlKey: true }),
  key('KeyN', 'N', { ctrlKey: true, shiftKey: true }),
  key('Period', '.', { ctrlKey: true }),
]

describe('theme keys', () => {
  it('⌃⌘. / ⌃⌘⇧. on macOS, Ctrl+Alt+. / Ctrl+Alt+Shift+. elsewhere', () => {
    expect(zenThemeCycleDir(key('Period', '.', { ctrlKey: true, metaKey: true }), 'mac')).toBe(1)
    expect(zenThemeCycleDir(key('Period', '>', { ctrlKey: true, metaKey: true, shiftKey: true }), 'mac')).toBe(-1)
    for (const os of ['linux', 'windows'] as DesktopOs[]) {
      expect(zenThemeCycleDir(key('Period', '.', { ctrlKey: true, altKey: true }), os)).toBe(1)
      expect(zenThemeCycleDir(key('Period', '>', { ctrlKey: true, altKey: true, shiftKey: true }), os)).toBe(-1)
      const altGr = { ...key('Period', '.', { ctrlKey: true, altKey: true }), getModifierState: (k: string) => k === 'AltGraph' }
      expect(zenThemeCycleDir(altGr, os)).toBeNull()
    }
    // The macOS chord is not the Linux one, and vice versa.
    expect(zenThemeCycleDir(key('Period', '.', { ctrlKey: true, altKey: true }), 'mac')).toBeNull()
    expect(zenThemeCycleDir(key('Period', '.', { ctrlKey: true, metaKey: true }), 'linux')).toBeNull()
  })

  it('collide with none of the window chords, and the escape hatch stays its own', () => {
    for (const e of TAKEN_MAC) {
      expect(zenThemeCycleDir(e, 'mac'), JSON.stringify(e)).toBeNull()
      expect(isZenCheatSheetKey(e, 'mac'), JSON.stringify(e)).toBe(false)
    }
    for (const e of TAKEN_OTHER) {
      expect(zenThemeCycleDir(e, 'linux'), JSON.stringify(e)).toBeNull()
      expect(isZenCheatSheetKey(e, 'linux'), JSON.stringify(e)).toBe(false)
    }
    expect(isZenChordNonMac(key('Period', '.', { ctrlKey: true, altKey: true }))).toBe(false)
  })

  it('? opens the sheet only outside a text field; ⌃⌘/ always', () => {
    expect(isZenCheatSheetKey(key('Slash', '?', { shiftKey: true }), 'mac')).toBe(true)
    const field = document.createElement('textarea')
    expect(isZenCheatSheetKey({ ...key('Slash', '?', { shiftKey: true }), target: field }, 'mac')).toBe(false)
    expect(isZenCheatSheetKey({ ...key('Slash', '/', { ctrlKey: true, metaKey: true }), target: field }, 'mac')).toBe(true)
    expect(isZenCheatSheetKey(key('Slash', '/', { ctrlKey: true, altKey: true }), 'windows')).toBe(true)
  })

  it('set, next and prev go to this computer’s daemon; a Garden’s own pick stays the Garden’s (G16, G30)', async () => {
    posts.length = 0
    await setZenTheme('paper', 'global', 'g-1')
    await cycleZenTheme(1, 'global', 'g-1')
    await cycleZenTheme(-1, 'global', 'g-1')
    await setZenTheme('midnight', 'garden', 'g-1')
    await cycleZenTheme(1, 'garden', 'g-1')
    await cycleZenTheme(-1, 'garden', 'g-1')
    expect(posts).toEqual([
      { hostKey: 'local', route: 'zen/theme/set', body: { name: 'paper' } },
      { hostKey: 'local', route: 'zen/theme/next', body: {} },
      { hostKey: 'local', route: 'zen/theme/prev', body: {} },
      { hostKey: 'local', route: 'zen/theme/set', body: { name: 'midnight', garden: 'g-1' } },
      { hostKey: 'local', route: 'zen/theme/next', body: { garden: 'g-1' } },
      { hostKey: 'local', route: 'zen/theme/prev', body: { garden: 'g-1' } },
    ])
  })

  it('the cheat sheet lists every Zen shortcut per platform', () => {
    const rows = zenShortcutGroups('mac').flatMap((g) => g.rows)
    expect(rows.find((r) => r.keys === '⌥⌘1–9')?.what).toBe('Switch to Garden 1–9')
    const mac = zenShortcutGroups('mac').flatMap((g) => g.rows.map((r) => r.keys))
    for (const k of ['⌃⌘Z', '⌃⌘.', '⌃⌘⇧.', '⌘1–9', '⌥⌘1–9']) expect(mac).toContain(k)
    const linux = zenShortcutGroups('linux').flatMap((g) => g.rows.map((r) => r.keys))
    for (const k of ['Ctrl+Alt+Z', 'Ctrl+Alt+.', 'Ctrl+Alt+Shift+.', '⌘1–9', '⌥⌘1–9']) expect(linux).toContain(k)
  })
})

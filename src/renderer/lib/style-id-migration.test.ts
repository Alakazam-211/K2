// The one-shot localStorage move from retired style ids
// (prd-themes-terminal-glass-v1 §7): Square → Terminal, Bezel folded in.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
  key: (i: number) => Array.from(mem.keys())[i] ?? null,
  get length() {
    return mem.size
  },
})

import {
  LS_ACCENT_PER_PALETTE_MIGRATED,
  migrateRetiredStyleKeys,
  migrateSharedAccentKeys,
} from './style-id-migration'

const snapshot = (): Record<string, string> => Object.fromEntries(mem)

beforeEach(() => mem.clear())

describe('migrateRetiredStyleKeys', () => {
  it('rewrites a saved square and keeps its palette, scheme and gaps', () => {
    mem.set('k2.style', 'square')
    mem.set('k2.palette', 'paper')
    mem.set('k2.palette.light', 'paper')
    mem.set('k2.palette.dark', 'charcoal')
    mem.set('k2.scheme', 'light')
    mem.set('k2.gaps', 'spacious')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({
      'k2.style': 'terminal',
      'k2.palette': 'paper',
      'k2.palette.light': 'paper',
      'k2.palette.dark': 'charcoal',
      'k2.scheme': 'light',
      'k2.gaps': 'spacious',
    })
  })

  it('a Bezel user on Auto lands on Glass with both palettes moved and the gold accent', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.palette', 'porcelain')
    mem.set('k2.palette.light', 'porcelain')
    mem.set('k2.palette.dark', 'graphite')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.gaps', 'spacious')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({
      'k2.style': 'glass',
      'k2.scheme': 'auto',
      'k2.palette': 'yellow',
      'k2.palette.dark': 'yellow',
      'k2.palette.light': 'veil',
      'k2.accent.glass.veil': 'yellow',
    })
  })

  it('a Graphite user on Dark lands on Yellow, and needs no accent override', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.palette', 'graphite')
    mem.set('k2.scheme', 'dark')
    migrateRetiredStyleKeys()
    expect(mem.get('k2.palette')).toBe('yellow')
    expect([...mem.keys()].filter((k) => k.startsWith('k2.accent.'))).toEqual([])
  })

  it('a Porcelain user on Light lands on Veil with the gold accent', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.palette', 'porcelain')
    mem.set('k2.scheme', 'light')
    migrateRetiredStyleKeys()
    expect(mem.get('k2.palette')).toBe('veil')
    expect(mem.get('k2.accent.glass.veil')).toBe('yellow')
  })

  it('never overwrites an accent already chosen on Glass', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.scheme', 'light')
    mem.set('k2.accent.glass.veil', '#123456')
    migrateRetiredStyleKeys()
    expect(mem.get('k2.accent.glass.veil')).toBe('#123456')
  })

  it('sets the accent once: clearing it later is not undone by another boot', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.scheme', 'light')
    migrateRetiredStyleKeys()
    mem.delete('k2.accent.glass.veil')
    migrateRetiredStyleKeys()
    expect(mem.has('k2.accent.glass.veil')).toBe(false)
  })

  it('moves Square gamma and dial keys to Terminal', () => {
    mem.set('k2.textGamma.square.dark', '1.75')
    mem.set('k2.textGamma.square.light', '0.9')
    mem.set('k2.dial.square.frost', '12')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({
      'k2.textGamma.terminal.dark': '1.75',
      'k2.textGamma.terminal.light': '0.9',
      'k2.dial.terminal.frost': '12',
    })
  })

  it('a saved glass-next becomes Glass, keeping palette, accent and tuning', () => {
    mem.set('k2.style', 'glass-next')
    mem.set('k2.palette', 'yellow')
    mem.set('k2.accent.glass-next', '#ff00aa')
    mem.set('k2.dial.glass-next.frost', '12')
    mem.set('k2.textGamma.glass-next.dark', '1.4')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({
      'k2.style': 'glass',
      'k2.palette': 'yellow',
      'k2.accent.glass': '#ff00aa',
      'k2.dial.glass.frost': '12',
      'k2.textGamma.glass.dark': '1.4',
    })
  })

  it('a saved glass stays as it is, palette and accent included', () => {
    mem.set('k2.style', 'glass')
    mem.set('k2.palette', 'veil')
    mem.set('k2.accent.glass', 'yellow')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.style': 'glass', 'k2.palette': 'veil', 'k2.accent.glass': 'yellow' })
  })

  it('moves the tuning even when the saved style is not Square any more', () => {
    mem.set('k2.style', 'glass')
    mem.set('k2.textGamma.square.dark', '1.75')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.style': 'glass', 'k2.textGamma.terminal.dark': '1.75' })
  })

  it('never overwrites a value already at the new key', () => {
    mem.set('k2.textGamma.square.dark', '1.75')
    mem.set('k2.textGamma.terminal.dark', '0.8')
    mem.set('k2.dial.square.frost', '12')
    mem.set('k2.dial.terminal.frost', '3')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.textGamma.terminal.dark': '0.8', 'k2.dial.terminal.frost': '3' })
  })

  it("leaves Bezel's own dial and gamma keys orphaned", () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.textGamma.bezel.dark', '1.2')
    mem.set('k2.dial.bezel.ring', '2')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({
      'k2.style': 'glass',
      'k2.palette': 'yellow',
      'k2.palette.dark': 'yellow',
      'k2.palette.light': 'veil',
      'k2.textGamma.bezel.dark': '1.2',
      'k2.dial.bezel.ring': '2',
    })
  })

  it('does not match a look-alike key', () => {
    mem.set('k2.textGamma.squared.dark', '1.2')
    mem.set('k2.dial.squarely', '2')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.textGamma.squared.dark': '1.2', 'k2.dial.squarely': '2' })
  })

  it('is idempotent', () => {
    mem.set('k2.style', 'square')
    mem.set('k2.textGamma.square.dark', '1.75')
    mem.set('k2.textGamma.terminal.light', '0.9')
    migrateRetiredStyleKeys()
    const once = snapshot()
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual(once)
    expect(once['k2.style']).toBe('terminal')
  })

  it('leaves a current selection, and an empty store, alone', () => {
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({})
    mem.set('k2.style', 'glass')
    mem.set('k2.palette', 'obsidian')
    mem.set('k2.gaps', '')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.style': 'glass', 'k2.palette': 'obsidian', 'k2.gaps': '' })
  })

  it('does not throw when storage does', () => {
    const real = globalThis.localStorage
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('denied')
      },
    })
    expect(() => migrateRetiredStyleKeys()).not.toThrow()
    vi.stubGlobal('localStorage', real)
  })
})

describe('migrateSharedAccentKeys (one accent per style → one per palette)', () => {
  const SWITCHED = 'k2.style.migrated-0-46-11-glass'
  const accentKeys = (): Record<string, string> =>
    Object.fromEntries([...mem].filter(([k]) => k.startsWith('k2.accent.')))

  it('copies a deliberate Glass accent to every Glass palette, then retires the shared key', () => {
    mem.set('k2.accent.glass', '#ff00aa')
    migrateSharedAccentKeys()
    expect(accentKeys()).toEqual({
      'k2.accent.glass.obsidian': '#ff00aa',
      'k2.accent.glass.veil': '#ff00aa',
      'k2.accent.glass.yellow': '#ff00aa',
    })
    expect(mem.get(LS_ACCENT_PER_PALETTE_MIGRATED)).toBe('1')
  })

  it('copies Terminal’s shared accent to every Terminal palette', () => {
    mem.set('k2.accent.terminal', 'yellow')
    migrateSharedAccentKeys()
    expect(accentKeys()).toEqual({
      'k2.accent.terminal.charcoal': 'yellow',
      'k2.accent.terminal.paper': 'yellow',
      'k2.accent.terminal.yellow': 'yellow',
    })
  })

  it('never overwrites a palette that already has its own accent', () => {
    mem.set('k2.accent.glass', '#ff00aa')
    mem.set('k2.accent.glass.veil', '#00ff88')
    migrateSharedAccentKeys()
    expect(mem.get('k2.accent.glass.veil')).toBe('#00ff88')
    expect(mem.get('k2.accent.glass.obsidian')).toBe('#ff00aa')
  })

  it('is idempotent: a second run changes nothing, even if a palette was reset since', () => {
    mem.set('k2.accent.glass', '#ff00aa')
    migrateSharedAccentKeys()
    mem.delete('k2.accent.glass.veil')
    const after = snapshot()
    migrateSharedAccentKeys()
    expect(snapshot()).toEqual(after)
  })

  it('drops the Yellow the 0.46.11 switch forced onto Glass, so each palette shows its own', () => {
    for (const forced of ['yellow', 'YELLOW', '#ffc700', '#E6B400']) {
      mem.clear()
      mem.set(SWITCHED, '1')
      mem.set('k2.accent.glass', forced)
      mem.set('k2.accent.terminal', '#ff00aa')
      migrateSharedAccentKeys()
      expect([forced, accentKeys()]).toEqual([
        forced,
        {
          'k2.accent.terminal.charcoal': '#ff00aa',
          'k2.accent.terminal.paper': '#ff00aa',
          'k2.accent.terminal.yellow': '#ff00aa',
        },
      ])
    }
  })

  it('keeps a Yellow chosen before the switch ran (no switch flag yet)', () => {
    mem.set('k2.accent.glass', 'yellow')
    migrateSharedAccentKeys()
    expect(mem.get('k2.accent.glass.obsidian')).toBe('yellow')
    expect(mem.get('k2.accent.glass.veil')).toBe('yellow')
  })

  it('past the switch, a Glass colour other than Yellow is a choice and is kept', () => {
    mem.set(SWITCHED, '1')
    mem.set('k2.accent.glass', '#3366cc')
    migrateSharedAccentKeys()
    expect(mem.get('k2.accent.glass.obsidian')).toBe('#3366cc')
  })

  it('drops an invalid shared value without copying it', () => {
    mem.set('k2.accent.glass', 'url(evil)')
    migrateSharedAccentKeys()
    expect(accentKeys()).toEqual({})
  })

  it('leaves the pre-paint stamps alone', () => {
    mem.set('k2.accent.stamp.dark', '{}')
    migrateSharedAccentKeys()
    expect(mem.get('k2.accent.stamp.dark')).toBe('{}')
  })
})

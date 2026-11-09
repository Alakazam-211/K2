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

import { migrateRetiredStyleKeys } from './style-id-migration'

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

  it('rewrites a saved bezel, clears its palettes and gaps, keeps the scheme mode', () => {
    mem.set('k2.style', 'bezel')
    mem.set('k2.palette', 'porcelain')
    mem.set('k2.palette.light', 'porcelain')
    mem.set('k2.palette.dark', 'graphite')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.gaps', 'spacious')
    migrateRetiredStyleKeys()
    expect(snapshot()).toEqual({ 'k2.style': 'terminal', 'k2.scheme': 'auto' })
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
      'k2.style': 'terminal',
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

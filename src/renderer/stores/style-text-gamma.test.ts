// Per-style textGamma: stored per (style, scheme); Styles UI owns the slider.
import { describe, it, expect, beforeEach, vi } from 'vitest'

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

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { useStyleStore } from './style'
import {
  TEXT_GAMMA_DARK,
  TEXT_GAMMA_LIGHT,
  textGammaStorageKey,
} from '@/lib/text-gamma'

beforeEach(() => {
  mem.clear()
  useStyleStore.getState().applyStyle({
    styleId: 'square',
    paletteId: 'charcoal',
    schemeMode: 'dark',
    gapsPreset: '',
  })
})

describe('per-style textGamma', () => {
  it('dark style lands on the dark polarity preset', () => {
    expect(useStyleStore.getState().textGamma).toBe(TEXT_GAMMA_DARK)
  })

  it('setTextGamma persists under k2.textGamma.<style>.<scheme> and is live', () => {
    useStyleStore.getState().setTextGamma(1.75)
    expect(useStyleStore.getState().textGamma).toBe(1.75)
    expect(mem.get(textGammaStorageKey('square', 'dark'))).toBe('1.75')
  })

  it('boot / no-op re-apply does not clobber a live slider value', () => {
    useStyleStore.getState().setTextGamma(1.75)
    useStyleStore.getState().applyStyle({})
    expect(useStyleStore.getState().textGamma).toBe(1.75)
  })

  it('switching style loads the other style preset; switching back restores saved value', () => {
    useStyleStore.getState().setTextGamma(1.75)
    // bezel graphite dark → dark preset (no stored override)
    useStyleStore.getState().applyStyle({
      styleId: 'bezel',
      paletteId: 'graphite',
      schemeMode: 'dark',
    })
    expect(useStyleStore.getState().textGamma).toBe(TEXT_GAMMA_DARK)

    // back to square dark → restores the 1.75 override
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
    })
    expect(useStyleStore.getState().textGamma).toBe(1.75)
  })

  it('light scheme loads light preset; dark and light keep independent overrides', () => {
    useStyleStore.getState().setTextGamma(0.9) // square dark
    useStyleStore.getState().applyStyle({ schemeMode: 'light' })
    // square light → paper palette → light preset (no override yet)
    expect(useStyleStore.getState().textGamma).toBe(TEXT_GAMMA_LIGHT)

    useStyleStore.getState().setTextGamma(1.4) // square light override
    expect(mem.get(textGammaStorageKey('square', 'light'))).toBe('1.4')
    expect(mem.get(textGammaStorageKey('square', 'dark'))).toBe('0.9')

    useStyleStore.getState().applyStyle({ schemeMode: 'dark' })
    expect(useStyleStore.getState().textGamma).toBe(0.9)
  })

  it('resetTextGamma clears storage and restores the polarity preset', () => {
    useStyleStore.getState().setTextGamma(1.75)
    useStyleStore.getState().resetTextGamma()
    expect(useStyleStore.getState().textGamma).toBe(TEXT_GAMMA_DARK)
    expect(mem.get(textGammaStorageKey('square', 'dark'))).toBeUndefined()
  })
})

// applyStyle ↔ textGamma preset: boot preserves manual; real switches overwrite.
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
import { useTerminalSettingsStore } from './terminal-settings'
import { TEXT_GAMMA_DARK, TEXT_GAMMA_LIGHT } from '@/lib/text-gamma'

beforeEach(() => {
  mem.clear()
  useStyleStore.getState().applyStyle({
    styleId: 'square',
    paletteId: 'charcoal',
    schemeMode: 'dark',
    gapsPreset: '',
  })
  // After the real switch above, gamma is the dark preset. Set a manual value.
  useTerminalSettingsStore.getState().setTextGamma(1.75)
})

describe('applyStyle textGamma gate', () => {
  it('boot / no-op re-apply does NOT overwrite a manual textGamma', () => {
    expect(useTerminalSettingsStore.getState().textGamma).toBe(1.75)
    // Same resolved identity as current store state.
    useStyleStore.getState().applyStyle({})
    expect(useTerminalSettingsStore.getState().textGamma).toBe(1.75)
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
    })
    expect(useTerminalSettingsStore.getState().textGamma).toBe(1.75)
  })

  it('real style switch overwrites with the new style preset', () => {
    expect(useTerminalSettingsStore.getState().textGamma).toBe(1.75)
    // square/charcoal dark → dark preset 0.7
    useStyleStore.getState().applyStyle({
      styleId: 'bezel',
      paletteId: 'graphite',
      schemeMode: 'dark',
    })
    expect(useTerminalSettingsStore.getState().textGamma).toBe(TEXT_GAMMA_DARK)

    // light palette → light preset 1.2
    useStyleStore.getState().applyStyle({
      styleId: 'bezel',
      paletteId: 'porcelain',
      schemeMode: 'light',
    })
    expect(useTerminalSettingsStore.getState().textGamma).toBe(TEXT_GAMMA_LIGHT)
  })

  it('schemeMode auto OS flip that changes resolved scheme overwrites gamma', () => {
    // Force dark resolved, manual tweak, then switch to light schemeMode
    // (simulates auto→light without needing matchMedia).
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
    })
    useTerminalSettingsStore.getState().setTextGamma(1.9)
    useStyleStore.getState().applyStyle({ schemeMode: 'light' })
    // square light resolves to paper (light bg) → 1.2
    expect(useTerminalSettingsStore.getState().textGamma).toBe(TEXT_GAMMA_LIGHT)
  })
})

import { describe, expect, it, beforeEach, vi } from 'vitest'

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

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))

import {
  migrateTerminalSettings,
  useTerminalSettingsStore,
} from './terminal-settings'
import { TEXT_GAMMA_LIGHT, TEXT_GAMMA_MIN, TEXT_GAMMA_MAX } from '@/lib/text-gamma'

describe('migrateTerminalSettings v5 → v6 (textGamma)', () => {
  it('stamps textGamma: 1.2 on a v5 blob without the field', () => {
    const out = migrateTerminalSettings(
      { renderer: 'kessel', painter: 'webgl', fontSize: 14 },
      5,
    )
    expect(out.textGamma).toBe(TEXT_GAMMA_LIGHT)
    expect(out.renderer).toBe('kessel')
    expect(out.painter).toBe('webgl')
  })

  it('leaves an already-present finite textGamma alone when version >= 6', () => {
    const out = migrateTerminalSettings(
      { renderer: 'kessel', painter: 'dom', textGamma: 0.7 },
      6,
    )
    expect(out.textGamma).toBe(0.7)
  })

  it('replaces non-finite textGamma on pre-v6', () => {
    const out = migrateTerminalSettings(
      { renderer: 'kessel', textGamma: Number.NaN },
      5,
    )
    expect(out.textGamma).toBe(TEXT_GAMMA_LIGHT)
  })
})

describe('setTextGamma clamp', () => {
  beforeEach(() => {
    mem.clear()
    useTerminalSettingsStore.setState({ textGamma: TEXT_GAMMA_LIGHT })
  })

  it('clamps to [0.5, 3]', () => {
    useTerminalSettingsStore.getState().setTextGamma(0.1)
    expect(useTerminalSettingsStore.getState().textGamma).toBe(TEXT_GAMMA_MIN)
    useTerminalSettingsStore.getState().setTextGamma(9)
    expect(useTerminalSettingsStore.getState().textGamma).toBe(TEXT_GAMMA_MAX)
    useTerminalSettingsStore.getState().setTextGamma(0.85)
    expect(useTerminalSettingsStore.getState().textGamma).toBe(0.85)
  })
})

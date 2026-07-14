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
  LINE_HEIGHT_MULT_DEFAULT,
  LINE_HEIGHT_MULT_MIN,
  LINE_HEIGHT_MULT_MAX,
  CHAR_TRACKING_DEFAULT,
  CHAR_TRACKING_MIN,
  CHAR_TRACKING_MAX,
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

describe('migrateTerminalSettings v6 → v7 (line height + tracking)', () => {
  it('stamps defaults on a v6 blob without the fields', () => {
    const out = migrateTerminalSettings(
      { renderer: 'kessel', painter: 'webgl', textGamma: 0.7 },
      6,
    )
    expect(out.lineHeightMultiplier).toBe(LINE_HEIGHT_MULT_DEFAULT)
    expect(out.charTracking).toBe(CHAR_TRACKING_DEFAULT)
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

describe('line height + char tracking clamps', () => {
  beforeEach(() => {
    mem.clear()
    useTerminalSettingsStore.setState({
      lineHeightMultiplier: LINE_HEIGHT_MULT_DEFAULT,
      charTracking: CHAR_TRACKING_DEFAULT,
    })
  })

  it('clamps lineHeightMultiplier', () => {
    useTerminalSettingsStore.getState().setLineHeightMultiplier(0.5)
    expect(useTerminalSettingsStore.getState().lineHeightMultiplier).toBe(LINE_HEIGHT_MULT_MIN)
    useTerminalSettingsStore.getState().setLineHeightMultiplier(3)
    expect(useTerminalSettingsStore.getState().lineHeightMultiplier).toBe(LINE_HEIGHT_MULT_MAX)
    useTerminalSettingsStore.getState().setLineHeightMultiplier(1.24)
    expect(useTerminalSettingsStore.getState().lineHeightMultiplier).toBe(1.24)
  })

  it('clamps charTracking', () => {
    useTerminalSettingsStore.getState().setCharTracking(0.5)
    expect(useTerminalSettingsStore.getState().charTracking).toBe(CHAR_TRACKING_MIN)
    useTerminalSettingsStore.getState().setCharTracking(3)
    expect(useTerminalSettingsStore.getState().charTracking).toBe(CHAR_TRACKING_MAX)
    useTerminalSettingsStore.getState().setCharTracking(1.03)
    expect(useTerminalSettingsStore.getState().charTracking).toBe(1.03)
  })
})

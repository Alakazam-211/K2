// Chords forwarded out of a sealed widget frame (prd-zen-user-widgets-v2
// UW33, TUW4.3): which keys forward, and the host's focus + 500 ms gate.
import { describe, expect, it, vi } from 'vitest'
import {
  createZenChordGate,
  isZenChordNonMac,
  ZEN_FORWARDED_CHORDS,
  zenForwardedChordForKey,
  zenFrameHasFocus,
  type ZenFrameKeyLike,
} from './zen-shortcut'
import { zenThemeCycleDir } from './zen-theme-switch'
import { homeSwitchDigit } from '../home-shortcuts'

function key(code: string, mods: Partial<ZenFrameKeyLike> = {}): ZenFrameKeyLike {
  return { code, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, isTrusted: true, ...mods }
}

function actions() {
  return { exitZen: vi.fn(), switchGarden: vi.fn(), cycleTheme: vi.fn() }
}

describe('zenForwardedChordForKey', () => {
  it('forwards exit on Linux / Windows only (macOS has the native accelerator)', () => {
    const e = key('KeyZ', { ctrlKey: true, altKey: true })
    expect(zenForwardedChordForKey(e, 'linux')).toBe('zen-exit')
    expect(zenForwardedChordForKey(e, 'windows')).toBe('zen-exit')
    expect(zenForwardedChordForKey(key('KeyZ', { ctrlKey: true, metaKey: true }), 'mac')).toBeNull()
    const altGr = { ...e, getModifierState: (k: string) => k === 'AltGraph' }
    expect(zenForwardedChordForKey(altGr, 'linux')).toBeNull()
  })

  it('forwards ⌥⌘1–9 as garden-N and the theme chords, agreeing with the window handlers', () => {
    for (const os of ['mac', 'linux', 'windows'] as const) {
      for (let n = 1; n <= 9; n++) {
        const e = key(`Digit${n}`, { metaKey: true, altKey: true })
        expect(zenForwardedChordForKey(e, os)).toBe(`garden-${n}`)
        expect(homeSwitchDigit(e)).toBe(n)
      }
      const themeMods = os === 'mac' ? { ctrlKey: true, metaKey: true } : { ctrlKey: true, altKey: true }
      const next = { ...key('Period', themeMods), key: '.' }
      const prev = { ...key('Period', { ...themeMods, shiftKey: true }), key: '>' }
      expect(zenForwardedChordForKey(next, os)).toBe('theme-next')
      expect(zenForwardedChordForKey(prev, os)).toBe('theme-prev')
      expect(zenThemeCycleDir(next, os)).toBe(1)
      expect(zenThemeCycleDir(prev, os)).toBe(-1)
    }
    expect(isZenChordNonMac(key('KeyZ', { ctrlKey: true, altKey: true }))).toBe(true)
  })

  it('never forwards untrusted, repeated or ordinary keys', () => {
    const exit = key('KeyZ', { ctrlKey: true, altKey: true })
    expect(zenForwardedChordForKey({ ...exit, isTrusted: false }, 'linux')).toBeNull()
    expect(zenForwardedChordForKey({ ...exit, repeat: true }, 'linux')).toBeNull()
    for (const e of [key('KeyA'), key('Digit1'), key('Digit1', { metaKey: true }), key('Digit0', { metaKey: true, altKey: true }), key('Period')]) {
      expect(zenForwardedChordForKey(e, 'mac')).toBeNull()
      expect(zenForwardedChordForKey(e, 'linux')).toBeNull()
    }
  })
})

describe('createZenChordGate (TUW4.3)', () => {
  it('a forwarded garden-2 from a focused frame switches Garden', () => {
    const a = actions()
    expect(createZenChordGate().run('garden-2', true, 1_000, a)).toBe(true)
    expect(a.switchGarden).toHaveBeenCalledWith(2)
  })

  it('the same from an unfocused frame is ignored', () => {
    const a = actions()
    expect(createZenChordGate().run('garden-2', false, 1_000, a)).toBe(false)
    expect(a.switchGarden).not.toHaveBeenCalled()
  })

  it('two within 200 ms switch once; after 500 ms a chord runs again', () => {
    const a = actions()
    const gate = createZenChordGate()
    expect(gate.run('garden-2', true, 1_000, a)).toBe(true)
    expect(gate.run('garden-3', true, 1_200, a)).toBe(false)
    expect(gate.run('garden-3', true, 1_499, a)).toBe(false)
    expect(gate.run('garden-3', true, 1_500, a)).toBe(true)
    expect(a.switchGarden.mock.calls).toEqual([[2], [3]])
  })

  it('runs exit and theme chords, and refuses anything outside the fixed set', () => {
    const a = actions()
    const gate = createZenChordGate(0)
    expect(gate.run('zen-exit', true, 1, a)).toBe(true)
    expect(gate.run('theme-next', true, 2, a)).toBe(true)
    expect(gate.run('theme-prev', true, 3, a)).toBe(true)
    for (const bad of ['garden-0', 'garden-10', 'zen.exit', 'controls.bind', '', 3, null, { chord: 'zen-exit' }]) {
      expect(gate.run(bad, true, 10, a)).toBe(false)
    }
    expect(a.exitZen).toHaveBeenCalledTimes(1)
    expect(a.cycleTheme.mock.calls).toEqual([[1], [-1]])
    expect(ZEN_FORWARDED_CHORDS).toHaveLength(12)
  })

  it('a refused chord does not use up the 500 ms budget', () => {
    const a = actions()
    const gate = createZenChordGate()
    expect(gate.run('garden-1', false, 1_000, a)).toBe(false)
    expect(gate.run('nope', true, 1_100, a)).toBe(false)
    expect(gate.run('garden-1', true, 1_200, a)).toBe(true)
  })

  it('zenFrameHasFocus compares the document focus', () => {
    const frame = {} as Element
    expect(zenFrameHasFocus(frame, { activeElement: frame })).toBe(true)
    expect(zenFrameHasFocus(frame, { activeElement: {} as Element })).toBe(false)
    expect(zenFrameHasFocus(null, { activeElement: null })).toBe(false)
  })
})

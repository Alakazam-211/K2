// The Home switcher chord (0.43.2): exactly Cmd+Option+1–9, read by
// `e.code`, and clear of the preset (Ctrl+N), row (Cmd+N), pane (Cmd+N on
// Projects) and screenshot (Cmd+Shift+N) chords.

import { describe, expect, it } from 'vitest'
import { HOME_SWITCH_BINDING, HOME_SWITCH_LIMIT, homeSwitchCombo, homeSwitchDigit } from './home-shortcuts'
import { paneSwitchDigit } from '@/components/Projects/project-tabs'

const none = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false }

describe('homeSwitchDigit', () => {
  it('is Cmd+Option+1–9 (by e.code), the Agents switch chord', () => {
    expect(HOME_SWITCH_BINDING).toMatchObject({ metaKey: true, altKey: true, ctrlKey: false, shiftKey: false })
    for (let n = 1; n <= 9; n++) {
      expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, code: `Digit${n}` })).toBe(n)
    }
    expect(HOME_SWITCH_LIMIT).toBe(9)
  })

  it('refuses Digit0, non-digits and any other modifier set', () => {
    expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, code: 'Digit0' })).toBeNull()
    expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, code: 'KeyA' })).toBeNull()
    expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, code: 'Numpad1' })).toBeNull()
    // Preset launch: Ctrl+N.
    expect(homeSwitchDigit({ ...none, ctrlKey: true, code: 'Digit1' })).toBeNull()
    // Home row / pinned: Cmd+N.
    expect(homeSwitchDigit({ ...none, metaKey: true, code: 'Digit1' })).toBeNull()
    // Rosson's first ask, macOS screenshots: Cmd+Shift+N.
    expect(homeSwitchDigit({ ...none, metaKey: true, shiftKey: true, code: 'Digit3' })).toBeNull()
    expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, shiftKey: true, code: 'Digit1' })).toBeNull()
    expect(homeSwitchDigit({ ...none, metaKey: true, altKey: true, ctrlKey: true, code: 'Digit1' })).toBeNull()
    expect(homeSwitchDigit({ ...none, altKey: true, code: 'Digit1' })).toBeNull()
    expect(homeSwitchDigit({ ...none, ctrlKey: true, altKey: true, code: 'Digit1' })).toBeNull()
  })

  it('never overlaps the Projects pane chord (plain Cmd+N)', () => {
    for (let n = 1; n <= 9; n++) {
      const sw = { ...none, metaKey: true, altKey: true, code: `Digit${n}`, key: String(n) }
      expect(homeSwitchDigit(sw)).toBe(n)
      expect(paneSwitchDigit(sw)).toBeNull()
      const pane = { ...none, metaKey: true, code: `Digit${n}`, key: String(n) }
      expect(paneSwitchDigit(pane)).toBe(n)
      expect(homeSwitchDigit(pane)).toBeNull()
    }
  })

  it('paints with the Agents pinned header glyphs', () => {
    expect(homeSwitchCombo(1)).toBe('⌥⌘1')
    expect(homeSwitchCombo(9)).toBe('⌥⌘9')
  })
})

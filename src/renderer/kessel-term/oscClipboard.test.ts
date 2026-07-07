import { describe, expect, it } from 'vitest'

import { shouldApplyOsc52, shouldApplyOsc52Frame } from './oscClipboard'

describe('shouldApplyOsc52', () => {
  it('applies the first payload', () => {
    expect(shouldApplyOsc52(null, 'quokka')).toBe(true)
  })

  it('dedupes a repainted identical payload', () => {
    // claude re-emits the same OSC 52 on every repaint while a
    // selection stays live — 5× observed per selection.
    expect(shouldApplyOsc52('quokka', 'quokka')).toBe(false)
  })

  it('applies a changed payload', () => {
    expect(shouldApplyOsc52('quokka', 'wombat')).toBe(true)
  })

  it('re-applies an earlier payload after a different one (A→B→A)', () => {
    expect(shouldApplyOsc52('wombat', 'quokka')).toBe(true)
  })

  it('never applies an empty payload', () => {
    expect(shouldApplyOsc52(null, '')).toBe(false)
    expect(shouldApplyOsc52('quokka', '')).toBe(false)
  })
})

describe('shouldApplyOsc52Frame', () => {
  // The full per-frame decision: DOM-truth visibility gate (the pane
  // the user is looking at and interacting with) + the empty/dedupe
  // payload policy. Deliberately NOT the pane's claim mirror
  // (lastSentActiveRef) — gating on it stranded remote viewers'
  // copies when the mirror diverged from the daemon's input-flipped
  // active_subscriber.
  const engaged = {
    visible: true,
    paneFocused: true,
    windowFocused: true,
    lastApplied: null,
  }

  it('applies for the visible, pane-focused, window-focused pane', () => {
    expect(shouldApplyOsc52Frame({ ...engaged, incoming: 'quokka' })).toBe(true)
  })

  it('refuses when the pane is hidden', () => {
    expect(
      shouldApplyOsc52Frame({ ...engaged, visible: false, incoming: 'quokka' }),
    ).toBe(false)
  })

  it('refuses when the pane is not shadow-input focused', () => {
    // A passive second pane in the same (focused) window — an old
    // broadcast-to-all daemon still must not stomp its clipboard.
    expect(
      shouldApplyOsc52Frame({
        ...engaged,
        paneFocused: false,
        incoming: 'quokka',
      }),
    ).toBe(false)
  })

  it('refuses when the OS window is blurred', () => {
    // A passive second window / machine viewing the same session.
    expect(
      shouldApplyOsc52Frame({
        ...engaged,
        windowFocused: false,
        incoming: 'quokka',
      }),
    ).toBe(false)
  })

  it('still dedupes repainted identical payloads', () => {
    expect(
      shouldApplyOsc52Frame({
        ...engaged,
        lastApplied: 'quokka',
        incoming: 'quokka',
      }),
    ).toBe(false)
  })

  it('still refuses empty payloads', () => {
    expect(shouldApplyOsc52Frame({ ...engaged, incoming: '' })).toBe(false)
  })

  it('applies a changed payload for the engaged pane', () => {
    expect(
      shouldApplyOsc52Frame({
        ...engaged,
        lastApplied: 'quokka',
        incoming: 'wombat',
      }),
    ).toBe(true)
  })
})

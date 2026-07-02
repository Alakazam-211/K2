import { describe, expect, it } from 'vitest'

import { shouldApplyOsc52 } from './oscClipboard'

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

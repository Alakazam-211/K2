// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest'
import { BothShiftsDetector, installBothShifts } from './both-shifts'

describe('both Shift keys together', () => {
  it('fires when the second Shift goes down while the first is held', () => {
    const d = new BothShiftsDetector()
    expect(d.down('ShiftLeft')).toBe(false)
    expect(d.down('ShiftRight')).toBe(true)
  })

  it('fires in either order, and once per press', () => {
    const d = new BothShiftsDetector()
    expect(d.down('ShiftRight')).toBe(false)
    expect(d.down('ShiftLeft')).toBe(true)
    // Releasing one and pressing it again while the other is still held is the same press.
    d.up('ShiftLeft')
    expect(d.down('ShiftLeft')).toBe(false)
    d.up('ShiftLeft')
    d.up('ShiftRight')
    expect(d.down('ShiftLeft')).toBe(false)
    expect(d.down('ShiftRight')).toBe(true)
  })

  it('never fires for Shift+typing, one Shift, or key repeat', () => {
    const d = new BothShiftsDetector()
    expect(d.down('ShiftLeft')).toBe(false)
    expect(d.down('KeyA')).toBe(false)
    expect(d.down('ShiftRight')).toBe(false)
    d.up('ShiftLeft')
    d.up('ShiftRight')
    expect(d.down('ShiftLeft')).toBe(false)
    expect(d.down('ShiftLeft', true)).toBe(false)
    d.up('ShiftLeft')
    expect(d.down('ShiftRight')).toBe(false)
  })

  it('forgets held keys when the window loses focus', () => {
    const d = new BothShiftsDetector()
    expect(d.down('ShiftLeft')).toBe(false)
    d.reset()
    expect(d.down('ShiftRight')).toBe(false)
  })

  it('listens on the window in the capture phase and cleans up', () => {
    const onChord = vi.fn()
    const stop = installBothShifts(onChord)
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'ShiftLeft' }))
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'ShiftRight' }))
    expect(onChord).toHaveBeenCalledTimes(1)
    window.dispatchEvent(new KeyboardEvent('keyup', { code: 'ShiftLeft' }))
    window.dispatchEvent(new KeyboardEvent('keyup', { code: 'ShiftRight' }))
    stop()
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'ShiftLeft' }))
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'ShiftRight' }))
    expect(onChord).toHaveBeenCalledTimes(1)
  })
})

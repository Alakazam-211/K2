// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { CALL_BAR_FIT_STEPS, fitCallBar } from './call-bar-fit'

/** A top bar `room` CSS px wide holding 500 px of other items and the call's
 *  slim bar, which measures per step: the name goes at `no-name` (-130), the
 *  time at `no-time` (-40), the action's word at `icons` (-70), the deafen,
 *  wave and camera buttons at `minimal` (-80). */
function topBar(room: number): { bar: HTMLElement; call: HTMLElement } {
  const bar = document.createElement('div')
  const call = document.createElement('div')
  bar.appendChild(call)
  const step = (): string => call.dataset.callBarFit ?? 'full'
  const callWidth = (): number => ({ full: 440, 'no-name': 310, 'no-time': 270, icons: 200, minimal: 120 })[step()] ?? 440
  Object.defineProperty(bar, 'clientWidth', { get: () => room })
  Object.defineProperty(bar, 'scrollWidth', { get: () => Math.max(room, 500 + callWidth()) })
  return { bar, call }
}

describe("the call's slim bar in a narrow top bar", () => {
  it('keeps everything with room for it', () => {
    const { bar, call } = topBar(1440)
    expect(fitCallBar(bar, call)).toBe('full')
    expect(call.dataset.callBarFit).toBe('full')
  })

  it('drops the name first, then the time, then the action word, before the top bar overflows', () => {
    const a = topBar(820)
    expect(fitCallBar(a.bar, a.call)).toBe('no-name')
    const b = topBar(780)
    expect(fitCallBar(b.bar, b.call)).toBe('no-time')
    const c = topBar(710)
    expect(fitCallBar(c.bar, c.call)).toBe('icons')
  })

  it('as the last resort keeps only mute, hang up and the action, and never more than the bar holds', () => {
    const { bar, call } = topBar(630)
    expect(fitCallBar(bar, call)).toBe('minimal')
    expect(bar.scrollWidth).toBeLessThanOrEqual(bar.clientWidth)
  })

  it('comes back as the window widens', () => {
    const narrow = topBar(650)
    expect(fitCallBar(narrow.bar, narrow.call)).toBe('minimal')
    const wide = topBar(1200)
    wide.call.dataset.callBarFit = 'minimal'
    expect(fitCallBar(wide.bar, wide.call)).toBe('full')
  })

  it('steps in this order, the full bar first', () => {
    expect(CALL_BAR_FIT_STEPS).toEqual(['full', 'no-name', 'no-time', 'icons', 'minimal'])
  })
})

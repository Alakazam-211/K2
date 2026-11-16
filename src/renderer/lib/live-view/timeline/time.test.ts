// Time rules: rates and frames in ticks, timecode counted on the frame
// grid, timecode the pack gives shown exactly as given (drop-frame ";"
// forms never reformatted), durations in words, and the ruler's density.

import { describe, expect, it } from 'vitest'
import { big, dfTc, durText, framesOf, pairOf, PER_S, rateOf, rulerSteps, tcAsGiven, tcText } from './time'

describe('rates and ticks', () => {
  it('reads a rate as the pack writes it, with ticks per frame and the timecode base', () => {
    expect(rateOf('24/1')).toMatchObject({ frame: 10584000000n, base: 24, fps: 24 })
    expect(rateOf('30000/1001')).toMatchObject({ frame: 8475667200n, base: 30 })
    expect(rateOf('60000/1001')?.base).toBe(60)
    expect(rateOf('48000/1')?.frame).toBe(5292000n)
    for (const bad of ['24', '0/1', '24/0', '', null, undefined, 'x/y']) expect(rateOf(bad)).toBeNull()
  })

  it('reads ticks as BigInt only from a decimal string, never through a number', () => {
    expect(big('12853823472000000')).toBe(12853823472000000n)
    for (const bad of ['-0', '01', '1.5', '', 12, null]) expect(big(bad)).toBeNull()
    expect(pairOf('0,3302208000000')).toEqual([0n, 3302208000000n])
    expect(pairOf('1,2,3')).toBeNull()
  })

  it('counts frames from ticks, keeping a fraction off the grid', () => {
    const fr = rateOf('24/1')!.frame!
    expect(framesOf(48n * fr, fr)).toBe(48)
    expect(framesOf(fr / 2n, fr)).toBe(0.5)
  })
})

describe('timecode', () => {
  const R24 = rateOf('24/1')!
  const fr = R24.frame!

  it('counts from the start timecode on the frame grid only', () => {
    expect(tcText(0n, R24, 3600n * 24n * fr)).toBe('01:00:00:00')
    expect(tcText(25n * fr, R24, 0n)).toBe('00:00:01:01')
    expect(tcText(fr / 2n, R24, 0n)).toBeNull()
    expect(tcText(10n * fr, rateOf('25/1'), 0n)).toBeNull() // 24 fps ticks aren't on the 25 fps grid
  })

  it('shows timecode the pack gives exactly as given: the drop-frame “;” form is never reformatted', () => {
    expect(tcAsGiven('01:00:10;10')).toBe('01:00:10;10')
    expect(tcAsGiven('01:00:14:00')).toBe('01:00:14:00')
    expect(tcAsGiven('1:00:10;10')).toBeNull()
    expect(tcAsGiven(36000)).toBeNull()
  })

  it('writes drop-frame timecode with frames 0 and 1 dropped each minute but every tenth', () => {
    expect(dfTc(0)).toBe('00:00:00;00')
    expect(dfTc(1799)).toBe('00:00:59;29')
    expect(dfTc(1800)).toBe('00:01:00;02')
    expect(dfTc(17982)).toBe('00:10:00;00')
    expect(dfTc(Math.floor(((3600 + 10.5) * 30000) / 1001))).toMatch(/^01:00:10;\d\d$/)
  })
})

describe('durations and the ruler', () => {
  it('says lengths in words: frames, seconds to one place, minutes', () => {
    expect(durText(6, 24)).toBe('6 frames')
    expect(durText(1, 24)).toBe('1 frame')
    expect(durText(36, 24)).toBe('1.5s')
    expect(durText(24, 24)).toBe('1s')
    expect(durText(-24 * 124, 24)).toBe('2m 4s')
  })

  it('spaces labels at least 110 px apart and minor ticks at least 12 px, the minor step dividing the label step', () => {
    for (const ppf of [0.004, 0.05, 0.42, 0.85, 1.1, 4, 16]) {
      const r = rulerSteps(ppf, 24)
      expect(r.label % r.minor).toBe(0)
      if (r.label < 86400) expect(r.label * ppf).toBeGreaterThanOrEqual(110)
      if (r.minor !== r.label) expect(r.minor * ppf).toBeGreaterThanOrEqual(12)
    }
    expect(rulerSteps(0.85, 24).label).toBe(240)
    expect(rulerSteps(16, 30).label).toBe(10)
  })

  it('keeps a tick a tick: 1/254,016,000,000 s', () => {
    expect(PER_S).toBe(254016000000n)
  })
})

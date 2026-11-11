// The low-latency VUI (z3mbpZ 11-10): the SPS VideoToolbox writes has no VUI,
// so WebKit's decoder held frames; the rewrite adds the bitstream
// restriction (no reorder, buffering = the reference frames) and leaves the
// rest of the SPS bit for bit.

import { describe, expect, it } from 'vitest'
import { spsRestriction, spsWithLowLatencyVui, withLowLatencyVui } from './h264-vui'
import { codecFromAnnexB, splitNals } from './stream-video'

const hex = (s: string): Uint8Array => Uint8Array.from(s.match(/../g)!.map((b) => parseInt(b, 16)))
// The helper's SPS and PPS as captured on z3mbpZ (Constrained High, level
// 3.2, 1 reference frame, no VUI).
const SPS = hex('27640c20ac568084091ea540')
const PPS = hex('28ee3cb0')
const IDR = hex('25b8040ffff0')

describe('h264-vui', () => {
  it('the captured SPS has no restriction; the rewrite adds reorder 0 and buffering 1', () => {
    expect(spsRestriction(SPS)).toBeNull()
    const fixed = spsWithLowLatencyVui(SPS)
    expect(fixed).not.toBe(SPS)
    expect(spsRestriction(fixed)).toEqual({ reorder: 0, buffering: 1 })
    // Profile, constraints and level are untouched (the codec string).
    expect(Array.from(fixed.subarray(0, 4))).toEqual(Array.from(SPS.subarray(0, 4)))
  })

  it('an SPS that already has a VUI, a non-SPS NAL and garbage are left alone', () => {
    const fixed = spsWithLowLatencyVui(SPS)
    expect(spsWithLowLatencyVui(fixed)).toBe(fixed)
    expect(spsWithLowLatencyVui(PPS)).toBe(PPS)
    const cut = SPS.subarray(0, 5)
    expect(spsWithLowLatencyVui(cut)).toBe(cut)
  })

  it('an access unit keeps its PPS and slices and its codec string; one with no SPS is the same array', () => {
    const au = Uint8Array.from([0, 0, 0, 1, ...SPS, 0, 0, 0, 1, ...PPS, 0, 0, 1, ...IDR])
    const out = withLowLatencyVui(au)
    const nals = splitNals(out)
    expect(nals.map((n) => n[0] & 0x1f)).toEqual([7, 8, 5])
    expect(spsRestriction(nals[0])).toEqual({ reorder: 0, buffering: 1 })
    expect(Array.from(nals[1])).toEqual(Array.from(PPS))
    expect(Array.from(nals[2])).toEqual(Array.from(IDR))
    expect(codecFromAnnexB(out)).toBe(codecFromAnnexB(au))
    const delta = Uint8Array.from([0, 0, 0, 1, 0x21, 0x9a, 0x00])
    expect(withLowLatencyVui(delta)).toBe(delta)
  })

  it('emulation prevention: the rewritten SPS never carries 00 00 0x (x ≤ 3) raw', () => {
    const fixed = spsWithLowLatencyVui(SPS)
    for (let i = 0; i + 2 < fixed.length; i++) {
      if (fixed[i] === 0 && fixed[i + 1] === 0) expect(fixed[i + 2]).toBeGreaterThan(3)
    }
  })
})

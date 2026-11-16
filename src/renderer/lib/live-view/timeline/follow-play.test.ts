// Following a playhead on the timeline (T5), pure: following someone who
// is playing extrapolates their playhead from the time position's at_ms and
// rate (never a stream of seeks); when the one you follow stops or leaves,
// following ends with the editors' finished words; the sample's scripted
// follows; and what the say row points at.

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { DEFAULT_SEQUENCE, finishedWords, followNow, sayAtOf, scriptedFollow, scriptedLoop, stoppedAt } from './follow-play'
import { monitorView } from './monitor-view'
import { WATCH } from './scenarios'
import { simulate } from './sim'
import { nowOf } from './snapshot'
import { ATLAS, CTX, FACTS, SAM } from './world'

const at = (label: string): number => WATCH.steps.findIndex((s) => s.label === label)
const state = (label: string) => simulate(WATCH, at(label), CTX)

describe('following while they play', () => {
  it('extrapolates the followed playhead from at_ms and rate, not a stream of seeks', () => {
    const S = state('follow Sam')
    const M = modelOf(S.B)
    const tp = S.peers[SAM].timePosition!
    expect(tp.state).toBe('playing')
    const f0 = monitorView({ S, M, facts: FACTS, now: tp.at_ms, whose: SAM, local: null }).head
    expect(f0.self).toBe(false)
    expect(f0.playing).toBe(true)
    const f2 = monitorView({ S, M, facts: FACTS, now: tp.at_ms + 2000, whose: SAM, local: null }).head
    expect(f2.f - f0.f).toBeCloseTo(2 * 24, 5)
    // at twice the rate, twice as far
    const S2 = { ...S, peers: { ...S.peers, [SAM]: { ...S.peers[SAM], timePosition: { ...tp, rate: '2' } } } }
    const g2 = monitorView({ S: S2, M, facts: FACTS, now: tp.at_ms + 2000, whose: SAM, local: null }).head
    expect(g2.f - f0.f).toBeCloseTo(4 * 24, 5)
  })

  it('the clock runs on from the snapshot’s own time by the wall time since it came', () => {
    expect(followNow(730000, 1000, 1000)).toBe(730000)
    expect(followNow(730000, 1000, 3500)).toBe(732500)
    expect(followNow(730000, 1000, 900)).toBe(730000)
  })

  it('a paused playhead stays put however long you watch', () => {
    const S = state('back to Atlas')
    const M = modelOf(S.B)
    const a = monitorView({ S, M, facts: FACTS, now: nowOf(S), whose: ATLAS, local: null }).head
    const b = monitorView({ S, M, facts: FACTS, now: nowOf(S) + 60000, whose: ATLAS, local: null }).head
    expect(a.playing).toBe(false)
    expect(b.f).toBe(a.f)
  })
})

describe('when the one you follow stops or leaves', () => {
  it('an agent stops when it lets go of its hold or its turn ends', () => {
    expect(stoppedAt(state('Atlas lets go'), ATLAS)).toBe(true)
    expect(stoppedAt(state('marker'), ATLAS)).toBe(false)
    expect(stoppedAt(state('Atlas lets go'), SAM)).toBe(false)
  })

  it('the finished words, as the editors say them, worded for the timeline', () => {
    expect(finishedWords({ who: ATLAS, name: 'Atlas', why: 'stopped', home: 40 })).toEqual({ text: 'Atlas stopped. Stopped following.', back: 'Back to my place', again: 'Follow again' })
    expect(finishedWords({ who: SAM, name: 'Sam', why: 'left', home: 40 })).toEqual({ text: 'Sam left the file. Stopped following.', back: 'Back to my place', again: null })
  })
})

describe('the sample’s script', () => {
  it('reads follow, back and stop from the step', () => {
    expect(scriptedFollow(state('follow Atlas'), ATLAS)).toEqual({ type: 'follow', who: ATLAS })
    expect(scriptedFollow(state('follow Sam'), ATLAS)).toEqual({ type: 'follow', who: SAM })
    expect(scriptedFollow(state('you scrub'), ATLAS)).toEqual({ type: 'away', who: ATLAS })
    expect(scriptedFollow(state('play'), ATLAS)).toEqual({ type: 'stop' })
    expect(scriptedFollow(state('cut'), ATLAS)).toBeNull()
    // their leaving ends following, not the script
    expect(scriptedFollow(state('Sam left'), ATLAS)).toBeNull()
  })

  it('a scripted loop runs from 1 s before the change to 1 s after', () => {
    const S = state('loop it')
    const l = scriptedLoop(S, modelOf(S.B), 'Atlas')
    expect(l).toMatchObject({ f0: 312 - 24, f1: 576 + 24, who: 'Atlas', verb: 'clip trim' })
    expect(scriptedLoop(state('move'), modelOf(state('move').B), 'Atlas')).toBeNull()
  })
})

describe('what the say row points at', () => {
  it('the clip under the playhead by its unit key, with the time as the playhead’s time position in the sequence', () => {
    const S = state('back to Atlas')
    const M = modelOf(S.B)
    const v = monitorView({ S, M, facts: FACTS, now: nowOf(S), whose: ATLAS, local: null })
    const a = sayAtOf(M, v, DEFAULT_SEQUENCE)
    expect(a.address).toBe(v.picture!.it!.key)
    expect(a.address).toMatch(/^@item:\d+$/)
    expect(a.label).toBe(v.picture!.it!.name)
    expect(a.time).toEqual({ at: String(BigInt(Math.round(v.head.f)) * M.FR), in: DEFAULT_SEQUENCE })
    expect(a.title).toBe(`${a.label} at ${M.tcF(Math.round(v.head.f))}`)
  })

  it('a gap points at its sequence, the time carrying where; no time address', () => {
    const S = state('back to Atlas')
    const M = modelOf(S.B)
    const v = monitorView({ S, M, facts: FACTS, now: nowOf(S), whose: ATLAS, local: null })
    const gap = { ...v, picture: { kind: 'gap' as const, f: v.head.f, skipped: [] } }
    const g = sayAtOf(M, gap, DEFAULT_SEQUENCE)
    expect(g.label).toBe('a gap')
    expect(g.address).toBe(DEFAULT_SEQUENCE)
    expect(g.time).toEqual({ at: String(BigInt(Math.round(v.head.f)) * M.FR), in: DEFAULT_SEQUENCE })
    expect(JSON.stringify(g)).not.toContain('@time')
    // a gap in a second sequence says which
    const g2 = sayAtOf(M, gap, '@seq:S2')
    expect(g2.address).toBe('@seq:S2')
    expect(g2.time?.in).toBe('@seq:S2')
  })
})

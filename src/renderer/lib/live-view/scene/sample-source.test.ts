// The scene's sample source: a LiveSource with the mock's 24 scenarios, each
// step the shell's snapshot (people, state, changes, messages, notices) and
// the scene's own state for the panel.

import { afterEach, describe, expect, it } from 'vitest'
import { liveTabState } from '../shared/state'
import { SampleSceneSource, isSceneSnapshot, sceneScenarios } from './sample-source'

let src: SampleSceneSource | null = null
afterEach(() => src?.close())

const open = (scenario: string, step = 0): SampleSceneSource => {
  src = new SampleSceneSource({ autoplay: false, scenario })
  src.player.goTo(step)
  return src
}
const chip = (s: SampleSceneSource): string => {
  const n = s.snapshot()
  return liveTabState({ tier: n.tier, connection: n.connection, outsideSave: n.outsideSave, behind: n.behind })
}

describe('SampleSceneSource', () => {
  it('plays all 24 of the mock’s scenarios, grouped as its player groups them', () => {
    const list = sceneScenarios()
    expect(list.map((s) => s.id)).toEqual(['turn', 'who', 'keys', 'follow', 'two', 'refuse', 'limits', 'first', 'failed', 'failfirst', 'nopic', 'nomod', 'turntable', 'large', 'lost', 'empty', 'blob', 'O1', 'O2', 'O3', 'O4', 'O5', 'pin-outliner', 'pin-rig'])
    expect(new Set(list.map((s) => s.group)).size).toBe(5)
    expect(list.reduce((n, s) => n + s.steps.length, 0)).toBe(98)
    const s = open('turn')
    expect(s.kind).toBe('sample')
    expect(s.player.scenarioList()).toHaveLength(24)
  })

  it('each step is the shell’s snapshot plus the scene', () => {
    const s = open('who')
    const snap = s.snapshot()
    expect(isSceneSnapshot(snap)).toBe(true)
    expect(snap.scene.scenario).toBe('who')
    expect(snap.tier).toBe('live')
    expect(snap.agent).toBe('agent:atlas')
    expect(snap.people.map((p) => p.name)).toEqual(['Sam', 'Atlas', 'Brook', 'You'])
    expect(snap.people.find((p) => p.self)?.id).toBe('person:ada')
    expect(snap.activity).toEqual({ who: 'agent:atlas', text: 'is arranging the dining set' })
  })

  it('first open is loading; too large opens read only', () => {
    expect(open('first').snapshot().tier).toBe('loading')
    expect(chip(open('blob'))).toBe('read-only')
  })

  it('the connection: reconnecting, then can’t reconnect, then back', () => {
    const s = open('lost', 1)
    expect(s.snapshot().connection).toBe('reconnecting')
    s.player.step()
    expect(s.snapshot().connection).toBe('cant')
    s.player.step()
    expect(s.snapshot().connection).toBe('back')
    expect(s.snapshot().caughtUp).toBe(0)
  })

  it('the outside saves arrive as the kit’s frames; declined and missing pause edits', () => {
    const o2 = open('O2', 1).snapshot().outsideSave
    expect(o2).toMatchObject({ outcome: 'kept_both', copy: { name: 'living-room (conflict Atlas 2026-11-13 142210).blend' } })
    expect(chip(open('O4', 1))).toBe('paused')
    expect(chip(open('O5', 1))).toBe('missing')
  })

  it('a failed still keeps the picture one change behind', () => {
    expect(chip(open('failed', 1))).toBe('live-behind')
    expect(chip(open('failed', 5))).toBe('live')
  })

  it('a refusal shows on its step, in names', () => {
    expect(open('refuse', 1).snapshot().refusal).toMatchObject({ actor: 'Brook', verb: 'rename', target: 'Chair.002', holder: 'Atlas' })
    expect(open('refuse', 2).snapshot().refusal).toBeNull()
  })

  it('what was said and the reply are messages', () => {
    const m = open('follow', 4).snapshot().messages
    expect(m.map((x) => [x.from, x.to, x.text])).toEqual([
      ['person:ada', 'agent:atlas', 'Could the table sit a little closer to the window?'],
      ['agent:atlas', 'person:ada', 'On it.'],
    ])
    expect(m[0].address).toEqual({ label: 'Table', address: '@obj:10' })
  })
})

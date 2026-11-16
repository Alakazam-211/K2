// The timeline's sample source: the mock's sample turn and scenarios, folded,
// reaching the shared shell as its snapshots (Watching, Changes, Messages,
// the header's sentence, the notices).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { personById } from '../shared/people'
import { sampleSnapshot, TimelineSampleSource, timelineBannerScenarios } from './sample-source'
import { SCENARIOS, WATCH } from './scenarios'
import type { TimelineSnapshot } from './snapshot'

let src: TimelineSampleSource
beforeEach(() => {
  vi.useFakeTimers()
  src = new TimelineSampleSource({ replyMs: 1000 })
})
afterEach(() => {
  src.close()
  vi.useRealTimers()
})

const snap = (): TimelineSnapshot => src.snapshot() as TimelineSnapshot
const goTo = (scenario: string, label: string): TimelineSnapshot => {
  src.player.setScenario(scenario)
  const steps = timelineBannerScenarios().find((s) => s.id === scenario)!.steps
  src.player.goTo(steps.findIndex((s) => s.label === label))
  return snap()
}
const doing = (s: TimelineSnapshot, id: string): string | null | undefined => personById(s.people, id)?.doing

describe('the banner', () => {
  it('opens on the watched edit; seven scenarios (the pinned file and a long edit for the lanes), each step a whole snapshot', () => {
    expect(src.kind).toBe('sample')
    expect(src.player.status()).toMatchObject({ scenario: 'watch', position: 1, length: WATCH.steps.length })
    expect(src.player.scenarioList().map((s) => s.label)).toEqual(['First open', 'Watching an edit', 'Two agents', 'Outside saves', 'Connection', 'Pinned file', 'Long edit'])
    expect(snap().timeline.scenario).toBe('watch')
    expect(snap().timeline.model.count).toBe(33)
  })

  it('every step of every mock scenario folds to a snapshot with no stray words', () => {
    for (const sc of SCENARIOS)
      sc.steps.forEach((_, i) => {
        const s = sampleSnapshot(sc, i)
        const words = [s.activity?.text, ...s.changes.map((c) => c.text), ...s.messages.map((m) => m.text), ...s.people.flatMap((p) => [p.doing, p.doingTip, p.place])].filter((x): x is string => typeof x === 'string')
        for (const w of words) expect(w, `${sc.id} ${i}`).not.toMatch(/undefined|NaN|null|MPC-E|@trk|@item|OTIO|Premiere|Resolve|\bE\d{3}\b/)
      })
  })
})

describe('the watched edit, in the panel', () => {
  it('first: Atlas is here and hasn’t changed anything; Sam is at a clip', () => {
    const s = snap()
    expect(s.tier).toBe('live')
    expect(s.agent).toBe('agent:atlas')
    expect(s.activity).toEqual({ who: 'agent:atlas', text: 'hasn’t changed anything yet' })
    expect(s.changes).toEqual([])
    expect(doing(s, 'agent:atlas')).toBe('in the file, not editing')
    expect(doing(s, 'person:sam')).toBe('at Interview B · V1')
    expect(personById(s.people, 'person:ada')).toMatchObject({ self: true, name: 'You' })
  })

  it('a hold: Watching says what Atlas holds, the reason only in the tooltip', () => {
    const s = goTo('watch', 'Atlas holds')
    const atlas = personById(s.people, 'agent:atlas')!
    expect(atlas.doing).toBe('holding 2 clips')
    expect(atlas.doingTip).toBe('holding 2 clips: “tightening the interview open”')
    expect(atlas.editing).toBe(true)
    expect(s.activity).toEqual({ who: 'agent:atlas', text: 'holds 2 clips: “tightening the interview open”' })
  })

  it('edits land in Changes with one verb each, the place out of the sentence', () => {
    const s = goTo('watch', 'marker')
    expect(s.changes.map((c) => `${c.who} ${c.text}`)).toEqual([
      'agent:atlas trimmed the start of Interview B by 1s',
      'agent:atlas moved B-roll 03 to 01:00:14:00',
      'agent:atlas renamed B-roll 03 to Harbour at dawn',
      'agent:atlas turned off Harbour at dawn',
      'agent:atlas dropped a marker “Check the cut”',
    ])
    expect(s.changes[0]).toMatchObject({ verb: 'trimmed', target: '@item:2' })
    expect(s.activity).toEqual({ who: 'agent:atlas', text: 'dropped a marker “Check the cut”' })
  })

  it('a refusal: the notice and a quiet Changes row say it in plain words, no code', () => {
    const s = goTo('watch', 'refused')
    expect(s.refusal).toMatchObject({ actor: 'Atlas', verb: 'trim', target: 'Interview B', holder: 'Brook' })
    expect(s.changes.at(-1)).toMatchObject({ who: 'agent:atlas', text: 'couldn’t trim Interview B: Brook is holding it', tone: 'refused' })
    expect(doing(s, 'agent:brook')).toBe('holding 1 clip')
  })

  it('messages: “You → Atlas” pointing at the clip, and Atlas’s reply', () => {
    const s = goTo('watch', 'Atlas answers')
    expect(s.messages.map((m) => [m.from, m.to, m.text, m.address?.label])).toEqual([
      ['person:ada', 'agent:atlas', 'Is B-roll 03 better over this line?', 'Interview B'],
      ['agent:atlas', 'person:ada', 'On it: I’ll try it there once Brook lets go of Interview B.', undefined],
    ])
    expect(s.messages[0].address?.address).toBe('@item:2 @ 01:00:14:00')
  })

  it('a message that doesn’t go through fails the say row and isn’t listed', () => {
    const s = goTo('watch', 'not sent')
    expect(s.hints?.sayFails).toBe(true)
    expect(s.messages).toHaveLength(2)
  })

  it('Sam leaves the file: Watching drops Sam', () => {
    expect(personById(goTo('watch', 'Sam left').people, 'person:sam')).toBeUndefined()
  })

  it('an outside save kept both: the notice’s frame, and the change from the file in Changes', () => {
    const s = goTo('watch', 'outside save')
    expect(s.outsideSave).toMatchObject({ outcome: 'kept_both', rev: 36, kept: 35, copy: { name: 'rough-cut (conflict Atlas 2026-11-13 142210).otio' } })
    expect(s.changes.at(-1)).toMatchObject({ who: 'file', tone: 'outside', text: '· 2 clips changed, 1 marker removed', at_ms: s.outsideSave!.at_ms })
  })
})

describe('outside saves and holds', () => {
  it('each outcome reaches the notice', () => {
    src.player.setScenario('saves')
    const got: (string | undefined)[] = []
    for (let i = 0; i < 4; i++) {
      src.player.goTo(i)
      got.push(snap().outsideSave?.outcome)
    }
    expect(got).toEqual(['ingested', 'kept_both', 'declined', 'missing'])
  })

  it('a hold that ends outside K2 says so in the header, Watching and Changes', () => {
    const s = goTo('saves', 'A hold ended outside')
    expect(s.activity).toEqual({ who: null, text: 'Brook’s hold on Music ended: the track was renamed outside K2' })
    expect(doing(s, 'agent:brook')).toBe('its hold on Music ended: the track was renamed outside K2')
    expect(s.changes.map((c) => `${c.who} ${c.text}`)).toContain('agent:brook lost its hold on Music: the track was renamed outside K2')
  })
})

describe('the connection and opening', () => {
  it('reconnecting, trying, back with one change, can’t reconnect, a failed retry, back with none', () => {
    src.player.setScenario('connection')
    const seen: unknown[] = []
    for (let i = 0; i < 8; i++) {
      src.player.goTo(i)
      const s = snap()
      seen.push([s.connection, s.caughtUp ?? null, s.retried ?? null])
    }
    expect(seen).toEqual([
      [null, null, null],
      ['reconnecting', null, null],
      ['trying', null, null],
      ['back', 1, null],
      ['reconnecting', null, null],
      ['cant', null, false],
      ['cant', null, true],
      ['back', 0, null],
    ])
  })

  it('first open loads, then is live with everyone', () => {
    src.player.setScenario('open')
    expect(snap().tier).toBe('loading')
    expect(snap().people.map((p) => p.id)).toEqual(['person:ada'])
    src.player.goTo(src.player.status().length - 1)
    expect(snap().tier).toBe('live')
    expect(snap().people.length).toBe(4)
  })
})

describe('say', () => {
  it('sends, then the sample’s agent answers', async () => {
    src.player.setScenario('watch')
    await expect(src.say({ to: 'agent:atlas', text: 'Keep the breath?', address: '@item:2' })).resolves.toEqual({ ok: true })
    expect(snap().messages.at(-1)).toMatchObject({ from: 'person:ada', text: 'Keep the breath?' })
    vi.advanceTimersByTime(1000)
    expect(snap().messages.at(-1)).toMatchObject({ from: 'agent:atlas', to: 'person:ada' })
  })
})

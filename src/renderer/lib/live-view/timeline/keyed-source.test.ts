// The real timeline source on a scripted socket: the timeline pack's own
// watched-turn golden (k2-mpc-timeline rel/v0.1.0 39d8a30a,
// crates/timeline/tests/watch_turn_golden/trim_held_outside_save.json, kept
// byte for byte as samples/pack-trim_held_outside_save.json) replayed frame by frame through the
// kit client, as K2's docs socket would deliver it: the watch's snapshot and
// pages, the roster's events, an outside save, an agent's held turn (trim,
// move, rename, off, a marker), a refused verb, the hold's end. Then a
// refusal at open, a resync, the say row's time and the monitor's honest
// states.

import { describe, expect, it } from 'vitest'
import type { ServerFrame, Transport } from '@k2/mpc-client'
import golden from './samples/pack-trim_held_outside_save.json'
import { KeyedWatchTimelineSource, refusalCode, type KeyedWelcome } from './keyed-source'
import { lanesOf } from './fold'
import { headOf } from './playheads'
import { monitorView } from './monitor-view'
import type { TimelineSnapshot } from './snapshot'

type Step = { step: string; roster: Record<string, unknown>[]; watch?: Record<string, unknown>[] }
const STEPS = (golden as unknown as { steps: Step[] }).steps
const T0 = 1793111401000

/** A scripted docs socket: what the test pushes is what the kit client reads; what it sends is kept. */
function socket(): Transport & { push: (f: unknown) => void; sent: Record<string, unknown>[] } {
  const subs = new Set<(f: ServerFrame) => void>()
  const sent: Record<string, unknown>[] = []
  return {
    send: (f: unknown) => void sent.push(f as Record<string, unknown>),
    onFrame: (cb: (f: ServerFrame) => void) => {
      subs.add(cb)
      return () => subs.delete(cb)
    },
    push: (f: unknown) => {
      for (const cb of [...subs]) cb(f as ServerFrame)
    },
    sent,
  } as Transport & { push: (f: unknown) => void; sent: Record<string, unknown>[] }
}

const WELCOME: KeyedWelcome = { root: '', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'time_position'] } as KeyedWelcome // what K2's daemon lists for a keyed doc (live_watch.rs keyed_socket_features)

function open(o: { upto?: string; welcome?: KeyedWelcome } = {}) {
  const t = socket()
  let clock = T0
  const posts: unknown[] = []
  const src = new KeyedWatchTimelineSource(t, { post: async (b) => void posts.push(b), now: () => clock })
  src.attach(o.welcome ?? WELCOME)
  const play = (name: string): void => {
    const s = STEPS.find((x) => x.step === name)
    if (!s) throw new Error('no step ' + name)
    clock += 1300
    for (const e of s.roster) t.push({ type: 'roster_event', event: e })
    for (const f of s.watch ?? []) t.push(f)
  }
  for (const s of STEPS) {
    play(s.step)
    if (s.step === (o.upto ?? 'watch')) break
  }
  const tl = (): TimelineSnapshot['timeline'] => src.timeline()
  return { t, src, tl, play, posts, tick: (ms: number) => (clock += ms) }
}

const itemNamed = (tl: TimelineSnapshot['timeline'], name: string) => lanesOf(tl.model).flatMap((x) => x.items).find((i) => i.name === name)

describe('KeyedWatchTimelineSource on the pack golden', () => {
  it('opens: the welcome, the snapshot head and its pages become the real lanes, live, with no sample', () => {
    const { src, tl, t } = open({ upto: 'watch' })
    expect(src.kind).toBe('live')
    expect(t.sent.some((f) => f.type === 'watch')).toBe(true)
    const v = tl()
    expect(v.sample).toBe(false)
    expect(v.state.loading).toBeNull()
    expect(v.state.tier).toBe('live')
    expect(v.sequence).toBe('@d-seq:1')
    // Each video source is known only as video: no proxy, poster or filmstrip, so nothing is drawn.
    expect(Object.values(v.facts ?? {})).toEqual(Object.values(v.facts ?? {}).map(() => ({ kind: 'video' })))
    expect(Object.keys(v.facts ?? {}).length).toBeGreaterThan(0)
  })

  it('structure: tracks, items and markers come from the keyed state, video above audio', () => {
    const v = open({ upto: 'watch' }).tl()
    const lanes = lanesOf(v.model)
    expect(lanes.map((l) => [l.title, l.kind])).toEqual([
      ['V2', 'video'],
      ['V1', 'video'],
      ['A1', 'audio'],
    ])
    const v1 = lanes[1]
    expect(v1.items.map((i) => [i.name, i.kind])).toEqual([
      ['Shot A', 'clip'],
      ['Shot B', 'clip'],
      [expect.any(String), 'transition'],
      ['Shot C', 'clip'],
      ['Shot A again', 'clip'],
    ])
    const a = itemNamed(v, 'Shot A')!
    // 254016000000 ticks a second, 24 fps: Shot A runs from 1 s for 2 s.
    expect([a.f0, a.f1]).toEqual([24, 72])
    expect(lanes[2].items.map((i) => i.name)).toEqual(['Music'])
    expect(v.model.markers?.length ?? lanes.flatMap((l) => l.markers).length).toBeGreaterThan(0)
  })

  it('a change: the outside save and the agent’s trim land on the lanes and in Changes', () => {
    const o = open({ upto: 'watch' })
    o.play('claim')
    o.play('outside_save')
    expect(itemNamed(o.tl(), 'Shot C graded')).toBeTruthy()
    o.play('trim')
    const a = itemNamed(o.tl(), 'Shot A')!
    expect([a.f0, a.f1]).toEqual([30, 72])
    const changes = o.src.snapshot().changes
    expect(changes.some((c) => c.who === 'atlas')).toBe(true)
    expect(changes.find((c) => c.target === '@item:4')).toMatchObject({ who: 'file', verb: 'saved', tone: 'outside' })
    expect(changes.find((c) => c.who === 'atlas')?.text).toBe('trimmed the start of Shot A by 6 frames')
  })

  it('presence: playheads by time position, the follow target by view address, holds as claims', () => {
    const o = open({ upto: 'claim' })
    let v = o.tl()
    expect(v.state.claims.map((c) => [c.id, c.actor.id, c.keys])).toEqual([['claim-1', 'atlas', ['@item:1']]])
    o.play('outside_save')
    o.play('trim')
    v = o.tl()
    const atlas = v.state.peers.atlas
    expect(atlas.kind).toBe('agent')
    expect(atlas.timePosition).toMatchObject({ at: '317520000000', state: 'paused', in: '@trk:1' })
    expect(headOf(v.model, atlas)?.f).toBe(30)
    // Following Atlas goes where Atlas looks (its view address), the same as any person's.
    o.t.push({ type: 'roster_event', event: { kind: 'view_address', who: 'atlas', view_address: '@item:4', client: 2 } })
    expect(o.tl().state.peers.atlas.viewAddress).toBe('@item:4')
    const people = o.src.snapshot().people
    expect(people.find((p) => p.id === 'atlas')).toBeTruthy()
    o.play('move')
    o.play('rename')
    o.play('off')
    o.play('marker')
    o.play('refused_trim')
    o.play('write_back')
    o.play('release')
    expect(o.tl().state.claims).toEqual([])
  })

  it('your own place goes out: the view address and time position, through the presence channel', () => {
    const o = open({ upto: 'watch' })
    o.src.setViewAddress('@item:2')
    o.src.setTimePosition({ at: '762048000000', state: 'paused', in: '@item:2' })
    expect(o.t.sent).toContainEqual(expect.objectContaining({ type: 'view_address', address: '@item:2' }))
    expect(o.t.sent).toContainEqual(expect.objectContaining({ type: 'time_position' }))
  })

  it('a refusal: a refused verb shows as refused, who holds it named', () => {
    const o = open({ upto: 'marker' })
    o.play('refused_trim')
    const v = o.tl()
    expect(v.state.refused).toMatchObject({ verb: 'clip trim', unit: '@item:1', holder: { id: 'atlas' } })
    expect(o.src.snapshot().changes.some((c) => c.tone === 'refused')).toBe(true)
  })

  it('a refusal at open: the pack’s decline code (details.code) picks the card’s words', () => {
    const t = socket()
    const src = new KeyedWatchTimelineSource(t, { post: async () => undefined, now: () => T0 })
    src.attach(WELCOME)
    t.push({ type: 'watch_refused', error: { id: 'MPC-E201', message: 'the timeline can’t open live: too many items', details: { code: 'live_too_large' } } })
    const S = src.timeline().state
    expect(S.tier).toBe('declined')
    expect(S.decline).toMatchObject({ code: 'live_too_large', title: 'Too large to watch live, for now' })
    expect(S.loading).toBeNull()
    expect(refusalCode({ id: 'MPC-E201', message: 'x', details: { code: 'live_many_timelines' } })).toBe('live_many_timelines')
    expect(refusalCode({ id: 'MPC-E201', message: 'declined: unreadable' })).toBe('unreadable')
  })

  it('a resync: a skipped frame asks for a fresh snapshot, the lanes stay until it comes, then it replaces them', () => {
    const o = open({ upto: 'watch' })
    const before = o.t.sent.filter((f) => f.type === 'watch').length
    // A change from further on, skipping seq 13: the kit asks again.
    const trim = STEPS.find((s) => s.step === 'trim')!.watch![0]
    o.t.push(trim)
    expect(o.t.sent.filter((f) => f.type === 'watch').length).toBe(before + 1)
    expect(lanesOf(o.tl().model).length).toBe(3)
    // The host sends the snapshot again, from the top.
    for (const f of STEPS[0].watch!) o.t.push(f)
    expect(itemNamed(o.tl(), 'Shot A')).toBeTruthy()
    expect(o.tl().state.loading).toBeNull()
  })

  it('a reconnect: down says reconnecting, the new welcome asks for a snapshot and clears it', () => {
    const o = open({ upto: 'watch' })
    o.src.setConnection('reconnecting')
    expect(o.tl().state.conn).toBe('reconnecting')
    const n = o.t.sent.filter((f) => f.type === 'watch').length
    o.src.reconnected({ ...WELCOME, client: 5 })
    expect(o.t.sent.filter((f) => f.type === 'watch').length).toBe(n + 1)
    expect(o.tl().state.conn).toBeNull()
  })

  it('say: the message carries the sequence’s address and the time, and shows at once', async () => {
    const o = open({ upto: 'watch' })
    const r = await o.src.say({ to: 'atlas', text: 'tighten this', address: '@d-seq:1', time: { at: '0', in: '@d-seq:1' } })
    expect(r).toEqual({ ok: true })
    expect(o.posts).toEqual([{ text: 'tighten this', to: 'atlas', at: '@d-seq:1', time: { at: '0', in: '@d-seq:1' } }])
    expect(o.src.snapshot().messages.at(-1)).toMatchObject({ text: 'tighten this', to: 'atlas' })
  })
})

describe('the monitor on a real file is honest', () => {
  it('says Preparing video and never draws sample footage', () => {
    const v = open({ upto: 'watch' }).tl()
    const mv = monitorView({ S: v.state, M: v.model, facts: v.facts!, now: T0, local: { f: 30, rate: 0 } })
    expect(v.sample).toBe(false)
    expect(mv.state).toBe('preparing')
    expect(mv.paint.how).toBe('none')
    expect(mv.prep?.text).toMatch(/^Preparing video/)
  })

  it('says no preview where this host can’t decode', () => {
    const o = open({ upto: 'watch' })
    o.src.setPreviewHost('not_on_this_host')
    const v = o.tl()
    const mv = monitorView({ S: v.state, M: v.model, facts: v.facts!, now: T0, local: { f: 30, rate: 0 } })
    expect(mv.state).toBe('no_preview')
    expect(mv.paint.how).toBe('none')
  })
})

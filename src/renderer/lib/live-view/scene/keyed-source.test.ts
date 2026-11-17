// The real scene source on a scripted socket: the BLEND pack's own watched
// turn (k2-mpc-blend main, crates/blend/tests/watch_turn_golden/
// scene_turn.json at aced5b5c, 2026-11-16; kept byte for byte in
// tests/fixtures/blend-watch/scene_turn.json) replayed frame by frame
// through the kit client, as K2's docs socket would deliver it: the watch's
// snapshot and pages, Atlas placing Table and Chair under its hold, Bolt
// refused on the held Vase. The roster's events (who's here, the hold,
// where each one looks) are scripted in the kit's shapes, as the golden
// carries none; so are a mesh change, a resync and a reconnect.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { ServerFrame, Transport } from '@k2/mpc-client'
import { KeyedWatchSceneSource, refusalCode, type KeyedWelcome } from './keyed-source'
import { hiddenOf, inSceneDrawn } from './overlay'
import type { SceneSnapshot } from './sample-source'

type Step = { step: string; by: string; watch: Record<string, unknown>[] | null }
const GOLDEN = JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'blend-watch', 'scene_turn.json'), 'utf8')) as { steps: Step[] }
const STEPS = GOLDEN.steps
const T0 = 1793111400000

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

const WELCOME: KeyedWelcome = { root: '', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'presence', 'claims'] } as KeyedWelcome
const member = (handle: string, name: string, kind: string, colour: string, client: number) => ({ handle, name, colour, kind, role: 'viewer', away: false, cursor: null, rights: 'edit', devices: [{ client, cursor: null }] })
const ROSTER = { kind: 'snapshot', version: 1, members: [member('ada', 'Ada', 'person', '#2563eb', 1), member('atlas', 'Atlas', 'agent', '#d9480f', 2), member('bolt', 'Bolt', 'agent', '#2b8a3e', 3)], watching: 0 }
const CLAIM = { kind: 'claim_started_elems', who: 'atlas', claim: 'claim-1', root: 'objs', keys: ['@obj:3', '@obj:4', '@obj:5'], note: 'arranging the set', hold: true, expires_ms: T0 + 600000, version: 1, actor: { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#d9480f' }, started_ms: T0 + 300 }

function open(o: { welcome?: KeyedWelcome; upto?: string } = {}) {
  const t = socket()
  let clock = T0
  const src = new KeyedWatchSceneSource(t, { post: async () => undefined, fileName: 'living-room', now: () => clock })
  src.attach(o.welcome ?? WELCOME)
  t.push({ type: 'roster_event', event: ROSTER })
  const play = (name: string): void => {
    const s = STEPS.find((x) => x.step === name)!
    clock += 250
    if (name === 'claim') t.push({ type: 'roster_event', event: CLAIM })
    if (name === 'release') t.push({ type: 'roster_event', event: { kind: 'claim_ended', claim: 'claim-1', who: 'atlas', how: 'released', keys: CLAIM.keys } })
    for (const f of s.watch ?? []) t.push(f)
  }
  for (const s of STEPS) {
    play(s.step)
    if (s.step === (o.upto ?? 'watch')) break
  }
  const snap = (): SceneSnapshot => src.snapshot() as SceneSnapshot
  return { t, src, play, snap }
}

describe('KeyedWatchSceneSource on the BLEND pack’s golden', () => {
  it('opens: the snapshot and its pages become the real scene, live, with only real people', () => {
    const { src, snap, t } = open()
    expect(src.kind).toBe('live')
    expect(t.sent.some((f) => f.type === 'watch')).toBe(true)
    const S = src.state()
    expect(S.loading).toBeNull()
    expect(S.D.lists.objs).toEqual(['@obj:1', '@obj:2', '@obj:3', '@obj:4', '@obj:5'])
    expect(S.D.props['@obj:3'].name).toBe('Table')
    // references between elements are addresses: Vase's collection and the collections' parents
    expect(S.D.props['@coll:2'].parent).toBe('@coll:1')
    expect(S.D.props['@obj:1'].coll).toBe('@coll:1')
    expect(S.D.units['@obj:3'].placement).not.toBeNull()
    expect(S.D.currentScene).toBe('@coll:1')
    const s = snap()
    expect(s.scene.fileName).toBe('living-room')
    expect(s.people.map((p) => p.name).sort()).toEqual(['Atlas', 'Bolt', 'You'])
    expect(s.people.find((p) => p.name === 'Atlas')?.kind).toBe('agent')
  })

  it('a transform change: Atlas places Table; the doc moves and Activity says so', () => {
    const o = open({ upto: 'claim' })
    o.play('place Table')
    const S = o.src.state()
    expect(S.D.rev).toBe(2)
    expect(S.D.props['@obj:3'].xform).toBe('0.5,0.25,0,0,0,0.5,1,1,1')
    expect(S.log.at(-1)).toMatchObject({ kind: 'frame', actor: 'atlas', rev: 2 })
    expect(o.snap().changes.at(-1)).toMatchObject({ who: 'atlas', target: '@obj:3' })
  })

  it('a mesh change: a unit’s mesh_hash moves, and the picture is asked for new stills', () => {
    const o = open({ upto: 'place Chair' })
    expect(o.snap().scene.meshEpoch).toBe(0)
    o.t.push({ type: 'ops_change', seq: 11, rev: 4, at_ms: T0 + 2000, change_kind: 'edit', actor: { id: 'atlas', name: 'atlas', kind: 'agent', colour: '#d9480f' }, client: 2, via: 'cli', on_behalf_of: null, turn: 'turn-1', verb: 'object mesh', claim: 'claim-1', ops: [{ root: 'obj_props', kind: 'map', op: 'set', key: '@obj:3', field: 'data', before: 'mesh @me:Table', after: 'mesh @me:Table.001' }], units: [{ key: '@obj:3', container: '@coll:2', ops: ['fields'], geom_before: null, geom_after: null, fields: ['data'] }], containers_changed: [], containers: [], derived: { units: { '@obj:3': { mesh_hash: 'f'.repeat(64), data: 'mesh @me:Table.001' } } }, render_hashes: null })
    expect(o.src.state().D.units['@obj:3'].derived.mesh_hash).toBe('f'.repeat(64))
    expect(o.snap().scene.meshEpoch).toBe(1)
  })

  it('a hold, when the host takes claims: on the objects, then let go', () => {
    const o = open({ upto: 'claim' })
    expect(o.src.state().claims.map((c) => [c.id, c.actor.id, c.keys, c.label])).toEqual([['claim-1', 'atlas', ['@obj:3', '@obj:4', '@obj:5'], 'arranging the set']])
    o.play('place Table')
    o.play('place Chair')
    o.play('refused')
    o.play('release')
    expect(o.src.state().claims).toEqual([])
    expect(o.src.state().log.at(-1)).toMatchObject({ kind: 'claim_end', id: 'claim-1', how: 'released' })
  })

  it('no claims in the welcome: holds never show, never made up', () => {
    const o = open({ upto: 'claim', welcome: { ...WELCOME, features: ['keyed_watch', 'view_address', 'presence'] } })
    expect(o.src.state().claims).toEqual([])
  })

  it('Follow by view address: where each one looks comes from presence; yours goes out', () => {
    const o = open()
    o.t.push({ type: 'roster_event', event: { kind: 'view_address', who: 'atlas', view_address: '@cam:1', client: 2 } })
    expect(o.snap().people.find((p) => p.id === 'atlas')?.viewAddress).toBe('@cam:1')
    o.src.control.viewChanged('@obj:3')
    expect(o.t.sent).toContainEqual(expect.objectContaining({ type: 'view_address', address: '@obj:3' }))
  })

  it('a refusal: Bolt refused on the held Vase, the holder named (the scene’s callout words)', () => {
    const o = open({ upto: 'place Chair' })
    o.play('refused')
    const r = o.snap().refusal
    expect(r).toMatchObject({ actor: 'Bolt', verb: 'move', target: 'Vase', holder: 'Atlas' })
    expect(o.src.state().callout?.r).toMatchObject({ unit: '@obj:4', code: 'MPC-E301', holder: 'atlas' })
  })

  it('a resync: a skipped frame asks for a fresh snapshot; the scene stays until it comes, then replaces it', () => {
    const o = open()
    const before = o.t.sent.filter((f) => f.type === 'watch').length
    o.t.push(STEPS.find((s) => s.step === 'place Chair')!.watch![0])
    expect(o.t.sent.filter((f) => f.type === 'watch').length).toBe(before + 1)
    expect(o.src.state().D.props['@obj:3'].name).toBe('Table')
    for (const f of STEPS[0].watch!) o.t.push(f)
    expect(o.src.state().loading).toBeNull()
    expect(o.src.state().D.lists.objs).toHaveLength(5)
  })

  it('a reconnect: down says reconnecting, the new welcome asks for a snapshot and clears it', () => {
    const o = open()
    o.src.setConnection('reconnecting')
    expect(o.snap().connection).toBe('reconnecting')
    const n = o.t.sent.filter((f) => f.type === 'watch').length
    o.src.reconnected({ ...WELCOME, client: 5 })
    expect(o.t.sent.filter((f) => f.type === 'watch').length).toBe(n + 1)
    expect(o.snap().connection).toBeNull()
  })

  it('a refusal at open: the saved version, read-only; the pack’s code kept', () => {
    const t = socket()
    const src = new KeyedWatchSceneSource(t, { post: async () => undefined, fileName: 'x', now: () => T0 })
    src.attach(WELCOME)
    t.push({ type: 'watch_refused', error: { id: 'MPC-E201', message: 'too many objects', details: { code: 'too_large' } } })
    expect(src.state().tier).toBe('blob')
    expect(src.refusedCode).toBe('too_large')
    expect(refusalCode({ id: 'MPC-E201', message: 'x' } as never)).toBe('MPC-E201')
  })

  it('the pack’s derived values: hidden_by decides hidden, and only the scene drawn gets boxes', () => {
    const { src } = open()
    const D = src.state().D
    expect(D.unitDerived).toEqual(expect.arrayContaining(['mesh_hash', 'hidden_by']))
    expect(hiddenOf(D, '@obj:3')).toBe('none')
    const hid = { ...D, units: { ...D.units, '@obj:3': { ...D.units['@obj:3'], derived: { ...D.units['@obj:3'].derived, hidden_by: 'collection:@coll:2' } } } }
    expect(hiddenOf(hid, '@obj:3')).toBe('collection')
    expect(inSceneDrawn(D, '@obj:3')).toBe(true)
    expect(inSceneDrawn({ ...D, currentScene: '@coll:9' }, '@obj:3')).toBe(false)
    // the view couldn't pair: no mark, nothing filtered
    expect(inSceneDrawn({ ...D, currentScene: null }, '@obj:3')).toBe(true)
  })
})

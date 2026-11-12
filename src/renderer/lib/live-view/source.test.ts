// KeyedWatchWorkbookSource on a doc socket: with `keyed_watch` in the
// welcome, the kit's KeyedWatch asks for the watch and builds the doc's
// state and view from the frames; the kit's own goldens
// (`client/test/golden/keyed_snapshot.json`, `keyed_frames.json`) go
// through it to the kit's final state, byte for byte. Without the feature
// nothing is sent and the frames pass on as before. And the view's fold
// (keyed-fold.ts) lands on the same state as the source's watch.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { keyedStateJson, type ClientFrame, type ServerFrame, type Transport } from './kit'
import { initialLiveState, liveWorkbookReducer, type LiveWorkbookFrame } from './reducer'
import { sampleRosterSnapshot, sampleSnapshotFrames } from './sample-source'
import { GOLDEN, stripAnnotations } from './samples/goldens'
import { CHANGES_KEPT, KeyedWatchWorkbookSource, listsKeyedWatch, type LiveWorkbookSourceEvent } from './source'
import { play } from './test-helpers'
import type { XlsxOpsChangeFrame, XlsxWelcome } from './xlsx-provisional'

const GOLDENS = join(dirname(fileURLToPath(import.meta.url)), '../../../../vendor/k2-mpc-base/client/test/golden')
type ViewGolden = { frames: ServerFrame[]; final_json: string; units: Record<string, unknown[]> }
type Golden = { frames: ServerFrame[]; final_json: string; final_rev: number; features?: string[]; view?: ViewGolden }
const load = (name: string): Golden => JSON.parse(readFileSync(join(GOLDENS, name), 'utf8')) as Golden
const SNAPSHOT = load('keyed_snapshot.json')
const FRAMES = load('keyed_frames.json')

function socket() {
  const ls = new Set<(f: ServerFrame) => void>()
  const sent: ClientFrame[] = []
  const t: Transport = {
    send: (f) => void sent.push(f),
    onFrame: (cb) => {
      ls.add(cb)
      return () => ls.delete(cb)
    },
  }
  return { t, sent, deliver: (f: ServerFrame | LiveWorkbookFrame) => [...ls].forEach((l) => l(f as ServerFrame)) }
}

const watched: XlsxWelcome = { client: 7, self: 'person:ada', rights: 'read', wire: 3, features: ['presence', 'keyed_watch'] }
const older: XlsxWelcome = { client: 7, self: 'person:ada', rights: 'read', wire: 3, features: ['presence'] }
const watches = (sent: ClientFrame[]) => sent.filter((f) => f.type === 'watch').length

describe('KeyedWatchWorkbookSource: the kit goldens through K2’s source', () => {
  it('keyed_frames.json: one watch asked, every frame passed on in order, the kit’s final state', () => {
    expect(FRAMES.features).toEqual(['keyed_watch'])
    const { t, sent, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    expect(watches(sent)).toBe(1)
    const got: LiveWorkbookSourceEvent[] = []
    src.subscribe((e) => void got.push(e))
    for (const f of FRAMES.frames) deliver(f)
    expect(src.watch?.stateJson()).toBe(FRAMES.final_json)
    expect(src.watch?.rev).toBe(FRAMES.final_rev)
    expect(src.watch?.ready).toBe(true)
    expect(watches(sent)).toBe(1)
    expect(sent.some((f) => f.type === 'report')).toBe(true)
    expect(got.map((e) => e.type)).toEqual(FRAMES.frames.map((f) => f.type))
    // A subscriber catching up gets the welcome, then the same frames.
    expect(src.snapshot().map((e) => e.type)).toEqual(['welcome', ...FRAMES.frames.map((f) => f.type)])
    src.close()
  })

  it("keyed_snapshot.json's deck: the view index's snapshot and each container's units, the state by address", () => {
    const g = SNAPSHOT.view as ViewGolden
    const head = g.frames[0] as unknown as { view: { containers: { key: string; unit_count: number }[] } }
    const { t, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    for (const f of g.frames) deliver(f)
    expect(src.watch?.stateJson()).toBe(g.final_json)
    expect(src.watch?.view).toEqual(head.view)
    for (const c of head.view.containers) {
      expect(src.watch?.units(c.key)).toEqual(g.units[c.key] ?? [])
      expect(src.watch?.units(c.key).length).toBe(c.unit_count)
    }
  })

  it('keyed_snapshot.json in pages of the smallest cap reassembles to the kit’s state', () => {
    const { t, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    for (const f of SNAPSHOT.frames) deliver(f)
    expect(src.watch?.stateJson()).toBe(SNAPSHOT.final_json)
    expect(src.watch?.rev).toBe(SNAPSHOT.final_rev)
  })

  it('a lost frame: the watch asks again, and a fresh snapshot replaces what a subscriber catches up from', () => {
    const { t, sent, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    const changes = FRAMES.frames.filter((f) => f.type === 'ops_change')
    const snap = FRAMES.frames.filter((f) => f.type !== 'ops_change')
    for (const f of snap) deliver(f)
    deliver(changes[1]) // changes[0] never came
    expect(watches(sent)).toBe(2)
    for (const f of SNAPSHOT.frames) deliver(f)
    expect(src.watch?.stateJson()).toBe(SNAPSHOT.final_json)
    expect(src.snapshot().map((e) => e.type)).toEqual(['welcome', ...SNAPSHOT.frames.map((f) => f.type)])
  })
})

describe('KeyedWatchWorkbookSource: without the feature, today’s path', () => {
  it('an older host: nothing sent, no watch, the frames pass on as they come', () => {
    expect(listsKeyedWatch(older)).toBe(false)
    expect(listsKeyedWatch(watched)).toBe(true)
    const { t, sent, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, older)
    expect(src.watch).toBeNull()
    expect(sent).toEqual([])
    const roster: LiveWorkbookFrame = { type: 'roster_event', event: { kind: 'joined', who: 'agent:atlas', participant: { name: 'Atlas', colour: 'teal', kind: 'agent' } } }
    deliver(roster)
    deliver({ type: 'ack', op: 1, version: 2, hunks: [] })
    expect(src.snapshot()).toEqual([{ type: 'welcome', ...older }, roster])
    expect(sent).toEqual([])
  })
})

describe('KeyedWatchWorkbookSource: the workbook view', () => {
  const xlsxFrames = (): LiveWorkbookFrame[] => {
    const snap = sampleSnapshotFrames()
    const last = (snap[snap.length - 1] as { seq: number }).seq
    const changes = GOLDEN.turn.frames.map((f, i): LiveWorkbookFrame => ({ ...(stripAnnotations(f) as XlsxOpsChangeFrame), type: 'ops_change', seq: last + 1 + i }))
    return [...snap, sampleRosterSnapshot(), ...changes]
  }

  it('the xlsx sample through the socket: the view’s book and the source’s watch hold the same state', () => {
    const { t, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    for (const f of xlsxFrames()) deliver(f)
    const s = src.snapshot().reduce(liveWorkbookReducer, initialLiveState())
    const final = play().states.at(-1)?.book
    expect(s.book?.rev).toBe(final?.rev)
    expect(s.book && keyedStateJson(s.book.state)).toBe(src.watch?.stateJson())
    expect(src.watch?.rev).toBe(s.book?.rev)
    expect(s.resync).toBeNull()
  })

  it('past CHANGES_KEPT changes since a snapshot, the source asks for a fresh one', () => {
    const { t, sent, deliver } = socket()
    const src = new KeyedWatchWorkbookSource(t, watched)
    for (let i = 0; i <= CHANGES_KEPT; i++) deliver({ type: 'roster_event', event: { kind: 'pointer', who: 'person:sam', pointer: { x: i, y: 0 } } })
    // Pointers keep only the latest per person.
    expect(src.snapshot().filter((e) => e.type === 'roster_event').length).toBe(1)
    const head = { type: 'keyed_snapshot', seq: 1, upto: 1, schema: { name: 'b', roots: [] }, view: null, pages: 0 } as ServerFrame
    deliver(head)
    const one = { type: 'ops_change', seq: 2, rev: 2, at_ms: 1, change_kind: 'edit', actor: { id: 'a', name: 'A', kind: 'agent', colour: null }, client: 1, via: 'cli', on_behalf_of: null, turn: null, verb: null, claim: null, ops: [], derived: null, render_hashes: null }
    for (let i = 0; i < CHANGES_KEPT + 1; i++) deliver({ ...one, seq: 2 + i, rev: 2 + i } as ServerFrame)
    expect(watches(sent)).toBe(2)
  })
})

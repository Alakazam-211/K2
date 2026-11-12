// The reducer folds the keyed watch as KeyedWatch does (it never guesses: a
// gap or a misfit asks for a snapshot), keeps every state it was given whole,
// and folds presence, claims and the client's follow gesture.

import { describe, expect, it } from 'vitest'
import { initialLiveState, liveWorkbookReducer, loadingOf, type LiveWorkbookFrame } from './reducer'
import { sampleRosterSnapshot, sampleSnapshotFrames, sampleSteps } from './sample-source'
import { fold, play, stateAtRev } from './test-helpers'
import { GOLDEN, stripAnnotations } from './samples/goldens'
import { EMPTY_LOCAL, formulaTarget, isFollowing, stageSheet } from './view-model'
import { a1Of } from './workbook'
import type { XlsxOpsChangeFrame } from './xlsx-provisional'

const welcome: LiveWorkbookFrame = { type: 'welcome', client: 90071, self: 'person:ada', rights: 'read' }
const loaded = () => fold([welcome, ...sampleSnapshotFrames(), sampleRosterSnapshot()])
const frame21 = (seq: number): LiveWorkbookFrame => ({ ...(stripAnnotations(GOLDEN.turn.frames[0]) as XlsxOpsChangeFrame), type: 'ops_change', seq })

describe('liveWorkbookReducer: the keyed watch', () => {
  it('builds the book page by page, reporting progress, and is live once every page is in', () => {
    const snap = sampleSnapshotFrames()
    let s = fold([welcome, snap[0]])
    expect(loadingOf(s)).toMatchObject({ phase: 'pages', done: 0, total: snap.length - 1 })
    s = fold(snap.slice(1, 3), s)
    expect(loadingOf(s).rootsIn).toEqual(['sheets', 'sheet_props'])
    expect(s.book).toBeNull()
    s = fold(snap.slice(3), s)
    expect(s.book?.rev).toBe(20)
    expect(loadingOf(s).phase).toBe('live')
  })

  it('a skipped seq asks for a fresh snapshot and drops frames until one comes', () => {
    const s0 = loaded()
    const s1 = liveWorkbookReducer(s0, frame21(s0.seq + 2))
    expect(s1.resync?.why).toBe('gap')
    expect(s1.awaiting).toBe(true)
    const s2 = liveWorkbookReducer(s1, frame21(s0.seq + 1))
    expect(s2.book?.rev).toBe(20)
  })

  it("a frame that doesn't fit (a delete of an absent entry) asks again; it never guesses", () => {
    const s0 = loaded()
    const bad: LiveWorkbookFrame = { ...(frame21(s0.seq + 1) as XlsxOpsChangeFrame & { type: 'ops_change' }), ops: [{ root: 'cells', kind: 'map', op: 'delete', key: 'nope', field: 'c1' }] }
    const s1 = liveWorkbookReducer(s0, bad)
    expect(s1.resync?.why).toBe('inconsistent')
    expect(s1.book).toBe(s0.book)
  })

  it('a change at or before the book’s rev is a misfit', () => {
    const s0 = loaded()
    const old: LiveWorkbookFrame = { ...(frame21(s0.seq + 1) as XlsxOpsChangeFrame & { type: 'ops_change' }), rev: 20 }
    expect(liveWorkbookReducer(s0, old).resync?.why).toBe('inconsistent')
  })

  it('never changes a state it was given (a reducer may run twice)', () => {
    const steps = sampleSteps('turn')
    let s = initialLiveState()
    for (const st of steps) {
      for (const e of st.events) {
        const before = JSON.stringify(s, (_k, v) => (v instanceof Map ? [...v] : v))
        const a = liveWorkbookReducer(s, e)
        const b = liveWorkbookReducer(s, e)
        expect(JSON.stringify(s, (_k, v) => (v instanceof Map ? [...v] : v))).toBe(before)
        expect(b.book?.rev).toBe(a.book?.rev)
        s = a
      }
    }
    expect(s.book?.rev).toBe(30)
  })
})

describe('liveWorkbookReducer: presence, claims and follow', () => {
  it('follows the agent by default: the stage is its sheet and the formula bar its caret', () => {
    const s = stateAtRev(play(), 21)
    expect(isFollowing(s)).toBe(true)
    expect(stageSheet(s, EMPTY_LOCAL)).toBe('@sheet:1')
    const t = formulaTarget(s, EMPTY_LOCAL, '@sheet:1')
    expect(t && { at: a1Of(t), who: t.who }).toEqual({ at: 'D5', who: 'agent' })
  })

  it('paused, the stage stays on the sheet you picked when the agent moves; follow again goes to the agent', () => {
    const p = play()
    const s25 = stateAtRev(p, 25)
    const paused = liveWorkbookReducer(s25, { type: 'local_follow', on: false })
    const i26 = p.steps.findIndex((st) => st.label === 'rev 26')
    const s26 = fold(p.steps[i26].events, paused)
    expect(stageSheet(s26, { pinned: '@sheet:1', caret: null })).toBe('@sheet:1')
    // Not following, your own focus didn't move with the agent's.
    expect(s26.peers['person:ada'].focus?.key).toBe('@sheet:1')
    const again = liveWorkbookReducer(s26, { type: 'local_follow', on: true })
    expect(stageSheet(again, { pinned: '@sheet:1', caret: null })).toBe('@sheet:2')
    expect(again.peers['person:ada'].cells).toBeNull()
  })

  it('a clicked cell is yours in the formula bar, not the agent’s', () => {
    const s = stateAtRev(play(), 21)
    expect(formulaTarget(s, { pinned: '@sheet:1', caret: { sheet: '@sheet:1', r: 2, c: 3 } }, '@sheet:1')).toEqual({ r: 2, c: 3, who: 'you' })
  })

  it('keeps the latest two changes for outlines, and the first live edit (results not recalculated since rev 21)', () => {
    const s = stateAtRev(play(), 24)
    expect(s.recent.map((r) => [r.frame.rev, r.age])).toEqual([[24, 0], [23, 1]])
    expect(s.firstLiveEdit).toBe(21)
  })

  it('joined, left and away fold into the peers', () => {
    let s = loaded()
    s = fold([{ type: 'roster_event', event: { kind: 'joined', who: 'agent:brook', participant: { name: 'Brook', colour: 'teal', kind: 'agent' } } }], s)
    expect(s.peers['agent:brook']).toMatchObject({ name: 'Brook', kind: 'agent', isSelf: false })
    s = fold([{ type: 'roster_event', event: { kind: 'away', who: 'person:sam', away: true } }], s)
    expect(s.peers['person:sam'].away).toBe(true)
    s = fold([{ type: 'roster_event', event: { kind: 'left', who: 'agent:brook' } }], s)
    expect(s.peers['agent:brook']).toBeUndefined()
  })

  it('a reset starts over', () => {
    expect(liveWorkbookReducer(loaded(), { type: 'reset' })).toMatchObject({ book: null, log: [], peers: {} })
  })
})

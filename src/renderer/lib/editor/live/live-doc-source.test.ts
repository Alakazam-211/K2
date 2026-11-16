// LiveDocSource on a scripted kit stream (build-editors.md E4a): a scripted
// editor emits what the kit's editor emits (state, roster, rights, change,
// pending, note, closed) and a scripted tap delivers the socket's own frames
// (the welcome with `features` and `access_owner`, `outside_save`,
// `verb_refused`, wire-shapes-k2-2026-11-15). A manual clock drives the
// finished-edit and connection timers. Pinned here:
//   - the roster: you first, each person's place from their caret, away people;
//   - the welcome: rights, access_owner (and its fallback), view_address;
//   - finished edits: one row per quiet run, Live edits and one announcement;
//     Undo on your own newest edit only;
//   - the connection: reconnecting, can't (after 30 s), Retry → trying,
//     offline, back; offline edits counted once and the same everywhere;
//   - outside_save ingested (with the text from before it), missing, declined;
//   - verb_refused: a Changes row with "Ask <holder>" and the sentence.

import { describe, expect, it, vi } from 'vitest'
import type { RosterEntry } from '@k2/mpc-client'
import type { LiveEditorEvents } from '@/components/LiveEditor'
import { CANT_AFTER_MS, FINISH_MS, LiveDocSource, TRYING_MS, type LiveDocEditor, type LiveDocSnapshot, type LiveDocTimers } from './live-doc-source'
import type { TappedWelcome, TransportTap } from './transport-tap'

const DOC = '# Harbor\n\nIntro line.\n\n## Setup\n\n1. Clone it.\n2. Open it.\n\n## Configuration\n\nA table.\n'
const byteOfLine = (text: string, line: number): number => new TextEncoder().encode(text.split('\n').slice(0, line - 1).join('\n') + '\n').length

class Clock implements LiveDocTimers {
  t = 1_000_000
  private q: { at: number; f: () => void; id: number }[] = []
  private n = 0
  set(f: () => void, ms: number): unknown {
    const id = ++this.n
    this.q.push({ at: this.t + ms, f, id })
    return id
  }
  clear(h: unknown): void {
    this.q = this.q.filter((x) => x.id !== h)
  }
  now(): number {
    return this.t
  }
  advance(ms: number): void {
    const end = this.t + ms
    for (;;) {
      const next = this.q.filter((x) => x.at <= end).sort((a, b) => a.at - b.at)[0]
      if (!next) break
      this.q = this.q.filter((x) => x !== next)
      this.t = next.at
      next.f()
    }
    this.t = end
  }
}

class ScriptedEditor implements LiveDocEditor {
  text = DOC
  state: LiveDocEditor['state'] = 'connecting'
  rights = 'edit'
  roster: RosterEntry[] = []
  pending = 0
  undo = vi.fn()
  redo = vi.fn()
  reconnectNow = vi.fn()
  private cbs = new Map<string, Set<(v: unknown) => void>>()
  on<K extends keyof LiveEditorEvents>(event: K, cb: (v: LiveEditorEvents[K]) => void): () => void {
    const set = this.cbs.get(event) ?? new Set()
    set.add(cb as (v: unknown) => void)
    this.cbs.set(event, set)
    return () => void set.delete(cb as (v: unknown) => void)
  }
  emit<K extends keyof LiveEditorEvents>(event: K, v: LiveEditorEvents[K]): void {
    if (event === 'state') this.state = v as LiveDocEditor['state']
    if (event === 'roster') this.roster = v as RosterEntry[]
    for (const cb of this.cbs.get(event) ?? []) cb(v)
  }
}

class ScriptedTap implements TransportTap {
  transport = {} as TransportTap['transport']
  private w = new Set<(w: TappedWelcome) => void>()
  private f = new Set<(f: Record<string, unknown>) => void>()
  onWelcome(cb: (w: TappedWelcome) => void): () => void {
    this.w.add(cb)
    return () => void this.w.delete(cb)
  }
  onFrame(cb: (f: Record<string, unknown>) => void): () => void {
    this.f.add(cb)
    return () => void this.f.delete(cb)
  }
  welcome(extra: Partial<TappedWelcome> = {}): void {
    for (const cb of this.w) cb({ root: 'source', version: 1, client: 7, self: 'person:owner', rights: 'edit', ...extra })
  }
  frame(f: Record<string, unknown>): void {
    for (const cb of this.f) cb(f)
  }
}

const entry = (handle: string, name: string, o: Partial<RosterEntry> = {}): RosterEntry => ({ handle, name, colour: '#7c3aed', kind: 'person', role: 'editor', away: false, cursor: null, ...o })
const at = (line: number) => ({ root: 'source', anchor: 0, head: 0, head_lc: `${line}:1` })

function setup(o: { markdown?: boolean; welcome?: Partial<TappedWelcome> } = {}) {
  const clock = new Clock()
  const ed = new ScriptedEditor()
  const tap = new ScriptedTap()
  const src = new LiveDocSource({ markdown: o.markdown ?? true, timers: clock, online: true })
  src.attachTap(tap)
  src.attach(ed)
  tap.welcome(o.welcome)
  ed.emit('state', 'live')
  ed.emit('roster', [
    entry('person:owner', 'Rosson', { self: true, cursor: at(3) }),
    entry('agent:atlas', 'Atlas', { kind: 'agent', cursor: at(7) }),
    entry('person:sam', 'Sam', { cursor: at(13) }),
    entry('person:brook', 'Brook', { away: true, cursor: at(6) }),
  ])
  const snap = (): LiveDocSnapshot => src.snapshot()
  /** One change by `by` ('kit:external': an outside save) at `line`. */
  const change = (by: string | null, line: number, local = false): void => {
    ed.emit('change', { hunks: [{ start: byteOfLine(ed.text, line), end: byteOfLine(ed.text, line), insert: 'x' }], by, local })
  }
  return { clock, ed, tap, src, snap, change }
}

describe('the roster and the welcome', () => {
  it('names everyone with their place, you first, away people apart, and counts others here', () => {
    const { snap } = setup()
    const s = snap()
    expect(s.tier).toBe('live')
    expect(s.self).toBe('person:owner')
    expect(s.people.map((p) => [p.id, p.self ?? false, p.away, p.doing])).toEqual([
      ['person:owner', true, false, 'in the intro'],
      ['agent:atlas', false, false, 'in Setup'],
      ['person:sam', false, false, 'reading Configuration'],
      ['person:brook', false, true, 'was in Setup'],
    ])
    expect(s.othersHere).toBe(2)
    expect(s.agent).toBe('agent:atlas')
  })

  it('reads rights, access_owner and view_address from the welcome', () => {
    const { snap } = setup({ welcome: { rights: 'read', features: ['view_address'], access_owner: { id: 'person:owner', name: 'Sam', kind: 'person', colour: null } } })
    expect(snap().rights).toBe('read')
    expect(snap().accessOwner).toBe('Sam')
    expect(snap().features.viewAddress).toBe(true)
  })

  it('without access_owner, the owner is unknown (the words fall back) and there is no view_address', () => {
    const { snap } = setup()
    expect(snap().accessOwner).toBeNull()
    expect(snap().features.viewAddress).toBe(false)
  })

  it('a rights change after the welcome follows', () => {
    const { ed, snap } = setup()
    ed.emit('rights', 'read')
    expect(snap().rights).toBe('read')
  })
})

describe('finished edits', () => {
  it('a run of someone’s changes is one edit once it goes quiet: a row, a Live edits line, one announcement', () => {
    const { clock, change, snap } = setup()
    change('agent:atlas', 7)
    clock.advance(400)
    change('agent:atlas', 8)
    expect(snap().activity).toEqual({ who: 'agent:atlas', text: 'is editing Setup' })
    expect(snap().people.find((p) => p.id === 'agent:atlas')?.doing).toBe('editing Setup')
    expect(snap().changes).toEqual([])
    clock.advance(FINISH_MS)
    const s = snap()
    expect(s.changes.map((c) => [c.who, c.text, c.target, !!c.undoable])).toEqual([['agent:atlas', 'edited Setup', 'lines:7-8', false]])
    expect(s.liveEdits.map((e) => e.text)).toEqual(['edited Setup'])
    expect(s.announce?.text).toBe('Atlas edited Setup')
    expect(s.activity).toEqual({ who: 'agent:atlas', text: 'edited Setup' })
  })

  it('code files name lines, not sections', () => {
    const { clock, change, snap } = setup({ markdown: false })
    change('person:sam', 12)
    clock.advance(FINISH_MS)
    expect(snap().changes[0].text).toBe('edited line 12')
  })

  it('your own edits are rows with Undo on the newest only, never announced; Undo and Redo go to the server’s undo', () => {
    const { clock, change, snap, src, ed } = setup()
    change('person:owner', 3, true)
    clock.advance(FINISH_MS)
    change('person:owner', 7, true)
    clock.advance(FINISH_MS)
    const rows = snap().changes
    expect(rows.map((c) => [c.text, !!c.undoable])).toEqual([
      ['edited the intro', false],
      ['edited Setup', true],
    ])
    expect(snap().announce).toBeNull()
    src.undoChange(rows[1].id)
    expect(ed.undo).toHaveBeenCalledTimes(1)
    expect(snap().changes[1].undone).toBe(true)
    // The undo's own change isn't a new edit of yours.
    change('person:owner', 7, true)
    clock.advance(FINISH_MS)
    expect(snap().changes).toHaveLength(2)
    src.redoChange(rows[1].id)
    expect(ed.redo).toHaveBeenCalledTimes(1)
    expect(snap().changes[1].undone).toBe(false)
  })
})

describe('the connection', () => {
  it('reconnecting, then can’t reconnect after 30 s; Retry shows trying and asks the editor to reconnect now', () => {
    const { clock, ed, snap, src } = setup()
    ed.emit('state', 'reconnecting')
    expect(snap().connection).toBe('reconnecting')
    expect(snap().undoOff).toBe('Back online to use this')
    expect(snap().people.find((p) => p.id === 'person:sam')?.doing).toBe('last seen in Configuration')
    clock.advance(CANT_AFTER_MS)
    expect(snap().connection).toBe('cant')
    src.retry()
    expect(ed.reconnectNow).toHaveBeenCalledTimes(1)
    expect(snap().connection).toBe('trying')
    clock.advance(TRYING_MS)
    expect(snap().connection).toBe('cant')
    expect(snap().retried).toBe(true)
  })

  it('offline while the computer is', () => {
    const { ed, snap, src } = setup()
    ed.emit('state', 'reconnecting')
    src.setOnline(false)
    expect(snap().connection).toBe('offline')
    src.setOnline(true)
    expect(snap().connection).toBe('reconnecting')
  })

  it('offline edits are counted once, the same in the bar, the Changes row and Back', () => {
    const { clock, ed, change, snap } = setup()
    ed.emit('state', 'reconnecting')
    change('person:owner', 3, true)
    clock.advance(FINISH_MS)
    change('person:owner', 7, true)
    change('person:owner', 7, true)
    clock.advance(FINISH_MS)
    expect(snap().offlineEdits).toBe(2)
    const row = snap().changes.find((c) => c.id.startsWith('offline-'))
    expect(row?.text).toBe('· 2 offline edits · waiting to sync')
    expect(snap().changes).toHaveLength(1)
    ed.emit('state', 'live')
    expect(snap().connection).toBe('back')
    expect(snap().synced).toBe(2)
    expect(snap().changes.find((c) => c.id === row?.id)?.text).toBe('· 2 offline edits · synced')
    expect(snap().undoOff).toBeNull()
  })
})

describe('outside saves and refusals', () => {
  it('ingested: the notice’s save, the text from before it, and a Changes row', () => {
    const { ed, tap, change, snap } = setup()
    const before = ed.text
    ed.text = before.replace('A table.', 'A table, saved outside.')
    change('kit:external', 13)
    tap.frame({ type: 'outside_save', at_ms: 5, doc: 'd', outcome: 'ingested', rev: 41 })
    const s = snap()
    expect(s.outsideSave).toEqual({ outcome: 'ingested', rev: 41, at_ms: 5 })
    expect(s.beforeExternal).toBe(before)
    expect(s.changes.map((c) => [c.who, c.text, c.tone])).toEqual([['file', '· merged', 'outside']])
  })

  it('missing and declined', () => {
    const { tap, snap, src } = setup()
    tap.frame({ type: 'outside_save', at_ms: 6, doc: 'd', outcome: 'missing' })
    expect(snap().outsideSave?.outcome).toBe('missing')
    expect(snap().undoOff).toBe('Live edits are off until the file is back')
    tap.frame({ type: 'outside_save', at_ms: 7, doc: 'd', outcome: 'declined', code: 'not_text', message: 'It is not UTF-8' })
    expect(snap().outsideSave).toEqual({ outcome: 'declined', code: 'not_text', message: 'It is not UTF-8', at_ms: 7 })
    expect(snap().undoOff).toBe('Live edits are paused until the file is text again')
    src.clearOutsideSave()
    expect(snap().outsideSave).toBeNull()
  })

  it('verb_refused: a quiet row with Ask <holder>, the holder’s place as the target, and the sentence', () => {
    const { tap, snap } = setup()
    tap.frame({
      type: 'verb_refused',
      at_ms: 9,
      actor: { id: 'agent:atlas', name: 'Atlas', kind: 'agent', colour: null },
      client: 3,
      turn: null,
      verb: 'edit',
      unit: null,
      code: 'MPC-E301',
      holder: { id: 'person:sam', name: 'Sam', kind: 'person', colour: null },
      claim: 'claim-2',
    })
    const s = snap()
    expect(s.changes.map((c) => [c.who, c.text, c.tone, c.ask])).toEqual([['agent:atlas', 'couldn’t edit Configuration: Sam is holding it', 'refused', { id: 'person:sam', name: 'Sam' }]])
    expect(s.refusal).toEqual({ actor: 'Atlas', verb: 'edit', target: 'Configuration', holder: 'Sam', at_ms: 9 })
    expect(s.activity).toEqual({ who: 'agent:atlas', text: 'couldn’t edit Configuration: Sam is holding it' })
  })

  it('the removed-text note and a close', () => {
    const { ed, snap, src } = setup()
    const note = { kind: 'overwritten', to: 'person:owner', change: 4, version: 4, restore_from: 4, source: 'an outside save', rule: 'removed_text', row: null, line: 2, cells: [], more: 0, not_shown: 0, withheld: false, no_history: false, message: 'm' } as LiveEditorEvents['note']
    ed.emit('note', note)
    expect(snap().note).toBe(note)
    src.dismissNote()
    expect(snap().note).toBeNull()
    ed.emit('closed', { reason: 'closed', retry: false, code: 'core_off' } as LiveEditorEvents['closed'])
    expect(snap().closed).toEqual({ reason: 'closed', code: 'core_off' })
  })
})

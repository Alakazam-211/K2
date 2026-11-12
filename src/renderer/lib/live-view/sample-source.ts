// SampleWorkbookSource: plays K2 Cores' xlsx goldens as a live doc would send
// them, frame for frame, the way a KeyedWatch receives them: the welcome, a
// keyed snapshot (head, one page per root in schema order, then the units
// pages), the roster, and the turn's timeline (roster flushes, ops_change
// frames, turn_end), ending with the outside save. It is sample data, never
// the file the person opened: the view shows a banner saying so.
//
// Constructed, not in the goldens (marked where built): the roster snapshot
// (the goldens give people only at point P1), the frames' `seq` (the
// goldens assume the snapshot took seq 1-10; here it takes what its pages
// need, and the changes follow on), and the view schema's grid slot in the
// kit's form (samples/golden-schema.ts).

import type { KeyedPageFrame, KeyedSchema, RosterEntry } from '@k2/mpc-client'
import type { XlsxViewSnapshot } from './not-in-kit'
import type { LiveTier, LiveWorkbookFrame } from './reducer'
import type { LiveWorkbookSource, LiveWorkbookSourceEvent } from './source'
import { GOLDEN, stripAnnotations } from './samples/goldens'
import { kitViewSchema } from './samples/golden-schema'

/** What the sample shows: the agent's turn (live), a workbook too large to watch live (blob), or one K2 can't open (none). */
export type SampleScenario = 'turn' | 'blob' | 'none'

export interface SampleStep {
  label: string
  /** The golden's clock for this step (`at_ms`), when it has one. */
  atMs: number | null
  /** Wait before this step, at speed 1. */
  delayMs: number
  events: LiveWorkbookSourceEvent[]
}

export interface SampleOptions {
  scenario?: SampleScenario
  /** Start over after the end (default true). */
  loop?: boolean
  /** Playback speed (default 1: the goldens' own gaps, capped at `maxGapMs`). */
  speed?: number
  /** Start playing at once (default true). */
  autoplay?: boolean
  /** The longest wait between two steps (default 2,500 ms). */
  maxGapMs?: number
  /** The pause at the end before a loop starts over (default 4,000 ms). */
  loopPauseMs?: number
}

export interface SampleStatus {
  playing: boolean
  /** Steps sent so far. */
  position: number
  length: number
  ended: boolean
  /** The last step sent. */
  label: string | null
}

export const SAMPLE_FILE_NAME = 'budget-golden.xlsx'
const PAGE_GAP_MS = 140

/** The keyed snapshot of the goldens' rev 20: head, root pages in schema order, units pages per sheet. */
export function sampleSnapshotFrames(firstSeq = 1): LiveWorkbookFrame[] {
  const st = GOLDEN.state
  const order = GOLDEN.schema.root_order ?? st.roots.map((r) => r.name)
  const roots = order.map((n) => st.roots.find((r) => r.name === n)).filter((r): r is NonNullable<typeof r> => !!r)
  const schema: KeyedSchema = {
    name: 'xlsx',
    roots: roots.map((r) => ({ name: r.name, kind: r.kind })),
    composite: roots.filter((r) => r.composite_of).map((r) => ({ map: r.name, list: r.composite_of as string })),
  }
  const idx = stripAnnotations(GOLDEN.index)
  const view: XlsxViewSnapshot = { schema: kitViewSchema(stripAnnotations(GOLDEN.schema)), tier: idx.tier, rev: idx.rev, version: idx.version, page: null, fonts: idx.fonts, layouts: idx.layouts, containers: idx.containers }
  const containers = idx.containers.map((c) => c.key)
  const pages = roots.length + containers.length
  const upto = st.upto
  const out: LiveWorkbookFrame[] = [{ type: 'keyed_snapshot', seq: firstSeq, upto, schema, view, pages }]
  let page = 0
  for (const r of roots) {
    page++
    const f: KeyedPageFrame = { type: 'keyed_page', seq: firstSeq + page, upto, page, pages, root: r.name }
    if (r.kind === 'list') f.live = (r.live ?? []).map(([k, v]) => [k, v])
    else f.entries = (r.entries ?? []).map(([k, fld, v]) => [k, fld, v])
    // r1.1: the cells page carries the A1 text of the formula cells in it (the kit takes it from Unit.derived.a1).
    if (r.name === GOLDEN.schema.grid.cells) {
      const rowsHere = new Set((f.entries ?? []).map((e) => e[0]))
      const a1: Record<string, Record<string, string>> = {}
      for (const u of idx.units) {
        const m = u.derived?.a1
        if (!m || !rowsHere.has(u.key)) continue
        const row: Record<string, string> = {}
        for (const [fld, t] of Object.entries(m)) if (t != null) row[fld] = t
        a1[u.key] = row
      }
      f.derived = { a1 }
    }
    out.push(f)
  }
  for (const c of containers) {
    page++
    const units = idx.units.filter((u) => u.container === c)
    out.push({ type: 'keyed_page', seq: firstSeq + page, upto, page, pages, root: GOLDEN.schema.units, units: { container: c, from: 0, units, next: null, total: units.length } })
  }
  return out
}

/** Constructed: the roster at the start. People as point P1 has them; the agent with no presence yet (it gets it from its claim). */
export function sampleRosterSnapshot(): Extract<LiveWorkbookFrame, { type: 'roster_event' }> {
  const p1 = stripAnnotations(GOLDEN.presence.points[0])
  const members: RosterEntry[] = p1.peers.map((q) =>
    q.kind === 'agent'
      ? { handle: q.handle, name: q.name, colour: q.colour, kind: q.kind, role: 'member', away: false }
      : { handle: q.handle, name: q.name, colour: q.colour, kind: q.kind, role: 'member', away: q.away, focus: q.focus, selection: q.selection, cells: q.cells, viewport: q.viewport, pointer: q.pointer },
  )
  return { type: 'roster_event', event: { kind: 'snapshot', version: GOLDEN.state.upto, members } }
}

/** The sample's steps for a scenario, in order. */
export function sampleSteps(scenario: SampleScenario = 'turn', maxGapMs = 2500): SampleStep[] {
  const tl = GOLDEN.turn.timeline
  const welcome = tl.find((e) => e.event === 'welcome')?.frame
  if (!welcome || welcome.type !== 'welcome') throw new Error('the goldens have no welcome')
  const tier = (t: LiveTier): LiveWorkbookFrame => ({ type: 'tier', tier: t })
  const steps: SampleStep[] = [{ label: 'welcome', atMs: tl[0].at_ms, delayMs: 0, events: [stripAnnotations(welcome), tier(scenario === 'turn' ? 'live' : scenario)] }]
  if (scenario === 'none') return steps
  const snap = sampleSnapshotFrames(1)
  snap.forEach((f, i) => steps.push({ label: f.type === 'keyed_snapshot' ? 'snapshot' : `page ${i}`, atMs: null, delayMs: i === 0 ? 250 : PAGE_GAP_MS, events: [f] }))
  if (scenario === 'blob') return steps
  steps.push({ label: 'roster', atMs: null, delayMs: 300, events: [sampleRosterSnapshot()] })
  const lastSnap = snap[snap.length - 1]
  let seq = lastSnap.type === 'keyed_page' || lastSnap.type === 'keyed_snapshot' ? lastSnap.seq : 1
  let prevAt: number | null = null
  for (const e of tl) {
    if (e.event === 'welcome' || e.event === 'keyed_snapshot') continue
    const gap = prevAt == null ? 900 : Math.min(maxGapMs, Math.max(0, e.at_ms - prevAt))
    prevAt = e.at_ms
    if (e.event === 'roster_flush') {
      const events = stripAnnotations(e.roster_events ?? []).map((ev): LiveWorkbookFrame => ({ type: 'roster_event', event: ev }))
      // A flush right after its frame lands with it.
      const last = steps[steps.length - 1]
      if (e.after_rev != null && last.events.some((x) => x.type === 'ops_change' && x.rev === e.after_rev)) last.events.push(...events)
      else steps.push({ label: events.map((x) => (x.type === 'roster_event' ? x.event.kind : x.type)).join(', '), atMs: e.at_ms, delayMs: gap, events })
    } else if (e.event === 'ops_change') {
      const f = GOLDEN.turn.frames.find((x) => x.rev === e.rev)
      if (!f) throw new Error(`the goldens have no frame for rev ${e.rev}`)
      seq += 1
      steps.push({ label: `rev ${f.rev}`, atMs: e.at_ms, delayMs: gap, events: [{ ...stripAnnotations(f), type: 'ops_change', seq }] })
    } else if (e.event === 'turn_end' && e.frame?.type === 'turn_end') {
      steps.push({ label: 'turn_end', atMs: e.at_ms, delayMs: gap, events: [stripAnnotations(e.frame)] })
    } else if (e.event === 'turn_ended_by_idle') {
      // Nothing is sent: the client's clock reaches turn_idle_ms after the agent's last change (r1.1).
      steps.push({ label: 'idle', atMs: e.at_ms, delayMs: gap, events: [{ type: 'clock', at_ms: e.at_ms }] })
    }
  }
  return steps
}

/** Plays the sample steps on a timer, with play, pause and step; loops or ends cleanly. */
export class SampleWorkbookSource implements LiveWorkbookSource {
  readonly kind = 'sample' as const
  readonly scenario: SampleScenario
  readonly steps: readonly SampleStep[]
  private at = 0
  private sent: LiveWorkbookSourceEvent[] = []
  private readonly subs = new Set<(e: LiveWorkbookSourceEvent) => void>()
  private readonly statusSubs = new Set<(s: SampleStatus) => void>()
  private timer: ReturnType<typeof setTimeout> | null = null
  private playing = false
  private closed = false
  private readonly loop: boolean
  private readonly speed: number
  private readonly loopPauseMs: number

  constructor(opts: SampleOptions = {}) {
    this.scenario = opts.scenario ?? 'turn'
    this.steps = sampleSteps(this.scenario, opts.maxGapMs ?? 2500)
    this.loop = opts.loop ?? true
    this.speed = opts.speed && opts.speed > 0 ? opts.speed : 1
    this.loopPauseMs = opts.loopPauseMs ?? 4000
    if (opts.autoplay ?? true) this.play()
  }

  subscribe(onEvent: (e: LiveWorkbookSourceEvent) => void): () => void {
    this.subs.add(onEvent)
    return () => {
      this.subs.delete(onEvent)
    }
  }

  snapshot(): readonly LiveWorkbookSourceEvent[] {
    return this.sent.slice()
  }

  status(): SampleStatus {
    return { playing: this.playing, position: this.at, length: this.steps.length, ended: this.at >= this.steps.length, label: this.at > 0 ? this.steps[this.at - 1].label : null }
  }

  onStatus(cb: (s: SampleStatus) => void): () => void {
    this.statusSubs.add(cb)
    return () => {
      this.statusSubs.delete(cb)
    }
  }

  play(): void {
    if (this.closed || this.playing) return
    if (this.at >= this.steps.length) this.restart()
    this.playing = true
    this.schedule()
    this.notify()
  }

  pause(): void {
    if (!this.playing) return
    this.playing = false
    this.clear()
    this.notify()
  }

  /** One step forward, paused. At the end: nothing (or, looping, the start again). */
  step(): void {
    if (this.closed) return
    this.playing = false
    this.clear()
    if (this.at >= this.steps.length) {
      if (!this.loop) return this.notify()
      this.restart()
    }
    this.sendNext()
    this.notify()
  }

  /** Back to the start: subscribers get a `reset`, then the steps play again from the welcome. */
  restart(): void {
    if (this.closed) return
    this.clear()
    this.at = 0
    this.sent = []
    this.emit({ type: 'reset' }, false)
    if (this.playing) this.schedule()
    this.notify()
  }

  close(): void {
    this.closed = true
    this.playing = false
    this.clear()
    this.subs.clear()
    this.statusSubs.clear()
  }

  private clear(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
  }

  private schedule(): void {
    this.clear()
    if (!this.playing || this.closed) return
    if (this.at < this.steps.length) {
      const wait = this.steps[this.at].delayMs / this.speed
      this.timer = setTimeout(() => {
        this.timer = null
        this.sendNext()
        this.schedule()
        this.notify()
      }, wait)
    } else if (this.loop) {
      this.timer = setTimeout(() => {
        this.timer = null
        this.restart()
      }, this.loopPauseMs / this.speed)
    } else {
      this.playing = false
      this.notify()
    }
  }

  private sendNext(): void {
    const s = this.steps[this.at]
    if (!s) return
    this.at += 1
    for (const e of s.events) this.emit(e, true)
  }

  private emit(e: LiveWorkbookSourceEvent, keep: boolean): void {
    if (keep) this.sent.push(e)
    for (const cb of this.subs) cb(e)
  }

  private notify(): void {
    const s = this.status()
    for (const cb of this.statusSubs) cb(s)
  }
}

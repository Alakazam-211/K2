// The live workbook's client state, as one pure reducer over what a source
// sends: the welcome, the keyed watch's frames (snapshot head, pages, changes)
// folded by the kit's own `KeyedWatch` (keyed-fold.ts), the roster's presence and
// claim events, and the host's `turn_end`. Plus the client's own gestures
// (follow on or off). No React, no DOM: the view renders `LiveWorkbookState`.

import type {
  CellRange,
  Cells,
  ClaimActor,
  ElemFocus,
  ElemSelection,
  ErrorReport,
  KeyedPageFrame,
  KeyedSchema,
  KeyedSnapshotFrame,
  OpsChangeFrame,
  OutsideSaveFrame,
  Point,
  Rect,
  RosterEntry,
  RosterEvent,
  VerbRefusedFrame,
} from '@k2/mpc-client'
import { NO_CELL_DERIVED, withPageDerived, type CellDerivedValues } from './cell-derived'
import { checkPage, foldSnapshot, KeyedMisfit } from './keyed-fold'
import type { ClockFrame, TurnEndFrame, XlsxViewSnapshot, XlsxViewUnit, XlsxWelcome } from './not-in-kit'
import { actorFrom, framePhrase, type Actor, type LogEntry } from './wording'
import { applyFrameToBook, bookFromSnapshot, cornerPlace, gridOf, overlaps, rangeA1, rangeRect, sheetOfRow, type Book, type Rect4 } from './workbook'

/** What a source sends: the kit's frames (`@k2/mpc-client`), the host's own (not-in-kit.ts) and its tier. */
export type LiveWorkbookFrame =
  | ({ type: 'welcome' } & XlsxWelcome)
  | (Omit<KeyedSnapshotFrame, 'view'> & { view: XlsxViewSnapshot | null })
  | KeyedPageFrame
  | OpsChangeFrame
  | { type: 'roster_event'; event: RosterEvent }
  | TurnEndFrame
  /** The host's word on the doc's tier: live, the blob tier (too large to watch live; read-only), or no doc (32 MiB and up). */
  | { type: 'tier'; tier: LiveTier }
  | { type: 'watch_refused'; error: ErrorReport }
  /** Kit v0.19.0's unnumbered keyed-watch notices: what came of an outside save, a refused verb. */
  | OutsideSaveFrame
  | VerbRefusedFrame
  | ClockFrame

export type LiveTier = 'live' | 'blob' | 'none'

/** The client's own gestures. */
export type LiveWorkbookAction = LiveWorkbookFrame | { type: 'local_follow'; on: boolean } | { type: 'reset' }

export type PeerMode = 'editing' | 'selecting' | 'viewing'

/** One person or agent on the doc (the kit's `PresencePeer`, plus `cells`). */
export interface Peer {
  handle: string
  name: string
  kind: string
  colour: string
  away: boolean
  isSelf: boolean
  sameUser: boolean
  focus: ElemFocus | null
  selection: ElemSelection | null
  cells: Cells | null
  viewport: Rect | null
  pointer: Point | null
}

/**
 * A live claim (RC Item 4's record, with r1's cells scope). A claim on cells
 * has exactly one range, as the kit's (`claim_started_cells`, `ClaimView.cells`,
 * `PresenceChannel.claimCells`): `ranges` keeps the kit's list form, and the
 * view reads only `ranges[0]` (`claimRect`, the claim's log line, `claimVerdict`).
 */
export interface ClaimRecord {
  id: string
  actor: ClaimActor
  scope: 'cells' | 'rows' | 'sheets'
  container: string
  keys: string[] | null
  ranges: CellRange[] | null
  label: string
  hold: boolean
  started_ms: number | null
  expires_ms: number
}

export interface EndedClaim {
  id: string
  actor: ClaimActor | null
  how: string
  at_ms: number | null
  at_rev: number | null
  lines: { first: number; last: number } | null
}

/** A change still on screen: its books before and after, for outlines and "was / now". Age 0 is the latest. */
export interface RecentChange {
  frame: OpsChangeFrame
  before: Book
  after: Book
  age: number
}

/** The snapshot being built, page by page: its head and the pages so far (each checked as it came), folded when whole. */
export interface Building {
  head: KeyedSnapshotFrame
  pagesIn: readonly KeyedPageFrame[]
  schema: KeyedSchema
  view: XlsxViewSnapshot | null
  got: number
  pages: number
  upto: number
  /** The roots whose pages are in, in order. */
  rootsIn: string[]
  /** r1.1: the cell-derived values (A1 text) the rows pages carried, kept as KeyedWatch keeps them. */
  cellDerived: CellDerivedValues
}

export interface LiveWorkbookState {
  tier: LiveTier
  welcome: XlsxWelcome | null
  /** The last numbered frame applied (the kit's `seq`). */
  seq: number
  /** A `watch` is out (a gap or a misfit): frames are dropped until the next snapshot head. */
  awaiting: boolean
  building: Building | null
  book: Book | null
  peers: Readonly<Record<string, Peer>>
  claims: readonly ClaimRecord[]
  ended: readonly EndedClaim[]
  log: readonly LogEntry[]
  recent: readonly RecentChange[]
  /** The rev of the first live write to a cell (Q-X10: results not recalculated since). */
  firstLiveEdit: number | null
  /** Your client follows the agent (the default). */
  follow: boolean
  turnEnds: Readonly<Record<string, number>>
  /** Why the watch last asked for a fresh snapshot. */
  resync: { why: string; detail: string } | null
  refused: ErrorReport | null
  /** Counts every input that changed the state: a key for the view's animations. */
  tick: number
  /** The log's running number. */
  logSeq: number
  /** The latest time seen, on the host's clock (frames, claims, turn_end, the client's clock): ends idle turns. */
  now: number | null
}

/** The history keeps this many entries (the oldest go first). */
export const LOG_MAX = 500

export function initialLiveState(): LiveWorkbookState {
  return {
    tier: 'live',
    welcome: null,
    seq: 0,
    awaiting: true,
    building: null,
    book: null,
    peers: {},
    claims: [],
    ended: [],
    log: [],
    recent: [],
    firstLiveEdit: null,
    follow: true,
    turnEnds: {},
    resync: null,
    refused: null,
    tick: 0,
    logSeq: 0,
    now: null,
  }
}

const selfHandle = (s: LiveWorkbookState): string | null => s.welcome?.self ?? null

function blankPeer(handle: string, a: { name: string; kind: string; colour: string | null } | null, self: string | null): Peer {
  const actor = actorFrom(a ? { id: handle, ...a } : { id: handle, name: handle, kind: 'person', colour: null })
  return { handle, name: actor.name, kind: actor.kind, colour: actor.colour, away: false, isSelf: handle === self, sameUser: false, focus: null, selection: null, cells: null, viewport: null, pointer: null }
}

function peerFromEntry(e: RosterEntry, self: string | null): Peer {
  return {
    ...blankPeer(e.handle, { name: e.name, kind: e.kind, colour: e.colour }, self),
    away: !!e.away,
    focus: e.focus ?? null,
    selection: e.selection ?? null,
    cells: e.cells ?? null,
    viewport: e.viewport ?? null,
    pointer: e.pointer ?? null,
  }
}

/** The agent the view follows: the last agent that wrote, else any agent here. */
export function agentId(s: LiveWorkbookState): string | null {
  for (let i = s.log.length - 1; i >= 0; i--) if (s.log[i].actor.kind === 'agent') return s.log[i].actor.id
  return Object.values(s.peers).find((p) => p.kind === 'agent')?.handle ?? null
}

export function agentFocus(s: LiveWorkbookState): string | null {
  const a = agentId(s)
  return (a && s.peers[a]?.focus?.key) || null
}

/** Your client follows the agent: when its focus moves to another sheet, yours does too, which drops your cells (CS10). */
function followSync(s: LiveWorkbookState): LiveWorkbookState {
  const self = selfHandle(s)
  const af = agentFocus(s)
  const me = self ? s.peers[self] : undefined
  if (!s.follow || !me || !af || !s.book || (me.focus && me.focus.key === af)) return s
  const next: Peer = { ...me, focus: { root: s.book.schema.containers, key: af }, cells: me.cells && me.cells.container !== af ? null : me.cells }
  return { ...s, peers: { ...s.peers, [me.handle]: next } }
}

type LogInput = LogEntry extends infer E ? (E extends LogEntry ? Omit<E, 'seq'> : never) : never

function pushLog(s: LiveWorkbookState, e: LogInput): Pick<LiveWorkbookState, 'log' | 'logSeq'> {
  const entry = { ...e, seq: s.logSeq + 1 } as LogEntry
  const log = s.log.length >= LOG_MAX ? [...s.log.slice(s.log.length - LOG_MAX + 1), entry] : [...s.log, entry]
  return { log, logSeq: s.logSeq + 1 }
}

function asked(s: LiveWorkbookState, why: string, detail: string): LiveWorkbookState {
  return { ...s, awaiting: true, building: null, resync: { why, detail } }
}

// ---------- the keyed watch (KeyedWatch's rules) ----------

/** The head as the kit's frame: the xlsx view is the kit's view plus r1's provisional fields. */
const kitHead = (f: Extract<LiveWorkbookFrame, { type: 'keyed_snapshot' }>): KeyedSnapshotFrame => f as KeyedSnapshotFrame

function onHead(s: LiveWorkbookState, f: Extract<LiveWorkbookFrame, { type: 'keyed_snapshot' }>): LiveWorkbookState {
  const building: Building = { head: kitHead(f), pagesIn: [], schema: f.schema, view: f.view ?? null, got: 0, pages: f.pages, upto: f.upto, rootsIn: [], cellDerived: NO_CELL_DERIVED }
  const next: LiveWorkbookState = { ...s, awaiting: false, seq: f.seq, building, refused: null }
  return f.pages === 0 ? finish(next) : next
}

function onPage(s: LiveWorkbookState, f: KeyedPageFrame): LiveWorkbookState {
  if (s.awaiting) return s
  if (f.seq !== s.seq + 1) return asked(s, 'gap', `page seq ${f.seq} after ${s.seq}`)
  const b = s.building
  if (!b) return asked(s, 'inconsistent', `a page with no snapshot (seq ${f.seq})`)
  if (f.page !== b.got + 1 || f.pages !== b.pages || f.upto !== b.upto) return asked(s, 'inconsistent', `page ${f.page} of ${f.pages} out of order`)
  // The page is checked by the kit's watch now; the snapshot is folded once it is whole.
  try {
    checkPage(b.head, b.pagesIn, f)
  } catch (e) {
    if (e instanceof KeyedMisfit) return asked(s, 'inconsistent', e.message)
    throw e
  }
  const rootsIn = f.units || b.rootsIn.includes(f.root) ? b.rootsIn : [...b.rootsIn, f.root]
  const cellDerived = withPageDerived(b.cellDerived, f, b.view?.schema.grid)
  const nb: Building = { ...b, pagesIn: [...b.pagesIn, f], got: b.got + 1, rootsIn, cellDerived }
  const next: LiveWorkbookState = { ...s, seq: f.seq, building: nb }
  return nb.got === nb.pages ? finish(next) : next
}

/** The building snapshot folded (the kit's watch over its head and pages); units carry the xlsx `derived` they came with. */
function foldBuilding(b: Building): Book | null {
  if (!b.view) return null
  const { state, units } = foldSnapshot(b.head, b.pagesIn)
  return bookFromSnapshot(state, b.view, units as Map<string, XlsxViewUnit[]>, b.upto, b.cellDerived)
}

function finish(s: LiveWorkbookState): LiveWorkbookState {
  const b = s.building as Building
  if (!b.view) return { ...s, building: null, book: null, resync: { why: 'inconsistent', detail: 'the snapshot has no view index' } }
  let book: Book | null
  try {
    book = foldBuilding(b)
  } catch (e) {
    if (e instanceof KeyedMisfit) return asked(s, 'inconsistent', e.message)
    throw e
  }
  return followSync({ ...s, building: null, book, recent: [] })
}

function onOps(s: LiveWorkbookState, f: OpsChangeFrame): LiveWorkbookState {
  if (s.awaiting) return s
  if (f.seq !== s.seq + 1) return asked(s, 'gap', `change seq ${f.seq} after ${s.seq}`)
  if (s.building || !s.book) return asked(s, 'inconsistent', `a change with no whole snapshot (seq ${f.seq})`)
  if (f.rev <= s.book.rev) return asked(s, 'inconsistent', `a change at rev ${f.rev}, at or before ${s.book.rev}`)
  const before = s.book
  let after: Book
  try {
    after = applyFrameToBook(before, f)
  } catch (e) {
    if (e instanceof KeyedMisfit) return asked(s, 'inconsistent', e.message)
    throw e
  }
  const phrase = framePhrase(f, before, after)
  const actor = actorFrom(f.actor)
  const writesCells = f.ops.some((o) => o.kind === 'map' && o.root === after.schema.grid.cells && o.field != null && !o.field.startsWith(after.schema.grid.row_fields_start))
  const recent: RecentChange[] = [{ frame: f, before, after, age: 0 }, ...s.recent.map((r) => ({ ...r, age: r.age + 1 }))].filter((r) => r.age < 2)
  const next: LiveWorkbookState = {
    ...s,
    seq: f.seq,
    book: after,
    recent,
    firstLiveEdit: s.firstLiveEdit ?? (writesCells && actor.kind !== 'file' ? f.rev : null),
    ...pushLog(s, { kind: 'frame', rev: f.rev, turn: f.turn, actor, at: f.at_ms ?? null, phrase, verb: f.verb, claim: f.claim }),
  }
  return followSync(next)
}

// ---------- the roster ----------

function setPeer(s: LiveWorkbookState, who: string, patch: (p: Peer) => Peer): LiveWorkbookState {
  const p = s.peers[who] ?? blankPeer(who, null, selfHandle(s))
  return { ...s, peers: { ...s.peers, [who]: patch(p) } }
}

function onRoster(s: LiveWorkbookState, e: RosterEvent): LiveWorkbookState {
  const self = selfHandle(s)
  switch (e.kind) {
    case 'snapshot': {
      const peers: Record<string, Peer> = {}
      for (const m of e.members) peers[m.handle] = peerFromEntry(m, self)
      return followSync({ ...s, peers })
    }
    case 'joined':
      return { ...s, peers: { ...s.peers, [e.who]: blankPeer(e.who, e.participant, self) } }
    case 'left': {
      const peers = { ...s.peers }
      delete peers[e.who]
      return { ...s, peers }
    }
    case 'away':
      return setPeer(s, e.who, (p) => ({ ...p, away: e.away }))
    case 'selection':
      return setPeer(s, e.who, (p) => ({ ...p, selection: e.selection }))
    case 'pointer':
      return setPeer(s, e.who, (p) => ({ ...p, pointer: e.pointer }))
    case 'viewport':
      return setPeer(s, e.who, (p) => ({ ...p, viewport: e.viewport }))
    case 'focus':
      // A focus move to another sheet drops that connection's cells (CS10); the host sends the same as an event too.
      return followSync(setPeer(s, e.who, (p) => ({ ...p, focus: e.focus, cells: p.cells && e.focus && p.cells.container !== e.focus.key ? null : p.cells })))
    case 'cells':
      // Setting cells sets the focus to its container (Item 3.4).
      return setPeer(s, e.who, (p) => ({ ...p, cells: e.cells, focus: e.cells && s.book ? { root: s.book.schema.containers, key: e.cells.container } : p.focus }))
    case 'claim_started_cells': {
      const c: ClaimRecord = { id: e.claim, actor: e.actor, scope: 'cells', container: e.container, keys: null, ranges: e.ranges, label: e.note ?? '', hold: e.hold, started_ms: e.started_ms ?? null, expires_ms: e.expires_ms }
      const g = s.book ? gridOf(s.book, c.container) : null
      const place = g && c.ranges?.[0] ? rangeA1(g, c.ranges[0]) : null
      return { ...s, claims: [...s.claims.filter((x) => x.id !== c.id), c], ...pushLog(s, { kind: 'claim', id: c.id, actor: actorFrom(e.actor), at: c.started_ms, label: c.label, place, sheet: c.container, hold: c.hold }) }
    }
    case 'claim_started_elems': {
      const actor: ClaimActor = e.actor ?? (() => { const p = s.peers[e.who]; return { id: e.who, name: p?.name ?? e.who, kind: p?.kind ?? 'agent', colour: p?.colour ?? 'var(--color-accent)' } })()
      const container = s.book && e.keys[0] ? (e.root === s.book.schema.containers ? e.keys[0] : sheetOfRow(s.book, e.keys[0])) : null
      if (!container) return s
      const c: ClaimRecord = { id: e.claim, actor, scope: s.book && e.root === s.book.schema.containers ? 'sheets' : 'rows', container, keys: e.keys, ranges: null, label: e.note ?? '', hold: e.hold, started_ms: e.started_ms ?? null, expires_ms: e.expires_ms }
      return { ...s, claims: [...s.claims.filter((x) => x.id !== c.id), c], ...pushLog(s, { kind: 'claim', id: c.id, actor: actorFrom(actor), at: c.started_ms, label: c.label, place: null, sheet: container, hold: c.hold }) }
    }
    case 'claim_ended': {
      const c = s.claims.find((x) => x.id === e.claim)
      const actor = e.actor ?? c?.actor ?? null
      const lines = e.lines ? { first: e.lines.first, last: e.lines.last } : null
      const ended: EndedClaim = { id: e.claim, actor, how: e.how, at_ms: e.at_ms ?? null, at_rev: e.at_rev ?? null, lines }
      return {
        ...s,
        claims: s.claims.filter((x) => x.id !== e.claim),
        ended: [...s.ended, ended],
        ...pushLog(s, { kind: 'claim_end', id: e.claim, actor: actorFrom(actor ?? { id: e.who, name: e.who, kind: 'agent', colour: null }), at: e.at_ms ?? null, how: e.how, label: c?.label ?? null, lines }),
      }
    }
    // The kit's other roster kinds (ranges, watching counts, text claims...): nothing a workbook view folds.
    default:
      return s
  }
}

// ---------- the reducer ----------

export function liveWorkbookReducer(s: LiveWorkbookState, a: LiveWorkbookAction): LiveWorkbookState {
  const next = step(s, a)
  return next === s ? s : { ...next, tick: s.tick + 1 }
}

const later = (now: number | null, at: number | null | undefined): number | null => (at == null ? now : now == null ? at : Math.max(now, at))

function step(s0: LiveWorkbookState, a: LiveWorkbookAction): LiveWorkbookState {
  // The host's clock moves forward with every timed input.
  const at = a.type === 'ops_change' || a.type === 'turn_end' || a.type === 'clock' ? a.at_ms : a.type === 'roster_event' && (a.event.kind === 'claim_started_cells' || a.event.kind === 'claim_started_elems') ? a.event.started_ms : a.type === 'roster_event' && a.event.kind === 'claim_ended' ? a.event.at_ms : null
  const now = later(s0.now, at)
  const s = now === s0.now ? s0 : { ...s0, now }
  switch (a.type) {
    case 'clock':
      return s
    case 'reset':
      return initialLiveState()
    case 'local_follow':
      return a.on === s.follow ? s : followSync({ ...s, follow: a.on })
    case 'welcome': {
      const { type: _t, ...w } = a
      const self = w.self
      const peers: Record<string, Peer> = {}
      for (const [k, p] of Object.entries(s.peers)) peers[k] = { ...p, isSelf: k === self }
      return { ...s, welcome: w, peers }
    }
    case 'tier':
      return { ...s, tier: a.tier }
    case 'watch_refused':
      return { ...s, awaiting: false, refused: a.error }
    // Notices, not state: the doc's change came in its own `ops_change`. Nothing a workbook view draws yet.
    case 'outside_save':
    case 'verb_refused':
      return s
    case 'keyed_snapshot':
      return onHead(s, a)
    case 'keyed_page':
      return onPage(s, a)
    case 'ops_change':
      return onOps(s, a)
    case 'roster_event':
      return onRoster(s, a.event)
    case 'turn_end':
      return { ...s, turnEnds: { ...s.turnEnds, [a.turn]: a.at_ms }, ...pushLog(s, { kind: 'turn_end', turn: a.turn, actor: actorFrom(a.actor), at: a.at_ms }) }
  }
}

// ---------- derived for the view ----------

/** A claim's rectangle on its sheet: a cells claim's range, a rows claim's band. */
export function claimRect(book: Book, c: ClaimRecord): Rect4 | null {
  const g = gridOf(book, c.container)
  if (!g) return null
  if (c.scope === 'cells' && c.ranges?.[0]) return rangeRect(g, c.ranges[0])
  if (c.scope === 'rows' && c.keys) {
    const ns = c.keys.map((k) => g.rowByKey.get(k)?.n).filter((n): n is number => n != null)
    return ns.length ? { r0: Math.min(...ns), r1: Math.max(...ns), c0: 1, c1: book.schema.grid.max.cols } : null
  }
  return null
}

/** The client's derived mode: editing = holds a claim covering its cells or selection; selecting = has a selection or a range; else viewing. */
export function modeOf(book: Book, p: Peer, claims: readonly ClaimRecord[]): PeerMode {
  const mine = claims.filter((c) => c.actor.id === p.handle)
  const areas: { sheet: string; R: Rect4 }[] = []
  if (p.cells) {
    const g = gridOf(book, p.cells.container)
    if (g) for (const r of p.cells.ranges) { const R = rangeRect(g, r); if (R) areas.push({ sheet: p.cells.container, R }) }
  }
  if (p.selection && p.selection.root === book.schema.grid.rows) {
    for (const k of p.selection.keys) {
      const sk = sheetOfRow(book, k)
      const r = sk ? gridOf(book, sk)?.rowByKey.get(k) : undefined
      if (sk && r) areas.push({ sheet: sk, R: { r0: r.n, r1: r.n, c0: 1, c1: book.schema.grid.max.cols } })
    }
  }
  if (mine.some((c) => { const CR = claimRect(book, c); return !!CR && areas.some((a) => a.sheet === c.container && overlaps(a.R, CR)) })) return 'editing'
  if ((p.selection && p.selection.keys.length) || (p.cells && p.cells.ranges.length)) return 'selecting'
  return 'viewing'
}

/** A peer's caret as numbers on a sheet, when it is there and places. */
export function caretOn(book: Book, p: Peer | undefined, sheet: string): { r: number; c: number } | null {
  if (!p?.cells || p.cells.container !== sheet || !p.cells.active) return null
  const g = gridOf(book, sheet)
  return g ? cornerPlace(g, p.cells.active) : null
}

/** The other people and agents (not you). */
export function others(s: LiveWorkbookState): Peer[] {
  return Object.values(s.peers).filter((p) => !p.isSelf)
}

/** Loading progress: 0..1, and the phase the loading card names. */
export function loadingOf(s: LiveWorkbookState): { phase: 'connect' | 'pages' | 'live'; done: number; total: number; rootsIn: string[]; roots: string[] } {
  if (s.book && !s.building) return { phase: 'live', done: 1, total: 1, rootsIn: [], roots: [] }
  const b = s.building
  if (!b) return { phase: 'connect', done: 0, total: 1, rootsIn: [], roots: [] }
  return { phase: 'pages', done: b.got, total: b.pages, rootsIn: b.rootsIn, roots: b.schema.roots.map((r) => r.name) }
}

/**
 * The book so far while the snapshot streams: once the cells page is in
 * (columns come before cells in r1's root order), the grid can draw with
 * the A1 text the cells pages carried (r1.1). Null before that.
 */
export function buildingBook(s: LiveWorkbookState): Book | null {
  const b = s.building
  if (!b?.view || !b.rootsIn.includes(b.view.schema.grid.cells)) return null
  try {
    return foldBuilding(b)
  } catch (e) {
    if (e instanceof KeyedMisfit) return null
    throw e
  }
}

/** The welcome's idle time after which a turn shows as ended with no `turn_end` (r1.1). */
export const idleOf = (s: LiveWorkbookState): { idleMs: number | null; now: number | null } => ({ idleMs: s.welcome?.turn_idle_ms ?? null, now: s.now })

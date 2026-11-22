// The words for changes (RC §3, addendum §3): K2 writes them from the
// actor's name, the verb, the ops and the sheet's title. Cell places are
// A1 at the change's rev, worked out when the frame lands; the history keeps
// that text while the outline follows the keys.

import { colLetters, colOfCellField, type Band, type OpsChangeFrame, type WatchOp } from '@k2/mpc-client'
import { a1Text, cornerPlace, gridOf, rectA1, sheetOfRow, titleOf, type Book, type Rect4 } from './workbook'
import { PRODUCT_NAME } from '@/lib/brand'

/** Who a change or claim is by, as the view shows them. */
export interface Actor {
  id: string
  name: string
  kind: string
  /** A CSS colour: the actor's, or K2's own for a file. */
  colour: string
}

/** K2's name and colour for an outside save's actor (`kit:external`, colour null: the host's to choose). */
export const FILE_ACTOR_NAME = `Saved outside ${PRODUCT_NAME}`
export const FILE_ACTOR_COLOUR = 'var(--color-text-muted)'

export function actorFrom(a: { id: string; name: string; kind: string; colour: string | null } | null | undefined): Actor {
  if (!a) return { id: 'unknown', name: 'Someone', kind: 'person', colour: 'var(--color-accent)' }
  if (a.kind === 'file') return { id: a.id, name: FILE_ACTOR_NAME, kind: 'file', colour: a.colour ?? FILE_ACTOR_COLOUR }
  return { id: a.id, name: a.name, kind: a.kind, colour: a.colour ?? 'var(--color-accent)' }
}

/** Where a line's click takes you. */
export interface PhraseTarget {
  sheet: string
  r: number
  c: number
  /** The whole row (an insert or delete). */
  row?: boolean
}

/** One change in words. `text` follows the actor's name ("Atlas set D5:D7 on Budget"); an outside save is worded by the file. */
export interface Phrase {
  /** The actor's display name; null for an outside save (the view names the file). */
  who: string | null
  /** "was saved outside K2: " for an outside save, else empty. */
  lead: string
  text: string
  /** The place, as text: A1, a row number, a sheet title. */
  place: string
  sheet: string | null
  target: PhraseTarget | null
}

/** The line as one string: "Atlas set D5:D7 on Budget"; "budget.xlsx was saved outside K2: C4 and 9 results changed". */
export function phraseLine(p: Phrase, fileName: string): string {
  return `${p.who ?? fileName} ${p.lead}${p.text}`
}

const isCellOp = (book: Book, o: WatchOp): o is Extract<WatchOp, { kind: 'map' }> =>
  o.kind === 'map' && o.root === book.schema.grid.cells && o.field != null && !o.field.startsWith(book.schema.grid.row_fields_start)

/** The cells an op list writes, as numbers at the book's rev. */
function cellPoints(book: Book, ops: readonly Extract<WatchOp, { kind: 'map' }>[]): { sheet: string; pts: { r: number; c: number }[] } | null {
  if (!ops.length) return null
  const sheet = sheetOfRow(book, ops[0].key)
  const g = sheet ? gridOf(book, sheet) : null
  if (!sheet || !g) return null
  const pts: { r: number; c: number }[] = []
  for (const o of ops) {
    const r = g.rowByKey.get(o.key)
    const address = colOfCellField(book.schema.grid, sheet, o.field as string)
    const c = address === null ? undefined : g.colByKey.get(address)
    if (r && c) pts.push({ r: r.n, c: c.n })
  }
  return pts.length ? { sheet, pts } : null
}

const box = (pts: { r: number; c: number }[]): Rect4 => ({
  r0: Math.min(...pts.map((p) => p.r)),
  r1: Math.max(...pts.map((p) => p.r)),
  c0: Math.min(...pts.map((p) => p.c)),
  c1: Math.max(...pts.map((p) => p.c)),
})

function bandWords(b: Band, title: string, blank: boolean): { text: string; place: string } {
  const one = b.from === b.to
  const at = (n: number): string => (b.axis === 'rows' ? String(n) : colLetters(n))
  const noun = b.axis === 'rows' ? (one ? 'row' : 'rows') : one ? 'column' : 'columns'
  const span = one ? at(b.from) : `${at(b.from)}–${at(b.to)}`
  if (b.op === 'insert') return { text: one ? `inserted a ${noun} at ${span} on ${title}` : `inserted ${b.to - b.from + 1} ${noun} at ${span} on ${title}`, place: at(b.from) }
  return { text: `deleted ${noun} ${span} on ${title}${blank ? (one ? `, a blank ${noun}` : ', all blank') : ''}`, place: at(b.from) }
}

/** One frame in words, from the books before and after it. */
export function framePhrase(f: OpsChangeFrame, B0: Book, B1: Book): Phrase {
  const file = f.actor.kind === 'file'
  const who = file ? null : actorFrom(f.actor).name
  const lead = file ? `was saved outside ${PRODUCT_NAME}: ` : ''
  const out = (text: string, place: string | number, sheet: string | null, target: PhraseTarget | null = null): Phrase => ({ who, lead, text, place: String(place), sheet, target })
  const sch = B1.schema
  const gs = sch.grid
  const CONT = sch.containers
  const CPROPS = B1.state.schema.composite?.find((c) => c.list === CONT)?.map ?? null
  const titleField = typeof sch.container_fields.title === 'string' ? sch.container_fields.title : null
  const hiddenField = typeof sch.container_fields.hidden === 'string' ? sch.container_fields.hidden : null
  const L = f.ops.filter((o): o is Extract<WatchOp, { kind: 'list' }> => o.kind === 'list')
  const M = f.ops.filter((o): o is Extract<WatchOp, { kind: 'map' }> => o.kind === 'map')

  const sAdd = L.find((o) => o.root === CONT && o.op === 'insert')
  if (sAdd) return out(`added the sheet ${titleOf(B1, sAdd.key)}`, titleOf(B1, sAdd.key), sAdd.key)
  const sDel = L.find((o) => o.root === CONT && o.op === 'delete')
  if (sDel) return out(`deleted the sheet ${titleOf(B0, sDel.key)}`, titleOf(B0, sDel.key), null)
  const ren = M.find((o) => o.root === CPROPS && o.field === titleField && o.before != null)
  if (ren) return out(`renamed ${ren.before} to ${ren.after}`, ren.after ?? '', ren.key)
  const hid = M.find((o) => o.root === CPROPS && o.field === hiddenField)
  if (hid) return out(`${hid.after === '1' ? 'hid' : 'showed'} the sheet ${titleOf(B1, hid.key)}`, titleOf(B1, hid.key), hid.key)

  // Inserts and deletes of rows or columns: `derived.bands` names them (r1 ruling 4).
  const bands = f.derived?.bands ?? []
  if (bands.length) {
    const b = bands[0]
    const title = titleOf(b.numbers === 'before' ? B0 : B1, b.container) || titleOf(B1, b.container)
    const listRoot = b.axis === 'rows' ? gs.rows : gs.cols
    const blank = b.op === 'delete' && !L.some((o) => o.root === listRoot && o.op === 'delete')
    const w = bandWords(b, title, blank)
    const first = b.op === 'delete' ? Math.max(1, b.from) : b.from
    const target: PhraseTarget = b.axis === 'rows' ? { sheet: b.container, r: first, c: 1, row: true } : { sheet: b.container, r: 1, c: first }
    return out(w.text, w.place, b.container, target)
  }
  // No bands (an older pack): the list ops, numbered at their rev.
  const rIns = L.filter((o) => o.root === gs.rows && o.op === 'insert')
  if (rIns.length) {
    const sk = sheetOfRow(B1, rIns[0].key)
    const g = sk ? gridOf(B1, sk) : null
    if (sk && g) {
      const ns = rIns.map((o) => g.rowByKey.get(o.key)?.n ?? 0).filter((n) => n > 0).sort((x, y) => x - y)
      if (ns.length) {
        const text = ns.length === 1 ? `inserted a row at ${ns[0]} on ${g.title}` : `inserted ${ns.length} rows at ${ns[0]}–${ns[ns.length - 1]} on ${g.title}`
        return out(text, ns[0], sk, { sheet: sk, r: ns[0], c: 1, row: true })
      }
    }
  }
  const rDel = L.filter((o) => o.root === gs.rows && o.op === 'delete')
  if (rDel.length) {
    const sk = sheetOfRow(B0, rDel[0].key)
    const g = sk ? gridOf(B0, sk) : null
    if (sk && g) {
      const ns = rDel.map((o) => g.rowByKey.get(o.key)?.n ?? 0).filter((n) => n > 0).sort((x, y) => x - y)
      return out(`deleted row${ns.length > 1 ? 's' : ''} ${ns.join(', ')} on ${g.title}`, ns[0] ?? '', sk)
    }
  }

  const cops = M.filter((o) => isCellOp(B1, o))
  if (cops.length) {
    if (file) return outsideSave(f, B1, cops, out)
    const at = cellPoints(B1, cops)
    if (!at) return out('changed the workbook', '', null)
    const g = gridOf(B1, at.sheet)
    // The verb's own rectangle when the frame carries it (r1 ruling 1), else the box of the cells written.
    const written = f.derived?.written
    let R = box(at.pts)
    if (written && written.container === at.sheet && g && written.ranges.length === 1) {
      const s = cornerPlace(g, written.ranges[0].start)
      const e = cornerPlace(g, written.ranges[0].end)
      if (s && e) R = { r0: Math.min(s.r, e.r), r1: Math.max(s.r, e.r), c0: Math.min(s.c, e.c), c1: Math.max(s.c, e.c) }
    }
    const place = rectA1(R)
    const title = g?.title ?? ''
    const target: PhraseTarget = { sheet: at.sheet, r: R.r0, c: R.c0 }
    if (cops.length === 1) {
      const o = cops[0]
      const fa = a1Text(B1, o.key, o.field as string)
      const fb = a1Text(B0, o.key, o.field as string)
      if (fa != null && fb != null && fa !== fb) return out(`changed the formula in ${place} on ${title}`, place, at.sheet, target)
      if (fa != null && fb == null) return out(`wrote a formula in ${place} on ${title}`, place, at.sheet, target)
      if (fa == null && fb != null) return out(`replaced the formula in ${place} on ${title}`, place, at.sheet, target)
      return out(`set ${place} on ${title}`, place, at.sheet, target)
    }
    const full = cops.length === (R.r1 - R.r0 + 1) * (R.c1 - R.c0 + 1)
    if (!full) return out(`changed ${cops.length} cells on ${title}`, `${cops.length} cells`, at.sheet, target)
    const allNew = cops.every((o) => o.before == null)
    return out(`${allNew ? 'filled' : 'set'} ${place} on ${title}`, place, at.sheet, target)
  }
  const merged = f.derived?.containers ? Object.keys(f.derived.containers) : []
  if (merged.length) return out(`changed merged cells or styles on ${titleOf(B1, merged[0])}`, titleOf(B1, merged[0]), merged[0])
  return out('changed the workbook', '', null)
}

/** An outside save: the cells someone typed, by A1, and how many cached results came back with them. */
function outsideSave(
  f: OpsChangeFrame,
  B1: Book,
  cops: Extract<WatchOp, { kind: 'map' }>[],
  out: (text: string, place: string | number, sheet: string | null, target?: PhraseTarget | null) => Phrase,
): Phrase {
  const values = cops.filter((o) => a1Text(B1, o.key, o.field as string) == null)
  const results = cops.length - values.length
  const at = cellPoints(B1, values.length ? values : cops)
  const places = at ? at.pts.map((p) => colLetters(p.c) + p.r) : []
  const named = values.length === 0 ? '' : values.length <= 3 ? places.join(', ') : `${values.length} cells`
  const res = results ? `${results} result${results === 1 ? '' : 's'}` : ''
  const text = `${[named, res].filter(Boolean).join(' and ')} changed`
  const target = at ? { sheet: at.sheet, r: at.pts[0].r, c: at.pts[0].c } : null
  return out(text, named || res, at?.sheet ?? null, target)
}

// ---------- history, grouped by turn ----------

export type LogEntry =
  | { kind: 'frame'; seq: number; rev: number; turn: string | null; actor: Actor; at: number | null; phrase: Phrase; verb: string | null; claim: string | null }
  | { kind: 'claim'; seq: number; id: string; actor: Actor; at: number | null; label: string; place: string | null; sheet: string; hold: boolean }
  | { kind: 'claim_end'; seq: number; id: string; actor: Actor; at: number | null; how: string; label: string | null; lines: { first: number; last: number } | null }
  | { kind: 'turn_end'; seq: number; turn: string; actor: Actor; at: number | null }

export interface TurnGroup {
  turn: string | null
  actor: Actor
  items: LogEntry[]
  endedAt: number | null
  /** Ended by `turn_end`, by idling past `turn_idle_ms`, or a change with no turn (an outside save). */
  ended: boolean
  endedBy: 'turn_end' | 'idle' | 'no turn' | null
  pendingOnly: boolean
}

export interface IdleRule {
  /** The welcome's `turn_idle_ms`. */
  idleMs: number | null
  /** The client's clock, in the host's time. */
  now: number | null
}

/**
 * History, grouped by turn: claims and claim ends join their actor's open turn; `turn_end` closes it.
 * With no `turn_end`, a turn ends `turn_idle_ms` after its last activity: a change, or a claim taken
 * or ended (presence alone doesn't keep it open). A `turn_end` after that changes nothing shown (r1.1).
 */
export function turnGroups(log: readonly LogEntry[], idle: IdleRule = { idleMs: null, now: null }): TurnGroup[] {
  const groups: TurnGroup[] = []
  const byTurn = new Map<string, TurnGroup>()
  let pending: LogEntry[] = []
  for (const e of log) {
    if (e.kind === 'frame') {
      const t = e.turn ?? `solo:${e.actor.id}:${e.rev}`
      let g = byTurn.get(t)
      if (!g) {
        g = { turn: e.turn, actor: e.actor, items: [], endedAt: null, ended: false, endedBy: null, pendingOnly: false }
        byTurn.set(t, g)
        groups.push(g)
      }
      const mine = pending.filter((p) => p.actor.id === e.actor.id)
      pending = pending.filter((p) => p.actor.id !== e.actor.id)
      g.items.push(...mine, e)
    } else if (e.kind === 'turn_end') {
      const g = byTurn.get(e.turn)
      if (g) {
        g.endedAt = e.at
        g.items.push(e)
      }
    } else {
      let g: TurnGroup | null = null
      for (let j = groups.length - 1; j >= 0; j--) {
        if (groups[j].actor.id === e.actor.id && groups[j].turn) {
          g = groups[j]
          break
        }
      }
      if (g && g.endedAt == null) g.items.push(e)
      else pending.push(e)
    }
  }
  if (pending.length) groups.push({ turn: null, actor: pending[0].actor, items: pending, endedAt: null, ended: false, endedBy: null, pendingOnly: true })
  for (const g of groups) {
    const last = g.items.filter((x) => x.kind !== 'turn_end' && x.at != null).reduce((m, x) => Math.max(m, x.at as number), -Infinity)
    const idleAt = g.turn && idle.idleMs != null && Number.isFinite(last) ? last + idle.idleMs : null
    const idled = idleAt != null && idle.now != null && idle.now >= idleAt && (g.endedAt == null || g.endedAt > idleAt)
    if (idled) {
      g.endedAt = idleAt
      g.endedBy = 'idle'
    } else if (g.endedAt != null) g.endedBy = 'turn_end'
    else if (!g.turn && !g.pendingOnly) g.endedBy = 'no turn'
    g.ended = g.endedBy != null
  }
  return groups
}

/** The activity line (RC §3): the latest change plus a count, the claim's label while one is held, or "done" after `turn_end`. */
export interface ActivityLine {
  actor: Actor | null
  /** The claim the actor holds now (its label leads the line). */
  claimLabel: string | null
  latest: Phrase | null
  more: number
  frames: number
  done: boolean
  endedBy: TurnGroup['endedBy']
  turn: string | null
  endedAt: number | null
  latestAt: number | null
  /** An agent's turn is open: its avatar shows working. */
  working: boolean
}

export function activityLine(log: readonly LogEntry[], claims: readonly { actor: Actor; label: string | null }[], idle?: IdleRule): ActivityLine {
  const groups = turnGroups(log, idle)
  const g = groups[groups.length - 1]
  if (!g) return { actor: null, claimLabel: null, latest: null, more: 0, frames: 0, done: false, endedBy: null, turn: null, endedAt: null, latestAt: null, working: false }
  const frames = g.items.filter((e): e is Extract<LogEntry, { kind: 'frame' }> => e.kind === 'frame')
  const last = frames[frames.length - 1]
  const mine = claims.filter((c) => c.actor.id === g.actor.id)
  const claim = !g.ended ? (mine[mine.length - 1] ?? null) : null
  return {
    actor: g.actor,
    claimLabel: claim?.label ?? null,
    latest: last?.phrase ?? null,
    more: Math.max(0, frames.length - 1),
    frames: frames.length,
    done: g.ended && !!g.turn,
    endedBy: g.endedBy,
    turn: g.turn,
    endedAt: g.endedAt,
    latestAt: last?.at ?? null,
    working: !g.ended && g.actor.kind === 'agent',
  }
}

/** A turn id's short form for the history header: the part after the last dot ("t:5e0a….7" → "7"). */
export const turnShort = (turn: string): string => turn.split('.').pop() ?? turn

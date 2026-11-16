// What the picture draws over the still (live-scene-v2-notes.md, "A live
// change, step by step" and "Colours and quiet holds"):
// - others' selections, quiet; holds as thin outlines with one small label
//   per hold ("Atlas · dining set", the full note on hover); your pick in
//   your colour with its name; and, strongest, the object moving right now;
// - the live change: a ghost where it was, the box sliding to where it is,
//   a tag ("Atlas moved Table · Vase moves with it"), direction arrows;
//   when the new still lands it settles to a plain outline, the tag and
//   arrows fade, and from the next step it lives in Activity;
// - dashed boxes where the bounds are approximate (a modifier adds what
//   they miss) or the place is driven;
// - hidden objects get no box, except a dashed "(hidden)" ghost while an
//   agent acts on or holds one; objects linked from another file get none;
// - your pick or the change under way off the picture: a chip on the edge.
// One presence token per person (`--color-presence-*`): yours is the 4th,
// as in the panel; the others take the free one nearest their own hue.
// Pure; geometry in overlay-geom.ts, label placement in labels.ts.

import { UNITS, collOf, nameOf, objOf, worldOf } from './fold'
import { agentDone, isLive, myPick, shortHold } from './panel'
import { actorOf, changesOf, phraseOne } from './wording'
import { arrowsFor, edgeAnchor, geomOf, lerpBox, offPicture, worldBox, type ArrowKind, type BoundsOf, type StillView } from './overlay-geom'
import type { Rect, Side } from './labels'
import type { Claim, SimState } from './sim'
import type { Actor, Doc, Frame } from './types'

export type { BoundsOf, StillView } from './overlay-geom'

// ── Colours ──

export type Slot = 1 | 2 | 3 | 4 | 'file'
/** Your own pick and selection: the panel's `--lv-mine`. */
export const MINE: Slot = 4
/** Each token's hue, so a person keeps about the colour their avatar has. */
const HUES: Record<1 | 2 | 3, number> = { 1: 250, 2: 200, 3: 330 }

const SLOT_VAR: Record<Slot, string> = { 1: 'var(--color-presence-1)', 2: 'var(--color-presence-2)', 3: 'var(--color-presence-3)', 4: 'var(--color-presence-4)', file: 'var(--color-text-secondary)' }
export const slotVar = (s: Slot): string => SLOT_VAR[s]

function hueOf(hex: string): number | null {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!m) return null
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255)
  const mx = Math.max(r, g, b)
  const d = mx - Math.min(r, g, b)
  if (!d) return null
  const h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4
  return (h * 60 + 360) % 360
}
const hueGap = (a: number, b: number): number => Math.min(Math.abs(a - b), 360 - Math.abs(a - b))

/** Each person's presence token, in the order they first appear (so it stays put as people come and go). */
export function presenceSlots(S: SimState): Record<string, Slot> {
  const ids: string[] = []
  const see = (id: string | null | undefined): void => {
    if (id && !ids.includes(id)) ids.push(id)
  }
  Object.keys(S.peers).forEach(see)
  S.log.forEach((e) => see(e.actor))
  S.claims.forEach((c) => see(actorOf(c.actor).id))
  S.recent.forEach((r) => see(actorOf(r.f.actor).id))
  const out: Record<string, Slot> = {}
  const free = new Set<1 | 2 | 3>([1, 2, 3])
  for (const id of ids) {
    const a = actorOf(id)
    if (id === S.self) out[id] = MINE
    else if (a.kind === 'file') out[id] = 'file'
    else {
      const h = hueOf(S.peers[id]?.colour || a.colour) ?? 0
      const pool = free.size ? [...free] : ([1, 2, 3] as const)
      const pick = [...pool].sort((x, y) => hueGap(h, HUES[x]) - hueGap(h, HUES[y]))[0]
      free.delete(pick)
      out[id] = pick
    }
  }
  return out
}

// ── What is drawn ──

export type Hidden = 'self' | 'collection' | 'none'
/** Hidden in the file (K2 Cores' mesh read flags it `hidden: self | collection | none`). */
export function hiddenOf(D: Doc, a: string): Hidden {
  const o = objOf(D, a)
  if (o.hidden) return 'self'
  const seen = new Set<string>()
  for (let c: string | null = o.container; c && !seen.has(c); c = collOf(D, c).parent) {
    seen.add(c)
    if (collOf(D, c).hidden) return 'collection'
  }
  return 'none'
}

export interface LiveChange {
  f: Frame
  before: Doc
  after: Doc
  actor: Actor
  /** Its still isn't on screen yet. */
  pending: boolean
  /** The new still landed this step. */
  landed: boolean
  /** live: drawn strong, sliding; settled: a plain outline, the tag fading. */
  phase: 'live' | 'settled'
  /** Objects the change touched, and everything whose place moved with them. */
  direct: string[]
  moved: string[]
}

/** The change being drawn: the newest frame while its still is pending, then the step its still lands (or the step it came in, if it needs none). */
export function liveChange(S: SimState): LiveChange | null {
  const r = (S.recent ?? []).find((x) => x.age === 0)
  if (!r || !isLive(S)) return null
  const pending = S.pv.shown != null && S.pv.shown < r.f.rev
  const thisStep = S.log.some((e) => e.kind === 'frame' && e.frame === r.f && e.i === S.i)
  const landed = !!S.pv.landed
  if (!pending && !thisStep && !landed) return null
  const moved: string[] = []
  for (const a of r.after.lists[UNITS] ?? []) {
    const wb = r.before.units[a] ? worldOf(r.before, a) : null
    const wa = r.after.units[a] ? worldOf(r.after, a) : null
    if (wb && wa && wb.some((v, i) => Math.abs(v - wa[i]) > 1e-6)) moved.push(a)
  }
  return { f: r.f, before: r.before, after: r.after, actor: actorOf(r.f.actor), pending, landed, phase: pending ? 'live' : 'settled', direct: changesOf(r.f).map((c) => c.addr), moved }
}

export type BoxStyle = 'sel' | 'hold' | 'mine' | 'live' | 'settled'

interface Item {
  a: string
  style: BoxStyle
  pri: number
  slot: Slot
  who: string
  agent: boolean
  away: boolean
  rides: boolean
  claim: Claim | null
}

function itemsOf(S: SimState, pick: string | null | undefined, slots: Record<string, Slot>, lc: LiveChange | null, bounds: BoundsOf): Item[] {
  if (!isLive(S) || !S.D) return []
  const items = new Map<string, Item>()
  const put = (a: string, o: Omit<Item, 'a'>): void => {
    if (!S.D.props[a]) return
    const cur = items.get(a)
    if (!cur || o.pri > cur.pri) items.set(a, { a, ...o })
  }
  const base = { away: false, rides: false, claim: null }
  for (const p of Object.values(S.peers)) {
    if (p.handle === S.self || !p.selection || (p.kind === 'agent' && agentDone(S, p.handle))) continue
    for (const a of p.selection.keys) put(a, { ...base, pri: 1, style: 'sel', slot: slots[p.handle] ?? 1, who: p.name, agent: p.kind === 'agent', away: !!p.away })
  }
  for (const c of S.claims) {
    const ac = actorOf(c.actor)
    for (const a of c.keys) put(a, { ...base, pri: 2, style: 'hold', slot: slots[ac.id] ?? 1, who: ac.name, agent: ac.kind === 'agent', claim: c })
  }
  const mine = myPick(S, pick)
  if (mine) put(mine, { ...base, pri: 3, style: 'mine', slot: MINE, who: 'You', agent: false })
  if (lc)
    for (const a of [...lc.moved, ...lc.direct])
      put(a, { ...base, pri: 4, style: lc.phase, slot: slots[lc.actor.id] ?? 1, who: lc.actor.name, agent: lc.actor.kind === 'agent', rides: !lc.direct.includes(a) })
  // Linked objects never; hidden ones only while an agent acts on or holds them.
  return [...items.values()].filter((it) => objOf(S.D, it.a).edit !== '0' && (!isHidden(S.D, it.a, bounds) || (it.agent && (it.style === 'live' || it.style === 'settled' || it.style === 'hold'))))
}

/** Driven (the place op or the pack's mark), or the listing's `approx: true` (its `approx_reason` the why). */
export function approxOf(D: Doc, a: string, bounds: BoundsOf): string | null {
  if (objOf(D, a).edit === 'a' || D.units[a]?.placement?.approx === 'driven') return 'driven'
  const b = bounds(D, a)
  return b?.approx === true ? (b.approx_reason ?? 'modifiers') : null
}

/** Hidden: the listing's `hidden: true` decides; an object the listing has no entry for (no bounds) falls back to the file's flags. */
export function isHidden(D: Doc, a: string, bounds: BoundsOf): boolean {
  const b = bounds(D, a)
  return b ? b.hidden === true : hiddenOf(D, a) !== 'none'
}

// ── The draw list ──

export interface BoxDraw {
  a: string
  style: BoxStyle
  slot: Slot
  approx: string | null
  hidden: boolean
  away: boolean
  rides: boolean
  d: string
  rect: Rect
}
export interface GhostDraw {
  a: string
  slot: Slot
  hidden: boolean
  d: string
}
export interface ArrowDraw {
  a: string
  slot: Slot
  kind: ArrowKind
  d: string
  fading: boolean
}
export type TagKind = 'live' | 'hold' | 'mine' | 'sel' | 'note'
export interface TagSpec {
  id: string
  a: string
  kind: TagKind
  /** tag: above its box; point: hanging under a point on its front edge. */
  place: 'tag' | 'point'
  anchor: Rect
  slot: Slot
  who: string
  text: string
  sub?: string
  title?: string
  away?: boolean
  agent?: boolean
  /** The change settled: the tag fades (not under reduced motion). */
  fading?: boolean
  /** The actor's kind, for the tag's avatar ("file": saved outside K2). */
  actorKind?: string
}
export interface ChipSpec {
  a: string
  name: string
  slot: Slot
  side: Side
  /** The point on the picture's edge nearest it, in still pixels. */
  at: { x: number; y: number }
}
export interface OverlayDraw {
  boxes: BoxDraw[]
  ghosts: GhostDraw[]
  arrows: ArrowDraw[]
  tags: TagSpec[]
  chips: ChipSpec[]
}

const EMPTY: OverlayDraw = { boxes: [], ghosts: [], arrows: [], tags: [], chips: [] }
const union = (rs: Rect[]): Rect => ({ x0: Math.min(...rs.map((r) => r.x0)), y0: Math.min(...rs.map((r) => r.y0)), x1: Math.max(...rs.map((r) => r.x1)), y1: Math.max(...rs.map((r) => r.y1)) })
const list = (names: string[]): string => (names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : names[0])

/** Everything over the still, `t` of the way through the slide (1: the end; tags and arrows only then). */
export function overlayDraw(S: SimState, o: { pick: string | null | undefined; still: StillView; bounds: BoundsOf; t?: number }): OverlayDraw {
  const t = o.t ?? 1
  const lc = liveChange(S)
  const items = itemsOf(S, o.pick, presenceSlots(S), lc, o.bounds)
  if (!items.length) return EMPTY
  const D = S.D
  const sv = o.still
  const out: OverlayDraw = { boxes: [], ghosts: [], arrows: [], tags: [], chips: [] }
  const geo = new Map<string, ReturnType<typeof geomOf>>()
  const off = new Set<string>()
  for (const it of items) {
    const now = worldBox(D, it.a, o.bounds)
    if (!now) continue
    const was = it.style === 'live' && lc ? worldBox(lc.before, it.a, o.bounds) : null
    const g = geomOf(lerpBox(was, now, it.style === 'live' ? t : 1), sv)
    const approx = approxOf(D, it.a, o.bounds)
    const hidden = isHidden(D, it.a, o.bounds)
    if (was && it.style === 'live') {
      const g0 = geomOf(was, sv)
      if (g0) out.ghosts.push({ a: it.a, slot: it.slot, hidden, d: g0.d })
    }
    if ((it.style === 'mine' || ((it.style === 'live' || it.style === 'settled') && !it.rides)) && t >= 1) {
      const edge = offPicture(now, sv)
      if (edge) {
        off.add(it.a)
        out.chips.push({ a: it.a, name: nameOf(D, it.a), slot: it.slot, side: edge.side, at: edge.at })
      }
    }
    if (!g) continue
    geo.set(it.a, g)
    out.boxes.push({ a: it.a, style: it.style, slot: it.slot, approx, hidden, away: it.away, rides: it.rides, d: g.d, rect: g.rect })
  }
  // Direction arrows on what is moving right now (never handles); they fade when the new still lands.
  if (t >= 1 && lc && (lc.phase === 'live' || lc.landed))
    for (const c of changesOf(lc.f)) {
      const it = items.find((x) => x.a === c.addr && !x.rides)
      if (!it || !c.fields.xform || off.has(c.addr)) continue
      for (const ar of arrowsFor(c.addr, c.fields.xform, lc.before, lc.after, o.bounds, sv)) out.arrows.push({ a: c.addr, slot: it.slot, kind: ar.kind, d: ar.d, fading: lc.phase === 'settled' })
    }
  if (t < 1) return out
  // Tags: the change under way, one per hold, your pick, notes, others' selections.
  const held = new Set<string>()
  for (const it of items) {
    const g = geo.get(it.a)
    if (!g || off.has(it.a)) continue
    const name = nameOf(D, it.a)
    const approx = approxOf(D, it.a, o.bounds) ? ' · approximate' : ''
    const hidden = isHidden(D, it.a, o.bounds)
    if ((it.style === 'live' || it.style === 'settled') && lc) {
      if (it.rides || S.pv.state === 'failed') continue
      const c = changesOf(lc.f).find((x) => x.addr === it.a)
      const riders = lc.moved.filter((x) => !lc.direct.includes(x)).map((x) => nameOf(D, x))
      out.tags.push({ id: `live:${it.a}`, a: it.a, kind: 'live', place: 'tag', anchor: g.rect, slot: it.slot, who: it.who, actorKind: lc.actor.kind, text: c ? phraseOne(c, lc.before, lc.after).text : `moved ${name}`, ...(riders.length ? { sub: `${list(riders)} ${riders.length > 1 ? 'move' : 'moves'} with it` } : {}), fading: it.style === 'settled' })
    } else if (it.style === 'hold' && it.claim && !held.has(it.claim.id)) {
      held.add(it.claim.id)
      const group = it.claim.keys.filter((k) => items.find((x) => x.a === k)?.style === 'hold' && geo.get(k) && !off.has(k))
      out.tags.push({ id: `hold:${it.claim.id}`, a: it.a, kind: 'hold', place: 'tag', anchor: union(group.map((k) => geo.get(k)!.rect)), slot: it.slot, who: it.who, text: shortHold(it.claim.label), title: `${it.who} is holding ${list(it.claim.keys.map((k) => nameOf(D, k)))}: ${it.claim.label}` })
    } else if (it.style === 'mine') out.tags.push({ id: `mine:${it.a}`, a: it.a, kind: 'mine', place: 'tag', anchor: g.rect, slot: it.slot, who: 'You', text: name + approx })
    else if (it.style === 'sel' && it.agent) out.tags.push({ id: `sel:${it.a}:${it.who}`, a: it.a, kind: 'sel', place: 'tag', anchor: g.rect, slot: it.slot, who: it.who, text: '', agent: true })
    else if (it.style === 'sel') {
      const p = edgeAnchor(g, sv)
      out.tags.push({ id: `sel:${it.a}:${it.who}`, a: it.a, kind: 'sel', place: 'point', anchor: { x0: p.x, y0: p.y, x1: p.x, y1: p.y }, slot: it.slot, who: it.who, text: it.away ? '(away)' : '', away: it.away })
    }
    if ((approx || hidden) && it.style !== 'mine') out.tags.push({ id: `note:${it.a}`, a: it.a, kind: 'note', place: 'tag', anchor: g.rect, slot: it.slot, who: '', text: `${name}${hidden ? ' (hidden)' : ''}${approx}` })
  }
  return out
}

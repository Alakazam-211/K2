// The lanes' view rules (T3), pure: lane heights S/M/L and the lanes'
// height; zoom, fit and the drawn window (only a screen either side of
// what shows is drawn); what each clip carries (changed, held, refused, let
// go: an outline and a dot, never words); the old spot of a moved or
// trimmed clip; the change brackets; whose playheads show and who is off
// screen; the readiness line; and the View choices, saved per tab.

import { actorOf } from './actors'
import { lanesOf, modelOf, type TimelineModel } from './fold'
import { cutsIn, LANE_SIZES, pictureAt, prepOf, type MediaFacts, type PreviewState } from './picture'
import { headOf } from './playheads'
import type { SimState } from './sim'
import { framePhrase, refusalPhrase } from './wording'

export type LaneSize = 'S' | 'M' | 'L'
export type PicPref = 'auto' | 'head' | 'headtail' | 'strip' | 'off'
export type ScrollPref = 'page' | 'centre'
export interface ViewPrefs {
  lanes: LaneSize
  pic: PicPref
  scroll: ScrollPref
}

/** The lane header's width; narrow (under 1,100 px) keeps the short label (V1) and the name goes in a tooltip. */
export const HEAD_W = 244
export const HEAD_W_NARROW = 56
/** The clip's name band. */
export const NAME_H = 16
/** The rule between video and audio. */
export const SPLIT_H = 3
export const MIN_PPF = 0.004
export const MAX_PPF = 64

export const isBig = (M: TimelineModel): boolean => M.count > 300

export const laneHeight = (size: LaneSize, kind: string): number => LANE_SIZES[size][kind === 'video' ? 'video' : 'audio']

/** Where the 3 px split goes: the first audio lane under a video one. */
export function splitBefore(M: TimelineModel): string | null {
  const L = lanesOf(M)
  const i = L.findIndex((t) => t.kind === 'audio')
  return i > 0 && L.slice(0, i).some((t) => t.kind === 'video') ? L[i].key : null
}

/** Every lane, its 1 px rule, and the split. */
export function lanesHeight(M: TimelineModel, size: LaneSize): number {
  return lanesOf(M).reduce((h, t) => h + laneHeight(size, t.kind) + 1, 0) + (splitBefore(M) ? SPLIT_H : 0)
}

export const defaultPrefs = (M: TimelineModel): ViewPrefs => ({ lanes: isBig(M) ? 'S' : 'M', pic: 'auto', scroll: 'page' })

/** Pixels a frame as the lanes open: a long edit at 1.1, else to the window (0.42 to 1.2; about 0.8 at the mock's 920 px of lanes, so B-roll names read). */
export const defaultPpf = (M: TimelineModel, tw: number): number => (isBig(M) ? 1.1 : Math.max(0.42, Math.min(1.2, tw / 1150)))
/** The whole edit in the lanes' width. */
export const fitPpf = (M: TimelineModel, tw: number): number => Math.max(MIN_PPF, (tw - 30) / Math.max(48, M.endF))

/** One zoom step: half again in (to 64 px a frame), or back out (to fit). */
export function zoomStep(ppf: number, dir: 1 | -1, M: TimelineModel, tw: number): number {
  if (dir > 0) return Math.min(MAX_PPF, ppf * 1.5)
  return Math.max(Math.min(fitPpf(M, tw), ppf), ppf / 1.5)
}

/** The lanes' width: the edit and some room after it, never narrower than the window. */
export const timeWidth = (M: TimelineModel, ppf: number, tw: number): number => Math.max(tw, Math.ceil((M.endF + Math.max(240, M.endF * 0.06)) * ppf))

/** The frames drawn: a screen either side of what shows. */
export const drawWindow = (left: number, tw: number, ppf: number): [number, number] => [Math.max(0, (left - tw) / ppf), (left + 2 * tw) / ppf]

export interface ClipMark {
  /** Changed by a recent edit: the outline in the editor's colour; the dot while fresh. */
  changed?: { colour: string; fresh: boolean; who: string; name: string }
  /** Held: a dashed outline and a lock in the holder's colour. */
  held?: { colour: string; who: string; name: string; note: string }
  /** The refused verb's clip: a red outline and the stop glyph. */
  refused?: { colour: string; line: string }
  /** Just let go: a fading dashed outline. */
  released?: { colour: string; name: string }
}

const colourOf = (c: string | null | undefined): string => c || '#8a8f98'

export function clipMarks(S: SimState, M: TimelineModel = modelOf(S.B)): Record<string, ClipMark> {
  const out: Record<string, ClipMark> = {}
  const at = (k: string): ClipMark => (out[k] ||= {})
  for (const r of S.recent) {
    const a = actorOf(r.f.actor)
    for (const u of r.f.units || []) {
      if (u.ops.includes('delete') || !M.items[u.key]) continue
      at(u.key).changed ??= { colour: colourOf(a.colour), fresh: r.age === 0, who: a.id, name: a.name }
    }
  }
  for (const c of S.claims) {
    const a = actorOf(c.actor)
    for (const k of c.keys) at(k).held = { colour: colourOf(a.colour), who: a.id, name: a.name, note: c.note || '' }
  }
  if (S.refused && S.refused.at === S.i) {
    const a = actorOf(S.refused.actor)
    at(S.refused.unit).refused = { colour: colourOf(a.colour), line: refusalPhrase(S.refused, M, S.claims).line }
  }
  for (const e of S.log)
    if (e.kind === 'claim_end' && e.i === S.i && e.how === 'released' && e.claim) {
      const a = actorOf(e.actor)
      for (const k of e.claim.keys) at(k).released = { colour: colourOf(a.colour), name: a.name }
    }
  return out
}

export interface Ghost {
  track: string
  f0: number
  f1: number
  colour: string
  kind: 'moved' | 'cut' | 'gone'
  fresh: boolean
}

/** Where a recently moved, trimmed or removed clip was: a dashed outline (no words). */
export function ghostsOf(S: SimState, M: TimelineModel = modelOf(S.B)): Ghost[] {
  const out: Ghost[] = []
  for (const r of S.recent) {
    const M0 = modelOf(r.before)
    const colour = colourOf(actorOf(r.f.actor).colour)
    for (const u of r.f.units || []) {
      const was = M0.items[u.key]
      const now = M.items[u.key]
      if (!was) continue
      if (u.ops.includes('delete')) out.push({ track: was.track, f0: was.f0, f1: was.f1, colour, kind: 'gone', fresh: r.age === 0 })
      else if (u.ops.includes('move_resize') && now) {
        const moved = (was.s !== now.s && was.e !== now.e) || was.track !== now.track
        out.push({ track: was.track, f0: was.f0, f1: was.f1, colour, kind: moved ? 'moved' : 'cut', fresh: r.age === 0 })
      }
    }
  }
  return out
}

export interface Bracket {
  f0: number
  f1: number
  /** Whose change (the agent's handle) and its verb ("clip trim"). */
  who: string
  verb: string | null
  colour: string
  line: string
  fresh: boolean
}

/** Each agent's change as a bracket over its span on the ruler, like a range comment. */
export function bracketsOf(S: SimState): Bracket[] {
  const out: Bracket[] = []
  for (const e of S.log) {
    if (e.kind !== 'frame' || actorOf(e.actor).kind !== 'agent') continue
    const M0 = modelOf(e.before)
    const M1 = modelOf(e.after)
    let f0 = Infinity
    let f1 = -Infinity
    for (const u of e.frame.units || [])
      for (const x of [M0.items[u.key], M1.items[u.key]])
        if (x) {
          f0 = Math.min(f0, x.f0)
          f1 = Math.max(f1, x.f1)
        }
    if (!Number.isFinite(f0)) continue
    out.push({ f0, f1, who: actorOf(e.actor).id, verb: e.frame.verb ?? null, colour: colourOf(actorOf(e.actor).colour), line: framePhrase(e.frame, e.before, e.after).line, fresh: e.i === S.i })
  }
  return out
}

export interface Playhead {
  id: string
  name: string
  f: number
  colour: string
  self: boolean
  away: boolean
  playing?: boolean
}

/** Everyone with a place on the edit (their time position, extrapolated while playing, else their pointer). */
export function playheadsOf(S: SimState, M: TimelineModel, now: number): Playhead[] {
  const out: Playhead[] = []
  for (const p of Object.values(S.peers)) {
    const h = headOf(M, p, now)
    if (!h) continue
    out.push({ id: p.handle, name: p.isSelf ? 'You' : p.name, f: h.f, colour: colourOf(p.colour), self: !!p.isSelf, away: !!p.away, playing: h.playing })
  }
  return out
}

/** Other people past either end of frames v0..v1, nearest first (you and anyone away never stack). */
export function offscreenOf<P extends Pick<Playhead, 'f' | 'self' | 'away'>>(heads: readonly P[], v0: number, v1: number): { left: P[]; right: P[] } {
  const others = heads.filter((h) => !h.self && !h.away)
  return {
    left: others.filter((h) => h.f < v0).sort((a, b) => b.f - a.f),
    right: others.filter((h) => h.f > v1).sort((a, b) => a.f - b.f),
  }
}

export interface Readiness {
  f0: number
  f1: number
  /** Plays here, stills only here, no preview here. */
  kind: 'ok' | 'stills' | 'none'
}

/** The 2 px readiness line under the ticks: can this stretch be previewed? Gaps draw nothing. */
export function readinessOf(M: TimelineModel, f0: number, f1: number, pv: PreviewState, facts: MediaFacts): Readiness[] {
  if (!M.count) return []
  const cuts = [f0, ...cutsIn(M, f0, f1, pv, facts), f1]
  const prep = prepOf(M, pv, facts)
  const out: Readiness[] = []
  for (let i = 0; i < cuts.length - 1; i++) {
    const a = cuts[i]
    const b = cuts[i + 1]
    const p = pictureAt(M, (a + b) / 2, pv, facts)
    if (p.kind === 'gap') continue
    const kind = p.kind === 'clip' ? (pv.player === 'still' || (prep && p.it && !prep.isReady(p.it.key)) ? 'stills' : 'ok') : 'none'
    const last = out[out.length - 1]
    if (last && last.kind === kind && Math.abs(last.f1 - a) < 1e-9) last.f1 = b
    else out.push({ f0: a, f1: b, kind })
  }
  return out
}

/** The View menu: three labelled groups of radios. */
export const VIEW_GROUPS = [
  { id: 'lanes', title: 'Lane height', options: [['S', 'Small'], ['M', 'Medium'], ['L', 'Large']] },
  { id: 'pic', title: 'Pictures on clips', options: [['auto', 'Automatic'], ['head', 'First frame'], ['headtail', 'First and last'], ['strip', 'Filmstrip'], ['off', 'None']] },
  { id: 'scroll', title: 'While playing', options: [['page', 'Page the lanes'], ['centre', 'Keep the playhead centred']] },
] as const satisfies readonly { id: keyof ViewPrefs; title: string; options: readonly (readonly [string, string])[] }[]

/** The line the menu shows after a choice: "Lane height: Large. Saved for this tab." */
export function confirmLine(group: keyof ViewPrefs, value: string): string {
  const g = VIEW_GROUPS.find((x) => x.id === group)!
  const o = (g.options as readonly (readonly [string, string])[]).find((x) => x[0] === value)
  return `${g.title}: ${o ? o[1] : value}. Saved for this tab.`
}

const STORE = 'k2.liveTimeline.view.v1:'
const MEMORY = new Map<string, ViewPrefs>()
const valid = (p: unknown): p is ViewPrefs => {
  const v = p as ViewPrefs | null
  return !!v && VIEW_GROUPS.every((g) => (g.options as readonly (readonly [string, string])[]).some((o) => o[0] === v[g.id]))
}

/** This tab's View choices: what it saved, else the defaults for this edit. */
export function loadPrefs(tab: string, M: TimelineModel): ViewPrefs {
  const mem = MEMORY.get(tab)
  if (mem) return mem
  try {
    const raw = localStorage.getItem(STORE + tab)
    const p: unknown = raw ? JSON.parse(raw) : null
    if (valid(p)) return p
  } catch {
    // no storage here: the defaults
  }
  return defaultPrefs(M)
}

export function savePrefs(tab: string, p: ViewPrefs): void {
  MEMORY.set(tab, p)
  try {
    localStorage.setItem(STORE + tab, JSON.stringify(p))
  } catch {
    // kept for this session only
  }
}

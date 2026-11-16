// Playheads and visible ranges on a timeline.
//
// - The kit's time position (02ddb6aa): `{at: ticks as a decimal string,
//   state: playing | paused, at_ms (stamped by the kit), in?, rate?}`.
//   Following someone who is playing extrapolates from `at_ms` and `rate`;
//   never a stream of seeks.
// - A viewport is rational time at the timeline's rate plus a lane range.
// - The kit moves an agent's playhead after each verb (verbs.rs, 0634ee32):
//   `verbLanding` says where.

import { lanesOf, modelOf, type TimelineModel } from './fold'
import { MKR, PROPS, type Book, type Pointer, type RationalTime, type SampleTimePosition, type TimelineFrame, type Viewport } from './schema'
import { big, PER_S, rateOf } from './time'

export const isRT = (v: unknown): v is RationalTime => {
  const r = v as RationalTime | null
  return !!r && typeof r === 'object' && typeof r.value === 'number' && Number.isFinite(r.value) && !!rateOf(r.rate)
}

/** Rational time as frames at the timeline's rate (24 while it's provisional). */
export function rtF(v: unknown, M?: TimelineModel | null): number | null {
  if (!isRT(v)) return null
  const r = rateOf(v.rate)!
  const fps = M && M.T.rate && !M.T.provisional ? M.T.rate.fps : 24
  return (v.value * fps) / r.fps
}

export const ptrF = (M: TimelineModel, p: Pointer | null | undefined): number | null => (p ? rtF(p.at, M) : null)

/** A viewport as frames and lane indexes (top to bottom). */
export function vpOf(M: TimelineModel, v: Viewport): { f0: number | null; f1: number | null; i0: number; i1: number } {
  const L = lanesOf(M).map((t) => t.key)
  const a = L.indexOf(v.tracks && v.tracks[0])
  const b = L.indexOf(v.tracks && v.tracks[1])
  return { f0: rtF(v.from, M), f1: rtF(v.to, M), i0: a < 0 ? 0 : a, i1: b < 0 ? L.length - 1 : Math.max(a, b) }
}

const F24 = PER_S / 24n

/** The view's own pointer, from the kit's time position (24 fps rational time, the lane note kept). */
export function tpPtr(v: SampleTimePosition | null | undefined): Pointer | null {
  const t = v ? big(String(v.at)) : null
  if (!v || t == null) return null
  return { at: { value: Number(t / F24) + Number(t % F24) / Number(F24), rate: '24/1' }, track: v._track || null }
}

export interface Head {
  f: number
  playing: boolean
  rate: number
  at_ms?: number
}

/** Someone's playhead now: their time position (extrapolated while playing), else their pointer. */
export function headOf(M: TimelineModel, p: { timePosition?: SampleTimePosition | null; pointer?: Pointer | null } | null | undefined, now?: number | null): Head | null {
  if (!p) return null
  const h = p.timePosition
  const at = h ? big(String(h.at)) : null
  if (h && at != null) {
    const playing = h.state === 'playing'
    const rate = h.rate ? String(h.rate).split('/').map(Number).reduce((a, b) => a / b) : 1
    const f0 = M.fr(at)
    const dt = playing && now != null && h.at_ms != null ? Math.max(0, now - h.at_ms) / 1000 : 0
    return { f: f0 + dt * (M.T.rate ? M.T.rate.fps : 24) * rate, playing, rate: playing ? rate : 0, at_ms: h.at_ms }
  }
  const f = p.pointer ? ptrF(M, p.pointer) : null
  return f == null ? null : { f, playing: false, rate: 0 }
}

/**
 * Where the kit puts an agent's playhead after a verb (verbs.rs): move, trim
 * --in, add, rename and enable on the clip's start; trim --out alone on the
 * new end; a lift where the clip started; a marker on its record time with
 * no track. Anything else doesn't move it.
 */
export function verbLanding(f: Pick<TimelineFrame, 'verb' | 'ops' | 'units'>, B0: Book, B1: Book): { at: string; track: string | null } | null {
  const M0 = modelOf(B0)
  const M1 = modelOf(B1)
  const v = f.verb || ''
  const tick = (fr: number): string => String(BigInt(Math.round(fr)) * M1.FR)
  if (/^marker /.test(v)) {
    const k = f.ops.find((o) => o.root === MKR || o.root === PROPS[MKR])?.key
    const mk = M1.markers.find((x) => x.key === k) || M0.markers.find((x) => x.key === k)
    return mk && mk.f != null ? { at: tick(mk.f), track: null } : null
  }
  const u = (f.units || [])[0]
  if (!u || !/^clip /.test(v)) return null
  const a = M0.items[u.key]
  const b = M1.items[u.key]
  if (v === 'clip delete') return a ? { at: tick(a.f0), track: a.track } : null
  if (!b) return null
  if (v === 'clip trim' && a && Math.abs(a.f0 - b.f0) < 1e-9 && Math.abs(a.f1 - b.f1) > 1e-9) return { at: tick(b.f1), track: b.track }
  return { at: tick(b.f0), track: b.track }
}

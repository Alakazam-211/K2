// Time on a timeline (plan §1.4): ticks, rates, frames and timecode.
//
// - A tick is 1/254,016,000,000 s, so a frame at 24, 25 and 30000/1001 and a
//   sample at 48 kHz and 44.1 kHz are each a whole number of ticks. Ticks are
//   decimal strings in the keyed state and are read as BigInt, never as a JS
//   number (a time-of-day source is past 2^53).
// - Timecode the pack gives (`derived.tc`, a drop-frame source's `;` form) is
//   shown exactly as given and never reformatted: `tcAsGiven`.
// - The view's own counting (the ruler, a playhead) is HH:MM:SS:FF on the
//   frame grid, from the timeline's start timecode (view.rs timecode()).

export const PER_S = 254016000000n
export const MAX53 = 9007199254740992n

/** A decimal tick string as BigInt; null for anything else (no "-0", no leading zeros). */
export function big(s: unknown): bigint | null {
  return typeof s === 'string' && /^-?(0|[1-9]\d*)$/.test(s) && s !== '-0' ? BigInt(s) : null
}

/** "start,duration" (geom) or "in,out" (a transition's tr). */
export function pairOf(s: unknown): [bigint, bigint] | null {
  if (typeof s !== 'string') return null
  const p = s.split(',')
  if (p.length !== 2) return null
  const a = big(p[0])
  const b = big(p[1])
  return a === null || b === null ? null : [a, b]
}

export interface Rate {
  text: string
  num: bigint
  den: bigint
  /** Ticks in a frame, or null when a frame isn't a whole number of ticks. */
  frame: bigint | null
  /** The timecode base (24, 25, 30 for 30000/1001). */
  base: number
  fps: number
}

/** A rate as the pack writes it ("24/1", "30000/1001"); null when it isn't one. */
export function rateOf(s: string | null | undefined): Rate | null {
  const m = /^(\d+)\/(\d+)$/.exec(s || '')
  if (!m) return null
  const num = BigInt(m[1])
  const den = BigInt(m[2])
  if (!num || !den) return null
  const t = PER_S * den
  return { text: s as string, num, den, frame: t % num === 0n ? t / num : null, base: Number((num + den / 2n) / den), fps: Number(num) / Number(den) }
}

const two = (n: bigint | number): string => String(n).padStart(2, '0')

/** view.rs timecode(): the start timecode plus the record time, on the frame grid only; HH:MM:SS:FF. Null between frames. */
export function tcText(ticks: bigint | null | undefined, R: Rate | null | undefined, tc0?: bigint | null): string | null {
  if (!R || R.frame == null || ticks == null) return null
  const at = ticks + (tc0 || 0n)
  if (at < 0n || at % R.frame !== 0n) return null
  const fr = at / R.frame
  const b = BigInt(R.base)
  const s = fr / b
  return [s / 3600n, (s / 60n) % 60n, s % 60n, fr % b].map(two).join(':')
}

/** A timecode the pack gave (a clip's `derived.tc`, a drop-frame "01:00:10;10"): shown as given, never reformatted. */
export function tcAsGiven(given: unknown): string | null {
  return typeof given === 'string' && TIMECODE.test(given) ? given : null
}

/** HH:MM:SS:FF, or the drop-frame HH:MM:SS;FF. */
export const TIMECODE = /^\d\d:\d\d:\d\d[:;]\d\d$/

/** Drop-frame timecode (pack 6aeaa1ed): 29.97 counted with frames 0 and 1 dropped each minute except every tenth, ";" before the frames. */
export function dfTc(n: number): string {
  const D = Math.floor(n / 17982)
  const m = n % 17982
  n += 18 * D + (m > 1 ? 2 * Math.floor((m - 2) / 1798) : 0)
  const f = n % 30
  const s = Math.floor(n / 30) % 60
  const mi = Math.floor(n / 1800) % 60
  const h = Math.floor(n / 108000)
  return `${two(h)}:${two(mi)}:${two(s)};${two(f)}`
}

/** A length in words: "6 frames", "1.5s", "2m 4s" (never "6f"). */
export function durText(frames: number, base?: number): string {
  const n = Math.round(Math.abs(frames))
  const b = base || 24
  const f = n % b
  const s = Math.floor(n / b)
  const m = Math.floor(s / 60)
  const ss = s % 60
  const out: string[] = []
  if (!m && !ss) return `${f} frame${f === 1 ? '' : 's'}`
  if (m) out.push(m + 'm')
  if (ss || f) out.push((f ? (ss + f / b).toFixed(1) : ss) + 's')
  return out.join(' ')
}

/** Ticks as frames (a fraction past the grid kept). */
export function framesOf(t: bigint | null | undefined, frame: bigint): number {
  return t != null && frame ? Number(t / frame) + Number(t % frame) / Number(frame) : 0
}

/** Ruler density (filmcraft ruler_steps): labels at least 110 px apart, minor ticks at least 12 px, the minor step dividing the label step. In frames. */
export function rulerSteps(ppf: number, base: number): { minor: number; label: number } {
  const b = (k: number): number => base * k
  const st = [1, 2, 5, 10, base / 2, base, b(2), b(5), b(10), b(15), b(30), b(60), b(120), b(300), b(600), b(1800), b(3600)]
  const label = Math.max(1, st.find((s) => s * ppf >= 110) || b(3600))
  let minor = label
  for (let k = 9; k >= 1; k--)
    if (label % k === 0 && (label / k) * ppf >= 12) {
      minor = label / k
      break
    }
  return { minor, label }
}

/** Scrolling while playing (filmcraft follow_playhead): `page` jumps a screen when the playhead passes 95% (it lands at 5%); `centre` keeps it at 50%. The new left edge, or null to stay. */
export function scrollFor(mode: 'page' | 'centre', x: number, left: number, tw: number): number | null {
  const a = (x - left) / tw
  if (mode === 'centre') return a > 0.5 || a < 0 ? x - tw * 0.5 : null
  return a > 0.95 || a < 0 ? x - tw * 0.05 : null
}

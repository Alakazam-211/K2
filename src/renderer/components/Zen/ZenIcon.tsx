// The Zen icon (Rosson 2026-10-04): small SVGs on a 24 viewBox in
// `currentColor`, drawn at ~16px. Off is outlined and light; on fills with
// the accent. The top bar's Zen toggle draws the one `lib/zen/zen-icon.ts`
// picks (the ensō); the other two stay so a change of mind is one constant.
//
//   ripples  a Zen rock garden: a pebble with two raked-sand ripple arcs.
//            Off: outlined stone, thin arcs. On: accent stone, solid arcs.
//   enso     the calligraphy circle: one open brush stroke that tapers to a
//            thin tail, with a small gap. On: heavier, in the accent.
//   bonsai   a pot, a curved trunk and three cloud pads. On: accent pads.
//
// Motion (Motion's `motion/react`) plays when the icon turns on (including
// when an on icon mounts), changes after mounting, or `replay` goes up (the
// top bar bumps it on hover): the ripples ease outward once, the ensō draws
// in, the bonsai's pads pop with a little sway. An off icon that just
// mounted stays still. `prefers-reduced-motion: reduce` skips all of it.

import { useEffect, useId, useState } from 'react'
import { motion, type Transition } from 'motion/react'
import type { ZenIconVariant } from '@/lib/zen/zen-icon'
import { REDUCED_MOTION_QUERY, zenMediaMatches } from '@/lib/zen/zen-theme'

const f = (n: number): string => String(Math.round(n * 100) / 100)

// ── ripples ────────────────────────────────────────────────────────────
const RIPPLE_CY = 10.4
const STONE_D = `M8.4 ${f(RIPPLE_CY + 1)}c0-2 1.6-3.6 3.7-3.6s3.5 1.5 3.5 3.3c0 1.2-1.4 1.8-3.6 1.8s-3.6-.4-3.6-1.5z`

/** An ellipse arc around the stone, open at the top (from 215° round the
 *  bottom to -35°). */
function rippleArc(rx: number, ry: number): string {
  const at = (deg: number): [number, number] => [
    12 + rx * Math.cos((deg * Math.PI) / 180),
    RIPPLE_CY + ry * Math.sin((deg * Math.PI) / 180),
  ]
  const [x0, y0] = at(215)
  const [x1, y1] = at(-35)
  return `M${f(x0)} ${f(y0)}A${rx} ${ry} 0 1 0 ${f(x1)} ${f(y1)}`
}
const RIPPLES_D = [rippleArc(6.6, 5), rippleArc(10, 7.6)]

// ── enso ───────────────────────────────────────────────────────────────
// Rosson 2026-10-07: the stroke starts at the bottom left and loops
// clockwise round to the bottom right (the old -52° start turned 180°).
const ENSO_START_DEG = 128
const ENSO_SWEEP_DEG = 318
const ENSO_STEPS = 48

function ensoPoint(t: number, offset: number): [number, number] {
  const a = ((ENSO_START_DEG + ENSO_SWEEP_DEG * t) * Math.PI) / 180
  // The tail drifts a little inward, like a hand finishing the circle.
  const r = 7.7 - 0.6 * t ** 3 + offset
  return [12 + r * Math.cos(a), 12 + r * Math.sin(a)]
}

/** The brush stroke as a filled outline: thick at the entry, tapering. */
function ensoBrush(maxW: number, minW: number): string {
  const width = (t: number): number => minW + (maxW - minW) * (1 - t) ** 0.9 * (t < 0.06 ? 0.75 + 0.25 * (t / 0.06) : 1)
  const outer: Array<[number, number]> = []
  const inner: Array<[number, number]> = []
  for (let i = 0; i <= ENSO_STEPS; i++) {
    const t = i / ENSO_STEPS
    outer.push(ensoPoint(t, width(t) / 2))
    inner.push(ensoPoint(t, -width(t) / 2))
  }
  const w0 = width(0)
  let d = `M${f(inner[0][0])} ${f(inner[0][1])}A${f(w0 / 2)} ${f(w0 / 2)} 0 0 1 ${f(outer[0][0])} ${f(outer[0][1])}`
  for (let i = 1; i <= ENSO_STEPS; i++) d += `L${f(outer[i][0])} ${f(outer[i][1])}`
  d += `A${f(minW / 2)} ${f(minW / 2)} 0 0 1 ${f(inner[ENSO_STEPS][0])} ${f(inner[ENSO_STEPS][1])}`
  for (let i = ENSO_STEPS - 1; i >= 0; i--) d += `L${f(inner[i][0])} ${f(inner[i][1])}`
  return `${d}Z`
}

/** The stroke's centre line: the mask that draws the ensō in. */
function ensoSpine(): string {
  let d = ''
  for (let i = 0; i <= ENSO_STEPS; i++) {
    const [x, y] = ensoPoint(i / ENSO_STEPS, 0)
    d += `${i === 0 ? 'M' : 'L'}${f(x)} ${f(y)}`
  }
  return d
}

const ENSO_OFF_D = ensoBrush(2.5, 0.6)
const ENSO_ON_D = ensoBrush(3.5, 0.9)
const ENSO_SPINE_D = ensoSpine()

// ── bonsai ─────────────────────────────────────────────────────────────
const BONSAI_TRUNK_D = 'M12 17c0-2-2.2-2.6-2.2-4.6s2.2-2.4 2.2-4M11.2 14.6c1.6 0 3.2-.4 4.4-1.4'
const BONSAI_POT_D = 'M5.5 17h13l-1.5 4H7z'
/** Three flat-bottomed cloud pads: top, left, right. */
const BONSAI_PADS_D = [
  'M6.8 8.6A2 2 0 0 1 9 5.6A3.4 3.4 0 0 1 15 5.6A2 2 0 0 1 17.2 8.6Z',
  'M2.6 12.8A1.5 1.5 0 0 1 4.2 10.6A2.6 2.6 0 0 1 8.4 10.4A1.5 1.5 0 0 1 9.8 12.8Z',
  'M14.2 13.4A1.5 1.5 0 0 1 15.6 11A2.6 2.6 0 0 1 19.8 11.2A1.5 1.5 0 0 1 21.4 13.4Z',
]

const GLIDE: Transition['ease'] = [0.22, 1, 0.36, 1]

/** `prefers-reduced-motion`, live. */
function useReducedMotion(): boolean {
  const [on, setOn] = useState(() => zenMediaMatches(REDUCED_MOTION_QUERY))
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(REDUCED_MOTION_QUERY)
    const sync = (): void => setOn(mq.matches)
    sync()
    mq.addEventListener?.('change', sync)
    return () => mq.removeEventListener?.('change', sync)
  }, [])
  return on
}

export const ZEN_ICON_DEFAULT_ACCENT = 'var(--zen-accent)'

export interface ZenIconProps {
  variant: ZenIconVariant
  /** Zen is on: the accent, filled state. */
  on: boolean
  /** CSS px. */
  size?: number
  /** The on state's fill. Defaults to Zen's accent (outside Zen, pass one). */
  accent?: string
  /** Bump to play the motion again in the current state (0: never). */
  replay?: number
}

export function ZenIcon({
  variant,
  on,
  size = 16,
  accent = ZEN_ICON_DEFAULT_ACCENT,
  replay = 0,
}: ZenIconProps): React.JSX.Element {
  const reduced = useReducedMotion()
  // Has `on` changed since mount? (Derived state, React's own pattern.)
  const [seen, setSeen] = useState({ on, toggled: false })
  if (seen.on !== on) setSeen({ on, toggled: true })
  const animate = !reduced && (on || seen.toggled || replay > 0)
  const maskId = `zen-enso-${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`
  // A new key per state (and per replay) plays the animation again.
  const k = `${on ? 'on' : 'off'}-${replay}`

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      focusable="false"
      data-zen-icon={variant}
      data-zen-icon-on={on ? 'true' : 'false'}
      data-zen-icon-motion={animate ? 'play' : 'still'}
      style={{ display: 'block', overflow: 'visible', flexShrink: 0 }}
    >
      {variant === 'ripples' && (
        <>
          <path
            d={STONE_D}
            fill={on ? accent : 'none'}
            stroke={on ? accent : 'currentColor'}
            strokeWidth={1.5}
            strokeLinejoin="round"
          />
          {RIPPLES_D.map((d, i) => (
            <motion.path
              key={`${k}-${i}`}
              d={d}
              stroke="currentColor"
              strokeWidth={on ? 1.6 : 1.2}
              strokeLinecap="round"
              data-zen-icon-part="ripple"
              initial={animate ? { scale: 0.72, opacity: 0 } : false}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ duration: 0.6, ease: GLIDE, delay: 0.05 + i * 0.09 }}
            />
          ))}
        </>
      )}
      {variant === 'enso' && (
        <>
          {animate && (
            <defs>
              <mask id={maskId} maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24">
                <motion.path
                  key={k}
                  d={ENSO_SPINE_D}
                  stroke="white"
                  strokeWidth={5.5}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  data-zen-icon-part="enso-draw"
                  initial={{ pathLength: 0 }}
                  animate={{ pathLength: 1 }}
                  transition={{ duration: 0.75, ease: [0.45, 0, 0.3, 1] }}
                />
              </mask>
            </defs>
          )}
          <path
            d={on ? ENSO_ON_D : ENSO_OFF_D}
            fill={on ? accent : 'currentColor'}
            mask={animate ? `url(#${maskId})` : undefined}
            data-zen-icon-part="enso"
          />
        </>
      )}
      {variant === 'bonsai' && (
        <>
          <path d={BONSAI_TRUNK_D} stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" />
          {BONSAI_PADS_D.map((d, i) => (
            <motion.path
              key={`${k}-${i}`}
              d={d}
              fill={on ? accent : 'none'}
              stroke={on ? accent : 'currentColor'}
              strokeWidth={1.4}
              strokeLinejoin="round"
              data-zen-icon-part="pad"
              // Sway from the pad's base, where it meets the branch.
              style={{ transformOrigin: '50% 100%' }}
              initial={animate ? { scale: 0.7, rotate: i === 1 ? 8 : -8 } : false}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: 'spring', stiffness: 420, damping: 12, mass: 0.6, delay: i * 0.06 }}
            />
          ))}
          <path d={BONSAI_POT_D} stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" />
        </>
      )}
    </svg>
  )
}

export default ZenIcon

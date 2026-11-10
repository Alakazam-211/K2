// The floating call tile's motion: drag past TAP_SLOP (lifted), let go to the
// corner it would coast to on a spring (reduced motion: a short ease, no
// lift), caught mid-flight it stops under the pointer. One saved corner per
// window.
//
// Ported from K2 Companion's `useCornerDrag` (src/zen/voice/videoLayer.tsx on
// its voice-video branch): the same press, slop, lift, snap and catch. What
// differs on the desktop: no native layer to follow and no haptic; the
// corners are measured spots (`cornerSpots`: the free content area under the
// tab strip and beside open drawers, clear of the
// composers) instead of CSS anchors; and the corner is saved per window
// (thin-client view state, never the daemon's).

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from 'react'
import { beginBrowserMotion } from '@/lib/browser-overlay'
import {
  CORNERS,
  DEFAULT_CORNER,
  easeOut,
  LIFT_DURATION,
  LIFT_SCALE,
  pickCorner,
  planSnap,
  releaseVelocity,
  REST,
  TAP_SLOP,
  type Corner,
  type Pose,
  type Sample,
  type Vec,
} from '@/lib/voice/mini-snap'

export const CORNER_KEY_PREFIX = 'k2.voice.callTileCorner.v1.'

/** The last corner set per window, for when storage is unavailable. */
const sessionCorner = new Map<string, Corner>()

export function readCorner(windowLabel: string): Corner {
  try {
    const v = localStorage.getItem(CORNER_KEY_PREFIX + windowLabel)
    if (CORNERS.includes(v as Corner)) return v as Corner
  } catch {
    // Storage unavailable: this session's corner.
  }
  return sessionCorner.get(windowLabel) ?? DEFAULT_CORNER
}

export function writeCorner(windowLabel: string, c: Corner): void {
  sessionCorner.set(windowLabel, c)
  try {
    localStorage.setItem(CORNER_KEY_PREFIX + windowLabel, c)
  } catch {
    // Storage unavailable: this session only.
  }
}

export function __resetCornersForTests(): void {
  sessionCorner.clear()
}

export function reduceMotion(): boolean {
  try {
    return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
  } catch {
    return false
  }
}

const round = (v: number): number => Math.round(v * 100) / 100
const atRest = (p: Pose): boolean => p.x === 0 && p.y === 0 && p.s === 1

export function poseTransform(p: Pose): string {
  if (atRest(p)) return ''
  const scale = p.s === 1 ? '' : ` scale(${Math.round(p.s * 10000) / 10000})`
  return `translate(${round(p.x)}px, ${round(p.y)}px)${scale}`
}

const now = (): number => performance.now()

interface Press {
  id: number
  x: number
  y: number
  from: Pose
  moved: boolean
  samples: Sample[]
  /** What was pressed: it takes the pointer once the press drags (the tile's
   *  own root lets clicks through its gaps, so it can't). */
  target: Element | null
}

export interface CornerDragOptions {
  windowLabel: string
  /** Each corner's spot (the tile's top left), measured now. */
  spots(): Record<Corner, Vec>
  /** A press that never became a drag, let go on `target`. */
  onTap(target: EventTarget | null): void
}

export interface CornerDrag {
  corner: Corner
  /** Held (dragged) now: lifted, the deeper shadow. */
  held: boolean
  /** In motion (dragged, or on its way to a corner). */
  moving: boolean
  /** Spots may have moved (a composer came or went): glide to the new one. */
  relayout(): void
  /** The tile changed size (a resize): stand on its corner's spot now, no
   *  glide, so the corner it's anchored to stays put. */
  reanchor(): void
  handlers: {
    onPointerDown(e: ReactPointerEvent): void
    onPointerMove(e: ReactPointerEvent): void
    onPointerUp(e: ReactPointerEvent): void
    onPointerCancel(e: ReactPointerEvent): void
    onClickCapture(e: ReactMouseEvent): void
  }
}

export function useCornerDrag(ref: RefObject<HTMLElement | null>, opts: CornerDragOptions): CornerDrag {
  const [corner, setCorner] = useState<Corner>(() => readCorner(opts.windowLabel))
  const [held, setHeld] = useState(false)
  const [moving, setMoving] = useState(false)
  /** Where it's drawn now (moved by the page itself, never through React). */
  const pose = useRef<Pose>(REST)
  /** The spot it's anchored to now. */
  const spot = useRef<Vec | null>(null)
  const raf = useRef(0)
  const press = useRef<Press | null>(null)
  /** A drag just ended: the click it makes is not a click on the tile. */
  const dragged = useRef(false)
  /** The Browser page gives way while the tile moves (a drag claim). */
  const motion = useRef<(() => void) | null>(null)
  const o = useRef(opts)
  o.current = opts
  const cornerRef = useRef(corner)
  cornerRef.current = corner

  const beginMotion = (): void => {
    if (!motion.current) motion.current = beginBrowserMotion('drag')
    setMoving(true)
  }
  const endMotion = (): void => {
    motion.current?.()
    motion.current = null
    setMoving(false)
  }

  const stop = (): void => {
    if (raf.current) cancelAnimationFrame(raf.current)
    raf.current = 0
  }
  useEffect(
    () => () => {
      stop()
      motion.current?.()
      motion.current = null
    },
    [],
  )

  const anchor = (el: HTMLElement, at: Vec): void => {
    spot.current = at
    el.style.left = `${round(at.x)}px`
    el.style.top = `${round(at.y)}px`
  }

  const draw = (p: Pose): void => {
    pose.current = p
    if (ref.current) ref.current.style.transform = poseTransform(p)
  }

  // First placement, before paint: the saved corner's spot.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    anchor(el, o.current.spots()[cornerRef.current])
    el.dataset.corner = cornerRef.current
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** Picked up: a little bigger (not with reduced motion). */
  const lift = (): void => {
    if (reduceMotion()) return
    const start = now()
    const s0 = pose.current.s
    const tick = (): void => {
      const u = Math.min(1, (now() - start) / (LIFT_DURATION * 1000))
      draw({ ...pose.current, s: s0 + (LIFT_SCALE - s0) * easeOut(u) })
      raf.current = u < 1 ? requestAnimationFrame(tick) : 0
    }
    raf.current = requestAnimationFrame(tick)
  }

  /** From the current pose to `next`'s spot at `velocity`. */
  const settle = (next: Corner, spots: Record<Corner, Vec>, velocity: Vec): void => {
    const el = ref.current
    if (!el) return
    stop()
    const here = spot.current ?? spots[next]
    const at = { x: here.x + pose.current.x, y: here.y + pose.current.y }
    const there = spots[next]
    // Same place on screen, now measured from the new spot.
    const from = { x: at.x - there.x, y: at.y - there.y, s: pose.current.s }
    anchor(el, there)
    el.dataset.corner = next
    draw(from)
    if (next !== cornerRef.current) {
      writeCorner(o.current.windowLabel, next)
      setCorner(next)
    }
    setHeld(false)
    if (atRest(from)) {
      endMotion()
      return
    }
    beginMotion()
    const plan = planSnap(from, velocity, reduceMotion())
    el.dataset.motion = plan.kind
    const start = now()
    const tick = (): void => {
      const t = (now() - start) / 1000
      draw(plan.at(t))
      if (t < plan.duration) {
        raf.current = requestAnimationFrame(tick)
      } else {
        raf.current = 0
        endMotion()
      }
    }
    raf.current = requestAnimationFrame(tick)
  }

  /** Let go at `velocity`: to the corner it would coast to. */
  const snap = (velocity: Vec): void => {
    const spots = o.current.spots()
    const el = ref.current
    if (!el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    const here = spot.current ?? spots[cornerRef.current]
    const center = { x: here.x + pose.current.x + w / 2, y: here.y + pose.current.y + h / 2 }
    const homes = {} as Record<Corner, Vec>
    for (const c of CORNERS) homes[c] = { x: spots[c].x + w / 2, y: spots[c].y + h / 2 }
    settle(pickCorner(center, velocity, homes), spots, velocity)
  }

  const relayout = (): void => {
    if (press.current || raf.current) return
    const spots = o.current.spots()
    const to = spots[cornerRef.current]
    const was = spot.current
    if (was && Math.abs(was.x - to.x) < 0.5 && Math.abs(was.y - to.y) < 0.5) return
    settle(cornerRef.current, spots, { x: 0, y: 0 })
  }

  const reanchor = (): void => {
    const el = ref.current
    if (!el || press.current) return
    stop()
    anchor(el, o.current.spots()[cornerRef.current])
    draw(REST)
    if (motion.current) endMotion()
  }

  const capture = (el: Element, id: number): void => {
    try {
      el.setPointerCapture?.(id)
    } catch {
      // A synthetic pointer.
    }
  }

  const onPointerDown = (e: ReactPointerEvent): void => {
    if (press.current || e.button !== 0) return
    dragged.current = false
    if (raf.current) {
      // Caught on its way: it stops under the pointer.
      stop()
    }
    press.current = {
      id: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      from: pose.current,
      moved: false,
      samples: [{ t: now(), x: e.clientX, y: e.clientY }],
      target: e.target instanceof Element ? e.target : null,
    }
  }
  const onPointerMove = (e: ReactPointerEvent): void => {
    const p = press.current
    if (!p || p.id !== e.pointerId) return
    const dx = e.clientX - p.x
    const dy = e.clientY - p.y
    if (!p.moved && Math.hypot(dx, dy) < TAP_SLOP) return
    if (!p.moved) {
      p.moved = true
      // Taken only once it drags: a click on a button stays the button's.
      capture(p.target ?? (e.currentTarget as Element), e.pointerId)
      setHeld(true)
      beginMotion()
      lift()
    }
    p.samples.push({ t: now(), x: e.clientX, y: e.clientY })
    if (p.samples.length > 16) p.samples.shift()
    draw({ x: p.from.x + dx, y: p.from.y + dy, s: pose.current.s })
  }
  const onPointerUp = (e: ReactPointerEvent): void => {
    const p = press.current
    if (!p || p.id !== e.pointerId) return
    press.current = null
    if (!p.moved) {
      if (!atRest(pose.current)) snap({ x: 0, y: 0 })
      o.current.onTap(e.target)
      return
    }
    dragged.current = true
    p.samples.push({ t: now(), x: e.clientX, y: e.clientY })
    snap(releaseVelocity(p.samples))
  }
  const onPointerCancel = (e: ReactPointerEvent): void => {
    const p = press.current
    if (!p || p.id !== e.pointerId) return
    press.current = null
    if (p.moved) dragged.current = true
    if (p.moved || !atRest(pose.current)) snap({ x: 0, y: 0 })
  }
  const onClickCapture = (e: ReactMouseEvent): void => {
    // The click a drag ends in: never a button's.
    if (!dragged.current) return
    dragged.current = false
    e.preventDefault()
    e.stopPropagation()
  }

  return { corner, held, moving, relayout, reanchor, handlers: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onClickCapture } }
}

/** The tile comes in with a short fade (none with reduced motion). */
export function useFadeIn(ref: RefObject<HTMLElement | null>): void {
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || reduceMotion() || typeof el.animate !== 'function') return
    el.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 180, easing: 'ease-out' })
  }, [ref])
}

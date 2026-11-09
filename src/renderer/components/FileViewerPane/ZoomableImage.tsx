import { useLayoutEffect, useRef, useState } from 'react'
import { Button, Surface } from '@/components/ui'
import { clientToCssPx } from '@/stores/context-menu'

/** Gap kept around the image at fit, and past its edges when panning. */
const PAD = 16
const MAX_SCALE = 16
const KEY_STEP = 1.25
const WHEEL_ZOOM_RATE = 0.01
/** A mouse wheel notch (with Ctrl) is ~100; a trackpad pinch tick is under 20. */
const WHEEL_ZOOM_MAX_DELTA = 50
const DRAG_SLOP_PX = 3
/** The default size of an image with no size of its own (some SVGs). */
const FALLBACK_NATURAL = { w: 300, h: 150 }
const READOUT_ATTR = 'data-image-zoom-readout'

interface Size {
  w: number
  h: number
}

/** `s` is image px → CSS px (1 = "100%"); `x`/`y` place its top-left corner. */
interface View {
  s: number
  x: number
  y: number
  /** Follow the pane's size: on when the view is at fit. */
  fit: boolean
}

/** Fit is what the viewer always showed: fill the width, never upscale, and
 *  let a tall image run past the bottom. */
function fitScale(nat: Size, vp: Size): number {
  return Math.min(1, Math.max(1, vp.w - PAD * 2) / nat.w)
}

/** Zooming out stops once the whole image is on screen. */
function minScale(nat: Size, vp: Size): number {
  return Math.min(fitScale(nat, vp), Math.max(1, vp.h - PAD * 2) / nat.h)
}

/** Centred when it fits on this axis; otherwise its edges can't pass the pad. */
function clampAxis(offset: number, scaled: number, viewport: number): number {
  if (scaled + PAD * 2 <= viewport) return (viewport - scaled) / 2
  return Math.min(PAD, Math.max(viewport - scaled - PAD, offset))
}

function clampView(v: View, nat: Size, vp: Size): View {
  return { ...v, x: clampAxis(v.x, nat.w * v.s, vp.w), y: clampAxis(v.y, nat.h * v.s, vp.h) }
}

function fitView(nat: Size, vp: Size): View {
  return clampView({ s: fitScale(nat, vp), x: PAD, y: PAD, fit: true }, nat, vp)
}

function isPannable(v: View, nat: Size, vp: Size): boolean {
  return nat.w * v.s + PAD * 2 > vp.w || nat.h * v.s + PAD * 2 > vp.h
}

/** Change scale keeping the image point under (px, py) where it is. */
function zoomAbout(v: View, target: number, px: number, py: number, nat: Size, vp: Size): View {
  const s = Math.min(MAX_SCALE, Math.max(minScale(nat, vp), target))
  if (s === v.s) return v
  const ratio = s / v.s
  return clampView(
    { s, x: px - (px - v.x) * ratio, y: py - (py - v.y) * ratio, fit: s === fitScale(nat, vp) },
    nat,
    vp,
  )
}

function wheelPx(delta: number, deltaMode: number, pagePx: number): number {
  if (deltaMode === 1) return delta * 16
  if (deltaMode === 2) return delta * pagePx
  return delta
}

function inReadout(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(`[${READOUT_ATTR}]`) !== null
}

interface ZoomableImageProps {
  src: string
  alt: string
  onError: () => void
}

/**
 * Pinch, pan and keyboard zoom for one loaded image. View state lives here
 * only, so it lasts while the tab is parked (display:none) and is gone with
 * the tab; never sent to the daemon.
 *
 * The <img> keeps its natural size in layout and is only scaled by a 2D
 * transform, with no compositor layer. The browser draws it from the full
 * bitmap at every zoom, so Retina and huge images stay sharp.
 */
export function ZoomableImage({ src, alt, onError }: ZoomableImageProps): React.JSX.Element {
  const viewportRef = useRef<HTMLDivElement>(null)
  // Refs for the native listeners, state for painting.
  const natRef = useRef<Size | null>(null)
  const vpRef = useRef<Size | null>(null)
  const viewRef = useRef<View | null>(null)
  const [natural, setNatural] = useState<Size | null>(null)
  const [view, setView] = useState<View | null>(null)
  const [dragging, setDragging] = useState(false)

  function apply(next: View): void {
    viewRef.current = next
    setView(next)
  }

  function reset(): void {
    if (natRef.current && vpRef.current) apply(fitView(natRef.current, vpRef.current))
  }

  useLayoutEffect(() => {
    const el = viewportRef.current
    if (!el) return

    const measure = (): void => {
      const w = el.clientWidth
      const h = el.clientHeight
      // display:none while the tab is parked: keep the view as it was.
      if (w === 0 || h === 0) return
      if (vpRef.current?.w === w && vpRef.current.h === h) return
      const vp = { w, h }
      vpRef.current = vp
      const nat = natRef.current
      const v = viewRef.current
      if (!nat) return
      if (!v || v.fit) {
        apply(fitView(nat, vp))
      } else {
        apply(clampView({ ...v, s: Math.max(minScale(nat, vp), v.s) }, nat, vp))
      }
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(el)

    const localPoint = (clientX: number, clientY: number): { x: number; y: number } => {
      const r = el.getBoundingClientRect()
      return { x: clientToCssPx(clientX - r.left), y: clientToCssPx(clientY - r.top) }
    }
    const zoomTo = (target: (v: View) => number, px: number, py: number): void => {
      const v = viewRef.current
      const nat = natRef.current
      const vp = vpRef.current
      if (v && nat && vp) apply(zoomAbout(v, target(v), px, py, nat, vp))
    }
    const zoomAtCentre = (factor: number): void => {
      const vp = vpRef.current
      if (vp) zoomTo((v) => v.s * factor, vp.w / 2, vp.h / 2)
    }
    const panBy = (dx: number, dy: number): void => {
      const v = viewRef.current
      const nat = natRef.current
      const vp = vpRef.current
      if (v && nat && vp) apply(clampView({ ...v, x: v.x + dx, y: v.y + dy }, nat, vp))
    }
    const canPan = (): boolean => {
      const v = viewRef.current
      const nat = natRef.current
      const vp = vpRef.current
      return !!v && !!nat && !!vp && isPannable(v, nat, vp)
    }

    // Trackpad pinch: Chromium and WebKitGTK send ctrl+wheel, WKWebView sends
    // gesture events. A real Ctrl+wheel lands here too.
    let gesturing = false
    const onWheel = (e: WheelEvent): void => {
      if (e.ctrlKey) {
        e.preventDefault()
        if (gesturing) return
        const raw = wheelPx(e.deltaY, e.deltaMode, el.clientHeight)
        const delta = Math.min(WHEEL_ZOOM_MAX_DELTA, Math.max(-WHEEL_ZOOM_MAX_DELTA, raw))
        const p = localPoint(e.clientX, e.clientY)
        zoomTo((v) => v.s * Math.exp(-delta * WHEEL_ZOOM_RATE), p.x, p.y)
        return
      }
      if (!canPan()) return
      e.preventDefault()
      panBy(
        -clientToCssPx(wheelPx(e.deltaX, e.deltaMode, el.clientWidth)),
        -clientToCssPx(wheelPx(e.deltaY, e.deltaMode, el.clientHeight)),
      )
    }

    type GestureLike = Event & { scale?: number; clientX?: number; clientY?: number }
    let lastScale = 1
    const onGestureStart = (e: Event): void => {
      e.preventDefault()
      gesturing = true
      lastScale = 1
    }
    const onGestureChange = (e: Event): void => {
      e.preventDefault()
      const g = e as GestureLike
      if (typeof g.scale !== 'number' || g.scale <= 0) return
      const factor = g.scale / lastScale
      lastScale = g.scale
      if (typeof g.clientX === 'number' && typeof g.clientY === 'number') {
        const p = localPoint(g.clientX, g.clientY)
        zoomTo((v) => v.s * factor, p.x, p.y)
      } else {
        zoomAtCentre(factor)
      }
    }
    const onGestureEnd = (e: Event): void => {
      e.preventDefault()
      gesturing = false
    }

    // Only while there's something to pan, so the image can still be dragged
    // out at fit as before.
    let drag: { id: number; x: number; y: number; moved: boolean } | null = null
    const onPointerDown = (e: PointerEvent): void => {
      if (drag || e.button !== 0 || inReadout(e.target) || !canPan()) return
      drag = { id: e.pointerId, x: e.clientX, y: e.clientY, moved: false }
      try {
        el.setPointerCapture(e.pointerId)
      } catch {
        // Synthetic events (tests) have no active pointer to capture.
      }
    }
    const onPointerMove = (e: PointerEvent): void => {
      if (!drag || e.pointerId !== drag.id) return
      const dx = e.clientX - drag.x
      const dy = e.clientY - drag.y
      if (!drag.moved && Math.abs(dx) + Math.abs(dy) < DRAG_SLOP_PX) return
      if (!drag.moved) setDragging(true)
      drag.moved = true
      panBy(clientToCssPx(dx), clientToCssPx(dy))
      drag.x = e.clientX
      drag.y = e.clientY
    }
    const onPointerUp = (e: PointerEvent): void => {
      if (!drag || e.pointerId !== drag.id) return
      drag = null
      setDragging(false)
    }

    const onDoubleClick = (e: MouseEvent): void => {
      if (inReadout(e.target)) return
      const v = viewRef.current
      if (!v) return
      if (!v.fit) {
        reset()
        return
      }
      const p = localPoint(e.clientX, e.clientY)
      zoomTo(() => 1, p.x, p.y)
    }

    // On the viewport, not the window: it only runs while focus is inside
    // this viewer, and stopping it here keeps App's Cmd+=/-/0 (whole-window
    // zoom, a bubble listener on window) from firing as well.
    const onKeyDown = (e: KeyboardEvent): void => {
      if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey) return
      const zoomIn = e.key === '=' || e.key === '+' || e.code === 'Equal'
      const zoomOut = e.key === '-' || e.code === 'Minus'
      const fit = e.key === '0' || e.code === 'Digit0'
      if (!zoomIn && !zoomOut && !fit) return
      e.preventDefault()
      e.stopPropagation()
      if (fit) reset()
      else zoomAtCentre(zoomIn ? KEY_STEP : 1 / KEY_STEP)
    }

    el.addEventListener('wheel', onWheel, { passive: false })
    el.addEventListener('gesturestart', onGestureStart, { passive: false })
    el.addEventListener('gesturechange', onGestureChange, { passive: false })
    el.addEventListener('gestureend', onGestureEnd, { passive: false })
    el.addEventListener('pointerdown', onPointerDown)
    el.addEventListener('pointermove', onPointerMove)
    el.addEventListener('pointerup', onPointerUp)
    el.addEventListener('pointercancel', onPointerUp)
    el.addEventListener('dblclick', onDoubleClick)
    el.addEventListener('keydown', onKeyDown)
    return () => {
      observer.disconnect()
      el.removeEventListener('wheel', onWheel)
      el.removeEventListener('gesturestart', onGestureStart)
      el.removeEventListener('gesturechange', onGestureChange)
      el.removeEventListener('gestureend', onGestureEnd)
      el.removeEventListener('pointerdown', onPointerDown)
      el.removeEventListener('pointermove', onPointerMove)
      el.removeEventListener('pointerup', onPointerUp)
      el.removeEventListener('pointercancel', onPointerUp)
      el.removeEventListener('dblclick', onDoubleClick)
      el.removeEventListener('keydown', onKeyDown)
    }
    // Everything it touches is a ref or a state setter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  function handleLoad(e: React.SyntheticEvent<HTMLImageElement>): void {
    const img = e.currentTarget
    const nat =
      img.naturalWidth > 0 && img.naturalHeight > 0
        ? { w: img.naturalWidth, h: img.naturalHeight }
        : FALLBACK_NATURAL
    natRef.current = nat
    setNatural(nat)
    // No size yet if the tab is parked: measure() fits it when it's shown.
    if (vpRef.current) apply(fitView(nat, vpRef.current))
  }

  const pannable = !!view && !!natural && !!vpRef.current && isPannable(view, natural, vpRef.current)
  const imageStyle: React.CSSProperties =
    view && natural
      ? {
          position: 'absolute',
          left: 0,
          top: 0,
          width: natural.w,
          height: natural.h,
          maxWidth: 'none',
          maxHeight: 'none',
          objectFit: 'contain',
          transformOrigin: '0 0',
          // Whole pixels: a half-pixel offset softens the image at 100%.
          transform: `translate(${Math.round(view.x)}px, ${Math.round(view.y)}px) scale(${view.s})`,
        }
      : { position: 'absolute', visibility: 'hidden' }

  return (
    <div
      ref={viewportRef}
      data-testid="image-zoom-viewport"
      data-k2-keeps-focus=""
      tabIndex={0}
      role="group"
      aria-label="Image viewer"
      className="relative h-full w-full overflow-hidden select-none bg-[var(--color-bg)] outline-none focus-visible:outline focus-visible:outline-1 focus-visible:-outline-offset-1 focus-visible:outline-[var(--color-accent)]"
      style={{ cursor: pannable ? (dragging ? 'grabbing' : 'grab') : undefined }}
    >
      <img
        src={src}
        alt={alt}
        style={imageStyle}
        draggable={pannable ? false : undefined}
        onLoad={handleLoad}
        onError={onError}
      />
      {view && (
        <Surface
          {...{ [READOUT_ATTR]: '' }}
          role2="elevated"
          className="absolute bottom-2 right-2 flex items-center gap-2 pl-2 pr-1 py-1 text-[10px] font-mono text-[var(--color-text-secondary)]"
          style={{ cursor: 'default' }}
        >
          <span data-testid="image-zoom-percent">{Math.round(view.s * 100)}%</span>
          <Button size="sm" disabled={view.fit} title="Reset to fit" onClick={reset}>
            Reset
          </Button>
        </Surface>
      )}
    </div>
  )
}

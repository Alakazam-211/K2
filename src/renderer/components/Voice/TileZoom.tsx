// Zoom on a video tile (Rosson 2026-11-11): a viewer magnifies a screen share
// to read what's small on it. This viewer's only: the sharer sends the same
// and everyone else sees what they saw (the sink just asks this viewer's
// media server subscription for a sharper picture, video-core `setZoom`).
//
//   ⌘/Ctrl + scroll, or a trackpad pinch   zoom about the pointer, 1× to 6×
//   double-click                           fit ↔ 2× at the click
//   drag, scroll, or the arrow keys        move around while zoomed
//   + and − (buttons or keys), 0           zoom about the middle; fit
//   Fit (button) or Esc                    back to fit
//
// The tile's picture is a canvas (the renderer draws every frame itself), so
// the zoom is one CSS transform on the layer that holds the canvas, inside
// the tile's overflow-hidden frame: no layout, and the wheel and the pinch
// are applied once per animation frame. The rules (the clamp, the pointer
// staying put, the resets) are lib/voice/share-zoom.ts.

import { useCallback, useEffect, useRef, useState } from 'react'
import { MinusIcon, PlusIcon } from '@/components/icons/K2Icons'
import {
  arrowPan,
  canvasImage,
  clampView,
  FIT,
  isZoomed,
  panBy,
  toggleAt,
  viewFor,
  wheelFactor,
  ZOOM_MAX,
  zoomBy,
  zoomPercent,
  zoomStep,
  zoomTo,
  type KeptView,
  type Point,
  type Size,
  type ZoomView,
} from '@/lib/voice/share-zoom'
import { PictureButton } from './CallViewButtons'

/** Wheel, pinch and drag, gathered until the next animation frame. */
interface Pending {
  /** A pinch's scale, from where it started (WebKit's gesture events). */
  scale: number | null
  factor: number
  at: Point
  dx: number
  dy: number
}

/** WebKit's pinch (the macOS webview sends these, not a Ctrl wheel). */
interface GestureLike extends UIEvent {
  scale: number
  clientX: number
  clientY: number
}

export interface TileZoomOptions {
  /** Zoom is on for this tile now (a live picture). Off draws fit. */
  enabled: boolean
  /** Another key starts again at fit (share-zoom `zoomResetKey`). */
  resetKey: string
  /** The tile's frame: overflow hidden, the canvas layer inset in it. */
  frameRef: React.RefObject<HTMLElement | null>
  canvasRef: React.RefObject<HTMLCanvasElement | null>
}

export interface TileZoom {
  view: ZoomView
  zoomed: boolean
  /** Spread on the frame. */
  frameProps: {
    onPointerDown: (e: React.PointerEvent<HTMLElement>) => void
    onPointerMove: (e: React.PointerEvent<HTMLElement>) => void
    onPointerUp: (e: React.PointerEvent<HTMLElement>) => void
    onPointerCancel: (e: React.PointerEvent<HTMLElement>) => void
    onDoubleClick: (e: React.MouseEvent<HTMLElement>) => void
    onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => void
  }
  zoomIn: () => void
  zoomOut: () => void
  fit: () => void
}

const onControl = (t: EventTarget | null): boolean => t instanceof Element && t.closest('button') !== null

function wheelPixels(delta: number, mode: number): number {
  return mode === 1 ? delta * 16 : mode === 2 ? delta * 400 : delta
}

export function useTileZoom({ enabled, resetKey, frameRef, canvasRef }: TileZoomOptions): TileZoom {
  const [kept, setKept] = useState<KeptView>({ key: resetKey, view: FIT })
  // A new key forgets the old view for good (the same share coming back
  // after it ended starts at fit too).
  if (kept.key !== resetKey) setKept({ key: resetKey, view: FIT })
  const view = enabled ? viewFor(kept, resetKey) : FIT
  const viewRef = useRef(view)
  viewRef.current = view
  const keyRef = useRef(resetKey)
  keyRef.current = resetKey

  const set = useCallback((next: ZoomView): void => {
    viewRef.current = next
    setKept({ key: keyRef.current, view: next })
  }, [])

  /** The frame's inside (CSS pixels) and the picture's size; null: not drawn. */
  const measure = useCallback((): { frame: Size; image: Size | null; el: HTMLElement } | null => {
    const el = frameRef.current
    if (!el) return null
    return { frame: { width: el.clientWidth, height: el.clientHeight }, image: canvasImage(canvasRef.current), el }
  }, [frameRef, canvasRef])

  const pointIn = (el: HTMLElement, clientX: number, clientY: number): Point => {
    const r = el.getBoundingClientRect()
    return { x: clientX - r.left - el.clientLeft, y: clientY - r.top - el.clientTop }
  }

  // ── once per animation frame ────────────────────────────────────────
  const pending = useRef<Pending | null>(null)
  const frameId = useRef<number | null>(null)
  const flush = useCallback((): void => {
    frameId.current = null
    const p = pending.current
    pending.current = null
    const m = measure()
    if (!p || !m) return
    let v = viewRef.current
    if (p.scale !== null) v = zoomTo(v, p.scale, p.at, m.frame, m.image)
    else if (p.factor !== 1) v = zoomBy(v, p.factor, p.at, m.frame, m.image)
    if (p.dx !== 0 || p.dy !== 0) v = panBy(v, p.dx, p.dy, m.frame, m.image)
    set(v)
  }, [measure, set])
  const queue = useCallback(
    (change: (p: Pending) => void): void => {
      const p = pending.current ?? { scale: null, factor: 1, at: { x: 0, y: 0 }, dx: 0, dy: 0 }
      change(p)
      pending.current = p
      if (frameId.current === null) frameId.current = requestAnimationFrame(flush)
    },
    [flush],
  )
  useEffect(
    () => () => {
      if (frameId.current !== null) cancelAnimationFrame(frameId.current)
      frameId.current = null
      pending.current = null
    },
    [],
  )
  // A new key (another track, the share ended): nothing left over applies.
  useEffect(() => {
    pending.current = null
  }, [resetKey])

  // ── wheel and pinch: native listeners, so they can stop the page's ──
  const pinch = useRef<{ base: number } | null>(null)
  useEffect(() => {
    const el = frameRef.current
    if (!enabled || !el) return
    const onWheel = (e: WheelEvent): void => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault()
        // A WebKit pinch is its gesture events; never both.
        if (pinch.current) return
        const at = pointIn(el, e.clientX, e.clientY)
        const f = wheelFactor(e.deltaY, e.deltaMode)
        queue((p) => {
          if (p.scale !== null) p.scale *= f
          else p.factor *= f
          p.at = at
        })
        return
      }
      // Plain scroll moves a zoomed picture; at fit it's the page's.
      if (!isZoomed(viewRef.current)) return
      e.preventDefault()
      const dx = -wheelPixels(e.deltaX, e.deltaMode)
      const dy = -wheelPixels(e.deltaY, e.deltaMode)
      queue((p) => {
        p.dx += dx
        p.dy += dy
      })
    }
    const onGestureStart = (e: Event): void => {
      e.preventDefault()
      pinch.current = { base: viewRef.current.scale }
    }
    const onGestureChange = (e: Event): void => {
      e.preventDefault()
      const g = e as GestureLike
      const base = pinch.current?.base ?? viewRef.current.scale
      const at = pointIn(el, g.clientX, g.clientY)
      queue((p) => {
        p.scale = base * g.scale
        p.factor = 1
        p.at = at
      })
    }
    const onGestureEnd = (e: Event): void => {
      e.preventDefault()
      pinch.current = null
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    el.addEventListener('gesturestart', onGestureStart)
    el.addEventListener('gesturechange', onGestureChange)
    el.addEventListener('gestureend', onGestureEnd)
    return () => {
      el.removeEventListener('wheel', onWheel)
      el.removeEventListener('gesturestart', onGestureStart)
      el.removeEventListener('gesturechange', onGestureChange)
      el.removeEventListener('gestureend', onGestureEnd)
      pinch.current = null
    }
  }, [enabled, frameRef, queue])

  // The frame resized (the window, the column, pop out): held inside it again.
  useEffect(() => {
    const el = frameRef.current
    if (!enabled || !el || typeof ResizeObserver !== 'function') return
    const ro = new ResizeObserver(() => {
      const m = measure()
      if (!m || !isZoomed(viewRef.current)) return
      set(clampView(viewRef.current, m.frame, m.image))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [enabled, frameRef, measure, set])

  // ── drag ──────────────────────────────────────────────────────────
  const drag = useRef<{ id: number; x: number; y: number } | null>(null)
  const endDrag = (e: React.PointerEvent<HTMLElement>): void => {
    if (drag.current?.id !== e.pointerId) return
    drag.current = null
  }

  const apply = (change: (v: ZoomView, frame: Size, image: Size | null) => ZoomView): void => {
    const m = measure()
    if (!m) return
    set(change(viewRef.current, m.frame, m.image))
  }
  const zoomIn = (): void => apply((v, f, i) => zoomStep(v, 1, f, i))
  const zoomOut = (): void => apply((v, f, i) => zoomStep(v, -1, f, i))
  const fit = (): void => set(FIT)

  const frameProps: TileZoom['frameProps'] = {
    onPointerDown: (e) => {
      if (!enabled || e.button !== 0 || !isZoomed(viewRef.current) || onControl(e.target)) return
      try {
        e.currentTarget.setPointerCapture?.(e.pointerId)
      } catch {
        // A synthetic pointer.
      }
      drag.current = { id: e.pointerId, x: e.clientX, y: e.clientY }
    },
    onPointerMove: (e) => {
      const d = drag.current
      if (!d || d.id !== e.pointerId) return
      const dx = e.clientX - d.x
      const dy = e.clientY - d.y
      d.x = e.clientX
      d.y = e.clientY
      queue((p) => {
        p.dx += dx
        p.dy += dy
      })
    },
    onPointerUp: endDrag,
    onPointerCancel: endDrag,
    onDoubleClick: (e) => {
      if (!enabled || onControl(e.target)) return
      const el = frameRef.current
      if (!el) return
      const at = pointIn(el, e.clientX, e.clientY)
      apply((v, f, i) => toggleAt(v, at, f, i))
    },
    onKeyDown: (e) => {
      if (!enabled || e.altKey || e.ctrlKey || e.metaKey) return
      const zoomed = isZoomed(viewRef.current)
      if (e.key === 'Escape') {
        if (!zoomed) return
        // Fit first; the next Esc is the view's (Shrink).
        e.preventDefault()
        e.stopPropagation()
        fit()
        return
      }
      if (e.key === '+' || e.key === '=') {
        e.preventDefault()
        zoomIn()
        return
      }
      if (e.key === '-' || e.key === '_') {
        e.preventDefault()
        zoomOut()
        return
      }
      if (e.key === '0') {
        e.preventDefault()
        fit()
        return
      }
      if (!zoomed) return
      const m = measure()
      const move = m ? arrowPan(e.key, m.frame, e.shiftKey) : null
      if (!move) return
      e.preventDefault()
      apply((v, f, i) => panBy(v, move.x, move.y, f, i))
    },
  }

  return { view, zoomed: isZoomed(view), frameProps, zoomIn, zoomOut, fit }
}

/** The zoom's buttons, on the picture's top right: − and +, and while
 *  zoomed, how far and Fit. They show on hover or focus, and stay while
 *  zoomed. */
export function ZoomControls({ zoom, name, chip }: { zoom: TileZoom; name: string; /** The labels' chip (VideoTile's MEDIA_LABEL). */ chip: string }): React.JSX.Element {
  const pct = zoomPercent(zoom.view)
  return (
    <div
      role="group"
      aria-label={`Zoom on ${name}`}
      data-testid="video-zoom-controls"
      data-zoomed={zoom.zoomed ? 'true' : 'false'}
      className={`absolute right-1.5 top-1.5 flex items-center gap-1 transition-opacity ${
        zoom.zoomed ? 'opacity-100' : 'opacity-0 group-hover/zoom:opacity-100 group-focus-within/zoom:opacity-100'
      }`}
    >
      <PictureButton label="Zoom out (−)" testId="video-zoom-out" disabled={!zoom.zoomed} onPress={zoom.zoomOut}>
        <MinusIcon size={13} />
      </PictureButton>
      {pct ? (
        <span data-testid="video-zoom-percent" className={`${chip} h-6 font-mono tabular-nums`}>
          {pct}
        </span>
      ) : null}
      <PictureButton label="Zoom in (+)" testId="video-zoom-in" disabled={zoom.view.scale >= ZOOM_MAX} onPress={zoom.zoomIn}>
        <PlusIcon size={13} />
      </PictureButton>
      {zoom.zoomed ? (
        <PictureButton label="Fit (Esc)" testId="video-zoom-fit" wide onPress={zoom.fit}>
          Fit
        </PictureButton>
      ) : null}
    </div>
  )
}

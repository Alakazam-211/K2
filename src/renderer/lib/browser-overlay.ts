/**
 * Browser overlay arbiter (thin client, per window).
 *
 * The Browser tab's page is a NATIVE child webview (src-tauri
 * browser_webviews.rs). The OS draws it above this whole DOM, so CSS
 * z-index cannot put a K2 menu, dialog, toast or drag ghost over it. Every
 * DOM overlay that can cover the page claims its area here while it is
 * open. A Browser pane gives way (freeze-frame, then hide) only when a claim
 * intersects its own dock rect. A menu on the far side of the window no
 * longer blanks the page.
 *
 * Registering:
 *  - `useBrowserOverlay({ active, ref, kind })` — the overlay's own box
 *    (measured, kept current with a ResizeObserver).
 *  - `useBrowserOverlay({ active, fullscreen: true, kind })` — a scrim or
 *    anything that covers the window.
 *  - `<BrowserOverlayClaim />` — the fullscreen claim as JSX.
 *  - `beginBrowserMotion()` — imperative claim for a layout animation
 *    (drawer slide) or a drag; returns the release function.
 *  The shared primitives (`DialogScrim`, `DialogFrame`, `ContextMenu`,
 *  Toast, the tab drag) already claim, so most overlays get it for free.
 *  `browser-overlay.coverage.test.ts` fails when a new fixed / portal
 *  overlay neither claims nor says why it does not need to.
 *
 * Nesting: a claim carries its DOM root. A Browser pane rendered INSIDE
 * that root (a Browser embedded in a dialog) is not covered by it.
 *
 * Not daemon state: this is per-window view state for one native view.
 */
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type RefObject } from 'react'

export interface OverlayRect {
  x: number
  y: number
  width: number
  height: number
}

/** What kind of overlay holds the claim. Only diagnostics and tests read it. */
export type OverlayKind = 'modal' | 'menu' | 'popover' | 'toast' | 'drag' | 'motion'

export type OverlayArea = OverlayRect | 'fullscreen'

export interface OverlayClaim {
  readonly id: number
  readonly kind: OverlayKind
  readonly area: OverlayArea
  /** The overlay's own DOM root, for the nesting rule. */
  readonly element: Element | null
}

export interface OverlayHandle {
  readonly id: number
  update(area: OverlayArea): void
  release(): void
}

let nextId = 1
const claims = new Map<number, OverlayClaim>()
let list: readonly OverlayClaim[] = []
const listeners = new Set<() => void>()

function publish(): void {
  list = [...claims.values()]
  for (const fn of [...listeners]) fn()
}

/** Within half a CSS pixel: the same box, do not wake every pane. */
function sameArea(a: OverlayArea, b: OverlayArea): boolean {
  if (a === 'fullscreen' || b === 'fullscreen') return a === b
  return (
    Math.abs(a.x - b.x) < 0.5 &&
    Math.abs(a.y - b.y) < 0.5 &&
    Math.abs(a.width - b.width) < 0.5 &&
    Math.abs(a.height - b.height) < 0.5
  )
}

export function claimBrowserOverlay(input: {
  kind: OverlayKind
  area: OverlayArea
  element?: Element | null
}): OverlayHandle {
  const id = nextId++
  claims.set(id, { id, kind: input.kind, area: input.area, element: input.element ?? null })
  publish()
  return {
    id,
    update(area) {
      const cur = claims.get(id)
      if (!cur || sameArea(cur.area, area)) return
      claims.set(id, { ...cur, area })
      publish()
    },
    release() {
      if (claims.delete(id)) publish()
    },
  }
}

/** Claim the whole window for a layout animation or a drag. The Browser
 *  page freezes (a picture of the page moves with the DOM) until release. */
export function beginBrowserMotion(kind: 'motion' | 'drag' = 'motion'): () => void {
  const handle = claimBrowserOverlay({ kind, area: 'fullscreen' })
  return () => handle.release()
}

export function overlayClaims(): readonly OverlayClaim[] {
  return list
}

export function subscribeOverlayClaims(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

/** Test seam: drop every claim. */
export function resetBrowserOverlaysForTest(): void {
  claims.clear()
  publish()
}

/** Positive-area overlap. Touching edges do not cover. */
export function rectsOverlap(a: OverlayRect, b: OverlayRect): boolean {
  if (a.width <= 0 || a.height <= 0 || b.width <= 0 || b.height <= 0) return false
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

export function claimCovers(claim: OverlayClaim, pane: OverlayRect, paneElement: Element | null): boolean {
  if (claim.element && paneElement && claim.element.contains(paneElement)) return false
  if (claim.area === 'fullscreen') return true
  return rectsOverlap(claim.area, pane)
}

export function coveringClaims(
  all: readonly OverlayClaim[],
  pane: OverlayRect | null,
  paneElement: Element | null,
): OverlayClaim[] {
  if (!pane || pane.width <= 0 || pane.height <= 0) return []
  return all.filter((c) => claimCovers(c, pane, paneElement))
}

export function measureElement(el: Element | null): OverlayRect | null {
  if (!el) return null
  const r = el.getBoundingClientRect()
  return { x: r.x, y: r.y, width: r.width, height: r.height }
}

export interface BrowserOverlayOptions {
  /** Claim only while true (an open menu, a visible toast). */
  active: boolean
  /** The overlay's own box. Measured; also its nesting root. */
  ref?: RefObject<Element | null>
  /** Cover the whole window (scrims). `ref`, if given, is the nesting root. */
  fullscreen?: boolean
  kind?: OverlayKind
  /** Re-claim when this changes (the ref moved to a new element, e.g. a
   *  portal that changed its container). */
  watch?: unknown
}

/**
 * Claim an area over the Browser page while `active`. Released on close and
 * on unmount. A ref claim is re-measured after the first frame (menus place
 * themselves in an effect), on its own resize, and on a window resize. A
 * ref that is not attached yet claims the whole window: over-hiding is the
 * safe bias.
 */
export function useBrowserOverlay({
  active,
  ref,
  fullscreen = false,
  kind = 'popover',
  watch,
}: BrowserOverlayOptions): void {
  const handleRef = useRef<OverlayHandle | null>(null)

  useLayoutEffect(() => {
    if (!active) return
    const el = ref?.current ?? null
    const measure = (): OverlayArea => {
      if (fullscreen || !el) return 'fullscreen'
      return measureElement(el) ?? 'fullscreen'
    }
    const handle = claimBrowserOverlay({ kind, area: measure(), element: el })
    handleRef.current = handle
    if (fullscreen || !el) {
      return () => {
        handle.release()
        handleRef.current = null
      }
    }
    const remeasure = (): void => handle.update(measure())
    const raf = typeof requestAnimationFrame === 'function' ? requestAnimationFrame(remeasure) : null
    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(remeasure) : null
    ro?.observe(el)
    window.addEventListener('resize', remeasure)
    return () => {
      if (raf !== null) cancelAnimationFrame(raf)
      ro?.disconnect()
      window.removeEventListener('resize', remeasure)
      handle.release()
      handleRef.current = null
    }
  }, [active, ref, fullscreen, kind, watch])
}

/** `useBrowserOverlay` as JSX for render-only spots. Claims the window. */
export function BrowserOverlayClaim({
  active = true,
  kind = 'modal',
}: {
  active?: boolean
  kind?: OverlayKind
}): null {
  useBrowserOverlay({ active, fullscreen: true, kind })
  return null
}

/**
 * True while any claim covers the element's box (the Browser dock). Not
 * covered when the dock has no size (display:none; the pane's own
 * visibility rules hide it then).
 */
export function useBrowserCovered(ref: RefObject<Element | null>): boolean {
  const all = useSyncExternalStore(subscribeOverlayClaims, overlayClaims, overlayClaims)
  const [covered, setCovered] = useState(false)
  const allRef = useRef(all)
  allRef.current = all

  const recompute = (): void => {
    const current = allRef.current
    if (current.length === 0) {
      setCovered(false)
      return
    }
    const el = ref.current
    setCovered(coveringClaims(current, measureElement(el), el).length > 0)
  }

  useLayoutEffect(recompute, [all])

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const onChange = (): void => {
      if (allRef.current.length > 0) recompute()
    }
    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(onChange) : null
    ro?.observe(el)
    window.addEventListener('resize', onChange)
    return () => {
      ro?.disconnect()
      window.removeEventListener('resize', onChange)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref])

  return covered
}

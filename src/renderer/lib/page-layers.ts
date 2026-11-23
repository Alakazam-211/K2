/**
 * PageLayers: the one description of
 * every browser page in this K2 window, sent whole on every change to
 * `browser_set_page_layers` (macOS underlay only; Windows and Linux keep
 * the legacy path).
 *
 * Before the canvas desktop, each visible Browser pane is one layer. The
 * overlay arbiter's claims (`browser-overlay.ts`) become `claims`: K2 UI
 * over pages that owns input there (menus, popovers, toasts, dialogs).
 * Drags and layout motion are not claims: they freeze the panes they cover
 * (`still`), as in the legacy path.
 *
 * Claims and nested-claim `minus` shapes carry the corners the claim's
 * element draws (radius, Terminal chamfer, frost; `claim-shape.ts`).
 *
 * Nesting: a claim whose DOM root holds a page (a Browser inside a dialog)
 * does not cover that page. It goes into the other pages' `minus` instead
 * of the global claims.
 *
 * Not daemon state: per-window view state for native views in this window.
 */
import { invoke } from '@tauri-apps/api/core'
import { appZoom, measuredScale } from './app-zoom'
import {
  overlayClaims,
  subscribeOverlayClaims,
  type OverlayClaim,
  type OverlayRect,
} from './browser-overlay'
import { ClaimShapes, type Corners } from './claim-shape'

export type { Corners }

/** Window CSS px from the top-left of the K2 window's content area. */
export interface PageShape {
  x: number
  y: number
  width: number
  height: number
  radius?: Corners
  chamfer?: Corners
}

export type PageMode = 'live' | 'still'

export interface PageLayer {
  /** The browser view: K2's native item id (stable across move/resize). */
  windowId: string
  z: number
  rect: PageShape
  /** K2 things drawn above this page. */
  minus: PageShape[]
  mode: PageMode
}

export type PageClaimKind = 'menu' | 'popover' | 'toast' | 'dock' | 'modal'

export interface PageClaim extends PageShape {
  id: number
  kind: PageClaimKind
  frost?: boolean
}

/** As sent, every shape in it (rects, minus, claims, corners) is in window
 *  px, the UI view's points: what K2 compares and places natively
 *  (`buildPageLayers` converts from CSS px under the app zoom). */
export interface PageLayers {
  parentWindow: string
  /** K2's own field: one id per page load (and per return to underlay).
   *  `generation` counts within it, so after a reload K2 starts afresh
   *  instead of dropping every set as older than the last load's. */
  epoch: string
  generation: number
  dpr: number
  /** The UI page size the rects were measured at (native stale guard). */
  viewport: [number, number]
  /** Back to front. */
  layers: PageLayer[]
  claims: PageClaim[]
}

/** One registered page, as the aggregator reads it at send time. */
export interface PageEntry {
  windowId: string
  /** Registration order: later panes sit in front (they never overlap now). */
  order: number
  /** The dock's rect now (null while it has no size). */
  rect: PageShape | null
  mode: PageMode
  /** The pane's dock element, for the nesting rule. */
  element: Element | null
}

/** Overlay kinds that are claims over pages. `drag` / `motion` are not. */
export function claimKind(kind: OverlayClaim['kind']): PageClaimKind | null {
  switch (kind) {
    case 'menu':
    case 'popover':
    case 'toast':
    case 'modal':
      return kind
    default:
      return null
  }
}

function areaRect(area: OverlayClaim['area'], viewport: [number, number]): OverlayRect {
  return area === 'fullscreen' ? { x: 0, y: 0, width: viewport[0], height: viewport[1] } : area
}

function shapeOf(r: OverlayRect): PageShape {
  return { x: r.x, y: r.y, width: r.width, height: r.height }
}

/** Overlay kinds that still freeze a pane in underlay: everything that isn't
 *  a menu, popover or toast (dialogs dim the page with a scrim; drags and
 *  motion move it). */
export function freezesInUnderlay(claim: OverlayClaim): boolean {
  return claim.kind === 'modal' || claim.kind === 'drag' || claim.kind === 'motion'
}

/**
 * The set K2 is sent, in window px (the UI view's points) throughout:
 * rects, minus, claims and every corner value.
 *
 * In: pages' rects and claims' areas as measured (getBoundingClientRect,
 * `measuredScale` px per CSS px), corner values in CSS px (from styles),
 * the viewport in window px. Under the app zoom (CSS `zoom` on <html>)
 * those differ, so everything is brought to CSS px first and scaled to
 * window px once, at the end (`toWindowPx`).
 */
export function buildPageLayers(
  input: Parameters<typeof buildCssSet>[0] & {
    /** CSS px to window px: the app zoom. Default 1. */
    zoom?: number
    /** Measured px per CSS px. Default: the zoom (WebKit and Chromium
     *  report getBoundingClientRect in window px under a CSS zoom). */
    measuredScale?: number
  },
): PageLayers {
  const ok = (v: number | undefined): v is number => typeof v === 'number' && Number.isFinite(v) && v > 0
  const zoom = ok(input.zoom) ? input.zoom : 1
  const measured = ok(input.measuredScale) ? input.measuredScale : zoom
  const css = (r: OverlayRect): OverlayRect =>
    measured === 1 ? r : { x: r.x / measured, y: r.y / measured, width: r.width / measured, height: r.height / measured }
  const set = buildCssSet({
    ...input,
    viewport: [input.viewport[0] / zoom, input.viewport[1] / zoom],
    pages: input.pages.map((p) => (p.rect ? { ...p, rect: { ...p.rect, ...css(p.rect) } } : p)),
    claims: input.claims.map((c) => (c.area === 'fullscreen' ? c : { ...c, area: css(c.area) })),
  })
  const out = toWindowPx(set, zoom)
  // Pages' edges on the device-pixel grid in window px, AFTER the zoom
  // (at 110 % a CSS-grid edge is off the window grid). Claims and minus
  // are snapped outward natively.
  return {
    ...out,
    viewport: input.viewport,
    layers: out.layers.map((l) => ({ ...l, rect: snapToDevice(l.rect, input.dpr) })),
  }
}

/** Edges (not origin and size apart) to whole device pixels. */
export function snapToDevice<T extends PageShape>(s: T, dpr: number): T {
  const k = Number.isFinite(dpr) && dpr > 0 ? dpr : 1
  const left = Math.round(s.x * k) / k
  const top = Math.round(s.y * k) / k
  const right = Math.round((s.x + s.width) * k) / k
  const bottom = Math.round((s.y + s.height) * k) / k
  return { ...s, x: left, y: top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) }
}

function scaleCorners(c: Corners | undefined, z: number): Corners | undefined {
  if (c === undefined) return undefined
  return typeof c === 'number' ? c * z : (c.map((v) => v * z) as [number, number, number, number])
}

function scaleShape<T extends PageShape>(s: T, z: number): T {
  const out: T = { ...s, x: s.x * z, y: s.y * z, width: s.width * z, height: s.height * z }
  const radius = scaleCorners(s.radius, z)
  const chamfer = scaleCorners(s.chamfer, z)
  if (radius !== undefined) out.radius = radius
  if (chamfer !== undefined) out.chamfer = chamfer
  return out
}

/** A set in CSS px to window px: every rect, minus, claim and corner value
 *  times the zoom (the viewport is the caller's). The one conversion. */
export function toWindowPx(set: PageLayers, zoom: number): PageLayers {
  if (zoom === 1) return set
  return {
    ...set,
    layers: set.layers.map((l) => ({ ...l, rect: scaleShape(l.rect, zoom), minus: l.minus.map((m) => scaleShape(m, zoom)) })),
    claims: set.claims.map((c) => scaleShape(c, zoom)),
  }
}

/** The set in CSS px (corners as styled, rects as laid out). */
function buildCssSet(input: {
  parentWindow: string
  epoch: string
  generation: number
  dpr: number
  viewport: [number, number]
  pages: readonly PageEntry[]
  claims: readonly OverlayClaim[]
  /** The shape reader; PageLayersSync keeps one so unchanged claims aren't re-read. */
  shapes?: ClaimShapes
}): PageLayers {
  const pages = input.pages
    .filter((p): p is PageEntry & { rect: PageShape } => p.rect !== null && p.rect.width > 0 && p.rect.height > 0)
    .slice()
    .sort((a, b) => a.order - b.order)
  const claims: PageClaim[] = []
  // Claims that hold a page: K2 things above the OTHER pages only.
  const nested: Array<{ shape: PageShape; holds: Set<string> }> = []
  const shapes = input.shapes ?? new ClaimShapes()
  const live = new Set<number>()
  for (const c of input.claims) {
    const kind = claimKind(c.kind)
    if (!kind) continue
    const area = areaRect(c.area, input.viewport)
    if (area.width <= 0 || area.height <= 0) continue
    live.add(c.id)
    const { frost, ...corners } = shapes.look(c, area, input.viewport)
    const shape: PageShape = { ...shapeOf(area), ...corners }
    const holds = new Set(
      pages.filter((p) => c.element && p.element && c.element.contains(p.element)).map((p) => p.windowId),
    )
    if (holds.size > 0) nested.push({ shape, holds })
    else claims.push({ ...shape, id: c.id, kind, ...(frost ? { frost } : null) })
  }
  shapes.keep(live)
  const layers: PageLayer[] = pages.map((p, i) => ({
    windowId: p.windowId,
    z: i + 1,
    rect: { ...p.rect },
    minus: nested.filter((n) => !n.holds.has(p.windowId)).map((n) => ({ ...n.shape })),
    mode: p.mode,
  }))
  return {
    parentWindow: input.parentWindow,
    epoch: input.epoch,
    generation: input.generation,
    dpr: input.dpr,
    viewport: input.viewport,
    layers,
    claims,
  }
}

/** Dock corners as a radius: `[tl, tr, br, bl]`, 0 where the corner is square. */
export function cornerRadius(radius: number, corners: readonly [boolean, boolean, boolean, boolean]): Corners {
  const r = Math.max(0, radius)
  const four = corners.map((on) => (on ? r : 0)) as [number, number, number, number]
  return four.every((v) => v === four[0]) ? four[0] : four
}

// ── The per-window aggregator ────────────────────────────────────────────

/** K2's switches that ride with each set. */
export interface UnderlayOptions {
  /** The named private WebKit hover route. Off by default. */
  hoverPrivateWebkitFallback: boolean
  /** The first click on a page in an inactive window reaches the page. Off. */
  pageFirstClick: boolean
  /** No hover gating at all (the release fallback). Off. */
  hoverNoGating?: boolean
}

let epochCount = 0
const loadId =
  (typeof performance !== 'undefined' && Number.isFinite(performance.timeOrigin) ? Math.round(performance.timeOrigin).toString(36) : '0') +
  '-' +
  Math.random().toString(36).slice(2, 10)

/** A new epoch: this page load, and how many times it entered underlay. */
export function nextEpoch(): string {
  epochCount += 1
  return `${loadId}:${epochCount}`
}

export interface PageSource {
  windowId: string
  element: () => Element | null
  measure: () => PageShape | null
  mode: () => PageMode
}

export interface PageLayersDeps {
  parentWindow: () => string
  send: (layers: PageLayers, options: UnderlayOptions) => Promise<void>
  /** The underlay failed: the window goes back to the legacy path. */
  fail: (reason: string) => void
  /** Only send while the underlay is in use in this window. */
  active: () => boolean
  options: () => UnderlayOptions
  viewport: () => [number, number]
  /** The app zoom (CSS px to window px). */
  zoom: () => number
  /** getBoundingClientRect px per CSS px. */
  measuredScale: () => number
  dpr: () => number
  raf: (fn: () => void) => void
  microtask: (fn: () => void) => void
}

const defaultDeps: PageLayersDeps = {
  parentWindow: () => 'main',
  send: (layers, options) => invoke('browser_set_page_layers', { layers, options }),
  fail: () => {},
  active: () => false,
  options: () => ({ hoverPrivateWebkitFallback: false, pageFirstClick: false }),
  // Window px on purpose: K2 compares it with the UI view's bounds in points
  // (the viewport guard in browser_page_layers.rs).
  viewport: () => [typeof window === 'undefined' ? 0 : window.innerWidth, typeof window === 'undefined' ? 0 : window.innerHeight],
  zoom: () => appZoom(),
  measuredScale: () => measuredScale(),
  dpr: () => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1),
  raf: (fn) => (typeof requestAnimationFrame === 'function' ? requestAnimationFrame(() => fn()) : setTimeout(fn, 16)),
  microtask: (fn) => queueMicrotask(fn),
}

/**
 * Collects this window's pages and sends one whole set per change:
 * geometry once per animation frame, claim changes at once (a menu that
 * closes must give its area back to the page in the same frame).
 */
export class PageLayersSync {
  private readonly sources = new Map<string, { src: PageSource; order: number }>()
  private nextOrder = 1
  private generation = 0
  private epochId = nextEpoch()
  private dprWatch: (() => void) | null = null
  private reportedEpoch: string | null = null
  private frameQueued = false
  private nowQueued = false
  private sentEmpty = true
  private failed = false
  private unsubscribe: (() => void) | null = null
  private readonly shapes = new ClaimShapes()
  private deps: PageLayersDeps

  constructor(deps: Partial<PageLayersDeps> = {}) {
    this.deps = { ...defaultDeps, ...deps }
  }

  configure(deps: Partial<PageLayersDeps>): void {
    this.deps = { ...this.deps, ...deps }
  }

  register(src: PageSource): () => void {
    const prev = this.sources.get(src.windowId)
    this.sources.set(src.windowId, { src, order: prev?.order ?? this.nextOrder++ })
    if (!this.unsubscribe) this.unsubscribe = subscribeOverlayClaims(() => this.now())
    this.watchDpr()
    this.frame()
    return () => {
      if (this.sources.get(src.windowId)?.src !== src) return
      this.sources.delete(src.windowId)
      if (this.sources.size === 0 && this.unsubscribe) {
        this.unsubscribe()
        this.unsubscribe = null
        this.dprWatch?.()
        this.dprWatch = null
      }
      this.now()
    }
  }

  /** Geometry changed: send on the next frame. */
  frame(): void {
    if (this.frameQueued) return
    this.frameQueued = true
    this.deps.raf(() => {
      this.frameQueued = false
      void this.flush()
    })
  }

  /** Claims or modes changed: send now (this task). */
  now(): void {
    if (this.nowQueued) return
    this.nowQueued = true
    this.deps.microtask(() => {
      this.nowQueued = false
      void this.flush()
    })
  }

  /** The set that would be sent now (also for tests). */
  build(): PageLayers {
    const pages: PageEntry[] = [...this.sources.values()].map(({ src, order }) => ({
      windowId: src.windowId,
      order,
      rect: src.measure(),
      mode: src.mode(),
      element: src.element(),
    }))
    return buildPageLayers({
      parentWindow: this.deps.parentWindow(),
      epoch: this.epochId,
      generation: this.generation + 1,
      dpr: this.deps.dpr(),
      zoom: this.deps.zoom(),
      measuredScale: this.deps.measuredScale(),
      viewport: this.deps.viewport(),
      pages,
      claims: overlayClaims(),
      shapes: this.shapes,
    })
  }

  async flush(): Promise<void> {
    if (this.failed || !this.deps.active()) return
    const set = this.build()
    // An empty set only once: it clears the holes, then nothing to say.
    if (set.layers.length === 0 && this.sentEmpty) return
    this.generation = set.generation
    this.sentEmpty = set.layers.length === 0
    try {
      await this.deps.send(set, this.deps.options())
      if (this.reportedEpoch !== set.epoch && set.layers.length > 0) {
        this.reportedEpoch = set.epoch
        console.info(`[browser-underlay] window ${set.parentWindow}: underlay ON, first set applied (${set.layers.length} page(s))`)
      }
    } catch (e) {
      this.failed = true
      this.deps.fail(String(e))
    }
  }

  /** The epoch sets are sent in now. */
  get epoch(): string {
    return this.epochId
  }

  /** Back to legacy (setting off or a failure handled). Returns the epoch
   *  being left (K2 ignores its late sets); a later underlay starts a new
   *  one, with its generations from 1. */
  reset(): string {
    const left = this.epochId
    this.epochId = nextEpoch()
    this.generation = 0
    this.failed = false
    this.sentEmpty = true
    return left
  }

  /** A move to a display with another scale: claims are snapped to device
   *  pixels natively, so send a fresh set. */
  private watchDpr(): void {
    if (this.dprWatch || typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const arm = (): void => {
      const mq = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      const onChange = (): void => {
        mq.removeEventListener?.('change', onChange)
        this.dprWatch = null
        if (this.sources.size > 0) {
          arm()
          this.frame()
        }
      }
      mq.addEventListener?.('change', onChange)
      this.dprWatch = () => mq.removeEventListener?.('change', onChange)
    }
    arm()
  }
}

/** This window's aggregator (one per renderer window). */
export const pageLayersSync = new PageLayersSync()

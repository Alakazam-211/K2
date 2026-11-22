/**
 * PageLayers (K2 Design's format, patch plan §2f): the one description of
 * every browser page in this K2 window, sent whole on every change to
 * `browser_set_page_layers` (macOS underlay only; Windows and Linux keep
 * today's path).
 *
 * Before the canvas desktop, each visible Browser pane is one layer. The
 * overlay arbiter's claims (`browser-overlay.ts`) become `claims`: K2 UI
 * over pages that owns input there (menus, popovers, toasts, dialogs).
 * Drags and layout motion are not claims: they freeze the panes they cover
 * (`still`), as today.
 *
 * Nesting: a claim whose DOM root holds a page (a Browser inside a dialog)
 * does not cover that page. It goes into the other pages' `minus` instead
 * of the global claims.
 *
 * Not daemon state: per-window view state for native views in this window.
 */
import { invoke } from '@tauri-apps/api/core'
import {
  overlayClaims,
  subscribeOverlayClaims,
  type OverlayClaim,
  type OverlayRect,
} from './browser-overlay'

export type Corners = number | [number, number, number, number]

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
  /** Registration order: later panes sit in front (they never overlap today). */
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

export function buildPageLayers(input: {
  parentWindow: string
  epoch: string
  generation: number
  dpr: number
  viewport: [number, number]
  pages: readonly PageEntry[]
  claims: readonly OverlayClaim[]
}): PageLayers {
  const pages = input.pages
    .filter((p): p is PageEntry & { rect: PageShape } => p.rect !== null && p.rect.width > 0 && p.rect.height > 0)
    .slice()
    .sort((a, b) => a.order - b.order)
  const claims: PageClaim[] = []
  // Claims that hold a page: K2 things above the OTHER pages only.
  const nested: Array<{ shape: PageShape; holds: Set<string> }> = []
  for (const c of input.claims) {
    const kind = claimKind(c.kind)
    if (!kind) continue
    const shape = shapeOf(areaRect(c.area, input.viewport))
    if (shape.width <= 0 || shape.height <= 0) continue
    const holds = new Set(
      pages.filter((p) => c.element && p.element && c.element.contains(p.element)).map((p) => p.windowId),
    )
    if (holds.size > 0) nested.push({ shape, holds })
    else claims.push({ ...shape, id: c.id, kind })
  }
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
  viewport: () => [typeof window === 'undefined' ? 0 : window.innerWidth, typeof window === 'undefined' ? 0 : window.innerHeight],
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
  private frameQueued = false
  private nowQueued = false
  private sentEmpty = true
  private failed = false
  private unsubscribe: (() => void) | null = null
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
      viewport: this.deps.viewport(),
      pages,
      claims: overlayClaims(),
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

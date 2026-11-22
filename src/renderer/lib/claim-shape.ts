/**
 * A claim's real corners for PageLayers: the native side masks the page to
 * the K2 menu, popover, toast or dialog drawn over it, so the hole has to
 * match what the style draws (Glass radii, Terminal chamfers, frost).
 *
 * Read from the claim's element at send time. The element is the shape's
 * source: a fullscreen claim, a claim with no element, or one whose area is
 * not the element's box gets no shape. A shadow wrapper with nothing drawn
 * on it (Terminal's `data-k2-chamfer-float`) is looked through to the one
 * child that fills it.
 *
 * Cheap: computed style is read once per claim (by id and element, and the
 * style on <html>); an unchanged claim costs nothing, a moved one only
 * re-checks its box. Corners that depend on the size (percentages, the
 * browser's scale-down when radii overlap) and the viewport clip are worked
 * out from the cached reading on each send.
 */
import type { OverlayClaim, OverlayRect } from './browser-overlay'

export type Corners = number | [number, number, number, number]

export interface ClaimLook {
  radius?: Corners
  chamfer?: Corners
  frost?: true
}

type Four<T> = [T, T, T, T]
/** One corner's horizontal radius: px, or % of the box width. */
interface Len {
  v: number
  pct: boolean
}

interface Reading {
  radius: Four<Len>
  chamfer: Four<number> | null
  frost: boolean
}

interface Entry {
  claim: OverlayClaim
  element: Element
  style: string
  reading: Reading
  ownBox: boolean
}

const CORNER_PROPS = [
  'border-top-left-radius',
  'border-top-right-radius',
  'border-bottom-right-radius',
  'border-bottom-left-radius',
] as const

/** Attributes that say "this box is the drawn surface". */
const SURFACE_ATTRS = ['data-k2-chamfer', 'data-k2-chamfer-rows', 'data-material', 'data-frost-shape'] as const

/** How far a wrapper is looked through to the box it holds. */
const WRAPPER_DEPTH = 2

function computed(el: Element): CSSStyleDeclaration {
  return window.getComputedStyle(el)
}

function prop(cs: CSSStyleDeclaration, name: string): string {
  return (cs.getPropertyValue(name) ?? '').trim()
}

function rectOf(el: Element): OverlayRect {
  const r = el.getBoundingClientRect()
  return { x: r.x, y: r.y, width: r.width, height: r.height }
}

/** Within a CSS pixel: the same box. */
function sameBox(a: OverlayRect, b: OverlayRect): boolean {
  return (
    Math.abs(a.x - b.x) < 1 &&
    Math.abs(a.y - b.y) < 1 &&
    Math.abs(a.width - b.width) < 1 &&
    Math.abs(a.height - b.height) < 1
  )
}

/** A computed corner radius ("16px", "8px 20px", "10%"): its horizontal radius. */
function parseRadius(value: string): Len {
  const first = value.split(/\s+/)[0] ?? ''
  const n = parseFloat(first)
  if (!Number.isFinite(n) || n <= 0) return { v: 0, pct: false }
  return { v: n, pct: first.endsWith('%') }
}

/** A computed length custom property in px. Plain px parse directly; an
 *  unevaluated `calc()` (custom properties keep theirs) is resolved once
 *  through a hidden probe. */
function lengthPx(value: string): number {
  if (!value) return 0
  if (/^-?[\d.]+(px)?$/.test(value)) return parseFloat(value) || 0
  if (typeof document === 'undefined' || !document.body) return 0
  const probe = document.createElement('div')
  probe.style.position = 'absolute'
  probe.style.visibility = 'hidden'
  probe.style.width = value
  document.body.appendChild(probe)
  try {
    return parseFloat(computed(probe).width) || 0
  } finally {
    probe.remove()
  }
}

/** The Terminal chamfer this element draws, per corner, or null. The cut
 *  rules only apply in Terminal outside Gardens, and each one sets a
 *  clip-path: an attribute with no clip-path is a chamfer nobody draws. */
function chamferOf(el: Element, cs: CSSStyleDeclaration): Four<number> | null {
  const rows = el.getAttribute('data-k2-chamfer-rows')
  if (!el.hasAttribute('data-k2-chamfer') && rows === null) return null
  const clip = prop(cs, 'clip-path')
  if (!clip || clip === 'none') return null
  const cut = lengthPx(prop(cs, '--k2-ch-cut'))
  if (!(cut > 0)) return null
  if (rows === 'first') return [cut, cut, 0, 0]
  if (rows === 'last') return [0, 0, cut, cut]
  return [cut, cut, cut, cut]
}

function htmlAttr(name: string): string | null {
  return typeof document === 'undefined' ? null : document.documentElement.getAttribute(name)
}

/** Draws frost: a live backdrop blur. Where the browser reports no
 *  backdrop-filter at all, Glass's frost marker stands in (Glass draws
 *  frost only on `data-material="frost"`, and not in its tile fallback). */
function drawsFrost(el: Element, cs: CSSStyleDeclaration): boolean {
  const filters = [prop(cs, 'backdrop-filter'), prop(cs, '-webkit-backdrop-filter')]
  if (filters.some((f) => f !== '' && f !== 'none')) return true
  if (filters.some((f) => f === 'none')) return false
  return el.getAttribute('data-material') === 'frost' && htmlAttr('data-style') === 'glass' && htmlAttr('data-frost') !== 'tile'
}

function drawsSomething(el: Element, cs: CSSStyleDeclaration): boolean {
  if (SURFACE_ATTRS.some((a) => el.hasAttribute(a))) return true
  if (CORNER_PROPS.some((p) => parseRadius(prop(cs, p)).v > 0)) return true
  return drawsFrost(el, cs)
}

function read(root: Element): Reading {
  let el = root
  let cs = computed(el)
  for (let depth = 0; depth < WRAPPER_DEPTH && !drawsSomething(el, cs); depth++) {
    const only = el.children.length === 1 ? el.children[0] : null
    if (!only || !sameBox(rectOf(el), rectOf(only))) break
    el = only
    cs = computed(el)
  }
  return {
    radius: CORNER_PROPS.map((p) => parseRadius(prop(cs, p))) as Four<Len>,
    chamfer: chamferOf(el, cs),
    frost: drawsFrost(el, cs) || (el !== root && drawsFrost(root, computed(root))),
  }
}

const round = (v: number): number => Math.round(v * 100) / 100

function compact(four: Four<number>): Corners | undefined {
  if (four.every((v) => v <= 0)) return undefined
  return four.every((v) => v === four[0]) ? four[0] : four
}

/** The reading on this rect: % against the box, overlapping radii scaled
 *  down as the browser does, and square corners where the viewport clips. */
function resolve(reading: Reading, rect: OverlayRect, viewport: [number, number]): ClaimLook {
  const { x, y, width: w, height: h } = rect
  const left = x < -0.5
  const top = y < -0.5
  const right = x + w > viewport[0] + 0.5
  const bottom = y + h > viewport[1] + 0.5
  const clipped = [top || left, top || right, bottom || right, bottom || left]
  const square = (four: Four<number>): Four<number> => four.map((v, i) => (clipped[i] ? 0 : round(v))) as Four<number>
  const look: ClaimLook = {}
  if (reading.chamfer) {
    const lim = Math.min(w, h) / 2
    look.chamfer = compact(square(reading.chamfer.map((c) => Math.min(c, lim)) as Four<number>))
  } else {
    const [tl, tr, br, bl] = reading.radius.map((l) => (l.pct ? (l.v / 100) * w : l.v))
    const fit = (side: number, a: number, b: number): number => (a + b > side ? side / (a + b) : 1)
    const f = Math.min(1, fit(w, tl, tr), fit(w, bl, br), fit(h, tl, bl), fit(h, tr, br))
    look.radius = compact(square([tl * f, tr * f, br * f, bl * f]))
  }
  if (look.radius === undefined) delete look.radius
  if (look.chamfer === undefined) delete look.chamfer
  if (reading.frost) look.frost = true
  return look
}

/** The style on <html> a reading depends on (attributes only, no layout). */
function styleKey(): string {
  return `${htmlAttr('data-style')}|${htmlAttr('data-palette')}|${htmlAttr('data-frost')}`
}

/** Claim shapes, cached per claim. One per PageLayersSync. */
export class ClaimShapes {
  private readonly cache = new Map<number, Entry>()

  /** The claim's corners and frost on `rect` (its area now), or `{}`. */
  look(claim: OverlayClaim, rect: OverlayRect, viewport: [number, number]): ClaimLook {
    const element = claim.element
    if (claim.area === 'fullscreen' || !element || typeof window === 'undefined') {
      this.cache.delete(claim.id)
      return {}
    }
    const style = styleKey()
    let entry = this.cache.get(claim.id)
    if (!entry || entry.element !== element || entry.style !== style) {
      entry = { claim, element, style, reading: read(element), ownBox: sameBox(rectOf(element), claim.area) }
      this.cache.set(claim.id, entry)
    } else if (entry.claim !== claim) {
      // Moved or resized: same element, same style; only the box is checked.
      entry.claim = claim
      entry.ownBox = sameBox(rectOf(element), claim.area)
    }
    return entry.ownBox ? resolve(entry.reading, rect, viewport) : {}
  }

  /** Forget claims that are gone. */
  keep(ids: ReadonlySet<number>): void {
    for (const id of this.cache.keys()) if (!ids.has(id)) this.cache.delete(id)
  }
}

import { useId, useLayoutEffect, useRef, type JSX } from 'react'
import { appZoom } from '@/lib/app-zoom'
import {
  MERGE_K,
  MERGE_SETTLE_MS,
  boundsOf,
  joinBar,
  maskPad,
  mergeNeighbours,
  renderMergeMasks,
  shapeFromBox,
  type Box,
  type MergeShadow,
} from './floatMerge'

/** On the floats' row while Glass's merged layer is drawn: the merging
 *  pieces drop their own glass (globals.css, "merged"). Until then, or with
 *  no canvas, each keeps its own. */
export const FLOAT_MERGED_ATTR = 'data-k2-merged'
/** On the floats' row while Terminal's joined bar is drawn. */
export const FLOAT_JOINED_ATTR = 'data-k2-joined'
/** On each piece drawn by the layer (the plate and what it merges with). */
export const FLOAT_PIECE_ATTR = 'data-k2-merge-piece'

type Look = 'glass' | 'terminal' | null

function lookNow(): Look {
  const root = document.documentElement
  const style = root.getAttribute('data-style')
  if (style === 'terminal') return 'terminal'
  // Glass's reduced-transparency fallback is a solid tile: nothing to melt.
  if (style === 'glass' && root.getAttribute('data-frost') !== 'tile') return 'glass'
  return null
}

function shown(el: HTMLElement, r: DOMRect): boolean {
  return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility === 'visible'
}

/** Resolves once a mask image is decoded. WebKit decodes a data URL set
 *  straight onto mask-image later, and shows the layer unmasked for that
 *  frame (a flash of the whole region). Resolves anyway where decoding isn't
 *  available. */
function decoded(url: string): Promise<void> {
  if (typeof Image === 'undefined') return Promise.resolve()
  const img = new Image()
  img.src = url
  return typeof img.decode === 'function' ? img.decode().catch(() => undefined) : Promise.resolve()
}

const px = (v: string): number => {
  const n = parseFloat(v)
  return Number.isFinite(n) ? n : 0
}

/**
 * The merged glass (Glass) or the joined bar (Terminal) under Thread view's
 * floating objects (floatMerge.ts): first in the floats' row, behind every
 * object. The name's object (the plate) melts into, or joins, the objects
 * it comes close to.
 *
 * Glass is drawn from canvas masks, which are only drawn when the row is
 * still: while a window resize or a pane drag moves it, each piece shows
 * its own glass, and the masks follow MERGE_SETTLE_MS after it stops. A
 * redraw is shown only once its masks are decoded, and only if nothing
 * newer came since; the first is shown at once, so a Thread opens merged.
 * Terminal's bar is an SVG path, cheap enough to follow every frame.
 *
 * The row is this layer's own parent, read from its DOM node (a parent's
 * ref is attached only after its children's layout effects run).
 */
export function ThreadFloatMerge(): JSX.Element {
  const rootRef = useRef<HTMLDivElement>(null)
  const clipId = `k2-merge-clip-${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`
  useLayoutEffect(() => {
    const root = rootRef.current
    const host = root?.parentElement ?? null
    if (!root || !host || typeof ResizeObserver === 'undefined') return
    const part = (name: string): HTMLElement => root.querySelector<HTMLElement>(`[data-k2-merge="${name}"]`)!
    const glass = part('glass')
    const layers = { fill: part('fill'), edge: part('edge'), sheen: part('sheen'), shadow: part('shadow') }
    const bar = root.querySelector<SVGSVGElement>('[data-k2-merge="bar"]')!
    const barPaths = bar.querySelectorAll('path')
    let frame = 0
    let timer: ReturnType<typeof setTimeout> | null = null
    let lastKey = ''
    let drawn = false
    // Bumped by every draw and every clear: a redraw whose masks finish
    // decoding after that is dropped.
    let generation = 0
    let pieces = new Set<HTMLElement>()

    const mark = (next: Set<HTMLElement>): void => {
      for (const el of pieces) if (!next.has(el)) el.removeAttribute(FLOAT_PIECE_ATTR)
      for (const el of next) el.setAttribute(FLOAT_PIECE_ATTR, '')
      pieces = next
    }
    const clear = (): void => {
      generation += 1
      host.removeAttribute(FLOAT_MERGED_ATTR)
      host.removeAttribute(FLOAT_JOINED_ATTR)
      mark(new Set())
    }

    // The plate and the objects beside it, in the layer's CSS px.
    const measure = () => {
      const look = lookNow()
      const plateEl = host.querySelector<HTMLElement>('[data-k2-float="name"]')
      if (!look || !plateEl) return null
      const z = appZoom()
      const base = host.getBoundingClientRect()
      const box = (r: DOMRect): Box => ({
        x0: (r.left - base.left) / z - host.clientLeft,
        y0: (r.top - base.top) / z - host.clientTop,
        x1: (r.right - base.left) / z - host.clientLeft,
        y1: (r.bottom - base.top) / z - host.clientTop,
      })
      const pr = plateEl.getBoundingClientRect()
      if (!shown(plateEl, pr)) return null
      const els: HTMLElement[] = []
      const boxes: Box[] = []
      for (const el of host.querySelectorAll<HTMLElement>('.thread-float-group [data-k2-float]')) {
        if (el.parentElement?.closest('[data-k2-float]')) continue
        const r = el.getBoundingClientRect()
        if (!shown(el, r)) continue
        els.push(el)
        boxes.push(box(r))
      }
      return { look, z, plateEl, plate: box(pr), els, boxes }
    }
    type Geometry = NonNullable<ReturnType<typeof measure>>

    const keyOf = (g: Geometry | null): string => {
      if (!g) return 'none'
      const q = (b: Box): number[] => [b.x0, b.y0, b.x1, b.y1].map((n) => Math.round(n * 4) / 4)
      const html = document.documentElement
      return JSON.stringify([
        g.look,
        html.getAttribute('data-palette'),
        g.z,
        window.devicePixelRatio,
        q(g.plate),
        g.boxes.map(q),
      ])
    }

    const drawTerminal = (g: Geometry): void => {
      const cut = px(getComputedStyle(root).getPropertyValue('--chamfer-control'))
      const joinedBar = joinBar(g.plate, g.boxes, { k: MERGE_K, cut })
      host.removeAttribute(FLOAT_MERGED_ATTR)
      if (!joinedBar) {
        clear()
        return
      }
      const b = joinedBar.bounds
      bar.style.left = `${b.x0}px`
      bar.style.top = `${b.y0}px`
      bar.setAttribute('width', String(b.x1 - b.x0))
      bar.setAttribute('height', String(b.y1 - b.y0))
      bar.setAttribute('viewBox', `${b.x0} ${b.y0} ${b.x1 - b.x0} ${b.y1 - b.y0}`)
      const dividers = joinedBar.dividers.map((d) => `M${d.x0} ${d.y0}H${d.x1}V${d.y1}H${d.x0}Z`).join('')
      barPaths[0].setAttribute('d', joinedBar.path)
      barPaths[1].setAttribute('d', joinedBar.path)
      barPaths[2].setAttribute('d', dividers)
      mark(new Set([g.plateEl, ...joinedBar.joined.map((i) => g.els[i])]))
      host.setAttribute(FLOAT_JOINED_ATTR, '')
    }

    const drawGlass = (g: Geometry, first: boolean): void => {
      host.removeAttribute(FLOAT_JOINED_ATTR)
      const near = mergeNeighbours(g.plate, g.boxes, MERGE_K)
      if (near.length === 0) {
        clear()
        return
      }
      // The look's own numbers, from the theme (globals.css): the rim's
      // width and, in Veil, the drop under the glass.
      const cs = getComputedStyle(root)
      const edge = px(cs.getPropertyValue('--k2-merge-edge')) || 0.5
      const blur = px(cs.getPropertyValue('--k2-merge-shadow-blur'))
      const shadow: MergeShadow | null = blur > 0 ? { blur, y: px(cs.getPropertyValue('--k2-merge-shadow-y')) } : null
      const radius = (el: HTMLElement): number => px(getComputedStyle(el).borderTopLeftRadius)
      const all = [g.plate, ...near.map((i) => g.boxes[i])]
      const region = boundsOf(all, maskPad(MERGE_K, shadow))
      const at = (b: Box): Box => ({ x0: b.x0 - region.x0, y0: b.y0 - region.y0, x1: b.x1 - region.x0, y1: b.y1 - region.y0 })
      const masks = renderMergeMasks({
        width: region.x1 - region.x0,
        height: region.y1 - region.y0,
        scale: g.z * (window.devicePixelRatio || 1),
        plate: shapeFromBox(at(g.plate), radius(g.plateEl)),
        others: near.map((i) => shapeFromBox(at(g.boxes[i]), radius(g.els[i]))),
        k: MERGE_K,
        edge,
        shadow,
      })
      if (!masks) {
        clear()
        return
      }
      // A layer with no mask (no shadow in this look) isn't drawn at all: an
      // unmasked layer would cover the whole region.
      const apply = (el: HTMLElement, url: string | null): void => {
        const v = url ? `url(${url})` : 'none'
        el.style.setProperty('-webkit-mask-image', v)
        el.style.setProperty('mask-image', v)
        el.hidden = !url
      }
      const show = (): void => {
        Object.assign(glass.style, {
          left: `${region.x0}px`,
          top: `${region.y0}px`,
          width: `${region.x1 - region.x0}px`,
          height: `${region.y1 - region.y0}px`,
        })
        apply(layers.fill, masks.fill)
        apply(layers.edge, masks.edge)
        apply(layers.sheen, masks.sheen)
        apply(layers.shadow, masks.shadow)
        mark(new Set([g.plateEl, ...near.map((i) => g.els[i])]))
        host.setAttribute(FLOAT_MERGED_ATTR, '')
      }
      if (first) {
        show()
        return
      }
      // Each piece keeps its own glass until the new masks are ready.
      const mine = generation
      const urls = [masks.fill, masks.edge, masks.sheen, masks.shadow].filter((u): u is string => !!u)
      void Promise.all(urls.map(decoded)).then(() => {
        if (mine === generation) show()
      })
    }

    const draw = (): void => {
      timer = null
      generation += 1
      const g = measure()
      lastKey = keyOf(g)
      const first = !drawn
      drawn = true
      if (!g) clear()
      else if (g.look === 'terminal') drawTerminal(g)
      else drawGlass(g, first)
    }

    const check = (): void => {
      frame = 0
      const g = measure()
      const key = keyOf(g)
      if (key === lastKey) return
      // Terminal's path and an empty layer are cheap: now. The first draw
      // too, so a Thread never opens unmerged.
      if (!g || g.look === 'terminal' || !drawn) {
        if (timer) clearTimeout(timer)
        draw()
        return
      }
      // Glass is moving: each piece shows its own glass until it stops (and
      // masks still decoding from before are dropped).
      clear()
      if (timer) clearTimeout(timer)
      timer = setTimeout(draw, MERGE_SETTLE_MS)
    }
    const schedule = (): void => {
      if (!frame) frame = requestAnimationFrame(check)
    }

    const ro = new ResizeObserver(schedule)
    const watch = (): void => {
      ro.disconnect()
      ro.observe(host)
      for (const el of host.querySelectorAll('[data-k2-float]')) ro.observe(el)
    }
    watch()
    // Objects come and go (search opens, a menu's label hides).
    const mo = new MutationObserver(() => {
      watch()
      schedule()
    })
    mo.observe(host, { childList: true, subtree: true })
    // The look, the palette and the app zoom (a style on <html>).
    const themeMo = new MutationObserver(schedule)
    themeMo.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-style', 'data-palette', 'data-frost', 'style'],
    })
    schedule()
    return () => {
      if (frame) cancelAnimationFrame(frame)
      if (timer) clearTimeout(timer)
      ro.disconnect()
      mo.disconnect()
      themeMo.disconnect()
      clear()
    }
  }, [])
  return (
    <div ref={rootRef} className="thread-float-merge" data-k2-merge-layer="" data-testid="thread-float-merge" aria-hidden="true">
      <div data-k2-merge="glass">
        <div data-k2-merge="shadow" />
        <div data-k2-merge="fill" />
        <div data-k2-merge="edge" />
        <div data-k2-merge="sheen" />
      </div>
      <svg data-k2-merge="bar" focusable="false">
        <defs>
          <clipPath id={clipId}>
            <path />
          </clipPath>
        </defs>
        <path data-k2-merge="bar-fill" clipPath={`url(#${clipId})`} />
        <path data-k2-merge="bar-dividers" />
      </svg>
    </div>
  )
}

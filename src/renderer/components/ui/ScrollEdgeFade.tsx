// Soft scroll edges for a list between two bars (Rosson 2026-11-14, the Home
// nav: "the top and bottom of the scrolling area should fade like the Thread
// does, not cut off hard"). The list scrolls in a frame; a band at each end
// blurs and tints the rows as they pass the bar, the Thread's treatment:
//
//   - Glass: the phone's header stack (the Thread's top band,
//     ProgressiveHeaderBlur), mirrored at the bottom as the Thread's footer
//     is. Four masked blurs under a tint of the list's own colour.
//   - Terminal (any other Style): the Thread kit's progressive blur (its
//     message-box strip) under the Thread header's no-blur scrim tint, so
//     the rows are near hidden at the bar instead of cut.
//   - No blur (reduced transparency, no backdrop-filter, Glass's tile
//     fallback): the tint alone, the heavier ramp in Glass.
//
// A band shows only on an edge with more list beyond it: none while the list
// fits, the top once scrolled down, the bottom while there is more below.
// The edges are read on scroll (one passive listener, at most once a frame)
// and on resize or the list growing (a ResizeObserver), and written as data
// attributes on the frame: no React render per scroll. Never takes clicks,
// never moves the layout; the list keeps its keyboard focus clear of the
// bands (scroll-padding). A blur never animates (the WebKit stale-layer
// rule): a band is shown or hidden, never faded.

import { useLayoutEffect, useRef, type CSSProperties, type JSX, type ReactNode, type RefObject } from 'react'
import { cx } from './cx'
import {
  FOOTER_BLUR_LAYERS,
  GLASS_BAND_PX,
  GLASS_BLUR_LAYERS,
  GLASS_TINT_STOPS,
  GLASS_TINT_STOPS_TILE,
  SCRIM_TINT_STOPS,
} from './scrollEdgeLayers'

export type ScrollEdge = 'top' | 'bottom'

/** A band's height (px), both looks: the phone's header band. */
export const SCROLL_EDGE_PX = GLASS_BAND_PX

/** Under this much (px) left to scroll, an edge counts as reached (zoom
 *  leaves fractional scroll offsets). */
export const SCROLL_EDGE_SLACK_PX = 1

/** The bands' tint in a dialog: the dialog's own fill (its Surface is a
 *  tile in Glass, the surface colour elsewhere). */
export const DIALOG_EDGE_TINT = 'var(--tile-fill, var(--color-bg-surface))'

/** The colour the bands tint with: the list's own surface. */
export const SCROLL_EDGE_TINT_VAR = '--k2-edge-tint'

/** Which ends have more list beyond them. All three numbers are the
 *  scroller's own CSS px (scrollTop, clientHeight, scrollHeight), so app
 *  zoom (CSS `zoom` on <html>) scales them alike and no rect math is needed. */
export function scrollEdges(scrollTop: number, clientHeight: number, scrollHeight: number): Record<ScrollEdge, boolean> {
  const room = scrollHeight - clientHeight
  if (room <= SCROLL_EDGE_SLACK_PX) return { top: false, bottom: false }
  return { top: scrollTop > SCROLL_EDGE_SLACK_PX, bottom: scrollTop < room - SCROLL_EDGE_SLACK_PX }
}

const FRAME = '[data-scroll-edges]'
const band = (edge: ScrollEdge): string => `${FRAME} > [data-scroll-edge="${edge}"]`
const GLASS = 'html[data-style="glass"]'
const EDGES: readonly ScrollEdge[] = ['top', 'bottom']
/** A mask reads only alpha: `black` is "shown", whatever the colour. */
const ON = 'black'
/** From the bar into the list, and from the list out to the bar. */
const fromBar = (edge: ScrollEdge): string => (edge === 'top' ? 'to bottom' : 'to top')
const toBar = (edge: ScrollEdge): string => (edge === 'top' ? 'to top' : 'to bottom')

const tintMix = (pct: number): string =>
  pct === 0 ? 'transparent' : `color-mix(in srgb, var(${SCROLL_EDGE_TINT_VAR}) ${pct}%, transparent)`

/** How much of a band (% from the bar) is solid before the Thread's ramp
 *  starts. Unlike the Thread's (which meets another surface), these bands
 *  meet a bar of the same fill: below full strength at the bar, the blurred
 *  rows showed as a haze that stopped in a straight line. 10% of 40px = 4px. */
export const SCROLL_EDGE_LIP_PCT = 10

/** A ramp with a solid lip at the bar: 100% for the lip, then the ramp
 *  squeezed into the rest. */
export function solidAtBar(stops: ReadonlyArray<readonly [number, number]>): Array<[number, number]> {
  const rest = (100 - SCROLL_EDGE_LIP_PCT) / 100
  return [[0, 100], ...stops.map(([at, pct]): [number, number] => [Math.round((SCROLL_EDGE_LIP_PCT + at * rest) * 10) / 10, pct])]
}

/** The tint, from the bar inward ([% from the bar, % of the list's colour]),
 *  solid at the bar. */
export function edgeTintCss(edge: ScrollEdge, stops: ReadonlyArray<readonly [number, number]>): string {
  return `background: linear-gradient(${fromBar(edge)}, ${solidAtBar(stops).map(([at, pct]) => `${tintMix(pct)} ${at}%`).join(', ')});`
}

const blurCss = (px: number, mask: string): string =>
  `-webkit-backdrop-filter: blur(${px}px); backdrop-filter: blur(${px}px); -webkit-mask-image: ${mask}; mask-image: ${mask};`

/** Terminal: the kit's layer, [blur, clear until %, fully on at %] from the
 *  list side out to the bar (heavy at the bar, as under the message box). */
const terminalLayer = (edge: ScrollEdge, [px, from, to]: readonly [number, number, number], n: number): string =>
  `${band(edge)} > .k2-edge-blur-${n + 1} { ${blurCss(px, `linear-gradient(${toBar(edge)}, transparent ${from}%, ${ON} ${to}%)`)} }`

/** Glass: the phone's layer, [blur, solid until %, clear at %] from the bar. */
const glassLayer = (edge: ScrollEdge, [px, solid, clear]: readonly [number, number, number], n: number): string =>
  `${GLASS} ${band(edge)} > .k2-edge-blur-${n + 1} { ${blurCss(px, `linear-gradient(${fromBar(edge)}, ${ON} 0%, ${ON} ${solid}%, transparent ${clear}%)`)} }`

const BLUR_SPANS = `${FRAME} > [data-scroll-edge] > [data-edge-blur]`

export const SCROLL_EDGE_CSS = `
${FRAME} { ${SCROLL_EDGE_TINT_VAR}: var(--color-bg); }
${FRAME} > [data-scroll-edges-list] { isolation: isolate; scroll-padding-block: ${SCROLL_EDGE_PX}px; }
${FRAME} > [data-scroll-edge] { position: absolute; left: 0; right: 0; height: ${SCROLL_EDGE_PX}px; z-index: 1; pointer-events: none; visibility: hidden; }
${band('top')} { top: 0; }
${band('bottom')} { bottom: 0; }
${FRAME}[data-edge-top="1"] > [data-scroll-edge="top"], ${FRAME}[data-edge-bottom="1"] > [data-scroll-edge="bottom"] { visibility: visible; }
${FRAME} > [data-scroll-edge] > span { position: absolute; inset: 0; pointer-events: none; }
${EDGES.map((e) => FOOTER_BLUR_LAYERS.map((l, i) => terminalLayer(e, l, i)).join('\n')).join('\n')}
${EDGES.map((e) => `${band(e)} > .k2-edge-tint { ${edgeTintCss(e, SCRIM_TINT_STOPS)} }`).join('\n')}
${EDGES.map((e) => GLASS_BLUR_LAYERS.map((l, i) => glassLayer(e, l, i)).join('\n')).join('\n')}
${EDGES.map((e) => `${GLASS} ${band(e)} > .k2-edge-tint { ${edgeTintCss(e, GLASS_TINT_STOPS)} }`).join('\n')}
${EDGES.map((e) => `${GLASS} ${FRAME}[data-edge-heavy] > [data-scroll-edge="${e}"] > .k2-edge-tint { ${edgeTintCss(e, GLASS_TINT_STOPS_TILE)} }`).join('\n')}
${GLASS}[data-frost="tile"] ${BLUR_SPANS} { display: none; }
${EDGES.map((e) => `${GLASS}[data-frost="tile"] ${band(e)} > .k2-edge-tint { ${edgeTintCss(e, GLASS_TINT_STOPS_TILE)} }`).join('\n')}
@media (prefers-reduced-transparency: reduce) {
  ${BLUR_SPANS} { display: none; }
}
@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {
  ${BLUR_SPANS} { display: none; }
}
`

/** Keeps `data-edge-top` / `data-edge-bottom` ("1" or "0") on `frame` in
 *  step with `scroller`: on scroll (passive, at most once a frame), and when
 *  the scroller or its `content` changes size (rows added or removed). */
export function useScrollEdges(
  frame: RefObject<HTMLElement | null>,
  scroller: RefObject<HTMLElement | null>,
  content: RefObject<HTMLElement | null>,
): void {
  useLayoutEffect(() => {
    const f = frame.current
    const s = scroller.current
    if (!f || !s) return
    let raf = 0
    const read = (): void => {
      raf = 0
      const edges = scrollEdges(s.scrollTop, s.clientHeight, s.scrollHeight)
      for (const edge of EDGES) {
        const key = edge === 'top' ? 'edgeTop' : 'edgeBottom'
        const next = edges[edge] ? '1' : '0'
        if (f.dataset[key] !== next) f.dataset[key] = next
      }
    }
    const soon = (): void => {
      if (raf === 0) raf = requestAnimationFrame(read)
    }
    read()
    s.addEventListener('scroll', soon, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(soon)
    ro?.observe(s)
    if (content.current) ro?.observe(content.current)
    return () => {
      s.removeEventListener('scroll', soon)
      ro?.disconnect()
      if (raf !== 0) cancelAnimationFrame(raf)
    }
  }, [frame, scroller, content])
}

/** One band: the blur layers under the tint. Inside a ScrollEdgeFrame. */
export function ScrollEdgeFade({ edge }: { edge: ScrollEdge }): JSX.Element {
  return (
    <div data-scroll-edge={edge} aria-hidden="true">
      {GLASS_BLUR_LAYERS.map((_, i) => (
        <span key={i} className={`k2-edge-blur-${i + 1}`} data-edge-blur="" />
      ))}
      <span className="k2-edge-tint" />
    </div>
  )
}

export interface ScrollEdgeFrameProps {
  /** The frame's classes: it takes the list's place in the layout (give it
   *  its size, e.g. `flex-1 min-h-0`). */
  className?: string
  /** The scroller's classes (overflow, padding). It fills the frame. */
  scrollClassName?: string
  /** What the bands tint with: the surface the list sits on (default the
   *  page, `var(--color-bg)`). Never black: it is the list's own colour. */
  tint?: string
  /** The heavier tint (Glass's no-blur ramp) under the blurs: for a list in
   *  a dialog, where a row must never read through the band beside a bar
   *  even if the webview draws no blur there. */
  heavy?: boolean
  testId?: string
  children: ReactNode
}

/** A scrolling list whose ends fade softly under the bars beside it. */
export function ScrollEdgeFrame({ className, scrollClassName, tint, heavy = false, testId, children }: ScrollEdgeFrameProps): JSX.Element {
  const frame = useRef<HTMLDivElement>(null)
  const scroller = useRef<HTMLDivElement>(null)
  const content = useRef<HTMLDivElement>(null)
  useScrollEdges(frame, scroller, content)
  const style = tint ? ({ [SCROLL_EDGE_TINT_VAR]: tint } as CSSProperties) : undefined
  return (
    <div ref={frame} className={cx('relative', className)} data-scroll-edges="" data-edge-heavy={heavy ? '' : undefined} data-testid={testId} style={style}>
      <style data-scroll-edge-styles="">{SCROLL_EDGE_CSS}</style>
      <div ref={scroller} className={cx('absolute inset-0', scrollClassName)} data-scroll-edges-list="">
        <div ref={content}>{children}</div>
      </div>
      <ScrollEdgeFade edge="top" />
      <ScrollEdgeFade edge="bottom" />
    </div>
  )
}

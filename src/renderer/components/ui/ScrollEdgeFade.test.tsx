// @vitest-environment jsdom
// Soft scroll edges (Rosson 2026-11-14, the Home nav: "the top and bottom of
// the scrolling area should fade like the Thread does, not cut off hard";
// only on an edge with more list beyond it).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { act, cleanup, render } from '@testing-library/react'
import {
  DIALOG_EDGE_TINT,
  edgeTintCss,
  SCROLL_EDGE_CSS,
  SCROLL_EDGE_LIP_PCT,
  SCROLL_EDGE_PX,
  solidAtBar,
  SCROLL_EDGE_TINT_VAR,
  ScrollEdgeFrame,
  scrollEdges,
} from './ScrollEdgeFade'
import {
  FOOTER_BLUR_LAYERS,
  GLASS_BAND_PX,
  GLASS_BLUR_LAYERS,
  GLASS_TINT_STOPS,
  GLASS_TINT_STOPS_TILE,
  SCRIM_TINT_STOPS,
} from './scrollEdgeLayers'
import * as kit from '@/components/Thread/ThreadZenKit'

const globals = readFileSync(resolve(__dirname, '..', '..', 'globals.css'), 'utf8')

/** The one line of a stylesheet holding `needle`, or a loud error. */
const ruleFor = (css: string, needle: string): string => {
  const line = css.split('\n').find((l) => l.includes(needle))
  if (!line) throw new Error(`no rule with ${needle}`)
  return line
}
/** A multi-line rule's body in globals.css, from its selector line. */
const globalsBody = (selector: string): string => {
  const at = globals.indexOf(`${selector} {`)
  if (at < 0) throw new Error(`no globals rule ${selector}`)
  return globals.slice(at, globals.indexOf('}', at))
}
/** [stop %, colour %] pairs of a `color-mix(…var(X) P%, transparent) A%` ramp. */
const stopsOf = (body: string): Array<[number, number]> =>
  Array.from(body.matchAll(/(?:color-mix\(in srgb, var\([^)]*\)\)? (\d+)%, transparent\)|transparent) ([\d.]+)%/g)).map((m) => [
    Number(m[2]),
    m[1] == null ? 0 : Number(m[1]),
  ])

describe('which edges have more list beyond them', () => {
  it('a list that fits has no edges', () => {
    expect(scrollEdges(0, 400, 400)).toEqual({ top: false, bottom: false })
    // A fraction of a px over (app zoom) still fits.
    expect(scrollEdges(0, 400, 400.6)).toEqual({ top: false, bottom: false })
  })
  it('at the top only the bottom; at the end only the top; in the middle both', () => {
    expect(scrollEdges(0, 400, 1200)).toEqual({ top: false, bottom: true })
    expect(scrollEdges(800, 400, 1200)).toEqual({ top: true, bottom: false })
    expect(scrollEdges(400, 400, 1200)).toEqual({ top: true, bottom: true })
    // Within a px of either end (zoom's fractional offsets) counts as there.
    expect(scrollEdges(0.5, 400, 1200)).toEqual({ top: false, bottom: true })
    expect(scrollEdges(799.4, 400, 1200)).toEqual({ top: true, bottom: false })
  })
})

describe('the frame follows the list', () => {
  let observers: Array<() => void> = []
  let frames: FrameRequestCallback[] = []
  /** Runs the frame the reads were queued for. */
  const nextFrame = (): void => {
    const due = frames
    frames = []
    for (const cb of due) cb(0)
  }
  beforeEach(() => {
    observers = []
    frames = []
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb))
    vi.stubGlobal('cancelAnimationFrame', () => {})
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(cb: ResizeObserverCallback) {
          observers.push(() => cb([], this as unknown as ResizeObserver))
        }
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    )
  })
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  /** Gives the scroller a size; the frame reads it on its next look. */
  const size = (list: HTMLElement, client: number, height: number, top = 0): void => {
    Object.defineProperty(list, 'clientHeight', { configurable: true, value: client })
    Object.defineProperty(list, 'scrollHeight', { configurable: true, value: height })
    // jsdom clamps a set scrollTop to 0 (no layout): give it outright.
    Object.defineProperty(list, 'scrollTop', { configurable: true, value: top })
  }
  const mount = (): { frame: HTMLElement; list: HTMLElement } => {
    render(
      <ScrollEdgeFrame className="flex-1 min-h-0" scrollClassName="overflow-y-auto" tint="var(--tile-fill)" testId="f">
        <button type="button">row</button>
      </ScrollEdgeFrame>,
    )
    const frame = document.querySelector('[data-testid="f"]') as HTMLElement
    return { frame, list: frame.querySelector('[data-scroll-edges-list]') as HTMLElement }
  }
  const edges = (frame: HTMLElement): [string | undefined, string | undefined] => [frame.dataset.edgeTop, frame.dataset.edgeBottom]

  it('no band while the list fits; bottom at the top; both in the middle; top at the end', () => {
    const { frame, list } = mount()
    // jsdom lays nothing out: an empty list fits.
    expect(edges(frame)).toEqual(['0', '0'])
    // The list grows past the frame (agents added): the content's size changes.
    size(list, 400, 1200)
    act(() => {
      observers.forEach((fire) => fire())
      nextFrame()
    })
    expect(edges(frame)).toEqual(['0', '1'])
    size(list, 400, 1200, 400)
    act(() => {
      // Many scroll events in a frame: one read.
      list.dispatchEvent(new Event('scroll'))
      list.dispatchEvent(new Event('scroll'))
      expect(frames).toHaveLength(1)
      nextFrame()
    })
    expect(edges(frame)).toEqual(['1', '1'])
    size(list, 400, 1200, 800)
    act(() => {
      // Many scroll events in a frame: one read.
      list.dispatchEvent(new Event('scroll'))
      list.dispatchEvent(new Event('scroll'))
      expect(frames).toHaveLength(1)
      nextFrame()
    })
    expect(edges(frame)).toEqual(['1', '0'])
    // The window grows taller than the list (a resize): no bands.
    size(list, 1300, 1200, 0)
    act(() => {
      observers.forEach((fire) => fire())
      nextFrame()
    })
    expect(edges(frame)).toEqual(['0', '0'])
  })

  it('watches the scroller and the list inside it, and lets go on unmount', () => {
    const watched: Element[] = []
    const gone = vi.fn()
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe(el: Element) {
          watched.push(el)
        }
        disconnect() {
          gone()
        }
        unobserve() {}
      },
    )
    const { frame, list } = mount()
    expect(watched).toEqual([list, list.firstElementChild])
    cleanup()
    expect(gone).toHaveBeenCalled()
    expect(frame.isConnected).toBe(false)
  })

  it('scroll is one passive listener: a scroll writes attributes, never renders', () => {
    const add = vi.spyOn(HTMLElement.prototype, 'addEventListener')
    const { list } = mount()
    // Only the list's own (React's root listens for its own reasons).
    const scrolls = add.mock.calls.filter(([type], i) => type === 'scroll' && add.mock.contexts[i] === list)
    expect(scrolls).toHaveLength(1)
    expect(scrolls[0][2]).toEqual({ passive: true })
    add.mockRestore()
  })

  it('two bands, hidden from assistive tech, after the list; the tint is the one asked for', () => {
    const { frame, list } = mount()
    const bands = Array.from(frame.querySelectorAll(':scope > [data-scroll-edge]'))
    expect(bands.map((b) => b.getAttribute('data-scroll-edge'))).toEqual(['top', 'bottom'])
    for (const b of bands) {
      expect(b.getAttribute('aria-hidden')).toBe('true')
      expect(Array.from(b.children).map((c) => c.className)).toEqual([
        'k2-edge-blur-1',
        'k2-edge-blur-2',
        'k2-edge-blur-3',
        'k2-edge-blur-4',
        'k2-edge-tint',
      ])
    }
    // The list fills the frame (absolute), so the frame keeps the list's
    // place in the layout and a band never moves anything.
    expect(list.className).toContain('absolute inset-0')
    expect(frame.className).toContain('relative')
    expect(frame.style.getPropertyValue(SCROLL_EDGE_TINT_VAR)).toBe('var(--tile-fill)')
  })
})

describe('the bands: never in the way', () => {
  it('take no clicks, sit over the rows, show only on an edge with more beyond it, and never animate', () => {
    const base = ruleFor(SCROLL_EDGE_CSS, '[data-scroll-edges] > [data-scroll-edge] {')
    expect(base).toContain('position: absolute;')
    expect(base).toContain(`height: ${SCROLL_EDGE_PX}px;`)
    expect(base).toContain('pointer-events: none;')
    expect(base).toContain('visibility: hidden;')
    expect(SCROLL_EDGE_CSS).toContain('[data-scroll-edges] > [data-scroll-edge] > span { position: absolute; inset: 0; pointer-events: none; }')
    expect(SCROLL_EDGE_CSS).toContain(
      '[data-scroll-edges][data-edge-top="1"] > [data-scroll-edge="top"], [data-scroll-edges][data-edge-bottom="1"] > [data-scroll-edge="bottom"] { visibility: visible; }',
    )
    // The list is its own layer, so a row's own z-index never paints over a band.
    const list = ruleFor(SCROLL_EDGE_CSS, '[data-scroll-edges] > [data-scroll-edges-list] {')
    expect(list).toContain('isolation: isolate;')
    // Keyboard focus scrolls a row clear of the bands.
    expect(list).toContain(`scroll-padding-block: ${SCROLL_EDGE_PX}px;`)
    // A blur never animates (WebKit's stale layers); no will-change.
    expect(SCROLL_EDGE_CSS).not.toMatch(/transition|animation|will-change/)
  })

  it('no blur with reduced transparency or without backdrop-filter: the tint alone', () => {
    for (const head of ['@media (prefers-reduced-transparency: reduce) {', '@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {']) {
      const at = SCROLL_EDGE_CSS.indexOf(head)
      expect(at, head).toBeGreaterThan(-1)
      const body = SCROLL_EDGE_CSS.slice(at + head.length, SCROLL_EDGE_CSS.indexOf('\n}', at))
      expect(body.trim()).toBe('[data-scroll-edges] > [data-scroll-edge] > [data-edge-blur] { display: none; }')
    }
  })
})

describe('the looks follow the Thread', () => {
  const tintRules = SCROLL_EDGE_CSS.split('\n').filter((l) => l.includes('.k2-edge-tint {'))

  it('Terminal: the kit\'s progressive blur, heavy at the bar, under the Thread header\'s scrim tint', () => {
    // The same numbers as the Thread's message-box strip, one source.
    expect(kit.FOOTER_BLUR_LAYERS).toBe(FOOTER_BLUR_LAYERS)
    FOOTER_BLUR_LAYERS.forEach(([px, from, to], i) => {
      for (const [edge, toward] of [['top', 'to top'], ['bottom', 'to bottom']] as const) {
        const rule = ruleFor(SCROLL_EDGE_CSS, `[data-scroll-edges] > [data-scroll-edge="${edge}"] > .k2-edge-blur-${i + 1} {`)
        expect(rule.startsWith('[data-scroll-edges]'), 'every Style but Glass').toBe(true)
        expect(rule).toContain(`backdrop-filter: blur(${px}px);`)
        expect(rule).toContain(`-webkit-mask-image: linear-gradient(${toward}, transparent ${from}%, black ${to}%);`)
      }
    })
    // The tint is the Thread header's no-blur scrim outside Glass (97 → 88 → clear).
    expect(stopsOf(globalsBody('.thread-float-scrim'))).toEqual(SCRIM_TINT_STOPS)
    for (const [edge, toward] of [['top', 'to bottom'], ['bottom', 'to top']] as const) {
      const rule = ruleFor(SCROLL_EDGE_CSS, `[data-scroll-edges] > [data-scroll-edge="${edge}"] > .k2-edge-tint {`)
      expect(rule.startsWith('[data-scroll-edges]')).toBe(true)
      expect(rule).toContain(`linear-gradient(${toward}, `)
      expect(stopsOf(rule)).toEqual(solidAtBar(SCRIM_TINT_STOPS))
    }
  })

  it("Glass: the phone's header stack (the Thread's top band), mirrored at the bottom", () => {
    expect(GLASS_BAND_PX).toBe(SCROLL_EDGE_PX)
    expect(globalsBody('html[data-style="glass"] .k2-header-blur:not([data-zen-root] *)')).toContain(`height: ${GLASS_BAND_PX}px;`)
    GLASS_BLUR_LAYERS.forEach(([px, solid, clear], i) => {
      // The same layer as the Thread's (globals.css .k2-header-blur-N).
      const thread = globalsBody(`.k2-header-blur-${i + 1}`)
      expect(thread).toContain(`backdrop-filter: blur(${px}px);`)
      expect(thread).toContain(`mask-image: linear-gradient(to bottom, #000 0%, #000 ${solid}%, transparent ${clear}%);`)
      for (const [edge, toward] of [['top', 'to bottom'], ['bottom', 'to top']] as const) {
        const rule = ruleFor(SCROLL_EDGE_CSS, `html[data-style="glass"] [data-scroll-edges] > [data-scroll-edge="${edge}"] > .k2-edge-blur-${i + 1} {`)
        expect(rule).toContain(`backdrop-filter: blur(${px}px);`)
        expect(rule).toContain(` mask-image: linear-gradient(${toward}, black 0%, black ${solid}%, transparent ${clear}%);`)
      }
    })
    expect(stopsOf(globalsBody('.k2-header-tint'))).toEqual(GLASS_TINT_STOPS)
    for (const edge of ['top', 'bottom']) {
      expect(stopsOf(ruleFor(SCROLL_EDGE_CSS, `html[data-style="glass"] [data-scroll-edges] > [data-scroll-edge="${edge}"] > .k2-edge-tint {`))).toEqual(solidAtBar(GLASS_TINT_STOPS))
    }
    // Glass's no-blur fallback (data-frost="tile"): no layers, the Thread's heavier ramp.
    expect(SCROLL_EDGE_CSS).toContain('html[data-style="glass"][data-frost="tile"] [data-scroll-edges] > [data-scroll-edge] > [data-edge-blur] { display: none; }')
    expect(stopsOf(globalsBody('html[data-style="glass"][data-frost="tile"] .k2-header-tint:not([data-zen-root] *)'))).toEqual(GLASS_TINT_STOPS_TILE)
    for (const edge of ['top', 'bottom']) {
      expect(stopsOf(ruleFor(SCROLL_EDGE_CSS, `html[data-style="glass"][data-frost="tile"] [data-scroll-edges] > [data-scroll-edge="${edge}"] > .k2-edge-tint {`))).toEqual(solidAtBar(GLASS_TINT_STOPS_TILE))
    }
  })

  it('every band is solid for its outer 4px, then runs the Thread\'s ramp: no haze line where it meets the bar', () => {
    expect(SCROLL_EDGE_LIP_PCT * SCROLL_EDGE_PX / 100).toBe(4)
    expect(solidAtBar(GLASS_TINT_STOPS)).toEqual([[0, 100], [10, 85], [50.5, 55], [100, 0]])
    expect(solidAtBar(SCRIM_TINT_STOPS)).toEqual([[0, 100], [10, 97], [59.5, 88], [100, 0]])
    for (const rule of tintRules) expect(rule).toMatch(/, color-mix\(in srgb, var\(--k2-edge-tint\) 100%, transparent\) 0%, /)
  })

  it("every band tints with the list's own colour, never black, in every palette and both schemes", () => {
    // Top and bottom, in Terminal, Glass, Glass's tile fallback and a
    // dialog's heavy band.
    expect(tintRules).toHaveLength(8)
    for (const rule of tintRules) {
      expect(rule).toContain(`color-mix(in srgb, var(${SCROLL_EDGE_TINT_VAR}) `)
      expect(rule).not.toMatch(/rgba\(0, 0, 0|#000\b|black/)
    }
    // One colour for every palette and scheme: the surface's own, never a palette's.
    expect(SCROLL_EDGE_CSS).not.toMatch(/data-palette|data-scheme/)
    // Unset, the page's own colour (never black).
    expect(ruleFor(SCROLL_EDGE_CSS, '[data-scroll-edges] {')).toContain(`${SCROLL_EDGE_TINT_VAR}: var(--color-bg);`)
    // No colour of its own anywhere in the sheet (a mask's `black` is alpha only).
    expect(SCROLL_EDGE_CSS).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i)
  })
})

describe('a heavy band (a list in a dialog)', () => {
  it('in Glass tints with the no-blur ramp under its blurs, solid at the bar', () => {
    for (const edge of ['top', 'bottom'] as const) {
      const rule = ruleFor(SCROLL_EDGE_CSS, `html[data-style="glass"] [data-scroll-edges][data-edge-heavy] > [data-scroll-edge="${edge}"] > .k2-edge-tint {`)
      expect(rule).toContain(edgeTintCss(edge, GLASS_TINT_STOPS_TILE).replace(/^background: /, '').replace(/;$/, ''))
    }
    // The blurs stay: only the tint is heavier.
    expect(SCROLL_EDGE_CSS).not.toMatch(/\[data-edge-heavy\][^{]*\[data-edge-blur\][^{]*\{ display: none/)
  })

  it('is marked on the frame only when asked for, with the dialog tint', () => {
    const { container, rerender } = render(<ScrollEdgeFrame tint={DIALOG_EDGE_TINT} heavy><div /></ScrollEdgeFrame>)
    const frame = container.querySelector('[data-scroll-edges]') as HTMLElement
    expect(frame.hasAttribute('data-edge-heavy')).toBe(true)
    expect(frame.style.getPropertyValue(SCROLL_EDGE_TINT_VAR)).toBe('var(--tile-fill, var(--color-bg-surface))')
    rerender(<ScrollEdgeFrame><div /></ScrollEdgeFrame>)
    expect((container.querySelector('[data-scroll-edges]') as HTMLElement).hasAttribute('data-edge-heavy')).toBe(false)
  })
})

describe('a band without blur (raised cards in a tile)', () => {
  it('drops only the blur layers, in every look, and keeps the tint', () => {
    expect(ruleFor(SCROLL_EDGE_CSS, '[data-scroll-edges][data-edge-noblur] > [data-scroll-edge] > [data-edge-blur] {')).toContain('display: none')
    expect(SCROLL_EDGE_CSS).not.toMatch(/\[data-edge-noblur\][^{]*\.k2-edge-tint[^{]*\{ display: none/)
  })

  it('is marked on the frame only when asked for', () => {
    const { container, rerender } = render(<ScrollEdgeFrame heavy blur={false}><div /></ScrollEdgeFrame>)
    expect((container.querySelector('[data-scroll-edges]') as HTMLElement).hasAttribute('data-edge-noblur')).toBe(true)
    rerender(<ScrollEdgeFrame heavy><div /></ScrollEdgeFrame>)
    expect((container.querySelector('[data-scroll-edges]') as HTMLElement).hasAttribute('data-edge-noblur')).toBe(false)
  })
})

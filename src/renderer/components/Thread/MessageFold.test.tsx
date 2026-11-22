// @vitest-environment jsdom
// "See more" on long messages (MessageFold, the phone's fold): the threshold
// by rendered height against the body's line-height (20 lines on the
// desktop), open and fold, a streaming message crossing the threshold
// without a jump, a search hit opening it, the state kept per message, and
// the bubble's top kept still.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  __resetFoldForTests,
  createFoldHost,
  foldClipLines,
  foldClipPx,
  foldable,
  FoldContext,
  insideFolded,
  MessageFold,
  readLinePx,
  revealFolded,
  THREAD_FOLD_LINES,
  type FoldHost,
} from './MessageFold'
import { THREAD_FOLD_CSS } from './messageFoldCss'

const LINE = 21
const CLIP = foldClipPx(LINE) // 420

// ── ResizeObserver: the test reports heights by hand.
type Obs = { cb: (entries?: ResizeObserverEntry[]) => void; targets: Element[] }
let observers: Obs[] = []
function report(target: Element, height: number): void {
  const o = observers.find((x) => x.targets.includes(target))
  if (!o) throw new Error('not observed')
  act(() => o.cb([{ target, contentRect: { height } } as unknown as ResizeObserverEntry]))
}

const realGcs = window.getComputedStyle.bind(window)
let scrollerPadTop = '0px'

beforeEach(() => {
  __resetFoldForTests()
  observers = []
  vi.stubGlobal(
    'ResizeObserver',
    class {
      o: Obs
      constructor(cb: Obs['cb']) {
        this.o = { cb, targets: [] }
        observers.push(this.o)
      }
      observe(t: Element): void {
        this.o.targets.push(t)
      }
      unobserve(): void {}
      disconnect(): void {
        this.o.targets = []
      }
    },
  )
  // The body's computed line-height (WebKit resolves `1.6` to px).
  vi.spyOn(window, 'getComputedStyle').mockImplementation((el: Element) => {
    const cs = realGcs(el)
    const over: Record<string, string> = el.classList?.contains('chat-markdown')
      ? { lineHeight: `${LINE}px`, fontSize: '13px' }
      : el.getAttribute?.('data-testid') === 'scroller'
        ? { scrollPaddingTop: scrollerPadTop, overflowY: 'auto' }
        : {}
    return new Proxy(cs, {
      get(t, k: string) {
        if (k in over) return over[k]
        const v = Reflect.get(t, k)
        return typeof v === 'function' ? v.bind(t) : v
      },
    })
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

function Body({ text }: { text: string }): React.JSX.Element {
  return (
    <div className="chat-markdown" data-testid="body">
      {text}
    </div>
  )
}

function host(over: Partial<FoldHost> = {}): FoldHost {
  return {
    ...createFoldHost({ scroller: () => document.querySelector<HTMLElement>('[data-testid=scroller]'), unfollow: vi.fn() }),
    ...over,
  }
}

type Opts = { text?: string; foldKey?: string; reveal?: boolean }
function renderFold(h: FoldHost, opts: Opts = {}) {
  const ui = (o: Opts) => (
    <FoldContext.Provider value={h}>
      <div data-testid="scroller">
        <div data-zen-message="" data-testid="row">
          <MessageFold foldKey={o.foldKey ?? 'm1'} reveal={o.reveal}>
            <Body text={o.text ?? 'long words'} />
          </MessageFold>
        </div>
      </div>
    </FoldContext.Provider>
  )
  const r = render(ui(opts))
  return { ...r, rerenderWith: (o: Opts) => r.rerender(ui({ ...opts, ...o })) }
}

const inner = () => screen.getByTestId('fold-inner')
const clip = () => screen.getByTestId('fold-clip')
const toggle = () => screen.queryByTestId('fold-toggle')

describe('the threshold', () => {
  it('folds only past 20 lines by height: 19 shown, the 20th row holds the fade and the pill', () => {
    expect(THREAD_FOLD_LINES).toBe(19)
    expect(foldClipLines()).toBe(20)
    expect(foldable(LINE * 19, LINE)).toBe(false)
    expect(foldable(LINE * 20, LINE)).toBe(false)
    expect(foldable(LINE * 20 + 1, LINE)).toBe(true)
    expect(foldable(LINE * 21, LINE)).toBe(true)
    expect(foldable(LINE * 500, LINE)).toBe(true)
    // No line-height yet: never folds.
    expect(foldable(10_000, 0)).toBe(false)
    // Another surface's count (the phone's 14 of 15).
    expect(foldable(LINE * 15, LINE, 14)).toBe(false)
    expect(foldable(LINE * 16, LINE, 14)).toBe(true)
  })

  it('reads the computed line-height: px, unitless × font-size, normal ≈ 1.2em', () => {
    const el = document.createElement('div')
    document.body.appendChild(el)
    vi.mocked(window.getComputedStyle).mockImplementation(() => ({ lineHeight: '22.5px', fontSize: '15px' }) as CSSStyleDeclaration)
    expect(readLinePx(el)).toBe(22.5)
    vi.mocked(window.getComputedStyle).mockImplementation(() => ({ lineHeight: '1.5', fontSize: '14px' }) as CSSStyleDeclaration)
    expect(readLinePx(el)).toBe(21)
    vi.mocked(window.getComputedStyle).mockImplementation(() => ({ lineHeight: 'normal', fontSize: '10px' }) as CSSStyleDeclaration)
    expect(readLinePx(el)).toBe(12)
    el.remove()
  })

  it('the clip is on from the first paint (no layout change when measured); a short message shows no pill', () => {
    renderFold(host())
    // Before any measurement: the CSS estimate in the body's em.
    expect(clip().style.maxHeight).toBe(`${foldClipLines() * 1.6}em`)
    report(inner(), LINE * 4)
    expect(clip().style.maxHeight).toBe(`${CLIP}px`)
    expect(toggle()).toBeNull()
    expect(screen.queryByTestId('fold-fade')).toBeNull()
  })

  it('19 lines never fold; 21 lines do', () => {
    renderFold(host())
    report(inner(), LINE * 19)
    expect(toggle()).toBeNull()
    report(inner(), LINE * 21)
    expect(toggle()!.textContent).toBe('See more')
  })

  it('a long message: the fade and a real See more button, aria-expanded false, controlling the clip', () => {
    renderFold(host())
    report(inner(), LINE * 40)
    const b = toggle()!
    expect(b.tagName).toBe('BUTTON')
    expect(b.getAttribute('type')).toBe('button')
    expect(b.textContent).toBe('See more')
    expect(b.getAttribute('aria-expanded')).toBe('false')
    expect(b.getAttribute('aria-controls')).toBe(clip().id)
    expect(screen.getByTestId('fold-fade').getAttribute('aria-hidden')).toBe('true')
    expect(clip().style.maxHeight).toBe(`${CLIP}px`)
  })

  it('a host can set another count', () => {
    renderFold(host({ lines: 14 }))
    report(inner(), LINE * 16)
    expect(clip().style.maxHeight).toBe(`${foldClipPx(LINE, 14)}px`)
    expect(toggle()).not.toBeNull()
  })
})

describe('the pill’s caret', () => {
  it('right of the words: down on See more, up on See less; hidden, so the name stays the words', () => {
    renderFold(host())
    report(inner(), LINE * 40)
    const chevron = () => toggle()!.querySelector('svg')!
    expect(toggle()!.lastElementChild).toBe(chevron())
    expect(toggle()!.firstChild!.nodeType).toBe(Node.TEXT_NODE)
    expect(chevron().getAttribute('aria-hidden')).toBe('true')
    expect(chevron().getAttribute('stroke')).toBe('currentColor')
    expect(chevron().getAttribute('class')).toBe('thread-fold-chevron')
    expect(chevron().getAttribute('data-dir')).toBe('down')
    expect(screen.getByRole('button', { name: 'See more' })).toBe(toggle())
    fireEvent.click(toggle()!)
    expect(chevron().getAttribute('data-dir')).toBe('up')
    expect(screen.getByRole('button', { name: 'See less' })).toBe(toggle())
    expect(toggle()!.textContent).toBe('See less')
  })

  it('its rule sizes it at 0.85em of the pill’s type, with a little air after the words, and no motion', () => {
    const at = THREAD_FOLD_CSS.indexOf('.thread-fold-chevron {')
    expect(at).toBeGreaterThan(-1)
    const rule = THREAD_FOLD_CSS.slice(at, THREAD_FOLD_CSS.indexOf('}', at))
    expect(rule).toContain('width: 0.85em;')
    expect(rule).toContain('height: 0.85em;')
    expect(rule).toContain('margin-left: 0.3em;')
    expect(rule).toContain('flex: none;')
    expect(THREAD_FOLD_CSS).not.toMatch(/transition|animation/)
  })
})

describe('the look reads the tokens', () => {
  it('the fade is the bubble’s own fill, on yours and the agent’s', () => {
    expect(THREAD_FOLD_CSS).toMatch(/\[data-zen-bubble="me"\] \.thread-fold \{ --thread-fold-fill: var\(--zen-bubble-me\);/)
    expect(THREAD_FOLD_CSS).toMatch(/\[data-zen-bubble="agent"\] \.thread-fold \{ --thread-fold-fill: var\(--zen-bubble-agent\);/)
    expect(THREAD_FOLD_CSS).toContain('linear-gradient(to bottom, transparent, var(--thread-fold-fill) 62%)')
  })

  it('no raw colours; clipped, never scrollable into view', () => {
    expect(THREAD_FOLD_CSS).not.toMatch(/#[0-9a-f]{3,6}\b|rgba?\(/i)
    expect(THREAD_FOLD_CSS).toContain('overflow: clip;')
  })

  it('Terminal: chamfered at the chip cut with an edge; focus thickens the cut edge', () => {
    expect(THREAD_FOLD_CSS).toContain('--ch-tl: var(--chamfer-chip);')
    expect(THREAD_FOLD_CSS).toContain('--ch-edge: var(--thread-fold-edge); --ch-bw: 1px;')
    expect(THREAD_FOLD_CSS).toMatch(/html\[data-style="terminal"\][^{]*\.thread-fold-toggle\[data-chamfer\]:focus-visible \{ outline: none; --ch-edge: var\(--thread-fold-ring\) !important; --ch-bw: 2px !important; \}/)
  })

  it('Glass: the accent ring on focus; the light palette gets the strong edge, the dark ones the soft one', () => {
    expect(THREAD_FOLD_CSS).toContain('.thread-fold-toggle:focus-visible { outline: 2px solid var(--thread-fold-ring);')
    expect(THREAD_FOLD_CSS).toMatch(/\[data-palette="veil"\][^{]*\.thread-fold-toggle \{ box-shadow: inset 0 0 0 1px var\(--thread-fold-edge\); \}/)
    expect(THREAD_FOLD_CSS).toMatch(/html\[data-style="glass"\] \[data-k2-thread-zen\] \.thread-fold-toggle \{ box-shadow: inset 0 0 0 0\.5px var\(--material-edge\); \}/)
  })
})

describe('open and fold', () => {
  it('See more opens in place (all the words, See less); See less folds again', () => {
    const h = host()
    renderFold(h)
    report(inner(), LINE * 40)
    fireEvent.click(toggle()!)
    expect(toggle()!.textContent).toBe('See less')
    expect(toggle()!.getAttribute('aria-expanded')).toBe('true')
    expect(clip().style.maxHeight).toBe('')
    expect(screen.queryByTestId('fold-fade')).toBeNull()
    expect(h.open.has('m1')).toBe(true)
    // Opening near the newest must not stick the list under the reader.
    expect(h.unfollow).toHaveBeenCalled()
    fireEvent.click(toggle()!)
    expect(toggle()!.textContent).toBe('See more')
    expect(clip().style.maxHeight).toBe(`${CLIP}px`)
    expect(h.open.has('m1')).toBe(false)
  })

  it('an open message stays open when its row is drawn again (the render window), per message', () => {
    const h = host()
    const r = renderFold(h)
    report(inner(), LINE * 40)
    fireEvent.click(toggle()!)
    r.unmount()
    renderFold(h)
    // Open from the first paint, before any measurement: no jump.
    expect(clip().style.maxHeight).toBe('')
    expect(toggle()!.textContent).toBe('See less')
    cleanup()
    renderFold(h, { foldKey: 'm2' })
    expect(clip().style.maxHeight).not.toBe('')
  })

  it('a streaming message folds once it passes the threshold, the clip never changing height', () => {
    const { rerenderWith } = renderFold(host(), { text: 'a' })
    report(inner(), LINE * 3)
    const heights: string[] = [clip().style.maxHeight]
    for (const lines of [6, 10, 15, 19, 20, 21, 60]) {
      rerenderWith({ text: 'a '.repeat(lines * 20) })
      report(inner(), LINE * lines)
      heights.push(clip().style.maxHeight)
      expect(toggle() != null).toBe(lines > foldClipLines())
    }
    // One height throughout: what shows grows to the clip, then stops.
    expect(new Set(heights)).toEqual(new Set([`${CLIP}px`]))
    expect(toggle()!.textContent).toBe('See more')
  })
})

describe('a search hit in the hidden part', () => {
  it('revealFolded on a node inside opens it', () => {
    renderFold(host())
    report(inner(), LINE * 40)
    expect(insideFolded(screen.getByTestId('body'))).toBe(true)
    act(() => revealFolded(screen.getByTestId('body')))
    expect(toggle()!.getAttribute('aria-expanded')).toBe('true')
    expect(insideFolded(screen.getByTestId('body'))).toBe(false)
  })

  it('a short message ignores it', () => {
    renderFold(host())
    report(inner(), LINE * 4)
    expect(insideFolded(screen.getByTestId('body'))).toBe(false)
    act(() => revealFolded(screen.getByTestId('body')))
    expect(toggle()).toBeNull()
    expect(clip().style.maxHeight).toBe(`${CLIP}px`)
  })

  it('landing on it (reveal) opens it once; See less while still landed stays folded', () => {
    const { rerenderWith } = renderFold(host(), { reveal: false })
    report(inner(), LINE * 40)
    rerenderWith({ reveal: true })
    expect(toggle()!.textContent).toBe('See less')
    fireEvent.click(toggle()!)
    expect(toggle()!.textContent).toBe('See more')
    rerenderWith({ reveal: true })
    expect(toggle()!.textContent).toBe('See more')
  })

  it('landing before it was measured opens it once measured', () => {
    renderFold(host(), { reveal: true })
    expect(toggle()).toBeNull()
    report(inner(), LINE * 40)
    expect(toggle()!.textContent).toBe('See less')
  })
})

describe('the bubble’s top stays put', () => {
  // The scroller sits at y=0; the row's top in the content is `offset`.
  let scrollTop = 0
  let offset = 500
  function wire(): void {
    const sc = screen.getByTestId('scroller')
    Object.defineProperty(sc, 'scrollTop', {
      configurable: true,
      get: () => scrollTop,
      set: (v: number) => {
        scrollTop = v
      },
    })
    sc.getBoundingClientRect = () => ({ top: 0, bottom: 600, left: 0, right: 400, height: 600, width: 400 }) as DOMRect
    screen.getByTestId('row').getBoundingClientRect = () => {
      const top = offset - scrollTop
      return { top, bottom: top + 100, left: 0, right: 300, height: 100, width: 300 } as DOMRect
    }
  }
  beforeEach(() => {
    scrollTop = 0
    offset = 500
    scrollerPadTop = '0px'
  })

  it('opening: content grows below, nothing above moves, scrollTop untouched', () => {
    const h = host({ ownScroll: vi.fn() })
    renderFold(h)
    wire()
    scrollTop = 200
    report(inner(), LINE * 40)
    fireEvent.click(toggle()!)
    expect(scrollTop).toBe(200)
    expect(h.ownScroll).not.toHaveBeenCalled()
  })

  it('opening where the layout would carry the top away (a bottom-aligned list): corrected before paint', () => {
    const h = host({ ownScroll: vi.fn() })
    renderFold(h)
    wire()
    scrollTop = 200
    report(inner(), LINE * 40)
    const row = screen.getByTestId('row')
    const before = row.getBoundingClientRect().top
    // Once open, the layout puts the row 300px higher in the content.
    row.getBoundingClientRect = () => {
      const open = clip().style.maxHeight === ''
      const top = (open ? offset - 300 : offset) - scrollTop
      return { top, bottom: top + 100, left: 0, right: 300, height: 100, width: 300 } as DOMRect
    }
    fireEvent.click(toggle()!)
    expect(row.getBoundingClientRect().top).toBe(before)
    expect(scrollTop).toBe(-100)
    expect(h.ownScroll).toHaveBeenCalled()
  })

  it('folding with the top in view keeps it where it was', () => {
    const h = host({ refollow: vi.fn() })
    renderFold(h)
    wire()
    report(inner(), LINE * 40)
    fireEvent.click(toggle()!)
    scrollTop = 300 // row top at 200: in view
    fireEvent.click(toggle()!)
    expect(scrollTop).toBe(300)
    expect(h.refollow).toHaveBeenCalled()
  })

  it('folding from deep inside a long message brings its top back under the floating header', () => {
    scrollerPadTop = '60px'
    renderFold(host())
    wire()
    report(inner(), LINE * 400)
    fireEvent.click(toggle()!)
    scrollTop = 4000 // row top far above the view
    fireEvent.click(toggle()!)
    // Top at the view's top (under the 60px header) plus 8px of air.
    expect(screen.getByTestId('row').getBoundingClientRect().top).toBe(68)
  })
})

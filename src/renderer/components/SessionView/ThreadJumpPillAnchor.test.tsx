// @vitest-environment jsdom
// Where the Jump to bottom pill floats (Rosson 2026-11-14, "match mobile"):
// THREAD_JUMP_PILL_GAP above the activity bar while it stands on the
// message box, down to the same gap above the box when the bar slides out
// (starting with the slide, on its curve), and up with the box as a draft
// grows. The column measures (threadDock) and the pill's CSS reads it.
// Layout is stubbed: a 900px-tall column, the bar's slot on its foot.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen } from '@testing-library/react'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'
import { coerceThreadTurn, type ThreadTurn } from './overlayThread'
import { measureThreadDock, writeThreadDock } from './threadDock'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { ThreadComposeBox } from '@/components/Thread/ThreadZenKit'
import {
  THREAD_DOCK_ABOVE_VAR,
  THREAD_DOCK_BOX_VAR,
  THREAD_DOCK_EASE_VAR,
  THREAD_JUMP_PILL_CSS,
  THREAD_JUMP_PILL_GAP,
} from '@/components/Thread/ThreadJumpPill'
import { ZEN_DRAWER_EASE_IN, ZEN_DRAWER_EASE_OUT, ZEN_DRAWER_MS, ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'

const room = testRoom({ tabs: {}, scope: primaryScope() })

const threadHook = vi.hoisted(() => ({
  items: [] as unknown[],
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loaded: true,
  outbox: [] as unknown[],
  loadOlder: async () => {},
}))
vi.mock('./useOverlayThread', () => ({ useOverlayThread: () => threadHook }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

const T0 = Date.now()
function working(): ThreadTurn {
  const t = coerceThreadTurn(
    {
      state: 'working',
      phase: 'tool',
      line: 'Running `npm test`',
      subagents: 0,
      background: 0,
      tally: { read: 1, cmd: 1 },
      end: null,
      rev: 1,
      serverNow: T0,
      turnId: 'anchor-1',
      startedAt: T0 - 8_000,
      phaseSince: T0 - 3_000,
    },
    T0,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

// The stubbed layout: the bar's slot sits on the column's foot (y = 900),
// 8px padding over what is topmost, the box 12px off the foot.
const FOOT = 900
const geo = { box: 40, bar: 32 }
function boxTop(): number {
  return FOOT - 12 - geo.box
}
function rectOf(node: Element): DOMRect | null {
  const el = node as HTMLElement
  if (el.matches('[data-zen-compose-box]')) return new DOMRect(0, boxTop(), 600, geo.box)
  // On the box's top edge, overlapping its border by 1px.
  if (el.matches('[data-zen-drawer-clip]')) return new DOMRect(0, boxTop() - geo.bar + 1, 600, geo.bar)
  if (el.matches('[data-testid="agent-session-thread-compose-slot"]')) {
    const clip = el.querySelector('[data-zen-drawer-clip]')
    const top = (clip ? boxTop() - geo.bar + 1 : boxTop()) - 8
    return new DOMRect(0, top, 600, FOOT - top)
  }
  return null
}

const observers: Array<() => void> = []
let restoreRect: (() => void) | null = null
beforeEach(() => {
  geo.box = 40
  geo.bar = 32
  observers.length = 0
  const orig = Element.prototype.getBoundingClientRect
  Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
    return rectOf(this) ?? orig.call(this)
  }
  restoreRect = () => {
    Element.prototype.getBoundingClientRect = orig
  }
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private readonly cb: ResizeObserverCallback) {
        observers.push(() => this.cb([], this as unknown as ResizeObserver))
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
})
afterEach(() => {
  cleanup()
  restoreRect?.()
  vi.unstubAllGlobals()
})

/** The message bar as TerminalComposeBar draws it: the activity bar on the
 *  box's top edge. */
function Bar({ turn }: { turn: ThreadTurn | null }) {
  return (
    <div style={{ padding: '8px 12px 12px' }}>
      <ThreadComposeBox drawer={<div data-zen-stash-row=""><ZenActivityDrawer turn={turn} margin="0" /></div>}>
        <textarea aria-label="Message" />
      </ThreadComposeBox>
    </div>
  )
}

function column(turn: ThreadTurn | null) {
  return <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={<Bar turn={turn} />} />
}

function root(): HTMLElement {
  return screen.getByTestId('agent-session-thread')
}
function dock(): { box: string; above: string; ease: string } {
  const st = root().style
  return {
    box: st.getPropertyValue(THREAD_DOCK_BOX_VAR),
    above: st.getPropertyValue(THREAD_DOCK_ABOVE_VAR),
    ease: st.getPropertyValue(THREAD_DOCK_EASE_VAR),
  }
}
function resize(): void {
  act(() => {
    for (const fire of observers) fire()
  })
}

describe('Jump to bottom pill: where it floats', () => {
  it('bar open: the dock reaches the activity bar’s top', () => {
    renderInRoom(room, column(working()))
    resize()
    // Box top 52px off the foot; the bar stands 31px on it (1px under the box's edge).
    expect(dock()).toMatchObject({ box: '52px', above: '31px' })
  })

  it('bar collapsed: the pill starts down as the bar starts to slide out, on its curve', async () => {
    const view = renderInRoom(room, column(working()))
    resize()
    expect(dock().above).toBe('31px')
    // The bar slides out but keeps its room until the slide ends: the
    // pill doesn't wait for that.
    await act(async () => {
      view.rerender(column(null))
    })
    expect(document.querySelector('[data-zen-drawer]')?.getAttribute('data-phase')).toBe('out')
    expect(dock()).toEqual({ box: '52px', above: '0px', ease: ZEN_DRAWER_EASE_OUT })
    // Back again: up on the slide-in's curve.
    await act(async () => {
      view.rerender(column(working()))
    })
    resize()
    expect(dock()).toEqual({ box: '52px', above: '31px', ease: ZEN_DRAWER_EASE_IN })
  })

  it('no activity bar: the dock is the box itself', () => {
    renderInRoom(room, column(null))
    resize()
    expect(dock()).toMatchObject({ box: '52px', above: '0px' })
  })

  it('a tall draft: the box grows and the pill rises with it, the bar still under it', () => {
    renderInRoom(room, column(working()))
    resize()
    geo.box = 140
    resize()
    expect(dock()).toMatchObject({ box: '152px', above: '31px' })
    // The bar's line grows (expanded): above it, on the slide-in's curve.
    geo.bar = 70
    resize()
    expect(dock()).toEqual({ box: '152px', above: '69px', ease: ZEN_DRAWER_EASE_IN })
  })

  it('no message bar: the column sets nothing (the pill sits on the list’s foot)', () => {
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={null} />)
    expect(dock()).toEqual({ box: '', above: '', ease: '' })
  })
})

describe('threadDock', () => {
  function slot(html: string): HTMLElement {
    const el = document.createElement('div')
    el.setAttribute('data-testid', 'agent-session-thread-compose-slot')
    el.innerHTML = html
    document.body.appendChild(el)
    return el
  }

  it('chips over a leaving bar: counted where they will land', () => {
    const el = slot(
      '<div data-zen-attachments=""></div><div data-zen-drawer-clip=""><div data-zen-drawer="" data-phase="out"></div></div><div data-zen-compose-box=""></div>',
    )
    const orig = Element.prototype.getBoundingClientRect
    const at: Record<string, DOMRect> = {
      'data-zen-attachments': new DOMRect(0, 780, 600, 40),
      'data-zen-drawer-clip': new DOMRect(0, 820, 600, 32),
      'data-zen-compose-box': new DOMRect(0, 851, 600, 37),
    }
    Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
      for (const [attr, r] of Object.entries(at)) if ((this as HTMLElement).hasAttribute(attr)) return r
      if (this === el) return new DOMRect(0, 772, 600, 128)
      return orig.call(this)
    }
    try {
      // The chips' top drops by the bar's 32px: 851 - (780 + 32).
      expect(measureThreadDock(el)).toEqual({ box: 49, above: 39 })
    } finally {
      Element.prototype.getBoundingClientRect = orig
      el.remove()
    }
  })

  it('a note over the box counts; a menu floating over the bar doesn’t', () => {
    const el = slot(
      '<div data-compose-bar=""><div data-note="">Waiting to send…</div><div data-menu="" style="position: absolute"></div><div data-zen-compose-box=""></div></div>',
    )
    const orig = Element.prototype.getBoundingClientRect
    const at: Record<string, DOMRect> = {
      'data-note': new DOMRect(0, 820, 600, 18),
      'data-menu': new DOMRect(0, 500, 600, 300),
      'data-zen-compose-box': new DOMRect(0, 844, 600, 44),
    }
    Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
      for (const [attr, r] of Object.entries(at)) if ((this as HTMLElement).hasAttribute(attr)) return r
      if (this === el) return new DOMRect(0, 812, 600, 88)
      return orig.call(this)
    }
    try {
      expect(measureThreadDock(el)).toEqual({ box: 56, above: 24 })
    } finally {
      Element.prototype.getBoundingClientRect = orig
      el.remove()
    }
  })

  it('the first write sets no curve; null clears', () => {
    const el = document.createElement('div')
    writeThreadDock(el, { box: 52, above: 0 })
    expect(el.style.getPropertyValue(THREAD_DOCK_EASE_VAR)).toBe('')
    writeThreadDock(el, { box: 52, above: 31 })
    expect(el.style.getPropertyValue(THREAD_DOCK_EASE_VAR)).toBe(ZEN_DRAWER_EASE_IN)
    writeThreadDock(el, null)
    expect(el.getAttribute('style') ?? '').toBe('')
  })

  it('the pill reads the dock: the gap over the box, the bar’s part on the bar’s slide', () => {
    expect(THREAD_JUMP_PILL_GAP).toBe(8)
    const css = THREAD_JUMP_PILL_CSS.replace(/\s+/g, ' ')
    expect(css).toContain(`bottom: calc(var(${THREAD_DOCK_BOX_VAR}, var(--thread-jump-base, 0px)) + 8px)`)
    expect(css).toContain(`translate: 0 calc(-1 * var(${THREAD_DOCK_ABOVE_VAR}, 0px))`)
    expect(css).toContain(`translate ${ZEN_DRAWER_MS}ms var(${THREAD_DOCK_EASE_VAR}, ${ZEN_DRAWER_EASE_IN})`)
    // Reduced motion: no transition at all (the pill jumps with the bar's fade).
    expect(css).toMatch(/prefers-reduced-motion: reduce\) \{[^}]*transition: none !important/)
  })
})

// @vitest-environment jsdom
//
// prd-garden-column-collapse-and-agent-list-v1 A1 (CA3, CA10, CA15) —
// collapsible Garden columns through the real ZenPage: K2's chevron folds a
// column to an edge tab on its outer side; the tab is one button with
// `aria-expanded`, `aria-controls`, the label and the unread / waiting
// counts of the widgets inside; folding moves the focus to the tab and the
// person's unfold back into the column; the person's click is saved on this
// computer per Garden and a second window follows; a widget's fold (the
// page's controller, reached through `k2.layout.*` only just after a press
// in the widget) is saved the same way and comes back after a reload
// (0.46.4); the last open column can't fold; the
// narrow window folds right to left and isn't saved; single-view pages never
// fold; widgets stay mounted while folded. Only the daemon and Tauri are
// faked. Fixtures are made up (g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', startDragging: async () => undefined, listen: async () => () => undefined }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act, useContext, useState } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests } from '@/lib/zen/zen-monitor'
import { registerZenVerb } from '@/lib/zen/zen-bridge'
import { parseZenGet, type ZenResolvedPage } from '@/lib/zen/zen-page'
import {
  createZenColumnsStore,
  useZenColumnsStore,
  ZenColumnsContext,
  ZEN_COLUMNS_KEY,
  type ZenColumnsController,
  type ZenLayoutColumn,
} from '@/lib/zen/zen-columns'
import { createZenCustomLayer, type ZenCustomLayerDeps } from '@/lib/zen/zen-custom-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZEN_COLUMN_TAB_PX } from '@/lib/zen/zen-overflow'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import { registerZenWidget, type ZenWidgetProps } from './zen-registry'
import { ZenPage } from './ZenPage'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const G = 'g-test0001'

/** The page's controller, as a custom frame on it would get it. */
let controller: ZenColumnsController | null = null
const mounts: Record<string, number> = {}

function StubWidget({ decl }: ZenWidgetProps): React.JSX.Element {
  controller = useContext(ZenColumnsContext)
  // Counts mounts: a fold must never remount a column's widgets.
  const [n] = useState(() => {
    mounts[decl.id] = (mounts[decl.id] ?? 0) + 1
    return mounts[decl.id]
  })
  return (
    <div data-test-widget={decl.id} data-mount={n}>
      <button type="button" data-test-first={decl.id}>
        {decl.id}
      </button>
    </div>
  )
}

function row(address: string, over: Record<string, unknown> = {}): ZenAgentRow {
  return { address, selected: false, unread: null, ...over } as unknown as ZenAgentRow
}

const ROWS: ZenAgentRow[] = [
  row('alice::local', { selected: true, unread: { unread: 2, waiting: 1, muted: false } }),
  row('bob::local', { unread: { unread: 5, waiting: 0, muted: true } }),
]

const CHROME = {
  items: [
    { id: 'garden-switcher', kind: 'garden-switcher', slot: 'top', align: 'start', props: {}, caps: ['gardens:manage'] },
    { id: 'zen-toggle', kind: 'zen-toggle', slot: 'top', align: 'end', props: {}, caps: [] },
  ],
  bands: { top: { start: ['garden-switcher'], center: [], end: ['zen-toggle'] }, bottom: null },
}

function page(opts: {
  columns: Array<Record<string, unknown>>
  full?: boolean
  rail?: boolean
  mins?: number[]
}): ZenResolvedPage {
  const split = opts.columns.map((c) => c.size as number)
  const mins = opts.mins ?? split.map(() => 0)
  return parseZenGet({
    ok: true,
    schema: 1,
    version: 'v1',
    garden: { id: G, name: 'Board', index: 1 },
    page: {
      template: 'k2.blank@1',
      layout: {
        kind: 'columns',
        split,
        minWidths: mins,
        columns: opts.columns.map((c, i) => ({ 'min-width': mins[i], ...c })),
        ...(opts.full ? { canvas: 'full' } : {}),
      },
      widgets: [
        { id: 'people', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
        { id: 'chat', kind: 'conversation', column: split.length - 1, props: { agents: 'people' }, caps: ['agents:read'], source: 'builtin' },
        ...(opts.rail ? [{ id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: ['app:navigate'], source: 'builtin' }] : []),
      ],
      controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
      chrome: { from: 'garden', items: CHROME.items },
      bands: CHROME.bands,
      edges: [],
      menus: {},
    },
    theme: { name: 'basic' },
    themes: [],
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  })
}

const TWO = [{ size: 60 }, { size: 40, collapsible: true, label: 'Chat' }]

function show(p: ZenResolvedPage): ReturnType<typeof render> {
  let r: ReturnType<typeof render> | null = null
  act(() => {
    r = render(
      <div data-zen-root="">
        <ZenPage page={p} safe={false} banner={null} onControlFailure={() => undefined} />
      </div>,
    )
  })
  return r!
}

function q(sel: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(sel)
  if (!el) throw new Error(`missing ${sel}\n${document.body.innerHTML.slice(0, 3000)}`)
  return el
}

let pushRows: Array<(rows: ZenAgentRow[]) => void> = []
let unregister: Array<() => void> = []

beforeEach(() => {
  try {
    localStorage.clear()
  } catch {
    // jsdom always has it; the store itself falls back to memory.
  }
  useZenColumnsStore.setState({ folds: {} })
  pushRows = []
  controller = null
  for (const k of Object.keys(mounts)) delete mounts[k]
  unregister = [
    registerZenWidget('agents', StubWidget),
    registerZenWidget('conversation', StubWidget),
    registerZenVerb('agents.list', () => ROWS),
    registerZenVerb('agents.subscribe', (_ctx, cb) => {
      const fn = cb as (rows: ZenAgentRow[]) => void
      pushRows.push(fn)
      return () => {
        pushRows = pushRows.filter((f) => f !== fn)
      }
    }),
  ]
  useZenGardensStore.setState({ gardens: [{ id: G, name: 'Board', template: 'k2.blank@1', index: 1 } as never] })
  useZenWindowStore.setState({ on: true, garden: G, view: 'home' })
  __setZenGeometryForTests({
    rect: () => ({ left: 120, top: 60, width: 80, height: 30 }),
    style: () => ({ display: 'block', visibility: 'visible', opacity: 1 }),
    viewport: () => ({ width: 1200, height: 800 }),
    elementFromPoint: () => null,
  })
})

afterEach(() => {
  cleanup()
  for (const u of unregister) u()
  __setZenGeometryForTests(null)
  vi.unstubAllGlobals()
})

describe('collapsible columns on the page (A1)', () => {
  it('a fixed page draws no chevron or tab, and its columns look as before', () => {
    show(page({ columns: [{ size: 60 }, { size: 40 }] }))
    expect(document.querySelector('[data-zen-column-chevron]')).toBeNull()
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(document.querySelector('[data-zen-column-collapsible]')).toBeNull()
    expect(document.querySelector('[data-zen-column-announce]')).toBeNull()
  })

  it('the chevron folds the column to a tab on its outer side; the focus moves to the tab; the click is saved', () => {
    show(page({ columns: TWO }))
    const chevron = q('[data-zen-column-chevron="1"]')
    expect(chevron.tagName).toBe('BUTTON')
    expect(chevron.getAttribute('type')).toBe('button')
    expect(chevron.getAttribute('aria-expanded')).toBe('true')
    expect(chevron.getAttribute('aria-label')).toBe('Collapse Chat')
    const region = q('[data-zen-column-region="1"]')
    expect(chevron.getAttribute('aria-controls')).toBe(region.id)
    expect(document.querySelector('[data-zen-column-chevron="0"]'), 'column 0 is fixed').toBeNull()
    const widget = q('[data-test-widget="chat"]')

    act(() => {
      chevron.focus()
      fireEvent.click(chevron)
    })
    const tab = q('[data-zen-column-tab="1"]')
    expect(tab.tagName).toBe('BUTTON')
    expect(tab.getAttribute('type')).toBe('button')
    expect(tab.getAttribute('data-zen-column-side')).toBe('right')
    expect(tab.getAttribute('aria-expanded')).toBe('false')
    expect(tab.getAttribute('aria-controls')).toBe(region.id)
    expect(tab.style.width).toBe(`${ZEN_COLUMN_TAB_PX}px`)
    expect(document.activeElement).toBe(tab)
    // The label and the badge: Alice's 2 unread and 1 waiting (Bob isn't
    // the Conversation's row; muted unread wouldn't count anyway).
    expect(q('[data-zen-column-tab-label]').textContent).toBe('Chat')
    expect(tab.getAttribute('aria-label')).toBe('Expand Chat, 1 waiting on you, 2 unread')
    q('[data-zen-column-tab-badge] [data-zen-unread-waiting="1"]')
    q('[data-zen-column-tab-badge] [data-zen-unread-count="2"]')
    // Hidden, never unmounted.
    expect(region.style.display).toBe('none')
    expect(widget.isConnected).toBe(true)
    expect(mounts.chat).toBe(1)
    const slot = q('[data-zen-column-slot="1"]')
    expect(slot.getAttribute('data-zen-column-folded')).toBe('')
    expect(slot.style.width).toBe(`${ZEN_COLUMN_TAB_PX}px`)
    // Saved for this Garden and column count, and announced.
    expect(JSON.parse(localStorage.getItem(ZEN_COLUMNS_KEY) ?? 'null')).toEqual({ version: 1, folds: { [`${G}/2/1`]: true } })
    expect(q('[data-zen-column-announce]').textContent).toBe('Chat collapsed')
    // No chord: the chevron and tab are the only way (CA3).
    expect(tab.getAttribute('aria-keyshortcuts')).toBeNull()
  })

  it('the badge follows the rows live', () => {
    useZenColumnsStore.getState().save(`${G}/2/1`, true)
    show(page({ columns: TWO }))
    const tab = q('[data-zen-column-tab="1"]')
    expect(tab.getAttribute('aria-label')).toBe('Expand Chat, 1 waiting on you, 2 unread')
    act(() => {
      for (const p of pushRows) p([row('alice::local', { selected: true, unread: null })])
    })
    expect(tab.getAttribute('aria-label')).toBe('Expand Chat')
    expect(document.querySelector('[data-zen-column-tab-badge]')).toBeNull()
  })

  it('a click anywhere on the tab unfolds; the focus goes to the column’s first control; that is saved too', () => {
    useZenColumnsStore.getState().save(`${G}/2/1`, true)
    show(page({ columns: TWO }))
    const tab = q('[data-zen-column-tab="1"]')
    act(() => {
      tab.focus()
      fireEvent.click(q('[data-zen-column-tab-label]'))
    })
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(q('[data-zen-column-region="1"]').style.display).toBe('flex')
    expect(document.activeElement).toBe(q('[data-test-first="chat"]'))
    expect(useZenColumnsStore.getState().folds[`${G}/2/1`]).toBe(false)
    expect(q('[data-zen-column-announce]').textContent).toBe('Chat expanded')
    expect(mounts.chat).toBe(1)
  })

  it('the file’s collapsed starts it folded; another Garden’s or another column count’s keys don’t apply', () => {
    useZenColumnsStore.getState().save(`g-test0002/2/1`, false)
    useZenColumnsStore.getState().save(`${G}/3/1`, false)
    show(page({ columns: [{ size: 60 }, { size: 40, collapsible: true, collapsed: true }] }))
    const tab = q('[data-zen-column-tab="1"]')
    expect(q('[data-zen-column-tab-label]').textContent).toBe('Column 2')
    expect(tab.getAttribute('aria-label')).toMatch(/^Expand Column 2/)
  })

  it('a second window follows through storage', () => {
    show(page({ columns: TWO }))
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    const value = JSON.stringify({ version: 1, folds: { [`${G}/2/1`]: true } })
    act(() => {
      window.dispatchEvent(Object.assign(new Event('storage'), { key: ZEN_COLUMNS_KEY, newValue: value }))
    })
    q('[data-zen-column-tab="1"]')
  })

  it('the last open column can’t fold: its chevron is disabled', () => {
    show(page({ columns: [{ size: 60, collapsible: true, label: 'Board' }, { size: 40, collapsible: true, label: 'Chat' }] }))
    expect((q('[data-zen-column-chevron="0"]') as HTMLButtonElement).disabled).toBe(false)
    act(() => {
      fireEvent.click(q('[data-zen-column-chevron="1"]'))
    })
    const left = q('[data-zen-column-chevron="0"]') as HTMLButtonElement
    expect(left.disabled).toBe(true)
    expect(left.getAttribute('aria-label')).toBe('Board is the only open column')
    act(() => {
      fireEvent.click(left)
    })
    expect(document.querySelector('[data-zen-column-tab="0"]')).toBeNull()
    // A left column's tab hugs the left edge.
    act(() => {
      fireEvent.click(q('[data-zen-column-tab="1"]'))
    })
    act(() => {
      fireEvent.click(q('[data-zen-column-chevron="0"]'))
    })
    expect(q('[data-zen-column-tab="0"]').getAttribute('data-zen-column-side')).toBe('left')
  })

  it('a widget’s fold through the page’s controller is drawn and saved like the person’s; subscribers hear every move', () => {
    show(page({ columns: TWO }))
    expect(controller).not.toBeNull()
    const heard: ZenLayoutColumn[][] = []
    const off = controller!.subscribe((c) => heard.push(c))
    expect(controller!.columns()).toEqual([
      { index: 0, label: 'Column 1', collapsible: false, collapsed: false },
      { index: 1, label: 'Chat', collapsible: true, collapsed: false },
    ])
    let r: unknown
    act(() => {
      r = controller!.setByWidget(1, true)
    })
    expect(r).toBeNull()
    q('[data-zen-column-tab="1"]')
    expect(JSON.parse(localStorage.getItem(ZEN_COLUMNS_KEY) ?? 'null'), 'a widget’s fold is saved').toEqual({
      version: 1,
      folds: { [`${G}/2/1`]: true },
    })
    expect(useZenColumnsStore.getState().folds).toEqual({ [`${G}/2/1`]: true })
    expect(q('[data-zen-column-announce]').textContent).toBe('Chat collapsed')
    expect(heard).toEqual([
      [
        { index: 0, label: 'Column 1', collapsible: false, collapsed: false },
        { index: 1, label: 'Chat', collapsible: true, collapsed: true },
      ],
    ])
    act(() => {
      r = controller!.setByWidget(0, true)
    })
    expect(r).toBe('fixed')
    expect(useZenColumnsStore.getState().folds, 'a refused fold saves nothing').toEqual({ [`${G}/2/1`]: true })
    // The person's click after it wins (the last saved fold), and is saved.
    act(() => {
      fireEvent.click(q('[data-zen-column-tab="1"]'))
    })
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(useZenColumnsStore.getState().folds).toEqual({ [`${G}/2/1`]: false })
    off()
  })

  it('two quick widget folds in one tick can’t fold every column', () => {
    show(page({ columns: [{ size: 60, collapsible: true }, { size: 40, collapsible: true }] }))
    const results: unknown[] = []
    act(() => {
      results.push(controller!.setByWidget(0, true))
      results.push(controller!.setByWidget(1, true))
    })
    expect(results).toEqual([null, 'last_open'])
    q('[data-zen-column-tab="0"]')
    expect(document.querySelector('[data-zen-column-tab="1"]')).toBeNull()
  })

  it('the narrow window folds right to left and saves nothing; wide again, it opens', () => {
    let fire: ((width: number) => void) | null = null
    // Other parts of the page observe too (the bands); only the layout
    // row's observer gets the widths.
    class FakeRO {
      private readonly cb: ResizeObserverCallback
      constructor(cb: ResizeObserverCallback) {
        this.cb = cb
      }
      observe(el: Element): void {
        if (!el.hasAttribute('data-zen-layout')) return
        fire = (width) =>
          this.cb([{ contentRect: { width }, target: el } as unknown as ResizeObserverEntry], this as unknown as ResizeObserver)
      }
      disconnect(): void {}
      unobserve(): void {}
    }
    vi.stubGlobal('ResizeObserver', FakeRO)
    show(page({ columns: TWO, mins: [400, 400] }))
    expect(fire).not.toBeNull()
    act(() => fire!(1000))
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    act(() => fire!(700))
    expect(q('[data-zen-column-slot="1"]').getAttribute('data-zen-column-folded')).toBe('auto')
    expect(localStorage.getItem(ZEN_COLUMNS_KEY)).toBeNull()
    // The person opens it while narrow: it stays open.
    act(() => {
      fireEvent.click(q('[data-zen-column-tab="1"]'))
    })
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    act(() => fire!(650))
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    act(() => fire!(1000))
    act(() => fire!(700))
    expect(q('[data-zen-column-slot="1"]').getAttribute('data-zen-column-folded')).toBe('auto')
  })

  it('a single-view page (Tickets) never folds; My Home keeps the folds', () => {
    useZenColumnsStore.getState().save(`${G}/2/1`, true)
    useZenWindowStore.setState({ on: true, garden: G, view: 'tickets' })
    // The rail lives in column 0, which is fixed here.
    show(page({ columns: TWO, rail: true }))
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(document.querySelector('[data-zen-column-chevron]')).toBeNull()
    act(() => {
      useZenWindowStore.setState({ view: 'home' })
    })
    q('[data-zen-column-tab="1"]')
  })

  it('on a full canvas the tab and chevron sit above the band and its holes, below the band’s controls', () => {
    show(page({ columns: TWO, full: true }))
    const chevron = q('[data-zen-column-chevron="1"]')
    expect(chevron.style.zIndex).toBe('4')
    expect(chevron.style.top).toContain('calc(')
    act(() => {
      fireEvent.click(chevron)
    })
    const tab = q('[data-zen-column-tab="1"]')
    expect(tab.style.zIndex).toBe('4')
    expect(tab.style.marginTop).not.toBe('')
    expect(tab.hasAttribute('data-tauri-drag-region')).toBe(false)
  })
})

/** A custom frame's `k2.layout.*` on the real page: the layer the frame
 *  talks to, with `deps.layout` built from the page's controller the way
 *  ZenCustomFrame builds it. `press` stands for the person's press in the
 *  frame within the last 5 s. */
function frameOnPage(): {
  call: (verb: string, ...args: unknown[]) => Promise<ZenFrameReply>
  press: { on: boolean }
  clock: { t: number }
} {
  const press = { on: false }
  const clock = { t: 1_000_000 }
  const deps: ZenCustomLayerDeps = {
    widget: () =>
      ({
        id: 'board',
        kind: 'custom',
        widget: 'kanban-lite',
        column: 0,
        props: { config: {} },
        caps: [],
        requested: [],
        source: 'user',
        name: 'Board',
        description: null,
        reasons: {},
        libs: [],
        hash: 'h1',
        state: 'ok',
        errors: [],
        warnings: [],
        origin: 'local',
        paused: null,
      }) as ZenCustomWidgetPayload,
    gardenId: () => G,
    inner: { widgetId: 'board', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: () => undefined,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => clock.t,
    // Read live: a remounted page hands its frames a new controller.
    layout: {
      columns: () => {
        if (!controller) throw new Error('no page controller')
        return controller.columns()
      },
      setByWidget: (index, collapsed) => {
        if (!controller) throw new Error('no page controller')
        return controller.setByWidget(index, collapsed)
      },
      subscribe: (fn) => {
        if (!controller) throw new Error('no page controller')
        return controller.subscribe(fn)
      },
      pressed: () => press.on,
    },
  }
  const layer = createZenCustomLayer(deps)
  let id = 0
  const call = async (verb: string, ...args: unknown[]): Promise<ZenFrameReply> => {
    const box: { reply: ZenFrameReply | null } = { reply: null }
    clock.t += 300
    await act(async () => {
      box.reply = await layer.handle({ id: ++id, verb, args })
    })
    if (!box.reply) throw new Error(`no reply to ${verb}`)
    return box.reply
  }
  return { call, press, clock }
}

function replyCode(r: ZenFrameReply): string {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (r.ok) throw new Error(`ok: ${JSON.stringify(r.value)}`)
  return r.error.code
}

function replyValue(r: ZenFrameReply): unknown {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (!r.ok) throw new Error(`refused: ${JSON.stringify(r.error)}`)
  return r.value
}

/** Close the page and open it again as a fresh window would: the saved
 *  folds come back from localStorage, not from memory. */
function reload(p: ZenResolvedPage): void {
  cleanup()
  controller = null
  useZenColumnsStore.setState({ folds: createZenColumnsStore(localStorage).getState().folds })
  show(p)
}

describe('a widget’s k2.layout.* fold is remembered (0.46.4)', () => {
  it('without a press nothing folds and nothing is saved', async () => {
    show(page({ columns: TWO }))
    const { call } = frameOnPage()
    expect(replyCode(await call('layout.toggle', 1))).toBe('failed')
    expect(replyCode(await call('layout.set', { column: 1, collapsed: true }))).toBe('failed')
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(localStorage.getItem(ZEN_COLUMNS_KEY), 'no press, no save').toBeNull()
    expect(useZenColumnsStore.getState().folds).toEqual({})
  })

  it('a toggle just after a press folds and is saved; a reload opens it folded; unfolding is saved too', async () => {
    const p = page({ columns: TWO })
    show(p)
    const frame = frameOnPage()
    frame.press.on = true
    expect(replyValue(await frame.call('layout.toggle', 1))).toEqual({ index: 1, label: 'Chat', collapsible: true, collapsed: true })
    q('[data-zen-column-tab="1"]')
    expect(JSON.parse(localStorage.getItem(ZEN_COLUMNS_KEY) ?? 'null')).toEqual({ version: 1, folds: { [`${G}/2/1`]: true } })

    reload(p)
    q('[data-zen-column-tab="1"]')
    expect(q('[data-zen-column-slot="1"]').getAttribute('data-zen-column-folded')).toBe('')

    // The widget unfolds it (a new press); that is kept as well.
    expect(replyValue(await frame.call('layout.set', { column: 1, collapsed: false }))).toMatchObject({ index: 1, collapsed: false })
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
    expect(JSON.parse(localStorage.getItem(ZEN_COLUMNS_KEY) ?? 'null')).toEqual({ version: 1, folds: { [`${G}/2/1`]: false } })
    reload(p)
    expect(document.querySelector('[data-zen-column-tab]')).toBeNull()
  })

  it('the saved widget fold beats the file’s collapsed = true after a reload', async () => {
    const p = page({ columns: [{ size: 60 }, { size: 40, collapsible: true, collapsed: true, label: 'Thread' }] })
    show(p)
    q('[data-zen-column-tab="1"]')
    const frame = frameOnPage()
    frame.press.on = true
    expect(replyValue(await frame.call('layout.toggle', 1))).toMatchObject({ index: 1, collapsed: false })
    reload(p)
    expect(document.querySelector('[data-zen-column-tab]'), 'the Thread stays open').toBeNull()
  })

  it('the last open column still never folds, and the refusal saves nothing', async () => {
    show(page({ columns: [{ size: 60, collapsible: true, label: 'Board' }, { size: 40, collapsible: true, label: 'Chat' }] }))
    const frame = frameOnPage()
    frame.press.on = true
    replyValue(await frame.call('layout.toggle', 1))
    const refused = await frame.call('layout.toggle', 0)
    expect(replyCode(refused)).toBe('failed')
    expect('ok' in refused && !refused.ok && refused.error.message).toMatch(/only open column/)
    expect(document.querySelector('[data-zen-column-tab="0"]')).toBeNull()
    q('[data-zen-column-tab="1"]')
    expect(JSON.parse(localStorage.getItem(ZEN_COLUMNS_KEY) ?? 'null')).toEqual({ version: 1, folds: { [`${G}/2/1`]: true } })
  })
})

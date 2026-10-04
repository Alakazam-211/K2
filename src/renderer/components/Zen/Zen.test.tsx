// @vitest-environment jsdom
//
// prd-zen-mode-v1 S4 — the renderer skeleton through the real ZenHost, Zen
// root, page, template controls, bridge, control registry and safe mode.
// Only the edges are faked: the daemon (`daemon-cli`), the app socket
// (`session-events`), Tauri, and layout (jsdom has none, so the control
// check reads an injected geometry).
//
// Asserted (fail loudly):
//   - the toggle row on regular Home enters Zen for the CURRENT Home, and
//     every Zen config request goes to this computer's daemon;
//   - on/off is per Home and shared across windows; the page's Home
//     switcher only steps away, Exit turns that Home off;
//   - Shift while toggling enters safe mode (and reads no user files);
//   - a crashing widget drops to safe mode with the message;
//   - a missing / invisible / unwired required control drops to safe mode
//     after two failed checks, not one;
//   - the escape hatch (menu event, macOS targeted menu, Linux Ctrl+Alt+Z
//     in the capture phase, AltGr ignored) always exits, safe mode included;
//   - an unreachable local daemon is safe mode with its cause.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  getImpl: null as null | ((route: string, params: unknown) => unknown),
  zenHandlers: [] as Array<{ hostKey: string; fn: () => void }>,
  sockets: [] as string[],
  closedSockets: [] as string[],
  invokes: [] as Array<{ cmd: string; args: unknown }>,
  tauriListens: [] as Array<{ event: string; handler: () => void; options: unknown }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown) => {
    h.invokes.push({ cmd, args })
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async (event: string, handler: () => void, options?: unknown) => {
    h.tauriListens.push({ event, handler, options })
    return () => {
      const i = h.tauriListens.findIndex((l) => l.handler === handler)
      if (i >= 0) h.tauriListens.splice(i, 1)
    }
  }),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    startDragging: async () => undefined,
    isMaximized: async () => false,
    maximize: async () => undefined,
    unmaximize: async () => undefined,
    minimize: async () => undefined,
    close: async () => undefined,
    listen: async () => () => undefined,
  }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (!h.getImpl) throw new Error(`unexpected GET ${route}`)
    return h.getImpl(route, params)
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    return { ok: true }
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...mod,
    onZenChanged: (scope: { hostKey: string }, fn: () => void) => {
      const entry = { hostKey: scope.hostKey, fn }
      h.zenHandlers.push(entry)
      return () => {
        const i = h.zenHandlers.indexOf(entry)
        if (i >= 0) h.zenHandlers.splice(i, 1)
      }
    },
    subscribeToActiveState: (scope: { hostKey: string }) => {
      h.sockets.push(scope.hostKey)
      return () => void h.closedSockets.push(scope.hostKey)
    },
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { ZEN_HOMES_STORAGE_KEY, useZenHomesStore } from '@/lib/zen/zen-homes'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import type { ZenGeometry, ZenRect } from '@/lib/zen/zen-controls'
import { useZenViewStore, zenShownNow } from '@/lib/zen/zen-view'
import { ZenHost } from './ZenHost'
import { ZenToggleRow } from '@/components/Home/ZenToggleRow'
import { registerZenTemplateControls, registerZenWidget, type ZenTemplateControlsProps } from './zen-registry'
import { useZenBind } from './ZenTemplateControls'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

/** A resolved page as S1 answers it (docs/zen-contract.md). */
function goodPage(version = 'v1'): unknown {
  return {
    ok: true,
    schema: 1,
    version,
    page: {
      template: 'k2.texting@1',
      layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
      widgets: [
        { id: 'agents', kind: 'agents', column: 0, props: {}, caps: ['agents:read'], source: 'builtin' },
        { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: ['thread:read'], source: 'builtin' },
      ],
      controls: ['zen-toggle', 'home-switcher', 'drag-region'],
    },
    theme: {},
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  }
}

// ── Fake layout ─────────────────────────────────────────────────────────
// Every element gets a sensible on-screen box; tests override one element's
// style or box, or mark it covered.
const styleOverride = new Map<Element, Partial<{ display: string; visibility: string; opacity: number }>>()
const rectOverride = new Map<Element, ZenRect>()
const covered = new Set<Element>()
let lastRected: Element | null = null
const fakeGeometry: ZenGeometry = {
  rect(el) {
    lastRected = el
    const o = rectOverride.get(el)
    if (o) return o
    if (el.hasAttribute('data-zen-drag')) return { left: 300, top: 8, width: 500, height: 28 }
    return { left: 120, top: 8, width: 80, height: 28 }
  },
  style(el) {
    return { display: 'block', visibility: 'visible', opacity: 1, ...styleOverride.get(el) }
  },
  viewport() {
    return { width: 1200, height: 800 }
  },
  elementFromPoint() {
    if (!lastRected) return null
    return covered.has(lastRected) ? document.body : lastRected
  },
}

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  __resetZenAvailableForTests()
}

const unregister: Array<() => void> = []

beforeEach(() => {
  h.calls.length = 0
  h.zenHandlers.length = 0
  h.sockets.length = 0
  h.closedSockets.length = 0
  h.invokes.length = 0
  h.getImpl = (route) => {
    if (route === 'zen/get') return goodPage()
    throw new Error(`unexpected GET ${route}`)
  }
  setPlatform('MacIntel')
  __setZenGeometryForTests(fakeGeometry)
  styleOverride.clear()
  rectOverride.clear()
  covered.clear()
  __resetZenApiForTests()
  localStorage.clear()
  useZenHomesStore.setState({ on: {} })
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('home')
})

afterEach(() => {
  cleanup()
  for (const off of unregister.splice(0)) off()
  __setZenGeometryForTests(null)
  // Two Homes at most survive between tests: drop the extras.
  const s = useHomesStore.getState()
  for (const home of s.homes.slice(1)) s.deleteHome(home.id)
})

function twoHomes(): { first: string; second: string } {
  const first = selectedHome(useHomesStore.getState()).id
  const second = useHomesStore.getState().createHome('Work')
  if (!second) throw new Error('createHome refused')
  return { first, second }
}

function mount(): void {
  render(
    <>
      <ZenToggleRow />
      <ZenHost />
    </>,
  )
}

function zenRoot(): HTMLElement | null {
  return document.querySelector('[data-zen-root]')
}

async function enterViaRow(shiftKey = false): Promise<void> {
  const row = document.querySelector('[data-zen-enter]')
  if (!row) throw new Error('no Zen toggle row')
  await act(async () => {
    fireEvent.click(row, { shiftKey })
  })
}

async function pageReady(): Promise<void> {
  await waitFor(() => {
    if (!document.querySelector('[data-zen-page]')) throw new Error('Zen page not drawn')
  })
}

function safeReason(): string {
  const el = document.querySelector('[data-zen-safe-reason]')
  if (!el) throw new Error(`not in safe mode\n${document.body.innerHTML.slice(0, 2000)}`)
  return el.textContent ?? ''
}

describe('entering Zen from the regular Home', () => {
  it('the toggle row renders on Home and starts the CURRENT Home’s Zen page from this computer’s daemon', async () => {
    const { first, second } = twoHomes()
    // createHome selects the new Home: the current Home is "Work".
    expect(selectedHome(useHomesStore.getState()).id).toBe(second)
    mount()
    const row = document.querySelector('[data-zen-toggle-row] [role="switch"]')
    if (!row) throw new Error('no toggle row')
    expect(row.getAttribute('aria-checked')).toBe('false')
    expect(zenRoot()).toBeNull()

    await enterViaRow()
    await pageReady()

    expect(useZenHomesStore.getState().on).toEqual({ [second]: true })
    expect(useZenHomesStore.getState().on[first]).toBeUndefined()
    expect(JSON.parse(localStorage.getItem(ZEN_HOMES_STORAGE_KEY) ?? 'null')).toEqual({
      version: 1,
      on: { [second]: true },
    })
    // The page Z10 resolved: the template band and one column per split.
    expect(document.querySelector('[data-zen-template-bar]')).not.toBeNull()
    expect(document.querySelectorAll('[data-zen-column]').length).toBe(2)
    expect(document.querySelector('[data-zen-widget="agents"]')).not.toBeNull()
    expect(document.querySelector('[data-zen-widget="conversation"]')).not.toBeNull()
    expect(zenRoot()?.style.zIndex).toBe('150')
    // T4.2: ensure, then get, both on the LOCAL daemon, never the window's.
    expect(h.calls.map((c) => [c.method, c.hostKey, c.route])).toEqual([
      ['POST', 'local', 'zen/page/ensure'],
      ['GET', 'local', 'zen/get'],
    ])
    expect(h.calls[0].data).toEqual({ homeId: second, name: 'Work' })
    expect(h.calls[1].data).toEqual({ home: second })
    // zen_changed is watched on this computer's own app socket.
    expect(h.sockets).toEqual(['local'])
    expect(h.zenHandlers.map((z) => z.hostKey)).toEqual(['local'])
  })

  it('entering from Settings or another page goes to Home first', async () => {
    mount()
    act(() => {
      usePageViewStore.getState().setPage('projects')
      useSettingsStore.setState({ settingsOpen: true })
    })
    await enterViaRow()
    await pageReady()
    expect(usePageViewStore.getState().page).toBe('home')
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    expect(zenShownNow()).toBe(true)
  })
})

describe('per-Home on/off', () => {
  it('is per Home: the page’s Home switcher steps away, coming back shows Zen, and the page’s toggle turns that Home off', async () => {
    const { first, second } = twoHomes()
    mount()
    await enterViaRow()
    await pageReady()

    // The template's Home pill: open it, pick the first Home.
    const pill = document.querySelector('[data-zen-home-pill]')
    if (!pill) throw new Error('no Home pill')
    act(() => void fireEvent.click(pill))
    const option = document.querySelector(`[data-zen-home-option="${first}"]`)
    if (!option) throw new Error('no option for the first Home')
    act(() => void fireEvent.click(option))
    expect(selectedHome(useHomesStore.getState()).id).toBe(first)
    // Stepped away: the first Home's Zen is off, the second's stays on.
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on).toEqual({ [second]: true })

    act(() => useHomesStore.getState().selectHome(second))
    await pageReady()
    expect(zenRoot()).not.toBeNull()

    // Settings hides Zen without turning it off.
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on[second]).toBe(true)
    act(() => useSettingsStore.setState({ settingsOpen: false }))
    await pageReady()

    // The page's Zen toggle is Exit: that Home goes off.
    const toggle = document.querySelector('[data-zen-switch]')
    if (!toggle) throw new Error('no Zen switch')
    act(() => void fireEvent.click(toggle))
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on).toEqual({})
    expect(JSON.parse(localStorage.getItem(ZEN_HOMES_STORAGE_KEY) ?? 'null')).toEqual({ version: 1, on: {} })
  })

  it('another window turning the Home off (storage event) leaves Zen here too', async () => {
    const home = selectedHome(useHomesStore.getState()).id
    mount()
    await enterViaRow()
    await pageReady()
    act(() => {
      window.dispatchEvent(
        new StorageEvent('storage', { key: ZEN_HOMES_STORAGE_KEY, newValue: JSON.stringify({ version: 1, on: {} }) }),
      )
    })
    expect(zenRoot()).toBeNull()
    act(() => {
      window.dispatchEvent(
        new StorageEvent('storage', {
          key: ZEN_HOMES_STORAGE_KEY,
          newValue: JSON.stringify({ version: 1, on: { [home]: true } }),
        }),
      )
    })
    await pageReady()
  })
})

describe('safe mode', () => {
  it('Shift while toggling enters safe mode and reads none of the user’s files', async () => {
    mount()
    await enterViaRow(true)
    await pageReady()
    expect(safeReason()).toBe('You held Shift while turning Zen on.')
    expect(document.querySelector('[data-zen-safe-banner]')?.textContent).toContain(
      'Zen is in safe mode. Your files are untouched.',
    )
    expect(h.calls.filter((c) => c.route.startsWith('zen/'))).toEqual([])
    // Try again reads the page.
    const again = document.querySelector('[data-zen-try-again]')
    if (!again) throw new Error('no Try again')
    await act(async () => void fireEvent.click(again))
    await waitFor(() => expect(document.querySelector('[data-zen-safe-banner]')).toBeNull())
    expect(h.calls.map((c) => c.route)).toEqual(['zen/page/ensure', 'zen/get'])
  })

  it('a widget that crashes drops to safe mode with the message, and Exit Zen still works', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    unregister.push(
      registerZenWidget('conversation', () => {
        throw new Error('boom')
      }),
    )
    const home = selectedHome(useHomesStore.getState()).id
    mount()
    await enterViaRow()
    await waitFor(() => expect(safeReason()).toBe('The page crashed: boom'))
    // Safe mode's page still crashes here (the same widget): the last-resort
    // panel shows with K2's banner; the menu path still exits.
    expect(document.querySelector('[data-zen-last-resort]')).not.toBeNull()
    act(() => void window.dispatchEvent(new Event('menu:zen-toggle')))
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on[home]).toBeUndefined()
    err.mockRestore()
  })

  it('a crash in a user page falls back to the built-in page with K2’s banner', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    // Breaks only on the user's page; the built-in safe page draws it.
    unregister.push(
      registerZenWidget('agents', () => {
        if (useZenViewStore.getState().safe === null) throw new Error('agents widget broke')
        return <div data-ok-agents="" />
      }),
    )
    mount()
    await enterViaRow()
    await waitFor(() => expect(safeReason()).toBe('The page crashed: agents widget broke'))
    expect(document.querySelector('[data-zen-page="k2.texting@1"]')).not.toBeNull()
    expect(document.querySelector('[data-ok-agents]')).not.toBeNull()
    expect(document.querySelector('[data-zen-last-resort]')).toBeNull()
    const exit = document.querySelector('[data-zen-safe-exit]')
    if (!exit) throw new Error('no Exit Zen in the banner')
    act(() => void fireEvent.click(exit))
    expect(zenRoot()).toBeNull()
    err.mockRestore()
  })

  function ControlsWithout({ omit }: { omit: 'home-switcher' | 'zen-toggle' }): (p: ZenTemplateControlsProps) => React.JSX.Element {
    return function Controls({ bridge }: ZenTemplateControlsProps): React.JSX.Element {
      const toggle = useZenBind(bridge, 'zen-toggle')
      const trigger = useZenBind(bridge, 'home-switcher')
      const drag = useZenBind(bridge, 'drag-region')
      return (
        <div>
          {omit !== 'home-switcher' && <button ref={trigger} data-test-trigger="">Homes</button>}
          <div ref={drag} data-zen-drag="" />
          {omit !== 'zen-toggle' && <button ref={toggle} data-zen-switch="">Zen</button>}
        </div>
      )
    }
  }

  it('a missing Home switcher: one failed check is not enough, two are safe mode', async () => {
    unregister.push(registerZenTemplateControls('k2.texting@1', ControlsWithout({ omit: 'home-switcher' })))
    mount()
    await enterViaRow()
    await pageReady()
    // The schedule's own first-paint check may already have run once.
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Home switcher isn’t on the page.')
  })

  it('a single failed check (a mid-animation frame) does not trip safe mode', async () => {
    mount()
    await enterViaRow()
    await pageReady()
    const toggle = document.querySelector('[data-zen-switch]')
    if (!toggle) throw new Error('no Zen switch')
    styleOverride.set(toggle, { opacity: 0 })
    act(() => runZenControlChecksNow())
    styleOverride.delete(toggle)
    act(() => runZenControlChecksNow())
    styleOverride.set(toggle, { opacity: 0 })
    act(() => runZenControlChecksNow())
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Zen toggle isn’t visible.')
  })

  it('an invisible switcher (covered by another element) is safe mode', async () => {
    mount()
    await enterViaRow()
    await pageReady()
    const pill = document.querySelector('[data-zen-home-pill]')
    if (!pill) throw new Error('no Home pill')
    covered.add(pill)
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Home switcher isn’t visible.')
  })

  it('a toggle drawn without bind (bare element) is "not wired"', async () => {
    unregister.push(
      registerZenTemplateControls('k2.texting@1', function Bare({ bridge }: ZenTemplateControlsProps) {
        const trigger = useZenBind(bridge, 'home-switcher')
        const drag = useZenBind(bridge, 'drag-region')
        return (
          <div>
            <button ref={trigger}>Homes</button>
            <div ref={drag} data-zen-drag="" />
            {/* Says it is the toggle, never handed to bind. */}
            <button data-zen-control="zen-toggle" onClick={() => undefined}>
              Zen
            </button>
          </div>
        )
      }),
    )
    mount()
    await enterViaRow()
    await pageReady()
    act(() => useZenViewStore.setState({ safe: null }))
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Zen toggle isn’t wired.')
  })

  it('a page that does not declare the controls is safe mode', async () => {
    h.getImpl = () => {
      const p = goodPage() as { page: { controls: string[] } }
      p.page.controls = ['zen-toggle', 'drag-region']
      return p
    }
    mount()
    await enterViaRow()
    await pageReady()
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(safeReason()).toBe('The Home switcher isn’t declared by the page.')
  })

  it('the local daemon not answering is safe mode: “Can’t reach K2 on this computer.”', async () => {
    h.getImpl = () => {
      throw new Error('connection refused')
    }
    mount()
    await enterViaRow()
    await waitFor(() => expect(safeReason()).toBe('Can’t reach K2 on this computer.'))
  })

  it('a zen_changed from this computer ends safe mode and re-reads the page', async () => {
    mount()
    await enterViaRow(true)
    await pageReady()
    expect(safeReason()).toBe('You held Shift while turning Zen on.')
    const local = h.zenHandlers.find((z) => z.hostKey === 'local')
    if (!local) throw new Error('no local zen_changed handler')
    await act(async () => local.fn())
    await waitFor(() => expect(document.querySelector('[data-zen-safe-banner]')).toBeNull())
    expect(h.calls.map((c) => c.route)).toEqual(['zen/page/ensure', 'zen/get'])
  })

  it('a config error shows the line and keeps the last good page', async () => {
    h.getImpl = () => {
      const p = goodPage() as { errors: unknown[] }
      p.errors = [{ file: 'zen.toml', line: 12, col: 3, message: "unknown color 'acent'" }]
      return p
    }
    mount()
    await enterViaRow()
    await pageReady()
    expect(document.querySelector('[data-zen-config-error]')?.textContent).toBe(
      "zen.toml line 12: unknown color 'acent'. Showing your last good version.",
    )
    expect(document.querySelector('[data-zen-safe-banner]')).toBeNull()
  })
})

describe('the escape hatch', () => {
  it('macOS: the native menu’s event (targeted at this window) exits and re-enters; no webview chord', async () => {
    const home = selectedHome(useHomesStore.getState()).id
    mount()
    await waitFor(() => expect(h.tauriListens.some((l) => l.event === 'menu:zen-toggle')).toBe(true))
    const menu = h.tauriListens.find((l) => l.event === 'menu:zen-toggle')
    if (!menu) throw new Error('no menu listener')
    // Only this window's label: a broadcast would flip a shared Home per window.
    expect(menu.options).toEqual({ target: { kind: 'AnyLabel', label: 'main' } })

    act(() => menu.handler())
    await pageReady()
    expect(useZenHomesStore.getState().on[home]).toBe(true)
    // ⌃⌘Z typed into the webview does nothing on macOS (the accelerator owns it).
    act(() => {
      fireEvent.keyDown(window, { code: 'KeyZ', key: 'z', ctrlKey: true, metaKey: true })
    })
    expect(zenRoot()).not.toBeNull()
    act(() => menu.handler())
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on[home]).toBeUndefined()
    // The menu text follows this (focused) window.
    expect(h.invokes.filter((i) => i.cmd === 'set_zen_menu_label').map((i) => i.args)).toEqual([
      { inZen: false },
      { inZen: true },
      { inZen: false },
    ])
  })

  it('Linux: Ctrl+Alt+Z exits even when a widget stops the key in the bubble phase; AltGr+Z does nothing', async () => {
    setPlatform('Linux x86_64')
    unregister.push(
      registerZenWidget('conversation', () => (
        <textarea data-swallow="" onKeyDown={(e) => e.stopPropagation()} />
      )),
    )
    const home = selectedHome(useHomesStore.getState()).id
    mount()
    await enterViaRow()
    await pageReady()
    // K2's own window controls + menu button are in Zen (Z25).
    expect(document.querySelector('[data-zen-chrome-cluster="left"]')).not.toBeNull()
    const box = document.querySelector('[data-swallow]')
    if (!box) throw new Error('no swallowing widget')
    act(() => {
      const ev = new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true, bubbles: true })
      Object.defineProperty(ev, 'getModifierState', { value: (k: string) => k === 'AltGraph' })
      box.dispatchEvent(ev)
    })
    expect(zenRoot()).not.toBeNull()
    act(() => {
      box.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true, bubbles: true }))
    })
    expect(zenRoot()).toBeNull()
    expect(useZenHomesStore.getState().on[home]).toBeUndefined()
    // And from outside Zen it goes to Home and turns Zen on.
    act(() => usePageViewStore.getState().setPage('agents'))
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true }))
    })
    await pageReady()
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('Linux: the app menu in Zen has Exit Zen Mode, and it works in safe mode', async () => {
    setPlatform('Linux x86_64')
    mount()
    await enterViaRow(true)
    await pageReady()
    const menuButton = document.querySelector('[data-zen-app-menu]')
    if (!menuButton) throw new Error('no menu button in Zen')
    act(() => void fireEvent.click(menuButton))
    const item = screen.getByRole('menuitem', { name: 'Exit Zen Mode' })
    await act(async () => void fireEvent.click(item))
    expect(zenRoot()).toBeNull()
  })
})

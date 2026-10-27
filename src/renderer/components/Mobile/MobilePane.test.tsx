// @vitest-environment jsdom
// The Mobile tab's pane (prd-mobile-dev-v1 §2, §5, §7; vs-live M-A1, M-A3,
// M-A19), against the stub stream, fail loud:
//  - the left column is the agent's Thread view, exactly like Design;
//  - no device → the chooser and Start; Start boots through `sim/boot`
//    (noTab, the tab is open) and remembers the device on the item;
//  - a running device streams onto the canvas; each frame is acked, also
//    while the frame is held (S5 hook);
//  - taps, swipes and keys go to the device; ⌘ chords stay with K2;
//  - a watch-only login (remote Members) gets no input and says Watching;
//  - a hidden tab closes its socket (it stops counting as a viewer);
//  - Rotate / Home / Back / Take over send on the socket; Stop and
//    Screenshot go through their routes.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  devices: { devices: [] as unknown[] },
  posts: [] as Array<{ route: string; body: unknown }>,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
      if (route === 'sim/status') {
        if (params?.workspace !== '/w/docs') throw new Error(`sim/status for ${String(params?.workspace)}`)
        return daemon.status
      }
      if (route === 'sim/list-devices') return daemon.devices
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      daemon.posts.push({ route, body })
      if (route === 'sim/boot') return { ok: true, session: RUNNING }
      if (route === 'sim/stop') return { ok: true }
      if (route === 'sim/screenshot') return { ok: true, path: '.k2/sim/shots/s1-0001.png' }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('@/components/Design/DesignThreadColumn', () => ({
  DesignThreadColumn: (props: { projectPath: string; agentName: string; visible: boolean }) => (
    <div data-testid="design-thread-column" data-path={props.projectPath} data-agent={props.agentName} data-visible={String(props.visible)} />
  ),
}))

import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { MobilePane } from './MobilePane'
import { mobileSplitKey } from './mobile-split-sizes'
import { createStubSimStream, type StubSimStream } from './sim-stream'

const RUNNING = {
  id: 's1',
  lane: 'android',
  device: 'Pixel 9',
  state: 'running',
  orientation: 'portrait',
  screen: { width: 412, height: 915 },
  control: null,
}
const ANDROID_OK = [{ lane: 'android', usable: true }]
const DEVICES = {
  devices: [
    { id: 'pixel-9', name: 'Pixel 9', lane: 'android' },
    { id: 'pixel-8a', name: 'Pixel 8a', lane: 'android' },
  ],
}

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const painted: string[] = []

function seedTabs(): { tab: Tab; itemId: string; pgId: string } {
  const chatPg = 'pg-chat'
  const chat: Tab = {
    id: 'tab-chat',
    title: 'Chat',
    mosaicTree: chatPg,
    isSystemAgent: true,
    paneGroups: new Map([[chatPg, { id: chatPg, activeItemIndex: 0, items: [{ id: 'i-chat', type: 'agent', data: { agentName: 'docs', projectPath: '/w/docs', section: 'chat' } }] }]]),
  }
  const pgId = 'pg-mobile'
  const tab: Tab = {
    id: 'tab-mobile',
    title: 'Mobile',
    mosaicTree: pgId,
    paneGroups: new Map([[pgId, { id: pgId, activeItemIndex: 0, items: [{ id: 'i-mobile', type: 'mobile', data: {} }] }]]),
  }
  useTabsStore.setState({ tabs: [chat, tab], activeTabId: tab.id, splitCount: 1, extraGroups: [], backgroundWorkspaces: {} })
  return { tab, itemId: 'i-mobile', pgId }
}

function renderMobile(stub: StubSimStream, visible = true): ReturnType<typeof renderInPrimaryRoom> {
  const { tab, itemId, pgId } = seedTabs()
  return renderInPrimaryRoom(
    <TabVisibilityContext.Provider value={visible}>
      <MobilePane itemId={itemId} tabId={tab.id} paneGroupId={pgId} data={{}} connector={stub.connector} />
    </TabVisibilityContext.Provider>,
  )
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 8; i++) await Promise.resolve()
  })
}

function screenEl(): HTMLElement {
  const el = document.querySelector('[data-mobile-screen]') as HTMLElement | null
  if (!el) throw new Error('no device screen')
  return el
}

function tool(name: string): HTMLButtonElement {
  const el = document.querySelector(`[data-mobile-tool="${name}"]`) as HTMLButtonElement | null
  if (!el) throw new Error(`no ${name} button`)
  return el
}

beforeEach(() => {
  daemon.status = { lanes: ANDROID_OK, session: RUNNING, canControl: true }
  daemon.devices = DEVICES
  daemon.posts = []
  painted.length = 0
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 100, y: 50, width: 206, height: 457.5, top: 50, left: 100, right: 306, bottom: 507.5, toJSON: () => ({}),
  } as DOMRect)
  // jsdom has no canvas: a 2d context that records what was painted.
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(function (this: HTMLCanvasElement) {
    const canvas = this
    return {
      // One entry per painted frame: the stub paints its background first.
      fillRect: (x: number, y: number) => {
        if (x === 0 && y === 0) painted.push(`fill:${canvas.width}x${canvas.height}`)
      },
      fillText: () => {},
      drawImage: () => painted.push('image'),
      set fillStyle(_v: string) {},
      set font(_v: string) {},
    } as unknown as CanvasRenderingContext2D
  } as unknown as typeof HTMLCanvasElement.prototype.getContext)
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  useTabsStore.setState({ tabs: [], activeTabId: null, extraGroups: [], backgroundWorkspaces: {} })
})

describe('layout (M-A1)', () => {
  it('the left column is the workspace agent’s Thread view, beside the device', async () => {
    renderMobile(createStubSimStream())
    await flush()
    const col = document.querySelector('[data-mobile-chat]') as HTMLElement | null
    if (!col) throw new Error('no Thread column')
    const thread = screen.getByTestId('design-thread-column')
    expect(col.contains(thread)).toBe(true)
    expect(thread.getAttribute('data-path')).toBe('/w/docs')
    expect(thread.getAttribute('data-agent')).toBe('docs')
    expect(document.querySelector('[data-mobile-split]')).not.toBeNull()
    expect(screen.getByTestId('mobile-toolbar')).toBeTruthy()
  })

  it('dragging the split saves it per window under the mobile prefix', async () => {
    renderMobile(createStubSimStream())
    await flush()
    const sep = document.querySelector('[data-mobile-split]') as HTMLElement
    fireEvent.mouseDown(sep, { clientX: 150 })
    fireEvent.mouseMove(window, { clientX: 150 })
    fireEvent.mouseUp(window)
    const saved = JSON.parse(localStorage.getItem(mobileSplitKey('main')) ?? 'null') as Record<string, number[]> | null
    if (!saved) throw new Error('split not saved')
    expect(Object.values(saved)).toEqual([[24, 76]])
    expect(localStorage.getItem('k2.windowChrome.design.main')).toBeNull()
  })
})

describe('no device yet', () => {
  it('shows the chooser and Start; Start boots the picked device with noTab and remembers it', async () => {
    daemon.status = { lanes: ANDROID_OK, session: null, canControl: true }
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe('No device is running.')
    expect(document.querySelector('[data-mobile-start]')?.textContent).toBe('Start Pixel 9')
    const chooser = document.querySelector('[data-mobile-device]') as HTMLSelectElement
    expect(Array.from(chooser.options).map((o) => o.textContent)).toEqual(['Pixel 9', 'Pixel 8a'])
    fireEvent.change(chooser, { target: { value: 'pixel-8a' } })
    expect(daemon.posts).toEqual([])
    await act(async () => {
      fireEvent.click(tool('boot'))
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/boot', body: { workspace: '/w/docs', device: 'pixel-8a', platform: 'android', noTab: true } }])
    const item = Array.from(useTabsStore.getState().tabs[1].paneGroups.values())[0].items[0]
    expect(item.data).toEqual({ deviceType: 'pixel-8a' })
  })

  it('a server with no usable lane says why; a watch-only login has no Start', async () => {
    daemon.status = { lanes: [{ lane: 'android', usable: false, reason: '/dev/kvm is not usable by K2.' }], session: null, canControl: true }
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe("This server can't run a device yet. /dev/kvm is not usable by K2.")
    cleanup()
    daemon.status = { lanes: ANDROID_OK, session: null, canControl: false }
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe('No device is running.')
    expect(document.querySelector('[data-mobile-tool="boot"]')).toBeNull()
    expect(document.querySelector('[data-mobile-start]')).toBeNull()
    expect(document.querySelector('[data-mobile-watching]')).not.toBeNull()
  })
})

describe('the running device', () => {
  it('streams frames onto the canvas and acks each one, also while held', async () => {
    const stub = createStubSimStream()
    const view = renderMobile(stub)
    await flush()
    expect(stub.openCount()).toBe(1)
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(stub.acks).toEqual([1])
    expect(painted).toEqual(['fill:824x1830'])
    expect(document.querySelector('[data-mobile-bezel]')?.getAttribute('data-mobile-bezel')).toBe('portrait')
    // S5 hook: hold the frame — still acked, not painted.
    const { tab, itemId, pgId } = { tab: useTabsStore.getState().tabs[1], itemId: 'i-mobile', pgId: 'pg-mobile' }
    view.rerender(
      <TabVisibilityContext.Provider value>
        <MobilePane itemId={itemId} tabId={tab.id} paneGroupId={pgId} data={{}} connector={stub.connector} holdFrame />
      </TabVisibilityContext.Provider>,
    )
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(stub.acks).toEqual([1, 2])
    expect(painted).toEqual(['fill:824x1830'])
  })

  it('a tap and a swipe on the canvas go to the device in dp', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    const el = screenEl()
    expect(el.getAttribute('data-mobile-screen')).toBe('interactive')
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 151, clientY: 101, button: 0, pointerId: 1 })
    expect(stub.inputs).toEqual([{ action: 'tap', x: 100, y: 100 }])
    fireEvent.pointerDown(el, { clientX: 200, clientY: 400, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 200, clientY: 150, button: 0, pointerId: 1 })
    expect(stub.inputs[1]).toMatchObject({ action: 'swipe', from: { x: 200, y: 700 }, to: { x: 200, y: 200 } })
  })

  it('keys go to the device; ⌘W stays with K2; Esc leaves the device', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    const el = screenEl()
    el.focus()
    fireEvent.keyDown(el, { key: 'h' })
    fireEvent.keyDown(el, { key: 'Enter' })
    fireEvent.keyDown(el, { key: 'H', metaKey: true, shiftKey: true })
    const cmdW = fireEvent.keyDown(el, { key: 'w', metaKey: true })
    expect(cmdW).toBe(true) // not prevented: K2 closes the tab as usual
    expect(stub.inputs).toEqual([
      { action: 'type', text: 'h' },
      { action: 'key', key: 'Enter' },
      { action: 'button', button: 'home' },
    ])
    expect(document.activeElement).toBe(el)
    fireEvent.keyDown(el, { key: 'Escape' })
    expect(document.activeElement).not.toBe(el)
    expect(stub.inputs).toHaveLength(3)
  })

  it('Rotate, Home, Back and Take over / Hand back send on the socket', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    fireEvent.click(tool('rotate'))
    await flush()
    expect(document.querySelector('[data-mobile-bezel]')?.getAttribute('data-mobile-bezel')).toBe('landscape')
    fireEvent.click(tool('home'))
    fireEvent.click(tool('back'))
    fireEvent.click(tool('take-over'))
    await act(async () => {
      stub.pushControl({ heldBy: 'takeover', by: 'rosson' })
    })
    fireEvent.click(tool('hand-back'))
    expect(stub.inputs).toEqual([
      { action: 'rotate', direction: 'right' },
      { action: 'button', button: 'home' },
      { action: 'button', button: 'back' },
      { action: 'take' },
      { action: 'release' },
    ])
    expect(document.querySelector('[data-mobile-tool="take-over"]')).not.toBeNull()
  })

  it('Stop and Screenshot go through their routes; the shot path is shown', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      fireEvent.click(tool('screenshot'))
    })
    await flush()
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toMatch(/^Saved \.k2\/sim\/shots\/s1-0001\.png/)
    await act(async () => {
      fireEvent.click(tool('stop'))
    })
    await flush()
    expect(daemon.posts.map((p) => p.route)).toEqual(['sim/screenshot', 'sim/stop'])
    expect(daemon.posts[1].body).toEqual({ workspace: '/w/docs' })
  })

  it('a person holding the device shows on the screen for other viewers', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushControl({ heldBy: 'person', by: 'jules' })
    })
    expect(document.querySelector('[data-mobile-held]')?.textContent).toBe('jules is using the device')
  })
})

describe('watching and visibility', () => {
  it('a watch-only login sees the stream but sends nothing (remote Members)', async () => {
    daemon.status = { lanes: ANDROID_OK, session: RUNNING, canControl: false }
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    expect(stub.openCount()).toBe(1)
    const el = screenEl()
    expect(el.getAttribute('data-mobile-screen')).toBe('watching')
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.keyDown(el, { key: 'a' })
    expect(stub.inputs).toEqual([])
    for (const name of ['rotate', 'home', 'back', 'take-over', 'stop']) {
      expect(document.querySelector(`[data-mobile-tool="${name}"]`)).toBeNull()
    }
    expect((tool('screenshot') as HTMLButtonElement).disabled).toBe(true)
    expect(document.querySelector('[data-mobile-watching]')?.textContent).toBe('Watching')
  })

  it('a hidden tab opens no socket; showing it connects', async () => {
    const stub = createStubSimStream()
    const view = renderMobile(stub, false)
    await flush()
    expect(stub.openCount()).toBe(0)
    const { tab } = { tab: useTabsStore.getState().tabs[1] }
    view.rerender(
      <TabVisibilityContext.Provider value>
        <MobilePane itemId="i-mobile" tabId={tab.id} paneGroupId="pg-mobile" data={{}} connector={stub.connector} />
      </TabVisibilityContext.Provider>,
    )
    await flush()
    expect(stub.openCount()).toBe(1)
    view.rerender(
      <TabVisibilityContext.Provider value={false}>
        <MobilePane itemId="i-mobile" tabId={tab.id} paneGroupId="pg-mobile" data={{}} connector={stub.connector} />
      </TabVisibilityContext.Provider>,
    )
    await flush()
    expect(stub.openCount()).toBe(0)
  })
})

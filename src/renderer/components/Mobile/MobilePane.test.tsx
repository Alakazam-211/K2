// @vitest-environment jsdom
// The Mobile tab's pane (prd-mobile-dev-v1 §2, §5, §7; vs-live M-A1, M-A3,
// M-A19) against the stub stream and Builder A's route shapes
// (k2-core::sim::api), fail loud:
//  - the left column is the agent's Thread view, exactly like Design;
//  - no device → the chooser (sim/devices) and Start; Start boots through
//    sim/boot (noTab, the tab is open) and remembers the device on the item;
//  - a lane waiting on the owner (licence, KVM) says so and offers no Start;
//  - a running device streams onto the canvas (the screen size from the
//    frame header); each frame is acked, also while held (S5 hook);
//  - taps, swipes and keys go to the device in points; ⌘ chords stay with K2;
//  - a watch-only login (remote Member) gets no input and says Watching;
//  - a hidden tab closes its socket (it stops counting as a viewer);
//  - Rotate / Home / Back / Take over send stream messages; Stop and
//    Screenshot go through their routes; a sim_state event re-reads status.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  devices: { lane: 'android', devices: [] as unknown[] },
  whoami: { owner: false, role: 'member' } as unknown,
  posts: [] as Array<{ route: string; body: unknown }>,
  gets: [] as string[],
}))
const simEvents = vi.hoisted(() => ({ fns: new Set<(e: { kind: string; workspace: string; sim: string }) => void>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onSimChanged: (_scope: unknown, fn: (e: { kind: string; workspace: string; sim: string }) => void) => {
      simEvents.fns.add(fn)
      return () => simEvents.fns.delete(fn)
    },
  }
})
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
      daemon.gets.push(route)
      if (route === 'sim/status') {
        if (params?.workspace !== '/w/docs') throw new Error(`sim/status for ${String(params?.workspace)}`)
        return daemon.status
      }
      if (route === 'sim/devices') return daemon.devices
      if (route === 'auth/whoami') return daemon.whoami
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      daemon.posts.push({ route, body })
      if (route === 'sim/boot') return { ...RUNNING, state: 'booting' }
      if (route === 'sim/stop') return { ...RUNNING, state: 'stopping' }
      if (route === 'sim/screenshot') return { path: '/w/docs/.k2/sim/shots/s1-0001.png', width: 412, height: 915 }
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

const PIXEL_9 = { id: 'pixel_9', name: 'Pixel 9', widthPt: 412, heightPt: 915, scale: 2.625, os: 'android-35', default: true }
const PIXEL_8A = { id: 'pixel_8a', name: 'Pixel 8a', widthPt: 412, heightPt: 892, scale: 2.625, os: 'android-35', default: false }
const RUNNING = {
  sim: 's1',
  workspace: 'p-docs',
  workspaceHandle: 'docs',
  lane: 'android',
  state: 'running',
  profile: PIXEL_9,
  lastUsedAt: 1,
  hold: { kind: 'free' },
  viewers: 1,
}
const STOPPED = { ...RUNNING, state: 'stopped' }
const ANDROID_OK = [{ lane: 'android', usable: true, ready: true }]

function status(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { lanes: ANDROID_OK, sim: RUNNING, running: 1, limits: {}, agentsAllowed: false, ...over }
}

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const painted: string[] = []

function seedTabs(): void {
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
}

function pane(stub: StubSimStream, visible: boolean, holdFrame = false): React.JSX.Element {
  return (
    <TabVisibilityContext.Provider value={visible}>
      <MobilePane itemId="i-mobile" tabId="tab-mobile" paneGroupId="pg-mobile" data={{}} connector={stub.connector} holdFrame={holdFrame} />
    </TabVisibilityContext.Provider>
  )
}

function renderMobile(stub: StubSimStream, visible = true): ReturnType<typeof renderInPrimaryRoom> {
  seedTabs()
  return renderInPrimaryRoom(pane(stub, visible))
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
  daemon.status = status()
  daemon.devices = { lane: 'android', devices: [PIXEL_9, PIXEL_8A] }
  daemon.whoami = { owner: false, role: 'member' }
  daemon.posts = []
  daemon.gets = []
  simEvents.fns.clear()
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

describe('no device running', () => {
  it('shows the chooser and Start; Start boots the picked device with noTab and remembers it', async () => {
    daemon.status = status({ sim: STOPPED })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe('No device is running.')
    expect(document.querySelector('[data-mobile-start]')?.textContent).toBe('Start Pixel 9')
    const chooser = document.querySelector('[data-mobile-device]') as HTMLSelectElement
    expect(Array.from(chooser.options).map((o) => o.textContent)).toEqual(['Pixel 9', 'Pixel 8a'])
    fireEvent.change(chooser, { target: { value: 'pixel_8a' } })
    expect(daemon.posts).toEqual([])
    await act(async () => {
      fireEvent.click(tool('boot'))
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/boot', body: { workspace: '/w/docs', device: 'pixel_8a', platform: 'android', noTab: true } }])
    const item = Array.from(useTabsStore.getState().tabs[1].paneGroups.values())[0].items[0]
    expect(item.data).toEqual({ deviceType: 'pixel_8a' })
  })

  it('a lane waiting on the owner says what; a server with no lane says why', async () => {
    daemon.status = status({
      sim: null,
      lanes: [{ lane: 'android', usable: true, ready: false, needs: [{ kind: 'licence', id: 'android-sdk-license', title: 'Android SDK licence' }] }],
    })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe(
      'The owner of this server must accept the Android SDK licence first (on its card in Settings).',
    )
    expect(document.querySelector('[data-mobile-start]')).toBeNull()
    expect(tool('boot').disabled).toBe(true)
    cleanup()
    daemon.status = status({ sim: null, lanes: [{ lane: 'android', usable: false, ready: false, detail: '/dev/kvm: permission denied for user k2.' }] })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe(
      "This server can't run a device yet. /dev/kvm: permission denied for user k2.",
    )
  })

  it('a watch-only login has no Start (the daemon says canControl false)', async () => {
    daemon.status = status({ sim: null, canControl: false })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe('No device is running.')
    expect(document.querySelector('[data-mobile-tool="boot"]')).toBeNull()
    expect(document.querySelector('[data-mobile-start]')).toBeNull()
    expect(document.querySelector('[data-mobile-watching]')).not.toBeNull()
    expect(daemon.gets).not.toContain('sim/devices')
  })

  it('a sim_state event for this workspace re-reads the status at once', async () => {
    daemon.status = status({ sim: STOPPED })
    renderMobile(createStubSimStream())
    await flush()
    const before = daemon.gets.filter((g) => g === 'sim/status').length
    daemon.status = status()
    await act(async () => {
      for (const fn of simEvents.fns) fn({ kind: 'sim_state', workspace: 'p-other', sim: 's9' })
    })
    await flush()
    expect(daemon.gets.filter((g) => g === 'sim/status').length).toBe(before)
    await act(async () => {
      for (const fn of simEvents.fns) fn({ kind: 'sim_state', workspace: 'p-docs', sim: 's1' })
    })
    await flush()
    expect(daemon.gets.filter((g) => g === 'sim/status').length).toBe(before + 1)
    expect(document.querySelector('[data-mobile-canvas]')).not.toBeNull()
  })
})

describe('the running device', () => {
  it('streams the daemon’s workspace, paints frames, acks each one, also while held', async () => {
    const stub = createStubSimStream()
    const view = renderMobile(stub)
    await flush()
    expect(stub.openCount()).toBe(1)
    expect(stub.lastWorkspace()).toBe('p-docs')
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(stub.acks).toEqual([1])
    expect(painted).toEqual(['fill:824x1830'])
    expect(document.querySelector('[data-mobile-bezel]')?.getAttribute('data-mobile-bezel')).toBe('portrait')
    expect(document.querySelector('[data-mobile-isolation]')?.textContent).toMatch(/^Separate, not sealed/)
    // S5 hook: hold the frame — still acked, not painted.
    view.rerender(pane(stub, true, true))
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(stub.acks).toEqual([1, 2])
    expect(painted).toEqual(['fill:824x1830'])
  })

  it('a tap and a swipe on the canvas go to the device in points', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const el = screenEl()
    expect(el.getAttribute('data-mobile-screen')).toBe('interactive')
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 151, clientY: 101, button: 0, pointerId: 1 })
    expect(stub.inputs).toEqual([{ t: 'tap', x: 100, y: 100 }])
    fireEvent.pointerDown(el, { clientX: 200, clientY: 400, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 200, clientY: 150, button: 0, pointerId: 1 })
    expect(stub.inputs[1]).toMatchObject({ t: 'swipe', x1: 200, y1: 700, x2: 200, y2: 200 })
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
      { t: 'text', text: 'h' },
      { t: 'key', key: 'enter' },
      { t: 'button', button: 'home' },
    ])
    expect(document.activeElement).toBe(el)
    fireEvent.keyDown(el, { key: 'Escape' })
    expect(document.activeElement).not.toBe(el)
    expect(stub.inputs).toHaveLength(3)
  })

  it('Rotate, Home, Back and Take over / Hand back send stream messages', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    fireEvent.click(tool('rotate'))
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(document.querySelector('[data-mobile-bezel]')?.getAttribute('data-mobile-bezel')).toBe('landscape')
    fireEvent.click(tool('rotate'))
    fireEvent.click(tool('home'))
    fireEvent.click(tool('back'))
    fireEvent.click(tool('take-over'))
    await act(async () => {
      stub.pushServer({ t: 'hold', hold: { kind: 'takeover', by: 'rosson', idleMs: 600000 } })
    })
    // This viewer took over: no banner about itself.
    expect(document.querySelector('[data-mobile-held]')).toBeNull()
    fireEvent.click(tool('hand-back'))
    expect(stub.inputs).toEqual([
      { t: 'rotate', orientation: 'landscape-right' },
      { t: 'rotate', orientation: 'portrait' },
      { t: 'button', button: 'home' },
      { t: 'button', button: 'back' },
      { t: 'take-over' },
      { t: 'hand-back' },
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
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toMatch(/^Saved \/w\/docs\/\.k2\/sim\/shots\/s1-0001\.png/)
    await act(async () => {
      fireEvent.click(tool('stop'))
    })
    await flush()
    expect(daemon.posts.map((p) => p.route)).toEqual(['sim/screenshot', 'sim/stop'])
    expect(daemon.posts[1].body).toEqual({ workspace: '/w/docs' })
  })

  it('a person holding the device shows for other viewers; a refused input shows its reason', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushServer({ t: 'hold', hold: { kind: 'person', by: 'jules', ms: 2000 } })
    })
    expect(document.querySelector('[data-mobile-held]')?.textContent).toBe('jules is using the device')
    await act(async () => {
      stub.pushServer({ t: 'hold', hold: { kind: 'takeover', by: 'jules', idleMs: 1 } })
    })
    expect(document.querySelector('[data-mobile-held]')?.textContent).toBe('jules has taken over the device')
    await act(async () => {
      stub.pushServer({ t: 'error', code: 'sim_stopped', message: 'The device is stopped.' })
    })
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toBe('The device is stopped.')
  })
})

describe('watching and visibility', () => {
  it('a watch-only login sees the stream but sends nothing (remote Members)', async () => {
    daemon.status = status({ canControl: false })
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
    expect(tool('screenshot').disabled).toBe(true)
    expect(document.querySelector('[data-mobile-watching]')?.textContent).toBe('Watching')
  })

  it('a hidden tab opens no socket; showing it connects; hiding it closes it', async () => {
    const stub = createStubSimStream()
    const view = renderMobile(stub, false)
    await flush()
    expect(stub.openCount()).toBe(0)
    view.rerender(pane(stub, true))
    await flush()
    expect(stub.openCount()).toBe(1)
    view.rerender(pane(stub, false))
    await flush()
    expect(stub.openCount()).toBe(0)
  })
})

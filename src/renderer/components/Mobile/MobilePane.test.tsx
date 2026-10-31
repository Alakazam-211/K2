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
//  - taps and swipes go to the device in points; clicking the screen puts the
//    keyboard on it (ring + hint; keys, IME commits, ⌘V paste; ⌘W stays K2's);
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
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: vi.fn(async () => 'hello from the clipboard'),
  writeText: vi.fn(async () => {}),
}))

// The keyboard's chord key is ⌘ on a Mac (jsdom's navigator is not one).
vi.mock('@/lib/desktop-chrome', async () => {
  const actual = await vi.importActual<typeof import('@/lib/desktop-chrome')>('@/lib/desktop-chrome')
  return { ...actual, isMacPlatform: () => true }
})

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  devices: { lane: 'android', devices: [] as unknown[] },
  whoami: { owner: false, role: 'member' } as unknown,
  posts: [] as Array<{ route: string; body: unknown }>,
  gets: [] as string[],
  devicesAsked: [] as unknown[],
  devicesByPlatform: null as Record<string, unknown> | null,
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
      if (route === 'sim/devices') {
        daemon.devicesAsked.push(params?.platform ?? null)
        const p = params?.platform
        return (typeof p === 'string' && daemon.devicesByPlatform?.[p]) || daemon.devices
      }
      if (route === 'auth/whoami') return daemon.whoami
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      daemon.posts.push({ route, body })
      if (route === 'sim/boot') return { ...RUNNING, state: 'booting' }
      if (route === 'sim/stop') return { ...RUNNING, state: 'stopping' }
      if (route === 'sim/screenshot') return { path: '/w/docs/.k2/sim/shots/s1-0001.png', width: 412, height: 915 }
      if (route === 'sim/setup') return { ok: true, lane: 'android', lanes: [] }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('@/components/Design/DesignThreadColumn', () => ({
  DesignThreadColumn: (props: { projectPath: string; agentName: string; visible: boolean }) => (
    <div data-testid="design-thread-column" data-path={props.projectPath} data-agent={props.agentName} data-visible={String(props.visible)}>
      <div data-compose-bar="" />
    </div>
  ),
}))

import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { readText as readClipboardText } from '@tauri-apps/plugin-clipboard-manager'
import { MobilePane, holdBanner } from './MobilePane'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { useSettingsStore } from '@/stores/settings'
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

/** The device's keyboard sink (a hidden textarea in the screen). */
function keysEl(): HTMLElement {
  const el = document.querySelector('[data-mobile-keys]') as HTMLElement | null
  if (!el) throw new Error('no keyboard sink on the device screen')
  return el
}

/** Click the device screen: the screen's focus goes on to the sink. */
function focusDevice(): void {
  act(() => screenEl().focus())
}

function tool(name: string): HTMLButtonElement {
  const el = document.querySelector(`[data-mobile-tool="${name}"]`) as HTMLButtonElement | null
  if (!el) throw new Error(`no ${name} button`)
  return el
}

/** Start lives on the empty state (the top bar has no power toggle). */
function start(): HTMLButtonElement {
  const el = document.querySelector('[data-mobile-start]') as HTMLButtonElement | null
  if (!el) throw new Error('no Start button')
  return el
}

/** Open the rail's ⋯ menu. */
function openMore(): HTMLElement {
  fireEvent.click(tool('more'))
  const menu = document.querySelector('[data-mobile-more]') as HTMLElement | null
  if (!menu) throw new Error('the ⋯ menu did not open')
  return menu
}

/** The empty state's device list: names per platform group. */
function listed(platform: string): string[] {
  return Array.from(document.querySelectorAll(`[data-mobile-empty] [data-mobile-group="${platform}"] [data-mobile-device-name]`)).map(
    (n) => n.textContent ?? '',
  )
}

function startRow(id: string): HTMLButtonElement {
  const el = document.querySelector(`[data-mobile-empty] [data-mobile-start="${id}"]`) as HTMLButtonElement | null
  if (!el) throw new Error(`no Start for ${id}`)
  return el
}

beforeEach(() => {
  daemon.status = status()
  daemon.devices = { lane: 'android', devices: [PIXEL_9, PIXEL_8A] }
  daemon.whoami = { owner: false, role: 'member' }
  daemon.posts = []
  daemon.gets = []
  daemon.devicesAsked = []
  daemon.devicesByPlatform = null
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
  vi.useRealTimers()
  localStorage.removeItem('k2.mobile.liveTouch')
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
    // Double-click resets it.
    fireEvent.doubleClick(sep)
    const reset = JSON.parse(localStorage.getItem(mobileSplitKey('main')) ?? 'null') as Record<string, number[]> | null
    if (!reset) throw new Error('split not saved')
    expect(Object.values(reset)).toEqual([[45, 55]])
  })
})

describe('no device running', () => {
  it('a start shows the loading screen: booting → connecting the screen → gone at the first frame', async () => {
    daemon.status = status({ sim: STOPPED })
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    expect(document.querySelector('[data-mobile-boot]')).toBeNull()
    await act(async () => {
      fireEvent.click(document.querySelector('[data-mobile-start]') as HTMLButtonElement)
    })
    await flush()
    // The status still says stopped: the screen covers the gap, inside
    // the phone's outline.
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('booting')
    expect(document.querySelector('[data-mobile-frame] [data-mobile-boot]')).not.toBeNull()
    expect(document.querySelector('[data-mobile-boot-title]')?.textContent).toBe('Starting Pixel 9')
    expect(document.querySelector('[data-mobile-blocked]')).toBeNull()
    const step = (s: string): string | null =>
      document.querySelector(`[data-mobile-boot-step="${s}"]`)?.getAttribute('data-state') ?? null
    daemon.status = status({ sim: { ...RUNNING, state: 'booting' } })
    await act(async () => {
      for (const fn of simEvents.fns) fn({ kind: 'sim_state', workspace: 'p-docs', sim: 's1' })
    })
    await flush()
    expect(step('booting')).toBe('now')
    daemon.status = status()
    await act(async () => {
      for (const fn of simEvents.fns) fn({ kind: 'sim_state', workspace: 'p-docs', sim: 's1' })
    })
    await flush()
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('connecting')
    expect(step('booting')).toBe('done')
    expect(step('connecting')).toBe('now')
    // The same outline now holds the canvas, hidden until its first frame.
    expect(document.querySelector('[data-mobile-frame-screen] [data-mobile-boot]')).not.toBeNull()
    expect((document.querySelector('[data-mobile-canvas]') as HTMLElement).style.opacity).toBe('0')
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(document.querySelector('[data-mobile-boot]')).toBeNull()
    expect((document.querySelector('[data-mobile-canvas]') as HTMLElement).style.opacity).toBe('1')
  })

  it('a start that fails keeps the screen with the error and Try again boots again', async () => {
    daemon.status = status({ sim: { ...RUNNING, state: 'booting' } })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('booting')
    daemon.status = status({ sim: { ...RUNNING, state: 'failed', reason: 'emulator exited' } })
    await act(async () => {
      for (const fn of simEvents.fns) fn({ kind: 'sim_state', workspace: 'p-docs', sim: 's1' })
    })
    await flush()
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('failed')
    expect(document.querySelector('[data-mobile-boot-error]')?.textContent).toBe('The device stopped: emulator exited')
    expect(document.querySelector('[data-mobile-boot-step="booting"]')?.getAttribute('data-state')).toBe('failed')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-mobile-boot-retry]') as HTMLButtonElement)
    })
    await flush()
    expect(daemon.posts.map((p) => p.route)).toEqual(['sim/boot'])
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('booting')
  })

  it('shows the device list; a row’s Start boots that device with noTab and remembers it', async () => {
    daemon.status = status({ sim: STOPPED })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe('No device is running.')
    expect(listed('android')).toEqual(['Pixel 9', 'Pixel 8a'])
    expect(document.querySelector('[data-mobile-device-label]')?.textContent).toBe('Pixel 9')
    expect(daemon.posts).toEqual([])
    await act(async () => {
      fireEvent.click(startRow('pixel_8a'))
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
      'The owner of this server must accept the Android SDK licence first.',
    )
    expect(startRow('pixel_9').disabled).toBe(true)
    expect(document.querySelector('[data-mobile-rail]')).toBeNull()
    // One click asks the daemon, which puts the owner's card on the Thread.
    await act(async () => {
      fireEvent.click(document.querySelector('[data-mobile-need-action="licence"]') as HTMLButtonElement)
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/boot', body: { workspace: '/w/docs', device: 'pixel_9', platform: 'android', noTab: true } }])
    cleanup()
    daemon.status = status({ sim: null, lanes: [{ lane: 'android', usable: false, ready: false, detail: '/dev/kvm: permission denied for user k2.' }] })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe(
      "This server can't run a device yet. /dev/kvm: permission denied for user k2.",
    )
  })

  it("the owner's stuck box: the licence the setup failed on is named, with why, and Show the licence asks for its card", async () => {
    const why =
      "system-images;android-35;google_apis;arm64-v8a needs the licence 'android-sdk-arm-dbt-license', which the owner hasn't accepted"
    daemon.status = status({
      sim: null,
      canSetup: true,
      lanes: [
        {
          lane: 'android',
          usable: true,
          ready: false,
          needs: [
            { kind: 'licence', id: 'android-sdk-arm-dbt-license', title: "Android SDK licence 'android-sdk-arm-dbt-license'" },
            { kind: 'install', what: 'Android system image (from Google)', bytes: 2_200_000_000 },
          ],
          detail: `the last setup failed: ${why}`,
          setup: { running: false, done: 0, total: 0, error: why },
        },
      ],
    })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe(
      "The owner of this server must accept the Android SDK licence 'android-sdk-arm-dbt-license' first.",
    )
    expect(document.querySelector('[data-mobile-empty] [data-mobile-setup-error]')?.textContent).toBe(`The last setup stopped: ${why}`)
    expect(startRow('pixel_9').disabled).toBe(true)
    await act(async () => {
      fireEvent.click(document.querySelector('[data-mobile-need-action="licence"]') as HTMLButtonElement)
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/boot', body: { workspace: '/w/docs', device: 'pixel_9', platform: 'android', noTab: true } }])
  })

  it('a setup that failed for another reason: the owner retries it from the tab', async () => {
    const why = 'download https://dl.google.com/android/repository/emulator.zip: connection reset'
    daemon.status = status({
      sim: null,
      canSetup: true,
      lanes: [
        {
          lane: 'android',
          usable: true,
          ready: false,
          needs: [{ kind: 'install', what: 'Android emulator (from Google)', bytes: 2_200_000_000 }],
          detail: `the last setup failed: ${why}`,
          setup: { running: false, done: 5, total: 100, error: why },
        },
      ],
    })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-blocked]')?.textContent).toBe(`The device setup didn't finish: ${why}`)
    await act(async () => {
      fireEvent.click(document.querySelector('[data-mobile-need-action="setup-failed"]') as HTMLButtonElement)
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/setup', body: { lane: 'android' } }])
  })

  it('a server with no iOS lane (Linux) offers Android only and says what an iPhone needs', async () => {
    daemon.whoami = { owner: true, role: 'owner' }
    daemon.status = status({ sim: STOPPED })
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-platform]')).toBeNull()
    expect(document.querySelector('[data-mobile-iphone-note]')?.textContent).toBe('iPhone needs a Mac (this computer or a Mac compute node)')
    expect(daemon.devicesAsked).toEqual(['android'])
    await act(async () => {
      fireEvent.click(start())
    })
    await flush()
    expect(daemon.posts.map((p) => (p.body as { platform?: string }).platform)).toEqual(['android'])
  })

  it('the list groups only usable platforms; an iPhone boots as platform ios, never the lane name', async () => {
    const IPHONE = { id: 'iphone_16', name: 'iPhone 16', widthPt: 393, heightPt: 852, scale: 3, os: 'ios-18', default: true }
    daemon.whoami = { owner: true, role: 'owner' }
    daemon.status = status({
      sim: null,
      lanes: [
        { lane: 'android', usable: true, ready: true },
        { lane: 'ios', usable: false, ready: false, detail: 'Xcode is not installed.' },
        { lane: 'ios-node', usable: true, ready: true },
      ],
    })
    daemon.devicesByPlatform = { ios: { lane: 'ios-node', devices: [IPHONE] } }
    renderMobile(createStubSimStream())
    await flush()
    expect(document.querySelector('[data-mobile-iphone-note]')).toBeNull()
    expect(Array.from(document.querySelectorAll('[data-mobile-empty] [data-mobile-group]')).map((g) => g.getAttribute('data-mobile-group'))).toEqual([
      'android',
      'ios',
    ])
    expect(daemon.devicesAsked).toEqual(['android', 'ios'])
    expect(listed('ios')).toEqual(['iPhone 16'])
    await act(async () => {
      fireEvent.click(startRow('iphone_16'))
    })
    await flush()
    expect(daemon.posts).toEqual([{ route: 'sim/boot', body: { workspace: '/w/docs', device: 'iphone_16', platform: 'ios', noTab: true } }])
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
    expect(openMore().querySelector('[data-mobile-about]')?.textContent).toMatch(/Separate, not sealed/)
    fireEvent.click(tool('more'))
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

  it('live touch (on by default): a drag follows the finger as touch down / move / up; a clean tap stays a tap', async () => {
    let now = 0
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const el = screenEl()
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    expect(document.querySelector('[data-mobile-touch-dot]')?.getAttribute('data-mobile-touch-dot')).toBe('down')
    now = 16
    fireEvent.pointerMove(el, { clientX: 150, clientY: 130, pointerId: 1 })
    now = 32
    fireEvent.pointerMove(el, { clientX: 150, clientY: 160, pointerId: 1 })
    // Held still before lifting: a stop, so the up carries no velocity.
    now = 120
    fireEvent.pointerUp(el, { clientX: 150, clientY: 200, button: 0, pointerId: 1 })
    expect(stub.inputs).toEqual([
      { t: 'touch', phase: 'down', x: 100, y: 100 },
      { t: 'touch', phase: 'move', x: 100, y: 220 },
      { t: 'touch', phase: 'up', x: 100, y: 300 },
    ])
    expect(document.querySelector('[data-mobile-touch-dot]')?.getAttribute('data-mobile-touch-dot')).toBe('up')
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.pointerUp(el, { clientX: 151, clientY: 101, button: 0, pointerId: 1 })
    expect(stub.inputs[3]).toEqual({ t: 'tap', x: 100, y: 100 })
  })

  it('live touch: a flick lifts with its release velocity (points per second) so the device coasts', async () => {
    let now = 0
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const el = screenEl()
    fireEvent.pointerDown(el, { clientX: 150, clientY: 300, button: 0, pointerId: 1 })
    now = 16
    fireEvent.pointerMove(el, { clientX: 150, clientY: 270, pointerId: 1 })
    now = 32
    fireEvent.pointerMove(el, { clientX: 150, clientY: 240, pointerId: 1 })
    now = 40
    fireEvent.pointerUp(el, { clientX: 150, clientY: 240, button: 0, pointerId: 1 })
    // 60 px up in 32 ms on a 2 pt/px canvas: -3750 pt/s.
    expect(stub.inputs).toEqual([
      { t: 'touch', phase: 'down', x: 100, y: 500 },
      { t: 'touch', phase: 'move', x: 100, y: 380 },
      { t: 'touch', phase: 'up', x: 100, y: 380, vx: 0, vy: -3750 },
    ])
  })

  it('live touch switched off: a drag is one swipe on release; the touch dot still shows', async () => {
    localStorage.setItem('k2.mobile.liveTouch', '0')
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const el = screenEl()
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    fireEvent.pointerMove(el, { clientX: 150, clientY: 160, pointerId: 1 })
    expect(document.querySelector('[data-mobile-touch-dot]')?.getAttribute('data-mobile-touch-dot')).toBe('down')
    expect(stub.inputs).toEqual([])
    fireEvent.pointerUp(el, { clientX: 150, clientY: 200, button: 0, pointerId: 1 })
    expect(stub.inputs).toHaveLength(1)
    expect(stub.inputs[0]).toMatchObject({ t: 'swipe', x1: 100, y1: 100, x2: 100, y2: 300 })
  })

  it('clicking the screen puts the keyboard on the device: a ring and a hint; Esc lets go', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    expect(document.querySelector('[data-mobile-focus-ring]')).toBeNull()
    expect(document.querySelector('[data-mobile-typing-hint]')).toBeNull()
    focusDevice()
    expect(document.activeElement).toBe(keysEl())
    expect(document.querySelector('[data-mobile-focus-ring]')).not.toBeNull()
    expect(document.querySelector('[data-mobile-typing-hint]')?.textContent).toBe('Typing into the device · Esc to stop')
    const esc = fireEvent.keyDown(keysEl(), { key: 'Escape' })
    expect(esc).toBe(false)
    expect(document.activeElement).not.toBe(keysEl())
    expect(document.querySelector('[data-mobile-focus-ring]')).toBeNull()
    expect(document.querySelector('[data-mobile-typing-hint]')).toBeNull()
    expect(stub.inputs).toEqual([])
  })

  it('a mouse click on the screen puts the keyboard there (the tap still goes to the device)', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const el = screenEl()
    fireEvent.pointerDown(el, { clientX: 150, clientY: 100, button: 0, pointerId: 1 })
    const md = fireEvent.mouseDown(el, { button: 0 })
    expect(md).toBe(false) // the browser doesn't move focus off the keyboard
    fireEvent.pointerUp(el, { clientX: 151, clientY: 101, button: 0, pointerId: 1 })
    expect(document.activeElement).toBe(keysEl())
    expect(document.querySelector('[data-mobile-typing-hint]')).not.toBeNull()
    expect(stub.inputs).toEqual([{ t: 'tap', x: 100, y: 100 }])
  })

  it('clicking outside the device lets the keyboard go', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    expect(document.querySelector('[data-mobile-typing-hint]')).not.toBeNull()
    const outside = document.createElement('button')
    document.body.appendChild(outside)
    act(() => outside.focus())
    expect(document.querySelector('[data-mobile-typing-hint]')).toBeNull()
    expect(document.querySelector('[data-mobile-focus-ring]')).toBeNull()
    fireEvent.keyDown(outside, { key: 'a' })
    expect(stub.inputs).toEqual([])
    outside.remove()
  })

  it('keys go to the device with Shift/Option; ⌘W and other K2 chords pass through untouched', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    const el = keysEl()
    fireEvent.keyDown(el, { key: 'h' })
    fireEvent.keyDown(el, { key: 'I', shiftKey: true })
    fireEvent.keyDown(el, { key: 'Enter' })
    fireEvent.keyDown(el, { key: 'Backspace' })
    fireEvent.keyDown(el, { key: 'ArrowLeft', shiftKey: true })
    fireEvent.keyDown(el, { key: 'ArrowRight', altKey: true })
    const tab = fireEvent.keyDown(el, { key: 'Tab' })
    expect(tab).toBe(false) // Tab types into the device, focus stays
    fireEvent.keyDown(el, { key: 'a', metaKey: true })
    fireEvent.keyDown(el, { key: 'z', metaKey: true })
    fireEvent.keyDown(el, { key: 'H', metaKey: true, shiftKey: true })
    for (const chord of ['w', 't', 'l', 'k', '1']) {
      expect(fireEvent.keyDown(el, { key: chord, metaKey: true }), `⌘${chord} is not prevented`).toBe(true)
    }
    expect(stub.inputs).toEqual([
      { t: 'text', text: 'h' },
      { t: 'text', text: 'I' },
      { t: 'key', key: 'enter' },
      { t: 'key', key: 'backspace' },
      { t: 'key', key: 'arrow-left', shift: true },
      { t: 'key', key: 'arrow-right', alt: true },
      { t: 'key', key: 'tab' },
      { t: 'edit', op: 'select-all' },
      { t: 'edit', op: 'undo' },
      { t: 'button', button: 'home' },
    ])
    expect(document.activeElement).toBe(el)
  })

  it('a ⌘ chord K2 keeps still reaches K2’s window handlers', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    const seen: string[] = []
    const onKey = (e: KeyboardEvent): void => {
      if (e.metaKey) seen.push(e.key)
    }
    window.addEventListener('keydown', onKey)
    fireEvent.keyDown(keysEl(), { key: 'w', metaKey: true })
    fireEvent.keyDown(keysEl(), { key: 'v', metaKey: true })
    window.removeEventListener('keydown', onKey)
    expect(seen).toEqual(['w'])
  })

  it('an input method composes in K2 and its commit goes to the device once', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    const el = keysEl() as HTMLTextAreaElement
    fireEvent.compositionStart(el, { data: '' })
    const k1 = fireEvent.keyDown(el, { key: 'n', isComposing: true, keyCode: 229 })
    expect(k1).toBe(true) // the IME keeps the key
    fireEvent.compositionUpdate(el, { data: 'に' })
    fireEvent.input(el, { target: { value: 'に' } })
    fireEvent.keyDown(el, { key: 'Process', keyCode: 229 })
    fireEvent.compositionUpdate(el, { data: '日本' })
    fireEvent.input(el, { target: { value: '日本' } })
    expect(stub.inputs).toEqual([]) // nothing half-composed
    fireEvent.compositionEnd(el, { data: '日本' })
    // Safari sends the committing Enter after compositionend with keyCode 229.
    fireEvent.keyDown(el, { key: 'Enter', keyCode: 229 })
    fireEvent.input(el, { target: { value: '日本' } })
    expect(stub.inputs).toEqual([{ t: 'text', text: '日本' }])
    expect(el.value).toBe('')
    // A dead key (Option+E, then E) is a composition too.
    fireEvent.keyDown(el, { key: 'Dead', altKey: true })
    fireEvent.compositionStart(el, { data: '' })
    fireEvent.compositionEnd(el, { data: 'é' })
    expect(stub.inputs).toEqual([{ t: 'text', text: '日本' }, { t: 'text', text: 'é' }])
    // A cancelled composition sends nothing.
    fireEvent.compositionStart(el, { data: '' })
    fireEvent.keyDown(el, { key: 'Escape', isComposing: true })
    fireEvent.compositionEnd(el, { data: '' })
    expect(stub.inputs).toHaveLength(2)
    expect(document.activeElement).toBe(el)
  })

  it('the clipboard is read only on ⌘V, and goes to the device as one paste', async () => {
    const clip = vi.mocked(readClipboardText)
    clip.mockClear()
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    const el = keysEl()
    fireEvent.keyDown(el, { key: 'v' })
    fireEvent.keyDown(el, { key: 'c', metaKey: true })
    fireEvent.keyDown(el, { key: 'x', metaKey: true })
    fireEvent.keyDown(el, { key: 'V', metaKey: true, shiftKey: true })
    fireEvent.keyDown(el, { key: 'Enter' })
    expect(clip).not.toHaveBeenCalled()
    const cmdV = fireEvent.keyDown(el, { key: 'v', metaKey: true })
    expect(cmdV).toBe(false)
    await flush()
    expect(clip).toHaveBeenCalledTimes(1)
    expect(stub.inputs.slice(-1)).toEqual([{ t: 'paste', text: 'hello from the clipboard' }])
    // The native paste event that may follow ⌘V is not a second paste.
    fireEvent.paste(el, { clipboardData: { getData: () => 'hello from the clipboard' } })
    expect(stub.inputs.filter((i) => i.t === 'paste')).toHaveLength(1)
    expect(clip).toHaveBeenCalledTimes(1)
  })

  it('a menu Paste (no ⌘V) pastes what the browser hands over; too much is refused with a note', async () => {
    const clip = vi.mocked(readClipboardText)
    clip.mockClear()
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    focusDevice()
    fireEvent.paste(keysEl(), { clipboardData: { getData: (type: string) => (type === 'text/plain' ? 'pasted text' : '') } })
    expect(stub.inputs).toEqual([{ t: 'paste', text: 'pasted text' }])
    expect(clip).not.toHaveBeenCalled()
    clip.mockResolvedValueOnce('x'.repeat(64 * 1024 + 1))
    fireEvent.keyDown(keysEl(), { key: 'v', metaKey: true })
    await flush()
    expect(stub.inputs).toHaveLength(1)
    expect(document.querySelector('[data-mobile-key-notice]')?.textContent).toBe('The clipboard text is over 64 KB; paste less at a time.')
  })

  it('the ⋯ menu says what the keyboard does', async () => {
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    const help = openMore().querySelector('[data-mobile-keyboard-help]')?.textContent ?? ''
    expect(help).toMatch(/Click the screen, then type/)
    expect(help).toMatch(/Paste this computer’s clipboard/)
    expect(help).toMatch(/Stop typing into the device/)
  })

  it('the rail (Rotate, Home, Back, Recents, Dark mode) and Take over / Hand back send stream messages', async () => {
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
    fireEvent.click(tool('recents'))
    fireEvent.click(tool('appearance'))
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
      { t: 'button', button: 'app-switch' },
      { t: 'appearance', appearance: 'dark' },
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
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toBe('Saved /w/docs/.k2/sim/shots/s1-0001.png')
    expect(document.querySelector('[data-mobile-notice-tone]')?.getAttribute('data-mobile-notice-tone')).toBe('success')
    // Attach to Thread: the Thread's message box beside the device gets the
    // shot the way a file drop gives it one.
    const got: unknown[] = []
    document.querySelector('[data-mobile-chat] [data-compose-bar]')?.addEventListener('k2so:compose-insert', (e) => {
      got.push((e as CustomEvent).detail)
    })
    fireEvent.click(document.querySelector('[data-mobile-notice-action="attach"]') as HTMLButtonElement)
    expect(got).toEqual([{ data: '/w/docs/.k2/sim/shots/s1-0001.png', paths: ['/w/docs/.k2/sim/shots/s1-0001.png'], uploaded: true }])
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toBe('Attached to your Thread message.')
    fireEvent.click(document.querySelector('[data-mobile-notice-dismiss]') as HTMLButtonElement)
    expect(document.querySelector('[data-mobile-notice]')).toBeNull()
    openMore()
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

describe('the hold banner: the holder sees no "is using" pop-up', () => {
  const OWNER = { principal: 'owner', aliases: ['rosson'] }
  const JULES = { principal: 'jules', aliases: [] }

  afterEach(() => {
    resetThreadViewersForTests()
    useSettingsStore.setState({ ownerDisplayName: '' })
  })

  it('pure: the holder sees nothing; others see the holder by name', () => {
    const owner = { kind: 'person' as const, by: 'owner', ms: 2000 }
    expect(holdBanner(owner, false, OWNER)).toBeNull()
    // The owner's linked login holding it is the owner too.
    expect(holdBanner({ kind: 'person', by: 'Rosson', ms: 2000 }, false, OWNER)).toBeNull()
    expect(holdBanner({ kind: 'person', by: 'jules', ms: 2000 }, false, JULES)).toBeNull()
    expect(holdBanner(owner, false, JULES, 'Rosson Long')).toBe('Rosson Long is using the device')
    expect(holdBanner(owner, false, JULES, '')).toBe("The server's owner is using the device")
    expect(holdBanner(owner, false, null, '')).toBe("The server's owner is using the device")
    expect(holdBanner({ kind: 'person', by: 'jules', ms: 2000 }, false, OWNER)).toBe('jules is using the device')
    // Take-over: others keep the text; the taker (Hand back) sees nothing.
    expect(holdBanner({ kind: 'takeover', by: 'jules', idleMs: 1 }, false, OWNER)).toBe('jules has taken over the device')
    expect(holdBanner({ kind: 'takeover', by: 'owner', idleMs: 1 }, true, OWNER)).toBeNull()
    expect(holdBanner({ kind: 'takeover', by: 'owner', idleMs: 1 }, false, OWNER)).toBe('You have taken over the device in another window')
    expect(holdBanner({ kind: 'free' }, false, OWNER)).toBeNull()
  })

  it('the owner using the device on this computer sees no pill', async () => {
    setThreadViewerForTests(primaryScope(), OWNER)
    useSettingsStore.setState({ ownerDisplayName: 'Rosson Long' })
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushServer({ t: 'hold', hold: { kind: 'person', by: 'owner', ms: 2000 } })
    })
    expect(document.querySelector('[data-mobile-held]')).toBeNull()
    expect(document.body.textContent).not.toContain('is using the device')
  })

  it('another viewer sees the owner by display name', async () => {
    setThreadViewerForTests(primaryScope(), JULES)
    useSettingsStore.setState({ ownerDisplayName: 'Rosson Long' })
    const stub = createStubSimStream()
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushServer({ t: 'hold', hold: { kind: 'person', by: 'owner', ms: 2000 } })
    })
    expect(document.querySelector('[data-mobile-held]')?.textContent).toBe('Rosson Long is using the device')
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
    // No keyboard for a watcher: no sink to type into, no ring, no hint.
    act(() => el.focus())
    expect(document.querySelector('[data-mobile-keys]')).toBeNull()
    fireEvent.keyDown(document.activeElement ?? el, { key: 'b' })
    fireEvent.keyDown(document.activeElement ?? el, { key: 'v', metaKey: true })
    expect(document.querySelector('[data-mobile-typing-hint]')).toBeNull()
    expect(stub.inputs).toEqual([])
    // No rail for a watcher: no device buttons, no screenshot, no Stop.
    expect(document.querySelector('[data-mobile-rail]')).toBeNull()
    for (const name of ['rotate', 'home', 'back', 'take-over', 'stop', 'screenshot', 'more']) {
      expect(document.querySelector(`[data-mobile-tool="${name}"]`)).toBeNull()
    }
    expect(document.querySelector('[data-mobile-watching]')?.textContent).toBe('Watching')
  })

  it('a reconnect keeps the last frame, dimmed, and says Reconnecting only after 2 s', async () => {
    const stub = createStubSimStream()
    const view = renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    const screenBox = document.querySelector('[data-mobile-frame-screen]') as HTMLElement
    expect(screenBox.style.filter).toBe('')
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    view.rerender(pane(stub, false))
    await flush()
    view.rerender(pane(stub, true))
    await flush()
    // No boot screen, no blank: the last frame stays, dimmed.
    expect(document.querySelector('[data-mobile-boot]')).toBeNull()
    expect((document.querySelector('[data-mobile-canvas]') as HTMLElement).style.opacity).toBe('1')
    expect((document.querySelector('[data-mobile-frame-screen]') as HTMLElement).style.filter).not.toBe('')
    expect(document.querySelector('[data-mobile-stream-note]')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(2000)
    })
    expect(document.querySelector('[data-mobile-stream-note]')?.textContent).toBe('Reconnecting…')
    vi.useRealTimers()
    await act(async () => {
      stub.pushFrame()
    })
    await flush()
    expect(document.querySelector('[data-mobile-stream-note]')).toBeNull()
    expect((document.querySelector('[data-mobile-frame-screen]') as HTMLElement).style.filter).toBe('')
  })

  it('a narrow pane switches Thread | Device instead of splitting', async () => {
    const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
      configurable: true,
      get(this: HTMLElement) {
        return this.hasAttribute('data-mobile-pane') ? 600 : 0
      },
    })
    try {
      renderMobile(createStubSimStream())
      await flush()
      expect(document.querySelector('[data-mobile-narrow]')).not.toBeNull()
      expect(document.querySelector('[data-mobile-split]')).toBeNull()
      const chat = document.querySelector('[data-mobile-chat]') as HTMLElement
      const deviceCol = (document.querySelector('[data-testid="mobile-device"]') as HTMLElement).parentElement as HTMLElement
      expect(chat.classList.contains('hidden')).toBe(true)
      expect(deviceCol.classList.contains('hidden')).toBe(false)
      fireEvent.click(document.querySelector('[data-mobile-narrow-view="thread"]') as HTMLButtonElement)
      expect(chat.classList.contains('hidden')).toBe(false)
      expect(deviceCol.classList.contains('hidden')).toBe(true)
    } finally {
      if (width) Object.defineProperty(HTMLElement.prototype, 'clientWidth', width)
    }
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

describe('trackpad and mouse gestures (0.46.7)', () => {
  // The canvas box is 206 × 457.5 at (100, 50); the stub screen is 412 ×
  // 915 pt, so 1 CSS px = 2 pt. The safe band is x 61.8–350.2, y 183–732.
  async function running(caps?: { multiTouch: boolean; fling: boolean }): Promise<{ stub: StubSimStream; el: HTMLElement }> {
    const stub = createStubSimStream(undefined, caps ? { input: caps } : {})
    renderMobile(stub)
    await flush()
    await act(async () => {
      stub.pushFrame()
    })
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'requestAnimationFrame', 'cancelAnimationFrame'] })
    return { stub, el: screenEl() }
  }

  function wheel(el: HTMLElement, init: WheelEventInit): boolean {
    return el.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, clientX: 150, clientY: 250, ...init }))
  }

  function gesture(el: HTMLElement, type: string, scale: number): void {
    const e = new Event(type, { bubbles: true, cancelable: true })
    Object.assign(e, { scale, clientX: 150, clientY: 250 })
    act(() => {
      el.dispatchEvent(e)
    })
  }

  function tick(ms: number): void {
    act(() => {
      vi.advanceTimersByTime(ms)
    })
  }

  it('two-finger scroll / the wheel drags the device: down under the pointer, one move per frame against the delta, up when quiet', async () => {
    const { stub, el } = await running()
    expect(wheel(el, { deltaY: 20 })).toBe(false) // the page doesn't scroll
    wheel(el, { deltaY: 30 })
    expect(stub.inputs).toEqual([{ t: 'touch', phase: 'down', x: 100, y: 400 }])
    tick(16)
    // 50 CSS px of scroll = 100 pt: the finger moves up by it.
    expect(stub.inputs[1]).toEqual({ t: 'touch', phase: 'move', x: 100, y: 300 })
    wheel(el, { deltaY: 10 })
    tick(16)
    expect(stub.inputs[2]).toEqual({ t: 'touch', phase: 'move', x: 100, y: 280 })
    expect(stub.inputs).toHaveLength(3)
    tick(200)
    expect(stub.inputs[3]).toEqual({ t: 'touch', phase: 'up', x: 100, y: 280 })
    expect(stub.inputs).toHaveLength(4)
  })

  it('trackpad pinch (WebKit gesture events) is two fingers down / move / up where the device takes them', async () => {
    const { stub, el } = await running({ multiTouch: true, fling: true })
    gesture(el, 'gesturestart', 1)
    // Two fingers a quarter of the short side (103 pt) apart around the pointer.
    expect(stub.inputs).toEqual([{ t: 'touch', phase: 'down', x: 48.5, y: 400, x2: 151.5, y2: 400 }])
    gesture(el, 'gesturechange', 2)
    tick(16)
    expect(stub.inputs[1]).toEqual({ t: 'touch', phase: 'move', x: 1, y: 400, x2: 203, y2: 400 })
    gesture(el, 'gestureend', 2)
    expect(stub.inputs[2]).toEqual({ t: 'touch', phase: 'up', x: 1, y: 400, x2: 203, y2: 400 })
    expect(stub.inputs).toHaveLength(3)
  })

  it('Ctrl+wheel pinches too, and ends when the wheel goes quiet', async () => {
    const { stub, el } = await running({ multiTouch: true, fling: true })
    wheel(el, { deltaY: -100 / 3, ctrlKey: true })
    expect(stub.inputs[0]).toMatchObject({ t: 'touch', phase: 'down', x2: 151.5 })
    tick(16)
    expect(stub.inputs[1]).toMatchObject({ t: 'touch', phase: 'move' })
    const m = stub.inputs[1] as { x: number; x2: number }
    expect(m.x2 - m.x).toBeCloseTo(103 * Math.exp(1 / 3), 0)
    tick(300)
    expect(stub.inputs[2]).toMatchObject({ t: 'touch', phase: 'up' })
    expect(stub.inputs).toHaveLength(3)
  })

  it('a device without multi-touch (iOS, or an older daemon) says pinch is not available and sends nothing', async () => {
    const { stub, el } = await running()
    gesture(el, 'gesturestart', 1)
    gesture(el, 'gesturechange', 1.5)
    gesture(el, 'gestureend', 1.5)
    wheel(el, { deltaY: -20, ctrlKey: true })
    tick(400)
    expect(stub.inputs).toEqual([])
    expect(document.querySelector('[data-mobile-notice]')?.textContent).toBe("Pinch isn't available on this device yet.")
    // ⌥-drag the same.
    fireEvent.pointerDown(el, { clientX: 150, clientY: 250, button: 0, pointerId: 1, altKey: true })
    fireEvent.pointerMove(el, { clientX: 160, clientY: 270, pointerId: 1, altKey: true })
    fireEvent.pointerUp(el, { clientX: 160, clientY: 270, button: 0, pointerId: 1, altKey: true })
    tick(16)
    expect(stub.inputs).toEqual([])
  })

  it('⌥-drag pinches with the second finger mirrored around the centre (like Xcode)', async () => {
    const { stub, el } = await running({ multiTouch: true, fling: true })
    fireEvent.pointerDown(el, { clientX: 150, clientY: 250, button: 0, pointerId: 1, altKey: true })
    expect(stub.inputs).toEqual([{ t: 'touch', phase: 'down', x: 100, y: 400, x2: 312, y2: 515 }])
    fireEvent.pointerMove(el, { clientX: 140, clientY: 240, pointerId: 1, altKey: true })
    fireEvent.pointerUp(el, { clientX: 140, clientY: 240, button: 0, pointerId: 1, altKey: true })
    expect(stub.inputs.slice(1)).toEqual([
      { t: 'touch', phase: 'move', x: 80, y: 380, x2: 332, y2: 535 },
      { t: 'touch', phase: 'up', x: 80, y: 380, x2: 332, y2: 535 },
    ])
  })

  it('⋯ → Show input latency puts the readout over the stage (off by default)', async () => {
    await running()
    expect(document.querySelector('[data-mobile-latency]')).toBeNull()
    const item = openMore().querySelector('[data-mobile-menu="latency"]') as HTMLButtonElement
    expect(item.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(item)
    expect(localStorage.getItem('k2.mobile.latencyHud')).toBe('1')
    expect(document.querySelector('[data-mobile-latency]')?.textContent).toMatch(/^input → frame — · \d+ fps$/)
  })
})

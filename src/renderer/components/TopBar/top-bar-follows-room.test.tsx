// @vitest-environment jsdom
//
// 0.43.2 item 3 (prd-home-seamless-0432 Z14–Z20, vs-live Z34–Z36, T3.1–T3.5):
// the Agents/Home top bar follows a focused remote Home room. For every
// element this switches focus to a room on server B and back, and checks
// which server it shows and where its requests go. A request for B's room
// that lands on the window's server (MS5) fails the test.

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createStore } from 'zustand/vanilla'
import type { ServerScope } from '@/kessel/server-scope'

interface Call {
  scope: ServerScope
  route: string
  body?: unknown
}

const h = vi.hoisted(() => ({
  gets: [] as Call[],
  posts: [] as Call[],
  /** route → (scope) → body or thrown error */
  answer: null as null | ((scope: ServerScope, route: string, body?: unknown) => unknown),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

// The request layer: record the scope of every call. POSTs go through the
// real room-write gate first, exactly as `daemonCliPost` does.
vi.mock('@/lib/daemon-cli', async () => {
  const { assertScopeMayPost } = await import('@/kessel/server-scope')
  const respond = async (scope: ServerScope, route: string, body?: unknown): Promise<unknown> => {
    if (!h.answer) throw new Error(`no answer set for ${route}`)
    const out = h.answer(scope, route, body)
    if (out instanceof Error) throw out
    return out
  }
  return {
    daemonCliGet: async (scope: ServerScope, route: string) => {
      h.gets.push({ scope, route })
      return respond(scope, route)
    },
    daemonCliGetText: async () => '',
    daemonCliPost: async (scope: ServerScope, route: string, body?: unknown) => {
      assertScopeMayPost(scope, route)
      h.posts.push({ scope, route, body })
      return respond(scope, route, body)
    },
  }
})

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) => sel({ openSettings: () => {} }),
}))
vi.mock('@/lib/remote-session', () => ({ reviveRemoteSession: vi.fn() }))
// Static or window-only elements: not what this file checks.
vi.mock('@/components/Timer/TimerButton', () => ({ default: () => <span data-testid="timer-stub" /> }))
vi.mock('@/components/CheatSheet/K2NounsCheatSheet', () => ({ default: () => null }))

import {
  primaryScope,
  remoteRoomScope,
  scopeForHost,
  viewOnlyScope,
  assertScopeMayWrite,
  ViewOnlyWriteError,
  __resetServerScopesForTests,
} from '@/kessel/server-scope'
import { ROOM_WRITE_ROUTES } from '@/kessel/room-writes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { usePageViewStore } from '@/stores/page-view'
import { useWindowRoomStore, __resetWindowRoomForTests } from '@/stores/window-room'
import { usePresenceStore } from '@/stores/presence'
import { setPoolStatusSource, type PoolHostStatus } from '@/lib/pool-hooks'
import { USAGE_CACHE_POLL_MS, resetSubscriptionUsageForTests, usageEntryFor } from '@/stores/subscription-usage'
import { keepAwakeEntryFor, resetKeepAwakeForTests } from '@/stores/keep-awake'
import { useServerSwitcherStore } from '@/stores/server-switcher'
import type { PresenceView } from '@/stores/server-view'
import type { Room } from '@/stores/room'
import type { KeepAwakeStatus } from '@/lib/keep-awake'
import { topBarTargetFor, keepAwakeMayChange, roomServerState, WINDOW_TOP_BAR_KEY } from './top-bar-scope'
import TopBarUtilities from './TopBarUtilities'
import ServerSwitcher from './ServerSwitcher'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: 59_998,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}
const C: ConnectHost = { ...B, id: 'id-c', label: 'C', port: 59_997, token: 'tok-c' }

const B_KEY = '127.0.0.1:59998'

function usageDoc(used: number): unknown {
  return {
    harnesses: [
      {
        harness: 'claude',
        plan: 'Max',
        windows: [{ label: 'Weekly', used, resetsAt: '2099-01-01T00:00:00Z' }],
        checkedAt: new Date().toISOString(),
        status: '',
      },
    ],
  }
}

function keepAwake(label: string, mode: 'off' | 'working' | 'always'): { keepAwake: KeepAwakeStatus } {
  return {
    keepAwake: {
      mode,
      state: mode === 'off' ? 'off' : 'waiting',
      label,
      detail: `${label} detail`,
      held: false,
      lidHeld: false,
      workingSessions: 0,
      powerSource: { onAc: true, batteryPercent: null },
      batteryFloorPercent: 20,
      alsoOnBattery: false,
      lidClosed: false,
      lidSwitch: true,
      lidSetup: 'needs_setup',
      lidSetupDetail: 'x',
      canSetUp: true,
      canApproveLid: true,
      canChange: true,
      lidDialogDeclined: false,
      platform: 'macos',
    },
  }
}

const isWindow = (scope: ServerScope): boolean => scope === primaryScope()
const isB = (scope: ServerScope): boolean => !scope.isPrimary && scope.hostKey === B_KEY

/** Answers: the window's server and B say different things. */
function defaultAnswer(scope: ServerScope, route: string, body?: unknown): unknown {
  const who = isWindow(scope) ? 'window' : isB(scope) ? 'b' : null
  if (!who) return new Error(`request to an unexpected server ${scope.id} (${route})`)
  switch (route) {
    case 'usage/subscriptions':
    case 'usage/subscriptions/refresh':
      return usageDoc(who === 'window' ? 0.1 : 0.42)
    case 'power/status':
      return who === 'window' ? keepAwake('Window awake', 'off') : keepAwake('B awake', 'working')
    case 'power/keep-awake': {
      const mode = (body as { mode?: 'off' | 'working' | 'always' }).mode ?? 'off'
      return keepAwake(who === 'window' ? 'Window awake' : 'B awake', mode)
    }
    case 'presence/roster':
      return { roster: [] }
    case 'auth/whoami':
      return { role: 'owner' }
    default:
      return new Error(`unexpected route ${route}`)
  }
}

const poolStore = createStore<{ entries: Record<string, PoolHostStatus> }>(() => ({ entries: {} }))
function setPool(status: Partial<PoolHostStatus> | null): void {
  poolStore.setState({
    entries: status
      ? { [B_KEY]: { reach: 'live', auth: 'ok', role: 'member', checkedAt: 1, ...status } }
      : {},
  })
}

function user(name: string, role = 'member'): PresenceView['roster'][number] {
  return { user: name, role, windowCount: 1, connectedAt: 0, workspaces: [] } as unknown as PresenceView['roster'][number]
}

/** A pinned room on B (the parts the top bar reads). */
function roomOnB(opts: { readOnly?: boolean } = {}): Room {
  const base = scopeForHost(B)
  const scope = opts.readOnly ? viewOnlyScope(base) : remoteRoomScope(base)
  const presence = createStore<PresenceView>(() => ({ roster: [user('zed'), user('yan')], supported: true }))
  return {
    key: `${B_KEY}|p1:w1`,
    isPrimary: false,
    scope,
    readOnly: opts.readOnly === true,
    presence,
  } as unknown as Room
}

const primaryRoomStub = { key: 'primary', isPrimary: true, scope: primaryScope() } as unknown as Room

function focus(room: Room, page: 'home' | 'agents' = 'home'): void {
  act(() => {
    usePageViewStore.setState({ page })
    useWindowRoomStore.setState({ shown: [room], focused: room })
  })
}

function backToAgents(): void {
  act(() => {
    usePageViewStore.setState({ page: 'agents' })
    useWindowRoomStore.setState({ shown: [primaryRoomStub], focused: primaryRoomStub })
  })
}

function bar(): ReturnType<typeof render> {
  return render(<TopBarUtilities followRoom />)
}

beforeEach(() => {
  cleanup()
  // jsdom has no layout; the switcher scrolls its highlighted row.
  Element.prototype.scrollIntoView = () => {}
  __resetServerScopesForTests()
  __resetWindowRoomForTests()
  resetSubscriptionUsageForTests()
  resetKeepAwakeForTests()
  h.gets.length = 0
  h.posts.length = 0
  h.answer = defaultAnswer
  useConnectHostStore.setState({ hosts: [B, C], activeHost: 'local' } as never)
  usePageViewStore.setState({ page: 'agents' })
  usePresenceStore.setState({ roster: [user('rosson', 'owner'), user('alice')], supported: true })
  useServerSwitcherStore.setState({ open: false })
  setPool({ role: 'member' })
  setPoolStatusSource(poolStore)
})

afterEach(() => {
  setPoolStatusSource(null)
})

describe('useTopBarScope (T3.1)', () => {
  it('follows a focused pinned room on Home, and nothing else', () => {
    const room = roomOnB()
    const followed = topBarTargetFor(true, 'home', room)
    expect(followed.room).toBe(room)
    expect(followed.scope).toBe(room.scope)
    expect(followed.key).toBe(B_KEY)
    expect(followed.label).toBe('B')
    expect(followed.isWindowServer).toBe(false)

    // The Agents page, Home showing the window's own room, and every bar
    // that does not opt in (Settings, Projects, Wiki, Tickets, Focus).
    for (const t of [
      topBarTargetFor(true, 'agents', room),
      topBarTargetFor(true, 'home', primaryRoomStub),
      topBarTargetFor(true, 'home', null),
      topBarTargetFor(false, 'home', room),
    ]) {
      expect(t.room).toBe(null)
      expect(t.scope).toBe(primaryScope())
      expect(t.key).toBe(WINDOW_TOP_BAR_KEY)
      expect(t.isWindowServer).toBe(true)
    }
  })

  it('a room on the window’s own server (promotion) is the window’s server', () => {
    const room = roomOnB()
    useConnectHostStore.setState({ activeHost: B } as never)
    const t = topBarTargetFor(true, 'home', room)
    expect(t.room).toBe(null)
    expect(t.scope).toBe(primaryScope())
  })

  it('reads offline and sign in from the pool, and gates Keep awake on Admin+ (Q3)', () => {
    expect(roomServerState(null)).toBe('ok')
    expect(roomServerState({ reach: 'offline', auth: 'ok', role: null, checkedAt: 1 })).toBe('offline')
    expect(roomServerState({ reach: 'live', auth: 'signin-required', role: null, checkedAt: 1 })).toBe('signin')
    expect(roomServerState({ reach: 'live', auth: 'kicked', role: null, checkedAt: 1 })).toBe('signin')
    // A blank entry (never checked) is not a sign-in prompt.
    expect(roomServerState({ reach: 'unknown', auth: 'signin-required', role: null, checkedAt: null })).toBe('ok')

    const usable = topBarTargetFor(true, 'home', roomOnB())
    const viewOnly = topBarTargetFor(true, 'home', roomOnB({ readOnly: true }))
    expect(keepAwakeMayChange(usable, 'owner')).toBe(true)
    expect(keepAwakeMayChange(usable, 'admin')).toBe(true)
    expect(keepAwakeMayChange(usable, 'member')).toBe(false)
    expect(keepAwakeMayChange(usable, null)).toBe(false)
    expect(keepAwakeMayChange(viewOnly, 'owner')).toBe(false)
    expect(keepAwakeMayChange(topBarTargetFor(true, 'agents', null), 'member')).toBe(true)
  })
})

/** "{server} | value": the server name, then the top bar's own vertical
 *  divider (the same `TopBarPipe` the right cluster uses), no dot. */
function expectServerDivider(): void {
  const server = screen.getByTestId('usage-server')
  expect(server.textContent).toBe('B')
  const pipes = server.querySelectorAll('[data-top-bar-pipe]')
  expect(pipes).toHaveLength(1)
  expect(pipes[0].className).toBe('block w-px h-4 bg-[var(--color-border)] mx-1')
  expect(screen.getByTestId('subscription-usage').textContent).not.toContain('·')
}

describe('usage chip follows the room (Z16, Z19, T3.2)', () => {
  it('the server divider is the same element the right cluster uses between items', () => {
    bar()
    const cluster = screen.getByTestId('subscription-usage').parentElement?.parentElement
    if (!cluster) throw new Error('no right cluster around the usage chip')
    const clusterPipe = Array.from(cluster.children).find((el) => el.hasAttribute('data-top-bar-pipe'))
    if (!clusterPipe) throw new Error('the right cluster has no divider')
    expect(clusterPipe.className).toBe('block w-px h-4 bg-[var(--color-border)] mx-1')
  })

  it('shows B’s numbers through B’s scope, then the window’s again on Agents', async () => {
    bar()
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Claude10%'))
    expect(screen.queryByTestId('usage-server')).toBe(null)
    const windowGets = h.gets.filter((c) => c.route === 'usage/subscriptions')
    expect(windowGets.map((c) => isWindow(c.scope))).toEqual([true])

    focus(roomOnB())
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BClaude42%'))
    expectServerDivider()
    const bGets = h.gets.filter((c) => c.route === 'usage/subscriptions' && isB(c.scope))
    expect(bGets).toHaveLength(1)
    const creds = await bGets[0].scope.creds()
    expect([creds.host, creds.port, creds.token]).toEqual(['127.0.0.1', 59_998, 'tok-b'])
    // Nothing for B's room went to the window's server.
    expect(h.gets.filter((c) => c.route === 'usage/subscriptions' && isWindow(c.scope))).toHaveLength(1)

    fireEvent.click(screen.getByTestId('subscription-usage'))
    const whose = await screen.findByTestId('subscription-usage-whose')
    expect(whose.textContent).toBe("These numbers are B's logins, not this computer's.")

    backToAgents()
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Claude10%'))
    expect(screen.queryByTestId('usage-server')).toBe(null)
  })

  it('Refresh in a usable room POSTs to B; a view-only room only reads', async () => {
    focus(roomOnB())
    bar()
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BClaude42%'))
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const refresh = (await screen.findByTestId('subscription-usage-refresh')) as HTMLButtonElement
    expect(refresh.disabled).toBe(false)
    // The doc lists Claude only, so it reads as stale: opening the menu
    // re-probes once (Z41's gate), then Refresh probes again.
    await waitFor(() => expect(h.posts).toHaveLength(1))
    await act(async () => {
      fireEvent.click(refresh)
    })
    const refreshes = h.posts.filter((c) => c.route === 'usage/subscriptions/refresh')
    expect(refreshes).toHaveLength(2)
    expect(h.posts).toHaveLength(2)
    for (const c of refreshes) {
      expect(isB(c.scope)).toBe(true)
      expect(c.scope.remoteRoom).toBe(true)
    }

    cleanup()
    resetSubscriptionUsageForTests()
    h.posts.length = 0
    focus(roomOnB({ readOnly: true }))
    bar()
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BClaude42%'))
    fireEvent.click(screen.getByTestId('subscription-usage'))
    const ro = (await screen.findByTestId('subscription-usage-refresh')) as HTMLButtonElement
    expect(ro.disabled).toBe(true)
    // The menu open re-reads with a GET, never the refresh POST.
    await waitFor(() =>
      expect(h.gets.filter((c) => c.route === 'usage/subscriptions' && isB(c.scope)).length).toBeGreaterThanOrEqual(2),
    )
    expect(h.posts).toHaveLength(0)
  })

  it('an offline room says so and never shows or asks the window’s server', async () => {
    setPool({ reach: 'offline' })
    focus(roomOnB())
    bar()
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Boffline'))
    expectServerDivider()
    expect(h.gets.filter((c) => c.route === 'usage/subscriptions')).toHaveLength(0)

    act(() => setPool({ auth: 'signin-required' }))
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BSign in'))
    expectServerDivider()
    expect(h.gets.filter((c) => c.route === 'usage/subscriptions')).toHaveLength(0)

    // Back online: B is asked, the window still is not.
    act(() => setPool({ role: 'member' }))
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BClaude42%'))
    expect(h.gets.filter((c) => c.route === 'usage/subscriptions').map((c) => isB(c.scope))).toEqual([true])
  })

  it('re-reads only the shown server’s cache on the 60 s timer, never probing', async () => {
    // Only intervals are fake; waitFor keeps its real timeouts.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    try {
      const usageGets = (pred: (s: ServerScope) => boolean): number =>
        h.gets.filter((c) => c.route === 'usage/subscriptions' && pred(c.scope)).length
      bar()
      await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Claude10%'))
      expect(usageGets(isWindow)).toBe(1)

      // The window's server is polled while it is shown.
      act(() => {
        vi.advanceTimersByTime(USAGE_CACHE_POLL_MS)
      })
      await waitFor(() => expect(usageGets(isWindow)).toBe(2))
      expect(usageGets(isB)).toBe(0)

      // Focus B's room: B is polled, the window's server is not.
      focus(roomOnB())
      await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('BClaude42%'))
      expect(usageGets(isB)).toBe(1)
      for (let i = 0; i < 3; i++) {
        act(() => {
          vi.advanceTimersByTime(USAGE_CACHE_POLL_MS)
        })
        await waitFor(() => expect(usageGets(isB)).toBe(2 + i))
      }
      expect(usageGets(isWindow)).toBe(2)
      expect(h.posts).toHaveLength(0)

      // Hidden: no reads. Shown again: one read, for B only.
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
      act(() => {
        vi.advanceTimersByTime(USAGE_CACHE_POLL_MS * 3)
      })
      expect(usageGets(isB)).toBe(4)
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
      act(() => {
        document.dispatchEvent(new Event('visibilitychange'))
      })
      await waitFor(() => expect(usageGets(isB)).toBe(5))
      expect(usageGets(isWindow)).toBe(2)

      // An offline room is not polled.
      act(() => setPool({ reach: 'offline' }))
      await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Boffline'))
      act(() => {
        vi.advanceTimersByTime(USAGE_CACHE_POLL_MS * 2)
      })
      expect(usageGets(isB)).toBe(5)
      expect(usageGets(isWindow)).toBe(2)
    } finally {
      vi.useRealTimers()
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
  })

  it('a top-switcher change drops the window’s entry before the new load; B’s stays', async () => {
    bar()
    await waitFor(() => expect(usageEntryFor(WINDOW_TOP_BAR_KEY)?.doc).not.toBe(undefined))
    focus(roomOnB())
    await waitFor(() => expect(usageEntryFor(B_KEY)).not.toBe(null))
    const bEntry = usageEntryFor(B_KEY)
    cleanup()
    act(() => {
      useConnectHostStore.setState({ activeHost: C } as never)
    })
    expect(usageEntryFor(WINDOW_TOP_BAR_KEY)).toBe(null)
    expect(usageEntryFor(B_KEY)).toBe(bEntry)
  })
})

describe('Keep awake follows the room (Z17, Z35, Q3)', () => {
  it('shows B’s mode; a Member sees it read-only and nothing is sent', async () => {
    bar()
    await waitFor(() => expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake: Window awake'))

    focus(roomOnB())
    await waitFor(() =>
      expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake on B: B awake'),
    )
    expect(h.gets.filter((c) => c.route === 'power/status' && isB(c.scope))).toHaveLength(1)
    fireEvent.click(screen.getByTestId('keep-awake'))
    expect((await screen.findByTestId('keep-awake-whose')).textContent).toBe('This keeps B awake, not this computer.')
    expect(screen.getByTestId('keep-awake-read-only').textContent).toBe(
      'Only an Admin or Owner of B can change this from here.',
    )
    const always = screen.getByTestId('keep-awake-mode-always') as HTMLInputElement
    expect(always.disabled).toBe(true)
    expect((screen.getByTestId('keep-awake-lid') as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByTestId('keep-awake-battery') as HTMLInputElement).disabled).toBe(true)
    // Set up (the host's admin dialog) never shows in a room.
    expect(screen.queryByTestId('keep-awake-setup')).toBe(null)
    fireEvent.click(always)
    expect(h.posts).toHaveLength(0)

    backToAgents()
    await waitFor(() => expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake: Window awake'))
  })

  it('an Admin on B changes B’s mode through the room scope, never the window’s', async () => {
    setPool({ role: 'admin' })
    focus(roomOnB())
    bar()
    await waitFor(() =>
      expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake on B: B awake'),
    )
    fireEvent.click(screen.getByTestId('keep-awake'))
    const always = (await screen.findByTestId('keep-awake-mode-always')) as HTMLInputElement
    expect(always.disabled).toBe(false)
    expect(screen.queryByTestId('keep-awake-read-only')).toBe(null)
    await act(async () => {
      fireEvent.click(always)
    })
    expect(h.posts.map((c) => [c.route, isB(c.scope), c.scope.remoteRoom, c.body])).toEqual([
      ['power/keep-awake', true, true, { mode: 'always' }],
    ])
    expect(keepAwakeEntryFor(B_KEY).status?.mode).toBe('always')
    expect(keepAwakeEntryFor(WINDOW_TOP_BAR_KEY).status).toBe(null)
    expect(h.posts.filter((c) => isWindow(c.scope))).toHaveLength(0)
  })

  it('B’s own canChange wins over a stale Admin role in the pool', async () => {
    setPool({ role: 'admin' })
    h.answer = (scope, route, body) =>
      isB(scope) && route === 'power/status'
        ? { keepAwake: { ...keepAwake('B awake', 'working').keepAwake, canChange: false } }
        : defaultAnswer(scope, route, body)
    focus(roomOnB())
    bar()
    await waitFor(() =>
      expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake on B: B awake'),
    )
    fireEvent.click(screen.getByTestId('keep-awake'))
    expect((await screen.findByTestId('keep-awake-read-only')).textContent).toBe(
      'Only an Admin or Owner of B can change this from here.',
    )
    expect((screen.getByTestId('keep-awake-mode-always') as HTMLInputElement).disabled).toBe(true)
    expect(h.posts).toHaveLength(0)
  })

  it('a view-only room is read-only even for an Owner', async () => {
    setPool({ role: 'owner' })
    focus(roomOnB({ readOnly: true }))
    bar()
    await waitFor(() =>
      expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake on B: B awake'),
    )
    fireEvent.click(screen.getByTestId('keep-awake'))
    expect((await screen.findByTestId('keep-awake-read-only')).textContent).toBe('View only: B runs an older K2.')
    expect((screen.getByTestId('keep-awake-mode-off') as HTMLInputElement).disabled).toBe(true)
  })

  it('offline shows unknown and asks nobody; a pre-0.43.0 server shows Not available and stops asking', async () => {
    setPool({ reach: 'offline' })
    focus(roomOnB())
    bar()
    await waitFor(() => expect(screen.getByTestId('keep-awake').getAttribute('data-state')).toBe('unknown'))
    expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake on B: unknown')
    expect(h.gets.filter((c) => c.route === 'power/status')).toHaveLength(0)

    cleanup()
    h.answer = (scope, route, body) =>
      isB(scope) && route === 'power/status' ? new Error('route not found') : defaultAnswer(scope, route, body)
    act(() => setPool({ role: 'member' }))
    bar()
    await waitFor(() => expect(screen.getByTestId('keep-awake').getAttribute('data-state')).toBe('unavailable'))
    expect(screen.getByTestId('keep-awake').getAttribute('title')).toBe('Keep awake: not available on B')
    fireEvent.click(screen.getByTestId('keep-awake'))
    expect((await screen.findByTestId('keep-awake-unknown')).textContent).toBe(
      'Not available on B. It runs a K2 from before Keep awake (0.43.0).',
    )
    expect(h.gets.filter((c) => c.route === 'power/status').map((c) => isB(c.scope))).toEqual([true])
    expect(keepAwakeEntryFor(B_KEY).unavailable).toBe(true)
  })

  it('the request layer refuses both new room writes from a view-only room (T3.3)', () => {
    expect(ROOM_WRITE_ROUTES.has('usage/subscriptions/refresh')).toBe(true)
    expect(ROOM_WRITE_ROUTES.has('power/keep-awake')).toBe(true)
    expect(ROOM_WRITE_ROUTES.has('power/helper')).toBe(false)
    const viewOnly = viewOnlyScope(scopeForHost(B))
    for (const route of ['usage/subscriptions/refresh', 'power/keep-awake']) {
      expect(() => assertScopeMayWrite(viewOnly, route)).toThrow(ViewOnlyWriteError)
      expect(() => assertScopeMayWrite(remoteRoomScope(scopeForHost(B)), route)).not.toThrow()
    }
  })
})

describe('presence roster follows the room (Z15, Z34)', () => {
  it('shows B’s people with no Kick, then the window’s people again', async () => {
    bar()
    const roster = await screen.findByTestId('presence-roster')
    expect(roster.getAttribute('title')).toBe('2 connected — click for details')

    focus(roomOnB())
    await waitFor(() =>
      expect(screen.getByTestId('presence-roster').getAttribute('title')).toBe('2 connected on B — click for details'),
    )
    fireEvent.click(screen.getByTestId('presence-roster'))
    expect(await screen.findByText('Connected to B')).toBeTruthy()
    expect(screen.queryByText('Kick')).toBe(null)
    // The modal never asked the window's server who we are (Kick's check).
    expect(h.gets.filter((c) => c.route === 'auth/whoami')).toHaveLength(0)
    cleanup()

    backToAgents()
    bar()
    fireEvent.click(await screen.findByTestId('presence-roster'))
    expect(await screen.findByText('Connected users')).toBeTruthy()
    await waitFor(() => expect(screen.getAllByText('Kick').length).toBeGreaterThan(0))
  })
})

describe('mode toggle shows the room’s mode (Z15)', () => {
  it('is the room’s mode, disabled, set by B; the window toggle comes back on Agents', async () => {
    const { container } = bar()
    const windowGroup = container.querySelector('[data-mode-toggle]')
    if (!windowGroup) throw new Error('mode toggle missing')
    expect(windowGroup.getAttribute('data-mode-source')).toBe(null)

    focus(roomOnB())
    const usable = container.querySelector('[data-mode-source="room"]')
    if (!usable) throw new Error('room mode toggle missing')
    expect(usable.getAttribute('data-mode-toggle')).toBe('claimer')
    expect(usable.getAttribute('title')).toBe('Set by B')
    for (const b of Array.from(usable.querySelectorAll('button'))) expect(b.disabled).toBe(true)

    focus(roomOnB({ readOnly: true }))
    expect(container.querySelector('[data-mode-source="room"]')?.getAttribute('data-mode-toggle')).toBe('viewer')

    backToAgents()
    expect(container.querySelector('[data-mode-source="room"]')).toBe(null)
    expect(container.querySelector('[data-mode-toggle]')).not.toBe(null)
  })
})

describe('server switcher label (Z20, Q4, T3.5)', () => {
  it('names the room’s server; the dropdown keeps the window’s check; picking B switches the window', async () => {
    const pickHost = vi.fn()
    const real = useConnectHostStore.getState().pickHost
    useConnectHostStore.setState({ pickHost } as never)
    try {
      render(<ServerSwitcher followRoom />)
      expect(screen.getByTestId('server-switcher-label').textContent).toBe('This computer')
      expect(screen.getByTestId('server-switcher-trigger').getAttribute('data-follows-room')).toBe(null)

      focus(roomOnB())
      expect(screen.getByTestId('server-switcher-label').textContent).toBe('B')
      const trigger = screen.getByTestId('server-switcher-trigger')
      expect(trigger.getAttribute('data-follows-room')).toBe('true')
      expect(trigger.getAttribute('title')).toBe('Looking at B on Home. This window is on This computer. (⌘L)')

      fireEvent.click(trigger)
      expect(screen.getByTestId('server-switcher-looking-at').textContent).toBe('Looking at B (Home room)')
      const rows = Array.from(document.querySelectorAll('button[role="option"]'))
      const thisComputer = rows.find((r) => r.textContent?.includes('This computer'))
      const bRow = rows.find((r) => r.textContent?.startsWith('B'))
      if (!thisComputer || !bRow) throw new Error('switcher rows missing')
      // The check (the active row's mark) stays on the window's server.
      expect(thisComputer.querySelector('polyline')).not.toBe(null)
      expect(bRow.querySelector('polyline')).toBe(null)

      fireEvent.click(bRow)
      expect(pickHost).toHaveBeenCalledTimes(1)
      expect(pickHost.mock.calls[0][0]).toMatchObject({ id: 'id-b' })

      backToAgents()
      expect(screen.getByTestId('server-switcher-label').textContent).toBe('This computer')
    } finally {
      useConnectHostStore.setState({ pickHost: real } as never)
    }
  })

  it('a bar that does not opt in (Settings, Projects, Focus …) keeps the window’s server', () => {
    focus(roomOnB())
    render(<ServerSwitcher />)
    expect(screen.getByTestId('server-switcher-label').textContent).toBe('This computer')
    expect(screen.queryByTestId('server-switcher-looking-at')).toBe(null)
  })

  it('a bar without followRoom keeps every element on the window’s server', async () => {
    focus(roomOnB())
    render(<TopBarUtilities />)
    await waitFor(() => expect(screen.getByTestId('subscription-usage').textContent).toBe('Claude10%'))
    expect(h.gets.filter((c) => !isWindow(c.scope))).toHaveLength(0)
  })
})

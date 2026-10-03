// @vitest-environment jsdom
// Home M4 — remote rooms inside Home's main area: the failure gate's states
// render over the room (MS45), a live room shows its bar ("Remote room",
// or View only with a Switch to {server} button on an older server, 0.43.2
// Z3/Z5) with its server and that server's people (R7), and only hot rooms
// are mounted (MS24).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
// The room's components are the Agents page's own; here a stub stands in
// for the terminal area so the test is about the shell around it.
vi.mock('@/components/Terminal/TerminalArea', () => ({
  TerminalArea: ({ cwd }: { cwd: string }) => <div data-stub-terminal-area={cwd} />,
}))

import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { createStore } from 'zustand/vanilla'
import { HomeRemoteRooms } from './HomeRemoteRooms'
import { useHomeRoomsStore, type HomeRoomEntry } from '@/stores/home-rooms'
import { DEFAULT_ROOM_TIER_CONFIG, roomTiers } from '@/lib/room-tiers'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { usePageViewStore } from '@/stores/page-view'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import type { PinnedRoom } from '@/stores/room'
import type { PresenceView } from '@/stores/server-view'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const KEY = 'b.k2.dev'
const B: ConnectHost = {
  id: 'id-b',
  label: 'Box B',
  hostname: 'b.k2.dev',
  port: 443,
  secure: true,
  token: 'tok',
  remember: false,
  lastConnectedAt: null,
}

function poolEntry(patch: Partial<HostEntry>): HostEntry {
  return {
    hostKey: KEY,
    saved: true,
    hostId: 'id-b',
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.41.6', protocol: 1, instanceId: 'i1', features: [], at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
    ...patch,
  }
}

const presence = createStore<PresenceView>(() => ({ roster: [], supported: true }))

function fakeRoom(readOnly = false): PinnedRoom {
  return {
    key: `${KEY}|pb:pb-ws`,
    isPrimary: false,
    readOnly,
    scope: readOnly
      ? { id: `host:${KEY}`, hostKey: KEY, label: 'Box B', isRemote: true, isPrimary: false, viewOnly: true }
      : { id: `host:${KEY}`, hostKey: KEY, label: 'Box B', isRemote: true, isPrimary: false, remoteRoom: true },
    presence,
    cwd: () => '/srv/anna',
  } as unknown as PinnedRoom
}

function openEntry(room: PinnedRoom): HomeRoomEntry {
  return {
    address: `anna::${KEY}`,
    hostKey: KEY,
    label: 'Anna',
    phase: 'open',
    error: null,
    room,
    access: room.readOnly ? 'view-older-server' : 'use',
    generation: 1,
  }
}

let container: HTMLDivElement
let root: Root

function render(): void {
  act(() => {
    root.render(<HomeRemoteRooms />)
  })
}

beforeEach(() => {
  container = document.createElement('div')
  document.body.appendChild(container)
  root = createRoot(container)
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
  usePageViewStore.getState().setPage('home')
  presence.setState({ roster: [], supported: true })
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
  // Forget the rooms and restore the numbers (dispose would also drop the
  // Home rooms manager's own tier listener).
  useHomeRoomsStore.setState({ entries: {}, shown: null })
  for (const key of Object.keys(roomTiers.store.getState().rooms)) roomTiers.close(key)
  roomTiers.reconfigure({ ...DEFAULT_ROOM_TIER_CONFIG })
  hostPool.store.setState({ entries: {} })
})

function showRoom(room: PinnedRoom): void {
  const entry = openEntry(room)
  useHomeRoomsStore.setState({ entries: { [entry.address]: entry }, shown: entry.address })
  roomTiers.show(room.key)
}

function q(sel: string): Element {
  const el = container.querySelector(sel)
  if (!el) throw new Error(`no element for ${sel}\n${container.innerHTML}`)
  return el
}

describe('remote rooms in Home (M4)', () => {
  it('a live server: the usable room renders with its room bar and no failure banner (M5)', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({}) } })
    showRoom(fakeRoom())
    render()
    expect(q(`[data-home-room="anna::${KEY}"] [data-room-bar]`).getAttribute('data-room-access')).toBe('use')
    expect(q('[data-room-chip]').textContent).toBe('Remote room')
    expect(q('[data-room-server]').textContent).toBe('Anna on Box B')
    expect(container.querySelector('[data-room-note]')).toBe(null)
    expect(container.querySelector('[data-room-switch]')).toBe(null)
    expect(q('[data-stub-terminal-area]').getAttribute('data-stub-terminal-area')).toBe('/srv/anna')
    expect(container.querySelector('[data-room-failure]')).toBe(null)
  })

  it('an older server: the room says View only and why, with a Switch to button that switches this window to B (MS43, Z5)', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({}) } })
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost } as never)
    showRoom(fakeRoom(true))
    render()
    expect(q('[data-room-bar]').getAttribute('data-room-access')).toBe('view-older-server')
    expect(q('[data-room-chip]').textContent).toBe('View only')
    expect(q('[data-room-note]').textContent).toBe('Box B runs K2 0.41.6, which can\u2019t save this room\u2019s tabs safely.')
    const button = q('[data-room-switch]') as HTMLButtonElement
    expect(button.textContent).toBe('Switch to Box B')
    expect(pickHost).not.toHaveBeenCalled()
    act(() => button.click())
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0]).toBe(B)
  })

  it('shows that server’s people on this agent (R7)', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({}) } })
    presence.setState({
      roster: [
        { user: 'rosson', role: 'owner', workspaces: ['/srv/anna'] },
        { user: 'appa', role: 'member', workspaces: ['/srv/other'] },
        { user: 'anna', role: 'member', workspaces: ['/srv/anna/.worktrees/x'] },
      ] as never,
      supported: true,
    })
    showRoom(fakeRoom())
    render()
    expect(q('[data-room-people]').getAttribute('data-room-people')).toBe('rosson,anna')
  })

  for (const [name, patch, kind, action] of [
    ['offline', { reach: 'offline' as const }, 'offline', 'retry'],
    ['restarting', { reach: 'starting' as const }, 'restarting', 'retry'],
    ['kicked', { auth: 'kicked' as const }, 'kicked', 'sign-in'],
    ['sign-in needed', { auth: 'signin-required' as const }, 'signin-required', 'sign-in'],
    ['too old', { boot: { phase: 'ready', ready: true, version: '0.40.30', protocol: 1, instanceId: 'i', features: [], at: 1 } }, 'version-too-old', 'open-server'],
  ] as const) {
    it(`${name}: the banner shows over the room's last frame, dimmed with input off`, () => {
      hostPool.store.setState({ entries: { [KEY]: poolEntry(patch as Partial<HostEntry>) } })
      showRoom(fakeRoom())
      render()
      const shell = q(`[data-home-room="anna::${KEY}"]`)
      expect(q(`[data-home-room="anna::${KEY}"] [data-room-failure]`).getAttribute('data-room-failure')).toBe(kind)
      expect(q('[data-room-failure-action]').getAttribute('data-room-failure-action')).toBe(action)
      const frame = q('[data-room-failing] [aria-disabled="true"]')
      expect(frame.hasAttribute('inert')).toBe(true)
      expect(frame.querySelector('[data-stub-terminal-area]')).not.toBe(null)
      expect(shell.contains(frame)).toBe(true)
    })
  }

  it('a row that could not open shows the gate and a Retry inside Home', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({ reach: 'offline' }) } })
    const entry: HomeRoomEntry = {
      address: `anna::${KEY}`,
      hostKey: KEY,
      label: 'Anna',
      phase: 'error',
      error: 'connection refused',
      room: null,
      access: null,
      generation: 0,
    }
    useHomeRoomsStore.setState({ entries: { [entry.address]: entry }, shown: entry.address })
    render()
    expect(q(`[data-home-room-phase="error"] [data-room-failure]`).getAttribute('data-room-failure')).toBe('offline')
    expect(container.textContent).toContain('Couldn’t open Anna: connection refused')
  })

  it('a warm room is not mounted (its grids are closed); a hidden hot room is mounted but hidden', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({}) } })
    const room = fakeRoom()
    showRoom(room)
    useHomeRoomsStore.setState({ shown: null })
    roomTiers.hide(room.key)
    render()
    const hidden = q(`[data-home-room="anna::${KEY}"]`) as HTMLElement
    expect(hidden.style.display).toBe('none')
    expect(hidden.getAttribute('aria-hidden')).toBe('true')

    act(() => {
      roomTiers.reconfigure({ hotGraceMs: 0 })
    })
    expect(roomTiers.tier(room.key)).toBe('warm')
    expect(container.querySelector('[data-home-room]')).toBe(null)
  })

  it('not on Home: no remote room is shown', () => {
    hostPool.store.setState({ entries: { [KEY]: poolEntry({}) } })
    showRoom(fakeRoom())
    usePageViewStore.getState().setPage('agents')
    render()
    const shell = q(`[data-home-room="anna::${KEY}"]`) as HTMLElement
    expect(shell.style.display).toBe('none')
  })
})

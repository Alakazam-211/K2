// @vitest-environment jsdom
// prd-home-seamless-0432 item 2 — T2.2 and T2.5 through the REAL
// ConnectionGate, the real rooms portal, the real slot and parking element.
//
// The App chunk is replaced by a stand-in App that renders only the rooms
// slot (where AgentsShell puts it). Its `HomeRoomsHost` is the real portal.
// A remote room for B is open and shown on Home. The window switches from
// `local` to C (the gate goes wait → overlay → accept, `<App key>`
// remounts). The rooms host must mount once and never unmount; the room's
// terminal must never unmount; its grid socket must stay open (PageLive
// unchanged) while the rooms sit in the parking element at the slot's last
// rect; a Browser pane in the room must read "parked".

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => {
  // Stores evaluated on import fetch at once; nothing here reaches a network.
  globalThis.fetch = (async () => new Response('{}', { status: 200 })) as typeof fetch
  return {
  app: { mounts: 0, unmounts: 0 },
  host: { mounts: 0, unmounts: 0 },
  terminal: { mounts: 0, unmounts: 0 },
  sockets: [] as Array<{ id: number; open: boolean }>,
  browserParked: [] as boolean[],
  fetched: [] as string[],
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 4100, token: 'own' }
    return null
  }),
}))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '9.9.9' }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
// The gate's own window chrome is not under test.
vi.mock('@/components/TopBar/GateChrome', () => ({ default: () => null }))

// The room's terminal area: holds one "grid socket" while its tab is
// visible (TabVisibility && PageLive), like TerminalPane, and closes it on
// hide or unmount. A Browser stand-in records what it reads for "parked".
vi.mock('@/components/Terminal/TerminalArea', async () => {
  const React = await import('react')
  const { useIsTabVisible } = await import('@/contexts/TabVisibilityContext')
  const { useRoomParked } = await import('@/contexts/RoomParkedContext')
  function TerminalArea(): React.JSX.Element {
    const visible = useIsTabVisible()
    const parked = useRoomParked()
    h.browserParked.push(parked)
    React.useEffect(() => {
      h.terminal.mounts += 1
      return () => {
        h.terminal.unmounts += 1
      }
    }, [])
    React.useEffect(() => {
      if (!visible) return
      const sock = { id: h.sockets.length + 1, open: true }
      h.sockets.push(sock)
      return () => {
        sock.open = false
      }
    }, [visible])
    return <div data-stub-terminal-area="" data-parked={parked ? '1' : '0'} />
  }
  return { TerminalArea }
})

vi.mock('@/App', async () => {
  const React = await import('react')
  const { HomeRoomsPortal, RoomSlot } = await import('@/components/Home/room/HomeRoomsHost')
  function App(): React.JSX.Element {
    React.useEffect(() => {
      h.app.mounts += 1
      return () => {
        h.app.unmounts += 1
      }
    }, [])
    return (
      <div data-stub-app="" style={{ width: 640, height: 480 }}>
        <RoomSlot visible />
      </div>
    )
  }
  function HomeRoomsHost(): React.JSX.Element {
    React.useEffect(() => {
      h.host.mounts += 1
      return () => {
        h.host.unmounts += 1
      }
    }, [])
    return <HomeRoomsPortal />
  }
  // Zen's layer (prd-zen-mode-v1 Z6) is not under test here.
  return { default: App, HomeRoomsHost, ZenHost: () => null }
})

import { act, cleanup, render, waitFor } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'
import { ConnectionGate } from './ConnectionGate'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { homeRooms, useHomeRoomsStore, type HomeRoomEntry } from '@/stores/home-rooms'
import { DEFAULT_ROOM_TIER_CONFIG, roomTiers } from '@/lib/room-tiers'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { usePageViewStore } from '@/stores/page-view'
import { __resetHomeRoomsHostForTests, homeRoomsContainer, homeRoomsParking, useRoomParkStore } from '@/components/Home/room/HomeRoomsHost'
import type { PinnedRoom } from '@/stores/room'
import type { PresenceView } from '@/stores/server-view'

const B_KEY = 'b.k2.dev'
const B: ConnectHost = {
  id: 'id-b', label: 'Box B', hostname: 'b.k2.dev', port: 443, secure: true,
  token: 'tok-b', remember: false, lastConnectedAt: null,
} as ConnectHost
const C: ConnectHost = {
  id: 'id-c', label: 'Box C', hostname: 'c.k2.dev', port: 443, secure: true,
  token: 'tok-c', remember: false, lastConnectedAt: null,
} as ConnectHost

function poolEntry(): HostEntry {
  return {
    hostKey: B_KEY,
    saved: true,
    hostId: 'id-b',
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.43.0', protocol: 1, instanceId: 'i1', features: [], at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
  }
}

function fakeRoom(): PinnedRoom {
  return {
    key: `${B_KEY}|pb:pb-ws`,
    isPrimary: false,
    readOnly: false,
    scope: { id: `host:${B_KEY}`, hostKey: B_KEY, label: 'Box B', isRemote: true, isPrimary: false, remoteRoom: true },
    presence: createStore<PresenceView>(() => ({ roster: [], supported: true })),
    cwd: () => '/srv/anna',
  } as unknown as PinnedRoom
}

const SLOT_RECT = { x: 12, y: 40, left: 12, top: 40, width: 640, height: 480, right: 652, bottom: 520 }

beforeEach(() => {
  h.app.mounts = h.app.unmounts = 0
  h.host.mounts = h.host.unmounts = 0
  h.terminal.mounts = h.terminal.unmounts = 0
  h.sockets.length = 0
  h.browserParked.length = 0
  h.fetched.length = 0
  vi.stubGlobal('fetch', async (input: RequestInfo | URL) => {
    const url = String(input)
    h.fetched.push(url)
    if (url.includes('/boot-status')) {
      return new Response(JSON.stringify({ version: '9.9.9', protocol: 1, phase: 'ready', detail: '' }), { status: 200 })
    }
    return new Response('{}', { status: 200 })
  })
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.hasAttribute('data-home-room-slot')) return { ...SLOT_RECT, toJSON: () => ({}) } as DOMRect
    return { x: 0, y: 0, left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0, toJSON: () => ({}) } as DOMRect
  })
  useConnectHostStore.setState({
    hosts: [B, C],
    activeHost: 'local',
    // Boot hydration reads the keychain; this suite owns the address book.
    hydrateFromDisk: async () => undefined,
  } as never)
  usePageViewStore.getState().setPage('home')
  hostPool.store.setState({ entries: { [B_KEY]: poolEntry() } })
  const room = fakeRoom()
  const entry: HomeRoomEntry = {
    address: `anna::${B_KEY}`, hostKey: B_KEY, label: 'Anna', phase: 'open',
    error: null, room, access: 'use', generation: 1,
  }
  useHomeRoomsStore.setState({ entries: { [entry.address]: entry }, shown: entry.address })
  roomTiers.show(room.key)
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  useHomeRoomsStore.setState({ entries: {}, shown: null })
  for (const key of Object.keys(roomTiers.store.getState().rooms)) roomTiers.close(key)
  roomTiers.reconfigure({ ...DEFAULT_ROOM_TIER_CONFIG })
  hostPool.store.setState({ entries: {} })
  __resetHomeRoomsHostForTests()
})

describe('a top-switcher change keeps Home’s remote rooms mounted (Z6, Z8, Z9)', () => {
  it('T2.2/T2.5: one rooms host across wait → overlay → accept; parked at the slot’s rect with its grid open; un-parked into the new App', async () => {
    const setPageVisible = vi.spyOn(homeRooms, 'setPageVisible')
    const view = render(<ConnectionGate />)
    await waitFor(() => expect(h.app.mounts).toBe(1), { timeout: 5_000 })
    expect(h.host.mounts).toBe(1)
    expect(h.terminal.mounts).toBe(1)
    const slot = view.container.querySelector('[data-home-room-slot]')
    if (!slot) throw new Error('no room slot in the App')
    expect(homeRoomsContainer().parentElement).toBe(slot)
    expect(useRoomParkStore.getState().parked).toBe(false)
    expect(h.sockets).toEqual([{ id: 1, open: true }])
    expect(h.browserParked.at(-1)).toBe(false)
    expect(setPageVisible).toHaveBeenLastCalledWith(true)

    // The switch: the gate drops to its overlay, App unmounts, the rooms park.
    act(() => {
      useConnectHostStore.getState().selectHost(C)
    })
    // (The gate's accept branch re-keys App for one commit before its
    // effect drops to the overlay; either way no App is live now.)
    expect(h.app.mounts - h.app.unmounts).toBe(0)
    expect(view.container.querySelector('[data-stub-app]')).toBe(null)
    const parking = homeRoomsParking()
    expect(homeRoomsContainer().parentElement).toBe(parking)
    expect(useRoomParkStore.getState()).toEqual({
      parked: true,
      rect: { left: 12, top: 40, width: 640, height: 480 },
    })
    expect(parking.style.position).toBe('fixed')
    expect(parking.style.visibility).toBe('hidden')
    expect(parking.style.pointerEvents).toBe('none')
    expect([parking.style.left, parking.style.top, parking.style.width, parking.style.height]).toEqual([
      '12px',
      '40px',
      '640px',
      '480px',
    ])
    // Still mounted, still `shown`, grid still open; Browser hides.
    expect(h.host.unmounts).toBe(0)
    expect(h.terminal.unmounts).toBe(0)
    expect(h.sockets).toEqual([{ id: 1, open: true }])
    expect(h.browserParked.at(-1)).toBe(true)
    expect(useHomeRoomsStore.getState().shown).toBe(`anna::${B_KEY}`)
    expect(setPageVisible).toHaveBeenLastCalledWith(false)

    // C accepts: the new App mounts and takes the rooms back.
    const mountsAtOverlay = h.app.mounts
    await waitFor(() => expect(h.app.mounts).toBe(mountsAtOverlay + 1), { timeout: 5_000 })
    expect(h.app.mounts - h.app.unmounts).toBe(1)
    const slot2 = view.container.querySelector('[data-home-room-slot]')
    if (!slot2) throw new Error('no room slot in the new App')
    expect(slot2).not.toBe(slot)
    expect(homeRoomsContainer().parentElement).toBe(slot2)
    expect(useRoomParkStore.getState().parked).toBe(false)
    expect(setPageVisible).toHaveBeenLastCalledWith(true)
    expect(h.browserParked.at(-1)).toBe(false)

    // Across the whole switch: one host, one terminal, one socket.
    expect(h.host).toEqual({ mounts: 1, unmounts: 0 })
    expect(h.terminal).toEqual({ mounts: 1, unmounts: 0 })
    expect(h.sockets).toEqual([{ id: 1, open: true }])
    expect(useHomeRoomsStore.getState().entries[`anna::${B_KEY}`]?.generation).toBe(1)
    // The kept room's server was never asked anything by the switch.
    expect(h.fetched.filter((u) => u.includes(B_KEY))).toEqual([])
  })

  it('the rooms host waits for the App chunk: nothing renders in its slot before the first accept', async () => {
    useConnectHostStore.setState({ activeHost: C } as never)
    // C's boot-status never answers ready: the gate stays on its overlay.
    vi.stubGlobal('fetch', async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/boot-status')) {
        return new Response(JSON.stringify({ version: '9.9.9', protocol: 1, phase: 'migrating', detail: 'x' }), { status: 200 })
      }
      return new Response('{}', { status: 200 })
    })
    render(<ConnectionGate />)
    await new Promise((r) => setTimeout(r, 700))
    expect(h.app.mounts).toBe(0)
    expect(h.host.mounts).toBe(0)
  })
})

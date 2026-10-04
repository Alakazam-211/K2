// @vitest-environment jsdom
//
// prd-zen-mode-v1 Z7/Z48 (T4.9) — while Zen owns the window, the Home under
// it is told it is hidden and parks the way a hidden tab does:
//   - the window's own room: `PageLiveContext` goes false (its grids close);
//   - a pinned remote room on Home: no longer `shown`, its PageLive goes
//     false, its shell is display:none, and Home's page visibility (the tier
//     clock) goes false. Its entry stays (Zen's conversations use it).
// Leaving Zen (exit) brings all of it back.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
// The room's terminal area reports the PageLive it reads (TerminalPane
// closes its grid socket when it goes false).
vi.mock('@/components/Terminal/TerminalArea', async () => {
  const React = await import('react')
  const { PageLiveContext } = await import('@/contexts/TabVisibilityContext')
  return {
    TerminalArea: () => {
      const pageLive = React.useContext(PageLiveContext)
      return <div data-stub-terminal="" data-page-live={String(pageLive)} />
    },
  }
})

import { act } from 'react'
import { cleanup, render } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'
import { HomeRoomsPortal, RoomSlot, __resetHomeRoomsHostForTests } from '@/components/Home/room/HomeRoomsHost'
import { homeRooms, useHomeRoomsStore, type HomeRoomEntry } from '@/stores/home-rooms'
import { roomTiers } from '@/lib/room-tiers'
import { hostPool } from '@/lib/host-pool-instance'
import { usePageViewStore, primaryRoomPageLive } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useZenHomesStore } from '@/lib/zen/zen-homes'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { exitZen } from '@/lib/zen/zen-view'
import type { PinnedRoom } from '@/stores/room'
import type { PresenceView } from '@/stores/server-view'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const KEY = 'b.k2.dev'
const presence = createStore<PresenceView>(() => ({ roster: [], supported: true }))

function fakeRoom(): PinnedRoom {
  return {
    key: `${KEY}|pb:pb-ws`,
    isPrimary: false,
    readOnly: false,
    scope: { id: `host:${KEY}`, hostKey: KEY, label: 'Box B', isRemote: true, isPrimary: false, remoteRoom: true },
    presence,
    cwd: () => '/srv/anna',
  } as unknown as PinnedRoom
}

function showRoom(room: PinnedRoom): string {
  const entry: HomeRoomEntry = {
    address: `anna::${KEY}`,
    hostKey: KEY,
    label: 'Anna',
    phase: 'open',
    error: null,
    room,
    access: 'use',
    generation: 1,
  }
  useHomeRoomsStore.setState({ entries: { [entry.address]: entry }, shown: entry.address })
  roomTiers.show(room.key)
  return entry.address
}

function zenOn(): void {
  act(() => useZenHomesStore.getState().setOn(selectedHome(useHomesStore.getState()).id, true))
}

beforeEach(() => {
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  useZenHomesStore.setState({ on: {} })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('home')
  hostPool.store.setState({ entries: {} })
})

afterEach(() => {
  cleanup()
  useHomeRoomsStore.setState({ entries: {}, shown: null })
  for (const key of Object.keys(roomTiers.store.getState().rooms)) roomTiers.close(key)
  __resetHomeRoomsHostForTests()
  vi.restoreAllMocks()
})

describe('the Home under Zen parks', () => {
  it('the window’s own room: PageLive is false on Home under Zen, true again after exit', () => {
    expect(primaryRoomPageLive('home', true, false)).toBe(true)
    expect(primaryRoomPageLive('home', true, true)).toBe(false)
    // Agents never shows Zen; the rule never parks it.
    expect(primaryRoomPageLive('agents', false, true)).toBe(true)
    expect(primaryRoomPageLive('home', false, false)).toBe(false)
  })

  it('a pinned remote room: hidden, PageLive false, Home page visibility false; back after exit', () => {
    const visible = vi.spyOn(homeRooms, 'setPageVisible')
    const address = showRoom(fakeRoom())
    // The rooms render through the real portal into the shell's slot.
    render(
      <>
        <HomeRoomsPortal />
        <RoomSlot visible />
      </>,
    )
    const shell = (): HTMLElement => {
      const el = document.querySelector(`[data-home-room="${address}"]`)
      if (!(el instanceof HTMLElement)) throw new Error('no room shell')
      return el
    }
    expect(shell().style.display).toBe('flex')
    expect(document.querySelector('[data-stub-terminal]')?.getAttribute('data-page-live')).toBe('true')
    expect(visible).toHaveBeenLastCalledWith(true)

    zenOn()
    expect(shell().style.display).toBe('none')
    expect(document.querySelector('[data-stub-terminal]')?.getAttribute('data-page-live')).toBe('false')
    expect(visible).toHaveBeenLastCalledWith(false)
    // The room itself is kept for Zen's conversation.
    expect(useHomeRoomsStore.getState().shown).toBe(address)
    expect(useHomeRoomsStore.getState().entries[address]?.phase).toBe('open')

    act(() => exitZen())
    expect(shell().style.display).toBe('flex')
    expect(document.querySelector('[data-stub-terminal]')?.getAttribute('data-page-live')).toBe('true')
    expect(visible).toHaveBeenLastCalledWith(true)
  })
})

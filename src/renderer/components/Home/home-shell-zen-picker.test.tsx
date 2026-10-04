// @vitest-environment jsdom
//
// vs-live P31 (prd-home-picker-and-remote-avatars-v1): the Zen toggle row
// sits inside the Add Agent picker's outside-click boundary, so clicking it
// never closed the picker. `HomeShellEffects` closes the picker when Zen
// turns on for the selected Home.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { HomeShellEffects, useHomeAddPickerStore } from './home-room'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useZenHomesStore } from '@/lib/zen/zen-homes'

afterEach(() => {
  cleanup()
})

describe('HomeShellEffects and Zen', () => {
  it('turning Zen on for this Home closes an open Add Agent picker', () => {
    const homeId = selectedHome(useHomesStore.getState()).id
    useZenHomesStore.getState().setOn(homeId, false)
    render(<HomeShellEffects />)
    act(() => useHomeAddPickerStore.getState().setOpen(true))
    expect(useHomeAddPickerStore.getState().open).toBe(true)
    act(() => useZenHomesStore.getState().setOn(homeId, true))
    expect(useHomeAddPickerStore.getState().open).toBe(false)
    useZenHomesStore.getState().setOn(homeId, false)
  })
})

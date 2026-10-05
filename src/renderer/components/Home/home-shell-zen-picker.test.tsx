// @vitest-environment jsdom
//
// vs-live P31 (prd-home-picker-and-remote-avatars-v1), prd-zen-gardens-v1
// G6: `HomeShellEffects` closes Home's Add Agent picker whenever Zen is
// shown in this window (the top-bar toggle, the chord, the app menu), so
// the picker never waits under Zen and comes back when Zen exits.

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
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { useSettingsStore } from '@/stores/settings'

afterEach(() => {
  cleanup()
})

describe('HomeShellEffects and Zen', () => {
  it('Zen shown in this window closes an open Add Agent picker; Zen hidden by Settings does not reopen it', () => {
    Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
    useSettingsStore.setState({ settingsOpen: false })
    useZenWindowStore.getState().setOn(false)
    render(<HomeShellEffects />)
    act(() => useHomeAddPickerStore.getState().setOpen(true))
    expect(useHomeAddPickerStore.getState().open).toBe(true)
    act(() => useZenWindowStore.getState().setOn(true))
    expect(useHomeAddPickerStore.getState().open).toBe(false)
    act(() => useZenWindowStore.getState().setOn(false))
    expect(useHomeAddPickerStore.getState().open).toBe(false)
  })
})

// @vitest-environment jsdom
//
// Settings search, through the real Settings router: Cmd/Ctrl+F opens the
// palette, a picked result opens its section and closes the palette, and
// Escape in the palette closes only the palette (Settings stays open). The
// daemon and Tauri are faked at the edges; nothing here needs their answers.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
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
    onResized: async () => () => undefined,
    onFocusChanged: async () => () => undefined,
  }),
}))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...mod,
    daemonCliGet: vi.fn(async () => ({})),
    daemonCliPost: vi.fn(async () => ({})),
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useSettingsStore } from '@/stores/settings'
import Settings from './Settings'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const PLACEHOLDER = /Search settings/

function palette(): HTMLInputElement | null {
  return screen.queryByPlaceholderText(PLACEHOLDER) as HTMLInputElement | null
}

async function openSettingsOn(section: 'timer' | 'keybindings'): Promise<void> {
  useSettingsStore.setState({ settingsOpen: true, activeSection: section })
  render(<Settings />)
  await act(async () => {
    await Promise.resolve()
  })
}

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
})
afterEach(() => {
  cleanup()
  useSettingsStore.setState({ settingsOpen: false, activeSection: 'general' })
})

describe('Settings search palette', () => {
  it('Cmd+F and Ctrl+F open it; it is closed until then', async () => {
    await openSettingsOn('timer')
    expect(palette()).toBeNull()
    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    expect(palette()).not.toBeNull()
    fireEvent.keyDown(palette()!, { key: 'Escape' })
    expect(palette()).toBeNull()
    fireEvent.keyDown(window, { key: 'f', ctrlKey: true })
    expect(palette()).not.toBeNull()
  })

  it('a picked result opens its section and closes the palette', async () => {
    await openSettingsOn('timer')
    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    fireEvent.change(palette()!, { target: { value: 'Scrollback Buffer' } })
    fireEvent.keyDown(palette()!, { key: 'Enter' })
    expect(palette()).toBeNull()
    expect(useSettingsStore.getState().activeSection).toBe('terminal')
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
  })

  it('Escape in the palette closes the palette, not Settings; the next Escape closes Settings', async () => {
    await openSettingsOn('timer')
    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    fireEvent.keyDown(palette()!, { key: 'Escape' })
    expect(palette()).toBeNull()
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(useSettingsStore.getState().settingsOpen).toBe(false))
  })
})

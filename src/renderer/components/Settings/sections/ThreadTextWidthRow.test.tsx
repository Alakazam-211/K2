// @vitest-environment jsdom
// "Thread text width": per client (localStorage), never a daemon setting
// (prd-thread-view-focus-v1 §5).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

const settingsUpdate = vi.hoisted(() => vi.fn(async (updates: Record<string, unknown>) => updates))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate,
  settingsReset: vi.fn(async () => ({})),
}))

import { WORKSPACE_SETTINGS_MANIFEST, WorkspaceBehaviorRows } from './WorkspaceSettingsPage'
import { useSettingsStore } from '@/stores/settings'
import { LS_THREAD_TEXT_WIDTH, THREAD_TEXT_WIDTH_DEFAULT } from '@/components/SessionView/threadTextWidth'

function row(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-settings-id="general.thread-text-width"]')
  if (!el) throw new Error('Thread text width row not rendered')
  return el
}

const slider = (): HTMLInputElement => within(row()).getByLabelText('Thread text width') as HTMLInputElement

beforeEach(() => {
  localStorage.clear()
  settingsUpdate.mockClear()
  useSettingsStore.setState({ threadTextWidth: THREAD_TEXT_WIDTH_DEFAULT })
})
afterEach(cleanup)

describe('Thread text width', () => {
  it('sits directly under New agents open in and is in search', () => {
    render(<WorkspaceBehaviorRows />)
    const ids = Array.from(document.querySelectorAll('[data-settings-id]')).map((el) =>
      el.getAttribute('data-settings-id'),
    )
    expect(ids[ids.indexOf('general.new-agent-view') + 1]).toBe('general.thread-text-width')
    expect(WORKSPACE_SETTINGS_MANIFEST.some((e) => e.id === 'general.thread-text-width')).toBe(true)
  })

  it('shows 820 at 480–960 in steps of 20, with no Reset at the default', () => {
    render(<WorkspaceBehaviorRows />)
    expect(slider().value).toBe('820')
    expect(slider().min).toBe('480')
    expect(slider().max).toBe('960')
    expect(slider().step).toBe('20')
    expect(screen.queryByTestId('thread-text-width-reset')).toBeNull()
  })

  it('the slider saves on this client and never the daemon', () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.change(slider(), { target: { value: '600' } })
    expect(useSettingsStore.getState().threadTextWidth).toEqual({ limited: true, px: 600 })
    expect(JSON.parse(localStorage.getItem(LS_THREAD_TEXT_WIDTH)!)).toEqual({ limited: true, px: 600 })
    expect(settingsUpdate).not.toHaveBeenCalled()
  })

  it('Full width turns the limit off, disables the slider and keeps the width', () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.change(slider(), { target: { value: '800' } })
    fireEvent.click(within(row()).getByRole('switch', { name: 'Full width' }))
    expect(useSettingsStore.getState().threadTextWidth).toEqual({ limited: false, px: 800 })
    expect(slider().disabled).toBe(true)
  })

  it('Reset appears away from the default and restores it', () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.change(slider(), { target: { value: '500' } })
    fireEvent.click(screen.getByTestId('thread-text-width-reset'))
    expect(useSettingsStore.getState().threadTextWidth).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(JSON.parse(localStorage.getItem(LS_THREAD_TEXT_WIDTH)!)).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(screen.queryByTestId('thread-text-width-reset')).toBeNull()
  })

  it('a daemon read-back does not overwrite the local pick', async () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.change(slider(), { target: { value: '640' } })
    await useSettingsStore.getState().fetchSettings()
    expect(useSettingsStore.getState().threadTextWidth).toEqual({ limited: true, px: 640 })
  })
})

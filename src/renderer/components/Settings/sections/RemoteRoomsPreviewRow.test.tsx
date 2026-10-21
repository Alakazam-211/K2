// @vitest-environment jsdom
// "Open other servers' agents in Home" graduated out of Experimental in
// 0.46.3 (Rosson 2026-10-21): the row lives in General → Workspaces (after
// the auto-select and new-agent-view rows), its search entry is in the
// Workspaces group, and the Experimental tab is gone.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { GENERAL_MANIFEST, WorkspaceBehaviorRows } from './GeneralSection'
import { REMOTE_ROOMS_LABEL } from './RemoteRoomsPreviewRow'
import { LS_REMOTE_ROOMS_PREVIEW, useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { webFeatures } from '@/web/features'

beforeEach(() => {
  localStorage.clear()
  useRemoteRoomsPreviewStore.setState({ enabled: true })
})

afterEach(() => {
  cleanup()
})

function settingsIds(): Array<string | null> {
  return Array.from(document.querySelectorAll('[data-settings-id]')).map((el) =>
    el.getAttribute('data-settings-id'),
  )
}

describe('graduated out of Experimental (0.46.3)', () => {
  it('the row renders in General → Workspaces, after New agents open in', () => {
    render(<WorkspaceBehaviorRows />)
    const ids = settingsIds()
    expect(ids.filter((id) => id === 'general.remote-rooms-preview')).toHaveLength(1)
    expect(ids.indexOf('general.remote-rooms-preview')).toBe(ids.indexOf('general.new-agent-view') + 1)
    expect(ids[ids.length - 1]).toBe('general.remote-rooms-preview')
    expect(ids).toContain('general.workspace-switch-focus')
    const row = document.querySelector<HTMLElement>('[data-settings-id="general.remote-rooms-preview"]')
    if (!row) throw new Error('remote rooms row not rendered in Workspaces')
    expect(within(row).getByText(REMOTE_ROOMS_LABEL)).toBeTruthy()
    const sw = within(row).getByRole('switch', { name: REMOTE_ROOMS_LABEL })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    // Still a toggle: people can turn it off.
    fireEvent.click(sw)
    expect(useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
  })

  it('the hosted web client does not show the row (single-server, MS56)', () => {
    const features = webFeatures as { multiHost: boolean }
    expect(features.multiHost).toBe(true)
    features.multiHost = false
    try {
      render(<WorkspaceBehaviorRows />)
      expect(settingsIds()).not.toContain('general.remote-rooms-preview')
      expect(screen.queryByText(REMOTE_ROOMS_LABEL)).toBeNull()
    } finally {
      features.multiHost = true
    }
  })

  it('the search entry moved from Experimental to Workspaces, with the plain label', () => {
    const entry = GENERAL_MANIFEST.find((e) => e.id === 'general.remote-rooms-preview')
    if (!entry) throw new Error('general.remote-rooms-preview missing from the search manifest')
    expect(entry.group).toBe('Workspaces')
    expect(entry.label).toBe(REMOTE_ROOMS_LABEL)
    expect(entry.keywords).not.toContain('experimental')
    const order = GENERAL_MANIFEST.map((e) => e.id)
    expect(order.indexOf('general.remote-rooms-preview')).toBe(order.indexOf('general.new-agent-view') + 1)
    expect(GENERAL_MANIFEST.filter((e) => e.group === 'Experimental')).toEqual([])
  })

  it('no Experimental tab or search mapping is left in Settings', () => {
    const dir = __dirname
    const general = readFileSync(join(dir, 'GeneralSection.tsx'), 'utf8')
    const settings = readFileSync(join(dir, '..', 'Settings.tsx'), 'utf8')
    const store = readFileSync(join(dir, '..', '..', '..', 'stores', 'settings.ts'), 'utf8')
    expect(general).not.toMatch(/id: 'experimental'/)
    expect(general).not.toMatch(/tab === 'experimental'/)
    expect(settings).not.toMatch(/Experimental: 'experimental'/)
    expect(store).not.toMatch(/'experimental'/)
  })
})

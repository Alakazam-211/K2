// @vitest-environment jsdom
// homes-sync-v1 — Settings → Network → Homes Sync: the devices syncing from
// this computer, Remove, Stop syncing / Sync again.

import { describe, it, expect, afterEach, vi } from 'vitest'
import { render, screen, cleanup, act, fireEvent, waitFor } from '@testing-library/react'
import { HomesSyncSection } from './HomesSyncSection'
import { FakeHomeServer } from '@/lib/homes-sync/fake-server'
import { INITIAL_STATUS, readControl, useHomesSyncStatus } from '@/lib/homes-sync/runner'
import { emptySyncState } from '@/lib/homes-sync/engine'
import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

class Mem {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
}

function setup(): { server: FakeHomeServer; kv: Mem } {
  const server = new FakeHomeServer('aaaaaaaaaaaaaaaa')
  server.register('dev-laptop', 'K2 desktop (macOS)', 'macos')
  server.register('dev-phone', 'Rosson’s iPhone', 'ios')
  const kv = new Mem()
  kv.setItem(HOMES_SYNC_STORAGE_KEY, JSON.stringify({ ...emptySyncState('dev-laptop'), merged: true }))
  return { server, kv }
}

describe('HomesSyncSection', () => {
  afterEach(() => {
    cleanup()
    useHomesSyncStatus.setState({ ...INITIAL_STATUS })
    vi.restoreAllMocks()
  })

  it('lists the devices syncing from this computer, this computer marked', async () => {
    const { server, kv } = setup()
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'synced' }))
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    const rows = screen.getAllByTestId('homes-sync-device').map((r) => r.textContent ?? '')
    expect(rows.some((t) => t.includes('K2 desktop (macOS)') && t.includes('(this computer)'))).toBe(true)
    expect(rows.some((t) => t.includes('Rosson’s iPhone') && t.includes('iPhone / iPad'))).toBe(true)
    expect(screen.getByTestId('homes-sync-status').textContent).toBe('Homes sync with This computer')
  })

  it('Remove takes a phone off the list after a confirm', async () => {
    const { server, kv } = setup()
    vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(true)
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Remove Rosson’s iPhone' }))
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(1))
    expect(server.devices.get('dev-phone')?.removed).toBe(true)
    expect(server.devices.get('dev-laptop')?.removed).toBe(false)
  })

  it('Stop syncing turns sync off and leaves the list; Start syncing bumps the control epoch', async () => {
    const { server, kv } = setup()
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'synced' }))
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Stop syncing' }))
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(1))
    expect(readControl(kv)).toEqual({ enabled: false, epoch: 0 })
    expect(server.devices.get('dev-laptop')?.removed).toBe(true)
    expect(screen.getByTestId('homes-sync-status').textContent).toBe('Homes sync is off: Homes stay on this computer')
    fireEvent.click(screen.getByRole('button', { name: 'Start syncing' }))
    expect(readControl(kv)).toEqual({ enabled: true, epoch: 1 })
  })

  it('a removed computer offers Sync again; a server without the feature says so', async () => {
    const { server, kv } = setup()
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'removed' }))
    const { unmount } = render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    expect(await screen.findByRole('button', { name: 'Sync again' })).toBeTruthy()
    unmount()
    server.supported = false
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    expect(screen.getByTestId('homes-sync-unsupported')).toBeTruthy()
  })
})

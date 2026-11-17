// @vitest-environment jsdom
// homes-sync-v1 — Settings → Network → Home Sync: the devices syncing from
// this computer, Remove, Change Home sync / Sync again.

import { describe, it, expect, afterEach, vi } from 'vitest'
import { render, screen, cleanup, act, fireEvent, waitFor } from '@testing-library/react'
import { HomesSyncSection } from './HomesSyncSection'
import { FakeHomeServer } from '@/lib/homes-sync/fake-server'
import { INITIAL_STATUS, readControl, useHomesSyncStatus } from '@/lib/homes-sync/runner'
import { emptySyncState } from '@/lib/homes-sync/engine'
import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'

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
  server.register('dev-phone', 'alex’s iPhone', 'ios')
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
    expect(rows.some((t) => t.includes('K2 desktop (macOS)') && t.includes('This computer'))).toBe(true)
    expect(rows.some((t) => t.includes('alex’s iPhone') && t.includes('iPhone / iPad') && !t.includes('This computer'))).toBe(true)
    expect(screen.getByTestId('homes-sync-title').textContent).toBe('On for this computer')
    expect(screen.getByTestId('homes-sync-status').textContent).toBe('Up to date.')
  })

  it('Remove takes a phone off the list after a confirm', async () => {
    const { server, kv } = setup()
    vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(true)
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Remove alex’s iPhone' }))
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(1))
    expect(server.devices.get('dev-phone')?.removed).toBe(true)
    expect(server.devices.get('dev-laptop')?.removed).toBe(false)
    expect(useConfirmDialogStore.getState().confirm).toHaveBeenCalledWith(
      expect.objectContaining({ title: 'Remove alex’s iPhone?', confirmLabel: 'Remove', destructive: true }),
    )
    expect(screen.getByTestId('homes-sync-list-outcome').textContent).toBe('Removed alex’s iPhone. Its groups stay on it.')
  })

  it('a failed Remove says what failed, keeps the device and gives the server’s reason', async () => {
    const { server, kv } = setup()
    vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(true)
    const transport = { ...server.transport(), remove: async () => { throw new Error('homes.db is locked') } }
    render(<HomesSyncSection transport={transport} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Remove alex’s iPhone' }))
    const note = await screen.findByTestId('homes-sync-list-outcome')
    expect(note.getAttribute('role')).toBe('alert')
    expect(note.textContent).toBe('Couldn’t remove alex’s iPhone. Nothing changed; try again.The server said: homes.db is locked.')
    expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2)
  })

  it('turning sync off when the server refuses keeps this computer syncing and says why', async () => {
    const { server, kv } = setup()
    vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(true)
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'synced' }))
    const transport = { ...server.transport(), remove: async () => { throw new Error('homes.db is locked') } }
    render(<HomesSyncSection transport={transport} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Change Home sync' }))
    const note = await screen.findByTestId('homes-sync-this-outcome')
    expect(note.textContent).toBe('Couldn’t take this computer off Home sync. Nothing changed; try again.The server said: homes.db is locked.')
    expect(readControl(kv).enabled).toBe(true)
    expect(screen.getByTestId('homes-sync-title').textContent).toBe('On for this computer')
  })

  it('a list that fails to load shows the error card with Try again, then the list', async () => {
    const { server, kv } = setup()
    let failing = true
    const base = server.transport()
    const transport = { ...base, listDevices: async (id: string) => { if (failing) throw new Error('homes.db is locked'); return base.listDevices(id) } }
    render(<HomesSyncSection transport={transport} kv={kv} />)
    const card = await screen.findByTestId('homes-sync-devices-error')
    expect(card.textContent).toContain('Couldn’t load the devices')
    expect(card.textContent).toContain('The server said: homes.db is locked.')
    failing = false
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
  })

  it('an answer that is not a device list is a plain error, not a parser message', async () => {
    const { server, kv } = setup()
    render(<HomesSyncSection transport={{ ...server.transport(), listDevices: async () => ({}) }} kv={kv} />)
    const card = await screen.findByTestId('homes-sync-devices-error')
    expect(card.textContent).toContain('sent a list K2 couldn’t read')
    expect(card.textContent).not.toContain('homes sync:')
  })

  it('no devices yet offers to pair a phone', async () => {
    const { server, kv } = setup()
    render(<HomesSyncSection transport={{ ...server.transport(), listDevices: async () => ({ devices: [] }) }} kv={kv} />)
    expect((await screen.findByTestId('homes-sync-no-devices')).textContent).toContain('No devices sync here yet')
    expect(screen.getByRole('button', { name: 'Pair a phone' })).toBeTruthy()
  })

  it('Change Home sync turns sync off after a confirm and leaves the list; Start syncing bumps the control epoch', async () => {
    const { server, kv } = setup()
    const confirm = vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(true)
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'synced' }))
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    expect(screen.queryByRole('button', { name: 'Stop syncing' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Change Home sync' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(confirm.mock.calls[0][0]).toMatchObject({ title: 'Change Home sync', confirmLabel: 'Turn off for this computer' })
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(1))
    expect(readControl(kv)).toEqual({ enabled: false, epoch: 0 })
    expect(server.devices.get('dev-laptop')?.removed).toBe(true)
    expect(screen.getByTestId('homes-sync-title').textContent).toBe('Off for this computer')
    expect(screen.getByTestId('homes-sync-this-outcome').textContent).toBe('Home sync is off for this computer. Its groups stay here.')
    fireEvent.click(screen.getByRole('button', { name: 'Start syncing' }))
    expect(readControl(kv)).toEqual({ enabled: true, epoch: 1 })
  })

  it('a removed computer offers Sync again; a server without the feature says so', async () => {
    const { server, kv } = setup()
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'removed' }))
    const { unmount } = render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    expect(await screen.findByRole('button', { name: 'Sync again' })).toBeTruthy()
    expect(screen.getByTestId('homes-sync-title').textContent).toBe('Taken off Home sync')
    unmount()
    server.supported = false
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    expect(screen.getByTestId('homes-sync-unsupported').textContent).toContain('This computer’s K2 server doesn’t sync groups yet')
    act(() => useSettingsStore.setState({ activeSection: 'homes-sync' }))
    fireEvent.click(screen.getByRole('button', { name: 'Check for updates' }))
    expect(useSettingsStore.getState().pendingUpdateCheck).toBe(true)
    expect(useSettingsStore.getState().activeSection).toBe('general')
    act(() => useSettingsStore.setState({ pendingUpdateCheck: false }))
  })
})

describe('Change Home sync, cancelled', () => {
  it('keeps syncing when the person cancels', async () => {
    const { server, kv } = setup()
    vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(false)
    act(() => useHomesSyncStatus.setState({ ...INITIAL_STATUS, phase: 'synced' }))
    render(<HomesSyncSection transport={server.transport()} kv={kv} />)
    await waitFor(() => expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2))
    fireEvent.click(screen.getByRole('button', { name: 'Change Home sync' }))
    await waitFor(() => expect(useConfirmDialogStore.getState().confirm).toHaveBeenCalled())
    expect(readControl(kv).enabled).toBe(true)
    expect(screen.getAllByTestId('homes-sync-device')).toHaveLength(2)
  })
})

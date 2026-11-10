// @vitest-environment jsdom
// "Couldn't read the list" is not the same as "no agents", and Try again
// reads it again. Removing a connection asks first.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const list = vi.hoisted(() => vi.fn())
const remove = vi.hoisted(() => vi.fn(async () => undefined))
vi.mock('@/lib/federation', async (orig) => ({
  ...(await orig<typeof import('@/lib/federation')>()),
  listAllRemoteConnections: list,
  removeRemoteConnection: remove,
}))

import { FederationOverview } from './FederationOverview'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const CONN = {
  remote: true,
  address: 'ai::rpm.k2.dev',
  host: 'rpm.k2.dev',
  agent: 'ai',
  gone: false,
  sourceWorkspace: 'Cortana',
  sourcePath: '/srv/cortana',
}

async function clickRemove(): Promise<void> {
  list.mockResolvedValue([CONN])
  render(<FederationOverview />)
  fireEvent.click(await screen.findByRole('button', { name: 'Remove' }))
  await waitFor(() => expect(useConfirmDialogStore.getState().isOpen).toBe(true))
}

afterEach(() => {
  cleanup()
  list.mockReset()
  remove.mockClear()
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null })
})

describe('FederationOverview', () => {
  it('says the list could not be read, and Try again recovers', async () => {
    list.mockRejectedValueOnce(new Error('internal error'))
    list.mockResolvedValueOnce([])
    render(<FederationOverview />)
    await waitFor(() => screen.getByText('Couldn’t load agents on other servers.'))
    expect(screen.getByText('internal error')).toBeTruthy()
    expect(screen.queryByText('No agents connected yet.')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => screen.getByText('No agents connected yet.'))
  })

  it('Remove asks first, names the agent and says what happens', async () => {
    await clickRemove()
    const dialog = useConfirmDialogStore.getState()
    expect(dialog.confirmDestructive).toBe(true)
    expect(dialog.confirmLabel).toBe('Remove')
    expect(dialog.title).toBe('Remove ai on rpm.k2.dev?')
    expect(dialog.message).toContain('Cortana will no longer be connected to ai')
    expect(dialog.message).toContain('The server stays paired')
    expect(remove).not.toHaveBeenCalled()
  })

  it('cancelling keeps the connection', async () => {
    await clickRemove()
    act(() => useConfirmDialogStore.getState().close())
    await act(async () => {})
    expect(remove).not.toHaveBeenCalled()
  })

  it('confirming removes the connection', async () => {
    await clickRemove()
    act(() => useConfirmDialogStore.getState().onResolve?.(true))
    await waitFor(() => expect(remove).toHaveBeenCalledWith('/srv/cortana', 'ai::rpm.k2.dev'))
  })
})

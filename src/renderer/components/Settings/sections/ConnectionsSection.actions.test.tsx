// @vitest-environment jsdom
// Connected Servers: each server shows one button at most, and every action
// the old tile had is still reachable from that button or the row's menu,
// with the same disabled rules. Remove asks first.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => undefined }))
vi.mock('./FederationOverview', () => ({ FederationOverview: () => null }))
vi.mock('@/lib/host-pool-instance', () => ({ signOutOfHost: vi.fn(async () => undefined) }))

const net = vi.hoisted(() => ({ federation: true as boolean, paired: false, online: true }))
vi.mock('@/lib/host-ops', async (orig) => ({
  ...(await orig<typeof import('@/lib/host-ops')>()),
  hostOpGet: vi.fn(async () => ({ federationEnabled: net.federation })),
  hostBootStatus: vi.fn(async () => (net.online ? { phase: 'ready', version: '1.0.0' } : null)),
}))
vi.mock('@/lib/federation', async (orig) => ({
  ...(await orig<typeof import('@/lib/federation')>()),
  isTrustedPeerHost: vi.fn(async () => net.paired),
}))

import { ConnectionsSection } from './ConnectionsSection'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const base: ConnectHost = {
  id: 'h1',
  label: 'z3',
  hostname: 'z3.k2.dev',
  port: 443,
  secure: true,
  username: 'rosson',
  token: 't',
  remember: true,
  lastConnectedAt: null,
}

function show(host: ConnectHost): void {
  useConnectHostStore.setState({ activeHost: 'local', hosts: [host], connectionStatus: 'connected' })
  render(<ConnectionsSection />)
}

function row(): HTMLElement {
  return document.querySelector<HTMLElement>('[data-server-row="z3"]')!
}

async function menuLabels(): Promise<string[]> {
  fireEvent.click(within(row()).getByRole('button', { name: 'More actions for z3' }))
  return screen.getAllByRole('menuitem').map((b) => b.firstChild?.textContent ?? '')
}

async function pickRemove(): Promise<void> {
  await waitFor(() => within(row()).getByText('Online'))
  await menuLabels()
  fireEvent.click(screen.getByRole('menuitem', { name: 'Remove' }))
}

beforeEach(() => {
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null })
  net.federation = true
  net.paired = false
  net.online = true
})
afterEach(cleanup)

describe('Connected Servers row actions', () => {
  it('signed in, not paired: Pair is the button and the rest are in the menu', async () => {
    show(base)
    await waitFor(() => within(row()).getByRole('button', { name: 'Pair' }))
    expect(within(row()).getByText('Online')).toBeTruthy()
    expect(await menuLabels()).toEqual(['Restart', 'Check for updates', 'Sign out', 'Edit', 'Remove'])
  })

  it('already paired: no button, Check pairing is in the menu', async () => {
    net.paired = true
    show(base)
    await waitFor(() => within(row()).getByText('Paired'))
    expect(within(row()).queryByRole('button', { name: 'Pair' })).toBeNull()
    expect(await menuLabels()).toEqual(['Check pairing', 'Restart', 'Check for updates', 'Sign out', 'Edit', 'Remove'])
  })

  it('federation off: Pair stays in the menu, disabled, and says why', async () => {
    net.federation = false
    show(base)
    await waitFor(() => within(row()).getByText('Federation: off'))
    await waitFor(() => expect(within(row()).queryByText('Checking…')).toBeNull())
    await menuLabels()
    const pair = screen.getAllByRole('menuitem')[0]
    expect(pair.getAttribute('aria-disabled')).toBe('true')
    expect(pair.textContent).toContain('Turn on federation on this server and on this computer first')
  })

  it('signed out: Sign in is the button; only Edit and Remove are in the menu', async () => {
    show({ ...base, token: '' })
    await waitFor(() => within(row()).getByText('Signed out'))
    within(row()).getByRole('button', { name: 'Sign in' })
    expect(await menuLabels()).toEqual(['Edit', 'Remove'])
  })

  it('offline: says so, offers Try again, and keeps every action in the menu', async () => {
    net.online = false
    show(base)
    await waitFor(() => within(row()).getByText('Offline'))
    within(row()).getByRole('button', { name: 'Try again' })
    expect(await menuLabels()).toEqual(['Pair', 'Restart', 'Check for updates', 'Sign out', 'Edit', 'Remove'])
  })

  it('Remove asks first, names the server and says what happens', async () => {
    show(base)
    await pickRemove()
    const dialog = useConfirmDialogStore.getState()
    expect(dialog.isOpen).toBe(true)
    expect(dialog.confirmDestructive).toBe(true)
    expect(dialog.confirmLabel).toBe('Remove')
    expect(dialog.title).toBe('Remove z3?')
    expect(dialog.message).toContain('K2 forgets z3 on this computer')
    expect(useConnectHostStore.getState().hosts).toHaveLength(1)
  })

  it('cancelling Remove keeps the server', async () => {
    show(base)
    await pickRemove()
    act(() => useConfirmDialogStore.getState().close())
    await act(async () => {})
    expect(useConnectHostStore.getState().hosts).toHaveLength(1)
  })

  it('confirming Remove removes the server', async () => {
    show(base)
    await pickRemove()
    act(() => useConfirmDialogStore.getState().onResolve?.(true))
    await waitFor(() => expect(useConnectHostStore.getState().hosts).toEqual([]))
  })

  it('no servers: says so and offers Add a server', () => {
    useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connected' })
    render(<ConnectionsSection />)
    expect(screen.getByText('No saved servers yet.')).toBeTruthy()
    fireEvent.click(within(screen.getByTestId('connections-empty')).getByRole('button', { name: '+ Add a server' }))
    expect(screen.getByRole('dialog', { name: 'Add server' })).toBeTruthy()
  })
})

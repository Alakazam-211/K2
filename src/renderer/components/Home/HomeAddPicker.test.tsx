// @vitest-environment jsdom
//
// prd-home-picker-and-remote-avatars-v1 S2 — From a server (T2.4, T2.5;
// vs-live P23, P24, P36). One list with a section per saved server, each
// listed with that server's own login: an ok list with images (only
// `data:image/` paints), a sign-in, an offline server with Retry, a 403
// (no access) and a 401 (sign in again); search keeps the sections that
// still have a status; no row asks the connected server for an icon; a
// pool check that just found a server offline skips the fetch; and a
// reopen within 60 s doesn't refetch.

import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({
  default: () => null,
  hostDisplayAddress: (h: { hostname: string; port: number; secure: boolean }) =>
    h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`,
}))
const cli = vi.hoisted(() => ({ daemonCliGet: vi.fn(async () => ({ found: false, dataUrl: null })) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: cli.daemonCliGet,
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
// One attempt per request: the retry schedule (~3.6 s for a refused
// connection) has its own tests in remote-retry.test.ts. The offline
// classifier stays real.
vi.mock('@/lib/remote-retry', async () => {
  const real = await vi.importActual<typeof import('@/lib/remote-retry')>('@/lib/remote-retry')
  return { ...real, withRemoteRetry: async <T,>(op: () => Promise<T>): Promise<T> => op() }
})

import { AddAgentPicker, __resetAddPickerListingsForTests, listedIconUrl, listingForError } from './HomeAddPanels'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { __resetHostPoolForTests, hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { HostOpError } from '@/lib/host-ops'

if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
Element.prototype.scrollIntoView = vi.fn()

const PNG = 'data:image/png;base64,iVBORw0KGgo='
const CACHED = 'data:image/png;base64,Q0FDSEVE'

function host(id: string, label: string, token: string): ConnectHost {
  return {
    id,
    label,
    hostname: `${id}.k2.dev`,
    username: 'rosson',
    port: 443,
    secure: true,
    token,
    remember: true,
    lastConnectedAt: null,
  }
}

const boxA = host('a', 'Alpha Box', 'tok-a')
const boxB = host('b', 'Bravo Box', '')
const boxC = host('c', 'Charlie Box', 'tok-c')
const boxD = host('d', 'Delta Box', 'tok-d')
const boxE = host('e', 'Echo Box', 'tok-e')

const fetchMock = vi.fn(async (url: string): Promise<Response> => {
  if (url === 'https://a.k2.dev/cli/projects/list?token=tok-a') {
    return new Response(
      JSON.stringify([
        { id: 'a1', name: 'Atlas', handle: 'atlas', path: '/srv/atlas', color: '#ff0000', iconUrl: PNG },
        { id: 'a2', name: 'Ark', handle: 'ark', path: '/srv/ark', color: '#00ff00', iconUrl: 'https://evil.example/x.png' },
        { id: 'a3', name: 'Aster', handle: 'aster', path: '/srv/aster', color: '#0000ff', iconUrl: null },
      ]),
      { status: 200 },
    )
  }
  if (url === 'https://b.k2.dev/cli/projects/list?token=tok-b-new') {
    return new Response(JSON.stringify([{ id: 'b1', name: 'Birch', handle: 'birch', path: '/srv/birch' }]), {
      status: 200,
    })
  }
  if (url.startsWith('https://c.k2.dev/')) throw new TypeError('Failed to fetch')
  if (url.startsWith('https://d.k2.dev/')) {
    return new Response(JSON.stringify({ error: 'role_required' }), { status: 403 })
  }
  if (url.startsWith('https://e.k2.dev/')) {
    return new Response(JSON.stringify({ error: 'session expired' }), { status: 401 })
  }
  throw new Error(`unexpected fetch ${url}`)
})

const signIn = vi.fn()

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  fetchMock.mockClear()
  cli.daemonCliGet.mockClear()
  signIn.mockClear()
  __resetConnectHostStoreForTests()
  __resetHostPoolForTests()
  __resetAddPickerListingsForTests()
  useConnectHostStore.setState({
    hosts: [boxE, boxC, boxA, boxD, boxB],
    connectionStatus: 'connected',
    signInForManagement: signIn,
  })
  const home = useHomesStore.getState().homes[0]
  for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
  useHomesStore.getState().selectHome(home.id)
  useHomesStore.getState().addRow(home.id, { address: 'atlas::a.k2.dev', workspaceId: 'a1', label: 'Atlas' })
})

afterEach(() => cleanup())
afterAll(() => vi.unstubAllGlobals())

function Picker(): React.JSX.Element {
  const home = useHomesStore(selectedHome)
  return <AddAgentPicker home={home} avatarFor={(address) => (address === 'aster::a.k2.dev' ? CACHED : null)} />
}

async function openFromServer(): Promise<HTMLElement> {
  render(<Picker />)
  const picker = screen.getByRole('dialog', { name: 'Add Agent' })
  fireEvent.click(within(picker).getByText('From a server'))
  await waitFor(() => expect(within(picker).queryByText('Loading agents…')).toBeNull())
  return picker
}

function section(picker: HTMLElement, label: string): HTMLElement {
  return within(picker).getByRole('group', { name: label })
}

function listCalls(hostId: string): number {
  return fetchMock.mock.calls.filter((c) => String(c[0]).startsWith(`https://${hostId}.k2.dev/cli/projects/list`)).length
}

describe('Add Agent — From a server (T2.4, T2.5)', () => {
  it('one section per saved server, by label, each with its own state', async () => {
    const picker = await openFromServer()
    expect(within(picker).getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual([
      'Alpha Box',
      'Bravo Box',
      'Charlie Box',
      'Delta Box',
      'Echo Box',
    ])
    // Ok: rows, alphabetical; Atlas is on this Home already.
    const a = section(picker, 'Alpha Box')
    expect(within(a).getAllByRole('option').map((o) => o.getAttribute('data-row-state'))).toEqual([
      'pickable',
      'pickable',
      'checked',
    ])
    // Letter avatar (Ark) or image (Atlas, Aster), name, handle.
    expect(within(a).getAllByRole('option').map((o) => o.textContent)).toEqual(['AArkark', 'Asteraster', 'Atlasatlas'])
    // No login: Sign in, and no request for its agents.
    const b = section(picker, 'Bravo Box')
    expect(b.textContent).toContain('Sign in to Bravo Box to list its agents.')
    expect(listCalls('b')).toBe(0)
    // A fetch that rejects with a TypeError: Offline + Retry.
    const c = section(picker, 'Charlie Box')
    expect(c.querySelector('[data-listing="offline"]')?.textContent).toBe('OfflineRetry')
    // 403: no access. 401: sign in again.
    expect(section(picker, 'Delta Box').querySelector('[data-listing="no-access"]')?.textContent).toBe(
      'Your login can’t list agents here',
    )
    expect(section(picker, 'Echo Box').textContent).toContain('Sign in to Echo Box to list its agents.')
  })

  it('images: data:image/ paints, https:// paints the letter, the cached hook fills a missing one', async () => {
    const picker = await openFromServer()
    const a = section(picker, 'Alpha Box')
    const img = (name: RegExp): string | null =>
      within(a).getByRole('option', { name }).querySelector('img')?.getAttribute('src') ?? null
    expect(img(/Atlas/)).toBe(PNG)
    expect(img(/Ark/)).toBeNull()
    expect(img(/Aster/)).toBe(CACHED)
    expect(Array.from(picker.querySelectorAll('img')).filter((i) => (i.getAttribute('src') ?? '').startsWith('https:'))).toEqual([])
    // T2.5: no row asked the connected server for an icon.
    expect(cli.daemonCliGet).not.toHaveBeenCalled()
  })

  it('Sign in calls signInForManagement with that host; the token landing re-lists it', async () => {
    const picker = await openFromServer()
    fireEvent.click(within(section(picker, 'Bravo Box')).getByRole('button', { name: 'Sign in' }))
    expect(signIn).toHaveBeenCalledTimes(1)
    expect(signIn.mock.calls[0][0]).toMatchObject({ id: 'b', hostname: 'b.k2.dev' })
    await act(async () => {
      useConnectHostStore.setState({
        hosts: useConnectHostStore.getState().hosts.map((h) => (h.id === 'b' ? { ...h, token: 'tok-b-new' } : h)),
      })
    })
    await waitFor(() => expect(within(section(picker, 'Bravo Box')).getAllByRole('option').length).toBe(1))
    expect(listCalls('b')).toBe(1)
  })

  it('Retry fetches the offline server again', async () => {
    const picker = await openFromServer()
    expect(listCalls('c')).toBe(1)
    fireEvent.click(within(section(picker, 'Charlie Box')).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(listCalls('c')).toBe(2))
    await waitFor(() => expect(section(picker, 'Charlie Box').querySelector('[data-listing="offline"]')).not.toBeNull())
  })

  it('a query that matches nothing on Alpha keeps the sections that have a status', async () => {
    const picker = await openFromServer()
    fireEvent.change(within(picker).getByRole('combobox'), { target: { value: 'birch' } })
    expect(within(picker).queryByRole('group', { name: 'Alpha Box' })).toBeNull()
    expect(within(picker).getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual([
      'Bravo Box',
      'Charlie Box',
      'Delta Box',
      'Echo Box',
    ])
    expect(section(picker, 'Charlie Box').querySelector('[data-listing="offline"]')).not.toBeNull()
    fireEvent.change(within(picker).getByRole('combobox'), { target: { value: '/srv/ast' } })
    expect(within(section(picker, 'Alpha Box')).getAllByRole('option').map((o) => o.textContent)).toEqual(['Asteraster'])
  })

  it('adding keeps the picker open; the row turns checked; clicking it again adds nothing', async () => {
    const picker = await openFromServer()
    const a = section(picker, 'Alpha Box')
    fireEvent.click(within(a).getByRole('option', { name: /Ark/ }))
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toEqual(['atlas::a.k2.dev', 'ark::a.k2.dev'])
    const ark = within(section(picker, 'Alpha Box')).getByRole('option', { name: /Ark/ })
    expect(ark.getAttribute('data-row-state')).toBe('checked')
    fireEvent.click(ark)
    expect(useHomesStore.getState().homes[0].rows.length).toBe(2)
    expect(screen.getByRole('dialog', { name: 'Add Agent' })).toBe(picker)
  })

  it('a pool check that just found a server offline shows Offline without a fetch (P24)', async () => {
    const entry: HostEntry = {
      hostKey: 'c.k2.dev',
      saved: true,
      hostId: 'c',
      reach: 'offline',
      boot: null,
      auth: 'ok',
      authNote: null,
      role: null,
      presence: null,
      offlineStreak: 1,
      checkedAt: Date.now() - 5_000,
    }
    hostPool.store.setState({ entries: { 'c.k2.dev': entry } })
    const picker = await openFromServer()
    expect(section(picker, 'Charlie Box').querySelector('[data-listing="offline"]')).not.toBeNull()
    expect(listCalls('c')).toBe(0)
    // Retry still fetches.
    fireEvent.click(within(section(picker, 'Charlie Box')).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(listCalls('c')).toBe(1))
  })

  it('open, close, open within 60 s lists each server once (P36)', async () => {
    await openFromServer()
    expect(listCalls('a')).toBe(1)
    cleanup()
    const picker = await openFromServer()
    expect(within(section(picker, 'Alpha Box')).getAllByRole('option').length).toBe(3)
    expect(listCalls('a')).toBe(1)
    // A failed server is not memoised.
    expect(listCalls('c')).toBe(2)
  })

  it('at most 4 listings run at once', async () => {
    let inFlight = 0
    let peak = 0
    const gate: (() => void)[] = []
    const hosts = Array.from({ length: 7 }, (_, i) => host(`s${i}`, `Server ${i}`, `tok-${i}`))
    useConnectHostStore.setState({ hosts })
    const gated = vi.fn(async (): Promise<Response> => {
      inFlight++
      peak = Math.max(peak, inFlight)
      await new Promise<void>((r) => gate.push(r))
      inFlight--
      return new Response('[]', { status: 200 })
    })
    vi.stubGlobal('fetch', gated)
    render(<Picker />)
    fireEvent.click(within(screen.getByRole('dialog', { name: 'Add Agent' })).getByText('From a server'))
    await waitFor(() => expect(gate.length).toBe(4))
    while (gate.length > 0 || inFlight > 0) {
      await act(async () => {
        gate.shift()?.()
        await new Promise((r) => setTimeout(r, 0))
      })
    }
    expect(peak).toBe(4)
    expect(gated).toHaveBeenCalledTimes(7)
    await waitFor(() => expect(screen.getAllByText('No agents').length).toBe(7))
  })
})

describe('pure helpers', () => {
  it('listedIconUrl keeps data:image/ only', () => {
    expect(listedIconUrl(PNG)).toBe(PNG)
    expect(listedIconUrl('https://x/y.png')).toBeNull()
    expect(listedIconUrl('data:text/html,<b>')).toBeNull()
    expect(listedIconUrl(null)).toBeNull()
    expect(listedIconUrl(42)).toBeNull()
  })

  it('listingForError maps 401, 403, network, timeout and the rest (P23)', () => {
    expect(listingForError(new HostOpError('session expired', 401), boxE)).toEqual({ kind: 'signin', host: boxE })
    expect(listingForError(new HostOpError('session expired', 401), null)).toEqual({
      kind: 'error',
      message: 'session expired',
    })
    expect(listingForError(new HostOpError('role_required', 403), boxD)).toEqual({ kind: 'no-access' })
    expect(listingForError(new HostOpError('boom', 500), boxD)).toEqual({ kind: 'error', message: 'boom' })
    expect(listingForError(new TypeError('Load failed'), boxC)).toEqual({ kind: 'offline' })
    const timeout = new Error('The operation timed out.')
    timeout.name = 'TimeoutError'
    expect(listingForError(timeout, boxC)).toEqual({ kind: 'offline' })
    expect(listingForError(new Error('weird'), boxC)).toEqual({ kind: 'error', message: 'weird' })
  })
})

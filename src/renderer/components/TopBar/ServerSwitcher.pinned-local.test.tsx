// @vitest-environment jsdom
//
// This computer is pinned above the scrolling server list: it renders
// outside the scroller, exactly once, stays while searching, and ↑/↓ cross
// between it and the list. Fail loud on any of these.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useServerSwitcherStore } from '@/stores/server-switcher'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) =>
    sel({ openSettings: () => {} }),
}))

vi.mock('@/lib/remote-session', () => ({
  reviveRemoteSession: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import ServerSwitcher, { THIS_COMPUTER_LABEL } from './ServerSwitcher'

function host(n: number): ConnectHost {
  const id = `box${String(n).padStart(2, '0')}`
  return {
    id,
    label: `Box ${String(n).padStart(2, '0')}`,
    hostname: `${id}.k2.dev`,
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

const pickHost = vi.fn<(host: 'local' | ConnectHost) => void>()
const realPickHost = useConnectHostStore.getState().pickHost

function one(root: ParentNode, sel: string): HTMLElement {
  const all = root.querySelectorAll(sel)
  if (all.length !== 1) throw new Error(`expected exactly one ${sel}, found ${all.length}`)
  const el = all[0]
  if (!(el instanceof HTMLElement)) throw new Error(`${sel} is not an element`)
  return el
}

function searchBox(root: ParentNode): HTMLInputElement {
  const el = one(root, 'input[aria-label="Search servers"]')
  if (!(el instanceof HTMLInputElement)) throw new Error('search box is not an input')
  return el
}

function localRows(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>('button[role="option"]')).filter(
    (b) => b.textContent?.includes(THIS_COMPUTER_LABEL),
  )
}

function highlightedId(root: ParentNode): string {
  return one(root, '[data-switcher-highlighted="true"]').id
}

function openWith(hosts: ConnectHost[], active: 'local' | ConnectHost = 'local'): HTMLElement {
  act(() => {
    useConnectHostStore.setState({ activeHost: active, hosts, pickHost })
  })
  const { container } = render(<ServerSwitcher />)
  act(() => {
    useServerSwitcherStore.getState().setOpen(true)
  })
  return container
}

describe('server switcher pins This computer', () => {
  beforeEach(() => {
    // jsdom has no layout. The open menu scrolls the highlighted row into view.
    Element.prototype.scrollIntoView = () => {}
    pickHost.mockClear()
    act(() => {
      useServerSwitcherStore.setState({ open: false })
    })
  })

  afterEach(() => {
    cleanup()
    act(() => {
      useServerSwitcherStore.setState({ open: false })
    })
    useConnectHostStore.setState({ pickHost: realPickHost, activeHost: 'local', hosts: [] })
    vi.restoreAllMocks()
  })

  it('renders This computer outside the scroll container, exactly once', () => {
    const c = openWith([host(1), host(2)])
    const pinned = one(c, '[data-testid="server-switcher-pinned"]')
    const scroll = one(c, '[data-testid="server-switcher-scroll"]')
    const rows = localRows(c)
    expect(rows).toHaveLength(1)
    expect(pinned.contains(rows[0])).toBe(true)
    expect(scroll.contains(rows[0])).toBe(false)
    expect(localRows(scroll)).toHaveLength(0)
    // Pinned sits above the scroller, both inside the one listbox.
    const listbox = one(c, '[role="listbox"]')
    expect(listbox.contains(pinned)).toBe(true)
    expect(listbox.contains(scroll)).toBe(true)
    expect(pinned.compareDocumentPosition(scroll) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Connected state: the active local row shows the status dot and check.
    expect(rows[0].querySelector('[aria-label^="connection "]')).not.toBeNull()
    expect(rows[0].querySelector('svg')).not.toBeNull()
  })

  it('selecting This computer calls pickHost("local")', () => {
    const c = openWith([host(1)], host(1))
    const rows = localRows(c)
    expect(rows).toHaveLength(1)
    // Not active while a remote is: no dot on the pinned row.
    expect(rows[0].querySelector('[aria-label^="connection "]')).toBeNull()
    fireEvent.click(rows[0])
    expect(pickHost.mock.calls).toEqual([['local']])
    expect(useServerSwitcherStore.getState().open).toBe(false)
  })

  it('arrow keys cross between the pinned row and the list', () => {
    const c = openWith([host(1), host(2)])
    const input = searchBox(c)
    expect(highlightedId(c)).toBe('server-switcher-opt-local')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(highlightedId(c)).toBe('server-switcher-opt-box01')
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(highlightedId(c)).toBe('server-switcher-opt-local')
    // Up from the pinned row wraps to the last list row, and back down.
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(highlightedId(c)).toBe('server-switcher-opt-box02')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(highlightedId(c)).toBe('server-switcher-opt-local')
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(pickHost.mock.calls).toEqual([['local']])
  })

  it('with 30 servers the scroller holds all 30 and This computer stays pinned', () => {
    const hosts = Array.from({ length: 30 }, (_, i) => host(i + 1))
    const c = openWith(hosts)
    const pinned = one(c, '[data-testid="server-switcher-pinned"]')
    const scroll = one(c, '[data-testid="server-switcher-scroll"]')
    expect(scroll.className).toContain('overflow-y-auto')
    expect(scroll.className).toMatch(/max-h-/)
    expect(pinned.className).not.toContain('overflow')
    expect(scroll.querySelectorAll('button[role="option"]')).toHaveLength(30)
    expect(pinned.querySelectorAll('button[role="option"]')).toHaveLength(1)
    // Siblings, not nested: the scroller can't scroll the pinned block away.
    expect(scroll.contains(pinned)).toBe(false)
    expect(pinned.contains(scroll)).toBe(false)
    expect(pinned.parentElement).toBe(scroll.parentElement)
    const input = searchBox(c)
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(highlightedId(c)).toBe('server-switcher-opt-box30')
    expect(localRows(c)).toHaveLength(1)
  })

  it('a search that misses This computer keeps it pinned but highlights the first matching host', () => {
    const c = openWith([host(1), host(2)])
    const input = searchBox(c)
    fireEvent.change(input, { target: { value: 'box 02' } })
    const rows = localRows(c)
    expect(rows).toHaveLength(1)
    expect(one(c, '[data-testid="server-switcher-pinned"]').contains(rows[0])).toBe(true)
    expect(highlightedId(c)).toBe('server-switcher-opt-box02')
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(highlightedId(c)).toBe('server-switcher-opt-local')
  })

  it('a search that matches nothing still shows This computer and says no match', () => {
    const c = openWith([host(1)])
    const input = searchBox(c)
    fireEvent.change(input, { target: { value: 'zzz' } })
    expect(localRows(c)).toHaveLength(1)
    expect(c.textContent).toContain('No matching servers')
    expect(highlightedId(c)).toBe('server-switcher-opt-local')
  })
})

// @vitest-environment jsdom
//
// 0.46.0 experiment (Rosson 2026-10-16): the server switcher's trigger is one
// fixed width whatever the server is called. A long name truncates with an
// ellipsis inside that width (full name in the tooltip) instead of growing
// the control. The dropdown still opens below it at its own width.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useServerSwitcherStore } from '@/stores/server-switcher'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) => sel({ openSettings: () => {} }),
}))
vi.mock('@/lib/remote-session', () => ({ reviveRemoteSession: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import ServerSwitcher, {
  SERVER_SWITCHER_WIDTH_CLASS,
  SERVER_SWITCHER_WIDTH_PX,
  THIS_COMPUTER_LABEL,
} from './ServerSwitcher'

function host(id: string, label: string): ConnectHost {
  return {
    id,
    label,
    hostname: `${id}.k2.dev`,
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

const LONG = 'a-really-long-customer-server-name.example.k2.dev'
const pickHost = vi.fn<(h: 'local' | ConnectHost) => void>()
const realPickHost = useConnectHostStore.getState().pickHost

function renderOn(active: 'local' | ConnectHost, hosts: ConnectHost[] = []): HTMLButtonElement {
  act(() => {
    useConnectHostStore.setState({ activeHost: active, hosts, pickHost })
  })
  render(<ServerSwitcher />)
  const t = screen.getByTestId('server-switcher-trigger')
  if (!(t instanceof HTMLButtonElement)) throw new Error('trigger is not a button')
  return t
}

describe('server switcher trigger: fixed width, ellipsis', () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = () => {}
    act(() => useServerSwitcherStore.setState({ open: false }))
  })

  afterEach(() => {
    cleanup()
    act(() => useServerSwitcherStore.setState({ open: false }))
    useConnectHostStore.setState({ pickHost: realPickHost, activeHost: 'local', hosts: [] })
  })

  it('the width class is the 160px constant and does not shrink', () => {
    expect(SERVER_SWITCHER_WIDTH_PX).toBe(160)
    expect(SERVER_SWITCHER_WIDTH_CLASS).toBe(`w-[${SERVER_SWITCHER_WIDTH_PX}px] shrink-0`)
  })

  it('a short and a long name get the same fixed-width trigger', () => {
    const short = renderOn(host('rosson', 'rosson.k2.dev'))
    const shortClass = short.className
    cleanup()
    const long = renderOn(host('long', LONG))
    for (const cls of [shortClass, long.className]) {
      expect(cls).toContain(`w-[${SERVER_SWITCHER_WIDTH_PX}px]`)
      expect(cls).toContain('shrink-0')
      expect(cls).not.toMatch(/max-w-/)
    }
  })

  it('the name truncates with an ellipsis inside the width (no max-width of its own)', () => {
    renderOn(host('long', LONG))
    const label = screen.getByTestId('server-switcher-label')
    expect(label.textContent).toBe(LONG)
    expect(label.className).toContain('truncate')
    expect(label.className).toContain('min-w-0')
    expect(label.className).toContain('flex-1')
    expect(label.className).not.toMatch(/max-w-/)
  })

  it('the tooltip carries the full name and the connection state', () => {
    const t = renderOn(host('long', LONG))
    const title = t.getAttribute('title') ?? ''
    expect(title.startsWith(`${LONG} — `)).toBe(true)
    expect(title).toContain('Connection:')
    expect(title.endsWith('(⌘L)')).toBe(true)
  })

  it('This computer: label and tooltip name it', () => {
    const t = renderOn('local')
    expect(screen.getByTestId('server-switcher-label').textContent).toBe(THIS_COMPUTER_LABEL)
    expect(t.getAttribute('title')?.startsWith(`${THIS_COMPUTER_LABEL} — `)).toBe(true)
  })

  it('the dropdown still opens below the trigger at its own 260px width, not the trigger width', () => {
    const t = renderOn(host('long', LONG), [host('long', LONG), host('z3thon', 'z3thon.k2.dev')])
    fireEvent.click(t)
    expect(useServerSwitcherStore.getState().open).toBe(true)
    const list = screen.getByRole('listbox', { name: 'Servers' })
    const menu = list.parentElement
    if (!menu) throw new Error('menu has no parent')
    expect(menu.className).toContain('absolute')
    expect(menu.className).toContain('w-[260px]')
    expect(menu.className).not.toContain('overflow-hidden')
    const options = screen.getAllByRole('option').map((o) => o.textContent ?? '')
    expect(options.some((o) => o.includes('z3thon.k2.dev'))).toBe(true)
    expect(options.some((o) => o.includes(LONG))).toBe(true)
  })
})

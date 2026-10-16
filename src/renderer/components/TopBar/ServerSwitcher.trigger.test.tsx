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

import type { ConnectionStatus } from '@/stores/connect-host'
import type { RemoteRecoveryState } from '@/lib/remote-recovery'
import ServerSwitcher, {
  SERVER_CHIP_MARGIN_RIGHT_PX,
  SERVER_ICON_PX,
  SERVER_SWITCHER_WIDTH_CLASS,
  serverChipClass,
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
const realStatus = useConnectHostStore.getState().connectionStatus
const realRecovery = useConnectHostStore.getState().recovery

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
    useConnectHostStore.setState({
      pickHost: realPickHost,
      activeHost: 'local',
      hosts: [],
      connectionStatus: realStatus,
      recovery: realRecovery,
    })
  })

  it('the width class is the 140px constant and does not shrink', () => {
    expect(SERVER_SWITCHER_WIDTH_PX).toBe(140)
    expect(SERVER_SWITCHER_WIDTH_CLASS).toBe(`w-[${SERVER_SWITCHER_WIDTH_PX}px] shrink-0`)
  })

  it('the trigger is a chip: theme wash, 1px border all round, field radius, same 24px height', () => {
    const t = renderOn(host('rosson', 'rosson.k2.dev'))
    const cls = t.className.split(/\s+/)
    for (const c of [
      'border',
      'rounded-[var(--radius-field)]',
      'bg-[var(--color-overlay-soft-bg)]',
      'border-[var(--color-overlay-soft-border)]',
      'text-[var(--color-text-secondary)]',
      'hover:bg-[var(--color-bg-elevated)]',
      'hover:border-[var(--color-border-strong)]',
      'h-6',
    ]) {
      expect(cls).toContain(c)
    }
    expect(cls).not.toContain('border-dashed')
    expect(t.getAttribute('aria-expanded')).toBe('false')
    // Tokens only: no hard-coded colour in the chip's classes.
    expect(t.className).not.toMatch(/#[0-9a-f]{3,8}|rgba?\(/i)
    // Air before the cog tab: -8 against the cluster's 14px gap.
    expect(SERVER_CHIP_MARGIN_RIGHT_PX).toBe(-8)
    expect(t.parentElement?.style.marginRight).toBe('-8px')
  })

  it('open lifts the chip a step (stronger fill + border) and drops the rest wash', () => {
    const t = renderOn(host('rosson', 'rosson.k2.dev'))
    fireEvent.click(t)
    const cls = screen.getByTestId('server-switcher-trigger').className.split(/\s+/)
    expect(cls).toContain('bg-[var(--color-bg-elevated)]')
    expect(cls).toContain('border-[var(--color-border-strong)]')
    expect(cls).toContain('text-[var(--color-text-primary)]')
    expect(cls).not.toContain('bg-[var(--color-overlay-soft-bg)]')
    expect(screen.getByTestId('server-switcher-trigger').getAttribute('aria-expanded')).toBe('true')
  })

  it('serverChipClass: rest vs open, never a dashed border', () => {
    const rest = serverChipClass({ open: false }).split(' ')
    const lifted = serverChipClass({ open: true }).split(' ')
    expect(rest).toContain('bg-[var(--color-overlay-soft-bg)]')
    expect(lifted).toContain('bg-[var(--color-bg-elevated)]')
    expect(rest).not.toContain('border-dashed')
    expect(lifted).not.toContain('border-dashed')
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

  it('the server icon sits at the left, before the name, at 16px (A3 server shape)', () => {
    const t = renderOn(host('rosson', 'rosson.k2.dev'))
    const first = t.firstElementChild
    expect(first?.getAttribute('data-testid')).toBe('server-status-icon')
    expect(first?.getAttribute('data-k2-icon')).toBe('server')
    expect(first?.nextElementSibling?.getAttribute('data-testid')).toBe('server-switcher-label')
    expect(SERVER_ICON_PX).toBe(16)
    expect(first?.getAttribute('width')).toBe(String(SERVER_ICON_PX))
    // Squared Lucide `server`: two square-cornered rack frames…
    const frames = Array.from(first?.querySelectorAll('rect:not([data-led])') ?? [])
    expect(frames.map((r) => [r.getAttribute('x'), r.getAttribute('y'), r.getAttribute('width'), r.getAttribute('height')])).toEqual([
      ['2', '2', '20', '8'],
      ['2', '14', '20', '8'],
    ])
    expect(frames.map((r) => r.getAttribute('rx'))).toEqual([null, null])
    expect([first?.getAttribute('stroke-linecap'), first?.getAttribute('stroke-linejoin')]).toEqual(['square', 'miter'])
    // …and two LED bars starting where Lucide draws its dots (the status light).
    expect(first?.querySelectorAll('[data-led]')).toHaveLength(2)
    expect(first?.querySelectorAll('circle')).toHaveLength(0)
  })

  it('the LEDs are the status light: colour per state, the words in the accessible name', () => {
    const cases: { status: ConnectionStatus; recovery: RemoteRecoveryState; color: string; words: string }[] = [
      { status: 'connected', recovery: { kind: 'connected' }, color: '#3fb950', words: 'Connection: connected' },
      { status: 'connecting', recovery: { kind: 'connected' }, color: '#d29922', words: 'Connection: connecting' },
      { status: 'offline', recovery: { kind: 'connected' }, color: '#f85149', words: 'Connection: offline' },
      {
        status: 'connected',
        recovery: { kind: 'reauthenticating' },
        color: '#d29922',
        words: 'Re-authenticating…',
      },
      {
        status: 'connected',
        recovery: { kind: 'signin-required' },
        color: '#f85149',
        words: 'Sign-in required — select this server to sign in',
      },
    ]
    for (const c of cases) {
      act(() => useConnectHostStore.setState({ connectionStatus: c.status, recovery: c.recovery }))
      const t = renderOn(host('rosson', 'rosson.k2.dev'))
      const icon = screen.getByTestId('server-status-icon')
      expect(icon.getAttribute('data-led-color')).toBe(c.color)
      const leds = Array.from(icon.querySelectorAll('[data-led]'))
      expect(leds).toHaveLength(2)
      // Bars 3x the old 3.5 square's width (Rosson 2026-10-16).
      expect(leds.map((l) => [l.getAttribute('width'), l.getAttribute('height')])).toEqual([
        ['10.5', '3.5'],
        ['10.5', '3.5'],
      ])
      expect(leds.map((l) => l.getAttribute('fill'))).toEqual([c.color, c.color])
      expect(t.getAttribute('aria-label')).toBe(`rosson.k2.dev — ${c.words}`)
      expect(t.getAttribute('title')).toBe(`rosson.k2.dev — ${c.words} (⌘L)`)
      cleanup()
    }
  })

  it('no separate status dot: the icon is the only status mark in the trigger', () => {
    const t = renderOn(host('rosson', 'rosson.k2.dev'))
    const spans = Array.from(t.querySelectorAll('span'))
    expect(spans.map((s) => s.getAttribute('data-testid'))).toEqual(['server-switcher-label'])
    expect(t.querySelectorAll('[style*="background"]')).toHaveLength(0)
    expect(t.querySelectorAll('[aria-label^="Connection"]')).toHaveLength(0)
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

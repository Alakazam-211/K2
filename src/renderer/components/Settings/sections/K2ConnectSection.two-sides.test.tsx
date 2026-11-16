// @vitest-environment jsdom
// Settings → Tunnel, two sides like Settings → Projects and Providers / Models
// (the shared halves, controls/halves): the tunnel's status, account and
// addresses (with every site this computer publishes) on the left, and
// Policies then Advanced on the right, each side scrolling on its own, so a
// long list of sites never pushes Policies far down. Under HALVES_MIN_WIDTH
// CSS px of body the sides stack, the left one first, in one scroll.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  getLocalDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  daemonHttpBase: () => 'http://daemon.test',
  invalidateDaemonWs: vi.fn(),
}))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onTunnelIdentityChanged: () => () => undefined,
}))
// The sites list has its own tests; here it stands in as a long list.
vi.mock('./TunnelUrlsPanel', () => ({
  TunnelUrlsPanel: () => (
    <div data-testid="tunnel-sites">
      {Array.from({ length: 30 }, (_, i) => (
        <p key={i}>site-{i}</p>
      ))}
    </div>
  ),
}))

import { K2ConnectSection } from './K2ConnectSection'
import { K2ConnectSettingsShell } from './K2ConnectSettingsShell'
import { HALVES_MIN_WIDTH } from '../controls/halves'
import { useSettingsStore } from '@/stores/settings'

let bodyWidth = 1000
class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
  unobserve(): void {}
}
const realRect = Element.prototype.getBoundingClientRect

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  bodyWidth = 1000
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
    if ((this as HTMLElement).dataset?.testid === 'tunnel-halves') {
      return { width: bodyWidth, height: 600, top: 0, left: 0, right: bodyWidth, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
    }
    return realRect.call(this)
  }
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const path = new URL(url).pathname
      if (path === '/cli/auth/whoami') return json({ owner: true, role: 'owner' })
      if (path === '/cli/tunnel/config') {
        return json({ serverAddr: '', serverPort: 0, subdomain: 'alex', tokenSet: true, publicUrl: null, autoStart: true, e2e: true, e2eEffective: true, enabled: true, released: false })
      }
      if (path === '/cli/tunnel/status') {
        return json({ running: true, public_url: 'https://alex.k2.dev', frpc_installed: true, enabled: true, released: false, subdomain: 'alex' })
      }
      throw new Error(`unexpected fetch ${path}`)
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
})

function headingsIn(el: HTMLElement): string[] {
  return within(el).queryAllByRole('heading', { level: 3 }).map((h) => h.textContent ?? '')
}

describe('Tunnel: two sides', () => {
  it('wide: status, account and addresses on the left; Policies then Advanced on the right; each side scrolls on its own', async () => {
    render(<K2ConnectSection panel="tunnel" />)
    await screen.findByTestId('tunnel-advanced')
    const halves = screen.getByTestId('tunnel-halves')
    expect(halves.getAttribute('data-layout')).toBe('split')
    expect(halves.className).toContain('grid-cols-2')
    const left = screen.getByTestId('tunnel-left')
    const right = screen.getByTestId('tunnel-right')
    // The account's heading carries its Beta badge.
    expect(headingsIn(left)).toEqual(['Status', 'k2.dev accountBeta', 'Addresses'])
    expect(within(left).getByTestId('tunnel-sites')).toBeTruthy()
    expect(headingsIn(right)).toEqual(['Policies', 'Advanced'])
    expect(left.className).toContain('overflow-y-auto')
    expect(right.className).toContain('overflow-y-auto')
    // Reading and keyboard order: the left side first.
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('keeps Policies’ rows and Advanced’s actions, in their order and words', async () => {
    render(<K2ConnectSection panel="tunnel" />)
    const advanced = await screen.findByTestId('tunnel-advanced')
    const policies = screen.getByTestId('tunnel-policies')
    const right = screen.getByTestId('tunnel-right')
    expect(right.contains(policies)).toBe(true)
    expect(right.contains(advanced)).toBe(true)
    expect(policies.compareDocumentPosition(advanced) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    const ids = Array.from(policies.querySelectorAll('[data-settings-id]')).map((e) => e.getAttribute('data-settings-id'))
    expect(ids).toEqual([
      'k2-connect.federation',
      'k2-connect.allow-remote-instruct',
      'k2-connect.public-api',
      'k2-connect.dns-manage-enabled',
      'k2-connect.agents-can-create-connections',
      'k2-connect.login-ingress',
      'k2-connect.boot-status-public',
    ])
    expect(within(advanced).getByRole('button', { name: 'Set up by hand' })).toBeTruthy()
    expect(within(advanced).getByText('Release alex.k2.dev from this computer')).toBeTruthy()
    expect(within(advanced).getByText('Forget the tunnel settings')).toBeTruthy()
  })

  it(`narrow (< ${HALVES_MIN_WIDTH}px): the sides stack, the left first, in one scroll`, async () => {
    bodyWidth = HALVES_MIN_WIDTH - 1
    render(<K2ConnectSection panel="tunnel" />)
    await screen.findByTestId('tunnel-advanced')
    const halves = screen.getByTestId('tunnel-halves')
    expect(halves.getAttribute('data-layout')).toBe('stacked')
    expect(halves.className).toContain('overflow-y-auto')
    expect(halves.className).not.toContain('grid-cols-2')
    const left = screen.getByTestId('tunnel-left')
    const right = screen.getByTestId('tunnel-right')
    expect(left.className).not.toContain('overflow-y-auto')
    expect(right.className).not.toContain('overflow-y-auto')
    expect(headingsIn(right)).toEqual(['Policies', 'Advanced'])
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('in Settings, the page title stays above both sides and nothing around them scrolls', async () => {
    useSettingsStore.setState({ activeSection: 'k2-connect' })
    render(<K2ConnectSettingsShell />)
    await screen.findByTestId('tunnel-advanced')
    const halves = screen.getByTestId('tunnel-halves')
    const title = screen.getByRole('heading', { level: 2, name: 'Tunnel' })
    expect(halves.contains(title)).toBe(false)
    expect(title.compareDocumentPosition(halves) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // No wrapper scrolls the two sides together.
    for (let el = halves.parentElement; el; el = el.parentElement) {
      expect(el.className).not.toContain('overflow-y-auto')
    }
  })
})

// @vitest-environment jsdom
// Connected Servers: from CONNECTED_SERVERS_TWO_COLUMN_MIN_PX of pane width,
// the servers sit on the left and the agents on other servers on the right;
// narrower, they stack, servers first. Crossing the breakpoint restyles the
// columns without remounting the rows, and agent rows carry the same status
// chips as server rows.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => undefined }))
vi.mock('@/lib/host-pool-instance', () => ({ signOutOfHost: vi.fn(async () => undefined) }))
vi.mock('@/lib/host-ops', async (orig) => ({
  ...(await orig<typeof import('@/lib/host-ops')>()),
  hostOpGet: vi.fn(async () => ({ federationEnabled: true })),
  hostBootStatus: vi.fn(async () => ({ phase: 'ready', version: '1.0.0' })),
}))
vi.mock('@/lib/federation', async (orig) => ({
  ...(await orig<typeof import('@/lib/federation')>()),
  isTrustedPeerHost: vi.fn(async () => true),
  listAllRemoteConnections: vi.fn(async () => [
    { remote: true, address: 'ai::rpm.k2.dev', host: 'rpm.k2.dev', agent: 'ai', gone: false, sourceWorkspace: 'Cortana', sourcePath: '/srv/cortana' },
    { remote: true, address: 'old::rpm.k2.dev', host: 'rpm.k2.dev', agent: 'old', gone: true, sourceWorkspace: 'Atlas', sourcePath: '/srv/atlas' },
  ]),
}))

import { ConnectionsSection, CONNECTED_SERVERS_TWO_COLUMN_MIN_PX } from './ConnectionsSection'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

// ── Pane width: jsdom has no layout, so the test sets it ──────────────────
let paneWidth = 1200
const observers = new Set<() => void>()
class FakeResizeObserver {
  private cb: () => void
  constructor(cb: () => void) {
    this.cb = cb
  }
  observe(): void {
    observers.add(this.cb)
  }
  unobserve(): void {}
  disconnect(): void {
    observers.delete(this.cb)
  }
}
function resizePane(width: number): void {
  paneWidth = width
  act(() => {
    for (const cb of observers) cb()
  })
}
const realClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')

const HOST: ConnectHost = {
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

function root(): HTMLElement {
  return document.querySelector<HTMLElement>('[data-connected-servers]')!
}
function column(name: 'servers' | 'agents'): HTMLElement {
  return root().querySelector<HTMLElement>(`[data-servers-column="${name}"]`)!
}

async function show(): Promise<void> {
  useConnectHostStore.setState({ activeHost: 'local', hosts: [HOST], connectionStatus: 'connected' })
  render(<ConnectionsSection />)
  await waitFor(() => expect(document.querySelector('[data-agent-row="ai::rpm.k2.dev"]')).not.toBeNull())
}

beforeEach(() => {
  paneWidth = 1200
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.connectedServers !== undefined ? paneWidth : 0
    },
  })
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
})
afterEach(() => {
  cleanup()
  observers.clear()
  vi.unstubAllGlobals()
  if (realClientWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', realClientWidth)
})

describe('Connected Servers layout', () => {
  it('wide: servers on the left, agents on other servers on the right, each scrolling on its own', async () => {
    await show()
    expect(root().getAttribute('data-servers-layout')).toBe('two-column')
    const [first, second] = Array.from(root().querySelectorAll<HTMLElement>('[data-servers-column]'))
    expect(first.dataset.serversColumn).toBe('servers')
    expect(second.dataset.serversColumn).toBe('agents')
    expect(first.className).toContain('overflow-y-auto')
    expect(second.className).toContain('overflow-y-auto')
    // Each heading stays with its own column, and so do search and anchors.
    expect(column('servers').textContent).toContain('Servers')
    expect(column('servers').querySelector('[data-server-row="z3"]')).not.toBeNull()
    expect(column('servers').querySelector('[data-settings-id="connections.search"]')).not.toBeNull()
    expect(column('servers').querySelector('[data-settings-id="connections.list"]')).not.toBeNull()
    expect(column('agents').textContent).toContain('Agents on other servers')
    expect(column('agents').querySelector('[data-settings-id="connections.federation-overview"]')).not.toBeNull()
    expect(column('servers').querySelector('[data-agent-row]')).toBeNull()
  })

  it(`below ${CONNECTED_SERVERS_TWO_COLUMN_MIN_PX}px: one column, servers first`, async () => {
    paneWidth = CONNECTED_SERVERS_TWO_COLUMN_MIN_PX - 1
    await show()
    expect(root().getAttribute('data-servers-layout')).toBe('stacked')
    expect(column('servers').className).not.toContain('overflow-y-auto')
    expect(column('agents').className).not.toContain('overflow-y-auto')
    expect(column('servers').compareDocumentPosition(column('agents')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('crossing the breakpoint keeps the same rows mounted', async () => {
    paneWidth = CONNECTED_SERVERS_TWO_COLUMN_MIN_PX - 40
    await show()
    const serverRow = document.querySelector('[data-server-row="z3"]')
    const agentRow = document.querySelector('[data-agent-row="ai::rpm.k2.dev"]')
    resizePane(CONNECTED_SERVERS_TWO_COLUMN_MIN_PX)
    expect(root().getAttribute('data-servers-layout')).toBe('two-column')
    expect(document.querySelector('[data-server-row="z3"]')).toBe(serverRow)
    expect(document.querySelector('[data-agent-row="ai::rpm.k2.dev"]')).toBe(agentRow)
    resizePane(600)
    expect(root().getAttribute('data-servers-layout')).toBe('stacked')
    expect(document.querySelector('[data-server-row="z3"]')).toBe(serverRow)
  })

  it('agent rows use the server rows’ chips: Connected, or Gone with the reason', async () => {
    await show()
    const ok = document.querySelector<HTMLElement>('[data-agent-row="ai::rpm.k2.dev"]')!
    const okChip = ok.querySelector<HTMLElement>('[data-server-status="health"]')!
    expect(okChip.textContent).toBe('Connected')
    expect(okChip.dataset.tone).toBe('ok')
    expect(ok.textContent).toContain('rpm.k2.dev')
    expect(ok.textContent).toContain('Workspace Cortana')
    const gone = document.querySelector<HTMLElement>('[data-agent-row="old::rpm.k2.dev"]')!
    const goneChip = gone.querySelector<HTMLElement>('[data-server-status="health"]')!
    expect(goneChip.textContent).toBe('Gone')
    expect(goneChip.dataset.tone).toBe('warn')
    expect(gone.querySelector('[data-gone="true"]')?.textContent).toContain('no longer exists on rpm.k2.dev')
  })
})

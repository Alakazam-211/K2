// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react'

const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => true,
}))

const tunnelState: {
  status: { public_url?: string | null } | null
  subs: { primary: string; targets: Record<string, { target: string; projectId: string | null }> } | null
} = {
  status: { public_url: 'https://rosson.k2.dev' },
  subs: { primary: 'rosson', targets: {} },
}

vi.mock('@/hooks/useTunnelUrls', () => ({
  useTunnelUrls: () => tunnelState,
}))

let publishChanged: ((e: { kind: string; projectId: string }) => void) | null = null

vi.mock('@/stores/session-events', () => ({
  onAppHello: () => () => {},
  onPublishServicesChanged: (cb: (e: { kind: string; projectId: string }) => void) => {
    publishChanged = cb
    return () => {
      publishChanged = null
    }
  },
}))

const hostState: { activeHost: 'local' | { label: string; hostname: string } } = {
  activeHost: 'local',
}

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: typeof hostState) => unknown) => sel(hostState),
}))

import { UrlsPortsSection } from './UrlsPortsSection'
import { PUBLISH_RUN_EXAMPLE } from './urls-ports'

function wire(over: Record<string, unknown> & { name: string }): Record<string, unknown> {
  return {
    cmd: 'npm start',
    cwd: '/proj',
    port: 3000,
    expose: 'tunnel',
    desired: 'running',
    status: 'running',
    pid: 42,
    url: 'https://web.rosson.k2.dev',
    target: 'localhost:3000',
    error: null,
    lastExitCode: null,
    kind: 'cmd',
    skinRoot: '',
    ...over,
  }
}

describe('UrlsPortsSection', () => {
  beforeEach(() => {
    cleanup()
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    publishChanged = null
    hostState.activeHost = 'local'
    tunnelState.status = { public_url: 'https://rosson.k2.dev' }
    tunnelState.subs = { primary: 'rosson', targets: {} }
    if (typeof localStorage !== 'undefined') localStorage.clear()
  })

  afterEach(() => {
    cleanup()
  })

  it('list GET failure is a loud error, not the empty publish hint (P11)', async () => {
    daemonCliGet.mockRejectedValue(new Error('daemon down'))
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('daemon down')).toBeTruthy()
    })
    expect(document.querySelector('[data-published-list-error]')).toBeTruthy()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
    expect(screen.queryByText(/Ask your agent to publish/)).toBeNull()
  })

  it('local-only shows name, listen, Details — no public link', async () => {
    daemonCliGet.mockResolvedValue({
      services: [
        wire({
          name: 'worker',
          expose: 'local',
          url: 'https://should-not-show.k2.dev',
          port: 8090,
        }),
      ],
    })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('worker')).toBeTruthy()
    })
    expect(screen.getByText('localhost:8090')).toBeTruthy()
    expect(screen.getByText('Details')).toBeTruthy()
    expect(screen.getByText('Stop')).toBeTruthy()
    expect(screen.queryByText('https://should-not-show.k2.dev')).toBeNull()
  })

  it('skin rows badge Skin; Details Launch is the gateway, not (skin)', async () => {
    daemonCliGet.mockResolvedValue({
      services: [wire({ name: 'agents', kind: 'skin', cmd: '(skin)', skinRoot: 'ui', pid: 7 })],
    })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('agents')).toBeTruthy()
    })
    const row = document.querySelector('[data-published-service="agents"]')
    expect(row).toBeTruthy()
    expect(within(row as HTMLElement).getByText('Skin')).toBeTruthy()
    expect(within(row as HTMLElement).getByText('PID 7')).toBeTruthy()
    fireEvent.click(within(row as HTMLElement).getByText('Details'))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText('Official skin gateway')).toBeTruthy()
    expect(within(dialog).getByText('ui')).toBeTruthy()
    expect(within(dialog).queryByText('(skin)')).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Start' })).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Stop' })).toBeNull()
  })

  it('BYO leftover Details is on the target row and opens with no fake PID', async () => {
    daemonCliGet.mockResolvedValue({ services: [] })
    tunnelState.subs = {
      primary: 'rosson',
      targets: { staging: { target: 'localhost:4000', projectId: 'proj-1' } },
    }
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('→ localhost:4000')).toBeTruthy()
    })
    const row = document.querySelector('[data-published-byo="staging"]')
    expect(row).toBeTruthy()
    expect(screen.queryByText(/PID/)).toBeNull()
    fireEvent.click(within(row as HTMLElement).getByText('Details'))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText('localhost:4000')).toBeTruthy()
    expect(within(dialog).getByText('Nested URL')).toBeTruthy()
    expect(within(dialog).queryByText(/PID/)).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Start' })).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Stop' })).toBeNull()
  })

  it('modal binds by name to the live list and closes when the row is gone (P14)', async () => {
    daemonCliGet.mockImplementation(async () => ({
      services: [wire({ name: 'web', pid: 99, status: 'running' })],
    }))
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('web')).toBeTruthy()
    })
    fireEvent.click(screen.getByText('Details'))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText('99')).toBeTruthy()

    daemonCliGet.mockImplementation(async () => ({
      services: [wire({ name: 'web', pid: null, status: 'stopped', desired: 'stopped' })],
    }))
    publishChanged?.({ kind: 'publish_services_changed', projectId: 'proj-1' })
    await waitFor(() => {
      expect(within(screen.getByRole('dialog')).getByText('not running')).toBeTruthy()
    })
    expect(within(screen.getByRole('dialog')).queryByText('99')).toBeNull()

    daemonCliGet.mockImplementation(async () => ({ services: [] }))
    publishChanged?.({ kind: 'publish_services_changed', projectId: 'proj-1' })
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeNull()
    })
  })
})

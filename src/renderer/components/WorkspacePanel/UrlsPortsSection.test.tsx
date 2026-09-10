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
  onTunnelSubdomainsChanged: () => () => {},
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

function mockPublishGets(opts: {
  list?: unknown
  leftovers?: unknown
  listError?: Error
  leftoversError?: Error
}): void {
  daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'publish/leftovers') {
      if (opts.leftoversError) throw opts.leftoversError
      return opts.leftovers ?? { leftovers: [] }
    }
    if (opts.listError) throw opts.listError
    return opts.list ?? { services: [] }
  })
}

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

  it('empty hint only when run and leftovers GET are both empty', async () => {
    mockPublishGets({ list: { services: [] }, leftovers: { leftovers: [] } })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText(PUBLISH_RUN_EXAMPLE)).toBeTruthy()
    })
  })

  it('unattributed tunnel host is not imported into this workspace list', async () => {
    mockPublishGets({ leftovers: { leftovers: [] } })
    tunnelState.subs = {
      primary: 'rosson',
      targets: { stray: { target: 'localhost:9', projectId: null } },
    }
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText(PUBLISH_RUN_EXAMPLE)).toBeTruthy()
    })
    expect(document.querySelector('[data-published-byo="stray"]')).toBeNull()
  })

  it('list GET failure is a loud error, not the empty publish hint (P11)', async () => {
    mockPublishGets({ listError: new Error('daemon down') })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('daemon down')).toBeTruthy()
    })
    expect(document.querySelector('[data-published-list-error]')).toBeTruthy()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
    expect(screen.queryByText(/Ask your agent to publish/)).toBeNull()
  })

  it('leftovers GET failure is a loud error, never the run example', async () => {
    mockPublishGets({ leftoversError: new Error('leftovers down') })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('leftovers down')).toBeTruthy()
    })
    expect(document.querySelector('[data-published-leftovers-error]')).toBeTruthy()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
  })

  it('0074 leftover with empty cache paints the label, unknown target, no fake PID', async () => {
    mockPublishGets({
      leftovers: { leftovers: [{ label: 'portal', target: '', url: null }] },
    })
    tunnelState.subs = { primary: '', targets: {} }
    tunnelState.status = { public_url: null }
    render(<UrlsPortsSection projectId="docs" />)
    await waitFor(() => {
      expect(document.querySelector('[data-published-byo="portal"]')).toBeTruthy()
    })
    expect(screen.getByText('portal')).toBeTruthy()
    expect(screen.getByText('→ (unknown)')).toBeTruthy()
    expect(screen.queryByText(/PID/)).toBeNull()
    expect(screen.queryByText('Start')).toBeNull()
    expect(screen.queryByText('Stop')).toBeNull()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
  })

  it('matching run name hides the leftover duplicate', async () => {
    mockPublishGets({
      list: { services: [wire({ name: 'portal' })] },
      leftovers: {
        leftovers: [{ label: 'portal', target: 'localhost:3000', url: 'https://portal.rosson.k2.dev' }],
      },
    })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('portal')).toBeTruthy()
    })
    expect(document.querySelector('[data-published-service="portal"]')).toBeTruthy()
    expect(document.querySelector('[data-published-byo="portal"]')).toBeNull()
  })

  it('leftovers GET 404 falls back to today tunnel ∩ filter', async () => {
    mockPublishGets({ leftoversError: new Error('route not found') })
    tunnelState.subs = {
      primary: 'rosson',
      targets: { staging: { target: 'localhost:4000', projectId: 'proj-1' } },
    }
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('→ localhost:4000')).toBeTruthy()
    })
    expect(document.querySelector('[data-published-byo="staging"]')).toBeTruthy()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
  })

  it('tunnel GET fail does not wipe leftovers GET rows', async () => {
    mockPublishGets({
      leftovers: { leftovers: [{ label: 'portal', target: 'localhost:3000', url: null }] },
    })
    tunnelState.subs = null
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(document.querySelector('[data-published-byo="portal"]')).toBeTruthy()
    })
    expect(screen.getByText('→ localhost:3000')).toBeTruthy()
    expect(screen.queryByText(PUBLISH_RUN_EXAMPLE)).toBeNull()
  })

  it('local-only shows name, listen, Details — no public link', async () => {
    mockPublishGets({
      list: {
        services: [
          wire({
            name: 'worker',
            expose: 'local',
            url: 'https://should-not-show.k2.dev',
            port: 8090,
          }),
        ],
      },
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
    mockPublishGets({
      list: { services: [wire({ name: 'agents', kind: 'skin', cmd: '(skin)', skinRoot: 'ui', pid: 7 })] },
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
    mockPublishGets({
      leftovers: {
        leftovers: [
          { label: 'staging', target: 'localhost:4000', url: 'https://staging.rosson.k2.dev' },
        ],
      },
    })
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
    mockPublishGets({
      list: { services: [wire({ name: 'web', pid: 99, status: 'running' })] },
    })
    render(<UrlsPortsSection projectId="proj-1" />)
    await waitFor(() => {
      expect(screen.getByText('web')).toBeTruthy()
    })
    fireEvent.click(screen.getByText('Details'))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText('99')).toBeTruthy()

    mockPublishGets({
      list: { services: [wire({ name: 'web', pid: null, status: 'stopped', desired: 'stopped' })] },
    })
    publishChanged?.({ kind: 'publish_services_changed', projectId: 'proj-1' })
    await waitFor(() => {
      expect(within(screen.getByRole('dialog')).getByText('not running')).toBeTruthy()
    })
    expect(within(screen.getByRole('dialog')).queryByText('99')).toBeNull()

    mockPublishGets({ list: { services: [] } })
    publishChanged?.({ kind: 'publish_services_changed', projectId: 'proj-1' })
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeNull()
    })
  })
})

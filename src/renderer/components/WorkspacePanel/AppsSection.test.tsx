// @vitest-environment jsdom
// The Workspace drawer's Apps section: the Apps this workspace's agent
// maintains, from GET /cli/app/list?workspace= (the daemon filters).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, fireEvent, cleanup, waitFor, act, within } from '@testing-library/react'

const daemonCliGet = vi.fn()

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...args: unknown[]) => daemonCliGet(...args)),
    daemonCliPost: primaryOnly(async () => ({})),
  }
})

vi.mock('@/lib/server-capabilities', () => ({ useServerSupports: () => true }))

const openSite = vi.fn(async () => undefined)
vi.mock('@/lib/published-sites', async (orig) => ({
  ...(await orig<typeof import('@/lib/published-sites')>()),
  openSite: (...args: unknown[]) => openSite(...(args as [])),
}))

let publishChanged: ((e: { kind: string; projectId: string }) => void) | null = null
let hello: (() => void) | null = null
let deployed: ((e: { kind: string; projectId: string; appId: string }) => void) | null = null

vi.mock('@/stores/session-events', async () => {
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    onAppHello: (scope: unknown, cb: () => void) => {
      expectPrimaryScope(scope)
      hello = cb
      return () => {
        hello = null
      }
    },
    onPublishServicesChanged: (scope: unknown, cb: (e: { kind: string; projectId: string }) => void) => {
      expectPrimaryScope(scope)
      publishChanged = cb
      return () => {
        publishChanged = null
      }
    },
    onAppDeployed: (scope: unknown, cb: (e: { kind: string; projectId: string; appId: string }) => void) => {
      expectPrimaryScope(scope)
      deployed = cb
      return () => {
        deployed = null
      }
    },
  }
})

import { AppsSection } from './AppsSection'
import { APP_DEPLOY_EXAMPLE } from './workspace-apps'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'

const room = testRoom({ tabs: {}, scope: primaryScope() })

function app(over: Record<string, unknown> & { appId: string }): Record<string, unknown> {
  return {
    label: 'Guarded (container)',
    state: 'running',
    stateReason: null,
    url: `https://${over.appId}.rosson.k2.dev`,
    hosts: [`${over.appId}.rosson.k2.dev`],
    workspace: 'p1',
    ...over,
  }
}

function serve(byWorkspace: Record<string, unknown[]>, services: unknown[] = []): void {
  daemonCliGet.mockImplementation(async (route: string, params: Record<string, string>) => {
    if (route === 'app/list') return { ok: true, apps: byWorkspace[params.workspace] ?? [], legacy: [] }
    if (route === 'publish/list') return { services }
    throw new Error(`unexpected GET ${route}`)
  })
}

const rows = (): string[] =>
  Array.from(document.querySelectorAll('[data-app-row]')).map((r) => r.getAttribute('data-app-row') ?? '')

describe('AppsSection', () => {
  beforeEach(() => {
    cleanup()
    daemonCliGet.mockReset()
    openSite.mockClear()
    publishChanged = null
    hello = null
    deployed = null
    localStorage.clear()
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('asks the daemon for this workspace’s Apps and lists exactly those', async () => {
    serve({ p1: [app({ appId: 'shop' }), app({ appId: 'blog' })], p2: [app({ appId: 'other' })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(rows()).toEqual(['shop', 'blog']))
    expect(daemonCliGet).toHaveBeenCalledWith('app/list', { workspace: 'p1' })
    expect(screen.queryByText('other')).toBeNull()
    // The count chip in the header.
    expect(within(screen.getByTestId('apps-section-header')).getByText('2')).toBeTruthy()
  })

  it('prints every state as a word beside its dot, with the reason as the tooltip', async () => {
    const states: Array<[string, string, string]> = [
      ['running', 'Running', 'ok'],
      ['stopped', 'Stopped', 'quiet'],
      ['installing', 'Installing', 'working'],
      ['restarting', 'Restarting', 'working'],
      ['failed', 'Failed', 'danger'],
      ['crashlooping', 'Crashing', 'danger'],
      ['parked', 'Parked', 'quiet'],
      ['waiting_engine', 'Waiting for engine', 'quiet'],
    ]
    serve({
      p1: states.map(([s]) => app({ appId: `a-${s}`, state: s, stateReason: s === 'failed' ? 'health check failed' : null })),
    })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(rows()).toHaveLength(states.length))
    for (const [s, word, tone] of states) {
      const row = document.querySelector(`[data-app-row="a-${s}"]`) as HTMLElement
      expect(within(row).getByText(word)).toBeTruthy()
      expect(row.querySelector(`[data-app-dot="${tone}"]`)).toBeTruthy()
    }
    const failed = document.querySelector('[data-app-row="a-failed"] [data-app-state-word]') as HTMLElement
    expect(failed.getAttribute('title')).toBe('health check failed')
    expect(failed.className).toContain('--color-status-error-text')
    const dot = document.querySelector('[data-app-row="a-installing"] [data-app-dot]') as HTMLElement
    expect(dot.className).toContain('--color-status-working')
    expect(dot.className).toContain('animate-pulse')
  })

  it('Open opens the App’s address the way Published opens a site', async () => {
    serve({ p1: [app({ appId: 'shop' })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    const open = await screen.findByRole('button', { name: 'Open shop at shop.rosson.k2.dev' })
    fireEvent.click(open)
    expect(openSite).toHaveBeenCalledTimes(1)
    expect(openSite).toHaveBeenCalledWith(primaryScope(), 'https://shop.rosson.k2.dev')
  })

  it('+N comes from hosts: the first host, then +N for the rest', async () => {
    serve({ p1: [app({ appId: 'shop', url: 'https://shop.rosson.k2.dev', hosts: ['shop.rosson.k2.dev', 'shop.example.com', 'www.shop.example.com'] })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    expect((await screen.findByTestId('app-open')).textContent).toBe('shop.rosson.k2.dev')
    expect(screen.getByTestId('app-more-hosts').textContent).toBe('+2')
  })

  it('two addresses: the first, then +1 with the full list in the tooltip', async () => {
    serve({ p1: [app({ appId: 'shop', hosts: ['shop.rosson.k2.dev', 'shop.example.com'] })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    const open = await screen.findByTestId('app-open')
    expect(open.textContent).toBe('shop.rosson.k2.dev')
    expect(open.getAttribute('title')).toBe('shop.rosson.k2.dev\nshop.example.com')
    const more = screen.getByTestId('app-more-hosts')
    expect(more.textContent).toBe('+1')
    expect(more.getAttribute('title')).toBe('shop.rosson.k2.dev\nshop.example.com')
  })

  it('no Apps: Published’s empty voice with the deploy command', async () => {
    serve({ p1: [] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(screen.getByTestId('apps-empty')).toBeTruthy())
    expect(screen.getByText(APP_DEPLOY_EXAMPLE)).toBeTruthy()
    expect(within(screen.getByTestId('apps-section-header')).queryByText('0')).toBeNull()
  })

  it('a failed list is an error, never the empty state', async () => {
    daemonCliGet.mockRejectedValue(new Error('daemon down'))
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(screen.getByText('daemon down')).toBeTruthy())
    expect(screen.queryByTestId('apps-empty')).toBeNull()
  })

  it('re-reads on publish_services_changed for this workspace (Published’s event), not another', async () => {
    serve({ p1: [app({ appId: 'shop' })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(rows()).toEqual(['shop']))
    expect(publishChanged).toBeTruthy()
    const before = daemonCliGet.mock.calls.length
    serve({ p1: [app({ appId: 'shop', state: 'failed' })] })
    act(() => publishChanged!({ kind: 'publish_services_changed', projectId: 'p2' }))
    expect(daemonCliGet.mock.calls.length).toBe(before)
    act(() => publishChanged!({ kind: 'publish_services_changed', projectId: 'p1' }))
    await waitFor(() => expect(screen.getByText('Failed')).toBeTruthy())
    // And on every (re)connect, as Published does.
    const n = daemonCliGet.mock.calls.length
    act(() => hello!())
    await waitFor(() => expect(daemonCliGet.mock.calls.length).toBe(n + 1))
  })

  it('re-reads on app_deployed for this workspace, not another', async () => {
    serve({ p1: [app({ appId: 'shop', state: 'installing' })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(screen.getByText('Installing')).toBeTruthy())
    expect(deployed).toBeTruthy()
    const before = daemonCliGet.mock.calls.length
    serve({ p1: [app({ appId: 'shop', state: 'running' })] })
    act(() => deployed!({ kind: 'app_deployed', projectId: 'p2', appId: 'other' }))
    expect(daemonCliGet.mock.calls.length).toBe(before)
    act(() => deployed!({ kind: 'app_deployed', projectId: 'p1', appId: 'shop' }))
    await waitFor(() => expect(screen.getByText('Running')).toBeTruthy())
  })

  it('does not poll: the daemon’s events are the only re-reads', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    serve({ p1: [app({ appId: 'shop' })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    await waitFor(() => expect(rows()).toEqual(['shop']))
    const before = daemonCliGet.mock.calls.length
    await act(async () => {
      await vi.advanceTimersByTimeAsync(120_000)
    })
    expect(daemonCliGet.mock.calls.length).toBe(before)
  })

  it('no hosts: the url’s host shows, with no +N', async () => {
    serve({ p1: [app({ appId: 'shop', hosts: [] })] })
    renderInRoom(room, <AppsSection projectId="p1" />)
    const open = await screen.findByTestId('app-open')
    expect(open.textContent).toBe('shop.rosson.k2.dev')
    expect(screen.queryByTestId('app-more-hosts')).toBeNull()
  })

  it('Details opens Published’s details sheet for the App’s service row', async () => {
    serve(
      { p1: [app({ appId: 'shop', serviceId: 'svc-shop' })] },
      [{ id: 'svc-shop', name: 'shop', cmd: 'npm start', cwd: '/p', port: 3000, expose: 'tunnel', desired: 'running', status: 'running', pid: 7, url: 'https://shop.rosson.k2.dev', target: null, error: null, lastExitCode: null, kind: 'cmd', skinRoot: '' }],
    )
    renderInRoom(room, <AppsSection projectId="p1" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Details for shop' }))
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy())
    expect(daemonCliGet).toHaveBeenCalledWith('publish/list', { project: 'p1' })
  })

  it('the cog opens Settings → Apps', async () => {
    const openSettings = vi.fn()
    const real = useSettingsStore.getState().openSettings
    useSettingsStore.setState({ openSettings })
    try {
      serve({ p1: [] })
      renderInRoom(room, <AppsSection projectId="p1" />)
      fireEvent.click(await screen.findByTitle('Manage Apps'))
      expect(openSettings).toHaveBeenCalledWith('skin-access')
    } finally {
      useSettingsStore.setState({ openSettings: real })
    }
  })
})

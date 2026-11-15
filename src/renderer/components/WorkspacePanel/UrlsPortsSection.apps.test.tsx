// @vitest-environment jsdom
// Published leaves out the services that belong to this workspace's Apps
// (matched by the App's serviceId = the service's id, never the name): they
// live in the Apps section, whose Details still opens their sheet.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react'

const daemonCliGet = vi.fn()

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...args: unknown[]) => daemonCliGet(...args)),
    daemonCliPost: primaryOnly(async () => ({})),
  }
})

vi.mock('@/lib/server-capabilities', () => ({ useServerSupports: () => true }))

vi.mock('@/hooks/useTunnelUrls', () => ({
  useTunnelUrls: () => ({ status: { public_url: null }, subs: { primary: '', targets: {} } }),
}))

vi.mock('@/stores/session-events', () => ({
  onAppHello: () => () => {},
  onTunnelSubdomainsChanged: () => () => {},
  onPublishServicesChanged: () => () => {},
  onAppDeployed: () => () => {},
}))

import { UrlsPortsSection } from './UrlsPortsSection'
import { AppsSection } from './AppsSection'
import { PUBLISH_RUN_EXAMPLE } from './urls-ports'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'

const room = testRoom({ tabs: {}, scope: primaryScope() })

function svc(id: string, name: string, cmd = 'npm start'): Record<string, unknown> {
  return {
    id, projectId: 'p1', name, cmd, cwd: '/p', port: 3000, expose: 'tunnel', desired: 'running', status: 'running',
    pid: null, url: `https://${name}.example.test`, target: 'localhost:3000', error: null, lastExitCode: null, kind: 'cmd', skinRoot: '',
  }
}

function app(appId: string, serviceId: string, state = 'running'): Record<string, unknown> {
  return { appId, serviceId, state, stateReason: null, label: null, url: null, workspace: 'p1' }
}

function serve(services: unknown[], apps: unknown[] | Error): void {
  daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'publish/list') return { services }
    if (route === 'publish/leftovers') return { leftovers: [] }
    if (route === 'app/list') {
      if (apps instanceof Error) throw apps
      return { ok: true, apps, legacy: [] }
    }
    throw new Error(`unexpected GET ${route}`)
  })
}

const published = (): string[] =>
  Array.from(document.querySelectorAll('[data-published-service]')).map((r) => r.getAttribute('data-published-service') ?? '')
const publishedHeader = (): HTMLElement => screen.getByTitle(/ Published$/)

describe('Published leaves out services that belong to an App', () => {
  beforeEach(() => {
    cleanup()
    daemonCliGet.mockReset()
    localStorage.clear()
  })
  afterEach(() => cleanup())

  it('hides the App’s service row (by serviceId) and counts only what it shows', async () => {
    serve([svc('svc-shop', 'shop', '(app)'), svc('svc-docs', 'docs'), svc('svc-api', 'api')], [app('shop', 'svc-shop')])
    renderInRoom(room, <UrlsPortsSection projectId="p1" />)
    await waitFor(() => expect(published()).toEqual(['api', 'docs']))
    expect(within(publishedHeader()).getByText('2')).toBeTruthy()
    expect(daemonCliGet).toHaveBeenCalledWith('app/list', { workspace: 'p1' })
  })

  it('a row that belongs to no App still shows, even when its name is an App’s name', async () => {
    // The App "docs" owns svc-9; the row named docs is svc-docs: not the App's.
    serve([svc('svc-docs', 'docs'), svc('svc-9', 'docs-runtime', '(app)')], [app('docs', 'svc-9', 'failed')])
    renderInRoom(room, <UrlsPortsSection projectId="p1" />)
    await waitFor(() => expect(published()).toEqual(['docs']))
  })

  it('every row an App’s: Published shows its empty state', async () => {
    serve([svc('svc-a', 'a', '(app)'), svc('svc-b', 'b', '(app)')], [app('a', 'svc-a', 'stopped'), app('b', 'svc-b', 'installing')])
    renderInRoom(room, <UrlsPortsSection projectId="p1" />)
    await waitFor(() => expect(screen.getByText(PUBLISH_RUN_EXAMPLE)).toBeTruthy())
    expect(published()).toEqual([])
    expect(within(publishedHeader()).queryByText(/\d/)).toBeNull()
  })

  it('no Apps answer (an older daemon): every row shows', async () => {
    serve([svc('svc-shop', 'shop', '(app)'), svc('svc-docs', 'docs')], new Error('route not found'))
    renderInRoom(room, <UrlsPortsSection projectId="p1" />)
    await waitFor(() => expect(published()).toEqual(['docs', 'shop']))
  })

  it('the App’s Details still opens its service’s sheet while Published hides the row', async () => {
    serve([svc('svc-shop', 'shop-runtime', '(app)'), svc('svc-docs', 'docs')], [app('shop', 'svc-shop', 'crashlooping')])
    renderInRoom(
      room,
      <>
        <AppsSection projectId="p1" />
        <UrlsPortsSection projectId="p1" />
      </>,
    )
    await waitFor(() => expect(published()).toEqual(['docs']))
    fireEvent.click(await screen.findByRole('button', { name: 'Details for shop' }))
    const dialog = await screen.findByRole('dialog')
    // The sheet is the App's service row (matched by id, not by name).
    expect(within(dialog).getAllByText('shop-runtime').length).toBeGreaterThan(0)
  })
})

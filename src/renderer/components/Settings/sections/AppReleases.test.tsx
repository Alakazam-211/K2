// @vitest-environment jsdom

// Settings → Apps → <App> → Releases (0.46.7): the panel draws the daemon's
// channels, releases, files (links and sizes), downloads and active users;
// promote, roll back and pull post the owner routes; a refusal shows the
// daemon's words; a non-runtime App has no panel.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
    daemonCliPostQuery: vi.fn(),
  }
})

import { AppReleases } from './AppReleases'
import { parseDownloads, parseUpdates, size } from './app-updates-api'

const LIST = {
  ok: true,
  app: 'jobs',
  dlId: 'dev.k2.box.jobs',
  key: { keyId: '5716343E2F010C58' },
  channels: [
    { channel: 'beta', version: '1.5.0', release: 'rel-b' },
    { channel: 'stable', version: '1.4.0', release: 'rel-a' },
  ],
  releases: [
    {
      id: 'rel-b',
      version: '1.5.0',
      state: 'live',
      error: null,
      channels: ['beta'],
      size: 3 * 1024 * 1024,
      activeUsers: 4,
      files: [{ platform: 'darwin-aarch64', file: 'Jobs.app.tar.gz', size: 3 * 1024 * 1024, url: 'https://dl.k2.dev/dev.k2.box.jobs/1.5.0/ab/Jobs.app.tar.gz', upload: 'uploaded' }],
    },
    {
      id: 'rel-a',
      version: '1.4.0',
      state: 'live',
      error: null,
      channels: ['stable'],
      size: 2048,
      activeUsers: 9,
      files: [{ platform: 'darwin-aarch64', file: 'Jobs.app.tar.gz', size: 2048, url: 'https://dl.k2.dev/dev.k2.box.jobs/1.4.0/cd/Jobs.app.tar.gz', upload: 'uploaded' }],
    },
  ],
  activeUsers: { today: 7, last7Days: 13, counting: 'installs' },
  endpoints: { dynamic: 'https://jobs.box.k2.dev/_k2/updates/<channel>/{{target}}/{{arch}}/{{current_version}}' },
}
const STATS = { ok: true, downloads: { available: true, total: 21, byVersion: [{ version: '1.5.0', downloads: 5 }, { version: '1.4.0', downloads: 16 }] } }

function routes(listAnswer: unknown = LIST, stats: unknown = STATS): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'app/updates') {
      if (listAnswer instanceof Error) throw listAnswer
      return listAnswer
    }
    if (route === 'app/updates/stats') return stats
    throw new Error(`unexpected GET ${route}`)
  })
}

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})
afterEach(() => cleanup())

describe('app-updates-api', () => {
  it('parses the list and the downloads, and says sizes in words', () => {
    const u = parseUpdates(LIST)
    expect(u.channels).toEqual([
      { channel: 'beta', version: '1.5.0' },
      { channel: 'stable', version: '1.4.0' },
    ])
    expect(u.releases[0].files[0].url).toContain('dl.k2.dev')
    expect(u.activeWeek).toBe(13)
    expect(u.keyId).toBe('5716343E2F010C58')
    expect(parseDownloads(STATS)).toEqual({ available: true, total: 21, byVersion: { '1.5.0': 5, '1.4.0': 16 } })
    expect(parseDownloads({ downloads: { available: false, why: 'host down' } })).toEqual({ available: false, why: 'host down' })
    expect(size(3 * 1024 * 1024)).toBe('3.0 MiB')
    expect(size(2048)).toBe('2 KiB')
  })
})

describe('AppReleases', () => {
  it('shows channels, files with links and sizes, downloads and users', async () => {
    routes()
    render(<AppReleases app="svc-1" />)
    await waitFor(() => expect(screen.getByTestId('releases-channels').textContent).toBe('beta 1.5.0 · stable 1.4.0'))
    const row = screen.getByTestId('release-1.5.0')
    await waitFor(() => expect(row.textContent).toContain('5'))
    expect(row.textContent).toContain('3.0 MiB')
    const links = screen.getAllByRole('link')
    expect(links[0].getAttribute('href')).toBe('https://dl.k2.dev/dev.k2.box.jobs/1.5.0/ab/Jobs.app.tar.gz')
    expect(screen.getByText(/Downloads \(30 days\): 21/)).toBeTruthy()
    expect(h.daemonCliGet).toHaveBeenCalledWith('app/updates', { app: 'svc-1' })
  })

  it('promotes, rolls back (showing the warning) and pulls after a second click', async () => {
    routes()
    h.daemonCliPost.mockImplementation(async (route: string) =>
      route === 'app/updates/rollback' ? { ok: true, warning: 'people already on the bad version stay on it' } : { ok: true },
    )
    render(<AppReleases app="svc-1" />)
    await waitFor(() => screen.getByText('Promote to stable'))
    fireEvent.click(screen.getByText('Promote to stable'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('app/updates/promote', { app: 'svc-1', from: 'beta', to: 'stable', version: '1.5.0' }),
    )
    // Forward only for promote, back only for rollback: 1.4.0 offers no
    // promote to beta, 1.5.0 no rollback of stable.
    expect(screen.queryByText('Promote to beta')).toBeNull()
    expect(screen.queryByText('Roll stable back to this')).toBeNull()
    await waitFor(() => screen.getByText('Roll beta back to this'))
    fireEvent.click(screen.getByText('Roll beta back to this'))
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('stay on it'))
    expect(h.daemonCliPost).toHaveBeenCalledWith('app/updates/rollback', { app: 'svc-1', channel: 'beta', to: '1.4.0' })
    const pulls = screen.getAllByText('Pull')
    fireEvent.click(pulls[0])
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('app/updates/pull', expect.anything())
    fireEvent.click(screen.getByText('Pull 1.5.0: sure?'))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('app/updates/pull', { app: 'svc-1', version: '1.5.0' }))
  })

  it('shows a refusal in the daemon’s words', async () => {
    routes()
    h.daemonCliPost.mockRejectedValue(new Error('stable is at 1.4.0; promote moves a channel forward only'))
    render(<AppReleases app="svc-1" />)
    await waitFor(() => screen.getByText('Promote to stable'))
    fireEvent.click(screen.getByText('Promote to stable'))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('forward only'))
  })

  it('says downloads are unavailable, never zero, when the host is down', async () => {
    routes(LIST, { ok: true, downloads: { available: false, why: 'the download host is unavailable' } })
    render(<AppReleases app="svc-1" />)
    await waitFor(() => expect(screen.getByText(/Downloads: unavailable/)).toBeTruthy())
  })

  it('has no panel for a service that is not a runtime App', async () => {
    const e = Object.assign(new Error('no App'), { code: 'app_not_found' })
    routes(e)
    const { container } = render(<AppReleases app="svc-legacy" />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalled())
    await waitFor(() => expect(container.textContent).toBe(''))
  })

  it('points at the CLI when there are no releases yet', async () => {
    routes({ ...LIST, key: null, channels: [], releases: [] })
    render(<AppReleases app="svc-1" />)
    await waitFor(() => expect(screen.getByText(/k2 app updates push jobs/)).toBeTruthy())
    expect(screen.getByText(/updater public key/)).toBeTruthy()
  })
})

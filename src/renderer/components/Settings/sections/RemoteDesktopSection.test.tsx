// @vitest-environment jsdom
// Settings → Access → Remote Desktop (prd-remote-desktop-tab-v1 §5, §8)
// against a fake daemon, fail loud:
//  - an Owner sees the switch, the hide-list, grants, viewers, the log; the
//    switch asks with the S14 sentence before it enables; grant, revoke and
//    kick send the routes' bodies;
//  - an Admin or a Member sees the state, read only: no switch, no grants,
//    and nothing is posted;
//  - the nav item is in Access and hidden without `remote-desktop-v1`; an
//    older server's page says to update K2.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  posts: [] as Array<{ route: string; body: unknown }>,
  gets: [] as string[],
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'themes/list') return []
      daemon.gets.push(route)
      if (route === 'desktop/status') return daemon.status
      if (route === 'desktop/audit') return { lines: [{ ts: '2026-11-10T01:00:00Z', event: 'enable', who: 'owner' }] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      daemon.posts.push({ route, body })
      return {}
    }),
  }
})

import { RemoteDesktopSection } from './RemoteDesktopSection'
import { settingsNav } from '../Settings'
import { DESKTOP_ENABLE_CARD, type DesktopStatus } from '@/components/Desktop/desktop-state'
import { fakeScope } from '@/test-utils/fake-scope'
import { __resetServerScopesForTests, noteServerVersion } from '@/kessel/server-scope'
import type { ServerScope } from '@/kessel/server-scope'

function status(over: Omit<Partial<DesktopStatus>, 'you'> & { you?: Partial<DesktopStatus['you']> } = {}): DesktopStatus {
  const { you, ...rest } = over
  return {
    desktop: 'console', machine: 'office-mini', enabled: false, backend: 'macos', state: 'unavailable',
    message: "K2 Screen isn't in this build of K2 yet.", lane: 'main',
    you: { role: 'owner', mayView: true, mayControl: true, mayManage: true, sameMachine: false, ...you },
    ...rest,
  }
}

const OWNER_EXTRA = {
  hideApps: ['com.apple.Passwords', 'com.1password.1password'],
  grants: [{ user: 'mel', userId: 'u-mel', mode: 'view' as const, grantedBy: 'owner', grantedAt: 1 }],
  viewers: [{ id: 'v1', user: 'ana', role: 'admin', mode: 'control' as const, ingress: 'tunnel', ip: '1.2.3.4', device: 'MacBook', display: '1', since: 1 }],
}

async function show(scope: ServerScope = fakeScope('box.k2.dev')): Promise<void> {
  await act(async () => {
    render(<RemoteDesktopSection scope={scope} />)
  })
  await act(async () => {})
}

beforeEach(() => {
  daemon.posts = []
  daemon.gets = []
})
afterEach(() => {
  cleanup()
  __resetServerScopesForTests()
})

describe('RemoteDesktopSection: Owner', () => {
  it('shows the switch, hidden apps, grants, viewers and the log', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    await show()
    expect(screen.getByTestId('remote-desktop-switch')).toBeTruthy()
    expect((screen.getByTestId('remote-desktop-hide-apps') as HTMLTextAreaElement).value).toBe('com.apple.Passwords\ncom.1password.1password')
    expect(screen.getAllByTestId('remote-desktop-grant').map((li) => li.textContent)).toEqual([expect.stringContaining('mel')])
    expect(screen.getAllByTestId('remote-desktop-viewer').map((li) => li.textContent)).toEqual([expect.stringContaining('ana')])
    expect(screen.getByTestId('remote-desktop-audit').textContent).toContain('enable')
    expect(screen.queryByTestId('remote-desktop-readonly')).toBeNull()
  })

  it('the switch shows the S14 sentence first; Turn on enables', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    await show()
    fireEvent.click(screen.getByTestId('remote-desktop-switch'))
    expect(screen.getByTestId('remote-desktop-enable-card').textContent).toContain(DESKTOP_ENABLE_CARD)
    expect(daemon.posts).toEqual([])
    await act(async () => {
      fireEvent.click(screen.getByTestId('remote-desktop-enable-confirm'))
    })
    expect(daemon.posts).toEqual([{ route: 'desktop/enable', body: {} }])
  })

  it('grant, revoke, kick and hidden apps send the routes’ bodies', async () => {
    daemon.status = status({ enabled: true, state: 'ready', ...OWNER_EXTRA })
    await show()
    fireEvent.change(screen.getByTestId('remote-desktop-grant-user'), { target: { value: ' sam ' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('remote-desktop-grant-add'))
    })
    await act(async () => {
      fireEvent.click(screen.getByText('Revoke'))
    })
    await act(async () => {
      fireEvent.click(screen.getByText('Kick'))
    })
    fireEvent.change(screen.getByTestId('remote-desktop-hide-apps'), { target: { value: 'com.apple.Passwords\n\n com.bitwarden.desktop ' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('remote-desktop-hide-save'))
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId('remote-desktop-switch'))
    })
    expect(daemon.posts).toEqual([
      { route: 'desktop/grant', body: { user: 'sam', mode: 'view' } },
      { route: 'desktop/revoke', body: { user: 'mel' } },
      { route: 'desktop/kick', body: { viewer: 'v1' } },
      { route: 'desktop/policy', body: { hideApps: ['com.apple.Passwords', 'com.bitwarden.desktop'] } },
      { route: 'desktop/disable', body: {} },
    ])
  })
})

describe('RemoteDesktopSection: Admin and Member', () => {
  it.each(['admin', 'member'])('%s sees the state read only', async (role) => {
    daemon.status = status({ enabled: true, state: 'ready', message: 'Ready', you: { role, mayManage: false, mayControl: role === 'admin', mayView: role === 'admin' } })
    await show()
    expect(screen.getByTestId('remote-desktop-state').textContent).toContain('On')
    expect(screen.getByTestId('remote-desktop-readonly')).toBeTruthy()
    expect(screen.queryByTestId('remote-desktop-switch')).toBeNull()
    expect(screen.queryByTestId('remote-desktop-grants')).toBeNull()
    expect(screen.queryByTestId('remote-desktop-hide-apps')).toBeNull()
    expect(daemon.gets).not.toContain('desktop/audit')
    expect(daemon.posts).toEqual([])
  })
})

describe('Remote Desktop availability', () => {
  it('an older server is never asked and says to update K2', async () => {
    await show({ ...fakeScope('old.k2.dev'), serverSupports: () => false })
    expect(screen.getByTestId('remote-desktop-unsupported').textContent).toContain('Update K2 on this computer')
    expect(daemon.gets).toEqual([])
  })

  it('the nav item is the last of Access, hidden without remote-desktop-v1', () => {
    const access = (): { id?: string; hide?: boolean } | undefined => {
      const g = settingsNav({ zenGardens: true }).find((b) => b.kind === 'group' && b.title === 'Access')
      if (!g || g.kind !== 'group') throw new Error('no Access group')
      expect(g.items[g.items.length - 1].id).toBe('remote-desktop')
      return g.items.find((i) => i.id === 'remote-desktop')
    }
    noteServerVersion('local', '0.46.11', ['tickets-list-all'])
    expect(access()).toEqual({ id: 'remote-desktop', label: 'Remote Desktop', beta: true, hide: true })
    noteServerVersion('local', '0.46.11', ['remote-desktop-v1'])
    expect(access()?.hide).toBe(false)
  })
})

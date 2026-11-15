// @vitest-environment jsdom
// Settings → Access → Remote Desktop (prd-remote-desktop-tab-v1 §5, §8)
// against a fake daemon, fail loud:
//  - an Owner sees the switch, the hide-list, grants, viewers, the log in
//    words; the switch asks with the S14 sentence before it enables; grant,
//    remove and disconnect send the routes' bodies, and anything that cuts
//    someone off asks first;
//  - an odd reply (no `you`) is a plain error card with Try again, never a
//    crash; a refused write says what failed and the server's reason;
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
  postError: null as Error | null,
  /** What a POST answers (default `{}`). */
  postReply: {} as unknown,
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
      if (daemon.postError) throw daemon.postError
      return daemon.postReply
    }),
  }
})

import ConfirmDialog from '@/components/ConfirmDialog/ConfirmDialog'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { RemoteDesktopSection, auditSentence } from './RemoteDesktopSection'
import { settingsNav } from '../Settings'
import { DESKTOP_ENABLE_CARD, VIRTUAL_ENABLE_CARD, type DesktopPermission, type DesktopStatus } from '@/components/Desktop/desktop-state'
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
    render(
      <>
        <RemoteDesktopSection scope={scope} />
        <ConfirmDialog />
      </>,
    )
  })
  await act(async () => {})
}

async function click(el: HTMLElement): Promise<void> {
  await act(async () => {
    fireEvent.click(el)
  })
  await act(async () => {})
}

/** Press the open confirm dialog's button (its name is the dialog's own). */
async function answer(label: string): Promise<void> {
  await click(screen.getByRole('button', { name: label }))
}

beforeEach(() => {
  daemon.posts = []
  daemon.gets = []
  daemon.postError = null
  daemon.postReply = {}
})
afterEach(() => {
  cleanup()
  __resetServerScopesForTests()
})

describe('RemoteDesktopSection: Owner', () => {
  it('shows the switch, hidden apps, grants, viewers and the log in words', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    await show()
    expect(screen.getByRole('heading', { name: /Remote Desktop/ })).toBeTruthy()
    expect(screen.getByTestId('remote-desktop-switch')).toBeTruthy()
    expect((screen.getByTestId('remote-desktop-hide-apps') as HTMLTextAreaElement).value).toBe('com.apple.Passwords\ncom.1password.1password')
    expect(screen.getAllByTestId('remote-desktop-grant').map((li) => li.textContent)).toEqual([expect.stringContaining('mel')])
    expect(screen.getAllByTestId('remote-desktop-viewer').map((li) => li.textContent)).toEqual([expect.stringContaining('ana')])
    expect(screen.getByTestId('remote-desktop-viewer').textContent).toContain('Using the screen')
    expect(screen.getByTestId('remote-desktop-audit').textContent).toContain('The owner turned Remote Desktop on')
    expect(screen.queryByTestId('remote-desktop-readonly')).toBeNull()
  })

  it('the switch asks with the S14 sentence first; Turn on enables and says so', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    await show()
    await click(screen.getByTestId('remote-desktop-switch'))
    expect(document.body.textContent).toContain(DESKTOP_ENABLE_CARD)
    expect(daemon.posts).toEqual([])
    await answer('Cancel')
    expect(daemon.posts).toEqual([])
    await click(screen.getByTestId('remote-desktop-switch'))
    await answer('Turn on')
    expect(daemon.posts).toEqual([{ route: 'desktop/enable', body: {} }])
    expect(screen.getByTestId('remote-desktop-switch-note').textContent).toContain('Remote Desktop is on')
  })

  it('on a Linux server with no screen the switch asks with the `virtual` card (LD-18)', async () => {
    daemon.status = status({ ...OWNER_EXTRA, desktop: 'virtual', backend: 'linux', state: 'not_set_up', message: '' })
    await show()
    await click(screen.getByTestId('remote-desktop-switch'))
    expect(document.body.textContent).toContain(VIRTUAL_ENABLE_CARD)
    expect(document.body.textContent).not.toContain(DESKTOP_ENABLE_CARD)
    await answer('Turn on')
    expect(daemon.posts).toEqual([{ route: 'desktop/enable', body: { desktop: 'virtual' } }])
  })

  it('grant, remove, disconnect, hidden apps and off send the routes’ bodies, asking before cutting anyone off', async () => {
    daemon.status = status({ enabled: true, state: 'ready', ...OWNER_EXTRA })
    await show()
    fireEvent.change(screen.getByTestId('remote-desktop-grant-user'), { target: { value: ' sam ' } })
    await click(screen.getByTestId('remote-desktop-grant-add'))
    expect(screen.getByTestId('remote-desktop-grant-note').textContent).toContain('sam can now watch this screen.')

    await click(screen.getByRole('button', { name: 'Stop letting mel watch' }))
    expect(document.body.textContent).toContain('Stop letting mel watch?')
    expect(daemon.posts).toHaveLength(1)
    await answer('Stop sharing')

    await click(screen.getByRole('button', { name: 'Disconnect ana' }))
    expect(document.body.textContent).toContain('Disconnect ana?')
    await answer('Disconnect')

    fireEvent.change(screen.getByTestId('remote-desktop-hide-apps'), { target: { value: 'com.apple.Passwords\n\n com.bitwarden.desktop ' } })
    await click(screen.getByTestId('remote-desktop-hide-save'))

    // Someone is watching: turning it off asks first.
    await click(screen.getByTestId('remote-desktop-switch'))
    expect(document.body.textContent).toContain('ana has this screen open')
    await answer('Turn off')
    expect(daemon.posts).toEqual([
      { route: 'desktop/grant', body: { user: 'sam', mode: 'view' } },
      { route: 'desktop/revoke', body: { user: 'mel' } },
      { route: 'desktop/kick', body: { viewer: 'v1' } },
      { route: 'desktop/policy', body: { hideApps: ['com.apple.Passwords', 'com.bitwarden.desktop'] } },
      { route: 'desktop/disable', body: {} },
    ])
  })

  it('turning off with nobody watching asks nothing', async () => {
    daemon.status = status({ enabled: true, state: 'ready', ...OWNER_EXTRA, viewers: [] })
    await show()
    await click(screen.getByTestId('remote-desktop-switch'))
    expect(screen.queryByText('Turn off Remote Desktop?')).toBeNull()
    expect(daemon.posts).toEqual([{ route: 'desktop/disable', body: {} }])
  })

  it('a refused grant says what failed and the server’s reason, and keeps the name', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    await show()
    daemon.postError = new DaemonHttpError('no such user: sam', 404, 'no_such_user')
    fireEvent.change(screen.getByTestId('remote-desktop-grant-user'), { target: { value: 'sam' } })
    await click(screen.getByTestId('remote-desktop-grant-add'))
    const note = screen.getByTestId('remote-desktop-grant-note')
    expect(note.getAttribute('role')).toBe('alert')
    expect(note.textContent).toBe('Couldn’t let sam watch.The server said: no such user: sam.')
    expect((screen.getByTestId('remote-desktop-grant-user') as HTMLInputElement).value).toBe('sam')
  })
})

describe('RemoteDesktopSection: an odd reply', () => {
  it('a status with no `you` is a plain error card with Try again, never a crash', async () => {
    daemon.status = {}
    await show()
    const card = screen.getByTestId('remote-desktop-error')
    expect(card.getAttribute('role')).toBe('alert')
    expect(card.textContent).toContain('Couldn’t load Remote Desktop')
    expect(card.textContent).toContain('the reply didn’t say whether Remote Desktop is on')
    daemon.status = status({ ...OWNER_EXTRA })
    await click(screen.getByRole('button', { name: 'Try again' }))
    expect(screen.queryByTestId('remote-desktop-error')).toBeNull()
    expect(screen.getByTestId('remote-desktop-switch')).toBeTruthy()
  })

  it('a refused read shows the server’s reason quietly', async () => {
    daemon.status = { enabled: true }
    await show()
    expect(screen.getByTestId('remote-desktop-error').textContent).toContain('the reply didn’t say what you may do here')
  })

  it('the log reads in words', () => {
    expect(auditSentence({ ts: 't', event: 'connect', who: 'jordan', mode: 'control' })).toBe('jordan connected to use the screen')
    expect(auditSentence({ ts: 't', event: 'grant', who: 'owner', target: 'sam' })).toBe('The owner let sam watch')
    expect(auditSentence({ ts: 't', event: 'enable', who: 'alex', outcome: 'unchanged' })).toBe('alex turned Remote Desktop on (no change)')
  })
})

describe('RemoteDesktopSection: Admin and Member', () => {
  it.each(['admin', 'member'])('%s sees the state read only', async (role) => {
    daemon.status = status({ enabled: true, state: 'ready', message: 'Ready', you: { role, mayManage: false, mayControl: role === 'admin', mayView: role === 'admin' } })
    await show()
    expect(screen.getByTestId('remote-desktop-machine').textContent).toContain('Ready')
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

// One action (Rosson 2026-11-15): the switch has K2 Screen ask macOS; the
// row shows the two permissions as steps, each with its System Settings
// button, and updates itself.
function permission(over: Partial<DesktopPermission> = {}): DesktopPermission {
  return {
    step: 'waiting_screen',
    screen: 'waiting',
    accessibility: 'waiting',
    opened: 'screen',
    checking: true,
    line: 'Allow K2 Screen in the window that just opened.',
    ...over,
  }
}

function word(pane: 'screen' | 'accessibility'): string | undefined {
  return screen.getByTestId(`remote-desktop-permission-${pane}`).querySelector<HTMLElement>('[data-permission-word]')?.dataset.permissionWord
}

describe('RemoteDesktopSection: allowing K2 Screen on a Mac', () => {
  it('shows each permission as a step with the daemon’s line, and a button to its pane', async () => {
    daemon.status = status({ enabled: true, state: 'needs_permission', message: 'Someone at this Mac has to allow K2 Screen once.', macPermission: permission(), ...OWNER_EXTRA })
    await show()
    expect(screen.getByTestId('remote-desktop-permission-line').textContent).toBe(
      'Allow K2 Screen in the window that just opened. K2 notices by itself when you do.',
    )
    expect([word('screen'), word('accessibility')]).toEqual(['waiting', 'waiting'])
    expect(screen.getByTestId('remote-desktop-permission-screen').textContent).toContain('Screen Recording')
    expect(screen.getByTestId('remote-desktop-permission-accessibility').textContent).toContain('type and click')
    expect(screen.queryByTestId('remote-desktop-permission-check')).toBeNull()
    await click(screen.getByTestId('remote-desktop-permission-open-screen'))
    expect(daemon.posts).toEqual([{ route: 'desktop/ask', body: { open: 'screen' } }])
    expect(screen.getByTestId('remote-desktop-permission-note').textContent).toContain('Opened System Settings on office-mini')
    const text = screen.getByTestId('remote-desktop-section').textContent ?? ''
    for (const jargon of ['TCC', 'deskd', 'tcc']) expect([jargon, text.includes(jargon)]).toEqual([jargon, false])
  })

  it('Screen Recording allowed: only Accessibility waits, with its button', async () => {
    daemon.status = status({
      enabled: true,
      state: 'needs_permission',
      macPermission: permission({ step: 'waiting_accessibility', screen: 'allowed', opened: 'accessibility' }),
      ...OWNER_EXTRA,
    })
    await show()
    expect([word('screen'), word('accessibility')]).toEqual(['allowed', 'waiting'])
    expect(screen.queryByTestId('remote-desktop-permission-open-screen')).toBeNull()
    await click(screen.getByTestId('remote-desktop-permission-open-accessibility'))
    expect(daemon.posts).toEqual([{ route: 'desktop/ask', body: { open: 'accessibility' } }])
  })

  it('once K2 stopped checking by itself, Check again asks it to look again', async () => {
    const line = 'Still waiting. Allow K2 Screen in System Settings, then press Check again.'
    daemon.status = status({ enabled: true, state: 'needs_permission', macPermission: permission({ checking: false, opened: null, line }), ...OWNER_EXTRA })
    await show()
    expect(screen.getByTestId('remote-desktop-permission-line').textContent).toBe(line)
    await click(screen.getByTestId('remote-desktop-permission-check'))
    expect(daemon.posts).toEqual([{ route: 'desktop/ask', body: { check: true } }])
  })

  it('ready, off or Linux: no steps', async () => {
    daemon.status = status({ enabled: true, state: 'ready', macPermission: permission({ step: 'ready', screen: 'allowed', accessibility: 'allowed' }), ...OWNER_EXTRA })
    await show()
    expect(screen.queryByTestId('remote-desktop-permission')).toBeNull()
    cleanup()
    daemon.status = status({ enabled: false, macPermission: permission(), ...OWNER_EXTRA })
    await show()
    expect(screen.queryByTestId('remote-desktop-permission')).toBeNull()
    cleanup()
    daemon.status = status({ enabled: true, backend: 'linux', state: 'ready', ...OWNER_EXTRA })
    await show()
    expect(screen.queryByTestId('remote-desktop-permission')).toBeNull()
  })

  it('someone who can’t manage it sees the steps without buttons', async () => {
    daemon.status = status({ enabled: true, state: 'needs_permission', macPermission: permission({ checking: false }), you: { role: 'admin', mayManage: false } })
    await show()
    expect([word('screen'), word('accessibility')]).toEqual(['waiting', 'waiting'])
    expect(screen.queryByTestId('remote-desktop-permission-open-screen')).toBeNull()
    expect(screen.queryByTestId('remote-desktop-permission-check')).toBeNull()
  })

  it('Turn on on a Mac says what happens next: allow K2 Screen in macOS’s prompts', async () => {
    daemon.status = status({ ...OWNER_EXTRA })
    daemon.postReply = { enabled: true, state: 'needs_permission', macPermission: permission({ step: 'asking', opened: null, line: 'Starting K2 Screen to ask macOS…' }) }
    await show()
    await click(screen.getByTestId('remote-desktop-switch'))
    await answer('Turn on')
    expect(daemon.posts).toEqual([{ route: 'desktop/enable', body: {} }])
    expect(screen.getByTestId('remote-desktop-switch-note').textContent).toContain(
      'Remote Desktop is on. Allow K2 Screen in the macOS prompts on this Mac; this page updates by itself.',
    )
  })
})

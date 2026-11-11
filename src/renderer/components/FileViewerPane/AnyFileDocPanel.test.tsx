// @vitest-environment jsdom
//
// The any-file panel over a scripted daemon: it renders tier 0's answers and
// sends the gestures on the room's own scope. Lock take/release, a lost lock,
// restore's confirm, empty states, a remote room, a view-only room.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const d = vi.hoisted(() => ({
  lease: null as Json | null,
  rights: { read: true, edit: true, history: true },
  conflictCopies: [] as Json[] | null,
  roster: [] as Json[],
  versions: [] as Json[],
  approvals: [] as Json[],
  mineApprovals: [] as Json[],
  posts: [] as Array<{ scope: unknown; route: string; body: Json }>,
  gets: [] as Array<{ scope: unknown; route: string; params: Json }>,
  failGet: null as unknown,
  failPost: null as unknown,
  holdStatus: null as Promise<void> | null,
  restoreResult: { changed: true, written: true },
  decision: {} as Json,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: async (scope: unknown, route: string, params: Json) => {
    d.gets.push({ scope, route, params })
    if (d.failGet) throw d.failGet
    switch (route) {
      case 'docs/status': {
        const lease = d.lease
        if (d.holdStatus) await d.holdStatus
        return { ok: true, tier: 0, version: 7, lease, rights: d.rights, conflictCopies: d.conflictCopies }
      }
      case 'docs/who':
        return { ok: true, roster: d.roster }
      case 'docs/versions':
        return { ok: true, versions: d.versions }
      case 'docs/approvals':
        return { ok: true, approvals: params.mine ? d.mineApprovals : d.approvals }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  },
  daemonCliPost: async (scope: unknown, route: string, body: Json) => {
    d.posts.push({ scope, route, body })
    if (d.failPost) throw d.failPost
    if (route === 'docs/lease') {
      if (body.action === 'acquire') d.lease = myLease()
      if (body.action === 'release') d.lease = null
      return { ok: true }
    }
    if (route === 'docs/restore') return { ok: true, result: d.restoreResult }
    if (route === 'docs/approve') return { ok: true, ...d.decision }
    throw new Error(`unexpected POST ${route}`)
  },
}))

import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { AnyFileDocPanel, POLL_MS, RENEW_MS, leftText, pidName } from './AnyFileDocPanel'

const FILE = '/ws/art/poster.psd'

function myLease(): Json {
  return { pid: 'person:owner', name: 'Rosson', label: 'Rosson (person)', expiresMs: Date.now() + 300_000, auto: false, mine: true }
}
function otherLease(name = 'Ada'): Json {
  return { pid: 'person:ada', name, label: `${name} (person)`, expiresMs: Date.now() + 240_000, auto: false, mine: false }
}
function version(id: number, extra: Json = {}): Json {
  return { id, atMs: Date.now() - 60_000, name: null, authorLabel: 'Ada (person)', size: 2048, restoredFrom: null, redacted: false, current: false, ...extra }
}
function refusal(status: number, code: string, msg: string): DaemonHttpError {
  return new DaemonHttpError(msg, status, code)
}

const localScope = { id: 'primary', connectionKey: 'local', isRemote: false } as unknown as ServerScope
const remoteScope = { id: 'host:far.k2.dev', connectionKey: 'far', isRemote: true } as unknown as ServerScope
const viewOnly = { id: 'host:far.k2.dev', connectionKey: 'far', isRemote: true, viewOnly: true } as unknown as ServerScope

const confirm = vi.fn(async () => true)
const onCoreOff = vi.fn()

function mount(scope: ServerScope = localScope): void {
  render(<AnyFileDocPanel scope={scope} filePath={FILE} onCoreOff={onCoreOff} />)
}
async function loaded(): Promise<void> {
  await waitFor(() => expect(screen.getByText('Who’s here')).toBeTruthy())
}
async function poll(ms = POLL_MS): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}
const holder = (): string | null => document.querySelector('[data-testid="any-file-lock"]')?.getAttribute('data-holder') ?? null

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true })
  d.lease = null
  d.rights = { read: true, edit: true, history: true }
  d.conflictCopies = []
  d.roster = [{ pid: 'person:owner', name: 'Rosson', colour: '#88aaff', kind: 'person', region: null, away: false }]
  d.versions = [version(6), version(7, { current: true })]
  d.approvals = []
  d.mineApprovals = []
  d.posts = []
  d.gets = []
  d.failGet = null
  d.failPost = null
  d.holdStatus = null
  d.restoreResult = { changed: true, written: true }
  d.decision = {}
  confirm.mockClear()
  confirm.mockResolvedValue(true)
  onCoreOff.mockClear()
  useConfirmDialogStore.setState({ confirm })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  Object.defineProperty(document, 'hidden', { configurable: true, value: false })
})

describe('what it shows', () => {
  it('renders who’s here, the holder, versions newest first, copies and approvals', async () => {
    d.roster = [
      { pid: 'person:owner', name: 'Rosson', colour: '#88aaff', kind: 'person', region: null, away: false },
      { pid: 'agent:w1', name: 'Ledger', colour: '', kind: 'agent', region: 'layer 3', away: true },
    ]
    d.lease = otherLease()
    d.conflictCopies = [{ path: '.k2/conflicts/poster.psd', version: 5, kept: 4, atMs: Date.now() - 1000 }]
    d.approvals = [{ change: 6, approval: { state: 'pending', author: 'agent:w1', onBehalfOf: 'person:ada' } }]
    mount()
    await loaded()
    const who = screen.getByTestId('any-file-who').textContent ?? ''
    expect(who).toContain('Rosson')
    expect(who).toContain('Ledger')
    expect(who).toContain('agent')
    expect(who).toContain('layer 3')
    expect(who).toContain('away')
    expect(holder()).toBe('other')
    expect(screen.getByTestId('any-file-lock').textContent).toContain('Ada holds the lock, about 4m left')
    const rows = [...screen.getByTestId('any-file-versions').querySelectorAll('li')].map((l) => l.getAttribute('data-testid'))
    expect(rows).toEqual(['any-file-version-7', 'any-file-version-6'])
    expect(screen.getByTestId('any-file-version-7').textContent).toContain('current')
    expect(screen.getByTestId('any-file-copies').textContent).toContain('.k2/conflicts/poster.psd')
    expect(screen.getByTestId('any-file-copies').textContent).toContain('Holds version 4')
    expect(screen.getByTestId('any-file-approval-6').textContent).toContain('Version 6 by Ada (person), waiting for ada')
  })

  it('says so when each section is empty', async () => {
    d.roster = []
    d.versions = []
    mount()
    await loaded()
    expect(screen.getByText('Nobody’s here right now.')).toBeTruthy()
    expect(screen.getByText('Nobody holds the lock.')).toBeTruthy()
    expect(screen.getByText('No versions yet.')).toBeTruthy()
    expect(screen.getByText('No conflict copies.')).toBeTruthy()
    expect(screen.getByText('Nothing waiting for approval.')).toBeTruthy()
  })

  it('without the history right it asks for no versions and says why', async () => {
    d.rights = { read: true, edit: true, history: false }
    d.conflictCopies = null
    mount()
    await loaded()
    expect(screen.getByText('You can’t see this file’s versions.')).toBeTruthy()
    expect(screen.getByText('You can’t see this file’s conflict copies.')).toBeTruthy()
    expect(d.gets.map((g) => g.route)).not.toContain('docs/versions')
  })

  it('a reader without edit is offered no lock and no restore', async () => {
    d.rights = { read: true, edit: false, history: true }
    mount()
    await loaded()
    expect(screen.queryByRole('button', { name: 'Take the lock' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Restore' })).toBeNull()
  })
})

describe('the lock', () => {
  it('takes and releases it through the daemon', async () => {
    mount()
    await loaded()
    expect(holder()).toBe('none')
    fireEvent.click(screen.getByRole('button', { name: 'Take the lock' }))
    await waitFor(() => expect(holder()).toBe('me'))
    expect(d.posts[0]).toMatchObject({ route: 'docs/lease', body: { path: FILE, action: 'acquire' } })
    expect(screen.getByTestId('any-file-lock').textContent).toContain('You hold the lock')

    fireEvent.click(screen.getByRole('button', { name: 'Release' }))
    await waitFor(() => expect(holder()).toBe('none'))
    expect(d.posts[1]).toMatchObject({ route: 'docs/lease', body: { path: FILE, action: 'release' } })
    await poll()
    expect(screen.queryByTestId('any-file-lost')).toBeNull()
  })

  it('shows the daemon’s refusal when someone else got there first', async () => {
    mount()
    await loaded()
    d.failPost = refusal(409, 'leased', 'leased by Ada (person) for another 4m 10s: nothing was written.')
    fireEvent.click(screen.getByRole('button', { name: 'Take the lock' }))
    await waitFor(() => expect(screen.getByTestId('any-file-error').textContent).toContain('leased by Ada'))
    expect(holder()).toBe('none')
    expect(screen.queryByTestId('any-file-lost')).toBeNull()
  })

  it('says who has it when the lock is lost to someone', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    expect(holder()).toBe('me')
    d.lease = otherLease('Grace')
    await poll()
    expect(screen.getByTestId('any-file-lost').textContent).toContain('You lost the lock. Grace holds it now.')
    expect(holder()).toBe('other')
  })

  it('says so when the lock runs out, and can take it again', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    d.lease = null
    await poll()
    expect(screen.getByTestId('any-file-lost').textContent).toContain('it expired or someone released it')
    fireEvent.click(screen.getByRole('button', { name: 'Take the lock' }))
    await waitFor(() => expect(holder()).toBe('me'))
    expect(screen.queryByTestId('any-file-lost')).toBeNull()
  })

  it('a read from before a release is not mistaken for a lost lock', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    // A read that saw "mine" is still out when the release lands.
    let finish: () => void = () => {}
    d.holdStatus = new Promise<void>((res) => (finish = res))
    await poll()
    d.holdStatus = null
    fireEvent.click(screen.getByRole('button', { name: 'Release' }))
    await waitFor(() => expect(d.posts.some((p) => p.body.action === 'release')).toBe(true))
    await poll(10)
    await act(async () => {
      finish()
      await vi.advanceTimersByTimeAsync(10)
    })
    await waitFor(() => expect(holder()).toBe('none'))
    await poll()
    expect(screen.queryByTestId('any-file-lost')).toBeNull()
  })

  it('renews while held and stops once released', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    await poll(RENEW_MS)
    const renews = (): number => d.posts.filter((p) => p.body.action === 'renew').length
    expect(renews()).toBe(1)
    fireEvent.click(screen.getByRole('button', { name: 'Release' }))
    await waitFor(() => expect(holder()).toBe('none'))
    await poll(RENEW_MS * 2)
    expect(renews()).toBe(1)
  })

  it('a renew answered “no lease” shows the lost lock without waiting for a poll', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    d.failPost = refusal(409, 'no_lease', 'nobody holds a lease on it')
    d.lease = null
    await poll(RENEW_MS)
    expect(screen.getByTestId('any-file-lost')).toBeTruthy()
  })

  it('does not renew a lease the kit took around a save, or show it as lost when it ends', async () => {
    d.lease = { ...myLease(), auto: true }
    mount()
    await loaded()
    await poll(RENEW_MS)
    expect(d.posts.filter((p) => p.body.action === 'renew')).toHaveLength(0)
    d.lease = null
    await poll()
    expect(screen.queryByTestId('any-file-lost')).toBeNull()
  })
})

describe('restore', () => {
  it('asks first, and does nothing on Cancel', async () => {
    confirm.mockResolvedValue(false)
    mount()
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    const arg = (confirm.mock.calls as unknown as Array<[Json]>)[0][0]
    expect(arg.title).toBe('Restore version 6?')
    expect(arg.message).toContain('Nothing is deleted')
    expect(arg.destructive).toBeUndefined()
    await poll(10)
    expect(d.posts).toEqual([])
  })

  it('restores the chosen version after the confirm', async () => {
    mount()
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    await waitFor(() => expect(d.posts).toHaveLength(1))
    expect(d.posts[0]).toMatchObject({ route: 'docs/restore', body: { path: FILE, version: 6 } })
    await waitFor(() => expect(screen.getByTestId('any-file-notice').textContent).toContain('Version 6 is now the newest version'))
  })

  it('says when the version already matches', async () => {
    d.restoreResult = { changed: false, written: false }
    mount()
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    await waitFor(() => expect(screen.getByTestId('any-file-notice').textContent).toContain('already matches'))
  })

  it('offers no restore on the current or a purged version, and waits on someone else’s lock', async () => {
    d.versions = [version(5, { redacted: true }), version(6), version(7, { current: true })]
    d.lease = otherLease()
    mount()
    await loaded()
    expect(screen.getByTestId('any-file-version-7').querySelector('button')).toBeNull()
    expect(screen.getByTestId('any-file-version-5').querySelector('button')).toBeNull()
    const b = screen.getByTestId('any-file-version-6').querySelector('button') as HTMLButtonElement
    expect(b.disabled).toBe(true)
    expect(b.title).toBe('Ada holds the lock')
  })
})

describe('approvals', () => {
  const row = { change: 6, approval: { state: 'pending', author: 'agent:w1', onBehalfOf: 'person:owner' } }

  it('the person it was made for approves it', async () => {
    d.approvals = [row]
    d.mineApprovals = [row]
    mount()
    await loaded()
    expect(screen.getByTestId('any-file-approval-6').textContent).toContain('for you')
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    await waitFor(() => expect(d.posts).toHaveLength(1))
    expect(d.posts[0]).toMatchObject({ route: 'docs/approve', body: { path: FILE, change: 6, state: 'approved' } })
  })

  it('a decline says how to undo it and what else that replaces', async () => {
    d.approvals = [row]
    d.mineApprovals = [row]
    d.decision = { restoreTo: 5, later: [7] }
    mount()
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: 'Decline' }))
    await waitFor(() => expect(screen.getByTestId('any-file-notice').textContent).toContain('To undo it, restore version 5. That also replaces version 7.'))
    expect(d.posts[0]).toMatchObject({ body: { state: 'declined' } })
  })

  it('someone else’s approval has no buttons', async () => {
    d.approvals = [row]
    mount()
    await loaded()
    expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull()
  })
})

describe('rooms', () => {
  it('a remote room uses its own scope for every read and gesture', async () => {
    mount(remoteScope)
    await loaded()
    fireEvent.click(screen.getByRole('button', { name: 'Take the lock' }))
    await waitFor(() => expect(holder()).toBe('me'))
    expect(d.gets.length).toBeGreaterThan(0)
    expect(d.gets.every((g) => g.scope === remoteScope)).toBe(true)
    expect(d.posts.every((p) => p.scope === remoteScope)).toBe(true)
  })

  it('a view-only room reads but offers no gesture and never renews', async () => {
    d.lease = myLease()
    d.mineApprovals = [{ change: 6, approval: { state: 'pending', author: 'agent:w1', onBehalfOf: 'person:owner' } }]
    d.approvals = d.mineApprovals
    mount(viewOnly)
    await loaded()
    for (const name of ['Release', 'Restore', 'Approve', 'Decline']) expect(screen.queryByRole('button', { name })).toBeNull()
    expect(screen.getByTestId('any-file-view-only')).toBeTruthy()
    await poll(RENEW_MS)
    expect(d.posts).toEqual([])
  })
})

describe('refusals and polling', () => {
  it('core_off hands back to the tab', async () => {
    mount()
    await loaded()
    d.failGet = refusal(403, 'core_off', 'off')
    await poll()
    expect(onCoreOff).toHaveBeenCalled()
  })

  it('shows a refusal in place of the panel, and recovers', async () => {
    d.failGet = refusal(404, 'workspace_not_found', 'that file is not inside a workspace on this server')
    mount()
    await waitFor(() => expect(screen.getByTestId('any-file-refused').textContent).toContain('not inside a workspace'))
    d.failGet = null
    await poll()
    expect(screen.queryByTestId('any-file-refused')).toBeNull()
    expect(screen.getByText('Who’s here')).toBeTruthy()
  })

  it('a dropped connection keeps what’s on screen and says it’s retrying', async () => {
    mount()
    await loaded()
    d.failGet = new TypeError('Load failed')
    await poll()
    expect(screen.getByTestId('any-file-trouble').textContent).toContain('Retrying')
    expect(screen.getByTestId('any-file-versions')).toBeTruthy()
  })

  it('stops asking while the window is hidden, but a held lock still renews', async () => {
    d.lease = myLease()
    mount()
    await loaded()
    Object.defineProperty(document, 'hidden', { configurable: true, value: true })
    const before = d.gets.length
    await poll(RENEW_MS)
    expect(d.gets.length).toBe(before)
    expect(d.posts.filter((p) => p.body.action === 'renew')).toHaveLength(1)
  })

  it('keeps asking when only the panel is closed (the person is still in the file)', async () => {
    // jsdom has no Tailwind: the class needs its rule for `display: none`.
    render(
      <div>
        <style>{'.hidden { display: none; }'}</style>
        <AnyFileDocPanel scope={localScope} filePath={FILE} hidden onCoreOff={onCoreOff} />
      </div>,
    )
    await waitFor(() => expect(d.gets.length).toBeGreaterThan(0))
    await poll(10)
    const before = d.gets.length
    await poll()
    expect(d.gets.length).toBeGreaterThan(before)
    expect((screen.getByTestId('any-file-panel') as HTMLElement).className).toBe('hidden')
  })

  it('stops asking while its tab is hidden', async () => {
    render(
      <div style={{ display: 'none' }}>
        <AnyFileDocPanel scope={localScope} filePath={FILE} onCoreOff={onCoreOff} />
      </div>,
    )
    await waitFor(() => expect(d.gets.length).toBeGreaterThan(0))
    await poll(10)
    const before = d.gets.length
    await poll(POLL_MS * 3)
    expect(d.gets.length).toBe(before)
  })
})

describe('helpers', () => {
  it('leftText rounds to minutes and never goes negative', () => {
    expect(leftText(240_000, 0)).toBe('about 4m left')
    expect(leftText(30_000, 0)).toBe('under a minute left')
    expect(leftText(0, 60_000)).toBe('under a minute left')
  })

  it('pidName prefers the roster', () => {
    const people = [{ pid: 'agent:w1', name: 'Ledger', colour: '', kind: 'agent', region: null, away: false }]
    expect(pidName('agent:w1', people)).toBe('Ledger')
    expect(pidName('person:ada', people)).toBe('ada')
  })
})

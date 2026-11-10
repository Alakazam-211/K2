// @vitest-environment jsdom
//
// Settings → User Access: the list loads part by part. One read failing or
// slow never blanks the rest; each part says so in place and retries alone;
// a late, older answer never overwrites a newer one; a read skipped while
// the server reconnects runs again by itself.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { act, render, renderHook, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import { PeopleSection } from './PeopleSection'
import { useAccessData } from './user-access-load'
import { useConnectHostStore } from '@/stores/connect-host'

const USERS = [
  { id: 'p-ada', username: 'ada', fullName: 'Ada Lovelace' },
  { id: 'p-bob', username: 'bob' },
]
const GRANTS = [
  { id: 'g1', subjectKind: 'principal', subjectId: 'p-ada', kind: 'app', targetId: 'svc-shop', roleId: 'role-staff', enabled: true },
  { id: 'g2', subjectKind: 'principal', subjectId: 'p-bob', kind: 'app', targetId: 'svc-desk', roleId: 'role-staff', enabled: true },
]
const ROLES = [{ id: 'role-staff', name: 'Staff' }]
const SERVICES: Record<string, unknown[]> = { 'ws-1': [{ id: 'svc-shop', name: 'Shop', kind: 'skin' }], 'ws-2': [{ id: 'svc-desk', name: 'Help Desk', kind: 'cmd' }] }

type Reply = unknown | Error | 'hang'
/** Each route's answer; an Error rejects, 'hang' never answers. */
function serve(over: Record<string, Reply | ((q?: Record<string, string>) => Reply)> = {}): void {
  h.daemonCliGet.mockImplementation(async (route: string, q?: Record<string, string>) => {
    const custom = over[route]
    const r: Reply =
      custom !== undefined
        ? typeof custom === 'function'
          ? (custom as (q?: Record<string, string>) => Reply)(q)
          : custom
        : route === 'skin/users'
          ? { users: USERS }
          : route === 'skin/grants'
            ? { grants: GRANTS }
            : route === 'skin/roles'
              ? { roles: ROLES }
              : route === 'projects/list'
                ? [{ id: 'ws-1' }, { id: 'ws-2' }]
                : route === 'publish/list'
                  ? { services: SERVICES[q?.project ?? ''] ?? [] }
                  : (() => {
                      throw new Error(`unexpected GET ${route}`)
                    })()
    if (r === 'hang') return new Promise(() => {})
    if (r instanceof Error) throw r
    return r
  })
}
const calls = (route: string): number => h.daemonCliGet.mock.calls.filter((c) => c[0] === route).length

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})

describe('User Access loads part by part', () => {
  it('one failing read (what each person can use) leaves the people listed; Try again re-reads only it', async () => {
    let grantsFail = true
    serve({ 'skin/grants': () => (grantsFail ? new Error('database is locked') : { grants: GRANTS }) })
    render(<PeopleSection />)
    expect(await screen.findByRole('button', { name: 'Open ada' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Open bob' })).toBeTruthy()
    await waitFor(() => expect(screen.getByTestId('summary-ada').textContent).toBe('Didn’t load'))
    const part = screen.getByTestId('people-part-error')
    expect(part.textContent).toContain('Couldn’t load which Apps each person can use')
    expect(part.textContent).toContain('The server said: database is locked.')
    const users = calls('skin/users')
    const roles = calls('skin/roles')
    grantsFail = false
    fireEvent.click(within(part).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getByTestId('summary-ada').textContent).toBe('Shop · Staff'))
    expect(screen.getByTestId('summary-bob').textContent).toBe('Help Desk · Staff')
    expect(screen.queryByTestId('people-part-error')).toBeNull()
    expect(calls('skin/users')).toBe(users)
    expect(calls('skin/roles')).toBe(roles)
  })

  it('failing roles still list everyone and their Apps, without the role', async () => {
    serve({ 'skin/roles': new Error('database is locked') })
    render(<PeopleSection />)
    await waitFor(() => expect(screen.getByTestId('summary-ada').textContent).toBe('Shop'))
    expect(screen.getByTestId('people-part-error').textContent).toContain('Couldn’t load the roles')
    expect(screen.queryByTestId('people-error')).toBeNull()
  })

  it('a slow read shows the people at once, with that part still loading in each row', async () => {
    serve({ 'skin/grants': 'hang' })
    render(<PeopleSection />)
    expect(await screen.findByRole('button', { name: 'Open ada' })).toBeTruthy()
    expect(screen.getByTestId('summary-ada').textContent).toBe('Loading…')
    expect(screen.queryByTestId('people-loading')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Open ada' }))
    expect(screen.getByTestId('person-grants-loading').textContent).toContain('Loading what Ada can use')
  })

  it('a workspace that fails to list its Apps hides only its own App names', async () => {
    serve({ 'publish/list': (q?: Record<string, string>) => (q?.project === 'ws-2' ? new Error('no registered workspace matches "ws-2"') : { services: SERVICES['ws-1'] }) })
    render(<PeopleSection />)
    await waitFor(() => expect(screen.getByTestId('summary-ada').textContent).toBe('Shop · Staff'))
    expect(screen.getByTestId('summary-bob').textContent).toBe('App name didn’t load · Staff')
    const missed = screen.getByTestId('people-apps-missed')
    expect(missed.textContent).toContain('One workspace didn’t answer')
    serve()
    fireEvent.click(within(missed).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getByTestId('summary-bob').textContent).toBe('Help Desk · Staff'))
    expect(screen.queryByTestId('people-apps-missed')).toBeNull()
  })

  it('only the people read failing fails the page, with Try again', async () => {
    serve({ 'skin/users': new Error('database is locked') })
    render(<PeopleSection />)
    const err = await screen.findByTestId('people-error')
    expect(err.textContent).toContain('The server said: database is locked.')
    serve()
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    expect(await screen.findByRole('button', { name: 'Open ada' })).toBeTruthy()
  })
})

describe('useAccessData', () => {
  it('drops an older answer that lands after a newer one', async () => {
    const pending: Array<(v: unknown) => void> = []
    h.daemonCliGet.mockImplementation((route: string) => {
      if (route !== 'skin/users') return Promise.resolve(route === 'skin/grants' ? { grants: [] } : { roles: [] })
      return new Promise((resolve) => pending.push(resolve))
    })
    const { result } = renderHook(() => useAccessData(false))
    await waitFor(() => expect(pending.length).toBeGreaterThan(0))
    // A save's re-read starts while the first read is still out.
    act(() => void result.current.reload('users'))
    await waitFor(() => expect(pending.length).toBe(2))
    // The newer one answers first, then the older one, late and stale.
    await act(async () => pending[1]({ users: [{ username: 'new' }] }))
    await act(async () => pending[0]({ users: [{ username: 'old' }] }))
    expect(result.current.users.data?.map((p) => p.username)).toEqual(['new'])
  })

  it('a read skipped while the server reconnects waits, then runs again once it’s back', async () => {
    useConnectHostStore.setState({ recovery: { kind: 'reconnecting' } as never })
    let skipped = true
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'skin/users' && skipped) {
        const e = new Error('host "Studio" is reconnecting — request skipped until recovery completes')
        e.name = 'RecoveringError'
        throw e
      }
      return route === 'skin/users' ? { users: [{ username: 'ada' }] } : route === 'skin/grants' ? { grants: [] } : { roles: [] }
    })
    const { result } = renderHook(() => useAccessData(false))
    await waitFor(() => expect(result.current.users.waiting).toBe(true))
    expect(result.current.users.status).toBe('loading')
    skipped = false
    act(() => useConnectHostStore.setState({ recovery: { kind: 'connected' } }))
    await waitFor(() => expect(result.current.users.data?.map((p) => p.username)).toEqual(['ada']))
    expect(result.current.users.waiting).toBe(false)
  })
})

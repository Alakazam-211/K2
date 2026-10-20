// @vitest-environment jsdom
//
// Sew access on the Access pages (S32 K28 prep): User Access's per-person
// Sew row over chat grants, and Admin Access's read-only Sew state.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

type Json = Record<string, unknown>

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import { PeopleSection } from './PeopleSection'
import { SewAdminAccessState } from './SewAccessParts'
import { resetSewStatusCacheForTests } from './sew-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'

const ADA = { id: 'p-ada', username: 'ada', fullName: 'Ada Lovelace' }
const RUNNING = { supported: true, state: 'running', target: { enabled: true } }

let state: { sew: Json | Error; grants: Json[]; blocked: Json[] }

function wire(): void {
  h.daemonCliGet.mockImplementation(async (route: string) => {
    switch (route) {
      case 'skin/users':
        return { users: [ADA] }
      case 'skin/grants':
        return { grants: state.grants }
      case 'skin/roles':
        return { roles: [] }
      case 'sew/status':
        if (state.sew instanceof Error) throw state.sew
        return state.sew
      case 'skin/grant-hooks':
        return { blocked: state.blocked, held: [] }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
}

function chat(over: Json = {}): Json {
  return { id: 'g-ada', subjectKind: 'principal', subjectId: 'p-ada', kind: 'chat', targetId: 'sew', scope: 'member', enabled: true, hookState: 'synced', ...over }
}

const confirmSpy = vi.fn(async () => true)

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  resetSewStatusCacheForTests()
  state = { sew: RUNNING, grants: [], blocked: [] }
  wire()
  confirmSpy.mockReset()
  confirmSpy.mockResolvedValue(true)
  useConfirmDialogStore.setState({ confirm: confirmSpy })
  useToastStore.setState({ toasts: [] } as never)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

async function openAda(): Promise<void> {
  render(<PeopleSection />)
  fireEvent.click(await screen.findByRole('button', { name: 'Open ada' }))
}

function pick(option: string): void {
  fireEvent.click(screen.getByRole('button', { name: 'Sew access for ada' }))
  fireEvent.click(screen.getByRole('button', { name: option }))
}

function posts(route: string): Json[] {
  return h.daemonCliPost.mock.calls.filter((c) => c[0] === route).map((c) => c[1] as Json)
}

describe('User Access: Sew column', () => {
  it('hidden when the daemon does not support Sew; chat grants never show in the generic list', async () => {
    state.sew = { supported: false, state: 'not-installed' }
    state.grants = [chat()]
    await openAda()
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('sew/status', undefined))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(screen.queryByTestId('sew-person-access')).toBeNull()
    expect(screen.queryByRole('button', { name: /^Show chat/ })).toBeNull()
    expect(screen.getByText('No grants for this person.')).toBeTruthy()
  })

  it('hidden when the status read fails (the page works as before)', async () => {
    state.sew = new DaemonHttpError('forbidden', 403, 'forbidden')
    await openAda()
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(screen.queryByTestId('sew-person-access')).toBeNull()
    expect(screen.getByLabelText('ada full name')).toBeTruthy()
  })

  it('not installed: no dropdown, a link to Chat Hosting', async () => {
    state.sew = { supported: true, state: 'not-installed' }
    await openAda()
    const link = await screen.findByRole('button', { name: 'Enable Sew in Cores → Chat Hosting to give people access.' })
    expect(screen.queryByRole('button', { name: 'Sew access for ada' })).toBeNull()
    fireEvent.click(link)
    expect(useSettingsStore.getState().activeSection).toBe('sew')
  })

  it('No access → Sew member posts a chat grant with the principal id, then refetches', async () => {
    h.daemonCliPost.mockResolvedValue({ id: 'g-new' })
    await openAda()
    await screen.findByRole('button', { name: 'Sew access for ada' })
    const grantReads = h.daemonCliGet.mock.calls.filter((c) => c[0] === 'skin/grants').length
    pick('Sew member')
    await waitFor(() => expect(posts('skin/grants')).toEqual([{ subjectKind: 'principal', subjectId: 'p-ada', kind: 'chat', scope: 'member' }]))
    await waitFor(() => expect(h.daemonCliGet.mock.calls.filter((c) => c[0] === 'skin/grants').length).toBe(grantReads + 1))
    expect(h.daemonCliGet.mock.calls.every((c) => c[0] !== 'projects/list')).toBe(true)
  })

  it('member → guest confirms and updates; a refusal reverts and shows the hint', async () => {
    state.grants = [chat()]
    h.daemonCliPost.mockRejectedValueOnce(new DaemonHttpError("remove the assistant grants for 'ada' first", 400, null))
    await openAda()
    expect(await screen.findByTestId('sew-tag-ada')).toBeTruthy()
    await screen.findByRole('button', { name: 'Sew access for ada' })
    pick('Sew guest')
    expect(await screen.findByText("remove the assistant grants for 'ada' first")).toBeTruthy()
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ message: 'Guests see only channels they are added to.' }))
    expect(posts('skin/grants/update')).toEqual([{ id: 'g-ada', scope: 'guest' }])
    expect(screen.getByRole('button', { name: 'Sew access for ada' }).textContent).toContain('Sew member')
  })

  it('remove confirms (destructive), deletes by id, toasts cascaded assistants', async () => {
    state.grants = [chat()]
    h.daemonCliPost.mockResolvedValue({ success: true, cascaded: [{ id: 'a1' }, { id: 'a2' }] })
    await openAda()
    await screen.findByRole('button', { name: 'Sew access for ada' })
    pick('No Sew access')
    await waitFor(() => expect(posts('skin/grants/delete')).toEqual([{ id: 'g-ada' }]))
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ destructive: true, message: "Removes ada's Sew account. Their messages stay." }))
    await waitFor(() => expect(JSON.stringify(useToastStore.getState())).toContain('Also removed Sew access for 2 assistant agents.'))
  })

  it('a blocked sync shows Failed; Retry uses the blocked row id', async () => {
    state.grants = [chat({ hookState: 'blocked', hookError: 'sew: 422 username_taken' })]
    state.blocked = [{ id: 'row-other', subjectKey: 'k2:principal:p-bob' }, { id: 'row-9', subjectKey: 'k2:principal:p-ada' }]
    h.daemonCliPost.mockResolvedValue({ id: 'row-9', state: 'pending' })
    await openAda()
    const pill = await screen.findByTestId('sew-hook-pill')
    expect(pill.textContent).toBe('Failed')
    expect(pill.getAttribute('title')).toBe('sew: 422 username_taken')
    fireEvent.click(screen.getByRole('button', { name: 'Retry Sew sync' }))
    await waitFor(() => expect(posts('skin/grant-hooks/retry')).toEqual([{ id: 'row-9' }]))
  })

  it('a pending sync repaints every 5 s and stops once synced', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: false })
    state.grants = [chat({ hookState: 'pending' })]
    render(<PeopleSection />)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Open ada' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    expect(screen.getByTestId('sew-hook-pill').textContent).toBe('Pending')
    const reads = (): number => h.daemonCliGet.mock.calls.filter((c) => c[0] === 'skin/grants').length
    const r0 = reads()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(reads()).toBe(r0 + 1)
    state.grants = [chat({ hookState: 'synced' })]
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(reads()).toBe(r0 + 2)
    expect(screen.getByTestId('sew-hook-pill').textContent).toBe('Active')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20_000)
    })
    expect(reads()).toBe(r0 + 2)
  })

  it('Sew off (target disabled): read only, removing still allowed', async () => {
    state.sew = { supported: true, state: 'disabled', target: { enabled: false } }
    state.grants = [chat()]
    h.daemonCliPost.mockResolvedValue({ success: true, cascaded: [] })
    await openAda()
    expect(await screen.findByText('Sew member (Sew is off; read only)')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Sew access for ada' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(posts('skin/grants/delete')).toEqual([{ id: 'g-ada' }]))
  })
})

describe('Admin Access: read-only Sew state', () => {
  it('only Connect logins with their own chat grant, by id, with no controls', async () => {
    state.grants = [
      chat(),
      { id: 'g-c1', subjectKind: 'connect', subjectId: 'uuid-gina', kind: 'chat', targetId: 'sew', scope: 'guest', enabled: true, hookState: 'pending' },
      { id: 'g-c2', subjectKind: 'connect', subjectId: 'uuid-gone', kind: 'chat', targetId: 'sew', scope: 'member', enabled: false },
      { id: 'g-app', subjectKind: 'connect', subjectId: 'uuid-z3', kind: 'app', targetId: 'host', scope: null, enabled: true },
    ]
    render(<SewAdminAccessState users={[{ id: 'uuid-gina', username: 'gina' }, { id: 'uuid-z3', username: 'z3thon' }]} />)
    const box = await screen.findByTestId('sew-admin-access')
    expect(screen.getByTestId('sew-admin-row-gina').textContent).toContain('Sew guest')
    expect(screen.getByTestId('sew-admin-row-uuid-gone').textContent).toContain('Sew member (off)')
    expect(screen.queryByTestId('sew-admin-row-z3thon')).toBeNull()
    expect(screen.queryByTestId('sew-admin-row-ada')).toBeNull()
    expect(box.querySelectorAll('button, input, select').length).toBe(0)
    expect(box.textContent).toContain('never because of its role')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('nothing when no Connect login holds a chat grant, or Sew is unsupported', async () => {
    state.grants = [chat()]
    const { container } = render(<SewAdminAccessState users={[]} />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('skin/grants'))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(container.innerHTML).toBe('')
    cleanup()
    resetSewStatusCacheForTests()
    h.daemonCliGet.mockClear()
    state.sew = { supported: false, state: 'not-installed' }
    state.grants = [{ id: 'g-c1', subjectKind: 'connect', subjectId: 'u', kind: 'chat', targetId: 'sew', scope: 'member', enabled: true }]
    const second = render(<SewAdminAccessState users={[]} />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('sew/status', undefined))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(second.container.innerHTML).toBe('')
    expect(h.daemonCliGet.mock.calls.some((c) => c[0] === 'skin/grants')).toBe(false)
  })
})

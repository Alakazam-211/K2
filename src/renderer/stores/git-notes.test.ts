// @vitest-environment jsdom
// C5 review notes store: one live list per server + workspace, refetched on
// git_notes_changed, on git_changed only while notes are open (outdated /
// moved lines follow the files), and on reconnect; writes POST on the
// given scope and refresh.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  notes: new Set<(e: unknown) => void>(),
  git: new Set<(e: unknown) => void>(),
  hello: new Set<() => void>(),
}))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  const sub = <T>(set: Set<T>) => (_scope: unknown, fn: T) => {
    set.add(fn)
    return () => set.delete(fn)
  }
  return { ...actual, onGitNotesChanged: sub(h.notes), onGitChanged: sub(h.git), onAppHello: sub(h.hello) }
})

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return { ...actual, daemonCliGet: cli.get, daemonCliPost: cli.post }
})

import { fakeScope } from '@/test-utils/fake-scope'
import { __resetGitNotesForTests, resolveNotes, sendNotes, useGitNotes, type NotesList } from './git-notes'

const WS = '/w/s'
let open = 0

function list(): NotesList {
  return { workspacePath: WS, counts: { open, unsent: open, sent: 0, resolved: 0 }, notes: [] }
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 6; i++) await Promise.resolve()
  })
}

const gets = () => cli.get.mock.calls.filter((c) => c[1] === 'git/notes').length

beforeEach(() => {
  __resetGitNotesForTests()
  for (const s of [h.notes, h.git, h.hello]) s.clear()
  open = 0
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async () => list())
  cli.post.mockImplementation(async () => ({ success: true, sent: 1, ids: ['n1'], remaining: 0, deliveryId: 'm', target: WS }))
})

afterEach(() => cleanup())

describe('useGitNotes', () => {
  it('fetches every status on the scope, and shares one list per workspace', async () => {
    const scope = fakeScope('b')
    renderHook(() => useGitNotes(scope, `${WS}/`))
    renderHook(() => useGitNotes(scope, WS))
    await flush()
    expect(cli.get.mock.calls).toEqual([[scope, 'git/notes', { path: WS, status: 'all' }]])
  })

  it('refetches on git_notes_changed for this workspace, git_changed only with open notes, and on reconnect', async () => {
    const { result } = renderHook(() => useGitNotes(fakeScope('b'), WS))
    await flush()
    expect(gets()).toBe(1)
    await act(async () => {
      for (const fn of h.notes) fn({ kind: 'git_notes_changed', workspacePath: '/other' })
      for (const fn of h.git) fn({ kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    })
    await flush()
    expect(gets()).toBe(1)
    open = 2
    await act(async () => {
      for (const fn of h.notes) fn({ kind: 'git_notes_changed', workspacePath: WS })
    })
    await flush()
    expect(gets()).toBe(2)
    expect(result.current.list?.counts.open).toBe(2)
    await act(async () => {
      for (const fn of h.git) fn({ kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r3' })
    })
    await flush()
    expect(gets()).toBe(3)
    await act(async () => {
      for (const fn of h.hello) fn()
    })
    await flush()
    expect(gets()).toBe(4)
  })

  it('writes go to the scope and refresh the mounted list', async () => {
    const scope = fakeScope('b')
    renderHook(() => useGitNotes(scope, WS))
    await flush()
    await act(async () => {
      await resolveNotes(scope, WS, ['n1'])
      await sendNotes(scope, `${WS}/`, ['n1'])
      await sendNotes(scope, WS)
    })
    expect(cli.post.mock.calls).toEqual([
      [scope, 'git/notes/resolve', { ids: ['n1'], resolved: true }],
      [scope, 'git/notes/send', { path: WS, ids: ['n1'] }],
      [scope, 'git/notes/send', { path: WS }],
    ])
    expect(gets()).toBe(4)
  })
})

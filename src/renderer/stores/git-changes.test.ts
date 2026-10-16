// @vitest-environment jsdom
// Changes tab C2 — the live snapshot store: git_changed refresh with rev
// dedupe, workspace filtering, the fs_changed fallback for pre-C1 daemons,
// reconnect refetch, and the write helpers' routes.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type Handler = (e: never) => void
const handlers = vi.hoisted(() => ({
  git: new Map<string, Set<(e: unknown) => void>>(),
  fs: new Map<string, Set<(e: unknown) => void>>(),
  hello: new Map<string, Set<(e: unknown) => void>>(),
}))

vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  const add = (bag: Map<string, Set<(e: unknown) => void>>, scope: { id: string }, fn: (e: unknown) => void) => {
    let set = bag.get(scope.id)
    if (!set) bag.set(scope.id, (set = new Set()))
    set.add(fn)
    return () => set!.delete(fn)
  }
  return {
    ...actual,
    onGitChanged: (scope: { id: string }, fn: Handler) => add(handlers.git, scope, fn as (e: unknown) => void),
    onFsChanged: (scope: { id: string }, fn: Handler) => add(handlers.fs, scope, fn as (e: unknown) => void),
    onAppHello: (scope: { id: string }, fn: Handler) => add(handlers.hello, scope, fn as (e: unknown) => void),
  }
})

const cli = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return { ...actual, daemonCliGet: cli.get, daemonCliPost: cli.post }
})

import { fakeScope } from '@/test-utils/fake-scope'
import {
  FS_FALLBACK_MS,
  __gitChangesLiveCountForTests,
  __resetGitChangesForTests,
  commitStaged,
  diffParamsFor,
  discardFiles,
  stageAllChanges,
  stageFiles,
  unstageFiles,
  useGitChanges,
  type GitSnapshot,
} from '@/stores/git-changes'

const WS = '/repo/ws'
const scopeA = fakeScope('a')
const scopeB = fakeScope('b')

function snap(rev: string, files: Array<{ path: string; group: string }> = []): GitSnapshot {
  return {
    isRepo: true,
    workspacePath: WS,
    repoRoot: WS,
    rev,
    branch: 'main',
    headOid: 'abc',
    detached: false,
    unborn: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    state: 'clean',
    base: null,
    counts: { staged: 0, unstaged: files.length, untracked: 0, conflicted: 0, base: 0, total: files.length },
    truncated: false,
    limit: 1000,
    files: files.map((f) => ({
      path: f.path,
      oldPath: null,
      group: f.group as never,
      status: 'M',
      additions: 1,
      deletions: 0,
      binary: false,
      dir: false,
    })),
  }
}

function emit(bag: 'git' | 'fs' | 'hello', scopeId: string, e: unknown): void {
  const set = handlers[bag].get(scopeId)
  if (!set) throw new Error(`no ${bag} handler on ${scopeId}`)
  for (const fn of [...set]) fn(e)
}

function snapshotCalls(): Array<Record<string, string>> {
  return cli.get.mock.calls.filter((c) => c[1] === 'git/snapshot').map((c) => c[2] as Record<string, string>)
}

/** Settle queued promise callbacks. */
async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

beforeEach(() => {
  __resetGitChangesForTests()
  handlers.git.clear()
  handlers.fs.clear()
  handlers.hello.clear()
  cli.get.mockReset()
  cli.post.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useGitChanges live store', () => {
  it('loads the snapshot once, shared by two consumers of the same workspace', async () => {
    cli.get.mockResolvedValue(snap('r1', [{ path: 'a.txt', group: 'unstaged' }]))
    const one = renderHook(() => useGitChanges(scopeA, WS))
    const two = renderHook(() => useGitChanges(scopeA, `${WS}/`))
    await flush()
    expect(snapshotCalls()).toEqual([{ path: WS }])
    expect(one.result.current.snapshot?.rev).toBe('r1')
    expect(two.result.current.snapshot?.rev).toBe('r1')
    expect(__gitChangesLiveCountForTests()).toBe(1)
    one.unmount()
    expect(__gitChangesLiveCountForTests()).toBe(1)
    two.unmount()
    expect(__gitChangesLiveCountForTests()).toBe(0)
    expect(handlers.git.get(scopeA.id)?.size ?? 0).toBe(0)
  })

  it('refetches on git_changed with a new rev, and drops a repeat of the rev it holds', async () => {
    cli.get.mockResolvedValueOnce(snap('r1')).mockResolvedValueOnce(snap('r2', [{ path: 'b.txt', group: 'unstaged' }]))
    const { result } = renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    expect(snapshotCalls()).toHaveLength(1)

    // Same rev as held: nothing.
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r1' })
    await flush()
    expect(snapshotCalls()).toHaveLength(1)

    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)
    expect(result.current.snapshot?.rev).toBe('r2')
    expect(result.current.snapshot && result.current.snapshot.isRepo && result.current.snapshot.files[0].path).toBe('b.txt')

    // The watcher's second frame for the same burst carries the rev we now hold.
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)
  })

  it('a burst while a fetch is in flight collapses into one follow-up fetch', async () => {
    let release!: (v: GitSnapshot) => void
    cli.get
      .mockResolvedValueOnce(snap('r1'))
      .mockImplementationOnce(() => new Promise((res) => (release = res)))
      .mockResolvedValueOnce(snap('r4'))
    const { result } = renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    // Duplicate of the rev being fetched: dropped.
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r3' })
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r4' })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)
    release(snap('r2'))
    await flush()
    expect(snapshotCalls()).toHaveLength(3)
    expect(result.current.snapshot?.rev).toBe('r4')
  })

  it('ignores git_changed for another workspace and from another server', async () => {
    cli.get.mockResolvedValue(snap('r1'))
    renderHook(() => useGitChanges(scopeA, WS))
    renderHook(() => useGitChanges(scopeB, '/other'))
    await flush()
    const before = snapshotCalls().length
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: '/repo/elsewhere', repoRoot: '/repo', rev: 'x' })
    // Same path, but scope B's bus: scope A's store does not hear it.
    emit('git', scopeB.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'x' })
    await flush()
    expect(snapshotCalls()).toHaveLength(before)
  })

  it('fs_changed refetches after a pause until the server has sent git_changed, then never', async () => {
    vi.useFakeTimers()
    cli.get.mockResolvedValue(snap('r1'))
    renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    expect(snapshotCalls()).toHaveLength(1)

    emit('fs', scopeA.id, { kind: 'fs_changed', workspacePath: '/elsewhere', paths: [`${WS}/a.txt`] })
    await act(async () => {
      vi.advanceTimersByTime(FS_FALLBACK_MS + 10)
    })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)

    // A git_changed (C1 daemon) arrives: the fs fallback is off from now on.
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r1' })
    emit('fs', scopeA.id, { kind: 'fs_changed', workspacePath: WS, paths: [`${WS}/b.txt`] })
    await act(async () => {
      vi.advanceTimersByTime(FS_FALLBACK_MS * 3)
    })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)
  })

  it('a socket reconnect (hello) refetches even at the same rev', async () => {
    cli.get.mockResolvedValue(snap('r1'))
    renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    emit('hello', scopeA.id, { kind: 'hello' })
    await flush()
    expect(snapshotCalls()).toHaveLength(2)
  })

  it('a base keeps its own store and sends base=', async () => {
    cli.get.mockResolvedValue(snap('r1'))
    renderHook(() => useGitChanges(scopeA, WS, 'main'))
    await flush()
    expect(snapshotCalls()).toEqual([{ path: WS, base: 'main' }])
  })

  it('a failed fetch keeps the last snapshot and reports the error', async () => {
    cli.get.mockResolvedValueOnce(snap('r1')).mockRejectedValueOnce(new Error('boom'))
    const { result } = renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    emit('git', scopeA.id, { kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    await flush()
    expect(result.current.error).toBe('boom')
    expect(result.current.snapshot?.rev).toBe('r1')
  })
})

describe('write helpers', () => {
  it('stage, unstage, stage-all, discard and commit post the contract bodies and refresh', async () => {
    cli.get.mockResolvedValue(snap('r1'))
    cli.post.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'git/discard') {
        return { success: true, restored: [], trashed: ['n.txt'], unstaged: [], skipped: [], failed: [], rev: 'r2' }
      }
      if (route === 'git/commit') return { oid: 'o1', message: 'm' }
      return { success: true }
    })
    renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    const base = snapshotCalls().length

    await act(async () => {
      await stageFiles(scopeA, `${WS}/`, ['a.txt', 'b.txt'])
      await unstageFiles(scopeA, WS, ['a.txt'])
      await stageAllChanges(scopeA, WS)
      await discardFiles(scopeA, WS, ['n.txt'])
      await commitStaged(scopeA, WS, 'msg')
    })
    expect(cli.post.mock.calls.map((c) => [c[1], c[2]])).toEqual([
      ['git/stage', { path: WS, filePath: 'a.txt' }],
      ['git/stage', { path: WS, filePath: 'b.txt' }],
      ['git/unstage', { path: WS, filePath: 'a.txt' }],
      ['git/stage-all', { path: WS }],
      ['git/discard', { path: WS, files: ['n.txt'], includeStaged: false }],
      ['git/commit', { path: WS, message: 'msg' }],
    ])
    for (const c of cli.post.mock.calls) expect(c[0]).toBe(scopeA)
    // One refresh per write.
    expect(snapshotCalls().length - base).toBe(5)
  })

  it('a failed write still refreshes and rethrows', async () => {
    cli.get.mockResolvedValue(snap('r1'))
    cli.post.mockRejectedValue(new Error('denied'))
    renderHook(() => useGitChanges(scopeA, WS))
    await flush()
    const base = snapshotCalls().length
    await expect(stageFiles(scopeA, WS, ['a.txt'])).rejects.toThrow('denied')
    expect(snapshotCalls().length - base).toBe(1)
  })
})

describe('diffParamsFor', () => {
  it('maps a row group to the diff-file mode', () => {
    expect(diffParamsFor({ group: 'staged', path: 'a', oldPath: null }, null)).toEqual({ file_path: 'a', sides: '1', mode: 'staged' })
    expect(diffParamsFor({ group: 'unstaged', path: 'a', oldPath: null }, null)).toEqual({ file_path: 'a', sides: '1', mode: 'unstaged' })
    expect(diffParamsFor({ group: 'untracked', path: 'n', oldPath: null }, null)).toEqual({ file_path: 'n', sides: '1', mode: 'worktree' })
    expect(diffParamsFor({ group: 'base', path: 'b', oldPath: 'old' }, 'main')).toEqual({
      file_path: 'b',
      sides: '1',
      old_path: 'old',
      base: 'main',
    })
  })
})

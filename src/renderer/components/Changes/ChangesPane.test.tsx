// @vitest-environment jsdom
// Changes tab C2 — the pane: grouping with counts, row and group actions on
// the right routes (room scope), the discard confirm, commit, the diff slot
// props, live refresh, and a view-only room.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const handlers = vi.hoisted(() => ({ git: new Set<(e: unknown) => void>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onGitChanged: (_scope: unknown, fn: (e: unknown) => void) => {
      handlers.git.add(fn)
      return () => handlers.git.delete(fn)
    },
    onFsChanged: () => () => {},
    onAppHello: () => () => {},
  }
})

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return { ...actual, daemonCliGet: cli.get, daemonCliPost: cli.post }
})

// The diff viewer is C3's; capture what C2 hands the slot.
const slotProps = vi.hoisted(() => ({ last: null as null | Record<string, unknown> }))
vi.mock('./ChangesDiffSlot', () => ({
  ChangesDiffSlot: (props: Record<string, unknown>) => {
    slotProps.last = props
    return <div data-testid="slot" />
  },
}))

import ConfirmDialog from '@/components/ConfirmDialog/ConfirmDialog'
import { ChangesPane } from './ChangesPane'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { viewOnlyScope, type ServerScope } from '@/kessel/server-scope'
import { __resetGitChangesForTests, type ChangesFile, type GitSnapshot } from '@/stores/git-changes'
import { useTabsStore, type ChangesItemData } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'

const WS = '/repo/ws'

function row(path: string, group: ChangesFile['group'], status: ChangesFile['status'] = 'M', extra: Partial<ChangesFile> = {}): ChangesFile {
  return { path, oldPath: null, group, status, additions: 3, deletions: 1, binary: false, dir: false, ...extra }
}

const FILES: ChangesFile[] = [
  row('src/s.rs', 'staged', 'A'),
  row('src/a.rs', 'unstaged'),
  row('src/deep/b.rs', 'unstaged', 'D', { additions: 0, deletions: 9 }),
  row('new.txt', 'untracked', 'U', { additions: 4, deletions: 0 }),
]

function snap(rev: string, files: ChangesFile[] = FILES, over: Partial<GitSnapshot> = {}): GitSnapshot {
  const count = (g: string) => files.filter((f) => f.group === g).length
  return {
    isRepo: true,
    workspacePath: WS,
    repoRoot: WS,
    rev,
    branch: 'feat/x',
    headOid: 'abc1234',
    detached: false,
    unborn: false,
    upstream: 'origin/feat/x',
    ahead: 2,
    behind: 0,
    state: 'clean',
    base: null,
    counts: {
      staged: count('staged'),
      unstaged: count('unstaged'),
      untracked: count('untracked'),
      conflicted: count('conflicted'),
      base: count('base'),
      total: files.length,
    },
    truncated: false,
    limit: 1000,
    files,
    ...over,
  } as GitSnapshot
}

let snapshots: GitSnapshot[] = []

function sidesFor(params: Record<string, string>) {
  return {
    path: params.file_path,
    oldPath: null,
    mode: params.mode ?? 'base',
    baseRef: params.base ?? null,
    rev: 'r1',
    old: { label: 'HEAD', exists: true, text: 'a\n', size: 2, binary: false, tooLarge: false },
    new: { label: 'WORKTREE', exists: true, text: 'b\n', size: 2, binary: false, tooLarge: false },
    binary: false,
    tooLarge: false,
    additions: 1,
    deletions: 1,
  }
}

function openPane(scope: ServerScope, data?: Partial<ChangesItemData>) {
  useTabsStore.getState().openChangesTab(WS)
  const tab = useTabsStore.getState().tabs[0]
  const pg = Array.from(tab.paneGroups.values())[0]
  const itemId = pg.items[0].id
  if (data) useTabsStore.getState().setChangesItemState(tab.id, pg.id, itemId, data)
  const room = testRoom({ tabs: useTabsStore, scope, projects: [] })
  function Live(): React.JSX.Element {
    const item = useTabsStore((s) => s.tabs[0]?.paneGroups.get(pg.id)?.items[0])
    if (!item) throw new Error('changes item vanished')
    return <ChangesPane tabId={tab.id} paneGroupId={pg.id} itemId={itemId} data={item.data as ChangesItemData} />
  }
  const view = renderInRoom(
    room,
    <>
      <Live />
      <ConfirmDialog />
    </>,
  )
  return { view, tab, pg, itemId }
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 8; i++) await Promise.resolve()
  })
}

function group(name: string): HTMLElement {
  const el = document.querySelector(`[data-changes-group="${name}"]`) as HTMLElement | null
  if (!el) throw new Error(`no ${name} group`)
  return el
}

function rowEl(groupName: string, path: string): HTMLElement {
  const el = document.querySelector(`[data-changes-row="${groupName}:${path}"]`) as HTMLElement | null
  if (!el) throw new Error(`no row ${groupName} ${path}`)
  return el
}

function posts(): Array<[string, unknown]> {
  return cli.post.mock.calls.map((c) => [c[1] as string, c[2]])
}

beforeEach(() => {
  __resetGitChangesForTests()
  handlers.git.clear()
  slotProps.last = null
  snapshots = [snap('r1')]
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
  useToastStore.setState({ toasts: [] })
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async (_scope: unknown, route: string, params: Record<string, string>) => {
    if (route === 'git/snapshot') {
      const next = snapshots.length > 1 ? snapshots.shift()! : snapshots[0]
      return next
    }
    if (route === 'git/diff-file') return sidesFor(params)
    if (route === 'git/branches') return { current: 'feat/x', local: ['main', 'feat/x'], remote: ['origin/main'] }
    throw new Error(`unexpected GET ${route}`)
  })
  cli.post.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'git/discard') {
      return { success: true, restored: ['src/a.rs'], trashed: ['src/a.rs'], unstaged: [], skipped: [], failed: [], rev: 'r2' }
    }
    if (route === 'git/commit') return { oid: 'o1', message: 'm' }
    return { success: true }
  })
})

afterEach(() => {
  cleanup()
  useConfirmDialogStore.getState().close()
})

describe('ChangesPane', () => {
  it('groups rows with counts, badges and +/- in snapshot order', async () => {
    const scope = fakeScope('b')
    openPane(scope)
    await flush()
    const groups = Array.from(document.querySelectorAll('[data-changes-group]')).map((g) => g.getAttribute('data-changes-group'))
    expect(groups).toEqual(['staged', 'unstaged', 'untracked'])
    const countOf = (g: string) => group(g).querySelector('[data-changes-group-count]')?.textContent
    expect(countOf('staged')).toBe('1')
    expect(countOf('unstaged')).toBe('2')
    expect(countOf('untracked')).toBe('1')
    expect(rowEl('unstaged', 'src/deep/b.rs').querySelector('[data-changes-status]')?.textContent).toBe('D')
    expect(rowEl('unstaged', 'src/deep/b.rs').querySelector('[data-changes-lines]')?.textContent).toBe('−9')
    expect(rowEl('staged', 'src/s.rs').querySelector('[data-changes-lines]')?.textContent).toBe('+3 −1')
    expect(document.querySelector('[data-changes-branch]')?.textContent).toBe('feat/x')
    // Every read went to the room's scope.
    for (const c of cli.get.mock.calls) expect(c[0]).toBe(scope)
  })

  it('the first row shows in the slot without saving; a click saves the selection and loads its sides', async () => {
    const scope = fakeScope('b')
    openPane(scope)
    await flush()
    expect((slotProps.last?.file as ChangesFile).path).toBe('src/s.rs')
    expect(useTabsStore.getState().tabs[0].paneGroups.values().next().value!.items[0].data).toEqual({ workspacePath: WS })
    expect(cli.get.mock.calls.find((c) => c[1] === 'git/diff-file')?.[2]).toEqual({
      path: WS,
      file_path: 'src/s.rs',
      sides: '1',
      mode: 'staged',
    })

    await act(async () => {
      fireEvent.click(rowEl('unstaged', 'src/a.rs'))
    })
    await flush()
    const data = useTabsStore.getState().tabs[0].paneGroups.values().next().value!.items[0].data as ChangesItemData
    expect(data).toMatchObject({ selectedPath: 'src/a.rs', selectedGroup: 'unstaged' })
    expect((slotProps.last?.file as ChangesFile).path).toBe('src/a.rs')
    expect(slotProps.last?.sides).toMatchObject({ status: 'ready', data: { path: 'src/a.rs', mode: 'unstaged' } })
    expect(slotProps.last?.mode).toBe('inline')
    expect(slotProps.last?.view).toBe('single')
  })

  it('row and group actions post stage / unstage / stage-all on the room scope', async () => {
    const scope = fakeScope('b')
    openPane(scope)
    await flush()
    await act(async () => {
      fireEvent.click(within(rowEl('unstaged', 'src/a.rs')).getByLabelText('Stage src/a.rs'))
    })
    await act(async () => {
      fireEvent.click(within(rowEl('staged', 'src/s.rs')).getByLabelText('Unstage src/s.rs'))
    })
    await act(async () => {
      fireEvent.click(within(group('unstaged')).getByText('Stage All'))
    })
    await act(async () => {
      fireEvent.click(screen.getByText('Stage all'))
    })
    await flush()
    expect(posts()).toEqual([
      ['git/stage', { path: WS, filePath: 'src/a.rs' }],
      ['git/unstage', { path: WS, filePath: 'src/s.rs' }],
      ['git/stage', { path: WS, filePath: 'src/a.rs' }],
      ['git/stage', { path: WS, filePath: 'src/deep/b.rs' }],
      ['git/stage-all', { path: WS }],
    ])
    for (const c of cli.post.mock.calls) expect(c[0]).toBe(scope)
  })

  it('discard asks first: cancel sends nothing, confirm posts git/discard', async () => {
    openPane(fakeScope('b'))
    await flush()
    await act(async () => {
      fireEvent.click(within(rowEl('unstaged', 'src/a.rs')).getByLabelText('Discard changes to src/a.rs'))
    })
    expect(useConfirmDialogStore.getState().isOpen).toBe(true)
    expect(useConfirmDialogStore.getState().title).toBe('Discard changes?')
    expect(useConfirmDialogStore.getState().message).toContain('Trash')
    expect(useConfirmDialogStore.getState().confirmDestructive).toBe(true)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    })
    await flush()
    expect(posts()).toEqual([])

    await act(async () => {
      fireEvent.click(within(rowEl('unstaged', 'src/a.rs')).getByLabelText('Discard changes to src/a.rs'))
    })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    })
    await flush()
    expect(posts()).toEqual([['git/discard', { path: WS, files: ['src/a.rs'], includeStaged: false }]])
  })

  it('untracked discard says Trash, and a failed file is reported', async () => {
    cli.post.mockImplementation(async () => ({
      success: true,
      restored: [],
      trashed: [],
      unstaged: [],
      skipped: [],
      failed: [{ path: 'new.txt', error: 'trash unavailable' }],
      rev: 'r2',
    }))
    openPane(fakeScope('b'))
    await flush()
    await act(async () => {
      fireEvent.click(within(group('untracked')).getByText('Trash All'))
    })
    expect(useConfirmDialogStore.getState().title).toBe('Move to Trash?')
    expect(useConfirmDialogStore.getState().confirmLabel).toBe('Move to Trash')
    expect(useConfirmDialogStore.getState().message).toContain('new.txt')
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Move to Trash' }))
    })
    await flush()
    expect(posts()).toEqual([['git/discard', { path: WS, files: ['new.txt'], includeStaged: false }]])
    const toasts = useToastStore.getState().toasts.map((t) => t.message)
    expect(toasts.some((m) => m.includes('new.txt') && m.includes('trash unavailable'))).toBe(true)
  })

  it('commit with Cmd+Enter posts git/commit and clears the box; disabled with nothing staged', async () => {
    openPane(fakeScope('b'))
    await flush()
    const box = document.querySelector('[data-changes-commit-message]') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: '  fix: thing  ' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    })
    await flush()
    expect(posts()).toEqual([['git/commit', { path: WS, message: 'fix: thing' }]])
    expect(box.value).toBe('')

    cleanup()
    __resetGitChangesForTests()
    useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
    snapshots = [snap('r9', FILES.filter((f) => f.group !== 'staged'))]
    cli.post.mockClear()
    openPane(fakeScope('b'))
    await flush()
    const btn = document.querySelector('[data-changes-commit]') as HTMLButtonElement
    const box2 = document.querySelector('[data-changes-commit-message]') as HTMLTextAreaElement
    fireEvent.change(box2, { target: { value: 'msg' } })
    expect(btn.disabled).toBe(true)
    await act(async () => {
      fireEvent.keyDown(box2, { key: 'Enter', ctrlKey: true })
    })
    expect(posts()).toEqual([])
  })

  it('git_changed with a new rev refreshes the list and the open diff', async () => {
    snapshots = [snap('r1'), snap('r2', [row('src/a.rs', 'staged')])]
    openPane(fakeScope('b'), { selectedPath: 'src/a.rs', selectedGroup: 'unstaged' })
    await flush()
    const diffCalls = () => cli.get.mock.calls.filter((c) => c[1] === 'git/diff-file').length
    const before = diffCalls()
    await act(async () => {
      for (const fn of handlers.git) fn({ kind: 'git_changed', workspacePath: WS, repoRoot: WS, rev: 'r2' })
    })
    await flush()
    // The file moved to staged: selection follows the path.
    expect(document.querySelector('[data-changes-group="unstaged"]')).toBeNull()
    expect(rowEl('staged', 'src/a.rs').hasAttribute('data-selected')).toBe(true)
    expect(diffCalls()).toBe(before + 1)
    expect(slotProps.last?.rev).toBe('r2')
  })

  it('shows the truncated banner and a not-a-repo state', async () => {
    snapshots = [snap('r1', FILES, { truncated: true, limit: 1000, counts: { staged: 1, unstaged: 2, untracked: 1, conflicted: 0, base: 0, total: 4321 } })]
    openPane(fakeScope('b'))
    await flush()
    expect(document.querySelector('[data-changes-truncated]')?.textContent).toContain('4,321')

    cleanup()
    __resetGitChangesForTests()
    useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
    snapshots = [{ isRepo: false, workspacePath: WS, rev: 'r0' }]
    openPane(fakeScope('b'))
    await flush()
    expect(screen.getByText('Not a git repository')).toBeTruthy()
  })

  it('a base pick saves base, refetches with base=, and lists branches from the room server', async () => {
    const scope = fakeScope('b')
    openPane(scope)
    await flush()
    const select = document.querySelector('[data-changes-base]') as HTMLSelectElement
    await act(async () => {
      fireEvent.focus(select)
    })
    await flush()
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['', 'main', 'origin/main'])
    await act(async () => {
      fireEvent.change(select, { target: { value: 'main' } })
    })
    await flush()
    const data = useTabsStore.getState().tabs[0].paneGroups.values().next().value!.items[0].data as ChangesItemData
    expect(data.base).toBe('main')
    expect(cli.get.mock.calls.some((c) => c[1] === 'git/snapshot' && (c[2] as Record<string, string>).base === 'main')).toBe(true)
  })

  it('a view-only room shows the list but no write actions', async () => {
    openPane(viewOnlyScope(fakeScope('b')))
    await flush()
    expect(rowEl('unstaged', 'src/a.rs')).toBeTruthy()
    expect(screen.queryByLabelText('Stage src/a.rs')).toBeNull()
    expect(screen.queryByLabelText('Discard changes to src/a.rs')).toBeNull()
    expect(screen.queryByText('Stage all')).toBeNull()
    expect(document.querySelector('[data-changes-commit-message]')).toBeNull()
    expect(screen.getByText(/View only/)).toBeTruthy()
  })
})

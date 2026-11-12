// The Board tab's switcher reads the Board Garden's list, cut to the open board's workspace.
import { describe, expect, it } from 'vitest'
import { createBoardFileListing, workspaceBoardsFor, workspaceOfBoard, type BoardFileRow } from './board-file-list'

const r = (path: string, workspace: string, name = path.split('/').pop()!.replace('.k5board', '')): BoardFileRow => ({
  path,
  rel: path.slice(workspace.length + 1),
  name,
  workspace,
  workspaceName: workspace.split('/').pop()!,
})

describe('workspaceOfBoard', () => {
  it('is the longest workspace root above the board (a workspace inside another)', () => {
    const rows = [r('/a/x.k5board', '/a'), r('/a/inner/y.k5board', '/a/inner')]
    expect(workspaceOfBoard(rows, '/a/inner/z.k5board')).toBe('/a/inner')
    expect(workspaceOfBoard(rows, '/a/q.k5board')).toBe('/a')
    expect(workspaceOfBoard(rows, '/ab/q.k5board')).toBeNull()
  })
})

describe('workspaceBoardsFor', () => {
  it('pushes only that workspace’s boards, by name, with their paths', async () => {
    const listing = createBoardFileListing({
      load: async () => ({
        boards: [r('/a/Zed.k5board', '/a'), r('/a/d/Alpha.k5board', '/a'), r('/b/Other.k5board', '/b')],
        truncated: false,
      }),
      onBoardFileChanged: () => () => undefined,
      onFsChanged: () => () => undefined,
      setTimer: () => 0,
      clearTimer: () => undefined,
    })
    const got = await new Promise<unknown>((resolve, reject) => workspaceBoardsFor(listing, '/a/Zed.k5board').subscribe(resolve, reject))
    expect(got).toEqual([
      { path: '/a/d/Alpha.k5board', name: 'Alpha', rel: 'd/Alpha.k5board' },
      { path: '/a/Zed.k5board', name: 'Zed', rel: 'Zed.k5board' },
    ])
  })
})

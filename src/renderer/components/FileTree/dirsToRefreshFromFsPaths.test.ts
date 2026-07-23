import { describe, expect, it } from 'vitest'
import { dirsToRefreshFromFsPaths, isNoisyFsPath } from './FileTree'

describe('isNoisyFsPath', () => {
  it('filters agent/VCS/build churn and keeps source', () => {
    expect(isNoisyFsPath('/proj/.k2/AGENTS.md')).toBe(true)
    expect(isNoisyFsPath('/proj/.git/HEAD')).toBe(true)
    expect(isNoisyFsPath('/proj/node_modules/x/index.js')).toBe(true)
    expect(isNoisyFsPath('/proj/src/main.rs')).toBe(false)
    expect(isNoisyFsPath('/proj/README.md')).toBe(false)
  })
})

describe('dirsToRefreshFromFsPaths', () => {
  const root = '/Users/me/proj'

  it('refreshes root parent for a file under root when root is expanded', () => {
    const dirs = dirsToRefreshFromFsPaths(
      [`${root}/README.md`],
      root,
      [root],
      [root],
    )
    expect(dirs).toContain(root)
  })

  it('refreshes a cached subdirectory parent', () => {
    const src = `${root}/src`
    const dirs = dirsToRefreshFromFsPaths(
      [`${src}/main.rs`],
      root,
      [root, src],
      [root, src],
    )
    expect(dirs).toContain(src)
  })

  it('ignores paths outside the tree root (sibling prefix-safe)', () => {
    const dirs = dirsToRefreshFromFsPaths(
      ['/Users/me/proj-website/index.html', '/other/x'],
      root,
      [root],
      [root],
    )
    expect(dirs).toEqual([])
  })

  it('refreshes a previously-cached directory when the path itself is the dir', () => {
    const src = `${root}/src`
    const dirs = dirsToRefreshFromFsPaths(
      [src],
      root,
      [root, src],
      [root, src],
    )
    // parent (root) + the dir itself
    expect(dirs).toEqual(expect.arrayContaining([root, src]))
  })

  it('skips uncached unexpanded parents that are not the root', () => {
    const deep = `${root}/a/b/c.txt`
    const dirs = dirsToRefreshFromFsPaths(
      [deep],
      root,
      [root], // only root cached; a/b not expanded
      [root],
    )
    // parent is /Users/me/proj/a/b — not cached, not expanded, not root → skip
    expect(dirs).toEqual([])
  })
})

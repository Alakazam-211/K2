import { describe, expect, it } from 'vitest'
import { classifyDiff, formatBytes, rowTitle, sidesMatchRow, viewFromSides } from './diffQuery'
import { diffRowKey, expectedDiffMode, type ChangesDiffFile, type DiffSide, type DiffSidesResponse } from './types'

function row(p: Partial<ChangesDiffFile> = {}): ChangesDiffFile {
  return { path: 'src/a.rs', oldPath: null, group: 'unstaged', status: 'M', binary: false, dir: false, additions: 1, deletions: 1, ...p }
}

function side(p: Partial<DiffSide> = {}): DiffSide {
  return { label: 'HEAD', exists: true, text: 'a\n', size: 2, binary: false, tooLarge: false, ...p }
}

function res(p: Partial<DiffSidesResponse> = {}): DiffSidesResponse {
  return {
    path: 'src/a.rs',
    oldPath: null,
    mode: 'worktree',
    baseRef: null,
    rev: 'r1',
    old: side({ label: 'HEAD', text: 'a\n' }),
    new: side({ label: 'WORKTREE', text: 'b\n' }),
    binary: false,
    tooLarge: false,
    additions: 1,
    deletions: 1,
    ...p,
  }
}

describe('row identity and expected mode', () => {
  it('matches the daemon mode for each group (C2 diffParamsFor rules)', () => {
    expect(expectedDiffMode({ group: 'staged' }, null)).toBe('staged')
    expect(expectedDiffMode({ group: 'unstaged' }, null)).toBe('unstaged')
    expect(expectedDiffMode({ group: 'untracked' }, null)).toBe('worktree')
    expect(expectedDiffMode({ group: 'conflicted' }, 'main')).toBe('worktree')
    expect(expectedDiffMode({ group: 'base' }, 'main')).toBe('base')
    expect(expectedDiffMode({ group: 'base' }, null)).toBe('worktree')
  })

  it('keys a row by group and path (a file staged and unstaged is two rows)', () => {
    expect(diffRowKey(row({ group: 'staged' }))).not.toBe(diffRowKey(row({ group: 'unstaged' })))
    expect(diffRowKey(row({ group: 'staged' }))).toBe('staged:src/a.rs')
  })

  it('an answer matches only its own row (path and mode)', () => {
    expect(sidesMatchRow(res({ mode: 'unstaged' }), row({ group: 'unstaged' }), null)).toBe(true)
    expect(sidesMatchRow(res({ mode: 'unstaged' }), row({ group: 'staged' }), null)).toBe(false)
    expect(sidesMatchRow(res({ mode: 'unstaged', path: 'other.rs' }), row({ group: 'unstaged' }), null)).toBe(false)
  })
})

describe('viewFromSides (single view, C2 holds the fetch)', () => {
  const r = row({ group: 'unstaged' })
  const mine = res({ mode: 'unstaged' })

  it('idle and a first load show the loading card', () => {
    expect(viewFromSides(r, { status: 'idle' }, null)).toMatchObject({ loading: true, content: null })
    expect(viewFromSides(r, { status: 'loading' }, null)).toMatchObject({ loading: true, content: null })
  })

  it('a refresh keeps painting the old text', () => {
    const v = viewFromSides(r, { status: 'loading', data: mine }, null)
    expect(v).toMatchObject({ loading: false, refreshing: true, content: { kind: 'text', newText: 'b\n' } })
  })

  it('ready shows the content; another row\'s data is not painted', () => {
    expect(viewFromSides(r, { status: 'ready', data: mine }, null).content).toMatchObject({ kind: 'text' })
    expect(viewFromSides(row({ group: 'staged' }), { status: 'ready', data: mine }, null)).toMatchObject({ loading: true, content: null })
    expect(viewFromSides(r, { status: 'loading', data: res({ mode: 'unstaged', path: 'x' }) }, null)).toMatchObject({ loading: true })
  })

  it('an error wins over stale data', () => {
    expect(viewFromSides(r, { status: 'error', error: 'bad_ref', data: mine }, null)).toEqual({
      loading: false,
      refreshing: false,
      content: null,
      error: 'bad_ref',
    })
  })

  it('a contract break surfaces as an error, not a crash', () => {
    const v = viewFromSides(r, { status: 'ready', data: res({ mode: 'unstaged', new: side({ text: null }) }) }, null)
    expect(v.error).toMatch(/text side with no text/)
  })

  it('a folder row never waits for a fetch', () => {
    expect(viewFromSides(row({ dir: true, path: 'new/', group: 'untracked' }), { status: 'idle' }, null).content).toEqual({ kind: 'dir' })
  })
})

describe('classifyDiff', () => {
  it('returns both texts for a modified text file', () => {
    expect(classifyDiff(row(), res())).toEqual({
      kind: 'text',
      change: 'modified',
      oldText: 'a\n',
      newText: 'b\n',
      oldLabel: 'HEAD',
      newLabel: 'WORKTREE',
      additions: 1,
      deletions: 1,
    })
  })

  it('treats a missing old side as an added file with an empty old text', () => {
    const c = classifyDiff(row({ group: 'untracked', status: 'U' }), res({ old: side({ exists: false, text: '', size: 0 }) }))
    expect(c).toMatchObject({ kind: 'text', change: 'added', oldText: '', newText: 'b\n' })
  })

  it('treats a missing new side as a deleted file', () => {
    const c = classifyDiff(row({ status: 'D' }), res({ new: side({ exists: false, text: '', size: 0, label: 'WORKTREE' }) }))
    expect(c).toMatchObject({ kind: 'text', change: 'deleted', oldText: 'a\n', newText: '' })
  })

  it('marks a rename', () => {
    const c = classifyDiff(row({ status: 'R', oldPath: 'src/old.rs' }), res({ oldPath: 'src/old.rs' }))
    expect(c).toMatchObject({ kind: 'text', change: 'renamed' })
  })

  it('returns a binary card (binary wins over too large)', () => {
    const c = classifyDiff(
      row(),
      res({
        binary: true,
        tooLarge: true,
        old: side({ text: null, binary: true, size: 2048 }),
        new: side({ text: null, binary: true, tooLarge: true, size: 20 * 1024 * 1024 }),
        additions: null,
        deletions: null,
      }),
    )
    expect(c).toEqual({ kind: 'binary', change: 'modified', oldSize: 2048, newSize: 20 * 1024 * 1024 })
  })

  it('returns a too-large card when either side is over the cap', () => {
    const c = classifyDiff(row(), res({ tooLarge: true, new: side({ text: null, tooLarge: true, size: 11_000_000 }), additions: null, deletions: null }))
    expect(c).toMatchObject({ kind: 'tooLarge', newSize: 11_000_000 })
  })

  it('says "identical" for a mode-only change', () => {
    expect(classifyDiff(row({ status: 'T' }), res({ old: side({ text: 'x' }), new: side({ text: 'x' }) }))).toEqual({
      kind: 'identical',
      change: 'modified',
    })
  })

  it('an added empty file is still a text diff, not "identical"', () => {
    const c = classifyDiff(row({ status: 'A' }), res({ old: side({ exists: false, text: '', size: 0 }), new: side({ text: '', size: 0 }) }))
    expect(c).toMatchObject({ kind: 'text', change: 'added' })
  })

  it('a dir row is a folder card whatever the answer', () => {
    expect(classifyDiff(row({ dir: true }), res())).toEqual({ kind: 'dir' })
  })

  it('throws when a text side has no text (contract break)', () => {
    expect(() => classifyDiff(row(), res({ new: side({ text: null }) }))).toThrow(/text side with no text/)
  })
})

describe('formatting', () => {
  it('formats sizes', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(10 * 1024 * 1024)).toBe('10 MB')
  })

  it('shows old → new for a rename', () => {
    expect(rowTitle({ path: 'b.ts', oldPath: 'a.ts' })).toBe('a.ts → b.ts')
    expect(rowTitle({ path: 'b.ts', oldPath: null })).toBe('b.ts')
  })
})
